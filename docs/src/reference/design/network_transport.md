---
title: Network Transport
description: Documentation page for Network Transport.
---

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
  "changes":    [ { "id": "<64-hex>", "deps": [...], "atoms": [...], ... } ],
  "view_heads": ["<64-hex>", ...]
}
```

| Field | Type | Purpose |
|---|---|---|
| `changes` | `Vec<Change>` | BFS-ordered slice of the DAG the remote is missing |
| `view_heads` | `HashSet<Blake3Hash>` | Sender's current view heads for CRDT union |

> **Phase 39 change:** the `blobs` map has been removed from `DeltaPayload`.
> Blobs are now transferred out-of-band via `PUT /blobs/:hash` (Section 8)
> before the `POST /sync` call.  This decouples the data plane from the
> control plane, keeping the JSON envelope small and memory usage flat even
> for repositories with large binary assets.

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

## 7. Dual-Provenance Identity Collapsing (Phase 39)

### Problem: Transient Identity Explosion

At scale, a long-running CRDT accumulates many **transient identities**:
short-lived CI runners, ephemeral contributors, or temporary author keys
that create one or two Changes and are never seen again.  Each unique
`Author` public key must be tracked in the graph's causal metadata forever,
causing unbounded tombstone growth.

### Solution: Server-Side Collapse with Dual Provenance

**Phase 39** introduces *Dual-Provenance Identity Collapsing*:

1. **Transient detection** — the server classifies a Change as transient
   when its author name contains `"-temp"`, `"transient"`, or starts with
   `"ci-"` (MVP heuristic; replaceable with a key registry in Phase 40).

2. **Cascade rule** — the rewrite is also triggered when any *dependency*
   of a non-transient Change was rewritten, because deps are included in
   `compute_id`.  Remapping a dep changes the Change's hash, which
   invalidates the original author's signature.  The server must therefore
   re-sign the cascaded Change under `Author::Server` as well.

3. **Server rewrite** — for each triggered Change `C`:
   - Remap deps through the accumulated `rewritten_map`.
   - Compute a new `canonical_id` with the remapped deps + `Author::Server`.
   - Sign the `canonical_id` with the server's Ed25519 key.
   - Set `collapsed_from = Some(C.id)` on the canonical Change.
   - Write **both** the original Change and the canonical Change to CAS.

4. **SLSA L4 preserved** — the original Change remains in CAS forever as
   the cryptographic audit root.  `collapsed_from` is a permanent pointer
   back to it; auditors can always verify the pre-collapse authorship.

5. **`SyncResponse`** — the server returns:
   ```json
   {
     "view_heads":    ["<canonical-64-hex>", ...],
     "rewritten_map": { "<original-hex>": "<canonical-hex>", ... }
   }
   ```
   Clients update their local view to point at canonical heads.
   The `rewritten_map` is empty for pushes with no transient Changes
   (the overwhelmingly common case).

### `collapsed_from` Field

`collapsed_from: Option<Blake3Hash>` is excluded from `compute_id`.  It is
provenance metadata, not content: changing it must not alter the
content-addressed identity of the canonical Change.  This means that two
Changes with identical `(deps, atoms, intent, author)` but different
`collapsed_from` values have the same `id` — an important invariant for
CAS deduplication.

---

## 8. Blob Streaming Protocol (Phase 39)

Blobs are no longer inline in `DeltaPayload`.  The client streams them
separately before posting the sync payload.

### `PUT /blobs/:hash` — Streaming intake

```
Client                                     Server
  │                                           │
  │  PUT /blobs/{blake3-64-hex}               │
  │  Content-Length: <size>                   │
  │  <raw bytes streamed>                     │
  │ ───────────────────────────────────────────>│
  │                                           │  stream body frame-by-frame
  │                                           │  compute BLAKE3 simultaneously
  │                                           │  write each chunk to .arc/tmp/{hash}.tmp
  │                                           │  compare hash to path param
  │                                           │  match  → rename to .arc/blobs/{hash}
  │  ← 201 Created                            │  (or 200 if already existed)
  │  (or 400 if hash mismatch)                │
```

**Key properties:**
- **Zero RAM buffering** — each body frame is fed to the BLAKE3 hasher AND
  written directly to a temp file; no full blob is ever held in heap.
- **Atomic commit** — `rename(tmp → blobs/{hash})` is OS-atomic.
- **Idempotent** — a second PUT for an existing blob returns `200 OK`.
- **Hash-verified** — content that does not match the path hash is deleted
  and a `400` is returned; the CAS invariant is never violated.

### Client Upload Contract

All blobs referenced by `Insert { content_hash }` or `Delete { prior_hash }`
atoms **must** be PUT to the server before the `POST /sync` call.
The server enforces this with `409 Conflict` + a JSON list of missing hex
hashes.  The client re-uploads the missing blobs and retries once.  If the
409 persists after the retry, the push hard-fails (hash algorithm mismatch
guard).

### Endpoint Table (Phase 39)

| Method | Path | Purpose |
|---|---|---|
| `GET`  | `/views/:name`      | Fetch view JSON |
| `GET`  | `/objects/:hash`    | Fetch raw bincode Change |
| `GET`  | `/blobs/:hash`      | Fetch raw blob bytes |
| `PUT`  | `/blobs/:hash`      | Stream-upload a blob (Phase 39) |
| `POST` | `/sync/:view_name`  | Push DAG delta + trigger collapse |
