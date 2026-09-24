use std::cmp::Reverse;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use glob::Pattern;
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use zerocode_core::compact_diff::{compact_line_diff, CompactDiffLineKind};

/// Maximum file size that can be read (10 MB).
const MAX_READ_SIZE: u64 = 10 * 1024 * 1024;

/// Maximum file size that can be written (10 MB).
const MAX_WRITE_SIZE: usize = 10 * 1024 * 1024;

/// Default number of lines returned by an unbounded `read_file` (Claude Code
/// parity). Without a line cap the only guard was [`MAX_READ_SIZE`], so a large
/// file could flood the context on a single read; a caller that wants more pages
/// through explicit `offset`/`limit`, which are honored verbatim.
const DEFAULT_READ_LINE_LIMIT: usize = 2000;

/// Check whether a file appears to contain binary content by examining
/// the first chunk for NUL bytes.
fn is_binary_file(path: &Path) -> io::Result<bool> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut buffer = [0u8; 8192];
    let bytes_read = file.read(&mut buffer)?;
    Ok(buffer[..bytes_read].contains(&0))
}

/// Validate that a resolved path stays within the given workspace root.
/// Returns the canonical path on success, or an error if the path escapes
/// the workspace boundary (e.g. via `../` traversal or symlink).
///
/// Wired by the file-tool dispatch (`crates/tools/src/file_tools.rs`)
/// to enforce the `workspace-write` permission promise: a path that
/// resolves outside the workspace root is rejected even when the
/// canonical permission mode would otherwise allow the write.
pub fn validate_workspace_boundary(resolved: &Path, workspace_root: &Path) -> io::Result<()> {
    if !resolved.starts_with(workspace_root)
        && !additional_workspace_roots()
            .iter()
            .any(|root| resolved.starts_with(root))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "path {} escapes workspace boundary {}",
                resolved.display(),
                workspace_root.display()
            ),
        ));
    }
    Ok(())
}

fn additional_roots_cell() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    static CELL: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> = std::sync::OnceLock::new();
    CELL.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Extra workspace roots granted by `--add-dir` (CC parity). Every boundary
/// check — read confinement, write enforcement, permission policy — consults
/// this single list, so an added directory behaves exactly like the primary
/// workspace and no check site can drift.
#[must_use]
pub fn additional_workspace_roots() -> Vec<PathBuf> {
    additional_roots_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Install the `--add-dir` roots (canonicalized by the caller). Called once at
/// argument-parse time; a mutex (not `OnceLock`) so tests can install/clear.
pub fn set_additional_workspace_roots(roots: Vec<PathBuf>) {
    *additional_roots_cell()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = roots;
}

/// Text payload returned by file-reading operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextFilePayload {
    #[serde(rename = "filePath")]
    pub file_path: String,
    pub content: String,
    #[serde(rename = "numLines")]
    pub num_lines: usize,
    #[serde(rename = "startLine")]
    pub start_line: usize,
    #[serde(rename = "totalLines")]
    pub total_lines: usize,
    /// Advisory note set only when the default line cap (not an explicit
    /// `limit`) shortened the read — e.g. "showing lines 1-2000 of N — pass
    /// offset/limit to read more". Absent (and omitted from JSON) for full
    /// reads and explicit windows, so those outputs are byte-for-byte unchanged.
    #[serde(rename = "notice", default, skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
    /// Set when the global char cap shortened [`Self::content`] to a head+tail
    /// digest (`tool_output_truncation::truncate_tool_output`). The model has to
    /// be able to tell a whole window from a cut one; a silent cut would read as
    /// "this is all there is". Absent (and omitted from JSON) whenever the
    /// content fit, so every unshortened read stays byte-for-byte unchanged.
    #[serde(rename = "contentTruncated", default, skip_serializing_if = "is_false")]
    pub content_truncated: bool,
    /// Unicode char count of `content` before that cap. Absent when nothing was
    /// cut. Paired with `contentTruncated` so a consumer can size what it lost
    /// without re-reading the file.
    #[serde(
        rename = "originalContentChars",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub original_content_chars: Option<usize>,
    /// The files next to this one — what it imports, its module, its tests —
    /// resolved from the text by `file_neighbours` on a whole-file code read.
    /// Empty (and omitted) for windows, non-code files and files with nothing
    /// resolvable, so those envelopes are byte-for-byte unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub neighbours: Vec<crate::file_neighbours::Neighbour>,
}

/// `skip_serializing_if` predicate for a `bool` that defaults to false.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// Output envelope for the `read_file` tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadFileOutput {
    #[serde(rename = "type")]
    pub kind: String,
    pub file: TextFilePayload,
}

/// Structured patch hunk emitted by write and edit operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StructuredPatchHunk {
    #[serde(rename = "oldStart")]
    pub old_start: usize,
    #[serde(rename = "oldLines")]
    pub old_lines: usize,
    #[serde(rename = "newStart")]
    pub new_start: usize,
    #[serde(rename = "newLines")]
    pub new_lines: usize,
    pub lines: Vec<String>,
}

/// Output envelope for full-file write operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriteFileOutput {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "filePath")]
    pub file_path: String,
    pub content: String,
    #[serde(rename = "structuredPatch")]
    pub structured_patch: Vec<StructuredPatchHunk>,
    // Internal only (same rationale as EditFileOutput): the pre-write whole-file
    // content bloats every tool result and re-bills as cache_read each later turn.
    // `structured_patch` is what the model needs.
    #[serde(skip)]
    pub original_file: Option<String>,
    #[serde(rename = "gitDiff")]
    pub git_diff: Option<serde_json::Value>,
}

/// Output envelope for targeted string-replacement edits.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditFileOutput {
    #[serde(rename = "filePath")]
    pub file_path: String,
    #[serde(rename = "oldString")]
    pub old_string: String,
    #[serde(rename = "newString")]
    pub new_string: String,
    // Internal only: the pre-edit whole-file content. Emitting it to the model
    // re-sends the entire file (30k+ chars) on every edit, which then re-bills as
    // cache_read on every later turn — the dominant token leak on multi-edit
    // tasks (17 edits × 30k = 511k chars of accumulated context on one run).
    // `structured_patch` (the diff) is all the model needs to confirm the change;
    // keep the field for in-process callers but never put it on the wire.
    #[serde(skip)]
    pub original_file: String,
    #[serde(rename = "structuredPatch")]
    pub structured_patch: Vec<StructuredPatchHunk>,
    /// Whether the file had changed on disk (user/external edit) since the
    /// model last read it. On the model tool path this is *enforced*, not
    /// merely reported: the tools-layer read-registry guard
    /// (`FileReadRegistry` on `ToolContext`, CC parity) rejects the edit
    /// before it runs when the file was never read or changed since the last
    /// read — so a successful `edit_file` implies `false` by construction.
    /// Internal (non-model) callers of [`edit_file`] bypass the guard and
    /// keep the historical constant `false` (no detection performed).
    #[serde(rename = "userModified")]
    pub user_modified: bool,
    #[serde(rename = "replaceAll")]
    pub replace_all: bool,
    #[serde(rename = "gitDiff")]
    pub git_diff: Option<serde_json::Value>,
}

/// Result of a glob-based filename search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlobSearchOutput {
    #[serde(rename = "durationMs")]
    pub duration_ms: u128,
    #[serde(rename = "numFiles")]
    pub num_files: usize,
    pub filenames: Vec<String>,
    pub truncated: bool,
}

/// Parameters accepted by the grep-style search tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GrepSearchInput {
    pub pattern: String,
    pub path: Option<String>,
    pub glob: Option<String>,
    #[serde(rename = "output_mode")]
    pub output_mode: Option<String>,
    #[serde(rename = "-B")]
    pub before: Option<usize>,
    #[serde(rename = "-A")]
    pub after: Option<usize>,
    #[serde(rename = "-C")]
    pub context_short: Option<usize>,
    pub context: Option<usize>,
    #[serde(rename = "-n")]
    pub line_numbers: Option<bool>,
    #[serde(rename = "-i")]
    pub case_insensitive: Option<bool>,
    #[serde(rename = "type")]
    pub file_type: Option<String>,
    pub head_limit: Option<usize>,
    pub offset: Option<usize>,
    pub multiline: Option<bool>,
}

/// Result payload returned by the grep-style search tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GrepSearchOutput {
    pub mode: Option<String>,
    #[serde(rename = "numFiles")]
    pub num_files: usize,
    pub filenames: Vec<String>,
    pub content: Option<String>,
    #[serde(rename = "numLines")]
    pub num_lines: Option<usize>,
    #[serde(rename = "numMatches")]
    pub num_matches: Option<usize>,
    #[serde(rename = "appliedLimit")]
    pub applied_limit: Option<usize>,
    #[serde(rename = "appliedOffset")]
    pub applied_offset: Option<usize>,
}

/// Reads a text file and returns a line-windowed payload.
///
/// PDF files (by extension or `%PDF-` magic) and notebooks (`.ipynb`, by
/// extension) are handled in-band — Claude Code parity: a single Read tool
/// covers text, PDFs, and notebooks. Extracted text uses the same line
/// windowing.
pub fn read_file(
    path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> io::Result<ReadFileOutput> {
    let absolute_path = normalize_path(path)?;

    // Check file size before reading
    let metadata = fs::metadata(&absolute_path)?;
    if metadata.len() > MAX_READ_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "file is too large ({} bytes, max {} bytes)",
                metadata.len(),
                MAX_READ_SIZE
            ),
        ));
    }

    // PDFs are binary but readable: route them to the text extractor *before*
    // the binary-file rejection below.
    if is_pdf_file(&absolute_path)? {
        return read_pdf_file(&absolute_path, offset, limit);
    }

    // Notebooks are JSON containers, but the useful context is the ordered cell
    // text and text-output summary, not the raw wire JSON. Route by extension
    // only so ordinary `.txt` JSON is still read as text.
    if is_notebook_file(&absolute_path) {
        return read_notebook_file(&absolute_path, offset, limit);
    }

    // Detect binary files
    if is_binary_file(&absolute_path)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file appears to be binary",
        ));
    }

    let content = fs::read_to_string(&absolute_path)?;
    let neighbours = if crate::file_neighbours::wants_neighbours(&absolute_path, offset) {
        crate::file_neighbours::neighbours_of(&absolute_path, &content)
    } else {
        Vec::new()
    };
    let mut output = windowed_read_output("text", &absolute_path, &content, offset, limit);
    output.file.neighbours = neighbours;
    Ok(output)
}

