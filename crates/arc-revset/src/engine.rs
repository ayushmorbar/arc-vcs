use std::{
    collections::{BTreeSet, HashSet, VecDeque},
    sync::Arc,
};

use anyhow::{Result, anyhow, bail};
use arc_algebra_types::Blake3Hash;
use arc_change::Change;
use arc_store_graph::ChangeGraph;
use arc_store_types::newtypes::ChangeId;

use crate::parser::RevsetExpression;

/// Iterator type produced by revset compilation.
pub type RevsetIterator<'a> = Box<dyn Iterator<Item = Blake3Hash> + 'a>;
/// Typed iterator over strongly-typed change identifiers.
pub type RevsetChangeIdIterator<'a> = Box<dyn Iterator<Item = ChangeId> + 'a>;

/// Resolver for metadata-backed reference functions such as `tags()`.
pub trait ReferenceResolver {
    /// Resolve function-backed reference heads by function name.
    fn resolve_reference_heads(&mut self, function_name: &str) -> Result<BTreeSet<ChangeId>>;
}

impl<F> ReferenceResolver for F
where
    F: FnMut(&str) -> Result<BTreeSet<ChangeId>>,
{
    fn resolve_reference_heads(&mut self, function_name: &str) -> Result<BTreeSet<ChangeId>> {
        self(function_name)
    }
}

/// Typed revset evaluator over a [`ChangeGraph`].
pub struct RevsetEvaluator<'g, F, R>
where
    F: FnMut(&str) -> Result<Option<ChangeId>> + ?Sized,
    R: ReferenceResolver + ?Sized,
{
    graph: Arc<ChangeGraph>,
    resolve_symbol: &'g mut F,
    resolve_refs: &'g mut R,
}

impl<'g, F, R> RevsetEvaluator<'g, F, R>
where
    F: FnMut(&str) -> Result<Option<ChangeId>> + ?Sized,
    R: ReferenceResolver + ?Sized,
{
    /// Create a new evaluator bound to `graph` and a repository-specific symbol resolver.
    pub fn new(
        graph: Arc<ChangeGraph>,
        resolve_symbol: &'g mut F,
        resolve_refs: &'g mut R,
    ) -> Self {
        Self { graph, resolve_symbol, resolve_refs }
    }

    /// Evaluate `expr` into a lazy iterator of typed [`ChangeId`] values.
    pub fn evaluate<'a>(&mut self, expr: &RevsetExpression) -> Result<RevsetChangeIdIterator<'a>> {
        compile_impl_change_ids(
            expr,
            Arc::clone(&self.graph),
            self.resolve_symbol,
            self.resolve_refs,
        )
    }
}

/// Compile a revset AST into a lazy hash iterator.
///
/// `resolve_symbol` handles repository-specific symbols such as `@` and view
/// names. Raw full hex hashes are parsed directly by this compiler.
pub fn compile<'a, F>(
    expr: &RevsetExpression,
    graph: Arc<ChangeGraph>,
    resolve_symbol: &mut F,
) -> Result<RevsetIterator<'a>>
where
    F: FnMut(&str) -> Result<Option<Blake3Hash>>,
{
    let mut typed_resolver =
        |symbol: &str| resolve_symbol(symbol).map(|opt| opt.map(ChangeId::from));
    let typed_iter = compile_change_ids(expr, graph, &mut typed_resolver)?;
    Ok(Box::new(typed_iter.map(Blake3Hash::from)))
}

/// Compile a revset AST into a typed lazy [`ChangeId`] iterator.
pub fn compile_change_ids<'a, F>(
    expr: &RevsetExpression,
    graph: Arc<ChangeGraph>,
    resolve_symbol: &mut F,
) -> Result<RevsetChangeIdIterator<'a>>
where
    F: FnMut(&str) -> Result<Option<ChangeId>>,
{
    let mut missing_refs = |name: &str| {
        bail!(
            "revset function '{name}' requires reference resolver; use \
             compile_change_ids_with_refs()"
        )
    };
    compile_impl_change_ids(expr, graph, resolve_symbol, &mut missing_refs)
}

