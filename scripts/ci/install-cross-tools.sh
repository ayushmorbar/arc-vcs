#!/usr/bin/env bash
# Install cross-compilation tools for release builds.
# Called by release.yml for cross-target matrix jobs.
#
# Usage: install-cross-tools.sh <target>
#   target: the Rust target triple (e.g. aarch64-unknown-linux-gnu, x86_64-pc-windows-msvc)

set -euo pipefail

TARGET="${1:?Usage: install-cross-tools.sh <target>}"
OS="$(uname -s)"

echo "==> Installing cross-compilation tools for ${TARGET}"

# ── cargo-cross ──────────────────────────────────────────────────────────────
# cargo-cross provides hermetic Docker-based cross-compilation for Linux targets.
# On macOS/Windows native runners it's a thin wrapper (often a no-op).
if ! command -v cross &>/dev/null; then
    echo "--- Installing cargo-cross via install-action (handled by workflow)"
else
    echo "--- cargo-cross already installed: $(cross --version)"
fi

# ── Target-specific setup ────────────────────────────────────────────────────
case "${TARGET}" in
    aarch64-unknown-linux-gnu)
        if [ "${OS}" = "Linux" ]; then
            echo "--- Installing gcc-aarch64-linux-gnu cross-compiler"
            sudo apt-get update -qq
            sudo apt-get install -y -qq gcc-aarch64-linux-gnu > /dev/null 2>&1 || true
            echo "--- Adding Rust target"
            rustup target add aarch64-unknown-linux-gnu || true
        fi
        ;;
    aarch64-apple-darwin)
        if [ "${OS}" = "Darwin" ]; then
            echo "--- macOS arm64 cross-compilation uses native toolchain"
            rustup target add aarch64-apple-darwin || true
        fi
        ;;
    x86_64-apple-darwin)
        if [ "${OS}" = "Darwin" ]; then
            echo "--- macOS x86_64 cross-compilation uses native toolchain"
            rustup target add x86_64-apple-darwin || true
        fi
        ;;
    x86_64-unknown-linux-gnu)
        echo "--- Linux x86_64 native build"
        ;;
    x86_64-unknown-linux-musl)
        if [ "${OS}" = "Linux" ]; then
            echo "--- Installing musl-tools"
            sudo apt-get update -qq
            sudo apt-get install -y -qq musl-tools > /dev/null 2>&1 || true
            rustup target add x86_64-unknown-linux-musl || true
        fi
        ;;
    x86_64-pc-windows-msvc)
        echo "--- Windows MSVC build uses native toolchain (windows-latest runner)"
        ;;
    *)
        echo "WARNING: Unknown target '${TARGET}', skipping target-specific setup"
        ;;
esac

echo "==> Cross-compilation tools ready for ${TARGET}"
