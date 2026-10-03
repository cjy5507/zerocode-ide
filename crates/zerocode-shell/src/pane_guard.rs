//! The Jev seats asked of the agents in this window's panes: the two tool
//! guards (t-10916) — what a pane's agent is about to run, ran and read — and
//! the completion claim and file pick seats (t-11349) — what it says its turn
//! finished, and which files the person's request needed. Each moment its
//! hooks carry ([`zerocode_core::hook_guard::moments`]) is put to its seat
//! with the questions zo asks ([`zerocode_core::jev::tool_guard`],
//! [`zerocode_core::jev::claim`], [`zerocode_core::jev::file_pick`]), and
//! hindsight's label on what became of each is filed where zo files its own:
//! the guards' in the machine's one ledger per seat ([`systemone::ledger_of`]),
//! the claim and file pick seats' in the ledger zo keeps for the pane's own
//! folder ([`systemone::project_ledger_of`]) — one series, one judge, one
//! standing with zo's.
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
//! # A seat left off costs nothing
//!
//! Which seats are asked is read off a snapshot ([`Standing`]) that the
//! window's file watcher refreshes whenever zo's settings file changes
//! ([`WATCH_LANE`]); a file that does not read switches every seat off. The
//! hook loop reads the snapshot before anything else: a seat left off reads
//! nothing of a pane's event — no `stat`, no thread, no text copied — and
//! with every seat off the event is not read at all.
//!
//! # What waits on hindsight
//!
//! Each pane keeps a book ([`PaneBook`]): the commands asked about and not yet
//! settled, the shell commands its turn started (a later restore is matched
//! on them), the blocks waiting on the step after them, and the prompt that
//! began the turn. A turn's end settles what its facts settle through the
//! same entries zo's books hold ([`CommandWaiting::settle_turn`]); a block
//! is settled by the calls that start after it, once one of them comes back.
//! A turn's file pick waits on its answer and on the files the turn's edits
//! wrote, and is settled when the turn ends; a turn's claims wait on their
//! verdict and on the person's next prompt ([`ClaimWaiting::label`]).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;
use zerocode_core::AgentKind;
use zerocode_core::file_find::{self, FindAsk};
use zerocode_core::hook_guard::tally::{self, TurnTally};
use zerocode_core::hook_guard::{self, Asking, Moment, PANE_SEATS, Sees};
use zerocode_core::jev::claim::{self, ClaimCheckRow, ClaimWaiting, CodeVerdict, Evidence};
use zerocode_core::jev::door::{ANSWERED_OUTCOME, Memo};
use zerocode_core::jev::file_pick::{
    self, CandidateBatch, FilePickAsk, FilePickCandidate, FilePickRow,
};
use zerocode_core::jev::summary::CONTROL;
use zerocode_core::jev::tool_guard::{
    ASKED_AFTER, ASKED_BEFORE, Asked, CommandGuardRow, CommandWaiting, HostFraming, TextSource,
    TextWaiting, ToolTextGuardRow, Verdict, asks_about, carries_out, command_questions,
    command_state, confidence_of, shelve, squeezed, task_line_of, text_questions, text_state,
};
use zerocode_core::jev::{
    CLAIM, COMMAND_GUARD, COMMAND_GUARD_FLAG_FLOOR_PERMILLE, FILE_PICK, FILE_PICK_CANDIDATE_CAP,
    FILE_PICK_HINT_FILE_CAP, JevMode, JevUse, ROUTE_USE_FALLBACK, Run, TOOL_TEXT_GUARD,
    TOOL_TEXT_INSTRUCTED_FLOOR_PERMILLE, choice, fingerprint_of, memo, noul, task_fingerprint,
};
use zerocode_core::transcript::SaidAt;

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

/// The wall one question of `seat`'s waits for its answer — the seat's own.
fn deadline_of(seat: &JevUse) -> Duration {
    Duration::from_millis(seat.apply_deadline_ms.unwrap_or_default())
}

/* ---- which seats are asked ------------------------------------------------------ */

/// The file watcher's lane zo's settings file rides in: a change re-reads
/// which seats are asked ([`settings_moved`]).
pub(crate) const WATCH_LANE: &str = "jev-settings";

/// Which of the seats a pane's moments serve are asked, as zo's settings
/// were last read — what the hook loop reads before anything else.
#[derive(Debug, Default)]
pub(crate) struct Standing {
    asking: Mutex<Asking>,
}

impl Standing {
    /// The seats asked now.
    pub(crate) fn asking(&self) -> Asking {
        *self.asking.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Read `wire`'s settings again: each seat is asked while its mode asks,
    /// and a file that does not read leaves every seat off.
    pub(crate) fn read(&self, wire: &Wire) {
        let root = wire.settings_root();
        let asking = Asking::of(
            PANE_SEATS
                .into_iter()
                .filter(|seat| seat.mode_in_run(&root, Run::Fresh).asks()),
        );
        *self.asking.lock().unwrap_or_else(PoisonError::into_inner) = asking;
    }
}

/// The window's standing.
static STANDING: LazyLock<Standing> = LazyLock::new(Standing::default);

/// Put zo's settings file on the window's file watcher, and read which seats
/// are asked now — the window's boot.
pub(crate) fn watch_settings(watched: &crate::file_watch::WatchSet) {
    if let Some(settings) = crate::api_routers::zo_settings_path() {
        watched.replace_lane(
            WATCH_LANE,
            vec![(settings.to_string_lossy().into_owned(), Some(settings))],
            false,
        );
    }
    STANDING.read(&Wire::of_this_machine());
}

/// zo's settings file changed: read which seats are asked again.
pub(crate) fn settings_moved() {
    STANDING.read(&Wire::of_this_machine());
}

/* ---- the books ---------------------------------------------------------------- */

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
    /// Whether the pane is a worker the window summoned, with its dispatch
    /// open: where a seat that speaks at a turn's start may act first
    /// (t-14869). The person's own panes are recorded only.
    pub worker: bool,
    /// The prompt as the bridge read it for a turn's brief, by fingerprint
    /// ([`prompt_key`]) — `None` where the event carries no prompt the
    /// bridge reads.
    pub prompt_key: Option<String>,
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

    /// The folder as the agent's own process sees it — its physical path,
    /// symlinks resolved — which zo run in the same folder keeps its
    /// project's rows under. Read off the hook loop.
    fn root(&self) -> PathBuf {
        self.worktree
            .canonicalize()
            .unwrap_or_else(|_| self.worktree.clone())
    }

