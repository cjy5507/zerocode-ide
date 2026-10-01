//! Chromium's signed copies of this app, and the stale ones its starts leave
//! behind (t-20243).
//!
//! At every start the embedded Chromium copies the whole app bundle into the
//! user's per-user `X` folder — `<bundle id>.code_sign_clone/
//! code_sign_clone.<random>/<name>.app.bundle`, every file an APFS clone and the
//! running executable a hard link — and nothing deletes the copy when the
//! window quits. The copy is not waste: the hard link is a second name for the
//! running executable, and it is what lets macOS go on recognising a window
//! whose installed bundle has been renamed away and deleted by an update
//! (`codesign --verify +pid` stays "dynamically valid"; with no second name it
//! answers "host has no guest with the requested attributes"). But an update
//! changes the blocks of most of those files, so each leftover copy keeps a
//! whole old version of the app on disk: 0.4 GiB per start, thirty-one starts
//! on the machine this was measured on (deleting thirty of them freed 12.3 GiB).
//!
//! macOS does not make them. A plain signed app launched by `open` or by path,
//! installed in place, by rename or quarantined, leaves nothing in that folder;
//! the engine's `MacAppCodeSignClone` feature does. The feature therefore stays
//! on (`chromium_browser::CEF_DISABLED_FEATURES` must not name it) and this
//! module deletes what the engine never does: once the window has painted, on
//! a thread of its own, this app's copies that no process runs and that are old
//! enough for nobody to be making them. One copy per running window stays, by
//! design. Another bundle id's copies are never looked at.
//!
//! Windows and Linux have no such folder, so none of this is compiled there.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// A bundle id's copies live in `<id>` plus this, inside the per-user folder.
const FOLDER_SUFFIX: &str = ".code_sign_clone";
/// Each copy is a directory in that folder, named like this plus random letters.
const CLONE_PREFIX: &str = "code_sign_clone.";
/// A copy younger than this stays whatever else is known.
const MIN_AGE: Duration = Duration::from_secs(5 * 60);

/// A file as the kernel names it for good: device and inode.
type Image = (u32, u64);

/// What one pass did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) removed: usize,
    pub(crate) removed_bytes: u64,
    pub(crate) in_use: usize,
    pub(crate) too_new: usize,
    pub(crate) postponed: usize,
    pub(crate) failed: usize,
}

impl Report {
    fn line(&self) -> String {
        String::new()
    }
}

/// What the live processes run.
#[derive(Debug, Default)]
struct Running {
    images: HashSet<Image>,
    paths: Vec<PathBuf>,
}

// Nothing is implemented yet: this is the state the tests below are written
// against, so a run of them shows each rule missing before it is added.

fn x_dir() -> Option<PathBuf> {
    None
}

fn running_processes() -> Option<Running> {
    None
}

fn sweep_under(
    _x_dir: &Path,
    _bundle_id: &str,
    _now: SystemTime,
    _deadline: Instant,
    _running: impl FnOnce() -> Option<Running>,
) -> Report {
    Report::default()
}

