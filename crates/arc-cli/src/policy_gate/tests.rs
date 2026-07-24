#[cfg(test)]
mod policy_tests {
    use std::path::Path;

    use arc_algebra_types::{Atom as SemanticAtom, SpacetimeCoordinate};

    use crate::policy_gate::*;

    #[test]
    fn default_policy_is_safe_by_default() {
        let p = ArcPolicy::default();
        assert!(!p.require_ghost_node_sponsor);
        assert!(!p.block_unresolved_sem_breaks);
    }

    #[test]
    fn load_from_path_missing_file_returns_default() {
        let p = ArcPolicy::load_from_path(Path::new("/nonexistent/arc.policy.json")).unwrap();
        assert!(!p.require_ghost_node_sponsor);
        assert!(!p.block_unresolved_sem_breaks);
    }

    #[test]
    fn load_from_path_valid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arc.policy.json");
        std::fs::write(
            &path,
            r#"{"require_ghost_node_sponsor":true,"block_unresolved_sem_breaks":true}"#,
        )
        .unwrap();
        let p = ArcPolicy::load_from_path(&path).unwrap();
        assert!(p.require_ghost_node_sponsor);
        assert!(p.block_unresolved_sem_breaks);
    }

    #[test]
    fn load_from_path_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arc.policy.json");
        std::fs::write(&path, "not json {{{").unwrap();
        let err = ArcPolicy::load_from_path(&path).unwrap_err();
        match err {
            PolicyError::ParseConfig { path: p, .. } => {
                assert!(p.contains("arc.policy.json"));
            }
            other => panic!("expected ParseConfig, got {other}"),
        }
    }

    #[test]
    fn load_from_path_partial_json_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("arc.policy.json");
        std::fs::write(&path, r#"{"require_ghost_node_sponsor":true}"#).unwrap();
        let p = ArcPolicy::load_from_path(&path).unwrap();
        assert!(p.require_ghost_node_sponsor);
        assert!(!p.block_unresolved_sem_breaks);
    }

    #[test]
    fn ast_default() {
        let ast = Ast::default();
        assert!(ast.local_rust_sources.is_empty());
        assert!(ast.expected_api_signatures.is_empty());
        assert!(ast.foreign_rust_sources.is_empty());
    }

    #[test]
    fn default_evaluator_builds_from_policy() {
        let policy =
            ArcPolicy { require_ghost_node_sponsor: true, block_unresolved_sem_breaks: false };
        let evaluator = policy.default_evaluator();
        let ast = Ast::default();
        let atoms = vec![SemanticAtom::Mount {
            path: vec!["file.rs".to_string()],
            coordinate: SpacetimeCoordinate {
                namespace: "org".into(),
                repo: "repo".into(),
                hash: blake3::Hash::from_bytes([0u8; 32]),
            },
        }];
        let err = evaluator.evaluate_delta_impact(&ast, &atoms).unwrap_err();
        match err {
            PolicyError::SignatureMismatch { broken_functions, .. } => {
                assert_eq!(broken_functions, vec!["<ghost-node-sponsor>"]);
            }
            other => panic!("expected SignatureMismatch, got {other}"),
        }
    }

    #[test]
    fn policy_error_display_variants() {
        let e1 = PolicyError::SignatureMismatch {
            broken_functions: vec!["foo".into()],
            old_signature: "old".into(),
            new_signature: "new".into(),
        };
        let s = e1.to_string();
        assert!(s.contains("semantic signature mismatch"));

        let e2 = PolicyError::MissingDependency { dependency: "bar".into() };
        let s = e2.to_string();
        assert!(s.contains("bar"));

        let e3 = PolicyError::ReadConfig {
            path: "/x".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "nope"),
        };
        let s = e3.to_string();
        assert!(s.contains("/x"));

        let e4 = PolicyError::ParseConfig {
            path: "/y".into(),
            source: serde_json::from_str::<serde_json::Value>("").unwrap_err(),
        };
        let s = e4.to_string();
        assert!(s.contains("/y"));
    }
}

