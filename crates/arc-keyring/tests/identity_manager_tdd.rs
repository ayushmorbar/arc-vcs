use arc_keyring::{IdentityManager, KeyringError, KeyringSessionFacade};
use ed25519_dalek::SigningKey;
use rand_core::OsRng;
use tempfile::TempDir;

fn setup() -> (TempDir, IdentityManager) {
    let dir = TempDir::new().expect("tempdir must be created");
    let manager = IdentityManager::init_at(dir.path()).expect("init must succeed");
    (dir, manager)
}

#[test]
fn init_generate_load_round_trip_identity() {
    let (_dir, manager) = setup();

    let generated_vk = manager
        .generate("alice", "correct horse battery staple")
        .expect("identity generation should succeed");
    let loaded = manager
        .load("alice", "correct horse battery staple")
        .expect("loading generated identity should succeed");

    assert_eq!(loaded.alias, "alice");
    assert_eq!(loaded.verifying_key, generated_vk);
    assert!(loaded.ai_provenance.is_none());
}

#[test]
fn sign_with_loaded_identity_produces_signature_bytes() {
    let (_dir, manager) = setup();
    manager
        .generate("bob", "passphrase-1")
        .expect("identity generation should succeed");
    manager
        .load("bob", "passphrase-1")
        .expect("load should succeed");

    let signature = manager
        .sign(b"semantic-change-digest")
        .expect("sign should succeed for loaded identity");

    assert_eq!(signature.to_bytes().len(), 64);
}

#[test]
fn load_with_wrong_passphrase_returns_invalid_passphrase_error() {
    let (dir, manager) = setup();
    manager
        .generate("carol", "good-passphrase")
        .expect("identity generation should succeed");

    let wrong_manager = IdentityManager::init_at(dir.path()).expect("init should succeed");

    let err = wrong_manager
        .load("carol", "wrong-passphrase")
        .expect_err("load must fail with wrong passphrase");
    assert!(matches!(err, KeyringError::InvalidPassphrase));
}

#[test]
fn corrupted_ciphertext_returns_corrupted_ciphertext_error() {
    let (_dir, manager) = setup();
    manager
        .generate("dana", "good-passphrase")
        .expect("identity generation should succeed");

    std::fs::write(
        manager.identity_dir().join("dana.json"),
        "{\n  \"version\": 1,\n  \"ciphertext\": \"this-is-not-valid-ciphertext\"\n}\n",
    )
    .expect("corrupting keyring file should succeed");

    let err = manager
        .load("dana", "good-passphrase")
        .expect_err("load should fail for corrupted ciphertext");
    assert!(matches!(err, KeyringError::CorruptedCiphertext));
}

#[test]
fn generate_alias_twice_without_overwrite_errors() {
    let (_dir, manager) = setup();
    manager
        .generate("eve", "passphrase-2")
        .expect("initial generation should succeed");
    let err = manager
        .generate("eve", "passphrase-2")
        .expect_err("duplicate alias without overwrite must fail");
    assert!(matches!(err, KeyringError::AliasExists(alias) if alias == "eve"));
}

#[test]
fn persisted_json_does_not_leak_plaintext_secrets_or_passphrase() {
    let passphrase = "super-secret-passphrase";
    let (_dir, manager) = setup();
    manager
        .generate("frank", passphrase)
        .expect("generation should succeed");

    let persisted = std::fs::read_to_string(manager.identity_dir().join("frank.json"))
        .expect("persisted keyring json should be readable");

    assert!(persisted.contains("verifying_key"));
    assert!(persisted.contains("ciphertext"));
    assert!(persisted.contains("salt"));
    assert!(persisted.contains("nonce"));
    assert!(persisted.contains("ciphertext"));
    assert!(!persisted.contains("secret_key"));
    assert!(!persisted.contains("signing_key"));
    assert!(!persisted.contains(passphrase));
}

#[test]
fn ai_sponsorship_signature_verifies_and_tampering_is_rejected() {
    let (_dir, manager) = setup();
    manager
        .generate("grace", "passphrase-3")
        .expect("sponsor identity generation should succeed");

    let ai_key = SigningKey::generate(&mut OsRng).verifying_key();
    let sponsorship = manager
        .create_ai_sponsorship("grace", "passphrase-3", "gpt-5.3-codex", &ai_key)
        .expect("sponsorship signing should succeed");
    manager
        .verify_ai_sponsorship(&ai_key, &sponsorship)
        .expect("valid sponsorship must verify");

    let other_ai_key = SigningKey::generate(&mut OsRng).verifying_key();

    let err = manager
        .verify_ai_sponsorship(&other_ai_key, &sponsorship)
        .expect_err("tampered sponsorship must fail verification");
    assert!(matches!(err, KeyringError::InvalidAiSponsorship));
}

#[test]
fn facade_lists_and_selects_active_alias() {
    let (_dir, manager) = setup();
    manager
        .generate("work", "p1")
        .expect("work identity should be generated");
    manager
        .generate("personal", "p2")
        .expect("personal identity should be generated");

    let facade = KeyringSessionFacade::new(manager);
    let aliases = facade.list_aliases().expect("aliases should list");
    assert_eq!(aliases, vec!["personal".to_string(), "work".to_string()]);

    facade
        .select_active_identity("work", "p1")
        .expect("select should load and persist active alias");
    let active = facade
        .active_alias()
        .expect("active alias read should succeed");
    assert_eq!(active, Some("work".to_string()));
}
