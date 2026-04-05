use std::time::Duration;

/// Thin progress wrapper used by CLI operations.
pub struct Progress;

/// Spinner handle wrapping indicatif progress operations.
pub struct Spinner {
    inner: indicatif::ProgressBar,
}

impl Progress {
    /// Create and start a spinner with an initial message.
    pub fn spinner(message: impl Into<String>) -> Spinner {
        let inner = indicatif::ProgressBar::new_spinner();
        inner.enable_steady_tick(Duration::from_millis(80));
        inner.set_message(message.into());
        Spinner { inner }
    }
}

impl Spinner {
    /// Update the spinner message.
    pub fn set_message(&self, message: impl Into<String>) {
        self.inner.set_message(message.into());
    }

    /// Stop the spinner and print a final message.
    pub fn finish_with_message(&self, message: impl Into<String>) {
        self.inner.finish_with_message(message.into());
    }
}
