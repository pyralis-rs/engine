//! Reconnection backoff and provider liveness tracking.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing::{error, info, warn};

/// Metric event emitted on provider disconnect and reconnect signals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconnectMetricEvent {
    /// Provider identifier associated with the event.
    pub provider: String,
    /// Event kind.
    pub kind: ReconnectMetricKind,
    /// Reconnect attempt number.
    pub attempt: u32,
}

/// Metric event kind emitted by [`ReconnectionHandler`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconnectMetricKind {
    /// Provider disconnected.
    Disconnect,
    /// Provider reconnected.
    Reconnect,
}

/// Sink trait for reconnection metric events.
pub trait MetricSink: Send + Sync {
    /// Emits a metric event.
    fn emit(&self, event: ReconnectMetricEvent);
}

/// No-op metric sink.
#[derive(Debug, Default)]
pub struct NoopMetricSink;

impl MetricSink for NoopMetricSink {
    fn emit(&self, _event: ReconnectMetricEvent) {}
}

#[derive(Debug, Clone, Copy, Default)]
struct ProviderState {
    connected: bool,
    attempt: u32,
}

/// Handles provider disconnect/reconnect lifecycle and backoff strategy.
pub struct ReconnectionHandler {
    provider_states: Mutex<HashMap<String, ProviderState>>,
    metrics: Arc<dyn MetricSink>,
}

impl ReconnectionHandler {
    /// Creates a new handler for the given provider names.
    pub fn new(
        provider_names: impl IntoIterator<Item = String>,
        metrics: Arc<dyn MetricSink>,
    ) -> Self {
        let provider_states = provider_names
            .into_iter()
            .map(|name| (name, ProviderState::default()))
            .collect();

        Self {
            provider_states: Mutex::new(provider_states),
            metrics,
        }
    }

    /// Creates a new handler using the no-op metric sink.
    pub fn new_noop(provider_names: impl IntoIterator<Item = String>) -> Self {
        Self::new(provider_names, Arc::new(NoopMetricSink))
    }

    /// Marks a provider as disconnected and emits a disconnect metric.
    pub fn record_disconnect(&self, provider: &str) {
        if let Ok(mut states) = self.provider_states.lock() {
            let state = states.entry(provider.to_string()).or_default();
            state.connected = false;
            self.metrics.emit(ReconnectMetricEvent {
                provider: provider.to_string(),
                kind: ReconnectMetricKind::Disconnect,
                attempt: state.attempt,
            });
            warn!("provider disconnected: {provider}");

            if states.values().all(|value| !value.connected) {
                error!("all providers are down; entering degraded mode");
            }
        }
    }

    /// Marks a provider as connected and resets the retry counter.
    pub fn record_reconnect(&self, provider: &str) {
        if let Ok(mut states) = self.provider_states.lock() {
            let state = states.entry(provider.to_string()).or_default();
            state.connected = true;
            state.attempt = 0;
            self.metrics.emit(ReconnectMetricEvent {
                provider: provider.to_string(),
                kind: ReconnectMetricKind::Reconnect,
                attempt: 0,
            });
            info!("provider reconnected: {provider}");
        }
    }

    /// Computes and records the next backoff duration for a provider.
    pub fn next_backoff(&self, provider: &str) -> Duration {
        if let Ok(mut states) = self.provider_states.lock() {
            let state = states.entry(provider.to_string()).or_default();
            state.attempt = state.attempt.saturating_add(1);
            let delay = Self::backoff_for_attempt(state.attempt);
            info!(
                "reconnect attempt {} for provider {} in {:?}",
                state.attempt, provider, delay
            );
            return delay;
        }

        Duration::from_secs(1)
    }

    /// Returns `true` when all providers are disconnected.
    pub fn is_degraded_mode(&self) -> bool {
        if let Ok(states) = self.provider_states.lock() {
            return !states.is_empty() && states.values().all(|value| !value.connected);
        }
        false
    }

    /// Returns exponential backoff with 30 seconds max cap.
    pub fn backoff_for_attempt(attempt: u32) -> Duration {
        let attempt = attempt.max(1);
        let seconds = 2u64.saturating_pow(attempt.saturating_sub(1)).min(30);
        Duration::from_secs(seconds)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::{MetricSink, ReconnectMetricEvent, ReconnectMetricKind, ReconnectionHandler};

    #[derive(Default)]
    struct RecordingMetricSink {
        events: Mutex<Vec<ReconnectMetricEvent>>,
    }

    impl MetricSink for RecordingMetricSink {
        fn emit(&self, event: ReconnectMetricEvent) {
            if let Ok(mut events) = self.events.lock() {
                events.push(event);
            }
        }
    }

    #[test]
    fn test_reconnection_backoff_timing() {
        assert_eq!(
            ReconnectionHandler::backoff_for_attempt(1),
            Duration::from_secs(1)
        );
        assert_eq!(
            ReconnectionHandler::backoff_for_attempt(2),
            Duration::from_secs(2)
        );
        assert_eq!(
            ReconnectionHandler::backoff_for_attempt(3),
            Duration::from_secs(4)
        );
        assert_eq!(
            ReconnectionHandler::backoff_for_attempt(4),
            Duration::from_secs(8)
        );
        assert_eq!(
            ReconnectionHandler::backoff_for_attempt(8),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn test_degraded_mode_when_all_providers_disconnect() {
        let metrics = Arc::new(RecordingMetricSink::default());
        let handler = ReconnectionHandler::new(
            vec!["provider-a".to_string(), "provider-b".to_string()],
            metrics,
        );

        handler.record_reconnect("provider-a");
        handler.record_reconnect("provider-b");
        assert!(!handler.is_degraded_mode());

        handler.record_disconnect("provider-a");
        assert!(!handler.is_degraded_mode());
        handler.record_disconnect("provider-b");
        assert!(handler.is_degraded_mode());
    }

    #[test]
    fn test_metric_events_emitted_on_disconnect_and_reconnect() {
        let metrics = Arc::new(RecordingMetricSink::default());
        let handler = ReconnectionHandler::new(
            vec!["provider-a".to_string()],
            Arc::clone(&metrics) as Arc<dyn MetricSink>,
        );

        handler.record_disconnect("provider-a");
        handler.record_reconnect("provider-a");

        let events = metrics.events.lock().ok().map(|value| value.clone());
        assert!(events.is_some());
        let events = events.unwrap_or_default();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, ReconnectMetricKind::Disconnect);
        assert_eq!(events[1].kind, ReconnectMetricKind::Reconnect);
    }
}
