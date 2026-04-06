//! Internal CLI runtime tooling for diagnostics, orchestration, and testing.

/// Graceful interrupt state and signal handler wiring.
pub mod interrupt;
/// Multicall executable-stem dispatch helpers.
pub mod multicall;
/// Shared telemetry/progress run wrapper for CLI execution.
pub mod run_wrapper;
