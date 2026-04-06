---
title: "Conflict Resolution Protocol"
description: "Reference for graph-native conflict handling and staged resolution in arc."
---

# Conflict Resolution Protocol

## BLUF

Arc conflicts are graph-native (`Atom::Conflict`), not text-hunk accidents. Resolution flows produce a pending Ghost Node (`PendingAiChange`) and require explicit approval with `arc ai approve`.

---

## Conflict State in Arc

When merge commutativity fails, arc persists conflict state in `.arc/conflict` and records conflicting change pairs.

Key elements:

- `Atom::Conflict` represents unresolved semantic collision.
- `PendingConflict` is persisted conflict context used during resolution.
- `.arc/conflict` is removed after a successful resolve staging step.

---

## Resolution Paths

### 1. AI provider path

```bash
# Requires ARC_AI_API_KEY at runtime
arc ai resolve
arc ai approve
```

Behavior:

1. Loads `.arc/conflict`
2. Computes base/ours/theirs for each conflicting path
3. Produces resolved inserts
4. Saves `PendingAiChange` (kind resolve)
5. Removes `.arc/conflict`
6. Waits for explicit `arc ai approve`

### 2. External merge-tool path (configured)

If `merge.tool` is set and defined under `[merge-tools.<name>]`, `arc ai resolve` attempts merge-tool resolution first.

If the merge tool fails to execute, exits non-zero, or yields empty output, arc falls back to AI provider resolution.

Both paths stage a Ghost Node; neither path finalizes history without approval.

---

## Operational Guardrails

- If a pending AI change already exists, resolve is blocked until approval or manual discard.
- Rust outputs are parser-verified before staging.
- Unresolved conflicts cannot be exported through the Git bridge.

---

## Configuration Surface

```toml
[merge]
tool = "meld"

[merge-tools.meld]
program = "meld"
merge_args = ["$left", "$base", "$right", "-o", "$output", "--auto-merge"]
```

---

## See Also

- [Conflict Algebra](../concepts/conflict-algebra.md)
- [Resolve Conflicts with AI or Merge Tool](../how-to/resolve-conflicts-with-ai-or-merge-tool.md)

