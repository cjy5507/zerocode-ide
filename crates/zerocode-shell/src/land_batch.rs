//! t-45315 (T3 stage 6): 통합 큐의 창 쪽 — 쌓기, 게시, 게이트 준비 위임, 올리기, 정리.
//!
//! 원장(`zerocode_core::orchestration::land_batch`)이 누가 무엇을 눌렀는지 정해 건네면, 이 파일이
//! git으로 그 일을 하고 본 것을 영수증으로 돌려준다. 원장에 직접 쓰지 않는다 — 모든 기록은
//! [`Roads::ledger`]로 가고, 원장이 묶음의 기록에 비추어 받거나 이유와 함께 거절한다.
//!
//! 지키는 것:
//!   - 쌓기는 앱 전용 ref(`refs/zerocode/stack/<묶음>/<세대>`)와 그 세대의 통합 ref
//!     (`refs/zerocode/integration/<묶음>/<세대>`)만 움직인다. 작업 트리, 색인, 가지는 건드리지 않는다.
//!   - 기준 가지는 올리기에서만 움직인다. 초록 게이트를 받은 끝 커밋으로 빨리감기하고, 새 커밋은 없다.
//!     올리기의 git은 제 스레드에서 돈다. 누름은 [`ANSWER_BUDGET`] 안에 그 끝을 듣고, 못 들으면
//!     「아직 올리는 중」을 듣는다 — 빨리감기 앞의 읽기만으로도 다리가 기다리는
//!     [`zerocode_hookd::TEAM_DEADLINE`]을 넘길 수 있다. 빨리감기 git은 죽이지 않는다 —
//!     [`PROMOTE_GIT_BUDGET`]이 지나면 지켜보는 스레드가 맡고, git이 끝나면 정리 길이 결과를 적는다.
//!   - 게이트 폴더는 착지 전 점검(`land_check`)의 준비 길로 만들고 기록 길로 치운다.
//!   - 이 저장소에서 큐가 꺼져 있으면 취소 말고는 아무것도 하지 않는다. ref도 폴더도 만들지 않는다.
//!   - ref와 경로는 [`is_batch_id`]가 받은 묶음 id로만 만든다.
//!
//! 「다음 실행」은 창이 다시 뜰 때와 다음 `land-batch` 호출 때다([`settle`]). 임대가 끝난 쌓기와
//! 올리기를 git이 남긴 대로 정리하고, 보관 기간이 지난 끝난 묶음의 ref를 지운다.

use super::*;

use std::collections::BTreeMap;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use crate::land_check::{ANSWER_BUDGET, CONFLICT_NAMES, GIT_BUDGET};
use crate::worktree_landing::{MergeProbe, parse_merge_probe, resolve_landing_base};
use serde_json::json;
use zerocode_core::host::{Host, Within, lock_free_git};
use zerocode_core::orchestration::land_batch::{
    Action, BATCH_PREFIX, BatchState, DropWhy, Dropped, LandBatchAsk, LandBatchReceipt, Landed,
    Member, PressedBy, PromoteStop, Promoting, Record, StackStop, Verdict, bisect_next,
    integration_queue_supported, is_batch_id, is_object_id, operation_id,
};
use zerocode_core::orchestration::land_check::{LandCheckAsk, LandCheckReceipt};
use zerocode_orchestrator::git_publisher::{
    APPLIED_REF_PREFIX, CANCELLED_REF_PREFIX, LocalGitRefPublisher, PUBLISH_GIT_TIMEOUT,
    StackPublication,
};
use zerocode_orchestrator::publish::{PublishVerification, PublisherFailure};
use zerocode_orchestrator::workflow::{INTEGRATION_REF_PREFIX, RefBinding, STACK_REF_PREFIX};

/// How long a promote's fast-forward of the base branch's checkout runs before a watcher takes it
/// over. The git is never killed at this mark: it finishes, and the settle road writes what it
/// left. The press hears its end only when it comes within [`Policy::answer_budget`].
///
/// Measured on a no-hardlink clone of this repository (build-line job measure-promote, 2026-10-14):
/// 18 fast-forwards of a checked-out main over 1 to 32 landings (249 files at most)
/// took 50 ms at the median and 201 ms at the worst. The budget is three times the worst,
/// rounded up to a whole second, at least 5 s and at most 20 s.
pub(crate) const PROMOTE_GIT_BUDGET: Duration = Duration::from_secs(5);

/// The slack a stacking or promoting lease gets past the sum of its git limits. A lease past its
/// end is somebody's crash, and the next run settles it.
const BATCH_LEASE_SLACK_MS: i64 = 10 * 60 * 1000;

/// How long an ended batch keeps its stack and integration refs and its generations' receipts.
const INTEGRATION_KEEP_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// The git calls one stacked member makes: the merge, the commit, two re-reads, the stack ref's
/// move and the overlap diff.
const GIT_CALLS_PER_STEP: u32 = 6;

/// The git calls around a generation's steps: the two refs made at the base, and the publisher's
/// reads and its one transaction.
const GIT_CALLS_AROUND: u32 = 8;

/// The git calls a promote makes besides its fast-forward: the base, the worktree list, the
/// status and the git folder before, and the same reads after.
const GIT_CALLS_PROMOTE: u32 = 8;

/// The largest landing message one member's file may hold.
const MESSAGE_MAX_BYTES: u64 = 64 * 1024;

/// The marks of unfinished work in a checkout's git folder: a merge, a rebase, a cherry-pick or a
/// revert in progress, or another git holding the index.
const UNFINISHED_MARKS: [&str; 6] = [
    "MERGE_HEAD",
    "rebase-merge",
    "rebase-apply",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "index.lock",
];

/// The exit code a gate folder taken down unrun is recorded with on land-check's record road. Its
/// letter is never written: the batch's own record says why the gate ended.
const UNRUN_RC: i32 = -1;

/// How often the window looks at a merge probe's or a fast-forward's exit.
const POLL: Duration = Duration::from_millis(50);

const LAND_BATCH_DIR: &str = "land-batch";
const MESSAGES_DIR: &str = "messages";

/// What every press but a cancel hears in a repository that has not turned the queue on.
const OFF: &str = "the integration queue is off in this repository — nothing was written; turn \
                   it on in the project's settings first";

/// How the queue runs in one repository: whether it is on, the compare ref its project pins, and
/// the budgets of its git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Policy {
    /// `integration_queue_enabled` in the project's settings, on a machine that runs the queue.
    pub(crate) enabled: bool,
    pub(crate) pinned_base: Option<String>,
    /// The budget of each git call: [`GIT_BUDGET`], or a shorter one a test injects.
    pub(crate) git_budget: Duration,
    /// [`PROMOTE_GIT_BUDGET`], or one a test injects: shorter to outlive, or out of reach.
    pub(crate) promote_budget: Duration,
    /// How long a press on promote waits for its end before it answers "still promoting":
    /// [`ANSWER_BUDGET`], or one a test injects: shorter, or out of reach.
    pub(crate) answer_budget: Duration,
    /// The git a fast-forward runs: `git`, or a test's slow or meddling stand-in.
    pub(crate) git: PathBuf,
}

impl Policy {
    fn off() -> Policy {
        Policy {
            enabled: false,
            pinned_base: None,
            git_budget: GIT_BUDGET,
            promote_budget: PROMOTE_GIT_BUDGET,
            answer_budget: ANSWER_BUDGET,
            git: PathBuf::from("git"),
        }
    }
}

/// The stored settings the queue reads its switch from. Set once at boot.
static SETTINGS: OnceLock<Arc<settings::SettingsRepository>> = OnceLock::new();

/// Keep the settings the queue reads each repository's switch from.
pub(crate) fn hold_settings(repository: Arc<settings::SettingsRepository>) {
    let _ = SETTINGS.set(repository);
}

fn policy_of(repo: &Path) -> Policy {
    SETTINGS
        .get()
        .map_or_else(Policy::off, |settings| policy_in(settings, repo))
}

/// The policy a repository's project stores: on only when it said so and this machine runs the
/// queue; the compare ref pinned the way land-check reads it.
pub(crate) fn policy_in(settings: &settings::SettingsRepository, repo: &Path) -> Policy {
    let stored = project_runtime::stored_project_settings_at(
        settings,
        &project_runtime::project_settings_key(repo),
    )
    .unwrap_or_default();
    Policy {
        enabled: integration_queue_supported() && stored.integration_queue_enabled == Some(true),
        pinned_base: stored
            .worktree_base_ref
            .map(|pin| pin.trim().to_string())
            .filter(|pin| !pin.is_empty()),
        ..Policy::off()
    }
}

/// Whether a repository has the queue on: the board offers a batch's presses only then.
pub(crate) fn switched_on(repo: &Path) -> bool {
    policy_of(repo).enabled
}

/// The repository a checkout's switch is kept under: the main folder of its common git folder,
/// the same one every press reads its policy from. A folder git does not know stands for itself.
pub(crate) fn settings_repository(checkout: &Path) -> PathBuf {
    repository_of(checkout).unwrap_or_else(|_| checkout.to_path_buf())
}

