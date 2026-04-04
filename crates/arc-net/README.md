# arc-net

Network services crate for arc.

## Responsibilities

- Read-only HTTP access to views and CAS objects.
- Blob upload/download endpoints for sync workflows.
- Signed delta ingestion and verification pipeline.
- Native arc sync protocol types and server/client codec.
- AI provider factory used by CLI resolution flows.

## Public Modules

- `server`: HTTP handlers and service bootstrap.
- `sync`: native protocol, framing, and transport code.
- `ai`: AI provider abstractions used by higher layers.

## Security Posture

- Validates hash-shaped path parameters before file access.
- Verifies payload signatures before CAS mutation.
- Enforces blob pre-existence checks during sync apply.
- Keeps network server stateless and storage-backed.

## Typical Usage

Usually consumed through CLI:

```sh
arc serve --port 8080
```

Library embedding is also supported for custom deployments.
