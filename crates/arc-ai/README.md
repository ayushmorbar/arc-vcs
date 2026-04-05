# arc-ai

![crate](https://img.shields.io/badge/crate-arc--ai-blue)
![role](https://img.shields.io/badge/role-ai%20boundary-f6a)

## BLUF

`arc-ai` is the AI integration boundary for message generation, conflict-resolution adapters, embeddings, and semantic retrieval. It isolates non-deterministic model interactions from deterministic DAG semantics.

## Architectural Role (The DAG)

- Depends on: HTTP/model clients, embedding/vector backends, and arc type crates.
- Depended on by: `arc-cli` and compatibility surfaces.
- Position: edge adapter between arc workflows and LLM/embedding providers.

## Purity & I/O Boundary

`arc-ai` is an I/O Boundary.

- Performs network calls to OpenAI-schema endpoints.
- Manages local embedding/vector persistence.
- Must not define DAG or rewrite semantics.

## Key Types/Exports

- `generate_message`, `generate_code`, `extract_code_fence`
- `AiResolver`, `LlmResolver`, `MockResolver`
- `embedding`, `vector_store` modules

```rust
let msg = arc_ai::generate_message("rename fn").await?;
# Ok::<(), anyhow::Error>(())
```
