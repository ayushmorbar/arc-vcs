# First Conflict: Your Arc Aha Moment

Status: Stable
Audience: Developers migrating from Git

This guide explains what Arc does when `commutes() == false`, how that differs from Git conflict behavior, and how to resolve conflicts with AI or manual intervention.

## What Triggers a Conflict

Arc computes exclusive deltas for each side of a merge and checks pairwise commutativity.

- If all pairs commute: Arc unions heads and completes merge.
- If any pair does not commute: Arc records a conflict state as first-class algebra (`Atom::Conflict`).

## Git vs Arc Conflict Model

| Topic | Git | Arc |
|---|---|---|
| Conflict representation | Inline text markers in a merge result | `Atom::Conflict` stored in the change DAG |
| Data fidelity | Markerized file content only | Conflict bases/sides persisted by CAS hash + anchor path |
| Merge determinism | Depends on line-based merge heuristics | Conflict detection is explicit from `commutes()` over atoms |
| Working copy projection | Immediate markerized file edits | Conflict node recorded first, then deterministic projection to files |

Important:
Arc currently projects conflict markers to working files for conflicted paths, but the conflict is also preserved as structured graph state, not only as ad-hoc file text.

## Walkthrough

### 1. Create competing edits

```sh
arc view create feature/a
arc view switch feature/a
# edit the same function/path as main will edit
arc snap -m "feat: change path A"

arc view switch main
# edit the same function/path differently
arc snap -m "feat: change path B"
```

### 2. Merge and observe conflict state

```sh
arc view merge feature/a
```

On conflict, Arc creates structured conflict state and persists metadata to `.arc/conflict` for resolver workflows.

### 3. Resolve path A: AI resolver

```sh
# required for provider-backed resolve
set ARC_AI_API_KEY=your_key_here

arc ai resolve
# inspect resulting working tree changes
arc diff

# human approval gate (final signature + commit)
arc ai approve
```

`arc ai approve` is the governance checkpoint that turns pending AI output into a signed, permanent change.

### 4. Resolve path B: Manual fallback

If you do not want to use AI resolution:

1. Open the conflicted file(s) and edit to the desired final code.
2. Validate locally (build/tests).
3. Commit your manual resolution.

```sh
arc snap -m "resolve conflict manually"
```

This path is fully supported and keeps human control over conflict decisions.

Note:
If you resolve manually, any existing `.arc/conflict` metadata may remain until a resolver flow clears it. If follow-up commands report a stale pending conflict, remove `.arc/conflict` after confirming your manual resolution is snapped.

## What to Read Next

- [Team Workflow](team-workflow.md)
- [Debugging](../reference/debugging.md)
- [Conflict Resolution Policy](../design/conflict-resolution.md)
