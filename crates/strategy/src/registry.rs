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