/// Apply the shared line window (`offset`/`limit`) to already-loaded content.
fn windowed_read_output(
    kind: &str,
    absolute_path: &Path,
    content: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> ReadFileOutput {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let start_index = offset.unwrap_or(0).min(total_lines);
    // An explicit `limit` is honored verbatim; an unbounded read falls back to
    // the default line cap (CC parity) instead of returning the whole file.
    let effective_limit = limit.unwrap_or(DEFAULT_READ_LINE_LIMIT);
    let end_index = start_index.saturating_add(effective_limit).min(total_lines);
    let selected = lines[start_index..end_index].join("\n");
    // Annotate ONLY when the default cap (not an explicit limit) actually cut the
    // read short. A file that fits within the cap, and every explicit window, is
    // returned exactly as before — no note.
    let notice = (limit.is_none() && end_index < total_lines).then(|| {
        format!(
            "showing lines {}-{} of {total_lines} — pass offset/limit to read more",
            start_index.saturating_add(1),
            end_index,
        )
    });
    ReadFileOutput {
        kind: kind.to_string(),
        file: TextFilePayload {
            file_path: absolute_path.to_string_lossy().into_owned(),
            content: selected,
            num_lines: end_index.saturating_sub(start_index),
            start_line: start_index.saturating_add(1),
            total_lines,
            notice,
            // The line window is applied here; the CHAR cap is a separate
            // concern applied by the truncation middleware, which is the one
            // place that reads `TruncationConfig`.
            content_truncated: false,
            original_content_chars: None,
            neighbours: Vec::new(),
        },
    }
}

/// Whether the file is a PDF — `.pdf` extension (case-insensitive) or the
/// `%PDF-` magic header, so renamed/extension-less PDFs are still recognized.
fn is_pdf_file(path: &Path) -> io::Result<bool> {
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        return Ok(true);
    }
    let mut head = [0u8; 5];
    let mut file = fs::File::open(path)?;
    let read = io::Read::read(&mut file, &mut head)?;
    Ok(read >= 5 && &head[..5] == b"%PDF-")
}

fn is_notebook_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ipynb"))
}

fn read_notebook_file(
    absolute_path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
) -> io::Result<ReadFileOutput> {
    let raw = fs::read_to_string(absolute_path)?;
    let notebook: serde_json::Value = serde_json::from_str(&raw).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to parse notebook JSON: {error}"),
        )
    })?;
    let cells = notebook
        .get("cells")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid notebook: missing cells array",
            )
        })?;

    let mut content = String::new();
    for (index, cell) in cells.iter().enumerate() {
        use std::fmt::Write as _;
        if index > 0 {
            content.push('\n');
        }
        let cell_type = cell
            .get("cell_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let _ = writeln!(content, "[cell {}] ({cell_type})", index + 1);
        if cell_type == "code" {
            if let Some(count) = cell.get("execution_count") {
                if count.is_null() {
                    content.push_str("execution_count: null\n");
                } else {
                    let _ = writeln!(content, "execution_count: {count}");
                }
            }
        }
        content.push_str(&notebook_source_text(cell)?);
        if !content.ends_with('\n') {
            content.push('\n');
        }
        if cell_type == "code" {
            let summaries = notebook_output_summaries(cell);
            if !summaries.is_empty() {
                content.push_str("outputs:\n");
                for summary in summaries {
                    let _ = writeln!(content, "- {summary}");
                }
            }
        }
    }

    Ok(windowed_read_output(
        "notebook",
        absolute_path,
        &content,
        offset,
        limit,
    ))
}

fn notebook_source_text(cell: &serde_json::Value) -> io::Result<String> {
    source_value_to_string(
        cell.get("source").unwrap_or(&serde_json::Value::Null),
        "cell source",
    )
}

fn source_value_to_string(value: &serde_json::Value, field: &str) -> io::Result<String> {
    match value {
        serde_json::Value::String(text) => Ok(text.clone()),
        serde_json::Value::Array(lines) => lines
            .iter()
            .map(|line| {
                line.as_str().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid notebook: {field} contains a non-string entry"),
                    )
                })
            })
            .collect::<Result<String, _>>(),
        serde_json::Value::Null => Ok(String::new()),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid notebook: {field} must be a string or string array"),
        )),
    }
}

fn notebook_output_summaries(cell: &serde_json::Value) -> Vec<String> {
    let Some(outputs) = cell.get("outputs").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    outputs
        .iter()
        .map(summarize_notebook_output)
        .filter(|summary| !summary.is_empty())
        .collect()
}

fn summarize_notebook_output(output: &serde_json::Value) -> String {
    let output_type = output
        .get("output_type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("output");
    if let Some(text) = output
        .get("text")
        .and_then(|value| source_value_to_string(value, "output text").ok())
        .filter(|text| !text.trim().is_empty())
    {
        return format!("{output_type}: {}", text.trim_end());
    }
    if let Some(data) = output.get("data").and_then(serde_json::Value::as_object) {
        if let Some(text) = data
            .get("text/plain")
            .and_then(|value| source_value_to_string(value, "text/plain output").ok())
            .filter(|text| !text.trim().is_empty())
        {
            return format!("{output_type}: {}", text.trim_end());
        }
        let mut binary = data
            .iter()
            .filter(|(mime, _)| !mime.starts_with("text/"))
            .map(|(mime, value)| {
                let bytes = value
                    .as_str()
                    .map(str::len)
                    .or_else(|| {
                        value.as_array().map(|array| {
                            array
                                .iter()
                                .filter_map(serde_json::Value::as_str)
                                .map(str::len)
                                .sum()
                        })
                    })
                    .unwrap_or(0);
                format!("[binary output: {mime}, {bytes} bytes]")
            })
            .collect::<Vec<_>>();
        if !binary.is_empty() {
            binary.sort();
            return format!("{output_type}: {}", binary.join(", "));
        }
    }
    output_type.to_string()
}

/// Extract a PDF's text per page (`[page N]` markers) and window it like a
/// text file. Scanned/image-only PDFs yield no extractable text — surfaced as
/// an explicit note instead of an error so the model can choose a fallback.
fn read_pdf_file(
    absolute_path: &Path,
    offset: Option<usize>,
    limit: Option<usize>,
) -> io::Result<ReadFileOutput> {
    let pages = pdf_extract::extract_text_by_pages(absolute_path).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to extract PDF text: {error}"),
        )
    })?;
    let total_pages = pages.len();
    let mut content = String::new();
    for (index, page) in pages.iter().enumerate() {
        use std::fmt::Write as _;
        if index > 0 {
            content.push('\n');
        }
        let _ = writeln!(content, "[page {}]", index + 1);
        content.push_str(page.trim_end());
        content.push('\n');
    }
    if pages.iter().all(|page| page.trim().is_empty()) {
        content = format!(
            "[PDF: {total_pages} page(s) — no extractable text; likely a scanned/image-only document]\n"
        );
    }
    Ok(windowed_read_output(
        "pdf",
        absolute_path,
        &content,
        offset,
        limit,
    ))
}

static ATOMIC_REPLACE_COUNTER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Cross-process advisory lock over one JSON settings document.
///
/// `settings.json` has more than one writer — the model catalog overlay and the
/// `/connect` / `/providers` registry both read-modify-write the same file — so
/// every writer must take this lock or a concurrent save silently drops the
/// other's section. The lock is a `create_new` sibling (`settings.json.lock`)
/// holding the owning pid, polled for up to ~2 s and released on drop.
///
/// A lock whose owner died holding it is taken back (`reclaim_from_a_dead_owner`):
/// a killed process never runs its drop, and before this every later writer
/// of that file waited out its poll and gave up, for good (t-6263 — the
/// challenger arm's day book and ledger take this lock too).
#[derive(Debug)]
pub struct SettingsFileLock(PathBuf);

impl SettingsFileLock {
    /// Take the lock guarding `settings_path`, creating its parent if needed.
    ///
    /// # Errors
    ///
    /// Returns [`io::ErrorKind::WouldBlock`] when another writer holds the lock
    /// for longer than the poll budget, or the underlying filesystem error.
    pub fn acquire(settings_path: &Path) -> io::Result<Self> {
        use io::Write as _;

        if let Some(parent) = settings_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let path = settings_path.with_extension("json.lock");
        for _ in 0..200 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    return Ok(Self(path));
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if !reclaim_from_a_dead_owner(&path) {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            format!("timed out waiting for {}", path.display()),
        ))
    }
}

impl Drop for SettingsFileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

/// Remove the lock file `lock` when the process it names has died holding it
/// — the one way a lock outlives its owner, since a live owner removes its
/// own on drop. Answers whether it removed one.
///
/// Reclaimers take turns on the lock file itself — an advisory lock on the
/// very file they found, which the kernel drops with whoever holds it — and
/// each checks that the path still names that file before reading whose it
/// is. So one that waited behind another finds the file it came for already
/// gone, and never removes the fresh lock a live owner took in its place.
/// An owner still writing its pid (an empty file) is alive; so is a pid this
/// cannot read, and every pid [`process_alive`] cannot see die.
#[cfg(unix)]
fn reclaim_from_a_dead_owner(lock: &Path) -> bool {
    use io::Read as _;
    use std::os::unix::fs::MetadataExt as _;

    let Ok(mut file) = fs::File::open(lock) else {
        return false;
    };
    if file.try_lock().is_err() {
        return false;
    }
    let still_this_file = match (file.metadata(), fs::metadata(lock)) {
        (Ok(held), Ok(named)) => held.dev() == named.dev() && held.ino() == named.ino(),
        _ => false,
    };
    let mut owner = String::new();
    if !still_this_file || file.read_to_string(&mut owner).is_err() {
        return false;
    }
    owner
        .trim()
        .parse::<u32>()
        .is_ok_and(|owner| !process_alive(owner))
        && fs::remove_file(lock).is_ok()
}

/// Nowhere else is a dead owner seen dying ([`process_alive`]): a lock is
/// never taken from one.
#[cfg(not(unix))]
fn reclaim_from_a_dead_owner(_lock: &Path) -> bool {
    false
}

/// Whether `pid` is a live process. Unix signal 0 reports `EPERM` for a live
/// process owned by another user, which must stay alive rather than be
/// mistaken for a dead holder; every other answer but "no such process"
/// stays alive too. Where liveness cannot be asked, every pid is alive: a
/// lock or a lease is never taken from an owner this cannot see die.
///
/// One reader for every holder a crash can strand — a write lease
/// (`tools::file_write_lease`) and a settings lock ([`SettingsFileLock`]).
#[cfg(unix)]
#[must_use]
pub fn process_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    process_alive_from_probe(nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        None,
    ))
}

/// [`process_alive`]'s reading of one signal-0 probe.
#[cfg(unix)]
#[must_use]
pub fn process_alive_from_probe(result: Result<(), nix::errno::Errno>) -> bool {
    !matches!(result, Err(nix::errno::Errno::ESRCH))
}

/// See the Unix twin: an owner this cannot see die is alive.
#[cfg(not(unix))]
#[must_use]
pub fn process_alive(_pid: u32) -> bool {
    true
}

