//! The record of the local files zo wrote: what makes a folder's
//! `.zo/settings.local.json` and `.zo/trusted-mcp-servers.json` the person's.

use super::{forget, may_write, recorded, remember, RECORD_FILE};
use crate::config::ConfigLoader;
use std::fs;
use std::path::PathBuf;

/// A project folder with a local settings file, and a config home of its own.
struct Folder {
    _root: crate::test_root::TestRoot,
    cwd: PathBuf,
    home: PathBuf,
}

const HOOKED: &str = r#"{"hooks":{"SessionStart":["echo local"]}}"#;

impl Folder {
    fn with_local(contents: &str) -> Self {
        let root = crate::test_root::TestRoot::new("zo-local-record");
        let cwd = root.join("project");
        let home = root.join("home").join(".zo");
        fs::create_dir_all(cwd.join(".zo")).expect("project .zo");
        fs::create_dir_all(&home).expect("config home");
        fs::write(cwd.join(".zo").join("settings.local.json"), contents).expect("local file");
        Self { cwd, home, _root: root }
    }

    fn local(&self) -> PathBuf {
        self.cwd.join(".zo").join("settings.local.json")
    }

    fn real(&self) -> PathBuf {
        self.local().canonicalize().expect("canonical")
    }

    fn record_as_written(&self) {
        let bytes = fs::read(self.local()).expect("read");
        remember(&self.home, &self.real(), &bytes).expect("record");
    }

    fn hooks(&self) -> usize {
        ConfigLoader::new(&self.cwd, &self.home)
            .load()
            .expect("config loads")
            .hooks()
            .session_start()
            .len()
    }
}

/// What zo wrote reads as the person's; a byte changed by hand, or the file
/// taken out of the record, reads as the repository's again.
#[test]
#[cfg(unix)]
fn a_file_is_the_persons_only_while_its_bytes_are_what_zo_wrote() {
    let folder = Folder::with_local(HOOKED);
    assert_eq!(folder.hooks(), 0, "not recorded");
    folder.record_as_written();
    assert_eq!(folder.hooks(), 1, "recorded");

    fs::write(folder.local(), HOOKED.replace("local", "edited")).expect("hand edit");
    assert_eq!(folder.hooks(), 0, "changed since zo wrote it");

    fs::write(folder.local(), HOOKED).expect("put back");
    assert_eq!(folder.hooks(), 1);
    assert!(forget(&folder.home, &folder.real()).expect("forget"));
    assert_eq!(folder.hooks(), 0, "forgotten");
}

/// The record names the path: the same bytes in the folder moved elsewhere are
/// not the person's there — a hook's relative path runs the new folder's file.
#[test]
#[cfg(unix)]
fn a_moved_folder_is_asked_again() {
    let folder = Folder::with_local(HOOKED);
    folder.record_as_written();
    assert_eq!(folder.hooks(), 1);

    let moved = folder.cwd.with_file_name("moved");
    fs::rename(&folder.cwd, &moved).expect("move the folder");
    let loaded = ConfigLoader::new(&moved, &folder.home).load().expect("config loads");
    assert!(loaded.hooks().session_start().is_empty(), "moved: not the person's");
    assert_eq!(loaded.feature_config().unowned_local_settings(), ["hooks"]);
}

/// A record another account could write is no record: it is neither read nor
/// written over, and the file reads as the repository's.
#[test]
#[cfg(unix)]
fn a_record_another_account_can_write_is_ignored() {
    use std::os::unix::fs::PermissionsExt as _;
    let folder = Folder::with_local(HOOKED);
    folder.record_as_written();
    assert_eq!(folder.hooks(), 1);

    let record = folder.home.join(RECORD_FILE);
    fs::set_permissions(&record, fs::Permissions::from_mode(0o666)).expect("chmod");
    assert_eq!(folder.hooks(), 0, "a record others can write");
    let bytes = fs::read(folder.local()).expect("read");
    assert!(remember(&folder.home, &folder.real(), &bytes).is_err(), "not written over");

    fs::set_permissions(&record, fs::Permissions::from_mode(0o600)).expect("chmod back");
    assert_eq!(folder.hooks(), 1);
    fs::set_permissions(&folder.home, fs::Permissions::from_mode(0o777)).expect("open home");
    assert_eq!(folder.hooks(), 0, "a home others can write");
    fs::set_permissions(&folder.home, fs::Permissions::from_mode(0o700)).expect("close home");
}

/// A record the folder brings is never read: only the config home's is.
#[test]
#[cfg(unix)]
fn a_record_inside_the_folder_is_not_read() {
    let folder = Folder::with_local(HOOKED);
    let elsewhere = folder.cwd.join(".zo");
    let bytes = fs::read(folder.local()).expect("read");
    remember(&elsewhere, &folder.real(), &bytes).expect("a record in the folder");
    remember(&folder.cwd, &folder.real(), &bytes).expect("and at its root");
    assert_eq!(folder.hooks(), 0);
}

/// zo writes a recorded-kind file only where nothing stands or where it wrote
/// the bytes on it; any other path (the config home's own file, a document that
/// is not a local control file) it may write.
#[test]
#[cfg(unix)]
fn zo_writes_only_where_nothing_stands_or_where_it_wrote() {
    let folder = Folder::with_local(HOOKED);
    assert!(!may_write(&folder.home, &folder.local()), "the folder came with it");
    folder.record_as_written();
    assert!(may_write(&folder.home, &folder.local()), "zo wrote it");
    let trusted = folder.cwd.join(".zo").join("trusted-mcp-servers.json");
    assert!(may_write(&folder.home, &trusted), "nothing stands there");
    fs::write(&trusted, "[\"repo\"]").expect("a record the folder came with");
    assert!(!may_write(&folder.home, &trusted));
    assert!(may_write(&folder.home, &folder.cwd.join(".zo").join("settings.json")));
    let own = folder.home.join("settings.local.json");
    fs::write(&own, "{}").expect("the config home's own file");
    assert!(may_write(&folder.home, &own));
}

/// Entries for files that are gone are dropped when the record is next written.
#[test]
#[cfg(unix)]
fn entries_for_files_that_are_gone_are_dropped() {
    let folder = Folder::with_local(HOOKED);
    folder.record_as_written();
    let other = Folder::with_local("{}");
    let other_real = other.real();
    fs::remove_file(other.local()).expect("gone");
    remember(&folder.home, &other_real, b"{}").expect("record a file that then goes");
    fs::write(other.local(), "{}").expect("back for a moment");
    assert!(recorded(&folder.home, &other_real, b"{}"));
    fs::remove_file(other.local()).expect("gone again");
    folder.record_as_written();
    let text = fs::read_to_string(folder.home.join(RECORD_FILE)).expect("record");
    assert!(!text.contains(&*other_real.to_string_lossy()), "{text}");
    assert!(text.contains(&*folder.real().to_string_lossy()), "{text}");
}

