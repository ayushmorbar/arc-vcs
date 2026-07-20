use alloc::string::{String, ToString};

use serde::{Deserialize, Serialize};

/// A 32-byte Ed25519 public key.
pub type PublicKeyBytes = [u8; 32];

/// The identity of a change's author, carried inside every `Change`.
///
/// This enum is part of the BLAKE3 hash payload, so changing the author
/// of a change changes its content-addressed identity.
///
/// # AI Governance Rule
///
/// `Author::AI` embeds the `human_sponsor`'s public key rather than its
/// own. Signature verification always runs against the human sponsor's key,
/// which means AI agents cannot authorize their own code — a human must
/// cryptographically vouch for every AI-authored change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Author {
    /// A human developer identified by name, email, and Ed25519 public key.
    Human {
        /// Full display name.
        name: String,
        /// Email address.
        email: String,
        /// Ed25519 public key bytes.
        key: PublicKeyBytes,
    },
    /// An AI agent identified by its model name plus the public key of the
    /// human sponsor who approved the change.
    AI {
        /// Model identifier (e.g. `"claude-3-opus"`).
        model: String,
        /// The human sponsor's Ed25519 public key.
        human_sponsor: PublicKeyBytes,
    },
    /// The arc server, acting as a canonical-identity issuer.
    ///
    /// Used in Dual-Provenance Identity Collapsing (Phase 39): the server
    /// re-signs transient-author Changes under its own Ed25519 key, folding
    /// ephemeral CRDT replica identities into a stable canonical identity.
    /// The original Change's BLAKE3 hash is preserved in
    /// `Change::collapsed_from` so auditors can reconstruct pre-collapse
    /// history (SLSA L4 audit trail).
    Server {
        /// Stable logical name for this server (e.g. `"arc-server"`).
        canonical_id: String,
        /// Ed25519 public key bytes for the server identity.
        key: PublicKeyBytes,
    },
    /// An ephemeral session identity for CI/CD runners and AI agents.
    ///
    /// `Transient` identities are provisioned automatically when the
    /// `ARC_EPHEMERAL_RUNNER` environment variable is set, and are
    /// persisted for the duration of a single workspace session in
    /// `.arc/local/session.json`.  They are cryptographically first-class:
    /// the holder signs with a real Ed25519 key and the server verifies the
    /// signature before executing Identity Collapsing (Phase 39).
    ///
    /// This makes the collapse trigger a strict type-system match rather
    /// than brittle string heuristics, satisfying the Phase 40 security
    /// requirement.
    Transient {
        /// Session identifier, e.g. `"ci-runner-42"` or a process-scoped UUID.
        session_id: String,
        /// Ed25519 public key bytes for this ephemeral session.
        key: PublicKeyBytes,
    },
}

/// An Ed25519 signature over a change's BLAKE3 id.
///
/// The signature is computed as `signing_key.sign(change.id)`.
/// It is intentionally excluded from the hash payload so that re-signing
/// with a different key does not change the content-addressed identity.
///
/// Manual `Serialize`/`Deserialize` are implemented because serde's derive
/// only covers fixed-size arrays up to `[u8; 32]`, and an Ed25519 signature
/// is 64 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature(pub [u8; 64]);

impl serde::Serialize for Signature {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeTuple;
        let mut tup = s.serialize_tuple(64)?;
        for b in &self.0 {
            tup.serialize_element(b)?;
        }
        tup.end()
    }
}

impl<'de> serde::Deserialize<'de> for Signature {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Vis;

        impl<'de> serde::de::Visitor<'de> for Vis {
            type Value = Signature;

            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "a 64-byte Ed25519 signature")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut bytes = [0u8; 64];
                for (i, b) in bytes.iter_mut().enumerate() {
                    *b = seq
                        .next_element()?
                        .ok_or_else(|| serde::de::Error::invalid_length(i, &self))?;
                }
                Ok(Signature(bytes))
            }
        }

        d.deserialize_tuple(64, Vis)
    }
}

/// Construct a deterministic test keypair (fixed seed `[42u8; 32]`).
///
/// Only compiled in test builds. Used by all test modules to produce a
/// consistent [`Author`] + [`ed25519_dalek::SigningKey`] pair without
/// touching the OS random-number generator.
pub fn test_keypair() -> (Author, ed25519_dalek::SigningKey) {
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
    let key: PublicKeyBytes = signing_key.verifying_key().to_bytes();
    let author =
        Author::Human { name: "Test User".to_string(), email: "test@example.com".to_string(), key };
    (author, signing_key)
}

