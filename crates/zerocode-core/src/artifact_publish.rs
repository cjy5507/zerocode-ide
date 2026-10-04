//! Shared HTML publication format for the window's store and headless zo.
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::artifact::{Artifact, ArtifactKind, Limits, Origin, Preview, Source};

/// Publication bounds; gallery listing/retention bounds remain in `artifact::Limits`.
pub const ARTIFACT_MAX_BYTES: u64 = 16 * 1024 * 1024;
pub const ARTIFACT_TITLE_MAX: usize = 120;
pub const ARTIFACT_DESCRIPTION_MAX: usize = 500;
pub const ARTIFACT_LABEL_MAX: usize = 80;
pub const ARTIFACT_FAVICON_MAX: usize = 2;
pub const ARTIFACT_TITLE_SCAN_BYTES: usize = 8 * 1024;
pub const ARTIFACT_SHIM_TIMEOUT_SECS: u64 = 30;
pub const ARTIFACT_INDEX_MAX_BYTES: u64 = 16 * 1024 * 1024;
pub const PAGES_DIR: &str = "pages";
pub const INDEX_FILE: &str = "index.jsonl";
pub const SHIM: &str = "zerocode-artifact";
pub const ROUTE: &str = "/artifact";
/// How long a publish waits for the store's lock before it answers busy,
/// and how often it tries. A lock just released can linger for the moment
/// another thread's child holds a copy of its descriptor between fork and
/// exec (a spawn that searches a changed PATH forks); that moment is
/// milliseconds, a publish never is.
pub const STORE_LOCK_PATIENCE_MS: u64 = 250;
pub const STORE_LOCK_RETRY_MS: u64 = 5;
pub const USAGE: &str = "zerocode-artifact publish --file-path <absolute.html> [--title <title>] [--description <text>] [--favicon <emoji>] [--label <label>]\nzerocode-artifact list\nzerocode-artifact read --id <id>\nzerocode-artifact export <id> [--version N] --out <path.html|path.pdf|path.png>";

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishInput {
    pub file_path: PathBuf,
    pub title: Option<String>,
    pub description: Option<String>,
    pub favicon: Option<String>,
    pub label: Option<String>,
}

/// Where a publish was asked from, as the door says it: the pane key its
/// shell carried (`ZEROCODE_PANE_KEY`) and the folder it stood in. Carried
/// beside [`PublishInput`], never in it, so the input stays strict; the
/// window resolves it into the row's `Origin` and it authorizes nothing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Caller {
    pub pane: Option<String>,
    pub cwd: Option<PathBuf>,
}

/// A publish request split into its strict input and the caller beside it
/// ([`request_from_argv`] puts `pane` and `cwd` next to the input's fields).
pub fn publish_parts(mut request: serde_json::Value) -> Result<(PublishInput, Caller), String> {
    let fields = request.as_object_mut().ok_or("expected object")?;
    fields.remove("action");
    let text = |value: Option<serde_json::Value>| {
        value.and_then(|value| value.as_str().map(str::to_string))
    };
    let caller = Caller {
        pane: text(fields.remove("pane")),
        cwd: text(fields.remove("cwd")).map(PathBuf::from),
    };
    let input = serde_json::from_value(request).map_err(|e| e.to_string())?;
    Ok((input, caller))
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExportInput {
    pub id: String,
    pub version: Option<u32>,
    pub out: PathBuf,
}

/// The kinds of file an export writes, named by the extension of `--out`
/// (t-18558). `.htm` is another spelling of HTML, not a fourth kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Html,
    Pdf,
    Png,
}

/// The extensions an export can be named with, as a refusal lists them.
pub const EXPORT_EXTENSIONS: &str = ".html, .htm, .pdf or .png";

impl ExportFormat {
    /// Every kind, in the order a menu offers them.
    pub const ALL: [Self; 3] = [Self::Html, Self::Pdf, Self::Png];

    /// The word a kind goes by on the wire and in a file name.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Html => "html",
            Self::Pdf => "pdf",
            Self::Png => "png",
        }
    }

    /// The kind a wire word names — exactly `html`, `pdf` or `png`.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.word() == word)
    }

    /// The kind an extension names, in any case; `htm` is HTML.
    #[must_use]
    pub fn from_extension(extension: &str) -> Option<Self> {
        match extension.to_ascii_lowercase().as_str() {
            "html" | "htm" => Some(Self::Html),
            "pdf" => Some(Self::Pdf),
            "png" => Some(Self::Png),
            _ => None,
        }
    }

    /// The kind `--out` asks for, or the refusal that lists what an export
    /// writes: another extension, none, and an empty one are all refused.
    pub fn of_path(path: &Path) -> Result<Self, String> {
        let extension = path.extension().and_then(|extension| extension.to_str());
        extension.and_then(Self::from_extension).ok_or_else(|| {
            let got = match extension {
                Some(extension) if !extension.is_empty() => format!(".{extension}"),
                _ => "no extension".to_string(),
            };
            format!("artifact export needs --out ending in {EXPORT_EXTENSIONS} (got {got})")
        })
    }

    /// Whether the kind is drawn by a page renderer rather than copied.
    #[must_use]
    pub const fn is_drawn(self) -> bool {
        !matches!(self, Self::Html)
    }
}

/// What a page renderer hands back for one page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    pub bytes: Vec<u8>,
    /// The pages of a PDF, when the renderer could count them.
    pub pages: Option<u32>,
    /// The pixel size of a picture.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// A picture cut at the renderer's height cap: the page was taller.
    pub truncated: bool,
}

/// The seam between an export and the window's WebKit. The core writes the
/// files and names their kinds; a renderer draws one page. It keeps the core
/// free of any WebKit, lets the headless catalog say plainly that it draws
/// nothing, and lets a test stand a fake in.
pub trait PageRenderer: Send + Sync {
    /// Whether this renderer can draw `format` at all.
    fn can_render(&self, format: ExportFormat) -> bool;

    /// Draw the page at `page` — the absolute path of one immutable version's
    /// HTML — as `format`.
    ///
    /// # Errors
    /// Why the page could not be drawn.
    fn render(&self, page: &Path, format: ExportFormat) -> Result<Rendered, String>;
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Serialize)]
pub struct ExportedFile {
    pub id: String,
    pub version: u32,
    pub path: PathBuf,
    pub bytes: u64,
    pub format: ExportFormat,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pages: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
}

