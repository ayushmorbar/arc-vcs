---
title: Team Workflow
description: Documentation page for Team Workflow.
---

# Team Workflow: Day 2 Operations

Status: Stable
Audience: Teams collaborating daily on Arc

This guide covers the practical collaboration loop with remotes and explains what `pull` does mathematically.

## One-Time Setup

```sh
arc remote add origin <url-or-path>
arc remote list
```

`origin` can point to native Arc peers (filesystem/network) or bridge-compatible remotes depending on your deployment.

## Daily Team Loop

### 1. Start by integrating latest shared history

```sh
arc pull origin main
```

`pull` is `fetch` plus `merge_heads`, not a text-based rebase workflow.

### 2. Do feature work on a view

```sh
arc view create feature/my-change
arc view switch feature/my-change
# edit files
arc snap -m "feat: implement X"
```

### 3. Publish your work

```sh
arc push origin feature/my-change
```

### 4. Integrate back into main

```sh
arc checkout main
arc view merge feature/my-change
arc push origin main
```

## Why This Avoids Git-Style Rebase Pain

When you run `arc pull origin main`:

1. Arc fetches missing changes.
2. Arc computes merge base(s) and exclusive deltas.
3. Arc runs commutativity checks across those deltas.
4. If changes commute, heads are merged without crafting ad-hoc line-based merge commits.
5. If changes do not commute, Arc records explicit conflict state for structured resolution.

Result:
You avoid the recurring rebase conflict treadmill caused by purely line-oriented history surgery.

## Operational Tips

- Pull early, pull often: smaller delta sets mean faster conflict reasoning.
- Keep intent messages specific (`arc snap -m`) to improve review and AI resolution quality.
- Use [First Conflict](first-conflict.md) as the team playbook when commutativity fails.
