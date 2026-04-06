//! Shared operation-stage taxonomy for tracing and reporting.

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

#[cfg(test)]
mod tests {
    use super::OperationStage;

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
}
