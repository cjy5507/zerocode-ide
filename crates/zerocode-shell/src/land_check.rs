//! 착지 전 점검 (t-34501 3단계, t-42447) — 창의 반쪽.
//!
//! 코디가 `land-check`로 부른 과업의 머리를, 비교 ref 위에 만든 **임시 작업 폴더**에서 합쳐 본다.
//! 충돌이면 파일 이름을 적고, 아니면 프로젝트 설정의 점검 명령을 그 폴더에서 돌려 rc와 로그를
//! 남긴다. 결과는 원장의 편지(`land_check`)로 간다 — 이 파일은 원장에 직접 쓰지 않고 [`Sink`]에
//! 넘긴다.
//!
//! 지키는 것:
//!   - 워커의 체크아웃은 읽지 않는다. 공용 `.git`을 찾는 데만 쓴다.
//!   - 임시 폴더는 `<orchestration 데이터>/land-check/work/<check>` 하나뿐이고, 이름은 원장이
//!     알려 준 check id다. 지우는 것도 그 이름과 행이 맞는 진짜 폴더뿐이다. 링크는 따라가지 않는다.
//!   - 점검 명령은 프로젝트 설정에서만 온다. 인자로 받지 않는다.
//!   - 통과(`passed`)는 rc 0인 점검이 끝까지 돌았을 때와 `--record --rc 0`일 때만 나온다.
//!     `merged`(점검 명령이 없음)·`prepared`·`started`는 통과가 아니다.
//!   - 준비된 점검의 행은 `rows/<check>.json`에 있다. 기록은 그 행과 맞을 때만 받는다.
//!
//! 기한: git 부분의 답은 [`ANSWER_BUDGET`] 안에 준다. 그 안에 끝나지 않으면 `started`로 답하고,
//! 판정은 같은 편지로 온다. 점검 명령은 이 창의 배경 스레드에서 돈다.

use super::*;

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant, SystemTime};

use crate::worktree_landing::resolve_landing_base;
use serde_json::json;
use zerocode_core::host::{Host, Within};
use zerocode_core::orchestration::land_check::{
    CHECK_PREFIX, LandCheckAsk, LandCheckReceipt, is_check_id,
};

/// How long the verb waits for the first answer before it says `started` and goes on alone.
pub(crate) const ANSWER_BUDGET: Duration = Duration::from_secs(7);

/// The limit a project gets when it names none for its check command.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1800);

/// The budget of one git call. The same as the evidence road's, so no git the window asks is
/// allowed to outlive the others.
const GIT_BUDGET: Duration = Duration::from_secs(8);

/// How long a check the window is running may hold its folder: the git calls, the check's limit
/// and slack for the rest. A row past this is somebody's crash, not somebody's check.
const LEASE_SLACK_MS: i64 = 10 * 60 * 1000;

/// How long a prepared folder waits for its record before the next check takes it down.
const PREPARED_TTL_MS: i64 = 24 * 60 * 60 * 1000;

/// How long a log is kept after its check ended.
const LOG_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// How many clashing file names a letter names; the total is always the true count.
const CONFLICT_NAMES: usize = 20;

/// The extension of the script a check's command is written to beside its log, where the shell
/// takes a file (cmd); it goes when the check ends.
const SCRIPT_EXTENSION: &str = "cmd";

/// How often the window looks at a running check's exit.
const POLL: Duration = Duration::from_millis(50);

const LAND_CHECK_DIR: &str = "land-check";
const WORK_DIR: &str = "work";
const ROWS_DIR: &str = "rows";
const LOGS_DIR: &str = "logs";

/// The one place a check's folders, prepared rows and logs live: `<orchestration data>/land-check`.
#[derive(Debug, Clone)]
pub(crate) struct Store {
    root: PathBuf,
}

impl Store {
    /// Make (or find) the store under the orchestration data root and answer its canonical path.
    pub(crate) fn at(data_root: &Path) -> Result<Store, String> {
        let root = data_root.join(LAND_CHECK_DIR);
        for dir in [WORK_DIR, ROWS_DIR, LOGS_DIR] {
            fs::create_dir_all(root.join(dir))
                .map_err(|why| format!("the land-check folder could not be made: {why}"))?;
        }
        let root = fs::canonicalize(&root)
            .map_err(|why| format!("the land-check folder could not be named: {why}"))?;
        Ok(Store {
            root: without_verbatim_prefix(root),
        })
    }

