---
title: Release Docs Checklist
description: Documentation page for Release Docs Checklist.
---

# Release Docs Checklist

Use this checklist before cutting a new release.

## Required

- [ ] Root docs are current: `README.md`, `CONTRIBUTING.md`, `DEVELOPMENT.md`.
- [ ] Crate READMEs reflect actual responsibilities and command/API behavior.
- [ ] `docs/src/SUMMARY.md` has no missing pages or stale links.
- [ ] CLI docs match current command tree from `crates/arc-cli/src/main.rs`.
- [ ] Architecture docs reflect current crate dependency boundaries.
- [ ] Research references in docs point to existing files under `research/`.
- [ ] `CHANGELOG.md` release notes include user-visible behavior changes.

## Validation Commands

```sh
mdbook build docs
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Final Pass

- [ ] Search for stale architecture wording:

```sh
# PowerShell
Select-String -Path README.md,CONTRIBUTING.md,DEVELOPMENT.md,docs/src/**/*.md -Pattern "four crates|4 crates|4-crate"
```

- [ ] Search for moved/duplicate CLI reference pages:

```sh
# PowerShell
Select-String -Path docs/src/**/*.md -Pattern "cli/reference.md"
```

- [ ] Compare documented command list with live CLI help output:

```sh
# PowerShell
arc --help
```

- [ ] Manually open top-level docs and verify links render correctly on GitHub and in mdBook.
