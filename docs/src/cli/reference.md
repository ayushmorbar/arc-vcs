# CLI Reference

Full reference for the `arc` binary.

## Global flags

| Flag | Description |
|------|-------------|
| `--help` | Print help |
| `--version` | Print version |

---

## `arc init [path]`

Initialize a new arc repository. Creates a `.arc/` directory with an empty CAS store and a default `main` view.

---

## `arc auth login --name <name> --email <email>`

Generate and persist a new Ed25519 identity to `~/.arc/identity`.

## `arc auth whoami`

Print the currently active name, email, and public key.

---

## `arc snap -m <message> [-i]`

Snapshot the working directory. Parses all tracked source files, diffs their ASTs against the current view, bundles the atoms into a signed `Change`, and stores it in the CAS.

`-i / --interactive` — choose which atoms to include atom-by-atom.

---

## `arc log`

Print all changes in the current view, newest first.

---

## `arc status`

Print uncommitted AST-level atoms against the current view without creating a change.

---

## `arc cherry-pick <hash>`

Port a change identified by its 64-character Blake3 hex hash into the current view.

---

## `arc blame <file>`

Print each interesting AST node in `<file>` alongside the change and author that last modified it.

---

## `arc stash push | pop | list`

`push` — save dirty changes into a hidden stash view and reset to the clean state.  
`pop` — apply the most recent stash and drop it.  
`list` — list all stashes.

---

## `arc view create <name>`

Fork a new view from the current state.

## `arc view switch <name>`

Check out a different view (updates the working directory).

## `arc view merge <name>`

Merge another view into the current view using the commutative patch algebra.

---

## `arc ai resolve`

Invoke the AI resolver to resolve a pending semantic conflict.

---

## `arc import git <path>`

Import the full commit history of the Git repository at `<path>` as arc changes.

---

## `arc fetch <remote_path> <view>`

Copy missing `Change` objects from a remote repository path.

## `arc pull <remote_path> <view>`

Fetch remote changes and merge the named view into the current view.

---

## `arc verify`

Rehydrate the current view and verify every change's Ed25519 signature and Blake3 hash.

---

## `arc serve [--port <port>]`

Start an HTTP server (default port 8080) exposing the repository for remote `fetch`/`pull` peers.
