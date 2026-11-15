//! The record of the local files zo wrote: what makes a folder's
//! `.zo/settings.local.json` and `.zo/trusted-mcp-servers.json` the person's.

use super::{
    carries_no_authority, forget, may_write, recorded, remember, widens, write_local_file, RECORD_FILE,
};
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

/// zo writes a local file only in the folder's own `.zo`: through a `.zo` that
/// links to a neighbouring project's or to the config home, `zo mcp --trust`
/// and `add --project` would write where this folder's loader never reads.
/// That holds in the person's home folder too: a config home that is itself a
/// link (a dotfiles folder) is one the loader does not read a local file
/// through, so zo does not write one there (153).
#[test]
fn zo_writes_a_local_file_only_in_the_folders_own_dot_zo() {
    let folder = Folder::with_local("{}");
    let root = folder.cwd.parent().expect("root").to_path_buf();
    let neighbour = root.join("neighbour").join(".zo");
    fs::create_dir_all(&neighbour).expect("the neighbouring project's .zo");
    let linked = root.join("linked");
    fs::create_dir_all(&linked).expect("linked");
    std::os::unix::fs::symlink(&neighbour, linked.join(".zo")).expect("link");
    for name in super::RECORDED_FILES {
        assert!(!may_write(&folder.home, &linked.join(".zo").join(name)), "{name}");
    }
    let to_home = root.join("to-home");
    fs::create_dir_all(&to_home).expect("to-home");
    std::os::unix::fs::symlink(&folder.home, to_home.join(".zo")).expect("link to the home");
    let through = to_home.join(".zo").join("trusted-mcp-servers.json");
    assert!(!may_write(&folder.home, &through), "the home, reached through a folder's .zo");

    let fresh = root.join("fresh");
    fs::create_dir_all(&fresh).expect("fresh");
    assert!(may_write(&folder.home, &fresh.join(".zo").join("settings.local.json")), "no .zo yet");
    assert!(may_write(&folder.home, &folder.home.join("trusted-mcp-servers.json")));
    let dotfiles = root.join("dotfiles").join("zo");
    fs::create_dir_all(&dotfiles).expect("dotfiles");
    let person = root.join("person");
    fs::create_dir_all(&person).expect("person");
    std::os::unix::fs::symlink(&dotfiles, person.join(".zo")).expect("a linked config home");
    let linked_home = person.join(".zo");
    assert!(!may_write(&linked_home, &linked_home.join("trusted-mcp-servers.json")));
}

