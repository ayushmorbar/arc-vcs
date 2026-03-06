# arc-net

Thin async HTTP API for the **arc** version-control system. Exposes repository content over the network so remote peers can fetch missing `Change` objects.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/view/{name}` | Serialized tip `View` (bincode) for the named view |
| `GET` | `/change/{hash}` | Raw CAS object bytes for a change identified by hex hash |

## Usage

Start the server from the CLI:

```sh
arc serve --port 8080
```

Or embed in code:

```rust
arc_net::server::serve(8080).await?;
```
