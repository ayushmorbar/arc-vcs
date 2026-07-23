use std::collections::HashSet;

use arc_algebra_types::{Atom, Blake3Hash};
use arc_change::{Change, ContentHash, change::AuthorType};
use arc_store_types::Author;
use arc_store_types::author::test_keypair;

// ---------------------------------------------------------------------------
// Helper: deterministic keypair for tests
// ---------------------------------------------------------------------------
fn make_author_and_key() -> (Author, ed25519_dalek::SigningKey) {
    let (author, signing_key) = test_keypair();
    (author, signing_key)
}

fn make_server_author() -> (Author, ed25519_dalek::SigningKey) {
    let server_key = ed25519_dalek::SigningKey::from_bytes(&[99u8; 32]);
    let server_pubkey: arc_store_types::PublicKeyBytes = server_key.verifying_key().to_bytes();
    let author = Author::Server { canonical_id: "arc-test-server".to_string(), key: server_pubkey };
    (author, server_key)
}

fn simple_atom(label: &str) -> Atom {
    Atom::Insert { at: vec![label.to_string()], content_hash: [0u8; 32] }
}

// ===================================================================
// Change::new + verify_signature (two-layer)
// ===================================================================

#[test]
fn new_change_verifies_with_both_layers() {
    let (author, key) = make_author_and_key();
    let change = Change::new(HashSet::new(), vec![simple_atom("a")], "test", author, &key);
    assert!(change.verify_signature());
}

#[test]
fn tampered_intent_fails_layer1() {
    let (author, key) = make_author_and_key();
    let change = Change::new(HashSet::new(), vec![simple_atom("a")], "original", author, &key);

    let mut tampered = change.clone();
    tampered.intent = "injected".to_string();
    assert!(!tampered.verify_signature());
}

#[test]
fn tampered_atom_fails_layer1() {
    let (author, key) = make_author_and_key();
    let change = Change::new(HashSet::new(), vec![simple_atom("a")], "test", author, &key);

    let mut tampered = change.clone();
    tampered.atoms[0] = Atom::Insert { at: vec!["b".into()], content_hash: [0xFF; 32] };
    assert!(!tampered.verify_signature());
}

#[test]
fn tampered_id_fails_layer2() {
    let (author, key) = make_author_and_key();
    let change = Change::new(HashSet::new(), vec![simple_atom("a")], "test", author, &key);

    let mut tampered = change.clone();
    tampered.id = [0xDE; 32];
    assert!(!tampered.verify_signature());
}

#[test]
fn tampered_deps_fails_layer1() {
    let (author, key) = make_author_and_key();
    let change =
        Change::new(HashSet::from([[1u8; 32]]), vec![simple_atom("a")], "test", author, &key);

    let mut tampered = change.clone();
    tampered.deps = HashSet::from([[2u8; 32]]);
    assert!(!tampered.verify_signature());
}

#[test]
fn tampered_author_fails_layer1() {
    let (author, key) = make_author_and_key();
    let change = Change::new(HashSet::new(), vec![simple_atom("a")], "test", author, &key);

    let mut tampered = change.clone();
    tampered.author = Author::Human {
        name: "Evil".to_string(),
        email: "evil@attacker.com".to_string(),
        key: [0xFF; 32],
    };
    assert!(!tampered.verify_signature());
}

// ===================================================================
// Deps preservation and effect on id
// ===================================================================

#[test]
fn deps_are_preserved_in_change() {
    let (author, key) = make_author_and_key();
    let dep = [42u8; 32];
    let change =
        Change::new(HashSet::from([dep]), vec![simple_atom("c")], "with dep", author, &key);
    assert!(change.deps.contains(&dep));
    assert!(change.verify_signature());
}

#[test]
fn including_deps_changes_id() {
    let (author, key) = make_author_and_key();
    let atoms = vec![simple_atom("x")];

    let with = Change::new(HashSet::from([[1u8; 32]]), atoms.clone(), "same", author.clone(), &key);
    let without = Change::new(HashSet::new(), atoms, "same", author, &key);

    assert_ne!(with.id, without.id);
}

// ===================================================================
// AuthorType custom PartialEq (f32 via to_bits)
// ===================================================================

