use std::time::Instant;

use crate::devtools::interrupt::InterruptState;
use crate::progress::Progress;

/// Execute CLI work with shared telemetry and progress lifecycle.
pub fn run_with_telemetry<F>(mode: &str, interrupts: &InterruptState, run: F) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()>,
{
    let spinner = Progress::spinner(format!("running {mode}"));
    let start = Instant::now();
    tracing::info!(mode, "cli run started");

    let result = run();
    let elapsed = start.elapsed();

    match &result {
        Ok(()) => {
            if interrupts.is_interrupted() {
                spinner.finish_with_message("interrupted");
            } else {
                spinner.finish_with_message("done");
            }
            tracing::info!(mode, elapsed_ms = elapsed.as_millis(), "cli run completed");
        }
        Err(error) => {
            spinner.finish_with_message("failed");
            tracing::error!(mode, elapsed_ms = elapsed.as_millis(), error = %error, "cli run failed");
        }
    }

    result
}
