//! Chromium's signed copies of this app, and the stale ones its starts leave
//! behind (t-20243).
//!
//! At every start the embedded Chromium copies the whole app bundle into the
//! user's per-user `X` folder — `<bundle id>.code_sign_clone/
//! code_sign_clone.<random>/<name>.app.bundle`, every file an APFS clone and the
//! running executable a hard link. The copy is not waste: the hard link is a
//! second name for the running executable, and it is what lets macOS go on
//! recognising a window whose installed bundle has been renamed away and
//! deleted by an update (`codesign --verify +pid` stays "dynamically valid";
//! with no second name it answers "host has no guest with the requested
//! attributes").
//!
//! Chromium deletes its copy only after an orderly shutdown (a helper, started
//! then with `--type=code-sign-clone-cleanup`, removes it once the browser has
//! exited). The app's one restart road — where an update is swapped in — ends
//! in `exit(0)` without that shutdown, and so do a crash and a force quit; the
//! copy of such a window stays. An update changes the blocks of most of those
//! files, so each leftover copy keeps a whole old version of the app on disk:
//! 0.4 GiB per restart, thirty-one on the machine this was measured on
//! (deleting thirty of them freed 12.3 GiB).
//!
//! macOS does not make them. A plain signed app launched by `open` or by path,
//! installed in place, by rename or quarantined, leaves nothing in that folder;
//! the engine's `MacAppCodeSignClone` feature does. The feature therefore stays
//! on (`chromium_browser::CEF_DISABLED_FEATURES` must not name it) and this
//! module deletes what the engine's own cleanup missed: once the window has
//! painted, on a thread of its own, this app's copies that no process runs and
//! that are old enough for nobody to be making them. One copy per running
//! window stays, by design. Another bundle id's copies are never looked at.
//!
//! Windows and Linux have no such folder, so none of this is compiled there.

use std::collections::HashSet;
use std::ffi::{CStr, OsStr};
use std::io::ErrorKind;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

/// A bundle id's copies live in `<id>` plus this, inside the per-user folder.
const FOLDER_SUFFIX: &str = ".code_sign_clone";
/// Each copy is a directory in that folder, named like this plus random letters.
const CLONE_PREFIX: &str = "code_sign_clone.";
/// A copy younger than this stays whatever else is known: a second window of
/// ours may be making it this very minute.
const MIN_AGE: Duration = Duration::from_secs(5 * 60);
/// How long one sweep may run. It deletes between copies and checks the clock
/// there, so what is left past this waits for the next start; the sweep is on
/// the lowest priority the system has, so on a slow machine it may take its
/// time but never the person's.
const BUDGET: Duration = Duration::from_secs(10);
/// `proc_pidinfo`'s flavor for "the memory region at or after this address,
/// with the file behind it" (`PROC_PIDREGIONPATHINFO`, `<sys/proc_info.h>`).
/// libc carries the file half of that answer, not the flavor or the region half.
const PROC_PIDREGIONPATHINFO: libc::c_int = 8;
/// How many regions to look through, per process, for the first one a file
/// backs. The executable's own code is region zero on every process this was
/// measured on (659 of the 659 this user may read, t-20243); a few more cost
/// nothing and cover a process whose first region is not a file.
const REGIONS_LOOKED_AT: usize = 4;

/// A file as the kernel names it for good: device and inode. A hard-linked
/// executable answers to whichever of its paths was looked up last, so its
/// path says nothing about which copy a process came from; this does.
type Image = (u32, u64);

/// `struct proc_regionwithpathinfo` (`<sys/proc_info.h>`): the region's own
/// numbers, then the file behind it. The kernel fills all of it; only the
/// region's extent and the file's identity are read.
#[repr(C)]
struct RegionWithPath {
    /// `struct proc_regioninfo`'s first 80 bytes: protections, share modes and
    /// page counts nobody here reads.
    _unread: [u64; 10],
    address: u64,
    size: u64,
    vnode: libc::vnode_info_path,
}

// The kernel refuses a buffer smaller than its struct, so a layout slip would
// show as no process answering at all — and nothing ever deleted. Better the
// compile stops.
const _: () = assert!(size_of::<RegionWithPath>() == 1272);

/// What one pass did. The window's log says the first two and nothing else;
/// the rest is what the tests read.
#[derive(Debug, Default, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct Report {
    /// Copies deleted.
    pub(crate) removed: usize,
    /// What those copies held, in bytes (the sum of their files' lengths).
    pub(crate) removed_bytes: u64,
    /// Copies kept because a process runs a file in them — or because the
    /// process list could not be read, so none could be ruled out.
    pub(crate) in_use: usize,
    /// Copies kept because they are younger than [`MIN_AGE`].
    pub(crate) too_new: usize,
    /// Copies left for the next start because the time budget ran out.
    pub(crate) postponed: usize,
    /// Copies the file system would not let go of.
    pub(crate) failed: usize,
}

