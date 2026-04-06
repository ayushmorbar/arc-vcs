# CI Scripts

This directory contains shell scripts used by CI and release validation.

## Scripts

- `check-package-size.sh`: enforce package size budgets with `cargo diet`.
- `enforce-unique-tag.sh`: ensure exactly one `v*` tag points at `HEAD`.

## Usage

```bash
bash scripts/ci/check-package-size.sh crates/arc-core:120KB crates/arc-cli:300KB
bash scripts/ci/enforce-unique-tag.sh
```

## Execution bits

If your platform does not preserve executable permissions, invoke these scripts with `bash` as shown above.
