//! Strategy abstraction for MEV opportunity detection.

use crate::error::Result;
use crate::types::{Opportunity, SimulationContext};

/// Evaluates simulation context and returns candidate opportunities.
pub trait Strategy: Send + Sync {
    /// Returns the unique strategy name.
    fn name(&self) -> &str;

    /// Evaluates the context and returns discovered opportunities.
    fn evaluate(&self, context: &SimulationContext) -> Result<Vec<Opportunity>>;
}
