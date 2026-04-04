use std::collections::{BTreeSet, HashSet, VecDeque};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::algebra::Blake3Hash;
use crate::revset::RevsetExpression;
use crate::store::graph::ChangeGraph;
use crate::store::newtypes::ChangeId;

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
    F: FnMut(&str) -> Result<Option<ChangeId>>,
    R: ReferenceResolver,
{
    graph: Arc<ChangeGraph>,
    resolve_symbol: &'g mut F,
    resolve_refs: &'g mut R,
}

impl<'g, F, R> RevsetEvaluator<'g, F, R>
where
    F: FnMut(&str) -> Result<Option<ChangeId>>,
    R: ReferenceResolver,
{
    /// Create a new evaluator bound to `graph` and a repository-specific symbol resolver.
    pub fn new(graph: Arc<ChangeGraph>, resolve_symbol: &'g mut F, resolve_refs: &'g mut R) -> Self {
        Self {
            graph,
            resolve_symbol,
            resolve_refs,
        }
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
    let mut typed_resolver = |symbol: &str| resolve_symbol(symbol).map(|opt| opt.map(ChangeId::from));
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
            "revset function '{name}' requires reference resolver; use compile_change_ids_with_refs()"
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
    F: FnMut(&str) -> Result<Option<ChangeId>>,
    R: ReferenceResolver,
{
    match expr {
        RevsetExpression::Symbol(name) => compile_symbol(name, resolve_symbol),
        RevsetExpression::Union(left, right) => {
            let left_iter = compile_impl_change_ids(
                left,
                Arc::clone(&graph),
                resolve_symbol,
                resolve_refs,
            )?;
            let right_iter = compile_impl_change_ids(right, graph, resolve_symbol, resolve_refs)?;
            Ok(Box::new(UnionIterator::new(left_iter, right_iter)))
        }
        RevsetExpression::Intersection(left, right) => {
            let left_iter = compile_impl_change_ids(
                left,
                Arc::clone(&graph),
                resolve_symbol,
                resolve_refs,
            )?;
            let right_iter = compile_impl_change_ids(right, graph, resolve_symbol, resolve_refs)?;
            let right_set: HashSet<ChangeId> = right_iter.collect();
            Ok(Box::new(
                left_iter.filter(move |hash| right_set.contains(hash)),
            ))
        }
        RevsetExpression::Function { name, args } => {
            compile_function(name, args, graph, resolve_symbol, resolve_refs)
        }
    }
}

fn compile_symbol<'a, F>(name: &str, resolve_symbol: &mut F) -> Result<RevsetChangeIdIterator<'a>>
where
    F: FnMut(&str) -> Result<Option<ChangeId>>,
{
    if let Ok(id) = ChangeId::from_hex(name) {
        return Ok(Box::new(std::iter::once(id)));
    }

    if let Some(hash) = parse_hex_hash(name) {
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
    F: FnMut(&str) -> Result<Option<ChangeId>>,
    R: ReferenceResolver,
{
    match name {
        "ancestors" => {
            if args.len() != 1 {
                bail!("ancestors() expects exactly one argument");
            }
            let arg = args
                .first()
                .ok_or_else(|| anyhow!("ancestors() expects exactly one argument"))?;
            let starts_iter = compile_impl_change_ids(
                arg,
                Arc::clone(&graph),
                resolve_symbol,
                resolve_refs,
            )?;
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
        _ => bail!("unsupported revset function '{name}'"),
    }
}

struct UnionIterator<'a> {
    left: RevsetChangeIdIterator<'a>,
    right: RevsetChangeIdIterator<'a>,
    seen: HashSet<ChangeId>,
    on_left: bool,
}

impl<'a> UnionIterator<'a> {
    fn new(left: RevsetChangeIdIterator<'a>, right: RevsetChangeIdIterator<'a>) -> Self {
        Self {
            left,
            right,
            seen: HashSet::new(),
            on_left: true,
        }
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

            match self.right.next() {
                Some(hash) => {
                    if self.seen.insert(hash) {
                        return Some(hash);
                    }
                }
                None => return None,
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
        Self {
            graph,
            queue: starts.into(),
            seen: HashSet::new(),
        }
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
                let mut deps: Vec<ChangeId> = change.deps.iter().copied().map(ChangeId::from).collect();
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

    use crate::algebra::Atom;
    use crate::revset::parse;
    use crate::store::author::test_keypair;
    use crate::store::change::Change;

    use super::*;

    fn make_change(graph: &mut ChangeGraph, deps: HashSet<Blake3Hash>, label: &str) -> Blake3Hash {
        let (author, key) = test_keypair();
        let content_hash: [u8; 32] = *blake3::hash(label.as_bytes()).as_bytes();
        let change = Change::new(
            deps,
            vec![Atom::Insert {
                at: vec![label.to_string()],
                content_hash,
            }],
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
        assert!(
            err.to_string().contains("exactly one argument"),
            "unexpected error: {err}"
        );
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
        assert!(
            err.to_string().contains("exactly one argument"),
            "unexpected error: {err}"
        );
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
}
