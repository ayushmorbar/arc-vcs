# Acknowledgements

Arc v0.1 was built as an original CRDT spacetime DAG implementation, but it was shaped by the standards set by exceptional open-source version-control systems projects and by foundational research across patch theory, distributed systems, semantic merging, and AI-assisted software engineering.

## Core Project Credits

### gitoxide (gix)

arc acknowledges gitoxide for engineering inspiration in:

- stage-oriented operation modeling (`discover`, `negotiate`, `transfer`, `materialize`, `finalize`),
- clear boundaries between pure computation and platform I/O,
- disciplined CI and release governance practices.

### jujutsu (jj)

arc acknowledges jujutsu for influential ideas in:

- conflict UX and history workflows,
- working-copy-centered operations,
- day-to-day ergonomics for developers navigating complex repositories.

### sapling
arc acknowledges Sapling for practical lessons in:

- snapshot-oriented workflows,
- large-repository usability,
- workspace materialization ergonomics.

## Literature & Research Influences

### 1) Patch Theory & Mathematical Change Algebra

- **David Roundy** — original Patch Theory and Darcs; first-class, commutable changes.
- **Judah Jacobson** — formalization of Patch Theory with inverse semigroups.
- **Jason Dagit** — analysis and modeling of Darcs patch theory and type-correct change systems.
- **The Pijul Team** — scalable advances that addressed early Patch Theory conflict complexity.

### 2) CRDTs & Collaborative Replication

- **Marc Shapiro, Nuno Preguiça, Carlos Baquero, Marek Zawirski** — foundational CRDT semantics.
- **Matthew Weidner, Heather Miller, and the Collabs Team** — composable and performant CRDT implementation frameworks.
- **Martin Kleppmann** — local-first software, JSON CRDTs, and interleaving anomaly analysis.
- **Shadaj Laddad et al.** — Katara research on CRDT synthesis and verification.

### 3) AST Differencing & Semantic Merging

- **Pouria Alikhanifard, Nikolaos Tsantalis** — RefactoringMiner 3.0 and semantic-aware AST differencing.
- **Sebastian Burckhardt, Jedidiah McClurg, Michał Moskal** — AST-based collaborative editing and correctness-preserving merge strategies.
- **Jean-Rémy Falleri et al.** — GumTree AST differencing.
- **Sven Apel, Jörg Liebig, Christian Lengauer** — semistructured merge research.

### 4) Semantic Conflict Detection & Versioning

- **Luís Carvalho, João Costa Seco** — deep semantic versioning and type-safe merge reasoning.
- **Martín Dias, Guillermo Polito, Damien Cassou, Stéphane Ducasse** — DELTAIMPACTFINDER and change-impact-based semantic conflict detection.
- **Galileu Santos de Jesus, Paulo Borba, et al.** — semantic conflict detection via static data-flow and dependence analysis.
- **Susan Horwitz, Jan Prins, Thomas Reps** — foundational PDG-based noninterference criteria for integrating program versions.

### 5) AI-Native Code Generation & Merging

- **Elizabeth Dinella, Todd Mytkowicz, Alexey Svyatkovskiy, Christian Bird, Mayur Naik, Shuvendu Lahiri** — DeepMerge and data-driven program merging.
- **Andrej Karpathy** — articulation of “vibe coding,” reflected in Arc’s separation of human intent and AI implementation paths.

## Statement of Originality

arc's CRDT spacetime DAG model, replay semantics, identity model, and `Author::AI` integration are original arc implementations. The projects above informed quality standards and workflow design, not direct code transplantation.