/// Lay each card's switch over the desk: an open batch in a repository that turned the queue off
/// keeps only its cancel.
pub(crate) fn dress_desk(desk: &mut crate::orchestration::desk::DeskSnapshot) {
    for card in &mut desk.land_batches {
        card.on = switched_on(Path::new(&card.batch.repository));
    }
}

/// Where the ledger's records go: the runtime's actor in the window, a channel in tests. A refusal
/// comes back in the ledger's own words.
pub(crate) type Records = Arc<dyn Fn(LandBatchReceipt) -> Result<(), String> + Send + Sync>;

/// One of land-check's roads, called with its ask and the time.
pub(crate) type Checks = Arc<dyn Fn(&LandCheckAsk, i64) -> Result<String, String> + Send + Sync>;

/// The roads one press takes out of this file.
#[derive(Clone)]
pub(crate) struct Roads {
    pub(crate) ledger: Records,
    /// land-check's own roads: prepare a gate folder, and record a gate's end with its letter.
    pub(crate) check: Checks,
    /// land-check's record road with its letter dropped: takes down a prepared folder no gate ran
    /// in, the way a record would.
    pub(crate) quiet: Checks,
    /// `<orchestration data>/land-batch`: the landing messages a batch keeps for its restacks.
    pub(crate) folder: PathBuf,
}

impl Roads {
    fn record(
        &self,
        run: &str,
        batch: &str,
        generation: u32,
        by: &PressedBy,
        record: Record,
    ) -> Result<(), String> {
        let call: &(dyn Fn(LandBatchReceipt) -> Result<(), String> + Send + Sync) =
            self.ledger.as_ref();
        call(LandBatchReceipt {
            run: run.to_string(),
            batch: batch.to_string(),
            generation,
            by: by.clone(),
            record,
        })
    }

    fn check(&self, ask: &LandCheckAsk, now_ms: i64) -> Result<String, String> {
        let call: &(dyn Fn(&LandCheckAsk, i64) -> Result<String, String> + Send + Sync) =
            self.check.as_ref();
        call(ask, now_ms)
    }

    fn quiet(&self, ask: &LandCheckAsk, now_ms: i64) -> Result<String, String> {
        let call: &(dyn Fn(&LandCheckAsk, i64) -> Result<String, String> + Send + Sync) =
            self.quiet.as_ref();
        call(ask, now_ms)
    }
}

/// The window's roads: the runtime's actor, land-check's store under the same data root.
fn roads_at(data_root: &Path) -> Result<Roads, String> {
    let store = crate::land_check::Store::at(data_root)?;
    let root = data_root.to_path_buf();
    let dropped: crate::land_check::Sink = Arc::new(|_: LandCheckReceipt| {});
    Ok(Roads {
        ledger: Arc::new(|receipt: LandBatchReceipt| {
            crate::orchestration::record_land_batch(receipt, crate::now_epoch_ms())
        }),
        check: Arc::new(move |ask: &LandCheckAsk, now_ms: i64| {
            crate::land_check::carry(&root, ask, now_ms)
        }),
        quiet: Arc::new(move |ask: &LandCheckAsk, now_ms: i64| {
            crate::land_check::run(
                &store,
                &|_| crate::land_check::Policy::from_settings(None, None, None),
                ask,
                now_ms,
                &dropped,
            )
        }),
        folder: data_root.join(LAND_BATCH_DIR),
    })
}

/// Carry out one `land-batch` press from the window's own data root, and answer the JSON the verb
/// prints. `held` is every batch of the press's run as the ledger reads it now.
pub(crate) fn carry(
    data_root: &Path,
    ask: &LandBatchAsk,
    held: &[BatchState],
    now_ms: i64,
) -> Result<String, String> {
    run(&policy_of, ask, held, now_ms, &roads_at(data_root)?)
}

/// The window's start: every run's batches settled and swept as the next `land-batch` call would.
pub(crate) fn settle_at_start(data_root: &Path, runs: &[(String, Vec<BatchState>)], now_ms: i64) {
    let Ok(roads) = roads_at(data_root) else {
        return;
    };
    for (run, held) in runs {
        settle(&policy_of, held, run, None, now_ms, &roads);
    }
}

/// Why git gave no answer to use: it refused, in its own words, or it said nothing in its budget.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Fault {
    Refused(String),
    Silent,
}

impl Fault {
    fn said(&self) -> String {
        match self {
            Self::Refused(why) => why.clone(),
            Self::Silent => "git did not answer within its budget".to_string(),
        }
    }

    const fn stack_stop(&self) -> StackStop {
        match self {
            Self::Refused(_) => StackStop::GitRefused,
            Self::Silent => StackStop::GitSilent,
        }
    }

    const fn promote_stop(&self) -> PromoteStop {
        match self {
            Self::Refused(_) => PromoteStop::GitRefused,
            Self::Silent => PromoteStop::GitSilent,
        }
    }
}

/// One git call under `budget`, killed past it: everything it printed.
fn git_raw(host: &Host, dir: &Path, args: &[&str], budget: Duration) -> Result<String, Fault> {
    match host.vcs().try_within(dir, args, budget) {
        Within::Said(said) => Ok(said),
        Within::Refused(why) => Err(Fault::Refused(why.trim().to_string())),
        Within::Silent => Err(Fault::Silent),
    }
}

/// The same, trimmed: for the calls that answer one line.
fn git_text(host: &Host, dir: &Path, args: &[&str], budget: Duration) -> Result<String, Fault> {
    git_raw(host, dir, args, budget).map(|said| said.trim().to_string())
}

/// One git call whose non-zero exit is an answer too (`merge-tree` names its clashes with 1): its
/// exit code and what it printed, under `budget`, killed past it.
fn git_answer(dir: &Path, args: &[&str], budget: Duration) -> Result<(i32, String), Fault> {
    let mut child = lock_free_git(crate::proc::quiet_command("git"))
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|why| Fault::Refused(why.to_string()))?;
    let Some(mut pipe) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(Fault::Silent);
    };
    let draining = std::thread::spawn(move || {
        let mut said = Vec::new();
        let _ = std::io::Read::read_to_end(&mut pipe, &mut said);
        said
    });
    let Some(status) = exit_within(&mut child, budget) else {
        let _ = child.kill();
        let _ = child.wait();
        let _ = draining.join();
        return Err(Fault::Silent);
    };
    let said = draining.join().unwrap_or_default();
    let code = status.code().ok_or(Fault::Silent)?;
    Ok((code, String::from_utf8_lossy(&said).into_owned()))
}

/// The child's exit, if it came within `budget`. The child is left as it is either way.
fn exit_within(child: &mut Child, budget: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            Ok(None) | Err(_) => return None,
        }
    }
}

/// The commit a word names, when git knows it as one.
fn commit_of(host: &Host, repo: &Path, word: &str, budget: Duration) -> Option<String> {
    let spec = format!("{word}^{{commit}}");
    git_text(
        host,
        repo,
        &["rev-parse", "--verify", "--quiet", spec.as_str()],
        budget,
    )
    .ok()
    .filter(|oid| is_object_id(oid))
}

/// The tree a commit carries.
fn tree_of(host: &Host, repo: &Path, commit: &str, budget: Duration) -> Result<String, Fault> {
    let spec = format!("{commit}^{{tree}}");
    git_text(
        host,
        repo,
        &["rev-parse", "--verify", "--quiet", spec.as_str()],
        budget,
    )
}

/// The object a ref names, when it exists.
fn ref_value(host: &Host, repo: &Path, name: &str, budget: Duration) -> Option<String> {
    git_text(
        host,
        repo,
        &["rev-parse", "--verify", "--quiet", name],
        budget,
    )
    .ok()
    .filter(|oid| is_object_id(oid))
}

/// Whether `ancestor` is reachable from `descendant`. git says "no" with exit 1 and no words.
fn is_ancestor(
    host: &Host,
    repo: &Path,
    ancestor: &str,
    descendant: &str,
    budget: Duration,
) -> Result<bool, Fault> {
    match host.vcs().try_within(
        repo,
        &["merge-base", "--is-ancestor", ancestor, descendant],
        budget,
    ) {
        Within::Said(_) => Ok(true),
        Within::Refused(why) if why.trim().is_empty() => Ok(false),
        Within::Refused(why) => Err(Fault::Refused(why.trim().to_string())),
        Within::Silent => Err(Fault::Silent),
    }
}

/// The repository a checkout belongs to: its common git folder's main folder.
fn repository_of(checkout: &Path) -> Result<PathBuf, String> {
    let host = Host::for_workspace(checkout);
    let common = git_text(
        &host,
        checkout,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        GIT_BUDGET,
    )
    .map_err(|fault| {
        format!(
            "{} is not a git checkout: {}",
            checkout.display(),
            fault.said()
        )
    })?;
    let common = PathBuf::from(common);
    Ok(
        if common.file_name() == Some(std::ffi::OsStr::new(".git")) {
            common
                .parent()
                .map_or_else(|| common.clone(), Path::to_path_buf)
        } else {
            common
        },
    )
}

