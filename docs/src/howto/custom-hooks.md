# Custom Hooks

This guide walks you through configuring and debugging lifecycle hooks in arc. By the end you will have a working pre-snap linter that blocks commits with compiler errors.

---

## Overview

arc hooks are external commands that run at specific points in the lifecycle. They are configured in `.arc/config.json` under the `hooks` key — not in hidden per-repo shell scripts.

**Supported events:**

| Event | When it fires |
|---|---|
| `pre-snap` | After the working-directory delta is computed, before the `Change` is written to the CAS |
| `post-merge` | After `merge_heads()` succeeds and the working directory is updated |

A non-zero exit code from any hook aborts the operation immediately. The `Change` is not recorded (for `pre-snap`) or the view is not updated (for `post-merge`).

---

## Step 1 — Write a Hook Script

Create `scripts/pre-snap-check.sh` in your repository:

```sh
#!/usr/bin/env bash
set -euo pipefail

echo "Running pre-snap checks..."

# Check that the project compiles
cargo check --quiet 2>&1
echo "Compilation check passed."
```

Make it executable:

```sh
chmod +x scripts/pre-snap-check.sh
```

---

## Step 2 — Register the Hook

Edit `.arc/config.json` (or use `arc config`):

```json
{
  "hooks": {
    "pre-snap": ["./scripts/pre-snap-check.sh"]
  }
}
```

Or register via the CLI (once `arc config set hooks` is implemented — until then, edit the JSON directly).

---

## Step 3 — Test It

```sh
arc snap -m "test: verify hook fires"
```

You should see `Running pre-snap checks...` before the snap succeeds. Introduce a syntax error, run `arc snap` again, and confirm the hook blocks the commit.

---

## Step 4 — Debug with ARC_TRACE

If a hook is misbehaving or you want to see exactly when it fires:

```sh
ARC_TRACE=1 arc snap -m "debug hook timing"
```

The tracing output will show `snap started` before the hook fires and `snap complete` only if it passes.

For a persistent trace log:

```sh
ARC_TRACE_EVENT=/tmp/arc-trace.jsonl arc snap -m "logged run"
cat /tmp/arc-trace.jsonl | jq 'select(.fields.message != null) | .fields.message'
```

---

## Multiple Commands

You can register multiple commands for one event. They run sequentially:

```json
{
  "hooks": {
    "pre-snap": [
      "./scripts/lint.sh",
      "./scripts/test-fast.sh",
      "./scripts/check-secrets.sh"
    ]
  }
}
```

If any command exits non-zero, subsequent commands are not run.

---

## Quoted Arguments

Commands are parsed with `shlex`, so quoted arguments work as expected:

```json
{
  "hooks": {
    "pre-snap": ["cargo clippy -- -D warnings"]
  }
}
```

---

## Windows: Shell Built-ins

Shell built-ins like `echo` are not standalone executables on Windows. This will **fail**:

```json
{ "hooks": { "pre-snap": ["echo pre-snap fired"] } }
```

Use `cmd /C` instead:

```json
{ "hooks": { "pre-snap": ["cmd /C echo pre-snap fired"] } }
```

Or use a real binary (e.g., a Rust binary you build, or PowerShell):

```json
{ "hooks": { "pre-snap": ["powershell -Command Write-Host 'pre-snap fired'"] } }
```

---

## Disabling a Hook Temporarily

Remove the hook entry from `config.json`, or return exit code 0 from a wrapper script that checks an environment variable:

```sh
#!/usr/bin/env bash
if [[ "${ARC_SKIP_HOOKS:-}" == "1" ]]; then
    echo "Hooks disabled via ARC_SKIP_HOOKS=1"
    exit 0
fi
cargo check --quiet
```

Then temporarily: `ARC_SKIP_HOOKS=1 arc snap -m "skip hooks this once"`.

---

## Error Messages

When a hook fails to launch (binary not found), arc reports:

```
Hook 'pre-snap' failed to launch 'missing-binary': No such file or directory.
Ensure the command is an executable in your PATH (shell built-ins like 'echo'
are not PATH executables on Windows — use 'cmd /C echo ...' instead).
```

When a hook exits non-zero:

```
hook 'pre-snap' exited with exit status: 1 — operation aborted.
```
