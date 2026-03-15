//! Error types used by Pyralis crates.

use alloy::transports::TransportError;
use thiserror::Error;

/// Shared error type for all pipeline stages.
#[derive(Debug, Error)]
pub enum PyralisError {
    /// Returned when loading or validating configuration fails.
    #[error("configuration error: {0}")]
    Config(String),
    /// Returned when fetching data from an external provider fails.
    #[error("provider error: {0}")]
    Provider(String),
    /// Returned when simulation fails to execute.
    #[error("simulation error: {0}")]
    Simulation(String),
    /// Returned when strategy evaluation fails.
    #[error("strategy error: {0}")]
    Strategy(String),
    /// Returned when transaction execution fails.
    #[error("execution error: {0}")]
    Execution(String),
    /// Returned when filesystem I/O fails.
    #[error("io error: {0}")]
    Io(#[source] std::io::Error),
    /// Returned when ABI encoding or decoding fails.
    #[error("abi error: {0}")]
    Abi(String),
    /// Returned when an unexpected internal failure happens.
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<std::io::Error> for PyralisError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<TransportError> for PyralisError {
    fn from(value: TransportError) -> Self {
        Self::Provider(format!("transport failure: {value}"))
    }
}

impl From<alloy::contract::Error> for PyralisError {
    fn from(value: alloy::contract::Error) -> Self {
        Self::Abi(format!("contract ABI error: {value}"))
    }
}

/// Convenience result alias for crate-level APIs.
pub type Result<T> = std::result::Result<T, PyralisError>;
