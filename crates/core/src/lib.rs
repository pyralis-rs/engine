//! Shared types, traits, and errors used across the Pyralis pipeline.

pub mod error;
pub mod types;

pub use error::{PyralisError, Result};
pub use types::{
    BlockInfo, ExecutionPlan, ExecutionResult, Opportunity, PoolState, SimulationContext,
};