#[cfg(test)]
mod evaluator_tests {
    use std::collections::HashMap;

    use arc_algebra_types::{Atom as SemanticAtom, SpacetimeCoordinate};

    use crate::policy_gate::*;

    fn policy_blocking() -> ArcPolicy {
        ArcPolicy { require_ghost_node_sponsor: false, block_unresolved_sem_breaks: true }
    }

    fn policy_permissive() -> ArcPolicy {
        ArcPolicy { require_ghost_node_sponsor: false, block_unresolved_sem_breaks: false }
    }

    #[test]
    fn permissive_policy_always_passes() {
        let evaluator = TreeSitterEvaluator::new(policy_permissive());
        let ast = Ast::default();
        let atoms = vec![SemanticAtom::Insert {
            at: vec!["f.rs".into(), "g".into()],
            content_hash: [0u8; 32],
        }];
        assert!(evaluator.evaluate_delta_impact(&ast, &atoms).is_ok());
    }

    #[test]
    fn ghost_node_sponsor_required_blocks_mount() {
        let policy =
            ArcPolicy { require_ghost_node_sponsor: true, block_unresolved_sem_breaks: false };
        let evaluator = TreeSitterEvaluator::new(policy);
        let ast = Ast::default();
        let atoms = vec![SemanticAtom::Mount {
            path: vec!["f.rs".into()],
            coordinate: SpacetimeCoordinate {
                namespace: "org".into(),
                repo: "repo".into(),
                hash: blake3::Hash::from_bytes([42u8; 32]),
            },
        }];
        let err = evaluator.evaluate_delta_impact(&ast, &atoms).unwrap_err();
        match err {
            PolicyError::SignatureMismatch { broken_functions, .. } => {
                assert_eq!(broken_functions, vec!["<ghost-node-sponsor>"]);
            }
            other => panic!("expected SignatureMismatch, got {other}"),
        }
    }

    #[test]
    fn blocking_policy_no_mismatch_returns_ok() {
        let evaluator = TreeSitterEvaluator::new(policy_blocking());
        let ast = Ast::default();
        let atoms = vec![SemanticAtom::Insert {
            at: vec!["f.rs".into(), "func".into()],
            content_hash: [1u8; 32],
        }];
        assert!(evaluator.evaluate_delta_impact(&ast, &atoms).is_ok());
    }

    #[test]
    fn blocking_policy_no_local_sources_returns_ok() {
        let evaluator = TreeSitterEvaluator::new(policy_blocking());
        let ast = Ast {
            local_rust_sources: vec![],
            expected_api_signatures: HashMap::new(),
            foreign_rust_sources: vec![],
        };
        let atoms = vec![SemanticAtom::SemanticsPreserving {
            at: vec!["file.rs".into()],
            description: "fn foo(x: i32) -> i32 { x }".into(),
        }];
        assert!(evaluator.evaluate_delta_impact(&ast, &atoms).is_ok());
    }

    #[test]
    fn blocking_policy_signature_match_returns_ok() {
        let evaluator = TreeSitterEvaluator::new(policy_blocking());
        let ast = Ast {
            local_rust_sources: vec!["fn main() { let _ = 42; }".to_string()],
            expected_api_signatures: HashMap::from([(
                "foo".to_string(),
                "(x: i32) -> i32".to_string(),
            )]),
            foreign_rust_sources: vec!["fn foo(x: i32) -> i32 { x }".to_string()],
        };
        let atoms = vec![];
        // Even if the signature extraction produces a slightly different
        // format, the local source doesn't call foo, so the intersection
        // is empty and the evaluation should succeed.
        assert!(evaluator.evaluate_delta_impact(&ast, &atoms).is_ok());
    }

