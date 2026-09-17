//! The update's disk side (t-3191, docs/design/versioned-auto-update.md
//! §2.2–2.3): the release-list cache, the verified archive, the bundle
//! unpacked *beside* the running one, and the one rename that swaps them.
//!
//! Every judgement lives in `update_runtime` and is pure; this file only
//! touches the file system, and touches the running bundle in exactly one
//! place — [`swap_in`], which `relaunch_window` calls right before the
//! restart. Nothing here overwrites a running binary: the new bundle
//! is renamed into place and the old one renamed aside, so the process that
//! is still running keeps the inodes it mapped
//! (deploy-overwrite-kills-running-binary).

use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::update_runtime::{
    self, FailureWord, HistoryCache, RELEASES_CACHE_FILE, retired_bundle_path, staged_bundle_path,
};

/// The cached release list, or nothing — missing, torn or an older shape all
/// read as 「ask the API」.
pub(crate) fn read_history_cache(dir: &Path) -> Option<HistoryCache> {
    let value = update_runtime::read_json(&dir.join(RELEASES_CACHE_FILE))?;
    serde_json::from_value(value).ok()
}

/// Temp file and rename, so a reader never sees half a list.
pub(crate) fn write_history_cache(dir: &Path, cache: &HistoryCache) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = serde_json::to_vec_pretty(cache).map_err(io::Error::other)?;
    write_then_rename(&dir.join(RELEASES_CACHE_FILE), &text)
}

/// The signature-verified archive, kept on disk between 「내려받기」 and
/// 「준비」 rather than in memory: tens of megabytes that may wait hours.
pub(crate) fn write_archive(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    write_then_rename(&path, bytes)?;
    Ok(path)
}

/// Write beside, then rename over: a reader never sees half a file.
pub(crate) fn write_then_rename(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("bin")
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Why a bundle could not be staged.
#[derive(Debug)]
pub(crate) enum StageError {
    /// The running executable is not inside an `.app`; there is nothing to
    /// put a new bundle beside.
    NotABundle,
    /// The archive is not the shape the bundler writes (`<Name>.app/…`), or
    /// carries a path that escapes.
    Archive(String),
    Io(io::Error),
}

impl From<io::Error> for StageError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl StageError {
    pub(crate) fn word(&self) -> FailureWord {
        match self {
            Self::NotABundle => FailureWord::NotABundle,
            Self::Archive(_) => FailureWord::Malformed,
            Self::Io(_) => FailureWord::Disk,
        }
    }

    pub(crate) fn detail(&self) -> String {
        match self {
            Self::NotABundle => String::new(),
            Self::Archive(why) => why.clone(),
            Self::Io(error) => error.to_string(),
        }
    }
}

/// Unpack the verified `.app.tar.gz` beside `bundle`, at
/// [`staged_bundle_path`], replacing whatever an earlier attempt left there.
/// The archive's one top-level entry must be an `.app`; its name is dropped
/// (the staged directory takes the hidden `.new` name) and every path is
/// checked to stay inside. Returns the staged bundle.
pub(crate) fn stage_beside(bundle: &Path, archive: &Path) -> Result<PathBuf, StageError> {
    if bundle.extension().is_none_or(|ext| ext != "app") {
        return Err(StageError::NotABundle);
    }
    let staged = staged_bundle_path(bundle);
    if staged.exists() {
        std::fs::remove_dir_all(&staged)?;
    }
    let file = std::fs::File::open(archive)?;
    let result = unpack_app(file, &staged);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staged);
    }
    result.map(|()| staged)
}

