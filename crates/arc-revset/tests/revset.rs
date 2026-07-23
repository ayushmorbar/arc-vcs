use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use arc_algebra_types::{Atom, Blake3Hash};
use arc_change::Change;
use arc_revset::{ReferenceResolver, RevsetEvaluator, RevsetExpression};
use arc_revset::{compile, compile_change_ids, compile_change_ids_with_refs, parse};
use arc_store_graph::ChangeGraph;
use arc_store_types::author::test_keypair;
use arc_store_types::newtypes::ChangeId;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn make_change(graph: &mut ChangeGraph, deps: HashSet<Blake3Hash>, label: &str) -> Blake3Hash {
    let (author, key) = test_keypair();
    let content_hash: [u8; 32] = *blake3::hash(label.as_bytes()).as_bytes();
    let change = Change::new(
        deps,
        vec![Atom::Insert { at: vec![label.to_string()], content_hash }],
        "test",
        author,
        &key,
    );
    let id = change.id;
    graph.add_change(change);
    id
}

/// Build a linear chain: root → a → b → c
fn linear_chain() -> (ChangeGraph, [Blake3Hash; 4]) {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let a = make_change(&mut graph, HashSet::from([root]), "a");
    let b = make_change(&mut graph, HashSet::from([a]), "b");
    let c = make_change(&mut graph, HashSet::from([b]), "c");
    (graph, [root, a, b, c])
}

/// Build a diamond: root → {a, b} → d
fn diamond() -> (ChangeGraph, [Blake3Hash; 4]) {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let a = make_change(&mut graph, HashSet::from([root]), "a");
    let b = make_change(&mut graph, HashSet::from([root]), "b");
    let d = make_change(&mut graph, HashSet::from([a, b]), "d");
    (graph, [root, a, b, d])
}

/// Build a diamond with two independent roots: {r1, r2} → {a, b} → d
fn diamond_two_roots() -> (ChangeGraph, [Blake3Hash; 5]) {
    let mut graph = ChangeGraph::new();
    let r1 = make_change(&mut graph, HashSet::new(), "r1");
    let r2 = make_change(&mut graph, HashSet::new(), "r2");
    let a = make_change(&mut graph, HashSet::from([r1, r2]), "a");
    let b = make_change(&mut graph, HashSet::from([r1, r2]), "b");
    let d = make_change(&mut graph, HashSet::from([a, b]), "d");
    (graph, [r1, r2, a, b, d])
}

/// Build a merge scenario: root → a, root → b, a → merge, b → merge
fn merge_scenario() -> (ChangeGraph, [Blake3Hash; 4]) {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let a = make_change(&mut graph, HashSet::from([root]), "a");
    let b = make_change(&mut graph, HashSet::from([root]), "b");
    let merge = make_change(&mut graph, HashSet::from([a, b]), "merge");
    (graph, [root, a, b, merge])
}

/// Build a fan: root → {a, b, c, d}
fn fan() -> (ChangeGraph, [Blake3Hash; 5]) {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let a = make_change(&mut graph, HashSet::from([root]), "a");
    let b = make_change(&mut graph, HashSet::from([root]), "b");
    let c = make_change(&mut graph, HashSet::from([root]), "c");
    let d = make_change(&mut graph, HashSet::from([root]), "d");
    (graph, [root, a, b, c, d])
}

type SymbolMap = std::collections::HashMap<String, ChangeId>;

fn resolver(map: &SymbolMap) -> impl FnMut(&str) -> Result<Option<ChangeId>, anyhow::Error> + '_ {
    move |name: &str| Ok(map.get(name).copied())
}

fn resolver_fn(map: SymbolMap) -> impl FnMut(&str) -> Result<Option<ChangeId>, anyhow::Error> {
    move |name: &str| Ok(map.get(name).copied())
}

/// Helper: `compile_change_ids` returns `Box<dyn Iterator>` which doesn't impl
/// `Debug`, so `.unwrap_err()` can't be used.  This helper unwraps the error for
/// tests that expect compilation to fail.
fn compile_change_ids_expect_err(
    ast: &RevsetExpression,
    graph: Arc<ChangeGraph>,
    resolver: &mut impl FnMut(&str) -> Result<Option<ChangeId>, anyhow::Error>,
) -> anyhow::Error {
    match compile_change_ids(ast, graph, resolver) {
        Ok(_) => panic!("expected compile_change_ids to return an error"),
        Err(e) => e,
    }
}

fn compile_change_ids_with_refs_expect_err(
    ast: &RevsetExpression,
    graph: Arc<ChangeGraph>,
    resolver: &mut impl FnMut(&str) -> Result<Option<ChangeId>, anyhow::Error>,
    refs: &mut impl arc_revset::ReferenceResolver,
) -> anyhow::Error {
    match compile_change_ids_with_refs(ast, graph, resolver, refs) {
        Ok(_) => panic!("expected compile_change_ids_with_refs to return an error"),
        Err(e) => e,
    }
}

struct MockRefResolver {
    refs: std::collections::HashMap<String, BTreeSet<ChangeId>>,
}

impl MockRefResolver {
    fn new() -> Self {
        Self { refs: std::collections::HashMap::new() }
    }