pub(crate) fn sweep_after_first_paint(_log_root: Option<PathBuf>) {}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const ID: &str = "dev.example.demo";

    /// A copy shaped like Chromium's, with one file of its own — a fresh inode
    /// no process runs.
    fn copy_in(x: &Path, id: &str, name: &str) -> PathBuf {
        let clone = x
            .join(format!("{id}{FOLDER_SUFFIX}"))
            .join(format!("{CLONE_PREFIX}{name}"));
        let macos = clone.join("Demo.app.bundle/Contents/MacOS");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::write(macos.join("demo"), name).unwrap();
        clone
    }

    fn image_of_file(path: &Path) -> Image {
        let meta = std::fs::metadata(path).unwrap();
        (meta.dev() as u32, meta.ino())
    }

    /// A moment far enough on that every copy made by now is old enough.
    fn later() -> SystemTime {
        SystemTime::now() + MIN_AGE + Duration::from_secs(60)
    }

    /// A deadline nobody reaches.
    fn unhurried() -> Instant {
        Instant::now() + Duration::from_secs(600)
    }

    fn nothing_runs() -> Option<Running> {
        Some(Running::default())
    }

    #[test]
    fn a_copy_of_ours_that_no_process_runs_and_is_old_enough_goes() {
        let x = tempfile::tempdir().unwrap();
        let stale = copy_in(x.path(), ID, "AAAAAA");
        let report = sweep_under(x.path(), ID, later(), unhurried(), nothing_runs);
        assert_eq!(
            report,
            Report {
                removed: 1,
                removed_bytes: 6,
                ..Report::default()
            }
        );
        assert!(!stale.exists());
    }

    #[test]
    fn what_a_removed_copy_held_is_the_sum_of_its_files() {
        let x = tempfile::tempdir().unwrap();
        let stale = copy_in(x.path(), ID, "BBBBBB");
        let inside = stale.join("Demo.app.bundle/Contents/Resources/deep");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::write(inside.join("one"), vec![1u8; 1000]).unwrap();
        std::fs::write(inside.join("two"), vec![2u8; 234]).unwrap();
        let report = sweep_under(x.path(), ID, later(), unhurried(), nothing_runs);
        // the copy's own `demo` file holds its six-letter name
        assert_eq!(report.removed, 1);
        assert_eq!(report.removed_bytes, 6 + 1000 + 234);
    }

    #[test]
    fn a_copy_whose_file_a_process_runs_stays_however_old() {
        let x = tempfile::tempdir().unwrap();
        let running = copy_in(x.path(), ID, "CCCCCC");
        let stale = copy_in(x.path(), ID, "DDDDDD");
        let held = image_of_file(&running.join("Demo.app.bundle/Contents/MacOS/demo"));
        let report = sweep_under(x.path(), ID, later(), unhurried(), || {
            Some(Running {
                images: HashSet::from([held]),
                ..Running::default()
            })
        });
        assert_eq!(
            report,
            Report {
                removed: 1,
                removed_bytes: 6,
                in_use: 1,
                ..Report::default()
            }
        );
        assert!(running.exists(), "the copy a process runs was deleted");
        assert!(!stale.exists());
    }

    #[test]
    fn a_copy_a_process_runs_from_by_path_stays_though_its_file_is_another_inode() {
        // A process that went in by the copy's own path, with a file of its own
        // rather than the hard link, is still running from it.
        let x = tempfile::tempdir().unwrap();
        let running = copy_in(x.path(), ID, "EEEEEE");
        let stale = copy_in(x.path(), ID, "FFFFFF");
        let by_path = std::fs::canonicalize(&running)
            .unwrap()
            .join("Demo.app.bundle/Contents/MacOS/demo");
        let report = sweep_under(x.path(), ID, later(), unhurried(), || {
            Some(Running {
                paths: vec![by_path],
                ..Running::default()
            })
        });
        assert_eq!(report.removed, 1);
        assert_eq!(report.in_use, 1);
        assert!(running.exists(), "the copy a process runs from was deleted");
        assert!(!stale.exists());
    }

    #[test]
    fn a_copy_younger_than_five_minutes_stays_even_when_nothing_runs_it() {
        let x = tempfile::tempdir().unwrap();
        let fresh = copy_in(x.path(), ID, "GGGGGG");
        let made = std::fs::metadata(&fresh).unwrap().modified().unwrap();
        let asked = Cell::new(0);
        let a_second_short = made + MIN_AGE - Duration::from_secs(1);
        let report = sweep_under(x.path(), ID, a_second_short, unhurried(), || {
            asked.set(asked.get() + 1);
            nothing_runs()
        });
        assert_eq!(
            report,
            Report {
                too_new: 1,
                ..Report::default()
            }
        );
        assert!(fresh.exists());
        assert_eq!(asked.get(), 0, "the process list was read for nothing");

        // The edge itself: exactly five minutes is old enough.
        let report = sweep_under(x.path(), ID, made + MIN_AGE, unhurried(), nothing_runs);
        assert_eq!(report.removed, 1);
        assert!(!fresh.exists());
    }

    #[test]
    fn another_bundle_ids_copies_and_the_nameless_folder_are_never_touched() {
        let x = tempfile::tempdir().unwrap();
        let ours = copy_in(x.path(), ID, "HHHHHH");
        let theirs = copy_in(x.path(), "com.example.other", "IIIIII");
        let longer = copy_in(x.path(), &format!("{ID}.helper"), "JJJJJJ");
        let nameless = copy_in(x.path(), "", "KKKKKK");
        let report = sweep_under(x.path(), ID, later(), unhurried(), nothing_runs);
        assert_eq!(report.removed, 1);
        assert!(!ours.exists());
        for kept in [&theirs, &longer, &nameless] {
            assert!(kept.exists(), "{} was swept", kept.display());
        }
    }

    #[test]
    fn only_directories_named_like_a_copy_are_taken_from_our_folder() {
        let x = tempfile::tempdir().unwrap();
        let stale = copy_in(x.path(), ID, "LLLLLL");
        let folder = stale.parent().unwrap().to_path_buf();
        let stranger = folder.join("keep-me");
        std::fs::create_dir(&stranger).unwrap();
        let file = folder.join(format!("{CLONE_PREFIX}file"));
        std::fs::write(&file, "not a directory").unwrap();
        let link = folder.join(format!("{CLONE_PREFIX}link"));
        std::os::unix::fs::symlink(&stranger, &link).unwrap();
        let report = sweep_under(x.path(), ID, later(), unhurried(), nothing_runs);
        assert_eq!(report.removed, 1);
        assert!(!stale.exists());
        assert!(stranger.exists() && file.exists());
        assert!(
            link.symlink_metadata().is_ok(),
            "the symlink was followed or removed"
        );
    }

    #[test]
    fn a_symlink_standing_where_our_folder_should_is_not_followed() {
        let x = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let behind = copy_in(elsewhere.path(), ID, "MMMMMM");
        std::os::unix::fs::symlink(
            behind.parent().unwrap(),
            x.path().join(format!("{ID}{FOLDER_SUFFIX}")),
        )
        .unwrap();
        let report = sweep_under(x.path(), ID, later(), unhurried(), nothing_runs);
        assert_eq!(report, Report::default());
        assert!(behind.exists(), "a copy behind a symlink was deleted");
    }

    #[test]
    fn an_id_that_could_name_another_path_sweeps_nothing() {
        let x = tempfile::tempdir().unwrap();
        let stale = copy_in(x.path(), ID, "NNNNNN");
        for id in [
            "",
            ".",
            "..",
            "a/b",
            "../x",
            "dev.example.demo/..",
            "with space",
            "nul\0",
        ] {
            assert_eq!(
                sweep_under(x.path(), id, later(), unhurried(), nothing_runs),
                Report::default(),
                "{id:?} swept"
            );
        }
        assert!(stale.exists());
    }

    #[test]
    fn when_the_process_list_cannot_be_read_nothing_is_removed() {
        let x = tempfile::tempdir().unwrap();
        let stale = copy_in(x.path(), ID, "OOOOOO");
        let report = sweep_under(x.path(), ID, later(), unhurried(), || None);
        assert_eq!(
            report,
            Report {
                in_use: 1,
                ..Report::default()
            }
        );
        assert!(stale.exists());
    }

    #[test]
    fn a_folder_that_is_not_there_is_a_clean_machine() {
        let x = tempfile::tempdir().unwrap();
        assert_eq!(
            sweep_under(x.path(), ID, later(), unhurried(), || panic!(
                "nothing to ask about"
            )),
            Report::default()
        );
    }

    #[test]
    fn a_sweep_past_its_time_budget_stops_and_leaves_the_rest_for_the_next_start() {
        let x = tempfile::tempdir().unwrap();
        let first = copy_in(x.path(), ID, "PPPPPP");
        let second = copy_in(x.path(), ID, "QQQQQQ");
        let report = sweep_under(x.path(), ID, later(), Instant::now(), nothing_runs);
        assert_eq!(
            report,
            Report {
                postponed: 2,
                ..Report::default()
            }
        );
        assert!(first.exists() && second.exists());
        // the next start, unhurried, finishes the job
        let report = sweep_under(x.path(), ID, later(), unhurried(), nothing_runs);
        assert_eq!(report.removed, 2);
        assert!(!first.exists() && !second.exists());
    }

    #[test]
    fn the_executable_this_process_runs_keeps_a_copy_that_hard_links_it() {
        // Chromium hard-links the running executable into its copy, so this is
        // the real shape: the copy's only file a process runs is another name
        // for the one it came in by. Beside the test binary, so the link
        // cannot cross a volume.
        let exe = std::env::current_exe().unwrap();
        let x = tempfile::tempdir_in(exe.parent().unwrap()).unwrap();
        let running = copy_in(x.path(), ID, "RRRRRR");
        std::fs::hard_link(exe, running.join("Demo.app.bundle/Contents/MacOS/linked")).unwrap();
        let stale = copy_in(x.path(), ID, "SSSSSS");
        let report = sweep_under(x.path(), ID, later(), unhurried(), running_processes);
        assert_eq!(report.removed, 1);
        assert_eq!(report.in_use, 1);
        assert!(
            running.exists(),
            "the copy this very process runs from was deleted"
        );
        assert!(!stale.exists());
    }

    #[test]
    fn the_processes_the_kernel_lists_include_this_one() {
        let exe = std::env::current_exe().unwrap();
        let running = running_processes().expect("the process list is readable");
        assert!(running.images.contains(&image_of_file(&exe)));
        assert!(!running.images.contains(&(0, 0)));
        // The paths are only counted, not compared: the kernel names a
        // hard-linked executable by the last name looked up, and a test beside
        // this one links this very binary.
        assert!(!running.paths.is_empty());
    }

    #[test]
    fn the_per_user_folder_is_the_x_beside_what_getconf_names_for_temp() {
        let named = crate::proc::quiet_command("/usr/bin/getconf")
            .arg("DARWIN_USER_TEMP_DIR")
            .output()
            .expect("getconf runs");
        assert!(named.status.success());
        let temp = String::from_utf8(named.stdout).unwrap();
        let expected = Path::new(temp.trim()).parent().unwrap().join("X");
        assert_eq!(x_dir(), Some(expected));
    }

    #[test]
    fn the_window_log_line_says_how_many_and_how_many_bytes_and_nothing_else() {
        let report = Report {
            removed: 3,
            removed_bytes: 1_234_567,
            in_use: 4,
            too_new: 5,
            postponed: 6,
            failed: 7,
        };
        assert_eq!(
            report.line(),
            "swept stale code-sign clones of this app: 3 removed, 1234567 bytes"
        );
    }

    #[test]
    fn the_shipped_half_spells_no_per_user_path_of_its_own() {
        let source = include_str!("code_sign_clone.rs");
        let shipped = source.split("#[cfg(test)]").next().unwrap();
        for literal in [concat!("/var/", "folders"), concat!("/private/", "var")] {
            assert!(
                !shipped.contains(literal),
                "the path is spelled out: {literal}"
            );
        }
    }

    /// What the worst case seen on a real machine costs: thirty-one copies of
    /// about five hundred files each, none of them run. Not part of the suite;
    /// `-- --ignored --nocapture` prints the time.
    #[test]
    #[ignore = "prints what sweeping thirty-one copies costs; run it by name"]
    fn the_cost_of_sweeping_thirty_one_copies_of_five_hundred_files() {
        let x = tempfile::tempdir().unwrap();
        for copy in 0..31 {
            let clone = x
                .path()
                .join(format!("{ID}{FOLDER_SUFFIX}"))
                .join(format!("{CLONE_PREFIX}{copy:06}"));
            for file in 0..526 {
                let dir = clone.join(format!("Demo.app.bundle/Contents/Resources/d{}", file % 20));
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join(format!("f{file}")), [7u8; 2048]).unwrap();
            }
        }
        let started = Instant::now();
        let report = sweep_under(x.path(), ID, later(), unhurried(), running_processes);
        eprintln!(
            "swept {} copies, {} bytes in {:?}",
            report.removed,
            report.removed_bytes,
            started.elapsed()
        );
        assert_eq!(report.removed, 31);
    }
}
