use std::collections::HashSet;

use arc_algebra_types::{Atom, Blake3Hash};
use arc_store_types::{Author, PublicKeyBytes, Signature};
use ed25519_dalek::Signer;
use serde::{Deserialize, Serialize};

/// High-level author classification for ghost-node governance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthorType {
    /// Human-authored node.
    Human,
    /// AI-authored node with confidence score and optional sponsor key.
    AI {
        /// Model-reported confidence score.
        confidence: f32,
        /// Optional sponsoring public key once approved.
        human_sponsor: Option<PublicKeyBytes>,
    },
}

impl PartialEq for AuthorType {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (AuthorType::Human, AuthorType::Human) => true,
            (
                AuthorType::AI { confidence: a, human_sponsor: as_ },
                AuthorType::AI { confidence: b, human_sponsor: bs },
            ) => a.to_bits() == b.to_bits() && as_ == bs,
            _ => false,
        }
    }
}

impl Eq for AuthorType {}

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
/// 1. Re-hash the fields → assert the recomputed id equals `self.id` (detects any field tampering
///    that left the signature byte-for-byte).
/// 2. Verify the signature against `self.id` using the author's public key (detects
///    `id`-substitution attacks where an attacker replaces the id bytes but cannot re-sign
///    cleanly).
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
    /// `Author::Server`.  In production deployments the original Change is
    /// expected to be retained in CAS so auditors can verify pre-collapse
    /// authorship (SLSA L4-style auditability).
    ///
    /// `None` for all ordinary (non-collapsed) Changes.
    /// Excluded from `compute_id` — it is provenance metadata, not content,
    /// so changing `collapsed_from` does not alter the content-addressed
    /// identity of the Change.
    #[serde(default)]
    pub collapsed_from: Option<Blake3Hash>,
    /// High-level author type used by ghost-node governance flows.
    #[serde(default = "default_human_author_type")]
    pub author_type: AuthorType,
    /// Whether this node is provisional (ghost) and excluded from default frontiers.
    #[serde(default)]
    pub is_ghost: bool,
}

fn default_human_author_type() -> AuthorType {
    AuthorType::Human
}

