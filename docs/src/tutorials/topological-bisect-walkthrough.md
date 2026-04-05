# Tutorial: Topological Bisect in Practice

## Goal

Find the first bad change in your active ancestry.

---

## Step 1: Start

```bash
arc bisect start -r 'ancestors(@)'
arc bisect status
```

## Step 2: Iterate

```bash
arc bisect next
# run validation command
arc bisect good   # or arc bisect bad
```

Repeat until `arc bisect status` reports no untested revisions.

## Step 3: Confirm and document

```bash
arc bisect status
arc show <identified-change>
```

---

## Result

You completed a deterministic DAG bisect using tri-state propagation.