    fn with(mut self, name: &str, ids: BTreeSet<ChangeId>) -> Self {
        self.refs.insert(name.to_string(), ids);
        self
    }
}

impl ReferenceResolver for MockRefResolver {
    fn resolve_reference_heads(
        &mut self,
        function_name: &str,
    ) -> anyhow::Result<BTreeSet<ChangeId>> {
        Ok(self.refs.get(function_name).cloned().unwrap_or_default())
    }
}

// ---------------------------------------------------------------------------
// Parser integration tests
// ---------------------------------------------------------------------------

#[test]
fn parser_symbol_at() {
    let expr = parse("@").unwrap();
    assert_eq!(expr, RevsetExpression::Symbol("@".to_string()));
}

#[test]
fn parser_symbol_main() {
    let expr = parse("main").unwrap();
    assert_eq!(expr, RevsetExpression::Symbol("main".to_string()));
}

#[test]
fn parser_symbol_with_underscore() {
    let expr = parse("my_branch").unwrap();
    assert_eq!(expr, RevsetExpression::Symbol("my_branch".to_string()));
}

#[test]
fn parser_string_literal_simple() {
    let expr = parse(r#"touched("src/lib.rs")"#).unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Function {
            name: "touched".to_string(),
            args: vec![RevsetExpression::StringLiteral("src/lib.rs".to_string())],
        }
    );
}

#[test]
fn parser_string_literal_escaped_quote() {
    let expr = parse(r#"touched("path/with\"quote")"#).unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Function {
            name: "touched".to_string(),
            args: vec![RevsetExpression::StringLiteral("path/with\"quote".to_string())],
        }
    );
}

#[test]
fn parser_string_literal_escaped_backslash() {
    let expr = parse(r#"touched("path\\file")"#).unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Function {
            name: "touched".to_string(),
            args: vec![RevsetExpression::StringLiteral("path\\file".to_string())],
        }
    );
}

#[test]
fn parser_empty_string_literal() {
    let expr = parse(r#"touched("")"#).unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Function {
            name: "touched".to_string(),
            args: vec![RevsetExpression::StringLiteral("".to_string())],
        }
    );
}

#[test]
fn parser_rejects_unterminated_string() {
    assert!(parse(r#"touched("src)"#).is_err());
}

#[test]
fn parser_rejects_unterminated_escape() {
    assert!(parse(r#"touched("src\")"#).is_err());
}

#[test]
fn parser_rejects_unknown_escape() {
    assert!(parse(r#"touched("src\n")"#).is_err());
}

#[test]
fn parser_function_no_args() {
    let expr = parse("tags()").unwrap();
    assert_eq!(expr, RevsetExpression::Function { name: "tags".to_string(), args: vec![] });
}

#[test]
fn parser_function_one_symbol_arg() {
    let expr = parse("ancestors(@)").unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Function {
            name: "ancestors".to_string(),
            args: vec![RevsetExpression::Symbol("@".to_string())],
        }
    );
}

#[test]
fn parser_function_two_symbol_args() {
    let expr = parse("range(main, @)").unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Function {
            name: "range".to_string(),
            args: vec![
                RevsetExpression::Symbol("main".to_string()),
                RevsetExpression::Symbol("@".to_string()),
            ],
        }
    );
}

#[test]
fn parser_function_nested_call() {
    let expr = parse("ancestors(tags())").unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Function {
            name: "ancestors".to_string(),
            args: vec![RevsetExpression::Function { name: "tags".to_string(), args: vec![] }],
        }
    );
}

#[test]
fn parser_intersection() {
    let expr = parse("ancestors(A) & @").unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Intersection(
            Box::new(RevsetExpression::Function {
                name: "ancestors".to_string(),
                args: vec![RevsetExpression::Symbol("A".to_string())],
            }),
            Box::new(RevsetExpression::Symbol("@".to_string())),
        )
    );
}

#[test]
fn parser_union() {
    let expr = parse("A | B").unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Union(
            Box::new(RevsetExpression::Symbol("A".to_string())),
            Box::new(RevsetExpression::Symbol("B".to_string())),
        )
    );
}

#[test]
fn parser_union_chain() {
    let expr = parse("A | B | C").unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Union(
            Box::new(RevsetExpression::Union(
                Box::new(RevsetExpression::Symbol("A".to_string())),
                Box::new(RevsetExpression::Symbol("B".to_string())),
            )),
            Box::new(RevsetExpression::Symbol("C".to_string())),
        )
    );
}

#[test]
fn parser_intersection_chain() {
    let expr = parse("A & B & C").unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Intersection(
            Box::new(RevsetExpression::Intersection(
                Box::new(RevsetExpression::Symbol("A".to_string())),
                Box::new(RevsetExpression::Symbol("B".to_string())),
            )),
            Box::new(RevsetExpression::Symbol("C".to_string())),
        )
    );
}

#[test]
fn parser_intersection_before_union() {
    let expr = parse("A & B | C").unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Union(
            Box::new(RevsetExpression::Intersection(
                Box::new(RevsetExpression::Symbol("A".to_string())),
                Box::new(RevsetExpression::Symbol("B".to_string())),
            )),
            Box::new(RevsetExpression::Symbol("C".to_string())),
        )
    );
}

