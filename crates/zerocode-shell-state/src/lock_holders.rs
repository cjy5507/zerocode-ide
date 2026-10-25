//! Who holds a lock file, and whether the boot may end that holder.
//!
//! Measured 2026-10-25 04:01–04:06 (t-73907): a window forked a child to
//! launch a pty, the child stood forever in a libSystem fork handler before
//! its exec, and it kept a copy of every descriptor the window had open —
//! among them `.settings.lock`, taken at that moment. The window was killed,
//! launchd adopted the child, and three new windows stood in `flock` inside
//! `build_app_state` behind it until the person killed it by hand.
//!
//! [`settle`] is the boot's way out: wait a bounded time, then name every
//! process with the lock file open, end only a holder that can be nothing
//! but such a child, and say why for every other one. [`judge`] is the whole
//! policy and reads nothing but its arguments.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::settings::{SettingsError, SettingsRepository};

/// What the boot knows about one process that has the lock file open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub pid: i32,
    pub uid: u32,
    /// `None` when its parent could not be read.
    pub parent: Option<i32>,
    /// `None` when its thread count could not be read.
    pub threads: Option<u32>,
    /// `None` when its executable could not be read.
    pub executable: Option<PathBuf>,
    /// When it began, since the epoch; `None` when it could not be read. A
    /// pid names one process only while that process lives, so this is what
    /// the pid is checked against again just before it is ended.
    pub started: Option<Duration>,
}

/// The process asking: whoever runs [`settle`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asker {
    pub pid: i32,
    pub uid: u32,
    pub executable: PathBuf,
    /// The running windows this process knows by pid. A window is never
    /// ended, whatever else it looks like.
    pub windows: Vec<i32>,
}

