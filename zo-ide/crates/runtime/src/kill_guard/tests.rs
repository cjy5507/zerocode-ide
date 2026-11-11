//! The kill guard as zo meets it (t-80732). The judgment is read off a
//! process table handed in wherever it can be; where a real run is under
//! test, the only process it may hit is a `sleep` the test started itself,
//! and whether the bait is still alive is the proof of a refusal.

use std::cell::Cell;

use zerocode_core::shell_rule::kill::{KillVerdict, ProcessTable, TABLE_READ_WALL, WINDOW_PID_ENV};

use super::{judge, judge_with, summoned_worker, table_read_time, TABLE_READ_TIME};

/* ---- a table handed in ------------------------------------------------------ */

/// zo's own pid in [`LISTING`].
const ME: u32 = 700;
/// The window's pid in [`LISTING`].
const WINDOW: u32 = 500;

/// A machine with the window (500), another pane (600) holding a build
/// runner (610), this zo (700) and the shell and `sleep` it started, and a
/// process that hangs in neither (900).
const LISTING: &str = "\
    1     0     1 /sbin/launchd
  500     1   500 /Applications/ZeroCode.app/Contents/MacOS/zerocode-shell
  600   500   600 -zsh
  610   600   600 python3 zo-step-1028.py c-red build
  700   500   700 zo --model opus
  710   700   710 /bin/bash -c sleep 86400.5
  711   710   710 sleep 86400.5
  900     1   900 sleep 3600
";

fn table() -> ProcessTable {
    ProcessTable::from_listing(LISTING, None)
}

fn in_window(name: &str) -> Option<String> {
    (name == WINDOW_PID_ENV).then(|| WINDOW.to_string())
}

fn outside_a_window(_name: &str) -> Option<String> {
    None
}

/// The judgment of `command` for zo at [`ME`] in the window, with [`LISTING`].
fn judged(command: &str) -> Option<KillVerdict> {
    judge_with(command, ME, in_window, table)
}

#[track_caller]
fn assert_refused(command: &str) {
    match judged(command) {
        Some(KillVerdict::Deny { reason }) => assert!(
            reason.contains("pid"),
            "`{command}`'s refusal names what it would hit: {reason}"
        ),
        other => panic!("`{command}` should be refused, judged {other:?}"),
    }
}

#[track_caller]
fn assert_asked(command: &str) {
    assert!(
        matches!(judged(command), Some(KillVerdict::Ask { .. })),
        "`{command}` should be a question for a person, judged {:?}",
        judged(command)
    );
}

#[test]
fn kill_guard_refuses_the_window_another_pane_and_what_runs_in_it() {
    for command in [
        "pkill -f zerocode-shell",
        "kill 500",
        "kill -9 600",
        "kill -TERM 610",
        "pkill -f zo-step",
        "killall python3",
        "killall -m 'pyth.*'",
    ] {
        assert_refused(command);
    }
}

#[test]
fn kill_guard_reads_backticks_and_dollar_parens_inside_double_quotes() {
    // The 10-28 shape: an example inside a double-quoted instruction runs.
    assert_refused("echo \"next, run `pkill -f zo-step` to stop it\"");
    assert_refused("printf '%s\\n' \"$(pkill -f zo-step)\"");
    assert_refused("echo \"$(echo \"$(kill 600)\")\"");
    // A single-quoted one is only letters, and is not judged at all.
    assert_eq!(judged("echo 'run `pkill -f zo-step` to stop it'"), None);
}

#[test]
fn kill_guard_takes_every_word_after_a_bsd_pkill_f_as_a_pattern() {
    // macOS pkill reads `pkill -f a b` as two patterns: `b` alone reaches
    // the build runner in the other pane.
    assert_refused("pkill -f nothing-matches-this zo-step");
    assert_refused("pkill -f sleep c-red");
}

#[test]
fn kill_guard_reads_the_builtin_and_a_program_named_by_its_path() {
    for command in [
        "builtin kill 600",
        "command kill 610",
        "/bin/kill 610",
        "/usr/bin/pkill -f zo-step",
        "/usr/bin/killall python3",
        "sudo pkill -f zo-step",
        "sh -c 'pkill -f zo-step'",
        "pgrep -f zo-step | xargs kill",
        "kill $(pgrep -f zo-step)",
    ] {
        assert_refused(command);
    }
}

#[test]
fn kill_guard_lets_zo_signal_what_it_started() {
    for command in [
        "kill 711",
        "kill -- -710",
        "pkill -f 'sleep 86400.5'",
        "kill $!",
        "sleep 30 & kill %1",
    ] {
        assert_eq!(
            judged(command),
            Some(KillVerdict::Allow),
            "`{command}` signals only zo's own tree"
        );
    }
}

#[test]
fn kill_guard_asks_for_a_pid_known_only_at_run_time_or_one_outside_the_window() {
    assert_asked("kill \"$TARGET\"");
    assert_asked("kill $(cat build.pid)");
    assert_asked("kill 900");
    assert_asked("pkill -f 'sleep 3600'");
}

#[test]
fn kill_guard_asks_when_the_table_does_not_read() {
    let unread = || ProcessTable::unreadable("ps did not finish");
    assert!(matches!(
        judge_with("pkill -f zo-step", ME, in_window, unread),
        Some(KillVerdict::Ask { .. })
    ));
}

/// The minute this crate's tests give `ps` never reaches zo: a build that is
/// not a test gives it the guard's own time, past which a kill call is a
/// question (t-81315, 4275 m-84899).
#[test]
fn kill_guard_gives_ps_the_guards_time_outside_a_test_build() {
    assert_eq!(table_read_time(false), TABLE_READ_WALL, "zo's build");
    assert_eq!(TABLE_READ_TIME, table_read_time(true), "this test build");
}

#[test]
fn kill_guard_outside_a_window_keeps_nothing_but_still_asks() {
    // No window in the environment: nothing is kept, so nothing is refused
    // outright — and everything outside zo's own tree is a question.
    for command in ["kill 600", "pkill -f zo-step", "kill 900"] {
        assert!(
            matches!(
                judge_with(command, ME, outside_a_window, table),
                Some(KillVerdict::Ask { .. })
            ),
            "`{command}` outside a window"
        );
    }
    assert_eq!(
        judge_with("kill 711", ME, outside_a_window, table),
        Some(KillVerdict::Allow)
    );
}

#[test]
fn kill_guard_reads_no_table_for_a_command_without_a_kill_call() {
    let reads = Cell::new(0);
    let counted = || {
        reads.set(reads.get() + 1);
        table()
    };
    for command in [
        "cargo test --locked -p runtime",
        "git log --oneline -5",
        "echo skill && grep -rn killer src/",
        "echo 'pkill -f zo-step'",
        "kill $!",
        "kill %1",
    ] {
        assert_ne!(
            judge_with(command, ME, in_window, counted).map(|verdict| matches!(verdict, KillVerdict::Deny { .. })),
            Some(true),
            "`{command}`"
        );
    }
    assert_eq!(reads.get(), 0, "no command above needs the process table");
    assert_eq!(judge("cargo build --release"), None);
}

#[test]
fn kill_guard_a_summoned_worker_pane_has_nobody_to_ask() {
    // As the ledger sets them: a coordinator's pane leads its own team, a
    // worker's pane is another, and a pane a person opened names neither.
    assert!(summoned_worker(Some("%3"), Some("%1")), "a worker's pane");
    assert!(!summoned_worker(Some("%1"), Some("%1")), "the coordinator's own pane");
    assert!(!summoned_worker(None, None), "a pane a person opened");
    assert!(!summoned_worker(None, Some("%1")));
}

/* ---- a real run: a bait the test started stands for the window ------------------ */

/// Every run here starts a real `sleep` and reads the real `ps`: unix only.
#[cfg(unix)]
mod real_run {
    use std::ffi::CString;
    use std::io::Read as _;
    use std::marker::PhantomData;
    use std::os::fd::{AsRawFd as _, OwnedFd};
    use std::process::{Child, Command, Stdio};

    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    use zerocode_core::agent_teams::{TEAM_LEADER_VAR, TEAM_PANE_VAR};
    use zerocode_core::hook::PANE_KEY_ENV;
    use zerocode_core::shell_rule::kill::WINDOW_PID_ENV;

    use crate::permission_enforcer::{EnforcementResult, PermissionEnforcer};
    use crate::sandbox::FilesystemIsolationMode;
    use crate::{
        execute_bash, BashCommandInput, PermissionContext, PermissionMode, PermissionOutcome,
        PermissionOverride, PermissionPolicy, PermissionPromptDecision, PermissionPrompter,
        PermissionRequest,
    };

    /// How long a bait or an orphan lives when nothing ends it, in seconds,
    /// and so the whole part of every marker. It was a day; a run killed
    /// outright left its baits standing that long (t-83535). Longer than any
    /// test here takes under a release lane's load: the longest runs bash
    /// twice, each with a 20-second timeout, and waits 10 seconds.
    const BAIT_LIFE_SECS: u32 = 120;

