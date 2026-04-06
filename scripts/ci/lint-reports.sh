#!/usr/bin/env bash

set -euo pipefail

reports_dir="${1:-docs/reports/monthly}"

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 is required" >&2
  exit 1
fi

if [ ! -d "$reports_dir" ]; then
  echo "error: monthly reports directory is missing: $reports_dir" >&2
  exit 1
fi

latest_report="$(ls -1 "$reports_dir"/*.md 2>/dev/null | sort | tail -n 1 || true)"
if [ -z "$latest_report" ]; then
  echo "error: no monthly report markdown files found in $reports_dir" >&2
  exit 1
fi

python3 - "$latest_report" <<'PY'
import pathlib
import re
import sys

path = pathlib.Path(sys.argv[1])
text = path.read_text(encoding="utf-8")

section_patterns = {
    "security": re.compile(r"^##\s+.*security.*$", re.IGNORECASE | re.MULTILINE),
    "performance": re.compile(r"^##\s+.*performance.*$", re.IGNORECASE | re.MULTILINE),
    "evidence": re.compile(r"^##\s+.*evidence.*$", re.IGNORECASE | re.MULTILINE),
}

headings = list(re.finditer(r"^##\s+.+$", text, re.MULTILINE))

def section_body(start_idx: int) -> str:
    next_idx = len(text)
    for h in headings:
        if h.start() > start_idx:
            next_idx = h.start()
            break
    return text[start_idx:next_idx]

errors = []
for name, pattern in section_patterns.items():
    match = pattern.search(text)
    if not match:
        errors.append(f"missing required section containing '{name}'")
        continue

    body = section_body(match.end()).strip()
    normalized = re.sub(r"[\s\-|:]+", "", body).lower()
    if not normalized:
        errors.append(f"section '{name}' is empty")
        continue

    if "todo" in body.lower():
        errors.append(f"section '{name}' contains TODO placeholder")

if errors:
    print(f"error: report lint failed for {path}", file=sys.stderr)
    for err in errors:
        print(f"  - {err}", file=sys.stderr)
    sys.exit(1)

print(f"Report lint passed: {path}")
PY
