//! A worker's own question goes to its run's coordinator, not to the person
//! (t-82296).
//!
//! A pane the ledger summoned is somebody's worker, and the somebody is the
//! coordinator of its run. When its CLI stops on an `AskUserQuestion`, the
//! person's 「당신 차례」 is the wrong door: nobody may be watching that
//! screen, and the coordinator who briefed the worker is the one who knows
//! the answer. So the question is filed in the ledger as the worker's own
//! `question` letter to its coordinator — the body and the choices word for
//! word, with the pane and the dispatch beside them — and the person's
//! screen does not draw it.
//!
//! The coordinator answers the way every question is answered, `reply
//! --to-message <id>`, with a choice's number or free words. The beat reads
//! that answer from the ledger and types it into the pane's picker through
//! the same answer door a card's answer takes
//! ([`crate::cmd::terminal::answer_ask`]) — no new verb, no new key road.
//!
//! The person is asked after all in three cases only: the run's coordinator
//! seat is empty, the person has taken the pane, or the coordinator has not
//! answered within [`COORDINATOR_ANSWER_WAIT_MS`]. A question given to the
//! person while it waited says why on the card (`ask:raised`).
//!
//! A question the pane moved past before the answer came is refused back to
//! the coordinator as 「지나간 질문」 and leaves nothing on the person's
//! screen: the moving past is the hook's word (a report that is no longer the
//! asking), or the answer door's, which reads the screen and refuses with
//! [`crate::cmd::terminal::QUESTION_CHANGED`] — the judgement t-82287 made
//! for the person's own answers.
//!
//! A permission request is not routed: the automatic approval rules read it
//! first and what they leave is the person's, as before.
//!
//! What the window carries is written down beside its other state
//! ([`crate::app_paths::artifact_file::WORKER_QUESTIONS`]): a restart forgets
//! it, and a picker that still stands reports nothing again. The window that
//! comes back takes each question up when its worker sits in a pane again and
//! the screen still shows it, its ten minutes counted from the asking.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

use tauri::{AppHandle, Emitter, Manager};
use zerocode_core::ask::{AskPrompt, AskSelection};

use crate::agent_teams::Host;
use crate::hooks::PaneHookReport;

/// How long a coordinator is given to answer its worker's question before
/// the person is asked instead.
pub(crate) const COORDINATOR_ANSWER_WAIT_MS: i64 = 10 * 60 * 1000;

/// How long a question read back after a restart waits, once its worker
/// sits in a pane again, for the pane's screen to show it: a pane that has
/// just come back may not have drawn yet.
const TAKE_UP_GRACE_MS: i64 = 5_000;

/// The window event that tells the page a worker's question reached the
/// person after all, and why: `{ term, why }` (t-82296).
pub(crate) const RAISED_EVENT: &str = "ask:raised";

/// The coordinator's whole answer that gives its worker's question to the
/// person at once (t-84343): a question that is the person's to answer — an
/// account to pick, a secret value, a payment, a login, a pane the person
/// works in. Either word, alone; the letter, the guide and the tests all read
/// them from here.
pub(crate) const TO_PERSON: &[&str] = &["사람에게", "to-person"];

/// Where the run's side of a worker's pane stands, read from the ledger
/// ([`crate::orchestration::worker_ask_seat`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AskSeat {
    pub(crate) run: String,
    pub(crate) worker: String,
    pub(crate) team: String,
    pub(crate) pane: String,
    /// The run's coordinator address, the one a worker's letter goes to.
    pub(crate) coordinator: String,
    /// Whether somebody sits in the run's coordinator seat now.
    pub(crate) coordinator_live: bool,
    /// Whether the person has taken the pane.
    pub(crate) taken: bool,
    /// The dispatch the worker is on, named in the letter.
    pub(crate) dispatch: Option<String>,
}

/// Why a worker's question is on the person's screen after all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Raised {
    /// The coordinator did not answer within [`COORDINATOR_ANSWER_WAIT_MS`].
    CoordinatorSilent,
    /// The run's coordinator seat emptied while the question waited.
    CoordinatorGone,
    /// The person took the pane while the question waited.
    TakenOver,
    /// The ledger would not file the letter.
    Unsent,
    /// The coordinator answered with [`TO_PERSON`].
    CoordinatorHanded,
    /// The ledger can no longer take an answer to the letter.
    Unanswerable,
}

/// Where a question a pane just asked goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// To the run's coordinator, as the worker's `question` letter.
    Coordinator,
    /// To the person's 「당신 차례」, as before: no run's worker sits here,
    /// the run's coordinator seat is empty, or the person has the pane.
    Person,
}

/// The road of a question a pane just asked: a worker's goes to its
/// coordinator while somebody sits there and the person has not taken the
/// pane.
pub(crate) fn route(seat: Option<&AskSeat>) -> Route {
    match seat {
        Some(seat) if seat.coordinator_live && !seat.taken => Route::Coordinator,
        _ => Route::Person,
    }
}

/// What the ledger says of a question filed for a worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Standing {
    /// No answer yet.
    Waiting,
    /// The coordinator answered with these words.
    Answered(String),
    /// The question can no longer be answered, or is gone from the ledger.
    Closed,
}

/// What one beat does with one filed question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// Nothing yet.
    Wait,
    /// Type the coordinator's answer into the pane's picker.
    Answer(Vec<AskSelection>),
    /// Tell the coordinator the pane moved past the question, and forget it.
    RefusePassed,
    /// Tell the coordinator the person holds the question — it was given to
    /// them, or they took the pane — type nothing, and forget it.
    RefuseHeld,
    /// Put the question on the person's screen, saying why.
    Raise(Raised),
    /// Forget the question: there is nobody left to answer it or to tell.
    Forget,
}

/// The beat's judgement on one question already filed with the coordinator.
///
/// `passed` is the hook's word that the pane is no longer asking. `seat` is
/// the run as the ledger holds it now. `raised` is whether the person already
/// holds the question, so it is not raised twice — an answer the coordinator
/// gives after that, or while the person has the pane, is refused back to it
/// rather than typed between the person's own keys.
pub(crate) fn step(
    prompt: &AskPrompt,
    standing: &Standing,
    passed: bool,
    seat: Option<&AskSeat>,
    raised: bool,
    filed_ms: i64,
    now_ms: i64,
) -> Step {
    match standing {
        Standing::Closed => return Step::Forget,
        Standing::Answered(_) if passed => return Step::RefusePassed,
        Standing::Answered(_) if raised || seat.is_some_and(|seat| seat.taken) => {
            return Step::RefuseHeld;
        }
        Standing::Answered(words) => {
            return match selections_from_reply(prompt, words) {
                Some(selections) => Step::Answer(selections),
                None => Step::Raise(Raised::CoordinatorSilent),
            };
        }
        Standing::Waiting if passed => return Step::Wait,
        Standing::Waiting => {}
    }
    if raised {
        return Step::Wait;
    }
    match seat {
        None => Step::Raise(Raised::CoordinatorGone),
        Some(seat) if !seat.coordinator_live => Step::Raise(Raised::CoordinatorGone),
        Some(seat) if seat.taken => Step::Raise(Raised::TakenOver),
        Some(_) if now_ms.saturating_sub(filed_ms) >= COORDINATOR_ANSWER_WAIT_MS => {
            Step::Raise(Raised::CoordinatorSilent)
        }
        Some(_) => Step::Wait,
    }
}