#[test]
fn parser_parenthesized_expression() {
    let expr = parse("(A | B) & C").unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Intersection(
            Box::new(RevsetExpression::Union(
                Box::new(RevsetExpression::Symbol("A".to_string())),
                Box::new(RevsetExpression::Symbol("B".to_string())),
            )),
            Box::new(RevsetExpression::Symbol("C".to_string())),
        )
    );
}

#[test]
fn parser_deeply_nested_function() {
    let expr = parse("ancestors(range(A, B))").unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Function {
            name: "ancestors".to_string(),
            args: vec![RevsetExpression::Function {
                name: "range".to_string(),
                args: vec![
                    RevsetExpression::Symbol("A".to_string()),
                    RevsetExpression::Symbol("B".to_string()),
                ],
            }],
        }
    );
}

#[test]
fn parser_rejects_empty_input() {
    assert!(parse("").is_err());
}

#[test]
fn parser_rejects_whitespace_only() {
    assert!(parse("   ").is_err());
}

#[test]
fn parser_rejects_unmatched_paren() {
    assert!(parse("(A | B").is_err());
}

#[test]
fn parser_rejects_trailing_operator() {
    assert!(parse("A |").is_err());
}

#[test]
fn parser_rejects_leading_operator() {
    assert!(parse("| A").is_err());
}

#[test]
fn parser_rejects_double_ampersand() {
    assert!(parse("A & & B").is_err());
}

#[test]
fn parser_rejects_unknown_function() {
    // PEG parser parses unknown functions fine at grammar level;
    // the engine rejects them later
    let expr = parse("unknown_func(x)").unwrap();
    assert_eq!(
        expr,
        RevsetExpression::Function {
            name: "unknown_func".to_string(),
            args: vec![RevsetExpression::Symbol("x".to_string())],
        }
    );
}

#[test]
fn parser_hex_hash_symbol() {
    let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let expr = parse(hex).unwrap();
    assert_eq!(expr, RevsetExpression::Symbol(hex.to_string()));
}

#[test]
fn parser_debug_format() {
    let expr = parse("ancestors(@)").unwrap();
    let debug = format!("{expr:?}");
    assert!(debug.contains("ancestors"));
    assert!(debug.contains("@"));
}

// ---------------------------------------------------------------------------
// compile() — Blake3Hash iterator path
// ---------------------------------------------------------------------------

#[test]
fn compile_symbol_returns_hash() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut resolver = |name: &str| -> anyhow::Result<Option<Blake3Hash>> {
        match name {
            "root" => Ok(Some(root)),
            _ => Ok(None),
        }
    };

    let ast = parse("root").unwrap();
    let result: Vec<Blake3Hash> =
        compile(&ast, Arc::clone(&graph), &mut resolver).unwrap().collect();

    assert_eq!(result, vec![root]);
}

#[test]
fn compile_union_returns_both_hashes() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let b = make_change(&mut graph, HashSet::new(), "b");
    let graph = Arc::new(graph);

    let mut resolver = |name: &str| -> anyhow::Result<Option<Blake3Hash>> {
        match name {
            "A" => Ok(Some(a)),
            "B" => Ok(Some(b)),
            _ => Ok(None),
        }
    };

    let ast = parse("A | B").unwrap();
    let mut result: Vec<Blake3Hash> =
        compile(&ast, Arc::clone(&graph), &mut resolver).unwrap().collect();
    result.sort();

    let mut expected = vec![a, b];
    expected.sort();
    assert_eq!(result, expected);
}

// ---------------------------------------------------------------------------
// compile_change_ids() — typed iterator path
// ---------------------------------------------------------------------------

#[test]
fn compile_single_symbol() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("root".to_string(), ChangeId::from(root));

    let ast = parse("root").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(root)]));
}

#[test]
fn compile_union_of_symbols() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let b = make_change(&mut graph, HashSet::new(), "b");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));

    let ast = parse("A | B").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(a), ChangeId::from(b)]));
}

#[test]
fn compile_intersection_of_symbols() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let b = make_change(&mut graph, HashSet::new(), "b");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));

    let ast = parse("A & B").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // A and B are distinct, intersection is empty
    assert!(result.is_empty());
}

#[test]
fn compile_intersection_overlapping() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let a = make_change(&mut graph, HashSet::from([root]), "a");
    let graph = Arc::new(graph);

    // Both "X" and "Y" resolve to the same change
    let mut map = SymbolMap::new();
    map.insert("X".to_string(), ChangeId::from(a));
    map.insert("Y".to_string(), ChangeId::from(a));

    let ast = parse("X & Y").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(a)]));
}

#[test]
fn compile_string_literal_rejected_at_top_level() {
    let mut graph = ChangeGraph::new();
    let _ = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse(r#""literal""#).unwrap();
    let err = compile_change_ids_expect_err(&ast, Arc::clone(&graph), &mut resolver);
    assert!(err.to_string().contains("string literals are only valid as function arguments"));
}

#[test]
fn compile_unknown_symbol_returns_error() {
    let mut graph = ChangeGraph::new();
    let _ = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("nonexistent").unwrap();
    let err = compile_change_ids_expect_err(&ast, Arc::clone(&graph), &mut resolver);
    assert!(err.to_string().contains("unknown revset symbol"));
}

#[test]
fn compile_hex_hash_in_graph() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let hex: String = root.iter().map(|b| format!("{b:02x}")).collect();
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse(&hex).unwrap();
    let result: Vec<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver).unwrap().collect();

    assert_eq!(result, vec![ChangeId::from(root)]);
}

