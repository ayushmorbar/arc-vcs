# .claude Customization Index (arc-vcs)

This file documents the purpose of every major file/folder under `.claude` so
contributors can reason about customization behavior without reverse-engineering
prompt internals.

## Top-level files

| Path            | Why it exists                         | Operational role                                                |
| --------------- | ------------------------------------- | --------------------------------------------------------------- |
| `CLAUDE.md`     | Defines always-on repository behavior | Baseline agent guardrails (purity, DAG model, validation order) |
| `settings.json` | Local automation ergonomics           | Auto-approve safe build/test commands and custom lint command   |

## `agents/`

| Path                 | Why it exists                | Operational role                                                     |
| -------------------- | ---------------------------- | -------------------------------------------------------------------- |
| `agents/reviewer.md` | Standardizes review behavior | Enforces severity-first findings and arc invariants in review passes |

## `commands/`

| Path                 | Why it exists              | Operational role                                          |
| -------------------- | -------------------------- | --------------------------------------------------------- |
| `commands/review.md` | Reusable review prompt     | Audits uncommitted changes against core axioms            |
| `commands/test.md`   | Reusable validation prompt | Runs repository validation pipeline and explains failures |

## `skills/` (domain workflow modules)

| Skill                  | Why it exists                       | Use when                                                                  |
| ---------------------- | ----------------------------------- | ------------------------------------------------------------------------- |
| `arc-cas-storage`      | Protects CAS invariants             | Reading/writing objects, hashing, storage paths, serialization boundaries |
| `arc-causal-graph`     | Preserves DAG causality model       | Frontier logic, concurrent state, CRDT reconciliation                     |
| `arc-docs`             | Standardizes documentation quality  | Writing/migrating docs, templates, audience-aware structure               |
| `arc-ghost-nodes`      | Governs provisional AI snapshots    | Agent checkpoints, sponsorship flows, non-stable transitions              |
| `arc-git-commit`       | Ensures high-signal commit messages | Conventional commits with architectural intent                            |
| `arc-intent-reasoning` | Decomposes large changes safely     | Splitting refactors into independent semantic intents                     |
| `arc-patch-theory`     | Defines semantic-change algebra     | Commutativity, replay, inversion, AST-native operations                   |
| `arc-property-testing` | Validates algebraic laws            | Property tests for commutativity/convergence/invertibility                |
| `arc-redb-indexes`     | Protects metadata-index discipline  | Redb schemas/transactions/table typing and separation from CAS blobs      |
| `arc-semantic-query`   | Formalizes intent retrieval         | Semantic history lookup and rationale ranking                             |
| `arc-semver-policy`    | Governs compatibility decisions     | Version bumps, schema/network epoch implications                          |
| `arc-sync-protocol`    | Normalizes replica synchronization  | 5-stage discover->finalize sync pipeline                                  |
| `arc-tree-sitter`      | Enforces AST parsing idioms         | Tree-sitter lifetimes, query strategy, byte-range semantics               |
| `semver`               | Generic SemVer reference            | Validation of version strings and precedence rules                        |

## Maintenance rules

1. Keep `CLAUDE.md`, `.github/copilot-instructions.md`, and `.github/instructions/*.md` semantically aligned.
2. If a skill changes behavior assumptions, update this index and the skill's `description` trigger text.
3. Treat `arc-docs` references as migration-sensitive: keep both `docs/src/` (legacy) and `docs/next/` (target) accurate.
4. Empty customization files are considered misconfiguration and should be filled or removed.
