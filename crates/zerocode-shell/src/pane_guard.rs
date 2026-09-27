//! The two tool guards asked of the agents in this window's panes (t-10916):
//! what a pane's agent is about to run, ran and read — the moments its hooks
//! carry ([`zerocode_core::hook_guard::moments`]) — put to the command guard
//! and the tool text guard with the questions zo asks
//! ([`zerocode_core::jev::tool_guard`]), and hindsight's label on what became
//! of each, filed where zo files its own ([`systemone::ledger_of`]): one
//! ledger per seat on this machine, one series, one judge, one standing.
//!
//! # Nothing waits, nothing is decided
//!
//! The bridge answered the agent's hook before this file sees the envelope
//! (`zerocode_hookd::receive_hook` sends it on, then replies): the script's
//! reply is the bytes it always was, and no permission is decided here. This
//! file reads the envelope on the window's hook loop — the few `stat`s of
//! the places a command names, taken before it runs — and asks each question
//! on a thread of its own. It records only: whatever the seat's mode, an
//! answer changes nothing the agent sees.
//!
//! # What waits on hindsight
//!
//! Each pane keeps a book ([`PaneBook`]): the commands asked about and not yet
//! settled, the shell commands its turn started (a later restore is matched
//! on them), the blocks waiting on the step after them, and the prompt that
//! began the turn. A turn's end settles what its facts settle through the
//! same entries zo's books hold ([`CommandWaiting::settle_turn`]); a block
//! is settled by the calls that start after it, once one of them comes back.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;
use zerocode_core::AgentKind;
use zerocode_core::hook_guard::{self, Moment, Sees};
use zerocode_core::jev::door::{ANSWERED_OUTCOME, Memo};
use zerocode_core::jev::tool_guard::{
    ASKED_AFTER, ASKED_BEFORE, Asked, CommandGuardRow, CommandWaiting, HostFraming, TextSource,
    TextWaiting, ToolTextGuardRow, Verdict, asks_about, carries_out, command_questions,
    command_state, confidence_of, shelve, squeezed, task_line_of, text_questions, text_state,
};
use zerocode_core::jev::{
    COMMAND_GUARD, COMMAND_GUARD_FLAG_FLOOR_PERMILLE, JevMode, JevUse, ROUTE_USE_FALLBACK,
    TOOL_TEXT_GUARD, TOOL_TEXT_INSTRUCTED_FLOOR_PERMILLE, fingerprint_of, memo, noul,
    task_fingerprint,
};

use crate::systemone::{self, SCHEMA, Wire, request_body};

/// What the two guards differ in, as this file asks them.
struct Guard {
    seat: &'static JevUse,
    /// The yes line, per thousand, at which a Noul flags the call.
    flag_floor_permille: u16,
}

const COMMAND: Guard = Guard {
    seat: &COMMAND_GUARD,
    flag_floor_permille: COMMAND_GUARD_FLAG_FLOOR_PERMILLE,
};

const TEXT: Guard = Guard {
    seat: &TOOL_TEXT_GUARD,
    flag_floor_permille: TOOL_TEXT_INSTRUCTED_FLOOR_PERMILLE,
};

impl Guard {
    /// The wall one question waits for its answer — the seat's own.
    fn deadline(&self) -> Duration {
        Duration::from_millis(self.seat.apply_deadline_ms.unwrap_or_default())
    }
}

/// The pane a moment is about.
#[derive(Debug, Clone)]
pub(crate) struct Pane {
    pub term: u32,
    pub agent: AgentKind,
    /// The agent's session, when its payload names one: a call's name is its
    /// session's and its own ([`task_fingerprint`]).
    pub session: Option<String>,
    /// The folder the pane works in — the project a command's folder is
    /// judged against, and the name a row files its project under.
    pub worktree: PathBuf,
}

impl Pane {
    /// What a book's entries are owned by, and a call's name is made from.
    fn owner(&self) -> String {
        self.session
            .clone()
            .unwrap_or_else(|| crate::hooks::pane_key_of(self.term))
    }

