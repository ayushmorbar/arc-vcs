---
name: semver
description: >
  Applies Semantic Versioning 2.0.0 (SemVer) rules to version numbers,
  release decisions, and dependency management. Use when the user asks:
  "should I bump major, minor, or patch?", "is this a breaking change?",
  "how do I version this release?", "what is a valid SemVer string?",
  "how do pre-release or build metadata labels work?", "how are two
  versions compared?", "is my version string valid?", or any question
  involving MAJOR.MINOR.PATCH version format, dependency hell, version
  precedence, or the SemVer spec.
license: CC-BY-3.0
compatibility:
  - claude-code
  - cursor
  - codex
  - gemini-cli
  - github-copilot
  - vs-code
  - jetbrains-junie
  - opencode
  - goose
metadata:
  author: semver.org (Tom Preston-Werner)
  spec_version: "2.0.0"
  skill_version: "1.0.0"
  source: https://semver.org/spec/v2.0.0.html
  tags:
    - versioning
    - releases
    - semver
    - dependency-management
    - api-design
allowed-tools:
  - read
  - write
***

# Semantic Versioning 2.0.0

## Core Format

`MAJOR.MINOR.PATCH` — all three parts are non-negative integers, no leading
zeroes. Each part increases numerically. A released version MUST NOT be
modified — changes always produce a new version number.

## Increment Decision Table

| Change type | Bump | Reset rule |
|---|---|---|
| Backward-incompatible API change | **MAJOR** | Reset MINOR and PATCH to 0 |
| New backward-compatible feature OR any deprecation | **MINOR** | Reset PATCH to 0 |
| Backward-compatible bug fix only | **PATCH** | — |
| Internal change with no public API effect | PATCH or MINOR | See FAQ |

**Special ranges:**
- `0.y.z` — initial development; anything may change; public API NOT stable.
- `1.0.0` — defines the first stable public API.

## Pre-release Labels

Append `-` + dot-separated identifiers immediately after PATCH:

```
1.0.0-alpha
1.0.0-alpha.1
1.0.0-0.3.7
1.0.0-x.7.z.92
```

Rules:
- Identifiers: `[0-9A-Za-z-]` only; MUST NOT be empty.
- Numeric identifiers MUST NOT have leading zeroes (`01` is invalid).
- Pre-release has **lower** precedence than the associated normal version:
  `1.0.0-alpha < 1.0.0`

## Build Metadata

Append `+` + dot-separated identifiers after PATCH or pre-release:

```
1.0.0+20130313144700
1.0.0-beta+exp.sha.5114f85
1.0.0+21AF26D3----117B344092BD
```

- Same identifiers rule as pre-release.
- Build metadata is **completely ignored for precedence**. Two versions
  differing only in build metadata have equal precedence.

## Precedence (Ordering) Rules

Compare left to right: **MAJOR → MINOR → PATCH** (always numeric), then
pre-release identifiers when cores are equal.

Pre-release comparison (left-to-right per dot-field):
1. Digits-only → compare numerically (`2 < 11`).
2. Any letter/hyphen → compare lexically in ASCII order.
3. Numeric identifier < alphanumeric identifier.
4. Larger set of fields wins when all preceding fields are equal.

**Canonical example:**
```
1.0.0-alpha < 1.0.0-alpha.1 < 1.0.0-alpha.beta
  < 1.0.0-beta < 1.0.0-beta.2 < 1.0.0-beta.11
  < 1.0.0-rc.1 < 1.0.0
```

## Validation Regex

Named groups (PCRE / Python / Go):
```
^(?P<major>0|[1-9]\d*)\.(?P<minor>0|[1-9]\d*)\.(?P<patch>0|[1-9]\d*)(?:-(?P<prerelease>(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+(?P<buildmetadata>[0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$
```

Numbered groups (JS / PCRE / Python / Go — cg1=major, cg2=minor, cg3=patch,
cg4=prerelease, cg5=buildmetadata):
```
^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-((?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*)(?:\.(?:0|[1-9]\d*|\d*[a-zA-Z-][0-9a-zA-Z-]*))*))?(?:\+([0-9a-zA-Z-]+(?:\.[0-9a-zA-Z-]+)*))?$
```

> `v1.2.3` is NOT a valid SemVer string — the `v` prefix is a tag convention
> (e.g., `git tag v1.2.3`). The semantic version itself is `1.2.3`.

## BNF Grammar (Quick Reference)

See `references/bnf-grammar.md` for the full grammar.

```
<valid semver> ::= <version core>
                 | <version core> "-" <pre-release>
                 | <version core> "+" <build>
                 | <version core> "-" <pre-release> "+" <build>
<version core>  ::= <major> "." <minor> "." <patch>
```

## FAQ — Common Scenarios

| Scenario | Answer |
|---|---|
| Accidentally released a breaking change as MINOR | Fix and release a new MINOR that restores compatibility; never modify the released version. |
| Updated internal deps, no API change | PATCH (bug fix) or MINOR (new functionality added). |
| Deprecating a feature | New MINOR release with deprecation noted in docs; only remove in a future MAJOR. |
| When to go 1.0.0 | When software is in production, has a stable API, or users depend on backward compat. |
| Initial development versioning | Start at `0.1.0`; bump MINOR for each subsequent release. |
| Documenting the entire public API | It is a professional responsibility — SemVer requires a declared public API. |
| Version string size limit | No spec limit; use good judgment (255 chars is overkill). |

*Source: https://semver.org/spec/v2.0.0.html — CC BY 3.0, Tom Preston-Werner*