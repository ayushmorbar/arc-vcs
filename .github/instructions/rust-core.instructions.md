---
applyTo: "crates/arc-core/**/*.rs,crates/**/src/**/*.rs"
---

# Rust core instructions

- Preserve purity in core crates.
- Do not introduce filesystem, network, process, clock, or random I/O.
- Prefer total or explicitly fallible typed APIs over implicit panics.
- Keep semantic operations AST-native, not line-based.
- Call out schema, epoch, or compatibility impact in comments or PR notes when
  serialized types change.