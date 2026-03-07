# ADR 001 — BLAKE3 for Content-Addressed Storage

| Field | Value |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-02-2 |
| **Deciders** | arc core team |

---

## Context

arc requires a cryptographic hash function for four distinct roles:

1. **Object identity:** every `Change`, `Atom`, and blob must have a stable unique ID.
2. **Integrity verification:** every object loaded from disk must be verifiable against tampering.
3. **Identity signing:** author identity is bound to Change content via hash-then-sign (Ed25519).
4. **Performance at scale:** repositories with 50 GB+ binary blobs must hash without performance degradation.

The two most common alternatives were SHA-1 (Git-legacy) and SHA-256 (security standard).

**SHA-1 was rejected** because:
- SHA-1 is cryptographically broken for collision resistance (SHAttered, 2017).
- Git's migration away from SHA-1 to SHA-256 is proof that SHA-1 is insufficient for a new VCS.

**SHA-256 was rejected** because:
- BLAKE3 is 3–5× faster than SHA-256 on modern hardware in both single-threaded and multi-threaded workloads.
- BLAKE3 supports a **keyed hash mode** natively, enabling fast collision-resistant identity signing without a separate HMAC construction.
- BLAKE3 supports a **key-derivation mode** for deriving per-context subkeys from a root secret.
- BLAKE3 is designed to be parallelised to saturate all CPU lanes simultaneously.

---

## Decision

Use **BLAKE3** (256-bit output) as the sole hash algorithm in arc's CAS.

Implementation: the `blake3 = "1.5"` crate in `arc-core`. All blob hashing uses `memmap2` for zero-copy I/O, enabling constant-memory hashing of arbitrarily large files.

The `Blake3Hash` type is a `[u8; 32]` newtype with `Display` (hex-encoded) and `FromStr` (hex-decode with length validation) implementations.

---

## Consequences

**Positive:**
- Object IDs are uniformly distributed 256-bit values — negligible collision probability.
- Hashing is fast enough that it is never the bottleneck, even for large binary repositories.
- Keyed mode provides a clean primitive for future per-user encryption.

**Negative:**
- arc object IDs are **not compatible** with Git's SHA-1 or SHA-256 namespaces. `arc git-import` must re-hash all objects.
- The BLAKE3 crate is an additional dependency. (It is the only cryptography dependency in `arc-core`.)

---

## References

- BLAKE3 specification: [https://github.com/BLAKE3-team/BLAKE3](https://github.com/BLAKE3-team/BLAKE3)
- SHAttered (SHA-1 collision): [https://shattered.io/](https://shattered.io/)
- Rust crate: [`blake3`](https://crates.io/crates/blake3)
