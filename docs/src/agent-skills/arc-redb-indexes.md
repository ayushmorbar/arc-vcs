---
name: arc-redb-indexes
description: >
  Rules for interacting with the Redb embedded database for metadata, 
  indexes, and graph state. Use when writing database schemas, transactions, 
  read/write tables, or separating CAS blob data from metadata.
---

# arc-redb-indexes

## Purpose
In `arc`, BLAKE3 object blobs are immutable and stored in the raw filesystem. `redb` is used strictly for mutable indexes, references, intent metadata, and the local view of the causal graph.

## Database Discipline

### 1. Separation of Concerns
- **Never** store file content, AST payloads, or large binary blobs in `redb`.
- **Always** store pointers (BLAKE3 hashes), vector clocks, local tags, and DAG adjacency lists in `redb`.

### 2. Transaction Lifetimes
- `redb` transactions borrow the database. Do not hold a `WriteTransaction` open while performing expensive parsing (`tree-sitter`) or network I/O.
- Compute the AST and BLAKE3 hashes first (pure functions), *then* open the `WriteTransaction` to commit the index updates instantly.

### 3. Table Definitions
- Table definitions must be strictly typed using `redb::TableDefinition`.
- Table names must be constant and versioned (e.g., `const EDGES_V1: TableDefinition<[u8; 32], [u8]>`).
- If the type signature of a table changes, you must bump the schema epoch (see `arc-semver-policy`) and write a migration.

### 4. Zero-Copy Reads
- Use `redb`'s `AccessGuard` to read index data without allocating `Vec<u8>` where possible, converting directly to fixed-size hash arrays or scalar types.