impl Report {
    /// The one line the window keeps: how many, how big, and no path.
    fn line(&self) -> String {
        format!(
            "swept stale code-sign clones of this app: {} removed, {} bytes",
            self.removed, self.removed_bytes
        )
    }
}

/// What the live processes run: the files their executables are, and the
/// paths those executables are at. A copy is in use if either names it.
#[derive(Debug, Default)]
struct Running {
    images: HashSet<Image>,
    paths: Vec<PathBuf>,
}

/// The per-user folder Chromium keeps its copies in: the `X` beside the
/// temporary folder `getconf DARWIN_USER_TEMP_DIR` names — that command is
/// this very `confstr` call. Not `$TMPDIR`: a window started with another one
/// (the CEF smoke does) still shares the folder.
fn x_dir() -> Option<PathBuf> {
    let mut buffer = [0 as libc::c_char; libc::PATH_MAX as usize];
    // SAFETY: `confstr` writes at most `len` bytes, the terminating NUL
    // included, and returns the length it needs, NUL included.
    let needed = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buffer.as_mut_ptr(),
            buffer.len(),
        )
    };
    if needed == 0 || needed > buffer.len() {
        return None;
    }
    // SAFETY: a call that fit its buffer left a NUL-terminated string in it.
    let temp = unsafe { CStr::from_ptr(buffer.as_ptr()) };
    Some(
        Path::new(OsStr::from_bytes(temp.to_bytes()))
            .parent()?
            .join("X"),
    )
}

/// The identifier of the bundle this process runs from, as Chromium names its
/// folder after it. `None` for a window with no bundle (a `cargo run` binary),
/// which has nothing of ours to sweep.
fn own_bundle_id() -> Option<String> {
    use objc2::runtime::{AnyClass, AnyObject};

    let class = AnyClass::get(c"NSBundle")?;
    // SAFETY: `mainBundle` is a class method returning an autoreleased
    // NSBundle and `bundleIdentifier` returns an NSString or nil; neither
    // throws, and both are safe to ask from any thread.
    unsafe {
        let bundle: *mut AnyObject = objc2::msg_send![class, mainBundle];
        if bundle.is_null() {
            return None;
        }
        let id: *mut AnyObject = objc2::msg_send![bundle, bundleIdentifier];
        if id.is_null() {
            return None;
        }
        Some((*(id as *const objc2_foundation::NSString)).to_string())
    }
}

/// A reverse-DNS name and nothing else. The id becomes part of a path that is
/// then emptied, so anything that could point elsewhere — empty (which is the
/// nameless folder, nobody's), a separator, `..` — sweeps nothing.
fn is_bundle_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 255
        && id != "."
        && id != ".."
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

/// The file behind a process's first file-backed memory region: its
/// executable. `None` for a process this user may not look into.
fn image_of(pid: libc::pid_t) -> Option<Image> {
    let size = libc::c_int::try_from(size_of::<RegionWithPath>()).ok()?;
    let mut address = 0u64;
    for _ in 0..REGIONS_LOOKED_AT {
        let mut region = std::mem::MaybeUninit::<RegionWithPath>::zeroed();
        // SAFETY: the buffer is exactly the size declared to the kernel, which
        // fills it whole or says it did not by returning less.
        let wrote = unsafe {
            libc::proc_pidinfo(
                pid,
                PROC_PIDREGIONPATHINFO,
                address,
                region.as_mut_ptr().cast(),
                size,
            )
        };
        if wrote != size {
            return None;
        }
        // SAFETY: zeroed plain integers, filled by the kernel, a full write
        // having been checked above.
        let region = unsafe { region.assume_init() };
        let file = &region.vnode.vip_vi.vi_stat;
        if file.vst_ino != 0 {
            return Some((file.vst_dev, file.vst_ino));
        }
        address = region.address.checked_add(region.size)?;
    }
    None
}

/// The path the kernel reports for a process's executable. It is the name
/// looked up last, so it can be the installed bundle's or a copy's; that is why
/// the file's identity is read too.
fn path_of(pid: libc::pid_t) -> Option<PathBuf> {
    let mut buffer = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let size = u32::try_from(buffer.len()).ok()?;
    // SAFETY: the buffer is exactly the size declared; `proc_pidpath` writes at
    // most that many bytes and returns how many, or 0 or less when it cannot.
    let wrote = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), size) };
    let length = usize::try_from(wrote).ok().filter(|length| *length > 0)?;
    Some(PathBuf::from(OsStr::from_bytes(&buffer[..length])))
}