/// The letter a worker's question is filed as: its subject and body.
///
/// The question and every choice travel word for word, numbered as the
/// answer names them, with the pane and the dispatch it is on, and one line
/// on how an answer is read ([`selections_from_reply`]).
pub(crate) fn letter(prompt: &AskPrompt, seat: &AskSeat, term: u32) -> (String, String) {
    let subject = prompt
        .questions
        .first()
        .map(|question| {
            question
                .header
                .clone()
                .filter(|header| !header.trim().is_empty())
                .unwrap_or_else(|| question.question.clone())
        })
        .unwrap_or_default();
    let mut body = format!(
        "워커 {}의 판(term-{term}{})이 묻습니다 — 사람 화면에는 올리지 않았습니다.\n",
        seat.worker,
        seat.dispatch
            .as_deref()
            .map(|id| format!(", {id}"))
            .unwrap_or_default(),
    );
    let several = prompt.questions.len() > 1;
    for (at, question) in prompt.questions.iter().enumerate() {
        body.push('\n');
        if several {
            body.push_str(&format!("{}) ", at + 1));
        }
        if let Some(header) = question
            .header
            .as_deref()
            .filter(|one| !one.trim().is_empty())
        {
            body.push_str(&format!("[{header}] "));
        }
        body.push_str(&question.question);
        if question.multi_select {
            body.push_str(" (여러 개 고를 수 있음)");
        }
        body.push('\n');
        for (row, option) in question.options.iter().enumerate() {
            body.push_str(&format!("  {}. {}", row + 1, option.label));
            if let Some(description) = option.description.as_deref() {
                body.push_str(&format!(" — {description}"));
            }
            body.push('\n');
        }
    }
    body.push_str(&format!(
        "\n답: reply --to-message 로 고른 번호(여러 개면 1,3) 또는 자유 글{}. {}분 안에 답이 없으면 사람에게 올립니다.",
        if several { "을 질문마다 한 줄에" } else { "" },
        COORDINATOR_ANSWER_WAIT_MS / 60_000,
    ));
    (subject, body)
}

/// The coordinator's reply as the picks a card would have sent.
///
/// One question takes the whole reply; several take one line each, in
/// order. A line that is only choice numbers inside the question's range —
/// one for a single-select, any for a multi-select — picks those rows;
/// anything else is the question's free answer. `None` when no question got
/// an answer at all.
pub(crate) fn selections_from_reply(prompt: &AskPrompt, reply: &str) -> Option<Vec<AskSelection>> {
    let reply = reply.trim();
    let lines: Vec<&str> = match prompt.questions.len() {
        0 => return None,
        1 => vec![reply],
        _ => reply.lines().map(str::trim).collect(),
    };
    let selections: Vec<AskSelection> = prompt
        .questions
        .iter()
        .enumerate()
        .map(|(at, question)| {
            let words = lines.get(at).copied().unwrap_or_default();
            match picked_rows(words, question.options.len(), question.multi_select) {
                Some(indices) => AskSelection {
                    indices,
                    other: String::new(),
                },
                None => AskSelection {
                    indices: Vec::new(),
                    other: words.to_string(),
                },
            }
        })
        .collect();
    zerocode_core::ask::has_answer(prompt, &selections).then_some(selections)
}

/// The rows `words` names when it is only choice numbers, 1-based, all in
/// `1..=rows`, and one of them unless `several` — as 0-based indices.
fn picked_rows(words: &str, rows: usize, several: bool) -> Option<Vec<usize>> {
    let numbers: Vec<&str> = words
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|one| !one.is_empty())
        .collect();
    if numbers.is_empty() || (!several && numbers.len() > 1) {
        return None;
    }
    let mut picked = Vec::with_capacity(numbers.len());
    for number in numbers {
        let row = number.parse::<usize>().ok()?;
        if row == 0 || row > rows {
            return None;
        }
        if !picked.contains(&(row - 1)) {
            picked.push(row - 1);
        }
    }
    Some(picked)
}

/// The words the coordinator is told when the pane moved past its question.
pub(crate) fn passed_letter(worker: &str, term: u32, question: &str) -> String {
    format!(
        "지나간 질문 — 워커 {worker}의 판(term-{term})은 질문 {question}을 이미 지나갔습니다. 답은 판에 넣지 않았습니다."
    )
}

/// The words the coordinator is told when its answer came after the person
/// was given the question or took the pane.
pub(crate) fn held_letter(worker: &str, term: u32, question: &str) -> String {
    format!(
        "사람이 맡은 질문 — 워커 {worker}의 판(term-{term}) 질문 {question}은 사람에게 넘어가 있습니다. 답은 판에 넣지 않았습니다."
    )
}

/// The words the coordinator is told when its answer could not be typed.
pub(crate) fn unanswered_letter(worker: &str, term: u32, question: &str, why: &str) -> String {
    format!("질문 {question}의 답을 워커 {worker}의 판(term-{term})에 넣지 못했습니다 — {why}")
}

/// Where one question the window carries for a worker stands.
#[derive(Debug, Clone)]
enum Filing {
    /// Routed, not yet in the ledger.
    Unfiled,
    /// In the ledger as this message.
    Filed(String),
}

/// One worker's question the window carries to its coordinator.
#[derive(Debug, Clone)]
struct Asked {
    /// The question as the hook reported it, to compare the next report to.
    signature: String,
    prompt: AskPrompt,
    /// The asking report itself, noted again when the person is asked after
    /// all, and the worktree it came from, for the bell.
    report: PaneHookReport,
    worktree: Option<String>,
    seat: AskSeat,
    filed: Filing,
    filed_ms: i64,
    passed: bool,
    raised: Option<Raised>,
}

/// A question a pane left before its coordinator answered — the pane asked
/// something else, or closed — kept by its message id so an answer that
/// lands later is refused as 「지나간 질문」 rather than lost.
#[derive(Debug, Clone)]
struct Left {
    seat: AskSeat,
    term: u32,
}

fn left() -> MutexGuard<'static, HashMap<String, Left>> {
    static LEFT: OnceLock<Mutex<HashMap<String, Left>>> = OnceLock::new();
    LEFT.get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|held| held.into_inner())
}

/// Keep what `term` asked of its coordinator after the window stops carrying
/// it: only a question already in the ledger can still be answered.
fn leave(term: u32, old: Asked) {
    if let Filing::Filed(question) = old.filed {
        left().insert(
            question,
            Left {
                seat: old.seat,
                term,
            },
        );
    }
}

fn asked() -> MutexGuard<'static, HashMap<u32, Asked>> {
    static ASKED: OnceLock<Mutex<HashMap<u32, Asked>>> = OnceLock::new();
    ASKED
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|held| held.into_inner())
}

/// One worker's question in the ledger as the window writes it down for the
/// window that comes back after a restart.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Kept {
    run: String,
    worker: String,
    question: String,
    filed_ms: i64,
    raised: Option<Raised>,
    report: PaneHookReport,
    worktree: Option<String>,
    /// When this window first found its worker in a pane, for
    /// [`TAKE_UP_GRACE_MS`].
    #[serde(skip)]
    seated_ms: Option<i64>,
}

/// What the window wrote down last, and what it read back and has not yet
/// taken up — `None` until the first beat reads the file.
struct Memory {
    waiting: Vec<Kept>,
    written: Vec<u8>,
}

