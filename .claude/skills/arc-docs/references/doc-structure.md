# Authoritative Doc Structure

> **Version:** 1.1.0 — Updated 2026-03-28
> **Owner:** arc-docs skill (`arc-docs` v1.1.0+)
> **Source of truth for:** `## Doc folder layout` in all arc-docs skill files.
> If this file conflicts with any other document, this file wins.

arc-vcs uses a **codebase-wide docs-as-code model**. `mdBook` in `docs/` is the
canonical published documentation site, but documentation governance extends
across the whole repository. We separate:

- **Entrypoint documents** at repo root (README, CHANGELOG, CONTRIBUTING, SECURITY, GOVERNANCE)
- **Authored documentation** in `docs/src/` — human-curated, published pages
- **Generated reference artifacts** in `docs/generated/` — never edited manually
- **Documentation policy and reusables** in `references/` — templates, checklists, style, terminology
- **Documentation automation** in `scripts/` and `.github/workflows/`

All authored docs are standard Markdown. No `.mdx` files in the canonical tree.
We use `mdBook` as the canonical documentation engine.


## Canonical Tree

```text
arc-vcs/
├── README.md                          # Repo entry: trust, install, first success
├── CHANGELOG.md                       # Keep a Changelog format, always has [Unreleased]
├── CONTRIBUTING.md                    # Contribution workflow + dev setup entry point
├── SECURITY.md                        # Vuln reporting, security policy, contact
├── GOVERNANCE.md                      # ADR process, decision ownership, roles
│
├── docs/
│   ├── book.toml                      # mdBook config: title, preprocessors, output
│   ├── theme/                         # Custom mdBook theme overrides (optional)
│   ├── src/
│   │   ├── SUMMARY.md                 # Navigation manifest — every page must be linked here
│   │   ├── introduction.md            # Site entry: what arc is, one-sentence anchor
│   │   ├── quickstart.md              # Fastest path to first success (Newcomer tier)
│   │   │
│   │   ├── learn/                     # Diataxis: Tutorials — learning-oriented
│   │   │   ├── first-repo.md
│   │   │   ├── first-snapshot.md
│   │   │   └── first-restore.md
│   │   │
│   │   ├── guides/                    # Diataxis: How-to Guides — task-oriented
│   │   │   ├── migrate-from-git.md
│   │   │   ├── team-workflows.md
│   │   │   ├── hooks-automation.md
│   │   │   ├── disaster-recovery.md
│   │   │   └── enterprise-deployment.md
│   │   │
│   │   ├── concepts/                  # Diataxis: Explanation — understanding-oriented
│   │   │   ├── change-algebra.md
│   │   │   ├── snapshots.md
│   │   │   ├── crdts.md
│   │   │   ├── trust-model.md
│   │   │   ├── ai-native-workflows.md
│   │   │   └── glossary.md
│   │   │
│   │   ├── reference/                 # Diataxis: Reference — information-oriented
│   │   │   ├── cli/                   # Derived from --help output, NOT handwritten
│   │   │   │   ├── index.md
│   │   │   │   ├── arc.md
│   │   │   │   ├── arc-init.md
│   │   │   │   ├── arc-snap.md
│   │   │   │   └── arc-restore.md
│   │   │   ├── config/
│   │   │   │   ├── index.md
│   │   │   │   ├── repo-config.md
│   │   │   │   └── user-config.md
│   │   │   ├── formats/               # Object model, schema, epoch contracts
│   │   │   │   ├── object-model.md
│   │   │   │   ├── schema.md
│   │   │   │   └── epochs.md
│   │   │   ├── api/                   # Derived from cargo doc — NOT handwritten
│   │   │   │   └── index.md
│   │   │   └── architecture/
│   │   │       ├── overview.md
│   │   │       ├── invariants.md
│   │   │       ├── design/            # Design notes that are not formal ADRs
│   │   │       └── decisions/         # ADR log
│   │   │           ├── README.md      # ADR index + process summary
│   │   │           ├── 0001-core-object-model.md
│   │   │           └── 0002-epoch-versioning.md
│   │   │
│   │   ├── agent-skills/              # Agent-facing docs: dual-track first-class
│   │   │   ├── index.md
│   │   │   ├── arc-docs.md
│   │   │   ├── arc-code.md
│   │   │   └── arc-release.md
│   │   │
│   │   ├── contributor/               # Contributor onboarding and process
│   │   │   ├── development-setup.md
│   │   │   ├── testing.md
│   │   │   ├── docs-style.md
│   │   │   ├── release-process.md
│   │   │   └── decision-process.md
│   │   │
│   │   └── internal/                  # Maintainer-facing metadata, NOT published
│   │       ├── doc-inventory.md       # Live registry of all docs and owners
│   │       ├── redirects.md           # URL redirect map for moved pages
│   │       └── source-of-truth.md     # Defines single source of truth per topic
│   │
│   └── generated/                     # Auto-generated artifacts — DO NOT EDIT MANUALLY
│       ├── cli-help/                  # Output of scripts/generate-cli-docs.sh
│       └── rustdoc/                   # Output of cargo doc / scripts/sync-rustdoc.sh
│
├── references/                        # Reusable docs policy — not user-facing
│   ├── audience-matrix.md             # Full writing guidance per audience tier
│   ├── doc-structure.md               # THIS FILE
│   ├── style-guide.md                 # Tone, voice, formatting standards
│   ├── terminology.md                 # Canonical glossary of arc-specific terms
│   ├── templates/
│   │   ├── adr.md
│   │   ├── cli-command.md
│   │   ├── concept.md
│   │   ├── guide.md
│   │   └── tutorial.md
│   └── checklists/
│       ├── docs-pr-checklist.md       # Required review gates before merging doc PRs
│       ├── release-docs-checklist.md  # Docs tasks required before any release
│       └── adr-checklist.md           # Conditions that require a new ADR
│
├── scripts/
│   ├── generate-cli-docs.sh           # Runs arc --help variants → docs/generated/cli-help/
│   ├── check-doc-links.sh             # Validates internal and external links
│   └── sync-rustdoc.sh                # Runs cargo doc → docs/generated/rustdoc/
│
├── .github/
│   └── workflows/
│       ├── docs.yml                   # Build, lint, link check on every PR
│       ├── docs-preview.yml           # Deploy preview for doc-only PRs
│       └── link-check.yml             # Scheduled external link rot check
│
├── .vale.ini                          # Vale prose linter configuration
├── .markdownlint.json                 # Markdown structure linter configuration
└── Vale/
    ├── Styles/                        # arc style rules (word choice, banned phrases)
    └── vocabularies/                  # arc-specific terms accepted by Vale
```


