---
title: Conflict Resolution
description: Documentation page for Conflict Resolution.
---

# Conflict Resolution Policy

Status: Stable policy for current implementation
Audience: Team leads, reviewers, and platform owners

This page defines when to use AI-assisted resolution and when to require manual intervention.

## Decision Tree: AI vs Manual

Use AI (`arc ai resolve` then `arc ai approve`) when:

- The conflict is mostly mechanical (formatting, imports, straightforward refactors).
- Intent from both sides is clear and low-risk.
- The affected surface is small and easily reviewable.

Prefer manual resolution (`edit + arc snap`) when:

- Business logic or security-sensitive behavior is involved.
- The conflict spans architectural boundaries.
- Reviewer confidence is low or semantics are ambiguous.

## Workflow States

1. Merge detects non-commuting deltas.
2. Arc records structured conflict (`Atom::Conflict`) and conflict metadata.
3. Team chooses AI or manual path.
4. AI path remains pending until explicit human approval.

## Trustless Security and Human Sponsorship

Arc uses a human-in-the-loop governance model for AI-authored changes:

- AI-authored commits are tagged as `Author::AI`.
- `Author::AI` includes a `human_sponsor` public key.
- Final approval (`arc ai approve`) signs with the human sponsor identity.
- Signature verification uses sponsor-backed provenance rules.

Why this matters:
AI cannot unilaterally authorize code into history. A human cryptographic sponsor is mandatory.

## Team Policy Recommendations

1. Define conflict classes that are AI-allowed vs human-required.
2. Require reviewer acknowledgment for all `Author::AI` merges.
3. Capture rationale in snap intent messages for auditability.
4. Keep `ARC_TRACE_EVENT` artifacts for high-risk conflict sessions.
