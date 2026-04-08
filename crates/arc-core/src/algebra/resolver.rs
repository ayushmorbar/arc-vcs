use std::collections::HashMap;

use arc_algebra_types::Atom as SemanticAtom;
use thiserror::Error;

use super::evaluator::TreeSitterEvaluator;
use super::policy::{ArcPolicy, Ast, Evaluator, PolicyError};

/// Errors returned by AI lens synthesis and validation.
#[derive(Debug, Error)]
pub enum ResolverError {
    /// The evaluator still reports a semantic break after applying the lens.
    #[error("lens verification failed: semantic break remains")]
    VerificationFailed,
    /// The resolver could not synthesize a lens from the policy payload.
    #[error("resolver synthesis failed: {reason}")]
    SynthesisFailed {
        /// Human-readable synthesis failure reason.
        reason: String,
    },
}

/// AI adapter contract for turning policy errors into semantic lens atoms.
pub trait AiResolver {
    /// Produce semantic adapter atoms from a structured policy error.
    fn synthesize_lens(&self, error: &PolicyError) -> Result<Vec<SemanticAtom>, ResolverError>;
}

#[derive(Default)]
struct TransientBuffer {
    atoms: Vec<SemanticAtom>,
}

impl TransientBuffer {
    fn apply_atoms(&mut self, atoms: &[SemanticAtom]) {
        self.atoms.extend_from_slice(atoms);
    }
}

/// Re-run evaluator after applying synthesized lens atoms to a transient buffer.
pub fn verify_lens(
    evaluator: &TreeSitterEvaluator,
    local_ast: &Ast,
    incoming_atoms: &[SemanticAtom],
    lens_atoms: &[SemanticAtom],
) -> Result<(), ResolverError> {
    let mut transient = TransientBuffer::default();
    transient.apply_atoms(incoming_atoms);
    transient.apply_atoms(lens_atoms);

    match evaluator.evaluate_delta_impact(local_ast, &transient.atoms) {
        Ok(()) => Ok(()),
        Err(PolicyError::SignatureMismatch { .. }) => Err(ResolverError::VerificationFailed),
        Err(other) => Err(ResolverError::SynthesisFailed {
            reason: other.to_string(),
        }),
    }
}

/// Deterministic local resolver scaffold for MCP handoff integration.
pub struct MockAiResolver;

impl AiResolver for MockAiResolver {
    fn synthesize_lens(&self, error: &PolicyError) -> Result<Vec<SemanticAtom>, ResolverError> {
        let payload = error.to_mcp_payload();
        let functions = payload
            .get("broken_functions")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ResolverError::SynthesisFailed {
                reason: "missing broken_functions in MCP payload".to_string(),
            })?;

        let mut atoms = Vec::new();
        for item in functions {
            let name = item.as_str().ok_or_else(|| ResolverError::SynthesisFailed {
                reason: "broken function entry was not a string".to_string(),
            })?;
            atoms.push(SemanticAtom::SemanticsPreserving {
                at: vec!["file".to_string(), "lens.rs".to_string(), name.to_string()],
                description: format!("fn {}() {{ /* lensed adapter */ }}", name),
            });
        }
        Ok(atoms)
    }
}

/// Build an AST context from local and foreign Rust source buffers.
pub fn ast_context(local_rust_sources: Vec<String>, foreign_rust_sources: Vec<String>) -> Ast {
    Ast {
        local_rust_sources,
        expected_api_signatures: HashMap::new(),
        foreign_rust_sources,
    }
}

/// Build the default evaluator used by resolver verification loop.
pub fn default_evaluator() -> TreeSitterEvaluator {
    TreeSitterEvaluator::new(ArcPolicy {
        require_ghost_node_sponsor: false,
        block_unresolved_sem_breaks: true,
    })
}
