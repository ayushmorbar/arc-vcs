#!/usr/bin/env bash
set -euo pipefail

BASE_REF="${1:-${GITHUB_BASE_REF:-HEAD~1}}"
HEAD_REF="${2:-${GITHUB_SHA:-HEAD}}"
OUT_FILE="${3:-CHANGELOG.semantic.md}"

if ! command -v git >/dev/null 2>&1; then
  echo "error: git is required" >&2
  exit 1
fi

# Semantic grouping taxonomy expressed as revset-inspired impact buckets.
declare -A IMPACT_PATHS
IMPACT_PATHS["Spacetime Algebra"]="crates/arc-algebra crates/arc-algebra-types crates/arc-change"
IMPACT_PATHS["Graph Topology"]="crates/arc-store-graph crates/arc-revset"
IMPACT_PATHS["Mutable State"]="crates/arc-store-view crates/arc-store-cas"
IMPACT_PATHS["Execution Engine"]="crates/arc-engine"
IMPACT_PATHS["Network Transport"]="crates/arc-network crates/arc-net"
IMPACT_PATHS["Orchestration"]="crates/arc-cli crates/arc-daemon"

IMPACT_BUCKETS=(
  "Spacetime Algebra"
  "Graph Topology"
  "Mutable State"
  "Execution Engine"
  "Network Transport"
  "Orchestration"
)

{
  echo "# Semantic Changelog"
  echo
  echo "Generated from ${BASE_REF}..${HEAD_REF}"
  echo
  echo "## Spacetime Impact Buckets"
  echo

  for bucket in "${IMPACT_BUCKETS[@]}"; do
    echo "### ${bucket}"
    found=0
    for path in ${IMPACT_PATHS[$bucket]}; do
      while IFS= read -r line; do
        if [ -n "$line" ]; then
          echo "- ${line}"
          found=1
        fi
      done < <(git log --no-merges --pretty=format:'%h %s' "${BASE_REF}..${HEAD_REF}" -- "$path")
    done
    if [ "$found" -eq 0 ]; then
      echo "- No changes"
    fi
    echo
  done

  echo "## Revset Queries Used"
  echo
  echo "- ancestors(@) & touched(\"crates/arc-algebra\")"
  echo "- ancestors(@) & touched(\"crates/arc-store-graph\")"
  echo "- ancestors(@) & touched(\"crates/arc-engine\")"
  echo "- ancestors(@) & touched(\"crates/arc-network\")"
} > "$OUT_FILE"

echo "Semantic changelog generated at $OUT_FILE"