/// Atomically replaces a file through a unique sibling temp while preserving
/// an existing destination's permissions and resolving leaf-symlink chains.
///
/// # Errors
///
/// Returns an error when the symlink chain does not settle, destination metadata
/// cannot be read, a sibling temp cannot be created or written, or the final
/// rename fails. Failed attempts remove their temp file before returning.
pub fn replace_file_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use io::Write as _;

    // Crash/power-loss durability has no deterministic in-process reproduction,
    // so a red-first test is impractical for the fsync effect itself.
    // Plain `fs::write` followed a leaf symlink and updated its target;
    // renaming a sibling temp over the link would instead replace the link
    // itself. Resolve the leaf chain so replacement lands on the exact target
    // the tools-layer workspace boundary check validated.
    let destination = resolve_leaf_symlinks(path)?;
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if let Some(error) = crate::bash::available_disk_bytes(parent).and_then(|available| {
        crate::bash::disk_critical_error(
            available,
            crate::bash::HARD_MIN_DISK_BYTES,
            parent,
        )
    }) {
        return Err(error);
    }

    let (temp_path, mut temp_file) = create_atomic_temp_file(&destination)?;
    // The fresh temp file is umask-default; carry over an existing
    // destination's permissions so replacement does not downgrade e.g. a 0600
    // file to 0644. A missing destination keeps the default (plain create).
    let write_result = match fs::metadata(&destination) {
        Ok(metadata) => temp_file.set_permissions(metadata.permissions()),
        // Only a missing destination (plain create) keeps the umask default;
        // any other stat failure must not silently drop the original's mode.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
    .and_then(|()| temp_file.write_all(bytes))
    .and_then(|()| temp_file.flush())
    .and_then(|()| temp_file.sync_all());
    drop(temp_file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    // `std::fs::rename` replaces an existing destination file on Unix
    // (`rename(2)`) and Windows (`MoveFileExW` + `MOVEFILE_REPLACE_EXISTING`).
    match fs::rename(&temp_path, &destination) {
        Ok(()) => {
            #[cfg(unix)]
            {
                let parent = destination
                    .parent()
                    .filter(|parent| !parent.as_os_str().is_empty())
                    .unwrap_or_else(|| Path::new("."));
                // The rename above published the new inode; fsync the directory
                // so the rename survives a crash. Best-effort: some filesystems
                // reject directory fsync, and the file data is already durable.
                let _ = fs::File::open(parent).and_then(|dir| dir.sync_all());
            }
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(temp_path);
            Err(error)
        }
    }
}

/// Follow a leaf-symlink chain (bounded like Linux `MAXSYMLINKS`) to the file
/// replacement actually targets. Mirrors the leaf-following half of
/// `tools`' `resolve_for_boundary_check`; parent directories resolve through
/// the OS during the rename itself. A cycle (or an unreadable/absurdly long
/// chain) refuses with the `ELOOP`-style error plain `fs::write` produced,
/// instead of renaming the temp file over the link itself.
fn resolve_leaf_symlinks(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..40 {
        match current.symlink_metadata() {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = fs::read_link(&current)?;
                if target.is_absolute() {
                    current = target;
                } else {
                    let base = current.parent().unwrap_or_else(|| Path::new(""));
                    current = base.join(target);
                }
            }
            Ok(_) => return Ok(current),
            // A missing leaf is the plain-create case; any other lstat failure
            // (e.g. an unsearchable parent) must propagate — proceeding could
            // rename over a path we could not prove is not a symlink.
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(current),
            Err(error) => return Err(error),
        }
    }
    // The hop budget is spent. A chain of exactly 40 links that settled on a
    // non-symlink is within the budget (Linux errors on the 41st hop, not the
    // 40th); refuse only when the path is STILL a symlink — a cycle or an
    // over-budget chain. `ErrorKind::FilesystemLoop` is still unstable
    // (`io_error_more`), so the ELOOP meaning travels in the message.
    match current.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => Err(io::Error::other(format!(
            "too many levels of symbolic links resolving {}",
            path.display()
        ))),
        Ok(_) => Ok(current),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(current),
        Err(error) => Err(error),
    }
}

fn create_atomic_temp_file(path: &Path) -> io::Result<(PathBuf, fs::File)> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("output");
    for _ in 0..128 {
        let counter =
            ATOMIC_REPLACE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp_path = parent.join(format!(
            ".{name}.tmp-{}-{counter}",
            std::process::id()
        ));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("could not allocate a temporary file for {}", path.display()),
    ))
}

/// Replaces a file's contents and returns patch metadata.
pub fn write_file(path: &str, content: &str) -> io::Result<WriteFileOutput> {
    if content.len() > MAX_WRITE_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "content is too large ({} bytes, max {} bytes)",
                content.len(),
                MAX_WRITE_SIZE
            ),
        ));
    }

    let absolute_path = normalize_path_allow_missing(path)?;
    let original_file = fs::read_to_string(&absolute_path).ok();
    if let Some(parent) = absolute_path.parent() {
        fs::create_dir_all(parent)?;
    }
    replace_file_atomic(&absolute_path, content.as_bytes())?;

    Ok(WriteFileOutput {
        kind: if original_file.is_some() {
            String::from("update")
        } else {
            String::from("create")
        },
        file_path: absolute_path.to_string_lossy().into_owned(),
        content: content.to_owned(),
        structured_patch: make_patch(original_file.as_deref().unwrap_or(""), content),
        original_file,
        git_diff: None,
    })
}

/// Performs an in-file string replacement and returns patch metadata.
pub fn edit_file(
    path: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> io::Result<EditFileOutput> {
    let absolute_path = normalize_path(path)?;
    let original_file = fs::read_to_string(&absolute_path)?;
    let updated = apply_text_edit(&original_file, old_string, new_string, replace_all)?;
    replace_file_atomic(&absolute_path, updated.as_bytes())?;

    Ok(EditFileOutput {
        file_path: absolute_path.to_string_lossy().into_owned(),
        old_string: old_string.to_owned(),
        new_string: new_string.to_owned(),
        original_file: original_file.clone(),
        structured_patch: make_patch(&original_file, &updated),
        // 성공 경로의 불변식 — 필드 doc 및 tools 층 read-registry 가드 참조.
        user_modified: false,
        replace_all,
        git_diff: None,
    })
}

/// Applies ordered replacements in memory, then publishes the final buffer with
/// one atomic replacement. Any failing edit returns before the write and names
/// its zero-based array index, making partial multi-edits impossible.
pub fn multi_edit_file(
    path: &str,
    edits: &[(&str, &str, bool)],
) -> io::Result<WriteFileOutput> {
    if edits.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "edits must contain at least one edit",
        ));
    }

    let absolute_path = normalize_path(path)?;
    let original_file = fs::read_to_string(&absolute_path)?;
    let mut updated = original_file.clone();
    for (index, (old_string, new_string, replace_all)) in edits.iter().enumerate() {
        if old_string == new_string {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("edit[{index}] failed: old_string and new_string must differ"),
            ));
        }
        let new_string = replacement_with_line_ending(&updated, new_string);
        updated = apply_text_edit(&updated, old_string, &new_string, *replace_all).map_err(|error| {
            io::Error::new(error.kind(), format!("edit[{index}] failed: {error}"))
        })?;
    }
    if updated.len() > MAX_WRITE_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "content is too large ({} bytes, max {} bytes)",
                updated.len(),
                MAX_WRITE_SIZE
            ),
        ));
    }
    replace_file_atomic(&absolute_path, updated.as_bytes())?;

    Ok(WriteFileOutput {
        kind: String::from("update"),
        file_path: absolute_path.to_string_lossy().into_owned(),
        content: updated.clone(),
        structured_patch: make_patch(&original_file, &updated),
        original_file: Some(original_file),
        git_diff: None,
    })
}

fn apply_text_edit(
    original: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> io::Result<String> {
    if old_string == new_string {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "old_string and new_string must differ",
        ));
    }
    let occurrences = original.matches(old_string).count();
    if occurrences == 0 {
        return whitespace_tolerant_replace(original, old_string, new_string, replace_all)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "old_string not found in file"));
    }
    if !replace_all && occurrences > 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "old_string is not unique: found {occurrences} matches. Add more \
                 surrounding context to target one occurrence, or set \
                 replace_all=true to replace every match."
            ),
        ));
    }

    Ok(if replace_all {
        original.replace(old_string, new_string)
    } else {
        original.replacen(old_string, new_string, 1)
    })
}

/// Per-line trailing-whitespace + line-ending normalization, used only to
/// *locate* a near-miss `old_string` whose sole difference from the file is
/// incidental whitespace. Leading indentation is preserved (it is semantically
/// meaningful), so a genuine wrong-anchor edit still fails loudly.
fn normalize_for_match(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.replace("\r\n", "\n").split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(line.trim_end());
    }
    out
}

#[derive(Clone, Copy)]
struct LineSpan<'a> {
    start: usize,
    content_end: usize,
    content: &'a str,
}

fn line_spans(text: &str) -> Vec<LineSpan<'_>> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    for segment in text.split_inclusive('\n') {
        let end = start + segment.len();
        let content_end = if segment.ends_with("\r\n") {
            end - 2
        } else if segment.ends_with('\n') {
            end - 1
        } else {
            end
        };
        lines.push(LineSpan {
            start,
            content_end,
            content: &text[start..content_end],
        });
        start = end;
    }
    if start < text.len() || text.is_empty() || text.ends_with('\n') {
        lines.push(LineSpan {
            start,
            content_end: text.len(),
            content: &text[start..],
        });
    }
    lines
}

fn preferred_line_ending(text: &str) -> &'static str {
    text.find('\n').map_or("\n", |index| {
        if index > 0 && text.as_bytes()[index - 1] == b'\r' {
            "\r\n"
        } else {
            "\n"
        }
    })
}

fn replacement_with_line_ending(original: &str, replacement: &str) -> String {
    replacement
        .replace("\r\n", "\n")
        .replace('\n', preferred_line_ending(original))
}

/// Attempt a whitespace-tolerant replacement when the exact `old_string` was
/// not found. Untouched bytes are copied verbatim and replacement newlines use
/// the file's existing style, so tolerant matching never normalizes a CRLF file.
fn whitespace_tolerant_replace(
    original: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> io::Result<Option<String>> {
    let needle = normalize_for_match(old_string);
    if needle.is_empty() {
        return Ok(None);
    }
    let lines = line_spans(original);
    let needle_lines: Vec<&str> = needle.split('\n').collect();
    let mut match_line_starts = Vec::new();
    if needle_lines.len() <= lines.len() {
        for start in 0..=(lines.len() - needle_lines.len()) {
            let window_matches = needle_lines
                .iter()
                .enumerate()
                .all(|(offset, needle_line)| {
                    lines[start + offset].content.trim_end() == *needle_line
                });
            if window_matches {
                match_line_starts.push(start);
            }
        }
    }

    if match_line_starts.is_empty() {
        return Ok(None);
    }
    if !replace_all && match_line_starts.len() > 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "old_string is not unique after whitespace-normalization: found {} \
                 near matches. Add more surrounding context to target one \
                 occurrence, or set replace_all=true.",
                match_line_starts.len()
            ),
        ));
    }

    let replacement = replacement_with_line_ending(original, new_string);
    let mut updated = String::with_capacity(
        original.len().saturating_sub(needle.len()) + replacement.len(),
    );
    let mut cursor = 0usize;
    let mut next_available_line = 0usize;
    for start_line in match_line_starts {
        if start_line < next_available_line {
            continue;
        }
        let end_line = start_line + needle_lines.len() - 1;
        updated.push_str(&original[cursor..lines[start_line].start]);
        updated.push_str(&replacement);
        cursor = lines[end_line].content_end;
        next_available_line = end_line + 1;
        if !replace_all {
            break;
        }
    }
    updated.push_str(&original[cursor..]);
    Ok(Some(updated))
}

