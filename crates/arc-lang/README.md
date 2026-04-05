# arc-lang

![crate](https://img.shields.io/badge/crate-arc--lang-blue)
![role](https://img.shields.io/badge/role-language%20plugin-4c8)

## BLUF

`arc-lang` provides language-plugin contracts and implementations for AST-aware diffing and reconstruction. It converts language syntax trees into typed atom streams consumed by the semantic engine.

## Architectural Role (The DAG)

- Depends on: `arc-algebra-types`, `arc-store-cas`, parser crates such as `tree-sitter`.
- Depended on by: `arc-cli`.
- Position: language adaptation layer between source text and arc atom algebra.

## Purity & I/O Boundary

`arc-lang` is **compute-dominant with delegated CAS access**.

- Parses and diffs syntax trees in-process.
- Does not perform network I/O.
- Uses `ObjectStore` provided by caller for blob-level interactions.

## Key Types/Exports

- `ast::LanguagePlugin`
- `ast::rust_plugin::RustPlugin`

```rust
use arc_lang::ast::{LanguagePlugin, rust_plugin::RustPlugin};

let plugin = RustPlugin::new();
let _tree = plugin.parse("fn main() {}")?;
# Ok::<(), String>(())
```
