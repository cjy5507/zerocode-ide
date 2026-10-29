//! A coordinator's own attempt on a task (t-78075): `task-take`, `hand-in` and `task-backfill`.
//!
//! A review names the attempt and the source it looked at, and an attempt has a source only when
//! a report ended it ([`Dispatch::source`]). So work a coordinator took over from a worker that
//! handed nothing in — or did by hand from the start — could take no review at all: `task-update`
//! refused it ("the attempt has not handed in a source yet"), and the only road left was closing
//! the task `outdated` with the facts in its result, where the board and the review count never
//! read them.
//!
//! This file fills the steps BEFORE the review and nothing else. The coordinator takes the task
//! as an attempt of its own seat ([`TAKE_VERB`]), hands a commit in on it ([`HAND_IN_VERB`]) and
//! then reviews it with the same `task-update --attempt --source --result` every other review
//! goes through. The ledger writes only what it heard: the commit is one git answered for, read
//! by the window before the plan ([`CommitsSeen`]) because the ledger runs no git and no process
//! ([`super::land_check`] keeps the same split). [`BACKFILL_VERB`] walks the three steps for old
//! finished work whose result already names a landed commit, writing only what git proves.

use std::collections::BTreeMap;

use super::*;

/// The verb that opens a coordinator's own attempt on a task.
pub const TAKE_VERB: &str = "task-take";

/// The verb that hands a commit in on a coordinator's own attempt.
pub const HAND_IN_VERB: &str = "hand-in";

/// The verb that writes the three steps for finished work whose result names a landed commit.
pub const BACKFILL_VERB: &str = "task-backfill";

/// What a coordinator's own attempt names in [`Dispatch::worker`], before its seat's generation:
/// `coordinator@g3`. Not a worker id — no worker row carries it — and read back by
/// [`carrier_generation`] alone.
pub const COORDINATOR_CARRIER: &str = "coordinator@g";

/// The ref a backfill measures landed commits against when `--base` names none.
pub const DEFAULT_BASE: &str = "main";

/// The tags a backfill reads as releases when `--tags` names none: a glob, as `git tag --list`
/// takes it.
pub const RELEASE_TAGS: &str = "v*";

/// The carrier a coordinator seat of this generation writes on its own attempt.
#[must_use]
pub fn coordinator_carrier(generation: u32) -> String {
    format!("{COORDINATOR_CARRIER}{generation}")
}

/// The generation of the coordinator seat carrying an attempt, when a coordinator carries it.
#[must_use]
pub fn carrier_generation(worker: &str) -> Option<u32> {
    worker.strip_prefix(COORDINATOR_CARRIER)?.parse().ok()
}

/// What git said about the commits a hand-in or a backfill names.
///
/// Read by the window before the ledger plans, in the checkout the verb names, and judged here:
/// the ledger runs no git, so a commit it was not told about is a commit it does not know. Keyed
/// by the commit as it was named, in lower case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitsSeen {
    /// The checkout git was asked in.
    pub repo: String,
    /// The ref the commits were measured against, as named, when the verb named one.
    pub base: Option<BaseSeen>,
    /// The glob of release tags each commit was looked for in, when the verb asked.
    pub tags: Option<String>,
    pub commits: BTreeMap<String, CommitSeen>,
}

/// The ref a verb measured against, and the commit git resolved it to — `None` when git does not
/// know the name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseSeen {
    pub name: String,
    pub head: Option<String>,
}

/// One commit, as git answered for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitSeen {
    /// The full hash git resolved the name to; `None` when git knows no such commit.
    pub full: Option<String>,
    /// Whether the commit is the base or one of its ancestors; `None` when nothing was asked or
    /// git could not say.
    pub in_base: Option<bool>,
    /// The first release tag (in version order) that holds the commit, when one does.
    pub tag: Option<String>,
}

impl CommitsSeen {
    /// What git said about `named`, whatever case it was written in.
    #[must_use]
    pub fn commit(&self, named: &str) -> Option<&CommitSeen> {
        self.commits.get(&named.trim().to_ascii_lowercase())
    }

    /// Write down what git said about `named`.
    pub fn insert(&mut self, named: &str, seen: CommitSeen) {
        self.commits.insert(named.trim().to_ascii_lowercase(), seen);
    }
}

