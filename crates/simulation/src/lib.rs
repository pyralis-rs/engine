//! Simulation layer for local EVM execution and state modeling.

pub mod engine;
pub mod gas;
pub mod state;
pub mod v4;

pub use engine::{SimulationEngine, SimulationResult};
pub use gas::estimate_total_gas_cost;
pub use state::{CachedStateProvider, LazyStateProvider, StateBackend};