/// One export made ready to write: the bytes of the file, before a name is
/// chosen for it. The window's export into a folder draws the page once and then
/// looks for a free name; the door names its file at once.
#[derive(Debug)]
pub struct PreparedExport {
    id: String,
    version: u32,
    format: ExportFormat,
    rendered: Rendered,
}

pub fn validate_export(input: &ExportInput) -> Result<(), String> {
    validate_id(&input.id)?;
    if input.version == Some(0) {
        return Err("artifact version must be positive".into());
    }
    if !input.out.is_absolute() {
        return Err("artifact export needs an absolute output path".into());
    }
    ExportFormat::of_path(&input.out)?;
    Ok(())
}

fn no_renderer(format: ExportFormat) -> String {
    format!(
        "artifact export to .{} needs the window's WebKit page renderer, and this build has none",
        format.word()
    )
}

/// Make ready the bytes of one immutable publication snapshot in `format`: the
/// version's own HTML, or the page drawn by `renderer`. A drawn format with no
/// renderer that can draw it is refused, and nothing is guessed.
pub fn prepare_export(
    root: &Path,
    id: &str,
    version: Option<u32>,
    format: ExportFormat,
    renderer: Option<&dyn PageRenderer>,
) -> Result<PreparedExport, String> {
    validate_id(id)?;
    if version == Some(0) {
        return Err("artifact version must be positive".into());
    }
    let renderer = if format.is_drawn() {
        Some(
            renderer
                .filter(|one| one.can_render(format))
                .ok_or_else(|| no_renderer(format))?,
        )
    } else {
        None
    };
    let meta = read_meta(root, id)?;
    let version = version.unwrap_or(meta.version);
    let kept: Vec<u32> = versions(root, id).into_iter().map(|kept| kept.n).collect();
    if !kept.contains(&version) {
        let kept = kept
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "artifact {id} version {version} is not kept (kept: {kept})"
        ));
    }
    let rendered = match renderer {
        None => {
            let mut artifact = meta.artifact(Origin::default());
            artifact.version = Some(version);
            Rendered {
                bytes: read_page(root, &artifact)?.into_bytes(),
                ..Rendered::default()
            }
        }
        Some(renderer) => {
            let rendered = renderer.render(&version_file(root, id, version), format)?;
            if rendered.bytes.is_empty() {
                return Err(format!("the renderer drew an empty .{}", format.word()));
            }
            rendered
        }
    };
    Ok(PreparedExport {
        id: id.to_string(),
        version,
        format,
        rendered,
    })
}

/// Write a prepared export to `out`, a file that must not exist yet. Never
/// truncate an existing destination (including store files or a symlink into
/// the store); a write that fails leaves no half file behind.
pub fn write_export(out: &Path, prepared: &PreparedExport) -> Result<ExportedFile, String> {
    let bytes = &prepared.rendered.bytes;
    let mut file = std::fs::File::create_new(out).map_err(|e| e.to_string())?;
    if let Err(error) = file.write_all(bytes) {
        drop(file);
        let _ = std::fs::remove_file(out);
        return Err(error.to_string());
    }
    Ok(ExportedFile {
        id: prepared.id.clone(),
        version: prepared.version,
        path: out.to_path_buf(),
        bytes: bytes.len() as u64,
        format: prepared.format,
        pages: prepared.rendered.pages,
        width: prepared.rendered.width,
        height: prepared.rendered.height,
        truncated: prepared.rendered.truncated,
    })
}

/// Export exactly one immutable publication snapshot, as the kind `--out`'s
/// extension names: `.html` and `.htm` copy the version's bytes, `.pdf` and
/// `.png` are drawn by `renderer`. Any other extension is refused with the
/// list of kinds.
pub fn export_with(
    root: &Path,
    input: &ExportInput,
    renderer: Option<&dyn PageRenderer>,
) -> Result<ExportedFile, String> {
    validate_export(input)?;
    let format = ExportFormat::of_path(&input.out)?;
    let prepared = prepare_export(root, &input.id, input.version, format, renderer)?;
    write_export(&input.out, &prepared)
}

