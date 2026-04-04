# Contributing to arc

Thank you for your interest in contributing to arc — a mathematically rigorous version control system built in Rust. This document covers workspace architecture, coding conventions, the AI-authorship signature protocol, and the PR submission process.

---

## Workspace Architecture

arc is a Cargo workspace with six crates arranged in a strict dependency hierarchy:

```
arc-core  ◄──  arc-lang
    ▲   ▲          ▲
    │   └──────┐   │
    │          │   │
arc-net   arc-git-bridge
    ▲          ▲
    └──────┬───┘
             │
         arc-cli
             ▲
        arc-daemon
```

| Crate | Dependencies | Responsibility |
|-------|-------------|----------------|
| `arc-core` | — | Core algebra and storage: `Atom`, `Change`, `ChangeGraph`, `ObjectStore`, `View`, `Author`, revset engine |
| `arc-lang` | `arc-core` | Language plug-ins and AST projection (`LanguagePlugin`, `RustPlugin`) |
| `arc-net` | `arc-core` | Network services: HTTP CAS/view API, sync protocol, AI provider integration |
| `arc-git-bridge` | `arc-core` | Just-in-time Git object translation and Smart HTTP push bridge |
| `arc-cli` | `arc-core`, `arc-lang`, `arc-net`, `arc-git-bridge` | User-facing command orchestration and repository workflows |
| `arc-daemon` | `arc-core`, `arc-cli` | JSON-RPC daemon backend for editors and IDE integrations |

**Rules:**

- `arc-core` must never depend on higher-layer crates.
- `arc-daemon` must stay orchestration-only and must not duplicate `arc-cli` repository logic.
- `arc-git-bridge` stays at the transport boundary and must not become a second repository engine.

---

## Development Prerequisites

- **Rust 1.85+** (edition 2024) — install via [rustup](https://rustup.rs/)
- `cargo clippy`, `cargo fmt` — `rustup component add clippy rustfmt`
- **`just`** (optional, recommended) — `cargo install just`
- **`mdbook`** (optional) — `cargo install mdbook`

See [DEVELOPMENT.md](DEVELOPMENT.md) for a full environment setup walkthrough.

---

## Building & Testing

```sh
cargo build --workspace
cargo test --workspace          # 0 failures required
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check

# Or via justfile:
just test
just lint
just fmt
just docs          # builds the mdBook
just docs-serve    # live-reload server at http://localhost:3000
```

---

## Zero-Warning Policy

`cargo clippy --all-targets -- -D warnings` is enforced in CI. Do not submit PRs with outstanding warnings. If a lint is a false positive, add `#[allow()]` with a comment explaining why.

---

## Conventional Commits

All commit messages must follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add post-snap hook support
fix: prevent trace file truncation on re-open
docs: update README telemetry section
refactor(gc): extract causal-stability predicate
test(hooks): add pre-snap hook failure coverage
```

---

## AI-Authorship Signature Protocol

arc is partially developed with AI agents. Any commit that includes AI-generated code **must** include a `Co-Authored-By` trailer to maintain SLSA L4 provenance:

```
feat: implement sparse checkout materialisation

Co-Authored-By: GitHub Copilot <copilot@github.com>
Co-Authored-By: arc-agent <arc-agent@arc-vcs>
```

This is **not optional**. AI-assisted commits without the trailer will be rejected in code review. The `Change.author` field carries the human reviewer's ed25519 identity; AI contribution is tracked via the Git trailer only.

---

## Stability Tiers

Before changing a public API, check [STABILITY.md](STABILITY.md):

- **Tier 1 (Production-stable):** Semver-breaking changes require an RFC and ADR.
- **Tier 2 (Stable with caveats):** Changes require a rationale in the PR description.
- **Tier 3 (Experimental):** Welcome, but document the impact on existing config files.
- **Tier 4 (Unstable):** Internal — change freely, update related tests.

---

## Architecture Decision Records (ADRs)

Any change that alters a fundamental design decision — storage format, diff algorithm, sync protocol — **requires an ADR** filed under `docs/src/architecture/ADRs/`. Use the existing ADRs as templates and reference the ADR number in your PR description.

---

## Submitting a Pull Request

1. Fork and create a branch from `main`.
2. Make focused, minimal changes — one logical change per PR.
3. `cargo test --workspace` and `cargo clippy --all` must both pass.
4. Update `docs/` if the change affects user-visible behaviour.
5. Open a PR using the [PR template](.github/PULL_REQUEST_TEMPLATE.md).

---

## License

By contributing you agree that your contributions will be dual-licensed under `MIT OR Apache-2.0`, matching the project license.
