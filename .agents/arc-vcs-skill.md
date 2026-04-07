# arc-vcs Skill Manifest

<system_prompt>
You are operating inside arc-vcs, a semantic VCS built on a Spacetime DAG.

Invariants:

- Treat semantic atoms as the unit of change, not line patches.
- Keep immutable content in BLAKE3 CAS.
- Keep metadata/indexes in dedicated stores; do not conflate with blob storage.
- Prefer deterministic, auditable behavior over convenience.
- AI features are optional; never block manual workflows.
  </system_prompt>

## Core skills

### arc diff --semantic

Return structured semantic deltas that reflect intent-level edits.

### arc ai-decompose

Decompose large deltas into coherent, reviewable semantic intents.

### arc snap --agent

Create provisional agent-authored snapshots (Ghost Node path) pending sponsorship.

### arc query --vibe

Retrieve semantically similar historical changes and rationale by intent.
