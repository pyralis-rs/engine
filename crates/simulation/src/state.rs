//! revm database backends for simulation.

use std::collections::HashMap;
use std::sync::Arc;

use alloy::primitives::{Address, B256, U256};
use dashmap::DashMap;
use pyralis_core::error::{PyralisError, Result};
use pyralis_core::traits::ChainDataProvider;
use revm::database_interface::DBErrorMarker;
use revm::state::{AccountInfo, Bytecode};
use revm::Database;
use thiserror::Error;

/// Errors emitted by simulation state providers.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StateProviderError {
    /// Generic provider failure.
    #[error("state provider error: {0}")]
    Message(String),
}

impl DBErrorMarker for StateProviderError {}

/// State provider optimized for startup speed and on-demand slot fetches.
pub struct LazyStateProvider<P> {
    provider: Arc<P>,
    block_number: Option<u64>,
    runtime: tokio::runtime::Runtime,
    slot_cache: DashMap<(Address, U256), U256>,
    account_cache: DashMap<Address, AccountInfo>,
    code_cache: DashMap<B256, Bytecode>,
    block_hash_cache: DashMap<u64, B256>,
}

impl<P> LazyStateProvider<P>
where
    P: ChainDataProvider + Send + Sync + 'static,
{
    /// Creates a new lazy provider.
    pub fn new(provider: Arc<P>, block_number: Option<u64>) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        Ok(Self {
            provider,
            block_number,
            runtime,
            slot_cache: DashMap::new(),
            account_cache: DashMap::new(),
            code_cache: DashMap::new(),
            block_hash_cache: DashMap::new(),
        })
    }

    /// Updates the block number used for RPC-backed storage reads.
    pub fn set_block_number(&mut self, block_number: Option<u64>) {
        self.block_number = block_number;
    }

    /// Inserts account metadata into cache.
    pub fn cache_account(&self, address: Address, info: AccountInfo) {
        if !info.code_hash.is_zero() {
            if let Some(code) = &info.code {
                self.code_cache.insert(info.code_hash, code.clone());
            }
        }
        self.account_cache.insert(address, info);
    }

    /// Inserts a pre-fetched storage slot into cache.
    pub fn cache_slot(&self, address: Address, slot: U256, value: U256) {
        self.slot_cache.insert((address, slot), value);
    }

    /// Inserts a block hash into cache.
    pub fn cache_block_hash(&self, block_number: u64, block_hash: B256) {
        self.block_hash_cache.insert(block_number, block_hash);
    }

    /// Returns the number of cached slots.
    pub fn cached_slot_count(&self) -> usize {
        self.slot_cache.len()
    }

    fn fetch_storage_slot(
        &self,
        address: Address,
        slot: U256,
    ) -> std::result::Result<U256, StateProviderError> {
        let slot_key = u256_to_b256(slot);
        let fetch_future = self
            .provider
            .get_storage_at(address, slot_key, self.block_number);
        let fetched = if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| self.runtime.block_on(fetch_future))
        } else {
            self.runtime.block_on(fetch_future)
        }
        .map_err(|error| StateProviderError::Message(error.to_string()))?;
        Ok(b256_to_u256(fetched))
    }
}

impl<P> Database for LazyStateProvider<P>
where
    P: ChainDataProvider + Send + Sync + 'static,
{
    type Error = StateProviderError;

    fn basic(&mut self, address: Address) -> std::result::Result<Option<AccountInfo>, Self::Error> {
        Ok(self
            .account_cache
            .get(&address)
            .map(|account| account.clone()))
    }

    fn code_by_hash(&mut self, code_hash: B256) -> std::result::Result<Bytecode, Self::Error> {
        Ok(self
            .code_cache
            .get(&code_hash)
            .map(|code| code.clone())
            .unwrap_or_default())
    }

    fn storage(&mut self, address: Address, index: U256) -> std::result::Result<U256, Self::Error> {
        if let Some(value) = self.slot_cache.get(&(address, index)) {
            return Ok(*value);
        }

        let fetched = self.fetch_storage_slot(address, index)?;
        self.slot_cache.insert((address, index), fetched);
        Ok(fetched)
    }

    fn block_hash(&mut self, number: u64) -> std::result::Result<B256, Self::Error> {
        Ok(self
            .block_hash_cache
            .get(&number)
            .map(|hash| *hash)
            .unwrap_or(B256::ZERO))
    }
}

