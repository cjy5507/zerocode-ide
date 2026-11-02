//! The window's half of the stray reaper (t-89544): reads the table and the
//! folders, carries out what `zerocode_core::orchestration::stray_reap` judged, and
//! leaves one line in the window's log. No mail: nobody has to do anything about
//! it.
//!
//! One `ps` every five minutes. Everything else here runs only on a look that found
//! a suspect, which is almost never: the build line's files, `lsof`, the signals.
//!
//! Only on macOS, where `lsof` says where a process stands: elsewhere the places do
//! not read, a stray cannot be told from anybody else's process, and nothing is
//! ended, not even the `ps` is run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use zerocode_core::orchestration::Ledger;
use zerocode_core::orchestration::process_sweep::{self as sweep, Target};
use zerocode_core::orchestration::stray_reap::{
    self as reap, CpuProof, Holders, Place, Places, Row, Stray, Stronger, Watch,
};
use zerocode_hookd::KillWitness;

pub(crate) const LOOK_EVERY: Duration = Duration::from_secs(reap::LOOK_EVERY_SECS);

/// Where a process's place can be read at all, and so where a stray can be told.
const READS_PLACES: bool = cfg!(target_os = "macos");

/// The read: every process, one row each. One `ps`, stopped at the status bar's own
/// wall when a machine cannot answer.
const TABLE_WALL: Duration = Duration::from_secs(8);
const TABLE_COLUMNS: &str = "pid=,ppid=,pgid=,uid=,tty=,time=,etime=,pcpu=,lstart=,command=";
/// The columns before `lstart`: `parse_columns` reads this many.
const NUMBER_COLUMNS: usize = 8;

/// The build line's runner, spelled as it spells them.
const QUEUE_RUN: &str = "run";
const QUEUE_FLAGS_STANDING_DOWN: [&str; 2] = ["PAUSE", "QUIET_WANTED"];
const TURN_LOCK_PREFIX: &str = "cargo-turn-";
const TURN_LOCK_SUFFIX: &str = ".lock";
const PID_FILE: &str = "pid";
const LANE_LOCK: &str = "lock";

/// The folders the Claude Code CLI gives one session's scratch work, under its temp
/// root: `<project folder>/<session id>/scratchpad`.
const SCRATCH_FOLDER: &str = "scratchpad";
const SCRATCH_DEPTH: usize = 3;

static WATCH: LazyLock<Mutex<Watch>> = LazyLock::new(|| Mutex::new(Watch::default()));
static CLOCK: LazyLock<Instant> = LazyLock::new(Instant::now);

fn now_secs() -> u64 {
    CLOCK.elapsed().as_secs()
}

/// Look, but not on the beat that has to draw. One look at a time: the signals'
/// wait is several seconds, and a look that outruns five minutes is a machine this
/// must not add to.
pub(crate) fn look_off_the_beat(log_root: &Path) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static LOOKING: AtomicBool = AtomicBool::new(false);

    struct Looking;
    impl Drop for Looking {
        fn drop(&mut self) {
            LOOKING.store(false, Ordering::Release);
        }
    }

    if !READS_PLACES || LOOKING.swap(true, Ordering::AcqRel) {
        return;
    }
    let log_root = log_root.to_path_buf();
    if std::thread::Builder::new()
        .name("stray-reap".into())
        .spawn(move || {
            let _clear = Looking;
            if let Some(line) = look() {
                crate::note_window_event(&log_root, &line);
            }
        })
        .is_err()
    {
        LOOKING.store(false, Ordering::Release);
    }
}

/// One look: `Some(line)` when anything was signalled.
fn look() -> Option<String> {
    let ledger = crate::orchestration::ledger_image()?;
    let rows = read_rows().ok()?;
    let (rows, strays) = judged(rows, &ledger, CpuProof::Watched);
    if strays.is_empty() {
        return None;
    }
    let reaped = reap::end(&strays, &rows, &crate::process_sweep::System::new(), &Kill);
    reaped.did_anything().then(|| reap::line(&reaped))
}

/// What the judgment names for a coordinator that read a list: the targets
/// `process-sweep` may end although nobody has watched them for ten minutes yet.
pub(crate) fn named_by_coordinator() -> Vec<Target> {
    if !READS_PLACES {
        return Vec::new();
    }
    let Some(ledger) = crate::orchestration::ledger_image() else {
        return Vec::new();
    };
    let Ok(rows) = read_rows() else {
        return Vec::new();
    };
    let (_, strays) = judged(rows, &ledger, CpuProof::NamedByCoordinator);
    strays.into_iter().map(|stray| stray.target).collect()
}

