//! Phase-4 artifact store: content-addressed persistence for large or important
//! tool outputs.
//!
//! Big command/test/diff outputs shouldn't be rendered without bound into the
//! transcript (and the model's context), but they also shouldn't be lost. This
//! store writes such content once, addressed by its SHA-256, under
//! `.zo/artifacts/<sha256>`, and hands back a small [`ArtifactRef`] (hash +
//! size + kind + a short preview). The truncated text still goes to the model;
//! the full bytes stay recoverable via [`read_artifact`]. Identical content
//! dedups to a single file — the `sha256` *is* the id.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::ToolError;

/// Env override for the store root, mirroring `ZO_WORKFLOW_STORE`. Lets tests
/// (and sandboxed runs) point the store at a temp dir instead of the project.
/// Where content-addressed artifacts are kept. Set by the session at open to
/// the project's home under `~/.zo/projects/<slug>/artifacts`; unset, the
/// store falls back to `.zo/artifacts` under the working directory — which
/// lands in the person's `git status` (measured 6/6 runs in the 2026-09-02
/// benchmark, and once made a rename look unfinished because the old name
/// survived in zo's own artifact log).
pub const ARTIFACT_STORE_ENV: &str = "ZO_ARTIFACT_STORE";

/// The folder beside a project's `sessions/` under the global Zo home that a
/// session's artifacts go to (`<home>/projects/<slug>/artifacts`, the value
/// zo-ide gives [`ARTIFACT_STORE_ENV`] for a session).
pub const PROJECT_ARTIFACTS_DIR: &str = "artifacts";

/// Chars kept inline on the [`ArtifactRef`] so a viewer can show a teaser without
/// reading the whole artifact back.
const PREVIEW_CHARS: usize = 400;

static ARTIFACT_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// What an artifact holds, for display/filtering. Advisory only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    CommandOutput,
    TestLog,
    Diff,
    AgentResult,
    WorkflowReport,
    Generic,
}

/// A small, serializable handle to stored content. The `sha256` IS the artifact
/// id (content-addressed): the same bytes always produce the same ref.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRef {
    pub sha256: String,
    pub size_bytes: u64,
    pub kind: ArtifactKind,
    /// First `PREVIEW_CHARS` chars of the content.
    pub preview: String,
}

/// Store `content` content-addressed by its SHA-256, returning a small ref. A
/// non-project cwd (no resolvable store dir) still returns a valid ref — only the
/// on-disk persistence is skipped, so hash/size/preview are always available.
pub fn store_artifact(content: &str, kind: ArtifactKind) -> Result<ArtifactRef, ToolError> {
    store_artifact_in(artifact_store_dir().as_deref(), content, kind)
}

fn store_artifact_in(
    dir: Option<&Path>,
    content: &str,
    kind: ArtifactKind,
) -> Result<ArtifactRef, ToolError> {
    let sha256 = sha256_hex(content.as_bytes());
    if let Some(dir) = dir {
        persist_artifact(dir, &sha256, content.as_bytes())?;
    }
    Ok(ArtifactRef {
        sha256,
        size_bytes: content.len() as u64,
        kind,
        preview: content.chars().take(PREVIEW_CHARS).collect(),
    })
}

/// Read stored content back by its SHA-256. `None` if absent, corrupt,
/// unreadable, non-UTF-8, or no store dir is resolvable.
#[must_use]
pub fn read_artifact(sha256: &str) -> Option<String> {
    let dir = artifact_store_dir()?;
    read_artifact_in(&dir, sha256)
}

fn read_artifact_in(dir: &Path, sha256: &str) -> Option<String> {
    valid_sha256_hex(sha256)
        .then(|| std::fs::read(dir.join(sha256)).ok())
        .flatten()
        .filter(|bytes| sha256_hex(bytes) == sha256)
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// Persist exactly `content` at its content address. A candidate is fully
/// written and synced before rename, so readers observe either a prior complete
/// artifact or the new complete artifact, never a partial file.
fn persist_artifact(dir: &Path, sha256: &str, content: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(sha256);
    if artifact_matches(&path, sha256, content)? {
        // The same bytes stored again are a fresh use: the age-based sweep
        // (`prune_artifact_files_in`) reads the file's mtime, and an artifact
        // first written forty days ago and stored again today is still read
        // by today's session. Best-effort — a file that cannot be touched is
        // still the right artifact, only its age stays the first write's.
        let _ = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(SystemTime::now()));
        return Ok(());
    }

    let tmp = write_artifact_candidate(&path, content)?;
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let verification = artifact_matches(&path, sha256, content);
            let _ = std::fs::remove_file(&tmp);
            match verification {
                Ok(true) => Ok(()),
                Ok(false) => Err(error),
                Err(verification_error) => Err(verification_error),
            }
        }
    }
}