fn unpack_app(archive: impl Read, staged: &Path) -> Result<(), StageError> {
    let mut entries = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut top: Option<String> = None;
    let mut unpacked = 0usize;
    for entry in entries.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let mut parts = path.components();
        let Some(std::path::Component::Normal(first)) = parts.next() else {
            return Err(StageError::Archive(format!(
                "entry does not start with a plain name: {}",
                path.display()
            )));
        };
        let first = first.to_string_lossy().into_owned();
        if !first.ends_with(".app") {
            return Err(StageError::Archive(format!(
                "the archive's top level is not an .app: {first}"
            )));
        }
        match &top {
            None => top = Some(first),
            Some(held) if *held != first => {
                return Err(StageError::Archive(format!(
                    "two top-level entries: {held} and {first}"
                )));
            }
            Some(_) => {}
        }
        let mut inside = PathBuf::new();
        for part in parts {
            match part {
                std::path::Component::Normal(name) => inside.push(name),
                std::path::Component::CurDir => {}
                other => {
                    return Err(StageError::Archive(format!(
                        "a path escapes the bundle: {} ({other:?})",
                        path.display()
                    )));
                }
            }
        }
        let destination = staged.join(&inside);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if inside.as_os_str().is_empty() {
            std::fs::create_dir_all(&destination)?;
            continue;
        }
        entry.unpack(&destination)?;
        unpacked += 1;
    }
    if unpacked == 0 {
        return Err(StageError::Archive("the archive holds no files".into()));
    }
    if !staged.join("Contents").join("MacOS").is_dir() {
        return Err(StageError::Archive(
            "the unpacked bundle has no Contents/MacOS".into(),
        ));
    }
    Ok(())
}

