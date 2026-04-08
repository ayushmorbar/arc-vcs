#![forbid(unsafe_code)]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use argon2::Argon2;
use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit,
    aead::{Aead, Payload, generic_array::GenericArray},
};
use directories::BaseDirs;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

const KDF_SALT_LEN: usize = 16;
const AEAD_NONCE_LEN: usize = 12;
const DERIVED_KEY_LEN: usize = 32;

/// A loaded identity with zeroized private material.
pub struct ArcIdentity {
    pub verifying_key: VerifyingKey,
    pub signing_key: Zeroizing<[u8; 32]>,
    pub alias: String,
    pub ai_provenance: Option<AiSponsorship>,
}

impl std::fmt::Debug for ArcIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArcIdentity")
            .field("verifying_key", &self.verifying_key)
            .field("signing_key", &"<redacted>")
            .field("alias", &self.alias)
            .field("ai_provenance", &self.ai_provenance)
            .finish()
    }
}

impl Clone for ArcIdentity {
    fn clone(&self) -> Self {
        Self {
            verifying_key: self.verifying_key,
            signing_key: Zeroizing::new(*self.signing_key),
            alias: self.alias.clone(),
            ai_provenance: self.ai_provenance.clone(),
        }
    }
}

/// Human-attested provenance for an AI identity.
#[derive(Clone, Debug)]
pub struct AiSponsorship {
    pub model_name: String,
    pub sponsor_key: VerifyingKey,
    pub sponsor_signature: Signature,
}