/// [`export_with`] with no renderer: the copy, and a plain refusal for a kind
/// that has to be drawn — the headless catalog's export.
pub fn export(root: &Path, input: &ExportInput) -> Result<ExportedFile, String> {
    export_with(root, input, None)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PageMeta {
    pub id: String,
    pub version: u32,
    pub url: String,
    pub title: String,
    pub description: Option<String>,
    pub favicon: Option<String>,
    pub label: Option<String>,
    pub source_path: PathBuf,
    pub sha256: String,
    pub at: i64,
    pub bytes: u64,
    pub created_ms: i64,
    pub path: PathBuf,
}

impl PageMeta {
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn artifact(&self, origin: Origin) -> Artifact {
        Artifact {
            id: self.id.clone(),
            kind: ArtifactKind::Page,
            subtype: None,
            title: self.title.clone(),
            path: self.path.clone(),
            bytes: self.bytes,
            created_ms: self.created_ms,
            modified_ms: self.at,
            url: Some(self.url.clone()),
            favicon: self.favicon.clone(),
            description: self.description.clone(),
            version: Some(self.version),
            source_path: Some(self.source_path.clone()),
            feedback_count: None,
            feedback_version: None,
            origin,
            tags: self.label.iter().cloned().collect(),
            source: Source::Manual,
            preview: Preview::Text {
                text: self.description.clone().unwrap_or_default(),
            },
        }
    }
}

pub mod skeleton {
    use super::ARTIFACT_TITLE_SCAN_BYTES;

    /// Text as it may sit in HTML — between tags, or inside a quoted
    /// attribute (`title` reads the same entities back).
    #[must_use]
    pub fn escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    /// A bounded title lookup; byte slicing never splits a UTF-8 character.
    #[must_use]
    pub fn title(body: &str) -> Option<String> {
        let front = &body[..body.floor_char_boundary(body.len().min(ARTIFACT_TITLE_SCAN_BYTES))];
        let lower = front.to_ascii_lowercase();
        let start = lower.find("<title>")? + "<title>".len();
        let end = start + lower[start..].find("</title>")?;
        let title = front[start..end].trim();
        (!title.is_empty()).then(|| {
            title
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&#39;", "'")
                .replace("&amp;", "&")
        })
    }

    /// Preserve complete documents; give a fragment its document head and body.
    #[must_use]
    pub fn wrap(body: &str, title: &str) -> String {
        let front = body
            .trim_start_matches('\u{feff}')
            .trim_start()
            .to_ascii_lowercase();
        if front.starts_with("<!doctype html") || front.starts_with("<html") {
            return body.to_string();
        }
        format!(
            "<!doctype html>\n<html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{}</title><style>:root {{ color-scheme: light dark; }} *, *::before, *::after {{ box-sizing: border-box; }} body {{ margin: 0; }} img, svg, canvas {{ max-width: 100%; }}</style></head><body>\n{body}\n</body></html>\n",
            escape(title)
        )
    }
}

fn bounded(value: &Option<String>, name: &str, cap: usize) -> Result<(), String> {
    if value.as_ref().is_some_and(|s| s.chars().count() > cap) {
        return Err(format!("{name} exceeds {cap} characters"));
    }
    Ok(())
}

pub fn validate(input: &PublishInput) -> Result<(), String> {
    if input.file_path.as_os_str().is_empty() {
        return Err("publish needs file_path".into());
    }
    bounded(&input.title, "title", ARTIFACT_TITLE_MAX)?;
    bounded(&input.description, "description", ARTIFACT_DESCRIPTION_MAX)?;
    bounded(&input.label, "label", ARTIFACT_LABEL_MAX)?;
    if let Some(favicon) = &input.favicon {
        let count =
            unicode_segmentation::UnicodeSegmentation::graphemes(favicon.as_str(), true).count();
        if count == 0 || count > ARTIFACT_FAVICON_MAX || favicon.chars().any(char::is_control) {
            return Err(format!("favicon needs 1–{ARTIFACT_FAVICON_MAX} emoji"));
        }
    }
    Ok(())
}

pub fn read_bounded(path: &Path, cap: u64) -> Result<String, String> {
    let before = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if !before.is_file() || before.len() > cap {
        return Err(format!("file must be regular and at most {cap} bytes"));
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > cap {
        return Err(format!("file must be regular and at most {cap} bytes"));
    }
    let mut text = String::new();
    file.take(cap + 1)
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    if text.len() as u64 > cap {
        return Err(format!("file exceeds {cap} bytes"));
    }
    Ok(text)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        std::fs::write(&temp, bytes)?;
        std::fs::rename(&temp, path)
    })()
    .map_err(|e| e.to_string());
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// OS lock is released on process exit, including an interrupted publication.
pub fn lock_store(root: &Path) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join(".publish.lock"))
        .map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock)
                if started.elapsed() < std::time::Duration::from_millis(STORE_LOCK_PATIENCE_MS) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(STORE_LOCK_RETRY_MS));
            }
            Err(e) => return Err(format!("artifact store busy: {e}; retry publish")),
        }
    }
}

/// Write a new immutable version and advance the stable URL. Caller holds the store lock.
pub fn publish(root: &Path, input: &PublishInput, limits: &Limits) -> Result<PageMeta, String> {
    validate(input)?;
    let source = input.file_path.canonicalize().map_err(|e| e.to_string())?;
    let body = read_bounded(&source, ARTIFACT_MAX_BYTES)?;
    let title = skeleton::title(&body)
        .or_else(|| input.title.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            source
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let title = crate::artifact::truncate_chars(&title, ARTIFACT_TITLE_MAX);
    let html = skeleton::wrap(&body, &title);
    let id = format!(
        "p-{}",
        crate::artifact::artifact_id(&Origin::default(), &source)
    );
    let seat = root.join(PAGES_DIR).join(&id);
    std::fs::create_dir_all(&seat).map_err(|e| e.to_string())?;
    let previous = read_meta(root, &id).ok();
    // A fully written version left by interruption still owns its number.
    let numbers = version_numbers(&seat);
    let version = numbers
        .last()
        .copied()
        .unwrap_or_default()
        .max(previous.as_ref().map_or(0, |m| m.version))
        .checked_add(1)
        .ok_or("artifact version overflow")?;
    let path = seat.join("index.html");
    let url = url::Url::from_file_path(&path)
        .map_err(|()| "artifact store needs an absolute path")?
        .to_string();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let at = i64::try_from(at).map_err(|e| e.to_string())?;
    let meta = PageMeta {
        id,
        version,
        url,
        title,
        description: input.description.clone(),
        favicon: input.favicon.clone(),
        label: input.label.clone(),
        source_path: source,
        sha256: format!("{:x}", Sha256::digest(html.as_bytes())),
        at,
        bytes: html.len() as u64,
        created_ms: previous.as_ref().map_or(at, |m| m.created_ms),
        path,
    };
    let encoded = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
    let version_dir = seat.join(format!("v{version}"));
    std::fs::create_dir(&version_dir).map_err(|e| e.to_string())?;
    let written = (|| {
        atomic_write(&version_dir.join("index.html"), html.as_bytes())?;
        atomic_write(&version_dir.join("meta.json"), &encoded)?;
        atomic_write(&meta.path, html.as_bytes())?;
        atomic_write(&seat.join("meta.json"), &encoded)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_dir_all(&version_dir);
        return Err(error);
    }
    for old in numbers
        .iter()
        .rev()
        .skip(limits.versions_per_artifact_max.saturating_sub(1))
    {
        let _ = std::fs::remove_dir_all(seat.join(format!("v{old}")));
    }
    Ok(meta)
}

/// One kept publication version: its number, its immutable file, and the
/// digest its metadata recorded when it was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeptVersion {
    pub n: u32,
    pub path: PathBuf,
    pub sha256: Option<String>,
}

/// Every kept version of one publication, oldest first — the files an
/// export or a version picker may name. Empty for an id that is not a
/// publication. The stable `index.html` beside them is the newest copy,
/// never a version of its own.
#[must_use]
pub fn versions(root: &Path, id: &str) -> Vec<KeptVersion> {
    if validate_id(id).is_err() {
        return Vec::new();
    }
    let seat = root.join(PAGES_DIR).join(id);
    version_numbers(&seat)
        .into_iter()
        .filter_map(|n| {
            let folder = seat.join(format!("v{n}"));
            let path = folder.join("index.html");
            path.is_file().then(|| KeptVersion {
                n,
                sha256: read_bounded(&folder.join("meta.json"), ARTIFACT_TITLE_SCAN_BYTES as u64)
                    .ok()
                    .and_then(|text| serde_json::from_str::<PageMeta>(&text).ok())
                    .filter(|meta| meta.version == n)
                    .map(|meta| meta.sha256),
                path,
            })
        })
        .collect()
}

fn version_numbers(seat: &Path) -> Vec<u32> {
    let mut numbers: Vec<_> = std::fs::read_dir(seat)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.strip_prefix('v')?.parse().ok())
        .collect();
    numbers.sort_unstable();
    numbers
}