/// The one swap: the running bundle steps aside as `<Name>.app.old` (a
/// previous `.old` is removed first) and the staged bundle takes its name.
/// Two renames on one volume; the running process keeps its inodes.
pub(crate) fn swap_in(bundle: &Path, staged: &Path) -> io::Result<()> {
    if !staged.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no staged bundle at {}", staged.display()),
        ));
    }
    let retired = retired_bundle_path(bundle);
    if retired.exists() {
        std::fs::remove_dir_all(&retired)?;
    }
    if bundle.exists() {
        std::fs::rename(bundle, &retired)?;
    }
    std::fs::rename(staged, bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A `.app.tar.gz` the way the bundler writes one: `<top>/Contents/…`.
    fn archive(dir: &Path, top: &str, files: &[(&str, &[u8], u32)]) -> PathBuf {
        let path = dir.join("ZeroCode_1.3.0.app.tar.gz");
        let file = std::fs::File::create(&path).unwrap();
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            file,
            flate2::Compression::fast(),
        ));
        for (name, data, mode) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(*mode);
            let path = format!("{top}/{name}");
            if path.contains("..") {
                // The tar crate refuses to *write* an escaping path; a hostile
                // archive has no such scruples, so the bytes go in raw.
                let raw = &mut header.as_gnu_mut().unwrap().name;
                raw[..path.len()].copy_from_slice(path.as_bytes());
                header.set_cksum();
                builder.append(&header, *data).unwrap();
            } else {
                header.set_cksum();
                builder.append_data(&mut header, path, *data).unwrap();
            }
        }
        builder
            .into_inner()
            .unwrap()
            .finish()
            .unwrap()
            .flush()
            .unwrap();
        path
    }

    fn fake_bundle(dir: &Path, marker: &str) -> PathBuf {
        let bundle = dir.join("Demo.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::write(bundle.join("Contents/MacOS/demo"), marker).unwrap();
        bundle
    }

    #[test]
    fn a_bundle_is_staged_beside_and_swapped_in_by_rename() {
        let home = tempfile::tempdir().unwrap();
        let bundle = fake_bundle(home.path(), "old");
        let tarball = archive(
            home.path(),
            "Demo.app",
            &[
                ("Contents/MacOS/demo", b"new", 0o755),
                ("Contents/Info.plist", b"<plist/>", 0o644),
                ("Contents/Resources/bin/zo", b"zo", 0o755),
            ],
        );
        let staged = stage_beside(&bundle, &tarball).expect("staged");
        assert_eq!(staged, home.path().join(".Demo.app.new"));
        assert_eq!(
            std::fs::read(staged.join("Contents/MacOS/demo")).unwrap(),
            b"new"
        );
        assert_eq!(
            std::fs::read(staged.join("Contents/Resources/bin/zo")).unwrap(),
            b"zo"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(staged.join("Contents/MacOS/demo"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "the executable bit survives");
        }
        // The running bundle is untouched until the swap.
        assert_eq!(
            std::fs::read(bundle.join("Contents/MacOS/demo")).unwrap(),
            b"old"
        );
        // Staging again replaces an earlier attempt.
        std::fs::write(staged.join("leftover"), b"x").unwrap();
        let again = stage_beside(&bundle, &tarball).unwrap();
        assert!(!again.join("leftover").exists());

        swap_in(&bundle, &again).expect("swapped");
        assert_eq!(
            std::fs::read(bundle.join("Contents/MacOS/demo")).unwrap(),
            b"new"
        );
        let retired = home.path().join("Demo.app.old");
        assert_eq!(
            std::fs::read(retired.join("Contents/MacOS/demo")).unwrap(),
            b"old",
            "the old bundle is kept aside"
        );
        assert!(!again.exists());
        // A second swap removes the earlier `.old` rather than failing on it.
        let staged = stage_beside(&bundle, &tarball).unwrap();
        swap_in(&bundle, &staged).expect("swapped over an .old");
        assert!(
            swap_in(&bundle, &staged).is_err(),
            "nothing staged is an error, not a swap"
        );
    }

    #[test]
    fn an_archive_that_is_not_an_app_or_escapes_is_refused_and_cleaned_up() {
        let home = tempfile::tempdir().unwrap();
        let bundle = fake_bundle(home.path(), "old");
        let flat = archive(home.path(), "zo-v1.2.7", &[("zo", b"bin", 0o755)]);
        match stage_beside(&bundle, &flat) {
            Err(StageError::Archive(why)) => assert!(why.contains("not an .app"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(!home.path().join(".Demo.app.new").exists(), "cleaned up");

        let escaping = archive(
            home.path(),
            "Demo.app",
            &[
                ("Contents/MacOS/demo", b"x", 0o755),
                ("../outside", b"x", 0o644),
            ],
        );
        assert!(matches!(
            stage_beside(&bundle, &escaping),
            Err(StageError::Archive(_))
        ));
        assert!(!home.path().join("outside").exists());

        let no_macos = archive(
            home.path(),
            "Demo.app",
            &[("Contents/Info.plist", b"x", 0o644)],
        );
        assert!(matches!(
            stage_beside(&bundle, &no_macos),
            Err(StageError::Archive(_))
        ));

        let not_a_bundle = home.path().join("target").join("zerocode-shell");
        assert!(matches!(
            stage_beside(&not_a_bundle, &flat),
            Err(StageError::NotABundle)
        ));
        assert_eq!(StageError::NotABundle.word(), FailureWord::NotABundle);
        assert_eq!(
            StageError::Archive("x".into()).word(),
            FailureWord::Malformed
        );
        assert_eq!(
            StageError::Io(io::Error::other("full")).word(),
            FailureWord::Disk
        );
        assert!(matches!(
            stage_beside(&bundle, &home.path().join("missing.tar.gz")),
            Err(StageError::Io(_))
        ));
    }

    #[test]
    fn the_cache_and_the_archive_are_written_whole() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join("update");
        assert_eq!(read_history_cache(&dir), None);
        let cache = HistoryCache {
            fetched_at: "2026-09-08T00:00:00.000Z".into(),
            releases: vec![update_runtime::Release {
                tag: "v1.3.0".into(),
                name: "ZeroCode 1.3.0".into(),
                body: "notes".into(),
                published_at: "2026-09-09T00:00:00Z".into(),
                prerelease: false,
            }],
        };
        write_history_cache(&dir, &cache).unwrap();
        assert_eq!(read_history_cache(&dir), Some(cache));
        assert!(!dir.join("releases.json.tmp").exists(), "no temp file left");
        std::fs::write(dir.join(RELEASES_CACHE_FILE), "{").unwrap();
        assert_eq!(read_history_cache(&dir), None, "torn is 「ask again」");

        let path = write_archive(&dir, &update_runtime::archive_file_name("1.3.0"), b"gz").unwrap();
        assert_eq!(path, dir.join("ZeroCode_1.3.0.app.tar.gz"));
        assert_eq!(std::fs::read(&path).unwrap(), b"gz");
    }
}
