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

        let mut active_workers = 0usize;

        for registration in &self.providers {
            let provider_name = registration.name.clone();
            let mut shutdown_rx = self.shutdown_tx.subscribe();
            let event_tx = event_tx.clone();
            let reconnection_handler = Arc::clone(&self.reconnection_handler);

            let mut receiver = match registration.provider.subscribe_blocks().await {
                Ok(value) => {
                    self.reconnection_handler.record_reconnect(&provider_name);
                    value
                }
                Err(_) => {
                    self.reconnection_handler.record_disconnect(&provider_name);
                    let backoff = self.reconnection_handler.next_backoff(&provider_name);
                    sleep(backoff).await;
                    continue;
                }
            };

            let handle = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        changed = shutdown_rx.changed() => {
                            if changed.is_ok() && *shutdown_rx.borrow() {
                                break;
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
                                        break;
                                    }
                                }
                                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(broadcast::error::RecvError::Closed) => {
                                    reconnection_handler.record_disconnect(&provider_name);
                                    let backoff = reconnection_handler.next_backoff(&provider_name);
                                    sleep(backoff).await;
                                    break;
                                }
                            }
                        }
                    }
                }
            });

            self.worker_handles.push(handle);
            active_workers = active_workers.saturating_add(1);
        }

        if active_workers == 0 {
            return Err(PyralisError::Provider(
                "no active provider subscriptions could be established".to_string(),
            ));
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
