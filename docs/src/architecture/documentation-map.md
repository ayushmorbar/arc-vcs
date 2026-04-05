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
  getting-started/
    tutorial.md
    everyday.md
    git-migration.md
    glossary.md
  reference/
    cli-reference.md
    config.md
    ai-intents.md
    ignore-and-attributes.md
  architecture/
    overview.md
    documentation-map.md
    patch_theory.md
    ast_diffing.md
    ADRs/
      001-blake3-cas.md
      002-ast-over-text.md
      003-crdt-over-ot.md
  design/
    VISION.md
    ADR-001-Change-Algebra.md
    ADR-002-Jujutsu-Workflow.md
    ADR-003-Git-Bridge.md
    patch_theory.md
    ast_diffing.md
    semantic_diff.md
    crdt_sync.md
    oplog.md
    history_rewriting.md
    network_transport.md
  howto/
    custom-hooks.md
    troubleshoot-sync.md
    release-docs-checklist.md
```

## Ownership and Synchronization

- Canonical architecture index (wiki-facing): `Architecture.md`.
- Canonical user/developer manual (book-facing): `docs/src/*`.
- If overlap exists, mdBook text is canonical and wiki files link back to it.

## Scope Guardrails (KISS, YAGNI, SSOT)

- Do not present roadmap items as shipped behavior.
- Prefer code-verified statements over aspirational language.
- Keep one canonical explanation per concept, then cross-link.
