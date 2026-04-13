//! Multi-provider stream orchestration and deduplication.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::primitives::B256;
use pyralis_core::error::{PyralisError, Result};
use pyralis_core::traits::ChainDataProvider;
use pyralis_core::types::BlockInfo;
use tokio::sync::{broadcast, mpsc, watch, RwLock};
use tokio::task::JoinHandle;
use tokio::time::sleep;

use crate::reconnect::ReconnectionHandler;

const UNIFIED_BLOCK_CHANNEL_SIZE: usize = 2048;
const PROVIDER_EVENT_CHANNEL_SIZE: usize = 1024;

struct ProviderBlockEvent {
    provider_name: String,
    block: BlockInfo,
}

/// Provider registration metadata used by [`StreamManager`].
#[derive(Debug)]
pub struct ProviderRegistration<P> {
    /// Human-readable provider name used in metrics and logs.
    pub name: String,
    /// Provider instance handling subscriptions.
    pub provider: Arc<P>,
}

impl<P> ProviderRegistration<P> {
    /// Creates a provider registration entry.
    pub fn new(name: impl Into<String>, provider: Arc<P>) -> Self {
        Self {
            name: name.into(),
            provider,
        }
    }
}

/// Deduplicates blocks by hash across multiple providers.
#[derive(Debug, Default)]
pub(crate) struct BlockDeduplicator {
    seen_hashes: HashSet<B256>,
}

impl BlockDeduplicator {
    /// Returns `true` if this hash has not been processed before.
    pub(crate) fn is_new_block(&mut self, block_hash: B256) -> bool {
        self.seen_hashes.insert(block_hash)
    }
}

/// Manages multiple provider streams and emits a unified block feed.
pub struct StreamManager<P> {
    providers: Vec<ProviderRegistration<P>>,
    block_sender: broadcast::Sender<BlockInfo>,
    provider_latencies: Arc<RwLock<HashMap<String, Duration>>>,
    reconnection_handler: Arc<ReconnectionHandler>,
    shutdown_tx: watch::Sender<bool>,
    worker_handles: Vec<JoinHandle<()>>,
    merge_handle: Option<JoinHandle<()>>,
}

