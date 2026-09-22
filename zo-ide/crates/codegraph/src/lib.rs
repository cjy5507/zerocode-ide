//! Tree-sitter-backed workspace symbol index, stored in one `SQLite` file per
//! workspace and kept fresh file by file.
//!
//! References deliberately use exact identifier-name matching. This crate does
//! not perform semantic resolution, cross-file import resolution, or LSP work.

mod extract;
mod index;
mod language;
mod model;
mod positions;
mod scan;
mod store;
mod test_path;

pub use index::{
    CodeGraph, CodeGraphError, RefreshSummary, DEFAULT_CACHE_FILE_NAME, MAX_INDEXABLE_FILE_SIZE,
    MAX_INDEXED_FILES,
};
pub use language::LanguageSpec;
pub use model::{
    ExtractedFile, FileFingerprint, FileLinks, Impact, Import, IndexStatus, LinkedFile, Position,
    Reference, Resolved, SkippedFile, SkipReason, SourceRange, Symbol, SymbolKind,
};
pub use test_path::is_test_path;

#[cfg(test)]
mod tests;
