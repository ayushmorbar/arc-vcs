---
applyTo: "crates/arc-core/**/*.rs,crates/arc-algebra/**/*.rs,crates/arc-algebra-types/**/*.rs,crates/arc-change/**/*.rs,crates/arc-engine/**/*.rs,crates/arc-revset/**/*.rs"
---

# Rust core instructions

- Preserve purity in core crates.
- Do not introduce filesystem, network, process, clock, or random I/O.
- Prefer total or explicitly fallible typed APIs over implicit panics.
- Keep semantic operations AST-native, not line-based.
- Call out schema, epoch, or compatibility impact in comments or PR notes when
  serialized types change.

> Note: This instruction intentionally targets pure crates only. Boundary crates
> like CLI/daemon/network have separate instructions.