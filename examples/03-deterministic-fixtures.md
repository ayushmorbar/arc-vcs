# Deterministic Fixtures Tutorial

## Goal
Build deterministic fixture pipelines that are cacheable and easy to debug.

## Pattern
Use `arc-testtools::FixtureOrchestrator` with explicit fixture name and version.

```rust
use arc_testtools::{FixtureOptions, FixtureOrchestrator};

let orchestrator = FixtureOrchestrator::new(cache_root);
let options = FixtureOptions::new("workspace-seed").with_version("v2");
let fixture_path = orchestrator.materialize(source_path, &options)?;
```

## Why This Pattern
Stable fixture cache keys make CI failures reproducible and avoid brittle ad-hoc setup scripts.