fn memory() -> MutexGuard<'static, Option<Memory>> {
    static MEMORY: OnceLock<Mutex<Option<Memory>>> = OnceLock::new();
    MEMORY
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|held| held.into_inner())
}

/// What is written down of what the window carries: every question in the
/// ledger the pane still asks, and what was read back but not yet taken up —
/// in one order, so the same questions are the same bytes.
fn kept_of(held: &HashMap<u32, Asked>, waiting: &[Kept]) -> Vec<Kept> {
    let mut kept: Vec<Kept> = held
        .values()
        .filter(|standing| !standing.passed)
        .filter_map(|standing| match &standing.filed {
            Filing::Filed(question) => Some(Kept {
                run: standing.seat.run.clone(),
                worker: standing.seat.worker.clone(),
                question: question.clone(),
                filed_ms: standing.filed_ms,
                raised: standing.raised,
                report: standing.report.clone(),
                worktree: standing.worktree.clone(),
                seated_ms: None,
            }),
            Filing::Unfiled => None,
        })
        .chain(waiting.iter().cloned())
        .collect();
    kept.sort_by(|one, other| one.question.cmp(&other.question));
    kept
}

/// Write `kept` down unless `written` already holds those bytes, and hold
/// them — even when the write fails, so the next change tries again rather
/// than every beat.
fn write_kept(path: &Path, kept: &[Kept], written: &mut Vec<u8>) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(kept).map_err(std::io::Error::other)?;
    if *written == bytes {
        return Ok(());
    }
    let wrote = crate::durable_file::replace_bytes(path, &bytes).map(drop);
    *written = bytes;
    wrote
}

/// Write `kept` down as [`write_kept`] does, and say what the window's log
/// is to note of a write that failed.
fn write_noting(path: &Path, kept: &[Kept], memory: &mut Memory) -> Option<String> {
    write_kept(path, kept, &mut memory.written)
        .err()
        .map(|error| format!("worker-questions: {error}"))
}

/// What the last window wrote down. A file that is missing or cannot be read
/// carries nothing: its questions are dropped, as before the file existed.
fn read_kept(path: &Path) -> Memory {
    match std::fs::read(path) {
        Ok(bytes) => Memory {
            waiting: serde_json::from_slice(&bytes).unwrap_or_default(),
            written: bytes,
        },
        Err(_) => Memory {
            waiting: Vec::new(),
            written: serde_json::to_vec(&Vec::<Kept>::new()).unwrap_or_default(),
        },
    }
}

/// A written-down question as the window carries it again, at the term its
/// worker sits at now; `None` when the report it kept asks nothing.
fn taken_up(kept: Kept, seat: AskSeat, term: u32) -> Option<Asked> {
    let mut report = kept.report;
    report.term = term;
    let prompt = asking(&report)?.clone();
    Some(Asked {
        signature: signature(&prompt),
        prompt,
        report,
        worktree: kept.worktree,
        seat,
        filed: Filing::Filed(kept.question),
        filed_ms: kept.filed_ms,
        passed: false,
        raised: kept.raised,
    })
}

fn signature(prompt: &AskPrompt) -> String {
    serde_json::to_string(prompt).unwrap_or_default()
}

/// Whether `report` is a pane asking an `AskUserQuestion` — not a permission,
/// and not a wait that described no question.
fn asking(report: &PaneHookReport) -> Option<&AskPrompt> {
    (report.state == zerocode_core::hook::HookState::NeedsAttention && report.approval.is_none())
        .then_some(report.ask_prompt.as_ref())
        .flatten()
}

/// Hear one hook report, and answer what the page is shown of it.
///
/// A worker's question routed to its coordinator is shown as the work it is
/// — the pane is mid-turn, waiting on its coordinator's answer — with no
/// question on it, so no card, no 「당신 차례」 row and no ring. Everything
/// else is shown as reported.
pub(crate) fn heard(
    report: &PaneHookReport,
    worktree: Option<&str>,
    now_ms: i64,
) -> PaneHookReport {
    let term = report.term;
    if let Some(prompt) = asking(report) {
        let sig = signature(prompt);
        let mut held = asked();
        if let Some(standing) = held.get_mut(&term)
            && standing.signature == sig
        {
            standing.passed = false;
            return match standing.raised {
                Some(_) => report.clone(),
                None => quiet(report),
            };
        }
        if let Some(old) = held.remove(&term) {
            leave(term, old);
        }
        drop(held);
        let seat = crate::orchestration::worker_ask_seat(term);
        if route(seat.as_ref()) == Route::Person {
            return report.clone();
        }
        let Some(seat) = seat else {
            return report.clone();
        };
        asked().insert(
            term,
            Asked {
                signature: sig,
                prompt: prompt.clone(),
                report: report.clone(),
                worktree: worktree.map(str::to_string),
                seat,
                filed: Filing::Unfiled,
                filed_ms: now_ms,
                passed: false,
                raised: None,
            },
        );
        return quiet(report);
    }
    let mut held = asked();
    if let Some(standing) = held.get_mut(&term) {
        // A wait that described no question (a notification while the picker
        // stands) is not the pane moving on; anything else is.
        if report.state == zerocode_core::hook::HookState::NeedsAttention
            && report.approval.is_none()
        {
            return match standing.raised {
                Some(_) => report.clone(),
                None => quiet(report),
            };
        }
        standing.passed = true;
        if matches!(standing.filed, Filing::Unfiled) {
            held.remove(&term);
        }
    }
    report.clone()
}

/// The report as the page is shown it while the coordinator holds the
/// question: the pane at work, asking nothing.
fn quiet(report: &PaneHookReport) -> PaneHookReport {
    let mut shown = report.clone();
    shown.state = zerocode_core::hook::HookState::Working;
    shown.ask = None;
    shown.ask_prompt = None;
    shown.submit_shape = zerocode_core::ask::SubmitShape::NotAQuestion;
    shown
}

/// Whether the person's screen keeps `term`'s question off: its worker's
/// coordinator holds it, and has not given it back to the person.
pub(crate) fn hidden(term: u32) -> bool {
    asked()
        .get(&term)
        .is_some_and(|standing| !standing.passed && standing.raised.is_none())
}

/// The pane is gone; whatever it asked goes with it, and an answer its
/// coordinator gives later is refused as passed.
pub(crate) fn forget(term: u32) {
    let old = asked().remove(&term);
    if let Some(old) = old {
        leave(term, old);
    }
}

/// One beat over every question the window carries for a worker: file what
/// is not filed yet, type what the coordinator answered, refuse what the pane
/// moved past, and give the person what nobody answers.
pub(crate) fn sweep(app: &AppHandle, host: &dyn Host, now_ms: i64) {
    let path = app
        .state::<crate::AppState>()
        .local_data_root()
        .join(crate::app_paths::artifact_file::WORKER_QUESTIONS);
    take_up(app, &path, now_ms);
    let gone: Vec<(String, Left)> = left()
        .iter()
        .map(|(question, was)| (question.clone(), was.clone()))
        .collect();
    for (question, was) in gone {
        match crate::orchestration::question_standing(&was.seat.run, &question) {
            Standing::Waiting => {}
            Standing::Answered(_) => {
                tell(
                    &was.seat,
                    &question,
                    &passed_letter(&was.seat.worker, was.term, &question),
                    now_ms,
                );
                left().remove(&question);
            }
            Standing::Closed => {
                left().remove(&question);
            }
        }
    }
    let terms: Vec<u32> = asked().keys().copied().collect();
    for term in terms {
        let Some(standing) = asked().get(&term).cloned() else {
            continue;
        };
        match &standing.filed {
            Filing::Unfiled if standing.raised.is_some() => {}
            Filing::Unfiled => file(app, host, term, &standing, now_ms),
            Filing::Filed(question) => carry(app, term, &standing, question, now_ms),
        }
    }
    write_down(app, &path);
}

