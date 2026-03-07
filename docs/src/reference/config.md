# Configuration

arc configuration is stored as JSON and merged hierarchically: global settings are overlaid with per-repo settings.

---

## Configuration Files

| Scope | Location | Managed by |
|---|---|---|
| Global | `~/.config/arc/config.json` (Linux/macOS) | `arc config set` (no repo required) |
| Global | `%APPDATA%\arc\config.json` (Windows) | `arc config set` |
| Per-repo | `<repo>/.arc/config.json` | `arc config set` (inside a repo) |

**Merge precedence:** per-repo values override global values for the same key.

---

## Schema

```json
{
  "remotes": {
    "<name>": "<url-or-path>"
  },
  "aliases": {
    "<shortname>": "<expansion>"
  },
  "hooks": {
    "<event>": ["<command-string>", "..."]
  }
}
```

All top-level keys are optional. Missing keys are treated as empty maps.

---

## `remotes`

Named remote aliases mapping a short name to a URL or filesystem path.

```json
{
  "remotes": {
    "origin": "http://arc-server:8080",
    "backup": "/mnt/nas/repos/my-project"
  }
}
```

Used by `arc fetch <name>`, `arc pull <name> <view>`, `arc push <name> <view>`.

---

## `aliases`

User-defined command expansions. The alias name is intercepted before argument parsing and expanded via `shlex::split`.

```json
{
  "aliases": {
    "st": "status",
    "cm": "snap -m",
    "pub": "push origin main"
  }
}
```

Aliases are single-pass (no recursive expansion). Global aliases are always available; per-repo aliases shadow global ones.

---

## `hooks`

Lifecycle hooks triggered by arc operations. The value is an array of command strings. Each command is run sequentially; the operation is aborted on the first non-zero exit.

```json
{
  "hooks": {
    "pre-snap": [
      "./scripts/lint.sh --strict",
      "./scripts/test-fast.sh"
    ],
    "post-merge": [
      "./scripts/notify-team.sh"
    ]
  }
}
```

### Supported Events

| Event | Trigger |
|---|---|
| `pre-snap` | Before `arc snap` records a new `Change` (after dirty-tree computation, before CAS write) |
| `post-merge` | After `arc merge` successfully updates the view and writes the working directory |

### Hook Execution Model

- Commands are parsed with `shlex::split` — no shell expansion, no glob injection.
- The process working directory is `work_root`.
- The process inherits the current environment.
- A non-zero exit code aborts the operation with a descriptive error message.

> **Windows:** shell built-ins (`echo`, `dir`) are not PATH executables. Use `cmd /C echo ...` or a real binary.

---

## Reading and Writing Config

```sh
# Read a key
arc config get remotes
arc config get aliases

# Write a remote
arc remote add origin http://arc-server:8080

# Write an alias
arc config alias st status
arc config alias cm "snap -m"
```

---

## Global Config

To write to the global config from outside a repository, `arc config` falls back to the global file automatically. You can also edit the JSON file directly.

---

## Environment Variables

| Variable | Effect |
|---|---|
| `ARC_TRACE=1` | Enable compact tracing output to stderr |
| `ARC_TRACE_EVENT=<path>` | Append structured JSON trace events to `<path>` |

See [Telemetry](../../README.md#telemetry) for details.