/// The keys a result can name a landed commit under, in the order a backfill believes them: the
/// commit a merge made first, a worker's branch head last — a head that was rebased onto `main`
/// is not the commit `main` holds, and git says so. Read at the top of the result and one object
/// down (`{"landed":{"mergeHead":…}}`), and from the end of a list (`"commits":[…]`).
const LANDED_COMMIT_KEYS: &[&str] = &[
    "mergeHead",
    "merged",
    "landedCommit",
    "landed",
    "onMain",
    "mergeCommit",
    "merge",
    "commit",
    "commits",
    "sha",
    "head",
    "tip",
    "branchTip",
];

/// What every task id starts with, as [`Ledger::mint`] cuts one.
const TASK_PREFIX: &str = "t-";

/// The key a backfill writes what it did under, beside the original result.
const BACKFILL_KEY: &str = "backfill";

/// Whether the review standing on this task was written by [`BACKFILL_VERB`] and not by a
/// coordinator who watched the work land: its `completed_ms` is the moment of the backfill, not of
/// the landing, so a time measured from it says nothing (review t-79586 P3-4). A later review the
/// coordinator writes replaces the whole result, the note with it.
pub(super) fn backfilled(task: &Task) -> bool {
    serde_json::from_str::<serde_json::Value>(&task.result).is_ok_and(|result| {
        result
            .get(BACKFILL_KEY)
            .and_then(|note| note.get("by"))
            .and_then(serde_json::Value::as_str)
            == Some(BACKFILL_VERB)
    })
}

/// What a backfill reads a review task's id under: any key that speaks of a review
/// (`reviews`, `reviewTask`, `reviewedIn`…), in lower case.
const REVIEW_WORD: &str = "review";

/// The flags each verb takes; anything else is refused rather than ignored.
const TAKE_FLAGS: &[&str] = &["--task", "--from", "--why", "--run", RETRY_REQUEST];
const HAND_IN_FLAGS: &[&str] = &[
    "--task",
    "--head",
    "--body",
    "--base",
    "--checkout",
    "--run",
    RETRY_REQUEST,
];
const BACKFILL_FLAGS: &[&str] = &[
    "--base",
    "--tags",
    "--checkout",
    "--apply",
    "--run",
    RETRY_REQUEST,
];

fn only_these(verb: &str, words: &Words, accepted: &[&str]) -> Result<(), String> {
    if let Some(flag) = words
        .values
        .iter()
        .map(|(name, _)| name)
        .chain(words.flags.iter())
        .find(|name| !accepted.contains(&name.as_str()))
    {
        return Err(format!(
            "{verb} does not support {flag}; it takes {}",
            accepted.join(" ")
        ));
    }
    if !words.positional.is_empty() {
        return Err(format!("{verb} does not accept positional arguments"));
    }
    Ok(())
}

/// The live coordinator seat, when the caller is sitting in it: its `team/pane` and generation.
///
/// Asked of [`sender`] as `task-update` asks it, so a pane that cannot sign as the coordinator
/// cannot take or hand in as one either.
fn the_seat(
    ledger: &Ledger,
    team: &Team,
    pane: &str,
    run_id: &str,
    verb: &str,
) -> Result<(String, u32), String> {
    let run = ledger.run(run_id).ok_or_else(|| unknown_run(run_id))?;
    let signed = sender(ledger, &team.leader_pane, run_id, (&team.id, pane));
    if let Some(worker) = signed.strip_prefix(WORKER_ADDRESS_PREFIX) {
        return Err(format!(
            "{verb} is the coordinator's verb, and this pane is worker {worker} — a worker hands \
             its own work in with `send --type worker_done`"
        ));
    }
    let Some(held) = run.coordinator_live() else {
        return Err(format!(
            "{verb} is the live coordinator seat's verb, and run {run_id} has nobody sitting — sit \
             first with `run-use {run_id}`"
        ));
    };
    if signed != run.address() {
        return Err(format!(
            "{verb} is the coordinator's verb: run {run_id} is coordinated from {} (generation \
             {}), and pane {}/{pane} is not that seat",
            held.seat, held.generation, team.id
        ));
    }
    Ok((held.seat.clone(), held.generation))
}

/// What a take wrote.
struct Taken {
    dispatch: String,
    retry_of: Option<String>,
    /// A coordinator attempt of an earlier generation the take ended.
    closed: Option<String>,
    status: TaskStatus,
}

