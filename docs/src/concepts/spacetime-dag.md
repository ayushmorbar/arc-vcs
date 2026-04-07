---
title: "The Spacetime DAG"
description: "Understand how arc tracks causal change without forcing a linear history narrative."
category: "Concepts"
audience: "All"
---

# The Spacetime DAG

The Spacetime DAG is arc's causal model of change.

Instead of treating history as a single line of commits, arc records typed semantic changes as nodes in a directed acyclic graph. Edges encode causal relationships, not just timestamp order.

## Why this matters

- You can preserve parallel intent safely.
- Merge and conflict behavior is modeled explicitly.
- Undo and replay operations are grounded in graph structure, not patch heuristics.

## Plain-English summary

A Spacetime DAG means arc remembers what changed, why it changed, and what each change depended on.
