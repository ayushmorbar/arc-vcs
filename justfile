# arc justfile — project task runner
# Install `just` with: cargo install just
# Run `just` or `just --list` to see available recipes.

# Show all available recipes
default:
    @just --list

# ── Testing ──────────────────────────────────────────────────────────────────

# Run all workspace tests
test:
    cargo test --workspace

# Verify local tooling + GitHub governance + root workspace policy checks
verify:
    cargo test -p arc-cli tooling::tests::tooling_audit_current_workspace governance::tests::governance_audit_current_workspace workspace_policy::tests::workspace_policy_audit_current_workspace

# Run tests matching a filter string
test-filter FILTER:
    cargo test --workspace {{FILTER}}

# ── Linting & Formatting ─────────────────────────────────────────────────────

# Run clippy with zero-warning policy (same as CI)
lint:
    cargo clippy --all-targets -- -D warnings

# Apply rustfmt to all files
fmt:
    cargo fmt --all

# Check formatting without modifying files (used in CI)
fmt-check:
    cargo fmt --all -- --check

# ── Documentation ─────────────────────────────────────────────────────────────

# Build the mdBook documentation (output: docs/book/)
docs:
    mdbook build docs

# Serve documentation with live reload; opens http://localhost:3000 automatically
docs-serve:
    mdbook serve --open docs

# ── Build ─────────────────────────────────────────────────────────────────────

# Debug build (fast compile)
build:
    cargo build --workspace

# Release build
release:
    cargo build --workspace --release

# Remove all build artefacts
clean:
    cargo clean

# ── Security ──────────────────────────────────────────────────────────────────

# Audit dependencies for known CVEs (requires: cargo install cargo-audit)
audit:
    cargo audit

# ── Full CI gate ──────────────────────────────────────────────────────────────

# Run the complete local CI check: test + lint + format (mirrors CI pipeline)
ci:
    cargo test --workspace
    cargo clippy --all-targets -- -D warnings
    cargo fmt --all -- --check
    cargo test -p arc-cli tooling::tests::tooling_audit_current_workspace governance::tests::governance_audit_current_workspace workspace_policy::tests::workspace_policy_audit_current_workspace
