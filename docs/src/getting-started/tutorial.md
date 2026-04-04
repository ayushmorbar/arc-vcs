# Tutorial: Zero to First Snap

This tutorial takes you from an empty directory to a working arc repository with a recorded change history in about five minutes. No prior experience with arc is required.

---

## Prerequisites

- `arc` installed (`cargo install --path crates/arc-cli`)
- A directory with at least one Rust `.rs` file (or create one below)

---

## Step 1 — Set Up Your Identity

arc signs every change with an Ed25519 keypair. Set it up once:

```sh
arc auth login --name "Your Name" --email "you@example.com"
```

Your keypair is stored in the platform config directory (`~/.config/arc/` on Linux/macOS, `%APPDATA%\arc\` on Windows). It is never transmitted without your permission.

---

## Step 2 — Initialise a Repository

```sh
mkdir my-project
cd my-project
arc init
```

arc creates a `.arc/` directory containing:
- `blobs/` — the content-addressable object store
- `views/` — named head sets (like branches, but algebraic)
- `HEAD` — the active view pointer

Configuration is created on first write (for example when you run `arc remote add` or `arc config set`).

---

## Step 3 — Write Some Code

```sh
mkdir src
cat > src/main.rs << 'EOF'
fn main() {
    println!("Hello from arc!");
}
EOF
```

---

## Step 4 — Check Status

```sh
arc status
```

arc compares the current working directory against the (empty) materialised state and shows every new AST atom detected.

---

## Step 5 — Record Your First Change

No explicit staging step is required. In arc, the working copy is implicitly tracked and auto-amended as you iterate, so there is no `arc add` command in the happy path.

```sh
arc snap -m "feat: initial hello world"
```

arc:
1. Parses `src/main.rs` with the Tree-sitter Rust plugin
2. Computes the AST delta as a set of `Atom::Insert` operations
3. Wraps them in a `Change` with your Ed25519 signature and BLAKE3 hash
4. Advances the `main` view's head pointer

---

## Step 6 — View History

```sh
arc log
```

You will see your change listed with its short hash, author, timestamp, and message.

---

## Step 7 — Make Another Change

Edit `src/main.rs`, then:

```sh
arc snap -m "feat: update greeting"
arc log
```

---

## Step 8 — Undo the Last View Mutation

If you want to instantly move your active view frontier back, use:

```sh
arc undo
```

`arc undo` is powered by the operation log and restores the prior head set in O(1) pointer-swap semantics.

---

## Step 9 — Explore Views

arc Views are not Git branches. A View is a **named set of DAG heads** — a continuous stream of semantic intents, not a pointer to a single snapshot:

```sh
arc view create feature/experiment
arc switch feature/experiment
# make changes, snap…
arc switch main
arc merge feature/experiment
```

Because all changes are algebraically checked for commutativity before the merge, there is no "rebase hell". See [Migrating from Git](git-migration.md) for the full conceptual explainer.

---

## Step 10 — Push to a Git Remote

arc can push directly to Git-hosting Smart HTTP endpoints through the on-the-wire translation bridge:

```sh
arc push https://github.com/<org>/<repo>.git
```

You can also push a specific view:

```sh
arc push https://github.com/<org>/<repo>.git feature/experiment
```

---

## What's Next?

- [Everyday Workflow](everyday.md) — the commands you'll use every day
- [CLI Reference](../reference/cli-reference.md) — full synopsis and options for all commands
- [Patch Theory](../design/patch_theory.md) — the mathematics behind arc's conflict detection