    /// The store's root, for the tests that name the folders they expect under it.
    #[cfg(test)]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    fn folder_of(&self, check: &str) -> PathBuf {
        self.root.join(WORK_DIR).join(check)
    }

    fn row_of(&self, check: &str) -> PathBuf {
        self.root.join(ROWS_DIR).join(format!("{check}.json"))
    }

    fn log_of(&self, check: &str) -> PathBuf {
        self.root.join(LOGS_DIR).join(format!("{check}.log"))
    }
}

/// How one check runs: the project's command (none means "merge only"), its limit, the compare ref
/// the project pins, and the shell that reads the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Policy {
    pub(crate) command: Option<String>,
    pub(crate) timeout: Duration,
    pub(crate) pinned_base: Option<String>,
    pub(crate) shell: PathBuf,
    /// The budget of each git call the check makes in its folder: [`GIT_BUDGET`], or the shorter
    /// one a test injects to cut a step short.
    pub(crate) git_budget: Duration,
}

impl Policy {
    /// The project's stored words, read the way the settings read them: a blank command is none, a
    /// missing or zero limit is [`DEFAULT_TIMEOUT`], a blank pin is none.
    pub(crate) fn from_settings(
        command: Option<&str>,
        timeout_secs: Option<u64>,
        pinned_base: Option<&str>,
    ) -> Policy {
        Policy {
            command: command
                .map(str::trim)
                .filter(|command| !command.is_empty())
                .map(str::to_string),
            timeout: timeout_secs
                .filter(|secs| *secs > 0)
                .map_or(DEFAULT_TIMEOUT, Duration::from_secs),
            pinned_base: pinned_base
                .map(str::trim)
                .filter(|pin| !pin.is_empty())
                .map(str::to_string),
            shell: default_shell(),
            git_budget: GIT_BUDGET,
        }
    }
}

/// The program that reads a check's command: `bash` reads it on stdin, `cmd` runs it as a script
/// beside the log (see [`spawn`]).
fn default_shell() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from("cmd")
    } else {
        PathBuf::from("/bin/bash")
    }
}

/// Where the window hands each receipt of one check's evidence: the ledger's letter road in
/// production, a channel in tests.
pub(crate) type Sink = Arc<dyn Fn(LandCheckReceipt) + Send + Sync>;

/// The stored settings of the repository a check merges in. Set once at boot.
static SETTINGS: OnceLock<Arc<settings::SettingsRepository>> = OnceLock::new();

/// Keep the settings the checks read their command and limit from.
pub(crate) fn hold_settings(repository: Arc<settings::SettingsRepository>) {
    let _ = SETTINGS.set(repository);
}

/// The policy a repository's project stores. A repository with no settings gets none.
fn policy_of(repo: &Path) -> Policy {
    let stored = SETTINGS
        .get()
        .and_then(|repository| {
            project_runtime::stored_project_settings_at(
                repository,
                &project_runtime::project_settings_key(repo),
            )
            .ok()
        })
        .unwrap_or_default();
    Policy::from_settings(
        stored.landing_check_command.as_deref(),
        stored.landing_check_timeout_secs,
        stored.worktree_base_ref.as_deref(),
    )
}

/// Carry out one `land-check` ask from the window's own data root, and answer the JSON the verb
/// prints. Its evidence goes to the ledger as the letter road's.
pub(crate) fn carry(data_root: &Path, ask: &LandCheckAsk, now_ms: i64) -> Result<String, String> {
    let store = Store::at(data_root)?;
    let sink: Sink = Arc::new(|receipt: LandCheckReceipt| {
        crate::orchestration::record_land_check(receipt, crate::now_epoch_ms());
    });
    run(&store, &policy_of, ask, now_ms, &sink)
}

/// Hand one letter's evidence to the sink. The sink sits behind an `Arc`, so it is called through its
/// `&dyn Fn` rather than by the `Arc` itself.
fn letter(sink: &Sink, receipt: LandCheckReceipt) {
    let call: &dyn Fn(LandCheckReceipt) = sink.as_ref();
    call(receipt);
}

