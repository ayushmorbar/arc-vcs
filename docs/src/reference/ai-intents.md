---
title: Ai Intents
description: Documentation page for Ai Intents.
---

# AI Intents & Resolution

arc provides a pluggable AI resolution layer for handling semantic conflicts that cannot be automatically merged. This page documents the `AiResolver` trait, the `MockResolver` for testing, and the full resolution workflow.

---

## Why AI Resolution?

When `commutes(a, b)` returns `false`, two changes have competed for the same AST structural node. The algebraic model cannot automatically choose a winner — both choices are mathematically valid representations of the intent on each side. A human (or an AI reasoning about code intent) must decide.

arc externalises this decision through the `AiResolver` trait, keeping the core algebra pure and deterministic.

---

## The `AiResolver` Trait

```rust
pub trait AiResolver {
    fn resolve(
        &self,
        base: &str,           // LCA content at the conflicting path
        ours: &str,           // content from the current view's delta
        theirs: &str,         // content from the target view's delta
        intent_ours: &str,    // commit message of our conflicting Change
        intent_theirs: &str,  // commit message of their conflicting Change
    ) -> anyhow::Result<String>;  // the resolved content
}
```

The resolver is called once per conflicting `(Change_A, Change_B)` pair at the overlapping path. It receives the LCA base content, both sides' content, and the human-readable commit messages (the "intents") of the conflicting changes.

The returned string is committed as a new merge `Change` that advances the view.

---

## `MockResolver`

`MockResolver` is a deterministic test resolver bundled with arc. It always chooses "ours" (the current view's content) and appends a comment marking the resolution:

```rust
pub struct MockResolver;

impl AiResolver for MockResolver {
    fn resolve(&self, _base: &str, ours: &str, ...) -> anyhow::Result<String> {
        Ok(format!("{ours}\n// [mock-resolved]"))
    }
}
```

Use `MockResolver` in tests and CI. It requires no API key and is side-effect-free.

---

## The Resolution Workflow

1. `arc merge <view>` detects one or more non-commuting change pairs.
2. arc serializes the `PendingConflict` struct to `.arc/conflict`:
   ```json
   {
     "current_view": "main",
     "target_heads": ["<hash>"],
     "conflicting_pairs": [["<hash_a>", "<hash_b>"]]
   }
   ```
3. arc aborts with an error reporting the first conflicting pair's hex IDs.
4. The user calls `arc resolve`.
5. `arc resolve` reads `.arc/conflict`, invokes the configured `AiResolver` for each pair, and commits the resolved content as a new `Change`.
6. `.arc/conflict` is deleted. The view advances.

---

## Configuring a Production Resolver

The production AI resolver is not bundled with arc (see [SHORTCOMINGS.md](../../SHORTCOMINGS.md)). To wire up your own:

1. Implement the `AiResolver` trait in a Rust library.
2. Pass the resolver instance to `Repository::resolve_conflict(&mut self, resolver: &dyn AiResolver)`.
3. Set your LLM API credentials via environment variables before calling `arc resolve`.

A reference integration with OpenAI-compatible APIs is planned for arc 1.1.

---

## Security Considerations

- The resolver receives file content and commit messages. Do not use a production resolver with a public API endpoint when working on proprietary code without reviewing the resolver's data handling policy.
- `MockResolver` never sends data anywhere and is safe for all environments.
