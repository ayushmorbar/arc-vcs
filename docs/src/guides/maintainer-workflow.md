---
title: "Maintainer Workflow"
description: "Operational runbook for maintainers reviewing, gating, and preparing arc changes."
category: "Guides"
audience: "Maintainers"
---

# Maintainer Workflow

Bottom line up front: this runbook keeps `main` green, policy-compliant, and release-ready.

## 1. Triage Incoming PRs

Classify each PR quickly:

- Docs-only: verify links and mdBook build.
- Behavioral change: require tests and docs sync.
- Boundary or schema impact: require explicit rationale and architecture review.

## 2. Enforce Non-Negotiable Checks

Require passing results for:

```bash
just verify-fast
```

Escalate to:

```bash
just verify-full
```

when the change touches security, policy, release, network sync, or storage behavior.

## 3. Review by Risk Surface

Review in this order:

1. Correctness and invariants
2. Boundary discipline (pure vs I/O crates)
3. Security and provenance impacts
4. Documentation sync and user-facing clarity

## 4. Validate CLI and Docs Drift

For CLI-affecting PRs, verify with runtime help output:

```bash
cargo run -p arc-cli -- -h
```

Then ensure `docs/src/reference/cli-reference.md` matches the command surface.

## 5. Merge Readiness Gate

Before merge, confirm:

- CI is green.
- No unresolved reviewer blocking comments.
- CHANGELOG/ADR updates are present when required.
- Docs references are not stale (including `docs/src/SUMMARY.md`).

## 6. Release-Adjacent Changes

For release prep and publishing docs updates, use:

- `docs/src/guides/release-docs-checklist.md`
- root `CHANGELOG.md`
- root `SECURITY.md` for security-related release notes

## Incident Shortcut

If `main` is broken:

1. Prioritize restoring green CI over feature throughput.
2. Revert or hotfix the smallest safe unit.
3. Capture root cause and preventive action in follow-up docs or ADR updates.
