use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::{Address, Bytes, B256, U256};
use alloy::rpc::types::eth::{Transaction, TransactionInput, TransactionRequest};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use dashmap::DashMap;
use pyralis_core::error::Result;
use pyralis_core::traits::{ChainDataProvider, Simulator};
use pyralis_core::types::BlockInfo;
use pyralis_simulation::SimulationEngine;
use revm::state::{AccountInfo, Bytecode};
use tokio::runtime::Runtime;
use tokio::sync::broadcast;

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

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime should build")
}

async fn setup_engine() -> (SimulationEngine<MockProvider>, Address) {
    let block = BlockInfo {
        number: 1,
        hash: B256::repeat_byte(0x42),
        timestamp: 1_700_000_000,
        base_fee: Some(1),
    };
    let provider = Arc::new(MockProvider::new(block));
    let engine = SimulationEngine::new(provider);
    engine.load_state(1).await.expect("state should load");

    let sender = Address::repeat_byte(0xAA);
    engine
        .insert_account_info(
            sender,
            AccountInfo::default().with_balance(U256::from(1_000_000_000_u64)),
        )
        .await;

    (engine, sender)
}

fn bench_simulate_tx_simple_transfer(c: &mut Criterion) {
    let rt = runtime();
    let (engine, sender) = rt.block_on(setup_engine());

    let recipient = Address::repeat_byte(0xBB);
    rt.block_on(engine.insert_account_info(recipient, AccountInfo::default()));
    let tx = TransactionRequest::default()
        .from(sender)
        .to(recipient)
        .gas_limit(21_000)
        .gas_price(1)
        .value(U256::from(1_000_u64));

    c.bench_function("simulate_tx_simple_transfer", |b| {
        b.to_async(&rt).iter(|| async {
            let result = engine
                .simulate_tx_detailed(&tx)
                .await
                .expect("simulation should succeed");
            black_box(result.gas_used);
        });
    });
}

fn bench_simulate_tx_uniswap_v3_style_swap(c: &mut Criterion) {
    let rt = runtime();
    let (engine, sender) = rt.block_on(setup_engine());

    let target = Address::repeat_byte(0xCC);
    rt.block_on(engine.insert_account_info(
        target,
        AccountInfo::default().with_code(Bytecode::new_raw(Bytes::from(vec![0x00]))),
    ));
    let mut calldata = vec![0x12, 0x8A, 0xF0, 0x0D];
    calldata.extend(vec![0_u8; 160]);
    let calldata = Bytes::from(calldata);
    let tx = TransactionRequest::default()
        .from(sender)
        .to(target)
        .gas_limit(300_000)
        .gas_price(1)
        .value(U256::ZERO)
        .input(TransactionInput::new(calldata));

    c.bench_function("simulate_tx_uniswap_v3_style_swap", |b| {
        b.to_async(&rt).iter(|| async {
            let result = engine
                .simulate_tx_detailed(&tx)
                .await
                .expect("simulation should succeed");
            black_box(result.gas_used);
        });
    });
}

fn criterion_config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_secs(2))
        .sample_size(30)
}

criterion_group! {
    name = benches;
    config = criterion_config();
    targets = bench_simulate_tx_simple_transfer, bench_simulate_tx_uniswap_v3_style_swap
}
criterion_main!(benches);
