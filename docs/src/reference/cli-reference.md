# CLI Reference

Command map for the `arc` binary.

For command-local flags and newest examples, run `arc <command> --help`.

## Global

- `arc --help`
- `arc --version`

## Repository Lifecycle

- `arc init [path] [--no-git]`
- `arc status`
- `arc diff [--semantic]`
- `arc snap -m <message> [--interactive] [--auto-msg]`
- `arc log [-r <revset>] [--intent <query>]`
- `arc verify`
- `arc info`
- `arc bug-report [--output <file>] [--include-raw-intent]`
- `arc blame <filepath>`
- `arc tour`
- `arc commit` (intentionally unsupported compatibility command)

## Change Operations

- `arc cherry-pick <hash>`
- `arc revert <hash-or-ref>`
- `arc restore <filepath>`
- `arc amend [-m <message>]`
- `arc squash --into <rev>`
- `arc diffedit --prepare <rev> [-m <message>]`
- `arc diffedit --apply [-m <message>]`

## Views and History Control

- `arc view create <name>`
- `arc view switch <name>`
- `arc view merge <name>`
- `arc checkout <name>` (alias for `view switch`)
- `arc branch [name]` (without `name`: list views; with `name`: create)
- `arc undo`
- `arc op log`

## Stash

- `arc stash push`
- `arc stash pop`
- `arc stash list`

## Tags

- `arc tag <name> <hash-or-ref>`
- `arc tags`

## Identity and Configuration

- `arc auth login --name <name> --email <email>`
- `arc auth whoami`
- `arc identity --name <name> --email <email>`
- `arc config [--global] alias <name> <expansion>`
- `arc config [--global] aliases`
- `arc config [--global] get <key>`
- `arc config [--global] set <key> <value>`
- `arc config [--global] unset <key>`
- `arc config [--global] list`

## AI Commands

- `arc ai resolve`
- `arc ai approve`
- `arc ai generate --goal <text> [--file <path>]`

## Import and Interop

- `arc import git <git_path>`
- `arc push <remote_url_or_alias> [view]`

When the resolved remote is `http` or `https`, push uses the Git Smart HTTP translation bridge from `arc-git-bridge`.

## Native Sync and Networking

- `arc sync <host:port>`
- `arc fetch <remote_path> <view>`
- `arc pull <remote_path> <view>`
- `arc serve [--port <port>]`
- `arc remote add <name> <url-or-path>`
- `arc remote list`
- `arc remote remove <name>`

## Workspace and Monorepo Features

- `arc sparse set <path>...`
- `arc sparse list`
- `arc sparse reset`
- `arc mount add --path <path> --url <url-or-path> --target <view>`
- `arc mount sync`
- `arc workspace add <path> [--view <name>]`
- `arc workspace list`

## Storage Maintenance

- `arc gc [--dry-run]`
- `arc compact`

## Hidden / Internal

- `arc daemon`

Used by editor integrations to start the JSON-RPC daemon backend.
