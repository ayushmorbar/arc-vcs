# Environment Isolation Tutorial

## Goal
Prevent test cross-talk caused by global environment variable mutation.

## Pattern
Use `arc-testtools::EnvGuard` for scoped environment overrides.

```rust
use arc_testtools::EnvGuard;

{
    let _guard = EnvGuard::set("ARC_EXPERIMENT", "on");
    // test logic sees ARC_EXPERIMENT=on
}
// previous state restored automatically
```

## Why This Pattern
RAII restoration ensures tests are hermetic, even during early returns or panics.