/// Generate a fresh `Author::Transient` identity + raw 32-byte signing key seed.
///
/// Returns `(Author::Transient { session_id, key }, seed_bytes)`.  The seed
/// bytes should be persisted in `.arc/local/session.json` (scoped to the
/// current workspace) so the same ephemeral key is reused for the lifetime
/// of the session.  A new key is generated on the next `arc init` or when
/// `ARC_EPHEMERAL_RUNNER` triggers a fresh workspace.
///
/// Uses the OS CSPRNG — forward-secure on all supported platforms.
#[cfg(feature = "std")]
pub fn generate_transient_keypair_seed(session_id: &str) -> (Author, [u8; 32]) {
    use rand_core::OsRng;
    let signing_key = ed25519_dalek::SigningKey::generate(&mut OsRng);
    let key: PublicKeyBytes = signing_key.verifying_key().to_bytes();
    let author = Author::Transient { session_id: session_id.to_string(), key };
    (author, signing_key.to_bytes())
}

/// Generate a fresh `Author::Server` identity + raw 32-byte signing key seed.
///
/// Returns `(Author::Server { canonical_id, key }, seed_bytes)`.  The seed
/// bytes should be persisted securely (e.g. in `.arc/server_identity.json`)
/// so the server uses the same signing key across restarts.  Only the seed
/// is stored; the public key is always re-derived on load.
///
/// Uses the OS CSPRNG via `rand_core::OsRng` — forward-secure on all
/// supported platforms.
#[cfg(feature = "std")]
pub fn generate_server_keypair_seed(canonical_id: &str) -> (Author, [u8; 32]) {
    use rand_core::OsRng;
    let signing_key = ed25519_dalek::SigningKey::generate(&mut OsRng);
    let key: PublicKeyBytes = signing_key.verifying_key().to_bytes();
    let author = Author::Server { canonical_id: canonical_id.to_string(), key };
    (author, signing_key.to_bytes())
}

/// Derive `Author::Server` from a previously-saved 32-byte signing key seed.
pub fn server_author_from_seed(canonical_id: &str, seed: &[u8; 32]) -> Author {
    let signing_key = ed25519_dalek::SigningKey::from_bytes(seed);
    let key: PublicKeyBytes = signing_key.verifying_key().to_bytes();
    Author::Server { canonical_id: canonical_id.to_string(), key }
}

// ---------------------------------------------------------------------------
// Persistent keyring helpers
// ---------------------------------------------------------------------------

/// Serialisable snapshot of a user's identity stored on disk.
///
/// Only the 32-byte Ed25519 seed is persisted; the public key is always
/// re-derived on load so there is a single source of truth.
#[derive(Debug, Serialize, Deserialize)]
#[cfg(feature = "std")]
pub struct IdentityProfile {
    /// The author identity (Human or AI).
    pub author: Author,
    /// The raw Ed25519 signing key seed bytes.
    pub secret_key: [u8; 32],
}

#[cfg(feature = "std")]
fn identity_path() -> anyhow::Result<std::path::PathBuf> {
    let proj = directories::ProjectDirs::from("", "", "arc")
        .ok_or_else(|| anyhow::anyhow!("could not determine OS config directory"))?;
    Ok(proj.config_dir().join("identity.json"))
}

/// Generate a fresh Ed25519 keypair for the given identity and persist it to
/// the OS-native config directory (e.g. `%APPDATA%\arc\identity.json`).
#[cfg(feature = "std")]
pub fn save_identity(name: &str, email: &str) -> anyhow::Result<()> {
    let mut rng = rand_core::OsRng;
    let signing_key = ed25519_dalek::SigningKey::generate(&mut rng);
    let key: PublicKeyBytes = signing_key.verifying_key().to_bytes();
    let profile = IdentityProfile {
        author: Author::Human { name: name.to_string(), email: email.to_string(), key },
        secret_key: signing_key.to_bytes(),
    };
    let path = identity_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&profile)?)?;
    Ok(())
}

