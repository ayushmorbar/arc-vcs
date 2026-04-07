---
title: "CLI Reference"
description: "Canonical command surface for arc CLI, derived from runtime help output."
category: "Reference"
audience: "Developers"
---

# CLI Reference

Bottom line up front: this page is the canonical command index for `arc`.

## Source of Truth

This page is updated from runtime help output:

```bash
cargo run -p arc-cli -- -h
```

For command-specific flags and examples, run:

```bash
arc <command> --help
```

## Top-Level Commands

Current command surface:

- Repository lifecycle: `init`, `snap`, `watch`, `status`, `log`, `verify`, `info`
- History and rewrite: `amend`, `absorb`, `squash`, `reorder`, `restack`, `undo`, `redo`, `revert`, `describe`, `cherry-pick`, `blame`, `abandon`
- Collaboration and sync: `remote`, `fetch`, `pull`, `push`, `sync`, `serve`
- Views and refs: `view`, `branch`, `bookmark`, `checkout`, `tag`, `tags`, `tag-set`, `tag-delete`, `tag-list`
- Workspaces and scale: `workspace`, `sparse`, `mount`, `gc`, `compact`
- Tooling and diagnostics: `diff`, `diffedit`, `bisect`, `bench`, `op`, `policy`, `synthesis`, `bug-report`, `config`
- Identity and auth: `identity`, `auth`
- Import and onboarding: `import`, `tour`
- Utilities and metadata: `util`, `root`, `version`, `help`, `ai`, `stash`, `restore`, `commit` (unsupported alias guidance)

## Drift Check for Maintainers

When CLI behavior changes, update this page in the same PR and keep wording aligned with command help output.

Minimal check:

```bash
cargo run -p arc-cli -- -h
```

If command names changed, also update navigation and task guides that reference those commands.

## Notes

- `arc commit` remains intentionally unsupported; use `arc snap`.
- Prefer command help output over narrative prose for flag-level truth.

