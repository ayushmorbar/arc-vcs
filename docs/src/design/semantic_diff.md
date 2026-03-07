# Semantic Diff Engine

This document explains why `arc diff` looks different from `git diff`, how the
two complementary diff views work, and how the output is produced.  The
implementation lives in `crates/arc-cli/src/semantic_diff.rs`.

---

## Dual-Diff Philosophy

`arc` exposes **two diff views** that serve different, complementary purposes
in a developer workflow.  Neither view alone is sufficient; the pair forms a
complete review tool.

| | `arc diff` (Text \u2014 Micro view) | `arc diff --semantic` (AST \u2014 Macro view) |
|---|---|---|
| **Shows** | Exact character-level edits | Structural intent operations |
| **Best for** | Verifying syntax, formatting, typos | Understanding architecture changes |
| **Unit** | Text lines / sub-tokens | AST atoms (Insert, Delete, Move, \u2248) |
| **Multi-mapping** | Cannot express | Shows all sites explicitly |
| **Boilerplate** | Collapsed automatically | Not applicable |

**Recommended workflow:**
1. **`arc diff --semantic`** \u2014 grasp the architectural intent in seconds.
2. **`arc diff`** \u2014 verify the exact syntax and formatting of each change.

The cognitive load reduction from this separation is the primary reason `arc`
provides a significantly better review experience than Git, `gitoxide`, or `jj`.
See Tsantalis et al. (2018) for empirical evidence that *shorter* edit scripts
do not always mean *less cognitive load* \u2014 intent labelling is essential.

---

## Motivation

Traditional VCS tools have one way to compare files: split each version into a
sequence of text lines and compute a Longest Common Subsequence (Myers
algorithm, 1986).  This approach has two deep problems:

1. **Structural blindness** — a single "Extract Method" refactoring that moves
   twenty lines from one function to a new one looks like twenty deletions
   followed by twenty additions.  There is no way to tell a reviewer "this code
   moved; it was not rewritten."

2. **Alignment instability** — when a developer wraps a block in a new `if`
   branch (adding one `{` and one `}`), Myers re-aligns the entire surrounding
   method body.  The result is a diff that highlights every indented line as
   changed, hiding the single character that actually changed.

`arc` solves problem 1 at the mathematical layer (AST Atoms).  The semantic
diff engine solves problem 2 at the presentation layer, using three techniques.

---

## Technique 1 — RefactoringMiner Intent Annotation

`arc`'s algebraic engine records [`Atom::Move`] and [`Atom::SemanticsPreserving`]
atoms when a refactoring is detected.  Before printing any text diff, the
semantic diff engine surfaces these atoms as labelled intent lines:

```
≈ [Move] fn_render → fn_paint
≈ [Refactor] fn_process (Extract Method)
```

This is directly inspired by the RefactoringMiner project (Tsantalis et al.,
2018): categorise the *intent* of a change first, then show the text evidence.
A code reviewer can immediately determine "this is a rename, I only need to
verify the call sites" without wading through walls of red and green text.

---

## Technique 2 — Sesame Syntactic Alignment

Before passing the two versions of a file to the `similar` line differ, the
engine applies a lightweight preprocessing step inspired by the **Sesame**
algorithm (2022):

```
" {"  →  "\n{"      (opening brace on its own line)
"} "  →  "}\n"      (closing brace on its own line)
"; "  →  ";\n"      (statement separator on its own line)
```

This forces the diff algorithm to treat every syntactic boundary as a separate
anchor point.  When a developer wraps a block in a new `if` branch:

| Without Sesame | With Sesame |
|---|---|
| Entire method body highlighted | Only the new `if (condition) {` line highlighted |

**Known limitation:** the heuristic operates on raw text and will also split
occurrences inside string literals (e.g. `let s = " {";`).  A future
enhancement will use tree-sitter byte-range information to restrict the
substitution to non-literal regions.

---

## Technique 3 — BDiff-Inspired Inline Sub-Expression Highlighting

