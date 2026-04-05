# Summary

[Introduction](introduction.md)

---

# Getting Started

- [Tutorial: Zero to First Snap](getting-started/tutorial.md)
- [First Conflict: Your Arc Aha Moment](getting-started/first-conflict.md)
- [Everyday Workflow](getting-started/everyday.md)
- [Team Workflow: Day 2 Operations](getting-started/team-workflow.md)
- [Migrating from Git](getting-started/git-migration.md)
- [Glossary](getting-started/glossary.md)

---

# Tutorials

- [Tutorial: First Useful Revset](tutorials/revset-basics.md)
- [Tutorial: Resolve a Semantic Conflict](tutorials/conflict-resolution-walkthrough.md)
- [Tutorial: Topological Bisect in Practice](tutorials/topological-bisect-walkthrough.md)
- [Tutorial: Linked Workspace with Sparse Scope](tutorials/workspace-sparse-onboarding.md)

---

# Concepts

- [Dynamic Revsets](concepts/dynamic-revsets.md)
- [Conflict Algebra in Arc](concepts/conflict-algebra.md)
- [Topological Bisect](concepts/topological-bisect.md)
- [Workspace and Sparse Boundaries](concepts/workspace-boundaries.md)

---

# Reference

- [CLI Reference](reference/cli-reference.md)
- [Configuration](reference/config.md)
- [Ignore & Attributes](reference/ignore-and-attributes.md)
- [AI Intents & Resolution](reference/ai-intents.md)
- [Debugging and Hyper-Observability](reference/debugging.md)
- [Revsets](reference/revsets.md)
- [Conflict Resolution Protocol](reference/conflicts.md)
- [Bisect and Bench Reference](reference/bisect-and-bench.md)
- [Workspaces, Sparse, and Mounts](reference/workspaces-sparse-mounts.md)

---

# Design

- [Vision: Agentic Era (2026)](design/VISION.md)
- [ADR 001 - Change Algebra](design/ADR-001-Change-Algebra.md)
- [ADR 002 - Jujutsu Workflow](design/ADR-002-Jujutsu-Workflow.md)
- [ADR 003 - Git Bridge](design/ADR-003-Git-Bridge.md)
- [Patch Theory](design/patch_theory.md)
- [CRDT Network Sync](design/crdt_sync.md)
- [AST Diffing](design/ast_diffing.md)
- [Semantic Diff Engine](design/semantic_diff.md)
- [Spacetime Operation Log](design/oplog.md)
- [History Rewriting](design/history_rewriting.md)
- [Network Transport](design/network_transport.md)
- [Conflict Resolution Policy](design/conflict-resolution.md)

---

# How-To Guides

- [Time-Travel With Operation Log](howto/oplog-time-travel.md)
- [Custom Hooks](howto/custom-hooks.md)
- [Revset-Driven Investigation](howto/revset-driven-investigation.md)
- [Resolve Conflicts with AI or Merge Tool](howto/resolve-conflicts-with-ai-or-merge-tool.md)
- [Isolate Regressions with Bisect and Bench](howto/isolate-regressions-with-bisect-and-bench.md)
- [Safe Linked Workspaces](howto/safe-linked-workspaces.md)
- [CI Integration (GitHub Actions and GitLab CI)](howto/ci-integration.md)
- [Troubleshoot Sync](howto/troubleshoot-sync.md)
- [Release Docs Checklist](howto/release-docs-checklist.md)

---

# Enterprise Operations

- [Disaster Recovery Runbook](howto/disaster-recovery.md)
- [Performance and Maintenance Runbook](howto/performance-maintenance.md)
- [Large Monorepo Playbook (Sparse and Mounts)](howto/large-monorepo-playbook.md)

---

# Architecture & ADRs

- [Architecture Overview](architecture/overview.md)
- [Documentation Map](architecture/documentation-map.md)
- [ADR 001 — BLAKE3 CAS](architecture/ADRs/001-blake3-cas.md)
- [ADR 002 — AST over Text Diff](architecture/ADRs/002-ast-over-text.md)
- [ADR 003 — CRDT over OT](architecture/ADRs/003-crdt-over-ot.md)