/// Read the version whose metadata was observed, even if publication advances
/// the stable browser URL between the catalog lookup and this file read.
pub fn read_page(root: &Path, artifact: &Artifact) -> Result<String, String> {
    if artifact.kind != ArtifactKind::Page {
        return Err("Artifact read needs a page id".into());
    }
    let path = artifact.version.map_or_else(
        || artifact.path.clone(),
        |version| version_file(root, &artifact.id, version),
    );
    read_bounded(&path, ARTIFACT_MAX_BYTES + ARTIFACT_TITLE_SCAN_BYTES as u64)
}

/// The immutable file of one kept version: where a copy reads it from and a
/// renderer loads it.
#[must_use]
pub fn version_file(root: &Path, id: &str, version: u32) -> PathBuf {
    root.join(PAGES_DIR)
        .join(id)
        .join(format!("v{version}"))
        .join("index.html")
}

fn validate_id(id: &str) -> Result<(), String> {
    if !id.starts_with("p-") || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return Err("invalid artifact id".into());
    }
    Ok(())
}

pub fn read_meta(root: &Path, id: &str) -> Result<PageMeta, String> {
    validate_id(id)?;
    let text = read_bounded(
        &root.join(PAGES_DIR).join(id).join("meta.json"),
        ARTIFACT_TITLE_SCAN_BYTES as u64,
    )?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

/// Read the existing gallery JSONL shape; newest row/tombstone wins.
pub fn catalog(root: &Path) -> Result<BTreeMap<String, Artifact>, String> {
    let path = root.join(INDEX_FILE);
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let text = read_bounded(&path, ARTIFACT_INDEX_MAX_BYTES)?;
    let mut rows = BTreeMap::new();
    for line in text.lines() {
        if let Ok(row) = serde_json::from_str::<Artifact>(line) {
            rows.insert(row.id.clone(), row);
        } else if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(id) = value.get("tombstone").and_then(serde_json::Value::as_str)
        {
            rows.remove(id);
        }
    }
    Ok(rows)
}

pub fn publish_headless(root: &Path, input: &PublishInput) -> Result<PageMeta, String> {
    let _lock = lock_store(root)?;
    let limits = Limits::default();
    let mut rows = catalog(root)?;
    let meta = publish(root, input, &limits)?;
    rows.insert(meta.id.clone(), meta.artifact(Origin::default()));
    while rows.len() > limits.index_rows_max {
        let Some(oldest) = rows
            .values()
            .min_by_key(|r| r.modified_ms)
            .map(|r| r.id.clone())
        else {
            break;
        };
        rows.remove(&oldest);
        let _ = std::fs::remove_dir_all(root.join(PAGES_DIR).join(oldest));
    }
    let mut encoded = Vec::new();
    for row in rows.values() {
        serde_json::to_writer(&mut encoded, row).map_err(|e| e.to_string())?;
        encoded.write_all(b"\n").map_err(|e| e.to_string())?;
    }
    atomic_write(&root.join(INDEX_FILE), &encoded)?;
    Ok(meta)
}

/// The CLI and model tool meet at the same validated JSON request.
///
/// The door appends `--cwd` to every verb and `--pane` when its shell has a
/// pane key, after the caller's own words, so the door's values are the ones
/// that stand. Export joins a relative `--out` onto the folder; publish
/// carries both as `cwd` and `pane` beside its input ([`publish_parts`]);
/// list and read have no use for either.
pub fn request_from_argv(argv: &[String]) -> Result<serde_json::Value, String> {
    let mut value = serde_json::json!({ "action": argv.first().ok_or(USAGE)? });
    let mut words = argv.iter().skip(1);
    let mut cwd = None;
    let mut pane = None;
    while let Some(flag) = words.next() {
        if flag == "--json" {
            continue;
        }
        if value["action"] == "export" && !flag.starts_with('-') && value.get("id").is_none() {
            value["id"] = serde_json::json!(flag);
            continue;
        }
        if flag == "--cwd" {
            cwd = Some(PathBuf::from(words.next().ok_or("--cwd needs a value")?));
            continue;
        }
        if flag == "--pane" {
            pane = Some(words.next().ok_or("--pane needs a value")?.clone());
            continue;
        }
        let key = match flag.as_str() {
            "--file-path" => "file_path",
            "--title" => "title",
            "--description" => "description",
            "--favicon" => "favicon",
            "--label" => "label",
            "--id" => "id",
            "--out" => "out",
            "--version" => "version",
            _ => return Err(format!("unknown option {flag}")),
        };
        let word = words
            .next()
            .ok_or_else(|| format!("{flag} needs a value"))?;
        value[key] = if key == "version" {
            serde_json::json!(
                word.parse::<u32>()
                    .map_err(|_| "artifact version must be a positive integer")?
            )
        } else {
            serde_json::json!(word)
        };
    }
    if value["action"] == "publish" {
        if let Some(pane) = pane {
            value["pane"] = serde_json::json!(pane);
        }
        if let Some(cwd) = &cwd {
            value["cwd"] = serde_json::json!(cwd);
        }
    }
    if value["action"] == "export" {
        if let (Some(cwd), Some(out)) = (cwd, value["out"].as_str()) {
            value["out"] = serde_json::json!(cwd.join(out));
        }
        let mut fields = value.clone();
        fields
            .as_object_mut()
            .ok_or("expected object")?
            .remove("action");
        let input: ExportInput = serde_json::from_value(fields).map_err(|e| e.to_string())?;
        validate_export(&input)?;
    }
    Ok(value)
}

/// Same private-token bridge renderer as the desktop and browser doors.
#[must_use]
pub fn shim_script(port_var: &str, hook_token_var: &str, powershell: bool) -> String {
    let shim = crate::computer_use::PowerShellBridgeShim {
        command: SHIM,
        manual: USAGE,
        prefix: "",
        route: ROUTE,
        port_var,
        capability_var: hook_token_var,
        capability_header: "x-zerocode-artifact-token",
        capability_missing: "this shell is not inside a ZeroCode window",
        hook_token_var,
        deadline_seconds: ARTIFACT_SHIM_TIMEOUT_SECS,
        stdin_flags: false,
        pane_header: None,
        cwd_verbs: &[],
        cwd_flag: Some("--cwd"),
        pane_flag: Some("--pane"),
    };
    if powershell {
        shim.render()
    } else {
        shim.render_posix()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lock another descriptor holds for a moment — a child between fork
    /// and exec keeping a copy of a publish's just-closed one — is waited
    /// out; a lock held past the patience is still answered busy.
    #[test]
    fn a_store_lock_held_for_a_moment_is_waited_out_and_one_held_on_is_busy() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("artifacts");
        let held = lock_store(&root).unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(40));
            drop(held);
        });
        let started = std::time::Instant::now();
        let second = lock_store(&root).expect("the moment is waited out");
        assert!(started.elapsed() >= std::time::Duration::from_millis(30));
        release.join().unwrap();
        let busy = lock_store(&root).unwrap_err();
        assert!(busy.starts_with("artifact store busy"), "{busy}");
        drop(second);
        assert!(lock_store(&root).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn artifact_export_shim_carries_the_callers_directory_and_quoted_path() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join(SHIM);
        std::fs::write(&shim, shim_script("TEST_PORT", "TEST_TOKEN", false)).unwrap();
        let curl = dir.path().join("curl");
        std::fs::write(
            &curl,
            "#!/bin/sh\ncat > request.argv\nprintf 'true\\n200'\n",
        )
        .unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = std::process::Command::new("sh")
            .arg(&shim)
            .args([
                "export",
                "p-test",
                "--version",
                "2",
                "--out",
                "share # 한.html",
            ])
            .current_dir(dir.path())
            .env("TEST_PORT", "1")
            .env("TEST_TOKEN", "fixture")
            .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let packed = std::fs::read_to_string(dir.path().join("request.argv")).unwrap();
        let request = request_from_argv(&crate::agent_teams::unpack_argv(&packed)).unwrap();
        assert_eq!(request["version"], 2);
        assert_eq!(
            PathBuf::from(request["out"].as_str().unwrap())
                .parent()
                .unwrap()
                .canonicalize()
                .unwrap(),
            dir.path().canonicalize().unwrap()
        );
        assert!(
            request["out"]
                .as_str()
                .unwrap()
                .ends_with("share # 한.html")
        );
        let powershell = shim_script("TEST_PORT", "TEST_TOKEN", true);
        assert!(powershell.contains("@('--cwd', (Get-Location).Path)"));
    }

    /// A publish carries the pane its shell sits in and the folder it stands
    /// in, after the caller's own words so the door's are the ones that
    /// stand; a shell with no pane key sends none. The PowerShell twin says
    /// the same.
    #[cfg(unix)]
    #[test]
    fn artifact_publish_shim_carries_the_pane_and_the_folder_beside_the_input() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join(SHIM);
        std::fs::write(&shim, shim_script("TEST_PORT", "TEST_TOKEN", false)).unwrap();
        let curl = dir.path().join("curl");
        std::fs::write(
            &curl,
            "#!/bin/sh\ncat > request.argv\nprintf 'true\\n200'\n",
        )
        .unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let publish = |pane: Option<&str>, words: &[&str]| {
            let mut command = std::process::Command::new("sh");
            command
                .arg(&shim)
                .arg("publish")
                .args(words)
                .current_dir(dir.path())
                .env("TEST_PORT", "1")
                .env("TEST_TOKEN", "fixture")
                .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
                .env_remove(crate::hook::PANE_KEY_ENV);
            if let Some(pane) = pane {
                command.env(crate::hook::PANE_KEY_ENV, pane);
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let packed = std::fs::read_to_string(dir.path().join("request.argv")).unwrap();
            publish_parts(request_from_argv(&crate::agent_teams::unpack_argv(&packed)).unwrap())
                .unwrap()
        };
        let (input, caller) = publish(
            Some("term-7"),
            &["--file-path", "/tmp/page.html", "--pane", "term-1"],
        );
        assert_eq!(input.file_path, PathBuf::from("/tmp/page.html"));
        assert_eq!(caller.pane.as_deref(), Some("term-7"));
        assert_eq!(
            caller.cwd.unwrap().canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
        let (_, caller) = publish(None, &["--file-path", "/tmp/page.html"]);
        assert_eq!(caller.pane, None, "a shell with no pane key names none");
        assert!(caller.cwd.is_some());
        let powershell = shim_script("TEST_PORT", "TEST_TOKEN", true);
        assert!(
            powershell.contains(
                "if ($env:ZEROCODE_PANE_KEY) { $args = @($args) + @('--pane', $env:ZEROCODE_PANE_KEY) }"
            ),
            "{powershell}"
        );
    }

    /// Publish keeps the pane and the folder beside its input, and the input
    /// stays strict; the other verbs take the door's words and drop them.
    #[test]
    fn artifact_publish_request_splits_the_caller_from_a_strict_input() {
        let argv = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_string())
                .collect::<Vec<_>>()
        };
        let request = request_from_argv(&argv(&[
            "publish",
            "--file-path",
            "/work/deck.html",
            "--title",
            "Deck",
            "--cwd",
            "/work",
            "--pane",
            "term-4",
        ]))
        .unwrap();
        assert_eq!(
            request,
            serde_json::json!({"action":"publish", "file_path":"/work/deck.html", "title":"Deck",
                "cwd":"/work", "pane":"term-4"})
        );
        let (input, caller) = publish_parts(request).unwrap();
        assert_eq!(input.title.as_deref(), Some("Deck"));
        assert_eq!(
            caller,
            Caller {
                pane: Some("term-4".into()),
                cwd: Some(PathBuf::from("/work")),
            }
        );
        let (_, nobody) =
            publish_parts(serde_json::json!({"action":"publish", "file_path":"/x.html"})).unwrap();
        assert_eq!(nobody, Caller::default());
        assert!(
            publish_parts(
                serde_json::json!({"action":"publish", "file_path":"/x.html", "origin":{}})
            )
            .is_err(),
            "an unknown field is still refused"
        );
        for verb in ["list", "read"] {
            let work = crate::test_paths::absolute("/work");
            let work = work.to_str().unwrap();
            let request =
                request_from_argv(&argv(&[verb, "--cwd", work, "--pane", "term-4"])).unwrap();
            assert_eq!(request, serde_json::json!({"action": verb}));
        }
        let work = crate::test_paths::absolute("/work");
        let export = request_from_argv(&argv(&[
            "export",
            "p-1",
            "--out",
            "a.html",
            "--cwd",
            work.to_str().unwrap(),
            "--pane",
            "term-4",
        ]))
        .unwrap();
        assert_eq!(
            export,
            serde_json::json!({"action":"export", "id":"p-1", "out":work.join("a.html")})
        );
        assert!(request_from_argv(&argv(&["publish", "--pane"])).is_err());
    }

    #[test]
    fn artifact_export_cli_carries_id_version_and_output_path() {
        let out = crate::test_paths::absolute("/tmp/share # 한.html");
        let out = out.to_str().unwrap();
        let words = ["export", "p-test", "--version", "2", "--out", out];
        let request = request_from_argv(&words.map(str::to_owned)).unwrap();
        assert_eq!(
            request,
            serde_json::json!({"action":"export", "id":"p-test", "version":2, "out":out})
        );
        let page = crate::test_paths::absolute("/tmp/page.html");
        for version in ["0", "-1", "1.5", "4294967296"] {
            let words = [
                "export",
                "p-test",
                "--version",
                version,
                "--out",
                page.to_str().unwrap(),
            ];
            assert!(request_from_argv(&words.map(str::to_owned)).is_err());
        }
    }

    /// One published page in a scratch store, for the export tests below.
    fn published_page(dir: &Path, html: &str) -> PageMeta {
        let source = dir.join("input.html");
        std::fs::write(&source, html).unwrap();
        publish(
            dir,
            &PublishInput {
                file_path: source,
                ..Default::default()
            },
            &Limits::default(),
        )
        .unwrap()
    }

    /// The door names the kind of file it writes by the extension of `--out`
    /// (t-18558). It used to copy the page's HTML bytes into whatever name it
    /// was given, so `--out report.pdf` left a file that claims to be a PDF
    /// and opens in no viewer. With no renderer behind the call (the headless
    /// catalog, a build without the window's WebKit) a `.pdf` or `.png` is
    /// refused with its format named, and nothing is written.
    #[test]
    fn an_export_named_pdf_or_png_is_never_html_bytes_in_disguise() {
        let dir = tempfile::tempdir().unwrap();
        let meta = published_page(dir.path(), "<title>Report</title><main>hello</main>");
        for (name, format) in [
            ("report.pdf", "pdf"),
            ("REPORT.PDF", "pdf"),
            ("picture.png", "png"),
        ] {
            let out = dir.path().join(name);
            let refused = export(
                dir.path(),
                &ExportInput {
                    id: meta.id.clone(),
                    version: None,
                    out: out.clone(),
                },
            )
            .expect_err(&format!("{name} was written as the page's HTML bytes"));
            assert!(
                refused.to_lowercase().contains(format),
                "{name}: the refusal does not name the format: {refused}"
            );
            assert!(!out.exists(), "{name}: a refused export left a file behind");
        }
    }

    /// Any other name — another extension, none, or an empty one — is refused
    /// with the formats the door can write, at the argv (before the window is
    /// asked) and again at the export itself.
    #[test]
    fn an_export_with_any_other_extension_is_refused_with_the_list_of_formats() {
        let dir = tempfile::tempdir().unwrap();
        let meta = published_page(dir.path(), "<main>hello</main>");
        for name in [
            "notes.txt",
            "deck.docx",
            "noext",
            "report.pdf.bak",
            "trailing.",
        ] {
            let out = dir.path().join(name);
            let refused = export(
                dir.path(),
                &ExportInput {
                    id: meta.id.clone(),
                    version: None,
                    out: out.clone(),
                },
            )
            .expect_err(&format!("{name} was written as HTML"));
            for format in ["html", "htm", "pdf", "png"] {
                assert!(
                    refused.contains(format),
                    "{name}: the refusal leaves out {format}: {refused}"
                );
            }
            assert!(!out.exists(), "{name}: a refused export left a file behind");
        }
        let notes = crate::test_paths::absolute("/tmp/notes.txt");
        let words = ["export", "p-test", "--out", notes.to_str().unwrap()];
        let early = request_from_argv(&words.map(str::to_owned))
            .expect_err("the argv accepted a name the door cannot write");
        assert!(early.contains("pdf") && early.contains("png"), "{early}");
    }

    /// `.html` and `.htm`, in any case, keep today's copy: the version's own
    /// bytes, byte for byte. (Three different names: this folder's file system
    /// may not tell `a.html` from `A.HTML`, and an export never overwrites.)
    #[test]
    fn an_export_named_html_or_htm_keeps_copying_the_versions_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let meta = published_page(dir.path(), "<title>Report</title><main>hello</main>");
        let snapshot = std::fs::read(meta.path()).unwrap();
        for name in ["page.html", "UPPER.HTML", "third.htm"] {
            let out = dir.path().join(name);
            export(
                dir.path(),
                &ExportInput {
                    id: meta.id.clone(),
                    version: None,
                    out: out.clone(),
                },
            )
            .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(std::fs::read(&out).unwrap(), snapshot, "{name}");
        }
    }

    /// A renderer with a fixed answer that remembers what it was asked — the
    /// seam's stand-in, since a unit test cannot start WebKit.
    struct Fake {
        can: bool,
        answer: Result<Rendered, String>,
        asked: std::sync::Mutex<Vec<(PathBuf, ExportFormat)>>,
    }

    impl Fake {
        fn answering(answer: Result<Rendered, String>) -> Self {
            Self {
                can: true,
                answer,
                asked: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn drawing(bytes: &[u8]) -> Self {
            Self::answering(Ok(Rendered {
                bytes: bytes.to_vec(),
                pages: Some(2),
                ..Rendered::default()
            }))
        }

        fn asked(&self) -> Vec<(PathBuf, ExportFormat)> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl PageRenderer for Fake {
        fn can_render(&self, _: ExportFormat) -> bool {
            self.can
        }

        fn render(&self, page: &Path, format: ExportFormat) -> Result<Rendered, String> {
            self.asked
                .lock()
                .unwrap()
                .push((page.to_path_buf(), format));
            self.answer.clone()
        }
    }

    /// The export names its kind by the extension, asks the renderer for the
    /// version's own immutable file — not the mutable latest copy — writes what
    /// it drew byte for byte, and answers the page count and the picture's size.
    #[test]
    fn a_drawn_export_asks_the_renderer_for_the_versions_file_and_writes_what_it_drew() {
        let dir = tempfile::tempdir().unwrap();
        let first = published_page(dir.path(), "<main>first</main>");
        std::fs::write(dir.path().join("input.html"), "<main>second</main>").unwrap();
        publish(
            dir.path(),
            &PublishInput {
                file_path: dir.path().join("input.html"),
                ..Default::default()
            },
            &Limits::default(),
        )
        .unwrap();

        let pdf = Fake::drawing(b"%PDF-1.7 drawn");
        let out = dir.path().join("report.pdf");
        let done = export_with(
            dir.path(),
            &ExportInput {
                id: first.id.clone(),
                version: Some(1),
                out: out.clone(),
            },
            Some(&pdf),
        )
        .unwrap();
        assert_eq!(
            pdf.asked(),
            vec![(version_file(dir.path(), &first.id, 1), ExportFormat::Pdf)]
        );
        assert!(
            version_file(dir.path(), &first.id, 1).is_file(),
            "the renderer was pointed at a file that does not exist"
        );
        assert_eq!(std::fs::read(&out).unwrap(), b"%PDF-1.7 drawn");
        assert_eq!(
            (done.version, done.format, done.pages, done.bytes),
            (1, ExportFormat::Pdf, Some(2), 14)
        );
        let json = serde_json::to_value(&done).unwrap();
        assert_eq!(json["format"], "pdf");
        assert_eq!(json["pages"], 2);
        assert!(
            json.get("truncated").is_none() && json.get("width").is_none(),
            "a pdf answered picture fields: {json}"
        );

        let picture = Fake::answering(Ok(Rendered {
            bytes: b"\x89PNG drawn".to_vec(),
            width: Some(2560),
            height: Some(16384),
            truncated: true,
            ..Rendered::default()
        }));
        let out = dir.path().join("Picture.PNG");
        let done = export_with(
            dir.path(),
            &ExportInput {
                id: first.id.clone(),
                version: None,
                out: out.clone(),
            },
            Some(&picture),
        )
        .unwrap();
        assert_eq!(picture.asked()[0].1, ExportFormat::Png);
        assert_eq!(
            picture.asked()[0].0,
            version_file(dir.path(), &first.id, 2),
            "no version named asks for the newest kept one"
        );
        let json = serde_json::to_value(&done).unwrap();
        assert_eq!(
            (
                json["format"].as_str(),
                json["width"].as_u64(),
                json["height"].as_u64()
            ),
            (Some("png"), Some(2560), Some(16384))
        );
        assert_eq!(json["truncated"], true);
    }

    /// A drawn export that cannot happen writes nothing: no renderer that can
    /// draw the kind, a version retention dropped, a renderer that fails or
    /// draws nothing, and a name already taken (the file there is left as it
    /// was).
    #[test]
    fn a_drawn_export_that_cannot_happen_writes_nothing_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let meta = published_page(dir.path(), "<main>hello</main>");
        let export_as = |name: &str, version: Option<u32>, renderer: &Fake| {
            let out = dir.path().join(name);
            let answer = export_with(
                dir.path(),
                &ExportInput {
                    id: meta.id.clone(),
                    version,
                    out: out.clone(),
                },
                Some(renderer),
            );
            (answer, out)
        };

        let mut cannot = Fake::drawing(b"%PDF-");
        cannot.can = false;
        let (refused, out) = export_as("a.pdf", None, &cannot);
        assert!(refused.unwrap_err().contains("WebKit"));
        assert!(
            cannot.asked().is_empty(),
            "a renderer that cannot was asked"
        );
        assert!(!out.exists());

        let fine = Fake::drawing(b"%PDF-");
        let (dropped, out) = export_as("b.pdf", Some(9), &fine);
        assert!(dropped.unwrap_err().contains("version 9 is not kept"));
        assert!(fine.asked().is_empty(), "a version not kept was drawn");
        assert!(!out.exists());

        let failing = Fake::answering(Err("the page never settled".into()));
        let (failed, out) = export_as("c.png", None, &failing);
        assert_eq!(failed.unwrap_err(), "the page never settled");
        assert!(!out.exists());

        let empty = Fake::answering(Ok(Rendered::default()));
        let (nothing, out) = export_as("d.pdf", None, &empty);
        assert!(nothing.unwrap_err().contains("empty .pdf"));
        assert!(!out.exists());

        let taken = dir.path().join("e.pdf");
        std::fs::write(&taken, "the person's own file").unwrap();
        let (clash, _) = export_as("e.pdf", None, &fine);
        assert!(clash.is_err());
        assert_eq!(
            std::fs::read_to_string(&taken).unwrap(),
            "the person's own file"
        );
    }

    /// A copy never asks the renderer, whatever renderer stands behind the call.
    #[test]
    fn an_html_export_never_asks_the_renderer() {
        let dir = tempfile::tempdir().unwrap();
        let meta = published_page(dir.path(), "<title>T</title><main>hello</main>");
        let renderer = Fake::drawing(b"%PDF-");
        let out = dir.path().join("copy.htm");
        let done = export_with(
            dir.path(),
            &ExportInput {
                id: meta.id.clone(),
                version: None,
                out: out.clone(),
            },
            Some(&renderer),
        )
        .unwrap();
        assert!(renderer.asked().is_empty());
        assert_eq!(done.format, ExportFormat::Html);
        assert_eq!(
            std::fs::read(&out).unwrap(),
            std::fs::read(meta.path()).unwrap()
        );
        let json = serde_json::to_value(&done).unwrap();
        assert!(
            json.get("pages").is_none() && json.get("truncated").is_none(),
            "{json}"
        );
    }

    /// Kinds read from a wire word only exactly, from an extension in any case,
    /// and a refusal says what it got.
    #[test]
    fn export_kinds_read_words_and_extensions_and_refuse_with_what_they_got() {
        assert_eq!(ExportFormat::from_word("pdf"), Some(ExportFormat::Pdf));
        for word in ["PDF", "htm", ".pdf", ""] {
            assert_eq!(ExportFormat::from_word(word), None, "{word:?}");
        }
        assert_eq!(
            ExportFormat::from_extension("HtM"),
            Some(ExportFormat::Html)
        );
        assert_eq!(ExportFormat::from_extension("Png"), Some(ExportFormat::Png));
        assert_eq!(
            ExportFormat::ALL.map(ExportFormat::word),
            ["html", "pdf", "png"]
        );
        assert!(!ExportFormat::Html.is_drawn());
        assert!(ExportFormat::Pdf.is_drawn() && ExportFormat::Png.is_drawn());
        let error = |name: &str| ExportFormat::of_path(Path::new(name)).unwrap_err();
        assert!(
            error("/x/notes.txt").ends_with("(got .txt)"),
            "{}",
            error("/x/notes.txt")
        );
        assert!(error("/x/noext").ends_with("(got no extension)"));
        assert!(error("/x/trailing.").ends_with("(got no extension)"));
        assert!(
            error("/x/.pdf").ends_with("(got no extension)"),
            "a dotfile has no extension"
        );
        assert_eq!(
            ExportFormat::of_path(Path::new("/x/a.b.PDF")),
            Ok(ExportFormat::Pdf)
        );
    }

    #[test]
    fn artifact_read_returns_the_immutable_version_named_by_its_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("input.html");
        std::fs::write(&source, "<main>first</main>").unwrap();
        let input = PublishInput {
            file_path: source.clone(),
            ..Default::default()
        };
        let first = publish(dir.path(), &input, &Limits::default()).unwrap();
        std::fs::write(&source, "<main>second</main>").unwrap();
        let second = publish(dir.path(), &input, &Limits::default()).unwrap();
        assert!(
            read_page(dir.path(), &first.artifact(Origin::default()))
                .unwrap()
                .contains("first")
        );
        assert!(
            read_page(dir.path(), &second.artifact(Origin::default()))
                .unwrap()
                .contains("second")
        );
    }

    #[test]
    fn artifact_bounds_reject_large_input_and_keep_only_the_configured_versions() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("input.html");
        let file = std::fs::File::create(&source).unwrap();
        file.set_len(ARTIFACT_MAX_BYTES + 1).unwrap();
        let mut input = PublishInput {
            file_path: source.clone(),
            ..Default::default()
        };
        assert!(publish(dir.path(), &input, &Limits::default()).is_err());
        std::fs::write(&source, "<main>ok</main>").unwrap();
        input.title = Some("가".repeat(ARTIFACT_TITLE_MAX + 1));
        assert!(publish(dir.path(), &input, &Limits::default()).is_err());
        input.title = Some("A Page".into());
        input.favicon = Some("👩🏽‍💻🌿".into());
        let limits = Limits {
            versions_per_artifact_max: 2,
            ..Limits::default()
        };
        let mut meta = publish(dir.path(), &input, &limits).unwrap();
        for _ in 0..3 {
            meta = publish(dir.path(), &input, &limits).unwrap();
        }
        assert_eq!(meta.version, 4);
        assert_eq!(
            version_numbers(&dir.path().join(PAGES_DIR).join(&meta.id)),
            vec![3, 4]
        );
        assert!(read_meta(dir.path(), "../outside").is_err());
        assert!(
            skeleton::title(&format!(
                "{}<title>Too late</title>",
                "가".repeat(ARTIFACT_TITLE_SCAN_BYTES)
            ))
            .is_none()
        );
    }

    /// A version picker and an export see the same kept versions: each with
    /// its immutable file and the digest written beside it, the row names
    /// its editable source, and a version retention dropped is refused by
    /// name rather than by a bare missing-file error.
    #[test]
    fn artifact_versions_name_each_kept_snapshot_with_its_digest_and_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("input.html");
        std::fs::write(&source, "<main>one</main>").unwrap();
        let input = PublishInput {
            file_path: source.clone(),
            ..Default::default()
        };
        let limits = Limits {
            versions_per_artifact_max: 2,
            ..Limits::default()
        };
        let first = publish(dir.path(), &input, &limits).unwrap();
        std::fs::write(&source, "<main>two</main>").unwrap();
        publish(dir.path(), &input, &limits).unwrap();
        std::fs::write(&source, "<main>three</main>").unwrap();
        let third = publish(dir.path(), &input, &limits).unwrap();
        let kept = versions(dir.path(), &third.id);
        assert_eq!(kept.iter().map(|one| one.n).collect::<Vec<_>>(), vec![2, 3]);
        assert_eq!(kept[1].sha256.as_deref(), Some(third.sha256.as_str()));
        assert!(
            std::fs::read_to_string(&kept[0].path)
                .unwrap()
                .contains("two")
        );
        assert_eq!(
            third.artifact(Origin::default()).source_path,
            Some(source.canonicalize().unwrap())
        );
        assert!(versions(dir.path(), "../outside").is_empty());
        let out = dir.path().join("old.html");
        let pruned = export(
            dir.path(),
            &ExportInput {
                id: first.id.clone(),
                version: Some(1),
                out: out.clone(),
            },
        )
        .unwrap_err();
        assert!(
            pruned.contains("version 1 is not kept") && pruned.contains("kept: 2, 3"),
            "{pruned}"
        );
        assert!(!out.exists());
    }

    #[test]
    fn artifact_skeleton_wraps_fragments_and_preserves_documents() {
        let html = skeleton::wrap("<main>안녕</main>", "A & <B>");
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<title>A &amp; &lt;B&gt;</title>"));
        assert!(html.contains("name=\"viewport\""));
        assert!(html.contains("color-scheme: light dark"));
        let document =
            "<!DOCTYPE html><HTML><head><title>Held</title></head><body>Hi</body></HTML>";
        assert_eq!(skeleton::wrap(document, "Other"), document);
    }

    #[test]
    fn artifact_publish_reuses_identity_and_url_but_keeps_versions() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source #한.html");
        std::fs::write(&source, "<title>First Report</title><main>one</main>").unwrap();
        let request = PublishInput {
            file_path: source,
            ..Default::default()
        };
        let store = root.path().join("artifacts");
        let first = publish(&store, &request, &crate::artifact::Limits::default()).unwrap();
        std::fs::write(&request.file_path, "<main>two</main>").unwrap();
        let second = publish(&store, &request, &crate::artifact::Limits::default()).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(first.url, second.url);
        assert_eq!(second.version, 2);
        assert_eq!(first.title, "First Report");
        assert!(
            store
                .join("pages")
                .join(&first.id)
                .join("v1/index.html")
                .is_file()
        );
        assert!(
            std::fs::read_to_string(second.path())
                .unwrap()
                .contains("two")
        );
    }
}
