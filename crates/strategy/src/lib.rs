//! Strategy layer for evaluating simulations and producing opportunities.

pub mod examples;
pub mod registry;

pub use examples::two_pool_arb::TwoPoolArbStrategy;
pub use registry::StrategyRegistry;
