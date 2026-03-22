//! Ingestion layer for collecting block and mempool data from chain providers.

pub mod provider;
pub mod stream;

pub use provider::AlloyProvider;
pub use stream::{ProviderRegistration, StreamManager};