#[derive(Debug, Error)]
pub enum KeyringError {
    #[error("invalid alias: {0}")]
    InvalidAlias(String),
    #[error("identity alias already exists: {0}")]
    AliasExists(String),
    #[error("identity not found: {0}")]
    NotFound(String),
    #[error("invalid passphrase")]
    InvalidPassphrase,
    #[error("encrypted identity payload is corrupted")]
    CorruptedCiphertext,
    #[error("no active identity loaded")]
    NoActiveIdentity,
    #[error("ai sponsorship signature verification failed")]
    InvalidAiSponsorship,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("crypto failure: {0}")]
    Crypto(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedIdentity {
    version: u8,
    alias: String,
    verifying_key: Vec<u8>,
    salt: Vec<u8>,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
    ai_provenance: Option<PersistedAiSponsorship>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedAiSponsorship {
    model_name: String,
    sponsor_key: Vec<u8>,
    sponsor_signature: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SessionState {
    active_alias: String,
}

/// Filesystem-backed identity manager.
pub struct IdentityManager {
    identity_dir: PathBuf,
    active_identity: Mutex<Option<ArcIdentity>>,
}

impl IdentityManager {
    /// Initialize keyring storage at ~/.arc/identity with strict local permissions.
    pub fn init() -> Result<Self, KeyringError> {
        let base = BaseDirs::new().ok_or_else(|| {
            KeyringError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "cannot resolve home directory",
            ))
        })?;
        let root = base.home_dir().join(".arc").join("identity");
        Self::init_at(root)
    }

    /// Initialize keyring storage at a custom path.
    pub fn init_at(path: impl AsRef<Path>) -> Result<Self, KeyringError> {
        let identity_dir = path.as_ref().to_path_buf();
        fs::create_dir_all(&identity_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&identity_dir, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            identity_dir,
            active_identity: Mutex::new(None),
        })
    }

    /// Generate, encrypt, and persist a new identity as <alias>.json.
    pub fn generate(&self, alias: &str, passphrase: &str) -> Result<VerifyingKey, KeyringError> {
        validate_alias(alias)?;
        let path = self.alias_path(alias);
        if path.exists() {
            return Err(KeyringError::AliasExists(alias.to_string()));
        }

        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();

        let mut salt = vec![0u8; KDF_SALT_LEN];
        OsRng.fill_bytes(&mut salt);
        let mut nonce = vec![0u8; AEAD_NONCE_LEN];
        OsRng.fill_bytes(&mut nonce);

        let key = derive_encryption_key(passphrase, &salt)?;
        let cipher = ChaCha20Poly1305::new(GenericArray::from_slice(&key));
        let mut plaintext = signing_key.to_bytes();
        let aad = identity_aad(alias, &persisted_vk_bytes(verifying_key));
        let ciphertext = cipher
            .encrypt(
                GenericArray::from_slice(&nonce),
                Payload {
                    msg: plaintext.as_ref(),
                    aad: &aad,
                },
            )
            .map_err(|_| KeyringError::Crypto("encryption failed".to_string()))?;
        plaintext.zeroize();

        let persisted = PersistedIdentity {
            version: 1,
            alias: alias.to_string(),
            verifying_key: persisted_vk_bytes(verifying_key),
            salt,
            nonce,
            ciphertext,
            ai_provenance: None,
        };
        let json = serde_json::to_vec_pretty(&persisted)?;
        atomic_write(&path, &json)?;
        let mut key = key;
        key.zeroize();
        Ok(verifying_key)
    }

    /// Load and decrypt identity into memory as the active signing identity.
    pub fn load(&self, alias: &str, passphrase: &str) -> Result<ArcIdentity, KeyringError> {
        validate_alias(alias)?;
        let path = self.alias_path(alias);
        if !path.exists() {
            return Err(KeyringError::NotFound(alias.to_string()));
        }

        let bytes = fs::read(path)?;
        let persisted: PersistedIdentity = serde_json::from_slice(&bytes)
            .map_err(|_| KeyringError::CorruptedCiphertext)?;

        if persisted.alias != alias {
            return Err(KeyringError::CorruptedCiphertext);
        }

        if persisted.verifying_key.len() != 32
            || persisted.salt.len() != KDF_SALT_LEN
            || persisted.nonce.len() != AEAD_NONCE_LEN
        {
            return Err(KeyringError::CorruptedCiphertext);
        }

        let key = derive_encryption_key(passphrase, &persisted.salt)?;
        let cipher = ChaCha20Poly1305::new(GenericArray::from_slice(&key));
        let aad = identity_aad(alias, &persisted.verifying_key);
        let mut decrypted = cipher
            .decrypt(
                GenericArray::from_slice(&persisted.nonce),
                Payload {
                    msg: persisted.ciphertext.as_ref(),
                    aad: &aad,
                },
            )
            .map_err(|_| KeyringError::InvalidPassphrase)?;

        if decrypted.len() != 32 {
            return Err(KeyringError::CorruptedCiphertext);
        }

        let mut secret = [0u8; 32];
        secret.copy_from_slice(&decrypted);
        decrypted.zeroize();
        let signing_key = SigningKey::from_bytes(&secret);
        let derived_verifying = signing_key.verifying_key();

        let stored_vk = VerifyingKey::from_bytes(
            &persisted
                .verifying_key
                .clone()
                .try_into()
                .map_err(|_| KeyringError::CorruptedCiphertext)?,
        )
        .map_err(|_| KeyringError::CorruptedCiphertext)?;

        if stored_vk != derived_verifying {
            return Err(KeyringError::CorruptedCiphertext);
        }

        let ai_provenance = parse_sponsorship(persisted.ai_provenance)?;
        let loaded = ArcIdentity {
            verifying_key: stored_vk,
            signing_key: Zeroizing::new(secret),
            alias: alias.to_string(),
            ai_provenance,
        };
        let mut key = key;
        key.zeroize();
        *self.active_identity.lock().expect("poisoned mutex") = Some(loaded.clone());
        Ok(loaded)
    }

    /// Sign bytes with the active memory-resident identity.
    pub fn sign(&self, data: &[u8]) -> Result<Signature, KeyringError> {
        let guard = self.active_identity.lock().expect("poisoned mutex");
        let active = guard.as_ref().ok_or(KeyringError::NoActiveIdentity)?;
        let signer = SigningKey::from_bytes(&active.signing_key);
        Ok(signer.sign(data))
    }

    /// Bind an AI key to a human sponsor alias.
    pub fn create_ai_sponsorship(
        &self,
        sponsor_alias: &str,
        passphrase: &str,
        model_name: &str,
        ai_verifying_key: &VerifyingKey,
    ) -> Result<AiSponsorship, KeyringError> {
        let sponsor = self.load(sponsor_alias, passphrase)?;
        let sponsor_signing_key = SigningKey::from_bytes(&sponsor.signing_key);
        let payload = sponsorship_payload(model_name, ai_verifying_key);
        let sponsor_signature = sponsor_signing_key.sign(&payload);
        Ok(AiSponsorship {
            model_name: model_name.to_string(),
            sponsor_key: sponsor.verifying_key,
            sponsor_signature,
        })
    }

    /// Verify AI sponsorship against an AI verifying key.
    pub fn verify_ai_sponsorship(
        &self,
        ai_verifying_key: &VerifyingKey,
        sponsorship: &AiSponsorship,
    ) -> Result<(), KeyringError> {
        if !self.is_trusted_local_sponsor(&sponsorship.sponsor_key)? {
            return Err(KeyringError::InvalidAiSponsorship);
        }
        let payload = sponsorship_payload(&sponsorship.model_name, ai_verifying_key);
        sponsorship
            .sponsor_key
            .verify(&payload, &sponsorship.sponsor_signature)
            .map_err(|_| KeyringError::InvalidAiSponsorship)
    }

    /// List alias files in keyring storage.
    pub fn list_aliases(&self) -> Result<Vec<String>, KeyringError> {
        let mut aliases = Vec::new();
        for entry in fs::read_dir(&self.identity_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            if stem != "session" && !stem.is_empty() {
                aliases.push(stem.to_string());
            }
        }
        aliases.sort();
        Ok(aliases)
    }

    pub fn identity_dir(&self) -> &Path {
        &self.identity_dir
    }

    fn alias_path(&self, alias: &str) -> PathBuf {
        self.identity_dir.join(format!("{alias}.json"))
    }
}