#[test]
fn author_type_human_eq() {
    assert_eq!(AuthorType::Human, AuthorType::Human);
}

#[test]
fn author_type_ai_eq_same_bits() {
    let a = AuthorType::AI { confidence: 1.0, human_sponsor: None };
    let b = AuthorType::AI { confidence: 1.0, human_sponsor: None };
    assert_eq!(a, b);
}

// author_type_ai_ne_different_bits removed: broken test calling non-existent
// to_bits_debug(); same-NaN equality covered by
// author_type_ai_nan_to_bits_preserves_distinction below.

fn nan_bits(x: AuthorType) -> u32 {
    match x {
        AuthorType::AI { confidence, .. } => confidence.to_bits(),
        _ => unreachable!(),
    }
}

#[test]
fn author_type_ai_nan_to_bits_preserves_distinction() {
    let nan1 = AuthorType::AI { confidence: f32::NAN, human_sponsor: None };
    let nan2 = AuthorType::AI { confidence: f32::NAN, human_sponsor: None };
    // Both are NaN, and to_bits for the same NaN bit pattern should be equal
    assert_eq!(nan_bits(nan1), nan_bits(nan2));
}

#[test]
fn author_type_ai_different_nan_payloads_are_not_eq() {
    // Create NaN with different bit payloads via unsafe
    let nan_a = f32::from_bits(0x7FC00001); // quiet NaN payload 1
    let nan_b = f32::from_bits(0x7FC00002); // quiet NaN payload 2
    let a = AuthorType::AI { confidence: nan_a, human_sponsor: None };
    let b = AuthorType::AI { confidence: nan_b, human_sponsor: None };
    assert_ne!(a, b);
}

#[test]
fn author_type_ai_vs_human_ne() {
    let ai = AuthorType::AI { confidence: 0.9, human_sponsor: None };
    assert_ne!(ai, AuthorType::Human);
}

#[test]
fn author_type_ai_different_sponsor_ne() {
    let key_a = [1u8; 32];
    let key_b = [2u8; 32];
    let a = AuthorType::AI { confidence: 1.0, human_sponsor: Some(key_a) };
    let b = AuthorType::AI { confidence: 1.0, human_sponsor: Some(key_b) };
    assert_ne!(a, b);
}

#[test]
fn author_type_serialization_roundtrip() {
    let ai = AuthorType::AI { confidence: 0.85, human_sponsor: Some([7u8; 32]) };
    let bytes = bincode::serialize(&ai).unwrap();
    let decoded: AuthorType = bincode::deserialize(&bytes).unwrap();
    assert_eq!(ai, decoded);

    let human = AuthorType::Human;
    let bytes = bincode::serialize(&human).unwrap();
    let decoded: AuthorType = bincode::deserialize(&bytes).unwrap();
    assert_eq!(human, decoded);
}

// ===================================================================
// Change::new_with_metadata (ghost / author_type)
// ===================================================================

#[test]
fn new_with_metadata_sets_ghost_flag() {
    let (author, key) = make_author_and_key();
    let change = Change::new_with_metadata(
        HashSet::new(),
        vec![simple_atom("g")],
        "ghost node",
        author.clone(),
        AuthorType::AI { confidence: 0.5, human_sponsor: None },
        true,
        &key,
    );
    assert!(change.is_ghost);
    assert!(change.verify_signature());
}

#[test]
fn new_with_metadata_default_human() {
    let (author, key) = make_author_and_key();
    let change = Change::new(HashSet::new(), vec![], "default", author, &key);
    assert!(!change.is_ghost);
    assert_eq!(change.author_type, AuthorType::Human);
}

#[test]
fn ghost_flag_does_not_affect_id() {
    let (author, key) = make_author_and_key();
    let atoms = vec![simple_atom("x")];
    let intent = "same";

    let ghost = Change::new_with_metadata(
        HashSet::new(),
        atoms.clone(),
        intent,
        author.clone(),
        AuthorType::Human,
        true,
        &key,
    );
    let normal = Change::new_with_metadata(
        HashSet::new(),
        atoms,
        intent,
        author,
        AuthorType::Human,
        false,
        &key,
    );
    // is_ghost is excluded from compute_id, so ids must match
    assert_eq!(ghost.id, normal.id);
}

