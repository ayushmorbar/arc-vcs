use miette::Report;

/// Shared output event protocol for CLI/TUI renderers.
pub enum OutputEvent {
    /// Operation started with a short human label.
    Started(String),
    /// Streaming progress update: current, total, message.
    Progress(u64, u64, String),
    /// Successful completion with summary and indented details.
    Success(String, Vec<String>),
    /// Non-fatal warning message.
    Warning(String),
    /// Rich diagnostic report.
    Diagnostic(Report),
}