    /// What every marker of this process begins with. The pid is written at
    /// the width of the largest one Linux gives (2^22, seven digits), so no
    /// other process's markers begin with this one's: a copy of this binary
    /// whose pid began with this pid's digits had them.
    fn marker_prefix() -> String {
        format!("{BAIT_LIFE_SECS}.{:07}", std::process::id())
    }

    /// The marker of bait `n`: a `sleep` duration no other process carries, so
    /// a pattern naming it can only ever match it.
    fn marker(n: u32) -> String {
        format!("{}{n}", marker_prefix())
    }

    /// Set in the copy of this test binary that [`clean_start`] starts: this
    /// process's pid and the command line the copy becomes once it has closed
    /// every descriptor above stderr.
    const CLEAN_START_ENV: &str = "ZO_KILL_GUARD_CLEAN_START";

    /// `argv`, started holding no descriptor but the three it is given
    /// (t-83535). std starts a child with every descriptor this process holds
    /// without close-on-exec, and on macOS another thread's pipe is one until
    /// std marks it; a `pre_exec` closure that closes them would be unsafe
    /// code, which the workspace forbids. So a copy of this binary starts
    /// first and runs [`clean_start_child`] alone, which closes every
    /// descriptor above stderr and becomes `argv` by `exec`: an end it took
    /// along is open only while that copy starts.
    fn clean_start(argv: &[&str]) -> Command {
        let copy = this_binary_running("clean_start_child");
        let asked = serde_json::to_string(&(std::process::id(), argv)).expect("what to start");
        let mut command = Command::new(&copy[0]);
        command
            .args(&copy[1..])
            .env(CLEAN_START_ENV, asked)
            .stdin(Stdio::null());
        command
    }

    /// Not a test of its own: the copy of this binary that [`clean_start`]
    /// starts runs only this, closes every descriptor above stderr and becomes
    /// the command line it was asked for. In a run of the suite it does
    /// nothing, and so does a run that finds the request in the environment it
    /// was started with: the request names the process that made it, and only
    /// that process's child acts on it.
    #[test]
    fn clean_start_child() {
        let Ok(asked) = std::env::var(CLEAN_START_ENV) else {
            return;
        };
        let (parent, argv): (u32, Vec<String>) =
            serde_json::from_str(&asked).expect("what to start");
        if parent != std::os::unix::process::parent_id() {
            return;
        }
        std::env::remove_var(CLEAN_START_ENV);
        let argv: Vec<CString> = argv
            .into_iter()
            .map(|word| CString::new(word).expect("a word without NUL"))
            .collect();
        let stderr = std::io::stderr().as_raw_fd();
        let open: Vec<i32> = std::fs::read_dir("/dev/fd")
            .expect("this process's descriptors")
            .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
            .collect();
        // The listing's own descriptor is closed by now and fails to close again.
        for fd in open.into_iter().filter(|fd| *fd > stderr) {
            let _ = nix::unistd::close(fd);
        }
        let Err(error) = nix::unistd::execvp(&argv[0], &argv);
        panic!("{:?} did not start: {error}", argv[0]);
    }

    /// How often a wait here looks again.
    const POLL: std::time::Duration = std::time::Duration::from_millis(10);

    /// Whether `done` comes true within [`SETTLE`], asked every [`POLL`].
    fn within(mut done: impl FnMut() -> bool) -> bool {
        let since = std::time::Instant::now();
        loop {
            if done() {
                return true;
            }
            if since.elapsed() >= SETTLE {
                return false;
            }
            std::thread::sleep(POLL);
        }
    }