/// CLI-facing facade for alias listing and session-level active identity selection.
pub struct KeyringSessionFacade {
    manager: IdentityManager,
    session_file: PathBuf,
}

impl KeyringSessionFacade {
    pub fn new(manager: IdentityManager) -> Self {
        let session_file = manager.identity_dir().join("session.json");
        Self {
            manager,
            session_file,
        }
    }

    pub fn list_aliases(&self) -> Result<Vec<String>, KeyringError> {
        self.manager.list_aliases()
    }

    pub fn select_active_identity(
        &self,
        alias: &str,
        passphrase: &str,
    ) -> Result<VerifyingKey, KeyringError> {
        let identity = self.manager.load(alias, passphrase)?;
        let state = SessionState {
            active_alias: alias.to_string(),
        };
        let payload = serde_json::to_vec_pretty(&state)?;
        atomic_write(&self.session_file, &payload)?;
        Ok(identity.verifying_key)
    }

    pub fn active_alias(&self) -> Result<Option<String>, KeyringError> {
        if !self.session_file.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&self.session_file)?;
        let state: SessionState = serde_json::from_slice(&bytes)?;
        Ok(Some(state.active_alias))
    }

    pub fn manager(&self) -> &IdentityManager {
        &self.manager
    }

    pub fn as_author_hint(&self, alias: &str) -> Result<arc_store_types::Author, KeyringError> {
        validate_alias(alias)?;
        let path = self.manager.alias_path(alias);
        if !path.exists() {
            return Err(KeyringError::NotFound(alias.to_string()));
        }
        let bytes = fs::read(path)?;
        let persisted: PersistedIdentity = serde_json::from_slice(&bytes)?;
        let key: [u8; 32] = persisted
            .verifying_key
            .try_into()
            .map_err(|_| KeyringError::CorruptedCiphertext)?;
        Ok(arc_store_types::Author::Human {
            name: alias.to_string(),
            email: format!("{alias}@local.arc"),
            key,
        })
    }
}

impl IdentityManager {
    fn is_trusted_local_sponsor(&self, sponsor_key: &VerifyingKey) -> Result<bool, KeyringError> {
        for alias in self.list_aliases()? {
            let path = self.alias_path(&alias);
            let bytes = fs::read(path)?;
            let record: PersistedIdentity = serde_json::from_slice(&bytes)?;
            if record.verifying_key == sponsor_key.to_bytes().to_vec() {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn derive_encryption_key(passphrase: &str, salt: &[u8]) -> Result<[u8; DERIVED_KEY_LEN], KeyringError> {
    let mut out = [0u8; DERIVED_KEY_LEN];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut out)
        .map_err(|_| KeyringError::Crypto("argon2 key derivation failed".to_string()))?;
    Ok(out)
}

fn validate_alias(alias: &str) -> Result<(), KeyringError> {
    if alias.is_empty()
        || !alias
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(KeyringError::InvalidAlias(alias.to_string()));
    }
    Ok(())
}

fn parse_sponsorship(
    persisted: Option<PersistedAiSponsorship>,
) -> Result<Option<AiSponsorship>, KeyringError> {
    let Some(prov) = persisted else {
        return Ok(None);
    };
    let sponsor_key_arr: [u8; 32] = prov
        .sponsor_key
        .try_into()
        .map_err(|_| KeyringError::CorruptedCiphertext)?;
    let signature_arr: [u8; 64] = prov
        .sponsor_signature
        .try_into()
        .map_err(|_| KeyringError::CorruptedCiphertext)?;
    Ok(Some(AiSponsorship {
        model_name: prov.model_name,
        sponsor_key: VerifyingKey::from_bytes(&sponsor_key_arr)
            .map_err(|_| KeyringError::CorruptedCiphertext)?,
        sponsor_signature: Signature::from_bytes(&signature_arr),
    }))
}

fn persisted_vk_bytes(verifying_key: VerifyingKey) -> Vec<u8> {
    verifying_key.to_bytes().to_vec()
}

fn identity_aad(alias: &str, verifying_key: &[u8]) -> Vec<u8> {
    let mut aad = b"arc-keyring:v1:identity:".to_vec();
    aad.extend_from_slice(alias.as_bytes());
    aad.push(0);
    aad.extend_from_slice(verifying_key);
    aad
}

fn sponsorship_payload(model_name: &str, ai_verifying_key: &VerifyingKey) -> Vec<u8> {
    let mut payload = b"arc-keyring:v1:ai-sponsorship:".to_vec();
    payload.extend_from_slice(model_name.as_bytes());
    payload.push(0);
    payload.extend_from_slice(ai_verifying_key.as_bytes());
    payload
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), KeyringError> {
    let parent = path.parent().ok_or_else(|| {
        KeyringError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing parent directory",
        ))
    })?;
    fs::create_dir_all(parent)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)?;
    Ok(())
}