#[test]
fn compile_hex_hash_not_in_graph() {
    let mut graph = ChangeGraph::new();
    let _ = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let unknown = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse(unknown).unwrap();
    let err = compile_change_ids_expect_err(&ast, Arc::clone(&graph), &mut resolver);
    assert!(err.to_string().contains("unknown revset symbol"));
}

#[test]
fn compile_union_deduplicates() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));

    let ast = parse("A | A").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(result.len(), 1);
    assert!(result.contains(&ChangeId::from(a)));
}

// ---------------------------------------------------------------------------
// ancestors() function
// ---------------------------------------------------------------------------

#[test]
fn ancestors_of_root() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("R".to_string(), ChangeId::from(root));

    let ast = parse("ancestors(R)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(root)]));
}

#[test]
fn ancestors_linear_chain() {
    let (graph, [root, a, b, c]) = linear_chain();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("C".to_string(), ChangeId::from(c));

    let ast = parse("ancestors(C)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(
        result,
        HashSet::from([
            ChangeId::from(root),
            ChangeId::from(a),
            ChangeId::from(b),
            ChangeId::from(c),
        ])
    );
}

#[test]
fn ancestors_diamond() {
    let (graph, [root, a, b, d]) = diamond();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("D".to_string(), ChangeId::from(d));

    let ast = parse("ancestors(D)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(
        result,
        HashSet::from([
            ChangeId::from(root),
            ChangeId::from(a),
            ChangeId::from(b),
            ChangeId::from(d),
        ])
    );
}

#[test]
fn ancestors_intermediate_node() {
    let (graph, [root, a, b, _c]) = linear_chain();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("B".to_string(), ChangeId::from(b));

    let ast = parse("ancestors(B)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(root), ChangeId::from(a), ChangeId::from(b)]));
}

#[test]
fn ancestors_of_union() {
    let (graph, [root, a, b, _d]) = diamond();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));

    let ast = parse("ancestors(A | B)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(root), ChangeId::from(a), ChangeId::from(b)]));
}

#[test]
fn ancestors_wrong_arity_zero_args() {
    let mut graph = ChangeGraph::new();
    let _ = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };
    let ast = parse("ancestors()").unwrap();
    assert!(compile_change_ids(&ast, Arc::clone(&graph), &mut resolver).is_err());
}

#[test]
fn ancestors_wrong_arity_two_args() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let b = make_change(&mut graph, HashSet::new(), "b");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));

    let ast = parse("ancestors(A, B)").unwrap();
    assert!(compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).is_err());
}

// ---------------------------------------------------------------------------
// range() function
// ---------------------------------------------------------------------------

#[test]
fn range_linear_chain() {
    let (graph, [_root, a, b, c]) = linear_chain();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("C".to_string(), ChangeId::from(c));

    let ast = parse("range(A, C)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // range(A, C) = ancestors(C) - ancestors(A) = {B, C}
    assert_eq!(result, HashSet::from([ChangeId::from(b), ChangeId::from(c)]));
}

#[test]
fn range_same_endpoint() {
    let (graph, [_root, a, b, _c]) = linear_chain();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));

    let ast = parse("range(A, B)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // range(A, B) = ancestors(B) - ancestors(A) = {B}
    assert_eq!(result, HashSet::from([ChangeId::from(b)]));
}

#[test]
fn range_diamond() {
    let (graph, [_root, a, b, d]) = diamond();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("D".to_string(), ChangeId::from(d));

    let ast = parse("range(A, D)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // range(A, D) = ancestors(D) - ancestors(A) = {B, D}
    assert_eq!(result, HashSet::from([ChangeId::from(b), ChangeId::from(d)]));
}

#[test]
fn range_wrong_arity() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));

    let ast = parse("range(A)").unwrap();
    assert!(compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).is_err());
}

// ---------------------------------------------------------------------------
// symmetric() function
// ---------------------------------------------------------------------------

#[test]
fn symmetric_linear_chain() {
    let (graph, [_root, a, b, c]) = linear_chain();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("C".to_string(), ChangeId::from(c));

    let ast = parse("symmetric(A, C)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // ancestors(A) = {root, a}, ancestors(C) = {root, a, b, c}
    // symmetric_difference = {b, c}
    assert_eq!(result, HashSet::from([ChangeId::from(b), ChangeId::from(c)]));
}

#[test]
fn symmetric_disjoint() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let b = make_change(&mut graph, HashSet::new(), "b");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));

    let ast = parse("symmetric(A, B)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // Both are roots, each has only itself as ancestor
    // symmetric_difference of {a} and {b} = {a, b}
    assert_eq!(result, HashSet::from([ChangeId::from(a), ChangeId::from(b)]));
}

#[test]
fn symmetric_same_set() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(a));

    let ast = parse("symmetric(A, B)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // Same set → symmetric_difference is empty
    assert!(result.is_empty());
}

#[test]
fn symmetric_wrong_arity() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));

    let ast = parse("symmetric(A)").unwrap();
    assert!(compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).is_err());
}