    /// What the read end `end` said until its last writer let it go, read
    /// within [`SETTLE`]; `None` while a writer still holds it then.
    fn read_within(end: OwnedFd) -> Option<Vec<u8>> {
        use nix::fcntl::{fcntl, FcntlArg, OFlag};
        fcntl(end.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK))
            .expect("the read end stops blocking");
        let mut end = std::fs::File::from(end);
        let mut said = Vec::new();
        within(|| end.read_to_end(&mut said).is_ok()).then_some(said)
    }

    /// Whether `pid` still runs a command that names `marker`: one that ended,
    /// or whose pid another process has taken since, does not.
    fn runs(pid: u32, marker: &str) -> bool {
        Command::new("ps")
            .args(["-o", "command=", "-p", &pid.to_string()])
            .stderr(Stdio::null())
            .output()
            .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains(marker))
    }

    /// Whether `pid` stops running `marker` within [`SETTLE`].
    fn ends(pid: u32, marker: &str) -> bool {
        within(|| !runs(pid, marker))
    }

    /// The pids of this user's `sleep` and `sh` processes with an argument
    /// that begins with `prefix`, as `ps` lists them. Nothing else is taken
    /// for a bait, whatever its command line says.
    fn carrying(prefix: &str) -> std::io::Result<Vec<u32>> {
        let output = Command::new("ps")
            .args(["-A", "-o", "uid=,pid=,command="])
            .stderr(Stdio::null())
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!("ps ended with {}", output.status)));
        }
        let listing = String::from_utf8_lossy(&output.stdout);
        let rows: Vec<(&str, u32, Vec<&str>)> = listing
            .lines()
            .filter_map(|line| {
                let mut words = line.split_whitespace();
                let uid = words.next()?;
                let pid = words.next()?.parse().ok()?;
                Some((uid, pid, words.collect()))
            })
            .collect();
        // This process's own row says which user it runs as.
        let me = std::process::id();
        let Some(user) = rows.iter().find(|(_, pid, _)| *pid == me).map(|(uid, ..)| *uid) else {
            return Err(std::io::Error::other("ps did not list this process"));
        };
        Ok(rows
            .iter()
            .filter(|(uid, _, words)| {
                *uid == user
                    && words.first().is_some_and(|program| {
                        matches!(program.rsplit('/').next(), Some("sleep" | "sh"))
                    })
                    && words.iter().skip(1).any(|word| word.starts_with(prefix))
            })
            .map(|(_, pid, _)| *pid)
            .collect())
    }

    /// `pid` sent a `SIGKILL`.
    fn end(pid: u32) {
        let _ = kill(Pid::from_raw(i32::try_from(pid).expect("a pid")), Signal::SIGKILL);
    }

    /// What went wrong as a test ended: red, unless the test is red already
    /// and a second panic would abort the run.
    fn red(said: &str) {
        if std::thread::panicking() {
            eprintln!("{said}");
        } else {
            panic!("{said}");
        }
    }

    /// A `sleep` this test started, whose duration no other process carries, so
    /// a pattern naming it can only ever match it. It starts only while the
    /// crate's env lock is held and cannot outlive it ([`GuardEnv`]), holds no
    /// descriptor but the three it is given ([`clean_start`]), and is ended
    /// and reaped when dropped.
    struct Bait<'env> {
        child: Child,
        marker: String,
        _env: PhantomData<&'env GuardEnv>,
    }

    impl<'env> Bait<'env> {
        fn start(_env: &'env GuardEnv, n: u32) -> Self {
            let marker = marker(n);
            let script = format!("exec sleep {marker} >/dev/null");
            let mut child = clean_start(&["sh", "-c", script.as_str()])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("the bait starts");
            // The shell's command line names the marker, and the shell lets its
            // stdout go as it becomes the `sleep`: once that stdout ends, the
            // marker names the bait.
            let stdout = child.stdout.take().expect("the bait's stdout");
            let pid = child.id();
            if read_within(stdout.into()).is_none() || !within(|| runs(pid, &marker)) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the bait {marker} did not stand within {SETTLE:?}");
            }
            Self {
                child,
                marker,
                _env: PhantomData,
            }
        }

        fn pid(&self) -> u32 {
            self.child.id()
        }

        fn alive(&mut self) -> bool {
            self.child.try_wait().expect("the bait can be asked").is_none()
        }
    }

    impl Drop for Bait<'_> {
        fn drop(&mut self) {
            let _ = self.child.kill();
            if let Err(error) = self.child.wait() {
                red(&format!(
                    "a test left running bait {} ({}): {error}",
                    self.child.id(),
                    self.marker
                ));
            }
        }
    }

    /// A `sleep` this test set loose: a shell started it in the background
    /// and exited, so it hangs under pid 1, outside this process's tree — a
    /// process zo started that is no longer its child. It carries no pane
    /// key, so no pane can claim it as its own job. Like a [`Bait`], it starts
    /// only while the env lock is held and holds no descriptor it was not
    /// given; it is ended when dropped, and a test it outlives is red.
    struct Orphan<'env> {
        pid: u32,
        marker: String,
        _env: PhantomData<&'env GuardEnv>,
    }

    impl<'env> Orphan<'env> {
        fn start(_env: &'env GuardEnv, n: u32) -> Self {
            let marker = marker(n);
            // The copy of this binary runs one test, and libtest writes that
            // test's name with no line end before it runs it: the shell starts
            // a line of its own for the pid.
            let script =
                format!("sleep {marker} </dev/null >/dev/null 2>&1 & printf '\\npid %s\\n' \"$!\"");
            let mut shell = clean_start(&["sh", "-c", script.as_str()])
                .env_remove(PANE_KEY_ENV)
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("the orphan starts");
            let said = read_within(shell.stdout.take().expect("the shell's stdout").into());
            if said.is_none() {
                let _ = shell.kill();
            }
            let _ = shell.wait();
            let said = said.expect("the shell lets its stdout go");
            let pid = String::from_utf8_lossy(&said)
                .lines()
                .find_map(|line| line.strip_prefix("pid ")?.trim().parse().ok())
                .expect("the shell names the orphan's pid");
            assert!(within(|| runs(pid, &marker)), "the orphan {marker} stands");
            Self {
                pid,
                marker,
                _env: PhantomData,
            }
        }

        fn alive(&self) -> bool {
            runs(self.pid, &self.marker)
        }
    }

    impl Drop for Orphan<'_> {
        fn drop(&mut self) {
            // Signalled only while it is still this test's `sleep`: a pid that
            // has ended may be another process's by now.
            if self.alive() {
                end(self.pid);
            }
            if !ends(self.pid, &self.marker) {
                red(&format!("a test left running orphan {} ({})", self.pid, self.marker));
            }
        }
    }

    /// What the guard reads from the environment, and the pane key its
    /// refusals read for the pane mark this process carries (t-82854).
    const GUARD_VARS: [&str; 4] = [WINDOW_PID_ENV, TEAM_PANE_VAR, TEAM_LEADER_VAR, PANE_KEY_ENV];

    /// The crate's env lock, held, and the environment the guard reads, put
    /// back when dropped: the window this process runs in, and the pane and
    /// team leader it runs under — so a test's answer never depends on the
    /// pane the suite happens to run in. A [`Bait`] or an [`Orphan`] starts
    /// only while one is held and cannot outlive it (t-83535): a test that
    /// started its bait and then waited for the lock held the pipe of the test
    /// that had the lock, and neither moved again. When dropped it ends
    /// whatever still carries a marker of this process, and the test that
    /// left it is red.
    struct GuardEnv {
        before: Vec<(&'static str, Option<String>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl GuardEnv {
        /// Takes the crate's env lock and notes what the guard reads, to put
        /// back.
        fn hold() -> Self {
            let lock = crate::test_env_lock();
            let before = GUARD_VARS
                .iter()
                .map(|name| (*name, std::env::var(name).ok()))
                .collect();
            Self {
                before,
                _lock: lock,
            }
        }

        /// The window this process runs in is `window`, and it runs in the
        /// pane `pane` of a team led from `leader`, carrying the pane mark
        /// `mark` ([`GUARD_VARS`], in order).
        fn set(&self, window: u32, pane: Option<&str>, leader: Option<&str>, mark: Option<&str>) {
            let window = window.to_string();
            let values = [Some(window.as_str()), pane, leader, mark];
            for ((name, _), value) in self.before.iter().zip(values) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }

        /// The window is `pid`, in a pane a person opened.
        fn window(&self, pid: u32) {
            self.set(pid, None, None, None);
        }

        /// The window is `pid`, and this process carries the mark of its pane
        /// `key`, as zo does in a pane the window started.
        fn marked_window(&self, pid: u32, key: &str) {
            self.set(pid, None, None, Some(key));
        }
    }

    impl Drop for GuardEnv {
        fn drop(&mut self) {
            for (name, value) in &self.before {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
            let prefix = marker_prefix();
            match carrying(&prefix) {
                Ok(left) if left.is_empty() => {}
                Ok(left) => {
                    for pid in &left {
                        end(*pid);
                    }
                    let standing: Vec<u32> =
                        left.iter().copied().filter(|pid| !ends(*pid, &prefix)).collect();
                    red(&format!(
                        "a test left running {left:?}, which carry this process's marker \
                         {prefix}; still running after {SETTLE:?}: {standing:?}"
                    ));
                }
                Err(error) => red(&format!(
                    "the baits a test may have left could not be listed: {error}"
                )),
            }
        }
    }

    /// How long the read end of a pipe is given to see its end once its last
    /// write end here is closed, and a process this test ended to be gone.
    const SETTLE: std::time::Duration = std::time::Duration::from_secs(10);

    /// Set in a copy of this test binary that runs one test of this module
    /// alone ([`run_alone`]).
    const ALONE_ENV: &str = "ZO_KILL_GUARD_TEST_ALONE";

    /// The command line that runs only `test` of this module, in a copy of
    /// this test binary.
    fn this_binary_running(test: &str) -> Vec<String> {
        let module = module_path!()
            .split_once("::")
            .map_or(module_path!(), |(_crate, path)| path);
        let binary = std::env::current_exe().expect("this test binary");
        vec![
            binary.to_str().expect("this test binary's path in UTF-8").to_owned(),
            format!("{module}::{test}"),
            "--exact".to_owned(),
            "--nocapture".to_owned(),
            "--test-threads=1".to_owned(),
        ]
    }

    /// Runs `test` alone in a copy of this binary, where no other test's child
    /// can take along a descriptor it leaves open, and asserts it passed. The
    /// copy holds no descriptor of this process ([`clean_start`]).
    fn run_alone(test: &str) {
        let argv = this_binary_running(test);
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let child = {
            // The copy starts from this process's environment as it stands; the
            // lock keeps another test's export out of that copy.
            let _lock = crate::test_env_lock();
            clean_start(&argv)
                .env(ALONE_ENV, "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("the test runs alone")
        };
        let output = child.wait_with_output().expect("the test ran alone");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains(" 1 passed;"),
            "{test}, run alone:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Which of `ends` still has a writer [`SETTLE`] after it is read. Each is
    /// the read end of a pipe whose write ends here are closed, named for what
    /// started while it was open.
    fn still_written(ends: Vec<(&'static str, OwnedFd)>) -> Vec<&'static str> {
        ends.into_iter()
            .filter_map(|(name, end)| read_within(end).is_none().then_some(name))
            .collect()
    }

    /// A bait and an orphan take along no descriptor but the three they are
    /// given (t-83535). On macOS std makes a pipe with `pipe()` and only then
    /// marks it close-on-exec, so a child another thread starts in between
    /// takes its write end along. On 10-29 a bait took one from a
    /// `git_snapshot` test's `git`: that test waited for the pipe's end
    /// holding the env lock, the bait's test waited for the lock, and the
    /// release lane stood for 55 minutes. Here a pipe is left open without
    /// close-on-exec on purpose while each starts; once its write end here
    /// closes, its read end ends. Run alone, so no other test's child can
    /// take them along.
    #[test]
    fn kill_guard_bait_and_orphan_take_no_pipe_along() {
        if std::env::var_os(ALONE_ENV).is_none() {
            run_alone("kill_guard_bait_and_orphan_take_no_pipe_along");
            return;
        }
        let env = GuardEnv::hold();
        let (bait_end, write) = nix::unistd::pipe().expect("a pipe a child takes along");
        let _bait = Bait::start(&env, 50);
        drop(write);
        let (orphan_end, write) = nix::unistd::pipe().expect("a pipe a child takes along");
        let _orphan = Orphan::start(&env, 51);
        drop(write);
        let held = still_written(vec![("the bait", bait_end), ("the orphan", orphan_end)]);
        assert!(
            held.is_empty(),
            "{held:?} took along the write end of a pipe nobody gave them"
        );
    }

    /// A test that leaves a bait standing is red, and the bait does not
    /// outlive it (t-83535): the env lock's holder ends, as it goes, whatever
    /// still carries a marker of this process.
    #[test]
    fn kill_guard_a_bait_left_standing_turns_its_test_red() {
        let env = GuardEnv::hold();
        let bait = Bait::start(&env, 52);
        let (pid, marker) = (bait.pid(), bait.marker.clone());
        std::mem::forget(bait);
        let red = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(env)))
            .expect_err("a test that left its bait standing is red");
        let said = red.downcast::<String>().expect("the panic says what was left");
        assert!(
            said.contains("a test left running") && said.contains(&format!("[{pid}]")),
            "{said}"
        );
        assert!(ends(pid, &marker), "the bait did not outlive its test");
    }

    fn run(command: &str) -> std::io::Result<crate::BashCommandOutput> {
        execute_bash(BashCommandInput {
            command: command.to_string(),
            timeout: Some(20_000),
            description: None,
            run_in_background: Some(false),
            dangerously_disable_sandbox: Some(true),
            namespace_restrictions: Some(false),
            isolate_network: Some(false),
            filesystem_mode: Some(FilesystemIsolationMode::WorkspaceOnly),
            allowed_mounts: None,
            cwd: None,
        })
    }

    #[test]
    fn kill_guard_execute_bash_refuses_a_signal_to_the_window_in_every_shape() {
        let env = GuardEnv::hold();
        let mut bait = Bait::start(&env, 1);
        env.window(bait.pid());
        let marker = bait.marker.clone();
        let pid = bait.pid();
        for command in [
            format!("pkill -f {marker}"),
            format!("echo \"stop it with `pkill -f {marker}`\""),
            format!("echo \"$(pkill -f {marker})\""),
            format!("pkill -f nothing-matches-this {marker}"),
            format!("kill {pid}"),
            format!("builtin kill -9 {pid}"),
            format!("/usr/bin/pkill -f {marker}"),
        ] {
            let refused = match run(&command) {
                Ok(output) => panic!("`{command}` ran: {output:?}"),
                Err(refused) => refused,
            };
            assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied, "`{command}`");
            assert!(
                refused.to_string().contains(&pid.to_string()),
                "`{command}`'s refusal names the pid it would hit: {refused}"
            );
            assert!(bait.alive(), "`{command}` left the bait alive");
        }
    }

    #[test]
    fn kill_guard_execute_bash_lets_zo_end_what_it_started() {
        let env = GuardEnv::hold();
        let window = Bait::start(&env, 2);
        let mut own = Bait::start(&env, 3);
        env.window(window.pid());
        let output = run(&format!("pkill -f '[{}]{}'", &own.marker[..1], &own.marker[1..]))
            .expect("a signal to zo's own child runs");
        assert_eq!(output.return_code_interpretation, None, "{output:?}");
        let ended = std::time::Instant::now();
        while own.alive() {
            assert!(
                ended.elapsed() < std::time::Duration::from_secs(10),
                "zo's own child ended"
            );
            std::thread::sleep(POLL);
        }
    }

    #[test]
    fn kill_guard_execute_bash_refuses_a_question_in_a_summoned_worker_pane() {
        let env = GuardEnv::hold();
        let window = Bait::start(&env, 7);
        let mut bait = Bait::start(&env, 8);
        let pid_file = std::env::temp_dir().join(format!("{}.pid", bait.marker));
        std::fs::write(&pid_file, bait.pid().to_string()).expect("the pid file is written");
        // A pid read at run time is a question, and the paths that reach the
        // chokepoint past the permission layer (a deep_gate check, a `!` line)
        // never put it to anyone.
        let command = format!("kill $(cat {})", pid_file.display());
        {
            env.set(window.pid(), Some("%3"), Some("%1"), None);
            let refused = match run(&command) {
                Ok(output) => panic!("`{command}` ran in a worker's pane: {output:?}"),
                Err(refused) => refused,
            };
            assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied, "{refused}");
            assert!(refused.to_string().contains("no person said yes"), "{refused}");
            assert!(bait.alive(), "the refusal left the bait alive");
        }
        // A pane a person opened keeps the question the permission layer's.
        env.window(window.pid());
        let output = run(&command).expect("a question a person could answer runs");
        assert_eq!(output.return_code_interpretation, None, "{output:?}");
        let ended = std::time::Instant::now();
        while bait.alive() {
            assert!(
                ended.elapsed() < std::time::Duration::from_secs(10),
                "the bait ended"
            );
            std::thread::sleep(POLL);
        }
        let _ = std::fs::remove_file(&pid_file);
    }

    /* ---- the permission layers -------------------------------------------------------- */

    fn bash(command: &str) -> String {
        serde_json::json!({ "command": command }).to_string()
    }

    struct Person {
        says_yes: bool,
        asked: Vec<PermissionRequest>,
    }

    impl Person {
        fn saying(says_yes: bool) -> Self {
            Self {
                says_yes,
                asked: Vec::new(),
            }
        }
    }

    impl PermissionPrompter for Person {
        fn decide(&mut self, request: &PermissionRequest) -> PermissionPromptDecision {
            self.asked.push(request.clone());
            if self.says_yes {
                PermissionPromptDecision::Allow
            } else {
                PermissionPromptDecision::Deny {
                    reason: "the person said no".to_string(),
                }
            }
        }
    }

    const EVERY_MODE: [PermissionMode; 5] = [
        PermissionMode::ReadOnly,
        PermissionMode::WorkspaceWrite,
        PermissionMode::DangerFullAccess,
        PermissionMode::Prompt,
        PermissionMode::Allow,
    ];

    #[test]
    fn kill_guard_check_bash_refuses_in_every_mode_and_cannot_ask() {
        let env = GuardEnv::hold();
        let bait = Bait::start(&env, 4);
        env.window(bait.pid());
        let aimed = format!("pkill -f {}", bait.marker);
        for mode in EVERY_MODE {
            let enforcer = PermissionEnforcer::new(PermissionPolicy::new(mode));
            match enforcer.check_bash(&aimed) {
                EnforcementResult::Denied { reason, .. } => assert!(
                    reason.contains(&bait.pid().to_string()),
                    "{mode:?}: {reason}"
                ),
                EnforcementResult::Allowed => panic!("{mode:?} let `{aimed}` through"),
            }
            // A question has nobody to answer it here: it is a refusal that says so.
            match enforcer.check_bash("kill \"$TARGET\"") {
                EnforcementResult::Denied { reason, .. } => {
                    assert!(reason.contains("yes"), "{mode:?}: {reason}");
                }
                EnforcementResult::Allowed => panic!("{mode:?} let an unasked kill through"),
            }
        }
        let enforcer = PermissionEnforcer::new(PermissionPolicy::new(PermissionMode::DangerFullAccess));
        assert_eq!(enforcer.check_bash("cargo test --locked"), EnforcementResult::Allowed);
    }

    #[test]
    fn kill_guard_authorize_refuses_the_window_without_asking_whatever_the_mode_or_rules() {
        let env = GuardEnv::hold();
        let bait = Bait::start(&env, 5);
        env.window(bait.pid());
        let aimed = bash(&format!("echo \"`pkill -f {}`\"", bait.marker));
        for mode in EVERY_MODE {
            let policy = PermissionPolicy::new(mode).with_permission_rules(
                &crate::RuntimePermissionRuleConfig::new(
                    vec!["bash(*)".to_string()],
                    Vec::new(),
                    Vec::new(),
                ),
            );
            let mut person = Person::saying(true);
            match policy.authorize("bash", &aimed, Some(&mut person)) {
                PermissionOutcome::Deny { reason } => {
                    assert!(reason.contains(&bait.pid().to_string()), "{mode:?}: {reason}");
                }
                PermissionOutcome::Allow => panic!("{mode:?} with an allow rule let it through"),
            }
            assert!(person.asked.is_empty(), "{mode:?}: a refusal is not a question");
        }
    }

    #[test]
    fn kill_guard_authorize_asks_a_person_once_and_refuses_when_nobody_can_answer() {
        let env = GuardEnv::hold();
        let bait = Bait::start(&env, 6);
        env.window(bait.pid());
        let unknown = bash("kill \"$TARGET\"");
        // The modes that would ask about this call anyway. Where the mode
        // runs it unasked, the question is refused instead (t-82420, below).
        for mode in [PermissionMode::WorkspaceWrite, PermissionMode::Prompt] {
            let policy = PermissionPolicy::new(mode);
            // `zo -p` and every run with nobody at the keyboard: no prompter.
            match policy.authorize("bash", &unknown, None) {
                PermissionOutcome::Deny { reason } => assert!(reason.contains("yes"), "{mode:?}: {reason}"),
                PermissionOutcome::Allow => panic!("{mode:?} ran an unasked kill with nobody to ask"),
            }
            let mut yes = Person::saying(true);
            assert_eq!(
                policy.authorize("bash", &unknown, Some(&mut yes)),
                PermissionOutcome::Allow,
                "{mode:?}: the person said yes"
            );
            assert_eq!(yes.asked.len(), 1, "{mode:?}: one question");
            let question = yes.asked[0].reason.clone().unwrap_or_default();
            assert!(question.contains("$TARGET"), "{mode:?}: the question names the call: {question}");
            assert!(yes.asked[0].person_only, "{mode:?}: only a person may answer it");
            let mut no = Person::saying(false);
            assert!(
                matches!(
                    policy.authorize("bash", &unknown, Some(&mut no)),
                    PermissionOutcome::Deny { .. }
                ),
                "{mode:?}: the person said no"
            );
        }
        // Read-only cannot run it whatever a person says, so nobody is asked.
        let mut person = Person::saying(true);
        assert!(matches!(
            PermissionPolicy::new(PermissionMode::ReadOnly).authorize("bash", &unknown, Some(&mut person)),
            PermissionOutcome::Deny { .. }
        ));
        assert!(person.asked.is_empty());
    }

    /// Dispatch runs a call to `Bash` or `BASH` as `bash`, so the guard judges
    /// it as `bash` (t-81312): a signal to the window is refused unasked, and
    /// a kill that needs a person's yes is refused with nobody to ask. A
    /// `Bash` call used to skip the guard and run.
    #[test]
    fn kill_guard_authorize_judges_a_call_to_bash_whatever_its_spelling() {
        // 0: no other bait's marker is a prefix of this one, nor this of theirs.
        let env = GuardEnv::hold();
        let bait = Bait::start(&env, 0);
        env.window(bait.pid());
        let aimed = bash(&format!("pkill -f {}", bait.marker));
        let unknown = bash("kill \"$TARGET\"");
        let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess);
        for tool in ["Bash", "BASH"] {
            let mut person = Person::saying(true);
            match policy.authorize(tool, &aimed, Some(&mut person)) {
                PermissionOutcome::Deny { reason } => {
                    assert!(reason.contains(&bait.pid().to_string()), "{tool}: {reason}");
                }
                PermissionOutcome::Allow => panic!("{tool}: the guard let `pkill` at the window through"),
            }
            assert!(person.asked.is_empty(), "{tool}: a refusal is not a question");
            match policy.authorize(tool, &unknown, None) {
                PermissionOutcome::Deny { reason } => assert!(reason.contains("yes"), "{tool}: {reason}"),
                PermissionOutcome::Allow => panic!("{tool}: an unasked kill ran with nobody to ask"),
            }
        }
    }

    /// Dispatch unwraps a call through `CapabilityInvoke` and runs the call it
    /// carries, so the guard judges that call: a signal to the window is
    /// refused unasked, and a kill that needs a person's yes is put to a
    /// person, or refused with nobody to ask. A shell command inside the
    /// wrapper used to be judged as the wrapper, which the guard does not
    /// read: the question was never put, and the kill went ahead at the
    /// shell's chokepoint, which leaves a question to this layer.
    #[test]
    fn kill_guard_authorize_judges_a_shell_command_the_wrapper_carries() {
        use core_types::tool_names::CAPABILITY_INVOKE;

        // 90: no other bait's marker begins with this one.
        let env = GuardEnv::hold();
        let bait = Bait::start(&env, 90);
        env.window(bait.pid());
        let carrying = |command: &str| {
            serde_json::json!({ "name": "Bash", "input": { "command": command } }).to_string()
        };
        let aimed = carrying(&format!("pkill -f {}", bait.marker));
        let unknown = carrying("kill \"$TARGET\"");
        let policy = PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement(CAPABILITY_INVOKE, PermissionMode::ReadOnly);

        let mut person = Person::saying(true);
        match policy.authorize(CAPABILITY_INVOKE, &aimed, Some(&mut person)) {
            PermissionOutcome::Deny { reason } => {
                assert!(reason.contains(&bait.pid().to_string()), "{reason}");
            }
            PermissionOutcome::Allow => panic!("the guard let `pkill` at the window through the wrapper"),
        }
        assert!(person.asked.is_empty(), "a refusal is not a question");

        match policy.authorize(CAPABILITY_INVOKE, &unknown, None) {
            PermissionOutcome::Deny { reason } => assert!(reason.contains("yes"), "{reason}"),
            PermissionOutcome::Allow => panic!("an unasked kill ran through the wrapper with nobody to ask"),
        }
        let mut yes = Person::saying(true);
        assert_eq!(
            policy.authorize(CAPABILITY_INVOKE, &unknown, Some(&mut yes)),
            PermissionOutcome::Allow,
            "the person said yes"
        );
        let [question] = yes.asked.as_slice() else {
            panic!("asked {} times", yes.asked.len());
        };
        assert!(question.person_only, "only a person may answer it");
        assert_eq!(question.tool_name, "bash", "the question is about the command that runs");
        assert!(
            question.reason.as_deref().unwrap_or_default().contains("$TARGET"),
            "the question names the call: {:?}",
            question.reason
        );
    }

    /// Where the mode or a rule would run the call without asking — full
    /// access, the allow mode, an allow rule for it — a kill that needs a
    /// person's yes is refused unasked, as the window's hook refuses it
    /// (t-82420): a question there is one nobody expects, and an unanswered
    /// one stalls the turn. The refusal says what to do instead. The targets
    /// are a process this test set loose, by its pid and by a pid read at run
    /// time.
    #[test]
    fn kill_guard_authorize_refuses_unasked_where_the_mode_or_a_rule_would_run_the_call() {
        let env = GuardEnv::hold();
        let window = Bait::start(&env, 40);
        let orphan = Orphan::start(&env, 41);
        let pid_file = std::env::temp_dir().join(format!("{}.pid", orphan.marker));
        std::fs::write(&pid_file, orphan.pid.to_string()).expect("the pid file is written");
        env.window(window.pid());
        let kill_rule = crate::RuntimePermissionRuleConfig::new(
            vec!["bash(kill:*)".to_string()],
            Vec::new(),
            Vec::new(),
        );
        let unasked = [
            ("danger-full-access", PermissionPolicy::new(PermissionMode::DangerFullAccess)),
            ("allow", PermissionPolicy::new(PermissionMode::Allow)),
            (
                "workspace-write with an allow rule",
                PermissionPolicy::new(PermissionMode::WorkspaceWrite).with_permission_rules(&kill_rule),
            ),
        ];
        for command in [format!("kill {}", orphan.pid), format!("kill $(cat {})", pid_file.display())] {
            for (what, policy) in &unasked {
                let mut person = Person::saying(true);
                match policy.authorize("bash", &bash(&command), Some(&mut person)) {
                    PermissionOutcome::Deny { reason } => {
                        assert!(reason.contains("no person said yes"), "{what}, `{command}`: {reason}");
                        assert!(
                            reason.contains("its pid as a number"),
                            "{what}, `{command}`: the refusal says what to do instead: {reason}"
                        );
                    }
                    PermissionOutcome::Allow => panic!("{what}: `{command}` was allowed"),
                }
                assert!(
                    person.asked.is_empty(),
                    "{what}, `{command}`: nobody is asked where the mode runs the call unasked"
                );
            }
        }
        assert!(orphan.alive(), "nothing ran, so the orphan lives");
        let _ = std::fs::remove_file(&pid_file);
    }

    /// What t-82420 keeps: where the mode would ask anyway the kill is still
    /// put to a person, once and for this call; a signal to the window is
    /// refused in every mode; and zo's own child is zo's to end.
    #[test]
    fn kill_guard_authorize_still_asks_where_the_mode_would_and_keeps_its_deny_and_allow() {
        let env = GuardEnv::hold();
        let window = Bait::start(&env, 43);
        let own = Bait::start(&env, 44);
        let orphan = Orphan::start(&env, 45);
        env.window(window.pid());

        let loose = bash(&format!("kill {}", orphan.pid));
        let mut yes = Person::saying(true);
        assert_eq!(
            PermissionPolicy::new(PermissionMode::Prompt).authorize("bash", &loose, Some(&mut yes)),
            PermissionOutcome::Allow,
            "the person said yes"
        );
        assert_eq!(yes.asked.len(), 1, "one question");
        assert!(yes.asked[0].person_only, "only a person may answer it");

        let aimed = bash(&format!("kill {}", window.pid()));
        for mode in EVERY_MODE {
            let mut person = Person::saying(true);
            match PermissionPolicy::new(mode).authorize("bash", &aimed, Some(&mut person)) {
                PermissionOutcome::Deny { reason } => {
                    assert!(reason.contains(&window.pid().to_string()), "{mode:?}: {reason}");
                }
                PermissionOutcome::Allow => panic!("{mode:?} let a signal to the window through"),
            }
            assert!(person.asked.is_empty(), "{mode:?}: a refusal is not a question");
        }

        let mut person = Person::saying(false);
        assert_eq!(
            PermissionPolicy::new(PermissionMode::DangerFullAccess).authorize(
                "bash",
                &bash(&format!("kill {}", own.pid())),
                Some(&mut person)
            ),
            PermissionOutcome::Allow,
            "zo's own child is zo's to end"
        );
        assert!(person.asked.is_empty(), "nobody is asked about zo's own child");
        assert!(orphan.alive(), "nothing ran, so the orphan lives");
    }

    #[test]
    fn kill_guard_authorize_refuses_a_question_in_a_summoned_worker_pane_unasked() {
        let env = GuardEnv::hold();
        let bait = Bait::start(&env, 9);
        env.set(bait.pid(), Some("%3"), Some("%1"), None);
        let unknown = bash("kill \"$TARGET\"");
        for mode in [PermissionMode::DangerFullAccess, PermissionMode::WorkspaceWrite, PermissionMode::Allow] {
            let mut person = Person::saying(true);
            match PermissionPolicy::new(mode).authorize("bash", &unknown, Some(&mut person)) {
                PermissionOutcome::Deny { reason } => assert!(reason.contains("yes"), "{mode:?}: {reason}"),
                PermissionOutcome::Allow => panic!("{mode:?}: a worker's pane allowed an unasked kill"),
            }
            assert!(person.asked.is_empty(), "{mode:?}: nobody watches a worker's pane, so nobody is asked");
        }
    }

    /* ---- what a refusal says this session may still end (t-82854) ------------------- */

    /// A kill whose pid is known only when it runs: asked of a person, with no
    /// table read and no process aimed at.
    const UNKNOWN: &str = "kill \"$TARGET\"";
    /// Said to a session that carries a pane mark: a job it set loose is still
    /// its own, since the mark goes with it.
    const LOOSE_IS_OWN: &str = "even once the shell that started it is gone";
    /// Said to a session with no pane mark: only what still runs under it.
    const TREE_ONLY: &str = "while it still runs under this session";
    /// `echo $!` and a job's own number mean nothing in a later command.
    const SAME_COMMAND: &str = "work only within the command that started it";

    /// What a session that runs calls unasked is told it may still end
    /// itself (t-82721 P3-1, P3-2): with a pane mark, a job it set loose too,
    /// save one started from macOS's own programs; without one, only what
    /// still runs under it. `echo $!` is told to be of use in the same
    /// command only.
    #[test]
    fn kill_guard_a_refusal_unasked_says_what_this_session_may_end_with_a_pane_mark_and_without() {
        let env = GuardEnv::hold();
        let window = Bait::start(&env, 46);
        env.window(window.pid());
        let refused = || match PermissionPolicy::new(PermissionMode::DangerFullAccess).authorize(
            "bash",
            &bash(UNKNOWN),
            None,
        ) {
            PermissionOutcome::Deny { reason } => reason,
            PermissionOutcome::Allow => panic!("full access ran a kill no person said yes to"),
        };
        let unmarked = refused();
        assert!(unmarked.contains("Nobody was asked"), "{unmarked}");
        assert!(unmarked.contains(TREE_ONLY), "{unmarked}");
        assert!(!unmarked.contains(LOOSE_IS_OWN), "{unmarked}");
        assert!(unmarked.contains(SAME_COMMAND), "{unmarked}");
        env.marked_window(window.pid(), "term-9");
        let marked = refused();
        assert!(marked.contains(LOOSE_IS_OWN), "{marked}");
        assert!(marked.contains("macOS's own programs"), "{marked}");
        assert!(!marked.contains(TREE_ONLY), "{marked}");
        assert!(marked.contains(SAME_COMMAND), "{marked}");
    }

    /// A sub-agent's or a Workflow helper's bash has nobody to ask, so a kill
    /// that needs a person's yes is refused there in every mode; the refusal
    /// says what the session may still end itself, and does not say the
    /// session runs calls unasked, which this layer does not know
    /// (t-82721 P3-4).
    #[test]
    fn kill_guard_check_bash_says_what_this_session_may_end_itself() {
        let env = GuardEnv::hold();
        let window = Bait::start(&env, 47);
        env.window(window.pid());
        for mode in EVERY_MODE {
            match PermissionEnforcer::new(PermissionPolicy::new(mode)).check_bash(UNKNOWN) {
                EnforcementResult::Denied { reason, .. } => {
                    assert!(reason.contains("no person said yes"), "{mode:?}: {reason}");
                    assert!(reason.contains(TREE_ONLY), "{mode:?}: {reason}");
                    assert!(reason.contains(SAME_COMMAND), "{mode:?}: {reason}");
                    assert!(!reason.contains("runs calls without asking"), "{mode:?}: {reason}");
                }
                EnforcementResult::Allowed => panic!("{mode:?} let an unasked kill through"),
            }
        }
    }

    /// A hook that allows the call is no person's yes (t-82721 P3-6a): in no
    /// mode does it run a kill that needs one. Where the mode would run the
    /// call unasked, the kill is refused with what to do instead and nobody
    /// is asked; where the mode would ask, the person is still asked, once;
    /// read-only refuses it as it refuses any such call, asking nobody.
    #[test]
    fn kill_guard_a_hooks_allow_is_no_persons_yes() {
        let env = GuardEnv::hold();
        let window = Bait::start(&env, 48);
        env.window(window.pid());
        let hook = PermissionContext::new(Some(PermissionOverride::Allow), None);
        for mode in EVERY_MODE {
            let mut no = Person::saying(false);
            let PermissionOutcome::Deny { reason } = PermissionPolicy::new(mode)
                .authorize_with_context("bash", &bash(UNKNOWN), &hook, Some(&mut no))
            else {
                panic!("{mode:?}: a hook's allow ran a kill no person said yes to");
            };
            match mode {
                PermissionMode::DangerFullAccess | PermissionMode::Allow => {
                    assert!(reason.contains("no person said yes"), "{mode:?}: {reason}");
                    assert!(reason.contains(SAME_COMMAND), "{mode:?}: {reason}");
                    assert!(
                        no.asked.is_empty(),
                        "{mode:?}: nobody is asked where the mode runs the call unasked"
                    );
                }
                PermissionMode::WorkspaceWrite | PermissionMode::Prompt => {
                    assert_eq!(no.asked.len(), 1, "{mode:?}: the mode would ask, so the person is asked once");
                    assert!(no.asked[0].person_only, "{mode:?}: only a person may answer it");
                }
                PermissionMode::ReadOnly => {
                    assert!(
                        reason.contains("requires danger-full-access permission"),
                        "{mode:?}: the mode's own refusal: {reason}"
                    );
                    assert!(no.asked.is_empty(), "{mode:?}: read-only runs no such call, so nobody is asked");
                }
            }
        }
    }

    /// Full access with an ask rule for the call, or a hook that asks, puts a
    /// kill that needs a person's yes to the person once, as a question only
    /// a person may answer (t-82721 P3-6b).
    #[test]
    fn kill_guard_full_access_with_an_ask_rule_or_a_hooks_ask_asks_the_person_once() {
        let env = GuardEnv::hold();
        let window = Bait::start(&env, 49);
        env.window(window.pid());
        let ask_rule = crate::RuntimePermissionRuleConfig::new(
            Vec::new(),
            Vec::new(),
            vec!["bash(kill:*)".to_string()],
        );
        let asking = [
            (
                "an ask rule",
                PermissionPolicy::new(PermissionMode::DangerFullAccess)
                    .with_permission_rules(&ask_rule),
                PermissionContext::default(),
            ),
            (
                "a hook's ask",
                PermissionPolicy::new(PermissionMode::DangerFullAccess),
                PermissionContext::new(Some(PermissionOverride::Ask), None),
            ),
        ];
        for (what, policy, context) in &asking {
            let mut yes = Person::saying(true);
            assert_eq!(
                policy.authorize_with_context("bash", &bash(UNKNOWN), context, Some(&mut yes)),
                PermissionOutcome::Allow,
                "{what}: the person said yes"
            );
            assert_eq!(yes.asked.len(), 1, "{what}: one question");
            assert!(yes.asked[0].person_only, "{what}: only a person may answer it");
            let mut no = Person::saying(false);
            assert!(
                matches!(
                    policy.authorize_with_context("bash", &bash(UNKNOWN), context, Some(&mut no)),
                    PermissionOutcome::Deny { .. }
                ),
                "{what}: the person said no"
            );
        }
    }

    /* ---- however long `ps` takes ------------------------------------------------------ */

    /// How late [`SlowPs`] answers, in seconds: well past the 800 ms zo gives
    /// `ps` ([`zerocode_core::shell_rule::kill::TABLE_READ_WALL`]).
    const PS_DELAY_SECS: u32 = 2;

    /// A `ps` that answers [`PS_DELAY_SECS`] late, first on `PATH` until
    /// dropped — the guard's `ps` under a release lane's load, which took up
    /// to 9.8 s at load 22 (t-83535). It changes `PATH` only while the env
    /// lock is held, and its folder carries no marker, so a `ps` the guard gave
    /// up on is no bait left running.
    struct SlowPs<'env> {
        dir: std::path::PathBuf,
        path: Option<std::ffi::OsString>,
        _env: PhantomData<&'env GuardEnv>,
    }

    impl<'env> SlowPs<'env> {
        fn start(_env: &'env GuardEnv) -> Self {
            use std::os::unix::fs::PermissionsExt as _;

            let path = std::env::var_os("PATH");
            let real = path
                .iter()
                .flat_map(std::env::split_paths)
                .map(|dir| dir.join("ps"))
                .find(|ps| ps.is_file())
                .expect("a ps on PATH");
            let dir = std::env::temp_dir().join(format!("zo-kill-guard-slow-ps-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("the slow ps's folder");
            let ps = dir.join("ps");
            let script = format!(
                "#!/bin/sh\n/bin/sleep {PS_DELAY_SECS}\nexec '{}' \"$@\"\n",
                real.display()
            );
            std::fs::write(&ps, script).expect("the slow ps is written");
            std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755)).expect("the slow ps runs");
            let mut dirs = vec![dir.clone()];
            dirs.extend(path.iter().flat_map(std::env::split_paths));
            std::env::set_var("PATH", std::env::join_paths(dirs).expect("a PATH"));
            Self {
                dir,
                path,
                _env: PhantomData,
            }
        }
    }

    impl Drop for SlowPs<'_> {
        fn drop(&mut self) {
            match self.path.take() {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A signal to the window is refused as the window's however late `ps`
    /// answers, not taken for a question nobody asked: `check_bash`, the
    /// policy and the bash chokepoint judge real processes here, and a test's
    /// answer must not turn on how fast `ps` is (t-81315). A `ps` slower than
    /// 800 ms once let the chokepoint run a kill at the window (t-83535).
    #[test]
    fn kill_guard_refuses_the_window_however_late_ps_answers() {
        let env = GuardEnv::hold();
        let mut bait = Bait::start(&env, 60);
        env.window(bait.pid());
        let _slow = SlowPs::start(&env);
        let pid = bait.pid().to_string();
        let aimed = format!("kill {pid}");
        match PermissionEnforcer::new(PermissionPolicy::new(PermissionMode::DangerFullAccess)).check_bash(&aimed) {
            EnforcementResult::Denied { reason, .. } => assert!(
                reason.starts_with("refused to run:") && reason.contains(&pid),
                "check_bash: {reason}"
            ),
            EnforcementResult::Allowed => panic!("check_bash let `{aimed}` through"),
        }
        match PermissionPolicy::new(PermissionMode::DangerFullAccess).authorize("bash", &bash(&aimed), None) {
            PermissionOutcome::Deny { reason } => assert!(
                reason.starts_with("refused to run:") && reason.contains(&pid),
                "authorize: {reason}"
            ),
            PermissionOutcome::Allow => panic!("authorize let `{aimed}` through"),
        }
        match run(&aimed) {
            Ok(output) => panic!("`{aimed}` ran: {output:?}"),
            Err(refused) => assert!(refused.to_string().contains(&pid), "execute_bash: {refused}"),
        }
        assert!(bait.alive(), "the refusals left the bait alive");
    }

    /* ---- one reading of the processes for one call (t-84911) --------------------------- */

    /// How long a gated [`NotingPs`] waits for its gate, in tenths of a
    /// second, before it lists anyway. Only how long a red run takes turns on
    /// it: a test reads which of the two happened, never a time.
    const GATE_WAIT_TENTHS: u32 = 100;

    /// A `ps` first on `PATH` until dropped, which notes in its log each time
    /// the guard runs it and then lists as the real `ps` does — how often the
    /// guard read the processes for one call. A gated one first marks that it
    /// started and waits for its gate file, up to [`GATE_WAIT_TENTHS`], and
    /// notes `opened` if the gate appeared and `waited-out` if not: whether
    /// other work of the turn ran while the guard read. Like [`SlowPs`], it
    /// changes `PATH` only while the env lock is held, and its folder carries
    /// no marker.
    struct NotingPs<'env> {
        dir: std::path::PathBuf,
        path: Option<std::ffi::OsString>,
        _env: PhantomData<&'env GuardEnv>,
    }

    impl<'env> NotingPs<'env> {
        fn start(_env: &'env GuardEnv, gated: bool) -> Self {
            use std::os::unix::fs::PermissionsExt as _;

            let path = std::env::var_os("PATH");
            let real = path
                .iter()
                .flat_map(std::env::split_paths)
                .map(|dir| dir.join("ps"))
                .find(|ps| ps.is_file())
                .expect("a ps on PATH");
            let dir = std::env::temp_dir().join(format!("zo-kill-guard-noting-ps-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("the noting ps's folder");
            let quoted = |name: &str| format!("'{}'", dir.join(name).display());
            let (log, started, gate) = (quoted("log"), quoted("started"), quoted("gate"));
            let note = if gated {
                format!(
                    ": > {started}\ni=0\nwhile [ ! -e {gate} ] && [ \"$i\" -lt {GATE_WAIT_TENTHS} ]; do /bin/sleep 0.1; i=$((i+1)); done\n\
                     if [ -e {gate} ]; then echo opened >> {log}; else echo waited-out >> {log}; fi\n"
                )
            } else {
                format!("echo read >> {log}\n")
            };
            let ps = dir.join("ps");
            std::fs::write(&ps, format!("#!/bin/sh\n{note}exec '{}' \"$@\"\n", real.display()))
                .expect("the noting ps is written");
            std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755)).expect("the noting ps runs");
            let mut dirs = vec![dir.clone()];
            dirs.extend(path.iter().flat_map(std::env::split_paths));
            std::env::set_var("PATH", std::env::join_paths(dirs).expect("a PATH"));
            Self {
                dir,
                path,
                _env: PhantomData,
            }
        }

        /// What it noted, one line a run.
        fn reads(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.join("log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }

        fn started(&self) -> std::path::PathBuf {
            self.dir.join("started")
        }

        fn gate(&self) -> std::path::PathBuf {
            self.dir.join("gate")
        }
    }

    impl Drop for NotingPs<'_> {
        fn drop(&mut self) {
            match self.path.take() {
                Some(path) => std::env::set_var("PATH", path),
                None => std::env::remove_var("PATH"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A model that asks for one `Bash` call, then ends its turn.
    struct OneBashCall(String);

    impl crate::conversation::AsyncApiClient for OneBashCall {
        fn stream_async<'a>(
            &'a self,
            request: crate::conversation::ApiRequest,
            _render_tx: tokio::sync::mpsc::Sender<crate::message_stream::types::RenderBlock>,
            _text_block_id: crate::message_stream::types::BlockId,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<Vec<crate::conversation::AssistantEvent>, crate::conversation::RuntimeError>,
                    > + Send
                    + 'a,
            >,
        > {
            use crate::conversation::AssistantEvent;
            let answered = request
                .messages
                .iter()
                .any(|message| message.role == crate::session::MessageRole::Tool);
            let command = self.0.clone();
            Box::pin(async move {
                Ok(if answered {
                    vec![AssistantEvent::TextDelta("done".to_string()), AssistantEvent::MessageStop]
                } else {
                    vec![
                        AssistantEvent::ToolUse {
                            id: "kill-guard-one-reading".to_string(),
                            name: "Bash".to_string(),
                            input: bash(&command),
                        },
                        AssistantEvent::MessageStop,
                    ]
                })
            })
        }
    }

    /// The blocking model the runtime is built with; the turn streams.
    struct Ends;

    impl crate::conversation::ApiClient for Ends {
        fn stream(
            &mut self,
            _request: crate::conversation::ApiRequest,
        ) -> Result<Vec<crate::conversation::AssistantEvent>, crate::conversation::RuntimeError> {
            Ok(vec![crate::conversation::AssistantEvent::MessageStop])
        }
    }

    /// Nobody at the window: a question would be a test's mistake.
    struct NobodyThere;

    impl crate::permission::PermissionPrompter for NobodyThere {
        fn decide<'a>(
            &'a self,
            _request: crate::permission::PermissionRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<crate::permission::PermissionDecision, crate::permission::PermissionError>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async { panic!("a kill zo may send needs nobody's yes") })
        }
    }

    /// The tool results of one streaming turn on `rt` in which the model
    /// calls `Bash` with `command`, zo in full access and the tool running it
    /// at the bash chokepoint ([`run`]) — beside `other`, a task of the same
    /// runtime, as the window's render loop is.
    fn a_turn_calling(
        rt: &tokio::runtime::Runtime,
        command: &str,
        other: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Vec<String> {
        let executor = crate::conversation::StaticToolExecutor::new().register("Bash", |input| {
            let input: serde_json::Value = serde_json::from_str(input).expect("the call's input");
            let command = input["command"].as_str().expect("a command");
            run(command)
                .map(|output| serde_json::to_string(&output).expect("the output"))
                .map_err(|refused| crate::ToolError::new(refused.to_string()))
        });
        let mut runtime = crate::conversation::ConversationRuntime::new(
            crate::session::Session::new(),
            Ends,
            executor,
            PermissionPolicy::new(PermissionMode::DangerFullAccess),
            vec!["system".to_string()],
        )
        .with_async_api_client(std::sync::Arc::new(OneBashCall(command.to_string())));
        let summary = rt.block_on(async {
            let other = tokio::spawn(other);
            let (render_tx, mut render_rx) = tokio::sync::mpsc::channel(64);
            tokio::spawn(async move { while render_rx.recv().await.is_some() {} });
            let summary = runtime
                .run_turn_streaming_maybe_deep("go", Vec::new(), render_tx, std::sync::Arc::new(NobodyThere))
                .await
                .expect("the turn ends");
            other.await.expect("the other task ends");
            summary
        });
        summary
            .tool_results
            .iter()
            .map(|message| match &message.blocks[0] {
                crate::session::ContentBlock::ToolResult { output, .. } => output.clone(),
                other => panic!("a tool result, not {other:?}"),
            })
            .collect()
    }

    /// One kill call in a turn is judged on one reading of the processes:
    /// the policy and the bash chokepoint each read them before, and under a
    /// release lane's load each reading could take seconds (t-84911). The
    /// call sends a running `sleep` a signal it ignores (`kill -CONT`), to zo's own child.
    #[test]
    fn kill_guard_a_turn_reads_the_processes_once_for_a_kill_call() {
        let env = GuardEnv::hold();
        let window = Bait::start(&env, 70);
        let mut own = Bait::start(&env, 71);
        env.window(window.pid());
        let ps = NotingPs::start(&env, false);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime");
        let outputs = a_turn_calling(&rt, &format!("kill -CONT {}", own.pid()), async {});
        let reads = ps.reads();
        assert_eq!(outputs.len(), 1, "{outputs:?}");
        assert!(!outputs[0].contains("refused"), "zo may signal its own child: {}", outputs[0]);
        assert_eq!(reads.len(), 1, "the processes were read {} times for one call", reads.len());
        assert!(own.alive(), "`kill -CONT` ends nothing");
    }

    /// While the guard reads the processes, the turn's runtime goes on with
    /// its other work — the window's render loop and an Esc — as it does
    /// while a pre-tool hook runs (t-84911). The runtime has one thread, and
    /// the `ps` waits for a gate that only another task of it opens: read on
    /// the turn's own thread, the gate stays shut until `ps` gives up.
    #[test]
    fn kill_guard_a_turn_goes_on_while_the_processes_are_read() {
        let env = GuardEnv::hold();
        let window = Bait::start(&env, 72);
        let mut own = Bait::start(&env, 73);
        env.window(window.pid());
        let ps = NotingPs::start(&env, true);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let (started, gate) = (ps.started(), ps.gate());
        let other = async move {
            while !started.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            std::fs::write(&gate, b"").expect("the gate opens");
        };
        let outputs = a_turn_calling(&rt, &format!("kill -CONT {}", own.pid()), other);
        let reads = ps.reads();
        assert_eq!(outputs.len(), 1, "{outputs:?}");
        assert_eq!(
            reads.first().map(String::as_str),
            Some("opened"),
            "the turn's other work waited for the guard's `ps`: {reads:?}"
        );
        assert!(own.alive(), "`kill -CONT` ends nothing");
    }

    /// A judgment handed to a command's way stands for that command alone,
    /// and only while fresh: a judgment for another command, or one older
    /// than [`super::super::HANDED_JUDGMENT_LIFETIME`] — a tool ran before
    /// this one, a person was asked — is read again, so a signal to the
    /// window is refused however it was judged before (t-84911). The handed
    /// answers here say yes to everything; only this module can make one.
    #[test]
    fn kill_guard_a_handed_judgment_stands_for_its_own_command_while_fresh() {
        use super::super::{for_one_command, judged, Judgment, Shell, HANDED_JUDGMENT_LIFETIME};
        use zerocode_core::shell_rule::kill::KillVerdict;

        let env = GuardEnv::hold();
        let mut window = Bait::start(&env, 74);
        env.window(window.pid());
        let aimed = format!("kill {}", window.pid());
        let says_yes = |command: &str, age: std::time::Duration| Judgment {
            shell: Shell::Bash,
            command: command.to_string(),
            verdict: Some(KillVerdict::Allow),
            read_at: std::time::Instant::now()
                .checked_sub(age)
                .expect("an instant that long ago"),
        };
        let refused = |verdict: Option<KillVerdict>| matches!(verdict, Some(KillVerdict::Deny { .. }));

        assert!(refused(judged(Shell::Bash, &aimed)), "read where nothing was handed");
        assert_eq!(
            for_one_command(Some(says_yes(&aimed, std::time::Duration::ZERO)), || judged(Shell::Bash, &aimed)),
            Some(KillVerdict::Allow),
            "a fresh judgment for the command stands for it"
        );
        assert!(
            refused(for_one_command(Some(says_yes("kill 1", std::time::Duration::ZERO)), || judged(Shell::Bash, &aimed))),
            "a judgment for another command is not this one's"
        );
        let old = HANDED_JUDGMENT_LIFETIME + std::time::Duration::from_secs(1);
        assert!(
            refused(for_one_command(Some(says_yes(&aimed, old)), || judged(Shell::Bash, &aimed))),
            "a judgment older than its lifetime is read again"
        );
        assert!(
            for_one_command(None, || refused(judged(Shell::Bash, &aimed)) && refused(judged(Shell::Bash, &aimed))),
            "the reading a way keeps is the guard's own"
        );
        assert!(window.alive(), "judging ends nothing");
    }

    /// A signal to the window from a turn is refused in the words it always
    /// was, on the one reading the turn made off its task, and nothing runs
    /// (t-84911): the policy refuses it before the call is dispatched.
    #[test]
    fn kill_guard_a_turn_refuses_the_window_on_its_one_reading() {
        let env = GuardEnv::hold();
        let mut window = Bait::start(&env, 75);
        env.window(window.pid());
        let ps = NotingPs::start(&env, false);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime");
        let pid = window.pid().to_string();
        let outputs = a_turn_calling(&rt, &format!("kill {pid}"), async {});
        let reads = ps.reads();
        assert_eq!(outputs.len(), 1, "{outputs:?}");
        assert!(
            outputs[0].contains("refused to run:") && outputs[0].contains(&pid),
            "the turn's refusal: {}",
            outputs[0]
        );
        assert_eq!(reads.len(), 1, "the processes were read {} times for one call", reads.len());
        assert!(window.alive(), "the refusal left the window alive");
    }

    /* ---- the PowerShell tool (t-84973) -------------------------------------------------- */

    /// The `PowerShell` calls that end a process by its pid, aimed at `pid`.
    /// `-WhatIf` keeps `Stop-Process` from ending anything were it run.
    fn powershell_ends(pid: &str) -> [String; 4] {
        [
            format!("Stop-Process -Id {pid} -WhatIf"),
            format!("spps {pid} -Force -WhatIf"),
            format!("Get-Process -Id {pid} | Stop-Process -WhatIf"),
            format!("taskkill /PID {pid}"),
        ]
    }

    /// A `PowerShell` call that ends the window is refused as a bash `kill` of
    /// it is, in its words, whatever the mode or a rule says, and nobody is
    /// asked (t-84973): the guard read only the bash tool's calls before, so
    /// `Stop-Process -Id <window>` went through.
    #[test]
    fn kill_guard_authorize_refuses_a_powershell_call_at_the_window_as_bash_is() {
        let env = GuardEnv::hold();
        let mut window = Bait::start(&env, 80);
        env.window(window.pid());
        let pid = window.pid().to_string();
        let bash_says = match PermissionPolicy::new(PermissionMode::DangerFullAccess)
            .authorize("bash", &bash(&format!("kill {pid}")), None)
        {
            PermissionOutcome::Deny { reason } => reason,
            PermissionOutcome::Allow => panic!("bash let `kill {pid}` through"),
        };
        let audit = &bash_says[bash_says.find(". Permission audit:").expect("the guard's audit")..];
        for command in powershell_ends(&pid) {
            for mode in EVERY_MODE {
                let policy = PermissionPolicy::new(mode).with_permission_rules(
                    &crate::RuntimePermissionRuleConfig::new(
                        vec!["PowerShell(*)".to_string()],
                        Vec::new(),
                        Vec::new(),
                    ),
                );
                let mut person = Person::saying(true);
                match policy.authorize("PowerShell", &bash(&command), Some(&mut person)) {
                    PermissionOutcome::Deny { reason } => assert!(
                        reason.starts_with("refused to run:") && reason.contains(&pid) && reason.ends_with(audit),
                        "{mode:?} `{command}`: {reason}"
                    ),
                    PermissionOutcome::Allow => panic!("{mode:?} let `{command}` through"),
                }
                assert!(person.asked.is_empty(), "{mode:?} `{command}`: a refusal is not a question");
            }
        }
        assert!(window.alive(), "judging ends nothing");
    }

    /// A `PowerShell` call whose target only running it tells is a question,
    /// as bash's is: refused unasked where the mode would run it, saying
    /// what to do instead, and put to a person once where the mode asks
    /// (t-84973). One that ends zo's own child goes ahead.
    #[test]
    fn kill_guard_authorize_asks_about_a_powershell_call_as_about_bash() {
        let env = GuardEnv::hold();
        let window = Bait::start(&env, 81);
        let mut own = Bait::start(&env, 82);
        env.window(window.pid());
        let full = PermissionPolicy::new(PermissionMode::DangerFullAccess);
        let unasked = |tool: &str, command: &str| match full.authorize(tool, &bash(command), None) {
            PermissionOutcome::Deny { reason } => reason,
            PermissionOutcome::Allow => panic!("full access let `{command}` through unasked"),
        };
        let bash_says = unasked("bash", UNKNOWN);
        for command in ["Stop-Process -Id $target -WhatIf", "(Get-Process -Id $target).Kill()"] {
            let says = unasked("PowerShell", command);
            assert!(
                says.starts_with("refused, since no person said yes to it:") && says.contains("Nobody was asked"),
                "`{command}`: {says}"
            );
            assert_eq!(
                says[says.find(". Permission audit:").expect("the audit")..],
                bash_says[bash_says.find(". Permission audit:").expect("the audit")..],
                "`{command}` is refused in bash's words"
            );
        }
        let mut person = Person::saying(true);
        let asks = PermissionPolicy::new(PermissionMode::WorkspaceWrite);
        assert_eq!(
            asks.authorize("PowerShell", &bash("Stop-Process -Id $target -WhatIf"), Some(&mut person)),
            PermissionOutcome::Allow
        );
        assert_eq!(person.asked.len(), 1, "the person is asked once");
        assert_eq!(
            full.authorize("PowerShell", &bash(&format!("Stop-Process -Id {} -WhatIf", own.pid())), None),
            PermissionOutcome::Allow,
            "zo may end its own child"
        );
        assert!(own.alive(), "judging ends nothing");
    }
}
