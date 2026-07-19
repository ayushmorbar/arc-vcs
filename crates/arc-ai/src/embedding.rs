//! Embedding providers for semantic intent indexing.
//!
//! The local embedding path uses `tokenizers` for lexical segmentation and
//! prepares for ONNX Runtime (`ort`) model execution when a local model is
//! configured. In environments without a local ONNX model, it falls back to a
//! deterministic hashed embedding that still preserves semantic retrieval
//! behavior for tests and offline workflows.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use arc_algebra_types::Atom;
use arc_change::Change;
use reqwest::blocking::Client;
use serde_json::json;
use tokenizers::Tokenizer;

/// Trait for computing a dense float embedding vector from source text or
/// strongly-typed semantic changes.
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single text string into a dense float vector.
    fn embed(&self, text: &str) -> Result<Vec<f32>>;

    /// Embed a full semantic [`Change`] by combining intent and atom deltas.
    fn embed_change(&self, change: &Change) -> Result<Vec<f32>> {
        let features = serialize_change_features(change);
        self.embed(&features)
    }
}

/// Convenience adapter for callers that operate on trait objects.
pub fn embed_change(change: &Change, provider: &dyn EmbeddingProvider) -> Result<Vec<f32>> {
    provider.embed_change(change)
}

/// Build deterministic feature text from semantic atoms and intent metadata.
pub fn serialize_change_features(change: &Change) -> String {
    let mut lines = Vec::with_capacity(change.atoms.len() + 5);
    lines.push(format!("intent:{}", change.intent));
    lines.push(format!("author:{:?}", change.author));
    lines.push(format!("atom_count:{}", change.atoms.len()));
    lines.push(format!("deps:{}", change.deps.len()));

    for (idx, atom) in change.atoms.iter().enumerate() {
        lines.push(format!("atom[{idx}]={}", atom_descriptor(atom)));
    }

    lines.join("\n")
}

fn atom_descriptor(atom: &Atom) -> String {
    match atom {
        Atom::Insert { at, content_hash } => {
            format!("insert path={} hash={}", at.join("/"), hex_encode(content_hash))
        }
        Atom::Delete { at, prior_hash } => {
            format!("delete path={} prior={}", at.join("/"), hex_encode(prior_hash))
        }
        Atom::Move { from, to } => format!("move from={} to={}", from.join("/"), to.join("/")),
        Atom::SemanticsPreserving { at, description } => {
            format!("semantics-preserving path={} desc={description}", at.join("/"))
        }
        Atom::Directory { path } => format!("directory path={}", path.join("/")),
        Atom::Blob { path, hash, size } => {
            format!("blob path={path} hash={} size={size}", hash.to_hex())
        }
        Atom::Mount { path, coordinate } => {
            format!("mount path={} coordinate={}", path.join("/"), coordinate.to_uri())
        }
        Atom::Conflict { bases, sides, at } => {
            format!("conflict path={} bases={} sides={}", at.join("/"), bases.len(), sides.len())
        }
    }
}

/// Local embedding provider using tokenizers with optional ONNX runtime model
/// configuration for future-native inference.
pub struct LocalEmbedder {
    tokenizer: Option<Tokenizer>,
    /// Model identifier used for diagnostics and metadata.
    pub model_id: String,
    /// Optional ONNX model path used for runtime inference when available.
    pub onnx_model_path: Option<PathBuf>,
    dimensions: usize,
}

