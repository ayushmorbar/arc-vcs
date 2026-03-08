# Network Transport

arc's network transport is built on **Delta-State CRDT** principles: instead
of negotiating what two repositories have in common (Git's `Have/Want`
multi-round-trip protocol), arc computes the exact missing slice of the
mathematical change graph, bundles it with all referenced CAS blobs in one
JSON envelope, and delivers it in a single HTTP POST.

---

## 1. Design Goals

| Goal | Mechanism |
|---|---|
| **Zero-negotiation push** | Sender computes delta locally; one POST to deliver |
| **Content-addressed deduplication** | BLAKE3 CAS — identical blobs are never re-sent |
| **Zero-trust ingress** | Ed25519 signature check before any CAS write (SLSA L4) |
| **CRDT convergence** | View merge is a pure set union — no coordination required |
| **Bounded memory (MVP)** | All blobs in RAM for `0.1.0-beta.2`; Phase 39 streams them |

---

## 2. `DeltaPayload` Wire Format

```json
{
  "changes": [ { "id": "<64-hex>", "deps": [...], "atoms": [...], ... } ],
  "blobs":   { "<64-hex>": [72, 101, 108, ...] },
  "view_heads": ["<64-hex>", ...]
}
```

| Field | Type | Purpose |
|---|---|---|
| `changes` | `Vec<Change>` | BFS-ordered slice of the DAG the remote is missing |
| `blobs` | `HashMap<String, Vec<u8>>` | CAS blob bytes, hex-key indexed |
| `view_heads` | `HashSet<Blake3Hash>` | Sender's current view heads for CRDT union |

`blobs` contains every blob whose BLAKE3 hash appears in an
`Insert { content_hash }` or `Delete { prior_hash }` atom in `changes`.
This one-shot sidecar eliminates a second round-trip for blob hydration.

> **Memory note:** For MVP (`0.1.0-beta.2`) all blobs are buffered in RAM.
> TODO: Phase 39 — chunked multipart streaming for blobs above a size
> threshold, keeping memory usage flat for large binary assets.

---

## 3. Push Protocol

```
Client                                     Server
  │                                           │
  │  GET /views/{view_name}                   │
  │ ─────────────────────────────────────────>│
  │  ← { heads: [...] }                       │
  │                                           │
  │  [BFS from local heads, cut at remote heads]
  │  [collect delta Changes + blob sidecars]  │
  │                                           │
  │  POST /sync/{view_name}                   │
  │  { changes, blobs, view_heads }           │
  │ ─────────────────────────────────────────>│
  │                                           │  verify_payload() → Ed25519
  │                                           │  write_change × N (CAS, idempotent)
  │                                           │  write_blob × M (CAS, idempotent)
  │                                           │  new_heads = remote ∪ view_heads
  │  ← 200 OK                                 │
```

### BFS delta computation

The sender BFS-traverses the local DAG from its view heads.  Any change ID
that is also a remote head is treated as a **causal cut-point**: the server
has all ancestors of its heads (CRDT causality invariant), so we stop
enqueueing past that point.

This eliminates the multi-round-trip `Have/Want` negotiation that Git uses
when establishing a common base.  Because CAS writes are idempotent, slightly
over-inclusive deltas (sending some changes the server already has) are
harmless and occasionally necessary when sender and receiver have diverged.

### Filesystem push (`push_local`)

When `remote` resolves to a local filesystem path, arc writes directly to the
remote's CAS and updates its view with an atomic rename — no HTTP involved.
This is used by `arc push ./sister-repo main` and by test infrastructure.

---

## 4. Fetch Protocol

```
Client                                     Server
  │                                           │
  │  GET /views/{view_name}                   │
  │ ─────────────────────────────────────────>│
  │  ← { heads: [...] }                       │
  │                                           │
  │  [BFS from remote heads, stop at local]   │
  │                                           │
  │  GET /objects/{hex}  (per missing change) │
  │ ─────────────────────────────────────────>│
  │  ← bincode Change bytes                   │
  │                                           │
  │  GET /blobs/{hex}    (per missing blob)   │
  │ ─────────────────────────────────────────>│
  │  ← raw blob bytes                  (×M)   │
```

After downloading each Change, `fetch_http` immediately fetches all blobs
referenced by its atoms.  A **404 on a blob is a hard error** — unlike the
pre-Phase-38 silent-skip, because `Insert` atoms no longer carry inline bytes
(Phase 37 hard break).  A missing blob leaves the CAS unable to materialise
the working directory.

---

## 5. Zero-Trust Ingress

`verify_payload(payload: &DeltaPayload) -> Result<()>` is called by the
server **before** any write to its CAS.

It iterates every `Change` in `payload.changes` and calls
`change.verify_signature()`, which performs two independent checks:

1. **Content integrity** — re-hash `(sorted_deps, atoms, intent, author)` and
   assert the result equals `change.id`.  Catches any field tampering.

2. **Signature integrity** — verify the Ed25519 signature against `change.id`
   using the public key embedded in `change.author`.

Why this is a complete supply-chain shield:

> An attacker tampers with a blob → the `content_hash` in the `Insert` atom
> changes → the `Change` id changes → the Ed25519 signature over the old id
> no longer verifies → `verify_payload` returns `Err` → the server rejects the
> entire payload → **no write ever reaches the CAS**.

This corresponds to **SLSA Level 4** provenance: every step of the dependency
chain is cryptographically authenticated before it can affect the build graph.

---

## 6. CRDT Convergence (O(1) Server-Side Merge)

The server's merge after a successful `POST /sync` is:

```rust
let new_heads = remote_heads.union(&payload.view_heads).copied().collect();
View::new(view_name, new_heads).save(&repo_root)?;
```

This is a **pure set union** — no working-directory materialisation, no
text-conflict resolution, no coordinator required.  Because arc tracks a set
of heads (not a single tip), two independent pushes from Alice and Bob both
succeed; the resulting view has two heads which will be merged when either
party next runs `arc merge`.

All writes to the CAS (`write_change`, `write_blob`) are idempotent — if two
pushes race to write the same change, both succeed silently.  The view update
uses an atomic OS-level rename (`View::save` writes to `.tmp` then renames),
ensuring the view pointer is never half-written.

---

## 7. Identity Collapsing (Phase 39 — Planned)

At scale, a long-running CRDT accumulates many **transient identities**:
short-lived agents, CI runners, or ephemeral contributors who create one or
two changes and are never seen again.  Each unique `Author` public key is
tracked in the graph forever, creating unbounded tombstone growth.

**Phase 39** will introduce an Identity Collapsing map: a server-side mapping
from transient key fingerprints to canonical contributor identities.  This
allows the graph to be compacted through the Epoch Map (`arc compact`) without
losing authorship attribution, keeping the graph's author-identity set bounded
as the project grows.

---

## 8. Future: HTTP/3 + Streaming (Post-1.0)

The current transport uses HTTP/1.1 via `reqwest`.  Post-1.0 work items:

- **HTTP/3 + QUIC** — reduces latency on high-packet-loss connections
  (satellite links, mobile).
- **Chunked multipart streaming** (Phase 39) — prevents OOM when pushing
  repositories with large binary assets.
- **Compression** — `Content-Encoding: zstd` on the `DeltaPayload` body;
  source code compresses 3–5×, dramatically reducing push time on slow links.
