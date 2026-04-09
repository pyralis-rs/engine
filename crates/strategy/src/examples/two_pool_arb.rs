//! Example strategy that looks for simple two-pool price dislocations.

use alloy::primitives::U256;
use pyralis_core::error::Result;
use pyralis_core::traits::Strategy;
use pyralis_core::types::{Opportunity, PoolState, SimulationContext};

/// Example arbitrage strategy comparing two pools for the same pair.
#[derive(Debug, Clone)]
pub struct TwoPoolArbStrategy {
    min_price_diff_bps: u32,
    min_profit_threshold: U256,
    notional_in_wei: U256,
    estimated_gas_cost_wei: U256,
}

impl Default for TwoPoolArbStrategy {
    fn default() -> Self {
        Self {
            min_price_diff_bps: 50,
            min_profit_threshold: U256::from(500_000_000_000_000_u64),
            notional_in_wei: U256::from(1_000_000_000_000_000_000_u64),
            estimated_gas_cost_wei: U256::from(200_000_000_000_000_u64),
        }
    }
}

impl TwoPoolArbStrategy {
    /// Creates a strategy with custom thresholds.
    pub fn new(
        min_price_diff_bps: u32,
        min_profit_threshold: U256,
        notional_in_wei: U256,
        estimated_gas_cost_wei: U256,
    ) -> Self {
        Self {
            min_price_diff_bps,
            min_profit_threshold,
            notional_in_wei,
            estimated_gas_cost_wei,
        }
    }

    fn evaluate_pair(
        &self,
        pool_a: &PoolState,
        pool_b: &PoolState,
        timestamp: u64,
    ) -> Option<Opportunity> {
        if !same_pair(pool_a, pool_b) {
            return None;
        }

        let price_a = sqrt_price_to_price(pool_a.sqrt_price_x96);
        let price_b = sqrt_price_to_price(pool_b.sqrt_price_x96);
        if price_a <= 0.0 || price_b <= 0.0 {
            return None;
        }

        let (cheap_pool, expensive_pool, diff_bps) = if price_a <= price_b {
            (
                pool_a,
                pool_b,
                ((price_b - price_a) / price_a * 10_000.0).round() as u64,
            )
        } else {
            (
                pool_b,
                pool_a,
                ((price_a - price_b) / price_b * 10_000.0).round() as u64,
            )
        };

        if diff_bps < u64::from(self.min_price_diff_bps) {
            return None;
        }

        let gross_profit = estimate_gross_profit(self.notional_in_wei, diff_bps);
        if gross_profit <= self.estimated_gas_cost_wei {
            return None;
        }

        let net_profit = gross_profit - self.estimated_gas_cost_wei;
        if net_profit <= self.min_profit_threshold {
            return None;
        }

        let confidence = ((diff_bps as f64) / 500.0).clamp(0.0, 1.0);
        Some(Opportunity {
            strategy_name: self.name().to_string(),
            estimated_profit: net_profit,
            confidence,
            pools_involved: vec![cheap_pool.address, expensive_pool.address],
            timestamp,
        })
    }
}

impl Strategy for TwoPoolArbStrategy {
    fn name(&self) -> &str {
        "two_pool_arb"
    }

    fn evaluate(&self, context: &SimulationContext) -> Result<Vec<Opportunity>> {
        let mut opportunities = Vec::new();
        for (left_index, left_pool) in context.pool_states.iter().enumerate() {
            for right_pool in context.pool_states.iter().skip(left_index + 1) {
                if let Some(opportunity) =
                    self.evaluate_pair(left_pool, right_pool, context.block.timestamp)
                {
                    opportunities.push(opportunity);
                }
            }
        }
        Ok(opportunities)
    }
}

fn same_pair(left: &PoolState, right: &PoolState) -> bool {
    (left.token0 == right.token0 && left.token1 == right.token1)
        || (left.token0 == right.token1 && left.token1 == right.token0)
}

fn sqrt_price_to_price(sqrt_price_x96: U256) -> f64 {
    let sqrt = u256_to_f64(sqrt_price_x96);
    if sqrt <= 0.0 {
        return 0.0;
    }

    let q96 = 2_f64.powi(96);
    let normalized = sqrt / q96;
    normalized * normalized
}

fn estimate_gross_profit(notional_in_wei: U256, diff_bps: u64) -> U256 {
    notional_in_wei.saturating_mul(U256::from(diff_bps)) / U256::from(10_000_u64)
}

fn u256_to_f64(value: U256) -> f64 {
    value.to_string().parse::<f64>().unwrap_or(0.0)
}
