---
title: Documentation Map
description: Documentation page for Documentation Map.
---

# Documentation Map (2026 DX/UX)

Purpose: Single source of truth for what is documented, where it lives, and which layer it serves.

## Audience Layers

- New users: fast mental model, first-success path.
- Working developers: command confidence and operational safety.
- Experts and maintainers: causal/architectural rationale and boundaries.
- AI agents: semantically stable sectioning and explicit ownership.

## Updated File Tree

```text
docs/src/
  introduction.md
  tutorials/
    revset-basics.md
    conflict-resolution-walkthrough.md
    topological-bisect-walkthrough.md
    workspace-sparse-onboarding.md
  how-to/
    oplog-time-travel.md
    custom-hooks.md
    troubleshoot-sync.md
    release-docs-checklist.md
  reference/
    cli-reference.md
    config.md
    ai-intents.md
    ignore-and-attributes.md
    revsets.md
    conflicts.md
    workspaces-sparse-mounts.md
  architecture/
    overview.md
    RISK_REGISTER.md
    component-graph.json
    documentation-map.md
    patch_theory.md
    ast_diffing.md
    decisions/
      001-blake3-cas.md
      002-ast-over-text.md
      003-crdt-over-ot.md
      004-gitoxide-architecture-study.md
      005-gitoxide-report-extraction.md
```

## Ownership and Synchronization

- Canonical architecture index (wiki-facing): `Architecture.md`.
- Canonical user/developer manual (book-facing): `docs/src/*`.
- If overlap exists, mdBook text is canonical and wiki files link back to it.

## Scope Guardrails (KISS, YAGNI, SSOT)

- Do not present roadmap items as shipped behavior.
- Prefer code-verified statements over aspirational language.
- Keep one canonical explanation per concept, then cross-link.

