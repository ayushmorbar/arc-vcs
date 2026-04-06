# syntax=docker/dockerfile:1
# ^ Enables BuildKit features: --mount=type=cache, heredocs, etc.

ARG VARIANT="bookworm"
FROM mcr.microsoft.com/devcontainers/rust:1-${VARIANT}

# ── System Dependencies ───────────────────────────────────────────────────────
# Installed as root before switching to vscode user.
# Each logical group is a separate layer for better cache reuse.

# Build essentials + SSL (needed by many -sys crates)
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# mold — the fastest available linker (replaces lld, gold, ld.bfd)
# Dramatically reduces incremental link time inside the container
RUN apt-get update && apt-get install -y --no-install-recommends \
    mold \
    && rm -rf /var/lib/apt/lists/*

# jq — required by the justfile `query-meta` helper
# (reads cargo metadata JSON for `just msrv`, `just dbg`, etc.)
RUN apt-get update && apt-get install -y --no-install-recommends \
    jq \
    && rm -rf /var/lib/apt/lists/*

# mdBook dependencies (needed if building docs inside container)
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl \
    git \
    && rm -rf /var/lib/apt/lists/*

# ── Rust Toolchains ────────────────────────────────────────────────────────────
# Switch to vscode user for all rustup/cargo operations so tools land in the
# correct user profile and not in root's home.
USER vscode

# Stable is pre-installed by the base image.
# Install nightly with the components your project actually needs:
#   rustfmt  → cargo +nightly fmt (rustfmt.toml unstable_features = true)
#   clippy   → optional nightly lints
#   rust-src → needed by rust-analyzer for std goto-definition
RUN rustup toolchain install nightly \
    --component rustfmt clippy rust-src \
    --allow-downgrade

# ── Cargo Tools ───────────────────────────────────────────────────────────────
# Use BuildKit cache mount so these layers survive `docker build --no-cache`
# at the Dockerfile level (cache is stored outside the image layer).
# --locked: pins to the exact versions in each tool's Cargo.lock for reproducibility.
RUN --mount=type=cache,target=/usr/local/cargo/registry,uid=1000 \
    --mount=type=cache,target=/usr/local/cargo/git,uid=1000 \
    cargo install --locked \
        cargo-nextest \
        cargo-deny \
        cargo-audit \
        just \
        mdbook

# ── Cargo Config: use mold as default linker ──────────────────────────────────
# Written to the vscode user's global cargo config so every project in the
# container benefits automatically, without touching per-project .cargo/config.toml
RUN mkdir -p /home/vscode/.cargo && cat >> /home/vscode/.cargo/config.toml <<'EOF'

[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]

[target.aarch64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
EOF