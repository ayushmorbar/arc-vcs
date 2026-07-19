#![warn(missing_docs)]

//! Structured diagnostics and actionable hints for arc commands.

use std::{
    error::Error as StdError,
    fmt::{Display, Formatter},
    sync::OnceLock,
};

use tracing_subscriber::EnvFilter;

static TRACING_INIT: OnceLock<()> = OnceLock::new();

#[cfg(not(target_arch = "wasm32"))]
pub mod native;

/// Initialize tracing from ARC_* environment variables.
///
/// Behavior:
/// - `ARC_TRACE_EVENT=<path>`: append JSON event stream to file.
/// - `ARC_TRACE=1`: compact stderr tracing.
/// - otherwise: no subscriber installed (zero-overhead default).
pub fn init_tracing(service_name: &str) {
    let _ = TRACING_INIT.get_or_init(|| {
        let filter = std::env::var("ARC_TRACE_FILTER")
            .unwrap_or_else(|_| format!("{service_name}=debug,info"));

        if let Ok(path) = std::env::var("ARC_TRACE_EVENT") {
            if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = tracing_subscriber::fmt()
                    .json()
                    .with_writer(std::sync::Mutex::new(file))
                    .with_env_filter(EnvFilter::new(filter))
                    .try_init();
            }
            return;
        }

        if std::env::var("ARC_TRACE").is_ok_and(|value| value == "1") {
            let _ = tracing_subscriber::fmt()
                .compact()
                .with_env_filter(EnvFilter::new(filter))
                .try_init();
        }
    });
}

/// Actionable hint attached to a command failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArcHint {
    explanation: String,
    suggested_command: Option<String>,
}

impl ArcHint {
    /// Create a hint with a human-readable explanation.
    pub fn new(explanation: impl Into<String>) -> Self {
        Self { explanation: explanation.into(), suggested_command: None }
    }

    /// Add a suggested follow-up command.
    pub fn with_suggested_command(mut self, suggested_command: impl Into<String>) -> Self {
        self.suggested_command = Some(suggested_command.into());
        self
    }

    /// Human-readable explanation.
    pub fn explanation(&self) -> &str {
        &self.explanation
    }

    /// Optional suggested command.
    pub fn suggested_command(&self) -> Option<&str> {
        self.suggested_command.as_deref()
    }
}

/// Render-ready error model for arc CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArcError {
    message: String,
    causes: Vec<String>,
    hint: Option<ArcHint>,
}

impl ArcError {
    /// Build a render-ready model from an [`anyhow::Error`].
    pub fn from_anyhow(error: &anyhow::Error) -> Self {
        let message = error.to_string();
        let mut causes = Vec::new();
        let mut previous: Option<String> = Some(message.clone());
        for cause in error.chain().skip(1) {
            if cause.downcast_ref::<HintedError>().is_some() {
                continue;
            }
            let rendered = cause.to_string();
            if previous.as_deref() == Some(rendered.as_str()) {
                continue;
            }
            previous = Some(rendered.clone());
            causes.push(rendered);
        }

        Self { message, causes, hint: extract_hint(error) }
    }

    /// Core error message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Ordered causal chain, nearest cause first.
    pub fn causes(&self) -> &[String] {
        &self.causes
    }

    /// Optional attached hint.
    pub fn hint(&self) -> Option<&ArcHint> {
        self.hint.as_ref()
    }
}

#[derive(Debug)]
struct HintedError {
    source: anyhow::Error,
    hint: ArcHint,
}

impl Display for HintedError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.source)
    }
}

impl StdError for HintedError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        Some(self.source.as_ref())
    }
}

fn extract_hint(error: &anyhow::Error) -> Option<ArcHint> {
    for cause in error.chain() {
        if let Some(hinted) = cause.downcast_ref::<HintedError>() {
            return Some(hinted.hint.clone());
        }
    }
    None
}

fn attach_hint(error: anyhow::Error, hint: ArcHint) -> anyhow::Error {
    if extract_hint(&error).is_some() {
        return error;
    }
    anyhow::Error::new(HintedError { source: error, hint })
}

/// Extension trait for attaching structured hints to fallible results.
pub trait ResultExt<T> {
    /// Attach a hint explanation to an error.
    fn with_hint(self, explanation: impl Into<String>) -> anyhow::Result<T>;

    /// Attach a hint explanation and suggested command to an error.
    fn with_hint_command(
        self,
        explanation: impl Into<String>,
        suggested_command: impl Into<String>,
    ) -> anyhow::Result<T>;
}

impl<T, E> ResultExt<T> for Result<T, E>
where
    E: Into<anyhow::Error>,
{
    fn with_hint(self, explanation: impl Into<String>) -> anyhow::Result<T> {
        let hint = ArcHint::new(explanation);
        self.map_err(|error| attach_hint(error.into(), hint))
    }

    fn with_hint_command(
        self,
        explanation: impl Into<String>,
        suggested_command: impl Into<String>,
    ) -> anyhow::Result<T> {
        let hint = ArcHint::new(explanation).with_suggested_command(suggested_command);
        self.map_err(|error| attach_hint(error.into(), hint))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_ext_ok_passthrough() {
        let ok: Result<u32, std::io::Error> = Ok(7);
        let out = ok.with_hint("unused").expect("ok should pass through");
        assert_eq!(out, 7);
    }

    #[test]
    fn result_ext_err_attaches_hint() {
        let err: Result<(), std::io::Error> = Err(std::io::Error::other("disk failed"));
        let out = err
            .with_hint_command("Check write permissions.", "arc doctor")
            .expect_err("error expected");
        let modeled = ArcError::from_anyhow(&out);
        assert_eq!(modeled.message(), "disk failed");
        assert_eq!(modeled.hint().map(|hint| hint.explanation()), Some("Check write permissions."));
        assert_eq!(modeled.hint().and_then(|hint| hint.suggested_command()), Some("arc doctor"));
    }

    #[test]
    fn existing_hint_is_preserved() {
        let out: anyhow::Result<()> = Err(anyhow::anyhow!("root"));
        let out = out
            .with_hint_command("First", "arc one")
            .with_hint_command("Second", "arc two")
            .expect_err("error expected");
        let modeled = ArcError::from_anyhow(&out);
        assert_eq!(modeled.hint().map(|hint| hint.explanation()), Some("First"));
    }

    #[test]
    fn from_anyhow_collects_and_deduplicates_causes() {
        let err = anyhow::anyhow!("disk write failed").context("cannot persist checkpoint");
        let modeled = ArcError::from_anyhow(&err);
        assert_eq!(modeled.message(), "cannot persist checkpoint");
        assert_eq!(modeled.causes(), &["disk write failed".to_string()]);
    }

    #[test]
    fn from_anyhow_omits_hint_wrapper_from_causes() {
        let err: anyhow::Result<()> = Err(anyhow::anyhow!("restack paused"));
        let err = err
            .with_hint_command("Resolve conflicts first.", "arc restack --continue")
            .expect_err("error expected");
        let modeled = ArcError::from_anyhow(&err);
        assert_eq!(modeled.message(), "restack paused");
        assert!(modeled.causes().is_empty());
    }

    #[test]
    fn tracing_init_is_idempotent() {
        init_tracing("arc_diagnostics_test");
        init_tracing("arc_diagnostics_test");
    }
}
