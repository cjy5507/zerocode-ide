//! The kill guard as zo meets it (t-80732). The judgment is read off a
//! process table handed in wherever it can be; where a real run is under
//! test, the only process it may hit is a `sleep` the test started itself,
//! and whether the bait is still alive is the proof of a refusal.

use std::cell::Cell;

use zerocode_core::shell_rule::kill::{KillVerdict, ProcessTable, WINDOW_PID_ENV};

use super::{judge, judge_with, summoned_worker};

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
    use std::io::Read as _;
    use std::os::fd::{AsRawFd as _, OwnedFd};
    use std::process::{Child, Command, Stdio};

    use zerocode_core::agent_teams::{TEAM_LEADER_VAR, TEAM_PANE_VAR};
    use zerocode_core::hook::PANE_KEY_ENV;
    use zerocode_core::shell_rule::kill::WINDOW_PID_ENV;

    use crate::permission_enforcer::{EnforcementResult, PermissionEnforcer};
    use crate::sandbox::FilesystemIsolationMode;
    use crate::{
        execute_bash, BashCommandInput, PermissionMode, PermissionOutcome, PermissionPolicy,
        PermissionPromptDecision, PermissionPrompter, PermissionRequest,
    };

    /// A `sleep` this test started, whose duration no other process carries, so
    /// a pattern naming it can only ever match it.
    struct Bait {
        child: Child,
        marker: String,
    }

    impl Bait {
        fn start(n: u32) -> Self {
            let marker = format!("86400.{}{n}", std::process::id());
            let child = Command::new("sleep")
                .arg(&marker)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("the bait starts");
            Self { child, marker }
        }

        fn pid(&self) -> u32 {
            self.child.id()
        }

        fn alive(&mut self) -> bool {
            self.child.try_wait().expect("the bait can be asked").is_none()
        }
    }

    impl Drop for Bait {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// A `sleep` this test set loose: a shell started it in the background
    /// and exited, so it hangs under pid 1, outside this process's tree — a
    /// process zo started that is no longer its child. It carries no pane
    /// key, so no pane can claim it as its own job. Ended when dropped.
    struct Orphan {
        pid: u32,
        marker: String,
    }

    impl Orphan {
        fn start(n: u32) -> Self {
            let marker = format!("86400.{}{n}", std::process::id());
            let output = Command::new("sh")
                .arg("-c")
                .arg(format!("sleep {marker} </dev/null >/dev/null 2>&1 & echo $!"))
                .env_remove(PANE_KEY_ENV)
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .expect("the orphan starts");
            let pid = String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse()
                .expect("the shell names the orphan's pid");
            Self { pid, marker }
        }

        fn alive(&self) -> bool {
            Command::new("kill")
                .args(["-0", &self.pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        }
    }

    impl Drop for Orphan {
        fn drop(&mut self) {
            let _ = Command::new("kill").arg(self.pid.to_string()).status();
        }
    }

    /// The environment the guard reads, held under the crate's env lock and put
    /// back when dropped: the window this process runs in is `window`, and it
    /// runs in the pane `pane` of a team led from `leader` — so a test's answer
    /// never depends on the pane the suite happens to run in.
    struct GuardEnv {
        before: Vec<(&'static str, Option<String>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl GuardEnv {
        fn set(window: u32, pane: Option<&str>, leader: Option<&str>) -> Self {
            let lock = crate::test_env_lock();
            let window = window.to_string();
            let wanted = [
                (WINDOW_PID_ENV, Some(window.as_str())),
                (TEAM_PANE_VAR, pane),
                (TEAM_LEADER_VAR, leader),
            ];
            let before = wanted
                .iter()
                .map(|(name, _)| (*name, std::env::var(name).ok()))
                .collect();
            for (name, value) in wanted {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
            Self {
                before,
                _lock: lock,
            }
        }

        /// The window is `pid`, in a pane a person opened.
        fn window(pid: u32) -> Self {
            Self::set(pid, None, None)
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
        }
    }

    /// How long the read end of a pipe is given to see its end once its last
    /// write end here is closed.
    const SETTLE: std::time::Duration = std::time::Duration::from_secs(10);

    /// Set in a copy of this test binary that runs one test of this module
    /// alone ([`run_alone`]).
    const ALONE_ENV: &str = "ZO_KILL_GUARD_TEST_ALONE";

    /// This test binary, started to run only `test` of this module.
    fn this_binary_running(test: &str) -> Command {
        let module = module_path!()
            .split_once("::")
            .map_or(module_path!(), |(_crate, path)| path);
        let mut command = Command::new(std::env::current_exe().expect("this test binary"));
        command
            .arg(format!("{module}::{test}"))
            .args(["--exact", "--nocapture", "--test-threads=1"]);
        command
    }

    /// Runs `test` alone in a copy of this binary, where no other test's child
    /// can take along a descriptor it leaves open, and asserts it passed.
    fn run_alone(test: &str) {
        let output = this_binary_running(test)
            .env(ALONE_ENV, "1")
            .stdin(Stdio::null())
            .output()
            .expect("the test runs alone");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains(" 1 passed;"),
            "{test}, run alone:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Which of `ends` still has a writer [`SETTLE`] after the call. Each is
    /// the read end of a pipe whose write ends here are closed, named for what
    /// started while it was open.
    fn still_written(ends: Vec<(&'static str, OwnedFd)>) -> Vec<&'static str> {
        use nix::fcntl::{fcntl, FcntlArg, OFlag};
        let mut open: Vec<(&str, std::fs::File)> = ends
            .into_iter()
            .map(|(name, end)| {
                fcntl(end.as_raw_fd(), FcntlArg::F_SETFL(OFlag::O_NONBLOCK))
                    .expect("the read end stops blocking");
                (name, std::fs::File::from(end))
            })
            .collect();
        let since = std::time::Instant::now();
        loop {
            open.retain_mut(|(_, end)| !matches!(end.read(&mut [0; 1]), Ok(0)));
            if open.is_empty() || since.elapsed() >= SETTLE {
                return open.into_iter().map(|(name, _)| name).collect();
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
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
        let (bait_end, write) = nix::unistd::pipe().expect("a pipe a child takes along");
        let _bait = Bait::start(50);
        drop(write);
        let (orphan_end, write) = nix::unistd::pipe().expect("a pipe a child takes along");
        let _orphan = Orphan::start(51);
        drop(write);
        let held = still_written(vec![("the bait", bait_end), ("the orphan", orphan_end)]);
        assert!(
            held.is_empty(),
            "{held:?} took along the write end of a pipe nobody gave them"
        );
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
        let mut bait = Bait::start(1);
        let _window = GuardEnv::window(bait.pid());
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
        let window = Bait::start(2);
        let mut own = Bait::start(3);
        let _window = GuardEnv::window(window.pid());
        let output = run(&format!("pkill -f '[{}]{}'", &own.marker[..1], &own.marker[1..]))
            .expect("a signal to zo's own child runs");
        assert_eq!(output.return_code_interpretation, None, "{output:?}");
        let ended = std::time::Instant::now();
        while own.alive() {
            assert!(
                ended.elapsed() < std::time::Duration::from_secs(10),
                "zo's own child ended"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn kill_guard_execute_bash_refuses_a_question_in_a_summoned_worker_pane() {
        let window = Bait::start(7);
        let mut bait = Bait::start(8);
        let pid_file = std::env::temp_dir().join(format!("{}.pid", bait.marker));
        std::fs::write(&pid_file, bait.pid().to_string()).expect("the pid file is written");
        // A pid read at run time is a question, and the paths that reach the
        // chokepoint past the permission layer (a deep_gate check, a `!` line)
        // never put it to anyone.
        let command = format!("kill $(cat {})", pid_file.display());
        {
            let _worker = GuardEnv::set(window.pid(), Some("%3"), Some("%1"));
            let refused = match run(&command) {
                Ok(output) => panic!("`{command}` ran in a worker's pane: {output:?}"),
                Err(refused) => refused,
            };
            assert_eq!(refused.kind(), std::io::ErrorKind::PermissionDenied, "{refused}");
            assert!(refused.to_string().contains("no person said yes"), "{refused}");
            assert!(bait.alive(), "the refusal left the bait alive");
        }
        // A pane a person opened keeps the question the permission layer's.
        let _person = GuardEnv::window(window.pid());
        let output = run(&command).expect("a question a person could answer runs");
        assert_eq!(output.return_code_interpretation, None, "{output:?}");
        let ended = std::time::Instant::now();
        while bait.alive() {
            assert!(
                ended.elapsed() < std::time::Duration::from_secs(10),
                "the bait ended"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
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
        let bait = Bait::start(4);
        let _window = GuardEnv::window(bait.pid());
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
        let bait = Bait::start(5);
        let _window = GuardEnv::window(bait.pid());
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
        let bait = Bait::start(6);
        let _window = GuardEnv::window(bait.pid());
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
        let bait = Bait::start(0);
        let _window = GuardEnv::window(bait.pid());
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

    /// Where the mode or a rule would run the call without asking — full
    /// access, the allow mode, an allow rule for it — a kill that needs a
    /// person's yes is refused unasked, as the window's hook refuses it
    /// (t-82420): a question there is one nobody expects, and an unanswered
    /// one stalls the turn. The refusal says what to do instead. The targets
    /// are a process this test set loose, by its pid and by a pid read at run
    /// time.
    #[test]
    fn kill_guard_authorize_refuses_unasked_where_the_mode_or_a_rule_would_run_the_call() {
        let window = Bait::start(40);
        let orphan = Orphan::start(41);
        let pid_file = std::env::temp_dir().join(format!("{}.pid", orphan.marker));
        std::fs::write(&pid_file, orphan.pid.to_string()).expect("the pid file is written");
        let _window = GuardEnv::window(window.pid());
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
        let window = Bait::start(43);
        let own = Bait::start(44);
        let orphan = Orphan::start(45);
        let _window = GuardEnv::window(window.pid());

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
        let bait = Bait::start(9);
        let _env = GuardEnv::set(bait.pid(), Some("%3"), Some("%1"));
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
}
