//! Ingestion layer for collecting block and mempool data from chain providers.

pub mod provider;
pub mod reconnect;
pub mod stream;

pub use provider::AlloyProvider;
pub use reconnect::{
    MetricSink, NoopMetricSink, ReconnectMetricEvent, ReconnectMetricKind, ReconnectionHandler,
};
pub use stream::{ProviderRegistration, StreamManager};
