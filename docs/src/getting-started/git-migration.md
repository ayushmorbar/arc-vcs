# Migrating from Git

This page explicitly breaks the mental model you built over years of using Git. Read it carefully before reaching for familiar concepts.

---

## The Fundamental Difference

> **In Git, a branch is a pointer to a snapshot.**
> **In arc, a View is a named set of DAG heads representing a continuous stream of semantic intents.**

Let that sit for a moment.

In Git, `git checkout feature/x` moves `HEAD` to a commit object. That commit is a complete snapshot of the entire working tree at a point in time. A "merge" is Git computing the difference between two snapshots and reconciling three text files.

In arc, `arc switch feature/x` rewrites your working directory to the *materialisation* of a set of change heads. Those heads are not snapshots — they are the frontier of a DAG of algebraic `Change` objects. Each `Change` is a set of typed *Atoms* (AST insertions, deletions, semantic operations) that carry a `commutes()` relationship with every other `Change`.

---

## Concepts Side-by-Side

| Git concept | arc equivalent | Key difference |
|---|---|---|
| Commit | `Change` | A `Change` is a set of typed `Atom`s with a BLAKE3 hash and an Ed25519 signature. It is not a full snapshot. |
| Branch | `View` | A `View` is a `HashSet<Blake3Hash>` of head `Change` IDs, not a single pointer. It can have multiple heads. |
| `git merge` | `arc merge` | arc merge runs a formal commutativity check. If all delta pairs commute, the merge is mathematically correct with no human input. |
| `git rebase` | *(no direct equivalent yet)* | Reordering is algebraically sound but `arc reorder` is not yet shipped. See [SHORTCOMINGS.md](../../SHORTCOMINGS.md). |
| `git cherry-pick` | *(no direct equivalent yet)* | Ditto. |
| `.git/hooks/` | `hooks` in `.arc/config.json` | Declarative JSON, not hidden shell scripts. Version-controlled by default. |
| `git stash` | `arc undo` + re-snap | Work in progress is captured by snapping; undo pops it. |
| SHA-1 object ID | `Blake3Hash` | BLAKE3 is 256-bit and 3× faster than SHA-256. SHA-1 is cryptographically broken. |
| Signed commits | automatic | Every `Change` is *always* Ed25519 signed. There is no unsigned mode. |

---

## Merge Without Rebase Hell

In Git, `git rebase` exists because merge commits create a non-linear history that is hard to read, and because textual three-way merge frequently produces conflicts that are actually safe to combine. People rebase to linearise history and avoid spurious conflicts.

In arc, both problems are solved at the root:

1. **Non-linear history is fine** — the change graph is a DAG by design. `arc log` renders it clearly.
2. **Conflicts are semantic, not textual** — `commutes(a, b)` checks whether two AST-level changes touch the same structural node. If they don't, they commute perfectly. A conflict in arc means two changes genuinely competed for the same AST location — not that they happened to be near each other in the file.

The result: `arc merge` is almost always automatic. You will reach for `arc merge` the way you currently reach for `git pull`.

---

## Importing an Existing Repository

```sh
arc git-import /path/to/your/git/repo
```

`arc git-import` walks the Git commit graph via `git2`, re-hashes all objects with BLAKE3, re-signs changes with your arc identity, and reconstructs the arc change graph. Your entire history comes with you.

Check `.mailmap` to ensure historical email addresses map correctly to your canonical arc identity.

---

## Common Git Commands and Their arc Equivalents

```sh
# Git                          arc
git add -A && git commit       arc snap -m "message"
git commit --amend             arc undo && arc snap -m "corrected message"
git status                     arc status
git diff                       arc diff
git log --oneline              arc log
git checkout -b feature/x      arc view create feature/x && arc switch feature/x
git merge feature/x            arc merge feature/x
git branch -d feature/x        arc view delete feature/x
git remote add origin URL      arc remote add origin URL
git pull origin main           arc pull origin main
git push origin main           arc push origin main
git tag v1.0                   arc tag create v1.0
git stash                      arc snap -m "wip: stash" (then arc undo to pop)
git blame                      (use arc log with file filter — full blame not yet implemented)
```