/// Carry out one ask. Answers the JSON the verb prints, or the refusal's words. `policy_for` names
/// the policy of the repository the check merges in; `sink` takes each letter's evidence.
pub(crate) fn run(
    store: &Store,
    policy_for: &dyn Fn(&Path) -> Policy,
    ask: &LandCheckAsk,
    now_ms: i64,
    sink: &Sink,
) -> Result<String, String> {
    match ask {
        LandCheckAsk::Merge {
            run,
            task,
            head,
            checkout,
            prepare,
        } => {
            let host = Host::for_workspace(Path::new(checkout));
            let common = git(
                &host,
                Path::new(checkout),
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .map_err(|why| format!("{checkout} is not a git checkout: {why}"))?;
            let repo = repo_root_of(Path::new(&common));
            sweep(store, now_ms, sink);
            let policy = policy_for(repo.as_path());
            let head_oid = commit_of(&host, &repo, head)
                .ok_or_else(|| format!("{head} is not a commit in this repository"))?;
            let base = resolve_landing_base(&host, &repo, policy.pinned_base.as_deref());
            let base_name = base
                .name
                .clone()
                .ok_or("this repository has no compare ref to merge into")?;
            let base_oid = base
                .oid
                .clone()
                .ok_or_else(|| format!("the compare ref {base_name} does not name a commit"))?;
            let check = new_check_id(now_ms);
            let job = Job {
                store: store.clone(),
                repo,
                run: run.clone(),
                task: task.clone(),
                check: check.clone(),
                head: head_oid,
                base_name,
                base_oid,
                folder: store.folder_of(&check),
                log: store.log_of(&check),
                prepare: *prepare,
                policy,
                sink: Arc::clone(sink),
                started_ms: now_ms,
            };
            let (send_answer, first_answer) = mpsc::channel::<String>();
            let folder_text = job.folder.display().to_string();
            let log_text = job.log.display().to_string();
            write_row(&job.store, &running_row(&job, now_ms))?;
            let row_path = job.store.row_of(&job.check);
            let spawned = std::thread::Builder::new()
                .name("land-check".to_string())
                .spawn(move || merge_job(job, send_answer));
            if let Err(why) = spawned {
                // Nothing runs under this row; the next sweep would only say so late.
                let _ = fs::remove_file(row_path);
                return Err(format!("the check could not start: {why}"));
            }
            match first_answer.recv_timeout(ANSWER_BUDGET) {
                Ok(said) => Ok(said),
                Err(mpsc::RecvTimeoutError::Timeout) => Ok(json!({
                    "state": "started",
                    "check": check,
                    "path": folder_text,
                    "log": log_text,
                })
                .to_string()),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    Err("the check stopped before it answered".to_string())
                }
            }
        }
        LandCheckAsk::Record { .. } => record(store, ask, now_ms, sink),
    }
}

/// One merge check's inputs, owned so the check can run after the verb has answered.
struct Job {
    store: Store,
    repo: PathBuf,
    run: String,
    task: String,
    check: String,
    head: String,
    base_name: String,
    base_oid: String,
    folder: PathBuf,
    log: PathBuf,
    prepare: bool,
    policy: Policy,
    sink: Sink,
    started_ms: i64,
}

/// The first answer's kinds, decided inside the check: a merge that conflicts, a merge that was
/// clean, or the git error that stopped it.
enum Merged {
    Clean(String),
    Clash(Vec<String>),
    Broken(String),
}

