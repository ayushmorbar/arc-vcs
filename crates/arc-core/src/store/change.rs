use std::collections::HashSet;

use ed25519_dalek::Signer;
use serde::{Deserialize, Serialize};

use crate::algebra::{Atom, Blake3Hash};
use crate::store::author::{Author, PublicKeyBytes, Signature};

/// An atomic, replayable change — the fundamental unit in arc.
///
/// A `Change` bundles one or more [`Atom`]s into a single semantic operation
/// whose identity is the BLAKE3 hash of its deterministically-serialized
/// content (`sorted_deps + atoms + intent + author`).
///
/// # Cryptographic Envelope
///
/// The `signature` field is an Ed25519 signature over `self.id` and is
/// intentionally excluded from the hash payload.  The `author` field IS
/// included in the hash, so the identity of the author is content-addressed
/// just like everything else.
///
/// # Verification (two-layer)
///
/// [`Change::verify_signature`] performs two independent checks:
/// 1. Re-hash the fields → assert the recomputed id equals `self.id`
///    (detects any field tampering that left the signature byte-for-byte).
/// 2. Verify the signature against `self.id` using the author's public key
///    (detects `id`-substitution attacks where an attacker replaces the id
///    bytes but cannot re-sign cleanly).
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// Content-addressed identity (BLAKE3 hash of `(sorted_deps, atoms, intent, author)`).
    pub id: Blake3Hash,
    /// The set of change IDs this change depends on (partial order edges).
    pub deps: HashSet<Blake3Hash>,
    /// The ordered list of AST-level atoms that compose this change.
    pub atoms: Vec<Atom>,
    /// Human- or AI-supplied semantic intent (commit message / goal).
    pub intent: String,
    /// The identity of the author who produced this change.
    pub author: Author,
    /// Ed25519 signature of `self.id` using the author's signing key.
    ///
    /// For `Author::AI`, this is signed by the human sponsor's key
    /// because AI agents cannot self-authorize.
    pub signature: Signature,
    /// Dual-Provenance audit link: the BLAKE3 hash of the original Change
    /// that this Change was collapsed from (Phase 39 Identity Collapsing).
    ///
    /// Set by the server when it re-signs a transient-author Change under
    /// `Author::Server`.  The original Change stays in CAS forever so
    /// auditors can always verify the pre-collapse authorship (SLSA L4).
    ///
    /// `None` for all ordinary (non-collapsed) Changes.
    /// Excluded from `compute_id` — it is provenance metadata, not content,
    /// so changing `collapsed_from` does not alter the content-addressed
    /// identity of the Change.
    #[serde(default)]
    pub collapsed_from: Option<Blake3Hash>,
}

impl Change {
    /// Create a new `Change`, computing its content-addressed `id` and
    /// signing it with `signing_key`.
    ///
    /// Dependencies are sorted before hashing so the `id` is deterministic
    /// regardless of `HashSet` iteration order.
    pub fn new(
        deps: HashSet<Blake3Hash>,
        atoms: Vec<Atom>,
        intent: impl Into<String>,
        author: Author,
        signing_key: &ed25519_dalek::SigningKey,
    ) -> Self {
        let intent = intent.into();
        let id = Self::compute_id(&deps, &atoms, &intent, &author);
        let sig: ed25519_dalek::Signature = signing_key.sign(&id);
        Self {
            id,
            deps,
            atoms,
            intent,
            author,
            signature: Signature(sig.to_bytes()),
            collapsed_from: None,
        }
    }

    /// Build a canonicalized Change from existing fields, signed with a new key.
    /// Used by the server for identity collapsing (Dual-Provenance -- Phase 39).
    ///
    /// Identical to [`new_canonical`] but accepts a raw 32-byte Ed25519 seed
    /// rather than a `SigningKey`.  This lets callers (e.g. `arc-net`) hold
    /// only the seed bytes in their state without taking a hard dep on the
    /// `ed25519-dalek` crate.
    pub fn new_canonical_from_seed(
        deps: HashSet<Blake3Hash>,
        atoms: Vec<Atom>,
        intent: impl Into<String>,
        author: Author,
        signing_seed: &[u8; 32],
        original_id: Blake3Hash,
    ) -> Self {
        let signing_key = ed25519_dalek::SigningKey::from_bytes(signing_seed);
        Self::new_canonical(deps, atoms, intent, author, &signing_key, original_id)
    }
    /// Build a canonicalized Change from an existing Change, re-signing it
    /// under a new author (typically `Author::Server` during Identity
    /// Collapsing) and recording the original BLAKE3 hash in `collapsed_from`.
    ///
    /// The remapped `deps` must already have been updated by the caller to
    /// point at canonical IDs for any dependencies that were themselves
    /// collapsed (Cryptographic Cascade rule: if any dep was rewritten, this
    /// Change must also be re-signed because deps are part of the hash).
    ///
    /// Both the original Change AND this canonical Change should be written
    /// to CAS — the original serves as the permanent SLSA L4 audit root.
    pub fn new_canonical(
        deps: HashSet<Blake3Hash>,
        atoms: Vec<Atom>,
        intent: impl Into<String>,
        author: Author,
        signing_key: &ed25519_dalek::SigningKey,
        original_id: Blake3Hash,
    ) -> Self {
        let intent = intent.into();
        let id = Self::compute_id(&deps, &atoms, &intent, &author);
        let sig: ed25519_dalek::Signature = signing_key.sign(&id);
        Self {
            id,
            deps,
            atoms,
            intent,
            author,
            signature: Signature(sig.to_bytes()),
            collapsed_from: Some(original_id),
        }
    }

