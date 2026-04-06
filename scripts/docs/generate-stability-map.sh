#!/usr/bin/env bash

set -euo pipefail

GRAPH_PATH="${1:-docs/architecture/component-graph.json}"
OUT_PATH="${2:-docs/architecture/API_STABILITY.md}"

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 is required" >&2
  exit 1
fi

if [ ! -f "$GRAPH_PATH" ]; then
  echo "error: graph file not found: $GRAPH_PATH" >&2
  exit 1
fi

python3 - "$GRAPH_PATH" "$OUT_PATH" <<'PY'
import datetime as dt
import json
import pathlib
import sys

in_path = pathlib.Path(sys.argv[1])
out_path = pathlib.Path(sys.argv[2])

data = json.loads(in_path.read_text(encoding="utf-8"))
nodes = data.get("nodes", [])

label = {
    1: "Stable",
    2: "Experimental",
    3: "Internal",
}

rows = []
for node in sorted(nodes, key=lambda x: x.get("id", "")):
    crate = node.get("id", "")
    owner = node.get("owner", "")
    tier = node.get("stabilityTier")
    status = label.get(tier, f"Unknown ({tier})")
    rows.append((crate, owner, status))

lines = [
    "# API Stability Map",
    "",
    "Generated from docs/architecture/component-graph.json.",
    f"Generated on: {dt.date.today().isoformat()}",
    "",
    "| Crate | Owner | Stability |",
    "| ----- | ----- | --------- |",
]

for crate, owner, status in rows:
    lines.append(f"| {crate} | {owner} | {status} |")

out_path.write_text("\n".join(lines) + "\n", encoding="utf-8")
PY

echo "Generated $OUT_PATH"
