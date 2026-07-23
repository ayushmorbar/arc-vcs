# Release Checklist

Follow this checklist for every `arc` release.

## Pre-release

- [ ] All CI jobs green on `main` (lint, build, test, doctests, policy, api-drift, arch-drift, supply-chain, fuzz, beta, no-default-features, miri, coverage, bench)
- [ ] `just verify` passes locally
- [ ] No open P0/P1 issues blocking release
- [ ] `CHANGELOG.md` updated (or auto-generated via `scripts/ci/generate-semantic-changelog.sh`)

## Tag & push

```bash
# Ensure tag is unique
bash scripts/ci/enforce-unique-tag.sh v1.X.Y

# Create annotated tag
git tag -a v1.X.Y -m "Release v1.X.Y"

# Push tag (triggers release workflow)
git push origin v1.X.Y
```

## Release workflow (automated)

The `release.yml` workflow runs automatically on tag push:

1. **Preflight** — tag uniqueness, package size checks
2. **Build** — 5 targets: linux-gnu, linux-musl, macOS x86, macOS arm, Windows
3. **Checksums** — SHA256SUMS.txt per target
4. **Cosign signing** — keyless OIDC signatures (.sig + .cert per artifact)
5. **SLSA provenance** — v3 provenance attestation via `slsa-github-generator`
6. **GitHub Release** — draft release with changelog and all artifacts

## Post-release

- [ ] Verify draft release on GitHub has all 5 target artifacts + signatures + provenance
- [ ] Review auto-generated changelog for accuracy
- [ ] Mark release as **latest** (undraft)
- [ ] Announce release (if applicable)

## Rollback

If a release has a critical issue:

1. **Do not delete the tag** — instead, create a hotfix branch
2. Bump version in `Cargo.toml` (patch increment)
3. Merge hotfix to `main`
4. Tag the new patch release

## Verification commands

```bash
# Verify checksums
sha256sum -c SHA256SUMS.txt

# Verify cosign signatures (requires cosign)
cosign verify-blob --signature arc.sig --certificate arc.cert \
  --certificate-identity-regexp 'https://github.com/.*/arc-vcs' \
  --certificate-oidc-issuer 'https://token.actions.githubusercontent.com' \
  arc
```
