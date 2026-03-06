use serde::{Deserialize, Serialize};

/// A 32-byte Ed25519 public key.
pub type PublicKeyBytes = [u8; 32];

/// The identity of a change's author, carried inside every [`Change`].
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
        name: String,
        email: String,
        key: PublicKeyBytes,
    },
    /// An AI agent identified by its model name plus the public key of the
    /// human sponsor who approved the change.
    AI {
        model: String,
        human_sponsor: PublicKeyBytes,
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

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "a 64-byte Ed25519 signature")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut bytes = [0u8; 64];
                for (i, b) in bytes.iter_mut().enumerate() {
                    *b = seq.next_element()?.ok_or_else(|| {
                        serde::de::Error::invalid_length(i, &self)
                    })?;
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
#[cfg(test)]
pub fn test_keypair() -> (Author, ed25519_dalek::SigningKey) {
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&[42u8; 32]);
    let key: PublicKeyBytes = signing_key.verifying_key().to_bytes();
    let author = Author::Human {
        name: "Test User".to_string(),
        email: "test@example.com".to_string(),
        key,
    };
    (author, signing_key)
}
