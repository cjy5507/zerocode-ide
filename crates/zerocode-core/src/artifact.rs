//! What an agent made, and where it came from (t-2720, Orca gap #1).
//!
//! Orca's `artifacts` view knows one thing about a file: the session it was
//! made in. This window already makes such files in four places — automation
//! evidence folders, worker reports under `/tmp`, Computer Use and browser
//! screenshots, workflow outputs — and each of them carries a richer answer to
//! "where did this come from" than a session id: the run, the task, the
//! worker, the pane, the agent and model, the checkout. This module is that
//! vocabulary and nothing else: no disk, no window, no ledger — the shell's
//! `artifact_runtime` stores and scans, this file says what a row IS.
//!
//! Three rules the design (docs/design/artifacts-view.md §1) makes
//! non-negotiable and this file keeps:
//!
//! - **One table for every number.** Preview lengths, scan bounds, retention,
//!   cache caps — all of them are fields of [`Limits`], and a settings
//!   overlay (`artifacts.*`) is the only way any of them changes. Nothing in
//!   the shell or the window spells a second copy.
//! - **The kind is judged by a table** ([`kind_of`]): extension first, then the
//!   place the file was found, because `report.md` inside an evidence folder
//!   is evidence and `report.md` a worker handed in is a report.
//! - **The origin is what the ledger knows.** Every field of [`Origin`] is an
//!   `Option`, absent when nobody recorded it — never inferred from a path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::orchestration::RETENTION_DEFAULT_DAYS;
use crate::second_brain_related::tokenize;

/// The kinds a card can wear. `Other` is the honest answer for a file the
/// table does not know, never a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Report,
    Screenshot,
    Evidence,
    Export,
    Transcript,
    /// A rendered page an agent wrote into its project — HTML or SVG
    /// (t-3233 §2b). Pointed at, never copied; thumbnailed by the browser.
    Page,
    /// A document an agent wrote into its project — Markdown or PDF (t-3233
    /// §2b). Markdown keeps its first block as the preview.
    Document,
    /// A claude.ai artifact the transcript saw published (t-3233 §2a): no
    /// file, a url.
    Web,
    Other,
}

impl ArtifactKind {
    /// Every kind, in the order the view's segment control offers them.
    pub const ALL: [Self; 9] = [
        Self::Report,
        Self::Screenshot,
        Self::Evidence,
        Self::Export,
        Self::Transcript,
        Self::Page,
        Self::Document,
        Self::Web,
        Self::Other,
    ];

    /// The wire word — the same one `serde` writes, spelled once.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Report => "report",
            Self::Screenshot => "screenshot",
            Self::Evidence => "evidence",
            Self::Export => "export",
            Self::Transcript => "transcript",
            Self::Page => "page",
            Self::Document => "document",
            Self::Web => "web",
            Self::Other => "other",
        }
    }

    /// Whether the card's face is a rendered thumbnail the browser door
    /// takes (t-3233 §3): pages and claude.ai artifacts. Screenshots draw
    /// themselves, documents draw their first block, the rest wear a glyph.
    #[must_use]
    pub const fn renders_thumbnail(self) -> bool {
        matches!(self, Self::Page | Self::Web)
    }

    /// The wire word read back; `None` for a word this build does not know,
    /// so a filter the window sends is refused rather than matched to nothing.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == word)
    }
}

/// Where a file was found — the half of the kind judgement an extension
/// cannot make on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A worker's closing report, copied into the store on `worker_done`.
    WorkerReport,
    /// A file inside an automation run's evidence folder, registered in place.
    Evidence,
    /// A file from a folder the person named as an export folder.
    Export,
    /// A page or document an agent's transcript says it wrote into its
    /// project (t-3233 §2b). Registered where it is; the row goes when the
    /// file goes.
    AgentPage,
    /// A claude.ai artifact read out of a transcript (t-3233 §2a). No file.
    Remote,
    /// Registered by hand, or by a caller that knows nothing more.
    #[default]
    Manual,
}

impl Source {
    /// The store's subtree a copied file lands in (`<root>/<bucket>/<id>/…`).
    /// Pages and remote rows are never copied; their seat is only the place
    /// versions and thumbnails are kept beside.
    #[must_use]
    pub const fn bucket(self) -> &'static str {
        match self {
            Self::WorkerReport => "run",
            Self::Evidence => "automation",
            Self::Export | Self::Manual | Self::AgentPage | Self::Remote => "manual",
        }
    }
}

/// The extensions a transcript's `Write`/`Edit` registers as a page or a
/// document (t-3233 §2b) — lower-cased, without the dot. Everything else an
/// agent writes is code, and code is the checkout's business.
pub const PAGE_EXTENSIONS: &[&str] = &["html", "htm", "md", "svg", "png", "jpg", "jpeg", "pdf"];

/// Whether a path's extension is in [`PAGE_EXTENSIONS`].
#[must_use]
pub fn is_page_extension(path: &Path) -> bool {
    path.extension()
        .map(|held| held.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|held| PAGE_EXTENSIONS.contains(&held.as_str()))
}