// ---------------------------------------------------------------------------
// merge_base() function
// ---------------------------------------------------------------------------

#[test]
fn merge_base_linear_chain() {
    let (graph, [_root, a, _b, c]) = linear_chain();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("C".to_string(), ChangeId::from(c));

    let ast = parse("merge_base(A, C)").unwrap();
    let result: Vec<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], ChangeId::from(a));
}

#[test]
fn merge_base_diamond() {
    let (graph, [root, a, b, _d]) = diamond();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));

    let ast = parse("merge_base(A, B)").unwrap();
    let result: Vec<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], ChangeId::from(root));
}

#[test]
fn merge_base_disjoint() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let b = make_change(&mut graph, HashSet::new(), "b");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));

    let ast = parse("merge_base(A, B)").unwrap();
    let result: Vec<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert!(result.is_empty());
}

#[test]
fn merge_base_wrong_arity() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));

    let ast = parse("merge_base(A)").unwrap();
    assert!(compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).is_err());
}

// ---------------------------------------------------------------------------
// touched() function
// ---------------------------------------------------------------------------

#[test]
fn touched_selects_by_path() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");

    let (author, key) = test_keypair();
    let content_hash: [u8; 32] = *blake3::hash(b"main").as_bytes();
    let main_change = Change::new(
        HashSet::from([root]),
        vec![Atom::Insert {
            at: vec!["file".to_string(), "src/main.rs".to_string()],
            content_hash,
        }],
        "touch main",
        author.clone(),
        &key,
    );
    let main_id = main_change.id;
    graph.add_change(main_change);

    let util_hash: [u8; 32] = *blake3::hash(b"util").as_bytes();
    let util_change = Change::new(
        HashSet::from([root]),
        vec![Atom::Insert {
            at: vec!["file".to_string(), "src/util.rs".to_string()],
            content_hash: util_hash,
        }],
        "touch util",
        author,
        &key,
    );
    graph.add_change(util_change);

    let graph = Arc::new(graph);
    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("touched(\"src/main.rs\")").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(main_id)]));
}

#[test]
fn touched_no_match() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");

    let (author, key) = test_keypair();
    let content_hash: [u8; 32] = *blake3::hash(b"main").as_bytes();
    let change = Change::new(
        HashSet::from([root]),
        vec![Atom::Insert {
            at: vec!["file".to_string(), "src/main.rs".to_string()],
            content_hash,
        }],
        "touch main",
        author,
        &key,
    );
    graph.add_change(change);

    let graph = Arc::new(graph);
    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("touched(\"src/util.rs\")").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver).unwrap().collect();

    assert!(result.is_empty());
}

#[test]
fn touched_multiple_matches() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");

    let (author, key) = test_keypair();
    let h1: [u8; 32] = *blake3::hash(b"c1").as_bytes();
    let c1 = Change::new(
        HashSet::from([root]),
        vec![Atom::Insert {
            at: vec!["file".to_string(), "src/main.rs".to_string()],
            content_hash: h1,
        }],
        "c1",
        author.clone(),
        &key,
    );
    let id1 = c1.id;
    graph.add_change(c1);

    let h2: [u8; 32] = *blake3::hash(b"c2").as_bytes();
    let c2 = Change::new(
        HashSet::from([root]),
        vec![Atom::Insert {
            at: vec!["file".to_string(), "src/main.rs".to_string()],
            content_hash: h2,
        }],
        "c2",
        author,
        &key,
    );
    let id2 = c2.id;
    graph.add_change(c2);

    let graph = Arc::new(graph);
    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("touched(\"src/main.rs\")").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(id1), ChangeId::from(id2)]));
}

#[test]
fn touched_rejects_non_string_arg() {
    let mut graph = ChangeGraph::new();
    let _ = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("touched(@)").unwrap();
    let err = compile_change_ids_expect_err(&ast, Arc::clone(&graph), &mut resolver);
    assert!(err.to_string().contains("expects a string path argument"));
}

#[test]
fn touched_rejects_zero_args() {
    let mut graph = ChangeGraph::new();
    let _ = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("touched()").unwrap();
    let err = compile_change_ids_expect_err(&ast, Arc::clone(&graph), &mut resolver);
    assert!(err.to_string().contains("exactly one argument"));
}

#[test]
fn touched_rejects_two_args() {
    let mut graph = ChangeGraph::new();
    let _ = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("touched(\"a\", \"b\")").unwrap();
    let err = compile_change_ids_expect_err(&ast, Arc::clone(&graph), &mut resolver);
    assert!(err.to_string().contains("exactly one argument"));
}

// ---------------------------------------------------------------------------
// Reference functions: tags(), remote_branches(), bookmarks()
// ---------------------------------------------------------------------------

#[test]
fn tags_resolves_via_ref_resolver() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let a = make_change(&mut graph, HashSet::from([root]), "a");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };
    let mut refs = MockRefResolver::new().with("tags", BTreeSet::from([ChangeId::from(a)]));

    let ast = parse("tags()").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids_with_refs(&ast, Arc::clone(&graph), &mut resolver, &mut refs)
            .unwrap()
            .collect();

    assert_eq!(result, HashSet::from([ChangeId::from(a)]));
}

