---
title: Large Monorepo Playbook
description: Documentation page for Large Monorepo Playbook.
---

# Large Monorepo Playbook (Sparse and Mounts)

Status: Stable
Audience: Platform teams, monorepo maintainers, and staff-level developers

This playbook documents how to operate Arc in very large repositories without full-tree checkout penalties.

## The Challenge

A 50GB monorepo is rarely limited by raw compute alone. The dominant costs are:

- materializing unnecessary files,
- scanning irrelevant directories during local workflows,
- and coupling independent dependency islands into one operational blast radius.

Arc addresses this with sparse materialization and mount atoms while preserving a complete DAG model.

## Sparse Checkouts: Keep the DAG, Shrink the Working Set

Sparse mode projects only selected path prefixes into `work_root`.

Core commands:

```sh
arc sparse set services/api web/frontend
arc sparse list
arc sparse reset
```

Operational model:

- DAG history remains complete; sparse controls materialization scope only.
- Out-of-cone files are removed from disk projection but remain represented in repository history.
- `arc sparse reset` returns to full projection.

Recommended workflow:

1. Start each team with a minimal cone matching ownership boundaries.
2. Expand cone temporarily for cross-cutting refactors.
3. Reset only when full-repo operations are required.

## Mount Atoms: Dependency Islands Without Submodule Fragility

Arc mount operations:

```sh
arc mount add --path deps/frontend --url <url-or-path> --target main
arc mount sync
```

Why this replaces Git submodule-style pain:

- Mount intent is represented as typed graph state (`Atom::Mount`) with path/url/target.
- Mount boundaries are explicit and auditable in history.
- Dependency islands can evolve independently while remaining compositionally linked.

### Security and Trust Boundary Model

Mounts create explicit repository boundaries:

- Parent and mounted repos preserve independent provenance chains.
- Mount metadata is part of signed change history.
- Promotion into production can require both parent and mount verification policies.

Example architecture:

- Backend monorepo mounts a frontend repository at `deps/frontend`.
- Backend teams operate sparse cones in `services/*`.
- Frontend updates arrive through mount sync, not ad-hoc vendoring.

## Monorepo Operations Matrix

| Scenario                     | Primary commands                               | Expected outcome                                  | Common pitfall                             | Guardrail                             |
| ---------------------------- | ---------------------------------------------- | ------------------------------------------------- | ------------------------------------------ | ------------------------------------- |
| Team-local daily work        | `arc sparse set ...` + `arc snap`              | Fast local iteration with reduced tree size       | Cone too broad, performance regresses      | Keep cone prefixes ownership-scoped   |
| Cross-team integration       | `arc sparse set` (expanded) + `arc view merge` | Controlled integration with full DAG context      | Forgetting to reset cone after integration | Standardize post-merge sparse policy  |
| Dependency update via mount  | `arc mount sync`                               | Deterministic mounted dependency refresh          | Hidden drift in target view assumptions    | Pin/validate mount target conventions |
| Incident triage in huge repo | `ARC_TRACE_EVENT=... arc pull ...`             | High-fidelity timing data for bottleneck analysis | Running destructive cleanup before traces  | Capture traces before mutation        |

## Recommended Enterprise Pattern

1. Define canonical sparse profiles per team.
2. Treat mounts as contractual boundaries, not ad-hoc folder mirrors.
3. Enforce `arc verify` in CI on parent and mounted repos.
4. Track sparse and mount conventions in architecture governance docs.
