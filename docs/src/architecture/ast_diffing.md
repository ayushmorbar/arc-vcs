---
title: "AST Diffing"
description: "Conceptual pipeline from language ASTs to typed atom streams in arc."
---

# AST Diffing

arc does not diff text. Instead it diffs **typed ASTs** produced by tree-sitter grammars and converts structural differences into `Atom` streams.

## Pipeline

```
working-directory files
        │
        ▼
   tree-sitter parse
        │
        ▼
   LanguagePlugin::diff_trees()    ← arc-lang
        │
        ▼
     Vec<Atom>                       ← arc-algebra-types / arc-algebra
        │
        ▼
   Change { atoms, intent, author, … }
        │
        ▼
   CAS store (blake3 hash)
```

## LanguagePlugin trait

Any language can be supported by implementing:

```rust
pub trait LanguagePlugin {
    fn parse(&self, source: &str) -> ASTNode;
    fn diff_trees(&self, old: &ASTNode, new: &ASTNode) -> Vec<Atom>;
    fn node_is_interesting(&self, kind: &str) -> bool;
}
```

`RustPlugin` ships with arc out of the box. Additional languages can be added as separate crates implementing this trait.

## Granularity

`node_is_interesting` controls which tree-sitter node kinds produce atoms. For Rust, this currently includes `function_item`, `struct_item`, `impl_item`, `trait_item`, and `use_declaration`.