const MAX_GLOB_BRACE_EXPANSIONS: usize = 128;

fn expand_glob_braces(pattern: &str) -> io::Result<Vec<String>> {
    let mut pending = vec![pattern.to_owned()];
    let mut expanded = Vec::new();

    while let Some(candidate) = pending.pop() {
        let Some((open, close, alternatives)) = next_brace_group(&candidate) else {
            expanded.push(candidate);
            continue;
        };

        for alternative in alternatives.into_iter().rev() {
            if Path::new(alternative).components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            }) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "glob brace alternatives cannot escape the search root",
                ));
            }
            if pending.len().saturating_add(expanded.len()) >= MAX_GLOB_BRACE_EXPANSIONS {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "glob brace expansion exceeds the {MAX_GLOB_BRACE_EXPANSIONS}-pattern limit"
                    ),
                ));
            }
            pending.push(format!(
                "{}{}{}",
                &candidate[..open],
                alternative,
                &candidate[close + 1..]
            ));
        }
    }

    Ok(expanded)
}

fn next_brace_group(pattern: &str) -> Option<(usize, usize, Vec<&str>)> {
    let mut offset = 0usize;
    while let Some(relative_open) = pattern[offset..].find('{') {
        let open = offset + relative_open;
        let relative_close = pattern[open + 1..].find('}')?;
        let close = open + 1 + relative_close;
        let alternatives = pattern[open + 1..close].split(',').collect::<Vec<_>>();
        if alternatives.len() > 1 {
            return Some((open, close, alternatives));
        }
        offset = close + 1;
    }
    None
}

/// Expands a glob pattern and returns matching filenames.
pub fn glob_search(pattern: &str, path: Option<&str>) -> io::Result<GlobSearchOutput> {
    let started = Instant::now();
    let base_dir = match path {
        Some(path) if path.chars().any(|character| "*?[{".contains(character)) => {
            normalize_path_allow_missing(path)?
        }
        Some(path) => normalize_path(path)?,
        None => std::env::current_dir()?,
    };
    let search_pattern = if Path::new(pattern).is_absolute() {
        pattern.to_owned()
    } else {
        base_dir.join(pattern).to_string_lossy().into_owned()
    };

    let mut matches = Vec::new();
    for expanded_pattern in expand_glob_braces(&search_pattern)? {
        let glob_matcher = Pattern::new(&expanded_pattern)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
        let literal_root = glob_literal_root(&expanded_pattern);
        match fs::symlink_metadata(&literal_root) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        }

        // Walk from each expanded pattern's literal prefix with the standard
        // ignore filters applied. Missing brace alternatives behave like shell
        // globs and contribute no matches instead of failing the whole search.
        for entry in WalkBuilder::new(literal_root).standard_filters(true).build() {
            let entry = entry.map_err(|error| io::Error::other(error.to_string()))?;
            if entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
                && glob_matcher.matches_path(entry.path())
            {
                matches.push(entry.path().to_path_buf());
            }
        }
    }

    matches.sort();
    matches.dedup();
    matches.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .map(Reverse)
    });

    let truncated = matches.len() > 100;
    let filenames = matches
        .into_iter()
        .take(100)
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();

    Ok(GlobSearchOutput {
        duration_ms: started.elapsed().as_millis(),
        num_files: filenames.len(),
        filenames,
        truncated,
    })
}

/// Runs a regex search over workspace files with optional context lines.
pub fn grep_search(input: &GrepSearchInput) -> io::Result<GrepSearchOutput> {
    let base_path = input
        .path
        .as_deref()
        .map(normalize_path)
        .transpose()?
        .unwrap_or(std::env::current_dir()?);

    let regex = build_search_regex(input)?;

    let glob_filter = input
        .glob
        .as_deref()
        .map(Pattern::new)
        .transpose()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
    let file_type = input.file_type.as_deref();
    let output_mode = input
        .output_mode
        .clone()
        .unwrap_or_else(|| String::from("files_with_matches"));
    let context = input.context.or(input.context_short).unwrap_or(0);
    let stop_after = grep_output_stop_after(input.head_limit, input.offset);

    let mut filenames = Vec::new();
    // Content rows carry the index of their source file in `filenames`, so the
    // post-window filename list can be derived from the rows that actually
    // survive `apply_limit` — keeping `filenames`/`content` consistent under
    // offset paging (the downstream per-file grouping splits rows by these
    // exact path prefixes).
    let mut content_rows: Vec<(usize, String)> = Vec::new();
    let mut total_matches = 0usize;

    visit_search_files(&base_path, |file_path| {
        if !matches_optional_filters(file_path, &base_path, glob_filter.as_ref(), file_type) {
            return Ok(true);
        }

        let Ok(file_contents) = fs::read_to_string(file_path) else {
            return Ok(true);
        };

        if output_mode == "count" {
            let count = regex.find_iter(&file_contents).count();
            if count > 0 {
                filenames.push(file_path.to_string_lossy().into_owned());
                total_matches += count;
            }
            return Ok(!grep_window_reached(filenames.len(), stop_after));
        }

        let lines: Vec<&str> = file_contents.lines().collect();
        let mut matched_lines = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            if regex.is_match(line) {
                total_matches += 1;
                matched_lines.push(index);
            }
        }

        if matched_lines.is_empty() {
            return Ok(true);
        }

        filenames.push(file_path.to_string_lossy().into_owned());
        let file_index = filenames.len() - 1;
        if output_mode == "content" {
            let ranges = merged_context_ranges(
                &matched_lines,
                input.before.unwrap_or(context),
                input.after.unwrap_or(context),
                lines.len(),
            );
            for (start, end) in ranges {
                for (current, line) in lines.iter().enumerate().take(end).skip(start) {
                    let prefix = if input.line_numbers.unwrap_or(true) {
                        format!("{}:{}:", file_path.to_string_lossy(), current + 1)
                    } else {
                        format!("{}:", file_path.to_string_lossy())
                    };
                    content_rows.push((file_index, format!("{prefix}{line}")));
                    if grep_window_reached(content_rows.len(), stop_after) {
                        return Ok(false);
                    }
                }
            }
        }

        Ok(output_mode == "content" || !grep_window_reached(filenames.len(), stop_after))
    })?;

    if output_mode == "content" {
        return Ok(windowed_grep_content_output(
            output_mode,
            &filenames,
            content_rows,
            input.head_limit,
            input.offset,
        ));
    }

    let (filenames, applied_limit, applied_offset) =
        apply_limit(filenames, input.head_limit, input.offset);
    Ok(GrepSearchOutput {
        mode: Some(output_mode.clone()),
        num_files: filenames.len(),
        filenames,
        content: None,
        num_lines: None,
        num_matches: (output_mode == "count").then_some(total_matches),
        applied_limit,
        applied_offset,
    })
}

/// Merge overlapping/adjacent per-match context windows into disjoint line
/// ranges so a line shared by several nearby matches is emitted once, not
/// once per match. `matched_lines` is ascending (file-scan order), so the
/// merged ranges come out sorted.
fn merged_context_ranges(
    matched_lines: &[usize],
    before: usize,
    after: usize,
    total_lines: usize,
) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for index in matched_lines {
        let start = index.saturating_sub(before);
        let end = (index + after + 1).min(total_lines);
        match ranges.last_mut() {
            Some((_, last_end)) if start <= *last_end => {
                *last_end = (*last_end).max(end);
            }
            _ => ranges.push((start, end)),
        }
    }
    ranges
}

/// Assemble the content-mode grep output: the offset/limit window is applied
/// to content rows only, and the filename list is rebuilt from the surviving
/// rows so it names exactly the files visible in `content` — paging both
/// lists with the same offset desynced them (a line offset is not a file
/// offset), which broke the downstream per-file grouping.
fn windowed_grep_content_output(
    mode: String,
    filenames: &[String],
    content_rows: Vec<(usize, String)>,
    head_limit: Option<usize>,
    offset: Option<usize>,
) -> GrepSearchOutput {
    let (rows, applied_limit, applied_offset) = apply_limit(content_rows, head_limit, offset);
    let mut window_filenames: Vec<String> = Vec::new();
    for (file_index, _) in &rows {
        // Every row is pushed after its file's name, so the index is always
        // in range today — `.get()` keeps a future reordering from panicking.
        let Some(path) = filenames.get(*file_index) else {
            continue;
        };
        if window_filenames.last() != Some(path) {
            window_filenames.push(path.clone());
        }
    }
    let rendered = rows
        .iter()
        .map(|(_, row)| row.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    GrepSearchOutput {
        mode: Some(mode),
        num_files: window_filenames.len(),
        filenames: window_filenames,
        num_lines: Some(rows.len()),
        content: Some(rendered),
        num_matches: None,
        applied_limit,
        applied_offset,
    }
}

fn grep_output_stop_after(limit: Option<usize>, offset: Option<usize>) -> Option<usize> {
    let explicit_limit = limit.unwrap_or(250);
    if explicit_limit == 0 {
        None
    } else {
        Some(
            offset
                .unwrap_or(0)
                .saturating_add(explicit_limit)
                .saturating_add(1),
        )
    }
}

fn grep_window_reached(seen: usize, stop_after: Option<usize>) -> bool {
    stop_after.is_some_and(|stop_after| seen >= stop_after)
}

/// Whether `path` satisfies a caller-supplied `glob`.
///
/// The walk yields absolute paths, so a pattern anchored at the search root —
/// the shape callers actually write, and the one `rg --glob` accepts
/// (`crates/**/*.rs`) — could never match: it starts at `crates/` while the
/// candidate starts at `/`. Such a glob silently selected zero files, which
/// reads as "no matches in the repo" rather than "your filter matched nothing".
/// Testing the root-relative path first gives those patterns ripgrep's
/// behavior; the absolute forms stay as fallbacks so patterns that already
/// worked (`**/*.rs`, or a genuinely absolute prefix) keep matching.
fn glob_matches_candidate(pattern: &Pattern, path: &Path, base_path: &Path) -> bool {
    let relative = path.strip_prefix(base_path).unwrap_or(path);
    pattern.matches_path(relative)
        || pattern.matches(&relative.to_string_lossy())
        || pattern.matches_path(path)
        || pattern.matches(&path.to_string_lossy())
}

fn matches_optional_filters(
    path: &Path,
    base_path: &Path,
    glob_filter: Option<&Pattern>,
    file_type: Option<&str>,
) -> bool {
    if let Some(pattern) = glob_filter {
        if !glob_matches_candidate(pattern, path, base_path) {
            return false;
        }
    }

    if let Some(ext) = file_type {
        if path.extension().and_then(|value| value.to_str()) != Some(ext) {
            return false;
        }
    }

    true
}

fn visit_search_files(
    base_path: &Path,
    mut visitor: impl FnMut(&Path) -> io::Result<bool>,
) -> io::Result<()> {
    if base_path.is_file() {
        let _ = visitor(base_path)?;
        return Ok(());
    }

    for entry in WalkBuilder::new(base_path).standard_filters(true).build() {
        let entry = entry.map_err(|error| io::Error::other(error.to_string()))?;
        if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
            && !visitor(entry.path())?
        {
            break;
        }
    }
    Ok(())
}

fn build_search_regex(input: &GrepSearchInput) -> io::Result<Regex> {
    let mut builder = RegexBuilder::new(&input.pattern);
    builder
        .case_insensitive(input.case_insensitive.unwrap_or(false))
        .dot_matches_new_line(input.multiline.unwrap_or(false));
    match builder.build() {
        Ok(regex) => Ok(regex),
        Err(error) => {
            let mut literal_builder = RegexBuilder::new(&regex::escape(&input.pattern));
            literal_builder
                .case_insensitive(input.case_insensitive.unwrap_or(false))
                .dot_matches_new_line(input.multiline.unwrap_or(false));
            literal_builder.build().map_err(|literal_error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{error}; literal fallback failed: {literal_error}"),
                )
            })
        }
    }
}