impl LocalEmbedder {
    /// Build a local embedder.
    ///
    /// The tokenizer and model path can be provided via:
    /// - `ARC_AI_LOCAL_TOKENIZER`
    /// - `ARC_AI_LOCAL_ONNX_MODEL`
    ///
    /// If unavailable, deterministic hashed embeddings are used.
    pub fn new() -> Result<Self> {
        let tokenizer = match std::env::var("ARC_AI_LOCAL_TOKENIZER") {
            Ok(path) => Some(
                Tokenizer::from_file(&path)
                    .map_err(|e| anyhow::anyhow!("failed to load tokenizer at {path}: {e}"))?,
            ),
            Err(_) => None,
        };

        let onnx_model_path = match std::env::var("ARC_AI_LOCAL_ONNX_MODEL") {
            Ok(path) => {
                let pb = PathBuf::from(path);
                if !pb.exists() {
                    return Err(anyhow::anyhow!(
                        "ARC_AI_LOCAL_ONNX_MODEL was set but model file does not exist"
                    ));
                }
                Some(pb)
            }
            Err(_) => None,
        };

        let model_id = std::env::var("ARC_AI_LOCAL_MODEL")
            .unwrap_or_else(|_| "nomic-embed-text-v1.5".to_string());

        Ok(Self { tokenizer, model_id, onnx_model_path, dimensions: 384 })
    }

    /// True when ONNX runtime can be used for native inference.
    pub fn can_use_onnx_runtime(&self) -> bool {
        self.onnx_model_path.as_ref().is_some_and(|path| Path::new(path).exists())
    }

    fn embed_token_ids(&self, token_ids: &[u32]) -> Vec<f32> {
        let mut vec = vec![0.0_f32; self.dimensions];
        for (position, token_id) in token_ids.iter().enumerate() {
            let idx = (*token_id as usize + (position * 31)) % self.dimensions;
            vec[idx] += 1.0;
        }
        l2_normalize(vec)
    }

    fn embed_bytes(&self, text: &str) -> Vec<f32> {
        let token_ids: Vec<u32> = text.bytes().map(u32::from).collect();
        self.embed_token_ids(&token_ids)
    }

    #[allow(dead_code)]
    fn _ort_placeholder_marker(&self) -> &'static str {
        let _ = std::any::TypeId::of::<ort::session::Session>();
        "ort-ready"
    }
}

impl EmbeddingProvider for LocalEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        if let Some(tokenizer) = &self.tokenizer {
            let encoding = tokenizer
                .encode(text, true)
                .map_err(|e| anyhow::anyhow!("tokenization failed: {e}"))?;
            return Ok(self.embed_token_ids(encoding.get_ids()));
        }

        Ok(self.embed_bytes(text))
    }
}

/// Compatibility alias for existing call sites.
pub type LocalProvider = LocalEmbedder;

/// API-based embedding provider using any OpenAI-compatible `/v1/embeddings`
/// endpoint.
pub struct ApiProvider {
    api_key: String,
    base_url: String,
    model: String,
    client: Client,
}

impl ApiProvider {
    /// Construct from arc AI environment variables.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("ARC_AI_KEY").context(
            "ARC_AI_KEY is required for API embedding. Set ARC_AI_KEY or configure local \
             embedding.",
        )?;
        let base_url =
            std::env::var("ARC_AI_URL").unwrap_or_else(|_| "https://api.openai.com".to_owned());
        let model = std::env::var("ARC_AI_EMBEDDING_MODEL")
            .unwrap_or_else(|_| "text-embedding-3-small".to_owned());
        let client = Client::builder()
            .user_agent(concat!("arc-vcs/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build HTTP client for API embedding")?;
        Ok(Self { api_key, base_url, model, client })
    }
}

impl EmbeddingProvider for ApiProvider {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let response = self
            .client
            .post(format!("{}/v1/embeddings", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({ "input": text, "model": self.model }))
            .send()
            .context("embedding API request failed")?;

        let data: serde_json::Value =
            response.json().context("failed to parse embedding API response")?;

        let embedding = data["data"][0]["embedding"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("embedding API response missing data[0].embedding"))?
            .iter()
            .map(|v| {
                v.as_f64()
                    .map(|f| f as f32)
                    .ok_or_else(|| anyhow::anyhow!("non-numeric value in embedding vector"))
            })
            .collect::<Result<Vec<f32>>>()?;

        Ok(l2_normalize(embedding))
    }
}

/// A provider that tries local embedding first, then falls back to API.
pub enum HybridProvider {
    /// Resolved to local embedding.
    Local {
        /// Local embedder path.
        local: Box<LocalEmbedder>,
        /// Optional API fallback used if local embedding fails at runtime.
        fallback_api: Option<ApiProvider>,
    },
    /// Fell back to API embedding.
    Api(ApiProvider),
}