#[test]
fn remote_branches_resolves_via_ref_resolver() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let b = make_change(&mut graph, HashSet::from([root]), "b");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };
    let mut refs =
        MockRefResolver::new().with("remote_branches", BTreeSet::from([ChangeId::from(b)]));

    let ast = parse("remote_branches()").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids_with_refs(&ast, Arc::clone(&graph), &mut resolver, &mut refs)
            .unwrap()
            .collect();

    assert_eq!(result, HashSet::from([ChangeId::from(b)]));
}

#[test]
fn bookmarks_resolves_via_ref_resolver() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let c = make_change(&mut graph, HashSet::from([root]), "c");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };
    let mut refs = MockRefResolver::new().with("bookmarks", BTreeSet::from([ChangeId::from(c)]));

    let ast = parse("bookmarks()").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids_with_refs(&ast, Arc::clone(&graph), &mut resolver, &mut refs)
            .unwrap()
            .collect();

    assert_eq!(result, HashSet::from([ChangeId::from(c)]));
}

#[test]
fn tags_rejects_args() {
    let mut graph = ChangeGraph::new();
    let _ = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };
    let mut refs = MockRefResolver::new();

    let ast = parse("tags(@)").unwrap();
    let err =
        compile_change_ids_with_refs_expect_err(&ast, Arc::clone(&graph), &mut resolver, &mut refs);
    assert!(err.to_string().contains("expects no arguments"));
}

#[test]
fn remote_branches_rejects_args() {
    let mut graph = ChangeGraph::new();
    let _ = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };
    let mut refs = MockRefResolver::new();

    let ast = parse("remote_branches(@)").unwrap();
    let err =
        compile_change_ids_with_refs_expect_err(&ast, Arc::clone(&graph), &mut resolver, &mut refs);
    assert!(err.to_string().contains("expects no arguments"));
}

#[test]
fn bookmarks_rejects_args() {
    let mut graph = ChangeGraph::new();
    let _ = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };
    let mut refs = MockRefResolver::new();

    let ast = parse("bookmarks(@)").unwrap();
    let err =
        compile_change_ids_with_refs_expect_err(&ast, Arc::clone(&graph), &mut resolver, &mut refs);
    assert!(err.to_string().contains("expects no arguments"));
}

#[test]
fn tags_without_ref_resolver_errors() {
    let mut graph = ChangeGraph::new();
    let _ = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("tags()").unwrap();
    let err = compile_change_ids_expect_err(&ast, Arc::clone(&graph), &mut resolver);
    assert!(err.to_string().contains("requires reference resolver"));
}

#[test]
fn tags_union_with_symbol() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let a = make_change(&mut graph, HashSet::from([root]), "a");
    let b = make_change(&mut graph, HashSet::from([root]), "b");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("B".to_string(), ChangeId::from(b));

    let mut resolver = resolver_fn(map);
    let mut refs = MockRefResolver::new().with("tags", BTreeSet::from([ChangeId::from(a)]));

    let ast = parse("tags() | B").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids_with_refs(&ast, Arc::clone(&graph), &mut resolver, &mut refs)
            .unwrap()
            .collect();

    assert_eq!(result, HashSet::from([ChangeId::from(a), ChangeId::from(b)]));
}

// ---------------------------------------------------------------------------
// Unsupported function
// ---------------------------------------------------------------------------

#[test]
fn unsupported_function_returns_error() {
    let mut graph = ChangeGraph::new();
    let _ = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("nonexistent()").unwrap();
    let err = compile_change_ids_expect_err(&ast, Arc::clone(&graph), &mut resolver);
    assert!(err.to_string().contains("unsupported revset function"));
}

// ---------------------------------------------------------------------------
// Complex nested expressions
// ---------------------------------------------------------------------------

#[test]
fn nested_ancestors_of_union() {
    let (graph, [root, a, b, d]) = diamond();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("D".to_string(), ChangeId::from(d));

    // ancestors(A | D) = ancestors(A) ∪ ancestors(D)
    let ast = parse("ancestors(A | D)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(
        result,
        HashSet::from([
            ChangeId::from(root),
            ChangeId::from(a),
            ChangeId::from(b),
            ChangeId::from(d),
        ])
    );
}

#[test]
fn union_of_ancestors() {
    let (graph, [root, a, b, _d]) = diamond();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));

    // ancestors(A) | ancestors(B)
    let ast = parse("ancestors(A) | ancestors(B)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // ancestors(A) = {root, a}, ancestors(B) = {root, b}
    // union = {root, a, b}
    assert_eq!(result, HashSet::from([ChangeId::from(root), ChangeId::from(a), ChangeId::from(b)]));
}

#[test]
fn intersection_of_ancestors() {
    let (graph, [root, a, b, _d]) = diamond();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));

    // ancestors(A) & ancestors(B)
    let ast = parse("ancestors(A) & ancestors(B)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // ancestors(A) = {root, a}, ancestors(B) = {root, b}
    // intersection = {root}
    assert_eq!(result, HashSet::from([ChangeId::from(root)]));
}

