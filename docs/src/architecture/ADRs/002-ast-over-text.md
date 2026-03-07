# ADR 002 — AST-Level Diffing over Line-Based Diffing

| Field | Value |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-03-02 |
| **Deciders** | arc core team |

---

## Context

The quality of a version control system's merge algorithm depends directly on the granularity at which it represents changes. Common alternatives considered:

**Line-based diffing (e.g., `diff -u`, Git):**
- Fast and language-agnostic.
- Produces false conflicts when two changes are textually near but semantically independent (e.g., adjacent function definitions in a file).
- Cannot distinguish formatting changes from semantic changes.
- Cannot represent rename-across-callsites as a single atomic operation.

**Token-based diffing (e.g., Semantic Diff tools):**
- Slightly better than line-based but still conflates unrelated syntax in the same token stream.

**AST-level diffing (arc's approach):**
- Diffs at the level of named syntax tree nodes (functions, structs, impl blocks, use declarations).
- Two changes to *different* top-level items always commute, regardless of their line proximity.
- A rename propagated across multiple call sites can be represented as a single `SemanticsPreserving` atom.
- Enables the `commutes(a, b)` predicate to be defined formally and checked efficiently.

The primary risk is that AST diffing requires a language grammar for each language. Line-based diffing is universal.

---

## Decision

Use **AST-level diffing** (Tree-sitter CST parsing + top-level item extraction) as arc's change representation. Line-based diffing is not used for source code files.

Non-source files (images, compiled artifacts, arbitrary binaries) fall back to whole-file `Atom::Blob` content addressing, which is universally applicable.

The first language supported is Rust (`tree-sitter-rust`), due to arc itself being a Rust project and the need for self-hosted development.

---

## Consequences

**Positive:**
- Eliminates the most common source of false merge conflicts (adjacent-but-independent changes).
- The `commutes(a, b)` predicate is exact and formally verifiable.
- `unparse()` reconstruction is deterministic — there is no ambiguity in the output.

**Negative:**
- arc currently supports only **Rust** as a language with AST-level diffing. All other languages fall back to `Atom::Blob`.
- Adding a new language requires implementing `LanguagePlugin` with `tree-sitter-<lang>`.
- `unparse()` (AST → text reconstruction) must be deterministic and lossless, which imposes constraints on how whitespace and comments are stored.
- `arc-lang` has compile-time dependencies on `tree-sitter` and `tree-sitter-rust`.

See [SHORTCOMINGS.md](../../../SHORTCOMINGS.md#1-rust-only-ast-diffing) for the multi-language plan.

---

## References

- Tree-sitter: [https://tree-sitter.github.io/](https://tree-sitter.github.io/)
- Darcs patch theory (foundational): [https://darcs.net/Theory/PatchTheory](https://darcs.net/Theory/PatchTheory)
- Pijul: [https://pijul.org/model](https://pijul.org/model)
- [Patch Theory Design Doc](../../design/patch_theory.md)
- [AST Diffing Design Doc](../../design/ast_diffing.md)