/// The judgment over one read table: the watch is told, the suspects are found with
/// nothing but the table, and only then does anything else get read.
fn judged(mut rows: Vec<Row>, ledger: &Ledger, proof: CpuProof) -> (Vec<Row>, Vec<Stray>) {
    let now = now_secs();
    let mut watch = WATCH.lock().unwrap_or_else(PoisonError::into_inner);
    watch.observe(&rows, now);
    let bare = Holders {
        uid: crate::pane_leftovers::own_user(),
        window: std::process::id(),
        ..Holders::default()
    };
    if reap::suspects(&rows, &bare, &watch, now, proof).is_empty() {
        return (rows, Vec::new());
    }
    let holders = holders_read(bare, &rows, &line_parents(ledger));
    let families = reap::suspects(&rows, &holders, &watch, now, proof);
    drop(watch);
    if families.is_empty() {
        return (rows, Vec::new());
    }
    let pids: Vec<u32> = families.iter().flatten().copied().collect();
    fold_places(&mut rows, &crate::system_runtime::process_cwds(&pids));
    let places = KnownPlaces::of(ledger);
    let strays = reap::strays_of(&families, &rows, &holders, &places);
    (rows, strays)
}

fn fold_places(rows: &mut [Row], cwds: &HashMap<u32, String>) {
    for row in rows {
        if let Some(cwd) = cwds.get(&row.pid) {
            row.cwd = Some(cwd.clone());
        }
    }
}

fn read_rows() -> Result<Vec<Row>, String> {
    let mut command = crate::proc::quiet_command("ps");
    command
        .args(["-ww", "-eo", TABLE_COLUMNS])
        .env("LC_ALL", "C")
        .env("LANG", "C");
    let output = crate::resource_usage::run_sampler(command, TABLE_WALL)?;
    let rows = parse_rows(&String::from_utf8_lossy(&output.stdout));
    if rows.is_empty() {
        return Err("the process table read empty".to_string());
    }
    Ok(rows)
}

/// The rows of [`TABLE_COLUMNS`], the start and command line in the same words and
/// bounds every reader of the table keys a process by, so a `pid@mark` made here is
/// the one a list shows. A row that does not read is left out, and a process left
/// out is a process not touched.
fn parse_rows(listing: &str) -> Vec<Row> {
    listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_ascii_whitespace();
            let numbers: Vec<&str> = fields.by_ref().take(NUMBER_COLUMNS).collect();
            let columns = reap::parse_columns(&numbers)?;
            let (started, args) = crate::resource_usage::start_and_command(fields)?;
            Some(Row {
                pid: columns.pid,
                ppid: columns.ppid,
                pgid: columns.pgid,
                uid: columns.uid,
                tty: columns.tty,
                cpu_cs: columns.cpu_cs,
                cpu_idle_now: columns.cpu_idle_now,
                age_secs: columns.age_secs,
                started,
                args,
                cwd: None,
            })
        })
        .collect()
}

/// The folders a build line can stand under: `<workspaces>/build-line`, beside the
/// checkouts the ledger's workers were given.
fn line_parents(ledger: &Ledger) -> Vec<PathBuf> {
    let mut parents: Vec<PathBuf> = sweep::checkouts(ledger)
        .iter()
        .filter_map(|checkout| Path::new(&checkout.path).parent().map(Path::to_path_buf))
        .collect();
    parents.sort();
    parents.dedup();
    parents
}

/// What the window may not touch, as this look reads it. Unreadable files add
/// nothing here, because each is only ever a reason to hold back and the group
/// rule beside it still holds; a running lane or a measuring build line stands the
/// whole look down.
fn holders_read(mut holders: Holders, rows: &[Row], workspaces: &[PathBuf]) -> Holders {
    let roots = crate::kill_witness::PaneProcesses.roots("");
    holders.panes = roots.protected.iter().map(|root| root.pid).collect();
    holders.panes.extend(roots.caller);
    holders.stand_down = crate::update_runtime::release_dir()
        .and_then(|dir| read_pid(&dir.join(LANE_LOCK).join(PID_FILE)))
        .is_some_and(|lane| rows.iter().any(|row| row.pid == lane));
    for workspace in workspaces {
        read_line(&mut holders, rows, workspace);
    }
    holders
}

