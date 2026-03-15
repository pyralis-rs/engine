//! Network-specific configuration abstraction.

use alloy::primitives::Address;
use alloy::rpc::types::eth::TransactionRequest;

use crate::error::Result;

/// Describes chain-specific execution and RPC details.
pub trait NetworkConfig: Send + Sync {
    /// Returns the chain ID.
    fn chain_id(&self) -> u64;

    /// Returns block time in milliseconds.
    fn block_time(&self) -> u64;

    /// Returns the configured PoolManager address.
    fn pool_manager_address(&self) -> Address;

    /// Estimates gas for a transaction according to network-specific rules.
    fn estimate_gas(&self, tx: &TransactionRequest) -> Result<u128>;

    /// Returns configured RPC endpoints.
    fn rpc_endpoints(&self) -> &[String];
}
