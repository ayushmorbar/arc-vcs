---
title: "Introduction"
description: "arc is an AI-native, AST-aware version control system with equal support for human-first manual workflows and optional autonomous agent workflows."
category: "Overview"
audience: "All"
---

# arc: Semantic Version Control for Humans and Agents

**arc is a version control system that records *why* you changed something,
not just what changed** — using typed semantic operations, cryptographic
provenance, and a pure mathematical change model instead of line diffs.

arc is designed for two equal user types. Neither path is secondary,
and AI features are always optional.

| Path | Who | What you get |
|---|---|---|
| **Human-first** | Developers using arc daily | Semantic diffs, safe undo/redo, ergonomic CLI, text-first feedback |
| **Agent-assisted** | Automation engineers and AI workflows | Typed intent surfaces, structured query primitives, MCP-ready skill manifests |


## How arc Works (5 Steps)

1. You edit files.
2. `arc snap` computes **semantic deltas** — AST-aware atoms, not raw line hunks.
3. Each `Change` object is content-addressed by **BLAKE3** and signed with **Ed25519**.
4. **Views** point to named head sets in the change graph.
5. Merge and rewrite operations apply **algebraic rules** — not line heuristics.

> These five steps are the same whether you are a human typing at a terminal
> or an agent issuing structured commands.


## If You Never Want AI

You still get the complete value of arc:

- Semantic change tracking over AST-aware atoms
- Safe undo/redo with causality-aware history operations
- Cryptographic provenance and deterministic, content-addressed storage
- Clear CLI workflows with predictable, text-first feedback


## If You Are Building Agentic Tooling

arc exposes structured surfaces for automation:

- Typed change atoms and semantic operations
- Intent-oriented query primitives via `arc-semantic-query`
- Agent skill manifests in `docs/src/agent-skills/`
- MCP-ready integration points across tooling surfaces

See [`agent-skills/index.md`](agent-skills/index.md) for the full agent
integration guide.


## Choose Your Starting Point

- **New to arc?** → [`learn/tutorial.md`](learn/tutorial.md)
- **Daily workflows** → [`learn/team-workflow.md`](learn/team-workflow.md)
- **How arc thinks** → [`concepts/conflict-algebra.md`](concepts/conflict-algebra.md)
- **Commands and flags** → [`reference/cli-reference.md`](reference/cli-reference.md)
- **System architecture** → [`reference/architecture/overview.md`](reference/architecture/overview.md)
- **Agent and MCP integration** → [`agent-skills/index.md`](agent-skills/index.md)


## Scope and Limitations

> **What is documented as implemented is code-verified.** If a capability
> has not shipped, it is explicitly marked with an admonition like:
> `> **Note:** This feature is planned for 0.x.0.`
> Nothing in this documentation is presented as working if it is not.

Workspace build truth lives in `crates/*`. The authoritative list of
what is implemented today is in [`reference/architecture/invariants.md`](reference/architecture/invariants.md).