---
name: arc-docs
version: "1.0.0"
last_updated: "2026-04-07"
depends_on: []
conflicts_with: []
description: >
  Write, update, and maintain all arc-vcs documentation. Triggers on:
  README, CHANGELOG, CONTRIBUTING, SECURITY, ADRs, CLI reference, tutorials,
  concept guides, architecture docs, mdBook structure, SUMMARY.md, and any
  docs/ content. Also use for clarity improvements, adding examples,
  Diataxis restructuring, or any user-facing, contributor-facing, or
  agent-facing documentation task.
---


# arc-docs


## Purpose


This skill governs how documentation is written, structured, and maintained
across arc-vcs. All documentation must serve users at every skill level
simultaneously using progressive disclosure. The documentation set must be
comprehensive, accurate, and well-organized, with clear ownership and
synchronization rules to ensure it remains up-to-date as the code evolves.


We use `mdBook` as the canonical documentation engine located in `docs/`.
All documentation must be standard Markdown.
We strictly enforce the Diataxis framework and the Dual-Track (Human/Agent)
philosophy.


## Out of Scope


This skill does NOT:

- Modify, refactor, or generate source code in `src/`
- Create ADRs autonomously without a triggering code or schema change
- Run `just docs` or `just docs-serve` unless explicitly instructed by the user
- Make release commits or version bumps (that belongs to a release skill)
- Infer CLI flag behavior from source code — always use `--help` output as
  the source of truth for CLI reference pages
- Touch files outside `docs/`, `README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`,
  and `SECURITY.md` unless explicitly instructed


## Audience Tiers


Every page must serve at least one tier and never exclude another.
If `references/audience-matrix.md` is unavailable, treat the table below
as the authoritative fallback.


| Tier       | Who                                    | Primary need                           |
|------------|----------------------------------------|----------------------------------------|
| Newcomer   | First-time VCS user or arc evaluator   | Install, trust, first success          |
| Developer  | Working with arc daily                 | CLI reference, workflow recipes        |
| Contributor| Building arc itself                    | Architecture, axioms, test protocol    |
| Power User | Scripting, hooks, automation           | Internal APIs, extension points        |
| Enterprise | Compliance, deployment, audit          | Security, stability, SLAs              |


Use progressive disclosure: lead with the simplest path, layer in depth.
Never bury the beginner path under architecture theory.


## Doc Folder Layout


If `references/doc-structure.md` is unavailable, treat the layout below
as the authoritative fallback.


```text
arc-vcs/
├── README.md
├── CHANGELOG.md
├── CONTRIBUTING.md
├── SECURITY.md
└── docs/
    ├── book.toml
    └── src/
        ├── SUMMARY.md
        ├── introduction.md
        ├── learn/
        ├── guides/
        ├── concepts/
        ├── reference/
        └── agent-skills/
```


## Tooling


- **mdBook** is the canonical doc renderer for arc-vcs.
- All diagrams use **Mermaid** via the `mdbook-mermaid` preprocessor where useful.
- API reference is generated via `cargo doc`.
- CLI reference is generated, embedded, and must be derived from command `--help` output.
- Do not duplicate CLI flag truth in narrative prose when a reference page exists.
- Use `just docs` to build and `just docs-serve` for live-reload during editing.


## Diataxis Mapping


| Diataxis type  | Directory                    |
|----------------|------------------------------|
| Learn          | `docs/src/learn/*`           |
| Guides         | `docs/src/guides/*`          |
| Concepts       | `docs/src/concepts/*`        |
| Reference      | `docs/src/reference/*`       |
| Agent Skills   | `docs/src/agent-skills/*`    |


When creating or migrating pages, always include frontmatter keys:
`title`, `description`, `category`, `audience`.


## Dual-Track Policy


- Human path must remain complete and first-class.
- Agent path must remain complete and first-class.
- AI features are optional and must never block manual workflows.


## Page Templates


Use the templates in `references/templates/` when creating new pages:


- `adr.md` — Architecture Decision Records
- `cli-command.md` — CLI command reference pages
- `concept.md` — Concept explanation pages
- `tutorial.md` — Step-by-step tutorial pages


If a template file is missing, use the corresponding writing rules section
below as the inline fallback.


## Writing Rules


1. Lead with the outcome, not the theory.
2. Every concept page must have a one-sentence plain-English summary
   before any technical detail.
3. Every code block must be copy-paste runnable or labeled `# conceptual`.
4. Every diagram must have a text alternative summary directly below it.
5. No page should require reading another page first unless explicitly
   stated at the top with a link.
