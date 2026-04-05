//! Uniswap V4 PoolManager storage and calldata helpers.

use alloy::primitives::{keccak256, Address, Bytes, B256, I256, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use pyralis_core::error::{PyralisError, Result};
use pyralis_core::types::PoolState;
use revm::Database;

/// Storage slot index for the PoolManager `pools` mapping.
pub const POOLS_MAPPING_SLOT: u64 = 6;
const SQRT_PRICE_SLOT_OFFSET: u64 = 0;
const LIQUIDITY_SLOT_OFFSET: u64 = 1;
const TICK_SLOT_OFFSET: u64 = 2;
const MAX_U24: u32 = 0x00FF_FFFF;
const MIN_I24: i32 = -8_388_608;
const MAX_I24: i32 = 8_388_607;

sol! {
    struct PoolKeyAbi {
        address token0;
        address token1;
        uint32 fee;
        int32 tickSpacing;
        address hooks;
    }

    struct SwapParamsAbi {
        bool zeroForOne;
        int256 amountSpecified;
        uint256 sqrtPriceLimitX96;
        bytes hookData;
    }

    function swap(PoolKeyAbi key, SwapParamsAbi params, bytes data) external;
}

/// Canonical V4 pool identifier fields.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PoolKey {
    /// First pool token.
    pub token0: Address,
    /// Second pool token.
    pub token1: Address,
    /// Fee tier in hundredths of a bip.
    pub fee: u32,
    /// Tick spacing for the pool.
    pub tick_spacing: i32,
    /// Hook contract address.
    pub hooks: Address,
}

/// Swap request parameters encoded into a V4 `swap` calldata payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapParams {
    /// Trade direction.
    pub zero_for_one: bool,
    /// Positive for exact input, negative for exact output.
    pub amount_specified: I256,
    /// Optional swap price boundary.
    pub sqrt_price_limit_x96: U256,
    /// Arbitrary hook payload.
    pub hook_data: Bytes,
}

/// Computes a deterministic pool id from a V4 [`PoolKey`].
pub fn pool_id_from_key(pool_key: &PoolKey) -> B256 {
    let mut encoded = Vec::with_capacity(68);
    encoded.extend_from_slice(pool_key.token0.as_slice());
    encoded.extend_from_slice(pool_key.token1.as_slice());
    encoded.extend_from_slice(&pool_key.fee.to_be_bytes());
    encoded.extend_from_slice(&pool_key.tick_spacing.to_be_bytes());
    encoded.extend_from_slice(pool_key.hooks.as_slice());
    keccak256(encoded)
}

/// Computes the base storage slot for a pool state entry.
pub fn pool_state_base_slot(pool_id: B256) -> U256 {
    mapping_slot(pool_id, POOLS_MAPPING_SLOT)
}

/// Computes an offset slot for a specific field inside one pool entry.
pub fn pool_field_slot(pool_id: B256, field_offset: u64) -> U256 {
    pool_state_base_slot(pool_id) + U256::from(field_offset)
}

/// Reads V4 pool state from PoolManager singleton storage.
pub fn read_pool_state<D>(
    db: &mut D,
    pool_manager: Address,
    pool_key: &PoolKey,
) -> Result<PoolState>
where
    D: Database,
    D::Error: std::fmt::Display,
{
    let pool_id = pool_id_from_key(pool_key);

    let sqrt_price_x96 = db
        .storage(
            pool_manager,
            pool_field_slot(pool_id, SQRT_PRICE_SLOT_OFFSET),
        )
        .map_err(|error| {
            PyralisError::Simulation(format!("failed to read sqrtPriceX96: {error}"))
        })?;
    let liquidity_slot = db
        .storage(
            pool_manager,
            pool_field_slot(pool_id, LIQUIDITY_SLOT_OFFSET),
        )
        .map_err(|error| PyralisError::Simulation(format!("failed to read liquidity: {error}")))?;
    let tick_slot = db
        .storage(pool_manager, pool_field_slot(pool_id, TICK_SLOT_OFFSET))
        .map_err(|error| PyralisError::Simulation(format!("failed to read tick: {error}")))?;

    Ok(PoolState {
        address: pool_manager,
        token0: pool_key.token0,
        token1: pool_key.token1,
        fee: pool_key.fee,
        sqrt_price_x96,
        liquidity: low_u128(liquidity_slot),
        tick: low_i32(tick_slot),
    })
}