/// How an agent-written file is judged (t-3233 §2b): rendered pages, read
/// documents, and pictures; the first row naming the extension wins.
const PAGE_KIND_BY_EXTENSION: &[(&[&str], ArtifactKind)] = &[
    (&["html", "htm", "svg"], ArtifactKind::Page),
    (&["md", "markdown", "pdf"], ArtifactKind::Document),
    (&["png", "jpg", "jpeg"], ArtifactKind::Screenshot),
];

/// The extension table. Lower-cased extensions, without the dot; the first
/// row that names an extension wins, and a file no row names is `Other`.
const KIND_BY_EXTENSION: &[(&[&str], ArtifactKind)] = &[
    (&["md", "markdown"], ArtifactKind::Report),
    (
        &["png", "jpg", "jpeg", "gif", "webp", "bmp"],
        ArtifactKind::Screenshot,
    ),
    (&["jsonl"], ArtifactKind::Transcript),
    (
        &[
            "csv", "json", "zip", "tar", "gz", "tgz", "pdf", "html", "xlsx", "svg",
        ],
        ArtifactKind::Export,
    ),
];

/// Judge a file's kind from its extension and where it was found.
///
/// The extension answers first. The source then corrects the two cases an
/// extension gets wrong: everything inside an evidence folder is evidence
/// (its `steps.jsonl` is not a transcript, its `report.md` is not a worker's
/// report), except screenshots, which stay screenshots because that is what a
/// person looks for; and a worker's report is a report whatever it was named.
#[must_use]
pub fn kind_of(path: &Path, source: Source) -> ArtifactKind {
    let extension = path
        .extension()
        .map(|held| held.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let by_extension = KIND_BY_EXTENSION
        .iter()
        .find(|(names, _)| names.contains(&extension.as_str()))
        .map_or(ArtifactKind::Other, |(_, kind)| *kind);
    match (source, by_extension) {
        // A transcript's page has its own table: `report.md` an agent wrote
        // into its project is a document to read, not a worker's report.
        (Source::AgentPage, _) => PAGE_KIND_BY_EXTENSION
            .iter()
            .find(|(names, _)| names.contains(&extension.as_str()))
            .map_or(ArtifactKind::Other, |(_, kind)| *kind),
        (Source::Remote, _) => ArtifactKind::Web,
        (Source::Evidence, ArtifactKind::Screenshot) => ArtifactKind::Screenshot,
        (Source::Evidence, _) => ArtifactKind::Evidence,
        (Source::WorkerReport, ArtifactKind::Report | ArtifactKind::Other) => ArtifactKind::Report,
        (Source::Export, ArtifactKind::Other) => ArtifactKind::Export,
        (_, kind) => kind,
    }
}

/// Where an artifact came from — every field the ledger can vouch for, and
/// nothing it cannot. Absent is `None`; a path is never parsed for a run id.
///
/// `#[serde(default)]` on the whole struct: a row written by a build that
/// knew fewer fields still reads, and a field this build has not met is
/// dropped rather than failing the whole index line.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Origin {
    /// The purpose of the linked task, copied from its ledger record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker: Option<String>,
    /// The seat, as the ledger spells it: `<team>/<pane>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation: Option<String>,
    /// The agent session the transcript names (t-3233 §2) — Claude's
    /// `sessionId`, zo's file stem. What the transcript knows, nothing more.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The project root the transcript names (`cwd`), or the one the caller
    /// judged pages against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<PathBuf>,
}

impl Origin {
    /// Nothing recorded at all — the card's origin line then says so instead
    /// of drawing an empty separator.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// What the card shows before the file is opened. Text previews hold the
/// first [`Limits::preview_chars`] characters; an image holds only its
/// dimensions — the pixels are decoded on request, never kept in the index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Preview {
    Markdown {
        text: String,
    },
    Image {
        w: u32,
        h: u32,
    },
    Text {
        text: String,
    },
    #[default]
    None,
}

/// One row of the index: one file, its kind, its origin, its preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub kind: ArtifactKind,
    pub title: String,
    /// The file — empty for a [`ArtifactKind::Web`] row, which has none.
    pub path: PathBuf,
    pub bytes: u64,
    pub created_ms: i64,
    pub modified_ms: i64,
    /// Where a claude.ai artifact lives (t-3233 §2a). `None` for every
    /// local row; a row written before this field existed reads as `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The emoji a claude.ai artifact was published with (t-3233 §2a) — the
    /// card's glyph when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// The file a publication was made from (t-3952) — the editable source
    /// whose republish advances this row's versions. `None` for every row
    /// that is not a publication; the store's copies are never the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<PathBuf>,
    #[serde(default)]
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub preview: Preview,
    #[serde(default)]
    pub source: Source,
}

