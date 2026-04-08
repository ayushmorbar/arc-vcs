#!/usr/bin/env bash
set -euo pipefail

if ! command -v cargo >/dev/null 2>&1; then
  echo "error: cargo is required" >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  echo "error: python3 is required" >&2
  exit 1
fi

# Use cargo-deny as a baseline supply-chain gate as required by policy.
if ! command -v cargo-deny >/dev/null 2>&1; then
  cargo install cargo-deny --locked --version 0.16.4
fi

if [ -f "deny.toml" ]; then
  cargo deny check bans --hide-inclusion-graph
fi

meta_file="$(mktemp)"
trap 'rm -f "$meta_file"' EXIT
cargo metadata --format-version 1 --locked > "$meta_file"

python3 - "$meta_file" <<'PY'
import json
import sys

meta_path = sys.argv[1]
with open(meta_path, "r", encoding="utf-8") as f:
    meta = json.load(f)

packages = {p["name"]: p for p in meta.get("packages", [])}

# Lower number means lower tier; lower-tier crates may not depend upward.
tier = {
    "arc-algebra-types": 1,
    "arc-store-types": 1,
    "arc-store-cas": 2,
    "arc-change": 3,
    "arc-store-graph": 4,
    "arc-algebra": 5,
    "arc-store-view": 6,
    "arc-engine": 7,
    "arc-revset": 8,
    "arc-network": 9,
    "arc-core": 10,
    "arc-lang": 11,
    "arc-git": 11,
    "arc-ai": 11,
    "arc-net": 11,
    "arc-policy": 11,
    "arc-store-policy": 11,
    "arc-transaction": 11,
    "arc-diagnostics": 11,
    "arc-git-bridge": 12,
    "arc-daemon": 12,
    "arc-cli": 12,
    "arc-content-hash-derive": 1,
    "arc-testtools": 12,
    "arc-git-native": 12,
    "arc-lsp": 12,
}

workspace_arc = sorted(
    name for name in packages.keys() if name.startswith("arc-")
)
missing_tiers = [name for name in workspace_arc if name not in tier]
if missing_tiers:
    print("error: missing purity tier assignments:", file=sys.stderr)
    for name in missing_tiers:
        print(f"  - {name}", file=sys.stderr)
    sys.exit(1)

violations = []
for name, pkg in packages.items():
    if name not in tier:
        continue
    src_tier = tier[name]
    for dep in pkg.get("dependencies", []):
        dst = dep.get("name")
        if dst not in tier:
            continue
        dst_tier = tier[dst]
        if src_tier < dst_tier:
            violations.append((name, src_tier, dst, dst_tier))

if violations:
    print("error: purity/layering violations detected", file=sys.stderr)
    for src, s_tier, dst, d_tier in sorted(violations):
        print(
            f"  - {src} (tier {s_tier}) depends on {dst} (tier {d_tier})",
            file=sys.stderr,
        )
    sys.exit(1)

print("Purity check passed: no lower-tier crate depends on a higher-tier crate.")
PY
