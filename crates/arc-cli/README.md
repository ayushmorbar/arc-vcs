# arc-cli

CLI orchestrator crate for arc.

## Purpose

`arc-cli` is the command-facing orchestration layer. It wires together core algebra/storage, language plugins, network services, and Git bridge interoperability.

## Command Surface

Top-level command groups include:

- Repository lifecycle: `init`, `status`, `diff`, `snap`, `log`, `verify`, `info`
- Change operations: `cherry-pick`, `revert`, `restore`, `amend`, `squash`, `diffedit`
- View control: `view`, `checkout`, `branch`, `undo`, `op`
- AI workflows: `ai resolve`, `ai approve`, `ai generate`
- Sync and remotes: `sync`, `fetch`, `pull`, `push`, `serve`, `remote`
- Advanced workflows: `sparse`, `mount`, `workspace`, `gc`, `compact`

See repository docs for full syntax: `docs/src/reference/cli-reference.md`.

## Module Layout

- `repo`: repository orchestration and command implementations.
- `sync`: native synchronization primitives.
- `interop`: external VCS import and interop helpers.
- `semantic_diff`: semantic/text diff rendering.
- `ai_pending`: pending AI change state handling.
- `bugreport`: diagnostics packaging.

## Dependency Boundaries

- Owns orchestration, not low-level storage primitives.
- Defers formal model and persistence mechanics to `arc-core`.
- Defers transport-specific Git protocol details to `arc-git-bridge`.
