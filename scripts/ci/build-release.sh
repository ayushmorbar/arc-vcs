#!/usr/bin/env bash
# Build arc-cli release binary for a specific target and package it.
#
# Usage: build-release.sh <target>
#   target: the Rust target triple (e.g. x86_64-pc-windows-msvc)
#
# Environment variables:
#   PROFILE  - build profile (default: release)
#   DIST_DIR - output directory for archives (default: dist/)

set -euo pipefail

TARGET="${1:?Usage: build-release.sh <target>}"
PROFILE="${PROFILE:-release}"
DIST_DIR="${DIST_DIR:-dist}"
VERSION="${VERSION:?VERSION must be set (e.g. from git tag)}"
OS="$(uname -s)"

echo "==> Building arc-cli for ${TARGET} (profile: ${PROFILE})"

# ── Determine build command ──────────────────────────────────────────────────
# cargo-cross handles Docker-based cross-compilation for foreign Linux targets.
# For native builds, use plain `cargo build`.
if command -v cross &>/dev/null && [[ "${TARGET}" == aarch64-unknown-linux-gnu ]] && [ "${OS}" = "Linux" ] && [ "$(uname -m)" != "aarch64" ]; then
    echo "--- Using cargo-cross for cross-compilation"
    BUILD_CMD=(cross build)
else
    BUILD_CMD=(cargo build)
fi

# ── Build ────────────────────────────────────────────────────────────────────
"${BUILD_CMD[@]}" \
    --profile "${PROFILE}" \
    --target "${TARGET}" \
    --package arc-cli

# ── Locate the built binary ──────────────────────────────────────────────────
# cargo places binaries at target/<target>/<profile>/arc-cli[.exe]
EXE_DIR="target/${TARGET}/${PROFILE}"
if [ "${OS}" = "MINGW"* ] || [ "${OS}" = "MSYS"* ] || [[ "${TARGET}" == *windows* ]]; then
    EXE_NAME="arc-cli.exe"
    ARCHIVE_EXT="zip"
else
    EXE_NAME="arc-cli"
    ARCHIVE_EXT="tar.gz"
fi

EXE_PATH="${EXE_DIR}/${EXE_NAME}"
if [ ! -f "${EXE_PATH}" ]; then
    echo "ERROR: Built binary not found at ${EXE_PATH}"
    exit 1
fi

echo "--- Binary: ${EXE_PATH} ($(du -h "${EXE_PATH}" | cut -f1))"

# ── Package ──────────────────────────────────────────────────────────────────
mkdir -p "${DIST_DIR}"

# Normalize target name for archive filename (replace / with -)
TARGET_SLASH="${TARGET//\//-}"
ARCHIVE_NAME="arc-${VERSION}-${TARGET_SLASH}"

if [ "${ARCHIVE_EXT}" = "zip" ]; then
    ARCHIVE_PATH="${DIST_DIR}/${ARCHIVE_NAME}.zip"
    echo "--- Creating ${ARCHIVE_PATH}"
    (cd "${EXE_DIR}" && powershell.exe -Command "Compress-Archive -Path '${EXE_NAME}' -DestinationPath '../../../${ARCHIVE_PATH}'" 2>/dev/null || {
        # Fallback: use zip if available
        (cd "${EXE_DIR}" && zip -q "../../${ARCHIVE_NAME}.zip" "${EXE_NAME}")
    })
else
    ARCHIVE_PATH="${DIST_DIR}/${ARCHIVE_NAME}.tar.gz"
    echo "--- Creating ${ARCHIVE_PATH}"
    (cd "${EXE_DIR}" && tar czf "../../${ARCHIVE_NAME}.tar.gz" "${EXE_NAME}")
fi

echo "--- Archive: ${ARCHIVE_PATH} ($(du -h "${ARCHIVE_PATH}" | cut -f1))"

# ── Compute checksum ─────────────────────────────────────────────────────────
CHECKSUM_PATH="${ARCHIVE_PATH}.sha256"
if command -v sha256sum &>/dev/null; then
    (cd "${DIST_DIR}" && sha256sum "$(basename "${ARCHIVE_PATH}")" > "$(basename "${CHECKSUM_PATH}")")
elif command -v shasum &>/dev/null; then
    (cd "${DIST_DIR}" && shasum -a 256 "$(basename "${ARCHIVE_PATH}")" > "$(basename "${CHECKSUM_PATH}")")
fi

echo "==> Build complete for ${TARGET}"
echo "    Archive: ${ARCHIVE_PATH}"
echo "    Checksum: ${CHECKSUM_PATH}"