/// Every number this feature has, in one place.
///
/// A settings overlay (`artifacts.<field>`) is the only road to a different
/// value; see [`Limits::overlaid`]. Retention starts at the ledger's own
/// default so a person who never opens settings keeps reports exactly as long
/// as the ledger keeps the runs they belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Limits {
    /// Characters of a text preview kept in the index.
    pub preview_chars: usize,
    /// Characters of a title, cut from the file name.
    pub title_chars: usize,
    /// The largest text file the drawer is handed whole.
    pub preview_text_bytes_max: u64,
    /// The largest image the drawer is handed as a `data:` URL.
    pub preview_image_bytes_max: u64,
    /// The preview cache's byte cap (LRU).
    pub preview_cache_bytes: u64,
    /// How deep a registered folder is walked.
    pub scan_depth: usize,
    /// How many files one scan pass looks at before reporting truncation.
    pub scan_files_max: usize,
    /// How many bytes one scan pass reads for previews and body tokens.
    pub scan_bytes_max: u64,
    /// The largest text file whose body is tokenised for search.
    pub body_index_bytes_max: u64,
    /// Tokens kept per artifact for body search.
    pub tokens_per_artifact_max: usize,
    /// Rows the index holds before the oldest are refused.
    pub index_rows_max: usize,
    /// Rows one listing answer carries.
    pub list_rows_max: usize,
    /// Registered folders the file watcher's artifact lane follows.
    pub watch_dirs_max: usize,
    /// Folders the scan walks; the newest registrations keep their seats.
    pub scan_sources_max: usize,
    /// Days a row and its copied file are kept. `0` follows the ledger.
    pub retention_days: u32,
    /// The most a settings overlay may ask for.
    pub retention_days_max: u32,
    /// The rendered thumbnail's logical width, in CSS pixels (t-3233 §3).
    pub thumb_width: u32,
    /// The rendered thumbnail's logical height — 4:3 against the width.
    pub thumb_height: u32,
    /// The hidden pane's width while a page lays itself out; the picture is
    /// asked for at `thumb_width` and the height follows 4:3.
    pub thumb_viewport_width: u32,
    /// How long one thumbnail may take to load and snap before it is a glyph.
    pub thumb_timeout_ms: u64,
    /// How many cards may wait for a thumbnail at once; the rest stay glyphs
    /// until they are asked for again.
    pub thumb_queue_max: usize,
    /// Pages one agent session may register (t-3233 §2b); the newest keep
    /// their rows.
    pub pages_per_session: usize,
    /// The largest file a transcript's `Write` registers as a page.
    pub page_bytes_max: u64,
    /// Transcript files one backfill pass reads, newest first.
    pub transcript_files_max: usize,
    /// Bytes one backfill pass reads across every transcript.
    pub transcript_bytes_max: u64,
    /// Bytes read from the end of a transcript the hook road has not met.
    pub transcript_tail_bytes: u64,
    /// Unfinished Artifact calls kept with each transcript cursor.
    pub transcript_pending_max: usize,
    /// Snapshots kept per page or document (t-3233 §5); the oldest go first.
    pub versions_per_artifact_max: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            preview_chars: 600,
            title_chars: 80,
            preview_text_bytes_max: 512 * 1024,
            preview_image_bytes_max: 4 * 1024 * 1024,
            preview_cache_bytes: 16 * 1024 * 1024,
            scan_depth: 4,
            scan_files_max: 5_000,
            scan_bytes_max: 64 * 1024 * 1024,
            body_index_bytes_max: 256 * 1024,
            tokens_per_artifact_max: 512,
            index_rows_max: 10_000,
            list_rows_max: 2_000,
            watch_dirs_max: 64,
            scan_sources_max: 256,
            retention_days: RETENTION_DEFAULT_DAYS,
            retention_days_max: 365,
            thumb_width: 320,
            thumb_height: 240,
            thumb_viewport_width: 1024,
            thumb_timeout_ms: 8_000,
            thumb_queue_max: 24,
            pages_per_session: 64,
            page_bytes_max: 8 * 1024 * 1024,
            transcript_files_max: 200,
            transcript_bytes_max: 64 * 1024 * 1024,
            transcript_tail_bytes: 256 * 1024,
            transcript_pending_max: 128,
            versions_per_artifact_max: 10,
        }
    }
}

/// The overlay's key prefix; `artifacts.retention_days` names
/// [`Limits::retention_days`].
pub const OVERLAY_PREFIX: &str = "artifacts.";

