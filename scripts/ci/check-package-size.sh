#!/usr/bin/env bash

set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is required" >&2
  exit 1
fi

if ! cargo diet --help >/dev/null 2>&1; then
  echo "error: cargo-diet is required (cargo install cargo-diet)" >&2
  exit 1
fi

if [ "$#" -lt 1 ]; then
  echo "usage: $0 <crate-dir[:limit]> [<crate-dir[:limit]> ...]" >&2
  echo "example: $0 crates/arc-core:120KB crates/arc-cli:300KB" >&2
  exit 1
fi

default_limit="${ARC_PACKAGE_SIZE_LIMIT:-200KB}"

for spec in "$@"; do
  crate_dir="${spec%%:*}"
  limit="${spec#*:}"
  if [ "$limit" = "$spec" ]; then
    limit="$default_limit"
  fi

  echo "Checking package size in $crate_dir (limit: $limit)"
  (
    cd "$crate_dir"
    cargo diet -n --package-size-limit "$limit" | grep -F "package size" || true
    cargo diet -n --package-size-limit "$limit" >/dev/null
  )
done

echo "Package size checks passed."
