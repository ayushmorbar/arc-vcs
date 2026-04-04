# arc-git-bridge

Network-boundary Git translation bridge for arc.

## Purpose

`arc-git-bridge` compiles arc materialized state into Git-compatible objects only when needed at push time. It preserves arc-native local storage while allowing interoperability with Git remotes.

## Responsibilities

- Build Git object payloads and hashes.
- Construct and encode Git packfiles.
- Discover remote refs over Smart HTTP.
- Push packfiles via `git-receive-pack` protocol.

## Crate Layout

- `src/object.rs`: Git object encoding and hashing helpers.
- `src/translator.rs`: arc state to Git commit/tree compilation.
- `src/pack.rs`: packfile encoding.
- `src/protocol.rs`: pkt-line and protocol helpers.
- `src/http.rs`: Smart HTTP discovery and push operations.

## Security and Correctness Notes

- Path and protocol payloads are validated before use.
- Push response is verified for `unpack ok` and per-ref status.
- Translation is performed at the transport boundary and does not mutate arc CAS internals.

## Usage

This crate is consumed by `arc-cli` during `arc push` when the remote resolves to `http` or `https`.
