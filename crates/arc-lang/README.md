# arc-lang

Language plugin layer for arc.

## Responsibilities

- Bridges generic `arc-core` semantic atoms with concrete language ASTs.
- Defines plugin contracts for parsing, diffing, and unparsing.
- Provides Rust implementation via tree-sitter.

## Current Implementation

- `ast/mod.rs`: `LanguagePlugin` trait and shared helpers.
- `ast/rust_plugin.rs`: Rust parser/unparser and interesting-node filtering.

## Extension Model

To add a language plugin:

1. Implement `LanguagePlugin`.
2. Define node filtering and path projection strategy.
3. Register plugin usage in command orchestration (`arc-cli`).

## Usage

```toml
[dependencies]
arc-lang = { path = "../arc-lang" }
```
