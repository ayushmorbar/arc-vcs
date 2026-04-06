# Multicall Dispatch Tutorial

## Goal
Use one binary with multiple invocation identities, similar to busybox-style dispatch.

## How It Works
`arc` inspects its executable stem before argument parsing. Known stems can inject a command prefix.

## Example
1. Create a symlink or copy:

```bash
ln -s arc arc-sync
```

2. Invoke through the alias stem:

```bash
./arc-sync --help
```

This is normalized internally to:

```bash
arc sync --help
```

## Why This Pattern
It allows dedicated operational entrypoints without duplicating binaries or command implementations.