/// Load the persisted identity from disk and re-derive the signing key.
///
/// Returns a descriptive error instructing the user to run `arc auth login`
/// if no identity file exists yet.
#[cfg(feature = "std")]
pub fn load_identity() -> anyhow::Result<(Author, ed25519_dalek::SigningKey)> {
    let path = identity_path()?;
    let json = std::fs::read_to_string(&path).map_err(|_| {
        anyhow::anyhow!(
            "No identity configured. Please set one using:\n  arc identity --name \"Your Name\" \
             --email \"you@example.com\""
        )
    })?;
    let profile: IdentityProfile = serde_json::from_str(&json)
        .map_err(|e| anyhow::anyhow!("identity file is corrupt: {e}"))?;
    Ok((profile.author, ed25519_dalek::SigningKey::from_bytes(&profile.secret_key)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn test_keypair_deterministic() {
        let (a1, sk1) = test_keypair();
        let (a2, sk2) = test_keypair();
        assert_eq!(a1, a2);
        assert_eq!(sk1.to_bytes(), sk2.to_bytes());
    }

    #[test]
    fn test_keypair_human_author_fields() {
        let (author, _) = test_keypair();
        match &author {
            Author::Human { name, email, key } => {
                assert_eq!(name, "Test User");
                assert_eq!(email, "test@example.com");
                assert_eq!(key.len(), 32);
            }
            other => panic!("expected Human variant, got: {:?}", other),
        }
    }

    #[test]
    fn server_author_from_seed_deterministic() {
        let seed = [7u8; 32];
        let a1 = server_author_from_seed("test-server", &seed);
        let a2 = server_author_from_seed("test-server", &seed);
        assert_eq!(a1, a2);
    }

    #[test]
    fn server_author_from_seed_canonical_id() {
        let seed = [7u8; 32];
        let author = server_author_from_seed("my-server", &seed);
        match &author {
            Author::Server { canonical_id, key } => {
                assert_eq!(canonical_id, "my-server");
                assert_eq!(key.len(), 32);
            }
            other => panic!("expected Server variant, got: {:?}", other),
        }
    }

    #[test]
    fn signature_serde_roundtrip() {
        let sig = Signature([0xAB; 64]);
        let json = serde_json::to_string(&sig).unwrap();
        let loaded: Signature = serde_json::from_str(&json).unwrap();
        assert_eq!(sig, loaded);
    }

    #[test]
    fn signature_clone_and_debug() {
        let sig = Signature([0x42; 64]);
        let cloned = sig.clone();
        assert_eq!(sig, cloned);
        let debug = format!("{:?}", sig);
        assert!(debug.contains("Signature"));
    }

    #[test]
    fn author_serde_roundtrip_human() {
        let author = Author::Human {
            name: "Alice".to_string(),
            email: "alice@example.com".to_string(),
            key: [1u8; 32],
        };
        let json = serde_json::to_string(&author).unwrap();
        let loaded: Author = serde_json::from_str(&json).unwrap();
        assert_eq!(author, loaded);
    }

    #[test]
    fn author_serde_roundtrip_ai() {
        let author = Author::AI { model: "claude-3-opus".to_string(), human_sponsor: [5u8; 32] };
        let json = serde_json::to_string(&author).unwrap();
        let loaded: Author = serde_json::from_str(&json).unwrap();
        assert_eq!(author, loaded);
    }

    #[test]
    fn author_serde_roundtrip_server() {
        let author = Author::Server { canonical_id: "arc-server".to_string(), key: [9u8; 32] };
        let json = serde_json::to_string(&author).unwrap();
        let loaded: Author = serde_json::from_str(&json).unwrap();
        assert_eq!(author, loaded);
    }

    #[test]
    fn author_serde_roundtrip_transient() {
        let author = Author::Transient { session_id: "ci-runner-42".to_string(), key: [3u8; 32] };
        let json = serde_json::to_string(&author).unwrap();
        let loaded: Author = serde_json::from_str(&json).unwrap();
        assert_eq!(author, loaded);
    }

    #[test]
    fn author_debug_format() {
        let author = Author::Human {
            name: "Bob".to_string(),
            email: "bob@example.com".to_string(),
            key: [0u8; 32],
        };
        let debug = format!("{:?}", author);
        assert!(debug.contains("Human"));
        assert!(debug.contains("Bob"));
    }

    #[test]
    fn generate_transient_keypair_seed_unique() {
        let (a1, s1) = generate_transient_keypair_seed("session-1");
        let (a2, s2) = generate_transient_keypair_seed("session-2");
        assert_ne!(s1, s2);
        match (&a1, &a2) {
            (
                Author::Transient { session_id: id1, .. },
                Author::Transient { session_id: id2, .. },
            ) => {
                assert_eq!(id1, "session-1");
                assert_eq!(id2, "session-2");
            }
            other => panic!("expected Transient variants: {:?}", other),
        }
    }

    #[test]
    fn generate_server_keypair_seed_unique() {
        let (a1, s1) = generate_server_keypair_seed("server-a");
        let (a2, s2) = generate_server_keypair_seed("server-b");
        assert_ne!(s1, s2);
        match (&a1, &a2) {
            (
                Author::Server { canonical_id: id1, .. },
                Author::Server { canonical_id: id2, .. },
            ) => {
                assert_eq!(id1, "server-a");
                assert_eq!(id2, "server-b");
            }
            other => panic!("expected Server variants: {:?}", other),
        }
    }

    #[test]
    fn key_bytes_length_matches() {
        let (_, sk) = test_keypair();
        assert_eq!(sk.to_bytes().len(), 32);
    }
}