/// Take up what the last window wrote down as each worker sits in a pane
/// again: a question the screen still shows is carried on, given back to the
/// person if it was theirs; one the ledger closed is dropped, and one the
/// pane no longer shows is left, so a late answer is refused as passed —
/// nothing of either reaches the person's screen.
fn take_up(app: &AppHandle, path: &Path, now_ms: i64) {
    let waiting = std::mem::take(&mut memory().get_or_insert_with(|| read_kept(path)).waiting);
    let mut later = Vec::new();
    for mut kept in waiting {
        let standing = crate::orchestration::question_standing(&kept.run, &kept.question);
        let seated = crate::orchestration::worker_term(&kept.run, &kept.worker)
            .and_then(|term| crate::orchestration::worker_ask_seat(term).map(|seat| (term, seat)));
        let waited_ms = seated.as_ref().map_or(0, |_| {
            now_ms.saturating_sub(*kept.seated_ms.get_or_insert(now_ms))
        });
        let at = seated.as_ref().map(|(term, _)| *term);
        let asking_there = at.is_some_and(|term| asked().contains_key(&term));
        let next = taking_up(
            &standing,
            at.is_some(),
            asking_there,
            || {
                at.zip(asking(&kept.report).and_then(|prompt| prompt.questions.first()))
                    .is_some_and(|(term, question)| {
                        crate::cmd::terminal::question_is_up(app, term, question)
                    })
            },
            waited_ms,
        );
        let Some((term, seat)) = seated else {
            if next == TakeUp::Later {
                later.push(kept);
            }
            continue;
        };
        match next {
            TakeUp::Drop => {}
            TakeUp::Later => later.push(kept),
            TakeUp::Leave => {
                if let Some(standing) = taken_up(kept, seat, term) {
                    leave(term, standing);
                }
            }
            TakeUp::Meet => {
                let mut held = asked();
                // The pane's question was settled in between: look again on
                // the next beat.
                let Some(there) = held.get_mut(&term) else {
                    drop(held);
                    later.push(kept);
                    continue;
                };
                match meet(there, kept, seat, term) {
                    Met::Adopted(Some(why)) => {
                        let (report, worktree) = (there.report.clone(), there.worktree.clone());
                        drop(held);
                        raise(app, term, &report, worktree.as_deref(), why);
                    }
                    Met::Adopted(None) | Met::Dropped => {}
                    Met::Left(old) => {
                        drop(held);
                        leave(term, old);
                    }
                }
            }
            TakeUp::Carry => {
                let Some(standing) = taken_up(kept, seat, term) else {
                    continue;
                };
                let back = standing
                    .raised
                    .map(|why| (why, standing.report.clone(), standing.worktree.clone()));
                asked().insert(term, standing);
                if let Some((why, report, worktree)) = back {
                    raise(app, term, &report, worktree.as_deref(), why);
                }
            }
        }
    }
    if let Some(memory) = memory().as_mut() {
        memory.waiting.extend(later);
    }
}

/// What one written-down question does on a beat ([`taking_up`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TakeUp {
    /// Nothing to carry and nobody to tell.
    Drop,
    /// Its worker sits in no pane yet, or the pane has not drawn it yet.
    Later,
    /// The pane at its worker's term asks already: the hook came first.
    Meet,
    /// The screen shows it: carry it on.
    Carry,
    /// The screen no longer shows it: keep its letter, so a late answer is
    /// refused as passed.
    Leave,
}

/// The judgement on one written-down question: `standing` is the ledger's
/// word on its letter, `seated` whether its worker sits in a pane,
/// `asking_there` whether the window carries a question at that pane already,
/// `up` whether the screen shows it — read only when it decides — and
/// `waited_ms` how long the worker has sat there.
fn taking_up(
    standing: &Standing,
    seated: bool,
    asking_there: bool,
    up: impl FnOnce() -> bool,
    waited_ms: i64,
) -> TakeUp {
    if *standing == Standing::Closed {
        return TakeUp::Drop;
    }
    if !seated {
        return TakeUp::Later;
    }
    // The pane asked again since: the hook has the word.
    if asking_there {
        return TakeUp::Drop;
    }
    if up() {
        return TakeUp::Carry;
    }
    if waited_ms < TAKE_UP_GRACE_MS {
        TakeUp::Later
    } else {
        TakeUp::Leave
    }
}

/// What became of a written-down question that met its pane asking already.
#[derive(Debug)]
enum Met {
    /// The pane asks the same question and has not filed it yet: it carries
    /// the written-down letter on — given to the person now, when it was
    /// theirs before the restart.
    Adopted(Option<Raised>),
    /// The pane asks something else: the written-down letter is left, so a
    /// late answer to it is refused as passed.
    Left(Asked),
    /// The written-down report asks nothing.
    Dropped,
}

/// A written-down question meets `held`, what the pane at `term` asks now.
fn meet(held: &mut Asked, kept: Kept, seat: AskSeat, term: u32) -> Met {
    let _ = (held, kept, seat, term);
    Met::Dropped
}

/// Write down what the window carries now, when it changed since the last
/// writing.
fn write_down(app: &AppHandle, path: &Path) {
    let held = asked();
    let mut memory = memory();
    let Some(memory) = memory.as_mut() else {
        return;
    };
    let kept = kept_of(&held, &memory.waiting);
    drop(held);
    if let Some(line) = write_noting(path, &kept, memory) {
        crate::system_runtime::note_window_event(
            app.state::<crate::AppState>().local_data_root(),
            &line,
        );
    }
}

/// File the worker's letter from the worker's own seat, through the one
/// road a typed `send` takes.
fn file(app: &AppHandle, host: &dyn Host, term: u32, standing: &Asked, now_ms: i64) {
    let seat = &standing.seat;
    let (subject, body) = letter(&standing.prompt, seat, term);
    let request = format!("worker-question-{}-{}", seat.worker, standing.filed_ms);
    let filed =
        crate::agent_teams::current_pane_capability(&seat.team, &seat.pane).and_then(|token| {
            let argv: Vec<String> = [
                "send",
                "--to",
                seat.coordinator.as_str(),
                "--type",
                "question",
                "--subject",
                subject.as_str(),
                "--body",
                body.as_str(),
                "--retry-request",
                request.as_str(),
            ]
            .iter()
            .map(|word| (*word).to_string())
            .collect();
            let answer = crate::orchestration::run(
                host,
                Vec::new(),
                &seat.team,
                &seat.pane,
                &token,
                &argv,
                now_ms,
            );
            (answer.exit_code == 0)
                .then(|| serde_json::from_str::<serde_json::Value>(answer.stdout.trim()).ok())
                .flatten()
                .and_then(|said| said["messageId"].as_str().map(str::to_string))
        });
    let mut held = asked();
    let Some(now) = held.get_mut(&term) else {
        return;
    };
    match filed {
        Some(question) => {
            now.filed = Filing::Filed(question);
            now.filed_ms = now_ms;
        }
        None if now.passed => {
            held.remove(&term);
        }
        None => {
            now.raised = Some(Raised::Unsent);
            let (report, worktree) = (now.report.clone(), now.worktree.clone());
            drop(held);
            raise(app, term, &report, worktree.as_deref(), Raised::Unsent);
        }
    }
}