/// Compile a revset AST with explicit support for metadata-backed ref functions.
pub fn compile_change_ids_with_refs<'a, F, R>(
    expr: &RevsetExpression,
    graph: Arc<ChangeGraph>,
    resolve_symbol: &mut F,
    resolve_refs: &mut R,
) -> Result<RevsetChangeIdIterator<'a>>
where
    F: FnMut(&str) -> Result<Option<ChangeId>>,
    R: ReferenceResolver,
{
    compile_impl_change_ids(expr, graph, resolve_symbol, resolve_refs)
}

fn compile_impl_change_ids<'a, F, R>(
    expr: &RevsetExpression,
    graph: Arc<ChangeGraph>,
    resolve_symbol: &mut F,
    resolve_refs: &mut R,
) -> Result<RevsetChangeIdIterator<'a>>
where
    F: FnMut(&str) -> Result<Option<ChangeId>> + ?Sized,
    R: ReferenceResolver + ?Sized,
{
    match expr {
        RevsetExpression::Symbol(name) => compile_symbol(name, &graph, resolve_symbol),
        RevsetExpression::StringLiteral(_) => {
            bail!("string literals are only valid as function arguments")
        }
        RevsetExpression::Union(left, right) => {
            let left_iter =
                compile_impl_change_ids(left, Arc::clone(&graph), resolve_symbol, resolve_refs)?;
            let right_iter = compile_impl_change_ids(right, graph, resolve_symbol, resolve_refs)?;
            Ok(Box::new(UnionIterator::new(left_iter, right_iter)))
        }
        RevsetExpression::Intersection(left, right) => {
            let left_iter =
                compile_impl_change_ids(left, Arc::clone(&graph), resolve_symbol, resolve_refs)?;
            let right_iter = compile_impl_change_ids(right, graph, resolve_symbol, resolve_refs)?;
            let right_set: HashSet<ChangeId> = right_iter.collect();
            Ok(Box::new(left_iter.filter(move |hash| right_set.contains(hash))))
        }
        RevsetExpression::Function { name, args } => {
            compile_function(name, args, graph, resolve_symbol, resolve_refs)
        }
    }
}

fn compile_symbol<'a, F>(
    name: &str,
    graph: &ChangeGraph,
    resolve_symbol: &mut F,
) -> Result<RevsetChangeIdIterator<'a>>
where
    F: FnMut(&str) -> Result<Option<ChangeId>> + ?Sized,
{
    if let Ok(id) = ChangeId::from_hex(name) {
        if graph.get(&Blake3Hash::from(id)).is_none() {
            bail!("unknown revset symbol '{name}'");
        }
        return Ok(Box::new(std::iter::once(id)));
    }

    if let Some(hash) = parse_hex_hash(name) {
        if graph.get(&hash).is_none() {
            bail!("unknown revset symbol '{name}'");
        }
        return Ok(Box::new(std::iter::once(ChangeId::from(hash))));
    }

    if let Some(id) = resolve_symbol(name)? {
        return Ok(Box::new(std::iter::once(id)));
    }

    bail!("unknown revset symbol '{name}'")
}

