---
description: Audit uncommitted code against arc-vcs architectural axioms.
---

# Code Review

Please review my uncommitted changes (or the specific files I provided) against the `arc-vcs` architectural axioms:

1. **Axiom of Purity:** Did I accidentally introduce `std::fs`, `std::net`, or random/clock I/O into `arc-core` or math crates?
2. **AST Supremacy:** Did I use regex, line-numbers, or string manipulation for diffs instead of tree-sitter AST nodes?
3. **CAS Strictness:** Are all object identities using `blake3`? Is the serialization boundary safe?
4. **Error Handling:** Am I using `thiserror` for core library boundaries and `anyhow` for the CLI/Daemon boundaries?

If you find violations, do not fix them automatically. Point out the exact line and explain which axiom it violates.