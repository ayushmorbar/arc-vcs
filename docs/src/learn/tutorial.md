---
title: Tutorial
description: Documentation page for Tutorial.
---

# Tutorial: Zero to First Snap

Status: Stable
Audience: New to Arc
Time: 5-10 minutes

## Prerequisites

- Arc installed (`cargo install --path crates/arc-cli`)
- A writable local directory

## Step 1: Set Identity (One-Time)

```sh
arc auth login --name "Your Name" --email "you@example.com"
```

Why this exists:
Every change is signed. Arc uses your configured identity to produce verifiable provenance.

## Step 2: Initialize Repository

```sh
mkdir my-project
cd my-project
arc init
```

Arc creates `.arc/` metadata and sets up the default view state.

## Step 3: Add Code

Shell note:
Examples below use a POSIX shell style heredoc. If you use PowerShell/cmd, create the file with your editor instead.

If you do not already have a Rust file, create one now:

```sh
mkdir src
cat > src/main.rs << 'EOF'
fn main() {
    println!("hello from arc");
}
EOF
```

## Step 4: Inspect Pending Work

```sh
arc status
```

Mental model:
`status` compares current working files against the materialized state of your current view and reports pending semantic changes.

## Step 5: Record First Change

```sh
arc snap -m "feat: initial hello world"
```

What happens:

1. Arc derives semantic atoms from your current files.
2. It creates a signed `Change` object.
3. The change is content-addressed and persisted.
4. Current view heads advance.

## Step 6: Read History

```sh
arc log
```

You should see your new change hash, author identity, and intent message.

## Step 7: Try Views and Merge

```sh
arc view create feature/experiment
arc view switch feature/experiment
# edit files and snap
arc checkout main
arc view merge feature/experiment
```

Mental model:
A view is a named head set over the graph, not a single branch pointer.

## Step 8: Push (Interop Boundary)

```sh
arc push https://github.com/<org>/<repo>.git
```

Arc uses the Git bridge at push/import boundaries while preserving Arc-native local semantics.

## Progressive Disclosure: Useful Next Commands

- `arc undo`: rollback last view-mutating operation.
- `arc diff`: inspect uncommitted working-tree differences.
- `arc verify`: verify graph provenance.

## Next Reads

- [Everyday Workflow](everyday.md)
- [CLI Reference](../reference/cli-reference.md)
- [Architecture Overview](../architecture/overview.md)