fn compile_function<'a, F, R>(
    name: &str,
    args: &[RevsetExpression],
    graph: Arc<ChangeGraph>,
    resolve_symbol: &mut F,
    resolve_refs: &mut R,
) -> Result<RevsetChangeIdIterator<'a>>
where
    F: FnMut(&str) -> Result<Option<ChangeId>> + ?Sized,
    R: ReferenceResolver + ?Sized,
{
    match name {
        "ancestors" => {
            if args.len() != 1 {
                bail!("ancestors() expects exactly one argument");
            }
            let arg =
                args.first().ok_or_else(|| anyhow!("ancestors() expects exactly one argument"))?;
            let starts_iter =
                compile_impl_change_ids(arg, Arc::clone(&graph), resolve_symbol, resolve_refs)?;
            let starts = starts_iter.collect::<Vec<_>>();
            Ok(Box::new(AncestorsIterator::new(graph, starts)))
        }
        "tags" | "remote_branches" | "bookmarks" => {
            if !args.is_empty() {
                bail!("{name}() expects no arguments");
            }
            let heads = resolve_refs.resolve_reference_heads(name)?;
            Ok(Box::new(heads.into_iter()))
        }
        "touched" => {
            let path = parse_single_string_arg(name, args)?;
            let selected: BTreeSet<ChangeId> = graph
                .iter()
                .filter(|change| change_touches_repo_path(change, &path))
                .map(|change| ChangeId::from(change.id))
                .collect();
            Ok(Box::new(selected.into_iter()))
        }
        "range" => {
            let [from_arg, to_arg] = expect_two_args(name, args)?;
            let from_heads =
                eval_as_head_set(from_arg, Arc::clone(&graph), resolve_symbol, resolve_refs)?;
            let to_heads =
                eval_as_head_set(to_arg, Arc::clone(&graph), resolve_symbol, resolve_refs)?;

            let from_ancestors = graph.ancestors(&from_heads);
            let to_ancestors = graph.ancestors(&to_heads);
            let selected: BTreeSet<ChangeId> =
                to_ancestors.difference(&from_ancestors).copied().map(ChangeId::from).collect();
            Ok(Box::new(selected.into_iter()))
        }
        "symmetric" => {
            let [left_arg, right_arg] = expect_two_args(name, args)?;
            let left_heads =
                eval_as_head_set(left_arg, Arc::clone(&graph), resolve_symbol, resolve_refs)?;
            let right_heads =
                eval_as_head_set(right_arg, Arc::clone(&graph), resolve_symbol, resolve_refs)?;

            let left_ancestors = graph.ancestors(&left_heads);
            let right_ancestors = graph.ancestors(&right_heads);

            let selected: BTreeSet<ChangeId> = left_ancestors
                .symmetric_difference(&right_ancestors)
                .copied()
                .map(ChangeId::from)
                .collect();
            Ok(Box::new(selected.into_iter()))
        }
        "merge_base" => {
            let [left_arg, right_arg] = expect_two_args(name, args)?;
            let left_heads =
                eval_as_head_set(left_arg, Arc::clone(&graph), resolve_symbol, resolve_refs)?;
            let right_heads =
                eval_as_head_set(right_arg, Arc::clone(&graph), resolve_symbol, resolve_refs)?;

            let selected = graph
                .merge_base_deterministic(&left_heads, &right_heads)
                .map(ChangeId::from)
                .into_iter()
                .collect::<BTreeSet<_>>();
            Ok(Box::new(selected.into_iter()))
        }
        _ => bail!("unsupported revset function '{name}'"),
    }
}

fn expect_two_args<'a>(
    name: &str,
    args: &'a [RevsetExpression],
) -> Result<[&'a RevsetExpression; 2]> {
    match args {
        [left, right] => Ok([left, right]),
        _ => bail!("{name}() expects exactly two arguments"),
    }
}

fn eval_as_head_set<F, R>(
    expr: &RevsetExpression,
    graph: Arc<ChangeGraph>,
    resolve_symbol: &mut F,
    resolve_refs: &mut R,
) -> Result<HashSet<Blake3Hash>>
where
    F: FnMut(&str) -> Result<Option<ChangeId>> + ?Sized,
    R: ReferenceResolver + ?Sized,
{
    let iter = compile_impl_change_ids(expr, graph, resolve_symbol, resolve_refs)?;
    Ok(iter.map(Blake3Hash::from).collect())
}

fn parse_single_string_arg(name: &str, args: &[RevsetExpression]) -> Result<String> {
    if args.len() != 1 {
        bail!("{name}() expects exactly one argument");
    }

    match args.first() {
        Some(RevsetExpression::StringLiteral(value)) => Ok(value.clone()),
        _ => bail!("{name}() expects a string path argument"),
    }
}

fn change_touches_repo_path(change: &Change, path: &str) -> bool {
    change.atoms.iter().any(|atom| {
        atom.paths().into_iter().any(|node_path| {
            if node_path.first().is_some_and(|segment| segment == "file") {
                return node_path.get(1).is_some_and(|segment| segment == path);
            }
            node_path.first().is_some_and(|segment| segment == path)
        })
    })
}

