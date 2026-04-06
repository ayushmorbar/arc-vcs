# Governance

arc follows a **benevolent-dictator-for-now** model during the pre-1.0 period, transitioning to a **maintainer council** model after the 1.0 stable release or soon.

---

## Decision Making

### Routine Changes

Bug fixes, documentation improvements, and minor feature additions that do not alter the algebraic model, storage format, sync protocol, or public API surface are decided by any maintainer with merge rights via standard PR review.

### Significant Changes

Changes that affect:

- The `Atom` or `Change` type definitions
- The commutativity predicate (`commutes()`)
- The CAS storage format (`.arc/blobs/` layout or encoding)
- The `arc-net` wire protocol
- Any Tier 1 or Tier 2 API (see [STABILITY.md](STABILITY.md))

…require:

1. **An RFC** — open a GitHub Issue titled `RFC: <description>` at least 7 days before implementing.
2. **An Architecture Decision Record (ADR)** — document the decision in `docs/src/architecture/decisions/` following the template in existing ADRs. Reference the ADR in the merge commit.
3. **Maintainer approval** — at least one maintainer who did not author the PR must approve.

### Breaking Changes

Breaking changes to Tier 1 APIs are not permitted without a major version bump (semver). They require an RFC, an ADR, a migration guide in `docs/`, and unanimous maintainer approval.

---

## Versioning Policy

arc follows [Semantic Versioning](https://semver.org/):

| Version type        | Trigger                                                                                                    |
| ------------------- | ---------------------------------------------------------------------------------------------------------- |
| **PATCH** (`1.0.x`) | Bug fixes, documentation, dependency updates, no API change                                                |
| **MINOR** (`1.x.0`) | New backwards-compatible features; new `arc` subcommands; new `RepoConfig` fields with `#[serde(default)]` |
| **MAJOR** (`x.0.0`) | Breaking changes to Tier 1 APIs, storage format changes, wire protocol changes                             |

---

## Stability Tiers

See [STABILITY.md](STABILITY.md) for the full tier classification of all public APIs.

---

## ADR Process

1. Identify that your change affects a fundamental design decision.
2. Copy `docs/src/architecture/decisions/001-blake3-cas.md` as a template.
3. Assign the next sequential ADR number.
4. Fill in: Title, Date, Status (`Proposed`), Context, Decision, Consequences.
5. Submit the ADR in the same PR as the implementation.
6. Once merged, update Status to `Accepted`.

---

## Maintainers

The current maintainer list is tracked in `.mailmap`. To become a maintainer, consistently contribute high-quality PRs for at least three months and request sponsorship from an existing maintainer.

---

## Code of Conduct

All contributors and maintainers are expected to follow the [Code of Conduct](CODE_OF_CONDUCT.md).

---

## License

arc is dual-licensed under MIT OR Apache-2.0. All contributions must be submitted under these same terms. By opening a PR you irrevocably agree to these license terms.
