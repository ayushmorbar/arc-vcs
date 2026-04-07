---
name: arc-intent-reasoning
description: >
  Reasoning framework for decomposing large code changes into commutative,
  independent semantic intents. Use when asked to split diffs, refactor
  code across multiple logical steps, or generate an intent graph.
---

# arc-intent-reasoning

## Purpose
`arc` models changes as an Intent Graph, not a linear history. When analyzing a monolithic change or planning a multi-step refactor, you must decompose the work into orthogonal, commutative subchanges.

## Decomposition Protocol

When asked to break down a change or reason about a complex refactoring:

1. **Infer Intent Nodes:** Analyze the current delta and identify distinct goals (e.g., "Extract trait", "Rename variable", "Update tests").
2. **Partition by Dependency Edges:** Determine which intents rely on others. If Intent B requires the signature changed in Intent A, draw a directed edge `A -> B`.
3. **Establish Commutativity Boundaries:** If Intent C (e.g., format a different file) does not share AST structural dependencies with A or B, mark it as commutative. It can be applied in any order.
4. **Propose Sequenced Subchanges:** Output a plan where each unit is replayable, independently reversible, and semantically pure.

## Output Format for Decomposition
Whenever proposing a decomposition, use this structure:

```text
**Intent 1: [Semantic Label]** (Independent)
- Action: [What AST nodes are changing]
- Preconditions: None
- Verification: [How to test this specific intent]

**Intent 2: [Semantic Label]** (Depends on Intent 1)
- Action: [What AST nodes are changing]
- Preconditions: Intent 1 must materialize first.
- Verification: [Validation step]
```

## Anti-Patterns
- Never decompose by file name (e.g., "Part 1: foo.rs, Part 2: bar.rs"). Files are a storage illusion; decompose by **AST semantics**.
- Never propose a subchange that leaves the repository in a broken compilation state. Every intent node must be valid.