fn read_line(holders: &mut Holders, rows: &[Row], workspace: &Path) {
    let line = workspace.join(crate::schedule_timeline::LINE_FOLDER[0]);
    let queue = line.join(crate::schedule_timeline::LINE_FOLDER[1]);
    if QUEUE_FLAGS_STANDING_DOWN
        .iter()
        .any(|flag| queue.join(flag).exists())
    {
        holders.stand_down = true;
    }
    let run = queue.join(QUEUE_RUN);
    for entry in read_dir_paths(&run) {
        match entry.extension().and_then(|ext| ext.to_str()) {
            Some("pid") => holders.recorded.extend(read_pid(&entry)),
            Some("job") => {
                if let Some(folder) = std::fs::read_to_string(&entry)
                    .ok()
                    .as_deref()
                    .and_then(reap::job_folder)
                {
                    holders.busy_folders.push(folder);
                }
            }
            _ => {}
        }
    }
    // A job's shell names its file in its command line; the runner writes no pid.
    let spelled = format!("{}/", run.display());
    holders.recorded.extend(
        rows.iter()
            .filter(|row| row.args.contains(&spelled))
            .map(|row| row.pid),
    );
    for entry in read_dir_paths(&line) {
        let is_turn = entry
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(TURN_LOCK_PREFIX) && name.ends_with(TURN_LOCK_SUFFIX)
            });
        if is_turn {
            holders.recorded.extend(read_pid(&entry.join(PID_FILE)));
        }
    }
}

fn read_dir_paths(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir).map_or_else(
        |_| Vec::new(),
        |entries| entries.flatten().map(|entry| entry.path()).collect(),
    )
}

fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// The checkouts of the repositories the ledger holds runs on, and this user's
/// agent scratch folders. A folder under anything else is nobody's this can name.
struct KnownPlaces {
    /// Each repository's git common directory, as the file system spells it.
    repositories: Vec<PathBuf>,
    temp: Option<PathBuf>,
}

impl KnownPlaces {
    fn of(ledger: &Ledger) -> Self {
        Self {
            repositories: sweep::project_roots(ledger)
                .iter()
                .filter_map(|root| Path::new(root).canonicalize().ok())
                .collect(),
            temp: crate::session_scratch::claude_temp_root(),
        }
    }

    /// The repository a `.git` entry belongs to: a folder is its own, and a linked
    /// worktree's file says `gitdir: <common>/worktrees/<name>`.
    fn common_dir(dot_git: &Path) -> Option<PathBuf> {
        let held = std::fs::symlink_metadata(dot_git).ok()?;
        if held.is_dir() {
            return dot_git.canonicalize().ok();
        }
        if !held.is_file() {
            return None;
        }
        let text = std::fs::read_to_string(dot_git).ok()?;
        let told = Path::new(text.strip_prefix("gitdir:")?.trim());
        let worktrees = told.parent()?;
        (worktrees.file_name()? == "worktrees")
            .then(|| worktrees.parent()?.canonicalize().ok())
            .flatten()
    }
}

impl Places for KnownPlaces {
    fn place_of(&self, path: &str) -> Option<Place> {
        let standing = Path::new(path);
        if !standing.is_absolute() {
            return None;
        }
        // The nearest `.git` decides: a checkout of another repository inside a
        // folder is that repository's, whatever holds it.
        for folder in standing.ancestors() {
            let dot_git = folder.join(".git");
            if std::fs::symlink_metadata(&dot_git).is_ok() {
                let common = Self::common_dir(&dot_git)?;
                return self
                    .repositories
                    .contains(&common)
                    .then(|| Place::Checkout(folder.to_path_buf()));
            }
        }
        let temp = self.temp.as_deref()?;
        scratch_top(standing, temp).map(Place::Scratch)
    }
}

/// `<temp>/<project folder>/<session id>/scratchpad`, when `path` stands under one.
fn scratch_top(path: &Path, temp: &Path) -> Option<PathBuf> {
    let inside = path.strip_prefix(temp).ok()?;
    let parts: Vec<_> = inside.iter().take(SCRATCH_DEPTH).collect();
    (parts.len() == SCRATCH_DEPTH && parts[SCRATCH_DEPTH - 1] == SCRATCH_FOLDER)
        .then(|| temp.join(parts.iter().collect::<PathBuf>()))
}

struct Kill;