#[test]
fn range_then_ancestors() {
    let (graph, [root, a, b, c]) = linear_chain();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("C".to_string(), ChangeId::from(c));

    // range(A, C) = {b, c}
    // ancestors(range(A, C)) = ancestors({b, c}) = {root, a, b, c}
    let ast = parse("ancestors(range(A, C))").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(
        result,
        HashSet::from([
            ChangeId::from(root),
            ChangeId::from(a),
            ChangeId::from(b),
            ChangeId::from(c),
        ])
    );
}

// ---------------------------------------------------------------------------
// RevsetEvaluator struct
// ---------------------------------------------------------------------------

#[test]
fn evaluator_basic() {
    let (graph, [root, a, b, c]) = linear_chain();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("C".to_string(), ChangeId::from(c));

    let ast = parse("ancestors(C)").unwrap();
    let mut resolver = resolver_fn(map);
    let mut refs = MockRefResolver::new();

    let mut evaluator = RevsetEvaluator::new(Arc::clone(&graph), &mut resolver, &mut refs);
    let result: HashSet<ChangeId> = evaluator.evaluate(&ast).unwrap().collect();

    assert_eq!(
        result,
        HashSet::from([
            ChangeId::from(root),
            ChangeId::from(a),
            ChangeId::from(b),
            ChangeId::from(c),
        ])
    );
}

#[test]
fn evaluator_with_ref_resolver() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");
    let a = make_change(&mut graph, HashSet::from([root]), "a");
    let graph = Arc::new(graph);

    let map = SymbolMap::new();
    let ast = parse("tags()").unwrap();
    let mut resolver = resolver_fn(map);
    let mut refs = MockRefResolver::new().with("tags", BTreeSet::from([ChangeId::from(a)]));

    let mut evaluator = RevsetEvaluator::new(Arc::clone(&graph), &mut resolver, &mut refs);
    let result: HashSet<ChangeId> = evaluator.evaluate(&ast).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(a)]));
}

// ---------------------------------------------------------------------------
// Empty graph
// ---------------------------------------------------------------------------

#[test]
fn empty_graph_symbol_errors() {
    let graph = Arc::new(ChangeGraph::new());
    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("main").unwrap();
    let err = compile_change_ids_expect_err(&ast, Arc::clone(&graph), &mut resolver);
    assert!(err.to_string().contains("unknown revset symbol"));
}

#[test]
fn empty_graph_ancestors_empty() {
    let mut graph = ChangeGraph::new();
    // We can't put anything in an empty graph, but we can test an ancestors
    // call with a symbol that resolves to nothing via the ref resolver
    // Actually, ancestors() needs at least one valid starting point.
    // Let's use the ref resolver to inject a reference
    let root = make_change(&mut graph, HashSet::new(), "root");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("ROOT".to_string(), ChangeId::from(root));

    let ast = parse("ancestors(ROOT)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(root)]));
}

#[test]
fn empty_graph_tags_empty() {
    let graph = Arc::new(ChangeGraph::new());
    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };
    let mut refs = MockRefResolver::new();

    let ast = parse("tags()").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids_with_refs(&ast, Arc::clone(&graph), &mut resolver, &mut refs)
            .unwrap()
            .collect();

    assert!(result.is_empty());
}

// ---------------------------------------------------------------------------
// UnionIterator deduplication
// ---------------------------------------------------------------------------

#[test]
fn union_deduplicates_across_sides() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let graph = Arc::new(graph);

    // Both A and B resolve to the same change
    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(a));

    let ast = parse("A | B").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(result.len(), 1);
}

#[test]
fn union_preserves_left_before_right_order() {
    let mut graph = ChangeGraph::new();
    let a = make_change(&mut graph, HashSet::new(), "a");
    let b = make_change(&mut graph, HashSet::new(), "b");
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));

    let ast = parse("A | B").unwrap();
    let result: Vec<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // Left side (A) should come before right side (B)
    assert_eq!(result.first(), Some(&ChangeId::from(a)));
    assert_eq!(result.last(), Some(&ChangeId::from(b)));
}

// ---------------------------------------------------------------------------
// AncestorsIterator BFS ordering
// ---------------------------------------------------------------------------

#[test]
fn ancestors_iterator_returns_all_nodes() {
    let (graph, [root, a, b, c]) = linear_chain();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("C".to_string(), ChangeId::from(c));

    let ast = parse("ancestors(C)").unwrap();
    let result: Vec<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // BFS from C produces [c, b, a, root] — all 4 nodes present
    assert_eq!(result.len(), 4);
    assert!(result.contains(&ChangeId::from(root)));
    assert!(result.contains(&ChangeId::from(a)));
    assert!(result.contains(&ChangeId::from(b)));
    assert!(result.contains(&ChangeId::from(c)));
}

#[test]
fn ancestors_iterator_no_duplicates() {
    let (graph, [_root, _a, _b, d]) = diamond();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("D".to_string(), ChangeId::from(d));

    let ast = parse("ancestors(D)").unwrap();
    let result: Vec<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // Each node should appear exactly once
    let mut seen = HashSet::new();
    for id in &result {
        assert!(seen.insert(*id), "duplicate in ancestors iterator: {id:?}");
    }
}

// ---------------------------------------------------------------------------
// Merge scenario with complex revset
// ---------------------------------------------------------------------------

#[test]
fn merge_base_of_merge_scenario() {
    let (graph, [root, a, b, _merge]) = merge_scenario();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));

    let ast = parse("merge_base(A, B)").unwrap();
    let result: Vec<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0], ChangeId::from(root));
}

