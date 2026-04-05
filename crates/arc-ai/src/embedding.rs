//! Embedding providers for semantic intent indexing.
//!
//! Implements a three-tier hybrid strategy:
//!
//! 1. **Local** - `fastembed` with `AllMiniLML6V2` (384-dim, ~23 MB model
//!    downloaded to `~/.arc/models/` on first call, then cached permanently).
//! 2. **API** - OpenAI-compatible `/v1/embeddings` endpoint (same
//!    `ARC_AI_KEY` / `ARC_AI_URL` env vars as `generate_message`).
//! 3. **Hybrid** - tries Local first, falls back to API if local init fails.
//!
//! All returned vectors are 384-dimensional and unit-norm, enabling
//! dot-product cosine similarity in the
//! [`VectorStore`](super::vector_store::VectorStore).

use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde_json::json;

// -- Trait --------------------------------------------------------------------

/// Trait for computing a dense float embedding vector from a text string.
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single text string into a dense float vector.
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

// -- LocalProvider ------------------------------------------------------------

/// Local embedding provider using `fastembed` with `AllMiniLML6V2`.
///
/// Downloads the quantized model (~23 MB) to `~/.arc/models/` on first
/// initialization.  Subsequent calls skip the download and load from disk.
///
/// `TextEmbedding::embed` requires `&mut self`, so the inner model is
/// protected by a `Mutex` to satisfy the `&self` signature in the trait.
pub struct LocalProvider {
    inner: std::sync::Mutex<fastembed::TextEmbedding>,
}

impl LocalProvider {
    /// Initialize the local embedding model.
    ///
    /// Prints a one-time progress message when the model needs to be
    /// downloaded.  Stores the model in `~/.arc/models/`.
    pub fn new() -> Result<Self> {
        use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

        let cache_dir = directories::BaseDirs::new()
            .map(|d| d.home_dir().join(".arc").join("models"))
            .unwrap_or_else(|| std::path::PathBuf::from(".arc/models"));

        std::fs::create_dir_all(&cache_dir)
            .context("failed to create model cache directory ~/.arc/models/")?;

        let opts = InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_cache_dir(cache_dir);

        let model =
            TextEmbedding::try_new(opts).context("failed to initialize local embedding model")?;

        Ok(Self {
            inner: std::sync::Mutex::new(model),
        })
    }
}

impl EmbeddingProvider for LocalProvider {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let mut model = self
            .inner
            .lock()
            .map_err(|_| anyhow::anyhow!("embedding model lock was poisoned"))?;
        let mut embeddings = model
            .embed(vec![text.to_owned()], None)
            .context("local embedding computation failed")?;
        embeddings
            .pop()
            .ok_or_else(|| anyhow::anyhow!("embedding model returned an empty result"))
    }
}

// -- ApiProvider --------------------------------------------------------------

/// API-based embedding provider using any OpenAI-compatible `/v1/embeddings`
/// endpoint.
///
/// Uses `ARC_AI_KEY`, `ARC_AI_URL`, and `ARC_AI_EMBEDDING_MODEL`
/// (default: `text-embedding-3-small`).
pub struct ApiProvider {
    api_key: String,
    base_url: String,
    model: String,
    client: Client,
}

impl ApiProvider {
    /// Construct from arc AI environment variables.
    ///
    /// Returns `Err` if `ARC_AI_KEY` is unset.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("ARC_AI_KEY").context(
            "ARC_AI_KEY is required for API embedding. \
             Set ARC_AI_KEY or ensure the local model is available.",
        )?;
        let base_url =
            std::env::var("ARC_AI_URL").unwrap_or_else(|_| "https://api.openai.com".to_owned());
        let model = std::env::var("ARC_AI_EMBEDDING_MODEL")
            .unwrap_or_else(|_| "text-embedding-3-small".to_owned());
        let client = Client::builder()
            .user_agent(concat!("arc-vcs/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build HTTP client for API embedding")?;
        Ok(Self {
            api_key,
            base_url,
            model,
            client,
        })
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

        let data: serde_json::Value = response
            .json()
            .context("failed to parse embedding API response")?;

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

        Ok(embedding)
    }
}

// -- HybridProvider -----------------------------------------------------------

/// A provider that tries [`LocalProvider`] first, falling back to
/// [`ApiProvider`] if local initialization fails.
pub enum HybridProvider {
    /// Resolved to the local model.
    Local(Box<LocalProvider>),
    /// Fell back to the OpenAI-compatible API.
    Api(ApiProvider),
}

impl HybridProvider {
    /// Construct, preferring local; falls back to API on local init failure.
    pub fn new() -> Result<Self> {
        match LocalProvider::new() {
            Ok(local) => Ok(Self::Local(Box::new(local))),
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
            Self::Local(p) => p.embed(text),
            Self::Api(p) => p.embed(text),
        }
    }
}
