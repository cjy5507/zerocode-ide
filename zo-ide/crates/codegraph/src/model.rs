use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::test_path::is_test_path;

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

impl Import {
    /// Whether this import spells `name` — as a whole identifier in the path
    /// it was written with, or as the name it binds. The evidence the index
    /// takes that a file means one particular definition of a name: against
    /// rust-analyzer on this repository (t-5970), a file linked to a name's
    /// only definer without it was right 52% of the time.
    #[must_use]
    pub fn spells(&self, name: &str) -> bool {
        self.name.as_deref() == Some(name)
            || self
                .path
                .split(|character: char| !(character.is_alphanumeric() || character == '_'))
                .any(|token| token == name)
    }
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

/// A file's neighbours as the index's exact names can tell them.
///
/// A name links a file to the one indexed file that defines it, and only
/// when the file's own imports spell the name ([`Import::spells`]). A name
/// several files define says nothing about which a spelling meant; a name one
/// file defines, spelled without an import, is as often a local or a
/// standard-library item (a method call, a field). Precision over reach,
/// because a reader acts on the list.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileLinks {
    /// Files defining names this file spells, the most spelled first.
    pub uses: Vec<LinkedFile>,
    /// Files spelling names this file defines, the most spelling first.
    pub used_by: Vec<LinkedFile>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LinkedFile {
    pub file: PathBuf,
    /// Occurrences of the linking names between the two files.
    pub references: usize,
    /// Whether `file` reads as a test file (`is_test_path`).
    pub test: bool,
}

impl LinkedFile {
    /// Sum `(file, occurrences)` pairs per file: the most referenced first,
    /// path order between equals, each marked test or not.
    pub(crate) fn tally<'a>(pairs: impl IntoIterator<Item = (&'a Path, usize)>) -> Vec<Self> {
        let mut totals = BTreeMap::<&Path, usize>::new();
        for (file, occurrences) in pairs {
            *totals.entry(file).or_default() += occurrences;
        }
        let mut linked = totals
            .into_iter()
            .map(|(file, references)| Self {
                test: is_test_path(file),
                file: file.to_path_buf(),
                references,
            })
            .collect::<Vec<_>>();
        // Stable: equals keep the path order the map gave them.
        linked.sort_by(|left, right| right.references.cmp(&left.references));
        linked
    }
}

/// What a mention of code names in the index
/// (`CodeGraph::resolve_mentions`): a file, or one definition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "resolved", content = "at", rename_all = "snake_case")]
pub enum Resolved {
    File(PathBuf),
    Symbol(Symbol),
}

/// What changing one definition reaches, counted before the change — the
/// callers and the tests as one answer (`CodeGraph::impact`).
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Impact {
    /// The workspace-relative file holding the definition.
    pub file: PathBuf,
    /// The name's definitions in that file, in source order — a method two
    /// types there define is two.
    pub definitions: Vec<Symbol>,
    /// Occurrences meant for them (`CodeGraph::references_to`).
    pub references: usize,
    /// The files those occurrences sit in, the most first; the defining file
    /// too when it uses its own definition.
    pub files: Vec<LinkedFile>,
}

impl Impact {
    /// Files other than the defining one that reference the definition.
    #[must_use]
    pub fn callers(&self) -> usize {
        self.files.iter().filter(|linked| linked.file != self.file).count()
    }

    /// Referencing files that read as tests.
    #[must_use]
    pub fn tests(&self) -> usize {
        self.files.iter().filter(|linked| linked.test).count()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IndexStatus {
    pub indexed_files: usize,
    pub skipped_files: usize,
    pub file_limit_reached: bool,
}
