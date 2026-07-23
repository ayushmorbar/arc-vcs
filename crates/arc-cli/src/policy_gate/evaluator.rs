use std::collections::{HashMap, HashSet};

use arc_algebra_types::Atom as SemanticAtom;
use serde_json::json;
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use super::policy::{ArcPolicy, Ast, Evaluator, PolicyError};

impl PolicyError {
    /// Convert a policy error into an MCP-friendly structured payload.
    pub fn to_mcp_payload(&self) -> serde_json::Value {
        match self {
            PolicyError::SignatureMismatch { broken_functions, old_signature, new_signature } => {
                json!({
                    "type": "SignatureMismatch",
                    "broken_functions": broken_functions,
                    "old_signature": old_signature,
                    "new_signature": new_signature,
                })
            }
            PolicyError::MissingDependency { dependency } => json!({
                "type": "MissingDependency",
                "dependency": dependency,
            }),
            PolicyError::ReadConfig { path, source } => json!({
                "type": "ReadConfig",
                "path": path,
                "error": source.to_string(),
            }),
            PolicyError::ParseConfig { path, source } => json!({
                "type": "ParseConfig",
                "path": path,
                "error": source.to_string(),
            }),
        }
    }
}

/// Rust query for exported function boundary extraction.
pub const RUST_BOUNDARY_QUERY: &str = r#"
(function_item
  name: (identifier) @api.name
  parameters: (parameters) @api.params
  return_type: (_) @api.return)

(function_item
  name: (identifier) @api.name
  parameters: (parameters) @api.params)
"#;

/// Rust query for call-site extraction.
pub const RUST_INVOCATION_QUERY: &str = r#"
(call_expression
  function: (identifier) @call.name)

(call_expression
  function: (field_expression
    field: (field_identifier) @call.name))
"#;

/// tree-sitter based policy evaluator for cross-boundary delta-impact checks.
pub struct TreeSitterEvaluator {
    pub(crate) policy: ArcPolicy,
}

impl TreeSitterEvaluator {
    /// Build a tree-sitter evaluator from policy configuration.
    pub fn new(policy: ArcPolicy) -> Self {
        Self { policy }
    }

    pub(crate) fn extract_foreign_sources(
        &self,
        local_ast: &Ast,
        incoming_atoms: &[SemanticAtom],
    ) -> Vec<String> {
        let mut out = local_ast.foreign_rust_sources.clone();
        for atom in incoming_atoms {
            if let SemanticAtom::SemanticsPreserving { description, .. } = atom
                && description.contains("fn ")
            {
                out.push(description.clone());
            }
        }
        out
    }
}

