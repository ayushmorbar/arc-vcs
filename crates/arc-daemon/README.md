# arc-daemon

JSON-RPC daemon backend for editor and IDE integrations.

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
- Delegates repository semantics to `arc-cli` and `arc-core`.

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
- Depends on `arc-core` for shared model types.
- Must not introduce competing domain logic that diverges from CLI behavior.
