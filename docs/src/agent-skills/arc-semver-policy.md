---
name: arc-semver-policy
description: >
  Apply versioning rules for arc-vcs. Use when changing crate versions,
  serialized structs, protocol compatibility, schema migrations, network epoch
  behavior, release tags, or SemVer decisions in the arc codebase.
---

# arc-semver-policy

## Purpose

Versioning in `arc-vcs` operates on two planes:

1. Rust crate SemVer
2. schema / protocol epoch compatibility

When asked to bump a version, review both.

## Plane 1: crate SemVer

Use standard Semantic Versioning for the Rust workspace.

- MAJOR: breaking CLI, API, or foundational compatibility changes
- MINOR: new backward-compatible functionality
- PATCH: backward-compatible fixes, internal improvements, dependency-safe
  maintenance, and performance work

For pre-1.0 work, still communicate breakage clearly and avoid hiding
incompatible changes under vague patch bumps.

## Plane 2: schema and protocol epochs

If you change any struct, enum, encoding, or invariant that is persisted to disk
or used across sync boundaries, treat it as an epoch review event.

Examples:

- `OpRecord`
- `Change`
- intent graph records
- CAS object headers
- sync payloads
- conflict or provenance records

When such a type changes, you must decide whether to:

1. preserve backward compatibility,
2. add an explicit migration, or
3. bump a schema or network epoch constant.

## Backward-compatibility rules

If adding a new persisted field:

- prefer a backward-compatible default,
- preserve deserialization of older data,
- document the semantic meaning of the default.

If removing or renaming a persisted field:

- do not assume compatibility,
- add a migration path or epoch bump.

If changing hash identity, canonical encoding, or sync protocol behavior:

- treat this as potentially epoch-breaking even if CLI flags do not change.

## Action protocol

When asked to “bump the version” or “prepare a release”:

1. Check whether serialized or synced data structures changed.
2. Check whether protocol or network compatibility changed.
3. Decide whether schema migration or epoch bump is required.
4. Update the workspace version in the root `Cargo.toml` if crate SemVer changed.
5. Call out any migration, replay, or sync compatibility consequences.
6. Never silently change version-sensitive formats.

## Output guidance

When answering, include:

- recommended SemVer bump,
- whether schema epoch review is required,
- whether network epoch review is required,
- why the change is or is not backward compatible,
- any migration follow-up.

## Decision examples

- New CLI command with no persisted schema change → MINOR, no epoch bump
- Bug fix in semantic diff labeling only → PATCH, no epoch bump unless stored
  output format changes
- Add field to persisted record with safe default and tested compatibility →
  MINOR or PATCH depending on behavior, plus migration note
- Change canonical hashing or object encoding → breaking compatibility review,
  likely MAJOR and epoch bump