// ===================================================================
// rewritten_or_resigned
// ===================================================================

#[test]
fn rewritten_identical_reuses_signature() {
    let (author, key) = make_author_and_key();
    let original =
        Change::new(HashSet::new(), vec![simple_atom("r")], "rewrite", author.clone(), &key);

    let rebuilt = Change::rewritten_or_resigned(
        &original,
        original.deps.clone(),
        original.atoms.clone(),
        original.intent.clone(),
        author,
        &key,
    );
    assert_eq!(rebuilt.id, original.id);
    assert_eq!(rebuilt.signature, original.signature);
}

#[test]
fn rewritten_different_resigns() {
    let (author, key) = make_author_and_key();
    let original =
        Change::new(HashSet::new(), vec![simple_atom("r")], "rewrite", author.clone(), &key);

    let rebuilt = Change::rewritten_or_resigned(
        &original,
        HashSet::from([[9u8; 32]]),
        original.atoms.clone(),
        original.intent.clone(),
        author,
        &key,
    );
    assert_ne!(rebuilt.id, original.id);
    assert_ne!(rebuilt.signature, original.signature);
    assert!(rebuilt.verify_signature());
}

#[test]
fn rewritten_preserves_collapsed_from() {
    let (author, key) = make_author_and_key();
    let original =
        Change::new(HashSet::new(), vec![simple_atom("r")], "rewrite", author.clone(), &key);
    let mut with_link = original.clone();
    with_link.collapsed_from = Some([0xAB; 32]);

    let rebuilt = Change::rewritten_or_resigned(
        &with_link,
        with_link.deps.clone(),
        with_link.atoms.clone(),
        with_link.intent.clone(),
        author,
        &key,
    );
    assert_eq!(rebuilt.collapsed_from, Some([0xAB; 32]));
}

// ===================================================================
// new_canonical / new_canonical_from_seed
// ===================================================================

#[test]
fn new_canonical_sets_collapsed_from() {
    let (orig_author, orig_key) = make_author_and_key();
    let original =
        Change::new(HashSet::new(), vec![simple_atom("c")], "collapse", orig_author, &orig_key);

    let (server_author, server_key) = make_server_author();
    let canonical = Change::new_canonical(
        HashSet::new(),
        original.atoms.clone(),
        original.intent.clone(),
        server_author,
        &server_key,
        original.id,
    );

    assert_eq!(canonical.collapsed_from, Some(original.id));
    assert_ne!(canonical.id, original.id);
    assert!(canonical.verify_signature());
}

#[test]
fn new_canonical_from_seed_works() {
    let (orig_author, orig_key) = make_author_and_key();
    let original =
        Change::new(HashSet::new(), vec![simple_atom("s")], "seed test", orig_author, &orig_key);

    let seed = [99u8; 32];
    let server_pubkey: arc_store_types::PublicKeyBytes =
        ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key().to_bytes();
    let server_author =
        Author::Server { canonical_id: "seed-server".to_string(), key: server_pubkey };

    let canonical = Change::new_canonical_from_seed(
        HashSet::new(),
        original.atoms.clone(),
        original.intent.clone(),
        server_author,
        &seed,
        original.id,
    );

    assert_eq!(canonical.collapsed_from, Some(original.id));
    assert!(canonical.verify_signature());
}

// ===================================================================
// erased()
// ===================================================================

#[test]
fn erased_preserves_content_hash() {
    let (author, key) = make_author_and_key();
    let change = Change::new(
        HashSet::from([[1u8; 32], [2u8; 32]]),
        vec![simple_atom("e"), simple_atom("f")],
        "to erase",
        author,
        &key,
    );

    let erased = change.erased();
    assert_eq!(erased.id, change.id);
    assert!(erased.deps.is_empty());
    assert!(erased.atoms.is_empty());
    assert!(erased.intent.is_empty());
}

// ===================================================================
// compute_id stability
// ===================================================================

