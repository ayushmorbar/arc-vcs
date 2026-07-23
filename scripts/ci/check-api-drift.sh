#!/usr/bin/env bash

set -euo pipefail

GRAPH_FILE="${GRAPH_FILE:-docs/src/architecture/component-graph.json}"
BASELINE_DIR="${BASELINE_DIR:-docs/architecture/api-baselines}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is required" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 is required" >&2
  exit 1
fi

if ! command -v diff >/dev/null 2>&1; then
  echo "error: diff is required" >&2
  exit 1
fi

if [ ! -f "$GRAPH_FILE" ]; then
  echo "error: component graph not found at $GRAPH_FILE" >&2
  exit 1
fi

if [ ! -d "$BASELINE_DIR" ]; then
  echo "error: baseline directory not found at $BASELINE_DIR" >&2
  exit 1
fi

tmp_dir="$(mktemp -d)"
meta_file="$tmp_dir/metadata.json"
stable_file="$tmp_dir/stable.tsv"
trap 'rm -rf "$tmp_dir"' EXIT

cargo metadata --format-version 1 --no-deps > "$meta_file"

python3 - "$GRAPH_FILE" "$meta_file" "$stable_file" <<'PY'
import json
import pathlib
import sys

graph_path = pathlib.Path(sys.argv[1])
meta_path = pathlib.Path(sys.argv[2])
out_path = pathlib.Path(sys.argv[3])

graph = json.loads(graph_path.read_text(encoding="utf-8"))
meta = json.loads(meta_path.read_text(encoding="utf-8"))

packages = {pkg["name"]: pkg for pkg in meta.get("packages", [])}

stable_nodes = []
for node in graph.get("nodes", []):
    tier = node.get("stabilityTier")
    status = str(node.get("stability", "")).strip().lower()
    if tier in (1, 2) or status == "stable":
        stable_nodes.append(node.get("id"))

lines = []
for crate in sorted(set(filter(None, stable_nodes))):
    pkg = packages.get(crate)
    if pkg is None:
        print(f"error: stable crate '{crate}' missing from cargo metadata", file=sys.stderr)
        sys.exit(1)

    has_lib = any("lib" in target.get("kind", []) for target in pkg.get("targets", []))
    manifest = pkg.get("manifest_path")
    if not manifest:
        print(f"error: crate '{crate}' missing manifest path", file=sys.stderr)
        sys.exit(1)

    lines.append(f"{crate}\t{manifest}\t{1 if has_lib else 0}")

out_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
PY

if [ ! -s "$stable_file" ]; then
  echo "error: no stable crates were discovered in component graph" >&2
  exit 1
fi

failures=0

while IFS=$'\t' read -r crate manifest has_lib; do
  baseline="$BASELINE_DIR/$crate.public-api.txt"
  if [ ! -f "$baseline" ]; then
    echo "error: missing API baseline for $crate at $baseline" >&2
    failures=1
    continue
  fi

  current="$tmp_dir/$crate.current.txt"

  if [ "$has_lib" = "1" ]; then
    cargo +nightly public-api --manifest-path "$manifest" -ss --color never > "$current"
  else
    {
      echo "# NO_PUBLIC_LIBRARY_API"
      echo "# crate=$crate"
    } > "$current"
  fi

  if ! diff -u "$baseline" "$current"; then
    echo "error: public API drift detected for $crate" >&2
    failures=1
  fi
done < "$stable_file"

if [ "$failures" -ne 0 ]; then
  exit 1
fi

echo "API drift check passed for all stable crates (tier 1 and tier 2)."
