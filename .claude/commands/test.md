---
description: Run the arc-vcs test suite, verify commutativity math, and analyze failures.
---

# arc-vcs Validation Pipeline

Please execute the following validation steps to ensure the repository remains mathematically pure and compiling:

1. Run `cargo fmt --check` to ensure formatting is clean.
2. Run `cargo check --workspace` to ensure all crates compile with default features.
3. Run `cargo test --workspace` to execute unit and integration tests.
4. Run `just test` to execute journey tests (if the `justfile` exists).

**If any test fails:**
- Do not immediately write a fix.
- Analyze the failure specifically looking for violations of AST supremacy, commutativity laws, or boundary I/O rules.
- Propose the fix to me and wait for my approval before modifying the code. 
- If a test failure is related to serialized types or BLAKE3 hashing, explicitly state if a schema epoch bump will be required to fix it.