impl<P> StreamManager<P>
where
    P: ChainDataProvider + Send + Sync + 'static,
{
    /// Creates a stream manager for the given providers.
    pub fn new(providers: Vec<ProviderRegistration<P>>) -> Self {
        let (block_sender, _) = broadcast::channel(UNIFIED_BLOCK_CHANNEL_SIZE);
        let (shutdown_tx, _) = watch::channel(false);
        let provider_names = providers
            .iter()
            .map(|registration| registration.name.clone());
        let reconnection_handler = Arc::new(ReconnectionHandler::new_noop(provider_names));

        Self {
            providers,
            block_sender,
            provider_latencies: Arc::new(RwLock::new(HashMap::new())),
            reconnection_handler,
            shutdown_tx,
            worker_handles: Vec::new(),
            merge_handle: None,
        }
    }

    /// Replaces the default reconnection handler.
    pub fn set_reconnection_handler(&mut self, reconnection_handler: Arc<ReconnectionHandler>) {
        self.reconnection_handler = reconnection_handler;
    }

    /// Starts provider workers and the merge loop.
    pub async fn start(&mut self) -> Result<()> {
        if self.providers.is_empty() {
            return Err(PyralisError::Provider(
                "no providers configured for stream manager".to_string(),
            ));
        }

        let (event_tx, mut event_rx) =
            mpsc::channel::<ProviderBlockEvent>(PROVIDER_EVENT_CHANNEL_SIZE);
        let unified_sender = self.block_sender.clone();
        let latency_store = Arc::clone(&self.provider_latencies);
        let mut merge_shutdown_rx = self.shutdown_tx.subscribe();

        self.merge_handle = Some(tokio::spawn(async move {
            let mut deduplicator = BlockDeduplicator::default();

            loop {
                tokio::select! {
                    changed = merge_shutdown_rx.changed() => {
                        if changed.is_ok() && *merge_shutdown_rx.borrow() {
                            break;
                        }
                    }
                    maybe_event = event_rx.recv() => {
                        let Some(event) = maybe_event else {
                            break;
                        };

                        let latency = block_latency(event.block.timestamp);
                        latency_store
                            .write()
                            .await
                            .insert(event.provider_name, latency);

                        if deduplicator.is_new_block(event.block.hash) {
                            let _ = unified_sender.send(event.block);
                        }
                    }
                }
            }
        }));

        for registration in &self.providers {
            let provider_name = registration.name.clone();
            let provider = Arc::clone(&registration.provider);
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            let event_tx = event_tx.clone();
            let reconnection_handler = Arc::clone(&self.reconnection_handler);
            let initial_receiver = match registration.provider.subscribe_blocks().await {
                Ok(value) => {
                    self.reconnection_handler.record_reconnect(&provider_name);
                    Some(value)
                }
                Err(_) => {
                    self.reconnection_handler.record_disconnect(&provider_name);
                    None
                }
            };

            let handle = tokio::spawn(async move {
                let mut current_receiver = initial_receiver;

                loop {
                    if *shutdown_rx.borrow() {
                        break;
                    }

                    let mut receiver = if let Some(value) = current_receiver.take() {
                        value
                    } else {
                        match subscribe_blocks_blocking(Arc::clone(&provider)) {
                            Ok(value) => {
                                reconnection_handler.record_reconnect(&provider_name);
                                value
                            }
                            Err(_) => {
                                reconnection_handler.record_disconnect(&provider_name);
                                let backoff = reconnection_handler.next_backoff(&provider_name);
                                tokio::select! {
                                    changed = shutdown_rx.changed() => {
                                        if changed.is_ok() && *shutdown_rx.borrow() {
                                            break;
                                        }
                                    }
                                    _ = sleep(backoff) => {}
                                }
                                continue;
                            }
                        }
                    };

                    loop {
                        tokio::select! {
                            changed = shutdown_rx.changed() => {
                                if changed.is_ok() && *shutdown_rx.borrow() {
                                    return;
                                }
                            }
                            incoming = receiver.recv() => {
                                match incoming {
                                    Ok(block) => {
                                        if event_tx
                                            .send(ProviderBlockEvent {
                                                provider_name: provider_name.clone(),
                                                block,
                                            })
                                            .await
                                            .is_err()
                                        {
                                            return;
                                        }
                                    }
                                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                                    Err(broadcast::error::RecvError::Closed) => {
                                        reconnection_handler.record_disconnect(&provider_name);
                                        let backoff = reconnection_handler.next_backoff(&provider_name);
                                        tokio::select! {
                                            changed = shutdown_rx.changed() => {
                                                if changed.is_ok() && *shutdown_rx.borrow() {
                                                    return;
                                                }
                                            }
                                            _ = sleep(backoff) => {}
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            });

            self.worker_handles.push(handle);
        }

        Ok(())
    }

    /// Returns a receiver for the unified block stream.
    pub fn subscribe_blocks(&self) -> broadcast::Receiver<BlockInfo> {
        self.block_sender.subscribe()
    }

    /// Returns the latest measured latency for a provider.
    pub async fn latency_for_provider(&self, provider_name: &str) -> Option<Duration> {
        self.provider_latencies
            .read()
            .await
            .get(provider_name)
            .copied()
    }

    /// Gracefully stops all worker tasks.
    pub async fn shutdown(&mut self) {
        let _ = self.shutdown_tx.send(true);

        for handle in self.worker_handles.drain(..) {
            let _ = handle.await;
        }

        if let Some(handle) = self.merge_handle.take() {
            let _ = handle.await;
        }
    }
}

fn block_latency(block_timestamp: u64) -> Duration {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs();
    Duration::from_secs(now.saturating_sub(block_timestamp))
}

fn subscribe_blocks_blocking<P>(provider: Arc<P>) -> Result<broadcast::Receiver<BlockInfo>>
where
    P: ChainDataProvider + Send + Sync + 'static,
{
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(provider.subscribe_blocks())
    })
    .join()
    .map_err(|_| PyralisError::Internal("provider subscription thread panicked".to_string()))?
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use alloy::primitives::{Address, B256};
    use alloy::rpc::types::eth::Transaction;
    use pyralis_core::error::Result;
    use pyralis_core::traits::ChainDataProvider;
    use tokio::sync::broadcast;
    use tokio::time::sleep;
    use tokio::time::timeout;

    use super::{BlockDeduplicator, ProviderRegistration, StreamManager};

    #[derive(Debug)]
    struct MockProvider {
        block_sender: broadcast::Sender<pyralis_core::types::BlockInfo>,
        pending_sender: broadcast::Sender<Transaction>,
    }

    impl MockProvider {
        fn new() -> Self {
            let (block_sender, _) = broadcast::channel(32);
            let (pending_sender, _) = broadcast::channel(32);
            Self {
                block_sender,
                pending_sender,
            }
        }

        fn emit_block(&self, block: pyralis_core::types::BlockInfo) {
            let _ = self.block_sender.send(block);
        }
    }

    #[allow(async_fn_in_trait)]
    impl ChainDataProvider for MockProvider {
        async fn subscribe_blocks(
            &self,
        ) -> Result<broadcast::Receiver<pyralis_core::types::BlockInfo>> {
            Ok(self.block_sender.subscribe())
        }

        async fn subscribe_pending_txs(&self) -> Result<broadcast::Receiver<Transaction>> {
            Ok(self.pending_sender.subscribe())
        }

        async fn get_storage_at(
            &self,
            _address: Address,
            _slot: B256,
            _block_number: Option<u64>,
        ) -> Result<B256> {
            Ok(B256::ZERO)
        }

        async fn get_block_by_number(
            &self,
            _block_number: u64,
        ) -> Result<Option<pyralis_core::types::BlockInfo>> {
            Ok(None)
        }
    }

    #[derive(Debug)]
    struct ReconnectingMockProvider {
        block_senders: Mutex<Vec<broadcast::Sender<pyralis_core::types::BlockInfo>>>,
        pending_sender: broadcast::Sender<Transaction>,
        subscribe_calls: AtomicUsize,
    }

    impl ReconnectingMockProvider {
        fn new() -> Self {
            let (pending_sender, _) = broadcast::channel(32);
            Self {
                block_senders: Mutex::new(Vec::new()),
                pending_sender,
                subscribe_calls: AtomicUsize::new(0),
            }
        }

        fn subscribe_calls(&self) -> usize {
            self.subscribe_calls.load(Ordering::Relaxed)
        }

        fn emit_latest(&self, block: pyralis_core::types::BlockInfo) -> bool {
            self.block_senders
                .lock()
                .ok()
                .and_then(|senders| senders.last().cloned())
                .map(|sender| sender.send(block).is_ok())
                .unwrap_or(false)
        }

        fn close_latest(&self) {
            if let Ok(mut senders) = self.block_senders.lock() {
                senders.pop();
            }
        }
    }

    #[allow(async_fn_in_trait)]
    impl ChainDataProvider for ReconnectingMockProvider {
        async fn subscribe_blocks(
            &self,
        ) -> Result<broadcast::Receiver<pyralis_core::types::BlockInfo>> {
            self.subscribe_calls.fetch_add(1, Ordering::Relaxed);
            let (sender, receiver) = broadcast::channel(32);
            if let Ok(mut senders) = self.block_senders.lock() {
                senders.push(sender);
            }
            Ok(receiver)
        }

        async fn subscribe_pending_txs(&self) -> Result<broadcast::Receiver<Transaction>> {
            Ok(self.pending_sender.subscribe())
        }

        async fn get_storage_at(
            &self,
            _address: Address,
            _slot: B256,
            _block_number: Option<u64>,
        ) -> Result<B256> {
            Ok(B256::ZERO)
        }

        async fn get_block_by_number(
            &self,
            _block_number: u64,
        ) -> Result<Option<pyralis_core::types::BlockInfo>> {
            Ok(None)
        }
    }

    #[test]
    fn test_deduplicator_ignores_duplicate_hashes() {
        let mut deduplicator = BlockDeduplicator::default();
        let hash = B256::repeat_byte(9);

        assert!(deduplicator.is_new_block(hash));
        assert!(!deduplicator.is_new_block(hash));
    }

    #[tokio::test]
    async fn test_stream_manager_deduplicates_same_hash_from_multiple_providers() {
        let provider_a = Arc::new(MockProvider::new());
        let provider_b = Arc::new(MockProvider::new());

        let mut manager = StreamManager::new(vec![
            ProviderRegistration::new("provider-a", Arc::clone(&provider_a)),
            ProviderRegistration::new("provider-b", Arc::clone(&provider_b)),
        ]);
        assert!(manager.start().await.is_ok());

        let mut unified = manager.subscribe_blocks();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs();
        let block = pyralis_core::types::BlockInfo {
            number: 777,
            hash: B256::repeat_byte(3),
            timestamp: now,
            base_fee: Some(10),
        };

        provider_a.emit_block(block.clone());
        provider_b.emit_block(block);

        let first = timeout(Duration::from_millis(300), unified.recv()).await;
        assert!(matches!(first, Ok(Ok(_))));

        let second = timeout(Duration::from_millis(300), unified.recv()).await;
        assert!(second.is_err());

        manager.shutdown().await;
    }

    #[tokio::test]
    async fn test_stream_manager_resubscribes_after_disconnect() {
        let provider = Arc::new(ReconnectingMockProvider::new());
        let mut manager = StreamManager::new(vec![ProviderRegistration::new(
            "provider-reconnect",
            Arc::clone(&provider),
        )]);
        assert!(manager.start().await.is_ok());

        let mut unified = manager.subscribe_blocks();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_else(|_| Duration::from_secs(0))
            .as_secs();

        assert!(provider.emit_latest(pyralis_core::types::BlockInfo {
            number: 1,
            hash: B256::repeat_byte(0x01),
            timestamp: now,
            base_fee: Some(1),
        }));
        assert!(matches!(
            timeout(Duration::from_millis(300), unified.recv()).await,
            Ok(Ok(_))
        ));

        provider.close_latest();
        let wait_reconnect = timeout(Duration::from_secs(2), async {
            loop {
                if provider.subscribe_calls() >= 2 {
                    break;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(wait_reconnect.is_ok());

        assert!(provider.emit_latest(pyralis_core::types::BlockInfo {
            number: 2,
            hash: B256::repeat_byte(0x02),
            timestamp: now,
            base_fee: Some(1),
        }));
        assert!(matches!(
            timeout(Duration::from_millis(300), unified.recv()).await,
            Ok(Ok(_))
        ));

        manager.shutdown().await;
    }
}