/// A new batch id: the time and a number this window counts up. Every ref and folder of the batch
/// is named after it.
fn new_batch_id(now_ms: i64) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let number = SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1;
    format!("{BATCH_PREFIX}{}-{number}", now_ms.max(0))
}

/// The ref a generation is stacked on, one merge per member.
pub(crate) fn stack_ref(batch: &str, generation: u32) -> String {
    format!("{STACK_REF_PREFIX}{batch}/{generation}")
}

/// The ref the publisher moves from the generation's base to its tip, with its receipt.
pub(crate) fn integration_ref(batch: &str, generation: u32) -> String {
    format!("{INTEGRATION_REF_PREFIX}{batch}/{generation}")
}

fn publication(run: &str, batch: &str, generation: u32, tip: &str, base: &str) -> StackPublication {
    StackPublication {
        batch: batch.to_string(),
        generation: u64::from(generation),
        operation_id: operation_id(run, batch, generation),
        stack: RefBinding {
            name: stack_ref(batch, generation),
            oid: tip.to_string(),
        },
        integration: RefBinding {
            name: integration_ref(batch, generation),
            oid: base.to_string(),
        },
    }
}

fn short(text: &str) -> &str {
    text.get(..7).unwrap_or(text)
}

fn short9(text: &str) -> &str {
    text.get(..9).unwrap_or(text)
}

fn branch_name(base_branch: &str) -> &str {
    base_branch
        .strip_prefix("refs/heads/")
        .unwrap_or(base_branch)
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

fn elapsed_ms(started: Instant) -> i64 {
    millis(started.elapsed())
}

/// How long one generation's stacking may hold the batch: its git limits and the slack.
fn stacking_lease_ms(size: usize, policy: &Policy) -> i64 {
    let calls = u32::try_from(size)
        .unwrap_or(u32::MAX)
        .saturating_mul(GIT_CALLS_PER_STEP)
        .saturating_add(GIT_CALLS_AROUND);
    let limit = policy.git_budget.max(PUBLISH_GIT_TIMEOUT);
    millis(limit.saturating_mul(calls)).saturating_add(BATCH_LEASE_SLACK_MS)
}

/// How long a promote may hold the batch: its reads, its fast-forward's budget and the slack.
fn promote_lease_ms(policy: &Policy) -> i64 {
    millis(
        policy
            .git_budget
            .saturating_mul(GIT_CALLS_PROMOTE)
            .saturating_add(policy.promote_budget),
    )
    .saturating_add(BATCH_LEASE_SLACK_MS)
}

/// The stackings and fast-forwards this window still runs, so no settle judges work that has not
/// ended. Keyed by run and batch.
static LIVE: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// One running stacking or fast-forward; it leaves [`LIVE`] when it is dropped.
struct LiveMark {
    run: String,
    batch: String,
}

impl Drop for LiveMark {
    fn drop(&mut self) {
        let mut live = LIVE.lock().unwrap_or_else(|held| held.into_inner());
        if let Some(at) = live
            .iter()
            .position(|(run, batch)| *run == self.run && *batch == self.batch)
        {
            live.remove(at);
        }
    }
}

fn mark_live(run: &str, batch: &str) -> LiveMark {
    LIVE.lock()
        .unwrap_or_else(|held| held.into_inner())
        .push((run.to_string(), batch.to_string()));
    LiveMark {
        run: run.to_string(),
        batch: batch.to_string(),
    }
}

fn live(run: &str, batch: &str) -> bool {
    LIVE.lock()
        .unwrap_or_else(|held| held.into_inner())
        .iter()
        .any(|(one, other)| one == run && other == batch)
}

/// One press's surroundings: whose run, who pressed, which repository and how it runs.
struct At<'a> {
    run: &'a str,
    by: &'a PressedBy,
    repo: &'a Path,
    host: &'a Host,
    policy: &'a Policy,
    roads: &'a Roads,
    now_ms: i64,
}

impl At<'_> {
    fn record(&self, batch: &str, generation: u32, record: Record) -> Result<(), String> {
        self.roads
            .record(self.run, batch, generation, self.by, record)
    }
}

/// Carry out one ask. Answers the JSON the verb prints, or the refusal's words. `policy_for` names
/// the policy of the repository a batch lives in; `held` is the run's batches, settled first.
pub(crate) fn run(
    policy_for: &dyn Fn(&Path) -> Policy,
    ask: &LandBatchAsk,
    held: &[BatchState],
    now_ms: i64,
    roads: &Roads,
) -> Result<String, String> {
    let pressed = match &ask.action {
        Action::Choose { .. } => None,
        Action::Prepare { batch, .. }
        | Action::Record { batch, .. }
        | Action::Promote { batch }
        | Action::Bisect { batch }
        | Action::Restack { batch, .. }
        | Action::Cancel { batch } => Some(batch),
    };
    if let Some(batch) = pressed
        && !is_batch_id(&batch.batch)
    {
        return Err(format!("{:?} is not a batch id", batch.batch));
    }
    let repo = match (&ask.action, pressed) {
        (Action::Choose { checkout, .. }, _) => repository_of(Path::new(checkout))?,
        (_, Some(batch)) => PathBuf::from(&batch.repository),
        (_, None) => return Err("this press names no batch".to_string()),
    };
    let policy = policy_for(&repo);
    if !policy.enabled && !matches!(ask.action, Action::Cancel { .. }) {
        return Err(OFF.to_string());
    }
    settle(
        policy_for,
        held,
        &ask.run,
        pressed.map(|batch| batch.batch.as_str()),
        now_ms,
        roads,
    );
    let host = Host::for_workspace(&repo);
    let at = At {
        run: &ask.run,
        by: &ask.by,
        repo: &repo,
        host: &host,
        policy: &policy,
        roads,
        now_ms,
    };
    match &ask.action {
        Action::Choose {
            members,
            message_dir,
            ..
        } => choose(&at, members, Path::new(message_dir)),
        Action::Prepare {
            batch,
            step,
            target,
        } => prepare(&at, batch, *step, target),
        Action::Record {
            batch,
            check,
            rc,
            log,
            took_ms,
        } => record(&at, batch, check, *rc, log, *took_ms),
        Action::Promote { batch } => promote(&at, batch),
        Action::Bisect { batch } => bisect(&at, batch),
        Action::Restack { batch, drop } => restack(&at, batch, drop),
        Action::Cancel { batch } => cancel(&at, batch),
    }
}

