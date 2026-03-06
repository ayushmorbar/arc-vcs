---
name: arc-patch-theory
description: Core mathematical rules for commutativity, AST manipulation, and Patch Theory. Use whenever creating, merging, or modifying the Change struct.
---
# Instructions
1. **The Core Law:** If $\Delta_A$ and $\Delta_B$ do not share structural dependencies, they MUST commute: $apply(\Delta_B, apply(\Delta_A, S)) = apply(\Delta_{A'}, apply(\Delta_{B'}, S))$.
2. **AST Supremacy:** Never use line numbers or regex for diffs. You must define operations as `Insert(Node)`, `Delete(Node)`, `Move(From, To)`, or `SemanticsPreserving`.
3. **Dependencies:** Every `Change` must contain a `HashSet<Blake3Hash>` of its dependencies to form the partial order graph.