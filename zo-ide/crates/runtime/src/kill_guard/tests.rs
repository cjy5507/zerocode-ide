//! The kill guard as zo meets it (t-80732). The judgment is read off a
//! process table handed in wherever it can be; where a real run is under
//! test, the only process it may hit is a `sleep` the test started itself,
//! and whether the bait is still alive is the proof of a refusal.

use std::cell::Cell;
use std::process::{Child, Command, Stdio};

use zerocode_core::agent_teams::{TEAM_LEADER_VAR, TEAM_PANE_VAR};
use zerocode_core::shell_rule::kill::{KillVerdict, ProcessTable, WINDOW_PID_ENV};

use super::{judge, judge_with, summoned_worker};
use crate::permission_enforcer::{EnforcementResult, PermissionEnforcer};
use crate::sandbox::FilesystemIsolationMode;
use crate::{
    execute_bash, BashCommandInput, PermissionMode, PermissionOutcome, PermissionPolicy,
    PermissionPromptDecision, PermissionPrompter, PermissionRequest,
};

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

/* ---- a real run: a bait the test started stands for the window ------------------ */

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

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
#[test]
fn kill_guard_authorize_asks_a_person_once_and_refuses_when_nobody_can_answer() {
    let bait = Bait::start(6);
    let _window = GuardEnv::window(bait.pid());
    let unknown = bash("kill \"$TARGET\"");
    for mode in [
        PermissionMode::WorkspaceWrite,
        PermissionMode::DangerFullAccess,
        PermissionMode::Prompt,
        PermissionMode::Allow,
    ] {
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

#[test]
fn kill_guard_a_summoned_worker_pane_has_nobody_to_ask() {
    // As the ledger sets them: a coordinator's pane leads its own team, a
    // worker's pane is another, and a pane a person opened names neither.
    assert!(summoned_worker(Some("%3"), Some("%1")), "a worker's pane");
    assert!(!summoned_worker(Some("%1"), Some("%1")), "the coordinator's own pane");
    assert!(!summoned_worker(None, None), "a pane a person opened");
    assert!(!summoned_worker(None, Some("%1")));
}

#[cfg(unix)]
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
