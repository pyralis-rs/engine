//! Registry for coordinating multiple strategy implementations.

use std::sync::Arc;
use std::time::Instant;

use pyralis_core::traits::Strategy;
use pyralis_core::types::{Opportunity, SimulationContext};
use tokio::task::JoinSet;
use tracing::{info, warn};

/// Stores and executes registered strategies.
#[derive(Default)]
pub struct StrategyRegistry {
    strategies: Vec<Arc<dyn Strategy + Send + Sync>>,
}

impl StrategyRegistry {
    /// Creates an empty strategy registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a strategy implementation.
    pub fn register<S>(&mut self, strategy: S)
    where
        S: Strategy + Send + Sync + 'static,
    {
        self.strategies.push(Arc::new(strategy));
    }

    /// Returns registered strategy names.
    pub fn list_strategies(&self) -> Vec<&str> {
        self.strategies
            .iter()
            .map(|strategy| strategy.name())
            .collect()
    }

    /// Executes all strategies in parallel and returns merged opportunities sorted by profit.
    pub async fn evaluate_all(&self, context: &SimulationContext) -> Vec<Opportunity> {
        let context = Arc::new(context.clone());
        let mut join_set = JoinSet::new();

        for strategy in &self.strategies {
            let strategy = Arc::clone(strategy);
            let context = Arc::clone(&context);

            join_set.spawn(async move {
                let strategy_name = strategy.name().to_string();
                let started_at = Instant::now();
                let result = strategy.evaluate(&context);
                let elapsed = started_at.elapsed().as_micros();

                match result {
                    Ok(opportunities) => {
                        info!(
                            strategy = %strategy_name,
                            elapsed_micros = elapsed,
                            opportunities = opportunities.len(),
                            "strategy evaluation complete"
                        );
                        opportunities
                    }
                    Err(error) => {
                        warn!(
                            strategy = %strategy_name,
                            elapsed_micros = elapsed,
                            error = %error,
                            "strategy evaluation failed"
                        );
                        Vec::new()
                    }
                }
            });
        }

        let mut opportunities = Vec::new();
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(mut current) => opportunities.append(&mut current),
                Err(error) => {
                    warn!(error = %error, "strategy task failed");
                }
            }
        }

        opportunities.sort_by(|left, right| right.estimated_profit.cmp(&left.estimated_profit));
        opportunities
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use alloy::primitives::{Address, B256, U256};
    use pyralis_core::error::{PyralisError, Result};
    use pyralis_core::traits::Strategy;
    use pyralis_core::types::{BlockInfo, Opportunity, SimulationContext};

    use super::StrategyRegistry;

    struct MockStrategy {
        name: &'static str,
        profits: Vec<U256>,
        should_fail: bool,
        sleep_millis: u64,
    }

    impl Strategy for MockStrategy {
        fn name(&self) -> &str {
            self.name
        }

        fn evaluate(&self, context: &SimulationContext) -> Result<Vec<Opportunity>> {
            if self.sleep_millis > 0 {
                std::thread::sleep(Duration::from_millis(self.sleep_millis));
            }

            if self.should_fail {
                return Err(PyralisError::Strategy("mock failure".to_string()));
            }

            Ok(self
                .profits
                .iter()
                .copied()
                .map(|profit| Opportunity {
                    strategy_name: self.name.to_string(),
                    estimated_profit: profit,
                    confidence: 0.9,
                    pools_involved: vec![Address::repeat_byte(0x11)],
                    timestamp: context.block.timestamp,
                })
                .collect())
        }
    }

    fn simulation_context() -> SimulationContext {
        SimulationContext {
            block: BlockInfo {
                number: 1,
                hash: B256::repeat_byte(0x01),
                timestamp: 1_700_000_000,
                base_fee: Some(1),
            },
            pool_states: Vec::new(),
            pending_txs: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_registry_list_strategies() {
        let mut registry = StrategyRegistry::new();
        registry.register(MockStrategy {
            name: "s1",
            profits: vec![],
            should_fail: false,
            sleep_millis: 0,
        });
        registry.register(MockStrategy {
            name: "s2",
            profits: vec![],
            should_fail: false,
            sleep_millis: 0,
        });

        let names = registry.list_strategies();
        assert_eq!(names, vec!["s1", "s2"]);
    }

    #[tokio::test]
    async fn test_registry_evaluate_all_merges_and_sorts_desc() {
        let mut registry = StrategyRegistry::new();
        registry.register(MockStrategy {
            name: "slow",
            profits: vec![U256::from(2_u64)],
            should_fail: false,
            sleep_millis: 30,
        });
        registry.register(MockStrategy {
            name: "fast",
            profits: vec![U256::from(10_u64), U256::from(5_u64)],
            should_fail: false,
            sleep_millis: 0,
        });

        let context = simulation_context();
        let opportunities = registry.evaluate_all(&context).await;

        assert_eq!(opportunities.len(), 3);
        assert_eq!(opportunities[0].estimated_profit, U256::from(10_u64));
        assert_eq!(opportunities[1].estimated_profit, U256::from(5_u64));
        assert_eq!(opportunities[2].estimated_profit, U256::from(2_u64));
    }

    #[tokio::test]
    async fn test_registry_evaluate_all_skips_failed_strategy() {
        let mut registry = StrategyRegistry::new();
        registry.register(MockStrategy {
            name: "ok",
            profits: vec![U256::from(7_u64)],
            should_fail: false,
            sleep_millis: 0,
        });
        registry.register(MockStrategy {
            name: "fail",
            profits: vec![U256::from(99_u64)],
            should_fail: true,
            sleep_millis: 0,
        });

        let context = simulation_context();
        let opportunities = registry.evaluate_all(&context).await;

        assert_eq!(opportunities.len(), 1);
        assert_eq!(opportunities[0].strategy_name, "ok");
        assert_eq!(opportunities[0].estimated_profit, U256::from(7_u64));
    }
}
