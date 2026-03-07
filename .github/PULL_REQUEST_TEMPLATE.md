## Summary

<!-- A concise description of what this PR does and why -->

## Checklist

- [ ] `cargo test --workspace` passes locally
- [ ] `cargo clippy --all-targets -- -D warnings` passes with zero new warnings
- [ ] If this changes a public API: [STABILITY.md](STABILITY.md) tier for the affected API is declared in this PR description
- [ ] `tracing::info!` / `tracing::debug!` spans added where new operations warrant observability
- [ ] Documentation in `docs/` updated (if user-visible behaviour changed)
- [ ] ADR created in `docs/src/architecture/ADRs/` if this introduces or changes an architectural decision
- [ ] `CHANGELOG.md` updated under `[Unreleased]`

## STABILITY Tier

<!-- State which STABILITY tier(s) apply to the APIs changed or added in this PR -->
<!-- Tier 1 = stable public API | Tier 2 = stable but evolving | Tier 3 = experimental | Tier 4 = internal -->

## Testing

<!-- How was this tested? Unit tests, integration tests, manual steps? -->

## Breaking Changes

<!-- List any breaking changes. If none, write "None." -->