/// ① Choose: every member's landing message read and the base resolved before anything is written,
/// then `chosen`, then the stacking.
fn choose(at: &At<'_>, members: &[Member], message_dir: &Path) -> Result<String, String> {
    let messages = members
        .iter()
        .map(|member| {
            read_message(
                &message_dir.join(format!("{}.txt", member.task)),
                &member.task,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (base_branch, base) = base_of(at)?;
    let batch = new_batch_id(at.now_ms);
    keep_messages(&at.roads.folder, &batch, members, &messages)?;
    let chosen = Record::Chosen {
        members: members.to_vec(),
        base_branch: base_branch.clone(),
        base: base.clone(),
        repository: at.repo.display().to_string(),
    };
    if let Err(why) = at.record(&batch, 1, chosen) {
        forget_messages(&at.roads.folder, &batch);
        return Err(why);
    }
    stack(
        at,
        StackJob {
            run: at.run.to_string(),
            batch,
            generation: 1,
            members: members.to_vec(),
            messages,
            base_branch,
            base,
            repo: at.repo.to_path_buf(),
            by: at.by.clone(),
            policy: at.policy.clone(),
        },
    )
}

/// The local branch a batch stacks on and promotes: the compare ref's own branch. The queue moves
/// a local branch only, never a remote-tracking one.
fn base_of(at: &At<'_>) -> Result<(String, String), String> {
    let landing = resolve_landing_base(at.host, at.repo, at.policy.pinned_base.as_deref());
    let name = landing
        .name
        .ok_or("this repository has no compare ref, so there is no base branch to stack on")?;
    let short_name = name.strip_prefix("refs/remotes/").unwrap_or(&name);
    let mut branches = vec![if short_name.starts_with("refs/heads/") {
        short_name.to_string()
    } else {
        format!("refs/heads/{short_name}")
    }];
    if let Some((_, branch)) = short_name.split_once('/')
        && !short_name.starts_with("refs/")
    {
        branches.push(format!("refs/heads/{branch}"));
    }
    branches
        .into_iter()
        .find_map(|branch| {
            commit_of(at.host, at.repo, &branch, at.policy.git_budget).map(|base| (branch, base))
        })
        .ok_or_else(|| {
            format!(
                "the compare ref {name} has no local branch to stack on and promote — the queue \
                 moves a local branch, never {name} itself"
            )
        })
}

/// One member's landing message: a regular file of bounded size whose placeholders are filled in.
fn read_message(path: &Path, task: &str) -> Result<String, String> {
    if task.is_empty()
        || !task
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!("{task:?} cannot name a landing message file"));
    }
    let meta = fs::symlink_metadata(path).map_err(|_| {
        format!(
            "there is no landing message for {task}: {} is missing",
            path.display()
        )
    })?;
    if !meta.is_file() {
        return Err(format!(
            "the landing message for {task} at {} is not a regular file",
            path.display()
        ));
    }
    if meta.len() > MESSAGE_MAX_BYTES {
        return Err(format!(
            "the landing message for {task} is {} bytes, past the {MESSAGE_MAX_BYTES} a merge \
             commit takes",
            meta.len()
        ));
    }
    let text = fs::read_to_string(path)
        .map_err(|why| format!("the landing message for {task} could not be read: {why}"))?;
    if text.trim().is_empty() {
        return Err(format!("the landing message for {task} is empty"));
    }
    if let Some(mark) = placeholder(&text) {
        return Err(format!(
            "the landing message for {task} still has the placeholder {mark} — fill it in first"
        ));
    }
    Ok(text)
}

/// The first `{UPPER_CASE}` placeholder a message still carries.
fn placeholder(text: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(open) = text[from..].find('{').map(|at| at + from) {
        let after = open + 1;
        if let Some(close) = text[after..].find('}').map(|at| at + after) {
            let inner = &text[after..close];
            if inner.starts_with(|first: char| first.is_ascii_uppercase())
                && inner
                    .chars()
                    .all(|one| one.is_ascii_uppercase() || one.is_ascii_digit() || one == '_')
            {
                return Some(&text[open..=close]);
            }
        }
        from = after;
    }
    None
}

fn messages_of(folder: &Path, batch: &str) -> PathBuf {
    folder.join(MESSAGES_DIR).join(batch)
}

/// Keep the members' landing messages for the batch's restacks: the ledger's records carry no
/// message, and a restack may stack a member no earlier generation reached.
fn keep_messages(
    folder: &Path,
    batch: &str,
    members: &[Member],
    messages: &[String],
) -> Result<(), String> {
    let kept = messages_of(folder, batch);
    fs::create_dir_all(&kept)
        .map_err(|why| format!("the batch's landing messages could not be kept: {why}"))?;
    for (member, text) in members.iter().zip(messages) {
        fs::write(kept.join(format!("{}.txt", member.task)), text).map_err(|why| {
            format!(
                "the landing message of {} could not be kept: {why}",
                member.task
            )
        })?;
    }
    Ok(())
}

fn kept_message(folder: &Path, batch: &str, task: &str) -> Result<String, String> {
    read_message(
        &messages_of(folder, batch).join(format!("{task}.txt")),
        task,
    )
}

fn forget_messages(folder: &Path, batch: &str) {
    if is_batch_id(batch) {
        let _ = fs::remove_dir_all(messages_of(folder, batch));
    }
}

/// One generation to stack: its members in order with their messages, the base, who pressed.
struct StackJob {
    run: String,
    batch: String,
    generation: u32,
    members: Vec<Member>,
    messages: Vec<String>,
    base_branch: String,
    base: String,
    repo: PathBuf,
    by: PressedBy,
    policy: Policy,
}

/// A stack built to its tip: every step's merge commit, and the tip's tree.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Built {
    steps: Vec<String>,
    tree: String,
}

/// Where a stacking stopped, as `stack_stopped` says it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Halt {
    step: u32,
    task: String,
    why: StackStop,
    files: Vec<String>,
    total: u64,
}

/// A stacking that did not reach `stacked`: stopped, or left for the next run because the
/// publisher's answer never came.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Halted {
    Stop(Halt),
    Unsettled(String),
}

/// ② Stack one generation: `stacking` first, then the merges on their own thread. The verb hears
/// the end when it comes within [`ANSWER_BUDGET`], and `stacking` otherwise.
fn stack(at: &At<'_>, job: StackJob) -> Result<String, String> {
    let operation = operation_id(&job.run, &job.batch, job.generation);
    let lease_until_ms = at
        .now_ms
        .saturating_add(stacking_lease_ms(job.members.len(), &job.policy));
    at.record(
        &job.batch,
        job.generation,
        Record::Stacking {
            operation: operation.clone(),
            lease_until_ms,
        },
    )?;
    let (batch, generation) = (job.batch.clone(), job.generation);
    let (answer, first_answer) = mpsc::channel::<String>();
    let roads = at.roads.clone();
    let mark = mark_live(&job.run, &job.batch);
    let spawned = std::thread::Builder::new()
        .name("land-batch".to_string())
        .spawn(move || {
            let said = stack_job(&job, &roads);
            drop(mark);
            let _ = answer.send(said);
        });
    if let Err(why) = spawned {
        return Err(format!(
            "the stacking could not start: {why} — the next run settles generation {generation}"
        ));
    }
    match first_answer.recv_timeout(ANSWER_BUDGET) {
        Ok(said) => Ok(said),
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(json!({
            "state": "stacking",
            "batch": batch,
            "generation": generation,
            "operation": operation,
            "leaseUntilMs": lease_until_ms,
        })
        .to_string()),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("the stacking stopped before it answered".to_string())
        }
    }
}

/// The whole stacking on its own thread: the merges, the publish, the record. Answers what the
/// verb prints.
fn stack_job(job: &StackJob, roads: &Roads) -> String {
    let started = Instant::now();
    let host = Host::for_workspace(&job.repo);
    let ended = build(&host, job).and_then(|built| publish(job, &built).map(|()| built));
    let record =
        |record: Record| roads.record(&job.run, &job.batch, job.generation, &job.by, record);
    let said = match ended {
        Ok(built) => {
            let tip = built.steps.last().cloned().unwrap_or_default();
            let overlaps = overlaps(
                &host,
                &job.repo,
                &job.base,
                &job.members,
                job.policy.git_budget,
            );
            let took_ms = elapsed_ms(started);
            let stacked = Record::Stacked {
                steps: built.steps.clone(),
                tip: tip.clone(),
                tree: built.tree.clone(),
                took_ms,
                overlaps,
            };
            match record(stacked) {
                Ok(()) => json!({
                    "state": "stacked",
                    "batch": job.batch,
                    "generation": job.generation,
                    "steps": built.steps,
                    "tip": tip,
                    "tree": built.tree,
                    "overlaps": overlaps,
                    "tookMs": took_ms,
                }),
                Err(why) => json!({
                    "state": "refused",
                    "batch": job.batch,
                    "generation": job.generation,
                    "why": why,
                }),
            }
        }
        Err(Halted::Stop(halt)) => {
            let _ = record(Record::StackStopped {
                step: halt.step,
                task: halt.task.clone(),
                why: halt.why,
                files: halt.files.clone(),
                total: halt.total,
            });
            json!({
                "state": "stack_stopped",
                "batch": job.batch,
                "generation": job.generation,
                "step": halt.step,
                "task": halt.task,
                "why": halt.why,
                "files": halt.files,
                "total": halt.total,
            })
        }
        Err(Halted::Unsettled(why)) => json!({
            "state": "stacking",
            "batch": job.batch,
            "generation": job.generation,
            "unsettled": why,
        }),
    };
    said.to_string()
}

/// The generation's two refs made at its base, then one merge commit per member on the stack ref,
/// each read back before the ref moves to it. No checkout, index or branch is touched.
fn build(host: &Host, job: &StackJob) -> Result<Built, Halted> {
    let budget = job.policy.git_budget;
    let stopped = |step: u32, task: &str, fault: Fault| {
        Halted::Stop(Halt {
            step,
            task: task.to_string(),
            why: fault.stack_stop(),
            files: Vec::new(),
            total: 0,
        })
    };
    let first = job
        .members
        .first()
        .map(|member| member.task.clone())
        .unwrap_or_default();
    let zero = "0".repeat(job.base.len());
    let stack = stack_ref(&job.batch, job.generation);
    let made = format!(
        "land-batch {}: generation {} at {}",
        job.batch,
        job.generation,
        short(&job.base)
    );
    for name in [stack.clone(), integration_ref(&job.batch, job.generation)] {
        git_text(
            host,
            &job.repo,
            &["update-ref", "-m", &made, &name, &job.base, &zero],
            budget,
        )
        .map_err(|fault| stopped(1, &first, fault))?;
    }
    let size = job.members.len();
    let mut steps: Vec<String> = Vec::new();
    let mut tree = String::new();
    for (index, (member, text)) in job.members.iter().zip(&job.messages).enumerate() {
        let step = u32::try_from(index + 1).unwrap_or(u32::MAX);
        let halt = |fault: Fault| stopped(step, &member.task, fault);
        let previous = steps.last().cloned().unwrap_or_else(|| job.base.clone());
        let (code, said) = git_answer(
            &job.repo,
            &[
                "merge-tree",
                "--write-tree",
                "--name-only",
                "--no-messages",
                &previous,
                &member.head,
            ],
            budget,
        )
        .map_err(&halt)?;
        let merged = match parse_merge_probe(code, &said) {
            MergeProbe::Clean(merged) => merged,
            MergeProbe::Conflict(files) => {
                let total = u64::try_from(files.len()).unwrap_or(u64::MAX);
                return Err(Halted::Stop(Halt {
                    step,
                    task: member.task.clone(),
                    why: StackStop::Conflict,
                    files: files.into_iter().take(CONFLICT_NAMES).collect(),
                    total,
                }));
            }
            MergeProbe::Failed => {
                return Err(halt(Fault::Refused(format!(
                    "git merge-tree could not merge {} into step {step}'s base",
                    short(&member.head)
                ))));
            }
        };
        let message = landing_message(job, step, member, text);
        let commit = git_text(
            host,
            &job.repo,
            &[
                "commit-tree",
                &merged,
                "-p",
                &previous,
                "-p",
                &member.head,
                "-m",
                &message,
            ],
            budget,
        )
        .map_err(&halt)?;
        let parents = git_text(
            host,
            &job.repo,
            &["rev-list", "--parents", "-n", "1", &commit],
            budget,
        )
        .map_err(&halt)?;
        let made_tree = tree_of(host, &job.repo, &commit, budget).map_err(&halt)?;
        if !is_object_id(&commit)
            || parents != format!("{commit} {previous} {}", member.head)
            || made_tree != merged
        {
            return Err(halt(Fault::Refused(format!(
                "the merge commit {commit} of step {step} is not the merge it was made as"
            ))));
        }
        let moved = format!("land-batch {}: step {step} of {size}", job.batch);
        git_text(
            host,
            &job.repo,
            &["update-ref", "-m", &moved, &stack, &commit, &previous],
            budget,
        )
        .map_err(&halt)?;
        steps.push(commit);
        tree = merged;
    }
    Ok(Built { steps, tree })
}

