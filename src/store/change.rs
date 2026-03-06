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
        }
    }

    /// Deterministic id derivation: `blake3(bincode(sorted_deps, atoms, intent, author))`.
    ///
    /// **Crypto invariants**:
    /// - `intent` is included so an attacker cannot rewrite the commit message
    ///   without changing the CAS address.
    /// - `author` is included so the identity is content-addressed alongside
    ///   the payload; substituting a different author changes the id.
    fn compute_id(
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
                content: b"fn main() {}".to_vec(),
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
        // The attacker did NOT update `id` or `signature` — purely in-memory tampering.
        assert!(
            !tampered_intent.verify_signature(),
            "tampered intent must fail layer-1 content re-hash check"
        );

        // --- Layer-1 tampering: replace an atom's content without re-signing ---
        let mut tampered_atom = change.clone();
        tampered_atom.atoms[0] = Atom::Insert {
            at: vec!["fn_main".into()],
            content: b"fn main() { unsafe { backdoor(); } }".to_vec(),
        };
        assert!(
            !tampered_atom.verify_signature(),
            "tampered atom content must fail layer-1 content re-hash check"
        );
    }
}