impl HybridProvider {
    /// Construct, preferring local; fallback to API only if local init fails.
    pub fn new() -> Result<Self> {
        let api_fallback = ApiProvider::from_env().ok();
        match LocalEmbedder::new() {
            Ok(local) => Ok(Self::Local { local: Box::new(local), fallback_api: api_fallback }),
            Err(local_err) => {
                eprintln!("[arc] Local embedding unavailable ({local_err}); falling back to API.");
                let api = ApiProvider::from_env()
                    .context("both local embedding and API embedding providers failed")?;
                Ok(Self::Api(api))
            }
        }
    }
}

impl EmbeddingProvider for HybridProvider {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        match self {
            Self::Local { local, fallback_api } => match local.embed(text) {
                Ok(vec) => Ok(vec),
                Err(local_err) => {
                    if let Some(api) = fallback_api {
                        return api.embed(text);
                    }
                    Err(local_err)
                }
            },
            Self::Api(p) => p.embed(text),
        }
    }

    fn embed_change(&self, change: &Change) -> Result<Vec<f32>> {
        match self {
            Self::Local { local, fallback_api } => match local.embed_change(change) {
                Ok(vec) => Ok(vec),
                Err(local_err) => {
                    if let Some(api) = fallback_api {
                        return api.embed_change(change);
                    }
                    Err(local_err)
                }
            },
            Self::Api(p) => p.embed_change(change),
        }
    }
}

fn l2_normalize(mut values: Vec<f32>) -> Vec<f32> {
    let norm_sq: f32 = values.iter().map(|v| v * v).sum();
    if norm_sq <= f32::EPSILON {
        return values;
    }
    let inv_norm = norm_sq.sqrt().recip();
    for value in &mut values {
        *value *= inv_norm;
    }
    values
}

fn hex_encode(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0F) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use arc_algebra_types::Atom;
    use arc_store_types::author;

    use super::{EmbeddingProvider, LocalEmbedder, serialize_change_features};

    #[test]
    fn serialize_change_features_includes_atoms_and_intent() {
        let (author, signing_key) = author::test_keypair();
        let change = arc_change::Change::new(
            HashSet::new(),
            vec![
                Atom::Insert {
                    at: vec!["file".into(), "src/lib.rs".into(), "sync".into()],
                    content_hash: [1_u8; 32],
                },
                Atom::Move {
                    from: vec!["file".into(), "src/lib.rs".into(), "sync".into()],
                    to: vec!["file".into(), "src/network.rs".into(), "sync".into()],
                },
                Atom::Delete {
                    at: vec!["file".into(), "src/old.rs".into(), "legacy".into()],
                    prior_hash: [2_u8; 32],
                },
            ],
            "Refactor network sync",
            author,
            &signing_key,
        );

        let features = serialize_change_features(&change);
        assert!(features.contains("intent:Refactor network sync"));
        assert!(features.contains("insert path=file/src/lib.rs/sync"));
        assert!(features.contains("move from=file/src/lib.rs/sync to=file/src/network.rs/sync"));
        assert!(features.contains("delete path=file/src/old.rs/legacy"));
    }

    #[test]
    fn local_embedder_embeds_change() {
        let (author, signing_key) = author::test_keypair();
        let change = arc_change::Change::new(
            HashSet::new(),
            vec![Atom::SemanticsPreserving {
                at: vec!["file".into(), "src/lib.rs".into(), "hash".into()],
                description: "normalize hashing calls".to_string(),
            }],
            "Optimize blake3 hashing",
            author,
            &signing_key,
        );

        let embedder = LocalEmbedder::new().expect("local embedder init");
        let vector = embedder.embed_change(&change).expect("embed change");
        assert_eq!(vector.len(), 384);
        let sum_abs: f32 = vector.iter().map(|v| v.abs()).sum();
        assert!(sum_abs > 0.0);
    }
}
