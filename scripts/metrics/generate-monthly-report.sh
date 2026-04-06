#!/usr/bin/env bash

set -euo pipefail

TEMPLATE_PATH="${1:-docs/reports/templates/monthly_report_template.md}"
OUT_DIR="${2:-docs/reports/monthly}"
PERIOD="${PERIOD:-$(date +%Y-%m)}"
OUT_PATH="$OUT_DIR/${PERIOD}-auto.md"

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 is required" >&2
  exit 1
fi

if [ ! -f "$TEMPLATE_PATH" ]; then
  echo "error: template not found: $TEMPLATE_PATH" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"

kpi_output="$(bash scripts/metrics/collect-review-kpis.sh || true)"
bench_raw="$(cargo bench -p arc-core --bench core_ops 2>&1 || true)"

python3 - "$TEMPLATE_PATH" "$OUT_PATH" "$PERIOD" "$kpi_output" "$bench_raw" <<'PY'
import datetime as dt
import re
import sys
from pathlib import Path

template_path = Path(sys.argv[1])
out_path = Path(sys.argv[2])
period = sys.argv[3]
kpi_output = sys.argv[4]
bench_output = sys.argv[5]

template = template_path.read_text(encoding="utf-8")

def extract(pattern: str, text: str, default: str = "N/A") -> str:
    m = re.search(pattern, text, re.IGNORECASE)
    return m.group(1).strip() if m else default

open_pr = extract(r"Open PR count:\s*([^\n]+)", kpi_output)
stale_pr = extract(r"Stale PR count .*?:\s*([^\n]+)", kpi_output)
median_turnaround = extract(r"Median review turnaround \(hours\):\s*([^\n]+)", kpi_output)

timing_lines = [
    line.strip() for line in bench_output.splitlines()
    if "arc_core_cas/" in line and "time:" in line
]
bench_summary = "\n".join(f"- {line}" for line in timing_lines[:10])
if not bench_summary:
    bench_summary = "- Benchmark summary unavailable in this run."

rendered = template
rendered = rendered.replace("- Period: YYYY-MM", f"- Period: {period}")
rendered = rendered.replace("- Generated on:", f"- Generated on: {dt.date.today().isoformat()}")
rendered = rendered.replace("- Report owner:", "- Report owner: automation")
rendered = rendered.replace("- Confidence level: high/medium/low", "- Confidence level: medium")

rendered = re.sub(
    r"\| Open PR count\s+\|\s*\|\s*\|\s*\|",
    f"| Open PR count                   | {open_pr} |      |       |",
    rendered,
)
rendered = re.sub(
    r"\| Median review turnaround \(days\) \|\s*\|\s*\|\s*\|",
    f"| Median review turnaround (days) | {median_turnaround}h |      |       |",
    rendered,
)
rendered = re.sub(
    r"\| Stale PRs \(>14d\)\s+\|\s*\|\s*\|\s*\|",
    f"| Stale PRs (>14d)                | {stale_pr} |      |       |",
    rendered,
)

perf_insert = (
    "\n- Auto benchmark summary:\n"
    f"{bench_summary}\n"
    "- Benchmark evidence: target/bench/core_ops.latest.txt\n"
)
rendered = rendered.replace("- Evidence links:\n\n## 6. Testing and Verification", f"- Evidence links:{perf_insert}\n## 6. Testing and Verification")

report_evidence = (
    "\n- Auto-hydration evidence:\n"
    "- KPI source: scripts/metrics/collect-review-kpis.sh\n"
    "- Benchmark command: cargo bench -p arc-core --bench core_ops\n"
)
rendered = rendered.replace("## 11. Evidence Links\n", "## 11. Evidence Links\n" + report_evidence)

out_path.write_text(rendered, encoding="utf-8")
print(f"Generated {out_path}")
PY