    /// The folder's last name, as a row of the machine's one ledger names
    /// its project.
    fn place(&self) -> Option<String> {
        self.worktree
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }
}

/// A block of text whose next step is being gathered.
#[derive(Debug)]
struct Step {
    judged: u64,
    /// The block, squeezed as the comparison reads it.
    block: String,
    /// The calls started after it: their ids, tools and what each carries out.
    calls: Vec<(Option<String>, String, Option<String>)>,
}

/// What one pane's agent has asked about and done that waits on hindsight.
#[derive(Debug, Default)]
struct PaneBook {
    /// Whose session the book is — a new session in the same pane starts a
    /// new book.
    owner: String,
    /// The first line of the prompt that began the turn: what a command is
    /// for.
    task: String,
    /// The person's words of the turn, squeezed: what an order in a tool's
    /// text must not be.
    persons: String,
    /// The shell commands the turn started, in order, each by its call's id.
    calls: Vec<(String, String)>,
    /// Commands asked about, waiting on their answer and their hindsight.
    commands: Vec<CommandWaiting>,
    /// Commands asked about that have not come back, each with whether the
    /// person was asked to allow it.
    open: Vec<(String, bool)>,
    /// Blocks waiting on their answer and on the step after them.
    texts: Vec<TextWaiting>,
    /// Blocks whose next step is being gathered.
    steps: Vec<Step>,
    /// How a call its payload names no id for is named.
    unnamed: u64,
}

/// How the book names a call its payload named none, before its count.
const UNNAMED: &str = "unnamed-";

impl PaneBook {
    /// A call's id, or the book's own name for one its payload named none.
    fn call_id(&mut self, id: Option<String>) -> String {
        id.unwrap_or_else(|| {
            self.unnamed += 1;
            format!("{UNNAMED}{}", self.unnamed)
        })
    }
}

/// A question to ask off the hook loop.
#[derive(Debug, Clone)]
enum Question {
    Command {
        judged: u64,
        attempt: String,
        command: String,
        cwd: PathBuf,
        task: String,
        rule: Verdict,
        outside_paths: usize,
        moment: &'static str,
    },
    Text {
        judged: u64,
        attempt: String,
        tool: String,
        source: TextSource,
        text: String,
    },
}

/// Every pane's book.
#[derive(Debug, Default)]
pub(crate) struct Guards {
    panes: HashMap<u32, PaneBook>,
}

/// The window's books.
static GUARDS: LazyLock<Mutex<Guards>> = LazyLock::new(Mutex::default);

/// Read one envelope the hook loop received, for the two guards: its
/// moments, if its agent's row sees any, from the pane it speaks for. Takes
/// the stamps a command needs before it runs and hands every question to a
/// thread; returns before any leaves.
pub(crate) fn note_hook(
    app: &tauri::AppHandle,
    envelope: &zerocode_core::HookEnvelope,
    expected_launch_token: Option<&str>,
) {
    // A payload the guards cannot read leaves that envelope unasked and the
    // hook loop standing: every road after this one reads the same envelope.
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        read_hook(app, envelope, expected_launch_token);
    })) {
        eprintln!(
            "jev guards: an envelope was left unasked: {}",
            crate::system_runtime::panic_payload(panic.as_ref())
        );
    }
}

/// [`note_hook`]'s reading, inside its guard.
fn read_hook(
    app: &tauri::AppHandle,
    envelope: &zerocode_core::HookEnvelope,
    expected_launch_token: Option<&str>,
) {
    let payload = zerocode_core::payload::HookPayload::of(&envelope.payload);
    let Some((term, event)) =
        crate::hooks::guard_event_of(envelope, &payload, expected_launch_token)
    else {
        return;
    };
    let moments = hook_guard::moments_parsed(envelope.agent, &event, &payload);
    if moments.is_empty() {
        return;
    }
    let session = zerocode_core::provider_session::session_in_parsed(envelope.agent, &payload)
        .map(|session| session.id);
    // A nested run's hooks wear the pane's identity: they are not the
    // pane's agent, and its guards are its own.
    if !crate::pane_runtime::speaks_for_its_pane(
        app,
        term,
        envelope.agent.slug(),
        session.as_deref(),
        false,
    ) {
        return;
    }
    let pane = Pane {
        term,
        agent: envelope.agent,
        session,
        worktree: PathBuf::from(&envelope.worktree_id),
    };
    drop(note(
        &GUARDS,
        &Wire::of_this_machine(),
        &pane,
        moments,
        crate::usage_runtime::epoch_ms_now(),
    ));
}