/// The merge commit's message: the member's own landing message under one line that names it, and
/// a closing paragraph that names the batch, the generation, the step and the base.
fn landing_message(job: &StackJob, step: u32, member: &Member, text: &str) -> String {
    let mut lines = text.trim().lines();
    let first = lines.next().unwrap_or_default().trim();
    let rest = lines.collect::<Vec<_>>().join("\n");
    let rest = rest.trim();
    let mut message = format!(
        "Merge {} at {} — {first}\n",
        member.task,
        short9(&member.head)
    );
    if !rest.is_empty() {
        message.push('\n');
        message.push_str(rest);
        message.push('\n');
    }
    message.push_str(&format!(
        "\nStacked by the integration queue: batch {}, generation {},\nstep {step} of {}, on {} {}.\n",
        job.batch,
        job.generation,
        job.members.len(),
        branch_name(&job.base_branch),
        short9(&job.base)
    ));
    message
}

/// The publisher moves the generation's integration ref from its base to the tip, with the
/// operation's receipt. A tombstoned operation is refused, so a cancel wins over a late stacker.
fn publish(job: &StackJob, built: &Built) -> Result<(), Halted> {
    let step = u32::try_from(job.members.len()).unwrap_or(u32::MAX);
    let task = job
        .members
        .last()
        .map(|member| member.task.clone())
        .unwrap_or_default();
    let stopped = |why: StackStop| {
        Halted::Stop(Halt {
            step,
            task: task.clone(),
            why,
            files: Vec::new(),
            total: 0,
        })
    };
    let tip = built.steps.last().cloned().unwrap_or_default();
    let publisher =
        LocalGitRefPublisher::open(&job.repo).map_err(|_| stopped(StackStop::GitRefused))?;
    match publisher.move_integration_ref(&publication(
        &job.run,
        &job.batch,
        job.generation,
        &tip,
        &job.base,
    )) {
        Ok(()) => Ok(()),
        Err(PublisherFailure::Refused) => Err(stopped(StackStop::PublishRefused)),
        Err(other) => Err(Halted::Unsettled(other.to_string())),
    }
}

/// How many files more than one member changes since its fork from the base. Recorded only: the
/// gate judges the stacked whole. A diff git does not answer counts nothing.
fn overlaps(host: &Host, repo: &Path, base: &str, members: &[Member], budget: Duration) -> u64 {
    let mut touched: BTreeMap<String, u32> = BTreeMap::new();
    for member in members {
        let range = format!("{base}...{}", member.head);
        let Ok(said) = git_text(
            host,
            repo,
            &["diff", "--name-only", "--no-renames", &range],
            budget,
        ) else {
            continue;
        };
        for file in said.lines().filter(|line| !line.is_empty()) {
            *touched.entry(file.to_string()).or_insert(0) += 1;
        }
    }
    u64::try_from(touched.values().filter(|count| **count > 1).count()).unwrap_or(u64::MAX)
}

/// ③ Prepare the gate of S(`step`) through land-check's own prepare road, and hold its tree to the
/// step's tree: a compare ref that is not an ancestor of the step merges into another tree, and
/// that folder is taken down.
fn prepare(at: &At<'_>, batch: &BatchState, step: u32, target: &str) -> Result<String, String> {
    let said = at.roads.check(
        &LandCheckAsk::Merge {
            run: at.run.to_string(),
            task: batch.task.clone(),
            head: target.to_string(),
            checkout: batch.repository.clone(),
            prepare: true,
        },
        at.now_ms,
    )?;
    let answer: serde_json::Value = serde_json::from_str(&said)
        .map_err(|_| format!("land-check answered what is not JSON: {said}"))?;
    if answer["state"] != "prepared" {
        return Err(format!(
            "the gate of step {step} could not be prepared — land-check answered {said}"
        ));
    }
    let field = |key: &str| answer[key].as_str().unwrap_or_default().to_string();
    let (check, tree, folder, log) = (field("check"), field("tree"), field("path"), field("log"));
    let want =
        tree_of(at.host, at.repo, target, at.policy.git_budget).map_err(|fault| fault.said())?;
    if tree != want {
        let why = format!(
            "the prepared tree {} is not step {step}'s tree {}: the compare ref {} is not an \
             ancestor of {} — restack the batch on the new base",
            short(&tree),
            short(&want),
            answer["base"]["ref"].as_str().unwrap_or("?"),
            short(target)
        );
        take_down(at, batch, &check, target, &log, &why);
        return Err(format!("{why}; the gate folder was taken down"));
    }
    let prepared = Record::GatePrepared {
        check: check.clone(),
        step,
        target: target.to_string(),
        tree: tree.clone(),
        folder: folder.clone(),
        log: log.clone(),
    };
    if let Err(why) = at.record(&batch.batch, batch.generation, prepared) {
        take_down(at, batch, &check, target, &log, &why);
        return Err(why);
    }
    Ok(json!({
        "state": "prepared",
        "batch": batch.batch,
        "generation": batch.generation,
        "check": check,
        "step": step,
        "target": target,
        "tree": tree,
        "path": folder,
        "log": log,
        "gate": "the gate is a set of build-line jobs: give each one `# cwd:` at path, write their \
                 per-job RC summary to log, then --record the check with --rc 0 only when every \
                 job passed and --log naming that summary",
    })
    .to_string())
}

/// Take a prepared gate folder down through land-check's own record road with its letter dropped:
/// the folder, its worktree entry and its row go as a record takes them. The log keeps the reason.
fn take_down(at: &At<'_>, batch: &BatchState, check: &str, head: &str, log: &str, why: &str) {
    let log_path = Path::new(log);
    if log_path.file_name() == Some(std::ffi::OsStr::new(&format!("{check}.log"))) {
        if let Some(parent) = log_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(mut file) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
        {
            let _ = writeln!(file, "land-batch {}: taken down unrun — {why}", batch.batch);
        }
    }
    let _ = at.roads.quiet(
        &LandCheckAsk::Record {
            run: at.run.to_string(),
            task: batch.task.clone(),
            head: head.to_string(),
            check: check.to_string(),
            rc: UNRUN_RC,
            log: log.to_string(),
            took_ms: None,
        },
        at.now_ms,
    );
}

/// ③ The end of the gate's jobs: land-check's record road writes its letter and takes the folder
/// down, then the batch's `gated`. In a culprit search the answer names the next step to gate.
fn record(
    at: &At<'_>,
    batch: &BatchState,
    check: &str,
    rc: i32,
    log: &str,
    took_ms: Option<i64>,
) -> Result<String, String> {
    let prepared = batch
        .prepared
        .as_ref()
        .filter(|held| held.check == check)
        .ok_or_else(|| format!("batch {} has no prepared gate {check}", batch.batch))?;
    let said = at.roads.check(
        &LandCheckAsk::Record {
            run: at.run.to_string(),
            task: batch.task.clone(),
            head: prepared.target.clone(),
            check: check.to_string(),
            rc,
            log: log.to_string(),
            took_ms,
        },
        at.now_ms,
    )?;
    let verdict = Verdict::of(rc);
    at.record(
        &batch.batch,
        batch.generation,
        Record::Gated {
            check: check.to_string(),
            step: prepared.step,
            target: prepared.target.clone(),
            tree: prepared.tree.clone(),
            verdict,
            rc,
            took_ms,
            log: log.to_string(),
        },
    )?;
    let mut answer = json!({
        "state": verdict.word(),
        "batch": batch.batch,
        "generation": batch.generation,
        "check": check,
        "step": prepared.step,
        "rc": rc,
        "evidence": serde_json::from_str::<serde_json::Value>(&said).unwrap_or_default(),
    });
    if let Some(search) = batch.search.as_ref().filter(|_| batch.culprit.is_none()) {
        let (lo, hi) = match verdict {
            Verdict::Green => (prepared.step, search.hi),
            Verdict::Red => (search.lo, prepared.step),
        };
        match bisect_next(lo, hi) {
            Some(next) => {
                answer["next"] = json!({ "step": next, "target": batch.step_commit(next) });
            }
            None => {
                answer["culprit"] = json!(
                    usize::try_from(hi)
                        .ok()
                        .and_then(|at| at.checked_sub(1))
                        .and_then(|at| batch.members.get(at))
                        .map(|member| member.task.clone())
                );
            }
        }
    }
    Ok(answer.to_string())
}

