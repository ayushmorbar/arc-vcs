#!/usr/bin/env -S just --justfile
# ^ Allows running as `./justfile <recipe>` directly (chmod +x justfile)
# Install `just` with: cargo install just
# Run `just` or `just --list` to see available recipes.

# Self-reference: safe nested recipe calls even with non-PATH just installs
j := quote(just_executable())

set shell := ["bash", "-cu"]

# Show all available recipes
default:
    @{{ j }} --list

# ── Aliases ───────────────────────────────────────────────────────────────────
alias t  := test
alias c  := check
alias l  := lint
alias nt := nextest
alias v  := verify-fast

# ── Testing ───────────────────────────────────────────────────────────────────

# Run all workspace tests (nextest: faster, better output)
test:
    cargo nextest run --workspace --no-fail-fast

# Run tests with optional custom flags, e.g. `just nextest -p arc-cli`
nextest *FLAGS='--workspace':
    cargo nextest run {{ FLAGS }} --no-fail-fast

# Run doctests separately (nextest does NOT run doctests)
doc-tests:
    cargo test --workspace --doc --no-fail-fast

# Verify local tooling + governance + workspace policy (regex OR filter via nextest)
[private]
verify:
    cargo nextest run -p arc-cli --no-fail-fast \
        -E 'test(tooling_audit_current_workspace) | test(governance_audit_current_workspace) | test(workspace_policy_audit_current_workspace)'

# Public policy lane
verify-policy:
    @{{ j }} verify

# Run tests matching a filter string, e.g. `just test-filter my_fn`
test-filter FILTER:
    cargo nextest run --workspace --no-fail-fast -E "test({{ FILTER }})"

# Run nextest, showing only final summary (good for large workspaces)
summarize EXPRESSION='all()':
    cargo nextest run --workspace --run-ignored all --no-fail-fast \
        --status-level none --final-status-level none -E {{ quote(EXPRESSION) }}

# ── Linting & Formatting ──────────────────────────────────────────────────────

# Run clippy with zero-warning policy; pass extra args e.g. `just lint -W clippy::pedantic`
lint *clippy-args:
    cargo clippy --all-targets --all-features -- -D warnings {{ clippy-args }}

# Apply nightly rustfmt, then verify it doesn't break stable rustfmt
# Also lints the justfile itself via --fmt --unstable
fmt:
    cargo +nightly fmt --all
    cargo +stable fmt --all -- --check
    {{ j }} --fmt --unstable

# Check formatting only (used in CI)
fmt-check:
    cargo +nightly fmt --all -- --check

# ── Documentation ─────────────────────────────────────────────────────────────

# Build docs, treating all rustdoc warnings as errors
doc $RUSTDOCFLAGS='-D warnings':
    cargo doc --workspace --no-deps

# Serve documentation with live reload
docs-serve:
    mdbook serve --open docs

# Build the mdBook documentation (output: docs/book/)
docs:
    mdbook build docs

# Documentation verification lane (book + rustdoc)
verify-docs: docs doc

# Generate API stability map from architecture component graph
stability-map:
    bash scripts/docs/generate-stability-map.sh

# ── Build ─────────────────────────────────────────────────────────────────────

# Debug build
build:
    cargo build --workspace

# Release build
release:
    cargo build --workspace --release

# Remove all build artefacts
clean:
    cargo clean

# ── Security ──────────────────────────────────────────────────────────────────

# Audit deps: CVEs + license compliance + banned crates (requires: cargo install cargo-deny)
audit:
    cargo deny --workspace --all-features check advisories bans licenses sources

# Fallback: quick CVE-only audit (requires: cargo install cargo-audit)
audit-quick:
    cargo audit

# Security verification lane
verify-security: audit

# Fuzzing smoke check for arc-lang parser target
fuzz-check:
    cargo fuzz run fuzz_lang_parser --sanitizer none -- -runs=1024 -max_total_time=10

# Benchmark arc-core operations and emit report-friendly summary
bench-trend:
    mkdir -p target/bench
    cargo bench -p arc-core --bench core_ops | tee target/bench/core_ops.latest.txt
    printf "## Benchmark Snapshot\n\n- Command: cargo bench -p arc-core --bench core_ops\n- Raw output: target/bench/core_ops.latest.txt\n\n### Extracted timings\n" > target/bench/core_ops-summary.md
    grep -E "arc_core_cas/.+time:" target/bench/core_ops.latest.txt >> target/bench/core_ops-summary.md || true

# Fast contributor lane: most common local pre-push checks
verify-fast: fmt-check lint test doc-tests verify-docs

# Full lane: mirrors strict CI + policy + supply chain checks
verify-full: verify-fast verify-policy verify-security

# ── Full CI gate ──────────────────────────────────────────────────────────────

# Complete local CI check — mirrors CI pipeline exactly
ci: test doc-tests lint fmt-check doc verify
