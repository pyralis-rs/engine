//! revm-backed transaction simulation engine.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Instant;

use alloy::primitives::{Address, Bytes, TxKind, U256};
use alloy::rpc::types::eth::{Transaction, TransactionRequest};
use pyralis_core::error::{PyralisError, Result};
use pyralis_core::traits::{ChainDataProvider, Simulator};
use pyralis_core::types::{BlockInfo, ExecutionResult};
use revm::context::{BlockEnv, Context, TxEnv};
use revm::database::{CacheDB, EmptyDB};
use revm::state::{AccountInfo, EvmState};
use revm::{ExecuteCommitEvm, ExecuteEvm, MainBuilder, MainContext};
use tokio::sync::RwLock;

/// Full simulation output returned by [`SimulationEngine::simulate_tx_detailed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationResult {
    /// Whether the transaction executed successfully.
    pub success: bool,
    /// Total gas used by the transaction.
    pub gas_used: u64,
    /// Return data or revert data when available.
    pub output: Option<Bytes>,
    /// Logs emitted during execution.
    pub logs: Vec<alloy::primitives::Log>,
    /// Account/storage changes produced by execution.
    pub state_changes: EvmState,
    /// End-to-end simulation time in microseconds.
    pub duration_micros: u128,
}

/// revm simulation engine that executes transactions against a forked in-memory state.
pub struct SimulationEngine<P> {
    provider: Arc<P>,
    base_db: Arc<RwLock<CacheDB<EmptyDB>>>,
    block_env: Arc<RwLock<BlockEnv>>,
    last_duration_micros: Arc<AtomicU64>,
}

