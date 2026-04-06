---
title: Cli Reference
description: Documentation page for Cli Reference.
---

# CLI Reference

## BLUF

Arc commands operate on two linked timelines:

- Change timeline: semantic changes (`arc snap`, `arc log`, `arc diff`, `arc revert`).
- Operation timeline: repository/view transitions (`arc op log`, `arc op restore`, `arc op revert`, `arc undo`).

If you are correcting repository state, start with operation commands.

---

## Global

```bash
arc --help
arc --version
```

Use command-local help for authoritative flags:

```bash
arc <command> --help
```

---

## Core Daily Commands

### Initialize

```bash
arc init [path] [--no-git]
```

- Creates `.arc/` metadata.
- Optionally imports Git history unless `--no-git` is set.

### Snapshot

```bash
arc snap -m "message"
arc snap --auto-msg
arc snap -i -m "message"
```

- Records semantic working changes as a new change.
- `--auto-msg` asks configured AI for a message.
- `-i/--interactive` enables interactive atom staging.

### Inspect Working State

```bash
arc status
arc diff [--semantic]
```

- `status` shows pending semantic atoms.
- `diff --semantic` emphasizes structure over plain text.

### Inspect History

```bash
arc log [-r <revset>] [--intent <query>] [--template <row-template>]
```

- Default revset is `ancestors(@)`.
- `--intent` performs semantic retrieval.
- `--template` controls non-semantic row rendering.

---

## Operation Log Time-Travel

### View Operation Timeline

```bash
arc op log
```

Shows operation id, timestamp, view, agent, command, and before/after heads.

### Restore To Post-Operation State

```bash
arc op restore <op-id>
```

Repositions current view to the selected operation's resulting state.

### Revert A Specific Operation

```bash
arc op revert <op-id>
```

Negates the selected operation by restoring its pre-operation heads.

### Undo Most Recent Operation

```bash
arc undo
```

Fast rollback for the latest view-mutating operation.

See also: [Time-Travel With Operation Log](../how-to/oplog-time-travel.md).

---

## Change Operations

```bash
arc cherry-pick <hash>
arc revert <hash-or-ref>
arc restore <filepath>
arc amend [-m <message>]
arc squash --into <rev>
```

- `cherry-pick`: port one change into current view.
- `revert`: semantically invert a change.
- `restore`: restore a file to snapped state.
- `amend`/`squash`: history-shaping commands.

### Diff Edit Workflow

```bash
arc diffedit --prepare <rev> [-m <message>]
arc diffedit --apply [-m <message>]
```

Two-step external edit/apply flow for controlled rewrites.

---

## Views, Checkout, and Branch-Like Flows

```bash
arc view create <name>
arc view switch <name>
arc view merge <name>
```

Aliases:

```bash
arc checkout <name>
arc branch [name]
```

- `checkout` aliases `view switch`.
- `branch` lists views when omitted, creates when provided.

---

## Verification and Policy

```bash
arc verify [--tooling] [--governance] [--workspace-policy]
```

- Base `verify`: provenance/signature consistency.
- `--tooling`: validates reproducible tooling policies under `.config/`.
- `--governance`: validates governance/CI policy under `.github/`.
- `--workspace-policy`: validates root workspace policy files (for example `.editorconfig`, `.gitattributes`, and required directives).

---

## Bisect and Bench

### Bisect

```bash
arc bisect start -r <revset> [--find-good]
arc bisect next
arc bisect good
arc bisect bad
arc bisect status
arc bisect reset
```

- Executes topological bisect over a revset-defined candidate range.
- `--find-good` inverts the search objective.

### Bench

```bash
arc bench common-ancestors <left> <right> [--iterations N]
arc bench is-ancestor <ancestor> <descendant> [--iterations N]
arc bench resolve-prefix <prefix> [--iterations N]
arc bench revset <expression> [--iterations N]
```

Use these for graph/revset performance diagnostics and regression tracking.

---

## Identity and Auth

```bash
arc auth login --name <name> --email <email>
arc auth whoami
arc identity --name <name> --email <email>
```

Use identity commands to configure signing metadata and operator attribution.

---

## Configuration Surface

```bash
arc config alias <name> <expansion>
arc config aliases
arc config get <key>
arc config set <key> <value>
arc config unset <key>
arc config path
arc config edit
arc config list
```

See [Configuration](config.md) for schema, layering, defaults, and advanced keys.

---

## Remotes and Sync

### Remote Aliases

```bash
arc remote add <name> <url-or-path>
arc remote list
arc remote remove <name>
```

### Interop and Native Sync

```bash
arc import git <git_path>
arc push <remote_url_or_alias> [view]
arc fetch <remote_path> <view>
arc pull <remote_path> <view>
arc sync <host:port>
arc serve [--port <port>]
```

---

## Monorepo and Workspace

### Sparse

```bash
arc sparse set <path>...
arc sparse edit
arc sparse list
arc sparse reset
```

### Mount

```bash
arc mount add --path <path> --url <url-or-path> --target <view>
arc mount sync
```

### Workspace

```bash
arc workspace add <path> [--view <name>]
arc workspace list
```

---

## AI Commands

```bash
arc ai resolve
arc ai approve
arc ai generate --goal <text> [--file <path>]
```

- `resolve`: propose resolution for pending conflict state.
- `approve`: explicitly accepts/signs pending AI result.
- `generate`: drafts file edits from operator goal.

---

## Tags, Stash, and Maintenance

### Tags

```bash
arc tag <name> <hash-or-ref>
arc tags
arc tag-set --rev <rev> [--allow-move] <name>...
arc tag-delete <pattern>...
```

### Stash

```bash
arc stash push
arc stash pop
arc stash list
```

### Maintenance

```bash
arc gc [--dry-run]
arc compact
```

- `gc`: reclaim unreachable/stable storage.
- `compact`: advanced history compaction.

---

## Internal and Tooling

```bash
arc daemon
```

Internal integration command for editor/tooling JSON-RPC workflows.

---

## Compatibility Notes

- `arc commit` is intentionally unsupported; use `arc snap`.
- Prefer operation commands for state recovery and change commands for content-level edits.