/// State provider optimized for prefetch-heavy simulation workloads.
#[derive(Default)]
pub struct CachedStateProvider {
    slot_cache: DashMap<Address, HashMap<U256, U256>>,
    account_cache: DashMap<Address, AccountInfo>,
    code_cache: DashMap<B256, Bytecode>,
    block_hash_cache: DashMap<u64, B256>,
}

impl CachedStateProvider {
    /// Creates an empty cached state provider.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces cached slots for one contract address.
    pub fn cache_slots(&self, address: Address, slots: HashMap<U256, U256>) {
        self.slot_cache.insert(address, slots);
    }

    /// Prefetches slots from chain provider and stores them in memory.
    pub async fn prefetch_slots<P>(
        &self,
        provider: &P,
        address: Address,
        slots: &[U256],
        block_number: u64,
    ) -> Result<()>
    where
        P: ChainDataProvider + Send + Sync + 'static,
    {
        let mut slot_values = HashMap::with_capacity(slots.len());
        for slot in slots {
            let value = provider
                .get_storage_at(address, u256_to_b256(*slot), Some(block_number))
                .await?;
            slot_values.insert(*slot, b256_to_u256(value));
        }
        self.slot_cache.insert(address, slot_values);
        Ok(())
    }

    /// Inserts account metadata into cache.
    pub fn cache_account(&self, address: Address, info: AccountInfo) {
        if !info.code_hash.is_zero() {
            if let Some(code) = &info.code {
                self.code_cache.insert(info.code_hash, code.clone());
            }
        }
        self.account_cache.insert(address, info);
    }

    /// Inserts a block hash into cache.
    pub fn cache_block_hash(&self, block_number: u64, block_hash: B256) {
        self.block_hash_cache.insert(block_number, block_hash);
    }
}

impl Database for CachedStateProvider {
    type Error = StateProviderError;

    fn basic(&mut self, address: Address) -> std::result::Result<Option<AccountInfo>, Self::Error> {
        Ok(self.account_cache.get(&address).map(|value| value.clone()))
    }

    fn code_by_hash(&mut self, code_hash: B256) -> std::result::Result<Bytecode, Self::Error> {
        Ok(self
            .code_cache
            .get(&code_hash)
            .map(|code| code.clone())
            .unwrap_or_default())
    }

    fn storage(&mut self, address: Address, index: U256) -> std::result::Result<U256, Self::Error> {
        let value = self
            .slot_cache
            .get(&address)
            .and_then(|slots| slots.get(&index).copied())
            .unwrap_or(U256::ZERO);
        Ok(value)
    }

    fn block_hash(&mut self, number: u64) -> std::result::Result<B256, Self::Error> {
        Ok(self
            .block_hash_cache
            .get(&number)
            .map(|hash| *hash)
            .unwrap_or(B256::ZERO))
    }
}

/// Configurable revm database backend.
pub enum StateBackend<P> {
    /// Lazy on-demand RPC-backed storage provider.
    Lazy(LazyStateProvider<P>),
    /// Cached slot provider.
    Cached(CachedStateProvider),
}

impl<P> StateBackend<P>
where
    P: ChainDataProvider + Send + Sync + 'static,
{
    /// Creates a state backend from a config string.
    pub fn from_config(
        provider_kind: &str,
        provider: Arc<P>,
        block_number: Option<u64>,
    ) -> Result<Self> {
        match provider_kind {
            "lazy" => Ok(Self::Lazy(LazyStateProvider::new(provider, block_number)?)),
            "cached" => Ok(Self::Cached(CachedStateProvider::new())),
            value => Err(PyralisError::Config(format!(
                "unknown state provider type: {value}"
            ))),
        }
    }
}

