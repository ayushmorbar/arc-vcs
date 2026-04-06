# Acknowledgements

Arc v0.1 was built as an original CRDT spacetime DAG implementation, but it was shaped by the standards set by exceptional open-source VCS projects and by foundational research across patch theory, distributed systems, semantic merging, and AI-assisted software engineering.

## Project Credits

### gitoxide (gix)

We acknowledge the gitoxide project for architectural inspiration across three areas that materially improved Arc’s v0.1 direction:

- the five-stage operation taxonomy (`discover`, `negotiate`, `transfer`, `materialize`, `finalize`),
- strict wasm boundary discipline between pure computation and platform I/O,
- rigorous CI, release, and reporting governance as engineering policy.

### jujutsu (jj)

We acknowledge Martin von Zweigbergk and the jujutsu contributors for first-class conflict UX, history operations, working-copy-as-a-commit workflows, and user-facing ergonomics that informed Arc’s interactive and operational design choices.

### sapling

We acknowledge sapling for practical snapshotting and workspace workflow ideas that informed Arc’s repository ergonomics and materialization strategy.

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

Arc’s CRDT spacetime DAG, identity model, replay semantics, and `Author::AI` integration model are original Arc implementations. The projects and research above served as technical foundations and quality benchmarks for architecture, correctness, maintainability, and open-source engineering rigor.its practical snapshotting and workspace workflow ideas that informed Arc's repository ergonomics and materialization strategy.
