---
title: Documentation Summary
description: Table of contents for arc documentation.
---

# Summary

[Introduction](introduction.md)

---

# Tutorials

- [Tutorial: Zero to First Snap](getting-started/tutorial.md)
- [First Conflict: Your Arc Aha Moment](getting-started/first-conflict.md)
- [Everyday Workflow](getting-started/everyday.md)
- [Team Workflow: Day 2 Operations](getting-started/team-workflow.md)
- [Migrating from Git](getting-started/git-migration.md)
- [Tutorial: First Useful Revset](tutorials/revset-basics.md)
- [Tutorial: Resolve a Semantic Conflict](tutorials/conflict-resolution-walkthrough.md)
- [Tutorial: Topological Bisect in Practice](tutorials/topological-bisect-walkthrough.md)
- [Tutorial: Linked Workspace with Sparse Scope](tutorials/workspace-sparse-onboarding.md)

---

# Reference

- [CLI Reference](reference/cli-reference.md)
- [Configuration](reference/config.md)
- [Ignore & Attributes](reference/ignore-and-attributes.md)
- [AI Intents & Resolution](reference/ai-intents.md)
- [Debugging and Hyper-Observability](reference/debugging.md)
- [Revsets](reference/revsets.md)
- [Bookmarks Reference](reference/bookmarks.md)
- [Conflict Resolution Protocol](reference/conflicts.md)
- [Bisect and Bench Reference](reference/bisect-and-bench.md)
- [Workspaces, Sparse, and Mounts](reference/workspaces-sparse-mounts.md)
- [Glossary](getting-started/glossary.md)
- [Dynamic Revsets](concepts/dynamic-revsets.md)
- [Conflict Algebra in Arc](concepts/conflict-algebra.md)
- [Topological Bisect](concepts/topological-bisect.md)
- [Workspace and Sparse Boundaries](concepts/workspace-boundaries.md)
- [OpLog and Optimistic Concurrency](concepts/oplog-concurrency.md)

---

# How-To Guides

- [Time-Travel With Operation Log](how-to/oplog-time-travel.md)
- [Custom Hooks](how-to/custom-hooks.md)
- [Revset-Driven Investigation](how-to/revset-driven-investigation.md)
- [Resolve Conflicts with AI or Merge Tool](how-to/resolve-conflicts-with-ai-or-merge-tool.md)
- [Isolate Regressions with Bisect and Bench](how-to/isolate-regressions-with-bisect-and-bench.md)
- [Safe Linked Workspaces](how-to/safe-linked-workspaces.md)
- [Recover from Divergent Heads](how-to/recover-from-divergence.md)
- [Collaborate with Multiple Remotes](how-to/multi-remote-collaboration.md)
- [CI Integration (GitHub Actions and GitLab CI)](how-to/ci-integration.md)
- [Troubleshoot Sync](how-to/troubleshoot-sync.md)
- [Release Docs Checklist](how-to/release-docs-checklist.md)

---

# Enterprise Operations

- [Disaster Recovery Runbook](how-to/disaster-recovery.md)
- [Performance and Maintenance Runbook](how-to/performance-maintenance.md)
- [Large Monorepo Playbook (Sparse and Mounts)](how-to/large-monorepo-playbook.md)

---

# Architecture

- [Architecture Overview](architecture/overview.md)
- [Documentation Map](architecture/documentation-map.md)
- [Architecture Risk Register](architecture/RISK_REGISTER.md)
- [Component Graph](architecture/component-graph.json)
- [Patch Theory](architecture/patch_theory.md)
- [AST Diffing](architecture/ast_diffing.md)
- [ADR 001 — BLAKE3 CAS](architecture/decisions/001-blake3-cas.md)
- [ADR 002 — AST over Text Diff](architecture/decisions/002-ast-over-text.md)
- [ADR 003 — CRDT over OT](architecture/decisions/003-crdt-over-ot.md)
- [ADR 004 — Gitoxide Architecture Study](architecture/decisions/004-gitoxide-architecture-study.md)
- [ADR 005 — Gitoxide Report Extraction](architecture/decisions/005-gitoxide-report-extraction.md)
- [ADR 006 - AiResolver static analysis sandwich](architecture/decisions/006-airesolver-pipeline.md)
- [Vision: Agentic Era (2026)](design/VISION.md)
- [Semantic Diff Engine](design/semantic_diff.md)
- [Spacetime Operation Log](design/oplog.md)
- [History Rewriting](design/history_rewriting.md)
- [Network Transport](design/network_transport.md)
- [Conflict Resolution Policy](design/conflict-resolution.md)
