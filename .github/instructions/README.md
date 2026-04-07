# Instruction Files Index

This index explains why each instruction file in this folder exists and when it applies.

## Files

| File                        | Scope                                                                                                           | Why it exists                                                                                        |
| --------------------------- | --------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- |
| `cli.instructions.md`       | `crates/arc-cli/**/*.rs`, `crates/arc-daemon/**/*.rs`                                                           | Keeps boundary crates deterministic and auditable while allowing controlled I/O adaptation at edges. |
| `rust-core.instructions.md` | Pure semantic crates (`arc-core`, `arc-algebra`, `arc-algebra-types`, `arc-change`, `arc-engine`, `arc-revset`) | Enforces purity, typed fallibility, and AST-native semantics in core math/dataflow code.             |

## Design intent

1. Separate boundary constraints from pure-core constraints.
2. Prevent accidental policy bleed where core-purity rules incorrectly block boundary crates.
3. Keep instruction scopes explicit and reviewable via `applyTo` globs.

## Maintenance guidance

- If a new pure crate is added, extend `rust-core.instructions.md` scope.
- If a new boundary crate is added, extend `cli.instructions.md` or add a dedicated boundary instruction.
- Keep this index aligned with `.github/copilot-instructions.md` and `.claude/CLAUDE.md`.
