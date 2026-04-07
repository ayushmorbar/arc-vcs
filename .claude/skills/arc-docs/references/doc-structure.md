# Authoritative Doc Structure

```
arc-vcs/
├── README.md
├── CHANGELOG.md
├── CONTRIBUTING.md
├── SECURITY.md
└── docs/
    ├── book.toml
    └── src/
        ├── SUMMARY.md
        ├── introduction.md
        ├── getting-started/
        │   ├── installation.md
        │   ├── quickstart.md
        │   └── migrating-from-git.md
        ├── concepts/
        │   ├── spacetime-dag.md
        │   ├── semantic-changes.md
        │   ├── cas-storage.md
        │   └── ai-integration.md
        ├── user-guide/
        │   ├── cli-reference.md
        │   ├── daily-workflows.md
        │   ├── collaboration.md
        │   └── advanced-automation.md
        ├── architecture/
        │   ├── overview.md
        │   ├── crate-map.md
        │   ├── axioms.md
        │   └── decisions/
        │       └── 0001-blake3-hashing.md
        └── contributing/
            ├── setup.md
            ├── coding-standards.md
            └── testing.md
```

## Ownership

Each directory has a primary audience:

| Directory | Primary tier | Can be skipped by |
|---|---|---|
| `getting-started/` | Newcomer | Contributor, Power User |
| `concepts/` | Newcomer → Developer | Enterprise |
| `user-guide/` | Developer, Power User | — |
| `architecture/` | Contributor | Newcomer |
| `contributing/` | Contributor | All others |
