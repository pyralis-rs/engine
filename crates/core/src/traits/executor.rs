//! Execution abstraction for dry-run and live transaction submission.

use crate::error::Result;
use crate::types::{ExecutionPlan, ExecutionResult};

/// Executes an execution plan and reports outcome metadata.
#[allow(async_fn_in_trait)]
pub trait Executor: Send + Sync {
    /// Executes the plan.
    async fn execute(&self, plan: &ExecutionPlan) -> Result<ExecutionResult>;
}
