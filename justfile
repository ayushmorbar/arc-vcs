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
verify-policy-inner:
    cargo nextest run -p arc-cli --no-fail-fast \
        -E 'test(tooling_audit_current_workspace) | test(governance_audit_current_workspace) | test(workspace_policy_audit_current_workspace)'

# Public policy lane
verify-policy:
    @{{ j }} verify-policy-inner

# Run tests matching a filter string, e.g. `just test-filter my_fn`
test-filter FILTER:
    cargo nextest run --workspace --no-fail-fast -E "test({{ FILTER }})"

# Run nextest, showing only final summary (good for large workspaces)
summarize EXPRESSION='all()':
    cargo nextest run --workspace --run-ignored all --no-fail-fast \
        --status-level none --final-status-level none -E {{ quote(EXPRESSION) }}

# Coverage report via cargo-tarpaulin (requires: cargo install cargo-tarpaulin)
# Fails if total line coverage < 80%
coverage:
    cargo tarpaulin --all-features --skip-clean --timeout 300 --fail-under 80 --out stdout --skip-build

# Run criterion benchmarks (smoke: compile + execute, no regression gate)
bench:
    cargo bench -p arc-core --bench core_ops

# Save criterion baseline for regression comparison
bench-save BASELINE='main':
    cargo bench -p arc-core --bench core_ops -- --save-baseline {{ BASELINE }}

# Compare current benchmarks against saved baseline (fails on regression)
bench-compare BASELINE='main':
    cargo bench -p arc-core --bench core_ops -- --baseline {{ BASELINE }} --load-baseline {{ BASELINE }}

# Chaos stress: run concurrent tests with high thread count + random scheduling
# Exercises CAS concurrent writes, lock races, and oplog append races
# Requires: nightly (for -Zrandomize-layout if desired)
chaos:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "=== Chaos stress: 64 threads, 120s timeout ==="
    RUSTFLAGS="-Zrandomize-layout" cargo +nightly test \
        --workspace --no-fail-fast \
        -j 64 \
        -E 'test(concurrent) | test(race) | test(parallel) | test(stress)' \
        -- --test-threads=64 2>&1 || true
    echo "=== Running full concurrent CAS + lock tests ==="
    cargo nextest run --workspace --no-fail-fast \
        -E 'test(concurrent_parent_creation) | test(concurrent_append) | test(dedup)' \
        --test-threads=32

# ── Linting & Formatting ──────────────────────────────────────────────────────

# Fast compile check without running tests
check:
    cargo check --workspace

# Run clippy with zero-warning policy; pass extra args e.g. `just lint -W clippy::pedantic`
lint *clippy-args:
    cargo clippy --workspace --all-targets --all-features -- -D warnings -A unknown-lints -W clippy::undocumented_unsafe_blocks --no-deps {{ clippy-args }}

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

# Generate an auto-hydrated monthly report from KPI + benchmark signals
report-hydrate:
    bash scripts/metrics/generate-monthly-report.sh

# ── Build ─────────────────────────────────────────────────────────────────────

[private]
assert-windows-rust-lld:
        @case "$(uname -s 2>/dev/null || echo unknown)" in \
            MINGW*|MSYS*|CYGWIN*) \
                if ! grep -Eq '^\[target\.x86_64-pc-windows-msvc\]' .cargo/config.toml || ! grep -Eq 'linker\s*=\s*"rust-lld"' .cargo/config.toml; then \
                    echo "error: Windows builds must use rust-lld in .cargo/config.toml"; \
                    exit 1; \
                fi ;; \
            *) ;; \
        esac

# Debug build
build:
    @{{ j }} assert-windows-rust-lld
    cargo build --workspace

# Release build
release:
    @{{ j }} assert-windows-rust-lld
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

# Print architecture graph drift summary against previous commit (or provided ref)
arch-drift BASE='HEAD~1':
    bash scripts/ci/detect-arch-drift.sh docs/src/architecture/component-graph.json {{ BASE }}

# Fuzzing smoke check for all fuzz targets
fuzz-check:
    cargo fuzz run fuzz_lang_parser --sanitizer none -- -runs=256 -max_total_time=5
    cargo fuzz run fuzz_net_protocol --sanitizer none -- -runs=256 -max_total_time=5
    cargo fuzz run fuzz_crdt_merge --sanitizer none -- -runs=256 -max_total_time=5

# Run Miri on selected crates (undefined-behavior detection)
miri:
    MIRIFLAGS="-Zmiri-disable-isolation" cargo +nightly miri setup
    MIRIFLAGS="-Zmiri-disable-isolation" cargo +nightly miri test -p arc-algebra --no-fail-fast

# Check API stability against published crate (requires cargo-public-api + nightly)
api-drift:
    cargo +nightly public-api --manifest-path crates/arc-algebra/Cargo.toml diff HEAD~1

# Check compilation on beta toolchain (informational, non-blocking)
beta-check:
    cargo +beta check --workspace

# Check that feature-gated crates compile without default features
no-default-features:
    cargo check -p arc-algebra-types --no-default-features
    cargo check -p arc-store-cas --no-default-features
    cargo check -p arc-store-types --no-default-features
    cargo check -p arc-store-graph --no-default-features
    cargo check -p arc-core --no-default-features
    cargo check -p arc-error --no-default-features

# Benchmark arc-core operations and emit report-friendly summary
bench-trend:
    mkdir -p target/bench
    cargo bench -p arc-core --bench core_ops | tee target/bench/core_ops.latest.txt
    printf "## Benchmark Snapshot\n\n- Command: cargo bench -p arc-core --bench core_ops\n- Raw output: target/bench/core_ops.latest.txt\n\n### Extracted timings\n" > target/bench/core_ops-summary.md
    grep -E "arc_core_cas/.+time:" target/bench/core_ops.latest.txt >> target/bench/core_ops-summary.md || true

# Fast contributor lane: most common local pre-push checks
verify-fast: fmt-check lint test doc-tests verify-docs

# Full lane: mirrors strict CI + policy + supply chain checks
verify-full: verify-fast verify-policy verify-security fuzz-check

# ── Full CI gate ──────────────────────────────────────────────────────────────

# Complete local CI check — mirrors CI pipeline exactly
ci: verify-full audit api-drift
    rustup toolchain install nightly --profile minimal
    cargo install cargo-public-api --locked --version 0.51.0
    bash scripts/ci/lint-reports.sh
