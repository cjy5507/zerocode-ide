//! Tree-sitter-backed workspace symbol index.
//!
//! References deliberately use exact identifier-name matching. This crate does
//! not perform semantic resolution, cross-file import resolution, or LSP work.

mod extract;
mod index;
mod language;
mod model;

pub use index::{
    CodeGraph, CodeGraphError, DEFAULT_CACHE_FILE_NAME, MAX_INDEXABLE_FILE_SIZE,
    MAX_INDEXED_FILES,
};
pub use language::LanguageSpec;
pub use model::{
    ExtractedFile, FileFingerprint, Import, IndexStatus, Position, Reference, SkippedFile,
    SkipReason, SourceRange, Symbol, SymbolKind,
};

#[cfg(test)]
mod tests;