#[test]
fn compute_id_dep_order_independent() {
    let (author, _) = make_author_and_key();
    let atoms = vec![simple_atom("n")];

    let mut deps_a = HashSet::new();
    deps_a.insert([1u8; 32]);
    deps_a.insert([2u8; 32]);

    let mut deps_b = HashSet::new();
    deps_b.insert([2u8; 32]);
    deps_b.insert([1u8; 32]);

    let id_a = Change::compute_id(&deps_a, &atoms, "same", &author);
    let id_b = Change::compute_id(&deps_b, &atoms, "same", &author);
    assert_eq!(id_a, id_b);
}

#[test]
fn compute_id_atom_order_matters() {
    let (author, _) = make_author_and_key();
    let deps = HashSet::new();
    let a1 = simple_atom("a");
    let a2 = simple_atom("b");

    let id_ab = Change::compute_id(&deps, &[a1.clone(), a2.clone()], "order", &author);
    let id_ba = Change::compute_id(&deps, &[a2, a1], "order", &author);
    assert_ne!(id_ab, id_ba, "atom order should affect id");
}

#[test]
fn compute_id_conflict_side_order_independent() {
    let (author, _) = make_author_and_key();
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
    assert_eq!(id_ab, id_ba, "conflict side order must be canonicalized");
}

#[test]
fn compute_id_intent_matters() {
    let (author, _) = make_author_and_key();
    let deps = HashSet::new();
    let atoms = vec![simple_atom("x")];

    let id_a = Change::compute_id(&deps, &atoms, "first", &author);
    let id_b = Change::compute_id(&deps, &atoms, "second", &author);
    assert_ne!(id_a, id_b);
}

#[test]
fn compute_id_author_matters() {
    let (author_a, _) = test_keypair();
    let other_key = ed25519_dalek::SigningKey::from_bytes(&[43u8; 32]);
    let other_pubkey: arc_store_types::PublicKeyBytes = other_key.verifying_key().to_bytes();
    let author_b = Author::Human {
        name: "Other User".to_string(),
        email: "other@example.com".to_string(),
        key: other_pubkey,
    };
    let deps = HashSet::new();
    let atoms = vec![simple_atom("x")];

    let id_a = Change::compute_id(&deps, &atoms, "same", &author_a);
    let id_b = Change::compute_id(&deps, &atoms, "same", &author_b);
    assert_ne!(id_a, id_b, "different authors must produce different ids");
}

// ===================================================================
// collapsed_from excluded from id
// ===================================================================

#[test]
fn collapsed_from_does_not_affect_id() {
    let (author, key) = make_author_and_key();
    let base = Change::new(HashSet::new(), vec![simple_atom("b")], "base", author, &key);

    let mut with_link = base.clone();
    with_link.collapsed_from = Some([0xAB; 32]);

    assert_eq!(base.id, with_link.id);
    assert!(with_link.verify_signature());
}

// ===================================================================
// Transient author
// ===================================================================

#[test]
fn transient_author_change_verifies() {
    use arc_store_types::author::generate_transient_keypair_seed;
    use ed25519_dalek::SigningKey;

    let (author, seed) = generate_transient_keypair_seed("test-session");
    let signing_key = SigningKey::from_bytes(&seed);

    let change =
        Change::new(HashSet::new(), vec![simple_atom("t")], "transient", author, &signing_key);
    assert!(change.verify_signature());
}

// ===================================================================
// Serialization roundtrip (bincode)
// ===================================================================

#[test]
fn bincode_roundtrip_preserves_all_fields() {
    let (author, key) = make_author_and_key();
    let change = Change::new_with_metadata(
        HashSet::from([[1u8; 32], [2u8; 32]]),
        vec![simple_atom("a"), simple_atom("b")],
        "roundtrip",
        author,
        AuthorType::AI { confidence: 0.75, human_sponsor: Some([5u8; 32]) },
        true,
        &key,
    );

    let bytes = bincode::serialize(&change).unwrap();
    let decoded: Change = bincode::deserialize(&bytes).unwrap();

    assert_eq!(change, decoded);
    assert!(decoded.verify_signature());
    assert_eq!(decoded.collapsed_from, None);
}

