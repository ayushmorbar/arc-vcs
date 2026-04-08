#!/usr/bin/env bash

set -euo pipefail

STALE_DAYS="${STALE_DAYS:-14}"
MERGED_SAMPLE="${MERGED_SAMPLE:-50}"

if ! command -v gh >/dev/null 2>&1; then
  echo "error: gh CLI is required" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 is required" >&2
  exit 1
fi

open_json="$(gh pr list --state open --limit 200 --json number,updatedAt 2>/dev/null || true)"
merged_json="$(gh pr list --state merged --limit "$MERGED_SAMPLE" --json createdAt,reviews 2>/dev/null || true)"

if [ -z "$open_json" ] || [ "$open_json" = "null" ]; then
  echo "error: unable to query open PRs. Ensure gh auth is configured." >&2
  exit 1
fi

if [ -z "$merged_json" ] || [ "$merged_json" = "null" ]; then
  echo "error: unable to query merged PRs. Ensure gh auth is configured." >&2
  exit 1
fi

python3 - "$open_json" "$merged_json" "$STALE_DAYS" <<'PY'
import json
import statistics
import sys
from datetime import datetime, timezone

open_prs = json.loads(sys.argv[1])
merged_prs = json.loads(sys.argv[2])
stale_days = int(sys.argv[3])

now = datetime.now(timezone.utc)
stale_seconds = stale_days * 86400

def parse_iso(ts: str):
    return datetime.fromisoformat(ts.replace("Z", "+00:00"))

open_count = len(open_prs)
stale_count = 0
for pr in open_prs:
    updated = pr.get("updatedAt")
    if not updated:
        continue
    delta = (now - parse_iso(updated)).total_seconds()
    if delta > stale_seconds:
        stale_count += 1

turnaround_hours = []
for pr in merged_prs:
    created = pr.get("createdAt")
    if not created:
        continue
    reviews = pr.get("reviews", [])
    if not reviews:
        continue
    first = None
    for rv in reviews:
        submitted = rv.get("submittedAt")
        if not submitted:
            continue
        dt = parse_iso(submitted)
        if first is None or dt < first:
            first = dt
    if first is None:
        continue
    created_dt = parse_iso(created)
    hours = (first - created_dt).total_seconds() / 3600.0
    if hours >= 0:
        turnaround_hours.append(hours)

median_turnaround = (
    round(statistics.median(turnaround_hours), 2) if turnaround_hours else None
)

print("Review KPI Snapshot")
print(f"- Open PR count: {open_count}")
print(f"- Stale PR count (>{stale_days} days): {stale_count}")
if median_turnaround is None:
    print("- Median review turnaround (hours): N/A (insufficient review data)")
else:
    print(f"- Median review turnaround (hours): {median_turnaround}")
PY