/// One beat for a question in the ledger.
fn carry(app: &AppHandle, term: u32, standing: &Asked, question: &str, now_ms: i64) {
    let seat_now = crate::orchestration::worker_ask_seat(term);
    let ledger = crate::orchestration::question_standing(&standing.seat.run, question);
    let next = step(
        &standing.prompt,
        &ledger,
        standing.passed,
        seat_now.as_ref(),
        standing.raised.is_some(),
        standing.filed_ms,
        now_ms,
    );
    let seat = &standing.seat;
    match next {
        Step::Wait => {}
        Step::Forget => settled(term),
        Step::RefusePassed => {
            tell(
                seat,
                question,
                &passed_letter(&seat.worker, term, question),
                now_ms,
            );
            settled(term);
        }
        Step::RefuseHeld => {
            tell(
                seat,
                question,
                &held_letter(&seat.worker, term, question),
                now_ms,
            );
            settled(term);
            // Taken between two beats, before it was raised: the person's
            // screen gets it now, as a pane the person has asks them.
            if standing.raised.is_none() {
                raise(
                    app,
                    term,
                    &standing.report,
                    standing.worktree.as_deref(),
                    Raised::TakenOver,
                );
            }
        }
        Step::Raise(why) => {
            // Only a question the pane still asks: a hook that moved on in
            // the meantime has the last word.
            let still = match asked().get_mut(&term) {
                Some(now) if !now.passed => {
                    now.raised = Some(why);
                    true
                }
                _ => false,
            };
            if still {
                raise(
                    app,
                    term,
                    &standing.report,
                    standing.worktree.as_deref(),
                    why,
                );
            }
        }
        Step::Answer(selections) => {
            match crate::cmd::terminal::answer_ask(
                app.clone(),
                term,
                standing.report.agent.slug().to_string(),
                standing.prompt.clone(),
                selections,
            ) {
                Ok(()) => typed(term),
                Err(why) if why == crate::cmd::terminal::ANSWER_IN_FLIGHT => {}
                Err(why) if why == crate::cmd::terminal::QUESTION_CHANGED => {
                    tell(
                        seat,
                        question,
                        &passed_letter(&seat.worker, term, question),
                        now_ms,
                    );
                    settled(term);
                }
                Err(why) => {
                    tell(
                        seat,
                        question,
                        &unanswered_letter(&seat.worker, term, question, &why),
                        now_ms,
                    );
                    settled(term);
                }
            }
        }
    }
}

/// The window is done with `term`'s question: answered, refused or closed.
fn settled(term: u32) {
    asked().remove(&term);
}

/// The coordinator's answer to `term`'s question is going into the picker.
fn typed(term: u32) {
    settled(term);
}

/// One letter from the ledger to the run's coordinator about `question`.
fn tell(seat: &AskSeat, question: &str, body: &str, now_ms: i64) {
    crate::orchestration::post_observation_once(
        Some(seat.run.as_str()),
        &seat.coordinator,
        body,
        &format!("worker-question-told-{question}"),
        now_ms,
    );
}

