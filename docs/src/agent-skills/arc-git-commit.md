---
name: arc-git-commit
description: >
  Write precise commit messages for the arc-vcs repository. Use when the user
  asks to write a commit message, summarize a diff into a commit, produce a
  Conventional Commit, or explain the architectural intent of a change in arc.
---

# arc-git-commit

## Purpose

Produce a single, high-signal commit message for `arc-vcs` that explains the
architectural intent of the change, not just the edited files.

## Output contract

Return either:

1. a commit message only, or
2. a complete `git commit -m` shell command,

depending on the user's request.

If the user does not specify, prefer the commit message only.

## Required format

Use Conventional Commits:

`<type>(<optional-scope>): <subject>`

Rules:

- Subject line is imperative.
- Subject line is at most 50 characters.
- No trailing period in the subject.
- Leave one blank line after the subject.
- Wrap body lines to about 72 characters.
- Focus on architectural intent and observable effect.
- Never fabricate tests, counts, versions, or commands.

## Prefix policy

Choose the primary value delivered:

- `feat` — new user-facing capability, command, API surface, or workflow
- `fix` — broken behavior corrected
- `perf` — measurable speed, memory, mmap, hashing, or parsing improvement
- `refactor` — structural change without behavior change
- `test` — tests only
- `docs` — docs only
- `style` — formatting only
- `chore` — tooling, CI, metadata, or dependency maintenance

Priority when several fit:

`feat > fix > perf > refactor > test > docs > style > chore`

## arc-specific requirements

- If touching pure crates, state whether purity is preserved.
- If touching serialized types, mention schema or migration implications.
- If touching network compatibility, mention epoch impact.
- If touching semantic operations, mention AST or patch-theory intent.
- Never describe semantic changes as line-based diffs.

## Scope guidance

Use a scope only when it meaningfully improves clarity:

- `core`
- `cli`
- `cas`
- `graph`
- `sync`
- `lsp`
- `daemon`
- `ci`
- `deps`

Omit scope when the change spans the workspace.

## Recommended body structure

Paragraph 1:
- Why this change exists.
- What system constraint, bug, or architectural need it addresses.

Optional themed bullets:
- Group by subsystem or behavior, not by filename.
- Mention user-visible CLI/API changes exactly.
- Mention migrations or epoch consequences when relevant.

Optional quality block:
Use only facts known from the input.

Example:

```text
feat(cli): add provisional snap sponsorship flow

Introduce a reviewable path for agent-created snapshots so autonomous work can
be recorded without entering stable history before explicit approval.

**Ghost snapshots**
- Add provisional snapshot creation with provenance metadata
- Keep generated changes out of stable history until sponsorship succeeds

**Approval flow**
- Preserve a deterministic transition from ghost to stable history
- Keep rollback and audit boundaries explicit

**Quality**
- cargo test -p arc-cli passed
- No new I/O introduced in pure crates
```

## Anti-patterns

Never produce:

- vague summaries like “misc improvements”
- phase or sprint labels
- file lists as section headers
- fake test counts
- passive voice when a precise imperative verb is available
- claims about semantics or compatibility not supported by the change