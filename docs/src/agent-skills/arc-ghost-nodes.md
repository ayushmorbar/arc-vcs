---
name: arc-ghost-nodes
description: >
  Rules for autonomous agent snapshots and provisional state. Use when 
  the agent needs to save its work, run `arc snap --agent`, or manage 
  changes that require human sponsorship.
---

# arc-ghost-nodes

## Purpose
AI-generated code in `arc-vcs` is advisory until verified. When the agent acts autonomously, it must checkpoint its work using **Ghost Nodes** rather than standard stable commits.

## The Ghost Node Protocol

1. **Provisional Snapshots:** When checkpointing work, use `arc snap --agent` (or the internal equivalent). This creates a Ghost Node snapshot with intent summaries and provenance metadata indicating AI authorship.
2. **Mutual Exclusion:** Before initiating a ghost snapshot, verify that `.arc/ai/pending.json` does not lock the repository. If it does, bail out with a clear error to prevent interleaved state.
3. **Non-Stable Marking:** Ensure the `OpRecord` flags the change as `stable = false` or requires a `SponsorshipGate`.
4. **Undo Boundaries:** Keep ghost nodes highly granular. They act as safe undo/compaction boundaries for local-only operations.

## Transition to Stable (Sponsorship)
If asked to "approve", "merge", or "finalize" an AI change:
- Do not bypass the verification hooks.
- A human must trigger `arc ai approve` or the internal transition mechanism to promote the Ghost Node into the stable DAG history.
- Ensure the transition logs the original AI provenance alongside the human sponsor's signature.