impl Ledger {
    /// Open an attempt on `task_id` carried by the coordinator seat of `generation`.
    ///
    /// Every refusal comes before anything is written. A worker still carrying the task keeps it —
    /// the coordinator stops it first, which is the road that ends an attempt with a worker behind
    /// it; an attempt an earlier sitting of the seat took is ended here, since nobody is behind it
    /// any more. A closed task takes an attempt only on the backfill's road, which hands it in in
    /// the same mutation: an open attempt under a closed task is a ledger the window refuses.
    #[allow(clippy::too_many_arguments)] // Each argument is one fact the take writes down.
    fn take_task(
        &mut self,
        run_id: &str,
        task_id: &str,
        (seat, generation): (&str, u32),
        from: Option<&str>,
        why: Option<&str>,
        by: &str,
        now_ms: i64,
    ) -> Result<Taken, String> {
        let run = self.run(run_id).ok_or_else(|| unknown_run(run_id))?;
        let task = run
            .task(task_id)
            .ok_or_else(|| format!("unknown task: {task_id}"))?;
        if task.status == TaskStatus::Closed && by != BACKFILL_VERB {
            return Err(format!(
                "task {task_id} is closed, and a closed task takes no attempt — put it back to \
                 work with `task-update --status ready`, or record work that landed with \
                 `{BACKFILL_VERB}`"
            ));
        }
        if let Some(gate) = run.pending_gate_on(task_id) {
            return Err(format!(
                "task {task_id} is blocked by gate {} — resolve it before taking the work",
                gate.id
            ));
        }
        if task.status == TaskStatus::Pending {
            return Err(format!(
                "task {task_id} is waiting on its dependencies — take it once they are met"
            ));
        }
        let newest = run.newest_attempt(task_id).map(|one| one.id.clone());
        if let Some(named) = from
            && newest.as_deref() != Some(named)
        {
            return Err(format!(
                "--from names attempt {named}, and task {task_id} is at attempt {} — take over \
                 the attempt the task is at",
                newest.as_deref().unwrap_or(NO_ATTEMPT)
            ));
        }
        let mut closed = None;
        if let Some(open) = run
            .dispatches
            .iter()
            .find(|one| one.task == task_id && one.is_open())
        {
            if open.remote.is_some() {
                return Err(format!(
                    "task {task_id} is carried by dispatch {} in another window — its home ends \
                     it, not a take",
                    open.id
                ));
            }
            match carrier_generation(&open.worker) {
                Some(held) if held == generation => {
                    return Err(format!(
                        "this seat is already carrying task {task_id} under {} — hand it in with \
                         `{HAND_IN_VERB}`",
                        open.id
                    ));
                }
                Some(_) => closed = Some(open.id.clone()),
                None => {
                    return Err(format!(
                        "task {task_id} is carried by worker {} under dispatch {} — stop it first \
                         with `worker-stop --worker {}`; a take ends only an attempt nobody is \
                         working behind",
                        open.worker, open.id, open.worker
                    ));
                }
            }
        }
        let dispatch = self.mint("dp-");
        let note = self.mint("m-");
        let run = self.run_mut(run_id).ok_or_else(|| unknown_run(run_id))?;
        if let Some(old) = closed.as_deref() {
            Self::close_dispatch(run, old, None, now_ms);
        }
        run.dispatches.push(Dispatch {
            id: dispatch.clone(),
            task: task_id.to_string(),
            worker: coordinator_carrier(generation),
            started_ms: now_ms,
            ended_ms: None,
            succeeded: None,
            retry_of: newest.clone(),
            remote: None,
            source: None,
            session_history: None,
        });
        let at = run
            .tasks
            .iter()
            .position(|one| one.id == task_id)
            .expect("checked above");
        // Finished work keeps its status: the take is for its record, not to do it again.
        if !matches!(
            run.tasks[at].status,
            TaskStatus::Completed | TaskStatus::Closed
        ) {
            run.tasks[at].status = TaskStatus::Dispatched;
        }
        let status = run.tasks[at].status;
        /* The take is written down as the seat's own word, in the log and in no inbox: it is
         * the seat's news to nobody, and a backfill of sixty tasks is sixty of them. */
        let address = run.address();
        run.messages.push(Message {
            id: note,
            from: address.clone(),
            to: address,
            kind: MessageKind::Status,
            body: serde_json::json!({
                "taken": dispatch,
                "by": by,
                "generation": generation,
                "retryOf": newest,
                "closed": closed,
                "why": why,
            })
            .to_string()
            .into(),
            subject: Text::default(),
            priority: Priority::Normal,
            payload: Text::default(),
            thread: None,
            task: Some(task_id.to_string()),
            dispatch: Some(dispatch.clone()),
            author_seat: Some(seat.to_string()),
            created_ms: now_ms,
        });
        Ok(Taken {
            dispatch,
            retry_of: newest,
            closed,
            status,
        })
    }

