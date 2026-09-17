use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Position {
    /// Zero-based source row.
    pub row: usize,
    /// Zero-based byte column.
    pub column: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceRange {
    pub start: Position,
    pub end: Position,
    pub start_byte: usize,
    pub end_byte: usize,
}

impl SourceRange {
    pub(crate) fn from_node(node: tree_sitter::Node<'_>) -> Self {
        let start = node.start_position();
        let end = node.end_position();
        Self {
            start: Position {
                row: start.row,
                column: start.column,
            },
            end: Position {
                row: end.row,
                column: end.column,
            },
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum SymbolKind {
    #[serde(rename = "fn")]
    Function,
    #[serde(rename = "struct")]
    Struct,
    #[serde(rename = "trait")]
    Trait,
    #[serde(rename = "class")]
    Class,
    #[serde(rename = "method")]
    Method,
    #[serde(rename = "const")]
    Const,
    #[serde(rename = "type")]
    Type,
    #[serde(rename = "mod")]
    Module,
    #[serde(rename = "interface")]
    Interface,
    #[serde(rename = "enum")]
    Enum,
    #[serde(rename = "union")]
    Union,
    #[serde(rename = "macro")]
    Macro,
    #[serde(rename = "static")]
    Static,
}

impl SymbolKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Function => "fn",
            Self::Struct => "struct",
            Self::Trait => "trait",
            Self::Class => "class",
            Self::Method => "method",
            Self::Const => "const",
            Self::Type => "type",
            Self::Module => "mod",
            Self::Interface => "interface",
            Self::Enum => "enum",
            Self::Union => "union",
            Self::Macro => "macro",
            Self::Static => "static",
        }
    }

    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "fn" | "function" => Some(Self::Function),
            "struct" => Some(Self::Struct),
            "trait" => Some(Self::Trait),
            "class" => Some(Self::Class),
            "method" => Some(Self::Method),
            "const" | "constant" => Some(Self::Const),
            "type" => Some(Self::Type),
            "mod" | "module" => Some(Self::Module),
            "interface" => Some(Self::Interface),
            "enum" => Some(Self::Enum),
            "union" => Some(Self::Union),
            "macro" => Some(Self::Macro),
            "static" => Some(Self::Static),
            _ => None,
        }
    }

    pub(crate) fn from_capture(capture: &str) -> Option<Self> {
        capture
            .strip_prefix("definition.")
            .and_then(Self::from_label)
    }

    pub(crate) const fn priority(self) -> u8 {
        match self {
            Self::Method => 2,
            _ => 1,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub file: PathBuf,
    pub range: SourceRange,
    pub container: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Import {
    /// Raw source text captured for the import path.
    pub path: String,
    /// Raw imported name or alias when the language query exposes one.
    pub name: Option<String>,
    pub file: PathBuf,
    pub range: SourceRange,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Reference {
    pub name: String,
    pub file: PathBuf,
    pub range: SourceRange,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExtractedFile {
    pub language: String,
    pub symbols: Vec<Symbol>,
    pub imports: Vec<Import>,
    pub references: Vec<Reference>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileFingerprint {
    pub modified_nanos: u128,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkipReason {
    TooLarge { size: u64, limit: u64 },
    Binary,
    ReadError { message: String },
    ParseError { message: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkippedFile {
    pub file: PathBuf,
    pub reason: SkipReason,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IndexStatus {
    pub indexed_files: usize,
    pub skipped_files: usize,
    pub file_limit_reached: bool,
}
