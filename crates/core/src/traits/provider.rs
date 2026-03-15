//! Data-provider abstraction over chain access.

use alloy::primitives::{Address, B256};
use alloy::rpc::types::eth::Transaction;
use tokio::sync::broadcast;

use crate::error::Result;
use crate::types::BlockInfo;

/// Provides data access and subscriptions from an EVM chain.
#[allow(async_fn_in_trait)]
pub trait ChainDataProvider: Send + Sync {
    /// Subscribes to new block notifications.
    async fn subscribe_blocks(&self) -> Result<broadcast::Receiver<BlockInfo>>;

    /// Subscribes to pending transactions.
    async fn subscribe_pending_txs(&self) -> Result<broadcast::Receiver<Transaction>>;

    /// Reads contract storage for an address and slot at an optional block height.
    async fn get_storage_at(
        &self,
        address: Address,
        slot: B256,
        block_number: Option<u64>,
    ) -> Result<B256>;

    /// Fetches block metadata by block number.
    async fn get_block_by_number(&self, block_number: u64) -> Result<Option<BlockInfo>>;
}
