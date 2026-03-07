# CLI Reference

Complete reference for all `arc` commands. Run `arc <command> --help` for inline help.

---

## Global Options

| Flag | Description |
|---|---|
| `--version` | Print the arc version and exit |
| `--help` | Print help for a command |

Telemetry is controlled via environment variables, not flags. See [Configuration](config.md).

---

## `arc compact`

Compact causally-stable history into a single Genesis base state.

```sh
arc compact
```

Collapses the entire causally-stable DAG into one synthetic **Genesis Change** whose atoms represent the exact materialised state at the compaction boundary. An Epoch Map is written to `.arc/epochs` so future `hydrate` calls transparently redirect compacted IDs to the Genesis node — no live `Change` object is ever rewritten.

Prints: `Successfully compacted causally stable history into new base state: <64-char hex>`

Exits with an error if there is no causally-stable history (e.g. a single-view repository with only one commit, or a brand-new repo).

See [CRDT Sync — PO-Log Compaction & Epoch Maps](../design/crdt_sync.md) for the full technical specification.

---

## `arc init`

Initialise a new arc repository in the current directory.

```sh
arc init
```

Creates `.arc/` with `blobs/`, `views/`, and an initial `main` view. Does nothing if `.arc/` already exists (idempotent).

---

## `arc auth`

Manage your cryptographic identity.

```sh
arc auth login --name "Ada Lovelace" --email "ada@example.com"
```

Generates an Ed25519 keypair and stores it in the platform config directory. If a keypair already exists, it is loaded and the metadata is updated. Every `Change` you create is signed with this key.

---

## `arc snap`

Record a new change from the working-directory delta.

```sh
arc snap -m <message>
arc snap --message <message>
arc snap -i -m <message>          # interactive staging
arc snap --interactive -m <message>
```

**Options:**

| Flag | Description |
|---|---|
| `-m`, `--message <msg>` | Commit message (required) |
| `-i`, `--interactive` | Stage individual AST atoms interactively |

Returns the BLAKE3 hash of the new `Change`. Exits with no output if the working directory is clean (nothing to snap).

Fires the `pre-snap` hook before executing. See [Custom Hooks](../howto/custom-hooks.md).

---

## `arc log`

Display the change history for the current view.

```sh
arc log
```

Walks the DAG from the current view's heads in topological order. Displays change hash, author, timestamp, and message. Output is coloured via `owo-colors`.

---

## `arc status`

Show the working-directory delta against the current view's materialised state.

```sh
arc status
```

Prints added, modified, and deleted atoms. Respects `.arcignore` and sparse checkout patterns.

---

## `arc diff`

Show uncommitted working-directory changes.  Two complementary views are
available: the default **text diff** (Micro view) and the `--semantic` **AST
diff** (Macro view).  Use them together for a complete picture of every change.

```sh
arc diff              # text diff — verify execution
arc diff --semantic   # AST diff  — understand intent
```

### Default: Sesame-Aligned Text Diff

The plain `arc diff` view re-projects AST atoms back into text and applies
four layers of improvement over a raw `git diff`:

1. **Refactoring intent annotation** — [`Move`] and [`SemanticsPreserving`]
   atoms are printed as labelled `≈ [Move]` / `≈ [Refactor]` lines *before*
   the text hunks so reviewers grasp intent at a glance.

2. **Sesame syntactic alignment** — structural newlines are injected before
   `{`, after `}`, and after `;` so the line differ aligns brace pairs
   correctly instead of staggering them across logical blocks.

3. **Inline sub-expression highlighting** — only the exact sub-token that
   changed is highlighted with a coloured background; the surrounding
   unchanged text on the same line is shown in a plain foreground colour.

4. **Boilerplate collapse** — if every changed line is a `use` / `import` /
   `#include` declaration the entire hunk is replaced by a single summary
   line: `@@ [Boilerplate] Import / use declarations modified @@`.

Files exceeding 1 MB skip the inline LCS calculation and print
`∆ [Change] File too large for inline diff — AST atoms shown above.`

**Sample output (text diff)**

```
On view: main
diff --arc a/src/widget.rs b/src/widget.rs
  ≈ [Move] fn_render → fn_paint
- fn render() { let x = 1;
+ fn paint() { let x = 1;
  ∑ +1 -1 ~1
```

### `--semantic`: Structural AST Diff

Passes `--semantic` to render each pending atom as a named structural
operation (the "Macro" view).  Instead of line-level `+`/`-` noise, the output
describes architectural intent in plain English:

```sh
arc diff --semantic
```

