---
title: "Bisect and Bench Reference"
description: "Command reference for topological bisect and operation benchmarks."
---

# Bisect and Bench Reference

## BLUF

Use `arc bisect` to isolate the first bad (or good) change over a revset-defined candidate set. Use `arc bench` to measure DAG and revset core operations with deterministic iteration counts.

---

## Bisect Model

Arc stores bisect session state under `.arc/bisect/state.bin`.

Tri-state marks:

- `Good`
- `Bad`
- `Untested`

Next candidate selection is deterministic and topological midpoint over untested candidates.

---

## Bisect Commands

```bash
arc bisect start -r 'ancestors(@)'
arc bisect next
arc bisect good
arc bisect bad
arc bisect status
arc bisect reset
```

Find-first-good mode:

```bash
arc bisect start -r 'ancestors(@)' --find-good
```

In find-good mode, internal mark propagation is inverted so search semantics remain correct.

---

## Bench Commands

```bash
arc bench common-ancestors <left> <right> --iterations 200
arc bench is-ancestor <ancestor> <descendant> --iterations 200
arc bench resolve-prefix <prefix> --iterations 200
arc bench revset 'ancestors(@) & touched("src/main.rs")' --iterations 200
```

Bench is for operation-level performance checks, not whole-system profiling.

---

## Practical Output Reading

Bisect status fields to watch:

- range expression
- good/bad/untested counts
- current candidate

Bench output fields to watch:

- operation name
- iteration count
- aggregate duration
- per-iteration estimate

---

## See Also

- [Topological Bisect](../concepts/topological-bisect.md)
- [Isolate Regressions with Bisect and Bench](../howto/isolate-regressions-with-bisect-and-bench.md)
