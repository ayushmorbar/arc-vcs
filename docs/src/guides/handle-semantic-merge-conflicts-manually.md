---
title: "How-To: Handle Semantic Merge Conflicts Manually"
description: "Resolve semantic conflicts in arc without AI, using deterministic manual workflows and safe rollback points."
category: "Guides"
audience: "Human"
---

# How-To: Handle Semantic Merge Conflicts Manually

This guide shows a human-first conflict workflow in arc. No AI is required.

## Outcome

By the end, you will:

1. identify the conflicting heads,
2. inspect the semantic conflict state,
3. resolve conflicts manually,
4. verify the result,
5. finalize a safe snapshot.

## When to use this guide

Use this when `arc` reports a semantic conflict and you want full manual control over the resolution.

> **Note:** arc treats conflict state as first-class data. You are resolving structured intent conflicts, not just editing line markers.

## Prerequisites

- A repository initialized with `arc init`
- A conflict state from concurrent edits
- Working familiarity with `arc log`, `arc status`, and `arc snap`

## Step 1: Inspect the active conflict state

Run:

```bash
arc status
arc log
```

Look for:

- multiple active heads,
- pending conflict indicators,
- files or symbols involved in non-commuting changes.

## Step 2: Inspect conflicting intent, not just file text

Use the conflict-oriented references:

```bash
arc log --revset "heads()"
arc status
```

Focus on:

- which symbols were edited,
- whether both sides changed the same semantic target,
- whether one side is a move/rename and the other is a behavior edit.

> **Tip:** if one side is mostly structural refactor and the other side is behavior changes, apply structural reconciliation first.

## Step 3: Perform manual file resolution

Open the affected files and produce the intended final state.

Guidelines:

1. Preserve behavior-critical edits first.
2. Re-apply structural refactors second.
3. Keep naming/type shape consistent after both edits are integrated.

Example conceptual conflict:

```rust
// branch A
fn compute_total(items: &[Item]) -> i64 { ... }

// branch B
fn sum_invoice(items: &[Item]) -> i64 { ... }
```

Manual resolution might become:

```rust
fn compute_total(items: &[Item]) -> i64 {
    // merged behavior + final naming decision
    ...
}
```

## Step 4: Validate before snapping

Run your local verification pipeline before recording the resolution.

```bash
cargo check
cargo test -p arc-core
```

If checks fail, fix the workspace first. Do not snap unstable resolution state.

## Step 5: Record the resolution snapshot

When the workspace is clean and validated:

```bash
arc snap -m "Resolve semantic conflict in billing totals"
```

Then confirm:

```bash
arc log
arc status
```

Expected result:

- conflict indicators cleared,
- a new head representing resolved semantic state,
- repository ready for subsequent integration steps.

## Recovery pattern (if you made a bad resolution)

If your manual merge is wrong, use arc's safe history operations:

```bash
arc undo
# or
arc op restore <operation-id>
```

> **Note:** keep resolution snapshots small and focused; smaller semantic units are easier to review, test, and revert.

## Related pages

- [Guides index](../index.mdx)
- [Spacetime DAG concept](../concepts/spacetime-dag.mdx)
- [Reference index](../reference/index.mdx)