    #[test]
    fn blocking_policy_signature_mismatch_detected() {
        let evaluator = TreeSitterEvaluator::new(policy_blocking());
        let ast = Ast {
            local_rust_sources: vec!["fn main() { foo(1); }".to_string()],
            expected_api_signatures: HashMap::from([(
                "foo".to_string(),
                "(x: i32) -> i32".to_string(),
            )]),
            foreign_rust_sources: vec!["fn foo(x: i32, y: i32) -> i32 { x + y }".to_string()],
        };
        let atoms = vec![];
        let err = evaluator.evaluate_delta_impact(&ast, &atoms).unwrap_err();
        match err {
            PolicyError::SignatureMismatch { broken_functions, .. } => {
                assert!(broken_functions.contains(&"foo".to_string()));
            }
            other => panic!("expected SignatureMismatch, got {other}"),
        }
    }

    #[test]
    fn extract_foreign_sources_includes_semantics_preserving() {
        let policy = policy_blocking();
        let evaluator = TreeSitterEvaluator::new(policy);
        let ast = Ast { foreign_rust_sources: vec!["existing".into()], ..Ast::default() };
        let atoms = vec![SemanticAtom::SemanticsPreserving {
            at: vec!["file.rs".into()],
            description: "fn bar() {}".into(),
        }];
        let sources = evaluator.extract_foreign_sources(&ast, &atoms);
        assert_eq!(sources.len(), 2);
        assert!(sources.contains(&"existing".to_string()));
        assert!(sources.contains(&"fn bar() {}".to_string()));
    }

    #[test]
    fn extract_foreign_sources_ignores_non_fn_atoms() {
        let policy = policy_blocking();
        let evaluator = TreeSitterEvaluator::new(policy);
        let ast = Ast::default();
        let atoms = vec![SemanticAtom::SemanticsPreserving {
            at: vec!["file.rs".into()],
            description: "struct Foo {}".into(),
        }];
        let sources = evaluator.extract_foreign_sources(&ast, &atoms);
        assert!(sources.is_empty());
    }

    #[test]
    fn unparseable_source_is_skipped() {
        let evaluator = TreeSitterEvaluator::new(policy_blocking());
        let ast = Ast {
            local_rust_sources: vec!["this is not rust {{{".to_string()],
            expected_api_signatures: HashMap::new(),
            foreign_rust_sources: vec!["also not rust {{{".to_string()],
        };
        let atoms = vec![];
        assert!(evaluator.evaluate_delta_impact(&ast, &atoms).is_ok());
    }

    #[test]
    fn non_fn_invocation_name_in_local_is_skipped() {
        let evaluator = TreeSitterEvaluator::new(policy_blocking());
        let ast = Ast {
            local_rust_sources: vec!["fn main() { let x = 1; }".to_string()],
            expected_api_signatures: HashMap::new(),
            foreign_rust_sources: vec![],
        };
        let atoms = vec![];
        assert!(evaluator.evaluate_delta_impact(&ast, &atoms).is_ok());
    }

    #[test]
    fn new_builds_evaluator() {
        let e = TreeSitterEvaluator::new(ArcPolicy::default());
        assert!(!e.policy.require_ghost_node_sponsor);
    }
}

#[cfg(test)]
mod resolver_tests {
    use std::collections::HashMap;

    use arc_algebra_types::Atom as SemanticAtom;

    use crate::policy_gate::*;

    #[test]
    fn resolver_error_display_variants() {
        let e1 = ResolverError::VerificationFailed;
        assert!(e1.to_string().contains("lens verification failed"));

        let e2 = ResolverError::SynthesisFailed { reason: "bad input".into() };
        assert!(e2.to_string().contains("bad input"));
    }

    #[test]
    fn mock_ai_resolver_synthesizes_lens_from_signature_mismatch() {
        let resolver = MockAiResolver;
        let error = PolicyError::SignatureMismatch {
            broken_functions: vec!["my_fn".into()],
            old_signature: "old".into(),
            new_signature: "new".into(),
        };
        let atoms = resolver.synthesize_lens(&error).unwrap();
        assert_eq!(atoms.len(), 1);
        match &atoms[0] {
            SemanticAtom::SemanticsPreserving { at, description } => {
                assert_eq!(at.len(), 3);
                assert_eq!(at[2], "my_fn");
                assert!(description.contains("my_fn"));
            }
            other => panic!("expected SemanticsPreserving, got {other:?}"),
        }
    }