Each atom is labelled by its type and the AST node it targets, using the
`<kind>_<name>` convention in arc NodePaths:

| Atom | Output |
|------|--------|
| `Insert { at: ["file", "lib.rs", "fn_parse"] }` | `[+] Insert function: 'fn_parse'` |
| `Delete { at: ["file", "lib.rs", "field_id"] }` | `[-] Delete field: 'field_id'` |
| `Move { from, to }` | `[~] Move 'fn_render' → fn_paint` |
| `SemanticsPreserving { description }` | `[≈] Refactor variable 'obj': renamed to 'item'` |

Cross-file moves show the destination filename in parentheses.  Multi-mappings
(three deletion sites → one extracted method) appear as separate `[-]` lines
linking to the same extracted target.

**Sample output (semantic diff)**

```
On view: main
semantic --arc src/engine.rs
  [+] Insert function: 'fn_validate'
  [-] Delete function: 'fn_check'
  [~] Move 'fn_render' → fn_paint
  [≈] Refactor variable 'obj': renamed to 'item'

  ∑ +1 -1 ~2
```

### Recommended Review Workflow

1. **Start with** `arc diff --semantic` — understand the architecture changes
   in seconds, without reading code.
2. **Then use** `arc diff` — verify the exact syntax and formatting of each
   change, with per-token highlighting so nothing slips through.

---

## `arc restore`

Revert one or more files to their last-snapped state.

```sh
arc restore <path> [<path>...]
arc restore src/widget.rs
```

---

## `arc undo`

Pop the last operation from the OpLog and reverse it.

```sh
arc undo
```

Creates a new reverse `Change` rather than rewriting history. Safe to run at any time.

---

## `arc view`

Manage views.

```sh
arc view create <name>            # create a new view forked from the current heads
arc view list                     # list all views
arc view delete <name>            # delete a view (does not delete changes from CAS)
```

---

## `arc switch`

Switch the working directory to a different view.

```sh
arc switch <view-name>
arc switch main
arc switch feature/my-work
```

Materialises the target view's state into `work_root`. Fails with an error if the working directory is dirty.

---

## `arc merge`

Merge another view into the current one.

```sh
arc merge <view-name>
arc merge feature/my-work
```

Runs a full commutativity check. On success, advances the current view's heads to the union. On conflict, writes `.arc/conflict` and reports the conflicting change pair IDs. See `arc resolve`.

Fires the `post-merge` hook on success.

---

## `arc resolve`

Resolve a pending semantic conflict using an AI resolver.

```sh
arc resolve
```

Reads `.arc/conflict`, invokes the configured `AiResolver`, and commits the resolved `Change`. Requires an AI API key to be configured. See [AI Intents & Resolution](ai-intents.md).

---

## `arc tag`

Manage signed tags.

```sh
arc tag create <name> [<change-hash>]   # tag a change (defaults to current head)
arc tag list
arc tag delete <name>
```

---

## `arc remote`

Manage remote aliases.

```sh
arc remote add <name> <url>
arc remote list
```

---

## `arc fetch`

Fetch objects and views from a remote.

```sh
arc fetch <remote-name>
arc fetch origin
```

---

## `arc pull`

Fetch and merge a remote view.

```sh
arc pull <remote> <view>
arc pull origin main
```

---

## `arc push`

Push local changes to a remote.

```sh
arc push <remote> <view>
arc push origin main
```

---

## `arc gc`

Run causal-stability garbage collection.

```sh
arc gc
```

Prints the number of retained and pruned changes.

---

## `arc config`

Read and write repository configuration.

```sh
arc config get <key>
arc config set <key> <value>
arc config alias <name> <expansion>
```

See [Configuration](config.md) for all supported keys.

---

## `arc workspace`

Manage split-root workspaces.

```sh
arc workspace add <path>           # register a new work root
arc workspace list                 # list all registered work roots
```

---

## `arc sparse`

Manage semantic sparse checkout patterns.

```sh
arc sparse set <pattern> [<pattern>...]     # replace all patterns
arc sparse add <pattern>                    # add a pattern
arc sparse remove <pattern>                 # remove a pattern
arc sparse list                             # show active patterns
```

Patterns are stored as `Atom::Mount` changes in the graph.

---

## `arc git-import`

Import a Git repository into arc.

```sh
arc git-import <path-to-git-repo>
```

Re-hashes all Git objects with BLAKE3, re-signs changes with your arc identity, and reconstructs the change graph. See [Migrating from Git](../getting-started/git-migration.md).
