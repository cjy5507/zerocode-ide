//! Dense, embedding-backed memory recall — gated behind the `memory-embed`
//! feature (heavy: pulls fastembed + ort/ONNX Runtime). The default build never
//! compiles this module and stays lexical-only.
//!
//! [`DenseMemoryRetriever`] embeds each memory entry once with
//! MultilingualE5Small (384-d, `passage:`/`query:` prefixes) and ranks entries
//! by cosine similarity to the query. Entry embeddings are cached on disk
//! (`<global-project-memory>/.embcache.bin`) keyed by model id + a content signature, so
//! an unchanged index is never re-embedded.

use std::path::Path;
use std::sync::Mutex;

use core_types::{MemoryEntry, MemoryHit, MemoryRetriever};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable id for the embedding model — also the cache key, so switching models
/// invalidates a stale cache.
const MODEL_ID: &str = "multilingual-e5-small";

/// Cache file under the global memory store.
const EMBCACHE_FILE: &str = ".embcache.bin";

/// Dense retriever holding entry embeddings + the model behind a `Mutex`
/// (embedding the query needs `&mut TextEmbedding`, but `recall` is `&self`).
pub struct DenseMemoryRetriever {
    entries: Vec<MemoryEntry>,
    /// L2-normalized entry embeddings, parallel to `entries`.
    entry_embeddings: Vec<Vec<f32>>,
    model: Mutex<TextEmbedding>,
}

#[derive(Serialize, Deserialize)]
struct EmbeddingCache {
    model: String,
    signature: String,
    slugs: Vec<String>,
    embeddings: Vec<Vec<f32>>,
}

impl DenseMemoryRetriever {
    /// Build a dense retriever for `entries`, reusing cached embeddings under
    /// `memory_dir` when the content signature + model match, otherwise embedding
    /// the entries and refreshing the cache. `memory_dir` is the committed
    /// global project memory directory (the cache is machine-local state).
    ///
    /// # Errors
    /// Returns the model/embedding error as a string if the model cannot load or
    /// embedding fails.
    pub fn new(entries: Vec<MemoryEntry>, memory_dir: &Path) -> Result<Self, String> {
        let mut model = build_model()?;

        let signature = content_signature(&entries);
        let cache_path = memory_dir.join(EMBCACHE_FILE);
        let entry_embeddings = match load_cache(&cache_path, &signature, &entries) {
            Some(cached) => cached,
            None => {
                let embeddings = embed_passages(&mut model, &entries)?;
                store_cache(&cache_path, &signature, &entries, &embeddings);
                embeddings
            }
        };

        Ok(Self {
            entries,
            entry_embeddings,
            model: Mutex::new(model),
        })
    }

    /// Embed `query`, recovering from a poisoned lock by REBUILDING the model
    /// rather than reusing possibly-torn ONNX state. Returns `None` (degrading
    /// this recall) when no healthy model is available — it never embeds on a
    /// corrupt session.
    fn embed_query(&self, query: &str) -> Option<Vec<f32>> {
        let mut model = match self.model.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                // A prior recall panicked mid-embed and poisoned the lock. The
                // ONNX session may hold torn internal state, so do NOT embed on
                // it: rebuild a fresh model in place. Clear the poison ONLY after
                // a healthy model is installed — if the rebuild fails we leave the
                // lock poisoned and degrade this recall, so the next recall
                // retries the rebuild instead of silently embedding on a corrupt
                // session. Runs in `spawn_blocking` (off the render loop) and only
                // after an exceptional panic — never on the hot path.
                let mut guard = poisoned.into_inner();
                let Ok(fresh) = build_model() else {
                    return None;
                };
                *guard = fresh;
                self.model.clear_poison();
                guard
            }
        };
        match model.embed(vec![as_query(query)], None) {
            Ok(mut vectors) => vectors.pop(),
            Err(_) => None,
        }
    }
}

impl MemoryRetriever for DenseMemoryRetriever {
    fn recall(&self, query: &str, k: usize) -> Vec<MemoryHit> {
        if k == 0 || self.entries.is_empty() {
            return Vec::new();
        }
        let Some(mut query_vector) = self.embed_query(query) else {
            return Vec::new();
        };
        l2_normalize(&mut query_vector);

        let mut hits: Vec<MemoryHit> = self
            .entries
            .iter()
            .zip(&self.entry_embeddings)
            .map(|(entry, embedding)| {
                // Both vectors are L2-normalized, so the dot product is cosine.
                let cosine = dot(&query_vector, embedding).clamp(0.0, 1.0);
                MemoryHit {
                    entry: entry.clone(),
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    score: (cosine * 10_000.0) as u32,
                }
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.entry.slug.cmp(&b.entry.slug))
        });
        hits.truncate(k);
        hits
    }
}

/// Build the embedding model. Shared by [`DenseMemoryRetriever::new`] and the
/// poison-recovery path in `recall`, so both construct it identically.
fn build_model() -> Result<TextEmbedding, String> {
    TextEmbedding::try_new(
        InitOptions::new(EmbeddingModel::MultilingualE5Small).with_show_download_progress(false),
    )
    .map_err(|error| error.to_string())
}

/// E5 models are trained with `passage:`/`query:` prefixes; keep them.
fn as_query(text: &str) -> String {
    format!("query: {text}")
}

fn as_passage(entry: &MemoryEntry) -> String {
    format!("passage: {} {} {}", entry.slug, entry.path, entry.summary)
}

fn embed_passages(
    model: &mut TextEmbedding,
    entries: &[MemoryEntry],
) -> Result<Vec<Vec<f32>>, String> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let passages: Vec<String> = entries.iter().map(as_passage).collect();
    let mut embeddings = model.embed(passages, None).map_err(|e| e.to_string())?;
    for vector in &mut embeddings {
        l2_normalize(vector);
    }
    Ok(embeddings)
}

/// Content signature over the entries (slug + summary), so any index edit
/// invalidates the cache without depending on filesystem mtimes.
fn content_signature(entries: &[MemoryEntry]) -> String {
    let mut hasher = Sha256::new();
    for entry in entries {
        hasher.update(entry.slug.as_bytes());
        hasher.update([0x1f]);
        hasher.update(entry.summary.as_bytes());
        hasher.update([0x1e]);
    }
    format!("{:x}", hasher.finalize())
}

/// Load cached embeddings only when the model, content signature, and slug set
/// all match — otherwise the cache is stale and ignored.
fn load_cache(path: &Path, signature: &str, entries: &[MemoryEntry]) -> Option<Vec<Vec<f32>>> {
    let bytes = std::fs::read(path).ok()?;
    let cache: EmbeddingCache = serde_json::from_slice(&bytes).ok()?;
    let slugs: Vec<&str> = entries.iter().map(|entry| entry.slug.as_str()).collect();
    if cache.model == MODEL_ID
        && cache.signature == signature
        && cache.slugs == slugs
        && cache.embeddings.len() == entries.len()
    {
        Some(cache.embeddings)
    } else {
        None
    }
}

fn store_cache(path: &Path, signature: &str, entries: &[MemoryEntry], embeddings: &[Vec<f32>]) {
    let cache = EmbeddingCache {
        model: MODEL_ID.to_string(),
        signature: signature.to_string(),
        slugs: entries.iter().map(|entry| entry.slug.clone()).collect(),
        embeddings: embeddings.to_vec(),
    };
    if let Ok(bytes) = serde_json::to_vec(&cache) {
        let _ = std::fs::write(path, bytes);
    }
}

fn l2_normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector {
            *value /= norm;
        }
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}
