//! Read-only reader for Electron's `asar` archive format.
//!
//! Written for one job: studying a locally installed Electron application
//! without touching the installed bundle. It only ever opens the archive for
//! reading and writes extracted entries under a caller-supplied output
//! directory. There is deliberately no writer/repacker — re-signing someone
//! else's bundle is not something this tool should make easy.
//!
//! ## Format
//!
//! Two nested Chromium "pickles", which is the part that trips up a from-memory
//! implementation: the outer pickle holds the *whole length of the header
//! pickle including its own length prefix*, so the JSON string length is the
//! fourth `u32`, not the third.
//!
//! ```text
//! offset  0  [u32 = 4]              payload size of the size pickle
//! offset  4  [u32 = header_len]     total bytes of the header pickle
//! offset  8  [u32 = header_len - 4] payload size of the header pickle
//! offset 12  [u32 = json_len]       bytes of JSON that follow
//! offset 16  [json_len bytes]       the directory, then padding to 4
//!            [file bytes ...]       entry offsets are relative to 8 + header_len
//! ```
//!
//! Verified against a real 121 MB bundle: `header_len = 433748`, JSON starting
//! with `{"files":{` at offset 16.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

/// Hard ceiling on the JSON directory block. The real archives are a few MiB of
/// JSON; anything past this is a corrupt or hostile file, not an app bundle.
const MAX_HEADER_BYTES: u32 = 128 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum AsarError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("not an asar archive: {0}")]
    BadMagic(String),
    #[error("header json is malformed: {0}")]
    BadHeader(#[from] serde_json::Error),
    #[error("entry {0} has a size/offset that runs past the end of the archive")]
    TruncatedEntry(String),
    #[error("entry {0} escapes the output directory")]
    UnsafePath(String),
}

type Result<T> = std::result::Result<T, AsarError>;

/// One node of the archive directory: either a directory (`files` present) or a
/// file (`size` + `offset`). Unknown keys are ignored so a newer Electron
/// writing extra metadata does not break the read.
#[derive(Debug, Clone, Deserialize)]
pub struct Node {
    #[serde(default)]
    pub files: Option<BTreeMap<String, Node>>,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub offset: Option<String>,
    /// `true` when the payload lives beside the archive in `app.asar.unpacked`.
    #[serde(default)]
    pub unpacked: bool,
    /// Present on symlink entries; the target path is relative to the archive root.
    #[serde(default)]
    pub link: Option<String>,
}

/// A parsed archive: the directory tree plus where the file payload starts.
#[derive(Debug, Clone)]
pub struct Archive {
    pub root: Node,
    pub data_start: u64,
    pub total_len: u64,
}

/// A flattened file entry, path-joined with `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub offset: u64,
    pub unpacked: bool,
    pub is_link: bool,
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> AsarError + '_ {
    move |source| AsarError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn read_u32(file: &mut File, path: &Path) -> Result<u32> {
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf).map_err(io(path))?;
    Ok(u32::from_le_bytes(buf))
}

/// Parse the archive header. The file is left positioned at `data_start`.
pub fn open(path: &Path) -> Result<(File, Archive)> {
    let mut file = File::open(path).map_err(io(path))?;
    let total_len = file.metadata().map_err(io(path))?.len();

    let pickle_len = read_u32(&mut file, path)?;
    if pickle_len != 4 {
        return Err(AsarError::BadMagic(format!(
            "expected a 4-byte size pickle, found {pickle_len}"
        )));
    }
    let header_len = read_u32(&mut file, path)?;
    if !(12..=MAX_HEADER_BYTES).contains(&header_len) {
        return Err(AsarError::BadMagic(format!(
            "header length {header_len} is out of range"
        )));
    }
    // The header pickle repeats its own payload size before the string.
    let header_payload_len = read_u32(&mut file, path)?;
    if header_payload_len != header_len - 4 {
        return Err(AsarError::BadMagic(format!(
            "header payload {header_payload_len} does not match header length {header_len}"
        )));
    }
    let json_len = read_u32(&mut file, path)?;
    if json_len > header_payload_len - 4 {
        return Err(AsarError::BadMagic(format!(
            "json length {json_len} does not fit in header block {header_len}"
        )));
    }

    let mut json = vec![0u8; json_len as usize];
    file.read_exact(&mut json).map_err(io(path))?;
    let root: Node = serde_json::from_slice(&json)?;

    let data_start = 8 + u64::from(header_len);
    file.seek(SeekFrom::Start(data_start)).map_err(io(path))?;

    Ok((
        file,
        Archive {
            root,
            data_start,
            total_len,
        },
    ))
}

/// Depth-first list of every file entry in the archive.
pub fn entries(archive: &Archive) -> Vec<Entry> {
    let mut out = Vec::new();
    walk(&archive.root, "", &mut out);
    out
}

fn walk(node: &Node, prefix: &str, out: &mut Vec<Entry>) {
    let Some(children) = &node.files else { return };
    for (name, child) in children {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if child.files.is_some() {
            walk(child, &path, out);
            continue;
        }
        out.push(Entry {
            path,
            size: child.size.unwrap_or(0),
            offset: child
                .offset
                .as_deref()
                .and_then(|o| o.parse().ok())
                .unwrap_or(0),
            unpacked: child.unpacked,
            is_link: child.link.is_some(),
        });
    }
}