impl<P> SimulationEngine<P>
where
    P: ChainDataProvider + Send + Sync + 'static,
{
    /// Creates a new simulation engine with an empty base database.
    pub fn new(provider: Arc<P>) -> Self {
        Self {
            provider,
            base_db: Arc::new(RwLock::new(CacheDB::new(EmptyDB::default()))),
            block_env: Arc::new(RwLock::new(BlockEnv::default())),
            last_duration_micros: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Inserts account info into the base database.
    pub async fn insert_account_info(&self, address: Address, info: AccountInfo) {
        self.base_db
            .write()
            .await
            .insert_account_info(address, info);
    }

    /// Returns the duration of the most recent simulation in microseconds.
    pub fn last_duration_micros(&self) -> u64 {
        self.last_duration_micros.load(Ordering::Relaxed)
    }

    /// Simulates one transaction and returns detailed output.
    pub async fn simulate_tx_detailed(&self, tx: &TransactionRequest) -> Result<SimulationResult> {
        let tx_env = tx_request_to_tx_env(tx)?;
        let block_env = self.block_env.read().await.clone();
        let base_db_snapshot = self.base_db.read().await.clone();

        let mut evm = Context::mainnet().with_db(base_db_snapshot).build_mainnet();
        evm.set_block(block_env);

        let started_at = Instant::now();
        let result = evm
            .transact(tx_env)
            .map_err(|error| PyralisError::Simulation(format!("revm execution failed: {error}")))?;
        let duration_micros = started_at.elapsed().as_micros();
        let duration_u64 = u64::try_from(duration_micros).unwrap_or(u64::MAX);
        self.last_duration_micros
            .store(duration_u64, Ordering::Relaxed);

        let exec = result.result;
        let output = exec.output().cloned();
        let logs = exec.logs().to_vec();
        Ok(SimulationResult {
            success: exec.is_success(),
            gas_used: exec.gas_used(),
            output,
            logs,
            state_changes: result.state,
            duration_micros,
        })
    }

    /// Simulates multiple transactions sequentially and returns detailed output for each one.
    pub async fn simulate_batch_detailed(
        &self,
        txs: &[Transaction],
    ) -> Result<Vec<SimulationResult>> {
        let block_env = self.block_env.read().await.clone();
        let base_db_snapshot = self.base_db.read().await.clone();
        let mut evm = Context::mainnet().with_db(base_db_snapshot).build_mainnet();
        evm.set_block(block_env);

        let mut results = Vec::with_capacity(txs.len());
        for tx in txs {
            let request = TransactionRequest::from_transaction(tx.clone());
            let tx_env = tx_request_to_tx_env(&request)?;

            let started_at = Instant::now();
            let execution = evm.transact_one(tx_env).map_err(|error| {
                PyralisError::Simulation(format!("revm execution failed in batch: {error}"))
            })?;
            let state_changes = evm.finalize();
            evm.commit(state_changes.clone());

            let duration_micros = started_at.elapsed().as_micros();
            let duration_u64 = u64::try_from(duration_micros).unwrap_or(u64::MAX);
            self.last_duration_micros
                .store(duration_u64, Ordering::Relaxed);

            results.push(SimulationResult {
                success: execution.is_success(),
                gas_used: execution.gas_used(),
                output: execution.output().cloned(),
                logs: execution.logs().to_vec(),
                state_changes,
                duration_micros,
            });
        }
        Ok(results)
    }

    async fn update_block_env(&self, block: &BlockInfo) {
        let mut env = self.block_env.write().await;
        env.number = U256::from(block.number);
        env.timestamp = U256::from(block.timestamp);
        env.basefee = block
            .base_fee
            .map(|fee| u64::try_from(fee).unwrap_or(u64::MAX))
            .unwrap_or_default();
    }
}

#[allow(async_fn_in_trait)]
impl<P> Simulator for SimulationEngine<P>
where
    P: ChainDataProvider + Send + Sync + 'static,
{
    async fn load_state(&self, block_number: u64) -> Result<()> {
        let block = self.provider.get_block_by_number(block_number).await?;
        let Some(block) = block else {
            return Err(PyralisError::Simulation(format!(
                "block {block_number} was not found"
            )));
        };

        self.base_db
            .write()
            .await
            .cache
            .block_hashes
            .insert(U256::from(block.number), block.hash);
        self.update_block_env(&block).await;
        Ok(())
    }

    async fn simulate_tx(&self, tx: &TransactionRequest) -> Result<ExecutionResult> {
        let result = self.simulate_tx_detailed(tx).await?;
        Ok(ExecutionResult {
            success: result.success,
            gas_used: result.gas_used,
            actual_profit: None,
            tx_hash: None,
        })
    }

    async fn simulate_batch(&self, txs: &[Transaction]) -> Result<Vec<ExecutionResult>> {
        let mut output = Vec::with_capacity(txs.len());
        for result in self.simulate_batch_detailed(txs).await? {
            output.push(ExecutionResult {
                success: result.success,
                gas_used: result.gas_used,
                actual_profit: None,
                tx_hash: None,
            });
        }
        Ok(output)
    }
}

fn tx_request_to_tx_env(tx: &TransactionRequest) -> Result<TxEnv> {
    let kind = match tx.to {
        Some(TxKind::Call(address)) => revm::primitives::TxKind::Call(address),
        Some(TxKind::Create) | None => revm::primitives::TxKind::Create,
    };

    TxEnv::builder()
        .caller(tx.from.unwrap_or(Address::ZERO))
        .gas_limit(tx.gas.unwrap_or(21_000))
        .gas_price(tx.max_fee_per_gas.or(tx.gas_price).unwrap_or_default())
        .gas_priority_fee(tx.max_priority_fee_per_gas)
        .kind(kind)
        .value(tx.value.unwrap_or(U256::ZERO))
        .data(tx.input.input().cloned().unwrap_or_default())
        .nonce(tx.nonce.unwrap_or_default())
        .chain_id(tx.chain_id)
        .build()
        .map_err(|error| {
            PyralisError::Simulation(format!("failed to build revm transaction env: {error:?}"))
        })
}

#[cfg(test)]
mod tests {
    use alloy::primitives::{Address, B256, U256};
    use alloy::rpc::types::eth::Transaction;
    use dashmap::DashMap;
    use tokio::sync::broadcast;

    use super::*;

    #[derive(Debug)]
    struct MockProvider {
        blocks: DashMap<u64, BlockInfo>,
    }

    impl MockProvider {
        fn new(block: BlockInfo) -> Self {
            let blocks = DashMap::new();
            blocks.insert(block.number, block);
            Self { blocks }
        }
    }

    #[allow(async_fn_in_trait)]
    impl ChainDataProvider for MockProvider {
        async fn subscribe_blocks(&self) -> Result<broadcast::Receiver<BlockInfo>> {
            let (_sender, receiver) = broadcast::channel(1);
            Ok(receiver)
        }

        async fn subscribe_pending_txs(&self) -> Result<broadcast::Receiver<Transaction>> {
            let (_sender, receiver) = broadcast::channel(1);
            Ok(receiver)
        }

        async fn get_storage_at(
            &self,
            _address: Address,
            _slot: B256,
            _block_number: Option<u64>,
        ) -> Result<B256> {
            Ok(B256::ZERO)
        }

        async fn get_block_by_number(&self, block_number: u64) -> Result<Option<BlockInfo>> {
            Ok(self.blocks.get(&block_number).map(|block| block.clone()))
        }
    }

    #[tokio::test]
    async fn test_simulation_engine_simulate_tx_with_hardcoded_state() {
        let block = BlockInfo {
            number: 1,
            hash: B256::repeat_byte(0x11),
            timestamp: 1_700_000_000,
            base_fee: Some(1),
        };
        let provider = Arc::new(MockProvider::new(block));
        let engine = SimulationEngine::new(provider);
        engine
            .load_state(1)
            .await
            .expect("state load should succeed");

        let sender = Address::repeat_byte(0xAA);
        let receiver = Address::repeat_byte(0xBB);
        engine
            .insert_account_info(
                sender,
                AccountInfo::default().with_balance(U256::from(10_000_000_u64)),
            )
            .await;
        engine
            .insert_account_info(receiver, AccountInfo::default())
            .await;

        let tx = TransactionRequest {
            from: Some(sender),
            to: Some(TxKind::Call(receiver)),
            gas: Some(21_000),
            gas_price: Some(1),
            value: Some(U256::from(1_000_u64)),
            ..Default::default()
        };

        let result = engine
            .simulate_tx_detailed(&tx)
            .await
            .expect("simulation should succeed");

        assert!(result.success);
        assert!(result.gas_used > 0);
        assert_eq!(
            engine.last_duration_micros(),
            u64::try_from(result.duration_micros).unwrap_or(u64::MAX)
        );
    }
}