/// ⑤ Start a culprit search over this generation's steps: the base green, the tip red.
fn bisect(at: &At<'_>, batch: &BatchState) -> Result<String, String> {
    let size = batch.size();
    let next = bisect_next(0, size);
    at.record(
        &batch.batch,
        batch.generation,
        Record::BisectStarted {
            lo: 0,
            hi: size,
            next,
        },
    )?;
    Ok(match next {
        Some(step) => json!({
            "state": "searching",
            "batch": batch.batch,
            "generation": batch.generation,
            "next": { "step": step, "target": batch.step_commit(step) },
        }),
        None => json!({
            "state": "culprit_found",
            "batch": batch.batch,
            "generation": batch.generation,
            "task": batch.members.first().map(|member| member.task.clone()),
        }),
    }
    .to_string())
}

/// ⑤ Restack: generation g+1 on the base branch as it stands, without the members the press drops
/// and without the ones that reached the base branch meanwhile. The old generation's refs stay.
fn restack(at: &At<'_>, batch: &BatchState, drop: &[String]) -> Result<String, String> {
    let budget = at.policy.git_budget;
    let base = commit_of(at.host, at.repo, &batch.base_branch, budget)
        .ok_or_else(|| format!("the base branch {} names no commit", batch.base_branch))?;
    let culprit = batch.culprit.as_ref().map(|found| found.task.as_str());
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    for member in &batch.members {
        let why = if drop.contains(&member.task) {
            Some(if culprit == Some(member.task.as_str()) {
                DropWhy::Culprit
            } else {
                DropWhy::Asked
            })
        } else if is_ancestor(at.host, at.repo, &member.head, &base, budget)
            .map_err(|fault| fault.said())?
        {
            Some(DropWhy::OnMain)
        } else {
            None
        };
        match why {
            Some(why) => dropped.push(Dropped {
                task: member.task.clone(),
                why,
            }),
            None => kept.push(member.clone()),
        }
    }
    if kept.is_empty() {
        return Err(format!(
            "every member of batch {} is dropped or already on {} — nothing is left to stack; \
             cancel the batch instead",
            batch.batch,
            branch_name(&batch.base_branch)
        ));
    }
    let messages = kept
        .iter()
        .map(|member| kept_message(&at.roads.folder, &batch.batch, &member.task))
        .collect::<Result<Vec<_>, _>>()?;
    let generation = batch.generation.saturating_add(1);
    at.record(
        &batch.batch,
        generation,
        Record::Restacked {
            members: kept.clone(),
            dropped,
            base: base.clone(),
        },
    )?;
    if let Some(prepared) = &batch.prepared {
        take_down(
            at,
            batch,
            &prepared.check,
            &prepared.target,
            &prepared.log,
            "the batch was restacked",
        );
    }
    tombstone(at, batch);
    stack(
        at,
        StackJob {
            run: at.run.to_string(),
            batch: batch.batch.clone(),
            generation,
            members: kept,
            messages,
            base_branch: batch.base_branch.clone(),
            base,
            repo: at.repo.to_path_buf(),
            by: at.by.clone(),
            policy: at.policy.clone(),
        },
    )
}

/// ⑥ Cancel: `cancelled` first, then this generation's publish tombstoned and its prepared gate
/// folder taken down. Refs stay for [`INTEGRATION_KEEP_MS`].
fn cancel(at: &At<'_>, batch: &BatchState) -> Result<String, String> {
    at.record(&batch.batch, batch.generation, Record::Cancelled {})?;
    let tombstoned = tombstone(at, batch);
    if let Some(prepared) = &batch.prepared {
        take_down(
            at,
            batch,
            &prepared.check,
            &prepared.target,
            &prepared.log,
            "the batch was cancelled",
        );
    }
    Ok(json!({
        "state": "cancelled",
        "batch": batch.batch,
        "generation": batch.generation,
        "tombstoned": tombstoned,
    })
    .to_string())
}

/// Settle this generation's publish: one that never applied gets its tombstone, so no stacker
/// still running can publish it. Answers whether it stands tombstoned now.
fn tombstone(at: &At<'_>, batch: &BatchState) -> bool {
    if batch.stacking.is_none() {
        return false;
    }
    let budget = at.policy.git_budget;
    let tip = batch
        .stacked
        .as_ref()
        .map(|stack| stack.tip.clone())
        .or_else(|| {
            ref_value(
                at.host,
                at.repo,
                &stack_ref(&batch.batch, batch.generation),
                budget,
            )
        })
        .unwrap_or_else(|| batch.base.clone());
    let publication = publication(at.run, &batch.batch, batch.generation, &tip, &batch.base);
    let settled = LocalGitRefPublisher::open(at.repo)
        .map_err(|_| PublisherFailure::Unavailable)
        .and_then(|publisher| publisher.settle_operation(&publication));
    match settled {
        Ok(PublishVerification::NotAppliedFinal(_)) => true,
        Ok(_) => false,
        Err(_) => ref_value(
            at.host,
            at.repo,
            &format!("{CANCELLED_REF_PREFIX}{}", publication.operation_id),
            budget,
        )
        .is_some(),
    }
}

/// ④ Promote: `promoting` first, then its git on its own thread. The press hears the end when it
/// comes within [`Policy::answer_budget`], and "still promoting" otherwise: the reads before the
/// fast-forward may each take [`Policy::git_budget`], longer together than the bridge holds a press.
fn promote(at: &At<'_>, batch: &BatchState) -> Result<String, String> {
    let stack = batch
        .stacked
        .as_ref()
        .ok_or_else(|| format!("batch {} has nothing stacked to promote", batch.batch))?;
    let promoting = Promoting {
        by: at.by.clone(),
        at: at.now_ms,
        target: stack.tip.clone(),
        expect: batch.base.clone(),
        base_branch: batch.base_branch.clone(),
        lease_until_ms: at.now_ms.saturating_add(promote_lease_ms(at.policy)),
    };
    at.record(
        &batch.batch,
        batch.generation,
        Record::Promoting {
            target: promoting.target.clone(),
            expect: promoting.expect.clone(),
            base_branch: promoting.base_branch.clone(),
            lease_until_ms: promoting.lease_until_ms,
        },
    )?;
    let (target, lease_until_ms) = (promoting.target.clone(), promoting.lease_until_ms);
    let (answer, first_answer) = mpsc::channel::<Result<String, String>>();
    let mark = mark_live(at.run, &batch.batch);
    let (run, by, state, repo, policy, roads, now_ms) = (
        at.run.to_string(),
        at.by.clone(),
        batch.clone(),
        at.repo.to_path_buf(),
        at.policy.clone(),
        at.roads.clone(),
        at.now_ms,
    );
    let spawned = std::thread::Builder::new()
        .name("land-batch-promoting".to_string())
        .spawn(move || {
            let host = Host::for_workspace(&repo);
            let at = At {
                run: &run,
                by: &by,
                repo: &repo,
                host: &host,
                policy: &policy,
                roads: &roads,
                now_ms,
            };
            let said = promote_job(&at, &state, promoting);
            drop(mark);
            let _ = answer.send(said);
        });
    if let Err(why) = spawned {
        return Err(format!(
            "the promote could not start: {why} — the next run settles batch {}",
            batch.batch
        ));
    }
    match first_answer.recv_timeout(at.policy.answer_budget) {
        Ok(said) => said,
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(json!({
            "state": "promoting",
            "still": true,
            "batch": batch.batch,
            "generation": batch.generation,
            "target": target,
            "leaseUntilMs": lease_until_ms,
            "message": "still promoting: git is still reading or fast-forwarding the base branch; \
                        it is not stopped, and the promote's end is written when git ends",
        })
        .to_string()),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            Err("the promote stopped before it answered".to_string())
        }
    }
}