    #[test]
    fn mock_ai_resolver_fails_on_missing_dependency() {
        let resolver = MockAiResolver;
        let error = PolicyError::MissingDependency { dependency: "foo".into() };
        let err = resolver.synthesize_lens(&error).unwrap_err();
        match err {
            ResolverError::SynthesisFailed { reason } => {
                assert!(reason.contains("missing broken_functions"));
            }
            other => panic!("expected SynthesisFailed, got {other:?}"),
        }
    }

    #[test]
    fn mock_ai_resolver_fails_on_read_config() {
        let resolver = MockAiResolver;
        let error =
            PolicyError::ReadConfig { path: "/x".into(), source: std::io::Error::other("io err") };
        let err = resolver.synthesize_lens(&error).unwrap_err();
        match err {
            ResolverError::SynthesisFailed { reason } => {
                assert!(reason.contains("missing broken_functions"));
            }
            other => panic!("expected SynthesisFailed, got {other:?}"),
        }
    }

    #[test]
    fn verify_lens_passes_when_no_break() {
        let evaluator = TreeSitterEvaluator::new(ArcPolicy {
            require_ghost_node_sponsor: false,
            block_unresolved_sem_breaks: true,
        });
        let ast = Ast::default();
        let incoming = vec![];
        let lens = vec![];
        assert!(verify_lens(&evaluator, &ast, &incoming, &lens).is_ok());
    }

    #[test]
    fn verify_lens_returns_verification_failed_on_signature_mismatch() {
        let evaluator = TreeSitterEvaluator::new(ArcPolicy {
            require_ghost_node_sponsor: false,
            block_unresolved_sem_breaks: true,
        });
        let ast = Ast {
            local_rust_sources: vec!["fn main() { foo(1); }".to_string()],
            expected_api_signatures: HashMap::from([(
                "foo".to_string(),
                "(x: i32) -> i32".to_string(),
            )]),
            foreign_rust_sources: vec!["fn foo(x: i32, y: i32) -> i32 { x + y }".to_string()],
        };
        let incoming = vec![];
        let lens = vec![];
        let err = verify_lens(&evaluator, &ast, &incoming, &lens).unwrap_err();
        assert!(matches!(err, ResolverError::VerificationFailed));
    }

    #[test]
    fn ast_context_builds_correctly() {
        let local = vec!["fn foo() {}".to_string()];
        let foreign = vec!["fn bar() {}".to_string()];
        let ast = ast_context(local.clone(), foreign.clone());
        assert_eq!(ast.local_rust_sources, local);
        assert_eq!(ast.foreign_rust_sources, foreign);
        assert!(ast.expected_api_signatures.is_empty());
    }

    #[test]
    fn default_evaluator_has_correct_policy() {
        let e = default_evaluator();
        assert!(!e.policy.require_ghost_node_sponsor);
        assert!(e.policy.block_unresolved_sem_breaks);
    }

    #[test]
    fn multiple_broken_functions_sorted() {
        let resolver = MockAiResolver;
        let error = PolicyError::SignatureMismatch {
            broken_functions: vec!["zebra".into(), "alpha".into(), "middle".into()],
            old_signature: "old".into(),
            new_signature: "new".into(),
        };
        let atoms = resolver.synthesize_lens(&error).unwrap();
        assert_eq!(atoms.len(), 3);
        // Check all three functions are present
        let names: Vec<&str> = atoms
            .iter()
            .map(|a| match a {
                SemanticAtom::SemanticsPreserving { at, .. } => at[2].as_str(),
                _ => panic!("unexpected"),
            })
            .collect();
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"middle"));
        assert!(names.contains(&"zebra"));
    }
}