/// The config home's own local files — the person's home folder's — are
/// written only as any folder's are: a trust list or a granting settings file
/// unpacked into `~/.zo` is not one zo wrote; plain settings are taken (153).
#[test]
fn the_config_homes_own_local_files_are_written_only_as_any_folders_are() {
    let folder = Folder::with_local("{}");
    let trusted = folder.home.join("trusted-mcp-servers.json");
    assert!(may_write(&folder.home, &trusted), "nothing stands there");
    fs::write(&trusted, r#"["planted"]"#).expect("a trust list unpacked into ~/.zo");
    assert!(!may_write(&folder.home, &trusted));
    let settings = folder.home.join("settings.local.json");
    fs::write(&settings, HOOKED).expect("granting settings unpacked into ~/.zo");
    assert!(!may_write(&folder.home, &settings));
    fs::write(&settings, r#"{"model":"opus"}"#).expect("plain settings");
    assert!(may_write(&folder.home, &settings));
}

/// What is taken without a record is named, not guessed: the plain settings
/// and the restrictions and modes a repository may set carry no authority;
/// every section the repository's strips take out or cut down, every
/// `enableAllProject…` opt-in, `cache`, `mcpServers`, a grant, a mode a
/// repository may not pin, a key zo does not know, and any trust list do —
/// the loader's own lists, so a section it learns to strip is held here
/// too (153).
#[test]
fn plain_settings_carry_no_authority_and_what_a_repository_strips_does() {
    let settings = |json: &str| carries_no_authority("settings.local.json", json.as_bytes());
    for plain in [
        r#"{}"#,
        r#"{"model":"opus","reasoningEffort":"high","alwaysThinkingEnabled":true}"#,
        r#"{"outputStyle":"Explanatory","language":"ko"}"#,
        r#"{"autoMemoryEnabled":false,"autoDreamEnabled":false,"recallHintEnabled":true}"#,
        r#"{"permissions":{"deny":["Bash(rm*)"],"ask":["Bash(git push*)"],"defaultMode":"plan"}}"#,
        r#"{"permissions":{"defaultMode":"acceptEdits"},"permissionMode":"workspace-write"}"#,
    ] {
        assert!(settings(plain), "{plain}");
    }
    let mut granting: Vec<String> = super::super::REPOSITORY_STRIPPED_SECTIONS
        .iter()
        .chain(&["cache", "mcpServers", "enabledPlugins", "aKeyZoLearnsLater"])
        .map(|section| format!(r#"{{"{section}":{{}}}}"#))
        .collect();
    granting.extend(
        super::super::PROJECT_OPT_INS
            .iter()
            .map(|flag| format!(r#"{{"{flag}":true}}"#)),
    );
    granting.extend(
        [
            r#"{"permissions":{"allow":["Bash(*)"]}}"#,
            r#"{"permissions":{"rules":[]}}"#,
            r#"{"permissions":{"defaultMode":"dontAsk"}}"#,
            r#"{"permissions":{"defaultMode":"danger-full-access"}}"#,
            r#"{"permissions":["allow"]}"#,
            r#"{"permissionMode":"dontAsk"}"#,
            r#"not json"#,
            r#"[]"#,
        ]
        .map(String::from),
    );
    for granting in &granting {
        assert!(!settings(granting), "{granting}");
    }
    assert!(!carries_no_authority("trusted-mcp-servers.json", b"[]"));
}

/// A change widens what a file grants when it adds or changes a grant;
/// a plain setting, a narrowing, or a grant taken out does not (153).
#[test]
fn a_change_widens_only_when_it_grants_what_was_not_granted() {
    let object = |json: &str| match serde_json::from_str(json).expect("json") {
        serde_json::Value::Object(document) => document,
        other => panic!("not an object: {other}"),
    };
    let granted = r#"{"permissions":{"allow":["Bash(cargo*)"],"defaultMode":"dontAsk"}}"#;
    for (before, after, widened) in [
        (granted, r#"{"permissions":{"allow":["Bash(cargo*)"],"defaultMode":"dontAsk"},"model":"opus"}"#, false),
        (granted, r#"{"permissions":{"allow":["Bash(cargo*)"],"defaultMode":"plan"}}"#, false),
        (granted, r#"{}"#, false),
        (r#"{}"#, r#"{"permissions":{"defaultMode":"acceptEdits"}}"#, false),
        (r#"{}"#, r#"{"permissions":{"defaultMode":"dontAsk"}}"#, true),
        (r#"{"permissions":{"defaultMode":"plan"}}"#, r#"{"permissions":{"defaultMode":"danger-full-access"}}"#, true),
        (granted, r#"{"permissions":{"allow":["Bash(*)"],"defaultMode":"dontAsk"}}"#, true),
        (r#"{}"#, r#"{"hooks":{}}"#, true),
    ] {
        assert_eq!(widens(&object(before), &object(after)), widened, "{before} -> {after}");
    }
}

/// The write itself follows no link: a `.zo` that became a link after
/// `may_write` looked, or a link at the file, is refused rather than written
/// through; the folder's own `.zo` is made where none stood (153).
#[test]
fn a_local_file_is_written_only_through_the_folders_own_dot_zo() {
    let folder = Folder::with_local("{}");
    let root = folder.cwd.parent().expect("root").to_path_buf();
    let neighbour = root.join("neighbour").join(".zo");
    fs::create_dir_all(&neighbour).expect("the neighbouring project's .zo");
    let swapped = root.join("swapped");
    fs::create_dir_all(&swapped).expect("swapped");
    std::os::unix::fs::symlink(&neighbour, swapped.join(".zo")).expect("link");
    assert!(write_local_file(&swapped.join(".zo").join("settings.local.json"), b"{}").is_err());
    assert!(!neighbour.join("settings.local.json").exists(), "nothing is made through the link");

    let elsewhere = root.join("elsewhere.json");
    fs::write(&elsewhere, "{}").expect("target");
    let at_the_file = root.join("at-the-file");
    fs::create_dir_all(at_the_file.join(".zo")).expect(".zo");
    std::os::unix::fs::symlink(&elsewhere, at_the_file.join(".zo").join("settings.local.json"))
        .expect("link at the file");
    assert!(write_local_file(&at_the_file.join(".zo").join("settings.local.json"), b"[]").is_err());
    assert_eq!(fs::read_to_string(&elsewhere).expect("target"), "{}");

    let fresh = root.join("fresh");
    fs::create_dir_all(&fresh).expect("fresh");
    write_local_file(&fresh.join(".zo").join("settings.local.json"), br#"{"model":"opus"}"#)
        .expect("the folder's own .zo");
    assert_eq!(
        fs::read_to_string(fresh.join(".zo").join("settings.local.json")).expect("written"),
        r#"{"model":"opus"}"#
    );
}

/// Sessions ending together each keep their line: the record is read and
/// written under its lock (153).
#[test]
fn sessions_recording_at_once_each_keep_their_line() {
    let folder = Folder::with_local("{}");
    let root = folder.cwd.parent().expect("root").to_path_buf();
    let files: Vec<std::path::PathBuf> = (0..8)
        .map(|index| {
            let file = root.join(format!("pane-{index}")).join(".zo").join("settings.local.json");
            fs::create_dir_all(file.parent().expect(".zo")).expect(".zo");
            fs::write(&file, format!("{{\"n\":{index}}}")).expect("file");
            file.canonicalize().expect("canonical")
        })
        .collect();
    std::thread::scope(|scope| {
        for (index, file) in files.iter().enumerate() {
            let home = &folder.home;
            scope.spawn(move || {
                remember(home, file, format!("{{\"n\":{index}}}").as_bytes()).expect("record");
            });
        }
    });
    for (index, file) in files.iter().enumerate() {
        assert!(recorded(&folder.home, file, format!("{{\"n\":{index}}}").as_bytes()), "{index}");
    }
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