impl Evaluator for TreeSitterEvaluator {
    fn evaluate_delta_impact(
        &self,
        local_ast: &Ast,
        incoming_atoms: &[SemanticAtom],
    ) -> Result<(), PolicyError> {
        if self.policy.require_ghost_node_sponsor
            && incoming_atoms.iter().any(|atom| matches!(atom, SemanticAtom::Mount { .. }))
        {
            return Err(PolicyError::SignatureMismatch {
                broken_functions: vec!["<ghost-node-sponsor>".to_string()],
                old_signature: "required=true".to_string(),
                new_signature: "required=false".to_string(),
            });
        }

        if !self.policy.block_unresolved_sem_breaks {
            return Ok(());
        }

        let mut parser = Parser::new();
        parser.set_language(&tree_sitter_rust::LANGUAGE.into()).map_err(|e| {
            PolicyError::MissingDependency {
                dependency: format!("tree-sitter-rust language init failed: {e}"),
            }
        })?;

        let boundary_query = Query::new(&tree_sitter_rust::LANGUAGE.into(), RUST_BOUNDARY_QUERY)
            .map_err(|e| PolicyError::MissingDependency {
                dependency: format!("boundary query compile failed: {e}"),
            })?;
        let invocation_query =
            Query::new(&tree_sitter_rust::LANGUAGE.into(), RUST_INVOCATION_QUERY).map_err(|e| {
                PolicyError::MissingDependency {
                    dependency: format!("invocation query compile failed: {e}"),
                }
            })?;

        let mut watch_list: HashSet<String> = HashSet::new();
        let mut old_signatures: HashMap<String, String> = HashMap::new();
        let mut new_signatures: HashMap<String, String> = HashMap::new();
        let mut preferred_signature_has_return: HashMap<String, bool> = HashMap::new();

        // Extract + filter incoming API boundaries.
        for source in self.extract_foreign_sources(local_ast, incoming_atoms) {
            let Some(tree) = parser.parse(&source, None) else {
                continue;
            };
            let mut cursor = QueryCursor::new();
            let capture_names = boundary_query.capture_names();
            let mut matches = cursor.matches(&boundary_query, tree.root_node(), source.as_bytes());
            while let Some(m) = matches.next() {
                let mut api_name: Option<String> = None;
                let mut api_params: Option<String> = None;
                let mut api_return: Option<String> = None;
                for capture in m.captures {
                    let cap = &capture_names[capture.index as usize];
                    let text =
                        capture.node.utf8_text(source.as_bytes()).ok().map(ToOwned::to_owned);
                    match *cap {
                        "api.name" => api_name = text,
                        "api.params" => api_params = text,
                        "api.return" => api_return = text,
                        _ => {}
                    }
                }

                let Some(name) = api_name else {
                    continue;
                };
                let Some(params) = api_params else {
                    continue;
                };
                let has_return = api_return.is_some();
                if let Some(existing_has_return) = preferred_signature_has_return.get(&name)
                    && *existing_has_return
                    && !has_return
                {
                    // Ignore fallback match without return when we already captured
                    // the canonical signature with explicit return type.
                    continue;
                }
                let ret = api_return.unwrap_or_else(|| "()".to_string());
                let new_sig = format!("{} -> {}", params.trim(), ret.trim());
                let new_hash = blake3::hash(new_sig.as_bytes()).to_hex().to_string();
                let old_sig =
                    local_ast.expected_api_signatures.get(&name).cloned().unwrap_or_default();
                let old_hash = blake3::hash(old_sig.as_bytes()).to_hex().to_string();

                if new_hash != old_hash {
                    watch_list.insert(name.clone());
                    old_signatures.insert(name.clone(), old_sig);
                    new_signatures.insert(name.clone(), new_sig);
                    preferred_signature_has_return.insert(name, has_return);
                } else {
                    preferred_signature_has_return.insert(name, has_return);
                }
            }
        }

        if watch_list.is_empty() {
            return Ok(());
        }

        // Scan local invocations.
        let mut local_calls: HashSet<String> = HashSet::new();
        for source in &local_ast.local_rust_sources {
            let Some(tree) = parser.parse(source, None) else {
                continue;
            };
            let mut cursor = QueryCursor::new();
            let capture_names = invocation_query.capture_names();
            let mut matches =
                cursor.matches(&invocation_query, tree.root_node(), source.as_bytes());
            while let Some(m) = matches.next() {
                for capture in m.captures {
                    let cap = &capture_names[capture.index as usize];
                    if *cap != "call.name" {
                        continue;
                    }
                    if let Ok(name) = capture.node.utf8_text(source.as_bytes()) {
                        local_calls.insert(name.to_string());
                    }
                }
            }
        }

        // Intersect.
        let delta_impact: HashSet<_> = watch_list.intersection(&local_calls).cloned().collect();
        if delta_impact.is_empty() {
            return Ok(());
        }

        let mut broken_functions: Vec<String> = delta_impact.into_iter().collect();
        broken_functions.sort();

        let old_signature = broken_functions
            .iter()
            .map(|name| {
                format!(
                    "{}:{}",
                    name,
                    old_signatures.get(name).cloned().unwrap_or_else(|| "<missing>".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let new_signature = broken_functions
            .iter()
            .map(|name| {
                format!(
                    "{}:{}",
                    name,
                    new_signatures.get(name).cloned().unwrap_or_else(|| "<missing>".to_string())
                )
            })
            .collect::<Vec<_>>()
            .join("; ");

        Err(PolicyError::SignatureMismatch { broken_functions, old_signature, new_signature })
    }
}