impl Change {
    /// Return a canonicalized atom vector used only for id derivation.
    ///
    /// Conflict sides are sorted lexicographically by hash bytes so merge
    /// order does not affect the resulting content-addressed identity.
    fn canonicalize_atoms_for_id(atoms: &[Atom]) -> Vec<Atom> {
        atoms
            .iter()
            .map(|atom| match atom {
                Atom::Conflict { bases, sides, at } => {
                    let mut canonical_sides = sides.clone();
                    canonical_sides.sort();
                    Atom::Conflict { bases: bases.clone(), sides: canonical_sides, at: at.clone() }
                }
                _ => atom.clone(),
            })
            .collect()
    }

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
        Self::new_with_metadata(deps, atoms, intent, author, AuthorType::Human, false, signing_key)
    }

    /// Create a new `Change` with explicit ghost-node governance metadata.
    pub fn new_with_metadata(
        deps: HashSet<Blake3Hash>,
        atoms: Vec<Atom>,
        intent: impl Into<String>,
        author: Author,
        author_type: AuthorType,
        is_ghost: bool,
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
            author_type,
            is_ghost,
        }
    }

    /// Build a rewritten change while preserving signature only when payload is identical.
    ///
    /// If `(deps, atoms, intent, author)` hashes to the same id as `original`
    /// and the author is unchanged, the original signature is reused.
    /// Otherwise, a new id is computed and signed with `signing_key`.
    pub fn rewritten_or_resigned(
        original: &Self,
        deps: HashSet<Blake3Hash>,
        atoms: Vec<Atom>,
        intent: impl Into<String>,
        author: Author,
        signing_key: &ed25519_dalek::SigningKey,
    ) -> Self {
        let intent = intent.into();
        let id = Self::compute_id(&deps, &atoms, &intent, &author);

        if id == original.id && author == original.author {
            return Self {
                id,
                deps,
                atoms,
                intent,
                author,
                signature: original.signature.clone(),
                collapsed_from: original.collapsed_from,
                author_type: original.author_type.clone(),
                is_ghost: original.is_ghost,
            };
        }

        let sig: ed25519_dalek::Signature = signing_key.sign(&id);
        Self {
            id,
            deps,
            atoms,
            intent,
            author,
            signature: Signature(sig.to_bytes()),
            collapsed_from: original.collapsed_from,
            author_type: original.author_type.clone(),
            is_ghost: original.is_ghost,
        }
    }

    /// Build a canonicalized Change from existing fields, signed with a new key.
    /// Used by the server for identity collapsing (Dual-Provenance -- Phase 39).
    ///
    /// Identical to [`Self::new_canonical`] but accepts a raw 32-byte Ed25519 seed
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
    /// to CAS by the calling layer — the original acts as the audit root for
    /// provenance reconstruction.
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
            author_type: AuthorType::Human,
            is_ghost: false,
        }
    }

    /// Deterministic id derivation: `blake3(bincode(sorted_deps, atoms, intent, author))`.
    ///
    /// **Crypto invariants**:
    /// - `intent` is included so an attacker cannot rewrite the commit message without changing the
    ///   CAS address.
    /// - `author` is included so the identity is content-addressed alongside the payload;
    ///   substituting a different author changes the id.
    /// - `deps` are sorted before hashing, so `HashSet` insertion order can never change the
    ///   resulting id across machines or runs.
    /// - `collapsed_from` and `signature` are intentionally excluded so provenance metadata never
    ///   affects the content-addressed identity.
    pub fn compute_id(
        deps: &HashSet<Blake3Hash>,
        atoms: &[Atom],
        intent: &str,
        author: &Author,
    ) -> Blake3Hash {
        let mut sorted_deps: Vec<&Blake3Hash> = deps.iter().collect();
        sorted_deps.sort();

        let canonical_atoms = Self::canonicalize_atoms_for_id(atoms);

        let payload = bincode::serialize(&(&sorted_deps, &canonical_atoms, intent, author))
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
            // Transient sessions sign with their own ephemeral key.
            Author::Transient { key, .. } => key,
        };

        let verifying_key = match ed25519_dalek::VerifyingKey::from_bytes(pub_key_bytes) {
            Ok(k) => k,
            Err(_) => return false,
        };

        let sig = ed25519_dalek::Signature::from_bytes(&self.signature.0);

        use ed25519_dalek::Verifier;
        verifying_key.verify(&self.id, &sig).is_ok()
    }

    /// Return an erased tombstone of this change.
    ///
    /// The tombstone preserves the original `id` (so graph references remain
    /// valid) but clears `deps`, `atoms`, and `intent` so the payload is
    /// empty.  Signature and authorship metadata are kept unchanged.
    pub fn erased(&self) -> Self {
        Self {
            id: self.id,
            deps: HashSet::new(),
            atoms: Vec::new(),
            intent: String::new(),
            author: self.author.clone(),
            signature: self.signature.clone(),
            collapsed_from: self.collapsed_from,
            author_type: self.author_type.clone(),
            is_ghost: self.is_ghost,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use arc_algebra_types::Atom;
    use arc_store_types::author::test_keypair;

    use super::*;

    #[test]
    fn test_cryptographic_provenance() {
        let (author, signing_key) = test_keypair();

        // --- Build a valid change ---
        let change = Change::new(
            HashSet::new(),
            vec![Atom::Insert { at: vec!["fn_main".into()], content_hash: [0u8; 32] }],
            "add main",
            author,
            &signing_key,
        );

        // A freshly created change must pass both verification layers.
        assert!(change.verify_signature(), "fresh change must pass cryptographic verification");

        // --- Layer-1 tampering: mutate intent without re-signing ---
        let mut tampered_intent = change.clone();
        tampered_intent.intent = "injected malicious intent".to_string();
        assert!(
            !tampered_intent.verify_signature(),
            "tampered intent must fail layer-1 content re-hash check"
        );

        // --- Layer-1 tampering: replace an atom's content without re-signing ---
        let mut tampered_atom = change.clone();
        tampered_atom.atoms[0] =
            Atom::Insert { at: vec!["fn_main".into()], content_hash: [0xDE; 32] };
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
            vec![Atom::Insert { at: vec!["fn_child".into()], content_hash: [0u8; 32] }],
            "child change",
            author.clone(),
            &signing_key,
        );

        assert!(with_dep.deps.contains(&dep_id), "deps must be present in the Change");
        assert!(with_dep.verify_signature(), "change with deps must carry a valid signature");

        // A change with the same atoms/intent/author but no deps must get a different id.
        let no_dep = Change::new(
            HashSet::new(),
            vec![Atom::Insert { at: vec!["fn_child".into()], content_hash: [0u8; 32] }],
            "child change",
            author,
            &signing_key,
        );

        assert_ne!(with_dep.id, no_dep.id, "including deps must change the content-addressed id");
    }

    /// A `Change` must survive a `bincode` serialise → deserialise roundtrip.
    #[test]
    fn test_change_serialization_roundtrip() {
        let (author, signing_key) = test_keypair();

        let original = Change::new(
            HashSet::from([[1u8; 32], [2u8; 32]]),
            vec![
                Atom::Insert { at: vec!["fn_a".into()], content_hash: [0u8; 32] },
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

    #[test]
    fn test_rewritten_or_resigned_reuses_signature_when_payload_identical() {
        let (author, signing_key) = test_keypair();
        let original = Change::new(
            HashSet::new(),
            vec![Atom::Insert { at: vec!["main".into()], content_hash: [7u8; 32] }],
            "same",
            author.clone(),
            &signing_key,
        );

        let rebuilt = Change::rewritten_or_resigned(
            &original,
            original.deps.clone(),
            original.atoms.clone(),
            original.intent.clone(),
            author,
            &signing_key,
        );

        assert_eq!(rebuilt.id, original.id);
        assert_eq!(rebuilt.signature, original.signature);
        assert!(rebuilt.verify_signature());
    }

    #[test]
    fn test_rewritten_or_resigned_re_signs_when_payload_changes() {
        let (author, signing_key) = test_keypair();
        let original = Change::new(
            HashSet::new(),
            vec![Atom::Insert { at: vec!["main".into()], content_hash: [7u8; 32] }],
            "same",
            author.clone(),
            &signing_key,
        );

        let rebuilt = Change::rewritten_or_resigned(
            &original,
            HashSet::from([[9u8; 32]]),
            original.atoms.clone(),
            original.intent.clone(),
            author,
            &signing_key,
        );

        assert_ne!(rebuilt.id, original.id);
        assert_ne!(rebuilt.signature, original.signature);
        assert!(rebuilt.verify_signature());
    }

    /// `collapsed_from` is excluded from `compute_id`, so setting it on an
    /// otherwise-identical Change must not change the content-addressed id.
    #[test]
    fn test_collapsed_from_excluded_from_id() {
        let (author, signing_key) = test_keypair();

        let base = Change::new(
            HashSet::new(),
            vec![Atom::Insert { at: vec!["main".into()], content_hash: [0u8; 32] }],
            "add main",
            author,
            &signing_key,
        );

        // Create a clone that has a `collapsed_from` pointer set.
        let mut with_link = base.clone();
        with_link.collapsed_from = Some([0xAB; 32]);

        // The id must be identical — collapsed_from is provenance metadata.
        assert_eq!(
            base.id, with_link.id,
            "collapsed_from must not affect the content-addressed identity"
        );
        // Both must still verify: the signature covers the id, not
        // collapsed_from, so setting collapsed_from does not break it.
        assert!(base.verify_signature(), "base Change must verify");
        assert!(
            with_link.verify_signature(),
            "Change with collapsed_from set must still verify (provenance field is outside the \
             hash)"
        );
    }

    /// `Change::new_canonical` produces a collapsed Change signed by a
    /// different author whose `collapsed_from` points to the original id.
    #[test]
    fn test_new_canonical_sets_collapsed_from_and_verifies() {
        use arc_store_types::author::PublicKeyBytes;

        let (original_author, original_key) = test_keypair();
        let original = Change::new(
            HashSet::new(),
            vec![Atom::Insert { at: vec!["lib".into()], content_hash: [1u8; 32] }],
            "add lib",
            original_author,
            &original_key,
        );

        // Build a server signing key (different seed from the test keypair).
        let server_key = ed25519_dalek::SigningKey::from_bytes(&[99u8; 32]);
        let server_pubkey: PublicKeyBytes = server_key.verifying_key().to_bytes();
        let server_author = arc_store_types::author::Author::Server {
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

    /// `Author::Transient` changes are cryptographically first-class:
    /// the ephemeral session key is a real Ed25519 key and must pass both
    /// verification layers.
    #[test]
    fn test_transient_author_verifies() {
        use arc_store_types::author::generate_transient_keypair_seed;
        use ed25519_dalek::SigningKey;

        let (author, seed) = generate_transient_keypair_seed("ci-runner-42");
        let signing_key = SigningKey::from_bytes(&seed);

        let change = Change::new(
            HashSet::new(),
            vec![Atom::Insert { at: vec!["lib.rs".into()], content_hash: [0u8; 32] }],
            "ephemeral CI commit",
            author,
            &signing_key,
        );

        assert!(
            change.verify_signature(),
            "Transient-authored change must pass both cryptographic verification layers"
        );
    }

    #[test]
    fn test_compute_id_stable_under_dependency_insertion_order() {
        let (author, _signing_key) = test_keypair();
        let atoms = vec![Atom::Insert { at: vec!["node".into()], content_hash: [3u8; 32] }];

        let mut deps_a = HashSet::new();
        deps_a.insert([1u8; 32]);
        deps_a.insert([2u8; 32]);

        let mut deps_b = HashSet::new();
        deps_b.insert([2u8; 32]);
        deps_b.insert([1u8; 32]);

        let id_a = Change::compute_id(&deps_a, &atoms, "same", &author);
        let id_b = Change::compute_id(&deps_b, &atoms, "same", &author);

        assert_eq!(
            id_a, id_b,
            "Change::compute_id must be independent of dependency insertion order"
        );
    }

    #[test]
    fn test_compute_id_stable_under_conflict_side_order() {
        let (author, _signing_key) = test_keypair();
        let deps = HashSet::new();
        let base = [7u8; 32];
        let side_a = [1u8; 32];
        let side_b = [2u8; 32];

        let atoms_ab = vec![Atom::Conflict {
            bases: vec![base],
            sides: vec![side_a, side_b],
            at: vec!["conflict/node".into()],
        }];

        let atoms_ba = vec![Atom::Conflict {
            bases: vec![base],
            sides: vec![side_b, side_a],
            at: vec!["conflict/node".into()],
        }];

        let id_ab = Change::compute_id(&deps, &atoms_ab, "merge", &author);
        let id_ba = Change::compute_id(&deps, &atoms_ba, "merge", &author);

        assert_eq!(id_ab, id_ba, "Change::compute_id must canonicalize conflict side ordering");
    }

    #[test]
    fn test_erased_creates_tombstone_preserving_id_and_author() {
        let (author, signing_key) = test_keypair();
        let mut deps = HashSet::new();
        deps.insert([10u8; 32]);
        let atoms = vec![Atom::Insert { at: vec!["src/main.rs".into()], content_hash: [5u8; 32] }];

        let original =
            Change::new(deps.clone(), atoms, "implement feature", author.clone(), &signing_key);
        let erased = original.erased();

        assert_eq!(erased.id, original.id, "erased must preserve the original id");
        assert!(erased.deps.is_empty(), "erased must clear deps");
        assert!(erased.atoms.is_empty(), "erased must clear atoms");
        assert!(erased.intent.is_empty(), "erased must clear intent");
        assert_eq!(erased.author, original.author, "erased must preserve author");
        assert_eq!(erased.signature, original.signature, "erased must preserve signature");
        assert_eq!(
            erased.collapsed_from, original.collapsed_from,
            "erased must preserve collapsed_from"
        );
        assert!(
            !erased.verify_signature(),
            "erased tombstone fails layer-1 re-hash (content changed)"
        );
    }

    #[test]
    fn test_new_canonical_from_seed_matches_new_canonical() {
        let (author, signing_key) = test_keypair();
        let deps = HashSet::new();
        let atoms = vec![Atom::Insert { at: vec!["lib".into()], content_hash: [99u8; 32] }];
        let original_id = [42u8; 32];
        let seed = signing_key.to_bytes();

        let from_seed = Change::new_canonical_from_seed(
            deps.clone(),
            atoms.clone(),
            "canonical intent",
            author.clone(),
            &seed,
            original_id,
        );

        let from_key = Change::new_canonical(
            deps,
            atoms,
            "canonical intent",
            author,
            &signing_key,
            original_id,
        );

        assert_eq!(from_seed.id, from_key.id, "new_canonical_from_seed must produce the same id");
        assert_eq!(
            from_seed.signature, from_key.signature,
            "new_canonical_from_seed must produce the same signature"
        );
        assert_eq!(from_seed.collapsed_from, Some(original_id));
        assert!(from_seed.verify_signature());
    }

    #[test]
    fn test_author_type_equality() {
        assert_eq!(AuthorType::Human, AuthorType::Human);
        assert_ne!(AuthorType::Human, AuthorType::AI { confidence: 0.9, human_sponsor: None });

        let ai_a = AuthorType::AI { confidence: 0.5, human_sponsor: Some([1u8; 32]) };
        let ai_b = AuthorType::AI { confidence: 0.5, human_sponsor: Some([1u8; 32]) };
        assert_eq!(ai_a, ai_b);

        let ai_c = AuthorType::AI { confidence: 0.5, human_sponsor: Some([2u8; 32]) };
        assert_ne!(ai_a, ai_c);

        let ai_d = AuthorType::AI { confidence: 0.6, human_sponsor: Some([1u8; 32]) };
        assert_ne!(ai_a, ai_d);
    }

    #[test]
    fn test_erased_preserves_ghost_and_author_type() {
        let (author, signing_key) = test_keypair();
        let atoms = vec![Atom::Delete { at: vec!["x".into()], prior_hash: [1u8; 32] }];
        let original = Change::new_with_metadata(
            HashSet::new(),
            atoms,
            "delete node",
            author,
            AuthorType::AI { confidence: 0.8, human_sponsor: None },
            true,
            &signing_key,
        );
        let erased = original.erased();

        assert!(erased.is_ghost, "erased must preserve is_ghost");
        assert_eq!(
            erased.author_type,
            AuthorType::AI { confidence: 0.8, human_sponsor: None },
            "erased must preserve author_type"
        );
    }
}
