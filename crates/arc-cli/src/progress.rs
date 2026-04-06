use std::time::Duration;

/// Thin progress wrapper used by CLI operations.
pub struct Progress;

/// Spinner handle wrapping indicatif progress operations.
pub struct Spinner {
    inner: indicatif::ProgressBar,
}

/// Fixed-order 5-stage pipeline phases for sync UX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineStage {
    /// Discovering graph delta and local/remote frontier context.
    Discover,
    /// Negotiating credentials/capabilities with remote.
    Negotiate,
    /// Transferring blobs and delta payloads.
    Transfer,
    /// Materializing AST-level state from transferred content.
    Materialize,
    /// Finalizing DAG/frontier updates and commit status.
    Finalize,
}

impl PipelineStage {
    fn index(self) -> usize {
        match self {
            Self::Discover => 0,
            Self::Negotiate => 1,
            Self::Transfer => 2,
            Self::Materialize => 3,
            Self::Finalize => 4,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Discover => "Discovering graph delta...",
            Self::Negotiate => "Negotiating with remote...",
            Self::Transfer => "Transferring blobs...",
            Self::Materialize => "Materializing AST...",
            Self::Finalize => "Finalizing DAG...",
        }
    }
}

/// Multi-line TTY pipeline renderer for sync operations.
pub struct SyncPipeline {
    bars: Vec<indicatif::ProgressBar>,
    _multi: indicatif::MultiProgress,
}

impl Progress {
    /// Create and start a spinner with an initial message.
    pub fn spinner(message: impl Into<String>) -> Spinner {
        let inner = indicatif::ProgressBar::new_spinner();
        inner.enable_steady_tick(Duration::from_millis(80));
        inner.set_message(message.into());
        Spinner { inner }
    }

    /// Build a 5-stage sync pipeline renderer.
    pub fn sync_pipeline() -> SyncPipeline {
        SyncPipeline::new()
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

impl SyncPipeline {
    fn new() -> Self {
        let multi = indicatif::MultiProgress::new();
        let mut bars = Vec::new();

        for stage in [
            PipelineStage::Discover,
            PipelineStage::Negotiate,
            PipelineStage::Transfer,
            PipelineStage::Materialize,
            PipelineStage::Finalize,
        ] {
            let pb = multi.add(indicatif::ProgressBar::new_spinner());
            pb.set_style(
                indicatif::ProgressStyle::with_template("{prefix} {spinner} {msg}")
                    .expect("valid progress style")
                    .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ "),
            );
            pb.set_prefix("[ ]");
            pb.set_message(stage.label().to_string());
            bars.push(pb);
        }

        Self {
            bars,
            _multi: multi,
        }
    }

    /// Mark a stage as currently running.
    pub fn start_stage(&self, stage: PipelineStage) {
        let bar = &self.bars[stage.index()];
        bar.set_prefix("[⠧]");
        bar.enable_steady_tick(Duration::from_millis(70));
    }

    /// Mark a stage as completed.
    pub fn finish_stage(&self, stage: PipelineStage) {
        let bar = &self.bars[stage.index()];
        bar.finish_and_clear();
        bar.set_prefix("[✔]");
        bar.set_message(stage.label().to_string());
        bar.println(format!("[✔] {}", stage.label()));
    }

    /// Mark a stage as failed and stop rendering.
    pub fn fail_stage(&self, stage: PipelineStage, message: &str) {
        let bar = &self.bars[stage.index()];
        bar.abandon_with_message(format!("[✖] {message}"));
    }
}
