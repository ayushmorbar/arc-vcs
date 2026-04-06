#!/usr/bin/env bash

set -euo pipefail

GRAPH_PATH="${1:-docs/src/architecture/component-graph.json}"
BASE_REF="${2:-HEAD~1}"

if ! command -v git >/dev/null 2>&1; then
  echo "error: git is required" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 is required" >&2
  exit 1
fi

if [ ! -f "$GRAPH_PATH" ]; then
  echo "error: graph file not found: $GRAPH_PATH" >&2
  exit 1
fi

current_json="$(cat "$GRAPH_PATH")"
base_json="$(git show "$BASE_REF:$GRAPH_PATH" 2>/dev/null || true)"

if [ -z "$base_json" ]; then
  echo "Architecture drift summary"
  echo "- Base graph not found at $BASE_REF. Treating all current nodes/rules as new."
  python3 - "$current_json" <<'PY'
import json
import sys
cur = json.loads(sys.argv[1])
print(f"- Added nodes: {len(cur.get('nodes', []))}")
print(f"- Removed nodes: 0")
print(f"- Added forbidden edges: {len(cur.get('rules', {}).get('forbiddenClassEdges', []))}")
print("- Removed forbidden edges: 0")
PY
  exit 0
fi

python3 - "$base_json" "$current_json" <<'PY'
import json
import sys

base = json.loads(sys.argv[1])
cur = json.loads(sys.argv[2])

base_nodes = {node.get("id") for node in base.get("nodes", []) if node.get("id")}
cur_nodes = {node.get("id") for node in cur.get("nodes", []) if node.get("id")}

added_nodes = sorted(cur_nodes - base_nodes)
removed_nodes = sorted(base_nodes - cur_nodes)

def edges(doc):
    out = set()
    for edge in doc.get("rules", {}).get("forbiddenClassEdges", []):
        src = edge.get("from")
        dst = edge.get("to")
        if src and dst:
            out.add((src, dst))
    return out

base_edges = edges(base)
cur_edges = edges(cur)

added_edges = sorted(cur_edges - base_edges)
removed_edges = sorted(base_edges - cur_edges)

print("Architecture drift summary")
print(f"- Added nodes ({len(added_nodes)}): {', '.join(added_nodes) if added_nodes else 'none'}")
print(f"- Removed nodes ({len(removed_nodes)}): {', '.join(removed_nodes) if removed_nodes else 'none'}")
print(
    f"- Added forbidden edges ({len(added_edges)}): "
    + (", ".join(f"{a}->{b}" for a, b in added_edges) if added_edges else "none")
)
print(
    f"- Removed forbidden edges ({len(removed_edges)}): "
    + (", ".join(f"{a}->{b}" for a, b in removed_edges) if removed_edges else "none")
)
PY