/// A pane that closed: what its agent left waiting on hindsight never gets
/// it, and the next occupant of its id starts a book of its own.
pub(crate) fn forget_term(term: u32) {
    GUARDS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .panes
        .remove(&term);
}

/// [`note_hook`]'s body, with the books and the wire handed in: file each
/// moment in `pane`'s book, write the labels a turn's end settles, and ask
/// each question on a thread of its own — whose handles are answered, for a
/// caller that waits on them.
pub(crate) fn note(
    guards: &'static Mutex<Guards>,
    wire: &Wire,
    pane: &Pane,
    moments: Vec<Moment>,
    now_ms: i64,
) -> Vec<JoinHandle<()>> {
    let mut questions = Vec::new();
    let mut labels: Vec<(&'static JevUse, Value)> = Vec::new();
    {
        let mut held = guards.lock().unwrap_or_else(PoisonError::into_inner);
        let owner = pane.owner();
        let book = held.panes.entry(pane.term).or_default();
        if book.owner != owner {
            *book = PaneBook {
                owner,
                ..PaneBook::default()
            };
        }
        let at = u64::try_from(now_ms).unwrap_or_default();
        for moment in moments {
            file(book, pane, moment, at, &mut questions, &mut labels);
        }
    }
    record_labels(wire, labels, now_ms);
    questions
        .into_iter()
        .filter_map(|question| {
            let wire = wire.clone();
            let pane = pane.clone();
            std::thread::Builder::new()
                .name("jev-pane-guard".to_string())
                .spawn(move || ask(guards, &wire, &pane, question))
                .ok()
        })
        .collect()
}

/// File one moment in `book`.
fn file(
    book: &mut PaneBook,
    pane: &Pane,
    moment: Moment,
    at: u64,
    questions: &mut Vec<Question>,
    labels: &mut Vec<(&'static JevUse, Value)>,
) {
    match moment {
        Moment::Prompt(words) => {
            book.task = task_line_of(&words);
            book.persons = squeezed(&words);
        }
        Moment::Started {
            call_id,
            tool,
            words,
        } => {
            for step in &mut book.steps {
                step.calls
                    .push((call_id.clone(), tool.clone(), words.clone()));
            }
        }
        Moment::CommandAbout(call) => {
            let Some(command) = call.command else {
                return;
            };
            // One call can be reported twice before it runs — a shell's own
            // event that names no call, and the tool event of the same call
            // (Cursor's `beforeShellExecution` and `preToolUse`): a command
            // whose words are a call still open, where one of the two reports
            // named no call, is that call. Two calls that each name
            // themselves are two, whatever they run.
            let named = call.id.is_some();
            if open_call_running(book, &command, |open| !named || is_unnamed(open)).is_some() {
                return;
            }
            let id = book.call_id(call.id);
            if book.calls.iter().any(|(seen, _)| *seen == id) {
                return;
            }
            book.calls.push((id.clone(), command.clone()));
            if !asks_about(&command) {
                return;
            }
            let cwd = call.cwd.unwrap_or_else(|| pane.worktree.clone());
            let judged = task_fingerprint(&book.owner, &id);
            let waiting =
                CommandWaiting::asked(judged, &book.owner, &id, &command, &cwd, &pane.worktree);
            let (rule, outside_paths) = (waiting.rule(), waiting.outside.len());
            shelve(&mut book.commands, waiting);
            shelve(&mut book.open, (id, false));
            questions.push(Question::Command {
                judged,
                attempt: book.owner.clone(),
                command,
                cwd,
                task: book.task.clone(),
                rule,
                outside_paths,
                moment: ASKED_BEFORE,
            });
        }
        Moment::Asked { command } => {
            let asked = book.open.iter_mut().rev().find(|(id, _)| {
                command.is_none()
                    || book
                        .calls
                        .iter()
                        .any(|(seen, words)| seen == id && Some(words) == command.as_ref())
            });
            if let Some((_, asked)) = asked {
                *asked = true;
            }
        }
        Moment::CommandRan {
            call,
            failed,
            stopped,
        } => {
            // The entry the call was asked as: by its id — else, for a payload
            // that names none or a call first reported by an event that named
            // none, the newest open call that ran these words.
            let entry = call
                .id
                .clone()
                .filter(|id| book.commands.iter().any(|one| one.tool_use_id == *id))
                .or_else(|| {
                    let named = call.id.is_some();
                    call.command.as_deref().and_then(|command| {
                        open_call_running(book, command, |open| !named || is_unnamed(open))
                    })
                });
            if let Some(id) = entry
                && let Some(one) = book.commands.iter_mut().find(|one| one.tool_use_id == id)
            {
                one.ran(failed, stopped);
                book.open.retain(|(open, _)| *open != id);
                return;
            }
            // An agent whose hooks carry nothing before a tool runs is asked
            // here: nothing was stamped before the run, so the row is all
            // there is — no hindsight is kept.
            if matches!(hook_guard::sight(pane.agent).before, Sees::No(_))
                && let Some(command) = call.command
            {
                let id = book.call_id(call.id);
                if book.calls.iter().any(|(seen, _)| *seen == id) {
                    return;
                }
                book.calls.push((id.clone(), command.clone()));
                if !asks_about(&command) {
                    return;
                }
                let cwd = call.cwd.unwrap_or_else(|| pane.worktree.clone());
                let (irreversible, reaches) =
                    zerocode_core::jev::tool_guard::todays_rule(&command, &cwd);
                questions.push(Question::Command {
                    judged: task_fingerprint(&book.owner, &id),
                    attempt: book.owner.clone(),
                    command,
                    cwd,
                    task: book.task.clone(),
                    rule: if irreversible || reaches {
                        Verdict::Flagged
                    } else {
                        Verdict::Plain
                    },
                    outside_paths: 0,
                    moment: ASKED_AFTER,
                });
            }
        }
        Moment::Text {
            call_id,
            tool,
            source,
            text,
        } => {
            let id = book.call_id(call_id);
            if book.texts.iter().any(|one| one.tool_use_id == id) {
                return;
            }
            let judged = task_fingerprint(&book.owner, &id);
            shelve(
                &mut book.texts,
                TextWaiting {
                    judged,
                    owner: book.owner.clone(),
                    tool_use_id: id,
                    // The host that framed the block is the agent's CLI, and
                    // nothing it sends says what it put around the words.
                    framing: HostFraming::Unknown,
                    verdict: None,
                    confidence: None,
                    applied: false,
                    decided: None,
                },
            );
            shelve(
                &mut book.steps,
                Step {
                    judged,
                    block: squeezed(&text),
                    calls: Vec::new(),
                },
            );
            questions.push(Question::Text {
                judged,
                attempt: book.owner.clone(),
                tool,
                source,
                text,
            });
        }
        Moment::Finished { call_id } => {
            let Some(call_id) = call_id else {
                return;
            };
            let over: Vec<Step> = extract(&mut book.steps, |step| {
                step.calls
                    .iter()
                    .any(|(id, _, _)| id.as_deref() == Some(call_id.as_str()))
            });
            for step in over {
                decide_step(book, step, at, labels);
            }
        }
        Moment::TurnEnded { stopped } => end_turn(book, stopped, at, labels),
    }
}

/// The newest call asked about that has not come back and runs `command`,
/// among the open calls `may_be` accepts.
fn open_call_running(
    book: &PaneBook,
    command: &str,
    may_be: impl Fn(&str) -> bool,
) -> Option<String> {
    book.open.iter().rev().find_map(|(id, _)| {
        (may_be(id)
            && book
                .calls
                .iter()
                .any(|(seen, words)| seen == id && words == command))
        .then(|| id.clone())
    })
}

/// Whether a call's id is the book's own name for a call its payload named
/// none ([`PaneBook::call_id`]).
fn is_unnamed(id: &str) -> bool {
    id.starts_with(UNNAMED)
}

/// Remove and answer the entries of `from` that `take` names.
fn extract<T>(from: &mut Vec<T>, take: impl Fn(&T) -> bool) -> Vec<T> {
    let (taken, kept): (Vec<T>, Vec<T>) =
        std::mem::take(from).into_iter().partition(|one| take(one));
    *from = kept;
    taken
}

/// Settle a block on the step after it: followed when one of the calls that
/// started after it carried out words it spelled and the person's did not.
fn decide_step(
    book: &mut PaneBook,
    step: Step,
    at: u64,
    labels: &mut Vec<(&'static JevUse, Value)>,
) {
    let followed = step.calls.iter().find_map(|(_, tool, words)| {
        words
            .as_deref()
            .map(squeezed)
            .filter(|words| carries_out(words, &step.block, &book.persons))
            .map(|_| tool.clone())
    });
    let Some(at_entry) = book.texts.iter().position(|one| one.judged == step.judged) else {
        return;
    };
    book.texts[at_entry].decided = Some((followed.is_some(), followed));
    if book.texts[at_entry].verdict.is_some() {
        let one = book.texts.remove(at_entry);
        push_label(labels, &TEXT, one.label(at));
    }
}

/// A turn of the pane's ended: settle what its facts settle.
fn end_turn(
    book: &mut PaneBook,
    stopped: bool,
    at: u64,
    labels: &mut Vec<(&'static JevUse, Value)>,
) {
    // A call still open when the turn ends: stopped with it when the person
    // stopped the turn while it ran, refused when the person was asked to
    // allow it and it never ran; otherwise nothing says what became of it.
    for (id, asked) in std::mem::take(&mut book.open) {
        let Some(at_entry) = book.commands.iter().position(|one| one.tool_use_id == id) else {
            continue;
        };
        if stopped || asked {
            book.commands[at_entry].ran(stopped, true);
        } else {
            book.commands.remove(at_entry);
        }
    }
    let calls = std::mem::take(&mut book.calls);
    for one in &mut book.commands {
        let from = calls
            .iter()
            .position(|(id, _)| *id == one.tool_use_id)
            .map_or(0, |at_call| at_call + 1);
        let later: Vec<&str> = calls[from..]
            .iter()
            .map(|(_, command)| command.as_str())
            .collect();
        one.settle_turn((!stopped).then_some(later.as_slice()));
    }
    for one in extract(&mut book.commands, |one| {
        one.decided.is_some() && one.verdict.is_some()
    }) {
        push_label(labels, &COMMAND, one.label(at));
    }
    // A block whose next step began is settled on what began; one with no
    // step after it before its turn ended never gets its hindsight.
    for step in std::mem::take(&mut book.steps) {
        if step.calls.is_empty() {
            book.texts.retain(|one| one.judged != step.judged);
        } else {
            decide_step(book, step, at, labels);
        }
    }
}

fn push_label<R: serde::Serialize>(
    labels: &mut Vec<(&'static JevUse, Value)>,
    guard: &Guard,
    row: Option<R>,
) {
    if let Some(row) = row.and_then(|row| serde_json::to_value(row).ok()) {
        labels.push((guard.seat, row));
    }
}

/// Write each seat's labels to its ledger, off the hook loop.
fn record_labels(wire: &Wire, labels: Vec<(&'static JevUse, Value)>, now_ms: i64) {
    if labels.is_empty() {
        return;
    }
    let wire = wire.clone();
    drop(std::thread::spawn(move || {
        let mut by_seat: BTreeMap<&'static str, (&'static JevUse, Vec<Value>)> = BTreeMap::new();
        for (seat, row) in labels {
            by_seat
                .entry(seat.id)
                .or_insert((seat, Vec::new()))
                .1
                .push(row);
        }
        for (seat, rows) in by_seat.into_values() {
            if let Some(ledger) = systemone::ledger_of(&wire, seat) {
                systemone::record_rows(seat, &ledger, &rows, now_ms);
            }
        }
    }));
}

/// Ask one question and file what came of it: the row, and the verdict in
/// the book — whose label, when its hindsight is already in, is written now.
fn ask(guards: &'static Mutex<Guards>, wire: &Wire, pane: &Pane, question: Question) {
    let guard = match question {
        Question::Command { .. } => &COMMAND,
        Question::Text { .. } => &TEXT,
    };
    let judged = match &question {
        Question::Command { judged, .. } | Question::Text { judged, .. } => *judged,
    };
    let (mode, _) = systemone::standing_in(wire, guard.seat, zerocode_core::jev::Run::Fresh);
    let Some(ledger) = systemone::ledger_of(wire, guard.seat).filter(|_| mode.asks()) else {
        settle(guards, wire, pane, judged, Verdict::Unavailable, None);
        return;
    };
    let now_ms = crate::usage_runtime::epoch_ms_now();
    let at = u64::try_from(now_ms).unwrap_or_default();
    let (row, verdict, confidence) = match question {
        Question::Command {
            judged,
            attempt,
            command,
            cwd,
            task,
            rule,
            outside_paths,
            moment,
        } => {
            let questions = command_questions(noul::question);
            let asked = put(
                wire,
                guard,
                &cwd,
                command_state(&command, &cwd, &task),
                &questions,
                mode,
            );
            let verdict = Verdict::of(asked.answers.as_ref(), guard.flag_floor_permille);
            let confidence = asked.answers.as_ref().and_then(confidence_of);
            let row = CommandGuardRow {
                at,
                attempt: Some(attempt),
                judged,
                rubric_version: guard.seat.rubric_version,
                command: fingerprint_of(&command),
                command_chars: command.chars().count(),
                rule: rule.word().to_string(),
                outside_paths,
                verdict: verdict.word().to_string(),
                noted: false,
                from: Some(pane.agent.slug().to_string()),
                moment: Some(moment.to_string()),
                pane: pane.place(),
                asked,
            };
            (serde_json::to_value(row).ok(), verdict, confidence)
        }
        Question::Text {
            judged,
            attempt,
            tool,
            source,
            text,
        } => {
            let questions = text_questions(noul::question);
            let asked = put(
                wire,
                guard,
                &pane.worktree,
                text_state(source, &text),
                &questions,
                mode,
            );
            let verdict = Verdict::of(asked.answers.as_ref(), guard.flag_floor_permille);
            let confidence = asked.answers.as_ref().and_then(confidence_of);
            let row = ToolTextGuardRow {
                at,
                attempt: Some(attempt),
                judged,
                rubric_version: guard.seat.rubric_version,
                tool,
                source: source.word().to_string(),
                text_chars: text.chars().count(),
                framing: HostFraming::Unknown.word().to_string(),
                verdict: verdict.word().to_string(),
                fenced: false,
                noted: false,
                from: Some(pane.agent.slug().to_string()),
                pane: pane.place(),
                asked,
            };
            (serde_json::to_value(row).ok(), verdict, confidence)
        }
    };
    if let Some(row) = row {
        systemone::record_rows(guard.seat, &ledger, &[row], now_ms);
    }
    settle(guards, wire, pane, judged, verdict, confidence);
}

/// Put `questions` over `state`, from words of `workspace`, through the door
/// and its memo: an answer the memo held for these exact bytes answers a
/// repeat with no request (`cached`), and a fresh one is remembered. Read the
/// way zo reads the same answer: every Noul, or the rule that broke.
fn put(
    wire: &Wire,
    guard: &Guard,
    workspace: &Path,
    state: Value,
    questions: &BTreeMap<String, Value>,
    mode: JevMode,
) -> Asked {
    let memo_file = systemone::memo_path(wire);
    let began = Instant::now();
    let body = request_body(
        &state,
        &Value::Object(
            questions
                .iter()
                .map(|(id, question)| (id.clone(), question.clone()))
                .collect(),
        ),
    );
    let answered = wire.ask_remembering(
        guard.seat,
        Some(workspace),
        body,
        guard.deadline(),
        memo_file.as_deref().map(|path| Memo {
            path,
            seat: guard.seat,
            applying: true,
        }),
    );
    let cached = answered.memo.as_ref().is_some_and(|memoed| memoed.answered);
    let mut asked = Asked {
        outcome: String::new(),
        answers: None,
        route_use: ROUTE_USE_FALLBACK.to_string(),
        applied: false,
        elapsed_ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
        retries: 0,
        requests: answered.spent.requests,
        redacted_lines: u32::try_from(answered.spent.redacted_lines).unwrap_or(u32::MAX),
        model: answered.spent.model.clone(),
        input_tokens: None,
        request_bytes: (answered.request_bytes > 0).then_some(answered.request_bytes),
        request_digest: None,
        rejected: None,
        cached,
    };
    let body = match answered.answer {
        Ok(body) => body,
        Err(token) => {
            asked.outcome = token;
            return asked;
        }
    };
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    asked.input_tokens = parsed
        .pointer("/usage/input_tokens")
        .and_then(Value::as_u64);
    let answers = parsed.get("answers").cloned().unwrap_or(Value::Null);
    let read: Result<BTreeMap<String, f64>, &'static str> = questions
        .keys()
        .map(|id| {
            noul::read(&answers, id)
                .map(|yes| (id.clone(), yes))
                .map_err(noul::NoulRefusal::token)
        })
        .collect();
    match read {
        Ok(read) => {
            asked.outcome = ANSWERED_OUTCOME.to_string();
            asked.answers = Some(read);
            asked.route_use = mode.key().to_string();
            if !cached
                && let (Some(path), Some(memoed)) = (memo_file.as_deref(), answered.memo.as_ref())
            {
                let _ = memo::remember(
                    path,
                    guard.seat,
                    &memoed.key,
                    &body,
                    crate::usage_runtime::epoch_ms_now(),
                );
            }
        }
        Err(rule) => {
            asked.outcome = SCHEMA.to_string();
            asked.rejected = Some(rule.to_string());
        }
    }
    asked
}

/// The verdict of the call `judged` names has answered — or has not: one
/// that reached no verdict leaves the book, one whose hindsight is already in
/// writes its label now.
fn settle(
    guards: &'static Mutex<Guards>,
    wire: &Wire,
    pane: &Pane,
    judged: u64,
    verdict: Verdict,
    confidence: Option<f64>,
) {
    let mut labels = Vec::new();
    {
        let mut held = guards.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(book) = held.panes.get_mut(&pane.term) else {
            return;
        };
        let at = u64::try_from(crate::usage_runtime::epoch_ms_now()).unwrap_or_default();
        if let Some(at_entry) = book.commands.iter().position(|one| one.judged == judged) {
            if verdict == Verdict::Unavailable {
                book.commands.remove(at_entry);
                book.open
                    .retain(|(id, _)| book.commands.iter().any(|one| one.tool_use_id == *id));
            } else {
                book.commands[at_entry].verdict = Some(verdict);
                book.commands[at_entry].confidence = confidence;
                if book.commands[at_entry].decided.is_some() {
                    let one = book.commands.remove(at_entry);
                    push_label(&mut labels, &COMMAND, one.label(at));
                }
            }
        }
        if let Some(at_entry) = book.texts.iter().position(|one| one.judged == judged) {
            if verdict == Verdict::Unavailable {
                book.texts.remove(at_entry);
                book.steps.retain(|step| step.judged != judged);
            } else {
                book.texts[at_entry].verdict = Some(verdict);
                book.texts[at_entry].confidence = confidence;
                if book.texts[at_entry].decided.is_some() {
                    let one = book.texts.remove(at_entry);
                    push_label(&mut labels, &TEXT, one.label(at));
                }
            }
        }
    }
    record_labels(wire, labels, crate::usage_runtime::epoch_ms_now());
}

#[cfg(test)]
mod replay;
#[cfg(test)]
mod tests;