/// A destination is a dedup hit only if both its bytes and their address match.
/// An absent destination is not a hit; an unreadable one is an explicit error.
fn artifact_matches(path: &Path, sha256: &str, expected: &[u8]) -> std::io::Result<bool> {
    match std::fs::read(path) {
        Ok(actual) => Ok(sha256_hex(&actual) == sha256 && actual == expected),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn write_artifact_candidate(path: &Path, content: &[u8]) -> std::io::Result<PathBuf> {
    for _ in 0..64 {
        let nonce = ARTIFACT_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("tmp.{}.{}", std::process::id(), nonce));
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        if let Err(error) = file.write_all(content).and_then(|()| file.sync_all()) {
            let _ = std::fs::remove_file(&tmp);
            return Err(error);
        }
        return Ok(tmp);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "unable to allocate a unique artifact candidate",
    ))
}

pub(crate) fn delete_artifact_files<I, S>(shas: I) -> usize
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    artifact_store_dir().map_or(0, |dir| delete_artifact_files_in(&dir, shas))
}

fn delete_artifact_files_in<I, S>(dir: &Path, shas: I) -> usize
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    shas.into_iter()
        .filter_map(|sha| {
            let sha = sha.as_ref();
            valid_sha256_hex(sha).then(|| dir.join(sha))
        })
        .filter(|path| std::fs::remove_file(path).is_ok())
        .count()
}

fn valid_sha256_hex(sha: &str) -> bool {
    sha.len() == 64 && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Delete content-addressed artifacts whose on-disk mtime is older than
/// `max_age`, returning the number of files removed. Unlike
/// `delete_artifact_files`, which retires a known set of hashes from the
/// workflow retention path, this is a blanket age-based sweep so ordinary tool
/// output artifacts (which no retention path owns) can't accumulate on disk
/// without bound. A non-project cwd (no resolvable store dir) is a no-op.
#[must_use]
pub fn prune_artifact_files_older_than(max_age: Duration) -> usize {
    let Some(dir) = artifact_store_dir() else {
        return 0;
    };
    let cutoff = SystemTime::now()
        .checked_sub(max_age)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    prune_artifact_files_in(&dir, cutoff)
}

/// The age sweep (`prune_artifact_files_in`) for every project's store under
/// `projects_root` (`<home>/projects/<slug>/artifacts`): the boot sweep's
/// [`prune_artifact_files_older_than`] reaches only the store of the project
/// zo was opened in, so a project nobody opened for a month kept every
/// artifact it ever wrote (11,490 files older than 30 days, 72 MB, across
/// this machine's idle projects on 2026-11-10). A store the sweep empties
/// goes too — the next artifact makes it again (`persist_artifact`) — so it
/// does not hold its project's folder from aging out with the sessions.
/// Answers the files removed.
#[must_use]
pub fn prune_artifact_stores_under(projects_root: &Path, cutoff: SystemTime) -> usize {
    let Ok(projects) = std::fs::read_dir(projects_root) else {
        return 0;
    };
    projects
        .filter_map(Result::ok)
        .filter(|project| project.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|project| {
            let store = project.path().join(PROJECT_ARTIFACTS_DIR);
            let removed = prune_artifact_files_in(&store, cutoff);
            // Only an empty folder goes: `remove_dir` refuses any other.
            let _ = std::fs::remove_dir(&store);
            removed
        })
        .sum()
}

/// Remove every direct child of `dir` that is a content-addressed artifact
/// (a 64-char hex sha256 filename) whose mtime predates `cutoff`. Safety
/// guards match the content-address invariant so a stray file can't be
/// collected: only regular files directly under `dir`, only ones named like a
/// sha256, and symlinks / subdirectories are skipped. Individual removal
/// failures are ignored so one locked file can't abort the sweep.
fn prune_artifact_files_in(dir: &Path, cutoff: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.filter_map(Result::ok) {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue; // non-UTF-8 name can't be a sha256 hex address
        };
        if !valid_sha256_hex(&name) {
            continue;
        }
        // `file_type` follows no symlink and needs no extra stat on most
        // platforms; skip anything that isn't a plain file (dirs, symlinks).
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let is_stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .is_ok_and(|modified| modified < cutoff);
        if is_stale && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// Dispatch hook: when output was compressed or truncated, preserve `full`
/// (the pre-transform output) as a content-addressed artifact and return its
/// ref; otherwise `None`. `dir_override` lets a test inject a temp store;
/// `None` resolves the global `.zo/artifacts` dir. A store failure degrades
/// to `None` — the artifact is advisory, never load-bearing.
pub(crate) fn store_transformed(
    dir_override: Option<&Path>,
    full: &str,
    was_compressed: bool,
    was_truncated: bool,
) -> Option<ArtifactRef> {
    if !was_compressed && !was_truncated {
        return None;
    }
    let dir = dir_override
        .map(Path::to_path_buf)
        .or_else(artifact_store_dir);
    let artifact = store_artifact_in(dir.as_deref(), full, ArtifactKind::CommandOutput).ok()?;
    // Phase-6: index the artifact's metadata in the SQLite store (audit/dedup).
    // Only on the production path (`dir_override` is `None`); a test injecting its
    // own artifact dir must not reach into the global workflow store. Best-effort.
    if dir_override.is_none() {
        crate::workflow_tools::record_artifact_meta(&artifact);
    }
    Some(artifact)
}

fn artifact_store_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(ARTIFACT_STORE_ENV) {
        return Some(PathBuf::from(path));
    }
    default_artifact_store_dir()
}

/// Where artifacts go when nothing names a folder: `.zo/artifacts` in the
/// working folder, the product's place (a session names its own by
/// [`ARTIFACT_STORE_ENV`]).
#[cfg(not(test))]
fn default_artifact_store_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    Some(cwd.join(".zo").join("artifacts"))
}

