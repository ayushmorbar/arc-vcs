---
title: "Agent Skills"
description: "Structured integration surface for autonomous agents and MCP-connected tooling."
category: "Agent Skills"
audience: "Agent"
---

# Agent Skills

This section documents the agent-facing contract for arc.

Canonical skills from the manifest:

- `arc diff --semantic`
- `arc ai-decompose`
- `arc snap --agent`
- `arc query --vibe`

Current implementation surface in `arc-cli` (today):

- `arc ai resolve`
- `arc ai approve`
- `arc ai generate`

> **Note:** `arc ai-decompose`, `arc snap --agent`, and `arc query --vibe` are skill-level contracts and planned product surfaces. They are not yet exposed as stable top-level CLI commands.

AI integration is optional and does not replace manual developer workflows.

## Skill Pages

- [arc-ghost-nodes](./arc-ghost-nodes.mdx)
- [arc-intent-reasoning](./arc-intent-reasoning.mdx)
- [arc-patch-theory](./arc-patch-theory.mdx)
- [arc-semantic-query](./arc-semantic-query.mdx)
- [arc-sync-protocol](./arc-sync-protocol.mdx)
