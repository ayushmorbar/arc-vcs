---
title: "Contributor Onboarding"
description: "Hands-on path for new contributors to make a first safe change in arc."
category: "Guides"
audience: "Contributors"
---

# Contributor Onboarding

Bottom line up front: use this page to land your first high-quality contribution without breaking repository invariants.

## 1. Install Tooling

Run:

```bash
rustup component add clippy rustfmt
cargo install just cargo-nextest
```

Optional but recommended:

```bash
cargo install mdbook cargo-deny cargo-audit
```

## 2. Build and Verify Locally

Run the fast lane before opening a PR:

```bash
just verify-fast
```

If you touched policy, security, or release-critical surfaces, run the full lane:

```bash
just verify-full
```

## 3. Pick the Right Crate Boundary

Use this rule before coding:

- Pure crates (algebra, graph, patch logic) must stay deterministic and side-effect free.
- Boundary crates (CLI, daemon, storage, transport) can do filesystem or network I/O.

If your change would move logic across this boundary, stop and add design rationale first.

## 4. Implement the Smallest Auditable Change

Keep PRs narrow and explicit:

- One logical behavior change per PR.
- Add or update tests with the change.
- Prefer explicit invariants over clever shortcuts.

## 5. Sync Documentation in the Same Change

If behavior changes, update docs in the same PR:

- CLI/flags: `docs/src/reference/cli-reference.md`
- Architecture behavior: `docs/src/concepts/` or `docs/src/reference/architecture/`
- New docs page: also update `docs/src/SUMMARY.md`

## 6. Open the PR

Pre-PR checklist:

- `cargo test --workspace`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo fmt --all -- --check`
- `mdbook build docs`

Then open a focused PR with:

- Clear user impact
- Any invariants touched
- Migration notes if behavior is breaking

## First Contribution Ideas

Good starter contributions:

- Clarify a confusing docs page and keep `SUMMARY.md` in sync.
- Add a focused unit test for an invariant that currently lacks coverage.
- Improve CLI help text where a command is hard to discover.