/// This crate's own tests run with the crate's folder as the working folder,
/// and `.gitignore` hides a `.zo/` there: every file tool a test dispatched
/// left its result in `crates/tools/.zo/artifacts` (1,748 files, 10 MB on
/// 2026-10-28). A test that names no folder gets one of the process's own
/// under the temp folder (t-78045). The product's place is unchanged.
#[cfg(test)]
#[allow(clippy::unnecessary_wraps)] // the product's place can fail to resolve; the signature is shared
fn default_artifact_store_dir() -> Option<PathBuf> {
    Some(std::env::temp_dir().join(format!("zo-tools-test-artifacts-{}", std::process::id())))
}

/// Lowercase-hex SHA-256, matching the codebase's existing hashing idiom
/// (`plugins::install`).
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("zo-artifacts-{tag}-{}-{n}", std::process::id()))
    }

    #[test]
    fn sha256_is_content_addressed() {
        let a = sha256_hex(b"hello");
        let b = sha256_hex(b"hello");
        let c = sha256_hex(b"world");
        assert_eq!(a, b, "same bytes hash identically");
        assert_ne!(a, c, "different bytes hash differently");
        assert_eq!(a.len(), 64, "sha-256 is 32 bytes = 64 hex chars");
        // Known vector for "hello".
        assert_eq!(
            a,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn store_then_read_round_trips() {
        let dir = unique_temp_dir("roundtrip");
        let content = "a large command output\n".repeat(50);
        let stored = store_artifact_in(Some(&dir), &content, ArtifactKind::CommandOutput)
            .expect("store succeeds");
        assert_eq!(stored.size_bytes, content.len() as u64);
        assert_eq!(stored.kind, ArtifactKind::CommandOutput);
        assert!(content.starts_with(&stored.preview));
        assert_eq!(
            stored.preview.chars().count(),
            PREVIEW_CHARS.min(content.chars().count())
        );

        let read = read_artifact_in(&dir, &stored.sha256).expect("read back");
        assert_eq!(
            read, content,
            "round-trips byte-for-byte by content address"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn identical_content_dedups_to_one_file() {
        let dir = unique_temp_dir("dedup");
        let content = "same bytes";
        let first = store_artifact_in(Some(&dir), content, ArtifactKind::Generic).unwrap();
        let second = store_artifact_in(Some(&dir), content, ArtifactKind::Generic).unwrap();
        assert_eq!(first, second, "same content → identical ref");
        // Exactly one file in the store (plus no stray temp file after rename).
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "deduped to a single content-addressed file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parallel_same_content_writers_leave_one_valid_artifact() {
        use std::sync::{Arc, Barrier};

        let dir = Arc::new(unique_temp_dir("parallel"));
        let content = Arc::new("same content from concurrent writers".to_string());
        let barrier = Arc::new(Barrier::new(4));
        let handles = (0..4)
            .map(|_| {
                let dir = Arc::clone(&dir);
                let content = Arc::clone(&content);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store_artifact_in(Some(&dir), &content, ArtifactKind::Generic)
                })
            })
            .collect::<Vec<_>>();
        let refs = handles
            .into_iter()
            .map(|handle| handle.join().expect("writer thread").expect("store"))
            .collect::<Vec<_>>();
        assert!(refs.windows(2).all(|pair| pair[0] == pair[1]));
        assert_eq!(
            read_artifact_in(&dir, &refs[0].sha256).as_deref(),
            Some(content.as_str())
        );
        assert_eq!(std::fs::read_dir(&*dir).expect("artifact dir").count(), 1);
        let _ = std::fs::remove_dir_all(&*dir);
    }

    #[cfg(unix)]
    #[test]
    fn corrupt_destination_is_repaired_before_returning_a_ref() {
        let dir = unique_temp_dir("corrupt");
        let content = "correct content";
        let sha256 = sha256_hex(content.as_bytes());
        std::fs::create_dir_all(&dir).expect("artifact dir");
        std::fs::write(dir.join(&sha256), "corrupt content").expect("corrupt destination");
        assert!(read_artifact_in(&dir, &sha256).is_none(), "corruption is rejected on read");

        let stored = store_artifact_in(Some(&dir), content, ArtifactKind::Generic)
            .expect("Unix rename repairs the corrupt destination");
        assert_eq!(stored.sha256, sha256);
        assert_eq!(read_artifact_in(&dir, &sha256).as_deref(), Some(content));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_store_dir_still_returns_a_ref() {
        // No persistence target, but the hash/size/preview are still computed.
        let stored = store_artifact_in(None, "content", ArtifactKind::Diff).expect("ref computed");
        assert_eq!(stored.sha256.len(), 64);
        assert_eq!(stored.size_bytes, 7);
    }

    #[test]
    fn store_transformed_persists_when_compressed_or_truncated() {
        // This is exactly what the dispatch hook calls; an injected dir keeps it
        // hermetic (no global `ZO_ARTIFACT_STORE`, so no race with the process
        // spawns of other tests, and no `.zo/` pollution).
        let dir = unique_temp_dir("store-truncated");

        // Untruncated output → nothing stored, no ref.
        assert!(store_transformed(Some(&dir), "small output", false, false).is_none());
        assert!(!dir.exists() || std::fs::read_dir(&dir).unwrap().next().is_none());

        // Truncated output → full content preserved, recoverable by content address.
        let full = "build log line\n".repeat(5_000);
        let stored =
            store_transformed(Some(&dir), &full, false, true).expect("artifact when truncated");
        assert_eq!(stored.kind, ArtifactKind::CommandOutput);
        assert_eq!(stored.size_bytes, full.len() as u64);
        assert_eq!(
            read_artifact_in(&dir, &stored.sha256).expect("recoverable"),
            full
        );

        // Compression-only output → original content preserved as well.
        let compressed_only = "\u{1b}[32mok\u{1b}[0m\n".repeat(20);
        let stored = store_transformed(Some(&dir), &compressed_only, true, false)
            .expect("artifact when compressed");
        assert_eq!(
            read_artifact_in(&dir, &stored.sha256).expect("recoverable"),
            compressed_only
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_older_than_past_cutoff_keeps_everything() {
        // A cutoff further in the past than any file's mtime removes nothing —
        // deterministic, no sleep: freshly written files are newer than a
        // cutoff set an hour ago.
        let dir = unique_temp_dir("prune-past");
        let a = store_artifact_in(Some(&dir), "one", ArtifactKind::Generic).unwrap();
        let b = store_artifact_in(Some(&dir), "two", ArtifactKind::Generic).unwrap();
        let cutoff = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(prune_artifact_files_in(&dir, cutoff), 0);
        assert!(read_artifact_in(&dir, &a.sha256).is_some());
        assert!(read_artifact_in(&dir, &b.sha256).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_older_than_future_cutoff_removes_only_sha256_files() {
        // A cutoff in the future is newer than every existing mtime, so every
        // *content-addressed* file is stale. Non-sha256 files and subdirs must
        // survive the sweep regardless of age.
        let dir = unique_temp_dir("prune-future");
        let a = store_artifact_in(Some(&dir), "content-a", ArtifactKind::Generic).unwrap();
        let b = store_artifact_in(Some(&dir), "content-b", ArtifactKind::Generic).unwrap();
        // A non-sha256 sibling file and a subdirectory (both must survive).
        std::fs::write(dir.join("index.json"), b"{}").unwrap();
        std::fs::write(dir.join("deadbeef"), b"short name").unwrap();
        std::fs::create_dir_all(dir.join("0".repeat(64))).unwrap(); // sha-shaped dir

        let cutoff = SystemTime::now() + Duration::from_secs(3600);
        assert_eq!(prune_artifact_files_in(&dir, cutoff), 2, "both artifacts stale");
        assert!(read_artifact_in(&dir, &a.sha256).is_none());
        assert!(read_artifact_in(&dir, &b.sha256).is_none());
        assert!(dir.join("index.json").exists(), "non-sha file survives");
        assert!(dir.join("deadbeef").exists(), "short-name file survives");
        assert!(
            dir.join("0".repeat(64)).is_dir(),
            "sha-shaped subdirectory survives"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn storing_the_same_bytes_again_renews_the_artifacts_age() {
        let dir = unique_temp_dir("renew");
        let stored = store_artifact_in(Some(&dir), "used again", ArtifactKind::Generic).unwrap();
        let path = dir.join(&stored.sha256);
        let forty_days = Duration::from_secs(40 * 24 * 60 * 60);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(SystemTime::now() - forty_days))
            .expect("age the artifact");
        let thirty_days_ago = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);

        store_artifact_in(Some(&dir), "used again", ArtifactKind::Generic).unwrap();

        assert_eq!(
            prune_artifact_files_in(&dir, thirty_days_ago),
            0,
            "an artifact stored again today is not forty days old"
        );
        assert!(read_artifact_in(&dir, &stored.sha256).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_projects_store_is_swept_and_nothing_beside_it() {
        let root = unique_temp_dir("projects");
        let store = |slug: &str| root.join(slug).join(PROJECT_ARTIFACTS_DIR);
        let a = store_artifact_in(Some(&store("idle-a")), "a", ArtifactKind::Generic).unwrap();
        let b = store_artifact_in(Some(&store("idle-b")), "b", ArtifactKind::Generic).unwrap();
        // A project's other folders hold artifact-shaped names too; they are not its store.
        std::fs::create_dir_all(root.join("idle-a").join("state")).unwrap();
        std::fs::write(root.join("idle-a").join("state").join(&a.sha256), b"a").unwrap();
        std::fs::write(root.join("stray-file"), b"not a project").unwrap();

        let removed = prune_artifact_stores_under(&root, SystemTime::now() + Duration::from_secs(3600));

        assert_eq!(removed, 2, "one in each project's store");
        assert!(read_artifact_in(&store("idle-a"), &a.sha256).is_none());
        assert!(read_artifact_in(&store("idle-b"), &b.sha256).is_none());
        assert!(!store("idle-b").exists(), "an emptied store does not hold its project's folder");
        assert!(root.join("idle-a").join("state").join(&a.sha256).exists(), "state is not the store");
        assert!(root.join("stray-file").exists());
        assert_eq!(prune_artifact_stores_under(&root.join("absent"), SystemTime::now()), 0);
        let fresh = store_artifact_in(Some(&store("busy")), "fresh", ArtifactKind::Generic).unwrap();
        assert_eq!(prune_artifact_stores_under(&root, SystemTime::now() - Duration::from_secs(3600)), 0);
        assert!(read_artifact_in(&store("busy"), &fresh.sha256).is_some(), "a store with a fresh artifact stays");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_artifact_files_removes_only_valid_content_addresses() {
        let dir = unique_temp_dir("gc");
        let stored = store_artifact_in(Some(&dir), "obsolete", ArtifactKind::Generic).unwrap();
        let keep = store_artifact_in(Some(&dir), "keep", ArtifactKind::Generic).unwrap();
        let deleted = delete_artifact_files_in(&dir, [stored.sha256.as_str(), "../not-a-sha"]);
        assert_eq!(deleted, 1);
        assert!(read_artifact_in(&dir, &stored.sha256).is_none());
        assert!(read_artifact_in(&dir, &keep.sha256).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
