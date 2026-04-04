# CI Integration (GitHub Actions and GitLab CI)

Status: Stable
Audience: Platform engineers and CI maintainers

This guide provides out-of-the-box CI templates for teams adopting `arc`.

## CI Philosophy in Arc

Arc is designed for fast repository operations (including zero-copy local materialization paths) and cryptographic provenance. In CI, treat provenance verification as mandatory.

Recommended order for every pipeline:

1. Install `arc`.
2. Bootstrap or restore the Arc repository state.
3. Run `arc verify` before any build or test step.
4. Execute project tests.

Why this order:

- `arc verify` validates graph cryptographic integrity before code execution.
- `Author::AI` changes are signed with the embedded `human_sponsor` key, and verification enforces this signature path.
- This supports supply-chain hardening goals (SLSA-style provenance controls) for enterprise CI.

## GitHub Actions Template

Create `.github/workflows/arc-ci.yml`:

```yaml
name: arc-ci

on:
  push:
    branches: ["**"]
  repository_dispatch:
    types: [arc-sync]

jobs:
  verify-and-test:
    runs-on: ubuntu-latest
    env:
      ARC_VERSION: v0.1.0
      ARC_RELEASE_ORG: REPLACE_ORG
      ARC_REMOTE_PATH: https://arc.example.com/my-org/my-repo
      ARC_VIEW: main

    steps:
      - name: Install dependencies
        run: |
          sudo apt-get update
          sudo apt-get install -y curl ca-certificates build-essential pkg-config libssl-dev

      - name: Install arc (release binary)
        run: |
          curl -fsSL -o arc.tar.gz "https://github.com/${ARC_RELEASE_ORG}/arc/releases/download/${ARC_VERSION}/arc-x86_64-unknown-linux-gnu.tar.gz"
          tar -xzf arc.tar.gz
          sudo install -m 0755 arc /usr/local/bin/arc
          arc --version

      # Alternative install path:
      # - name: Install arc (cargo)
      #   run: cargo install --locked arc-cli

      - name: Bootstrap Arc workspace from remote
        run: |
          mkdir repo
          cd repo
          arc init --no-git .
          arc remote add origin "${ARC_REMOTE_PATH}"
          arc pull origin "${ARC_VIEW}"

      - name: Verify cryptographic provenance (critical)
        working-directory: repo
        run: |
          arc verify

      - name: Run tests (Rust example)
        working-directory: repo
        run: |
          cargo test --all --locked

      # Example for Node projects:
      # - name: Run tests (Node example)
      #   working-directory: repo
      #   run: |
      #     npm ci
      #     npm test
```

Notes:

- `repository_dispatch` can be used as a custom webhook trigger when Arc-native sync does not directly emit GitHub push events (for example, when workflows depend on Git Bridge mirroring or an external event relay).
- Keep `arc verify` ahead of any script execution to fail fast on provenance violations.
- Set `ARC_RELEASE_ORG` to your release publisher and `ARC_REMOTE_PATH` to an Arc-native remote endpoint/path (not a Git clone URL).

## GitLab CI Template

Create `.gitlab-ci.yml`:

```yaml
stages:
  - verify
  - test

variables:
  ARC_VERSION: "v0.1.0"
  ARC_RELEASE_ORG: "REPLACE_ORG"
  ARC_REMOTE_PATH: "https://arc.example.com/my-org/my-repo"
  ARC_VIEW: "main"

default:
  image: rust:1.87
  before_script:
    - apt-get update
    - apt-get install -y curl ca-certificates
    - curl -fsSL -o arc.tar.gz "https://github.com/${ARC_RELEASE_ORG}/arc/releases/download/${ARC_VERSION}/arc-x86_64-unknown-linux-gnu.tar.gz"
    - tar -xzf arc.tar.gz
    - install -m 0755 arc /usr/local/bin/arc
    - arc --version

verify:
  stage: verify
  script:
    - mkdir repo
    - cd repo
    - arc init --no-git .
    - arc remote add origin "$ARC_REMOTE_PATH"
    - arc pull origin "$ARC_VIEW"
    - arc verify
  artifacts:
    paths:
      - repo/

test:
  stage: test
  needs: ["verify"]
  script:
    - cd repo
    - cargo test --all --locked

  # Node example:
  # script:
  #   - cd repo
  #   - npm ci
  #   - npm test
```

Notes:

- You can use a prebuilt internal image with `arc` already installed to reduce setup time.
- Replace `ARC_RELEASE_ORG` and `ARC_REMOTE_PATH` placeholders before use.

## Operational Recommendations

1. Pin the `arc` version in CI and update it on a controlled cadence.
2. Archive provenance logs/artifacts for failed `arc verify` runs.
3. Add branch protection rules that require the verification job to pass.
