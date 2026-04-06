# Quickstart CLI Tutorial

## Goal
Create a repository, record semantic history, and inspect it with predictable output.

## Steps
1. Initialize a repository:

```bash
arc init
```

2. Snapshot current workspace:

```bash
arc snap -m "Initial semantic snapshot"
```

3. Inspect history:

```bash
arc log
```

4. Review workspace status:

```bash
arc status
```

## Why This Pattern
This sequence keeps repository bootstrap and first snapshot explicit, making subsequent automation easier to reason about.
