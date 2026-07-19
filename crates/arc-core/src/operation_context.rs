//! Generic operation context for command-style orchestration.

/// Optional render format for operation-side summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationReportFormat {
    /// Human-readable report rendering.
    Human,
    /// Machine-readable JSON report rendering.
    Json,
}

/// Metadata attached to an operation invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationMetadata {
    /// Stable operation name (for logs and metrics).
    pub name: String,
    /// Correlation id used to group child operations.
    pub correlation_id: String,
}

impl OperationMetadata {
    /// Build metadata with explicit name and correlation id.
    pub fn new(name: impl Into<String>, correlation_id: impl Into<String>) -> Self {
        Self { name: name.into(), correlation_id: correlation_id.into() }
    }
}

/// A generic operation context with separated output and error channels.
#[derive(Debug)]
pub struct OperationContext<WOut, WErr> {
    /// Output channel for operation results.
    pub out: WOut,
    /// Output channel for operation diagnostics and errors.
    pub err: WErr,
    /// Optional reporting preference.
    pub report_format: Option<OperationReportFormat>,
    /// Operation identity metadata.
    pub metadata: OperationMetadata,
}

impl<WOut, WErr> OperationContext<WOut, WErr> {
    /// Build a context from explicitly-provided channels and metadata.
    pub fn new(
        out: WOut,
        err: WErr,
        report_format: Option<OperationReportFormat>,
        metadata: OperationMetadata,
    ) -> Self {
        Self { out, err, report_format, metadata }
    }

    /// Transform context channels while preserving metadata and report format.
    pub fn map_channels<Out2, Err2>(
        self,
        map_out: impl FnOnce(WOut) -> Out2,
        map_err: impl FnOnce(WErr) -> Err2,
    ) -> OperationContext<Out2, Err2> {
        OperationContext {
            out: map_out(self.out),
            err: map_err(self.err),
            report_format: self.report_format,
            metadata: self.metadata,
        }
    }
}

impl Default for OperationContext<Vec<u8>, Vec<u8>> {
    fn default() -> Self {
        Self {
            out: Vec::new(),
            err: Vec::new(),
            report_format: None,
            metadata: OperationMetadata::new("operation", "local"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_context_provides_separate_channels() {
        let context = OperationContext::<Vec<u8>, Vec<u8>>::default();
        assert!(context.out.is_empty());
        assert!(context.err.is_empty());
        assert_eq!(context.metadata.name, "operation");
        assert_eq!(context.metadata.correlation_id, "local");
    }

    #[test]
    fn map_channels_keeps_metadata() {
        let context = OperationContext::new(
            vec![1_u8, 2_u8],
            vec![3_u8],
            Some(OperationReportFormat::Json),
            OperationMetadata::new("sync", "req-42"),
        );
        let mapped = context.map_channels(|out| out.len(), |err| err.len());

        assert_eq!(mapped.out, 2);
        assert_eq!(mapped.err, 1);
        assert_eq!(mapped.report_format, Some(OperationReportFormat::Json));
        assert_eq!(mapped.metadata.name, "sync");
        assert_eq!(mapped.metadata.correlation_id, "req-42");
    }
}