/// Create `dir` (which must already be under `base`) one component at a time,
/// refusing to descend through a symlink.
///
/// `fs::create_dir_all` follows symlinks, so a link that already exists inside
/// the output directory — left by an earlier extraction, or pointed somewhere
/// by the user — is enough for an archive entry to land its bytes outside the
/// root we were handed. Rejecting `..` in entry paths does not cover this.
fn create_dir_all_no_symlink(base: &Path, dir: &Path) -> Result<()> {
    let rel = dir
        .strip_prefix(base)
        .map_err(|_| AsarError::UnsafePath(dir.display().to_string()))?;
    let mut cursor = base.to_path_buf();
    for part in rel.components() {
        cursor.push(part);
        match fs::symlink_metadata(&cursor) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(AsarError::UnsafePath(cursor.display().to_string()));
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&cursor).map_err(io(&cursor))?;
            }
            Err(err) => return Err(io(&cursor)(err)),
        }
    }
    Ok(())
}

/// Join `rel` under `base`, rejecting anything that would escape it.
fn safe_join(base: &Path, rel: &str) -> Result<PathBuf> {
    let mut out = base.to_path_buf();
    for part in rel.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        let candidate = Path::new(part);
        let mut components = candidate.components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(segment)), None) => out.push(segment),
            _ => return Err(AsarError::UnsafePath(rel.to_string())),
        }
    }
    if !out.starts_with(base) {
        return Err(AsarError::UnsafePath(rel.to_string()));
    }
    Ok(out)
}

/// Extract every entry whose path contains `filter` (empty filter = all).
/// Returns the number of files written. Symlink and `unpacked` entries carry no
/// payload inside the archive and are skipped.
pub fn extract(archive_path: &Path, out_dir: &Path, filter: &str) -> Result<usize> {
    let (mut file, archive) = open(archive_path)?;
    fs::create_dir_all(out_dir).map_err(io(out_dir))?;
    let base = fs::canonicalize(out_dir).map_err(io(out_dir))?;

    let mut written = 0usize;
    for entry in entries(&archive) {
        if entry.is_link || entry.unpacked {
            continue;
        }
        if !filter.is_empty() && !entry.path.contains(filter) {
            continue;
        }
        let start = archive.data_start + entry.offset;
        let end = start
            .checked_add(entry.size)
            .ok_or_else(|| AsarError::TruncatedEntry(entry.path.clone()))?;
        if end > archive.total_len {
            return Err(AsarError::TruncatedEntry(entry.path.clone()));
        }

        let target = safe_join(&base, &entry.path)?;
        if let Some(parent) = target.parent() {
            create_dir_all_no_symlink(&base, parent)?;
        }
        // And the leaf itself: writing to an existing symlink would follow it.
        match fs::symlink_metadata(&target) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(AsarError::UnsafePath(entry.path.clone()));
            }
            _ => {}
        }
        file.seek(SeekFrom::Start(start))
            .map_err(io(archive_path))?;
        let mut buf = vec![0u8; entry.size as usize];
        file.read_exact(&mut buf).map_err(io(archive_path))?;
        fs::write(&target, &buf).map_err(io(&target))?;
        written += 1;
    }
    Ok(written)
}

/// Build an archive from `(path, bytes)` pairs. Exists so the reader can be
/// tested against a fixture we generated ourselves — no third-party archive is
/// ever committed as test data.
pub fn pack(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut tree = serde_json::Map::new();
    let mut payload = Vec::new();
    for (path, bytes) in files {
        let offset = payload.len();
        payload.extend_from_slice(bytes);
        let mut node = serde_json::Map::new();
        node.insert("size".into(), serde_json::json!(bytes.len()));
        node.insert("offset".into(), serde_json::json!(offset.to_string()));
        insert_path(&mut tree, path, serde_json::Value::Object(node));
    }
    let json = serde_json::to_vec(&serde_json::json!({ "files": tree })).expect("header json");

    let json_len = json.len() as u32;
    let padding = (4 - (json_len % 4)) % 4;
    let header_payload_len = 4 + json_len + padding;
    let header_len = 4 + header_payload_len;

    let mut out = Vec::new();
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&header_len.to_le_bytes());
    out.extend_from_slice(&header_payload_len.to_le_bytes());
    out.extend_from_slice(&json_len.to_le_bytes());
    out.extend_from_slice(&json);
    out.extend(std::iter::repeat_n(0u8, padding as usize));
    out.extend_from_slice(&payload);
    out
}

fn insert_path(
    tree: &mut serde_json::Map<String, serde_json::Value>,
    path: &str,
    leaf: serde_json::Value,
) {
    let mut parts = path.split('/').peekable();
    let mut cursor = tree;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            cursor.insert(part.to_string(), leaf);
            return;
        }
        let dir = cursor
            .entry(part.to_string())
            .or_insert_with(|| serde_json::json!({ "files": {} }));
        cursor = dir
            .get_mut("files")
            .and_then(|f| f.as_object_mut())
            .expect("directory node");
    }
}