The `similar` crate's `iter_inline_changes` function decomposes each changed
line into unchanged and changed sub-tokens.  The semantic diff engine renders:

- **Changed sub-tokens** — coloured background (reversed terminal cell): exact
  changed word is visually isolated.
- **Unchanged sub-tokens on a changed line** — plain foreground colour: the
  context remains readable without competing with the highlighted token.
- **Unchanged lines** — no colour.

This replicates the insight from the Kuhn–Munkres (Hungarian) optimal-matching
algorithm used in IntelliJ IDEA's "word diff" mode, without requiring a full
bipartite graph solver: the LCS computation inside `similar` produces the same
sub-token boundaries at a fraction of the cost.

**Example** (variable rename `old_name` → `new_name`):

```
- fn process(old_name: &str) {      ← "old_name" has red background
+ fn process(new_name: &str) {      ← "new_name" has green background
    // rest of function unchanged
```

Only the three characters `old` / `new` are highlighted; the reviewer sees the
change immediately without scanning the full line.

---

## Technique 4 — Semantic Import / Boilerplate Collapsing

When every line in a changed hunk is a `use`/`import`/`#include` declaration
(or a blank spacer line within the import block), the entire hunk is collapsed
to a one-line summary:

```
@@ [Boilerplate] Import / use declarations modified @@
  ∑ +3 -1
```

This prevents import reordering or dependency updates from burying logic
changes in noise — one of the most common reviewer complaints about standard
text diffs.

---

## Architecture

```
arc diff [--semantic]
  └─ main.rs: Command::Diff { semantic }
       └─ repo.diff_info() ──────────────── single materialization pass
            │  returns (Vec<Atom>, HashMap<filepath, old_text>)
            └─ ┌─────────────────────────────────────────────────────────┐
               │  if semantic                                             │
               │    semantic_diff::group_and_render_semantic()           │
               │      ├─ groups atoms by filepath (BTreeMap)             │
               │      └─ render_semantic_file() ── per-file pipeline     │
               │           ├─ "semantic --arc <path>" header             │
               │           ├─ [+] Insert <kind>: '<name>'               │
               │           ├─ [-] Delete <kind>: '<name>'               │
               │           ├─ [~] Move '<from>' → <to>                  │
               │           └─ [≈] Refactor <kind> '<name>': <desc>      │
               │                                                         │
               │  else (default text diff)                               │
               │    semantic_diff::group_and_render()                    │
               │      ├─ groups atoms by filepath (BTreeMap)             │
               │      ├─ reads new text from disk                        │
               │      └─ render_diff() ── per-file pipeline              │
               │           1. header                                     │
               │           2. intent annotation (≈ [Move] / ≈ [Refactor])│
               │           3. boilerplate collapse                       │
               │           4. mega-file guard (> 1 MB → skip LCS)       │
               │           5. sesame_align() on both blobs               │
               │           6. similar::grouped_ops(3) + iter_inline_changes│
               │           7. summary footer (∑ +N -N ~N)               │
               └─────────────────────────────────────────────────────────┘
```

Both `render_diff` and `render_semantic_file` are **pure functions** — they
take only `&str` and `&[&Atom]` and never touch the filesystem.  This makes
them trivially unit-testable without mocking the repository.

---

## Performance

| Scenario | Behaviour |
|---|---|
| Clean working directory | Returns immediately after `status()` check |
| Deleted file | `work_root.join(path)` → `NotFound` → `""` (no crash) |
| Import-only changes | Boilerplate collapsed, no LCS run |
| File > 1 MB (combined old+new) | LCS skipped, atom intent shown |
| Normal file | Sesame + `similar` inline LCS |

---

## References

- Tsantalis, N. et al. (2018). *RefactoringMiner 2.0*. IEEE TSE.
- Frick, V. et al. (2022). *Sesame*. ICSME 2022.
- Nugroho, Y.S. et al. (2019). *BDiff*. IEICE 2019.
- Myers, E. (1986). *An O(ND) Difference Algorithm*. Algorithmica.
