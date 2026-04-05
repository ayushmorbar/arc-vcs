# arc-daemon

JSON-RPC daemon backend for editor and IDE integrations.

## Bottom Line Up Front

`arc-daemon` is a long-lived orchestration surface for IDEs.
It does not own VCS semantics, persistence internals, or network transport protocols.
It delegates repository behavior to `arc-cli` and consumes typed state contracts from split domain crates.

## Purpose

`arc-daemon` keeps a long-lived process attached to a repository so editors can query status without repeatedly spawning `arc` commands.

## Responsibilities

- Implements JSON-RPC 2.0 request and response envelopes.
- Serves IDE-facing methods:
  - `get_status`
  - `get_oplog`
  - `get_file_states`
- Watches repository paths and emits notifications:
  - `arc/stateChanged`
  - `arc/fileDecorationsChanged`
- Delegates repository semantics to `arc-cli` and shared micro-crate domain types.

## Non-Goals

- No custom VCS semantics separate from `arc-cli`.
- No direct network sync protocol ownership.
- No alternate CAS format.

## Crate Layout

- `src/protocol.rs`: JSON-RPC wire types and serialization helpers.
- `src/server.rs`: async stdin/stdout server loop and method dispatch.
- `src/main.rs`: daemon entry point.

## Run

From repository root:

```sh
cargo run -p arc-daemon
```

Or through CLI internal command path:

```sh
arc daemon
```

## Dependency Boundaries

- Depends on `arc-cli` for repository orchestration.
- Depends on split domain crates (`arc-algebra`, `arc-algebra-types`, `arc-change`, `arc-store-types`, `arc-store-view`) for typed state.
- Must not introduce competing domain logic that diverges from CLI behavior.

## Crash and Purity Notes

- `arc-daemon` is orchestration-only and should remain thin.
- Crash-consistent storage guarantees are provided by lower persistence slices, not reimplemented here.
- Semantic algebra remains outside daemon runtime paths.
