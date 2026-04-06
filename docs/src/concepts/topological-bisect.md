---
title: "Topological Bisect"
description: "How arc bisect converges over DAG topology with tri-state marks."
---

# Topological Bisect

## BLUF

Arc bisect works over DAG topology with deterministic midpoint selection and tri-state propagation (`Good`, `Bad`, `Untested`). It is not a linear-history assumption patched onto a graph.

---

## State Model

A bisect session stores:

- candidate set (from revset)
- mark map (`Good`/`Bad`/`Untested`)
- current candidate
- mode (`find_good` or first-bad default)

Persistence under `.arc/bisect/state.bin` enables resumable workflows.

---

## Propagation Semantics

- Marking `Good` propagates through ancestors in-range.
- Marking `Bad` propagates through descendants in-range.
- Contradictions are rejected.

This enforces monotonic search constraints and accelerates convergence.

---

## Why Topological Midpoint

DAGs do not have one canonical linear midpoint. Arc chooses midpoint after topological ordering of remaining untested candidates, yielding deterministic behavior without pretending history is linear.

---

## Bench Pairing

`arc bench` complements bisect by timing core graph/revset operations used during diagnosis and prevention.

---

## See Also

- [Bisect and Bench Reference](../reference/bisect-and-bench.md)
- [Isolate Regressions with Bisect and Bench](../how-to/isolate-regressions-with-bisect-and-bench.md)

