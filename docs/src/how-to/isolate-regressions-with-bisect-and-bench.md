---
title: "Isolate Regressions with Bisect and Bench"
description: "Use bisect to locate causative changes and bench to quantify impact."
---

# Isolate Regressions with Bisect and Bench

## BLUF

Use `arc bisect` to identify the first causative change, then run `arc bench` to quantify operational impact and guard against recurrence.

---

## Step 1: Start bisect over a bounded range

```bash
arc bisect start -r 'ancestors(@)'
arc bisect status
```

## Step 2: Test candidate and mark

```bash
arc bisect next
# run your test/build/check
arc bisect good   # or: arc bisect bad
```

Repeat `next` + mark until convergence.

## Step 3: Capture final candidate

```bash
arc bisect status
arc show <candidate-id>
```

## Step 4: Measure graph/revset surfaces

```bash
arc bench common-ancestors <left> <right> --iterations 200
arc bench revset 'ancestors(@) & touched("src/main.rs")' --iterations 200
```

Archive these numbers in incident notes or CI baselines.

---

## Troubleshooting

- "no active bisect session": run `arc bisect start` first.
- Contradiction error after mark: a previous propagated mark conflicts with latest assertion.
- Candidate set too broad: use a narrower revset range expression.

---

## See Also

- [Bisect and Bench Reference](../reference/bisect-and-bench.md)
