# arc-network

![crate](https://img.shields.io/badge/crate-arc--network-blue)
![role](https://img.shields.io/badge/role-sync%20protocol-f6a)

## BLUF

`arc-network` defines transport wire types and client helpers for arc-to-arc sync. It is the protocol contract for payload verification, push/pull metadata exchange, and blob fetch/upload flows.

## Architectural Role (The DAG)

- Depends on: `arc-algebra-types`, `arc-change`, `arc-store-types`, HTTP/serde stack.
- Depended on by: `arc-net`, `arc-cli`, and compatibility facades.
- Position: protocol layer between pure DAG semantics and runtime network servers/clients.

## Purity & I/O Boundary

`arc-network` is an I/O Boundary.

- Performs HTTP request/response operations.
- Encodes/decodes sync payloads.
- Does not own runtime startup policy.

## Key Types/Exports

- `DeltaPayload`
- `SyncResponse`
- `verify_payload`
- `NetworkClient`

```rust
use arc_network::NetworkClient;
let _client = NetworkClient::new()?;
# Ok::<(), anyhow::Error>(())
```