    /// Deterministic id derivation: `blake3(bincode(sorted_deps, atoms, intent, author))`.
    ///
    /// **Crypto invariants**:
    /// - `intent` is included so an attacker cannot rewrite the commit message
    ///   without changing the CAS address.
    /// - `author` is included so the identity is content-addressed alongside
    ///   the payload; substituting a different author changes the id.
    /// - `collapsed_from` and `signature` are intentionally excluded so
    ///   provenance metadata never affects the content-addressed identity.
    pub(crate) fn compute_id(
        deps: &HashSet<Blake3Hash>,
        atoms: &[Atom],
        intent: &str,
        author: &Author,
    ) -> Blake3Hash {
        let mut sorted_deps: Vec<&Blake3Hash> = deps.iter().collect();
        sorted_deps.sort();

        let payload = bincode::serialize(&(&sorted_deps, atoms, intent, author))
            .expect("bincode serialization is infallible for these types");

        *blake3::hash(&payload).as_bytes()
    }

    /// Verify the cryptographic integrity of this change.
    ///
    /// Returns `true` only when BOTH layers pass:
    ///
    /// **Layer 1 — Content integrity**: re-hash the fields and assert the
    /// result equals `self.id`. This catches any field tampering (intent,
    /// atoms, author, deps) even when the attacker left the id and signature
    /// untouched.
    ///
    /// **Layer 2 — Signature integrity**: verify `self.signature` against
    /// `self.id` using the public key embedded in `self.author`. For
    /// `Author::AI`, the human sponsor's key is used, enforcing the
    /// governance rule that AI agents cannot self-authorize.
    pub fn verify_signature(&self) -> bool {
        // Layer 1: content-address re-check.
        let expected_id = Self::compute_id(&self.deps, &self.atoms, &self.intent, &self.author);
        if expected_id != self.id {
            return false;
        }

        // Layer 2: Ed25519 signature check.
        let pub_key_bytes: &PublicKeyBytes = match &self.author {
            Author::Human { key, .. } => key,
            Author::AI { human_sponsor, .. } => human_sponsor,
            // Server-signed canonical Changes are verified against the
            // server's own public key (same Ed25519 path as Human).
            Author::Server { key, .. } => key,
        };

        let verifying_key = match ed25519_dalek::VerifyingKey::from_bytes(pub_key_bytes) {
            Ok(k) => k,
            Err(_) => return false,
        };

        let sig = ed25519_dalek::Signature::from_bytes(&self.signature.0);

        use ed25519_dalek::Verifier;
        verifying_key.verify(&self.id, &sig).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::algebra::Atom;
    use crate::store::author::test_keypair;

    #[test]
    fn test_cryptographic_provenance() {
        let (author, signing_key) = test_keypair();

        // --- Build a valid change ---
        let change = Change::new(
            HashSet::new(),
            vec![Atom::Insert {
                at: vec!["fn_main".into()],
                content_hash: [0u8; 32],
            }],
            "add main",
            author,
            &signing_key,
        );

        // A freshly created change must pass both verification layers.
        assert!(
            change.verify_signature(),
            "fresh change must pass cryptographic verification"
        );

        // --- Layer-1 tampering: mutate intent without re-signing ---
        let mut tampered_intent = change.clone();
        tampered_intent.intent = "injected malicious intent".to_string();
        assert!(
            !tampered_intent.verify_signature(),
            "tampered intent must fail layer-1 content re-hash check"
        );

        // --- Layer-1 tampering: replace an atom's content without re-signing ---
        let mut tampered_atom = change.clone();
        tampered_atom.atoms[0] = Atom::Insert {
            at: vec!["fn_main".into()],
            content_hash: [0xde; 32],
        };
        assert!(
            !tampered_atom.verify_signature(),
            "tampered atom content must fail layer-1 content re-hash check"
        );
    }

    /// Deps must survive the content-addressed id computation and be included
    /// in the hash (so changing deps changes the id).
    #[test]
    fn test_change_deps_are_preserved() {
        let (author, signing_key) = test_keypair();
        let dep_id = [42u8; 32];

        let with_dep = Change::new(
            HashSet::from([dep_id]),
            vec![Atom::Insert {
                at: vec!["fn_child".into()],
                content_hash: [0u8; 32],
            }],
            "child change",
            author.clone(),
            &signing_key,
        );

        assert!(with_dep.deps.contains(&dep_id), "deps must be present in the Change");
        assert!(with_dep.verify_signature(), "change with deps must carry a valid signature");

        // A change with the same atoms/intent/author but no deps must get a different id.
        let no_dep = Change::new(
            HashSet::new(),
            vec![Atom::Insert {
                at: vec!["fn_child".into()],
                content_hash: [0u8; 32],
            }],
            "child change",
            author,
            &signing_key,
        );

        assert_ne!(
            with_dep.id, no_dep.id,
            "including deps must change the content-addressed id"
        );
    }

    /// A `Change` must survive a `bincode` serialise → deserialise roundtrip.
    #[test]
    fn test_change_serialization_roundtrip() {
        let (author, signing_key) = test_keypair();

        let original = Change::new(
            HashSet::from([[1u8; 32], [2u8; 32]]),
            vec![
                Atom::Insert {
                    at: vec!["fn_a".into()],
                    content_hash: [0u8; 32],
                },
                Atom::Delete { at: vec!["fn_b".into()], prior_hash: [0u8; 32] },
            ],
            "roundtrip",
            author,
            &signing_key,
        );

        let bytes = bincode::serialize(&original).expect("serialization must succeed");
        let decoded: Change = bincode::deserialize(&bytes).expect("deserialization must succeed");

        assert_eq!(original, decoded, "Change must survive a bincode roundtrip");
        assert!(decoded.verify_signature(), "decoded Change must still verify");
    }

    /// `collapsed_from` is excluded from `compute_id`, so setting it on an
    /// otherwise-identical Change must not change the content-addressed id.
    #[test]
    fn test_collapsed_from_excluded_from_id() {
        let (author, signing_key) = test_keypair();

        let base = Change::new(
            HashSet::new(),
            vec![Atom::Insert {
                at: vec!["main".into()],
                content_hash: [0u8; 32],
            }],
            "add main",
            author,
            &signing_key,
        );

        // Create a clone that has a `collapsed_from` pointer set.
        let mut with_link = base.clone();
        with_link.collapsed_from = Some([0xab; 32]);

        // The id must be identical — collapsed_from is provenance metadata.
        assert_eq!(
            base.id, with_link.id,
            "collapsed_from must not affect the content-addressed identity"
        );
        // Both must still verify: the signature covers the id, not
        // collapsed_from, so setting collapsed_from does not break it.
        assert!(
            base.verify_signature(),
            "base Change must verify"
        );
        assert!(
            with_link.verify_signature(),
            "Change with collapsed_from set must still verify (provenance field is outside the hash)"
        );
    }

    /// `Change::new_canonical` produces a collapsed Change signed by a
    /// different author whose `collapsed_from` points to the original id.
    #[test]
    fn test_new_canonical_sets_collapsed_from_and_verifies() {
        use crate::store::author::PublicKeyBytes;

        let (original_author, original_key) = test_keypair();
        let original = Change::new(
            HashSet::new(),
            vec![Atom::Insert {
                at: vec!["lib".into()],
                content_hash: [1u8; 32],
            }],
            "add lib",
            original_author,
            &original_key,
        );

        // Build a server signing key (different seed from the test keypair).
        let server_key = ed25519_dalek::SigningKey::from_bytes(&[99u8; 32]);
        let server_pubkey: PublicKeyBytes = server_key.verifying_key().to_bytes();
        let server_author = crate::store::author::Author::Server {
            canonical_id: "arc-server".to_string(),
            key: server_pubkey,
        };

        let canonical = Change::new_canonical(
            HashSet::new(),
            original.atoms.clone(),
            original.intent.clone(),
            server_author,
            &server_key,
            original.id,
        );

        // collapsed_from must point at the original.
        assert_eq!(
            canonical.collapsed_from,
            Some(original.id),
            "canonical Change must carry collapsed_from = original.id"
        );
        // The canonical Change has a different author, so it gets a different id.
        assert_ne!(
            canonical.id, original.id,
            "canonical and original must have different ids (different author)"
        );
        // The canonical Change must pass cryptographic verification.
        assert!(
            canonical.verify_signature(),
            "canonical Change signed by Author::Server must verify"
        );
    }
}