impl Stronger for Kill {
    fn kill(&self, pid: u32) -> Result<(), String> {
        crate::resource_usage::force_one_process(pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As `ps -ww -eo <TABLE_COLUMNS>` prints it under LC_ALL=C on macOS.
    const LISTING: &str = "\
          501 1 501 501 ??      0:42.70 17-03:42:23   0.0 Mon Oct 16 12:10:05 2026     node server/src/main.mjs\n\
        70414 1 70414 501 ??      0:00.15 05:02:11   0.0 Mon Nov  2 10:32:00 2026     /c/ms-playwright/chromium_headless_shell-1248/mac/chrome-headless-shell --headless\n\
        70481 70414 70414 501 ttys012      0:00.05 45:00   1.5 Mon Nov  2 10:32:01 2026     /c/ms-playwright/x/chrome-headless-shell --type=gpu-process\n\
        not a row at all\n";

    #[test]
    fn a_listing_reads_in_the_words_a_mark_is_made_of() {
        let rows = parse_rows(LISTING);
        assert_eq!(rows.len(), 3, "the broken row is left out");
        let chrome = &rows[1];
        assert_eq!(chrome.pid, 70414);
        assert_eq!(chrome.ppid, 1);
        assert_eq!(chrome.tty, None);
        assert_eq!(chrome.cpu_cs, Some(15));
        assert_eq!(chrome.age_secs, Some(5 * 3600 + 2 * 60 + 11));
        assert!(chrome.cpu_idle_now);
        // One space between lstart's words, whatever `ps` padded them with: the
        // reader the end road compares against writes them this way.
        assert_eq!(chrome.started, "Mon Nov 2 10:32:00 2026");
        let helper = &rows[2];
        assert_eq!(helper.tty.as_deref(), Some("ttys012"));
        assert_eq!(helper.age_secs, Some(45 * 60));
        assert!(!helper.cpu_idle_now);
    }

    /// The read itself, against this machine: only that it parses, and that the
    /// numbers the judgment depends on arrive. Nothing is signalled.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_real_table_reads_with_every_column_the_judgment_needs() {
        let rows = read_rows().expect("the table");
        let own = rows
            .iter()
            .find(|row| row.pid == std::process::id())
            .expect("this process is in its own table");
        assert!(own.cpu_cs.is_some(), "{own:?}");
        assert!(own.age_secs.is_some(), "{own:?}");
        assert_eq!(own.uid, crate::pane_leftovers::own_user());
    }

    /// The start this table reads is the one the end road reads right before its
    /// signal: if they differed, every stray would read as a reused pid and
    /// nothing would ever be ended.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_tables_identity_is_the_one_the_end_road_reads_again() {
        let rows = read_rows().expect("the table");
        let own = rows
            .iter()
            .find(|row| row.pid == std::process::id())
            .expect("this process is in its own table");
        let again = crate::resource_usage::process_identities(&[own.pid]).expect("the read");
        let (started, args) = again.get(&own.pid).expect("this process stands");
        assert_eq!(
            sweep::mark(own.pid, &own.started, &own.args),
            sweep::mark(own.pid, started, args)
        );
    }

    fn git_file(checkout: &Path, common: &Path, name: &str) {
        std::fs::create_dir_all(checkout).expect("checkout");
        std::fs::write(
            checkout.join(".git"),
            format!("gitdir: {}/worktrees/{name}\n", common.display()),
        )
        .expect("git file");
    }

    #[test]
    fn a_checkout_counts_only_when_its_repository_is_the_ledgers() {
        let scene = tempfile::tempdir().expect("scene");
        let root = scene.path().canonicalize().expect("real path");
        let common = root.join("main/.git");
        std::fs::create_dir_all(&common).expect("main");
        git_file(&root.join("wt-1"), &common, "wt-1");
        let other_common = root.join("other/.git");
        std::fs::create_dir_all(&other_common).expect("other");
        git_file(&root.join("wt-2"), &other_common, "wt-2");
        std::fs::create_dir_all(root.join("wt-1/ui/tests")).expect("folders");
        let places = KnownPlaces {
            repositories: vec![common.clone()],
            temp: Some(root.join("tmp")),
        };
        let say = |path: &Path| places.place_of(path.to_str().expect("utf-8"));
        assert_eq!(
            say(&root.join("wt-1/ui/tests")),
            Some(Place::Checkout(root.join("wt-1")))
        );
        assert_eq!(
            say(&root.join("main")),
            Some(Place::Checkout(root.join("main")))
        );
        assert_eq!(say(&root.join("wt-2")), None, "another repository's");
        assert_eq!(say(&root.join("nowhere")), None, "no .git above it");
        assert_eq!(places.place_of("relative/path"), None);
    }

    #[test]
    fn a_scratch_folder_is_a_sessions_own_and_nothing_else_under_the_temp_root() {
        let temp = Path::new("/private/tmp/claude-501");
        assert_eq!(
            scratch_top(&temp.join("-project/s-1/scratchpad/copy/ui"), temp),
            Some(temp.join("-project/s-1/scratchpad"))
        );
        assert_eq!(
            scratch_top(&temp.join("-project/s-1/scratchpad"), temp),
            Some(temp.join("-project/s-1/scratchpad"))
        );
        assert_eq!(scratch_top(&temp.join("-project/s-1/tasks"), temp), None);
        assert_eq!(scratch_top(&temp.join("-project"), temp), None);
        assert_eq!(
            scratch_top(Path::new("/private/tmp/x/scratchpad"), temp),
            None
        );
    }