/// Deepest non-wildcard directory prefix of a glob pattern, used as the walk
/// root so the ignore filters apply before the pattern is matched. Falls back
/// to `.` when the pattern begins with a wildcard.
fn glob_literal_root(pattern: &str) -> PathBuf {
    let mut root = PathBuf::new();
    for component in Path::new(pattern).components() {
        if component
            .as_os_str()
            .to_string_lossy()
            .contains(['*', '?', '['])
        {
            break;
        }
        root.push(component);
    }
    if root.as_os_str().is_empty() {
        root.push(".");
    }
    root
}

fn apply_limit<T>(
    items: Vec<T>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> (Vec<T>, Option<usize>, Option<usize>) {
    let offset_value = offset.unwrap_or(0);
    let mut items = items.into_iter().skip(offset_value).collect::<Vec<_>>();
    let explicit_limit = limit.unwrap_or(250);
    if explicit_limit == 0 {
        return (items, None, (offset_value > 0).then_some(offset_value));
    }

    let truncated = items.len() > explicit_limit;
    items.truncate(explicit_limit);
    (
        items,
        truncated.then_some(explicit_limit),
        (offset_value > 0).then_some(offset_value),
    )
}

fn make_patch(original: &str, updated: &str) -> Vec<StructuredPatchHunk> {
    compact_line_diff(original, updated)
        .into_iter()
        .map(|hunk| StructuredPatchHunk {
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
            lines: hunk
                .lines
                .into_iter()
                .map(|line| {
                    let prefix = match line.kind {
                        CompactDiffLineKind::Context => ' ',
                        CompactDiffLineKind::Removed => '-',
                        CompactDiffLineKind::Added => '+',
                    };
                    format!("{prefix}{}", line.text)
                })
                .collect(),
        })
        .collect()
}

fn normalize_path(path: &str) -> io::Result<PathBuf> {
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        std::env::current_dir()?.join(path)
    };
    candidate.canonicalize()
}

fn normalize_path_allow_missing(path: &str) -> io::Result<PathBuf> {
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        std::env::current_dir()?.join(path)
    };

    if let Ok(canonical) = candidate.canonicalize() {
        return Ok(canonical);
    }

    if let Some(parent) = candidate.parent() {
        let canonical_parent = parent
            .canonicalize()
            .unwrap_or_else(|_| parent.to_path_buf());
        if let Some(name) = candidate.file_name() {
            return Ok(canonical_parent.join(name));
        }
    }

    Ok(candidate)
}

/// Read a file with workspace boundary enforcement.
pub fn read_file_in_workspace(
    path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
    workspace_root: &Path,
) -> io::Result<ReadFileOutput> {
    let absolute_path = normalize_path(path)?;
    let canonical_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    validate_workspace_boundary(&absolute_path, &canonical_root)?;
    read_file(path, offset, limit)
}