impl Limits {
    /// Lay a settings overlay over the table. Unknown keys are ignored, a
    /// retention above the cap is clamped, and every other field takes the
    /// value as given — the table is the authority on what exists, the
    /// overlay only on what it says.
    #[must_use]
    pub fn overlaid(mut self, overlay: &BTreeMap<String, u64>) -> Self {
        for (key, value) in overlay {
            let Some(field) = key.strip_prefix(OVERLAY_PREFIX) else {
                continue;
            };
            let value = *value;
            let as_usize = usize::try_from(value).unwrap_or(usize::MAX);
            let as_u32 = u32::try_from(value).unwrap_or(u32::MAX);
            match field {
                "preview_chars" => self.preview_chars = as_usize,
                "title_chars" => self.title_chars = as_usize,
                "preview_text_bytes_max" => self.preview_text_bytes_max = value,
                "preview_image_bytes_max" => self.preview_image_bytes_max = value,
                "preview_cache_bytes" => self.preview_cache_bytes = value,
                "scan_depth" => self.scan_depth = as_usize,
                "scan_files_max" => self.scan_files_max = as_usize,
                "scan_bytes_max" => self.scan_bytes_max = value,
                "body_index_bytes_max" => self.body_index_bytes_max = value,
                "tokens_per_artifact_max" => self.tokens_per_artifact_max = as_usize,
                "index_rows_max" => self.index_rows_max = as_usize,
                "list_rows_max" => self.list_rows_max = as_usize,
                "watch_dirs_max" => self.watch_dirs_max = as_usize,
                "scan_sources_max" => self.scan_sources_max = as_usize,
                "retention_days" => self.retention_days = as_u32.min(self.retention_days_max),
                "thumb_width" => self.thumb_width = as_u32,
                "thumb_height" => self.thumb_height = as_u32,
                "thumb_viewport_width" => self.thumb_viewport_width = as_u32,
                "thumb_timeout_ms" => self.thumb_timeout_ms = value,
                "thumb_queue_max" => self.thumb_queue_max = as_usize,
                "pages_per_session" => self.pages_per_session = as_usize,
                "page_bytes_max" => self.page_bytes_max = value,
                "transcript_files_max" => self.transcript_files_max = as_usize,
                "transcript_bytes_max" => self.transcript_bytes_max = value,
                "transcript_tail_bytes" => self.transcript_tail_bytes = value,
                "transcript_pending_max" => self.transcript_pending_max = as_usize,
                "versions_per_artifact_max" => self.versions_per_artifact_max = as_usize,
                _ => {}
            }
        }
        self
    }

    /// The retention the sweep actually uses: the overlay's days, or — when
    /// the person left it at "follow the ledger" — the ledger's own.
    #[must_use]
    pub fn effective_retention_days(&self, ledger_days: u32) -> u32 {
        if self.retention_days == 0 {
            ledger_days.max(1)
        } else {
            self.retention_days
        }
    }
}

/// The first `max` characters of `text`, cut on a character boundary. No
/// ellipsis: the caller knows the limit and can say so itself.
#[must_use]
pub fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// The image's pixel size from its header — PNG's IHDR, JPEG's first SOF
/// marker — without decoding a pixel. `None` for anything else, including a
/// truncated header: an unknown size is not a size.
#[must_use]
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.starts_with(PNG_SIGNATURE) {
        // Signature (8) + length (4) + "IHDR" (4) + width (4) + height (4).
        if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
            return None;
        }
        let w = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        let h = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
        return (w > 0 && h > 0).then_some((w, h));
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        // Walk the marker chain to the first start-of-frame segment.
        let mut at = 2;
        while at + 9 < bytes.len() {
            if bytes[at] != 0xFF {
                return None;
            }
            let marker = bytes[at + 1];
            // Padding bytes between markers are legal.
            if marker == 0xFF {
                at += 1;
                continue;
            }
            let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
            let start_of_frame = matches!(
                marker,
                0xC0 | 0xC1
                    | 0xC2
                    | 0xC3
                    | 0xC5
                    | 0xC6
                    | 0xC7
                    | 0xC9
                    | 0xCA
                    | 0xCB
                    | 0xCD
                    | 0xCE
                    | 0xCF
            );
            if start_of_frame {
                let h = u32::from(u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]));
                let w = u32::from(u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]));
                return (w > 0 && h > 0).then_some((w, h));
            }
            if length < 2 {
                return None;
            }
            at += 2 + length;
        }
    }
    None
}

/// The preview a file gets, judged by its kind and bounded by the table.
///
/// Markdown and text keep their first characters; an image keeps its size;
/// everything else keeps nothing. Bytes that are not UTF-8 make no text
/// preview — a viewer that shows mojibake is worse than one that shows a glyph.
#[must_use]
pub fn preview_of(kind: ArtifactKind, bytes: &[u8], limits: &Limits) -> Preview {
    match kind {
        ArtifactKind::Screenshot => {
            image_dimensions(bytes).map_or(Preview::None, |(w, h)| Preview::Image { w, h })
        }
        // A document's Markdown keeps its first block; a PDF's bytes are
        // not text and fall to `None` on their own.
        ArtifactKind::Report | ArtifactKind::Document => {
            std::str::from_utf8(bytes).map_or(Preview::None, |text| Preview::Markdown {
                text: truncate_chars(text, limits.preview_chars),
            })
        }
        ArtifactKind::Evidence | ArtifactKind::Transcript | ArtifactKind::Other => {
            std::str::from_utf8(bytes).map_or(Preview::None, |text| Preview::Text {
                text: truncate_chars(text, limits.preview_chars),
            })
        }
        // Pages and claude.ai artifacts are drawn by the browser door
        // (t-3233 §3), not by their first bytes.
        ArtifactKind::Export | ArtifactKind::Page | ArtifactKind::Web => Preview::None,
    }
}