    /// End the seat's own attempt with its hand-in: the letter a worker's `worker_done` is, from
    /// the run's address, settled by the same road ([`Self::settle_report`]) so the attempt's
    /// source, its end and the task's ending are written exactly as a worker's are. Filed in the
    /// log and in no inbox, like the take.
    fn hand_in_attempt(
        &mut self,
        run_id: &str,
        (task_id, dispatch_id): (&str, &str),
        body: String,
        ok: bool,
        seat: &str,
        now_ms: i64,
    ) -> Result<String, String> {
        let id = self.mint("m-");
        let run = self.run_mut(run_id).ok_or_else(|| unknown_run(run_id))?;
        let address = run.address();
        let message = Message {
            id: id.clone(),
            from: address.clone(),
            to: address,
            kind: MessageKind::WorkerDone,
            body: body.into(),
            subject: Text::default(),
            priority: Priority::Normal,
            payload: Text::default(),
            thread: None,
            task: Some(task_id.to_string()),
            dispatch: Some(dispatch_id.to_string()),
            author_seat: Some(seat.to_string()),
            created_ms: now_ms,
        };
        Self::settle_report(run, &message, ok);
        run.messages.push(message);
        Ok(id)
    }
}

/// `task-take --task <id> [--from <attempt>] [--why <line>]`.
pub(super) fn take(
    ledger: &mut Ledger,
    team: &Team,
    pane: &str,
    words: &Words,
    run_id: &str,
    now_ms: i64,
) -> Result<Decided, String> {
    only_these(TAKE_VERB, words, TAKE_FLAGS)?;
    let task_id = words
        .value("--task")
        .ok_or_else(|| format!("{TAKE_VERB} needs --task"))?;
    let (seat, generation) = the_seat(ledger, team, pane, run_id, TAKE_VERB)?;
    let taken = ledger.take_task(
        run_id,
        task_id,
        (&seat, generation),
        words.value("--from"),
        words.value("--why").filter(|why| !why.trim().is_empty()),
        TAKE_VERB,
        now_ms,
    )?;
    Ok(said(serde_json::json!({
        "taskId": task_id,
        "dispatchId": taken.dispatch,
        "carrier": coordinator_carrier(generation),
        "generation": generation,
        "retryOf": taken.retry_of,
        "closed": taken.closed,
        "status": taken.status.as_str(),
    })))
}

