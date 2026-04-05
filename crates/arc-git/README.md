# arc-git

![crate](https://img.shields.io/badge/crate-arc--git-blue)
![role](https://img.shields.io/badge/role-git%20ingress-f6a)

## BLUF

`arc-git` is the legacy Git ingress edge for arc. It reads Git object graphs and exposes deterministic commit/tree/blob analysis for translation into arc-native history.

## Architectural Role (The DAG)

- Depends on: filesystem/zlib parsing support.
- Depended on by: `arc-cli` and Git interop paths.
- Position: compatibility ingress boundary outside pure DAG semantics.

## Purity & I/O Boundary

`arc-git` is an I/O Boundary.

- Reads `.git` refs and objects from disk.
- No network I/O in core analysis paths.
- Must not own arc rewrite or storage semantics.

## Key Types/Exports

- `GitCommit`, `GitAnalysis`
- `analyze_git_repo`, `resolve_git_dir`, `list_branch_heads`
- `extract_tree_to_memory`, `read_git_user_config`

```rust
let analysis = arc_git::analyze_git_repo(std::path::Path::new("."))?;
println!("{}", analysis.commit_count);
# Ok::<(), anyhow::Error>(())
```