/// The whole merge check on its own thread: merge, then (when there is one) the command, then the
/// letter and the folder's end. The first answer it sends is what the verb says.
fn merge_job(job: Job, answer: mpsc::Sender<String>) {
    let host = Host::for_workspace(&job.repo);
    let merged = merge_in_folder(&host, &job);
    let mut evidence = base_evidence(&job, crate::now_epoch_ms());
    match merged {
        Merged::Clean(tree) if job.prepare => {
            let row = prepared_row(&job, &tree, crate::now_epoch_ms());
            if write_row(&job.store, &row).is_err() {
                evidence.insert("state".into(), "error".into());
                evidence.insert(
                    "message".into(),
                    "the prepared row could not be written".into(),
                );
                finish(&job, &host, evidence, &answer);
                return;
            }
            let _ = answer.send(
                json!({
                    "state": "prepared",
                    "check": job.check,
                    "task": job.task,
                    "head": job.head,
                    "base": { "ref": job.base_name, "oid": job.base_oid },
                    "tree": tree,
                    "path": job.folder.display().to_string(),
                    "log": job.log.display().to_string(),
                })
                .to_string(),
            );
        }
        Merged::Clean(tree) => {
            evidence.insert("tree".into(), tree.into());
            let Some(command) = job.policy.command.clone() else {
                evidence.insert("state".into(), "merged".into());
                finish(&job, &host, evidence, &answer);
                return;
            };
            let _ = answer.send(
                json!({
                    "state": "started",
                    "check": job.check,
                    "log": job.log.display().to_string(),
                    "timeoutSecs": job.policy.timeout.as_secs(),
                })
                .to_string(),
            );
            let end = run_command(&job, &command);
            end.apply(&mut evidence);
            evidence.insert("log".into(), job.log.display().to_string().into());
            finish(&job, &host, evidence, &answer);
        }
        Merged::Clash(files) => {
            let total = files.len();
            let kept: Vec<String> = files.into_iter().take(CONFLICT_NAMES).collect();
            evidence.insert("state".into(), "conflict".into());
            evidence.insert("conflicts".into(), json!({ "total": total, "files": kept }));
            finish(&job, &host, evidence, &answer);
        }
        Merged::Broken(why) => {
            evidence.insert("state".into(), "error".into());
            evidence.insert("message".into(), why.into());
            finish(&job, &host, evidence, &answer);
        }
    }
}

/// The end of a check that is not prepared: its folder and row go, its letter goes to the ledger,
/// and the verb's answer (when it is still waiting) is the same evidence.
fn finish(
    job: &Job,
    host: &Host,
    mut evidence: serde_json::Map<String, serde_json::Value>,
    answer: &mpsc::Sender<String>,
) {
    evidence.insert("endedMs".into(), crate::now_epoch_ms().into());
    take_down(&job.store, host, &job.repo, &job.check, &job.folder);
    let text = serde_json::Value::Object(evidence.clone()).to_string();
    letter(
        &job.sink,
        LandCheckReceipt {
            run: job.run.clone(),
            task: job.task.clone(),
            check: job.check.clone(),
            evidence: text.clone(),
        },
    );
    let _ = answer.send(text);
}

/// The merge in the throwaway folder: the worktree, the merge without a commit, and the merged
/// tree's id. A clash names its files; any other git refusal is the error.
///
/// The worktree is made empty (`--no-checkout`) and filled by a `reset --hard` of its own. One
/// `worktree add` fills it through a grandchild `reset --hard` that outlives the kill a budget
/// brings and keeps the entry locked as "initializing" meanwhile — an entry `remove --force`
/// refuses and `prune` skips. An empty add is over in an instant, and a fill that is the direct
/// child stops writing when it is cut, leaving an entry that comes down with the folder.
fn merge_in_folder(host: &Host, job: &Job) -> Merged {
    let folder = job.folder.display().to_string();
    let budget = job.policy.git_budget;
    if let Err(why) = git_within(
        host,
        &job.repo,
        &[
            "worktree",
            "add",
            "--detach",
            "--no-checkout",
            folder.as_str(),
            job.base_oid.as_str(),
        ],
        budget,
    ) {
        return Merged::Broken(format!("the throwaway checkout could not be made: {why}"));
    }
    if let Err(why) = git_within(host, &job.folder, &["reset", "--hard", "--quiet"], budget) {
        return Merged::Broken(format!("the throwaway checkout could not be filled: {why}"));
    }
    if let Err(why) = git_within(
        host,
        &job.folder,
        &["merge", "--no-commit", "--no-ff", job.head.as_str()],
        budget,
    ) {
        let clashing = git_within(
            host,
            &job.folder,
            &["diff", "--name-only", "--diff-filter=U"],
            budget,
        )
        .unwrap_or_default();
        let files: Vec<String> = clashing
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        return if files.is_empty() {
            Merged::Broken(format!("the merge did not finish: {why}"))
        } else {
            Merged::Clash(files)
        };
    }
    match git_within(host, &job.folder, &["write-tree"], budget) {
        Ok(tree) => Merged::Clean(tree),
        Err(why) => Merged::Broken(format!("the merged tree could not be named: {why}")),
    }
}

