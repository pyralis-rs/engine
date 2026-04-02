//! Hook-aware helpers for V4 swap simulation.
//!
//! The callbacks that can change swap behavior are:
//! - `beforeSwap`: runs before core swap math.
//! - `afterSwap`: runs after swap deltas are computed.

use alloy::primitives::{Address, Bytes};
use revm::database::CacheDB;
use revm::state::{AccountInfo, Bytecode};
use revm::DatabaseRef;

use super::pool_manager::PoolKey;

/// Returns `true` if this pool has a non-zero hook contract address.
pub fn pool_has_hooks(pool_key: &PoolKey) -> bool {
    pool_key.hooks != Address::ZERO
}

/// Injects hook bytecode into the in-memory simulation database.
pub fn load_hook_contract_code<DB>(
    db: &mut CacheDB<DB>,
    hook_address: Address,
    runtime_bytecode: Bytes,
) where
    DB: DatabaseRef,
{
    let mut account = AccountInfo::default();
    account.set_code(Bytecode::new_raw(runtime_bytecode));
    db.insert_account_info(hook_address, account);
}

/// Callback names that affect swap execution semantics.
pub fn swap_hook_callbacks() -> [&'static str; 2] {
    ["beforeSwap", "afterSwap"]
}