/// The card's title: the file name, cut to the table's length.
#[must_use]
pub fn title_of(path: &Path, limits: &Limits) -> String {
    let name = path
        .file_name()
        .map(|held| held.to_string_lossy().into_owned())
        .unwrap_or_default();
    truncate_chars(&name, limits.title_chars)
}

/// Prefer a document's own account and its task's purpose to storage names.
/// The caller supplies bounded bytes already read for previews/search.
#[must_use]
pub fn descriptive_title(path: &Path, bytes: &[u8], work: Option<&str>, limits: &Limits) -> String {
    let readable = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        ["md", "txt", "html", "htm", "json", "jsonl"].contains(&e.to_ascii_lowercase().as_str())
    });
    let content = readable
        .then(|| std::str::from_utf8(bytes).ok())
        .flatten()
        .and_then(|text| {
            let text = text.trim_start_matches('\u{feff}').trim();
            if text.starts_with('<') {
                return crate::artifact_publish::skeleton::title(text);
            }
            if text.starts_with('{') || text.starts_with('[') {
                return text.lines().find_map(|line| {
                    let value: serde_json::Value = serde_json::from_str(line).ok()?;
                    ["title", "summary", "goal", "description"]
                        .into_iter()
                        .find_map(|key| {
                            value
                                .get(key)?
                                .as_str()
                                .filter(|s| !s.trim().is_empty())
                                .map(str::to_string)
                        })
                });
            }
            crate::skill::document_title(text)
        });
    let meaningful = |text: &str| {
        let text = text.trim();
        !text.is_empty()
            && ![
                "report",
                "readme",
                "summary",
                "보고서",
                "작업 보고서",
                "완료 보고",
                "요약",
            ]
            .contains(&text.to_lowercase().as_str())
    };
    let title = work
        .filter(|s| meaningful(s))
        .or_else(|| content.as_deref().filter(|s| meaningful(s)))
        .or(content.as_deref());
    title.map_or_else(
        || title_of(path, limits),
        |text| {
            truncate_chars(
                &text.split_whitespace().collect::<Vec<_>>().join(" "),
                limits.title_chars,
            )
        },
    )
}