6. Use second-person active voice: "Run `arc snap`" not "The user runs".
7. Avoid jargon without an inline definition on first use.
8. Never write a wall of prose where a comparison table or code example
   would communicate faster.


## ADR Decision Logic


Create a new ADR when **any** of the following are true:

- A data model, schema, or storage format changes (including epoch bumps)
- A CLI command is removed or its semantics change irreversibly
- A core dependency is replaced rather than upgraded
- A new automation surface or agent skill is introduced
- A security boundary or trust model changes

Otherwise, update the relevant concept or reference page. Do not create
ADRs for purely cosmetic or docs-only changes.


File naming: `NNNN-short-title.md` where NNNN is zero-padded sequence.
Location: `docs/src/reference/architecture/decisions/`
Follow the ADR process defined in `GOVERNANCE.md`.


## Changelog Discipline


Follow Keep a Changelog (keepachangelog.com) format exactly:


- Sections: `Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, `Security`
- Latest version on top
- `[Unreleased]` section always present above versioned releases
- Link version numbers to diff URLs when the repo is public


## Sync Rules


When code changes, verify the corresponding docs before closing a task:


| Code change                          | Doc to verify                                              |
|--------------------------------------|------------------------------------------------------------|
| New CLI command or flag              | `docs/src/reference/cli-reference.md`                      |
| New concept or data type             | `docs/src/concepts/`                                       |
| Breaking change                      | `CHANGELOG.md` — `Removed` or `Changed` section           |
| New automation surface or skill      | `docs/src/agent-skills/`                                   |
| Schema / epoch bump                  | New ADR in `docs/src/reference/architecture/decisions/`    |
| Security fix                         | `SECURITY.md` and `CHANGELOG.md`                           |
| Any doc add / move / delete          | `docs/src/SUMMARY.md` must be updated in the same change   |


## Agent Output Contract


After completing any documentation task, output a structured completion
report in this exact format:


```
## Doc Task Complete

**Files created:** <list relative paths, or "none">
**Files modified:** <list relative paths, or "none">
**SUMMARY.md updated:** <yes / no / not required>
**Sync rules checked:** <list which rows from the sync table were verified>
**ADR created:** <yes (NNNN-title.md) / no>
**Confidence:** <high / medium / low — with reason if medium or low>
```

Always emit this block even if no files changed (state why).


## Uncertainty Protocol


If you cannot verify a fact (e.g., exact current CLI flags, current crate
version, whether a feature is implemented or only planned):

1. Do **not** invent or assume values.
2. Leave an inline HTML comment at the exact location:
   `<!-- VERIFY: <specific question> -->`
3. Add an entry under `[Unreleased]` in `CHANGELOG.md`:
   `- [ ] DOCS: Verify <topic> before publishing — ref: <task/issue if known>`
4. Set `Confidence: low` in the Agent Output Contract.

Never use placeholder text (`TODO`, `TBD`, `coming soon`) in published
documentation without a linked tracking issue.


## Anti-Patterns


- Never write documentation that only makes sense if you already understand
  arc internals. Always provide a plain-English anchor sentence.
- Never keep duplicated canonical truth across multiple pages. Use links.
- Never leave placeholder text (`TODO`, `TBD`, `coming soon`) visible
  in published documentation without a tracking issue link.
- Never document unimplemented features as if they exist. Use a clear
  admonition: `> **Note:** This feature is planned for 0.3.0.`
- Never write a wall of prose where a comparison table or code example
  would communicate faster.
- Never create an ADR for a docs-only change.
- Never update `SUMMARY.md` in a separate commit from the page it references.
```

***

## What Changed vs. Your Original

| Section | Change |
|---|---|
| Frontmatter | Added `version`, `last_updated`, `depends_on`, `conflicts_with` |
| `description` | Reordered for keyword-density, front-loaded trigger terms |
| **`Out of Scope`** | New — defines hard boundaries for agent behavior |
| Audience Tiers | Added inline fallback if `references/audience-matrix.md` is missing |
| Doc Folder Layout | Added inline fallback if `references/doc-structure.md` is missing |
| Page Templates | Added missing-template fallback instruction |
| **`ADR Decision Logic`** | New — tells the agent *when* to create an ADR vs. edit a page |
| **`Agent Output Contract`** | New — structured completion report format |
| **`Uncertainty Protocol`** | New — `<!-- VERIFY -->` pattern + Unreleased CHANGELOG entry |
| Anti-Patterns | Added two new entries (ADR misuse, SUMMARY.md split commits) |