/// Check whether a path is a symlink that resolves outside the workspace.
pub fn is_symlink_escape(path: &Path, workspace_root: &Path) -> io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_symlink() {
        return Ok(false);
    }
    let resolved = path.canonicalize()?;
    let canonical_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    Ok(!resolved.starts_with(&canonical_root))
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        edit_file, glob_search, grep_search, is_symlink_escape, read_file, read_file_in_workspace,
        write_file, GrepSearchInput, DEFAULT_READ_LINE_LIMIT, MAX_WRITE_SIZE,
    };

    fn temp_path(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("zo-native-{name}-{unique}"))
    }

    #[test]
    fn reads_and_writes_files() {
        let path = temp_path("read-write.txt");
        let write_output = write_file(path.to_string_lossy().as_ref(), "one\ntwo\nthree")
            .expect("write should succeed");
        assert_eq!(write_output.kind, "create");

        let read_output = read_file(path.to_string_lossy().as_ref(), Some(1), Some(1))
            .expect("read should succeed");
        assert_eq!(read_output.file.content, "two");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_file_failure_preserves_existing_destination() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_path("atomic-write-failure");
        std::fs::create_dir_all(&dir).expect("create temporary directory");
        let destination = dir.join("source.rs");
        std::fs::write(&destination, b"original source").expect("seed source file");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555))
            .expect("make source directory read-only");

        let probe = dir.join("probe");
        if std::fs::write(&probe, b"probe").is_ok() {
            let _ = std::fs::remove_file(probe);
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
                .expect("restore source directory permissions");
            let _ = std::fs::remove_dir_all(dir);
            return;
        }

        let result = write_file(destination.to_string_lossy().as_ref(), "replacement source");

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("restore source directory permissions");
        let persisted = std::fs::read(&destination).expect("read source after failed replacement");
        let _ = std::fs::remove_dir_all(dir);
        assert!(result.is_err(), "creating the sibling temp file must fail");
        assert_eq!(
            persisted, b"original source",
            "failed replacement must leave the user's source intact"
        );
    }

    #[cfg(unix)]
    #[test]
    fn leaf_resolver_propagates_unreadable_parent_errors() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_path("resolver-unreadable");
        std::fs::create_dir_all(&dir).expect("create temporary directory");
        let target = dir.join("target.rs");
        std::fs::write(&target, b"source").expect("seed target");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000))
            .expect("lock parent directory");

        // Root ignores permission bits: probe and skip instead of failing.
        if target.symlink_metadata().is_ok() {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
                .expect("restore parent permissions");
            let _ = std::fs::remove_dir_all(dir);
            return;
        }

        let result = super::resolve_leaf_symlinks(&target);

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("restore parent permissions");
        let _ = std::fs::remove_dir_all(dir);
        assert!(
            result.is_err(),
            "an unreadable parent must propagate its error, not fail open as a regular file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_file_preserves_destination_mode() {
        use std::os::unix::fs::PermissionsExt;

        let destination = temp_path("atomic-write-mode.rs");
        std::fs::write(&destination, b"original source").expect("seed source file");
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o600))
            .expect("restrict source mode");

        write_file(destination.to_string_lossy().as_ref(), "replacement source")
            .expect("replace source atomically");

        let mode = std::fs::metadata(&destination)
            .expect("stat replaced source")
            .permissions()
            .mode()
            & 0o777;
        let _ = std::fs::remove_file(destination);
        assert_eq!(mode, 0o600, "replacement must preserve the source mode");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_file_follows_leaf_symlink() {
        let dir = temp_path("atomic-write-symlink");
        std::fs::create_dir_all(&dir).expect("create temporary directory");
        let target = dir.join("real-source.rs");
        std::fs::write(&target, b"original source").expect("seed symlink target");
        let link = dir.join("source-link.rs");
        std::os::unix::fs::symlink(&target, &link).expect("create leaf symlink");

        write_file(link.to_string_lossy().as_ref(), "replacement source")
            .expect("replace through symlink");

        assert!(
            link.symlink_metadata()
                .expect("lstat source link")
                .file_type()
                .is_symlink(),
            "the source symlink must survive replacement"
        );
        assert_eq!(
            std::fs::read(&target).expect("read symlink target"),
            b"replacement source",
            "the write must land on the symlink's target"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_file_refuses_symlink_cycle() {
        let dir = temp_path("atomic-write-cycle");
        std::fs::create_dir_all(&dir).expect("create temporary directory");
        let a = dir.join("a-link.rs");
        let b = dir.join("b-link.rs");
        std::os::unix::fs::symlink(&b, &a).expect("create a->b");
        std::os::unix::fs::symlink(&a, &b).expect("create b->a");

        let error = write_file(a.to_string_lossy().as_ref(), "replacement source")
            .expect_err("a symlink cycle must refuse replacement");

        let normalized = a
            .parent()
            .expect("cyclic link has a parent")
            .canonicalize()
            .expect("canonicalize link parent")
            .join(a.file_name().expect("cyclic link has a name"));
        assert_eq!(
            error.to_string(),
            format!(
                "too many levels of symbolic links resolving {}",
                normalized.display()
            )
        );
        assert!(
            a.symlink_metadata()
                .expect("lstat cyclic source link")
                .file_type()
                .is_symlink(),
            "the cyclic link must be left intact, not renamed over"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_file_follows_exactly_forty_symlink_hops() {
        let dir = temp_path("atomic-write-forty");
        std::fs::create_dir_all(&dir).expect("create temporary directory");
        let real = dir.join("real-source.rs");
        std::fs::write(&real, b"original source").expect("seed chain target");
        let mut previous = real.clone();
        for i in 1..=40 {
            let link = dir.join(format!("link-{i}.rs"));
            std::os::unix::fs::symlink(&previous, &link).expect("create chain link");
            previous = link;
        }
        let head = previous;

        write_file(head.to_string_lossy().as_ref(), "replacement source")
            .expect("a 40-hop chain is within the budget");

        assert_eq!(
            std::fs::read(&real).expect("read chain target"),
            b"replacement source",
            "the write must land on the chain's final target"
        );
        assert!(
            head.symlink_metadata()
                .expect("lstat chain head")
                .file_type()
                .is_symlink(),
            "the chain head must remain a symlink"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_edit_file_follows_leaf_symlink() {
        let dir = temp_path("atomic-edit-symlink");
        std::fs::create_dir_all(&dir).expect("create temporary directory");
        let target = dir.join("real-source.rs");
        std::fs::write(&target, b"alpha beta").expect("seed symlink target");
        let link = dir.join("source-link.rs");
        std::os::unix::fs::symlink(&target, &link).expect("create leaf symlink");

        edit_file(link.to_string_lossy().as_ref(), "alpha", "omega", false)
            .expect("edit through symlink");

        assert!(
            link.symlink_metadata()
                .expect("lstat source link")
                .file_type()
                .is_symlink(),
            "the source symlink must survive the edit"
        );
        assert_eq!(
            std::fs::read(&target).expect("read symlink target"),
            b"omega beta",
            "the edit must patch the symlink's target"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn write_numbered_lines(name: &str, count: usize) -> std::path::PathBuf {
        let path = temp_path(name);
        let mut body = String::new();
        for n in 0..count {
            use std::fmt::Write as _;
            let _ = writeln!(body, "line {n}");
        }
        std::fs::write(&path, body).expect("seed numbered file");
        path
    }

    #[test]
    fn unbounded_read_caps_at_default_line_limit_with_notice() {
        // 2001 lines, unbounded read -> only 2000 returned, with an actionable
        // note; total_lines still reports the real length.
        let path = write_numbered_lines("cap-2001.txt", 2001);
        let output =
            read_file(path.to_string_lossy().as_ref(), None, None).expect("read should succeed");

        assert_eq!(output.file.num_lines, DEFAULT_READ_LINE_LIMIT);
        assert_eq!(output.file.start_line, 1);
        assert_eq!(output.file.total_lines, 2001);
        assert_eq!(output.file.content.lines().count(), DEFAULT_READ_LINE_LIMIT);
        // The last returned line is line 1999 (0-based line 1999 == the 2000th),
        // NOT the final line 2000.
        assert!(output.file.content.contains("line 1999"));
        assert!(!output.file.content.contains("line 2000"));
        let notice = output.file.notice.expect("capped read must carry a notice");
        assert_eq!(
            notice,
            "showing lines 1-2000 of 2001 — pass offset/limit to read more"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn file_under_default_cap_reads_whole_without_notice() {
        // 1999 lines fits under the cap: whole file, no notice, no output change.
        let path = write_numbered_lines("under-cap-1999.txt", 1999);
        let output =
            read_file(path.to_string_lossy().as_ref(), None, None).expect("read should succeed");

        assert_eq!(output.file.num_lines, 1999);
        assert_eq!(output.file.total_lines, 1999);
        assert!(output.file.notice.is_none(), "under-cap read must not annotate");
        assert!(output.file.content.contains("line 1998"));
        // The JSON envelope must omit the notice key entirely (byte-for-byte
        // unchanged for the common case).
        let json = serde_json::to_string(&output).expect("serialize");
        assert!(!json.contains("notice"), "notice key must be omitted when absent");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn explicit_limit_is_never_capped_and_has_no_notice() {
        // An explicit limit larger than the default cap is honored verbatim and
        // never annotated, even though the read stops before EOF is irrelevant —
        // here it reaches EOF at 2500 while exceeding the 2000 default cap.
        let path = write_numbered_lines("explicit-2500.txt", 2500);
        let output = read_file(path.to_string_lossy().as_ref(), None, Some(2500))
            .expect("read should succeed");

        assert_eq!(output.file.num_lines, 2500);
        assert!(output.file.notice.is_none(), "explicit limit must not annotate");
        assert!(output.file.content.contains("line 2499"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn explicit_offset_and_limit_window_is_unchanged() {
        // Regression guard: an explicit offset+limit window behaves exactly as
        // before (no cap, no notice), including start_line reporting.
        let path = write_numbered_lines("window-3000.txt", 3000);
        let output = read_file(path.to_string_lossy().as_ref(), Some(10), Some(5))
            .expect("read should succeed");

        assert_eq!(output.file.start_line, 11);
        assert_eq!(output.file.num_lines, 5);
        assert_eq!(output.file.total_lines, 3000);
        assert!(output.file.notice.is_none());
        assert_eq!(output.file.content, "line 10\nline 11\nline 12\nline 13\nline 14");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unbounded_read_with_offset_caps_relative_to_offset() {
        // offset with no limit: CC parity defaults limit to 2000 from the offset.
        let path = write_numbered_lines("offset-nolimit-3000.txt", 3000);
        let output = read_file(path.to_string_lossy().as_ref(), Some(100), None)
            .expect("read should succeed");

        assert_eq!(output.file.start_line, 101);
        assert_eq!(output.file.num_lines, DEFAULT_READ_LINE_LIMIT);
        let notice = output.file.notice.expect("capped read must carry a notice");
        assert_eq!(
            notice,
            "showing lines 101-2100 of 3000 — pass offset/limit to read more"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reads_notebook_as_cell_text() {
        let path = temp_path("read-notebook").with_extension("ipynb");
        let notebook = serde_json::json!({
            "cells": [
                {
                    "cell_type": "markdown",
                    "source": ["# Title\n", "body\n"]
                },
                {
                    "cell_type": "code",
                    "execution_count": 7,
                    "source": "print('hi')\n",
                    "outputs": [{
                        "output_type": "stream",
                        "name": "stdout",
                        "text": ["hi\n"]
                    }]
                }
            ]
        });
        std::fs::write(&path, serde_json::to_string(&notebook).expect("json")).expect("write");

        let output = read_file(path.to_string_lossy().as_ref(), None, None).expect("read notebook");
        assert_eq!(output.kind, "notebook");
        assert!(output.file.content.contains("[cell 1] (markdown)"));
        assert!(output.file.content.contains("# Title"));
        assert!(output.file.content.contains("[cell 2] (code)"));
        assert!(output.file.content.contains("execution_count: 7"));
        assert!(output.file.content.contains("print('hi')"));
        assert!(output.file.content.contains("stream: hi"));
    }

    #[test]
    fn txt_notebook_json_reads_as_plain_text() {
        let path = temp_path("notebook-json.txt");
        std::fs::write(&path, r#"{"cells":[]}"#).expect("write");

        let output = read_file(path.to_string_lossy().as_ref(), None, None).expect("read text");
        assert_eq!(output.kind, "text");
        assert_eq!(output.file.content, r#"{"cells":[]}"#);
    }

    #[test]
    fn malformed_notebook_json_is_error() {
        let path = temp_path("bad").with_extension("ipynb");
        std::fs::write(&path, "not json").expect("write");

        let error = read_file(path.to_string_lossy().as_ref(), None, None)
            .expect_err("malformed notebook must fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("failed to parse notebook JSON"));
    }

    #[test]
    fn edit_tolerates_trailing_whitespace_mismatch() {
        // File has a trailing space the model's old_string omits.
        let path = temp_path("edit-trailing-ws.txt");
        write_file(
            path.to_string_lossy().as_ref(),
            "fn main() { \n    body\n}\n",
        )
        .expect("seed");
        let out = edit_file(
            path.to_string_lossy().as_ref(),
            "fn main() {", // no trailing space — exact match would fail
            "fn main() { // edited",
            false,
        )
        .expect("whitespace-tolerant edit should succeed");
        let after = std::fs::read_to_string(&path).expect("read back");
        assert!(
            after.contains("// edited"),
            "edit must apply, got:\n{after}"
        );
        assert!(!out.file_path.is_empty());
    }

    #[test]
    fn edit_tolerates_crlf_vs_lf() {
        let path = temp_path("edit-crlf.txt");
        // File on disk uses CRLF; model supplies LF.
        write_file(
            path.to_string_lossy().as_ref(),
            "alpha\r\nbeta\r\ngamma\r\n",
        )
        .expect("seed");
        edit_file(
            path.to_string_lossy().as_ref(),
            "alpha\nbeta",
            "ALPHA\nBETA",
            false,
        )
        .expect("CRLF-tolerant edit should succeed");
        let after = std::fs::read(&path).expect("read back");
        assert_eq!(
            after,
            b"ALPHA\r\nBETA\r\ngamma\r\n",
            "tolerant edit must preserve CRLF line endings"
        );
    }

    #[test]
    fn edit_refuses_ambiguous_whitespace_match_without_replace_all() {
        let path = temp_path("edit-ambiguous.txt");
        write_file(
            path.to_string_lossy().as_ref(),
            "log() \nmid\nlog() \nend\n",
        )
        .expect("seed");
        // "log()" (no trailing space) normalizes to two near-matches.
        let err = edit_file(path.to_string_lossy().as_ref(), "log()", "trace()", false)
            .expect_err("ambiguous tolerant match must be refused");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        // File must be untouched on refusal.
        let after = std::fs::read_to_string(&path).expect("read back");
        assert!(!after.contains("trace()"), "ambiguous edit must not apply");
    }

    #[test]
    fn edit_still_fails_on_genuinely_absent_string() {
        let path = temp_path("edit-absent.txt");
        write_file(path.to_string_lossy().as_ref(), "hello\nworld\n").expect("seed");
        let err = edit_file(
            path.to_string_lossy().as_ref(),
            "nonexistent anchor",
            "x",
            false,
        )
        .expect_err("absent string must still fail");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    /// Build a minimal-but-valid single-page PDF with `text` drawn in
    /// Helvetica. Offsets in the xref table are computed, not hand-typed, so
    /// the fixture stays valid however the body strings change.
    fn minimal_pdf(text: &str) -> Vec<u8> {
        use std::fmt::Write as _;
        let stream = format!("BT /F1 24 Tf 72 720 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
            format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        ];
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (index, body) in objects.iter().enumerate() {
            offsets.push(out.len());
            let _ = write!(out, "{} 0 obj\n{body}\nendobj\n", index + 1);
        }
        let xref_at = out.len();
        let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
        for offset in &offsets {
            let _ = writeln!(out, "{offset:010} 00000 n ");
        }
        let _ = write!(
            out,
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        );
        out.into_bytes()
    }

    /// `--add-dir` 루트는 워크스페이스 경계의 일부가 된다 — 단일 리스트를
    /// 모든 경계 검사가 공유하므로, 추가 후 허용·해제 후 거부가 즉시 일관된다.
    #[test]
    fn add_dir_roots_extend_workspace_boundary() {
        use super::{set_additional_workspace_roots, validate_workspace_boundary};
        let workspace = temp_path("boundary-ws");
        let extra = temp_path("boundary-extra");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::create_dir_all(&extra).expect("extra root");
        let outside_file = extra.join("notes.txt");

        assert!(
            validate_workspace_boundary(&outside_file, &workspace).is_err(),
            "outside path must be rejected before --add-dir"
        );
        set_additional_workspace_roots(vec![extra.clone()]);
        assert!(
            validate_workspace_boundary(&outside_file, &workspace).is_ok(),
            "added root must be accepted"
        );
        // 전역 복원 — 다른 경계 테스트에 영향 금지.
        set_additional_workspace_roots(Vec::new());
        assert!(validate_workspace_boundary(&outside_file, &workspace).is_err());
    }

    /// CC 패리티: 단일 Read 가 PDF 를 처리한다 — 텍스트 추출 + `[page N]` 마커
    /// + kind="pdf".
    #[test]
    fn reads_pdf_text_with_page_markers() {
        let path = temp_path("doc.pdf");
        std::fs::write(&path, minimal_pdf("Hello zo PDF")).expect("write pdf");

        let output =
            read_file(path.to_string_lossy().as_ref(), None, None).expect("pdf read should work");
        assert_eq!(output.kind, "pdf");
        assert!(
            output.file.content.contains("[page 1]"),
            "page marker missing: {}",
            output.file.content
        );
        assert!(
            output.file.content.contains("Hello zo PDF"),
            "extracted text missing: {}",
            output.file.content
        );
    }

    /// 확장자가 없어도 `%PDF-` 매직으로 PDF 를 인식한다 (binary 거부 경로를
    /// 타지 않는다).
    #[test]
    fn detects_pdf_by_magic_without_extension() {
        let path = temp_path("renamed-pdf.bin");
        std::fs::write(&path, minimal_pdf("magic sniffed")).expect("write pdf");

        let output =
            read_file(path.to_string_lossy().as_ref(), None, None).expect("pdf read should work");
        assert_eq!(output.kind, "pdf");
        assert!(output.file.content.contains("magic sniffed"));
    }

    /// offset/limit 라인 윈도잉이 PDF 추출 텍스트에도 동일하게 적용된다.
    #[test]
    fn pdf_read_applies_line_window() {
        let path = temp_path("windowed.pdf");
        std::fs::write(&path, minimal_pdf("windowed body")).expect("write pdf");

        let output = read_file(path.to_string_lossy().as_ref(), Some(0), Some(1))
            .expect("pdf read should work");
        assert_eq!(output.file.content, "[page 1]");
        assert_eq!(output.file.start_line, 1);
        assert_eq!(output.file.num_lines, 1);
    }

    #[test]
    fn edits_file_contents() {
        let path = temp_path("edit.txt");
        write_file(path.to_string_lossy().as_ref(), "alpha beta alpha")
            .expect("initial write should succeed");
        let output = edit_file(path.to_string_lossy().as_ref(), "alpha", "omega", true)
            .expect("edit should succeed");
        assert!(output.replace_all);
    }

    #[test]
    fn edit_patch_shows_changed_window_not_whole_file() {
        let path = temp_path("edit-window.txt");
        let original = (1..=12)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        write_file(path.to_string_lossy().as_ref(), &original).expect("initial write");

        let output = edit_file(path.to_string_lossy().as_ref(), "line 8", "changed", false)
            .expect("edit should succeed");
        assert_eq!(output.structured_patch.len(), 1);
        let hunk = &output.structured_patch[0];
        assert_eq!(hunk.old_start, 5);
        assert!(
            hunk.lines.iter().any(|line| line == "-line 8"),
            "removed changed line must be present: {hunk:?}"
        );
        assert!(
            hunk.lines.iter().any(|line| line == "+changed"),
            "added changed line must be present: {hunk:?}"
        );
        assert!(
            !hunk.lines.iter().any(|line| line == " line 1"),
            "distant unchanged prefix must not be displayed: {hunk:?}"
        );
        assert!(
            !hunk.lines.iter().any(|line| line == " line 12"),
            "distant unchanged suffix must not be displayed: {hunk:?}"
        );
    }

    #[test]
    fn edit_patch_splits_distant_replace_all_changes_into_hunks() {
        let path = temp_path("edit-multi-window.txt");
        let original = (1..=30)
            .map(|n| {
                if n == 5 || n == 20 {
                    format!("needle {n}")
                } else {
                    format!("line {n}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        write_file(path.to_string_lossy().as_ref(), &original).expect("initial write");

        let output = edit_file(path.to_string_lossy().as_ref(), "needle", "changed", true)
            .expect("edit should succeed");
        assert_eq!(output.structured_patch.len(), 2);
        assert!(output.structured_patch[0]
            .lines
            .iter()
            .any(|line| line == "-needle 5"));
        assert!(output.structured_patch[0]
            .lines
            .iter()
            .any(|line| line == "+changed 5"));
        assert!(output.structured_patch[1]
            .lines
            .iter()
            .any(|line| line == "-needle 20"));
        assert!(output.structured_patch[1]
            .lines
            .iter()
            .any(|line| line == "+changed 20"));
        assert!(
            !output
                .structured_patch
                .iter()
                .flat_map(|hunk| hunk.lines.iter())
                .any(|line| line == " line 12"),
            "unchanged lines between distant edits must stay hidden: {output:?}"
        );
    }

    #[test]
    fn edit_rejects_non_unique_anchor() {
        let path = temp_path("edit-ambiguous.txt");
        write_file(path.to_string_lossy().as_ref(), "alpha beta alpha")
            .expect("initial write should succeed");
        // Two `alpha` matches with replace_all=false must be refused rather
        // than silently editing only the first occurrence.
        let err = edit_file(path.to_string_lossy().as_ref(), "alpha", "omega", false)
            .expect_err("ambiguous edit should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("not unique"));
        // File must be left untouched on rejection.
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            "alpha beta alpha"
        );
        // replace_all=true still succeeds on the same ambiguous input.
        let output = edit_file(path.to_string_lossy().as_ref(), "alpha", "omega", true)
            .expect("replace_all should succeed");
        assert!(output.replace_all);
    }

    #[test]
    fn rejects_binary_files() {
        let path = temp_path("binary-test.bin");
        std::fs::write(&path, b"\x00\x01\x02\x03binary content").expect("write should succeed");
        let result = read_file(path.to_string_lossy().as_ref(), None, None);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("binary"));
    }

    #[test]
    fn rejects_oversized_writes() {
        let path = temp_path("oversize-write.txt");
        let huge = "x".repeat(MAX_WRITE_SIZE + 1);
        let result = write_file(path.to_string_lossy().as_ref(), &huge);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("too large"));
    }

    #[test]
    fn enforces_workspace_boundary() {
        let workspace = temp_path("workspace-boundary");
        std::fs::create_dir_all(&workspace).expect("workspace dir should be created");
        let inside = workspace.join("inside.txt");
        write_file(inside.to_string_lossy().as_ref(), "safe content")
            .expect("write inside workspace should succeed");

        // Reading inside workspace should succeed
        let result =
            read_file_in_workspace(inside.to_string_lossy().as_ref(), None, None, &workspace);
        assert!(result.is_ok());

        // Reading outside workspace should fail
        let outside = temp_path("outside-boundary.txt");
        write_file(outside.to_string_lossy().as_ref(), "unsafe content")
            .expect("write outside should succeed");
        let result =
            read_file_in_workspace(outside.to_string_lossy().as_ref(), None, None, &workspace);
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("escapes workspace"));
    }

    #[test]
    fn detects_symlink_escape() {
        let workspace = temp_path("symlink-workspace");
        std::fs::create_dir_all(&workspace).expect("workspace dir should be created");
        let outside = temp_path("symlink-target.txt");
        std::fs::write(&outside, "target content").expect("target should write");

        let link_path = workspace.join("escape-link.txt");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, &link_path).expect("symlink should create");
            assert!(is_symlink_escape(&link_path, &workspace).expect("check should succeed"));
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(&outside, &link_path)
                .expect("symlink should create (Windows requires Developer Mode or privilege)");
            assert!(is_symlink_escape(&link_path, &workspace).expect("check should succeed"));
        }

        // Non-symlink file should not be an escape
        let normal = workspace.join("normal.txt");
        std::fs::write(&normal, "normal content").expect("normal file should write");
        assert!(!is_symlink_escape(&normal, &workspace).expect("check should succeed"));
    }

    #[test]
    fn globs_and_greps_directory() {
        let dir = temp_path("search-dir");
        std::fs::create_dir_all(&dir).expect("directory should be created");
        let file = dir.join("demo.rs");
        write_file(
            file.to_string_lossy().as_ref(),
            "fn main() {\n println!(\"hello\");\n}\n",
        )
        .expect("file write should succeed");

        let globbed = glob_search("**/*.rs", Some(dir.to_string_lossy().as_ref()))
            .expect("glob should succeed");
        assert_eq!(globbed.num_files, 1);

        let grep_output = grep_search(&GrepSearchInput {
            pattern: String::from("hello"),
            path: Some(dir.to_string_lossy().into_owned()),
            glob: Some(String::from("**/*.rs")),
            output_mode: Some(String::from("content")),
            before: None,
            after: None,
            context_short: None,
            context: None,
            line_numbers: Some(true),
            case_insensitive: Some(false),
            file_type: None,
            head_limit: Some(10),
            offset: Some(0),
            multiline: Some(false),
        })
        .expect("grep should succeed");
        assert!(grep_output.content.unwrap_or_default().contains("hello"));
    }

    /// A glob anchored at the search root (`crates/**/*.rs`) is what callers
    /// write and what `rg --glob` accepts. Matching it against the walk's
    /// absolute paths made it select nothing, so the search reported "no
    /// matches" for code that was plainly there.
    #[test]
    fn grep_glob_anchored_at_search_root_matches_like_ripgrep() {
        let dir = temp_path("grep-rooted-glob");
        let nested = dir.join("crates").join("tools").join("src");
        std::fs::create_dir_all(&nested).expect("directory should be created");
        write_file(
            nested.join("bash_tools.rs").to_string_lossy().as_ref(),
            "pub run_in_background: Option<bool>,\n",
        )
        .expect("file write should succeed");

        let grep = |glob: &str| {
            grep_search(&GrepSearchInput {
                pattern: String::from("run_in_background"),
                path: Some(dir.to_string_lossy().into_owned()),
                glob: Some(String::from(glob)),
                output_mode: Some(String::from("files_with_matches")),
                before: None,
                after: None,
                context_short: None,
                context: None,
                line_numbers: Some(false),
                case_insensitive: Some(false),
                file_type: None,
                head_limit: None,
                offset: Some(0),
                multiline: Some(false),
            })
            .expect("grep should succeed")
            .num_files
        };

        assert_eq!(grep("crates/**/*.rs"), 1, "root-anchored glob must match");
        assert_eq!(grep("crates/tools/**/*.rs"), 1, "deeper anchor must match");
        assert_eq!(grep("**/*.rs"), 1, "unanchored glob must keep working");
        assert_eq!(
            grep("crates/**/*.toml"),
            0,
            "a glob that genuinely matches nothing must still filter everything out"
        );
    }

    #[test]
    fn glob_search_expands_braced_paths_and_filenames() {
        let dir = temp_path("glob-braces");
        let fixtures = [
            ("core", "balancesheetDrilldown2.sql"),
            ("database", "AccountStatementDAO.cs"),
            ("mobile", "SubLedger.aspx.cs"),
            ("kftc", "Payments.csproj"),
        ];
        for (subdir, filename) in fixtures {
            let subdir = dir.join(subdir);
            std::fs::create_dir_all(&subdir).expect("brace fixture directory");
            std::fs::write(subdir.join(filename), "fixture\n").expect("brace fixture file");
        }

        let output = glob_search(
            "{core,database,mobile,kftc,missing}/**/{balancesheetDrilldown2.sql,AccountStatementDAO.cs,SubLedger.aspx.cs,*.csproj}",
            Some(dir.to_string_lossy().as_ref()),
        )
        .expect("braced glob should succeed");
        assert_eq!(output.num_files, 4);
        for expected in [
            "balancesheetDrilldown2.sql",
            "AccountStatementDAO.cs",
            "SubLedger.aspx.cs",
            "Payments.csproj",
        ] {
            assert!(
                output.filenames.iter().any(|path| path.ends_with(expected)),
                "missing {expected}: {:?}",
                output.filenames
            );
        }

        let braced_path = dir.join("{core,database,mobile,kftc,missing}");
        let path_output = glob_search(
            "**/{balancesheetDrilldown2.sql,AccountStatementDAO.cs,SubLedger.aspx.cs,*.csproj}",
            Some(braced_path.to_string_lossy().as_ref()),
        )
        .expect("braces in the path argument should succeed");
        assert_eq!(path_output.num_files, 4);
        for expected in [
            "balancesheetDrilldown2.sql",
            "AccountStatementDAO.cs",
            "SubLedger.aspx.cs",
            "Payments.csproj",
        ] {
            assert!(
                path_output
                    .filenames
                    .iter()
                    .any(|path| path.ends_with(expected)),
                "path-brace search missing {expected}: {:?}",
                path_output.filenames
            );
        }

        let error = glob_search(
            "{core,../../outside}/**/*",
            Some(dir.to_string_lossy().as_ref()),
        )
        .expect_err("brace alternatives cannot escape the search root");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn search_skips_gitignored_and_hidden_paths() {
        let dir = temp_path("ignore-search");
        std::fs::create_dir_all(dir.join("build")).expect("build dir");
        std::fs::create_dir_all(dir.join(".hidden")).expect("hidden dir");
        // A `.git` dir makes this a repo so `ignore` honors `.gitignore`
        // (its default `require_git` semantics) — mirroring real zo runs.
        std::fs::create_dir_all(dir.join(".git")).expect("git dir");
        std::fs::write(dir.join(".gitignore"), "build/\n").expect("gitignore");
        std::fs::write(dir.join("keep.rs"), "needle here\n").expect("keep file");
        std::fs::write(dir.join("build").join("skip.rs"), "needle here\n").expect("ignored file");
        std::fs::write(dir.join(".hidden").join("h.rs"), "needle here\n").expect("hidden file");

        let globbed = glob_search("**/*.rs", Some(dir.to_string_lossy().as_ref()))
            .expect("glob should succeed");
        assert_eq!(
            globbed.num_files, 1,
            "glob must skip gitignored (build/) and hidden (.hidden/) paths"
        );
        assert!(globbed.filenames[0].ends_with("keep.rs"));

        let grep_output = grep_search(&GrepSearchInput {
            pattern: String::from("needle"),
            path: Some(dir.to_string_lossy().into_owned()),
            glob: None,
            output_mode: Some(String::from("files_with_matches")),
            before: None,
            after: None,
            context_short: None,
            context: None,
            line_numbers: Some(true),
            case_insensitive: Some(false),
            file_type: None,
            head_limit: Some(10),
            offset: Some(0),
            multiline: Some(false),
        })
        .expect("grep should succeed");
        assert_eq!(
            grep_output.num_files, 1,
            "grep must skip gitignored (build/) and hidden (.hidden/) paths"
        );
        assert!(grep_output.filenames[0].ends_with("keep.rs"));
    }

    #[test]
    #[cfg(unix)]
    fn grep_search_head_limit_stops_before_later_walk_errors_and_reports_limit() {
        use std::os::unix::fs::PermissionsExt;

        let dir = temp_path("grep-head-limit-early-stop");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let open_dir = dir.join("a_open");
        std::fs::create_dir_all(&open_dir).expect("mkdir open");
        std::fs::write(open_dir.join("a_match.txt"), "needle a
needle b
")
            .expect("write first match file");
        let denied = dir.join("z_denied");
        std::fs::create_dir_all(&denied).expect("mkdir denied");
        std::fs::write(denied.join("late.txt"), "needle late
").expect("write late");
        let mut permissions = std::fs::metadata(&denied).expect("metadata").permissions();
        permissions.set_mode(0o000);
        std::fs::set_permissions(&denied, permissions).expect("chmod denied");

        let grep_output = grep_search(&GrepSearchInput {
            pattern: "needle".to_string(),
            path: Some(dir.to_string_lossy().into_owned()),
            glob: Some("**/*.txt".to_string()),
            output_mode: Some("content".to_string()),
            before: None,
            after: None,
            context_short: None,
            context: None,
            line_numbers: None,
            case_insensitive: None,
            file_type: None,
            head_limit: Some(1),
            offset: None,
            multiline: None,
        })
        .expect("grep should stop before walking the unreadable tail");

        let mut permissions = std::fs::metadata(&denied).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&denied, permissions).ok();
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(grep_output.num_lines, Some(1));
        assert_eq!(grep_output.applied_limit, Some(1));
        let content = grep_output.content.unwrap_or_default();
        assert!(content.contains("a_match.txt"));
        assert!(!content.contains("late.txt"));
    }

    #[test]
    fn grep_search_merges_overlapping_context_windows() {
        let dir = temp_path("grep-context-merge");
        std::fs::create_dir_all(&dir).expect("directory should be created");
        let file = dir.join("demo.txt");
        // Matches on lines 3 and 5 (1-based); context 2 makes their windows
        // overlap on lines 3-5, which the old per-match emission duplicated.
        std::fs::write(
            &file,
            "l1\nl2\nneedle a\nl4\nneedle b\nl6\nl7\nl8\nl9\nl10\n",
        )
        .expect("file write should succeed");

        let grep_output = grep_search(&GrepSearchInput {
            pattern: String::from("needle"),
            path: Some(dir.to_string_lossy().into_owned()),
            glob: None,
            output_mode: Some(String::from("content")),
            before: None,
            after: None,
            context_short: None,
            context: Some(2),
            line_numbers: Some(true),
            case_insensitive: None,
            file_type: None,
            head_limit: None,
            offset: None,
            multiline: None,
        })
        .expect("grep should succeed");

        // Windows [1,6) and [3,8) merge to lines 1-7: seven unique rows.
        assert_eq!(grep_output.num_lines, Some(7));
        let content = grep_output.content.unwrap_or_default();
        let rows: Vec<&str> = content.lines().collect();
        let mut deduped = rows.clone();
        deduped.dedup();
        assert_eq!(rows, deduped, "no row may be emitted twice: {rows:?}");
        assert!(rows.iter().any(|row| row.contains(":3:needle a")));
        assert!(rows.iter().any(|row| row.contains(":5:needle b")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn grep_search_offset_window_keeps_filenames_consistent_with_content() {
        let dir = temp_path("grep-offset-window");
        std::fs::create_dir_all(&dir).expect("directory should be created");
        std::fs::write(dir.join("a.txt"), "needle 1\nneedle 2\nneedle 3\n")
            .expect("first file write");
        std::fs::write(dir.join("b.txt"), "needle 4\nneedle 5\nneedle 6\n")
            .expect("second file write");

        // Six content rows total; the offset window selects the second
        // visited file's three rows. `filenames` must name exactly the files
        // whose rows survive the window — not the file list paged by the
        // same (line-based) offset, which left it empty here.
        let grep_output = grep_search(&GrepSearchInput {
            pattern: String::from("needle"),
            path: Some(dir.to_string_lossy().into_owned()),
            glob: None,
            output_mode: Some(String::from("content")),
            before: None,
            after: None,
            context_short: None,
            context: None,
            line_numbers: Some(true),
            case_insensitive: None,
            file_type: None,
            head_limit: Some(3),
            offset: Some(3),
            multiline: None,
        })
        .expect("grep should succeed");

        let content = grep_output.content.clone().unwrap_or_default();
        let rows: Vec<&str> = content.lines().collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(grep_output.num_files, 1);
        assert_eq!(grep_output.filenames.len(), 1);
        let path = &grep_output.filenames[0];
        assert!(
            rows.iter().all(|row| row.starts_with(path.as_str())),
            "every visible row must belong to the reported filename: {path} vs {rows:?}"
        );
        assert_eq!(grep_output.applied_offset, Some(3));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn grep_search_invalid_regex_falls_back_to_literal_search() {
        let dir = temp_path("literal-grep-search");
        std::fs::create_dir_all(&dir).expect("directory should be created");
        let file = dir.join("scan.go");
        write_file(
            file.to_string_lossy().as_ref(),
            "func (runner localScanRunner\ncase \"}\n",
        )
        .expect("file write should succeed");

        let grep_output = grep_search(&GrepSearchInput {
            pattern: String::from("func (runner localScanRunner"),
            path: Some(dir.to_string_lossy().into_owned()),
            glob: Some(String::from("**/*.go")),
            output_mode: Some(String::from("content")),
            before: None,
            after: None,
            context_short: None,
            context: None,
            line_numbers: Some(true),
            case_insensitive: Some(false),
            file_type: None,
            head_limit: Some(10),
            offset: Some(0),
            multiline: Some(false),
        })
        .expect("invalid regex should be retried as a literal search");

        assert!(grep_output
            .content
            .unwrap_or_default()
            .contains("func (runner localScanRunner"));
    }

    /// 잠금을 쥔 채 죽은 주인의 잠금은 되찾는다: 살아 있는 주인의 것은 폴 예산만큼 기다린 뒤 비켜서고, pid를 아직 적지 않은
    /// 빈 잠금은 살아 있는 주인의 것으로 둔다; 죽은 뒤에는 바로 잡힌다(t-6263).
    #[cfg(unix)]
    #[test]
    fn a_settings_lock_is_taken_back_only_from_an_owner_that_died_holding_it() {
        let dir = temp_path("settings-lock-owner");
        std::fs::create_dir_all(&dir).expect("dir");
        let settings = dir.join("settings.json");
        let lock = settings.with_extension("json.lock");
        let mut owner = std::process::Command::new("sleep").arg("60").spawn().expect("an owner process");
        std::fs::write(&lock, format!("{}\n", owner.id())).expect("its lock");
        let waited = std::time::Instant::now();
        let refused = super::SettingsFileLock::acquire(&settings).expect_err("a live owner keeps its lock");
        assert_eq!(refused.kind(), io::ErrorKind::WouldBlock);
        assert!(waited.elapsed() >= std::time::Duration::from_millis(1_500), "after the whole poll");
        assert!(lock.exists(), "the live owner's lock stands");

        owner.kill().expect("kill the owner");
        owner.wait().expect("the owner is gone");
        let started = std::time::Instant::now();
        let taken = super::SettingsFileLock::acquire(&settings).expect("a dead owner's lock is taken back");
        assert!(started.elapsed() < std::time::Duration::from_millis(500), "at once, not after the poll");
        assert_eq!(
            std::fs::read_to_string(&lock).expect("the new lock").trim(),
            std::process::id().to_string(),
            "and it is this process's now"
        );
        drop(taken);
        assert!(!lock.exists(), "released as ever");

        std::fs::write(&lock, "").expect("an owner still writing its pid");
        assert!(
            super::SettingsFileLock::acquire(&settings).is_err(),
            "an owner that has not written its pid yet is alive"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
