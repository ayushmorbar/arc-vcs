# arc-cli

Command-line interface and repository orchestration for the **arc** version-control system.

## Commands

```
arc init [path]             Initialize a new repository
arc snap -m <message>       Snapshot working directory as a semantic change
arc log                     Show the change history
arc status                  Show uncommitted AST-level changes
arc cherry-pick <hash>      Port a change into the current view
arc blame <file>            Semantic blame: author per AST node
arc stash push/pop/list     Stash dirty changes
arc view create/switch/merge Manage views (like branches)
arc ai resolve              AI-powered conflict resolution
arc import git <path>       Import history from a Git repo
arc fetch <remote> <view>   Fetch changes from a remote
arc pull <remote> <view>    Fetch + merge from a remote
arc verify                  Verify cryptographic provenance of all changes
arc auth login/whoami       Manage Ed25519 identity
arc serve [--port]          Start HTTP server
```

## Crate layout

```
arc-cli
├── lib.rs           – crate root (arc_cli library target)
├── main.rs          – binary entry point (arc binary target)
├── repo.rs          – Repository struct: the main orchestrator
├── sync.rs          – fetch/pull from remote peers
└── interop/
    └── git.rs       – Import Git history → arc changes
```
