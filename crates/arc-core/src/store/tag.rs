//! Cryptographically-signed immutable tags.
//!
//! A [`Tag`](crate::store::tag::Tag) is arc's equivalent of Git's annotated tag: a human-readable name
//! permanently bound to a [`Blake3Hash`](crate::algebra::Blake3Hash).  Unlike a
//! [`View`](crate::store::view::View), a tag never moves.  Every tag is
//! signed with the author's Ed25519 key, making supply-chain spoofing
//! detectable without any external PKI.

use ed25519_dalek::Signer;
use serde::{Deserialize, Serialize};

use crate::algebra::Blake3Hash;
use crate::store::author::{Author, PublicKeyBytes, Signature};

/// An immutable, cryptographically-signed pointer to a specific [`Change`](crate::store::change::Change).
///
/// # Cryptographic Guarantee
///
/// `signature` is an Ed25519 signature over
/// `blake3(bincode(name, target, author))`.  An attacker cannot redirect an
/// existing tag to a different change without the author's private key,
/// and tampering with the name or author fields is equally detectable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tag {
    /// Human-readable tag name, e.g. `"v1.0.0"` or `"release-2026-03-07"`.
    pub name: String,
    /// The [`Blake3Hash`] of the [`Change`](crate::store::change::Change) this
    /// tag permanently points to.
    pub target: Blake3Hash,
    /// Identity of the author who created this tag.
    pub author: Author,
    /// Ed25519 signature over `blake3(bincode(name, target, author))`.
    pub signature: Signature,
}

impl Tag {
    /// Create a new signed `Tag`.
    ///
    /// Computes the BLAKE3 commitment over `(name, target, author)` and signs
    /// it with `signing_key`.  The commitment hash is NOT stored separately —
    /// `verify()` always recomputes it from the fields.
    pub fn new(
        name: impl Into<String>,
        target: Blake3Hash,
        author: Author,
        signing_key: &ed25519_dalek::SigningKey,
    ) -> Self {
        let name = name.into();
        let commitment = Self::compute_commitment(&name, &target, &author);
        let sig: ed25519_dalek::Signature = signing_key.sign(&commitment);
        Self {
            name,
            target,
            author,
            signature: Signature(sig.to_bytes()),
        }
    }

    /// Verify the tag's cryptographic signature.
    ///
    /// Re-hashes `(name, target, author)` and verifies the stored signature
    /// against it using the public key embedded in `self.author`.
    ///
    /// Returns `true` only when the signature is valid and the commitment
    /// matches the current field values.
    pub fn verify(&self) -> bool {
        let commitment = Self::compute_commitment(&self.name, &self.target, &self.author);

        let pub_key_bytes: &PublicKeyBytes = match &self.author {
            Author::Human { key, .. } => key,
            Author::AI { human_sponsor, .. } => human_sponsor,
            // Server-signed tags are verified against the server's own key.
            Author::Server { key, .. } => key,
        };

        let Ok(verifying_key) = ed25519_dalek::VerifyingKey::from_bytes(pub_key_bytes) else {
            return false;
        };

        let sig = ed25519_dalek::Signature::from_bytes(&self.signature.0);

        use ed25519_dalek::Verifier;
        verifying_key.verify(&commitment, &sig).is_ok()
    }

    /// Deterministic BLAKE3 commitment: `blake3(bincode(name, target, author))`.
    fn compute_commitment(name: &str, target: &Blake3Hash, author: &Author) -> Blake3Hash {
        let payload = bincode::serialize(&(name, target, author))
            .expect("bincode serialization is infallible for Tag fields");
        *blake3::hash(&payload).as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::author::test_keypair;

    #[test]
    fn test_tag_verify() {
        let (author, signing_key) = test_keypair();
        let target = [7u8; 32];

        let tag = Tag::new("v1.0.0", target, author.clone(), &signing_key);

        assert_eq!(tag.name, "v1.0.0");
        assert_eq!(tag.target, target);
        assert!(tag.verify(), "freshly created tag must verify");
    }

    #[test]
    fn test_tag_tamper_detection() {
        let (author, signing_key) = test_keypair();
        let target = [7u8; 32];

        // Tamper the target hash — signature must no longer verify.
        let mut tag = Tag::new("v1.0.0", target, author.clone(), &signing_key);
        tag.target = [99u8; 32];
        assert!(!tag.verify(), "tampered target must fail signature check");

        // Tamper the name — signature must no longer verify.
        let mut tag2 = Tag::new("v1.0.0", target, author, &signing_key);
        tag2.name = "evil".to_string();
        assert!(!tag2.verify(), "tampered name must fail signature check");
    }
}