impl<P> Database for StateBackend<P>
where
    P: ChainDataProvider + Send + Sync + 'static,
{
    type Error = StateProviderError;

    fn basic(&mut self, address: Address) -> std::result::Result<Option<AccountInfo>, Self::Error> {
        match self {
            Self::Lazy(provider) => provider.basic(address),
            Self::Cached(provider) => provider.basic(address),
        }
    }

    fn code_by_hash(&mut self, code_hash: B256) -> std::result::Result<Bytecode, Self::Error> {
        match self {
            Self::Lazy(provider) => provider.code_by_hash(code_hash),
            Self::Cached(provider) => provider.code_by_hash(code_hash),
        }
    }

    fn storage(&mut self, address: Address, index: U256) -> std::result::Result<U256, Self::Error> {
        match self {
            Self::Lazy(provider) => provider.storage(address, index),
            Self::Cached(provider) => provider.storage(address, index),
        }
    }

    fn block_hash(&mut self, number: u64) -> std::result::Result<B256, Self::Error> {
        match self {
            Self::Lazy(provider) => provider.block_hash(number),
            Self::Cached(provider) => provider.block_hash(number),
        }
    }
}

fn u256_to_b256(value: U256) -> B256 {
    B256::from(value.to_be_bytes::<32>())
}

fn b256_to_u256(value: B256) -> U256 {
    U256::from_be_slice(value.as_slice())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use alloy::rpc::types::eth::Transaction;
    use pyralis_core::traits::ChainDataProvider;
    use tokio::sync::broadcast;

    use super::*;

    #[derive(Debug)]
    struct MockProvider {
        storage_values: DashMap<(Address, B256, Option<u64>), B256>,
        storage_calls: AtomicUsize,
    }

    impl MockProvider {
        fn new() -> Self {
            Self {
                storage_values: DashMap::new(),
                storage_calls: AtomicUsize::new(0),
            }
        }

        fn insert_storage(
            &self,
            address: Address,
            slot: B256,
            block_number: Option<u64>,
            value: B256,
        ) {
            self.storage_values
                .insert((address, slot, block_number), value);
        }

        fn storage_calls(&self) -> usize {
            self.storage_calls.load(Ordering::Relaxed)
        }
    }

    #[allow(async_fn_in_trait)]
    impl ChainDataProvider for MockProvider {
        async fn subscribe_blocks(
            &self,
        ) -> Result<broadcast::Receiver<pyralis_core::types::BlockInfo>> {
            let (_sender, receiver) = broadcast::channel(1);
            Ok(receiver)
        }

        async fn subscribe_pending_txs(&self) -> Result<broadcast::Receiver<Transaction>> {
            let (_sender, receiver) = broadcast::channel(1);
            Ok(receiver)
        }

        async fn get_storage_at(
            &self,
            address: Address,
            slot: B256,
            block_number: Option<u64>,
        ) -> Result<B256> {
            self.storage_calls.fetch_add(1, Ordering::Relaxed);
            Ok(self
                .storage_values
                .get(&(address, slot, block_number))
                .map(|value| *value)
                .unwrap_or(B256::ZERO))
        }

        async fn get_block_by_number(
            &self,
            _block_number: u64,
        ) -> Result<Option<pyralis_core::types::BlockInfo>> {
            Ok(None)
        }
    }

    #[test]
    fn test_lazy_state_provider_caches_storage_slots() {
        let provider = Arc::new(MockProvider::new());
        let address = Address::repeat_byte(0xAA);
        let slot = U256::from(9_u64);
        let value = U256::from(42_u64);
        provider.insert_storage(address, u256_to_b256(slot), Some(123), u256_to_b256(value));

        let mut state = LazyStateProvider::new(Arc::clone(&provider), Some(123))
            .expect("lazy state provider should initialize");

        let first = state
            .storage(address, slot)
            .expect("first slot read should succeed");
        let second = state
            .storage(address, slot)
            .expect("second slot read should hit cache");

        assert_eq!(first, value);
        assert_eq!(second, value);
        assert_eq!(state.cached_slot_count(), 1);
        assert_eq!(provider.storage_calls(), 1);
    }
}
