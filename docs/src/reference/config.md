---
title: Config
description: Documentation page for Config.
---

# Configuration

## BLUF

Arc configuration is TOML-based, layered, and typed:

1. Synthesized defaults (platform-aware).
2. Global `config.toml`.
3. Repository-local `.arc/config.toml`.

Local values override global values, and map-like sections merge by key.

---

## File Locations

| Scope                | Path                                                                    |
| -------------------- | ----------------------------------------------------------------------- |
| Global (Linux/macOS) | `~/.config/arc/config.toml`                                             |
| Global (Windows)     | platform-resolved app config directory (use `arc config --global path`) |
| Repository local     | `<repo>/.arc/config.toml`                                               |

Check the active target path with:

```bash
arc config path
arc config --global path
```

Edit in your configured editor with:

```bash
arc config edit
```

---

## Legacy Migration

Arc automatically migrates legacy JSON config on first load:

- If `config.toml` is missing and `config.json` exists in the same location,
- arc reads JSON,
- writes equivalent TOML,
- removes old JSON,
- prints a one-time migration notice.

---

## Layering and Merge Semantics

Arc loads configuration in this order:

1. Synthesized defaults.
2. Global config.
3. Local config.

Rules:

- Scalar/object fields (for example `ui.color`, `merge.tool`) are overridden by higher-precedence layer when present.
- Map sections (for example `aliases`, `remotes`, `hooks`, `revsets`, `templates`, `template-aliases`, `colors`, `merge-tools`) are merged key-by-key with local entries overriding global entries with the same key.

---

## Top-Level Schema

```toml
[user]
name = "Jane Dev"
email = "jane@example.com"

[merge]
tool = "vscode"

[ui]
color = "auto"
pager = "less -FRX"
editor = "nano"
graph_style = "curved"
diff_formatter = ":color-words"
conflict_marker_style = "diff"
progress_indicator = true
greet = "Welcome to arc"

[ui.movement]
edit = false

[ai]
provider = "openai-compatible"
model = "gpt-4o-mini"
endpoint = "http://localhost:11434/v1"

[snapshot]
max_new_file_size = "1MiB"
auto_track = "all()"
auto_update_stale = false

[hints]
resolving_conflicts = true

[remotes]
origin = "http://arc-server:8080"
backup = "D:/repos/backup"

[aliases]
st = "status"
ci = "commit"

[hooks]
pre-snap = ["cargo test -q"]
post-merge = ["cargo check -q"]

[revsets]
log = "present(@) | ancestors(immutable_heads().., 2) | trunk()"

[templates]
log = "builtin_log_compact"
op_log = "builtin_op_log_compact"

[template-aliases]
my_short = "format_commit_summary_with_refs(self, format_commit_ref_names(bookmarks))"

[colors]
error = "bold"
warning = "yellow bold"

[merge-tools.vscode]
program = "code.cmd"
merge_args = ["--wait", "--merge", "$left", "$right", "$base", "$output"]
diff_args = ["--diff", "$left", "$right", "--wait"]

[merge-tools.meld]
program = "meld"
merge_args = ["$left", "$base", "$right", "-o", "$output", "--auto-merge"]
edit_args = ["$left", "$right"]
```

All sections are optional. Missing keys fall back to defaults.

---

## CLI-Managed vs File-Managed Keys

### CLI-Friendly Typed Keys

These are directly supported by `arc config get/set/unset`:

- `user.name`
- `user.email`
- `ui.color`
- `ui.pager`
- `ui.editor`
- `ui.graph_style`
- `ui.diff_formatter`
- `ui.conflict_marker_style`
- `ui.progress_indicator`
- `ui.greet`
- `ui.movement.edit`
- `merge.tool`
- `ai.provider`
- `ai.model`
- `ai.endpoint`
- `hints.resolving_conflicts`
- `snapshot.max_new_file_size`
- `snapshot.auto_track`
- `snapshot.auto_update_stale`
- `remotes.<name>`
- `aliases.<name>`
- `revsets.<name>`
- `templates.<name>`
- `template-aliases.<name>`
- `colors.<name>`

Examples:

```bash
arc config set ui.color always
arc config set user.name "Jane Dev"
arc config set ui.greet "Welcome to arc"
arc config set snapshot.auto_update_stale false
arc config set remotes.origin http://arc-server:8080
arc config unset aliases.st
arc config get ai.model
```

### Advanced Keys (Edit TOML)

These are part of the schema but typically managed by editing TOML directly:

- `merge-tools`
- `hooks`

You can still read/write many nested map values through `arc config set`, but `merge-tools` and `hooks` are generally easier to maintain directly in TOML.

---

## Hooks

Hooks map event name to command list:

```toml
[hooks]
pre-snap = ["cargo fmt --check", "cargo test -q"]
post-merge = ["cargo check -q"]
```

Supported events:

- `pre-snap`
- `post-merge`

Execution model:

- Commands run in order.
- First non-zero exit aborts the operation.
- Commands are argument-split safely (no implicit shell expansion).

Windows note:

- Shell built-ins may require explicit `cmd /C ...`.

---

## Templates and Template Aliases

`templates` selects named renderers for outputs such as log rows.

`template-aliases` provides reusable template expressions.

Example:

```toml
[templates]
log = "builtin_log_compact"
show = "builtin_log_detailed"
op_log = "builtin_op_log_compact"

[template-aliases]
short_refs = "format_commit_ref_names(bookmarks)"
```

---

## Merge Tools

`merge.tool` chooses which entry in `[merge-tools.<name>]` is active.

Each merge tool can define:

- `program`
- `merge_args`
- `edit_args`
- `diff_args`

Placeholders like `$left`, `$right`, `$base`, `$output` are substituted by arc for tool invocation.

---

## UI and Greeting

`[ui]` controls terminal behavior and rendering defaults.

Notable keys:

- `ui.color`
- `ui.pager`
- `ui.editor`
- `ui.graph_style`
- `ui.diff_formatter`
- `ui.conflict_marker_style`
- `ui.progress_indicator`
- `ui.greet`
- `ui.movement.edit`

`ui.greet` allows a custom welcome message for local operator UX.

---

## Remote Aliases and Command Aliases

### Remotes

```toml
[remotes]
origin = "http://arc-server:8080"
```

Used by fetch/pull/push workflows.

### Aliases

```toml
[aliases]
st = "status"
co = "checkout"
```

Aliases are expanded before command parsing.

---

## Inspect Effective Configuration

```bash
arc config list
```

This prints merged effective values after layering.

For audits and policy-sensitive workflows, pair config inspection with:

```bash
arc verify --workspace-policy
```

---

## Related

- [CLI Reference](cli-reference.md)
- [Debugging and Hyper-Observability](debugging.md)
