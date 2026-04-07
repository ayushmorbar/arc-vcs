---
name: arc-tree-sitter
description: >
  Rules and Rust idioms for parsing and manipulating Abstract Syntax Trees 
  using tree-sitter in arc-vcs. Use when writing semantic diffing logic, 
  tree traversals, or syntax mapping.
---

# arc-tree-sitter

## Purpose
`arc` operates exclusively on syntax trees, not text diffs. You will frequently interact with the `tree-sitter` crate to build semantic atoms.

## Strict Parsing Rules

### 1. Lifetime Management
`tree-sitter`'s `Node<'a>` is tied to the lifetime of the underlying `Tree`, which in turn borrows the source text. 
- **Never** attempt to store a `Node<'a>` in a long-lived struct like `Change` or `OpRecord`. 
- Instead, extract the necessary semantic data (e.g., byte ranges, node kinds, or semantic hashes) into owned structs during the parsing phase, and drop the `Tree`.

### 2. Semantic Queries over Manual Traversal
When identifying specific language constructs (e.g., finding all function signatures), prefer using tree-sitter `Query` and `QueryCursor` over manually walking the tree with `TreeCursor`.
- Queries are declarative, language-agnostic at the Rust layer, and less prone to off-by-one errors.

### 3. Whitespace and Comments
- In `arc-vcs`, whitespace and comments are often orthogonal to semantic execution.
- When generating a semantic hash or checking for commutativity, explicitly ignore or strip `named_nodes` that correspond to comments/whitespace unless the user's intent specifically targets documentation.

### 4. Byte Offsets, Not Line Numbers
When storing the location of an edit in a semantic atom, **always use byte ranges** (`start_byte`, `end_byte`). 
- Never use line/column numbers for internal CRDT resolution, as line numbers shift dynamically and break commutativity proofs.