impl Asker {
    /// This process, knowing no window but itself: the single-instance lock
    /// keeps a profile to one window, and a window asking is that one.
    pub fn this_process() -> std::io::Result<Self> {
        let pid = i32::try_from(std::process::id()).unwrap_or(i32::MAX);
        Ok(Self {
            pid,
            uid: current_uid(),
            executable: std::env::current_exe()?,
            windows: vec![pid],
        })
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

/// The pid launchd runs as; a process whose parent died is adopted by it.
pub const LAUNCHD: i32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A child of this same program that its parent left behind with one
    /// thread: a fork that never reached its exec. Ended.
    EndStuckLaunch,
    /// Anything else. Left running, with the reason said.
    Leave(LeaveReason),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaveReason {
    ThisProcess,
    KnownWindow,
    OtherUser,
    OtherProgram,
    HasParent(i32),
    Threads(u32),
    Unreadable(&'static str),
}

/// Whether `holder` is a launch of this program that never reached its exec.
///
/// Each condition is a fact a forked-but-not-exec'd child of a window has and
/// a running window does not: the window's own executable (a child that had
/// exec'd would be some other program), launchd as parent (the window that
/// forked it is gone), and one thread (fork copies only the calling thread;
/// every window runs dozens). Anything unread counts against ending it.
#[must_use]
pub fn judge(holder: &Holder, asker: &Asker) -> Verdict {
    if holder.pid == asker.pid {
        return Verdict::Leave(LeaveReason::ThisProcess);
    }
    if asker.windows.contains(&holder.pid) {
        return Verdict::Leave(LeaveReason::KnownWindow);
    }
    if holder.uid != asker.uid {
        return Verdict::Leave(LeaveReason::OtherUser);
    }
    match &holder.executable {
        None => return Verdict::Leave(LeaveReason::Unreadable("executable")),
        Some(executable) if !same_file(executable, &asker.executable) => {
            return Verdict::Leave(LeaveReason::OtherProgram);
        }
        Some(_) => {}
    }
    match holder.parent {
        None => return Verdict::Leave(LeaveReason::Unreadable("parent")),
        Some(LAUNCHD) => {}
        Some(parent) => return Verdict::Leave(LeaveReason::HasParent(parent)),
    }
    match holder.threads {
        None => return Verdict::Leave(LeaveReason::Unreadable("threads")),
        Some(1) => {}
        Some(threads) => return Verdict::Leave(LeaveReason::Threads(threads)),
    }
    // Without its start, the pid cannot be checked again before the kill.
    match holder.started {
        None => Verdict::Leave(LeaveReason::Unreadable("start")),
        Some(_) => Verdict::EndStuckLaunch,
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (left.canonicalize(), right.canonicalize()),
            (Ok(left), Ok(right)) if left == right
        )
}

/// What [`settle`] found and did, one line per fact, for the window log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settled {
    /// Whether the lock came free before `settle` returned.
    pub free: bool,
    pub ended: Vec<i32>,
    /// Every holder that was left running, with why.
    pub left: Vec<(i32, LeaveReason)>,
    /// What could not be done: listing holders, or ending one.
    pub failures: Vec<String>,
    pub waited: Duration,
}

impl Settled {
    /// One line for the window log and the person; `None` when the lock was
    /// free at the first ask and there is nothing to say.
    #[must_use]
    pub fn line(&self, lock: &Path) -> Option<String> {
        if self.free && self.ended.is_empty() && self.failures.is_empty() {
            return None;
        }
        let mut line = format!(
            "settings lock {}: {} after {} ms",
            lock.display(),
            if self.free { "free" } else { "still held" },
            self.waited.as_millis()
        );
        for pid in &self.ended {
            line.push_str(&format!(
                "; ended pid {pid} (a launch of this program left with one thread under launchd)"
            ));
        }
        for (pid, reason) in &self.left {
            line.push_str(&format!("; left pid {pid} running ({})", reason.said()));
        }
        for failure in &self.failures {
            line.push_str(&format!("; {failure}"));
        }
        Some(line)
    }
}

impl LeaveReason {
    fn said(&self) -> String {
        match self {
            Self::ThisProcess => "this window".into(),
            Self::KnownWindow => "a running window".into(),
            Self::OtherUser => "another user's process".into(),
            Self::OtherProgram => "another program".into(),
            Self::HasParent(parent) => format!("its parent pid {parent} is alive"),
            Self::Threads(threads) => format!("{threads} threads"),
            Self::Unreadable(what) => format!("its {what} could not be read"),
        }
    }
}

/// Wait up to `budget` for the repository lock. If it does not come free,
/// end every holder [`judge`] says is a stuck launch and wait `budget` once
/// more. Never waits longer than twice `budget` plus the ending itself.
pub fn settle(
    repository: &SettingsRepository,
    budget: Duration,
    asker: &Asker,
) -> Result<Settled, SettingsError> {
    let started = Instant::now();
    let mut settled = Settled::default();
    if repository.lock_free_within(budget)? {
        settled.free = true;
        settled.waited = started.elapsed();
        return Ok(settled);
    }
    let holders = match holders_of(&repository.lock_path()) {
        Ok(holders) => holders,
        Err(error) => {
            settled
                .failures
                .push(format!("could not list the lock's holders: {error}"));
            Vec::new()
        }
    };
    for holder in &holders {
        match judge(holder, asker) {
            Verdict::EndStuckLaunch => match end(holder) {
                Ok(()) => settled.ended.push(holder.pid),
                Err(error) => settled
                    .failures
                    .push(format!("could not end pid {}: {error}", holder.pid)),
            },
            Verdict::Leave(reason) => settled.left.push((holder.pid, reason)),
        }
    }
    settled.free = !settled.ended.is_empty() && repository.lock_free_within(budget)?;
    settled.waited = started.elapsed();
    Ok(settled)
}

/// How long [`end`] waits for an ended process to be gone.
#[cfg(target_os = "macos")]
const END_WAIT: Duration = Duration::from_secs(1);

/// End the process [`judge`] judged, if `holder.pid` still names it.
///
/// Between the reading and the kill that process can end and its pid go to
/// another, so the pid's start is read again just before the signal and must
/// be the one the holder was judged with. What is left between that read and
/// the kill is one system call: macOS has no descriptor that names a process
/// rather than a pid.
#[cfg(target_os = "macos")]
fn end(holder: &Holder) -> std::io::Result<()> {
    let now = macos::bsd_info(holder.pid).map(|bsd| macos::started_of(&bsd));
    if holder.started.is_none() || now != holder.started {
        return Err(std::io::Error::other(format!(
            "pid {} is no longer the process that was judged",
            holder.pid
        )));
    }
    kill_and_wait(holder.pid)
}

#[cfg(target_os = "macos")]
fn kill_and_wait(pid: i32) -> std::io::Result<()> {
    // SIGKILL: a process standing in a mutex inside a fork handler runs no
    // signal handler that a gentler signal would be asking.
    if unsafe { libc::kill(pid, libc::SIGKILL) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // Its descriptors close when the kernel tears it down, a moment after the
    // signal; the caller's next ask waits for that anyway, this only keeps the
    // log from saying "ended" of a process still on its way out.
    let until = Instant::now() + END_WAIT;
    while Instant::now() < until {
        if unsafe { libc::kill(pid, 0) } != 0 {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// Only macOS lists a lock's holders ([`holders_of`]), so only there is one
/// ever judged and ended.
#[cfg(not(target_os = "macos"))]
fn end(_holder: &Holder) -> std::io::Result<()> {
    Err(std::io::Error::other("ending a holder is macOS-only"))
}

/// Every process of this user that has `path` open, matched by device and
/// inode rather than by name, so a link or a second spelling still matches.
#[cfg(target_os = "macos")]
pub fn holders_of(path: &Path) -> std::io::Result<Vec<Holder>> {
    use std::os::unix::fs::MetadataExt as _;

    let target = std::fs::metadata(path)?;
    let me = unsafe { libc::getuid() };
    let mut holders = Vec::new();
    for pid in macos::all_pids()? {
        let Some(bsd) = macos::bsd_info(pid) else {
            continue;
        };
        if bsd.pbi_uid != me {
            continue;
        }
        if !macos::has_open(pid, target.dev(), target.ino()) {
            continue;
        }
        holders.push(Holder {
            pid,
            uid: bsd.pbi_uid,
            parent: i32::try_from(bsd.pbi_ppid).ok(),
            threads: macos::threads(pid),
            executable: macos::executable(pid),
            started: Some(macos::started_of(&bsd)),
        });
    }
    Ok(holders)
}

#[cfg(not(target_os = "macos"))]
pub fn holders_of(_path: &Path) -> std::io::Result<Vec<Holder>> {
    Err(std::io::Error::other(
        "listing a lock's holders is macOS-only",
    ))
}

#[cfg(target_os = "macos")]
mod macos {
    use std::ffi::{c_int, c_void};
    use std::mem::{MaybeUninit, size_of};
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    /// `PROC_PIDFDVNODEPATHINFO` and the record it fills, from
    /// `<sys/proc_info.h>`; the `libc` crate has the inner records only.
    const PROC_PIDFDVNODEPATHINFO: c_int = 2;
    const PROX_FDTYPE_VNODE: u32 = 1;

    #[repr(C)]
    struct ProcFileinfo {
        fi_openflags: u32,
        fi_status: u32,
        fi_offset: i64,
        fi_type: i32,
        fi_guardflags: u32,
    }

    #[repr(C)]
    struct VnodeFdinfoWithPath {
        pfi: ProcFileinfo,
        pvip: libc::vnode_info_path,
    }

    pub(super) fn all_pids() -> std::io::Result<Vec<i32>> {
        let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if count <= 0 {
            return Err(std::io::Error::last_os_error());
        }
        // Room for the processes born between the two calls.
        let mut pids = vec![0i32; count as usize + 64];
        let bytes = c_int::try_from(pids.len() * size_of::<i32>()).unwrap_or(c_int::MAX);
        let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), bytes) };
        if filled <= 0 {
            return Err(std::io::Error::last_os_error());
        }
        pids.truncate(filled as usize);
        pids.retain(|pid| *pid > 0);
        Ok(pids)
    }

    pub(super) fn bsd_info(pid: i32) -> Option<libc::proc_bsdinfo> {
        info::<libc::proc_bsdinfo>(pid, libc::PROC_PIDTBSDINFO)
    }

    /// When the process `bsd` describes began, since the epoch.
    pub(super) fn started_of(bsd: &libc::proc_bsdinfo) -> std::time::Duration {
        std::time::Duration::from_secs(bsd.pbi_start_tvsec)
            + std::time::Duration::from_micros(bsd.pbi_start_tvusec)
    }

    pub(super) fn threads(pid: i32) -> Option<u32> {
        info::<libc::proc_taskinfo>(pid, libc::PROC_PIDTASKINFO)
            .and_then(|task| u32::try_from(task.pti_threadnum).ok())
    }

    pub(super) fn executable(pid: i32) -> Option<PathBuf> {
        let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let length = unsafe {
            libc::proc_pidpath(
                pid,
                buffer.as_mut_ptr().cast::<c_void>(),
                buffer.len() as u32,
            )
        };
        if length <= 0 {
            return None;
        }
        buffer.truncate(length as usize);
        Some(PathBuf::from(
            <std::ffi::OsString as OsStringExt>::from_vec(buffer),
        ))
    }

    pub(super) fn has_open(pid: i32, device: u64, inode: u64) -> bool {
        let bytes =
            unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0) };
        if bytes <= 0 {
            return false;
        }
        // Room for descriptors opened between the two calls.
        let room = bytes as usize / size_of::<libc::proc_fdinfo>() + 16;
        let mut fds: Vec<libc::proc_fdinfo> = Vec::with_capacity(room);
        let filled = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDLISTFDS,
                0,
                fds.as_mut_ptr().cast::<c_void>(),
                c_int::try_from(room * size_of::<libc::proc_fdinfo>()).unwrap_or(c_int::MAX),
            )
        };
        if filled <= 0 {
            return false;
        }
        unsafe { fds.set_len(filled as usize / size_of::<libc::proc_fdinfo>()) };
        fds.iter()
            .filter(|fd| fd.proc_fdtype == PROX_FDTYPE_VNODE)
            .any(|fd| {
                let mut vnode = MaybeUninit::<VnodeFdinfoWithPath>::zeroed();
                let size = size_of::<VnodeFdinfoWithPath>() as c_int;
                let got = unsafe {
                    libc::proc_pidfdinfo(
                        pid,
                        fd.proc_fd,
                        PROC_PIDFDVNODEPATHINFO,
                        vnode.as_mut_ptr().cast::<c_void>(),
                        size,
                    )
                };
                if got != size {
                    return false;
                }
                let stat = unsafe { vnode.assume_init_ref() }.pvip.vip_vi.vi_stat;
                u64::from(stat.vst_dev) == device && stat.vst_ino == inode
            })
    }

    fn info<T>(pid: i32, flavor: c_int) -> Option<T> {
        let mut value = MaybeUninit::<T>::zeroed();
        let size = size_of::<T>() as c_int;
        let got = unsafe {
            libc::proc_pidinfo(pid, flavor, 0, value.as_mut_ptr().cast::<c_void>(), size)
        };
        (got == size).then(|| unsafe { value.assume_init() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asker() -> Asker {
        Asker {
            pid: 500,
            uid: 501,
            executable: PathBuf::from("/Applications/ZeroCode.app/Contents/MacOS/zerocode-shell"),
            windows: vec![500],
        }
    }

    /// The holder measured on 2026-10-25: the same program, adopted by
    /// launchd, one thread.
    fn stuck_launch() -> Holder {
        Holder {
            pid: 700,
            uid: 501,
            parent: Some(LAUNCHD),
            threads: Some(1),
            executable: Some(asker().executable),
            started: Some(Duration::from_secs(1_792_900_000)),
        }
    }

    #[test]
    fn a_launch_of_this_program_left_under_launchd_with_one_thread_is_ended() {
        assert_eq!(judge(&stuck_launch(), &asker()), Verdict::EndStuckLaunch);
    }

    #[test]
    fn every_other_holder_is_left_with_its_reason() {
        let cases = [
            (
                Holder {
                    pid: 500,
                    ..stuck_launch()
                },
                LeaveReason::ThisProcess,
            ),
            (
                Holder {
                    uid: 502,
                    ..stuck_launch()
                },
                LeaveReason::OtherUser,
            ),
            (
                Holder {
                    executable: Some(PathBuf::from("/bin/zsh")),
                    ..stuck_launch()
                },
                LeaveReason::OtherProgram,
            ),
            (
                Holder {
                    parent: Some(600),
                    ..stuck_launch()
                },
                LeaveReason::HasParent(600),
            ),
            (
                Holder {
                    threads: Some(41),
                    ..stuck_launch()
                },
                LeaveReason::Threads(41),
            ),
            (
                Holder {
                    executable: None,
                    ..stuck_launch()
                },
                LeaveReason::Unreadable("executable"),
            ),
            (
                Holder {
                    parent: None,
                    ..stuck_launch()
                },
                LeaveReason::Unreadable("parent"),
            ),
            (
                Holder {
                    threads: None,
                    ..stuck_launch()
                },
                LeaveReason::Unreadable("threads"),
            ),
            (
                Holder {
                    started: None,
                    ..stuck_launch()
                },
                LeaveReason::Unreadable("start"),
            ),
        ];
        for (holder, reason) in cases {
            assert_eq!(
                judge(&holder, &asker()),
                Verdict::Leave(reason),
                "{holder:?}"
            );
        }
    }

    #[test]
    fn a_known_window_is_never_ended_whatever_it_looks_like() {
        let mut asker = asker();
        asker.windows.push(700);
        assert_eq!(
            judge(&stuck_launch(), &asker),
            Verdict::Leave(LeaveReason::KnownWindow)
        );
    }

    #[test]
    fn a_free_lock_says_nothing_and_a_held_one_names_its_holders() {
        let lock = Path::new("/Users/dev/.zerocode/.settings.lock");
        assert_eq!(
            Settled {
                free: true,
                ..Settled::default()
            }
            .line(lock),
            None
        );
        let line = Settled {
            free: false,
            left: vec![(41, LeaveReason::HasParent(40))],
            waited: Duration::from_millis(3000),
            ..Settled::default()
        }
        .line(lock)
        .unwrap();
        assert!(line.contains("still held after 3000 ms"), "{line}");
        assert!(
            line.contains("left pid 41 running (its parent pid 40 is alive)"),
            "{line}"
        );
    }

    /// Kills the process it names when the test ends, however it ends.
    #[cfg(target_os = "macos")]
    struct Reaped(i32);

    #[cfg(target_os = "macos")]
    impl Drop for Reaped {
        fn drop(&mut self) {
            unsafe { libc::kill(self.0, libc::SIGKILL) };
        }
    }

    /// A child of this test, ended and reaped by its guard however the test
    /// ends — through `Child`, which signals nothing once it has reaped.
    #[cfg(target_os = "macos")]
    struct Sleeper(std::process::Child);

    #[cfg(target_os = "macos")]
    impl Drop for Sleeper {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// The pid [`settle`] judged can end and be handed to another process
    /// before the kill: the start read with the holder must still be that
    /// pid's, or the process now behind it is left alone (t-74976).
    #[cfg(target_os = "macos")]
    #[test]
    fn a_pid_that_now_names_another_process_is_not_ended() {
        use std::os::unix::process::ExitStatusExt as _;

        let mut sleeper = Sleeper(
            std::process::Command::new("/bin/sleep")
                .arg("30")
                .spawn()
                .expect("a child to judge"),
        );
        let pid = i32::try_from(sleeper.0.id()).unwrap();
        let started = macos::started_of(&macos::bsd_info(pid).expect("the child's record"));

        let another = Holder {
            pid,
            started: Some(started + Duration::from_micros(1)),
            ..stuck_launch()
        };
        assert!(
            end(&another).is_err(),
            "a pid judged with another process's start was ended"
        );
        assert!(
            sleeper.0.try_wait().unwrap().is_none(),
            "the process now behind the pid was killed"
        );
        let unread = Holder {
            started: None,
            ..another.clone()
        };
        assert!(end(&unread).is_err(), "a pid with no start read was ended");
        assert!(sleeper.0.try_wait().unwrap().is_none());

        end(&Holder {
            started: Some(started),
            ..another
        })
        .expect("the judged process itself is ended");
        assert_eq!(sleeper.0.wait().unwrap().signal(), Some(libc::SIGKILL));
    }

    /// The measured case with a real process: a launch forked while the lock
    /// was held, never exec'd, and outlived the process that forked it, so
    /// launchd adopted it with the lock's copy in hand. The fork here is a
    /// double fork whose middle process exits at once, which is what leaves
    /// the grandchild under launchd; it calls nothing but `pause` after.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_stuck_launch_holding_the_lock_is_ended_and_the_lock_comes_free() {
        use std::io::Read as _;
        use std::os::fd::FromRawFd as _;

        let directory = tempfile::tempdir().unwrap();
        let repository = SettingsRepository::new(directory.path().join("settings"));
        assert!(repository.lock_free_within(Duration::ZERO).unwrap());
        let window = crate::durable_file::private_lock_file(&repository.lock_path()).unwrap();
        window.lock().unwrap();

        let mut ends = [0; 2];
        assert_eq!(unsafe { libc::pipe(ends.as_mut_ptr()) }, 0);
        let middle = unsafe { libc::fork() };
        assert!(middle >= 0, "fork: {}", std::io::Error::last_os_error());
        if middle == 0 {
            // Only async-signal-safe calls between here and the end of the
            // process: this test binary has other threads.
            unsafe {
                let stuck = libc::fork();
                if stuck == 0 {
                    loop {
                        libc::pause();
                    }
                }
                libc::write(ends[1], (&raw const stuck).cast(), size_of::<i32>());
                libc::_exit(0);
            }
        }
        unsafe { libc::close(ends[1]) };
        let mut status = 0;
        unsafe { libc::waitpid(middle, &raw mut status, 0) };
        let mut said = [0u8; size_of::<i32>()];
        let mut pipe = unsafe { std::fs::File::from_raw_fd(ends[0]) };
        pipe.read_exact(&mut said).unwrap();
        let stuck = Reaped(i32::from_ne_bytes(said));

        // The window that forked it is gone: its descriptor closes, no unlock.
        drop(window);
        assert!(
            !repository
                .lock_free_within(Duration::from_millis(50))
                .unwrap(),
            "the copy in the stuck launch must still hold the lock"
        );

        let holders = holders_of(&repository.lock_path()).unwrap();
        let held = holders
            .iter()
            .find(|holder| holder.pid == stuck.0)
            .unwrap_or_else(|| panic!("the stuck launch is not among {holders:?}"));
        assert_eq!(held.parent, Some(LAUNCHD), "{held:?}");
        assert_eq!(held.threads, Some(1), "{held:?}");

        let asker = Asker::this_process().unwrap();
        let settled = settle(&repository, Duration::from_millis(200), &asker).unwrap();
        assert_eq!(settled.ended, vec![stuck.0], "{settled:?}");
        assert!(settled.free, "{settled:?}");
        assert!(settled.line(&repository.lock_path()).is_some());
    }
}