/// Every pid there is. `None` when the kernel will not say.
fn list_pids() -> Option<Vec<libc::pid_t>> {
    // SAFETY: with no buffer `proc_listallpids` only counts.
    let counted = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let capacity = usize::try_from(counted).ok().filter(|count| *count > 0)?;
    // Processes start between the two calls: room for sixty-four more.
    let mut pids: Vec<libc::pid_t> = vec![0; capacity + 64];
    let bytes = libc::c_int::try_from(pids.len() * size_of::<libc::pid_t>()).ok()?;
    // SAFETY: the buffer holds exactly `bytes` bytes of pids.
    let listed = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    let listed = usize::try_from(listed).ok().filter(|count| *count > 0)?;
    pids.truncate(listed);
    pids.retain(|pid| *pid > 0);
    Some(pids)
}

/// What every live process runs. `None` when the process list cannot be read
/// at all, which the caller treats as "might be anything"; processes this user
/// may not look into (other users', the system's) simply add nothing.
fn running_processes() -> Option<Running> {
    let mut running = Running::default();
    for pid in list_pids()? {
        running.images.extend(image_of(pid));
        running.paths.extend(path_of(pid));
    }
    Some(running)
}

/// What looking through one copy found.
struct Inspected {
    /// A process runs a file in it.
    in_use: bool,
    /// The lengths of its files, added up (only meaningful when not in use).
    bytes: u64,
}

/// Look through the copy at `dir` against what runs. A process is in it when
/// its executable's path is under it, or when its executable is one of the
/// copy's files — by inode, so it does not matter which hard link of the file
/// the process went in by. Symlinks are never followed. `None` when the copy
/// cannot be named (it went while we looked).
fn inspect(dir: &Path, running: &Running) -> Option<Inspected> {
    let canonical = std::fs::canonicalize(dir).ok()?;
    if running
        .paths
        .iter()
        .any(|path| path.starts_with(&canonical))
    {
        return Some(Inspected {
            in_use: true,
            bytes: 0,
        });
    }
    let mut bytes = 0u64;
    let mut pending = vec![canonical];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                pending.push(entry.path());
            } else if meta.is_file() {
                if running.images.contains(&(meta.dev() as u32, meta.ino())) {
                    return Some(Inspected {
                        in_use: true,
                        bytes: 0,
                    });
                }
                bytes = bytes.saturating_add(meta.len());
            }
        }
    }
    Some(Inspected {
        in_use: false,
        bytes,
    })
}

/// Delete the copies of `bundle_id` under `x_dir` that nothing runs and that
/// were made at least [`MIN_AGE`] before `now`, until `deadline`. `running`
/// says what processes run; it is asked at most once, and only when some copy
/// is old enough to matter, so a clean machine pays for one directory read.
fn sweep_under(
    x_dir: &Path,
    bundle_id: &str,
    now: SystemTime,
    deadline: Instant,
    running: impl FnOnce() -> Option<Running>,
) -> Report {
    let mut report = Report::default();
    if !is_bundle_id(bundle_id) {
        return report;
    }
    let folder = x_dir.join(format!("{bundle_id}{FOLDER_SUFFIX}"));
    // Not followed: a symlink standing where the folder should be leads
    // somewhere that is not ours.
    if !std::fs::symlink_metadata(&folder).is_ok_and(|meta| meta.is_dir()) {
        return report;
    }
    let Ok(entries) = std::fs::read_dir(folder) else {
        return report;
    };
    let mut old = Vec::new();
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .as_bytes()
            .starts_with(CLONE_PREFIX.as_bytes())
        {
            continue;
        }
        // Not followed: a symlink named like a copy is not one.
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let age = meta
            .modified()
            .ok()
            .and_then(|made| now.duration_since(made).ok());
        match age {
            Some(age) if age >= MIN_AGE => old.push(entry.path()),
            _ => report.too_new += 1,
        }
    }
    if old.is_empty() {
        return report;
    }
    let Some(running) = running() else {
        report.in_use += old.len();
        return report;
    };
    let total = old.len();
    for (done, clone) in old.into_iter().enumerate() {
        if Instant::now() >= deadline {
            report.postponed = total - done;
            break;
        }
        match inspect(&clone, &running) {
            None => report.failed += 1,
            Some(Inspected { in_use: true, .. }) => report.in_use += 1,
            Some(Inspected { bytes, .. }) => match std::fs::remove_dir_all(clone) {
                Ok(()) => {
                    report.removed += 1;
                    report.removed_bytes = report.removed_bytes.saturating_add(bytes);
                }
                // Another window's sweep got there first: it is gone either way.
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(_) => report.failed += 1,
            },
        }
    }
    report
}