    /// Who asked, as a row names it.
    fn asker(&self) -> Option<String> {
        Some(self.agent.slug().to_string())
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

/// A turn's claims, waiting on their verdict and the person's next prompt,
/// and the ledger their request was filed in.
#[derive(Debug)]
struct ClaimEntry {
    waiting: ClaimWaiting,
    ledger: Option<PathBuf>,
}

impl ClaimEntry {
    /// Its label as it leaves the book, once both halves are in
    /// ([`ClaimWaiting::label`]).
    fn label(self, at: u64) -> Option<Filed> {
        Some(Filed {
            seat: &CLAIM,
            ledger: Some(self.ledger?),
            row: serde_json::to_value(self.waiting.label(at)?).ok()?,
        })
    }
}

/// A turn's file pick, waiting on its answer and on the files its turn's
/// edits wrote.
#[derive(Debug)]
struct PickWaiting {
    judged: u64,
    /// The answered request's row, the folder it was asked from — its
    /// physical path — and the ledger it was filed in.
    asked: Option<(Value, PathBuf, PathBuf)>,
    /// The files its turn's edits wrote, once the turn ended.
    edited: Option<Vec<String>>,
    /// The calls its turn looked around with before its first edit, once the
    /// turn ended ([`TurnTally::explore_before_first_edit`]).
    looked: Option<usize>,
}

impl PickWaiting {
    /// Its label, once both halves are in ([`file_pick::label_row`]): the
    /// edits read under the folder's physical path, or under `handed` — the
    /// spelling the pane was handed, which an agent may name its files by.
    fn label(&self, handed: &Path, at: u64) -> Option<Filed> {
        let ((row, root, ledger), edited) = (self.asked.as_ref()?, self.edited.as_ref()?);
        let label = file_pick::label_row(
            row,
            self.judged,
            &file_pick::edited_fingerprints(&[root.as_path(), handed], edited),
            self.looked,
            at,
        );
        Some(Filed {
            seat: &FILE_PICK,
            ledger: Some(ledger.clone()),
            row: serde_json::to_value(label).ok()?,
        })
    }
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
    /// The turns the book has seen end: what a turn's claim and file pick
    /// are named by.
    turns: u64,
    /// The turn's finished calls, as a claim may cite them — gathered while
    /// the claim seat is asked, and dropped when the turn ends.
    evidence: Vec<Evidence<String>>,
    /// The files this session's edits wrote, newest first: the recent-edit
    /// order a file pick is held against, and a source of its candidates.
    edited: Vec<String>,
    /// The files this turn's edits wrote.
    turn_edited: Vec<String>,
    /// Turns' claims waiting on their verdict and the person's next prompt.
    claims: Vec<ClaimEntry>,
    /// Turns' file picks waiting on their answer and their turn's edits.
    picks: Vec<PickWaiting>,
    /// The turn's calls, as its ruler counts them (t-14869).
    tally: TurnTally,
    /// What the turn's brief waits on, in a summoned worker's pane.
    briefing: Option<Briefing>,
    /// The pane as its hooks last said it: what `zerocode-find` asks from.
    pane: Option<Pane>,
    /// The picks `zerocode-find` asked this turn.
    finds: u64,
    /// Rows of the turns counted and not yet written.
    counted: Vec<Value>,
}

/// A turn's file pick as its brief waits on it (t-14869): the prompt the
/// bridge read, by [`prompt_key`], the pick it asked, when the waiting brief
/// gives up, and what there is to say.
#[derive(Debug)]
struct Briefing {
    key: String,
    judged: u64,
    until: Option<Instant>,
    said: Said,
}

/// Where a turn's brief stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Said {
    /// The question is out.
    Asking,
    /// Its line, offered to the waiting brief.
    Ready(String),
    /// The brief took the line.
    Taken,
    /// Nothing to say.
    Nothing,
}

/// How often a brief and the question it waits on look at each other's
/// word: a tenth of the pane's median trip from a prompt to its file pick's
/// answer (96 ms over 102 rows, 2026-09-29).
const BRIEF_POLL: Duration = Duration::from_millis(10);

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

