# CLI Reference

Status key:

- Stable: expected daily user surface
- Advanced: power-user or specialist operation
- Internal: integration/runtime command, not normal workflow

Use `arc <command> --help` for command-local defaults and clap-generated details.

## Global

Syntax:

- `arc --help`
- `arc --version`

Mental model:
Global flags are discovery and version checks; behavior lives in subcommands.

## Repository Lifecycle (Stable)

### `arc init`

- Syntax: `arc init [path] [--no-git]`
- Why: create Arc repository metadata and optional Git import onboarding path.
- Flags:
  - Safety: `--no-git` skips git auto-detection/import prompt.
- Example: `arc init .`

### `arc snap`

- Syntax: `arc snap -m <message> [--auto-msg] [--interactive]`
- Why: snapshot working semantic delta into a signed change.
- Flags:
  - Output: `-m, --message` explicit intent message.
  - Productivity: `--auto-msg` asks configured AI to synthesize message.
  - Advanced: `-i, --interactive` is accepted but currently deprecated and ignored in current auto-snapshot flow.
- Example: `arc snap -m "feat: add parser"`

### `arc status`

- Syntax: `arc status`
- Why: inspect unsnapped semantic changes for current view.
- Example: `arc status`

### `arc diff`

- Syntax: `arc diff [--semantic]`
- Why: view working delta as text or structural semantic view.
- Flags:
  - Output: `--semantic` for structural/intent-centric output.
- Example: `arc diff --semantic`

### `arc log`

- Syntax: `arc log [-r <revset>] [--intent <query>]`
- Why: inspect history by graph expression or semantic query.
- Flags:
  - Output: `-r, --revset` graph selection expression (default `ancestors(@)`).
  - Advanced: `--intent` embedding-based semantic lookup.
- Example: `arc log -r "ancestors(@)"`

### `arc verify`

- Syntax: `arc verify`
- Why: verify graph provenance/signature consistency.
- Example: `arc verify`

### `arc blame`

- Syntax: `arc blame <filepath>`
- Why: attribute semantic nodes to authored changes.
- Example: `arc blame src/main.rs`

### `arc info`

- Syntax: `arc info`
- Why: show repository telemetry/dashboard summary.

### `arc bug-report`

- Syntax: `arc bug-report [--output <file>] [--include-raw-intent]`
- Why: create reproducible support artifact.
- Flags:
  - Output: `--output` path override.
  - Safety: `--include-raw-intent` may include sensitive text.

### `arc tour`

- Syntax: `arc tour`
- Why: interactive onboarding in terminal.

## Change Operations (Stable + Advanced)

### `arc cherry-pick`

- Syntax: `arc cherry-pick <hash>`
- Why: port one change into current view.

### `arc revert`

- Syntax: `arc revert <hash-or-ref>`
- Why: semantically invert and apply a change.

### `arc restore`

- Syntax: `arc restore <filepath>`
- Why: restore file to snapped state of current view.

### `arc amend` (Advanced)

- Syntax: `arc amend [-m <message>]`
- Why: rewrite most recent snap while preserving operation continuity.

### `arc squash` (Advanced)

- Syntax: `arc squash --into <rev>`
- Why: collapse a linear range into one canonical change.

### `arc diffedit` (Advanced)

- Syntax:
  - `arc diffedit --prepare <rev> [-m <message>]`
  - `arc diffedit --apply [-m <message>]`
- Why: two-step external edit and apply flow for change rewriting.

## Views and History Control (Stable)

### `arc view`

- Syntax:
  - `arc view create <name>`
  - `arc view switch <name>`
  - `arc view merge <name>`
- Why: create/switch/merge named head sets.

### Aliases

- `arc checkout <name>` is an alias for `view switch`.
- `arc branch [name]` lists views when omitted, creates view when provided.

### `arc undo`

- Syntax: `arc undo`
- Why: rollback latest view-mutating operation using operation log.

### `arc op log`

- Syntax: `arc op log`
- Why: inspect operation history (not just change history).

## Stash and Tags (Stable)

### Stash

- `arc stash push`
- `arc stash pop`
- `arc stash list`

Why: temporary parking lot for dirty state.

### Tags

- `arc tag <name> <hash-or-ref>`
- `arc tags`

Why: immutable named references for release/signoff points.

## Identity and Configuration (Stable)

### Identity

- `arc auth login --name <name> --email <email>`
- `arc auth whoami`
- `arc identity --name <name> --email <email>`

Why: configure signing identity and operator metadata.

### Config

- `arc config [--global] alias <name> <expansion>`
- `arc config [--global] aliases`
- `arc config [--global] get <key>`
- `arc config [--global] set <key> <value>`
- `arc config [--global] unset <key>`
- `arc config [--global] list`

Why: define policy, remotes, aliases, and behavior knobs.

## AI Commands (Advanced)

### `arc ai resolve`

- Syntax: `arc ai resolve`
- Why: resolve pending semantic conflict through configured AI provider path.

### `arc ai approve`

- Syntax: `arc ai approve`
- Why: approve and sign pending AI output (ghost node flow).

### `arc ai generate`

- Syntax: `arc ai generate --goal <text> [--file <path>]`
- Why: generate targeted file changes with explicit operator goal.

Safety note:
Approval is explicit; generation is not final until approved.

## Import, Sync, and Networking

### Import and Push

- `arc import git <git_path>`
- `arc push <remote_url_or_alias> [view]`

Why:
Import from Git and push through interop boundaries when needed.

### Native Sync

- `arc sync <host:port>`
- `arc fetch <remote_path> <view>`
- `arc pull <remote_path> <view>`
- `arc serve [--port <port>]`

Why:
Use Arc-native remote exchange and local/native server workflows.

### Remote Aliases

- `arc remote add <name> <url-or-path>`
- `arc remote list`
- `arc remote remove <name>`

## Workspace and Monorepo Commands

### Sparse

- `arc sparse set <path>...`
- `arc sparse list`
- `arc sparse reset`

Why: bound working-set materialization for large trees.

### Mount

- `arc mount add --path <path> --url <url-or-path> --target <view>`
- `arc mount sync`

Why: sub-repository composition workflow.

### Workspace

- `arc workspace add <path> [--view <name>]`
- `arc workspace list`

Why: linked split-root workflows sharing repository data.

## Storage Maintenance

### `arc gc`

- Syntax: `arc gc [--dry-run]`
- Why: reclaim unreachable/stable storage.
- Flags:
  - Safety: `--dry-run` previews deletions.

### `arc compact` (Advanced)

- Syntax: `arc compact`
- Why: compact causally-stable history into a base state.

## Internal

- `arc daemon`

Status: Internal
Purpose: JSON-RPC daemon entrypoint for editor/tooling integrations.

## Compatibility Notice

- `arc commit` is intentionally unsupported (use `arc snap`).
