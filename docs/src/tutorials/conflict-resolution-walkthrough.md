# Tutorial: Resolve a Semantic Conflict

## Goal

Experience arc's full conflict loop: conflict state, staged resolution, explicit approval.

---

## Step 1: Reach conflicted state

Run your normal merge flow until arc reports semantic conflict and writes `.arc/conflict`.

## Step 2: Stage resolution

```bash
arc ai resolve
```

If a merge tool is configured, arc attempts it first; otherwise provider AI path is used.

## Step 3: Validate staged output

```bash
arc diff
arc status
```

## Step 4: Finalize

```bash
arc ai approve
```

---

## Result

You resolved a graph-native conflict and finalized it through explicit approval, preserving provenance and operator control.