/// How the check's command ended.
enum CheckEnd {
    Exited { rc: Option<i32>, took_ms: i64 },
    TimedOut { took_ms: i64 },
    Unstartable(String),
}

impl CheckEnd {
    /// Write the command's outcome into the evidence: `passed` only for rc 0.
    fn apply(self, evidence: &mut serde_json::Map<String, serde_json::Value>) {
        match self {
            Self::Exited { rc, took_ms } => {
                let state = if rc == Some(0) { "passed" } else { "failed" };
                evidence.insert("state".into(), state.into());
                evidence.insert("rc".into(), rc.map_or(serde_json::Value::Null, Into::into));
                evidence.insert("tookMs".into(), took_ms.into());
            }
            Self::TimedOut { took_ms } => {
                evidence.insert("state".into(), "timed_out".into());
                evidence.insert("tookMs".into(), took_ms.into());
            }
            Self::Unstartable(why) => {
                evidence.insert("state".into(), "unstartable".into());
                evidence.insert("message".into(), why.into());
            }
        }
    }
}

/// Run the project's command in the folder, its output in the log, its own process group, with
/// the limit on it. The group is killed when the limit passes.
fn run_command(job: &Job, command: &str) -> CheckEnd {
    let started = Instant::now();
    let log = match File::create(&job.log) {
        Ok(log) => log,
        Err(why) => return CheckEnd::Unstartable(format!("the log could not be made: {why}")),
    };
    let script = job.log.with_extension(SCRIPT_EXTENSION);
    let mut child = match spawn(&job.policy, command, &job.folder, log, &script) {
        Ok(child) => child,
        Err(why) => return CheckEnd::Unstartable(format!("the check did not start: {why}")),
    };
    let pid = child.id();
    register_running(pid);
    let end = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                break CheckEnd::Exited {
                    rc: status.code(),
                    took_ms: elapsed_ms(started),
                };
            }
            Ok(None) if started.elapsed() >= job.policy.timeout => {
                kill_group(pid);
                let _ = child.wait();
                break CheckEnd::TimedOut {
                    took_ms: elapsed_ms(started),
                };
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(why) => {
                kill_group(pid);
                let _ = child.wait();
                break CheckEnd::Unstartable(format!("the check could not be watched: {why}"));
            }
        }
    };
    unregister_running(pid);
    // The script a Windows check ran from goes with the check; the log stays for its keep.
    let _ = fs::remove_file(&script);
    end
}

fn elapsed_ms(started: Instant) -> i64 {
    i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX)
}

/// The shell that reads the command, started in the folder with its output on the log. bash reads
/// the command on its stdin (`-s`). cmd runs it as a script written beside the log (`script_at`)
/// with `/C`, whose exit code is the script's last errorlevel: cmd reading its commands from stdin
/// exits 0 at the end of the input whatever the last command's errorlevel, so a failing check was
/// judged `passed` on Windows (draft PR #3's leg, t-42447).
fn spawn(
    policy: &Policy,
    command: &str,
    folder: &Path,
    log: File,
    script_at: &Path,
) -> std::io::Result<Child> {
    let stderr = log.try_clone()?;
    let mut process = crate::proc::quiet_command(&policy.shell);
    process
        .current_dir(folder)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    {
        let _ = script_at;
        process.arg("-s");
        crate::codex_queue::prepare_process_group(&mut process);
    }
    #[cfg(windows)]
    {
        fs::write(script_at, command)?;
        process.arg("/D").arg("/C").arg(script_at);
        process.stdin(Stdio::null());
    }
    let mut child = process.spawn()?;
    {
        use std::io::Write as _;
        if let Some(mut stdin) = child.stdin.take() {
            // The shell reads the whole command from its stdin; a shell that exits early has
            // nothing more to read, which is not an error here.
            let _ = stdin.write_all(command.as_bytes());
        }
    }
    Ok(child)
}

/// Kill a check's whole process group. A check that is gone already is not an error.
#[cfg(unix)]
fn kill_group(pid: u32) {
    let _ = crate::codex_queue::signal_process_group(pid, libc::SIGKILL);
}

#[cfg(windows)]
fn kill_group(pid: u32) {
    let pid = pid.to_string();
    let _ = crate::proc::quiet_command("taskkill")
        .args(["/T", "/F", "/PID", pid.as_str()])
        .output();
}