/// `hand-in --task <id> --head <commit> --body <json> [--base <ref>] [--checkout <path>]`.
#[allow(clippy::too_many_arguments)] // Each argument is one fact the hand-in rests on.
pub(super) fn hand_in(
    ledger: &mut Ledger,
    team: &Team,
    pane: &str,
    words: &Words,
    run_id: &str,
    seen: Option<&CommitsSeen>,
    now_ms: i64,
) -> Result<Decided, String> {
    only_these(HAND_IN_VERB, words, HAND_IN_FLAGS)?;
    let task_id = words
        .value("--task")
        .ok_or_else(|| format!("{HAND_IN_VERB} needs --task"))?;
    let (seat, generation) = the_seat(ledger, team, pane, run_id, HAND_IN_VERB)?;
    let named = words
        .value("--head")
        .ok_or_else(|| format!("{HAND_IN_VERB} needs --head <commit>"))?;
    let head = commit_named(named).ok_or_else(|| {
        format!("--head names a commit, seven hex digits or more, and {named:?} is not one")
    })?;
    let body = words.value("--body").ok_or_else(|| {
        format!("{HAND_IN_VERB} needs --body '{{\"ok\":true,\"summary\":\"…\"}}'")
    })?;
    let ok = worker_done_succeeded(body)?;
    let serde_json::Value::Object(mut said_body) =
        serde_json::from_str::<serde_json::Value>(body).map_err(|why| why.to_string())?
    else {
        return Err("--body is a JSON object, as a worker_done body is".to_string());
    };

    let run = ledger.run(run_id).ok_or_else(|| unknown_run(run_id))?;
    if run.task(task_id).is_none() {
        return Err(format!("unknown task: {task_id}"));
    }
    let attempt = match run
        .dispatches
        .iter()
        .find(|one| one.task == task_id && one.is_open())
    {
        Some(open) => match carrier_generation(&open.worker) {
            Some(held) if held == generation => open.id.clone(),
            Some(held) => {
                return Err(format!(
                    "task {task_id} is carried under {} by the coordinator seat's generation \
                     {held}, and this seat sits as generation {generation} — take it over with \
                     `{TAKE_VERB} --task {task_id} --from {}` and hand in on your own attempt",
                    open.id, open.id
                ));
            }
            None => {
                return Err(format!(
                    "task {task_id} is carried by worker {} under dispatch {} — a worker hands \
                     its own work in with `send --type worker_done`",
                    open.worker, open.id
                ));
            }
        },
        None => {
            return Err(match run.newest_attempt(task_id) {
                Some(done)
                    if carrier_generation(&done.worker).is_some() && done.source.is_some() =>
                {
                    format!(
                        "attempt {} has already handed in {} — a hand-in is made once; a later \
                         commit is a new `{TAKE_VERB}`",
                        done.id,
                        done.source.as_deref().unwrap_or_default()
                    )
                }
                _ => format!(
                    "no attempt of this seat is open on task {task_id} — open one with \
                     `{TAKE_VERB} --task {task_id}` first"
                ),
            });
        }
    };

    let seen = seen.ok_or_else(|| {
        format!(
            "git was not asked about {head} — the window reads git for a hand-in before the \
             ledger writes it; ask from a coordinator pane inside ZeroCode"
        )
    })?;
    let commit = seen
        .commit(&head)
        .ok_or_else(|| format!("git was not asked about {head} in {}", seen.repo))?;
    let full = commit
        .full
        .clone()
        .ok_or_else(|| format!("{head} is not a commit in {}", seen.repo))?;
    if let Some(base) = words.value("--base") {
        let measured = seen
            .base
            .as_ref()
            .filter(|asked| asked.name == base)
            .ok_or_else(|| format!("git was not asked about --base {base}"))?;
        if measured.head.is_none() {
            return Err(format!("git does not know {base} in {}", seen.repo));
        }
        match commit.in_base {
            Some(true) => {}
            Some(false) => return Err(format!("{full} is not in {base} — it is not handed in")),
            None => {
                return Err(format!(
                    "git could not say whether {full} is in {base} — nothing was handed in"
                ));
            }
        }
    }
    /* The body's own head, when it names one, is the same commit: a report that says one
     * commit while the hand-in names another is two answers to what was handed in. */
    if let Some(stated) = said_body
        .get(HANDED_IN_HEAD)
        .and_then(serde_json::Value::as_str)
        && !same_source(stated, &head)
        && !same_source(stated, &full)
    {
        return Err(format!(
            "the body's head {stated} is not --head {head} — name one commit"
        ));
    }
    said_body.insert(
        HANDED_IN_HEAD.to_string(),
        serde_json::Value::String(full.clone()),
    );
    let message = ledger.hand_in_attempt(
        run_id,
        (task_id, &attempt),
        serde_json::Value::Object(said_body).to_string(),
        ok,
        &seat,
        now_ms,
    )?;
    let status = ledger
        .run(run_id)
        .and_then(|run| run.task(task_id))
        .map(|task| task.status.as_str());
    Ok(said(serde_json::json!({
        "taskId": task_id,
        "dispatchId": attempt,
        "source": full,
        "ok": ok,
        "status": status,
        "messageId": message,
    })))
}

/// A commit a result names, and the key it was found under (`landed.mergeHead`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Named {
    under: String,
    commit: String,
}

/// The commits `result` names under [`LANDED_COMMIT_KEYS`], in the order they are believed.
fn commits_named(result: &str) -> Vec<Named> {
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(result)
    else {
        return Vec::new();
    };
    let mut found: Vec<Named> = Vec::new();
    let mut take = |under: String, value: &serde_json::Value| {
        if let Some(commit) = value.as_str().and_then(commit_named)
            && !found
                .iter()
                .any(|held| held.commit.eq_ignore_ascii_case(&commit))
        {
            found.push(Named { under, commit });
        }
    };
    let mut read = |map: &serde_json::Map<String, serde_json::Value>, above: Option<&str>| {
        for key in LANDED_COMMIT_KEYS {
            let under = above.map_or_else(|| (*key).to_string(), |above| format!("{above}.{key}"));
            match map.get(*key) {
                Some(serde_json::Value::Array(items)) => {
                    for item in items.iter().rev() {
                        take(under.clone(), item);
                    }
                }
                Some(value) => take(under, value),
                None => {}
            }
        }
    };
    read(&map, None);
    for (key, value) in &map {
        if let serde_json::Value::Object(inner) = value {
            read(inner, Some(key));
        }
    }
    found
}