/// The promote's git on its own thread: the base branch, its checkout and that checkout's
/// unfinished work read fresh, then the fast-forward — never killed. Past
/// [`Policy::promote_budget`] a watcher holds git and the settle road writes the end when git has
/// one. Answers what the press hears if it still waits.
fn promote_job(at: &At<'_>, batch: &BatchState, promoting: Promoting) -> Result<String, String> {
    let budget = at.policy.git_budget;
    let Some(base_now) = commit_of(at.host, at.repo, &promoting.base_branch, budget) else {
        return promote_stopped(at, batch, PromoteStop::GitRefused, Vec::new(), 0, None);
    };
    if base_now != promoting.expect {
        let between = is_ancestor(at.host, at.repo, &promoting.expect, &base_now, budget)
            .unwrap_or(false)
            && is_ancestor(at.host, at.repo, &base_now, &promoting.target, budget).unwrap_or(false);
        if !between {
            return promote_stopped(
                at,
                batch,
                PromoteStop::MainMoved,
                Vec::new(),
                0,
                Some(base_now),
            );
        }
    }
    let checkouts = match checkouts_of(at.host, at.repo, &promoting.base_branch, budget) {
        Ok(found) => found,
        Err(fault) => {
            return promote_stopped(
                at,
                batch,
                fault.promote_stop(),
                Vec::new(),
                0,
                Some(base_now),
            );
        }
    };
    let checkout = match checkouts.as_slice() {
        [] => {
            let moved = format!("land-batch {}: fast-forward to the green tip", batch.batch);
            let updated = git_text(
                at.host,
                at.repo,
                &[
                    "update-ref",
                    "-m",
                    &moved,
                    &promoting.base_branch,
                    &promoting.target,
                    &base_now,
                ],
                budget,
            );
            return after_fast_forward(at, batch, &promoting, &base_now, updated.is_ok());
        }
        [one] => one.clone(),
        many => {
            let total = u64::try_from(many.len()).unwrap_or(u64::MAX);
            let named = many
                .iter()
                .take(CONFLICT_NAMES)
                .map(|path| path.display().to_string())
                .collect();
            return promote_stopped(
                at,
                batch,
                PromoteStop::TwoCheckouts,
                named,
                total,
                Some(base_now),
            );
        }
    };
    match unfinished(at.host, &checkout, budget) {
        Ok((_, 0)) => {}
        Ok((files, total)) => {
            return promote_stopped(
                at,
                batch,
                PromoteStop::UnfinishedFiles,
                files,
                total,
                Some(base_now),
            );
        }
        Err(fault) => {
            return promote_stopped(
                at,
                batch,
                fault.promote_stop(),
                Vec::new(),
                0,
                Some(base_now),
            );
        }
    }
    let mut child = match start_fast_forward(&at.policy.git, &checkout, &promoting.target) {
        Ok(child) => child,
        Err(_) => {
            return promote_stopped(
                at,
                batch,
                PromoteStop::GitRefused,
                Vec::new(),
                0,
                Some(base_now),
            );
        }
    };
    if let Some(status) = exit_within(&mut child, at.policy.promote_budget) {
        return after_fast_forward(at, batch, &promoting, &base_now, status.success());
    }
    still_promoting(at, batch, promoting, child, &checkout)
}

/// `git merge --ff-only <tip>` in the base branch's checkout. Its output goes nowhere, so no
/// closed pipe can end it either.
fn start_fast_forward(git: &Path, checkout: &Path, target: &str) -> std::io::Result<Child> {
    lock_free_git(crate::proc::quiet_command(git))
        .args(["merge", "--ff-only", target])
        .current_dir(checkout)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// A fast-forward past its budget: it runs on, watched by a thread that settles the promote when
/// git ends. The press hears "still promoting".
fn still_promoting(
    at: &At<'_>,
    batch: &BatchState,
    promoting: Promoting,
    mut child: Child,
    checkout: &Path,
) -> Result<String, String> {
    let mark = mark_live(at.run, &batch.batch);
    let (run, state, repo, policy, roads) = (
        at.run.to_string(),
        batch.clone(),
        at.repo.to_path_buf(),
        at.policy.clone(),
        at.roads.clone(),
    );
    let target = promoting.target.clone();
    let watched = std::thread::Builder::new()
        .name("land-batch-promote".to_string())
        .spawn(move || {
            let _live = mark;
            let _ = child.wait();
            let host = Host::for_workspace(&repo);
            let at = At {
                run: &run,
                by: &PressedBy::Ledger,
                repo: &repo,
                host: &host,
                policy: &policy,
                roads: &roads,
                now_ms: crate::now_epoch_ms(),
            };
            settle_promote(&at, &state, &promoting);
        })
        .is_ok();
    Ok(json!({
        "state": "promoting",
        "still": true,
        "watched": watched,
        "batch": batch.batch,
        "generation": batch.generation,
        "target": target,
        "checkout": checkout.display().to_string(),
        "message": "still promoting: git is still fast-forwarding the base branch; it is not \
                    stopped, and the promote's end is written when git ends",
    })
    .to_string())
}

/// What a promote left, read fresh: the base branch, and its checkout's head and unfinished work.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Left {
    /// The base branch and its checkout stand at the tip, and nothing is unfinished.
    AtTip,
    /// Both stand together at this commit, and nothing is unfinished.
    Clean(String),
    /// Anything else, as it was seen.
    Other {
        files: Vec<String>,
        total: u64,
        seen: Option<String>,
    },
}

fn left_by_promote(host: &Host, repo: &Path, branch: &str, target: &str, budget: Duration) -> Left {
    let other =
        |files: Vec<String>, total: u64, seen: Option<String>| Left::Other { files, total, seen };
    let Some(base) = commit_of(host, repo, branch, budget) else {
        return other(Vec::new(), 0, None);
    };
    let Ok(checkouts) = checkouts_of(host, repo, branch, budget) else {
        return other(Vec::new(), 0, Some(base));
    };
    match checkouts.as_slice() {
        [] => {}
        [checkout] => {
            let head = git_text(host, checkout, &["rev-parse", "HEAD"], budget).ok();
            match unfinished(host, checkout, budget) {
                Ok((files, total)) if total > 0 || head.as_deref() != Some(base.as_str()) => {
                    return other(files, total, Some(base));
                }
                Ok(_) => {}
                Err(_) => return other(Vec::new(), 0, Some(base)),
            }
        }
        _ => return other(Vec::new(), 0, Some(base)),
    }
    if base == target {
        Left::AtTip
    } else {
        Left::Clean(base)
    }
}

/// The end of a fast-forward that ended in its budget, read fresh: `promoted` only when the base
/// branch and its checkout stand at the tip with nothing unfinished.
fn after_fast_forward(
    at: &At<'_>,
    batch: &BatchState,
    promoting: &Promoting,
    base_before: &str,
    exited_ok: bool,
) -> Result<String, String> {
    let budget = at.policy.git_budget;
    let left = left_by_promote(
        at.host,
        at.repo,
        &promoting.base_branch,
        &promoting.target,
        budget,
    );
    let (why, files, total, seen) = match left {
        Left::AtTip => {
            at.record(
                &batch.batch,
                batch.generation,
                Record::Promoted {
                    from: promoting.expect.clone(),
                    to: promoting.target.clone(),
                    members: landed(batch),
                    settled_later: false,
                },
            )?;
            return Ok(json!({
                "state": "promoted",
                "batch": batch.batch,
                "generation": batch.generation,
                "from": promoting.expect,
                "to": promoting.target,
            })
            .to_string());
        }
        Left::Clean(seen) if seen == base_before => {
            let why = if exited_ok {
                PromoteStop::UnknownState
            } else {
                PromoteStop::GitRefused
            };
            (why, Vec::new(), 0, Some(seen))
        }
        Left::Clean(seen) => (PromoteStop::MainMoved, Vec::new(), 0, Some(seen)),
        Left::Other { files, total, seen } => {
            let why = if seen.as_deref() != Some(base_before) {
                PromoteStop::MainMoved
            } else if total > 0 {
                PromoteStop::UnfinishedFiles
            } else {
                PromoteStop::GitRefused
            };
            (why, files, total, seen)
        }
    };
    promote_stopped(at, batch, why, files, total, seen)
}

fn promote_stopped(
    at: &At<'_>,
    batch: &BatchState,
    why: PromoteStop,
    files: Vec<String>,
    total: u64,
    seen: Option<String>,
) -> Result<String, String> {
    at.record(
        &batch.batch,
        batch.generation,
        Record::PromoteStopped {
            why,
            files: files.clone(),
            total,
            seen: seen.clone(),
        },
    )?;
    Ok(json!({
        "state": "promote_stopped",
        "batch": batch.batch,
        "generation": batch.generation,
        "why": why,
        "files": files,
        "total": total,
        "seen": seen,
    })
    .to_string())
}

/// Each member as it landed: its head and the step's merge commit that carried it.
fn landed(batch: &BatchState) -> Vec<Landed> {
    let steps = batch
        .stacked
        .as_ref()
        .map(|stack| stack.steps.as_slice())
        .unwrap_or_default();
    batch
        .members
        .iter()
        .zip(steps)
        .map(|(member, step)| Landed {
            task: member.task.clone(),
            head: member.head.clone(),
            merged_as: step.clone(),
        })
        .collect()
}