/// The checks this window is running right now, by the pid of their group leader.
static RUNNING: Mutex<Vec<u32>> = Mutex::new(Vec::new());

fn register_running(pid: u32) {
    RUNNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(pid);
}

fn unregister_running(pid: u32) {
    RUNNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .retain(|running| *running != pid);
}

/// The window is going away: every check it still runs goes with it. Their rows stay, and the next
/// window's sweep takes them down with an abandoned letter.
pub(crate) fn stop_running() {
    let pids: Vec<u32> = RUNNING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    for pid in pids {
        kill_group(pid);
    }
}

/// The end of a prepared check: its exit code and log, written as the evidence, and the folder and
/// row removed. Refused unless the check was prepared for this task and head, in this store, with
/// the folder and log it named.
fn record(store: &Store, ask: &LandCheckAsk, now_ms: i64, sink: &Sink) -> Result<String, String> {
    let LandCheckAsk::Record {
        run,
        task,
        head,
        check,
        rc,
        log,
        took_ms,
    } = ask
    else {
        return Err("only a record ends a prepared check".to_string());
    };
    let (rc, took_ms) = (*rc, *took_ms);
    let row = read_row(store, check).ok_or_else(|| {
        format!("no prepared check {check}: it was never prepared here, or it has ended")
    })?;
    if row["state"].as_str() != Some("prepared") {
        return Err(format!(
            "check {check} is still running — its merge is not prepared yet, ask again later"
        ));
    }
    if row["task"].as_str() != Some(task.as_str()) {
        return Err(format!("check {check} belongs to another task than {task}"));
    }
    let prepared_head = row["head"].as_str().unwrap_or_default();
    if !prepared_head
        .to_ascii_lowercase()
        .starts_with(&head.to_ascii_lowercase())
    {
        return Err(format!(
            "check {check} was prepared for another head than {head}"
        ));
    }
    let folder = store.folder_of(check);
    if row["path"].as_str() != Some(folder.display().to_string().as_str()) {
        return Err(format!(
            "check {check} names a folder the store does not own"
        ));
    }
    if Path::new(log) != store.log_of(check) {
        return Err(format!("the log is not the log check {check} named"));
    }
    match fs::symlink_metadata(store.log_of(check)) {
        Ok(meta) if meta.is_file() => {}
        _ => return Err(format!("the log of check {check} is not a regular file")),
    }
    let repo = PathBuf::from(row["repo"].as_str().unwrap_or_default());
    let host = Host::for_workspace(&repo);
    let base_oid = row["base"]["oid"].as_str().unwrap_or_default();
    if folder.is_dir() {
        let base_now = git(&host, &folder, &["rev-parse", "HEAD"]).unwrap_or_default();
        if base_now != base_oid {
            return Err(format!(
                "the base of check {check} moved: the folder is at {base_now}, not {base_oid}"
            ));
        }
        if let Ok(merge_head) = git(
            &host,
            &folder,
            &["rev-parse", "-q", "--verify", "MERGE_HEAD"],
        ) && !merge_head.eq_ignore_ascii_case(prepared_head)
        {
            return Err(format!(
                "the merge in check {check} is not the prepared head any more"
            ));
        }
    }
    let mut evidence = serde_json::Map::new();
    let state = if rc == 0 { "passed" } else { "failed" };
    evidence.insert("state".into(), state.into());
    evidence.insert("rc".into(), rc.into());
    evidence.insert("head".into(), prepared_head.into());
    evidence.insert("base".into(), row["base"].clone());
    evidence.insert("tree".into(), row["tree"].clone());
    evidence.insert("log".into(), log.as_str().into());
    if let Some(took_ms) = took_ms {
        evidence.insert("tookMs".into(), took_ms.into());
    }
    evidence.insert("startedMs".into(), row["startedMs"].clone());
    evidence.insert("endedMs".into(), now_ms.into());
    take_down(store, &host, &repo, check, &folder);
    let text = serde_json::Value::Object(evidence.clone()).to_string();
    letter(
        sink,
        LandCheckReceipt {
            run: run.to_string(),
            task: task.to_string(),
            check: check.to_string(),
            evidence: text.clone(),
        },
    );
    Ok(text)
}

