# Audience Matrix

## How to write for each tier

### Newcomer
Goal: First successful command within 5 minutes.
- Start with "what problem does this solve" in one sentence.
- Provide a copy-paste install block immediately.
- Use Git analogies where helpful, but immediately explain why arc differs.
- Avoid all acronyms (CRDT, CAS, DAG) in the first two pages. Introduce
  them with a one-line definition when they first appear.
- Every step has expected output shown.
- Failure modes show a recovery path, not just an error message.

### Developer
Goal: Fast lookup. No re-reading of theory.
- Organize by task, not by concept: "How do I revert a change?" not
  "The Revert Operation".
- Every command page follows the cli-command template exactly so
  location of flags is predictable.
- Show realistic examples, not toy strings. Use a real-looking project.
- Link to concept pages for depth; don't embed theory in the reference.

### Contributor
Goal: Architectural confidence before touching code.
- Lead with invariants and axioms, not file names.
- Explain the mathematical model before the Rust types.
- Show what a correct change looks like AND what a wrong change looks like.
- Every axiom has a name so it can be referenced in code review.

### Power User
Goal: Extend and automate arc without reading source code.
- Document hook entry points, environment variables, and exit codes.
- Provide machine-readable output formats (`--output json`).
- Show shell scripting examples with real pipelines.
- Document what is stable (public API) vs. unstable (internal).

### Enterprise
Goal: Risk assessment and integration confidence.
- Security model: what arc can and cannot access.
- Upgrade policy: what constitutes a breaking change.
- Data durability: what the BLAKE3 CAS guarantees.
- Audit trail: what the DAG records and what it does not.
- Compliance: reproducibility, immutability, and provenance guarantees.
