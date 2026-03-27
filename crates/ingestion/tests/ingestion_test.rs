use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, B256};
use alloy::rpc::types::eth::Transaction;
use pyralis_core::error::Result;
use pyralis_core::traits::ChainDataProvider;
use pyralis_core::types::BlockInfo;
use pyralis_ingestion::{ProviderRegistration, StreamManager};
use tokio::sync::broadcast;
use tokio::time::timeout;

#[derive(Debug)]
struct MockProvider {
    block_sender: broadcast::Sender<BlockInfo>,
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

    fn emit_block(&self, block: BlockInfo) {
        let _ = self.block_sender.send(block);
    }
}

#[allow(async_fn_in_trait)]
impl ChainDataProvider for MockProvider {
    async fn subscribe_blocks(&self) -> Result<broadcast::Receiver<BlockInfo>> {
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

    async fn get_block_by_number(&self, _block_number: u64) -> Result<Option<BlockInfo>> {
        Ok(None)
    }
}

fn load_ws_endpoints_from_config() -> Vec<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config/default.toml");
    let contents = fs::read_to_string(path).unwrap_or_default();
    let parsed: toml::Table = toml::from_str(&contents).unwrap_or_default();

    parsed
        .get("network")
        .and_then(toml::Value::as_table)
        .and_then(|network| network.get("rpc_ws_endpoints"))
        .and_then(toml::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(toml::Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn test_stream_manager_constructs_with_config() {
    let endpoints = load_ws_endpoints_from_config();
    assert!(!endpoints.is_empty());

    let providers = endpoints
        .iter()
        .enumerate()
        .map(|(index, endpoint)| {
            ProviderRegistration::new(
                format!("mock-{index}:{endpoint}"),
                Arc::new(MockProvider::new()),
            )
        })
        .collect::<Vec<_>>();

    let mut manager = StreamManager::new(providers);
    assert!(manager.start().await.is_ok());
    manager.shutdown().await;
}

#[tokio::test]
async fn test_stream_manager_graceful_shutdown() {
    let provider = Arc::new(MockProvider::new());
    let mut manager = StreamManager::new(vec![ProviderRegistration::new(
        "mock-provider",
        Arc::clone(&provider),
    )]);
    assert!(manager.start().await.is_ok());

    let mut receiver = manager.subscribe_blocks();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_secs();
    provider.emit_block(BlockInfo {
        number: 1,
        hash: B256::repeat_byte(1),
        timestamp: now,
        base_fee: Some(1),
    });

    let received = timeout(Duration::from_millis(300), receiver.recv()).await;
    assert!(matches!(received, Ok(Ok(_))));

    manager.shutdown().await;
}
