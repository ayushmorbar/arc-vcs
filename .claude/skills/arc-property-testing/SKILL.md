---
name: arc-property-testing
description: >
  Strategy for testing mathematical invariants in arc-vcs using property-based
  testing. Use when writing tests for commutativity, CRDT merge correctness,
  CAS round-trips, DAG traversal, or any algebraic law in the codebase.
---

# arc-property-testing

## Purpose
Mathematical laws in `arc` — commutativity, invertibility, idempotency,
convergence — must be verified by property-based tests, not hand-written
examples. A single example proves nothing about the algebra.

## Tooling
- Use the `proptest` crate for all property-based tests.
- Use `proptest::arbitrary::Arbitrary` derive macros on core types
  (`Change`, `OpRecord`, `AstAtom`) to generate structured random inputs.

## Required Property Tests

### 1. Commutativity
For any two independent changes A and B:
```rust
proptest! {
    fn prop_commutative(a: Change, b: Change) {
        prop_assume!(independent(&a, &b));
        let s = arbitrary_state();
        prop_assert_eq!(
            apply(b.commute(&a), apply(a.clone(), s.clone())),
            apply(a.commute(&b), apply(b.clone(), s.clone()))
        );
    }
}
```

### 2. CAS Round-Trip
For any serializable object:
- Serialize → hash → write to CAS → read from CAS → deserialize.
- Assert the deserialized value equals the original.
- Assert the re-computed BLAKE3 hash equals the stored path.

### 3. DAG Frontier Convergence
For any two replicas that receive the same set of operations in any order:
- Assert their final computed Frontiers are identical.
- This is the core CRDT convergence guarantee.

### 4. Inverse Operations
For operations that support inversion:
- Assert `apply(invert(op), apply(op, sta