//! Simulation abstraction over EVM state execution.

use alloy::rpc::types::eth::{Transaction, TransactionRequest};

use crate::error::Result;
use crate::types::ExecutionResult;

/// Simulates transactions against a local EVM state view.
#[allow(async_fn_in_trait)]
pub trait Simulator: Send + Sync {
    /// Loads chain state for simulation at the requested block number.
    async fn load_state(&self, block_number: u64) -> Result<()>;

    /// Simulates a single transaction against the currently loaded state.
    async fn simulate_tx(&self, tx: &TransactionRequest) -> Result<ExecutionResult>;

    /// Simulates a batch of transactions in order.
    async fn simulate_batch(&self, txs: &[Transaction]) -> Result<Vec<ExecutionResult>>;
}
