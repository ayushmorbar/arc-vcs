---
title: "Recover from Divergent Heads"
description: "Resolve concurrent or contradictory head movement using operation replay and targeted merge/snap workflows."
---

# Recover from Divergent Heads

## BLUF

When head state diverges unexpectedly, use the operation timeline first (`arc op log`), then restore or revert a specific boundary before applying semantic fixes.

---

## Step 1: Diagnose divergence boundary

```bash
arc op log
arc log -r "ancestors(@)"
```

Expected signal:

```text
... before -> after transitions show the first unexpected head jump
```

---

## Step 2: Re-enter known-good state

```bash
arc op restore <op-id>
arc status
```

If one operation was specifically wrong:

```bash
arc op revert <op-id>
arc status
```

---

## Step 3: Re-apply intended semantic changes

```bash
arc view merge <view-name>
arc snap -i -m "resolve divergence cleanly"
```

> **Note:** `arc snap -i` keeps conflict recovery auditable because staged atoms are explicit before signing.

---

## Step 4: Validate policy and graph integrity

```bash
arc verify --workspace-policy
arc log -r "ancestors(@)"
```

Expected output excerpt:

```text
Workspace policy: verified
```

---

## Troubleshooting

| Symptom | Likely Cause | Action |
| --- | --- | --- |
| "ambiguous operation id" | Prefix collision | Copy a longer id from `arc op log` |
| restore succeeds but state still wrong | Target op was already post-failure | restore an earlier operation |
| verify fails | policy drift in root files | fix policy files and re-run verify |

---

## See Also

- [Time-Travel With Operation Log](oplog-time-travel.md)
- [Topological Bisect](../concepts/topological-bisect.md)