/// Give the person the question after all: why first, so the card says it,
/// then the asking report noted again through the one road every report
/// takes to the page, and the bell a question rings.
fn raise(app: &AppHandle, term: u32, report: &PaneHookReport, worktree: Option<&str>, why: Raised) {
    let _ = app.emit(
        RAISED_EVENT,
        serde_json::json!({ "term": term, "why": why }),
    );
    crate::pane_runtime::note_pane_state(app, worktree, report);
    if let Some(worktree) = worktree {
        crate::pane_runtime::ring_for_pane(app, worktree, report);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::ask::{AskOption, AskQuestion};

    const NOW: i64 = 1_800_000_000_000;

    fn prompt(multi: bool) -> AskPrompt {
        AskPrompt {
            questions: vec![AskQuestion {
                question: "cargo 시험 다음 단계로 넘어갈까요?".to_string(),
                header: Some("다음 단계".to_string()),
                multi_select: multi,
                options: vec![
                    AskOption {
                        label: "진행".to_string(),
                        description: Some("clippy까지 돌림".to_string()),
                    },
                    AskOption {
                        label: "멈춤".to_string(),
                        description: None,
                    },
                    AskOption {
                        label: "다시".to_string(),
                        description: None,
                    },
                ],
            }],
        }
    }

    fn seat(coordinator_live: bool, taken: bool) -> AskSeat {
        AskSeat {
            run: "run-1".to_string(),
            worker: "w-2".to_string(),
            team: "team-3".to_string(),
            pane: "%4".to_string(),
            coordinator: "run:run-1".to_string(),
            coordinator_live,
            taken,
            dispatch: Some("dp-9".to_string()),
        }
    }

    #[test]
    fn a_workers_question_goes_to_its_coordinator_while_somebody_sits_there() {
        assert_eq!(route(Some(&seat(true, false))), Route::Coordinator);
    }

    #[test]
    fn a_question_with_no_coordinator_or_no_worker_or_a_taken_pane_is_the_persons() {
        assert_eq!(route(Some(&seat(false, false))), Route::Person);
        assert_eq!(route(Some(&seat(true, true))), Route::Person);
        assert_eq!(route(None), Route::Person);
    }

    #[test]
    fn a_question_the_pane_moved_past_is_refused_back_to_the_coordinator_and_never_raised() {
        let asked = prompt(false);
        let answered = Standing::Answered("1".to_string());
        assert_eq!(
            step(
                &asked,
                &answered,
                true,
                Some(&seat(true, false)),
                false,
                NOW,
                NOW
            ),
            Step::RefusePassed
        );
        // Past the coordinator's time, a passed question still goes to nobody.
        let late = NOW + COORDINATOR_ANSWER_WAIT_MS * 3;
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                true,
                Some(&seat(true, false)),
                false,
                NOW,
                late
            ),
            Step::Wait
        );
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                true,
                Some(&seat(false, false)),
                false,
                NOW,
                late
            ),
            Step::Wait
        );
    }

    #[test]
    fn a_coordinator_silent_for_ten_minutes_gives_the_question_to_the_person() {
        let asked = prompt(false);
        let live = seat(true, false);
        let almost = NOW + COORDINATOR_ANSWER_WAIT_MS - 1;
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                false,
                Some(&live),
                false,
                NOW,
                almost
            ),
            Step::Wait
        );
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                false,
                Some(&live),
                false,
                NOW,
                NOW + COORDINATOR_ANSWER_WAIT_MS
            ),
            Step::Raise(Raised::CoordinatorSilent)
        );
        // Once, not on every beat after.
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                false,
                Some(&live),
                true,
                NOW,
                NOW + COORDINATOR_ANSWER_WAIT_MS * 2
            ),
            Step::Wait
        );
        assert_eq!(COORDINATOR_ANSWER_WAIT_MS, 600_000);
    }

    #[test]
    fn a_seat_that_empties_or_a_pane_the_person_takes_raises_the_question_at_once() {
        let asked = prompt(false);
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                false,
                Some(&seat(false, false)),
                false,
                NOW,
                NOW
            ),
            Step::Raise(Raised::CoordinatorGone)
        );
        assert_eq!(
            step(&asked, &Standing::Waiting, false, None, false, NOW, NOW),
            Step::Raise(Raised::CoordinatorGone)
        );
        assert_eq!(
            step(
                &asked,
                &Standing::Waiting,
                false,
                Some(&seat(true, true)),
                false,
                NOW,
                NOW
            ),
            Step::Raise(Raised::TakenOver)
        );
    }

    #[test]
    fn the_coordinators_answer_is_typed_as_the_picks_a_card_would_send() {
        let asked = prompt(false);
        assert_eq!(
            step(
                &asked,
                &Standing::Answered("2".to_string()),
                false,
                Some(&seat(true, false)),
                false,
                NOW,
                NOW
            ),
            Step::Answer(vec![AskSelection {
                indices: vec![1],
                other: String::new(),
            }])
        );
        // Raised to the person and then answered by the coordinator: the
        // person holds it, and the late answer goes back with no key typed.
        assert_eq!(
            step(
                &asked,
                &Standing::Answered("멈추고 보고해".to_string()),
                false,
                Some(&seat(true, false)),
                true,
                NOW,
                NOW
            ),
            Step::RefuseHeld
        );
        assert_eq!(
            step(
                &asked,
                &Standing::Closed,
                false,
                Some(&seat(true, false)),
                false,
                NOW,
                NOW
            ),
            Step::Forget
        );
    }

    #[test]
    fn a_late_answer_to_a_question_the_person_holds_is_refused_and_never_typed() {
        let asked = prompt(false);
        let answered = Standing::Answered("2".to_string());
        let late = NOW + COORDINATOR_ANSWER_WAIT_MS + 1;
        // Given to the person after ten silent minutes, for an empty seat, or
        // because the person took the pane: every raised question refuses.
        for live in [seat(true, false), seat(false, false), seat(true, true)] {
            assert_eq!(
                step(&asked, &answered, false, Some(&live), true, NOW, late),
                Step::RefuseHeld,
                "{live:?}"
            );
        }
        assert_eq!(
            step(&asked, &answered, false, None, true, NOW, late),
            Step::RefuseHeld
        );
        // Taken but not raised yet — the answer and the hand met between two
        // beats: the hand wins.
        assert_eq!(
            step(
                &asked,
                &answered,
                false,
                Some(&seat(true, true)),
                false,
                NOW,
                NOW
            ),
            Step::RefuseHeld
        );
        // Nobody holds it but the coordinator: typed as before.
        assert_eq!(
            step(
                &asked,
                &answered,
                false,
                Some(&seat(true, false)),
                false,
                NOW,
                NOW
            ),
            Step::Answer(vec![AskSelection {
                indices: vec![1],
                other: String::new(),
            }])
        );
        let words = held_letter("w-2", 7, "m-5");
        assert!(words.starts_with("사람이 맡은 질문"), "{words}");
        assert!(words.contains("m-5") && words.contains("term-7"), "{words}");
        assert!(words.contains("넣지 않았습니다"), "{words}");
    }

    #[test]
    fn a_reply_reads_as_choice_numbers_or_as_free_words() {
        let single = prompt(false);
        let picked = |reply: &str, prompt: &AskPrompt| selections_from_reply(prompt, reply);
        assert_eq!(picked(" 3 ", &single).unwrap()[0].indices, vec![2]);
        // Two numbers on a single-select, a number out of range, or words are
        // the free answer, word for word.
        for words in ["1,2", "4", "0", "2번으로 하되 clippy 먼저"] {
            let selection = &picked(words, &single).unwrap()[0];
            assert!(selection.indices.is_empty(), "{words}");
            assert_eq!(selection.other, words);
        }
        let multi = prompt(true);
        assert_eq!(picked("1, 3", &multi).unwrap()[0].indices, vec![0, 2]);
        assert_eq!(picked("", &single), None);
    }

    #[test]
    fn several_questions_take_one_line_each() {
        let mut both = prompt(false);
        let mut second = both.questions[0].clone();
        second.question = "보고서는 어디에?".to_string();
        both.questions.push(second);
        let selections = selections_from_reply(&both, "1\nreviews 폴더").unwrap();
        assert_eq!(selections[0].indices, vec![0]);
        assert_eq!(selections[1].other, "reviews 폴더");
    }

    #[test]
    fn the_letter_carries_the_question_and_every_choice_word_for_word_with_the_pane() {
        let (subject, body) = letter(&prompt(false), &seat(true, false), 7);
        assert_eq!(subject, "다음 단계");
        assert!(body.contains("w-2"), "{body}");
        assert!(body.contains("term-7"), "{body}");
        assert!(body.contains("dp-9"), "{body}");
        assert!(
            body.contains("cargo 시험 다음 단계로 넘어갈까요?"),
            "{body}"
        );
        assert!(body.contains("  1. 진행 — clippy까지 돌림\n"), "{body}");
        assert!(body.contains("  2. 멈춤\n"), "{body}");
        assert!(body.contains("  3. 다시\n"), "{body}");
        assert!(body.contains("10분"), "{body}");
    }

    fn asking_report(term: u32) -> PaneHookReport {
        serde_json::from_value(serde_json::json!({
            "term": term,
            "agent": "claude",
            "state": "needs-attention",
            "event": "PreToolUse",
            "resumable": false,
            "ask_prompt": prompt(false),
        }))
        .expect("an asking report reads back")
    }

    fn filed(term: u32, question: &str) -> Asked {
        Asked {
            signature: signature(&prompt(false)),
            prompt: prompt(false),
            report: asking_report(term),
            worktree: Some("/work/t-1".to_string()),
            seat: seat(true, false),
            filed: Filing::Filed(question.to_string()),
            filed_ms: NOW,
            passed: false,
            raised: None,
        }
    }

    #[test]
    fn a_question_written_down_before_a_restart_is_carried_on_by_the_next_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join(crate::app_paths::artifact_file::WORKER_QUESTIONS);
        let mut held = HashMap::new();
        held.insert(7, filed(7, "m-5"));
        held.insert(
            8,
            Asked {
                raised: Some(Raised::CoordinatorSilent),
                ..filed(8, "m-6")
            },
        );
        // Not in the ledger yet, or moved past: nothing to carry on.
        held.insert(
            9,
            Asked {
                filed: Filing::Unfiled,
                ..filed(9, "m-7")
            },
        );
        held.insert(
            10,
            Asked {
                passed: true,
                ..filed(10, "m-8")
            },
        );
        let mut written = Vec::new();
        write_kept(&path, &kept_of(&held, &[]), &mut written).unwrap();
        // The window restarts: what it carried is gone, the file is not.
        drop(held);
        let memory = read_kept(&path);
        assert_eq!(memory.written, written);
        let questions: Vec<(&str, Option<Raised>)> = memory
            .waiting
            .iter()
            .map(|kept| (kept.question.as_str(), kept.raised))
            .collect();
        assert_eq!(
            questions,
            vec![("m-5", None), ("m-6", Some(Raised::CoordinatorSilent))]
        );
        let kept = memory.waiting[0].clone();
        assert_eq!((kept.run.as_str(), kept.worker.as_str()), ("run-1", "w-2"));
        // Its worker sits at another term in the new window.
        let live = seat(true, false);
        let back = taken_up(kept, live.clone(), 12).expect("the kept report still asks");
        assert_eq!(back.report.term, 12);
        assert_eq!(back.prompt, prompt(false));
        assert_eq!(back.signature, signature(&prompt(false)));
        assert!(matches!(&back.filed, Filing::Filed(question) if question == "m-5"));
        let after = |standing: &Standing, now_ms: i64| {
            step(
                &back.prompt,
                standing,
                back.passed,
                Some(&live),
                back.raised.is_some(),
                back.filed_ms,
                now_ms,
            )
        };
        // The coordinator's answer goes into the picker...
        assert_eq!(
            after(&Standing::Answered("2".to_string()), NOW + 60_000),
            Step::Answer(vec![AskSelection {
                indices: vec![1],
                other: String::new(),
            }])
        );
        // ...and with none, the ten minutes count from the asking, not from
        // the restart.
        assert_eq!(
            after(&Standing::Waiting, NOW + COORDINATOR_ANSWER_WAIT_MS - 1),
            Step::Wait
        );
        assert_eq!(
            after(&Standing::Waiting, NOW + COORDINATOR_ANSWER_WAIT_MS),
            Step::Raise(Raised::CoordinatorSilent)
        );
        // The same questions are the same bytes: nothing is written again.
        let mut again = memory.written.clone();
        write_kept(
            &path,
            &kept_of(&HashMap::new(), &memory.waiting),
            &mut again,
        )
        .unwrap();
        assert_eq!(again, memory.written);
    }

    #[test]
    fn a_file_that_is_missing_or_unreadable_carries_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join(crate::app_paths::artifact_file::WORKER_QUESTIONS);
        let mut nothing = read_kept(&path);
        assert!(nothing.waiting.is_empty());
        // Nothing carried and nothing written before: no file appears.
        write_kept(&path, &[], &mut nothing.written).unwrap();
        assert!(!path.exists());
        std::fs::write(&path, b"{not json").unwrap();
        assert!(read_kept(&path).waiting.is_empty());
        // A kept report that asks nothing is not taken up.
        let mut said = asking_report(3);
        said.ask_prompt = None;
        let kept = Kept {
            run: "run-1".to_string(),
            worker: "w-2".to_string(),
            question: "m-5".to_string(),
            filed_ms: NOW,
            raised: None,
            report: said,
            worktree: None,
            seated_ms: None,
        };
        assert!(taken_up(kept, seat(true, false), 4).is_none());
    }

    #[test]
    fn the_passed_letter_says_the_question_passed_and_nothing_was_typed() {
        let words = passed_letter("w-2", 7, "m-5");
        assert!(words.starts_with("지나간 질문"), "{words}");
        assert!(words.contains("m-5") && words.contains("term-7"), "{words}");
    }

    /// t-84343: an account to pick, a secret value, a payment, a login — the
    /// coordinator gives the question to the person with one word, and the
    /// person is asked at once rather than after ten silent minutes.
    #[test]
    fn a_coordinator_who_answers_to_person_gives_the_question_to_the_person_at_once() {
        let asked = prompt(false);
        let live = seat(true, false);
        // Spaces around it, English in any case, and a sentence's end after
        // it — 「사람에게.」 is the word too, never an answer typed in.
        let spoken = TO_PERSON
            .iter()
            .map(|word| (*word).to_string())
            .chain([
                format!(" {}\n", TO_PERSON[0]),
                TO_PERSON[1].to_uppercase(),
                format!("{}.", TO_PERSON[0]),
                format!("{}。", TO_PERSON[0]),
                format!("{}!!", TO_PERSON[1]),
                format!("{} ?", TO_PERSON[0]),
            ]);
        for word in spoken {
            let handed = Standing::Answered(word.clone());
            // At once, and nothing is typed into the picker.
            assert_eq!(
                step(&asked, &handed, false, Some(&live), false, NOW, NOW),
                Step::Raise(Raised::CoordinatorHanded),
                "{word:?}"
            );
            // Given already: the worker waits for the person, and the word is
            // never refused back to the coordinator as a late answer.
            assert_eq!(
                step(&asked, &handed, false, Some(&live), true, NOW, NOW + 1),
                Step::Wait,
                "{word:?}"
            );
            // The person answered — the pane moved past it: nobody is told.
            assert_eq!(
                step(&asked, &handed, true, Some(&live), true, NOW, NOW + 2),
                Step::Forget,
                "{word:?}"
            );
        }
        // Any other words are the answer, typed into the picker as before.
        let other = format!("{} 물어봐", TO_PERSON[0]);
        assert_eq!(
            step(
                &asked,
                &Standing::Answered(other.clone()),
                false,
                Some(&live),
                false,
                NOW,
                NOW
            ),
            Step::Answer(vec![AskSelection {
                indices: Vec::new(),
                other,
            }])
        );
    }

    /// t-84343 (4275's word): a question given to the person whose worker
    /// ends, or whose pane goes, before the person answers is let go quietly —
    /// no letter to the coordinator, nothing left to refuse later.
    #[test]
    fn a_question_given_to_the_person_goes_quietly_with_its_worker_or_its_pane() {
        let question = prompt(false);
        let handed = Standing::Answered(TO_PERSON[0].to_string());
        // The worker is no live worker at that pane any more.
        for raised in [false, true] {
            assert_eq!(
                step(&question, &handed, false, None, raised, NOW, NOW),
                Step::Forget,
                "raised: {raised}"
            );
        }
        // The pane goes: nothing is kept for a late answer to refuse.
        let term = u32::MAX - 84_343;
        asked().insert(
            term,
            Asked {
                raised: Some(Raised::CoordinatorHanded),
                ..filed(term, "m-handed")
            },
        );
        forget(term);
        assert!(asked().get(&term).is_none());
        assert!(left().get("m-handed").is_none());
    }

    #[test]
    fn the_letter_ends_on_the_words_that_give_the_question_to_the_person() {
        let (_, body) = letter(&prompt(false), &seat(true, false), 7);
        let last = body.lines().last().unwrap_or_default();
        // Each word in quotes, so it reads as what to write, not whom to ask.
        for word in TO_PERSON {
            assert!(last.contains(&format!("「{word}」")), "{last}");
        }
        // The answer line says a number stands alone on the first line.
        assert!(body.contains("번호만"), "{body}");
    }

    /// t-84470 (r1 P3-1): a letter the ledger can no longer take an answer to
    /// — the worker's dispatch ended, the letter is gone — while the pane
    /// still asks is the person's, with its bell and its reason.
    #[test]
    fn a_question_the_ledger_closed_while_the_pane_still_asks_is_given_to_the_person() {
        let asked = prompt(false);
        for live in [Some(seat(true, false)), None] {
            assert_eq!(
                step(
                    &asked,
                    &Standing::Closed,
                    false,
                    live.as_ref(),
                    false,
                    NOW,
                    NOW
                ),
                Step::Raise(Raised::Unanswerable),
                "{live:?}"
            );
        }
        // Moved past, or the person holds it already: nothing to give.
        assert_eq!(
            step(
                &asked,
                &Standing::Closed,
                true,
                Some(&seat(true, false)),
                false,
                NOW,
                NOW
            ),
            Step::Forget
        );
        assert_eq!(
            step(
                &asked,
                &Standing::Closed,
                false,
                Some(&seat(true, false)),
                true,
                NOW,
                NOW
            ),
            Step::Forget
        );
    }

    /// t-84470 (r1 P3-5): 「2」 and a reason under it picks row 2; 「1.」 and
    /// 「3)」 are numbers.
    #[test]
    fn a_choice_number_is_read_off_the_first_line_with_or_without_its_dot() {
        let single = prompt(false);
        for (reply, row) in [
            ("2\n이유: clippy 먼저", 1),
            ("1.", 0),
            ("3)", 2),
            (" 2. \n\n왜냐하면", 1),
        ] {
            let picked = selections_from_reply(&single, reply).expect(reply);
            assert_eq!(picked[0].indices, vec![row], "{reply:?}");
            assert!(picked[0].other.is_empty(), "{reply:?}");
        }
        // Words on the first line are the free answer, the whole of it.
        let words = "clippy 먼저\n그다음 2";
        assert_eq!(
            selections_from_reply(&single, words).expect(words)[0].other,
            words
        );
        let multi = prompt(true);
        assert_eq!(
            selections_from_reply(&multi, "1., 3)\n둘 다").expect("multi")[0].indices,
            vec![0, 2]
        );
    }

    /// t-84470 (r1 P3-3): the walk that types an answer takes hundreds of
    /// milliseconds; until the pane says it moved on, the person's screen does
    /// not see the question come back for that moment.
    #[test]
    fn an_answer_being_typed_keeps_the_question_off_the_persons_screen_until_the_pane_moves_on() {
        // Terms no other test holds: the table is the window's own.
        let term = u32::MAX - 84_470;
        asked().insert(term, filed(term, "m-typed"));
        assert!(hidden(term));
        typed(term);
        assert!(
            hidden(term),
            "the answer is still being typed and the person's screen shows the question"
        );
        // The pane moved on: the answer landed, and nothing is left to refuse.
        let mut working = asking_report(term);
        working.state = zerocode_core::hook::HookState::Working;
        working.ask_prompt = None;
        heard(&working, None, NOW);
        assert!(!hidden(term));
        assert!(asked().get(&term).is_none());
        assert!(left().get("m-typed").is_none());
        // A pane that goes while its answer is typed leaves nothing either.
        let gone = u32::MAX - 84_471;
        asked().insert(gone, filed(gone, "m-typed-gone"));
        typed(gone);
        forget(gone);
        assert!(left().get("m-typed-gone").is_none());
    }

    /// t-84470 (r2 P3-1): after a restart the pane asks the same question
    /// again and its hook comes before the beat that would take the written
    /// letter up — one letter, not two, and a late answer is never lost.
    #[test]
    fn a_question_the_hook_asks_again_before_the_beat_takes_it_up_keeps_its_first_letter() {
        assert_eq!(
            taking_up(
                &Standing::Waiting,
                true,
                true,
                || panic!("the screen is not read"),
                0
            ),
            TakeUp::Meet
        );
        let written = HashMap::from([(
            7,
            Asked {
                raised: Some(Raised::CoordinatorSilent),
                ..filed(7, "m-5")
            },
        )]);
        let kept = kept_of(&written, &[]).remove(0);
        // The same question, not filed yet: it takes the written letter on,
        // its ten minutes from the first asking, the person's again.
        let mut same = Asked {
            filed: Filing::Unfiled,
            filed_ms: NOW + 90_000,
            ..filed(12, "unused")
        };
        match meet(&mut same, kept.clone(), seat(true, false), 12) {
            Met::Adopted(raise) => assert_eq!(raise, Some(Raised::CoordinatorSilent)),
            other => panic!("{other:?}"),
        }
        assert!(matches!(&same.filed, Filing::Filed(question) if question == "m-5"));
        assert_eq!(same.filed_ms, NOW);
        assert_eq!(same.raised, Some(Raised::CoordinatorSilent));
        // Another question at the pane: the written letter is left.
        let mut other = Asked {
            signature: "another question".to_string(),
            filed: Filing::Unfiled,
            ..filed(12, "unused")
        };
        match meet(&mut other, kept.clone(), seat(true, false), 12) {
            Met::Left(old) => {
                assert!(matches!(&old.filed, Filing::Filed(question) if question == "m-5"));
                assert_eq!(old.report.term, 12);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(other.filed, Filing::Unfiled));
        // The same question filed again already — a second letter went before
        // this beat: the first is left, so an answer to it is refused.
        let mut again = filed(12, "m-9");
        assert!(matches!(
            meet(&mut again, kept, seat(true, false), 12),
            Met::Left(_)
        ));
        assert!(matches!(&again.filed, Filing::Filed(question) if question == "m-9"));
    }

    /// t-84470 (r2 P3-2): the answer was typed and the window died before it
    /// wrote that down. The next window finds the letter answered and the
    /// screen past it: the answer went in, and nobody is told otherwise.
    #[test]
    fn a_written_question_answered_and_gone_from_the_screen_is_dropped_quietly() {
        let answered = Standing::Answered("2".to_string());
        assert_eq!(
            taking_up(&answered, true, false, || false, TAKE_UP_GRACE_MS),
            TakeUp::Drop
        );
        // Unanswered and gone: kept, so a late answer is refused as passed.
        assert_eq!(
            taking_up(&Standing::Waiting, true, false, || false, TAKE_UP_GRACE_MS),
            TakeUp::Leave
        );
        // Not drawn yet, shown, unseated, closed: as before.
        assert_eq!(
            taking_up(&answered, true, false, || false, TAKE_UP_GRACE_MS - 1),
            TakeUp::Later
        );
        assert_eq!(
            taking_up(&answered, true, false, || true, 0),
            TakeUp::Carry
        );
        assert_eq!(
            taking_up(&Standing::Waiting, false, false, || true, 0),
            TakeUp::Later
        );
        assert_eq!(
            taking_up(&Standing::Closed, true, false, || true, 0),
            TakeUp::Drop
        );
    }

    /// t-84470 (r2 P3-3): a write that fails is tried again on the next beat
    /// rather than at the next change, and the window's log says it once.
    #[test]
    fn a_write_that_fails_is_tried_again_and_noted_once() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the folder should be: every write fails until it goes.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, b"").unwrap();
        let path = blocked.join(crate::app_paths::artifact_file::WORKER_QUESTIONS);
        let mut memory = read_kept(&path);
        let kept = kept_of(&HashMap::from([(7, filed(7, "m-5"))]), &[]);
        assert!(write_noting(&path, &kept, &mut memory).is_some());
        assert_eq!(
            write_noting(&path, &kept, &mut memory),
            None,
            "the same failure is noted once"
        );
        std::fs::remove_file(&blocked).unwrap();
        assert_eq!(write_noting(&path, &kept, &mut memory), None);
        assert_eq!(
            std::fs::read(&path).ok(),
            serde_json::to_vec(&kept).ok(),
            "the next beat wrote what the failed one could not"
        );
        // A failure after a write that went is noted again.
        std::fs::remove_dir_all(&blocked).unwrap();
        std::fs::write(&blocked, b"").unwrap();
        let more = kept_of(
            &HashMap::from([(7, filed(7, "m-5")), (8, filed(8, "m-6"))]),
            &[],
        );
        assert!(write_noting(&path, &more, &mut memory).is_some());
    }
}