/// The task ids a result names under a key that speaks of a review — the review tasks whose own
/// verdict a backfill may cite.
fn reviews_named(result: &str, own: &str) -> Vec<String> {
    fn walk(value: &serde_json::Value, under_review: bool, found: &mut Vec<String>) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, inner) in map {
                    let speaks = under_review || key.to_ascii_lowercase().contains(REVIEW_WORD);
                    walk(inner, speaks, found);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(item, under_review, found);
                }
            }
            serde_json::Value::String(text) if under_review => {
                let text = text.trim();
                if text.strip_prefix(TASK_PREFIX).is_some_and(|digits| {
                    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
                }) && !found.iter().any(|held| held == text)
                {
                    found.push(text.to_string());
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(result) {
        walk(&value, false, &mut found);
    }
    found.retain(|id| id != own);
    found
}

/// The finished tasks a backfill looks at: completed or closed, with no review standing and no
/// waiver — what the board counts as finished and never reviewed.
fn awaiting_review<'a>(run: &'a Run) -> impl Iterator<Item = &'a Task> + 'a {
    run.tasks.iter().filter(move |task| {
        matches!(task.status, TaskStatus::Completed | TaskStatus::Closed) && {
            let review = run.review_of(task);
            !review.written && !review.waived
        }
    })
}

/// The commits a finished task's result names, when nothing else stands in the way of
/// backfilling it — or why it cannot be.
fn commits_to_prove(run: &Run, task: &Task) -> Result<Vec<Named>, String> {
    if let Some(open) = run
        .dispatches
        .iter()
        .find(|one| one.task == task.id && one.is_open())
    {
        return Err(format!("attempt {} is still open on it", open.id));
    }
    if let Some(handed) = run
        .newest_attempt(&task.id)
        .filter(|one| one.source.is_some())
    {
        return Err(format!(
            "attempt {} handed in {} — review it with task-update",
            handed.id,
            handed.source.as_deref().unwrap_or_default()
        ));
    }
    let named = commits_named(&task.result);
    if named.is_empty() {
        return Err("the result names no commit".to_string());
    }
    Ok(named)
}

/// Every commit a backfill of `run` would ask git about — what the window reads before the plan.
#[must_use]
pub fn backfill_commits(run: &Run) -> Vec<String> {
    let mut commits: Vec<String> = Vec::new();
    for task in awaiting_review(run) {
        for named in commits_to_prove(run, task).unwrap_or_default() {
            if !commits
                .iter()
                .any(|held| held.eq_ignore_ascii_case(&named.commit))
            {
                commits.push(named.commit);
            }
        }
    }
    commits
}

/// What git proves for one finished task.
struct Proven {
    /// The checkout git was asked in.
    repo: String,
    /// The commit as git resolved it, and the key the result named it under.
    commit: String,
    under: String,
    tag: Option<String>,
    /// The review tasks the result names, and whether each one's own review passed.
    reviews: Vec<(String, bool)>,
}

impl Proven {
    fn verified(&self) -> bool {
        !self.reviews.is_empty() && self.reviews.iter().all(|(_, passed)| *passed)
    }

    /// The review keys a backfill writes — the facts, and only the facts git or the ledger
    /// proved. Read by the list and written by the apply, from this one place.
    fn facts(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut facts = serde_json::Map::new();
        facts.insert("merged".to_string(), true.into());
        facts.insert("mergeHead".to_string(), self.commit.clone().into());
        if self.tag.is_some() {
            facts.insert(DEPLOYED_KEY.to_string(), true.into());
        }
        if self.verified() {
            facts.insert("verified".to_string(), true.into());
        }
        facts
    }

    fn verified_word(&self) -> serde_json::Value {
        if self.verified() {
            true.into()
        } else {
            "unknown".into()
        }
    }

    fn reviews_json(&self) -> serde_json::Value {
        self.reviews
            .iter()
            .map(|(task, passed)| serde_json::json!({ "taskId": task, "passed": passed }))
            .collect()
    }
}

