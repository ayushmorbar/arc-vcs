use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context as _;

/// Shared interrupt state for graceful shutdown coordination.
#[derive(Clone, Debug)]
pub struct InterruptState {
    interrupted: Arc<AtomicBool>,
}

impl InterruptState {
    /// Create a new non-interrupted state.
    #[must_use]
    pub fn new() -> Self {
        Self { interrupted: Arc::new(AtomicBool::new(false)) }
    }

    /// Mark process state as interrupted.
    pub fn mark_interrupted(&self) {
        self.interrupted.store(true, Ordering::SeqCst);
    }

    /// Return `true` if an interrupt signal was observed.
    #[must_use]
    pub fn is_interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }
}

impl Default for InterruptState {
    fn default() -> Self {
        Self::new()
    }
}

/// Install interrupt cleanup handlers and wire them to shared interrupt state.
pub fn install_cleanup_handlers(state: InterruptState) -> anyhow::Result<()> {
    let ctrlc_state = state.clone();
    ctrlc::set_handler(move || {
        ctrlc_state.mark_interrupted();
        arc_store_view::tempfile::cleanup_signal_safe();
    })
    .context("failed to install Ctrl+C cleanup handler")?;

    #[cfg(unix)]
    {
        use signal_hook::consts::signal::SIGTERM;
        use signal_hook::iterator::Signals;

        let mut signals =
            Signals::new([SIGTERM]).context("failed to register SIGTERM cleanup handler")?;
        let signal_state = state.clone();
        std::thread::Builder::new()
            .name("arc-sigterm-cleanup".to_string())
            .spawn(move || {
                #[allow(clippy::never_loop)]
                for _ in signals.forever() {
                    signal_state.mark_interrupted();
                    arc_store_view::tempfile::cleanup_signal_safe();
                    std::process::exit(143);
                }
            })
            .context("failed to spawn SIGTERM cleanup thread")?;
    }

    Ok(())
}