    /// The name the turn under way is asked under — its claim's and its
    /// file pick's: the book's owner and the turns it has seen end.
    fn attempt(&self) -> String {
        format!(
            "{:016x}",
            task_fingerprint(&self.owner, &self.turns.to_string())
        )
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
    /// A turn's claims: where its answer is, and the turn's finished calls
    /// they may cite.
    Claim {
        judged: u64,
        said: SaidAt,
        evidence: Vec<Evidence<String>>,
    },
    /// A turn's file pick: the person's request, and the files this
    /// session's edits wrote before it, newest first.
    FilePick {
        judged: u64,
        attempt: String,
        request: String,
        edited: Vec<String>,
    },
}

/// A row to write off the hook loop: its seat's, in `ledger` when it names
/// one, else in the seat's machine ledger.
#[derive(Debug)]
struct Filed {
    seat: &'static JevUse,
    ledger: Option<PathBuf>,
    row: Value,
}

/// Every pane's book.
#[derive(Debug, Default)]
pub(crate) struct Guards {
    panes: HashMap<u32, PaneBook>,
}

/// The window's books.
static GUARDS: LazyLock<Mutex<Guards>> = LazyLock::new(Mutex::default);

/// Read one envelope the hook loop received, for the seats a pane's work is
/// put to: nothing at all while none is asked; else its moments, if its
/// agent's row sees any, from the pane it speaks for. Takes the stamps a
/// command needs before it runs and hands every question to a thread;
/// returns before any leaves.
pub(crate) fn note_hook(
    app: &tauri::AppHandle,
    envelope: &zerocode_core::HookEnvelope,
    expected_launch_token: Option<&str>,
) {
    // A payload the seats cannot read leaves that envelope unasked and the
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
    let asking = STANDING.asking();
    if asking.nothing() {
        return;
    }
    let payload = zerocode_core::payload::HookPayload::of(&envelope.payload);
    let Some((term, event)) =
        crate::hooks::guard_event_of(envelope, &payload, expected_launch_token)
    else {
        return;
    };
    let mut moments = hook_guard::moments_parsed(envelope.agent, &event, &payload, asking);
    // The turn's ruler rides the file pick seat: its label reads the count.
    if asking.file_pick {
        moments.extend(
            tally::tallied_parsed(envelope.agent, &event, &payload)
                .into_iter()
                .map(Moment::Tally),
        );
    }
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
        worker: summoned(term),
        prompt_key: zerocode_hookd::turn_prompt(envelope).map(|prompt| prompt_key(&prompt)),
    };
    drop(note(
        &GUARDS,
        &Wire::of_this_machine(),
        &pane,
        asking,
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
/// moment in `pane`'s book for the seats `asking` asks, write the labels a
/// turn's end settles, and ask each question on a thread of its own — whose
/// handles are answered, for a caller that waits on them.
pub(crate) fn note(
    guards: &'static Mutex<Guards>,
    wire: &Wire,
    pane: &Pane,
    asking: Asking,
    moments: Vec<Moment>,
    now_ms: i64,
) -> Vec<JoinHandle<()>> {
    let mut questions = Vec::new();
    let mut labels: Vec<Filed> = Vec::new();
    let counted;
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
        book.pane = Some(pane.clone());
        for moment in moments {
            file(book, pane, asking, moment, at, &mut questions, &mut labels);
        }
        counted = std::mem::take(&mut book.counted);
    }
    record_labels(wire, labels, now_ms);
    record_turns(wire, counted);
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
    asking: Asking,
    moment: Moment,
    at: u64,
    questions: &mut Vec<Question>,
    labels: &mut Vec<Filed>,
) {
    match moment {
        Moment::Prompt(words) => {
            // What the turn before left: its file pick is settled on the
            // files its edits wrote so far, and the answer before this
            // prompt is graded by what the person opened it with.
            settle_picks(book, pane, at, labels);
            count_turn(book, pane, at);
            if let Some(failure) = claim::next_person_failed(&words) {
                grade_claims(book, failure, at, labels);
            }
            book.turn_edited.clear();
            book.evidence.clear();
            let asks_pick = asking.file_pick && file_pick::is_code_edit_intent(&words);
            let attempt = book.attempt();
            let judged = task_fingerprint(&attempt, FILE_PICK.id);
            book.briefing = pane
                .prompt_key
                .clone()
                .filter(|_| pane.worker)
                .map(|key| Briefing {
                    key,
                    judged,
                    until: None,
                    said: if asks_pick {
                        Said::Asking
                    } else {
                        Said::Nothing
                    },
                });
            if asks_pick && !book.picks.iter().any(|pick| pick.judged == judged) {
                shelve(
                    &mut book.picks,
                    PickWaiting {
                        judged,
                        asked: None,
                        edited: None,
                        looked: None,
                    },
                );
                questions.push(Question::FilePick {
                    judged,
                    attempt,
                    request: words.clone(),
                    edited: book.edited.clone(),
                });
            }
            book.task = task_line_of(&words);
            book.persons = squeezed(&words);
        }
        Moment::Started {
            call_id,
            tool,
            words,
            paths,
        } => {
            for step in &mut book.steps {
                step.calls
                    .push((call_id.clone(), tool.clone(), words.clone()));
            }
            for path in paths {
                note_edit(book, path);
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
        Moment::Finished { call_id, evidence } => {
            if let Some(evidence) = evidence {
                shelve(&mut book.evidence, evidence);
            }
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
        Moment::Tally(told) => book.tally.take(told),
        Moment::TurnEnded { stopped, said } => {
            end_turn(book, stopped, at, labels);
            settle_picks(book, pane, at, labels);
            count_turn(book, pane, at);
            book.turn_edited.clear();
            let evidence = std::mem::take(&mut book.evidence);
            if let Some(said) = said {
                let judged = task_fingerprint(&book.owner, &book.attempt());
                shelve(
                    &mut book.claims,
                    ClaimEntry {
                        waiting: ClaimWaiting {
                            judged,
                            verdict: None,
                            failure: None,
                            confidence: None,
                            compared: false,
                        },
                        ledger: None,
                    },
                );
                questions.push(Question::Claim {
                    judged,
                    said,
                    evidence,
                });
            }
            book.turns += 1;
        }
    }
}

/// An edit wrote `path`: the turn's, and this session's newest.
fn note_edit(book: &mut PaneBook, path: String) {
    book.edited.retain(|held| *held != path);
    book.edited.insert(0, path.clone());
    book.edited.truncate(FILE_PICK_CANDIDATE_CAP);
    shelve(&mut book.turn_edited, path);
}

/// A turn ended, or the next one began: every file pick still waiting on its
/// turn's edits has them now, and each whose answer is in is labeled.
fn settle_picks(book: &mut PaneBook, pane: &Pane, at: u64, labels: &mut Vec<Filed>) {
    for pick in &mut book.picks {
        if pick.edited.is_none() {
            pick.edited = Some(book.turn_edited.clone());
            pick.looked = book.tally.explore_before_first_edit();
        }
    }
    for pick in extract(&mut book.picks, |pick| {
        pick.asked.is_some() && pick.edited.is_some()
    }) {
        labels.extend(pick.label(&pane.worktree, at));
    }
}

/// A turn ended, or the next one began: its ruler's row is kept to be
/// written, and the next turn is counted from nothing.
fn count_turn(book: &mut PaneBook, pane: &Pane, at: u64) {
    let tally = std::mem::take(&mut book.tally);
    if let Some(row) = tally
        .row(at, pane.agent.slug(), pane.place())
        .and_then(|row| serde_json::to_value(row).ok())
    {
        book.counted.push(row);
    }
}

/// The person's next prompt opened with `failure` or not: the newest claim
/// still waiting on it is graded — labeled now when its verdict is in.
fn grade_claims(book: &mut PaneBook, failure: bool, at: u64, labels: &mut Vec<Filed>) {
    let Some(at_entry) = book
        .claims
        .iter()
        .rposition(|entry| entry.waiting.failure.is_none())
    else {
        return;
    };
    book.claims[at_entry].waiting.failure = Some(failure);
    if book.claims[at_entry].waiting.verdict.is_some() {
        let entry = book.claims.remove(at_entry);
        labels.extend(entry.label(at));
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
fn decide_step(book: &mut PaneBook, step: Step, at: u64, labels: &mut Vec<Filed>) {
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
fn end_turn(book: &mut PaneBook, stopped: bool, at: u64, labels: &mut Vec<Filed>) {
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

fn push_label<R: serde::Serialize>(labels: &mut Vec<Filed>, guard: &Guard, row: Option<R>) {
    if let Some(row) = row.and_then(|row| serde_json::to_value(row).ok()) {
        labels.push(Filed {
            seat: guard.seat,
            ledger: None,
            row,
        });
    }
}

/// Write each ledger's labels, off the hook loop: a seat's machine ledger,
/// or the one a row names.
fn record_labels(wire: &Wire, labels: Vec<Filed>, now_ms: i64) {
    if labels.is_empty() {
        return;
    }
    let wire = wire.clone();
    drop(std::thread::spawn(move || {
        let mut by_ledger: BTreeMap<PathBuf, (&'static JevUse, Vec<Value>)> = BTreeMap::new();
        for filed in labels {
            let Some(ledger) = filed
                .ledger
                .or_else(|| systemone::ledger_of(&wire, filed.seat))
            else {
                continue;
            };
            by_ledger
                .entry(ledger)
                .or_insert((filed.seat, Vec::new()))
                .1
                .push(filed.row);
        }
        for (ledger, (seat, rows)) in by_ledger {
            systemone::record_rows(seat, &ledger, &rows, now_ms);
        }
    }));
}

/// Write the turns counted, off the hook loop, in the ledger beside the
/// seats' ([`tally::PANE_TURNS_LEDGER`]).
fn record_turns(wire: &Wire, rows: Vec<Value>) {
    let Some(ledger) = rows
        .first()
        .and_then(|_| systemone::requests_file(wire, tally::PANE_TURNS_LEDGER))
    else {
        return;
    };
    drop(std::thread::spawn(move || {
        systemone::append_rows(&ledger, &rows);
    }));
}

/* ---- the turn's brief (t-14869) --------------------------------------------------- */

/// One voice in a turn's brief (t-24298): what it has to tell the agent in
/// the asking pane as the person's prompt begins, in at most the characters
/// it is given — the room the voices before it left — or nothing. Answered
/// within the ask's wall.
pub(crate) type BriefContributor = fn(&zerocode_hookd::TurnBriefAsk, usize) -> Option<String>;

/// The voices a turn's brief is made of, in the order the agent reads them:
/// the person's own pointer first — what "this" in the prompt means — then
/// the file pick seat's likely files. A new voice is one more entry.
const BRIEF_CONTRIBUTORS: &[BriefContributor] =
    &[crate::tree_selection::brief_line, file_pick_line];

/// What separates two voices in a brief: a blank line, the way the bridge
/// itself sets the vault's block apart from the contract.
const BRIEF_VOICE_GAP: &str = "\n\n";

/// The voices' lines joined a blank line apart, in order, each voice handed
/// the room the ones before it left under `cap` — the bridge's cap, past
/// which it drops a brief whole (`zerocode_hookd::TURN_BRIEF_CHAR_CAP`).
///
/// A voice answers inside its room or not at all, so a line it returns is
/// never thrown away after it was given (a voice that records having said
/// something can trust that it reached the agent); a voice left no room is
/// not asked. A line over its room breaks that promise and is left out —
/// the bridge would drop every voice's line with it.
///
/// Time is spent the same way. The bridge's timeout wraps the whole brief
/// (`ask.wall`), so each voice is handed what is left of it, and once it has
/// run out no voice is asked: a voice waiting out a full wall after another
/// spent part of it would have the bridge drop every line.
pub(crate) fn compose_brief(
    voices: &[BriefContributor],
    ask: &zerocode_hookd::TurnBriefAsk,
    cap: usize,
) -> Option<String> {
    let began = Instant::now();
    let mut brief = String::new();
    let mut used = 0;
    for voice in voices {
        let wall = ask.wall.saturating_sub(began.elapsed());
        if wall.is_zero() {
            break;
        }
        let gap = if brief.is_empty() {
            0
        } else {
            BRIEF_VOICE_GAP.chars().count()
        };
        let room = cap.saturating_sub(used + gap);
        if room == 0 {
            continue;
        }
        let asked = zerocode_hookd::TurnBriefAsk {
            wall,
            ..ask.clone()
        };
        let Some(line) = voice(&asked, room) else {
            continue;
        };
        let line = line.trim();
        let size = line.chars().count();
        if size == 0 {
            continue;
        }
        debug_assert!(
            size <= room,
            "a brief voice said {size} characters in a room of {room}"
        );
        if size > room {
            continue;
        }
        if gap > 0 {
            brief.push_str(BRIEF_VOICE_GAP);
        }
        brief.push_str(line);
        used += gap + size;
    }
    (!brief.is_empty()).then_some(brief)
}

/// The file pick seat's voice: its likely files, while it acts for a
/// summoned worker's pane, when they fit the room it is handed.
fn file_pick_line(ask: &zerocode_hookd::TurnBriefAsk, room: usize) -> Option<String> {
    if !STANDING.asking().file_pick {
        return None;
    }
    let term = crate::hooks::term_of_pane_key(&ask.pane_key)?;
    brief_for(
        &GUARDS,
        &Wire::of_this_machine(),
        term,
        summoned(term),
        ask,
        room,
    )
}

/// The window's answer to a turn's brief, installed on the bridge.
pub(crate) struct PaneBrief;

impl zerocode_hookd::TurnBrief for PaneBrief {
    fn brief(&self, ask: zerocode_hookd::TurnBriefAsk) -> Option<String> {
        compose_brief(
            BRIEF_CONTRIBUTORS,
            &ask,
            zerocode_hookd::TURN_BRIEF_CHAR_CAP,
        )
    }
}

/// Whether `term`'s pane holds a worker the window summoned, its dispatch
/// still open — read off the board's snapshot of the ledger, which takes no
/// lock the ledger's actor holds and reads no file.
fn summoned(term: u32) -> bool {
    crate::orchestration::ledger_states_by_term()
        .get(&term)
        .is_some_and(|seated| seated.work_ended_ms.is_none())
}

/// The name a prompt goes by between the bridge and the books: the
/// fingerprint of the words the bridge read for a turn's brief
/// ([`zerocode_hookd::turn_prompt`]), which
/// both read off the same envelope.
pub(crate) fn prompt_key(prompt: &str) -> String {
    fingerprint_of(prompt)
}

/// What a turn's start says of the file pick seat in `term`'s pane: the
/// likely files its one question selected, while the seat acts for a
/// summoned worker's pane (`worker`) — waited for within the ask's wall and
/// the seat's own, whichever is shorter. Nothing for the person's panes, a
/// seat that records, an answer that abstained, came late or was refused.
pub(crate) fn brief_for(
    guards: &'static Mutex<Guards>,
    wire: &Wire,
    term: u32,
    worker: bool,
    ask: &zerocode_hookd::TurnBriefAsk,
    room: usize,
) -> Option<String> {
    if !worker {
        return None;
    }
    let wall = ask.wall.min(deadline_of(&FILE_PICK));
    let until = Instant::now() + wall;
    let root = Path::new(&ask.worktree);
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !systemone::applies_in_project(wire, &FILE_PICK, &root) {
        return None;
    }
    let key = prompt_key(&ask.prompt);
    loop {
        {
            let mut held = guards.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(briefing) = held
                .panes
                .get_mut(&term)
                .and_then(|book| book.briefing.as_mut())
                .filter(|briefing| briefing.key == key)
            {
                briefing.until.get_or_insert(until);
                match &briefing.said {
                    // A pick that does not fit the room the voices before it
                    // left is not said — and is not recorded as said.
                    Said::Ready(line) if line.chars().count() > room => {
                        briefing.said = Said::Nothing;
                        return None;
                    }
                    Said::Ready(line) => {
                        let line = line.clone();
                        briefing.said = Said::Taken;
                        return Some(line);
                    }
                    Said::Taken | Said::Nothing => return None,
                    Said::Asking => {}
                }
            }
        }
        if Instant::now() >= until {
            return None;
        }
        std::thread::sleep(BRIEF_POLL);
    }
}

/// Offer `line` — the turn's file pick, in the seat's words — to the brief
/// waiting on the pick `judged` names in `pane`, and say whether it took it
/// before giving up. Nothing to offer, or no brief waiting by the time the
/// answer is in, and nothing is said.
fn offer_brief(
    guards: &'static Mutex<Guards>,
    pane: &Pane,
    judged: u64,
    line: Option<String>,
) -> bool {
    let until = {
        let mut held = guards.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(briefing) = briefing_of(&mut held, pane.term, judged) else {
            return false;
        };
        match (line, briefing.until) {
            (Some(line), Some(until)) if briefing.said == Said::Asking => {
                briefing.said = Said::Ready(line);
                until
            }
            _ => {
                briefing.said = Said::Nothing;
                return false;
            }
        }
    };
    loop {
        std::thread::sleep(BRIEF_POLL);
        let mut held = guards.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(briefing) = briefing_of(&mut held, pane.term, judged) else {
            return false;
        };
        if briefing.said == Said::Taken {
            return true;
        }
        if Instant::now() >= until {
            briefing.said = Said::Nothing;
            return false;
        }
    }
}

/// The briefing of `term`'s pane that waits on the pick `judged` names.
fn briefing_of(held: &mut Guards, term: u32, judged: u64) -> Option<&mut Briefing> {
    held.panes
        .get_mut(&term)
        .and_then(|book| book.briefing.as_mut())
        .filter(|briefing| briefing.judged == judged)
}

/* ---- the questions --------------------------------------------------------------- */

/// Ask one question and file what came of it.
fn ask(guards: &'static Mutex<Guards>, wire: &Wire, pane: &Pane, question: Question) {
    match question {
        Question::Claim {
            judged,
            said,
            evidence,
        } => ask_claim(guards, wire, pane, judged, said, &evidence),
        Question::FilePick {
            judged,
            attempt,
            request,
            edited,
        } => ask_file_pick(guards, wire, pane, judged, attempt, request, &edited),
        guarded => ask_guard(guards, wire, pane, guarded),
    }
}

/// Ask one guard's question and file what came of it: the row, and the
/// verdict in the book — whose label, when its hindsight is already in, is
/// written now.
fn ask_guard(guards: &'static Mutex<Guards>, wire: &Wire, pane: &Pane, question: Question) {
    let (guard, judged) = match &question {
        Question::Command { judged, .. } => (&COMMAND, *judged),
        Question::Text { judged, .. } => (&TEXT, *judged),
        Question::Claim { .. } | Question::FilePick { .. } => return,
    };
    let (mode, _) = systemone::standing_in(wire, guard.seat, Run::Fresh);
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
            let asked = put_nouls(
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
                from: pane.asker(),
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
            let asked = put_nouls(
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
                from: pane.asker(),
                pane: pane.place(),
                asked,
            };
            (serde_json::to_value(row).ok(), verdict, confidence)
        }
        Question::Claim { .. } | Question::FilePick { .. } => return,
    };
    if let Some(row) = row {
        systemone::record_rows(guard.seat, &ledger, &[row], now_ms);
    }
    settle(guards, wire, pane, judged, verdict, confidence);
}

/// Ask a turn's claims — the claims its answer's last paragraph makes, over
/// the turn's finished calls ([`claim::scan`]) — and file what came of them:
/// the row in the ledger zo keeps for the pane's folder, and the verdict in
/// the book. A turn whose answer claims nothing is asked nothing.
fn ask_claim(
    guards: &'static Mutex<Guards>,
    wire: &Wire,
    pane: &Pane,
    judged: u64,
    said: SaidAt,
    evidence: &[Evidence<String>],
) {
    let mode = CLAIM.mode_in_run(&wire.settings_root(), Run::Fresh);
    let root = pane.root();
    let Some(ledger) = systemone::project_ledger_of(wire, &CLAIM, &root).filter(|_| mode.asks())
    else {
        settle_claim(guards, wire, pane, judged, None);
        return;
    };
    let words = match said {
        SaidAt::Words(words) => Some(words),
        SaidAt::Transcript(path) => zerocode_core::transcript::last_assistant_words(&path)
            .map(|words| zerocode_core::clone::scrub_credentials(&words)),
    };
    let claims = words.map_or_else(Vec::new, |words| claim::scan(evidence, &words));
    if claims.is_empty() {
        settle_claim(guards, wire, pane, judged, None);
        return;
    }
    let now_ms = crate::usage_runtime::epoch_ms_now();
    let mut row = ClaimCheckRow {
        at: u64::try_from(now_ms).unwrap_or_default(),
        judged,
        session: fingerprint_of(&pane.owner()),
        rubric_version: claim::CLAIM_RUBRIC_VERSION,
        claims: claims.len(),
        code_settled: claims
            .iter()
            .filter(|claim| claim.code != CodeVerdict::NeedsReading)
            .count(),
        outcome: CONTROL.to_string(),
        verdict: claim::UNAVAILABLE.to_string(),
        answers: BTreeMap::new(),
        route_use: ROUTE_USE_FALLBACK.to_string(),
        applied: false,
        elapsed_ms: 0,
        requests: 0,
        redacted_lines: 0,
        model: None,
        input_tokens: None,
        request_digest: None,
        confidence: None,
        from: pane.asker(),
        pane: pane.place(),
        cached: false,
    };
    let mut verdicts = claim::code_verdicts(&claims);
    let questions = claim::questions(&claims, choice::question);
    if !questions.is_empty() {
        let (trip, answers) = put(
            wire,
            &CLAIM,
            &root,
            claim::state(&claims),
            &questions,
            |answers| {
                claim::read_choices(answers, questions.keys()).ok_or_else(|| SCHEMA.to_string())
            },
        );
        row.outcome = trip.outcome;
        row.elapsed_ms = trip.elapsed_ms;
        row.requests = trip.requests;
        row.redacted_lines = trip.redacted_lines;
        row.model = trip.model;
        row.input_tokens = trip.input_tokens;
        row.cached = trip.cached;
        if let Some(answers) = answers {
            row.confidence = answers
                .values()
                .map(|answer| answer.confidence)
                .min_by(f64::total_cmp);
            for (id, answer) in answers {
                verdicts.push(claim::answered_verdict(&answer.word));
                row.answers.insert(id, answer.word);
            }
        }
    }
    row.verdict = claim::turn_verdict(&verdicts, claims.len()).to_string();
    row.route_use = if row.outcome == ANSWERED_OUTCOME || row.outcome == CONTROL {
        mode.key()
    } else {
        ROUTE_USE_FALLBACK
    }
    .to_string();
    let compared = claim::compared(&row.outcome, u64::try_from(row.code_settled).ok());
    let (verdict, confidence) = (row.verdict.clone(), row.confidence);
    if let Ok(value) = serde_json::to_value(&row) {
        systemone::record_rows(&CLAIM, &ledger, &[value], now_ms);
    }
    settle_claim(
        guards,
        wire,
        pane,
        judged,
        Some((verdict, confidence, compared, ledger)),
    );
}

/// Ask a turn's file pick: the files the window's own search finds for the
/// request's words, beside the files this session's edits wrote, put to the
/// seat in one batch ([`file_pick::questions`]), and file what came of it —
/// the row in the ledger zo keeps for the pane's folder, and, when it was
/// answered, the request its turn's edits will grade. In a summoned worker's
/// pane, while the seat acts for its project, the files it selected are
/// offered to the turn's brief.
fn ask_file_pick(
    guards: &'static Mutex<Guards>,
    wire: &Wire,
    pane: &Pane,
    judged: u64,
    attempt: String,
    request: String,
    edited: &[String],
) {
    let mode = FILE_PICK.mode_in_run(&wire.settings_root(), Run::Fresh);
    let root = pane.root();
    let Some(ledger) =
        systemone::project_ledger_of(wire, &FILE_PICK, &root).filter(|_| mode.asks())
    else {
        offer_brief(guards, pane, judged, None);
        settle_pick(guards, wire, pane, judged, None);
        return;
    };
    let (batch, candidate_elapsed_ms) = candidates(&root, &request, edited);
    let (mut row, selected) = put_pick(
        wire,
        pane,
        mode,
        &PickAsk {
            judged,
            attempt,
            request,
            batch: &batch,
            candidate_elapsed_ms,
        },
    );
    // Said at the turn's start only in a summoned worker's pane, while the
    // seat acts for its project, and only when the brief took it in time.
    let acting = pane.worker && systemone::applies_in_project(wire, &FILE_PICK, &root);
    let line = selected
        .filter(|_| acting)
        .and_then(|selected| file_pick::hint(&selected))
        .map(|hint| hint.text);
    row.applied = offer_brief(guards, pane, judged, line);
    row.noted = row.applied;
    file_pick_row(guards, wire, pane, judged, &row, (root, ledger));
}

/// A pick to put to the seat: its name and turn, the words, and the
/// candidates found for them.
struct PickAsk<'a> {
    judged: u64,
    attempt: String,
    request: String,
    batch: &'a CandidateBatch,
    candidate_elapsed_ms: u64,
}

/// The candidates for `request` in `root` — the window's own search for its
/// words beside the files `edited` (this session's, newest first) that are
/// still there — and how long finding them took.
fn candidates(root: &Path, request: &str, edited: &[String]) -> (CandidateBatch, u64) {
    let began = Instant::now();
    let search = searched(root, &file_pick::search_terms(request));
    let recent: Vec<String> = edited
        .iter()
        .filter_map(|path| file_pick::workspace_relative_path(root, Path::new(path)))
        .filter(|path| root.join(path).is_file())
        .collect();
    let batch = CandidateBatch {
        files: file_pick::interleave(root, &[&search, &recent]),
        recent_paths: recent,
        search_candidates: search.len(),
        graph_candidates: 0,
    };
    (
        batch,
        u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
    )
}

/// Put one pick to the seat, as `mode` reads it: its row, not yet written,
/// and — when the reply read — the files it selected, most likely first.
fn put_pick(
    wire: &Wire,
    pane: &Pane,
    mode: JevMode,
    ask: &PickAsk<'_>,
) -> (FilePickRow, Option<Vec<String>>) {
    let batch = ask.batch;
    let asked = FilePickAsk {
        attempt: ask.attempt.clone(),
        session_id: pane.owner(),
        request: ask.request.clone(),
    };
    let mut row = FilePickRow::of_batch(
        u64::try_from(crate::usage_runtime::epoch_ms_now()).unwrap_or_default(),
        &asked,
        batch,
        ask.candidate_elapsed_ms,
    );
    row.judged = ask.judged;
    row.from = pane.asker();
    row.pane = pane.place();
    let questions = file_pick::questions(&batch.files, noul::question);
    let (trip, answered) = put(
        wire,
        &FILE_PICK,
        &pane.root(),
        file_pick::state(&asked.request, &batch.files),
        &questions,
        |answers| {
            file_pick::read_answers_naming(&batch.files, answers)
                .map(|readings| (answers.clone(), readings))
                .map_err(|(question, refusal)| format!("{question}: {}", refusal.token()))
        },
    );
    row.outcome = trip.outcome;
    row.elapsed_ms = trip.elapsed_ms;
    row.requests = trip.requests;
    row.redacted_lines = trip.redacted_lines;
    row.model = trip.model;
    row.input_tokens = trip.input_tokens;
    row.output_tokens = trip.output_tokens;
    row.cached = trip.cached;
    row.rejected = trip.rejected;
    let selected = answered.map(|(value, readings)| {
        row.answers = Some(file_pick::probabilities(readings));
        row.ranked_paths = file_pick::rank_candidates(&batch.files, &value)
            .iter()
            .map(|path| fingerprint_of(path))
            .collect();
        let selected = file_pick::select_candidates(&batch.files, &value);
        row.selected_paths = selected.iter().map(|path| fingerprint_of(path)).collect();
        row.route_use = mode.key().to_string();
        selected
    });
    (row, selected)
}

/// Write a pick's row in the pane's project ledger (`at`: the folder it was
/// asked from and the ledger), and hand the book what came of it: an
/// answered pick waits on its turn's edits for its label.
fn file_pick_row(
    guards: &'static Mutex<Guards>,
    wire: &Wire,
    pane: &Pane,
    judged: u64,
    row: &FilePickRow,
    at: (PathBuf, PathBuf),
) {
    let Ok(value) = serde_json::to_value(row) else {
        settle_pick(guards, wire, pane, judged, None);
        return;
    };
    let (root, ledger) = at;
    systemone::record_rows(
        &FILE_PICK,
        &ledger,
        std::slice::from_ref(&value),
        crate::usage_runtime::epoch_ms_now(),
    );
    let answered = (row.outcome == ANSWERED_OUTCOME).then_some((value, root, ledger));
    settle_pick(guards, wire, pane, judged, answered);
}

/* ---- zerocode-find (t-14869) ------------------------------------------------------ */

/// The window's answer to `zerocode-find`.
pub(crate) struct PaneFind;

impl zerocode_hookd::FileFind for PaneFind {
    fn find(&self, ask: FindAsk) -> Result<String, String> {
        find_in(&GUARDS, &Wire::of_this_machine(), &ask)
    }
}

/// Answer one `zerocode-find` from `ask`'s pane (t-14869): the candidates a
/// turn's start would find for its words, listed in today's order — or, in a
/// summoned worker's pane while the seat acts for its project, the ones the
/// seat selected. While the seat asks, the pick is put to it as a turn's is
/// and its row names the agent and the moment ([`file_find::ASKED`]); a pick
/// that does not act is asked beside the answer, never before it. The pick
/// waits on its turn's edits for its label.
///
/// # Errors
/// When neither the pane nor the shell says which folder to look in.
pub(crate) fn find_in(
    guards: &'static Mutex<Guards>,
    wire: &Wire,
    ask: &FindAsk,
) -> Result<String, String> {
    let term = ask.pane.as_deref().and_then(crate::hooks::term_of_pane_key);
    let held_pane = {
        let held = guards.lock().unwrap_or_else(PoisonError::into_inner);
        term.and_then(|term| held.panes.get(&term))
            .and_then(|book| Some((book.pane.clone()?, book.edited.clone())))
    };
    let root = match (&held_pane, &ask.cwd) {
        (Some((pane, _)), _) => pane.root(),
        (None, Some(cwd)) => Path::new(cwd)
            .canonicalize()
            .map_err(|error| error.to_string())?,
        (None, None) => return Err(file_find::USAGE.to_string()),
    };
    let edited = held_pane
        .as_ref()
        .map(|(_, edited)| edited.clone())
        .unwrap_or_default();
    let (batch, candidate_elapsed_ms) = candidates(&root, &ask.request, &edited);
    let today: Vec<FilePickCandidate> = batch
        .files
        .iter()
        .take(FILE_PICK_HINT_FILE_CAP)
        .cloned()
        .collect();
    let mode = FILE_PICK.mode_in_run(&wire.settings_root(), Run::Fresh);
    let Some((pane, ledger)) = held_pane.map(|(pane, _)| pane).and_then(|pane| {
        let ledger = systemone::project_ledger_of(wire, &FILE_PICK, &root)
            .filter(|_| mode.asks() && !batch.files.is_empty())?;
        Some((pane, ledger))
    }) else {
        return Ok(file_find::listing(&today));
    };
    let Some((judged, attempt)) = shelve_find(guards, &pane) else {
        return Ok(file_find::listing(&today));
    };
    let acting = pane.worker && systemone::applies_in_project(wire, &FILE_PICK, &root);
    let pick = PickAsk {
        judged,
        attempt,
        request: ask.request.clone(),
        batch: &batch,
        candidate_elapsed_ms,
    };
    if !acting {
        let wire = wire.clone();
        let files = batch.clone();
        let request = pick.request.clone();
        let attempt = pick.attempt.clone();
        drop(
            std::thread::Builder::new()
                .name("jev-file-find".to_string())
                .spawn(move || {
                    let pick = PickAsk {
                        judged,
                        attempt,
                        request,
                        batch: &files,
                        candidate_elapsed_ms,
                    };
                    let (mut row, _) = put_pick(&wire, &pane, mode, &pick);
                    row.moment = Some(file_find::ASKED.to_string());
                    file_pick_row(guards, &wire, &pane, judged, &row, (root, ledger));
                }),
        );
        return Ok(file_find::listing(&today));
    }
    let (mut row, selected) = put_pick(wire, &pane, mode, &pick);
    row.moment = Some(file_find::ASKED.to_string());
    let chosen: Vec<FilePickCandidate> = selected
        .unwrap_or_default()
        .iter()
        .filter_map(|path| batch.files.iter().find(|file| file.path == *path).cloned())
        .collect();
    row.applied = !chosen.is_empty();
    row.noted = row.applied;
    file_pick_row(guards, wire, &pane, judged, &row, (root, ledger));
    Ok(file_find::listing(if chosen.is_empty() {
        &today
    } else {
        &chosen
    }))
}

/// Shelve a pick asked by `zerocode-find` in `pane`'s book, named apart from
/// its turn's own pick and from every other asked in the turn, to wait on
/// the turn's edits: its name and its turn's.
fn shelve_find(guards: &'static Mutex<Guards>, pane: &Pane) -> Option<(u64, String)> {
    let mut held = guards.lock().unwrap_or_else(PoisonError::into_inner);
    let book = held.panes.get_mut(&pane.term)?;
    book.finds += 1;
    let attempt = book.attempt();
    let judged = task_fingerprint(
        &format!("{attempt}{}{}", file_find::ASKED, book.finds),
        FILE_PICK.id,
    );
    shelve(
        &mut book.picks,
        PickWaiting {
            judged,
            asked: None,
            edited: None,
            looked: None,
        },
    );
    Some((judged, attempt))
}

/// The files the window's own project search finds for `terms` in `root` —
/// each file that names a term, the ones that name more of them first
/// ([`crate::project_search::files_naming`]), at most the seat's candidate
/// cap; under the search's own deadline (`ExplorerPolicy`).
fn searched(root: &Path, terms: &[String]) -> Vec<String> {
    crate::project_search::files_naming(
        root,
        terms,
        FILE_PICK_CANDIDATE_CAP,
        &crate::explorer_policy::ExplorerPolicy::default(),
    )
}

/// What one request's trip through the door came to — the facts every row
/// the window writes for a pane carries, whatever its seat.
#[derive(Debug, Default)]
struct Trip {
    outcome: String,
    elapsed_ms: u64,
    requests: u32,
    redacted_lines: u32,
    model: Option<String>,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    request_bytes: Option<usize>,
    cached: bool,
    rejected: Option<String>,
}

/// Put `questions` over `state`, from words of `workspace`, through the door
/// and its memo: an answer the memo held for these exact bytes answers a
/// repeat with no request (`cached`), and a fresh one is remembered once
/// `read` — the seat's own reader of a reply's answers — takes it; one it
/// refuses is the reply's schema breaking, and `read` says which rule.
fn put<R>(
    wire: &Wire,
    seat: &'static JevUse,
    workspace: &Path,
    state: Value,
    questions: &BTreeMap<String, Value>,
    read: impl FnOnce(&Value) -> Result<R, String>,
) -> (Trip, Option<R>) {
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
        seat,
        Some(workspace),
        body,
        deadline_of(seat),
        memo_file.as_deref().map(|path| Memo {
            path,
            seat,
            applying: true,
        }),
    );
    let cached = answered.memo.as_ref().is_some_and(|memoed| memoed.answered);
    let mut trip = Trip {
        elapsed_ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
        requests: answered.spent.requests,
        redacted_lines: u32::try_from(answered.spent.redacted_lines).unwrap_or(u32::MAX),
        model: answered.spent.model.clone(),
        request_bytes: (answered.request_bytes > 0).then_some(answered.request_bytes),
        cached,
        input_tokens: answered.spent.input_tokens,
        output_tokens: answered.spent.output_tokens,
        ..Trip::default()
    };
    let body = match answered.answer {
        Ok(body) => body,
        Err(token) => {
            trip.outcome = token;
            return (trip, None);
        }
    };
    let parsed: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let answers = parsed.get("answers").cloned().unwrap_or(Value::Null);
    match read(&answers) {
        Ok(read) => {
            trip.outcome = ANSWERED_OUTCOME.to_string();
            if !cached
                && let (Some(path), Some(memoed)) = (memo_file.as_deref(), answered.memo.as_ref())
            {
                let _ = memo::remember(
                    path,
                    seat,
                    &memoed.key,
                    &body,
                    crate::usage_runtime::epoch_ms_now(),
                );
            }
            (trip, Some(read))
        }
        Err(rule) => {
            trip.outcome = SCHEMA.to_string();
            trip.rejected = Some(rule);
            (trip, None)
        }
    }
}

/// A guard's Nouls through [`put`], read the way zo reads the same answer:
/// every Noul, or the rule that broke.
fn put_nouls(
    wire: &Wire,
    guard: &Guard,
    workspace: &Path,
    state: Value,
    questions: &BTreeMap<String, Value>,
    mode: JevMode,
) -> Asked {
    let (trip, answers) = put(wire, guard.seat, workspace, state, questions, |answers| {
        questions
            .keys()
            .map(|id| {
                noul::read(answers, id)
                    .map(|yes| (id.clone(), yes))
                    .map_err(|refusal| refusal.token().to_string())
            })
            .collect::<Result<BTreeMap<String, f64>, String>>()
    });
    Asked {
        outcome: trip.outcome,
        route_use: if answers.is_some() {
            mode.key()
        } else {
            ROUTE_USE_FALLBACK
        }
        .to_string(),
        answers,
        applied: false,
        elapsed_ms: trip.elapsed_ms,
        retries: 0,
        requests: trip.requests,
        redacted_lines: trip.redacted_lines,
        model: trip.model,
        input_tokens: trip.input_tokens,
        request_bytes: trip.request_bytes,
        request_digest: None,
        rejected: trip.rejected,
        cached: trip.cached,
    }
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

/// The claims `judged` names came to `answered` — their verdict, its
/// confidence, whether it was the model's alone, and the ledger their row
/// went to — or to nothing, which takes them out of the book. Labeled now
/// when the person's next prompt is already in.
fn settle_claim(
    guards: &'static Mutex<Guards>,
    wire: &Wire,
    pane: &Pane,
    judged: u64,
    answered: Option<(String, Option<f64>, bool, PathBuf)>,
) {
    let mut labels = Vec::new();
    {
        let mut held = guards.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(book) = held.panes.get_mut(&pane.term) else {
            return;
        };
        let Some(at_entry) = book
            .claims
            .iter()
            .position(|entry| entry.waiting.judged == judged)
        else {
            return;
        };
        let Some((verdict, confidence, compared, ledger)) = answered else {
            book.claims.remove(at_entry);
            return;
        };
        let entry = &mut book.claims[at_entry];
        entry.waiting.verdict = Some(verdict);
        entry.waiting.confidence = confidence;
        entry.waiting.compared = compared;
        entry.ledger = Some(ledger);
        if entry.waiting.failure.is_some() {
            let at = u64::try_from(crate::usage_runtime::epoch_ms_now()).unwrap_or_default();
            let entry = book.claims.remove(at_entry);
            labels.extend(entry.label(at));
        }
    }
    record_labels(wire, labels, crate::usage_runtime::epoch_ms_now());
}

/// The file pick `judged` names came back `answered` — its row, the folder
/// it was asked from and the ledger it went to — or unanswered, which takes
/// it out of the book: only an answered request is graded. Labeled now when
/// its turn's edits are already in.
fn settle_pick(
    guards: &'static Mutex<Guards>,
    wire: &Wire,
    pane: &Pane,
    judged: u64,
    answered: Option<(Value, PathBuf, PathBuf)>,
) {
    let mut labels = Vec::new();
    {
        let mut held = guards.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(book) = held.panes.get_mut(&pane.term) else {
            return;
        };
        let Some(at_entry) = book.picks.iter().position(|pick| pick.judged == judged) else {
            return;
        };
        if answered.is_none() {
            book.picks.remove(at_entry);
            return;
        }
        book.picks[at_entry].asked = answered;
        if book.picks[at_entry].edited.is_some() {
            let at = u64::try_from(crate::usage_runtime::epoch_ms_now()).unwrap_or_default();
            let pick = book.picks.remove(at_entry);
            labels.extend(pick.label(&pane.worktree, at));
        }
    }
    record_labels(wire, labels, crate::usage_runtime::epoch_ms_now());
}

#[cfg(test)]
mod replay;
#[cfg(test)]
mod tests;