/// A stable id for one file under one origin: the same report registered
/// twice for the same worker is the same artifact, which is what makes the
/// copy on `worker_done` idempotent. Sixteen hex characters of a SHA-256 —
/// short enough for a folder name, long enough that two of them do not meet.
///
/// The session is deliberately NOT part of the id: a page an agent rewrote in
/// a later session is the same page (t-3233 §2b, "같은 경로 두 번 = 한 행").
#[must_use]
pub fn artifact_id(origin: &Origin, source_path: &Path) -> String {
    let mut hasher = Sha256::new();
    for part in [
        origin.run.as_deref(),
        origin.task.as_deref(),
        origin.worker.as_deref(),
        origin.automation.as_deref(),
    ] {
        hasher.update(part.unwrap_or("").as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(source_path.to_string_lossy().as_bytes());
    short_digest(hasher)
}

/// A stable id for one claude.ai artifact: the url alone (t-3233 §2a, "같은
/// url은 한 행"), so a second session republishing the same artifact updates
/// the row rather than adding one.
#[must_use]
pub fn artifact_id_for_url(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"url\0");
    hasher.update(url.trim().as_bytes());
    short_digest(hasher)
}

fn short_digest(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The tokens a body is searched by — the second brain's own recall
/// tokenizer, bounded. Bounded by COUNT rather than by bytes so a row's
/// footprint in memory is a number the table owns, and the earliest tokens
/// win because a report's opening lines name what it is about.
#[must_use]
pub fn body_tokens(text: &str, limits: &Limits) -> Vec<String> {
    tokenize(text)
        .into_iter()
        .take(limits.tokens_per_artifact_max)
        .collect()
}

/// Whether every token of the query occurs in the haystack — title tokens
/// and body tokens together, which is why the haystack is a sorted slice
/// rather than a set: a row keeps one `Vec`, and a binary search per query
/// token costs nothing a set would not.
#[must_use]
pub fn query_matches(query: &str, haystack: &[String]) -> bool {
    let wanted = tokenize(query);
    if wanted.is_empty() {
        return true;
    }
    wanted
        .iter()
        .all(|token| haystack.binary_search(token).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The table, row by row: extension first, source second, and the two
    /// corrections the source makes.
    #[test]
    fn descriptive_titles_follow_work_and_document_content() {
        let limits = Limits::default();
        let path = Path::new("/tmp/t-123-report.md");
        assert_eq!(
            descriptive_title(
                path,
                b"# Report",
                Some("SSH connection reuse and file transfers"),
                &limits
            ),
            "SSH connection reuse and file transfers"
        );
        assert_eq!(
            descriptive_title(
                path,
                "# 원격 파일 전송 오류 수정\n\nDetails".as_bytes(),
                None,
                &limits
            ),
            "원격 파일 전송 오류 수정"
        );
        assert_eq!(
            descriptive_title(
                path,
                b"---\ntitle: Migration plan\n---\n# Draft",
                None,
                &limits
            ),
            "Migration plan"
        );
        assert_eq!(
            descriptive_title(
                Path::new("index.html"),
                b"<title>Transfer queue</title>",
                None,
                &limits
            ),
            "Transfer queue"
        );
        assert_eq!(
            descriptive_title(
                Path::new("steps.jsonl"),
                br#"{"goal":"Verify remote file editing"}"#,
                None,
                &limits
            ),
            "Verify remote file editing"
        );
    }

    #[test]
    fn the_kind_table_judges_by_extension_then_by_source() {
        let cases: &[(&str, Source, ArtifactKind)] = &[
            ("report.md", Source::WorkerReport, ArtifactKind::Report),
            ("notes", Source::WorkerReport, ArtifactKind::Report),
            ("shot.PNG", Source::Manual, ArtifactKind::Screenshot),
            ("shot.png", Source::Evidence, ArtifactKind::Screenshot),
            ("steps.jsonl", Source::Evidence, ArtifactKind::Evidence),
            ("report.md", Source::Evidence, ArtifactKind::Evidence),
            ("turns.jsonl", Source::Manual, ArtifactKind::Transcript),
            ("rows.csv", Source::Manual, ArtifactKind::Export),
            ("bundle.bin", Source::Export, ArtifactKind::Export),
            ("bundle.bin", Source::Manual, ArtifactKind::Other),
        ];
        for (name, source, expected) in cases {
            assert_eq!(
                kind_of(Path::new(name), *source),
                *expected,
                "{name} from {source:?}"
            );
        }
        for kind in ArtifactKind::ALL {
            assert_eq!(ArtifactKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(ArtifactKind::parse("picture"), None);
    }

    /// The overlay changes only what it names, clamps retention to the
    /// table's cap, and ignores keys outside its prefix — a stray
    /// `terminal.font_size` is not a bound of ours.
    #[test]
    fn a_settings_overlay_changes_named_fields_and_nothing_else() {
        let overlay: BTreeMap<String, u64> = [
            ("artifacts.retention_days".to_string(), 7),
            ("artifacts.preview_chars".to_string(), 42),
            ("artifacts.unknown".to_string(), 9),
            ("terminal.font_size".to_string(), 13),
        ]
        .into_iter()
        .collect();
        let limits = Limits::default().overlaid(&overlay);
        assert_eq!(limits.retention_days, 7);
        assert_eq!(limits.preview_chars, 42);
        assert_eq!(limits.scan_depth, Limits::default().scan_depth);

        let clamped: BTreeMap<String, u64> = [("artifacts.retention_days".to_string(), 10_000)]
            .into_iter()
            .collect();
        assert_eq!(
            Limits::default().overlaid(&clamped).retention_days,
            Limits::default().retention_days_max
        );
        // The default follows the ledger's own table.
        assert_eq!(Limits::default().retention_days, RETENTION_DEFAULT_DAYS);
        let follows: BTreeMap<String, u64> = [("artifacts.retention_days".to_string(), 0)]
            .into_iter()
            .collect();
        assert_eq!(
            Limits::default()
                .overlaid(&follows)
                .effective_retention_days(90),
            90
        );
        assert_eq!(
            Limits::default().effective_retention_days(90),
            RETENTION_DEFAULT_DAYS
        );
    }

    /// An origin is all `Option`s with `serde(default)`: an old row with no
    /// origin reads as an empty one, a full one round-trips, and absent fields
    /// are not written.
    #[test]
    fn an_origin_reads_from_nothing_and_writes_only_what_it_knows() {
        let empty: Origin = serde_json::from_str("{}").expect("an empty origin reads");
        assert!(empty.is_empty());
        let row: Artifact = serde_json::from_str(
            r#"{"id":"a","kind":"report","title":"t","path":"/x","bytes":1,"created_ms":2,"modified_ms":3}"#,
        )
        .expect("a row without origin, tags, preview or source reads");
        assert!(row.origin.is_empty());
        assert_eq!(row.preview, Preview::None);
        assert_eq!(row.source, Source::Manual);

        let full = Origin {
            run: Some("run-1".into()),
            worker: Some("w-2".into()),
            worktree: Some(PathBuf::from("/wt")),
            ..Origin::default()
        };
        let text = serde_json::to_string(&full).expect("serialises");
        assert!(
            !text.contains("\"task\""),
            "an absent field was written: {text}"
        );
        assert_eq!(
            serde_json::from_str::<Origin>(&text).expect("reads back"),
            full
        );
        let with_unknown: Origin = serde_json::from_str(r#"{"run":"r","future_field":1}"#)
            .expect("a newer field is dropped");
        assert_eq!(with_unknown.run.as_deref(), Some("r"));
    }

    /// Previews are cut by the table and never split a character; an image
    /// keeps only its size, read from the header.
    #[test]
    fn previews_are_cut_by_the_table_and_images_keep_only_their_size() {
        let limits = Limits {
            preview_chars: 4,
            title_chars: 4,
            ..Limits::default()
        };
        assert_eq!(
            preview_of(ArtifactKind::Report, "한글과 english".as_bytes(), &limits),
            Preview::Markdown {
                text: "한글과 ".into()
            }
        );
        assert_eq!(
            preview_of(ArtifactKind::Other, b"abcdef", &limits),
            Preview::Text {
                text: "abcd".into()
            }
        );
        assert_eq!(
            preview_of(ArtifactKind::Export, b"abcdef", &limits),
            Preview::None
        );
        assert_eq!(
            preview_of(ArtifactKind::Other, &[0xFF, 0xFE, 0x00], &limits),
            Preview::None,
            "bytes that are not text make no text preview"
        );

        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0, 0, 0, 13]);
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(image_dimensions(&png), Some((640, 480)));
        assert_eq!(
            preview_of(ArtifactKind::Screenshot, &png, &limits),
            Preview::Image { w: 640, h: 480 }
        );
        assert_eq!(
            image_dimensions(&png[..20]),
            None,
            "a cut header is not a size"
        );

        // A JPEG: SOI, an APP0 segment of 16 bytes, then SOF0 with 300x200.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        jpeg.extend_from_slice(&[0u8; 14]);
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        jpeg.extend_from_slice(&200u16.to_be_bytes());
        jpeg.extend_from_slice(&300u16.to_be_bytes());
        jpeg.extend_from_slice(&[0x03, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(image_dimensions(&jpeg), Some((300, 200)));
        assert_eq!(image_dimensions(b"not an image"), None);
        assert_eq!(title_of(Path::new("/a/very-long-name.md"), &limits), "very");
    }

    /// The same file under the same origin is the same artifact; a different
    /// worker's copy of the same path is not.
    #[test]
    fn the_id_is_stable_for_one_origin_and_one_path() {
        let origin = Origin {
            run: Some("run-1".into()),
            worker: Some("w-1".into()),
            ..Origin::default()
        };
        let path = Path::new("/tmp/t-1-report.md");
        assert_eq!(artifact_id(&origin, path), artifact_id(&origin, path));
        assert_eq!(artifact_id(&origin, path).len(), 16);
        let other = Origin {
            worker: Some("w-2".into()),
            ..origin.clone()
        };
        assert_ne!(artifact_id(&origin, path), artifact_id(&other, path));
    }

    /// Body search reuses the recall tokenizer: Korean bigrams and Latin runs,
    /// bounded per row, and a query matches only when every token is there.
    #[test]
    fn body_search_speaks_the_recall_tokenizer_and_is_bounded() {
        let limits = Limits {
            tokens_per_artifact_max: 3,
            ..Limits::default()
        };
        let tokens = body_tokens("아티팩트 view report", &limits);
        assert_eq!(
            tokens.len(),
            3,
            "the row keeps the table's count: {tokens:?}"
        );
        let mut haystack = tokenize("워커 보고서 — artifacts view landed")
            .into_iter()
            .collect::<Vec<_>>();
        haystack.sort();
        assert!(query_matches("보고서 view", &haystack));
        assert!(
            query_matches("ARTIFACTS", &haystack),
            "case does not matter"
        );
        assert!(!query_matches("view missing", &haystack));
        assert!(query_matches("", &haystack), "no query matches everything");
    }
    /// The gallery's three new kinds (t-3233): a transcript's page is judged
    /// by its own table, a remote row is always `web`, and the segment order
    /// keeps the old kinds where they were.
    #[test]
    fn pages_documents_and_web_rows_are_judged_by_their_source() {
        let cases: &[(&str, Source, ArtifactKind)] = &[
            ("index.html", Source::AgentPage, ArtifactKind::Page),
            ("logo.SVG", Source::AgentPage, ArtifactKind::Page),
            ("report.md", Source::AgentPage, ArtifactKind::Document),
            ("paper.pdf", Source::AgentPage, ArtifactKind::Document),
            ("shot.png", Source::AgentPage, ArtifactKind::Screenshot),
            ("main.rs", Source::AgentPage, ArtifactKind::Other),
            ("whatever", Source::Remote, ArtifactKind::Web),
            // The old sources keep their old answers.
            ("report.md", Source::WorkerReport, ArtifactKind::Report),
            ("index.html", Source::Manual, ArtifactKind::Export),
        ];
        for (name, source, expected) in cases {
            assert_eq!(
                kind_of(Path::new(name), *source),
                *expected,
                "{name} from {source:?}"
            );
        }
        assert!(is_page_extension(Path::new("/p/a.HTML")));
        assert!(is_page_extension(Path::new("/p/a.md")));
        assert!(!is_page_extension(Path::new("/p/a.rs")));
        assert!(!is_page_extension(Path::new("/p/Makefile")));
        assert_eq!(ArtifactKind::ALL.len(), 9);
        assert_eq!(
            &ArtifactKind::ALL[..5],
            &[
                ArtifactKind::Report,
                ArtifactKind::Screenshot,
                ArtifactKind::Evidence,
                ArtifactKind::Export,
                ArtifactKind::Transcript,
            ]
        );
        assert_eq!(ArtifactKind::parse("web"), Some(ArtifactKind::Web));
        assert_eq!(ArtifactKind::parse("page"), Some(ArtifactKind::Page));
        assert_eq!(
            ArtifactKind::parse("document"),
            Some(ArtifactKind::Document)
        );
        assert!(ArtifactKind::Page.renders_thumbnail());
        assert!(ArtifactKind::Web.renders_thumbnail());
        assert!(!ArtifactKind::Document.renders_thumbnail());
        assert!(!ArtifactKind::Screenshot.renders_thumbnail());
        assert_eq!(Source::AgentPage.bucket(), "manual");
        assert_eq!(Source::Remote.bucket(), "manual");
    }

    /// `url` is absent from every row written before it existed and from
    /// every local row written after; a web row carries it. The origin's
    /// session and project ride the same `serde(default)`.
    #[test]
    fn the_url_reads_as_none_when_absent_and_is_written_only_when_present() {
        let old: Artifact = serde_json::from_str(
            r#"{"id":"a","kind":"report","title":"t","path":"/x","bytes":1,"created_ms":2,"modified_ms":3}"#,
        )
        .expect("an old row reads");
        assert_eq!(old.url, None);
        let text = serde_json::to_string(&old).expect("serialises");
        assert!(!text.contains("\"url\""), "a local row wrote a url: {text}");
        assert!(
            !text.contains("\"source_path\""),
            "a row that is not a publication wrote a source: {text}"
        );
        assert!(
            !text.contains("\"favicon\""),
            "a local row wrote a favicon: {text}"
        );
        let web = Artifact {
            id: artifact_id_for_url("https://claude.ai/code/artifact/abc"),
            kind: ArtifactKind::Web,
            title: "a page".into(),
            path: PathBuf::new(),
            bytes: 0,
            created_ms: 1,
            modified_ms: 2,
            url: Some("https://claude.ai/code/artifact/abc".into()),
            favicon: Some("🧱".into()),
            description: None,
            version: None,
            source_path: None,
            origin: Origin {
                agent: Some("claude".into()),
                session: Some("s-1".into()),
                project: Some(PathBuf::from("/p")),
                ..Origin::default()
            },
            tags: Vec::new(),
            preview: Preview::None,
            source: Source::Remote,
        };
        let text = serde_json::to_string(&web).expect("serialises");
        assert!(text.contains("\"url\":\"https://claude.ai/code/artifact/abc\""));
        assert!(text.contains("\"session\":\"s-1\"") && text.contains("\"project\":\"/p\""));
        assert!(text.contains("\"kind\":\"web\"") && text.contains("\"source\":\"remote\""));
        let back: Artifact = serde_json::from_str(&text).expect("reads back");
        assert_eq!(back, web);
        assert_eq!(
            artifact_id_for_url("https://claude.ai/code/artifact/abc"),
            artifact_id_for_url(" https://claude.ai/code/artifact/abc "),
            "the id is the url's, whitespace aside"
        );
        assert_ne!(
            artifact_id_for_url("https://claude.ai/code/artifact/abc"),
            artifact_id(
                &Origin::default(),
                Path::new("https://claude.ai/code/artifact/abc")
            ),
            "a url id never collides with a path id"
        );
        let stamped: Origin =
            serde_json::from_str(r#"{"session":"s","project":"/p"}"#).expect("reads");
        assert!(!stamped.is_empty());
    }

    /// Every new number is a field of the table, reachable through the
    /// overlay, and a document previews as Markdown while a page does not.
    #[test]
    fn the_gallery_numbers_live_in_the_table_and_a_document_previews_as_markdown() {
        let overlay: BTreeMap<String, u64> = [
            ("artifacts.thumb_width".to_string(), 400),
            ("artifacts.thumb_timeout_ms".to_string(), 1_000),
            ("artifacts.thumb_queue_max".to_string(), 3),
            ("artifacts.pages_per_session".to_string(), 5),
            ("artifacts.versions_per_artifact_max".to_string(), 2),
            ("artifacts.transcript_files_max".to_string(), 7),
        ]
        .into_iter()
        .collect();
        let limits = Limits::default().overlaid(&overlay);
        assert_eq!(limits.thumb_width, 400);
        assert_eq!(limits.thumb_height, Limits::default().thumb_height);
        assert_eq!(limits.thumb_timeout_ms, 1_000);
        assert_eq!(limits.thumb_queue_max, 3);
        assert_eq!(limits.pages_per_session, 5);
        assert_eq!(limits.versions_per_artifact_max, 2);
        assert_eq!(limits.transcript_files_max, 7);
        assert_eq!(
            Limits::default().thumb_width * 3,
            Limits::default().thumb_height * 4,
            "the default thumbnail is 4:3"
        );
        assert_eq!(
            preview_of(
                ArtifactKind::Document,
                b"# Title\n\nbody",
                &Limits::default()
            ),
            Preview::Markdown {
                text: "# Title\n\nbody".into()
            }
        );
        assert_eq!(
            preview_of(ArtifactKind::Page, b"<html>", &Limits::default()),
            Preview::None
        );
        assert_eq!(
            preview_of(ArtifactKind::Web, b"", &Limits::default()),
            Preview::None
        );
    }
}
