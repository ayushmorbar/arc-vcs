# arc Architecture (Code-Verified)

Status: ADR-004 micro-crate architecture is active.

## Bottom Line Up Front

arc is no longer organized as a monolithic core engine. The system is decomposed into vertical slices with strict boundaries: domain math and identity logic stay effect-free, while disk and network effects are isolated to dedicated crates.

## Canonical Entry Points

- `docs/src/architecture/overview.md`
- `docs/src/architecture/documentation-map.md`
- `docs/src/reference/cli-reference.md`

## Workspace Architecture (Current)

| Layer                | Crates                                                          | Contract                                                      |
| -------------------- | --------------------------------------------------------------- | ------------------------------------------------------------- |
| Domain types         | `arc-algebra-types`, `arc-store-types`, `arc-change`            | Typed IDs, atoms, author and ref models                       |
| Pure math and query  | `arc-algebra`, `arc-engine`, `arc-revset`                       | Algebra, spacetime rewrites, revset compilation               |
| Persistence          | `arc-store-cas`, `arc-store-graph`, `arc-store-view`            | CAS I/O, graph state, view/oplog/snapshot persistence         |
| Transport            | `arc-network`, `arc-net`                                        | Payload protocol, sync server/client, ingress verification    |
| Product surfaces     | `arc-cli`, `arc-daemon`, `arc-git-bridge`, `arc-lang`, `arc-ai` | UX, editor integration, Git interop, AST plugins, AI adapters |
| Compatibility facade | `arc-core`                                                      | Transitional re-export layer for migration stability          |

## Axiom Of Purity

Purity in arc means semantic crates do not perform filesystem or network side effects. This enables deterministic replay, narrower review surfaces, and safer agent-assisted refactors.

I/O placement is explicit:

- Disk effects in storage crates.
- Network effects in transport crates.
- Orchestration effects in CLI and daemon surfaces.

## Crash-Consistency Model

Crash consistency is enforced as a design property:

- Atomic rename update paths for mutable pointers.
- Append-only operation logging for durable intent ordering.
- Persistence barriers at durability boundaries.

## High-Level Data Path

1. Command enters via `arc-cli` or `arc-daemon`.
2. Semantics route through algebra and graph slices.
3. State is persisted via CAS and view/oplog slices.
4. Sync uses transport slices with zero-trust ingress checks.
5. Git interoperability is translated at the bridge boundary.

## Scope Boundary

Workspace build truth is `crates/*` in `Cargo.toml`.
Top-level `src/` may exist for historical context and experiments, but it is not the canonical product architecture surface.

## Non-Speculation Rule

If a behavior is not code-verified, document it as a limitation in `SHORTCOMINGS.md` or roadmap material, not as shipped capability.
