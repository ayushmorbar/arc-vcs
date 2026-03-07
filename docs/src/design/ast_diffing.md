# AST Diffing

This document explains how arc translates source code changes into typed `Atom` objects using Tree-sitter's concrete syntax tree (CST), and how those atoms drive the algebraic patch theory.

---

## Why AST Diffing?

Traditional VCS tools diff source files as sequences of text lines. This approach has two fundamental problems:

1. **Proximity false conflicts:** two changes that are near each other in the file — but completely independent in meaning — look like a conflict to a line-based differ.
2. **Semantic blindness:** a refactor that renames a function across 50 call sites looks like 50 separate line changes, not one semantic operation.

arc solves both by parsing the source into an abstract syntax tree *before* computing differences. Changes at the AST level are automatically classified by their structural role.

---

## Tree-sitter Integration

arc uses [Tree-sitter](https://tree-sitter.github.io/) to parse source files into a lossless concrete syntax tree (CST). Tree-sitter is:

- **Error-tolerant:** it produces a partial tree even for files with syntax errors.
- **Incremental:** it can update a tree after a small edit without reparsing the whole file.
- **Multi-language:** the same API supports hundreds of languages via grammar plugins.

arc's language plugin abstraction lives in `arc-lang`:

```rust
pub trait LanguagePlugin {
    fn diff(&self, old_src: &str, new_src: &str, path: &str) -> Vec<Atom>;
    fn unparse(&self, state: &MaterializedState, path: &str) -> Option<String>;
}
```

The current implementation ships one plugin: `RustPlugin` (via `tree-sitter-rust`). See [SHORTCOMINGS.md](../../SHORTCOMINGS.md) for the multi-language roadmap.

---

## The `RustPlugin` Algorithm

### Diffing (`diff()`)

1. Parse `old_src` and `new_src` with the Tree-sitter Rust grammar.
2. Walk the CST at the **top-level item** granularity (functions, structs, enums, impl blocks, use declarations, etc.).
3. For each top-level item present in `new_src` but not in `old_src`: emit `Atom::Insert { at: path, content }`.
4. For each top-level item present in `old_src` but not in `new_src`: emit `Atom::Delete { at: path }`.
5. The `at` path is constructed as `["file", "<filepath>", "<item-name>"]` — e.g. `["file", "src/widget.rs", "fn_render"]`.

### Reconstruction (`unparse()`)

1. Filter the `MaterializedState` for atoms whose path prefix is `["file", "<filepath>"]`.
2. Sort by item order (insertion order as recorded in the atoms).
3. Concatenate the content strings of all inserted items with appropriate whitespace.
4. Return the reconstructed source.

Round-trip stability (diff → unparse → diff) is verified by tests in `arc-cli`.

---

## Atom Path Structure

All AST-level atoms use the path prefix `["file", "<filepath>"]` to namespace them within the materialised state. The third element identifies the top-level item:

```
["file", "src/widget.rs", "fn_render"]        Insert/Delete a function
["file", "src/widget.rs", "struct_Widget"]    Insert/Delete a struct
["file", "src/widget.rs", "impl_Widget"]      Insert/Delete an impl block
["file", "src/lib.rs",    "use_std_fmt"]      Insert/Delete a use declaration
```

This structure makes the `NodePath` overlap check in `commutes()` fast and precise.

---

## Non-Rust Files: `Atom::Blob`

Files not handled by any language plugin fall back to `Atom::Blob`:

```rust
Atom::Blob { path: String, hash: Blake3Hash }
```

The entire file content is stored as a single CAS object. The hash is computed via `memmap2` zero-copy I/O — even multi-gigabyte files are processed without loading into memory.

`Atom::Blob` atoms for the same path always conflict with each other (whole-file replacement), which is conservative but correct.

---

## Interactive Staging

When `arc snap -i` is used, the computed `Atom::Insert` / `Atom::Delete` atoms are presented one at a time. The user can accept or reject each atom. `Atom::Blob` and directory atoms are always staged automatically.

---

## Current Limitation

AST diffing is currently **Rust-only**. Files in other languages are stored as `Atom::Blob`. Adding a new language requires:

1. Adding the `tree-sitter-<lang>` crate to `arc-lang/Cargo.toml`.
2. Implementing `LanguagePlugin` for the new grammar.
3. Registering it in the plugin dispatch table.

See [SHORTCOMINGS.md](../../SHORTCOMINGS.md#1-rust-only-ast-diffing) and [ADR 002](../architecture/ADRs/002-ast-over-text.md) for context.