/// Encodes a PoolManager `swap` call for simulation.
pub fn encode_v4_swap_call(
    pool_key: &PoolKey,
    swap_params: &SwapParams,
    swap_data: Bytes,
) -> Result<Bytes> {
    validate_pool_key(pool_key)?;

    let call = swapCall {
        key: PoolKeyAbi {
            token0: pool_key.token0,
            token1: pool_key.token1,
            fee: pool_key.fee,
            tickSpacing: pool_key.tick_spacing,
            hooks: pool_key.hooks,
        },
        params: SwapParamsAbi {
            zeroForOne: swap_params.zero_for_one,
            amountSpecified: swap_params.amount_specified,
            sqrtPriceLimitX96: swap_params.sqrt_price_limit_x96,
            hookData: swap_params.hook_data.clone(),
        },
        data: swap_data,
    };
    Ok(Bytes::from(call.abi_encode()))
}

fn validate_pool_key(pool_key: &PoolKey) -> Result<()> {
    if pool_key.fee > MAX_U24 {
        return Err(PyralisError::Abi(format!(
            "fee value {} does not fit into uint24",
            pool_key.fee
        )));
    }

    if !(MIN_I24..=MAX_I24).contains(&pool_key.tick_spacing) {
        return Err(PyralisError::Abi(format!(
            "tick spacing value {} does not fit into int24",
            pool_key.tick_spacing
        )));
    }

    Ok(())
}

fn mapping_slot(key: B256, slot: u64) -> U256 {
    let mut preimage = [0_u8; 64];
    preimage[..32].copy_from_slice(key.as_slice());
    preimage[32..].copy_from_slice(&U256::from(slot).to_be_bytes::<32>());
    U256::from_be_slice(keccak256(preimage).as_slice())
}

fn low_u128(value: U256) -> u128 {
    let bytes = value.to_be_bytes::<32>();
    let mut raw = [0_u8; 16];
    raw.copy_from_slice(&bytes[16..]);
    u128::from_be_bytes(raw)
}

fn low_i32(value: U256) -> i32 {
    let bytes = value.to_be_bytes::<32>();
    let mut raw = [0_u8; 4];
    raw.copy_from_slice(&bytes[28..]);
    i32::from_be_bytes(raw)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use alloy::primitives::I256;

    use crate::state::CachedStateProvider;

    use super::*;

    #[test]
    fn test_read_pool_state_from_cached_storage() {
        let pool_manager = Address::repeat_byte(0x99);
        let pool_key = PoolKey {
            token0: Address::repeat_byte(0x01),
            token1: Address::repeat_byte(0x02),
            fee: 3_000,
            tick_spacing: 60,
            hooks: Address::ZERO,
        };
        let pool_id = pool_id_from_key(&pool_key);
        let mut slot_map = HashMap::new();
        slot_map.insert(
            pool_field_slot(pool_id, SQRT_PRICE_SLOT_OFFSET),
            U256::from(123_456_u64),
        );
        slot_map.insert(
            pool_field_slot(pool_id, LIQUIDITY_SLOT_OFFSET),
            U256::from(999_u64),
        );
        slot_map.insert(
            pool_field_slot(pool_id, TICK_SLOT_OFFSET),
            U256::from_be_slice(&{
                let mut bytes = [0_u8; 32];
                bytes[28..].copy_from_slice(&(-120_i32).to_be_bytes());
                bytes
            }),
        );

        let mut db = CachedStateProvider::new();
        db.cache_slots(pool_manager, slot_map);

        let state = read_pool_state(&mut db, pool_manager, &pool_key)
            .expect("pool state read should succeed");

        assert_eq!(state.sqrt_price_x96, U256::from(123_456_u64));
        assert_eq!(state.liquidity, 999);
        assert_eq!(state.tick, -120);
        assert_eq!(state.token0, pool_key.token0);
        assert_eq!(state.token1, pool_key.token1);
    }

    #[test]
    fn test_encode_v4_swap_call_rejects_out_of_range_fee() {
        let pool_key = PoolKey {
            token0: Address::repeat_byte(0x11),
            token1: Address::repeat_byte(0x22),
            fee: MAX_U24 + 1,
            tick_spacing: 10,
            hooks: Address::ZERO,
        };
        let swap_params = SwapParams {
            zero_for_one: true,
            amount_specified: I256::from_raw(U256::from(1_u64)),
            sqrt_price_limit_x96: U256::from(1_u64),
            hook_data: Bytes::new(),
        };

        let encoded = encode_v4_swap_call(&pool_key, &swap_params, Bytes::new());
        assert!(encoded.is_err());
    }
}