struct UnionIterator<'a> {
    left: RevsetChangeIdIterator<'a>,
    right: RevsetChangeIdIterator<'a>,
    seen: HashSet<ChangeId>,
    on_left: bool,
}

impl<'a> UnionIterator<'a> {
    fn new(left: RevsetChangeIdIterator<'a>, right: RevsetChangeIdIterator<'a>) -> Self {
        Self { left, right, seen: HashSet::new(), on_left: true }
    }
}

impl<'a> Iterator for UnionIterator<'a> {
    type Item = ChangeId;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.on_left {
                match self.left.next() {
                    Some(hash) => {
                        if self.seen.insert(hash) {
                            return Some(hash);
                        }
                        continue;
                    }
                    None => self.on_left = false,
                }
            }

            {
                let hash = self.right.next()?;
                if self.seen.insert(hash) {
                    return Some(hash);
                }
            }
        }
    }
}

struct AncestorsIterator {
    graph: Arc<ChangeGraph>,
    queue: VecDeque<ChangeId>,
    seen: HashSet<ChangeId>,
}

impl AncestorsIterator {
    fn new(graph: Arc<ChangeGraph>, starts: Vec<ChangeId>) -> Self {
        Self { graph, queue: starts.into(), seen: HashSet::new() }
    }
}

impl Iterator for AncestorsIterator {
    type Item = ChangeId;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(current) = self.queue.pop_front() {
            if !self.seen.insert(current) {
                continue;
            }

            if let Some(change) = self.graph.get(&Blake3Hash::from(current)) {
                let mut deps: Vec<ChangeId> =
                    change.deps.iter().copied().map(ChangeId::from).collect();
                deps.sort();
                for dep in deps {
                    if !self.seen.contains(&dep) {
                        self.queue.push_back(dep);
                    }
                }
            }

            return Some(current);
        }
        None
    }
}

