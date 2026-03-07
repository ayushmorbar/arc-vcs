# Security Policy

---

## Supported Versions

| Version | Supported |
|---------|----------|
| `main` (1.0.0-rc.1+) | ✓ Active security maintenance |
| `< 1.0.0-rc.1` | ✗ No security backports |

---

## Cryptographic Architecture

arc's security model is built on three pillars:

### 1. BLAKE3 Content-Addressed Storage
Every `Atom`, `Change`, and `Tag` is identified solely by its BLAKE3 hash. Tampering with any stored object changes its hash, making it unreferenceable from the graph — equivalent to **SLSA L4 build provenance** for every version control operation.

### 2. Ed25519 Per-Change Signatures
Every `Change` carries an Ed25519 signature from the author's keypair (stored in the OS config directory via the `directories` crate). Signature verification runs on every graph load. A change whose signature does not match its author's public key is rejected before it can affect the working directory.

### 3. Hook Sandbox (`.agentignore`)
The hook engine executes binaries configured in `.arc/config.json`. To mitigate supply-chain attacks:
- Hooks run in `work_root`, **not** in the `.arc` store directory.
- `shlex::split` is used for command parsing — no shell expansion, no glob injection.
- AI agents operating on this repository are constrained by `.agentignore` patterns that prevent modifying `.arc/config.json`, `justfile`, or `.github/` without explicit human approval.

---

## Reporting a Vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Report via:
1. **GitHub Private Vulnerability Reporting** — use the "Report a vulnerability" button on the Security tab (preferred).
2. **Email** — contact the maintainers listed in `crates/arc-cli/Cargo.toml` directly.

Include:
1. A clear description of the vulnerability and its potential impact.
2. Reproduction steps or a minimal proof-of-concept (if safe to share).
3. The arc version affected (`arc --version` or commit hash).
4. Any suggested mitigations.

We will acknowledge receipt within **72 hours** and provide a resolution timeline within **14 days** for confirmed vulnerabilities. Critical vulnerabilities (arbitrary code execution, signature bypass) are patched on an emergency basis.

---

## Vulnerability Scope

**In scope:**
- Arbitrary code execution via crafted CAS blobs or `Change` atoms from a remote
- Path traversal in `write_state_to_working_dir()` or the CAS blob writer
- Ed25519 signature bypass or downgrade attacks
- Hook injection via crafted `.arc/config.json` delivered by remote sync
- BLAKE3 collision exploitation (theoretical; report immediately if found)

**Out of scope:**
- Denial-of-service via extremely large repositories (operator responsibility)
- Vulnerabilities in third-party crates (report upstream; we will update and note in `CHANGELOG.md`)
- Social engineering attacks targeting contributors

---

## Dependency Auditing

Run `cargo audit` (install: `cargo install cargo-audit`) to check for known CVEs in the dependency tree. This check will be added to CI before the 1.0 stable release.