/// The checkouts that have `branch` checked out, from `git worktree list --porcelain`. No path is
/// assumed; a listed folder that is gone is no checkout.
fn checkouts_of(
    host: &Host,
    repo: &Path,
    branch: &str,
    budget: Duration,
) -> Result<Vec<PathBuf>, Fault> {
    let said = git_raw(host, repo, &["worktree", "list", "--porcelain"], budget)?;
    let mut found = Vec::new();
    let mut path: Option<PathBuf> = None;
    for line in said.lines() {
        if let Some(listed) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(listed));
        } else if line.strip_prefix("branch ") == Some(branch)
            && let Some(listed) = &path
            && listed.is_dir()
        {
            found.push(listed.clone());
        }
    }
    Ok(found)
}

/// A checkout's unfinished work, read fresh: tracked files changed or staged, and the marks of a
/// merge, rebase, cherry-pick or revert in progress or of a held index. At most
/// [`CONFLICT_NAMES`] names, and the true count.
fn unfinished(host: &Host, checkout: &Path, budget: Duration) -> Result<(Vec<String>, u64), Fault> {
    let said = git_raw(
        host,
        checkout,
        &["status", "--porcelain=v1", "-z", "--untracked-files=no"],
        budget,
    )?;
    let mut names = Vec::new();
    let mut entries = said.split('\0').filter(|entry| !entry.is_empty());
    while let Some(entry) = entries.next() {
        let (Some(code), Some(name)) = (entry.get(..2), entry.get(3..)) else {
            continue;
        };
        names.push(name.to_string());
        if code.contains(['R', 'C']) {
            entries.next();
        }
    }
    let folder = git_text(host, checkout, &["rev-parse", "--absolute-git-dir"], budget)?;
    names.extend(
        UNFINISHED_MARKS
            .iter()
            .filter(|mark| Path::new(&folder).join(mark).exists())
            .map(|mark| (*mark).to_string()),
    );
    let total = u64::try_from(names.len()).unwrap_or(u64::MAX);
    names.truncate(CONFLICT_NAMES);
    Ok((names, total))
}

/// The next run's look at what earlier runs left: a stacking or a promote whose lease ended with no
/// record of its end, and ended batches past [`INTEGRATION_KEEP_MS`]. Only where the queue is on,
/// never the batch being pressed, and never work this window still runs.
pub(crate) fn settle(
    policy_for: &dyn Fn(&Path) -> Policy,
    held: &[BatchState],
    run: &str,
    pressed: Option<&str>,
    now_ms: i64,
    roads: &Roads,
) {
    for state in held {
        if pressed == Some(state.batch.as_str())
            || !is_batch_id(&state.batch)
            || live(run, &state.batch)
        {
            continue;
        }
        let repo = PathBuf::from(&state.repository);
        let policy = policy_for(&repo);
        if !policy.enabled {
            continue;
        }
        let host = Host::for_workspace(&repo);
        let at = At {
            run,
            by: &PressedBy::Ledger,
            repo: &repo,
            host: &host,
            policy: &policy,
            roads,
            now_ms,
        };
        if state.ended() {
            if !state.swept && state.last_at.saturating_add(INTEGRATION_KEEP_MS) < now_ms {
                sweep(&at, state);
            }
        } else if state.stacking_open() {
            if let Some(stacking) = state
                .stacking
                .as_ref()
                .filter(|stacking| stacking.lease_until_ms < now_ms)
            {
                settle_stacking(&at, state, &stacking.operation);
            }
        } else if let Some(promoting) = state
            .promoting
            .as_ref()
            .filter(|promoting| promoting.lease_until_ms < now_ms)
        {
            settle_promote(&at, state, promoting);
        }
    }
}

/// A stacking whose lease ended: an applied publish is the stack it was, written late; one that
/// never applied is tombstoned by the settle itself and abandoned.
fn settle_stacking(at: &At<'_>, state: &BatchState, operation: &str) {
    let budget = at.policy.git_budget;
    let tip = ref_value(
        at.host,
        at.repo,
        &stack_ref(&state.batch, state.generation),
        budget,
    )
    .unwrap_or_else(|| state.base.clone());
    let Ok(publisher) = LocalGitRefPublisher::open(at.repo) else {
        return;
    };
    let record = match publisher.settle_operation(&publication(
        at.run,
        &state.batch,
        state.generation,
        &tip,
        &state.base,
    )) {
        Ok(PublishVerification::Applied(_)) => match chain_of(at, state, &tip) {
            Some((steps, tree)) => Record::Stacked {
                steps,
                tip: tip.clone(),
                tree,
                took_ms: at.now_ms.saturating_sub(state.last_at).max(0),
                overlaps: overlaps(at.host, at.repo, &state.base, &state.members, budget),
            },
            None => Record::StackStopped {
                step: state.size(),
                task: state
                    .members
                    .last()
                    .map(|member| member.task.clone())
                    .unwrap_or_default(),
                why: StackStop::GitRefused,
                files: Vec::new(),
                total: 0,
            },
        },
        Ok(PublishVerification::NotAppliedFinal(_)) => Record::StackAbandoned {
            operation: operation.to_string(),
        },
        Ok(PublishVerification::Pending) | Err(_) => return,
    };
    let _ = at.roads.record(
        at.run,
        &state.batch,
        state.generation,
        &PressedBy::Ledger,
        record,
    );
}

/// The steps of a published tip, read back one merge at a time: each step's parents are the step
/// before and its member's head, and the first stands on the base. Answers the tip's tree too.
fn chain_of(at: &At<'_>, state: &BatchState, tip: &str) -> Option<(Vec<String>, String)> {
    let budget = at.policy.git_budget;
    let mut steps = Vec::new();
    let mut at_step = tip.to_string();
    for member in state.members.iter().rev() {
        let parents = git_text(
            at.host,
            at.repo,
            &["rev-list", "--parents", "-n", "1", &at_step],
            budget,
        )
        .ok()?;
        let named: Vec<&str> = parents.split(' ').collect();
        let [commit, previous, head] = named.as_slice() else {
            return None;
        };
        if *commit != at_step || *head != member.head {
            return None;
        }
        steps.push(at_step.clone());
        at_step = (*previous).to_string();
    }
    if at_step != state.base {
        return None;
    }
    steps.reverse();
    let tree = tree_of(at.host, at.repo, tip, budget).ok()?;
    Some((steps, tree))
}

/// A promote whose lease ended, or whose fast-forward outlived its budget and has ended now: the
/// base branch at the tip with its checkout clean is `promoted` in the presser's name; still at
/// the base and clean, `promote_abandoned`; anything else `promote_stopped` as `unknown_state`.
/// Nothing is reset.
fn settle_promote(at: &At<'_>, state: &BatchState, promoting: &Promoting) {
    let left = left_by_promote(
        at.host,
        at.repo,
        &promoting.base_branch,
        &promoting.target,
        at.policy.git_budget,
    );
    let (by, record) = match left {
        Left::AtTip => (
            promoting.by.clone(),
            Record::Promoted {
                from: promoting.expect.clone(),
                to: promoting.target.clone(),
                members: landed(state),
                settled_later: true,
            },
        ),
        Left::Clean(seen) if seen == promoting.expect => {
            (PressedBy::Ledger, Record::PromoteAbandoned { seen })
        }
        Left::Clean(seen) => (
            PressedBy::Ledger,
            Record::PromoteStopped {
                why: PromoteStop::UnknownState,
                files: Vec::new(),
                total: 0,
                seen: Some(seen),
            },
        ),
        Left::Other { files, total, seen } => (
            PressedBy::Ledger,
            Record::PromoteStopped {
                why: PromoteStop::UnknownState,
                files,
                total,
                seen,
            },
        ),
    };
    let _ = at
        .roads
        .record(at.run, &state.batch, state.generation, &by, record);
}

/// An ended batch past its keep: every generation's stack and integration ref and publish receipt
/// deleted, its kept messages removed, and `swept` with the count.
fn sweep(at: &At<'_>, state: &BatchState) {
    let budget = at.policy.git_budget;
    let mut refs = Vec::new();
    for generation in 1..=state.generation {
        let operation = operation_id(at.run, &state.batch, generation);
        refs.push(stack_ref(&state.batch, generation));
        refs.push(integration_ref(&state.batch, generation));
        refs.push(format!("{APPLIED_REF_PREFIX}{operation}"));
        refs.push(format!("{CANCELLED_REF_PREFIX}{operation}"));
    }
    let mut removed: u64 = 0;
    for name in &refs {
        if let Some(oid) = ref_value(at.host, at.repo, name, budget)
            && git_text(at.host, at.repo, &["update-ref", "-d", name, &oid], budget).is_ok()
        {
            removed += 1;
        }
    }
    forget_messages(&at.roads.folder, &state.batch);
    let _ = at.roads.record(
        at.run,
        &state.batch,
        state.generation,
        &PressedBy::Ledger,
        Record::Swept { refs: removed },
    );
}

#[cfg(test)]
mod tests;
