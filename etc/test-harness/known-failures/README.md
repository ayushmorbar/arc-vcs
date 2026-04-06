# Known Test Failures Registry

Track platform-specific expected failures to keep CI signal actionable.

## Rules

1. Every entry must include an issue reference.
2. Entries must be removed once the fix is merged and released.
3. Do not add flaky tests without root-cause notes.

## Files

- `windows.txt`: expected failures on Windows.
- add additional platform files as needed (`linux-arm64.txt`, `macos.txt`, etc.).