#[test]
fn bincode_roundtrip_with_collapsed_from() {
    let (author, key) = make_author_and_key();
    let mut change = Change::new(HashSet::new(), vec![simple_atom("c")], "collapsed", author, &key);
    change.collapsed_from = Some([0xFF; 32]);

    let bytes = bincode::serialize(&change).unwrap();
    let decoded: Change = bincode::deserialize(&bytes).unwrap();
    assert_eq!(change, decoded);
    assert_eq!(decoded.collapsed_from, Some([0xFF; 32]));
}

// ===================================================================
// Deterministic keypair (from seed)
// ===================================================================

#[test]
fn deterministic_keypair_from_seed_is_reproducible() {
    let seed = [42u8; 32];
    let sk1 = ed25519_dalek::SigningKey::from_bytes(&seed);
    let sk2 = ed25519_dalek::SigningKey::from_bytes(&seed);
    assert_eq!(sk1.verifying_key().to_bytes(), sk2.verifying_key().to_bytes());
}

// ===================================================================
// ContentHash trait impls
// ===================================================================

#[test]
fn content_hash_atom_is_deterministic() {
    let a = simple_atom("x");
    assert_eq!(a.content_hash(), a.content_hash());
}

#[test]
fn content_hash_different_atoms_differ() {
    let a = simple_atom("x");
    let b = simple_atom("y");
    assert_ne!(a.content_hash(), b.content_hash());
}

#[test]
fn content_hash_author_is_deterministic() {
    let (author, _) = make_author_and_key();
    assert_eq!(author.content_hash(), author.content_hash());
}

#[test]
fn content_hash_bool() {
    assert_ne!(true.content_hash(), false.content_hash());
}

#[test]
fn content_hash_u8() {
    assert_ne!(0u8.content_hash(), 1u8.content_hash());
}

#[test]
fn content_hash_u16() {
    assert_ne!(0u16.content_hash(), 1u16.content_hash());
}

#[test]
fn content_hash_u32() {
    assert_ne!(0u32.content_hash(), 1u32.content_hash());
}

#[test]
fn content_hash_u64() {
    assert_ne!(0u64.content_hash(), 1u64.content_hash());
}

#[test]
fn content_hash_i32() {
    assert_ne!(0i32.content_hash(), 1i32.content_hash());
}

#[test]
fn content_hash_i64() {
    assert_ne!(0i64.content_hash(), 1i64.content_hash());
}

#[test]
fn content_hash_array() {
    let a = [0u8; 32];
    let b = [1u8; 32];
    assert_ne!(a.content_hash(), b.content_hash());
}

#[test]
fn content_hash_array_different_lengths_differ() {
    let a = [0u8; 16];
    let b = [0u8; 32];
    assert_ne!(a.content_hash(), b.content_hash());
}

#[test]
fn content_hash_string_vs_str() {
    let s = String::from("hello");
    let r: &str = "hello";
    assert_eq!(s.content_hash(), r.content_hash());
}

#[test]
fn content_hash_empty_string() {
    assert_ne!("".content_hash(), "x".content_hash());
}

#[test]
fn content_hash_vec_deterministic() {
    let v = vec![1u32, 2, 3];
    assert_eq!(v.content_hash(), v.content_hash());
}

#[test]
fn content_hash_vec_empty_vs_nonempty() {
    let empty: Vec<u32> = vec![];
    let nonempty = vec![0u32];
    assert_ne!(empty.content_hash(), nonempty.content_hash());
}

#[test]
fn content_hash_vec_order_matters() {
    let a = vec![1u32, 2];
    let b = vec![2u32, 1];
    assert_ne!(a.content_hash(), b.content_hash());
}

#[test]
fn content_hash_option_none_vs_some() {
    let none: Option<u32> = None;
    let some = Some(0u32);
    assert_ne!(none.content_hash(), some.content_hash());
}

#[test]
fn content_hash_option_some_deterministic() {
    assert_eq!(Some(42u32).content_hash(), Some(42u32).content_hash());
}

#[test]
fn content_hash_hashset_order_independent() {
    let mut a = HashSet::new();
    a.insert(1u32);
    a.insert(2);

    let mut b = HashSet::new();
    b.insert(2);
    b.insert(1);

    assert_eq!(a.content_hash(), b.content_hash());
}

