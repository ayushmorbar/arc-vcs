---
name: arc-docs
description: >
  Maintain, create, and update all documentation for arc-vcs. Use when writing
  or updating README files, concept guides, CLI references, architecture
  decision records (ADRs), changelogs, tutorials, contributing guides, or any
  user-facing or contributor-facing documentation. Also use when asked to
  improve docs clarity, add examples, or restructure the docs folder.
---

# arc-docs

## Purpose

This skill governs how documentation is written, structured, and maintained
across arc-vcs. All documentation must serve users at every skill level
simultaneously using progressive disclosure.

## Audience tiers

Every page must serve at least one tier and never exclude another.
See `references/audience-matrix.md` for full writing guidance per tier.

| Tier | Who | Primary need |
|---|---|---|
| Newcomer | First-time VCS user or arc evaluator | Install, trust, first success |
| Developer | Working with arc daily | CLI reference, workflow recipes |
| Contributor | Building arc itself | Architecture, axioms, test protocol |
| Power User | Scripting, hooks, automation | Internal APIs, extension points |
| Enterprise | Compliance, deployment, audit | Security, stability, SLAs |

Use progressive disclosure: lead with the simplest path, layer in depth.
Never bury the getting-started path under architecture theory.

## Doc folder layout

See `references/doc-structure.md` for the authoritative directory tree.
The canonical layout is:

```
arc-vcs/
├── README.md                      # Newcomer entry point
├── CHANGELOG.md                   # Keep a Changelog format
├── CONTRIBUTING.md                # Contributor quick-start
├── SECURITY.md                    # Vulnerability disclosure policy
└── docs/
    ├── book.toml                  # mdBook config
    └── src/
        ├── SUMMARY.md             # mdBook navigation
        ├── introduction.md        # What arc is and is not
        ├── getting-started/       # Newcomer tier
        ├── concepts/              # Mental model building
        ├── user-guide/            # Developer tier daily reference
        ├── architecture/          # Contributor tier deep dives
        │   └── decisions/         # ADRs
        └── contributing/          # Contributor setup and axioms
```

## Tooling

- **mdBook** is the canonical doc renderer for arc-vcs.
- All diagrams use **Mermaid** via the `mdbook-mermaid` preprocessor.
- API reference is generated via `cargo doc`.
- CLI reference is generated and embedded from command `--help` output.
- Do not manually duplicate CLI flags in narrative prose; link to the
  generated reference.

## Page templates

Use the templates in `references/templates/` when creating new pages:

- `adr.md` for Architecture Decision Records
- `cli-command.md` for new CLI command reference pages
- `concept.md` for new concept explanation pages
- `tutorial.md` for step-by-step how-to guides

## Writing rules

1. Lead with the outcome, not the theory.
2. Every concept page must have a one-sentence plain-English summary
   before any technical detail.
3. Every code block must be copy-paste runnable or labeled `# conceptual`.
4. Every diagram must have a text alternative summary below it.
5. No page should require reading another page first unless explicitly
   stated at the top with a link.
6. Use second-person active voice: "Run `arc snap`" not "The user runs".
7. Avoid jargon without inline definition on first use.

## Changelog discipline

Follow Keep a Changelog (keepachangelog.com) format exactly:

- Sections: Added, Changed, Deprecated, Removed, Fixed, Security
- Latest version on top
- Unreleased section always present above versioned releases
- Link version numbers to diff URLs when the repo is public

## ADR discipline

Every significant architectural decision gets an ADR in
`docs/src/architecture/decisions/`.
File naming: `NNNN-short-title.md` where NNNN is a zero-padded sequence.
Use the template at `references/templates/adr.md`.

## Sync rules

When code changes, check these docs for staleness:

| Code change | Doc to verify |
|---|---|
| New CLI command or flag | `docs/src/user-guide/cli-reference.md` |
| New concept or data type | `docs/src/concepts/` |
| Breaking change | `CHANGELOG.md` Removed or Changed section |
| Schema / epoch bump | `docs/src/architecture/decisions/` new ADR |
| New crate or crate rename | `docs/src/architecture/crate-map.md` |
| Security fix | `SECURITY.md` and `CHANGELOG.md` Security section |

## Anti-patterns

- Never write documentation that only makes sense if you already understand
  arc internals. Always provide a plain-English anchor sentence.
- Never copy-paste the same content into two pages. Use links.
- Never leave placeholder text (`TODO`, `TBD`, `coming soon`) visible
  in published documentation without a tracking issue link.
- Never document unimplemented features as if they exist. Use a clear
  admonition: `> Note: This feature is planned for 0.3.0.`
- Never write a wall of prose where a comparison table or code example
  would communicate faster.
