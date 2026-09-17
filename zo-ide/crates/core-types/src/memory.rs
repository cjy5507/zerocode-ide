/// One durable project-memory pointer from Zo's global project `MEMORY.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEntry {
    pub slug: String,
    pub path: String,
    pub summary: String,
}

/// A memory entry selected by a retriever for a specific query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryHit {
    pub entry: MemoryEntry,
    pub score: u32,
}

/// Domain interface for query-aware memory recall.
///
/// Implementations live outside `core-types` so runtime can choose lexical,
/// BM25, or feature-gated dense retrieval without coupling callers to an
/// indexing backend.
pub trait MemoryRetriever {
    fn recall(&self, query: &str, k: usize) -> Vec<MemoryHit>;
}
