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
    policy: ArcPolicy,
}

impl TreeSitterEvaluator {
    /// Build a tree-sitter evaluator from policy configuration.
    pub fn new(policy: ArcPolicy) -> Self {
        Self { policy }
    }

    fn extract_foreign_sources(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algebra::policy::Evaluator;

    fn default_policy() -> ArcPolicy {
        ArcPolicy { require_ghost_node_sponsor: false, block_unresolved_sem_breaks: true }
    }

    fn make_change(src: &str) -> (Ast, Vec<SemanticAtom>) {
        let local_ast = Ast {
            local_rust_sources: vec![],
            expected_api_signatures: std::collections::HashMap::new(),
            foreign_rust_sources: vec![],
        };
        let atoms = vec![SemanticAtom::SemanticsPreserving {
            at: vec!["src/lib.rs".to_string(), "fn".to_string(), "added".to_string()],
            description: src.to_string(),
        }];
        (local_ast, atoms)
    }

    #[test]
    fn no_local_calls_always_passes() {
        let evaluator = TreeSitterEvaluator::new(default_policy());
        let (ast, atoms) = make_change("pub fn added(x: i32) -> i32 { x + 1 }");
        assert!(evaluator.evaluate_delta_impact(&ast, &atoms).is_ok());
    }

    #[test]
    fn signature_mismatch_detected() {
        let mut sigs = std::collections::HashMap::new();
        sigs.insert("compute".to_string(), "(old_arg: i32) -> i32".to_string());
        let local_ast = Ast {
            local_rust_sources: vec!["fn caller() { let _ = compute(42); }".to_string()],
            expected_api_signatures: sigs,
            foreign_rust_sources: vec![],
        };
        let incoming = vec![SemanticAtom::SemanticsPreserving {
            at: vec!["src/lib.rs".to_string(), "fn".to_string(), "compute".to_string()],
            description: "pub fn compute(x: i32, y: i32) -> i32 { x + y }".to_string(),
        }];
        let evaluator = TreeSitterEvaluator::new(default_policy());
        let result = evaluator.evaluate_delta_impact(&local_ast, &incoming);
        assert!(result.is_err());
    }

    #[test]
    fn ghost_node_sponsor_required() {
        let mut policy = default_policy();
        policy.require_ghost_node_sponsor = true;
        let evaluator = TreeSitterEvaluator::new(policy);
        let ast = Ast::default();
        let atoms = vec![SemanticAtom::Mount {
            path: vec!["node".to_string()],
            coordinate: arc_algebra_types::SpacetimeCoordinate {
                namespace: "ns".to_string(),
                repo: "repo".to_string(),
                hash: blake3::hash(b"test"),
            },
        }];
        let err = evaluator.evaluate_delta_impact(&ast, &atoms).unwrap_err();
        assert!(matches!(err, PolicyError::SignatureMismatch { .. }));
    }

    #[test]
    fn payload_signature_mismatch() {
        let err = PolicyError::SignatureMismatch {
            broken_functions: vec!["foo".to_string(), "bar".to_string()],
            old_signature: "(x: i32) -> i32".to_string(),
            new_signature: "(x: i32, y: i32) -> i32".to_string(),
        };
        let payload = err.to_mcp_payload();
        assert_eq!(payload["type"], "SignatureMismatch");
        assert_eq!(payload["broken_functions"][0], "foo");
        assert_eq!(payload["broken_functions"][1], "bar");
        assert_eq!(payload["old_signature"], "(x: i32) -> i32");
        assert_eq!(payload["new_signature"], "(x: i32, y: i32) -> i32");
    }

    #[test]
    fn payload_missing_dependency() {
        let err = PolicyError::MissingDependency { dependency: "some_crate".to_string() };
        let payload = err.to_mcp_payload();
        assert_eq!(payload["type"], "MissingDependency");
        assert_eq!(payload["dependency"], "some_crate");
    }

    #[test]
    fn payload_read_config() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = PolicyError::ReadConfig { path: "/arc/policy.json".to_string(), source: io_err };
        let payload = err.to_mcp_payload();
        assert_eq!(payload["type"], "ReadConfig");
        assert_eq!(payload["path"], "/arc/policy.json");
        assert!(payload["error"].as_str().unwrap().contains("file not found"));
    }

    #[test]
    fn payload_parse_config() {
        let parse_err = serde_json::from_str::<serde_json::Value>("not json!!!").unwrap_err();
        let err =
            PolicyError::ParseConfig { path: "/arc/policy.json".to_string(), source: parse_err };
        let payload = err.to_mcp_payload();
        assert_eq!(payload["type"], "ParseConfig");
        assert_eq!(payload["path"], "/arc/policy.json");
        assert!(payload["error"].as_str().unwrap().contains("expected"));
    }

    #[test]
    fn boundary_query_is_nonempty() {
        assert!(!RUST_BOUNDARY_QUERY.is_empty());
    }

    #[test]
    fn invocation_query_is_nonempty() {
        assert!(!RUST_INVOCATION_QUERY.is_empty());
    }
}
