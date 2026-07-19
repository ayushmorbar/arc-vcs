use std::collections::HashMap;
use std::path::Path;

use arc_algebra_types::Atom as SemanticAtom;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Local AST snapshot placeholder for policy evaluation.
///
/// The full AST model will be wired in once transport-stage delta payloads
/// expose typed tree material.
#[derive(Debug, Default, Clone)]
pub struct Ast {
    /// Local Rust source buffers used for invocation scanning.
    pub local_rust_sources: Vec<String>,
    /// Expected local API signatures keyed by exported function name.
    pub expected_api_signatures: HashMap<String, String>,
    /// Foreign Rust source buffers representing incoming mounted deltas.
    pub foreign_rust_sources: Vec<String>,
}

/// Repository policy loaded from `.arc/arc.policy.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ArcPolicy {
    /// Require cryptographic sponsorship when accepting ghost-node-style updates.
    pub require_ghost_node_sponsor: bool,
    /// Block sync when unresolved semantic contract breaks are detected.
    pub block_unresolved_sem_breaks: bool,
}

impl ArcPolicy {
    /// Load policy from a JSON file. Missing files resolve to defaults.
    pub fn load_from_path(path: &Path) -> Result<Self, PolicyError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = std::fs::read(path).map_err(|source| PolicyError::ReadConfig {
            path: path.display().to_string(),
            source,
        })?;
        serde_json::from_slice(&bytes)
            .map_err(|source| PolicyError::ParseConfig { path: path.display().to_string(), source })
    }
}

/// Semantic policy evaluation failures.
#[derive(Debug, Error)]
pub enum PolicyError {
    /// I/O failure while reading policy configuration.
    #[error("failed to read policy file '{path}': {source}")]
    ReadConfig {
        /// Filesystem path of the policy file.
        path: String,
        /// Underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// JSON parse failure while reading policy configuration.
    #[error("failed to parse policy file '{path}': {source}")]
    ParseConfig {
        /// Filesystem path of the policy file.
        path: String,
        /// Underlying JSON parse failure.
        #[source]
        source: serde_json::Error,
    },
    /// Incoming payload breaks semantic signatures consumed by the local graph.
    #[error("incoming update introduced semantic signature mismatch")]
    SignatureMismatch {
        /// Function names whose signatures changed and are referenced locally.
        broken_functions: Vec<String>,
        /// Previous expected signature representation.
        old_signature: String,
        /// New foreign signature representation.
        new_signature: String,
    },
    /// Incoming payload references graph dependencies absent from local boundary.
    #[error("incoming update references missing dependency '{dependency}'")]
    MissingDependency {
        /// Missing dependency identifier.
        dependency: String,
    },
}

/// AST firewall contract for incoming semantic deltas.
pub trait Evaluator {
    /// Validate incoming semantic atoms against the local AST contract.
    fn evaluate_delta_impact(
        &self,
        local_ast: &Ast,
        incoming_atoms: &[SemanticAtom],
    ) -> Result<(), PolicyError>;
}

/// Default policy evaluator alias backed by tree-sitter delta-impact analysis.
pub type DefaultEvaluator = super::evaluator::TreeSitterEvaluator;

impl ArcPolicy {
    /// Construct the default policy evaluator for this configuration.
    pub fn default_evaluator(&self) -> DefaultEvaluator {
        DefaultEvaluator::new(self.clone())
    }
}
