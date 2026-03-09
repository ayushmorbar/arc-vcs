//! SQLite-backed vector store for semantic intent embeddings.
//!
//! Each change's BLAKE3 hash (hex-encoded) is mapped to its 384-dimensional
//! embedding vector, persisted as a little-endian `f32` blob.
//!
//! Similarity search loads all vectors into memory and computes dot-product
//! cosine similarity.  This is optimal for repository-scale data sets (tens of
//! thousands of changes) where the entire index easily fits in memory and
//! ANN structures would add more overhead than they save.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, params};

/// A persistent store mapping change IDs to their embedding vectors.
pub struct VectorStore {
    conn: Connection,
}

impl VectorStore {
    /// Open (or create) the vector store at `path`.
    ///
    /// Automatically runs the schema migration on first open.
    pub fn open(path: &Path) -> Result<Self> {
        // Ensure parent directory exists.
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("failed to create vector store directory")?;
        }

        let conn = Connection::open(path).context("failed to open vector store database")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS embeddings (
               id     TEXT PRIMARY KEY NOT NULL,
               vector BLOB NOT NULL
             );",
        )
        .context("failed to create embeddings table")?;

        Ok(Self { conn })
    }

    /// Insert or update the embedding for `id`.
    ///
    /// The vector is serialized as a packed little-endian `f32` blob.
    pub fn upsert(&self, id: &str, vec: &[f32]) -> Result<()> {
        let blob: Vec<u8> = vec.iter().flat_map(|f| f.to_le_bytes()).collect();
        self.conn
            .execute(
                "INSERT OR REPLACE INTO embeddings (id, vector) VALUES (?1, ?2)",
                params![id, blob],
            )
            .context("failed to upsert embedding")?;
        Ok(())
    }

    /// Return the top `k` IDs sorted by descending cosine similarity to `query`.
    ///
    /// Uses dot-product similarity, which equals cosine similarity when both
    /// vectors are unit-norm (guaranteed by `AllMiniLML6V2`).
    pub fn search(&self, query: &[f32], k: usize) -> Result<Vec<(String, f32)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, vector FROM embeddings")
            .context("failed to prepare search query")?;

        let mut scores: Vec<(String, f32)> = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((id, blob))
            })
            .context("failed to execute search query")?
            .filter_map(|r| r.ok())
            .map(|(id, blob)| {
                let vec: Vec<f32> = blob
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                let score = dot_product(query, &vec);
                (id, score)
            })
            .collect();

        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scores.truncate(k);
        Ok(scores)
    }
}

fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_vector_store_upsert_and_search() {
        let f = NamedTempFile::new().unwrap();
        let store = VectorStore::open(f.path()).unwrap();

        // Three 2-dimensional unit-ish vectors.
        store.upsert("aaa", &[1.0_f32, 0.0]).unwrap();
        store.upsert("bbb", &[0.0_f32, 1.0]).unwrap();
        store.upsert("ccc", &[0.7_f32, 0.7]).unwrap();

        // Query most similar to [1.0, 0.0] → "aaa" should rank first.
        let results = store.search(&[1.0_f32, 0.0], 3).unwrap();
        assert_eq!(results[0].0, "aaa");
        assert!(results[0].1 > results[1].1);
    }
}