/// What git says about one task's named commits, measured against `base`: the first one in its
/// base, or why none is.
fn prove(
    run: &Run,
    task: &Task,
    named: &[Named],
    seen: Option<&CommitsSeen>,
    base: &str,
) -> Result<Proven, String> {
    let seen = seen
        .filter(|seen| seen.base.as_ref().is_some_and(|asked| asked.name == base))
        .ok_or_else(|| format!("git was not asked about these commits against {base}"))?;
    if seen.base.as_ref().is_some_and(|asked| asked.head.is_none()) {
        return Err(format!("git does not know {base} in {}", seen.repo));
    }
    let mut first_failure = None;
    for one in named {
        let failure = match seen.commit(&one.commit) {
            None => format!("git was not asked about {}", one.commit),
            Some(CommitSeen { full: None, .. }) => {
                format!("{} is not a commit in {}", one.commit, seen.repo)
            }
            Some(CommitSeen {
                full: Some(full),
                in_base: Some(true),
                tag,
            }) => {
                let reviews = reviews_named(&task.result, &task.id)
                    .into_iter()
                    .map(|id| {
                        let passed = run
                            .task(&id)
                            .is_some_and(|review| run.review_of(review).verified);
                        (id, passed)
                    })
                    .collect();
                return Ok(Proven {
                    repo: seen.repo.clone(),
                    commit: full.clone(),
                    under: one.under.clone(),
                    tag: tag.clone(),
                    reviews,
                });
            }
            Some(CommitSeen {
                in_base: Some(false),
                ..
            }) => format!("{} is not in {base}", one.commit),
            Some(CommitSeen { in_base: None, .. }) => {
                format!("git could not say whether {} is in {base}", one.commit)
            }
        };
        first_failure.get_or_insert(failure);
    }
    Err(first_failure.unwrap_or_else(|| "the result names no commit".to_string()))
}

/// The task's result with a backfill's facts added: every original key kept as it was, except a
/// review key the original held, which moves under `backfill.replaced` — kept, but no longer
/// read as a review the coordinator seat wrote. A result that is not a JSON object is kept whole
/// under `backfill.original`.
fn backfilled_result(original: &str, proven: &Proven, base: &BaseSeen, now_ms: i64) -> String {
    let (mut map, text) = match serde_json::from_str::<serde_json::Value>(original) {
        Ok(serde_json::Value::Object(map)) => (map, None),
        _ => (
            serde_json::Map::new(),
            (!original.trim().is_empty()).then(|| original.to_string()),
        ),
    };
    let mut replaced = serde_json::Map::new();
    for key in review_keys().chain(std::iter::once(BACKFILL_KEY)) {
        if let Some(value) = map.remove(key) {
            replaced.insert(key.to_string(), value);
        }
    }
    map.extend(proven.facts());
    let mut note = serde_json::json!({
        "by": BACKFILL_VERB,
        "atMs": now_ms,
        "repo": proven.repo,
        "base": base.name,
        "baseHead": base.head,
        "commit": proven.commit,
        "foundUnder": proven.under,
        "tag": proven.tag,
        "verified": proven.verified_word(),
        "reviews": proven.reviews_json(),
    });
    if !replaced.is_empty() {
        note["replaced"] = serde_json::Value::Object(replaced);
    }
    if let Some(text) = text {
        note["original"] = text.into();
    }
    map.insert(BACKFILL_KEY.to_string(), note);
    serde_json::Value::Object(map).to_string()
}

