---
name: arc-cas-storage
description: >
  Rules for content-addressable storage in arc-vcs. Use when reading, writing,
  hashing, serializing, indexing, memory-mapping, or migrating CAS-backed data
  and metadata.
---

# arc-cas-storage

## Purpose

This skill governs how `arc-vcs` reads and writes content-addressable data.

## Storage model

- Immutable object content lives in BLAKE3-addressed CAS paths.
- Structured metadata and indexes may live in Redb.
- Do not blur metadata stores and blob stores.
- Blob identity must remain deterministic.

## Hashing

- Always use the `blake3` crate for object identity.
- Never introduce SHA-1 or SHA-256 as the canonical object identity mechanism.
- If hashing strategy changes, treat it as a compatibility event.

## Paths

Canonical blob path format:

`.arc/store/{hash[0:2]}/{hash[2:]}`

Do not invent alternate layouts without an explicit migration plan.

## Serialization

- Use binary serialization for persisted internal objects unless a specific
  interoperability boundary requires another format.
- Be explicit about canonical encoding rules.
- Treat encoding changes as schema-compatibility events.

## Read/write discipline

- Writes must preserve immutability semantics.
- Never rewrite the contents of an already-addressed object.
- New content produces a new object.
- Validate decoded objects before trusting them at higher layers.

## Performance rules

- Prefer zero-copy or low-copy reads where safe and appropriate.
- Use `memmap2` when reading large graph or blob data where mmap semantics are
  part of the intended performance model.
- Keep unsafe boundaries minimal and well-justified if the storage layer ever
  requires them.

## Metadata separation

Use Redb for:

- indexes,
- lookup tables,
- op metadata,
- intent graph metadata,
- auxiliary structured state.

Use raw CAS paths for:

- immutable object blobs,
- canonical object payloads.

## Review checklist

Before landing CAS changes, verify:

- Is BLAKE3 still the canonical identity?
- Are blob paths canonical?
- Is the encoding stable and documented?
- Is compatibility impact called out?
- Are metadata and blob responsibilities still separated?