    fn scene_line(root: &Path) -> PathBuf {
        let queue = root.join("build-line/build-queue");
        std::fs::create_dir_all(queue.join("run")).expect("queue");
        queue
    }

    fn row(pid: u32, args: &str) -> Row {
        Row {
            pid,
            ppid: 1,
            pgid: pid,
            uid: 501,
            tty: None,
            cpu_cs: Some(0),
            cpu_idle_now: true,
            age_secs: Some(10_000),
            started: "s".to_string(),
            args: args.to_string(),
            cwd: None,
        }
    }

    /// A running job, its turn's lock and a measurement's flag, read off the disk.
    #[test]
    fn the_build_lines_records_are_read_into_the_holders() {
        let scene = tempfile::tempdir().expect("scene");
        let root = scene.path();
        let queue = scene_line(root);
        std::fs::write(queue.join("run/a.pid"), "4242\n").expect("pid");
        std::fs::write(queue.join("run/b.pid"), "not a pid\n").expect("pid");
        std::fs::write(queue.join("run/c.job"), "# who: w-1\n# cwd: /w/t-1\necho\n").expect("job");
        let lock = root.join("build-line/cargo-turn-run-1.lock");
        std::fs::create_dir_all(&lock).expect("lock");
        std::fs::write(lock.join("pid"), "4343\n").expect("pid");
        let job = format!("zsh {}/run/c.job", queue.display());
        let rows = vec![row(4444, &job), row(4545, "zsh something else")];
        let mut holders = Holders::default();
        read_line(&mut holders, &rows, root);
        holders.recorded.sort_unstable();
        assert_eq!(holders.recorded, vec![4242, 4343, 4444]);
        assert_eq!(holders.busy_folders, vec!["/w/t-1".to_string()]);
        assert!(!holders.stand_down);

        std::fs::write(queue.join("QUIET_WANTED"), "").expect("flag");
        read_line(&mut holders, &rows, root);
        assert!(holders.stand_down, "a measuring line stands the look down");
    }

    /// A process this test started and left under pid 1, ended by the real signal
    /// road and read back. No other process is signalled: the only pid named is the
    /// one this test spawned, with the mark read from the table.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_synthetic_orphan_is_ended_through_the_real_signals_and_its_mark() {
        use std::process::{Command, Stdio};
        let scene = tempfile::tempdir().expect("scene");
        let pid_file = scene.path().join("pid");
        // The shell forks a sleep and leaves at once, so the sleep hangs under pid 1.
        let spawned = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "sleep 120 >/dev/null 2>&1 </dev/null & echo $! > {}",
                pid_file.display()
            ))
            .stdin(Stdio::null())
            .status()
            .expect("the shell");
        assert!(spawned.success());
        let pid = read_pid(&pid_file).expect("the orphan's pid");
        let deadline = Instant::now() + Duration::from_secs(5);
        let orphan = loop {
            let rows = read_rows().expect("the table");
            if let Some(found) = rows.iter().find(|row| row.pid == pid && row.ppid == 1) {
                break found.clone();
            }
            assert!(
                Instant::now() < deadline,
                "the sleep never hung under pid 1"
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        let stray = Stray {
            pid,
            target: Target {
                pid,
                mark: sweep::mark(pid, &orphan.started, &orphan.args),
            },
            said: "sleep".to_string(),
        };

        // A mark from before a reuse: read again, it differs, and no signal goes out.
        let stale = Stray {
            target: Target {
                pid,
                mark: "000000000000".to_string(),
            },
            ..stray.clone()
        };
        let mut stale_row = orphan.clone();
        stale_row.args = "another process".to_string();
        let reaped = reap::end(
            &[stale],
            std::slice::from_ref(&stale_row),
            &crate::process_sweep::System::new(),
            &Kill,
        );
        assert_eq!(reaped.changed, 1);
        assert!(
            crate::resource_usage::process_identities(&[pid])
                .expect("the read")
                .contains_key(&pid),
            "a changed mark sends nothing"
        );

        let reaped = reap::end(
            &[stray],
            std::slice::from_ref(&orphan),
            &crate::process_sweep::System::new(),
            &Kill,
        );
        assert_eq!(reaped.by_term, 1, "{reaped:?}");
        assert_eq!(reaped.left, 0, "{reaped:?}");
    }
}