/// One pass over this app's own folder, within [`BUDGET`]. The window-log line
/// it returns is `None` when nothing was deleted.
fn sweep_now() -> Option<String> {
    let x_dir = x_dir()?;
    let bundle_id = own_bundle_id()?;
    let deadline = Instant::now() + BUDGET;
    let report = sweep_under(
        &x_dir,
        &bundle_id,
        SystemTime::now(),
        deadline,
        running_processes,
    );
    (report.removed > 0).then(|| report.line())
}

/// The window has painted: sweep, once, on a thread of its own at the lowest
/// priority the system has (CPU and disk), and say in the window log how many
/// copies went and how many bytes they held. Nothing the person is waiting on
/// waits for this.
pub(crate) fn sweep_after_first_paint(log_root: Option<PathBuf>) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("code-sign-clone-sweep".into())
        .spawn(move || {
            // SAFETY: lowers only the calling thread's own priority.
            let _ = unsafe { libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG) };
            if let (Some(root), Some(line)) = (log_root, sweep_now()) {
                crate::note_window_event(&root, &line);
            }
        });
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const ID: &str = "dev.example.demo";

    /// A copy shaped like Chromium's, with one file of its own — a fresh inode
    /// no process runs.
    /// A system program every Mac has, copied for a child to run — any
    /// Mach-O works; this one does nothing until it is killed.
    const SLEEPER: &str = "/bin/sleep";
    /// Longer than any sweep in these tests; the child is killed at the end.
    const SLEEPER_SECONDS: &str = "30";

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

    /// How long a child just spawned may take before the kernel reports the
    /// file it runs. A test that sweeps against a running process must not
    /// look before the process can be seen running its file: on a loaded Mac
    /// on battery (10-02 20:5x, a full suite beside other work) the sweep once
    /// looked first and removed the copy the child ran (`removed` 2, not 1).
    const CHILD_SHOWS_ITS_IMAGE_WITHIN: Duration = Duration::from_secs(10);

    /// Wait until the kernel reports `pid` running `file`, or fail saying so —
    /// the precondition of every sweep a running child should hold back.
    fn wait_until_running(pid: u32, file: &Path) {
        let pid = libc::pid_t::try_from(pid).unwrap();
        let wanted = image_of_file(file);
        let deadline = Instant::now() + CHILD_SHOWS_ITS_IMAGE_WITHIN;
        while image_of(pid) != Some(wanted) {
            assert!(
                Instant::now() < deadline,
                "the child {pid} was never seen running {} within {:?}: the sweep below would \
                 judge against a process the kernel does not show yet",
                file.display(),
                CHILD_SHOWS_ITS_IMAGE_WITHIN
            );
            std::thread::sleep(Duration::from_millis(10));
        }
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
    fn the_executable_a_process_runs_keeps_a_copy_that_hard_links_it() {
        // Chromium hard-links the running executable into its copy, so this is
        // the real shape: the copy's only file a process runs is another name
        // for the one it came in by. The process is a child running a copy of
        // a system program, never this test binary: the kernel names a
        // hard-linked executable by the last name looked up, so a link to the
        // test binary moved `current_exe()` into a folder this test deletes,
        // and every later test that starts the test binary as its helper
        // started a file that was gone (10-02: nine tests failed in full runs
        // only). One folder for the program and the copies, so the link
        // cannot cross a volume.
        let x = tempfile::tempdir().unwrap();
        let program = x.path().join("sleeper");
        std::fs::copy(SLEEPER, &program).unwrap();
        let running = copy_in(x.path(), ID, "RRRRRR");
        std::fs::hard_link(
            &program,
            running.join("Demo.app.bundle/Contents/MacOS/linked"),
        )
        .unwrap();
        let mut child = crate::proc::quiet_command(&program)
            .arg(SLEEPER_SECONDS)
            .spawn()
            .unwrap();
        wait_until_running(child.id(), &program);
        let stale = copy_in(x.path(), ID, "SSSSSS");
        let report = sweep_under(x.path(), ID, later(), unhurried(), running_processes);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(report.removed, 1);
        assert_eq!(report.in_use, 1);
        assert!(
            running.exists(),
            "the copy a running process's executable is linked into was deleted"
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
