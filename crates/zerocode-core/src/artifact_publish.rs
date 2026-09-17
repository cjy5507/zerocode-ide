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
pub const USAGE: &str = "zerocode-artifact publish --file-path <absolute.html> [--title <title>] [--description <text>] [--favicon <emoji>] [--label <label>]\nzerocode-artifact list\nzerocode-artifact read --id <id>\nzerocode-artifact export <id> [--version N] --out <path>";

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishInput {
    pub file_path: PathBuf,
    pub title: Option<String>,
    pub description: Option<String>,
    pub favicon: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExportInput {
    pub id: String,
    pub version: Option<u32>,
    pub out: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct ExportedFile {
    pub id: String,
    pub version: u32,
    pub path: PathBuf,
    pub bytes: u64,
}

pub fn validate_export(input: &ExportInput) -> Result<(), String> {
    validate_id(&input.id)?;
    if input.version == Some(0) {
        return Err("artifact version must be positive".into());
    }
    if !input.out.is_absolute() {
        return Err("artifact export needs an absolute output path".into());
    }
    Ok(())
}

/// Export exactly one immutable publication snapshot. Never truncate an
/// existing destination (including store files or a symlink into the store).
pub fn export(root: &Path, input: &ExportInput) -> Result<ExportedFile, String> {
    validate_export(input)?;
    let meta = read_meta(root, &input.id)?;
    let version = input.version.unwrap_or(meta.version);
    let kept: Vec<u32> = versions(root, &input.id)
        .into_iter()
        .map(|kept| kept.n)
        .collect();
    if !kept.contains(&version) {
        let kept = kept
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "artifact {} version {version} is not kept (kept: {kept})",
            input.id
        ));
    }
    let mut artifact = meta.artifact(Origin::default());
    artifact.version = Some(version);
    let html = read_page(root, &artifact)?;
    let mut file = std::fs::File::create_new(&input.out).map_err(|e| e.to_string())?;
    if let Err(error) = file.write_all(html.as_bytes()) {
        drop(file);
        let _ = std::fs::remove_file(&input.out);
        return Err(error.to_string());
    }
    Ok(ExportedFile {
        id: input.id.clone(),
        version,
        path: input.out.clone(),
        bytes: html.len() as u64,
    })
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
        |version| {
            root.join(PAGES_DIR)
                .join(&artifact.id)
                .join(format!("v{version}"))
                .join("index.html")
        },
    );
    read_bounded(&path, ARTIFACT_MAX_BYTES + ARTIFACT_TITLE_SCAN_BYTES as u64)
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
pub fn request_from_argv(argv: &[String]) -> Result<serde_json::Value, String> {
    let mut value = serde_json::json!({ "action": argv.first().ok_or(USAGE)? });
    let mut words = argv.iter().skip(1);
    let mut cwd = None;
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

    #[test]
    fn artifact_export_cli_carries_id_version_and_output_path() {
        let words = [
            "export",
            "p-test",
            "--version",
            "2",
            "--out",
            "/tmp/share # 한.html",
        ];
        let request = request_from_argv(&words.map(str::to_owned)).unwrap();
        assert_eq!(
            request,
            serde_json::json!({"action":"export", "id":"p-test", "version":2, "out":"/tmp/share # 한.html"})
        );
        for version in ["0", "-1", "1.5", "4294967296"] {
            let words = [
                "export",
                "p-test",
                "--version",
                version,
                "--out",
                "/tmp/page.html",
            ];
            assert!(request_from_argv(&words.map(str::to_owned)).is_err());
        }
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
