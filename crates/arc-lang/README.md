# arc-lang

Language plug-in layer for **arc**. Bridges generic `arc-core` atoms to concrete tree-sitter parse trees and provides the `LanguagePlugin` trait for any language parser.

## Crate layout

```
arc-lang
└── ast/
    ├── mod.rs           – LanguagePlugin trait, ASTNode ↔ Atom conversion helpers
    └── rust_plugin.rs   – RustPlugin: Rust language via tree-sitter-rust
```

## Adding a new language

1. Implement `LanguagePlugin` from `arc_lang::ast`.
2. Report the tree-sitter grammar in `node_is_interesting` to control granularity.
3. Register the plugin in `arc-cli`'s `snap` command.

## Usage

```toml
[dependencies]
arc-lang = { path = "../arc-lang" }
```

```rust
use arc_lang::ast::{LanguagePlugin, rust_plugin::RustPlugin};
let plugin = RustPlugin::new();
```