/// Take down what the window made for `check`: the worktree entry and the folder, then the row.
/// Only the store's own folder under the check's name, and only when it is a real folder — a link
/// in its place is never followed.
fn take_down(store: &Store, host: &Host, repo: &Path, check: &str, folder: &Path) {
    if !is_check_id(check) || folder != store.folder_of(check) {
        return;
    }
    let text = folder.display().to_string();
    let _ = git(
        host,
        repo,
        &["worktree", "remove", "--force", text.as_str()],
    );
    let _ = git(host, repo, &["worktree", "prune"]);
    if let Ok(meta) = fs::symlink_metadata(folder)
        && meta.is_dir()
    {
        let _ = fs::remove_dir_all(folder);
    }
    let _ = fs::remove_file(store.row_of(check));
}

/// The leftovers of earlier runs: rows whose lease ran out (a running one is told abandoned),
/// folders with no row, and logs past their keep. A live lease is never touched.
fn sweep(store: &Store, now_ms: i64, sink: &Sink) {
    let rows = store.root.join(ROWS_DIR);
    for entry in fs::read_dir(&rows).into_iter().flatten().flatten() {
        let Some(row) = fs::read_to_string(entry.path())
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        else {
            continue;
        };
        let check = row["check"].as_str().unwrap_or_default().to_string();
        let lease = row["leaseUntilMs"].as_i64().unwrap_or(0);
        if !is_check_id(&check) || lease > now_ms {
            continue;
        }
        let repo = PathBuf::from(row["repo"].as_str().unwrap_or_default());
        let host = Host::for_workspace(&repo);
        take_down(store, &host, &repo, &check, &store.folder_of(&check));
        if row["state"].as_str() == Some("running") {
            let mut evidence = serde_json::Map::new();
            evidence.insert("state".into(), "abandoned".into());
            evidence.insert("head".into(), row["head"].clone());
            evidence.insert("base".into(), row["base"].clone());
            evidence.insert("startedMs".into(), row["startedMs"].clone());
            evidence.insert("endedMs".into(), now_ms.into());
            letter(
                sink,
                LandCheckReceipt {
                    run: row["run"].as_str().unwrap_or_default().to_string(),
                    task: row["task"].as_str().unwrap_or_default().to_string(),
                    check: check.clone(),
                    evidence: serde_json::Value::Object(evidence).to_string(),
                },
            );
        }
    }
    for entry in fs::read_dir(store.root.join(WORK_DIR))
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        let folder = entry.path();
        let real_folder = fs::symlink_metadata(&folder).is_ok_and(|meta| meta.is_dir());
        if is_check_id(&name) && real_folder && !store.row_of(&name).exists() {
            let _ = fs::remove_dir_all(&folder);
        }
    }
    let keep_after = SystemTime::now().checked_sub(Duration::from_millis(
        u64::try_from(LOG_TTL_MS).unwrap_or(0),
    ));
    for entry in fs::read_dir(store.root.join(LOGS_DIR))
        .into_iter()
        .flatten()
        .flatten()
    {
        let old = match (
            keep_after,
            entry.metadata().and_then(|meta| meta.modified()),
        ) {
            (Some(keep_after), Ok(modified)) => modified < keep_after,
            _ => false,
        };
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The git calls of this module, each with the same budget. Its text is the first line's trimmed
/// output, or git's refusal.
fn git(host: &Host, dir: &Path, args: &[&str]) -> Result<String, String> {
    git_within(host, dir, args, GIT_BUDGET)
}

/// One git call under the budget given: the steps in a check's folder take the policy's.
fn git_within(host: &Host, dir: &Path, args: &[&str], budget: Duration) -> Result<String, String> {
    match host.vcs().try_within(dir, args, budget) {
        Within::Said(said) => Ok(said.trim().to_string()),
        Within::Refused(why) => Err(why.trim().to_string()),
        Within::Silent => Err(format!(
            "git did not answer within {} ms",
            budget.as_millis()
        )),
    }
}

/// A canonical path as git reads it. On Windows `canonicalize` answers a verbatim path
/// (`\\?\C:\…`), which git refuses as a worktree's folder ("could not create leading
/// directories … Invalid argument"). A plain disk path loses the prefix when the plain form
/// fits in a classic path; a UNC path and a path longer than that keep it, as the prefix is
/// what makes them reachable (part of the rule of `dunce::simplified`). Any other path, and
/// one that is not Unicode, is answered as it came.
fn without_verbatim_prefix(path: PathBuf) -> PathBuf {
    let Some(rest) = path.to_str().and_then(|text| text.strip_prefix(r"\\?\")) else {
        return path;
    };
    let mut letters = rest.chars();
    let is_disk = letters.next().is_some_and(|c| c.is_ascii_alphabetic())
        && letters.next() == Some(':')
        && letters.next() == Some('\\');
    if is_disk && rest.len() < CLASSIC_PATH_MAX {
        PathBuf::from(rest)
    } else {
        path
    }
}

/// The length a Windows path may have without the verbatim prefix (MAX_PATH, the terminating
/// NUL included).
const CLASSIC_PATH_MAX: usize = 260;

/// The commit a word names in the repository, if git knows it as one.
fn commit_of(host: &Host, repo: &Path, word: &str) -> Option<String> {
    let spec = format!("{word}^{{commit}}");
    git(
        host,
        repo,
        &["rev-parse", "--verify", "--quiet", spec.as_str()],
    )
    .ok()
    .filter(|oid| !oid.is_empty())
}

/// The repository's main folder, from its common git directory.
fn repo_root_of(common: &Path) -> PathBuf {
    if common.file_name() == Some(std::ffi::OsStr::new(".git")) {
        common.parent().unwrap_or(common).to_path_buf()
    } else {
        common.to_path_buf()
    }
}

/// A new check id: the time and a number this window counts up. The folder, row and log are all
/// named after it.
fn new_check_id(now_ms: i64) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let number = SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    format!("{CHECK_PREFIX}{}-{number}", now_ms.max(0))
}

fn base_evidence(job: &Job, now_ms: i64) -> serde_json::Map<String, serde_json::Value> {
    let mut evidence = serde_json::Map::new();
    evidence.insert("check".into(), job.check.clone().into());
    evidence.insert("head".into(), job.head.clone().into());
    evidence.insert(
        "base".into(),
        json!({ "ref": job.base_name, "oid": job.base_oid }),
    );
    evidence.insert("startedMs".into(), job.started_ms.into());
    evidence.insert("endedMs".into(), now_ms.into());
    evidence
}

fn running_row(job: &Job, now_ms: i64) -> serde_json::Value {
    let lease = now_ms
        .saturating_add(3 * i64::try_from(GIT_BUDGET.as_millis()).unwrap_or(i64::MAX))
        .saturating_add(i64::try_from(job.policy.timeout.as_millis()).unwrap_or(i64::MAX))
        .saturating_add(LEASE_SLACK_MS);
    json!({
        "check": job.check,
        "run": job.run,
        "task": job.task,
        "head": job.head,
        "base": { "name": job.base_name, "oid": job.base_oid },
        "repo": job.repo.display().to_string(),
        "path": job.folder.display().to_string(),
        "log": job.log.display().to_string(),
        "state": "running",
        "startedMs": now_ms,
        "leaseUntilMs": lease,
        "tree": serde_json::Value::Null,
    })
}

fn prepared_row(job: &Job, tree: &str, now_ms: i64) -> serde_json::Value {
    let mut row = running_row(job, job.started_ms);
    row["state"] = "prepared".into();
    row["tree"] = tree.into();
    row["leaseUntilMs"] = now_ms.saturating_add(PREPARED_TTL_MS).into();
    row
}

/// Write a row by its own name: a temporary file, then a rename, so a reader never sees half of one.
fn write_row(store: &Store, row: &serde_json::Value) -> Result<(), String> {
    let check = row["check"].as_str().unwrap_or_default();
    if !is_check_id(check) {
        return Err(format!("{check:?} is not a check id"));
    }
    let path = store.row_of(check);
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, row.to_string())
        .and_then(|()| fs::rename(&temporary, &path))
        .map_err(|why| format!("the row for {check} could not be written: {why}"))
}

fn read_row(store: &Store, check: &str) -> Option<serde_json::Value> {
    if !is_check_id(check) {
        return None;
    }
    fs::read_to_string(store.row_of(check))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

#[cfg(test)]
mod tests;
