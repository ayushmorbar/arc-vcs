use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::algebra::Blake3Hash;
use crate::revset::RevsetExpression;
use crate::store::graph::ChangeGraph;

/// Iterator type produced by revset compilation.
pub type RevsetIterator<'a> = Box<dyn Iterator<Item = Blake3Hash> + 'a>;

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
    compile_impl(expr, graph, resolve_symbol)
}

fn compile_impl<'a, F>(
    expr: &RevsetExpression,
    graph: Arc<ChangeGraph>,
    resolve_symbol: &mut F,
) -> Result<RevsetIterator<'a>>
where
    F: FnMut(&str) -> Result<Option<Blake3Hash>>,
{
    match expr {
        RevsetExpression::Symbol(name) => compile_symbol(name, resolve_symbol),
        RevsetExpression::Union(left, right) => {
            let left_iter = compile_impl(left, Arc::clone(&graph), resolve_symbol)?;
            let right_iter = compile_impl(right, graph, resolve_symbol)?;
            Ok(Box::new(UnionIterator::new(left_iter, right_iter)))
        }
        RevsetExpression::Intersection(left, right) => {
            let left_iter = compile_impl(left, Arc::clone(&graph), resolve_symbol)?;
            let right_iter = compile_impl(right, graph, resolve_symbol)?;
            let right_set: HashSet<Blake3Hash> = right_iter.collect();
            Ok(Box::new(
                left_iter.filter(move |hash| right_set.contains(hash)),
            ))
        }
        RevsetExpression::Function { name, args } => {
            compile_function(name, args, graph, resolve_symbol)
        }
    }
}

fn compile_symbol<'a, F>(name: &str, resolve_symbol: &mut F) -> Result<RevsetIterator<'a>>
where
    F: FnMut(&str) -> Result<Option<Blake3Hash>>,
{
    if let Some(hash) = parse_hex_hash(name) {
        return Ok(Box::new(std::iter::once(hash)));
    }

    if let Some(hash) = resolve_symbol(name)? {
        return Ok(Box::new(std::iter::once(hash)));
    }

    bail!("unknown revset symbol '{name}'")
}

fn compile_function<'a, F>(
    name: &str,
    args: &[RevsetExpression],
    graph: Arc<ChangeGraph>,
    resolve_symbol: &mut F,
) -> Result<RevsetIterator<'a>>
where
    F: FnMut(&str) -> Result<Option<Blake3Hash>>,
{
    match name {
        "ancestors" => {
            if args.len() != 1 {
                bail!("ancestors() expects exactly one argument");
            }
            let arg = args
                .first()
                .ok_or_else(|| anyhow!("ancestors() expects exactly one argument"))?;
            let starts_iter = compile_impl(arg, Arc::clone(&graph), resolve_symbol)?;
            let starts = starts_iter.collect::<Vec<_>>();
            Ok(Box::new(AncestorsIterator::new(graph, starts)))
        }
        _ => bail!("unsupported revset function '{name}'"),
    }
}

struct UnionIterator<'a> {
    left: RevsetIterator<'a>,
    right: RevsetIterator<'a>,
    seen: HashSet<Blake3Hash>,
    on_left: bool,
}

impl<'a> UnionIterator<'a> {
    fn new(left: RevsetIterator<'a>, right: RevsetIterator<'a>) -> Self {
        Self {
            left,
            right,
            seen: HashSet::new(),
            on_left: true,
        }
    }
}

impl<'a> Iterator for UnionIterator<'a> {
    type Item = Blake3Hash;

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
    queue: VecDeque<Blake3Hash>,
    seen: HashSet<Blake3Hash>,
}

impl AncestorsIterator {
    fn new(graph: Arc<ChangeGraph>, starts: Vec<Blake3Hash>) -> Self {
        Self {
            graph,
            queue: starts.into(),
            seen: HashSet::new(),
        }
    }
}

impl Iterator for AncestorsIterator {
    type Item = Blake3Hash;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(current) = self.queue.pop_front() {
            if !self.seen.insert(current) {
                continue;
            }

            if let Some(change) = self.graph.get(&current) {
                let mut deps: Vec<Blake3Hash> = change.deps.iter().copied().collect();
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
        let mut resolver = |name: &str| -> Result<Option<Blake3Hash>> {
            match name {
                "A" => Ok(Some(a)),
                "B" => Ok(Some(b)),
                _ => Ok(None),
            }
        };

        let iter = compile(&ast, Arc::clone(&graph), &mut resolver).expect("compile should work");
        let result: HashSet<Blake3Hash> = iter.collect();

        let expected = HashSet::from([a, b, root]);
        assert_eq!(result, expected);
    }

    #[test]
    fn rejects_ancestors_with_no_args() {
        let graph = Arc::new(ChangeGraph::new());
        let ast = parse("ancestors()").expect("query should parse");
        let mut resolver = |_name: &str| -> Result<Option<Blake3Hash>> { Ok(None) };

        let err = match compile(&ast, graph, &mut resolver) {
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
        let mut resolver = move |name: &str| -> Result<Option<Blake3Hash>> {
            match name {
                "A" => Ok(Some(a)),
                "B" => Ok(Some(b)),
                _ => Ok(None),
            }
        };

        let err = match compile(&ast, graph, &mut resolver) {
            Ok(_) => panic!("compile should fail"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("exactly one argument"),
            "unexpected error: {err}"
        );
    }
}
