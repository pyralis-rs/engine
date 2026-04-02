//! Simulation layer for local EVM execution and state modeling.

pub mod engine;
pub mod state;

pub use engine::{SimulationEngine, SimulationResult};
pub use state::{CachedStateProvider, LazyStateProvider, StateBackend};