## Structural Constraints

1. Only `introduction.md`, `quickstart.md`, and `SUMMARY.md` are allowed at
   `docs/src/` root. All other pages must live inside a Diataxis folder.
2. Every user-facing or agent-facing Markdown page must live in exactly one
   Diataxis folder (`learn/`, `guides/`, `concepts/`, `reference/`,
   `agent-skills/`) or in `contributor/` or `internal/`.
3. Every Markdown page under `docs/src/` (except `SUMMARY.md`) must be linked
   from `docs/src/SUMMARY.md`.
4. New files must be added to `SUMMARY.md` in the **same change**, not a
   follow-up commit.
5. `docs/generated/` must never be manually edited. It is produced by scripts
   and committed or served at build time only.
6. `reference/cli/` pages must derive from `--help` output via
   `scripts/generate-cli-docs.sh`. CLI flag truth must not be duplicated in
   narrative prose pages.
7. `reference/api/` must derive from `cargo doc` output. Do not maintain a
   hand-written API surface reference.
8. Pages in `internal/` are maintainer-only and must be excluded from
   `SUMMARY.md` (they are not part of the published mdBook site).


## Section Ownership

| Directory          | Primary tier                  | Audience that may skip      |
|--------------------|-------------------------------|------------------------------|
| `learn/`           | Newcomer                      | Contributor, Power User      |
| `guides/`          | Developer                     | —                            |
| `concepts/`        | Newcomer → Developer          | —                            |
| `reference/cli/`   | Developer, Power User         | Newcomer                     |
| `reference/api/`   | Contributor, Power User       | Newcomer, Developer          |
| `reference/formats/` | Contributor, Power User     | Newcomer                     |
| `reference/architecture/decisions/` | Contributor  | Newcomer, Developer          |
| `agent-skills/`    | Agent Integrator, Power User  | Newcomer                     |
| `contributor/`     | Contributor                   | Newcomer, Developer          |
| `internal/`        | Maintainer only               | All end users                |


## Source of Truth Rules

| Topic                     | Canonical source                              | What must NOT duplicate it         |
|---------------------------|-----------------------------------------------|-------------------------------------|
| CLI flags and commands     | `--help` output + `docs/generated/cli-help/`  | Narrative prose in `guides/`        |
| Rust API surface           | `cargo doc` + `docs/generated/rustdoc/`       | Hand-written `reference/api/` pages |
| Audience definitions       | `references/audience-matrix.md`               | Any skill or page re-defining tiers |
| Canonical terminology      | `references/terminology.md`                   | Any page inventing alternate terms  |
| ADR process                | `GOVERNANCE.md`                               | `contributor/decision-process.md` may link, not restate |
| Security policy            | `SECURITY.md`                                 | Any page inside `docs/src/`         |


## Validation Checklist

Run before merging any documentation change:

1. `just docs` (`mdbook build docs`) exits 0 with no warnings.
2. `scripts/check-doc-links.sh` reports no broken internal links.
3. `SUMMARY.md` link count equals Markdown file count in `docs/src/`
   (excluding `SUMMARY.md` and any files in `internal/`).
4. `vale docs/src/` reports no errors (warnings are advisory).
5. `markdownlint docs/src/` reports no errors.
6. No `.mdx` files remain anywhere in `docs/`.
7. No page in `docs/generated/` was manually edited (verify via `git diff`
   against last generation commit).
8. If any file in `docs/src/reference/cli/` was modified, confirm
   `scripts/generate-cli-docs.sh` was re-run in the same change.