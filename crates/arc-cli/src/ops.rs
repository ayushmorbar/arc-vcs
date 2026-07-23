//! Shared operation-stage taxonomy for tracing and reporting.

use std::time::{Duration, Instant};

use tracing::{info, info_span, warn};

/// Default sync-cycle SLO threshold in milliseconds.
pub const DEFAULT_SYNC_SLO_MS: u64 = 500;

/// Canonical lifecycle stages for heavy operations in Arc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationStage {
    /// Identify candidate objects and preconditions.
    Discover,
    /// Resolve strategy/capabilities before data movement.
    Negotiate,
    /// Move bytes or records across subsystem boundaries.
    Transfer,
    /// Build or decode typed state from transferred payloads.
    Materialize,
    /// Commit results and emit terminal status.
    Finalize,
}

impl OperationStage {
    /// Stable lowercase token for logs, traces, and metrics dimensions.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discover => "discover",
            Self::Negotiate => "negotiate",
            Self::Transfer => "transfer",
            Self::Materialize => "materialize",
            Self::Finalize => "finalize",
        }
    }
}

impl core::fmt::Display for OperationStage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// End-to-end latency timer for CRDT synchronization cycles.
pub struct SloTimer {
    operation: String,
    threshold: Duration,
    started_at: Instant,
}

impl SloTimer {
    /// Build a new SLO timer for one named sync operation.
    pub fn new(operation: impl Into<String>, threshold: Duration) -> Self {
        Self { operation: operation.into(), threshold, started_at: Instant::now() }
    }

    /// Build a timer from `ARC_SYNC_SLO_MS` environment override.
    pub fn from_env(operation: impl Into<String>) -> Self {
        let threshold_ms = std::env::var("ARC_SYNC_SLO_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_SYNC_SLO_MS);
        Self::new(operation, Duration::from_millis(threshold_ms))
    }

    /// Execute `run` inside a stage-tagged span.
    pub fn stage<T>(&self, stage: OperationStage, run: impl FnOnce() -> T) -> T {
        let span = info_span!(
            "arc_core.sync_cycle.stage",
            operation = %self.operation,
            stage = %stage
        );
        span.in_scope(run)
    }

    /// Emit SLO status and return total elapsed duration.
    pub fn finish(self) -> Duration {
        let elapsed = self.started_at.elapsed();
        let elapsed_ms = elapsed.as_millis() as u64;
        let threshold_ms = self.threshold.as_millis() as u64;

        if elapsed > self.threshold {
            warn!(
                operation = %self.operation,
                elapsed_ms,
                threshold_ms,
                "CRDT sync cycle exceeded latency SLO"
            );
        } else {
            info!(
                operation = %self.operation,
                elapsed_ms,
                threshold_ms,
                "CRDT sync cycle finished within latency SLO"
            );
        }

        elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_tokens_are_stable_and_lowercase() {
        let matrix = [
            (OperationStage::Discover, "discover"),
            (OperationStage::Negotiate, "negotiate"),
            (OperationStage::Transfer, "transfer"),
            (OperationStage::Materialize, "materialize"),
            (OperationStage::Finalize, "finalize"),
        ];

        for (stage, expected) in matrix {
            assert_eq!(stage.as_str(), expected);
            assert_eq!(stage.to_string(), expected);
        }
    }

    #[test]
    fn slo_timer_finish_returns_duration() {
        let timer = SloTimer::new("test", Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
        let elapsed = timer.finish();
        assert!(elapsed.as_millis() >= 10, "elapsed must be at least 10ms");
    }

    #[test]
    fn slo_timer_from_env_uses_default() {
        let timer = SloTimer::from_env("env-test");
        assert_eq!(timer.threshold, Duration::from_millis(DEFAULT_SYNC_SLO_MS));
    }

    #[test]
    fn slo_timer_stage_runs_closure() {
        let timer = SloTimer::new("stage-test", Duration::from_secs(10));
        let result = timer.stage(OperationStage::Transfer, || 42);
        assert_eq!(result, 42);
    }

    #[test]
    fn slo_timer_all_stages() {
        let stages = [
            OperationStage::Discover,
            OperationStage::Negotiate,
            OperationStage::Transfer,
            OperationStage::Materialize,
            OperationStage::Finalize,
        ];
        for stage in stages {
            let timer = SloTimer::new("all-stages", Duration::from_secs(10));
            let result = timer.stage(stage, || stage.as_str());
            assert!(!result.is_empty());
        }
    }

    #[test]
    fn default_slo_ms_constant() {
        assert_eq!(DEFAULT_SYNC_SLO_MS, 500);
    }
}
