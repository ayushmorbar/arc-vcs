---
applyTo: "crates/arc-cli/**/*.rs,crates/arc-daemon/**/*.rs"
---

# CLI and boundary instructions

- Boundary crates may perform I/O, but must adapt external input before it
  enters pure core logic.
- Favor actionable errors with stage context.
- Keep user-facing commands deterministic and auditable.
- Do not leak boundary concerns into core data structures.