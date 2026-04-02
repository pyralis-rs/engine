//! Uniswap V4 helpers used by the simulation layer.

pub mod hooks;
pub mod pool_manager;

pub use hooks::{load_hook_contract_code, pool_has_hooks, swap_hook_callbacks};
pub use pool_manager::{
    encode_v4_swap_call, pool_field_slot, pool_id_from_key, pool_state_base_slot, read_pool_state,
    PoolKey, SwapParams, POOLS_MAPPING_SLOT,
};