#[test]
fn range_in_merge_scenario() {
    let (graph, [_root, a, b, merge]) = merge_scenario();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("MERGE".to_string(), ChangeId::from(merge));

    let ast = parse("range(A, MERGE)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // range(A, MERGE) = ancestors(MERGE) - ancestors(A) = {B, MERGE}
    assert_eq!(result, HashSet::from([ChangeId::from(b), ChangeId::from(merge)]));
}

// ---------------------------------------------------------------------------
// Two-root diamond scenario
// ---------------------------------------------------------------------------

#[test]
fn ancestors_two_roots_diamond() {
    let (graph, [r1, r2, a, b, d]) = diamond_two_roots();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("D".to_string(), ChangeId::from(d));

    let ast = parse("ancestors(D)").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(
        result,
        HashSet::from([
            ChangeId::from(r1),
            ChangeId::from(r2),
            ChangeId::from(a),
            ChangeId::from(b),
            ChangeId::from(d),
        ])
    );
}

// ---------------------------------------------------------------------------
// Fan scenario
// ---------------------------------------------------------------------------

#[test]
fn fan_union_of_all_children() {
    let (graph, [_root, a, b, c, d]) = fan();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));
    map.insert("C".to_string(), ChangeId::from(c));
    map.insert("D".to_string(), ChangeId::from(d));

    let ast = parse("A | B | C | D").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    assert_eq!(
        result,
        HashSet::from(
            [ChangeId::from(a), ChangeId::from(b), ChangeId::from(c), ChangeId::from(d),]
        )
    );
}

#[test]
fn fan_intersection_of_all_children_empty() {
    let (graph, [_root, a, b, c, d]) = fan();
    let graph = Arc::new(graph);

    let mut map = SymbolMap::new();
    map.insert("A".to_string(), ChangeId::from(a));
    map.insert("B".to_string(), ChangeId::from(b));
    map.insert("C".to_string(), ChangeId::from(c));
    map.insert("D".to_string(), ChangeId::from(d));

    let ast = parse("A & B & C & D").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver(&map)).unwrap().collect();

    // All are siblings, no overlap
    assert!(result.is_empty());
}

// ---------------------------------------------------------------------------
// touched() with Delete atom
// ---------------------------------------------------------------------------

#[test]
fn touched_matches_delete_atom() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");

    let (author, key) = test_keypair();
    let content_hash: [u8; 32] = *blake3::hash(b"del").as_bytes();
    let change = Change::new(
        HashSet::from([root]),
        vec![Atom::Delete {
            at: vec!["file".to_string(), "old.rs".to_string()],
            prior_hash: content_hash,
        }],
        "delete old",
        author,
        &key,
    );
    let id = change.id;
    graph.add_change(change);

    let graph = Arc::new(graph);
    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("touched(\"old.rs\")").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(id)]));
}

// ---------------------------------------------------------------------------
// touched() with Directory atom
// ---------------------------------------------------------------------------

#[test]
fn touched_matches_directory_atom() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");

    let (author, key) = test_keypair();
    let change = Change::new(
        HashSet::from([root]),
        vec![Atom::Directory { path: vec!["src".to_string()] }],
        "add dir",
        author,
        &key,
    );
    let id = change.id;
    graph.add_change(change);

    let graph = Arc::new(graph);
    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("touched(\"src\")").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(id)]));
}

// ---------------------------------------------------------------------------
// touched() with Blob atom
// ---------------------------------------------------------------------------

#[test]
fn touched_matches_blob_atom() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");

    let (author, key) = test_keypair();
    let content_hash = blake3::hash(b"blob");
    let change = Change::new(
        HashSet::from([root]),
        vec![Atom::Blob { path: "data.bin".to_string(), hash: content_hash, size: 1024 }],
        "add blob",
        author,
        &key,
    );
    let id = change.id;
    graph.add_change(change);

    let graph = Arc::new(graph);
    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    let ast = parse("touched(\"data.bin\")").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(id)]));
}

// ---------------------------------------------------------------------------
// Move atom doesn't match touched() path
// ---------------------------------------------------------------------------

#[test]
fn touched_does_not_match_move_atom() {
    let mut graph = ChangeGraph::new();
    let root = make_change(&mut graph, HashSet::new(), "root");

    let (author, key) = test_keypair();
    let from: Vec<String> = vec!["file".to_string(), "old.rs".to_string()];
    let to: Vec<String> = vec!["file".to_string(), "new.rs".to_string()];
    let change = Change::new(
        HashSet::from([root]),
        vec![Atom::Move { from, to }],
        "move file",
        author,
        &key,
    );
    let _id = change.id;
    graph.add_change(change);

    let graph = Arc::new(graph);
    let mut resolver = |_name: &str| -> anyhow::Result<Option<ChangeId>> { Ok(None) };

    // Move atom has from/to paths; touched("old.rs") matches via from=["file", "old.rs"]
    let ast = parse("touched(\"old.rs\")").unwrap();
    let result: HashSet<ChangeId> =
        compile_change_ids(&ast, Arc::clone(&graph), &mut resolver).unwrap().collect();

    assert_eq!(result, HashSet::from([ChangeId::from(_id)]));
}
