#!/usr/bin/env bash

set -euo pipefail

GRAPH_FILE="${1:-docs/src/architecture/component-graph.json}"

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is required" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 is required" >&2
  exit 1
fi

if [ ! -f "$GRAPH_FILE" ]; then
  echo "error: graph file not found: $GRAPH_FILE" >&2
  exit 1
fi

meta_file="$(mktemp)"
trap 'rm -f "$meta_file"' EXIT

cargo metadata --format-version 1 --no-deps > "$meta_file"

python3 - "$GRAPH_FILE" "$meta_file" <<'PY'
import json
import sys

graph_path = sys.argv[1]
meta_path = sys.argv[2]

with open(graph_path, "r", encoding="utf-8") as f:
    graph = json.load(f)

with open(meta_path, "r", encoding="utf-8") as f:
    meta = json.load(f)

classes = {}
for node in graph.get("nodes", []):
    node_id = node.get("id")
    node_class = node.get("class")
    if node_id and node_class:
        classes[node_id] = node_class

forbidden_pairs = set()
for edge in graph.get("rules", {}).get("forbiddenClassEdges", []):
    src = edge.get("from")
    dst = edge.get("to")
    if src and dst:
        forbidden_pairs.add((src, dst))

workspace_names = {pkg["name"] for pkg in meta.get("packages", [])}

missing = sorted(name for name in workspace_names if name not in classes)
if missing:
    print("error: component graph is missing workspace nodes:", file=sys.stderr)
    for name in missing:
        print(f"  - {name}", file=sys.stderr)
    sys.exit(1)

violations = []
for pkg in meta.get("packages", []):
    src = pkg["name"]
    src_class = classes.get(src)
    if not src_class:
        continue
    for dep in pkg.get("dependencies", []):
        dst = dep.get("name")
        if dst not in workspace_names:
            continue
        dst_class = classes.get(dst)
        if not dst_class:
            continue
        if (src_class, dst_class) in forbidden_pairs:
            violations.append((src, src_class, dst, dst_class))

if violations:
    print("error: forbidden dependency edges found", file=sys.stderr)
    for src, src_class, dst, dst_class in sorted(violations):
        print(
            f"  - {src} ({src_class}) -> {dst} ({dst_class})",
            file=sys.stderr,
        )
    sys.exit(1)

print("Layering rules passed: no forbidden edges found.")
PY