/// `task-backfill [--base <ref>] [--tags <glob>] [--checkout <path>] [--apply]`.
///
/// Lists every finished task no review stands on, and for each one either what it would write —
/// a coordinator attempt, a hand-in of the commit git proved, and a review of that commit — or
/// why it cannot. `--apply` writes the listed rows from the coordinator seat; one task that
/// cannot be written never stops the others.
#[allow(clippy::too_many_arguments)] // Each argument is one fact the backfill rests on.
pub(super) fn backfill(
    ledger: &mut Ledger,
    team: &Team,
    pane: &str,
    words: &Words,
    run_id: &str,
    seen: Option<&CommitsSeen>,
    now_ms: i64,
) -> Result<Decided, String> {
    only_these(BACKFILL_VERB, words, BACKFILL_FLAGS)?;
    let apply = words.has("--apply");
    let seat = match apply {
        true => Some(the_seat(ledger, team, pane, run_id, BACKFILL_VERB)?),
        false => None,
    };
    let base = words.value("--base").unwrap_or(DEFAULT_BASE).to_string();
    let run = ledger.run(run_id).ok_or_else(|| unknown_run(run_id))?;
    let mut rows = Vec::new();
    let mut ready = Vec::new();
    for task in awaiting_review(run) {
        let proven =
            commits_to_prove(run, task).and_then(|named| prove(run, task, &named, seen, &base));
        let attempt = run.newest_attempt(&task.id).map(|one| one.id.clone());
        let row = match &proven {
            Ok(proven) => serde_json::json!({
                "taskId": task.id,
                "status": task.status.as_str(),
                "attempt": attempt,
                "commit": proven.commit,
                "foundUnder": proven.under,
                "tag": proven.tag,
                "write": proven.facts(),
                "verified": proven.verified_word(),
                "reviews": proven.reviews_json(),
                "why": null,
            }),
            Err(why) => serde_json::json!({
                "taskId": task.id,
                "status": task.status.as_str(),
                "attempt": attempt,
                "commit": commits_named(&task.result).first().map(|one| one.commit.clone()),
                "write": null,
                "why": why,
            }),
        };
        if let Ok(proven) = proven {
            ready.push((task.id.clone(), task.result.to_string(), proven, rows.len()));
        }
        rows.push(row);
    }
    let base_seen = seen
        .and_then(|seen| seen.base.clone())
        .filter(|asked| asked.name == base);
    let mut applied = Vec::new();
    if let (Some((seat, generation)), Some(base_seen)) = (seat, base_seen.as_ref()) {
        for (task_id, original, proven, at) in &ready {
            match apply_one(
                ledger,
                (team, pane),
                run_id,
                task_id,
                (&seat, generation),
                (original, proven, base_seen),
                now_ms,
            ) {
                Ok(dispatch) => {
                    rows[*at]["dispatchId"] = dispatch.into();
                    applied.push(task_id.clone());
                }
                Err(why) => rows[*at]["why"] = why.into(),
            }
        }
    }
    Ok(said(serde_json::json!({
        "run": run_id,
        "apply": apply,
        "base": base,
        "baseHead": base_seen.as_ref().and_then(|asked| asked.head.clone()),
        "tags": seen.and_then(|seen| seen.tags.clone()),
        "repo": seen.map(|seen| seen.repo.clone()),
        "listed": rows.len(),
        "ready": ready.len(),
        "applied": applied,
        "rows": rows,
    })))
}

/// One task's three steps: the seat's attempt, its hand-in of the proven commit, and the review
/// written by the road every coordinator review takes ([`correction_author`]).
#[allow(clippy::too_many_arguments)] // Each argument is one fact the write rests on.
fn apply_one(
    ledger: &mut Ledger,
    (team, pane): (&Team, &str),
    run_id: &str,
    task_id: &str,
    (seat, generation): (&str, u32),
    (original, proven, base): (&str, &Proven, &BaseSeen),
    now_ms: i64,
) -> Result<String, String> {
    let taken = ledger.take_task(
        run_id,
        task_id,
        (seat, generation),
        None,
        Some(&format!("{} is in {}", proven.commit, base.name)),
        BACKFILL_VERB,
        now_ms,
    )?;
    let body = serde_json::json!({
        "ok": true,
        HANDED_IN_HEAD: proven.commit,
        HANDED_IN_SUMMARY: format!(
            "{BACKFILL_VERB}: {} is in {}{}",
            proven.commit,
            base.name,
            proven
                .tag
                .as_deref()
                .map_or_else(String::new, |tag| format!(" and in {tag}"))
        ),
        "by": BACKFILL_VERB,
    });
    ledger.hand_in_attempt(
        run_id,
        (task_id, &taken.dispatch),
        body.to_string(),
        true,
        seat,
        now_ms,
    )?;
    let result = backfilled_result(original, proven, base, now_ms);
    let author = correction_author(
        ledger,
        &team.leader_pane,
        run_id,
        task_id,
        (&team.id, pane),
        Observed {
            attempt: Some(&taken.dispatch),
            source: Some(&proven.commit),
            review: true,
            bare_verification: verifies_on_nothing_new(&result),
            tested_head: None,
        },
    )?;
    ledger.update_task(run_id, task_id, None, Some(result), author, now_ms)?;
    Ok(taken.dispatch)
}