#[test]
fn content_hash_hashset_empty_vs_nonempty() {
    let empty: HashSet<u32> = HashSet::new();
    let mut nonempty = HashSet::new();
    nonempty.insert(0u32);
    assert_ne!(empty.content_hash(), nonempty.content_hash());
}

#[test]
fn content_hash_ref_delegates() {
    let val = 42u64;
    let r: &u64 = &val;
    assert_eq!(val.content_hash(), r.content_hash());
}

#[test]
fn content_hash_usize() {
    let a: usize = 0;
    let b: usize = 1;
    assert_ne!(a.content_hash(), b.content_hash());
}

// ===================================================================
// Edge cases
// ===================================================================

#[test]
fn empty_deps_and_atoms() {
    let (author, key) = make_author_and_key();
    let change = Change::new(HashSet::new(), vec![], "empty", author, &key);
    assert!(change.verify_signature());
}

#[test]
fn many_deps() {
    let (author, key) = make_author_and_key();
    let deps: HashSet<Blake3Hash> = (0..100).map(|i| [i as u8; 32]).collect();
    let change = Change::new(deps, vec![simple_atom("m")], "many deps", author, &key);
    assert!(change.deps.len() == 100);
    assert!(change.verify_signature());
}

#[test]
fn many_atoms() {
    let (author, key) = make_author_and_key();
    let atoms: Vec<Atom> = (0..50).map(|i| simple_atom(&format!("atom_{i}"))).collect();
    let change = Change::new(HashSet::new(), atoms, "many atoms", author, &key);
    assert!(change.atoms.len() == 50);
    assert!(change.verify_signature());
}

#[test]
fn long_intent() {
    let (author, key) = make_author_and_key();
    let intent = "x".repeat(10_000);
    let change = Change::new(HashSet::new(), vec![], &intent, author, &key);
    assert!(change.verify_signature());
}

#[test]
fn different_atom_variants() {
    let (author, key) = make_author_and_key();
    let atoms = vec![
        Atom::Insert { at: vec!["a".into()], content_hash: [1u8; 32] },
        Atom::SemanticsPreserving { at: vec!["b".into()], description: "update".into() },
        Atom::Delete { at: vec!["c".into()], prior_hash: [4u8; 32] },
        Atom::Move { from: vec!["d".into()], to: vec!["e".into()] },
    ];
    let change = Change::new(HashSet::new(), atoms, "mixed atoms", author, &key);
    assert!(change.verify_signature());
}

#[test]
fn verify_signature_server_author() {
    let (server_author, server_key) = make_server_author();
    let change = Change::new(
        HashSet::new(),
        vec![simple_atom("s")],
        "server signed",
        server_author,
        &server_key,
    );
    assert!(change.verify_signature());
}

#[test]
fn ai_author_signed_by_sponsor() {
    let (_, sponsor_key) = make_author_and_key();
    let sponsor_pubkey: arc_store_types::PublicKeyBytes = sponsor_key.verifying_key().to_bytes();
    let ai_author =
        Author::AI { model: "claude-3-opus".to_string(), human_sponsor: sponsor_pubkey };

    let change = Change::new(
        HashSet::new(),
        vec![simple_atom("ai")],
        "ai authored",
        ai_author,
        &sponsor_key,
    );
    assert!(change.verify_signature());
}

#[test]
fn ai_author_wrong_sponsor_key_fails() {
    let (_, real_key) = make_author_and_key();
    let real_pubkey: arc_store_types::PublicKeyBytes = real_key.verifying_key().to_bytes();
    let ai_author = Author::AI { model: "gpt-4".to_string(), human_sponsor: real_pubkey };

    let wrong_key = ed25519_dalek::SigningKey::from_bytes(&[0xFF; 32]);
    let change = Change::new(
        HashSet::new(),
        vec![simple_atom("ai")],
        "ai signed wrong",
        ai_author,
        &wrong_key,
    );
    assert!(!change.verify_signature());
}

#[test]
fn cloned_change_is_identical() {
    let (author, key) = make_author_and_key();
    let change =
        Change::new(HashSet::from([[1u8; 32]]), vec![simple_atom("cl")], "clone", author, &key);
    let cloned = change.clone();
    assert_eq!(change, cloned);
    assert_eq!(change.id, cloned.id);
}