fn parse_hex_hash(input: &str) -> Option<Blake3Hash> {
    if input.len() != 64 {
        return None;
    }

    let mut out = [0u8; 32];
    for (idx, chunk) in input.as_bytes().chunks_exact(2).enumerate() {
        let pair = std::str::from_utf8(chunk).ok()?;
        out[idx] = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use arc_algebra_types::Atom;
    use arc_store_types::author::test_keypair;

    use super::*;
    use crate::parser::parse;

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

    #[test]
    fn compiles_ancestors_union_symbol() {
        let mut graph = ChangeGraph::new();
        let root = make_change(&mut graph, HashSet::new(), "root");
        let a = make_change(&mut graph, HashSet::from([root]), "a");
        let b = make_change(&mut graph, HashSet::from([root]), "b");

        let graph = Arc::new(graph);
        let ast = parse("ancestors(A) | B").expect("query should parse");
        let mut resolver = |name: &str| -> Result<Option<ChangeId>> {
            match name {
                "A" => Ok(Some(ChangeId::from(a))),
                "B" => Ok(Some(ChangeId::from(b))),
                _ => Ok(None),
            }
        };

        let iter = compile_change_ids(&ast, Arc::clone(&graph), &mut resolver)
            .expect("compile should work");
        let result: HashSet<ChangeId> = iter.collect();

        let expected = HashSet::from([ChangeId::from(a), ChangeId::from(b), ChangeId::from(root)]);
        assert_eq!(result, expected);
    }

    #[test]
    fn rejects_ancestors_with_no_args() {
        let graph = Arc::new(ChangeGraph::new());
        let ast = parse("ancestors()").expect("query should parse");
        let mut resolver = |_name: &str| -> Result<Option<ChangeId>> { Ok(None) };

        let err = match compile_change_ids(&ast, graph, &mut resolver) {
            Ok(_) => panic!("compile should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("exactly one argument"), "unexpected error: {err}");
    }

    #[test]
    fn rejects_ancestors_with_many_args() {
        let mut graph = ChangeGraph::new();
        let a = make_change(&mut graph, HashSet::new(), "a");
        let b = make_change(&mut graph, HashSet::new(), "b");
        let graph = Arc::new(graph);

        let ast = parse("ancestors(A, B)").expect("query should parse");
        let mut resolver = move |name: &str| -> Result<Option<ChangeId>> {
            match name {
                "A" => Ok(Some(ChangeId::from(a))),
                "B" => Ok(Some(ChangeId::from(b))),
                _ => Ok(None),
            }
        };

        let err = match compile_change_ids(&ast, graph, &mut resolver) {
            Ok(_) => panic!("compile should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("exactly one argument"), "unexpected error: {err}");
    }

    #[test]
    fn unions_tags_and_remote_branches_with_mock_reference_resolver() {
        let mut graph = ChangeGraph::new();
        let root = make_change(&mut graph, HashSet::new(), "root");
        let a = make_change(&mut graph, HashSet::from([root]), "a");
        let b = make_change(&mut graph, HashSet::from([root]), "b");

        let graph = Arc::new(graph);
        let ast = parse("remote_branches() | tags()").expect("query should parse");
        let mut resolver = |_name: &str| -> Result<Option<ChangeId>> { Ok(None) };
        let mut refs = |name: &str| -> Result<BTreeSet<ChangeId>> {
            match name {
                "remote_branches" => Ok(BTreeSet::from([ChangeId::from(a)])),
                "tags" => Ok(BTreeSet::from([ChangeId::from(b)])),
                _ => Ok(BTreeSet::new()),
            }
        };

        let result: HashSet<ChangeId> =
            compile_change_ids_with_refs(&ast, Arc::clone(&graph), &mut resolver, &mut refs)
                .expect("compile should succeed")
                .collect();

        let expected = HashSet::from([ChangeId::from(a), ChangeId::from(b)]);
        assert_eq!(result, expected);
    }

    #[test]
    fn unions_bookmarks_with_tags_with_mock_reference_resolver() {
        let mut graph = ChangeGraph::new();
        let root = make_change(&mut graph, HashSet::new(), "root");
        let a = make_change(&mut graph, HashSet::from([root]), "a");
        let b = make_change(&mut graph, HashSet::from([root]), "b");

        let graph = Arc::new(graph);
        let ast = parse("bookmarks() | tags()").expect("query should parse");
        let mut resolver = |_name: &str| -> Result<Option<ChangeId>> { Ok(None) };
        let mut refs = |name: &str| -> Result<BTreeSet<ChangeId>> {
            match name {
                "bookmarks" => Ok(BTreeSet::from([ChangeId::from(a)])),
                "tags" => Ok(BTreeSet::from([ChangeId::from(b)])),
                _ => Ok(BTreeSet::new()),
            }
        };

        let result: HashSet<ChangeId> =
            compile_change_ids_with_refs(&ast, Arc::clone(&graph), &mut resolver, &mut refs)
                .expect("compile should succeed")
                .collect();

        let expected = HashSet::from([ChangeId::from(a), ChangeId::from(b)]);
        assert_eq!(result, expected);
    }

    #[test]
    fn touched_selects_changes_by_path() {
        let mut graph = ChangeGraph::new();
        let root = make_change(&mut graph, HashSet::new(), "root");

        let (author, key) = test_keypair();
        let main_hash: [u8; 32] = *blake3::hash(b"main").as_bytes();
        let util_hash: [u8; 32] = *blake3::hash(b"util").as_bytes();

        let main_change = Change::new(
            HashSet::from([root]),
            vec![Atom::Insert {
                at: vec!["file".to_string(), "src/main.rs".to_string(), "fn_main".to_string()],
                content_hash: main_hash,
            }],
            "touch main",
            author.clone(),
            &key,
        );
        let main_id = main_change.id;
        graph.add_change(main_change);

        let util_change = Change::new(
            HashSet::from([root]),
            vec![Atom::Insert {
                at: vec!["file".to_string(), "src/util.rs".to_string(), "fn_util".to_string()],
                content_hash: util_hash,
            }],
            "touch util",
            author,
            &key,
        );
        graph.add_change(util_change);

        let graph = Arc::new(graph);
        let ast = parse("touched(\"src/main.rs\")").expect("query should parse");
        let mut resolver = |_name: &str| -> Result<Option<ChangeId>> { Ok(None) };

        let result: HashSet<ChangeId> = compile_change_ids(&ast, Arc::clone(&graph), &mut resolver)
            .expect("compile should work")
            .collect();

        assert_eq!(result, HashSet::from([ChangeId::from(main_id)]));
    }

    #[test]
    fn touched_rejects_non_string_argument() {
        let mut graph = ChangeGraph::new();
        let _ = make_change(&mut graph, HashSet::new(), "root");
        let graph = Arc::new(graph);
        let ast = parse("touched(@)").expect("query should parse");
        let mut resolver = |_name: &str| -> Result<Option<ChangeId>> { Ok(None) };

        let err = match compile_change_ids(&ast, Arc::clone(&graph), &mut resolver) {
            Ok(_) => panic!("compile should fail"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("expects a string path argument"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn touched_rejects_wrong_arity() {
        let mut graph = ChangeGraph::new();
        let _ = make_change(&mut graph, HashSet::new(), "root");
        let graph = Arc::new(graph);
        let ast = parse("touched()").expect("query should parse");
        let mut resolver = |_name: &str| -> Result<Option<ChangeId>> { Ok(None) };

        let err = match compile_change_ids(&ast, Arc::clone(&graph), &mut resolver) {
            Ok(_) => panic!("compile should fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("exactly one argument"), "unexpected error: {err}");
    }

    #[test]
    fn range_selects_to_ancestors_minus_from_ancestors() {
        let mut graph = ChangeGraph::new();
        let root = make_change(&mut graph, HashSet::new(), "root");
        let a = make_change(&mut graph, HashSet::from([root]), "a");
        let b = make_change(&mut graph, HashSet::from([a]), "b");
        let c = make_change(&mut graph, HashSet::from([a]), "c");
        let d = make_change(&mut graph, HashSet::from([b, c]), "d");

        let graph = Arc::new(graph);
        let ast = parse("range(A, D)").expect("query should parse");
        let mut resolver = move |name: &str| -> Result<Option<ChangeId>> {
            match name {
                "A" => Ok(Some(ChangeId::from(a))),
                "D" => Ok(Some(ChangeId::from(d))),
                _ => Ok(None),
            }
        };

        let result: HashSet<ChangeId> = compile_change_ids(&ast, Arc::clone(&graph), &mut resolver)
            .expect("compile should succeed")
            .collect();

        let expected = HashSet::from([ChangeId::from(b), ChangeId::from(c), ChangeId::from(d)]);
        assert_eq!(result, expected);
    }

    #[test]
    fn symmetric_returns_xor_of_ancestor_sets() {
        let mut graph = ChangeGraph::new();
        let root = make_change(&mut graph, HashSet::new(), "root");
        let a = make_change(&mut graph, HashSet::from([root]), "a");
        let b = make_change(&mut graph, HashSet::from([a]), "b");
        let c = make_change(&mut graph, HashSet::from([a]), "c");
        let d = make_change(&mut graph, HashSet::from([b, c]), "d");
        let e = make_change(&mut graph, HashSet::from([a]), "e");

        let graph = Arc::new(graph);
        let ast = parse("symmetric(D, E)").expect("query should parse");
        let mut resolver = move |name: &str| -> Result<Option<ChangeId>> {
            match name {
                "D" => Ok(Some(ChangeId::from(d))),
                "E" => Ok(Some(ChangeId::from(e))),
                _ => Ok(None),
            }
        };

        let result: HashSet<ChangeId> = compile_change_ids(&ast, Arc::clone(&graph), &mut resolver)
            .expect("compile should succeed")
            .collect();

        let expected = HashSet::from([
            ChangeId::from(b),
            ChangeId::from(c),
            ChangeId::from(d),
            ChangeId::from(e),
        ]);
        assert_eq!(result, expected);
    }

    #[test]
    fn merge_base_returns_single_deterministic_base() {
        let mut graph = ChangeGraph::new();
        let a = make_change(&mut graph, HashSet::new(), "a");
        let b = make_change(&mut graph, HashSet::new(), "b");
        let c = make_change(&mut graph, HashSet::from([a, b]), "c");
        let d = make_change(&mut graph, HashSet::from([a, b]), "d");

        let graph = Arc::new(graph);
        let ast = parse("merge_base(C, D)").expect("query should parse");
        let mut resolver = move |name: &str| -> Result<Option<ChangeId>> {
            match name {
                "C" => Ok(Some(ChangeId::from(c))),
                "D" => Ok(Some(ChangeId::from(d))),
                _ => Ok(None),
            }
        };

        let result: Vec<ChangeId> = compile_change_ids(&ast, Arc::clone(&graph), &mut resolver)
            .expect("compile should succeed")
            .collect();

        assert_eq!(result.len(), 1, "merge_base must return a single deterministic id");
        let expected = ChangeId::from(a.min(b));
        assert_eq!(result[0], expected);
    }

    #[test]
    fn range_rejects_wrong_arity() {
        let graph = Arc::new(ChangeGraph::new());
        let ast = parse("range(@)").expect("query should parse");
        let mut resolver = |_name: &str| -> Result<Option<ChangeId>> { Ok(None) };

        let err = match compile_change_ids(&ast, Arc::clone(&graph), &mut resolver) {
            Ok(_) => panic!("range() must reject wrong arity"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("exactly two arguments"), "unexpected error: {err}");
    }

    #[test]
    fn symmetric_rejects_wrong_arity() {
        let graph = Arc::new(ChangeGraph::new());
        let ast = parse("symmetric(@)").expect("query should parse");
        let mut resolver = |_name: &str| -> Result<Option<ChangeId>> { Ok(None) };

        let err = match compile_change_ids(&ast, Arc::clone(&graph), &mut resolver) {
            Ok(_) => panic!("symmetric() must reject wrong arity"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("exactly two arguments"), "unexpected error: {err}");
    }

    #[test]
    fn merge_base_rejects_wrong_arity() {
        let graph = Arc::new(ChangeGraph::new());
        let ast = parse("merge_base(@)").expect("query should parse");
        let mut resolver = |_name: &str| -> Result<Option<ChangeId>> { Ok(None) };

        let err = match compile_change_ids(&ast, Arc::clone(&graph), &mut resolver) {
            Ok(_) => panic!("merge_base() must reject wrong arity"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("exactly two arguments"), "unexpected error: {err}");
    }

    #[test]
    fn merge_base_disjoint_returns_empty() {
        let mut graph = ChangeGraph::new();
        let x = make_change(&mut graph, HashSet::new(), "x");
        let y = make_change(&mut graph, HashSet::new(), "y");

        let graph = Arc::new(graph);
        let ast = parse("merge_base(X, Y)").expect("query should parse");
        let mut resolver = move |name: &str| -> Result<Option<ChangeId>> {
            match name {
                "X" => Ok(Some(ChangeId::from(x))),
                "Y" => Ok(Some(ChangeId::from(y))),
                _ => Ok(None),
            }
        };

        let result: Vec<ChangeId> = compile_change_ids(&ast, Arc::clone(&graph), &mut resolver)
            .expect("compile should succeed")
            .collect();
        assert!(result.is_empty(), "disjoint heads have no merge-base");
    }

    #[test]
    fn rejects_unknown_literal_hash_symbol() {
        let graph = Arc::new(ChangeGraph::new());
        let unknown = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let ast = parse(unknown).expect("query should parse");
        let mut resolver = |_name: &str| -> Result<Option<ChangeId>> { Ok(None) };

        let err = match compile_change_ids(&ast, Arc::clone(&graph), &mut resolver) {
            Ok(_) => panic!("unknown literal hash must fail"),
            Err(err) => err,
        };
        assert!(err.to_string().contains("unknown revset symbol"));
    }
}
