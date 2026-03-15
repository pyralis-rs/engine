//! Core domain types exchanged across pipeline layers.

use alloy::primitives::{Address, B256, U256};
use alloy::rpc::types::eth::{Transaction, TransactionRequest};

/// Metadata for a chain block used during simulation and strategy evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockInfo {
    /// Block number.
    pub number: u64,
    /// Block hash.
    pub hash: B256,
    /// Unix timestamp in seconds.
    pub timestamp: u64,
    /// Base fee in wei, when available.
    pub base_fee: Option<u128>,
}

/// Snapshot of a V4 pool state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolState {
    /// Pool contract address.
    pub address: Address,
    /// Token0 address.
    pub token0: Address,
    /// Token1 address.
    pub token1: Address,
    /// Pool fee tier.
    pub fee: u32,
    /// Current sqrt price Q96 fixed point.
    pub sqrt_price_x96: U256,
    /// Current active liquidity.
    pub liquidity: u128,
    /// Current pool tick.
    pub tick: i32,
}

/// Candidate MEV opportunity found by a strategy.
#[derive(Debug, Clone, PartialEq)]
pub struct Opportunity {
    /// Strategy identifier that produced this opportunity.
    pub strategy_name: String,
    /// Estimated gross profit in wei.
    pub estimated_profit: U256,
    /// Confidence score in the [0.0, 1.0] range.
    pub confidence: f64,
    /// Pool addresses touched by this opportunity.
    pub pools_involved: Vec<Address>,
    /// Unix timestamp in seconds when the opportunity was produced.
    pub timestamp: u64,
}

/// Set of opportunities and transactions selected for execution.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionPlan {
    /// Opportunities selected by strategy evaluation.
    pub opportunities: Vec<Opportunity>,
    /// Transactions that implement the plan.
    pub transactions: Vec<TransactionRequest>,
}

/// Result returned by an executor implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionResult {
    /// Whether the plan was executed successfully.
    pub success: bool,
    /// Total gas consumed by execution.
    pub gas_used: u64,
    /// Profit realized in wei, if measurable.
    pub actual_profit: Option<U256>,
    /// Final transaction hash, when applicable.
    pub tx_hash: Option<B256>,
}

/// Input context passed to strategies for evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct SimulationContext {
    /// Current block metadata.
    pub block: BlockInfo,
    /// Pool snapshots available for this evaluation window.
    pub pool_states: Vec<PoolState>,
    /// Pending mempool transactions.
    pub pending_txs: Vec<Transaction>,
}
