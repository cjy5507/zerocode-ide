//! Every road out of this window, by name (t-6428).
//!
//! Leaving the window cuts what runs in its panes — a worker's turn, the
//! gate it left running in the background — so each road is named where it
//! begins, and the goodbye says which one it was
//! (`orchestration::window_exiting`). Measured before this module existed,
//! over 37 hours of the window's own log (2026-09-22 12:13 – 09-24 01:32):
//! nine exits with workers seated, twenty sleeping panes, and every one of
//! the nine the main window closing — `window destroyed: main`, then tauri's
//! `ExitRequested` without a code — none of them through a restart button.

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// A road out of this window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitRoad {
    /// A restart button (`relaunch_window`), by the door it stands in when
    /// the window named one.
    Restart(Option<RestartDoor>),
    /// The last window closed — its red button, ⌘W. tauri says so only
    /// once the window is gone (`ExitRequested` without a code).
    Close,
    /// `terminate:` — ⌘Q, the Dock's Quit, a logout or a shutdown. The app
    /// hears it only as it ends (`RunEvent::Exit`), so nothing can ask first.
    Terminate,
    /// The menu-bar icon's Quit.
    Tray,
    /// The window's own `app.exit(code)`, with no road named before it.
    App,
}

/// The four restart buttons: the lane's 「새 빌드 준비됨」 toast (t-3005),
/// the update feed's install (t-3191), the settings notice beside them, and
/// the window material's relaunch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RestartDoor {
    UpdateToast,
    UpdateInstall,
    SettingsNotice,
    WindowMaterial,
}

/// Each door's word, in one table: the window names its door with it, and
/// the goodbye's line says it back.
const DOORS: [(RestartDoor, &str); 4] = [
    (RestartDoor::UpdateToast, "update-toast"),
    (RestartDoor::UpdateInstall, "update-install"),
    (RestartDoor::SettingsNotice, "settings-notice"),
    (RestartDoor::WindowMaterial, "window-material"),
];

impl RestartDoor {
    /// The door a word names, or `None` for a word no door wears.
    pub(crate) fn named(word: &str) -> Option<Self> {
        DOORS
            .iter()
            .find(|(_, held)| *held == word)
            .map(|(door, _)| *door)
    }

    pub(crate) fn word(self) -> &'static str {
        DOORS
            .iter()
            .find(|(door, _)| *door == self)
            .map_or("", |(_, word)| word)
    }
}

impl fmt::Display for ExitRoad {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Restart(Some(door)) => write!(out, "restart:{}", door.word()),
            Self::Restart(None) => out.write_str("restart"),
            Self::Close => out.write_str("close"),
            Self::Terminate => out.write_str("terminate"),
            Self::Tray => out.write_str("tray"),
            Self::App => out.write_str("app"),
        }
    }
}

/// The two roads that ask before they go (t-6428): a restart button, and
/// the main window's close. The others cannot ask — `terminate:` is heard
/// only as the app ends — or are the window's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Asking {
    Restart(Option<RestartDoor>),
    Close,
}

impl Asking {
    /// A road by the window's word for it — `restart`, with the door it
    /// stands in, or `close`.
    pub(crate) fn named(road: &str, door: Option<&str>) -> Option<Self> {
        match road {
            "restart" => Some(Self::Restart(door.and_then(RestartDoor::named))),
            "close" => Some(Self::Close),
            _ => None,
        }
    }

    pub(crate) fn patience(self) -> Patience {
        match self {
            Self::Restart(_) => RESTART_PATIENCE,
            Self::Close => CLOSE_PATIENCE,
        }
    }

    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Restart(_) => "restart",
            Self::Close => "close",
        }
    }
}

/// A second, in the table's milliseconds.
pub(crate) const SECOND_MS: i64 = 1_000;

/// A minute, in the table's milliseconds.
pub(crate) const MINUTE_MS: i64 = 60_000;

/// How long a road that asks will wait, in one table (t-6428).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Patience {
    /// The longest 「끝나면」 waits for the first gap: nothing running under
    /// any worker's pane and nothing the window could not read.
    pub(crate) gap_wait_ms: i64,
    /// How long the question stands unanswered before the road goes anyway;
    /// `None` stands until someone answers.
    pub(crate) answer_ms: Option<i64>,
    /// Whether a wait that runs out leaves anyway — the person asked to go —
    /// or asks again.
    pub(crate) overdue_leaves: bool,
    /// How long the road's notice stands on the screen before it goes, once
    /// it has been decided (t-67936) — by a press, or by a wait that found
    /// its gap; `None` goes at once.
    pub(crate) notice_ms: Option<i64>,
    /// How long the person's hands must have rested before a standing notice
    /// may end (t-67936): a key they pressed inside it holds the road until
    /// they have been still this long.
    pub(crate) rest_ms: i64,
}

/// A restart waits half an hour for the first gap, then asks again. Measured
/// over the workers' own transcripts (09-16 – 09-24, 780 background
/// commands): half end within 3.2 minutes, nine in ten within 14.7, and 97 %
/// within 30 — a turn is not waited out (half run past 56 minutes), only
/// what runs under it.
///
/// And it never goes unseen (t-67936). On 2026-10-21 the window restarted
/// twice by this road (05:05 and 12:37, each a clean exit), and the person
/// saw both as 「갑자기 꺼짐」: a wait goes at a gap the census finds, which
/// can come half an hour after the press, in the middle of a sentence. So
/// the road shows its notice ten seconds first — long enough to read one
/// line and reach its 취소, short enough that a restart somebody pressed is
/// still a restart — and a key pressed inside it holds the road until the
/// hands have rested five seconds: five of the window's typing-run gaps
/// (`TYPING_RUN_GAP_MS`, the longest pause inside a word), so a person who
/// stopped that long is reading, not mid-word.
const RESTART_PATIENCE: Patience = Patience {
    gap_wait_ms: 30 * MINUTE_MS,
    answer_ms: None,
    overdue_leaves: false,
    notice_ms: Some(10 * SECOND_MS),
    rest_ms: 5 * SECOND_MS,
};

/// A close waits ten minutes (82 % of those commands end within it) and then
/// goes, because closing is the person leaving; and its question stands one
/// minute, so a close nobody is there to answer still closes. It shows no
/// notice: the person pressed the close of the window they are looking at.
const CLOSE_PATIENCE: Patience = Patience {
    gap_wait_ms: 10 * MINUTE_MS,
    answer_ms: Some(MINUTE_MS),
    overdue_leaves: true,
    notice_ms: None,
    rest_ms: 0,
};

/// What the person chose when asked, said back in the goodbye's line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Choice {
    /// Nothing asked: nothing was busy, or the road asks nobody.
    #[default]
    Unasked,
    /// 「지금」: leave now, whatever runs.
    Now,
    /// 「끝나면」, and the first gap came.
    Gap,
    /// 「끝나면」, and the wait ran out on a road that leaves anyway.
    Overdue,
    /// The question stood unanswered for its whole time.
    Unanswered,
    /// The census did not answer within [`LEDGER_PATIENCE`] (t-49062): a way
    /// out the person took went on without asking.
    Unread,
}

/// Each choice's word in the goodbye's line, in one table.
const CHOICES: [(Choice, &str); 6] = [
    (Choice::Unasked, "unasked"),
    (Choice::Now, "now"),
    (Choice::Gap, "gap"),
    (Choice::Overdue, "overdue"),
    (Choice::Unanswered, "unanswered"),
    (Choice::Unread, "unread"),
];

impl Choice {
    pub(crate) fn word(self) -> &'static str {
        CHOICES
            .iter()
            .find(|(held, _)| *held == self)
            .map_or("", |(_, word)| word)
    }
}

/// A 「끝나면」 armed: which road, since when, and what its line last said
/// (commands running, workers unread, whole minutes waited).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wait {
    asking: Asking,
    since_ms: i64,
    told: Option<(usize, usize, i64)>,
    /// Whether the log has said this wait's census went unanswered
    /// (t-49062): said once, until a census answers it again.
    unread_said: bool,
}

/// The line a wait stands on the screen as: what still runs, what nobody
/// could read, how many whole minutes it has waited and the most it will.
/// Said again when any of them moves — at most once a minute while nothing
/// else does — so the window keeps no clock of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WaitLine {
    pub(crate) road: &'static str,
    pub(crate) running: usize,
    pub(crate) unknown: usize,
    pub(crate) waited_min: i64,
    pub(crate) wait_min: i64,
}

impl WaitLine {
    /// The line for a wait on `asking` that has waited `waited_ms` with
    /// this census.
    pub(crate) fn of(
        asking: Asking,
        busy: crate::orchestration::restart_census::Busy,
        waited_ms: i64,
    ) -> Self {
        Self {
            road: asking.word(),
            running: busy.running,
            unknown: busy.unknown,
            waited_min: waited_ms / MINUTE_MS,
            wait_min: asking.patience().gap_wait_ms / MINUTE_MS,
        }
    }
}

/// A road decided and not gone yet (t-67936): its notice stands on the
/// screen — which road, what the person chose, when it was raised and the
/// earliest it may go, and what its line last said (whole seconds left, and
/// whether the person's typing holds it).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Notice {
    asking: Asking,
    choice: Choice,
    raised_ms: i64,
    due_ms: i64,
    told: Option<(i64, bool)>,
}

/// The line a notice stands on the screen as (t-67936): its road and door,
/// the whole seconds left, and whether the person's typing is holding it.
/// Said again when either moves, so the window keeps no clock of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NoticeLine {
    pub(crate) road: &'static str,
    pub(crate) door: Option<&'static str>,
    pub(crate) seconds: i64,
    pub(crate) held: bool,
}

impl NoticeLine {
    fn of(asking: Asking, seconds: i64, held: bool) -> Self {
        Self {
            road: asking.word(),
            door: match asking {
                Asking::Restart(door) => door.map(RestartDoor::word),
                Asking::Close => None,
            },
            seconds,
            held,
        }
    }
}

/// How a notice ended (t-67936), for the door's line: how long it stood,
/// and how much of that the person's typing held it past its time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Went {
    stood_ms: i64,
    held_ms: i64,
}

/// What the beat does about the way out, this second.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExitBeat {
    Nothing,
    /// The wait's line moved.
    Tell(WaitLine),
    /// The notice's line moved — or it was just raised (t-67936).
    Notice(NoticeLine),
    /// The wait ran out on a road that asks again.
    AskAgain(Asking),
    /// Go, by this road.
    Leave(Asking),
    /// The census did not answer the wait on this road within
    /// [`LEDGER_PATIENCE`] (t-49062): unknown counts as busy, so the wait
    /// stands — said once, until a census answers it again.
    Unread(Asking),
}

/// The window's way out, as far as it has got.
#[derive(Debug, Default)]
struct Exiting {
    road: Option<ExitRoad>,
    /// A question standing, and since when.
    question: Option<(Asking, i64)>,
    wait: Option<Wait>,
    choice: Choice,
    /// The person chose to go, the road went for them, or the census found
    /// nothing a close would cut: the next close passes without a question.
    confirmed: bool,
    /// A close's census being read off the close event (t-49062): the close
    /// is held until it answers, and a second press waits for the same
    /// answer rather than reading it twice.
    reading: bool,
    /// A road decided and showing its notice (t-67936).
    notice: Option<Notice>,
    /// When the person's hands last moved while the notice stood, as the
    /// window saw them (t-67936).
    hand_ms: Option<i64>,
    /// Whose hand pressed or armed a road, in the window's own word
    /// (`person click`, `person keydown Enter +40ms`, `synthetic click`),
    /// and the road it was said for — said in the door's line (t-67936).
    by: Option<(Asking, String)>,
    /// How the last notice ended, said in the door's line (t-67936).
    went: Option<Went>,
    /// Whether the door's line has been said: a road heard twice — a close,
    /// then the browser's own `app.exit(0)` finishing it — says it once.
    door_told: bool,
}

static EXITING: Mutex<Exiting> = Mutex::new(Exiting {
    road: None,
    question: None,
    wait: None,
    choice: Choice::Unasked,
    confirmed: false,
    reading: false,
    notice: None,
    hand_ms: None,
    by: None,
    went: None,
    door_told: false,
});

/// A wait's census still out (t-49062): the beat does not ask the actor a
/// second time over a reading it has not answered.
static WAIT_CENSUS_OUT: AtomicBool = AtomicBool::new(false);

fn exiting() -> std::sync::MutexGuard<'static, Exiting> {
    lock(&EXITING)
}

fn lock(cell: &Mutex<Exiting>) -> std::sync::MutexGuard<'_, Exiting> {
    cell.lock().unwrap_or_else(|held| held.into_inner())
}

/// Name the road this window is leaving by, and answer the road it is.
///
/// The first road named is the road. A close is followed by the window's
/// own `app.exit(0)` once the embedded browser has shut down, and then by
/// tauri's `Exit`: all three reach the goodbye, and the person closed the
/// window.
pub(crate) fn begin(road: ExitRoad) -> ExitRoad {
    first(&mut exiting().road, road)
}

fn first(held: &mut Option<ExitRoad>, road: ExitRoad) -> ExitRoad {
    *held.get_or_insert(road)
}

/// Whether this window is on its way out — a road has been named (t-7812 D).
///
/// From that moment the pane layouts on disk are the list the next window
/// restores: what the panes do on their way out — a shell ending, a tab
/// pruned around it — describes the window dying, not the one coming back.
pub(crate) fn leaving() -> bool {
    exiting().road.is_some()
}

/// What `terminate:` must get done before the process is gone, one step
/// each, so the order is a thing a test can hold (t-7812 D1).
pub(crate) trait Terminating {
    /// The ledger's goodbye: seated workers sleep with their dispatches
    /// open, so a pane that exits after this settles nothing.
    fn goodbye(&self, road: ExitRoad);
    /// Every remembered leaf's screen, into the layouts file.
    fn keep_screens(&self);
    /// The last statuses, flushed past their debounce.
    fn keep_statuses(&self);
    /// Every pane ended, here, rather than whenever the process happens to.
    fn end_panes(&self);
    /// The embedded browser's own shutdown.
    fn shut_browser(&self);
    /// The crash watchdog's word that this exit was a clean one.
    fn mark_clean_exit(&self);
}

/// `terminate:` — ⌘Q, the Dock's Quit, a logout — in the order that keeps
/// the next window's list (t-7812 D1).
///
/// The goodbye first; then the screens and statuses the next window reopens
/// from; then the panes; and only then the embedded browser, the one step
/// measured to stall. On 2026-09-25 01:02 a ⌘Q spent twenty seconds inside
/// the browser's shutdown (the crash record's `main_scope=event_exit`), the
/// panes ended in that time, and the ledger — whose goodbye stood behind the
/// browser — wrote three of them down as dead workers. The clean-exit mark
/// is last: a quit that never got that far was not a clean one.
pub(crate) fn terminate(steps: &dyn Terminating) {
    leave_in_order(begin(ExitRoad::Terminate), steps);
}

/// A restart button's way out (`relaunch_window`), which tauri follows with
/// no exit event at all — tauri's restart replaces the process — so what the
/// next window restores from is written here or nowhere (t-7812 D): the
/// goodbye, then the screens and the statuses. The panes and the browser end
/// with the process it replaces.
pub(crate) fn relaunch(road: ExitRoad, steps: &dyn Terminating) {
    steps.goodbye(road);
    steps.keep_screens();
    steps.keep_statuses();
}

/// [`terminate`]'s order, for the road already named.
fn leave_in_order(road: ExitRoad, steps: &dyn Terminating) {
    steps.goodbye(road);
    steps.keep_screens();
    steps.keep_statuses();
    steps.end_panes();
    steps.shut_browser();
    steps.mark_clean_exit();
}

/// What the person chose on the way out, for the goodbye's line.
pub(crate) fn choice() -> Choice {
    exiting().choice
}

/// A question stands on this road, from now: the close's minute runs — in
/// place of any wait or notice another road had going.
fn asked(state: &mut Exiting, asking: Asking, now_ms: i64) {
    state.question = Some((asking, now_ms));
    state.wait = None;
    state.notice = None;
}

/// 「끝나면」: wait on this road for the first gap, from now — armed by
/// `by`, the window's word for the hand that pressed it (t-67936).
pub(crate) fn arm(asking: Asking, now_ms: i64, by: Option<&str>) {
    let mut state = exiting();
    state.question = None;
    state.notice = None;
    state.by = Some((asking, crate::cmd::board::window_word(by)));
    state.wait = Some(Wait {
        asking,
        since_ms: now_ms,
        told: None,
        unread_said: false,
    });
}

/// 「지금」 — or a door that asked nothing, because nothing it would cut was
/// running (`Choice::Unasked`): this road goes, pressed by `by`. A road that
/// shows a notice raises it here and answers its first line (t-67936); one
/// that shows none is settled, and `None` tells the caller to go now.
pub(crate) fn leave_now(
    asking: Asking,
    choice: Choice,
    by: Option<&str>,
    now_ms: i64,
) -> Option<NoticeLine> {
    let mut state = exiting();
    state.by = Some((asking, crate::cmd::board::window_word(by)));
    match go(&mut state, asking, choice, now_ms) {
        ExitBeat::Notice(line) => Some(line),
        _ => None,
    }
}

/// 「취소」: no question stands, nothing waits and no notice runs.
pub(crate) fn cancel() {
    let mut state = exiting();
    state.question = None;
    state.wait = None;
    state.notice = None;
    state.hand_ms = None;
    state.by = None;
}

/// The person's hands moved at `at_ms`, as the window saw them (t-67936): a
/// standing notice holds its road until they have rested.
pub(crate) fn hands_moved(at_ms: i64) {
    hands_at(&mut exiting(), at_ms);
}

/// [`hands_moved`], on the way out `state` holds. With no notice standing
/// there is nothing to hold, and nothing is kept.
fn hands_at(state: &mut Exiting, at_ms: i64) {
    if state.notice.is_some() {
        state.hand_ms = Some(state.hand_ms.map_or(at_ms, |held| held.max(at_ms)));
    }
}

/// Every road out's own line in the window's log, said once per exit
/// (t-67936): the road taken, whose hand moved it, what the person chose,
/// and for a restart how long its notice stood and their typing held it — or
/// that it went with no notice at all, the line a restart nobody saw coming
/// would leave. `None` once it has been said.
pub(crate) fn door_line(road: ExitRoad) -> Option<String> {
    let mut state = exiting();
    if std::mem::replace(&mut state.door_told, true) {
        return None;
    }
    Some(door_said(&state, road))
}

/// The hand a road names when the window said none for it: the restart's is
/// always said, so a silent one is unsaid; the others are heard from outside
/// the page — the window's close, a quit, the tray's quit, the window's own
/// exit — and say which.
const ROAD_HANDS: [(ExitRoad, &str); 4] = [
    (ExitRoad::Close, "window close"),
    (ExitRoad::Terminate, "system quit"),
    (ExitRoad::Tray, "tray quit"),
    (ExitRoad::App, "window exit"),
];

/// [`door_line`], of the way out `state` holds.
fn door_said(state: &Exiting, road: ExitRoad) -> String {
    let said = state.by.as_ref().filter(|(asking, by)| {
        !by.is_empty()
            && match road {
                ExitRoad::Restart(_) => matches!(asking, Asking::Restart(_)),
                ExitRoad::Close => *asking == Asking::Close,
                _ => false,
            }
    });
    let by = said.map_or_else(
        || {
            ROAD_HANDS
                .iter()
                .find(|(held, _)| *held == road)
                .map_or("unsaid", |(_, hand)| *hand)
        },
        |(_, by)| by.as_str(),
    );
    let notice = match (road, state.went) {
        (_, Some(went)) => format!(
            " · notice {} s · held {} s",
            went.stood_ms / SECOND_MS,
            went.held_ms / SECOND_MS
        ),
        (ExitRoad::Restart(_), None) => " · no notice".to_string(),
        _ => String::new(),
    };
    format!(
        "exit door {road} · by {by} · choice {}{notice}",
        state.choice.word()
    )
}

/// The note a restart leaves for the window it starts (t-67936): the door it
/// went by, whose hand, and the build it left — so the next window says once
/// which build it came back as, and why it went. A restart the person sees
/// coming and then sees land is not 「갑자기 꺼짐」.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestartNote {
    pub(crate) door: Option<String>,
    pub(crate) by: Option<String>,
    pub(crate) from_version: String,
    pub(crate) from_commit: String,
    pub(crate) at_ms: i64,
}

/// Where the note waits, under the window's own data root.
const RESTART_NOTE_FILE: &str = "restart-note.json";

/// How old a note may be and still be the restart that started this window.
/// A restart is back up in seconds — the slowest start this machine has
/// logged took half a minute (2026-10-01, t-20078) — so a note ten minutes
/// old was left by a restart that never came back, and says nothing of this
/// start.
const RESTART_NOTE_FRESH_MS: i64 = 10 * MINUTE_MS;

/// The note this road leaves (t-67936): its door, whose hand, and this build.
pub(crate) fn restart_note_of(
    road: ExitRoad,
    stamp: &crate::update_runtime::BuildStamp,
    now_ms: i64,
) -> RestartNote {
    RestartNote {
        door: match road {
            ExitRoad::Restart(door) => door.map(|door| door.word().to_string()),
            _ => None,
        },
        by: exiting().by.as_ref().map(|(_, by)| by.clone()),
        from_version: stamp.version.clone(),
        from_commit: stamp.commit.clone(),
        at_ms: now_ms,
    }
}

/// Leave the note for the next window, on the restart road right before the
/// process is replaced (t-67936).
pub(crate) fn leave_restart_note(root: &Path, note: &RestartNote) -> Result<(), String> {
    let bytes = serde_json::to_vec(note).map_err(|error| error.to_string())?;
    crate::durable_file::replace_bytes(&root.join(RESTART_NOTE_FILE), &bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The note the last window left, taken (t-67936): read once and removed
/// whatever it says, and answered only when it is fresh.
pub(crate) fn take_restart_note(root: &Path, now_ms: i64) -> Option<RestartNote> {
    let path = root.join(RESTART_NOTE_FILE);
    let bytes = crate::durable_file::read_plain_file(&path).ok()?;
    let _ = crate::durable_file::remove_file(&path);
    let note: RestartNote = serde_json::from_slice(&bytes).ok()?;
    (now_ms.saturating_sub(note.at_ms) <= RESTART_NOTE_FRESH_MS).then_some(note)
}

/// How long a way out waits on the ledger's actor before it goes on without
/// it (t-49062): the close's census, a wait's census on the beat, and the
/// goodbye (`orchestration::goodbye_within`). Over the eleven goodbyes the
/// window's log held on 2026-10-12 the census answered in 46 ms at the median
/// and 313 ms at the slowest; twice that day it never answered, and the
/// window stood until it was started again 20 and 34 s later. 1.5 s is about
/// five times the slowest answer seen, and stays under the hang watchdog's
/// first threshold (2 s, `crash::Limits::DEFAULT`), so a goodbye that waits it
/// out on the main thread is never filed as a hang.
pub(crate) const LEDGER_PATIENCE: Duration = Duration::from_millis(1_500);

/// A census read on a thread named `thread`, and its answer if it came within
/// [`LEDGER_PATIENCE`] (t-49062). `None` is a census that did not answer — it
/// may still stand in the actor's mailbox, and its answer, if it ever comes,
/// goes nowhere — or one that panicked.
pub(crate) fn census_within(
    thread: &str,
    read: impl FnOnce() -> crate::orchestration::restart_census::Busy + Send + 'static,
) -> Option<crate::orchestration::restart_census::Busy> {
    let (answer, answered) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name(thread.to_string())
        .spawn(move || {
            let _ = answer.send(read());
        })
        .ok()?;
    answered.recv_timeout(LEDGER_PATIENCE).ok()
}

/// What a press of the main window's close needs from the window: who a
/// close would cut, and the two ways that census answers — one door each,
/// so where each one runs is a thing a test can hold, as [`Terminating`]
/// holds the order of the way out.
pub(crate) trait Closing: Send + Sync + 'static {
    /// Who leaving would cut. It asks the ledger's actor, whose mailbox
    /// holds four commands and waits when full, and runs the host's `ps` —
    /// so it is read on a thread of its own, never in the close event.
    fn census(&self) -> crate::orchestration::restart_census::Busy;
    /// Work in progress: the close's question goes to the person.
    fn ask(&self, busy: crate::orchestration::restart_census::Busy);
    /// Nothing in progress, or a census that did not answer: the window
    /// closes again, and passes.
    fn close(&self);
}

/// Whether a press of the main window's close is held (t-6428) — the road
/// the window was left by nine times in nine before this. A close the
/// person chose, or one the road went for them, passes; a close that
/// already has a question standing, a wait armed or its census being read
/// keeps it; otherwise the close is held while the census is read on a
/// thread of its own, and the census answers: work in progress asks, the
/// question's minute running on the beat, and nothing in progress closes
/// the window again, unasked. A census that does not answer within
/// [`LEDGER_PATIENCE`] closes it all the same: the person pressed it, and a
/// close that waits on the ledger's actor for good is a close that does not
/// work — the goodbye's line says the census went unread.
///
/// The close event runs on the main thread, so nothing here waits on
/// another thread (t-49062): twice on 2026-10-12 a close's census put its
/// first question to the ledger's actor and waited for room in a full
/// mailbox, the window stood frozen from the hang watchdog's first word
/// (2,027 ms) until it was started again 20 and 34 s later, and the person
/// saw a crash.
pub(crate) fn close_pressed(window: impl Closing) -> bool {
    press_close(&EXITING, window)
}

/// The thread a close's census is read on, by the name a sample shows.
const CENSUS_THREAD: &str = "zerocode-close-census";
/// The thread a close waits for its census on.
const CLOSE_THREAD: &str = "zerocode-close";
/// The thread a wait's census is read on.
const WAIT_CENSUS_THREAD: &str = "zerocode-wait-census";

/// [`close_pressed`], on the way out `cell` holds.
fn press_close(cell: &'static Mutex<Exiting>, window: impl Closing) -> bool {
    {
        let mut state = lock(cell);
        if state.confirmed {
            return false;
        }
        if holds_close(&state) {
            return true;
        }
        state.reading = true;
    }
    let spawned = std::thread::Builder::new()
        .name(CLOSE_THREAD.to_string())
        .spawn(move || read_close(cell, window));
    if spawned.is_err() {
        // No thread to read it on: the close goes as it came, and the
        // goodbye still reads the census and puts every seated worker to
        // sleep on the way out.
        lock(cell).reading = false;
        return false;
    }
    true
}

/// A close's census, read off the close event, and what it answers.
fn read_close(cell: &'static Mutex<Exiting>, window: impl Closing) {
    let _reading = Reading(cell);
    let window = Arc::new(window);
    let reader = Arc::clone(&window);
    match census_within(CENSUS_THREAD, move || reader.census()) {
        Some(busy) if busy.busy => {
            asked(&mut lock(cell), Asking::Close, crate::now_epoch_ms());
            window.ask(busy);
        }
        answered => {
            {
                let mut state = lock(cell);
                state.confirmed = true;
                if answered.is_none() {
                    state.choice = Choice::Unread;
                }
            }
            window.close();
        }
    }
}

/// A close's census being read, until the reading ends however it ends —
/// answered, unanswered, or a door that panicked — so the press after it
/// reads again rather than being held by a reading nobody is doing.
struct Reading(&'static Mutex<Exiting>);

impl Drop for Reading {
    fn drop(&mut self) {
        lock(self.0).reading = false;
    }
}

/// Whether the close already has a question standing, a wait armed or its
/// census being read: a second press of the close keeps it rather than
/// asking twice.
fn holds_close(state: &Exiting) -> bool {
    state.reading
        || state
            .question
            .is_some_and(|(asking, _)| asking == Asking::Close)
        || state
            .wait
            .as_ref()
            .is_some_and(|wait| wait.asking == Asking::Close)
}

/// Whether anything on the way out needs the beat: a question standing, a
/// wait armed or a notice running.
pub(crate) fn watching() -> bool {
    let state = exiting();
    state.question.is_some() || state.wait.is_some() || state.notice.is_some()
}

/// The beat's second (t-6428): `read` is the census, read only while a wait
/// is armed — on a thread of its own, within [`LEDGER_PATIENCE`] (t-49062).
/// A wait goes by itself, nobody pressing anything at that moment, so a
/// census that does not answer is unknown, and unknown counts as busy
/// ([`UNREAD`]): never a gap, so the wait does not go on it, while its own
/// patience still runs out on it as on any busy census. The window's log is
/// told once ([`ExitBeat::Unread`]), and while that reading is still out
/// the actor is not asked again.
pub(crate) fn beat_reading(
    now_ms: i64,
    read: impl FnOnce() -> crate::orchestration::restart_census::Busy + Send + 'static,
) -> ExitBeat {
    beat_on(&EXITING, &WAIT_CENSUS_OUT, now_ms, read)
}

/// A wait's census that did not answer (t-49062): the reading itself is the
/// one thing nobody could read, and unknown counts as busy.
const UNREAD: crate::orchestration::restart_census::Busy =
    crate::orchestration::restart_census::Busy {
        busy: true,
        workers: 0,
        turning: 0,
        background: 0,
        unknown: 1,
        running: 0,
        gap: false,
    };

/// [`beat_reading`], on the way out `cell` holds, with `out` marking a
/// wait's census still out.
fn beat_on(
    cell: &Mutex<Exiting>,
    out: &'static AtomicBool,
    now_ms: i64,
    read: impl FnOnce() -> crate::orchestration::restart_census::Busy + Send + 'static,
) -> ExitBeat {
    if lock(cell).wait.is_none() {
        return step(&mut lock(cell), now_ms, None);
    }
    let busy = if out.swap(true, Ordering::AcqRel) {
        None
    } else {
        // Held by the reading itself, so a thread that never starts lets go
        // of it too.
        let held = Out(out);
        census_within(WAIT_CENSUS_THREAD, move || {
            let _held = held;
            read()
        })
    };
    let mut state = lock(cell);
    let Some(asking) = state.wait.as_ref().map(|wait| wait.asking) else {
        return step(&mut state, now_ms, None);
    };
    let busy = match busy {
        Some(busy) => {
            if let Some(wait) = state.wait.as_mut() {
                wait.unread_said = false;
            }
            busy
        }
        None => {
            let said = state
                .wait
                .as_mut()
                .is_some_and(|wait| std::mem::replace(&mut wait.unread_said, true));
            if !said {
                return ExitBeat::Unread(asking);
            }
            UNREAD
        }
    };
    step(&mut state, now_ms, Some(busy))
}

/// A wait's census still out, until its reading ends however it ends — or
/// never starts.
struct Out(&'static AtomicBool);

impl Drop for Out {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn step(
    state: &mut Exiting,
    now_ms: i64,
    busy: Option<crate::orchestration::restart_census::Busy>,
) -> ExitBeat {
    if state.notice.is_some() {
        return noticed(state, now_ms);
    }
    if let Some((asking, since_ms)) = state.question
        && asking
            .patience()
            .answer_ms
            .is_some_and(|limit| now_ms.saturating_sub(since_ms) >= limit)
    {
        return go(state, asking, Choice::Unanswered, now_ms);
    }
    let (Some(wait), Some(busy)) = (state.wait.clone(), busy) else {
        return ExitBeat::Nothing;
    };
    if busy.gap {
        return go(state, wait.asking, Choice::Gap, now_ms);
    }
    let patience = wait.asking.patience();
    if now_ms.saturating_sub(wait.since_ms) >= patience.gap_wait_ms {
        if patience.overdue_leaves {
            return go(state, wait.asking, Choice::Overdue, now_ms);
        }
        state.wait = None;
        return ExitBeat::AskAgain(wait.asking);
    }
    let line = WaitLine::of(wait.asking, busy, now_ms.saturating_sub(wait.since_ms));
    let told = (line.running, line.unknown, line.waited_min);
    if wait.told == Some(told) {
        return ExitBeat::Nothing;
    }
    if let Some(held) = state.wait.as_mut() {
        held.told = Some(told);
    }
    ExitBeat::Tell(line)
}

/// A road decided goes (t-67936): at once when its table shows no notice,
/// else its notice is raised from now and its first line answered.
fn go(state: &mut Exiting, asking: Asking, choice: Choice, now_ms: i64) -> ExitBeat {
    let Some(notice_ms) = asking.patience().notice_ms else {
        return settle(state, asking, choice);
    };
    state.question = None;
    state.wait = None;
    state.hand_ms = None;
    state.notice = Some(Notice {
        asking,
        choice,
        raised_ms: now_ms,
        due_ms: now_ms.saturating_add(notice_ms),
        told: None,
    });
    noticed(state, now_ms)
}

/// A standing notice, this second (t-67936): it goes once its time is up and
/// the person's hands have rested its road's `rest_ms`; until then its line
/// is told whenever the whole seconds left, or whether typing holds it, move.
fn noticed(state: &mut Exiting, now_ms: i64) -> ExitBeat {
    let Some(notice) = state.notice.clone() else {
        return ExitBeat::Nothing;
    };
    let rested_ms = state.hand_ms.map_or(i64::MIN, |at| {
        at.saturating_add(notice.asking.patience().rest_ms)
    });
    let goes_ms = notice.due_ms.max(rested_ms);
    if now_ms >= goes_ms {
        state.went = Some(Went {
            stood_ms: now_ms.saturating_sub(notice.raised_ms),
            held_ms: goes_ms.saturating_sub(notice.due_ms),
        });
        return settle(state, notice.asking, notice.choice);
    }
    let left_ms = goes_ms.saturating_sub(now_ms);
    let told = (
        left_ms.saturating_add(SECOND_MS - 1) / SECOND_MS,
        rested_ms > notice.due_ms,
    );
    if notice.told == Some(told) {
        return ExitBeat::Nothing;
    }
    if let Some(standing) = state.notice.as_mut() {
        standing.told = Some(told);
    }
    ExitBeat::Notice(NoticeLine::of(notice.asking, told.0, told.1))
}

fn settle(state: &mut Exiting, asking: Asking, choice: Choice) -> ExitBeat {
    state.question = None;
    state.wait = None;
    state.notice = None;
    state.choice = choice;
    state.confirmed = true;
    ExitBeat::Leave(asking)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::restart_census::Busy;
    use zerocode_orchestrator::runtime_actor::MAX_RUNTIME_MAILBOX;

    const MINUTE: i64 = 60_000;

    /// A census as the beat reads it: `running` commands under the panes,
    /// `unknown` workers nobody could read, and whether that is a gap.
    fn census(running: usize, unknown: usize) -> Busy {
        Busy {
            busy: running + unknown > 0,
            workers: 2,
            turning: 1,
            background: 0,
            running,
            unknown,
            gap: running == 0 && unknown == 0,
        }
    }

    fn waiting_on(asking: Asking) -> Exiting {
        Exiting {
            wait: Some(Wait {
                asking,
                since_ms: 0,
                told: None,
                unread_said: false,
            }),
            ..Exiting::default()
        }
    }

    #[test]
    fn a_wait_leaves_at_the_first_gap_and_tells_its_line_only_when_it_moves() {
        let toast = Asking::Restart(Some(RestartDoor::UpdateToast));
        let mut state = waiting_on(toast);
        assert_eq!(
            step(&mut state, 1_000, Some(census(2, 0))),
            ExitBeat::Tell(WaitLine {
                road: "restart",
                running: 2,
                unknown: 0,
                waited_min: 0,
                wait_min: 30,
            })
        );
        // The same numbers again: the line stands as it is.
        assert_eq!(
            step(&mut state, 2_000, Some(census(2, 0))),
            ExitBeat::Nothing
        );
        // A whole minute more with the same numbers: said once, for the
        // minute — the window keeps no clock of its own.
        assert!(matches!(
            step(&mut state, MINUTE + 2_000, Some(census(2, 0))),
            ExitBeat::Tell(line) if line.waited_min == 1 && line.running == 2
        ));
        assert_eq!(
            step(&mut state, MINUTE + 3_000, Some(census(2, 0))),
            ExitBeat::Nothing
        );
        assert!(matches!(
            step(&mut state, MINUTE + 4_000, Some(census(1, 0))),
            ExitBeat::Tell(line) if line.running == 1
        ));
        // A second nobody read a census for says nothing.
        assert_eq!(step(&mut state, MINUTE + 5_000, None), ExitBeat::Nothing);
        // The first gap: the road armed raises its notice (t-67936) — the
        // wait is over, nothing is confirmed yet.
        assert_eq!(
            step(&mut state, MINUTE + 6_000, Some(census(0, 0))),
            ExitBeat::Notice(NoticeLine {
                road: "restart",
                door: Some("update-toast"),
                seconds: 10,
                held: false,
            })
        );
        assert!(state.wait.is_none() && state.notice.is_some() && !state.confirmed);
        // Its ten seconds out, the road goes, and the goodbye says why.
        assert_eq!(
            step(&mut state, MINUTE + 16_000, Some(census(2, 0))),
            ExitBeat::Leave(toast)
        );
        assert_eq!(state.choice, Choice::Gap);
        assert!(state.confirmed && state.wait.is_none() && state.notice.is_none());
    }

    /// t-67936: 2026-10-21's two restarts went by this road at a gap nobody
    /// was watching for, and the person read both as 「갑자기 꺼짐」. A
    /// restart decided — pressed, or a wait that found its gap — raises a
    /// notice of the table's ten seconds first, says it once a second, and a
    /// key the person presses inside it holds the road until their hands
    /// have rested the table's five.
    #[test]
    fn a_restart_goes_only_after_its_notice_and_the_persons_typing_holds_it() {
        let toast = Asking::Restart(Some(RestartDoor::UpdateToast));
        let mut state = Exiting::default();
        let line = |seconds, held| {
            ExitBeat::Notice(NoticeLine {
                road: "restart",
                door: Some("update-toast"),
                seconds,
                held,
            })
        };
        assert_eq!(go(&mut state, toast, Choice::Now, 0), line(10, false));
        assert!(!state.confirmed && state.notice.is_some());
        // The same whole second again says nothing; the next one says itself.
        assert_eq!(step(&mut state, 500, None), ExitBeat::Nothing);
        assert_eq!(step(&mut state, 1_000, None), line(9, false));
        // A key at eight seconds: the road holds until the hands have rested
        // five, and the line says it is held.
        hands_at(&mut state, 8_000);
        assert_eq!(step(&mut state, 8_000, None), line(5, true));
        assert_eq!(step(&mut state, 10_000, None), line(3, true));
        assert_eq!(step(&mut state, 12_999, None), line(1, true));
        assert_eq!(step(&mut state, 13_000, None), ExitBeat::Leave(toast));
        assert_eq!(state.choice, Choice::Now);
        assert_eq!(
            state.went,
            Some(Went {
                stood_ms: 13_000,
                held_ms: 3_000,
            })
        );
        assert!(state.confirmed && state.notice.is_none());
        // Hands with no notice standing hold nothing.
        let mut idle = Exiting::default();
        hands_at(&mut idle, 5_000);
        assert_eq!(idle.hand_ms, None);
    }

    /// t-67936: a close shows no notice — the person pressed the close of
    /// the window they are looking at — so 「지금 종료」 and a close's
    /// unanswered minute still go at once; and a question standing replaces
    /// a restart's notice rather than leaving it counting under the question.
    #[test]
    fn a_close_goes_without_a_notice_and_a_question_takes_a_notices_place() {
        let mut close = Exiting::default();
        assert_eq!(
            go(&mut close, Asking::Close, Choice::Now, 0),
            ExitBeat::Leave(Asking::Close)
        );
        assert!(close.notice.is_none() && close.confirmed);
        let mut state = Exiting::default();
        let _ = go(&mut state, Asking::Restart(None), Choice::Unasked, 0);
        assert!(state.notice.is_some());
        asked(&mut state, Asking::Close, 1_000);
        assert!(state.notice.is_none() && state.question.is_some());
    }

    /// t-67936: every road out's own line says the road, whose hand, the
    /// choice, and for a restart how long the notice stood and the typing
    /// held it — a restart that went with no notice says so in so many words,
    /// a hand said for one road is not borrowed by another, and the line is
    /// said once however often the road is heard.
    #[test]
    fn the_door_line_says_whose_hand_and_how_long_the_notice_stood() {
        let toast = Asking::Restart(Some(RestartDoor::UpdateToast));
        let mut state = Exiting {
            by: Some((toast, "person click".to_string())),
            choice: Choice::Now,
            went: Some(Went {
                stood_ms: 13_400,
                held_ms: 3_000,
            }),
            ..Exiting::default()
        };
        assert_eq!(
            door_said(&state, ExitRoad::Restart(Some(RestartDoor::UpdateToast))),
            "exit door restart:update-toast · by person click · choice now · notice 13 s · held 3 s"
        );
        // The restart's hand is not the close's.
        state.went = None;
        assert_eq!(
            door_said(&state, ExitRoad::Close),
            "exit door close · by window close · choice now"
        );
        state.by = None;
        assert_eq!(
            door_said(&state, ExitRoad::Restart(None)),
            "exit door restart · by unsaid · choice now · no notice"
        );
        for (road, hand) in ROAD_HANDS {
            assert!(door_said(&state, road).contains(&format!(" · by {hand} · ")));
        }
        state.by = Some((Asking::Close, "person click +40ms".to_string()));
        assert_eq!(
            door_said(&state, ExitRoad::Close),
            "exit door close · by person click +40ms · choice now"
        );
    }

    /// t-67936: the note a restart leaves is taken once by the window it
    /// starts — and a note too old to be this start's is taken and dropped.
    #[test]
    fn a_restart_note_is_taken_once_and_only_while_fresh() {
        let root = tempfile::tempdir().unwrap();
        let note = RestartNote {
            door: Some("update-toast".to_string()),
            by: Some("person click".to_string()),
            from_version: "1.1.85".to_string(),
            from_commit: "9be7b2a47".to_string(),
            at_ms: 1_000,
        };
        leave_restart_note(root.path(), &note).unwrap();
        assert_eq!(take_restart_note(root.path(), 6_000), Some(note.clone()));
        assert_eq!(take_restart_note(root.path(), 7_000), None);
        leave_restart_note(root.path(), &note).unwrap();
        assert_eq!(
            take_restart_note(root.path(), 1_000 + RESTART_NOTE_FRESH_MS + 1),
            None
        );
        assert!(!root.path().join(RESTART_NOTE_FILE).exists());
    }

    #[test]
    fn a_restart_wait_that_runs_out_asks_again_and_a_close_wait_goes() {
        let mut restart = waiting_on(Asking::Restart(None));
        assert_eq!(
            step(&mut restart, 30 * MINUTE, Some(census(1, 0))),
            ExitBeat::AskAgain(Asking::Restart(None))
        );
        assert!(restart.wait.is_none() && !restart.confirmed);
        assert_eq!(restart.choice, Choice::Unasked);
        let mut close = waiting_on(Asking::Close);
        assert_eq!(
            step(&mut close, 10 * MINUTE, Some(census(1, 0))),
            ExitBeat::Leave(Asking::Close)
        );
        assert_eq!(close.choice, Choice::Overdue);
        // A worker nobody could read holds the gap back: unknown is busy.
        let mut unread = waiting_on(Asking::Close);
        assert!(matches!(
            step(&mut unread, 1_000, Some(census(0, 1))),
            ExitBeat::Tell(line) if line.unknown == 1 && line.running == 0
        ));
        assert!(unread.wait.is_some());
    }

    #[test]
    fn a_close_question_nobody_answers_goes_when_its_minute_is_up() {
        let mut close = Exiting {
            question: Some((Asking::Close, 0)),
            ..Exiting::default()
        };
        assert_eq!(step(&mut close, MINUTE - 1, None), ExitBeat::Nothing);
        assert_eq!(
            step(&mut close, MINUTE, None),
            ExitBeat::Leave(Asking::Close)
        );
        assert_eq!(close.choice, Choice::Unanswered);
        assert!(close.confirmed && close.question.is_none());
        // A restart's question stands until somebody answers it.
        let mut restart = Exiting {
            question: Some((Asking::Restart(None), 0)),
            ..Exiting::default()
        };
        assert_eq!(
            step(&mut restart, 24 * 60 * MINUTE, None),
            ExitBeat::Nothing
        );
    }

    #[test]
    fn a_second_close_keeps_its_question_or_its_wait_and_a_chosen_close_passes() {
        let mut state = Exiting::default();
        assert!(!holds_close(&state) && !state.confirmed);
        state.question = Some((Asking::Close, 0));
        assert!(holds_close(&state), "a question standing holds the close");
        let mut waiting = waiting_on(Asking::Close);
        assert!(holds_close(&waiting), "「끝나면 종료」 holds the close");
        assert!(
            !holds_close(&waiting_on(Asking::Restart(None))),
            "a restart's wait is not the close's"
        );
        // 「지금 종료」: nothing holds it, and the close it makes passes.
        assert_eq!(
            settle(&mut waiting, Asking::Close, Choice::Now),
            ExitBeat::Leave(Asking::Close)
        );
        assert!(!holds_close(&waiting) && waiting.confirmed);
        assert_eq!(waiting.choice, Choice::Now);
    }

    #[test]
    fn the_window_names_a_road_and_the_goodbye_says_every_choice_by_one_word() {
        assert_eq!(
            Asking::named("restart", Some("update-toast")),
            Some(Asking::Restart(Some(RestartDoor::UpdateToast)))
        );
        assert_eq!(Asking::named("close", None), Some(Asking::Close));
        assert_eq!(Asking::named("quit", None), None);
        for (choice, word) in CHOICES {
            assert_eq!(choice.word(), word);
        }
        assert_eq!(Choice::default(), Choice::Unasked);
    }

    #[test]
    fn the_first_road_named_is_the_road_the_window_left_by() {
        let mut held = None;
        assert_eq!(first(&mut held, ExitRoad::Close), ExitRoad::Close);
        // The browser's own `app.exit(0)` and tauri's `Exit` come after.
        assert_eq!(first(&mut held, ExitRoad::App), ExitRoad::Close);
        assert_eq!(first(&mut held, ExitRoad::Terminate), ExitRoad::Close);
    }

    #[test]
    fn every_door_is_named_by_one_word_and_said_back_by_it() {
        for (door, word) in DOORS {
            assert_eq!(RestartDoor::named(word), Some(door));
            assert_eq!(
                ExitRoad::Restart(Some(door)).to_string(),
                format!("restart:{word}")
            );
        }
        assert_eq!(RestartDoor::named("a-door-nobody-built"), None);
        assert_eq!(ExitRoad::Restart(None).to_string(), "restart");
        for (road, word) in [
            (ExitRoad::Close, "close"),
            (ExitRoad::Terminate, "terminate"),
            (ExitRoad::Tray, "tray"),
            (ExitRoad::App, "app"),
        ] {
            assert_eq!(road.to_string(), word);
        }
    }

    /// Each step a road out takes, written down as it is taken, and the
    /// road its goodbye named.
    #[derive(Default)]
    struct Steps {
        taken: std::cell::RefCell<Vec<&'static str>>,
        road: std::cell::Cell<Option<ExitRoad>>,
    }

    impl Terminating for Steps {
        fn goodbye(&self, road: ExitRoad) {
            self.road.set(Some(road));
            self.taken.borrow_mut().push("goodbye");
        }
        fn keep_screens(&self) {
            self.taken.borrow_mut().push("screens");
        }
        fn keep_statuses(&self) {
            self.taken.borrow_mut().push("statuses");
        }
        fn end_panes(&self) {
            self.taken.borrow_mut().push("panes");
        }
        fn shut_browser(&self) {
            // The stall: whatever ends in here ends after everything above.
            self.taken.borrow_mut().push("browser");
        }
        fn mark_clean_exit(&self) {
            self.taken.borrow_mut().push("clean");
        }
    }

    /// t-7812 D1: the goodbye, the screens, the statuses and the panes all
    /// come before the embedded browser's shutdown — the step that stalled
    /// twenty seconds while the panes died and the ledger, its goodbye
    /// still queued behind it, heard three workers as dead.
    #[test]
    fn terminate_says_goodbye_and_keeps_the_list_before_the_browser_can_stall() {
        let steps = Steps::default();
        leave_in_order(ExitRoad::Terminate, &steps);
        assert_eq!(steps.road.get(), Some(ExitRoad::Terminate));
        assert_eq!(
            *steps.taken.borrow(),
            [
                "goodbye", "screens", "statuses", "panes", "browser", "clean"
            ]
        );
    }

    /// The restart button's road: the goodbye and what the next window
    /// restores from, before tauri's restart replaces the process — which
    /// no exit event follows.
    #[test]
    fn a_restart_button_says_goodbye_and_keeps_the_list_before_it_restarts() {
        let steps = Steps::default();
        let road = ExitRoad::Restart(Some(RestartDoor::UpdateToast));
        relaunch(road, &steps);
        assert_eq!(steps.road.get(), Some(road));
        assert_eq!(*steps.taken.borrow(), ["goodbye", "screens", "statuses"]);
    }

    /// The layouts file stops being the window's to rewrite the moment a
    /// road is named, and not before: a question asked and cancelled is not
    /// leaving.
    #[test]
    fn a_window_is_leaving_once_a_road_is_named_and_not_while_it_only_asks() {
        let mut state = Exiting::default();
        assert!(state.road.is_none(), "a fresh window is already leaving");
        state.question = Some((Asking::Close, 0));
        state.question = None;
        assert!(state.road.is_none(), "an answered question is leaving");
        assert_eq!(
            first(&mut state.road, ExitRoad::Terminate),
            ExitRoad::Terminate
        );
        assert!(state.road.is_some());
    }

    /// The longest a close event may stand in a press of the close
    /// (t-49062): far under the hang watchdog's first threshold, and far
    /// over a press that only reads its own state.
    const PRESS: Duration = Duration::from_millis(50);
    /// How long a test lets a press stand before the ledger's actor drains:
    /// ten times [`PRESS`], so a press that waited on its census stood at
    /// least this long.
    const STANDING: Duration = Duration::from_millis(500);
    /// How long a census, once nothing holds it, is given to answer.
    const ANSWER: Duration = Duration::from_secs(10);

    /// A way out of its own, so a test that presses the close never touches
    /// the window's.
    fn a_window() -> &'static Mutex<Exiting> {
        Box::leak(Box::new(Mutex::new(Exiting::default())))
    }

    /// The main window as a press of its close reaches it: its census, and
    /// what the close said through each door.
    struct Window {
        census: Census,
        read: mpsc::Sender<std::thread::Thread>,
        asked: mpsc::Sender<Busy>,
        closed: mpsc::Sender<()>,
    }

    /// How a test's census answers.
    enum Census {
        /// Its first question stands in the ledger's actor's mailbox the
        /// way `RuntimeActor::request` does — a full mailbox waits for room
        /// — and then it says `Busy`.
        Mailbox(mpsc::SyncSender<()>, Busy),
        /// The actor never answers it.
        Never,
        /// It panics, as a bug in it would.
        Panics,
    }

    /// What a test hears from the window's doors: the thread each census
    /// was read on, each question, each close.
    struct Said {
        read: mpsc::Receiver<std::thread::Thread>,
        asked: mpsc::Receiver<Busy>,
        closed: mpsc::Receiver<()>,
    }

    impl Window {
        fn new(mailbox: mpsc::SyncSender<()>, busy: Busy) -> (Self, Said) {
            Self::answering(Census::Mailbox(mailbox, busy))
        }

        fn answering(census: Census) -> (Self, Said) {
            let (read, reads) = mpsc::channel();
            let (asked, asks) = mpsc::channel();
            let (closed, closes) = mpsc::channel();
            (
                Self {
                    census,
                    read,
                    asked,
                    closed,
                },
                Said {
                    read: reads,
                    asked: asks,
                    closed: closes,
                },
            )
        }
    }

    impl Closing for Window {
        fn census(&self) -> Busy {
            let _ = self.read.send(std::thread::current());
            match &self.census {
                Census::Mailbox(mailbox, busy) => {
                    let _ = mailbox.send(());
                    *busy
                }
                Census::Never => never(),
                Census::Panics => panic!("a census that panics on purpose (t-49062 test)"),
            }
        }

        fn ask(&self, busy: Busy) {
            let _ = self.asked.send(busy);
        }

        fn close(&self) {
            let _ = self.closed.send(());
        }
    }

    /// The ledger's actor, busy: its mailbox full, and nothing taken from it
    /// until the test drops it.
    fn a_busy_actor() -> (mpsc::SyncSender<()>, mpsc::Receiver<()>) {
        let (mailbox, actor) = mpsc::sync_channel(MAX_RUNTIME_MAILBOX);
        for _ in 0..MAX_RUNTIME_MAILBOX {
            let _ = mailbox.try_send(());
        }
        (mailbox, actor)
    }

    /// Press the close on a thread standing in for the main thread: what the
    /// press answered and how long it stood in it.
    fn press_on_main(
        cell: &'static Mutex<Exiting>,
        window: Window,
    ) -> (
        mpsc::Receiver<(bool, Duration)>,
        std::thread::JoinHandle<()>,
    ) {
        let (answer, answered) = mpsc::channel();
        let main = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let held = press_close(cell, window);
            let _ = answer.send((held, started.elapsed()));
        });
        (answered, main)
    }

    /// t-49062: a press of the close never waits on its census. The close
    /// event runs on the main thread, and the census asks the ledger's actor
    /// — whose mailbox holds four commands and waits when full — and runs
    /// the host's `ps`: on 2026-10-12 a close stood in that mailbox
    /// (`event_window_close` → `leave_census` → `restart_census::take` →
    /// `RuntimeActor::request`) from the hang watchdog's first word at
    /// 2,027 ms until the window was started again 20 s later.
    /// Here the actor is busy and its mailbox full: the press comes back held
    /// within 50 ms, the census is read on a thread of its own, and once the
    /// actor drains the census still asks.
    #[test]
    fn a_close_press_never_waits_on_its_census_in_the_close_event() {
        let cell = a_window();
        let (mailbox, actor) = a_busy_actor();
        let (window, said) = Window::new(mailbox, census(1, 0));
        let (answered, main) = press_on_main(cell, window);
        // A press that waits on its census is still standing here.
        let early = answered.recv_timeout(STANDING);
        drop(actor);
        let Ok((held, stood)) = early.or_else(|_| answered.recv_timeout(ANSWER)) else {
            panic!("the press never came back, even once the ledger's actor drained");
        };
        assert!(
            stood <= PRESS,
            "the close event stood {} ms in a press of the close while the ledger's actor had a \
             full mailbox — the press waited on its census on the main thread (t-49062)",
            stood.as_millis()
        );
        assert!(held, "a press with work in progress let the close go");
        let reader = said.read.recv_timeout(ANSWER);
        assert!(
            reader
                .as_ref()
                .is_ok_and(|reader| reader.id() != main.thread().id()
                    && reader.name() == Some("zerocode-close-census")),
            "the census was not read on a thread of its own: {reader:?}"
        );
        assert_eq!(
            said.asked.recv_timeout(ANSWER),
            Ok(census(1, 0)),
            "the census, read off the close event, never asked"
        );
        assert!(
            holds_close(&lock(cell)),
            "the question it asked does not hold the next press"
        );
        assert!(
            said.closed.try_recv().is_err(),
            "a close with work in progress went"
        );
        assert!(
            said.read.try_recv().is_err(),
            "one press read the census twice"
        );
        assert!(main.join().is_ok(), "the press panicked");
    }

    /// The ledger's actor, idle: its mailbox empty, so a census's question
    /// goes straight in.
    fn an_idle_actor() -> (mpsc::SyncSender<()>, mpsc::Receiver<()>) {
        mpsc::sync_channel(MAX_RUNTIME_MAILBOX)
    }

    /// t-49062: a close nothing would cut is held only while its census is
    /// read, then goes unasked — and the close that makes passes, with
    /// nothing read for it.
    #[test]
    fn a_close_nothing_would_cut_goes_unasked_once_its_census_answers() {
        let cell = a_window();
        let (mailbox, _actor) = an_idle_actor();
        let (window, said) = Window::new(mailbox, Busy::default());
        assert!(
            press_close(cell, window),
            "the close went before its census was read"
        );
        assert_eq!(
            said.closed.recv_timeout(ANSWER),
            Ok(()),
            "the census never let the close go"
        );
        assert!(
            said.asked.try_recv().is_err(),
            "a close nothing would cut asked"
        );
        {
            let state = lock(cell);
            assert!(
                state.confirmed && state.question.is_none(),
                "the close the census let go is held: {state:?}"
            );
            assert_eq!(
                state.choice,
                Choice::Unasked,
                "the goodbye would say the person chose"
            );
        }
        let (mailbox, _actor) = an_idle_actor();
        let (window, again) = Window::new(mailbox, Busy::default());
        assert!(
            !press_close(cell, window),
            "the close the census let go was held again"
        );
        assert!(
            again.read.try_recv().is_err(),
            "the close the census let go read the census again"
        );
    }

    /// t-49062: a second press while the census is still being read is held
    /// as the first was and reads nothing more — one census, one question.
    #[test]
    fn a_second_press_while_the_census_is_read_waits_for_the_same_answer() {
        let cell = a_window();
        let (mailbox, actor) = a_busy_actor();
        let (window, said) = Window::new(mailbox, census(1, 0));
        assert!(
            press_close(cell, window),
            "the first press let the close go"
        );
        // The census stands in the actor's full mailbox from here on.
        assert!(
            said.read.recv_timeout(ANSWER).is_ok(),
            "the census was never read"
        );
        let (mailbox, _idle) = an_idle_actor();
        let (window, again) = Window::new(mailbox, Busy::default());
        assert!(
            press_close(cell, window),
            "a second press let the close go while its census was read"
        );
        assert!(
            again.read.try_recv().is_err(),
            "a second press read the census again"
        );
        drop(actor);
        assert_eq!(
            said.asked.recv_timeout(ANSWER),
            Ok(census(1, 0)),
            "the census never asked"
        );
        assert!(
            again.asked.try_recv().is_err() && again.closed.try_recv().is_err(),
            "the second press answered on its own"
        );
    }

    /// A census the ledger's actor never answers: its thread parks for good,
    /// as one standing in a mailbox nobody drains does.
    fn never() -> Busy {
        loop {
            std::thread::park();
        }
    }

    /// t-49062: a close whose census the ledger's actor never answers goes
    /// all the same once the ledger's patience is out. The person pressed it,
    /// so it proceeds unasked, and the goodbye's line says the census went
    /// unread. (A wait, which goes by itself, stands instead — two tests on.)
    #[test]
    fn a_close_whose_census_never_answers_goes_unasked_on_the_ledgers_patience() {
        let cell = a_window();
        let (window, said) = Window::answering(Census::Never);
        let pressed = std::time::Instant::now();
        assert!(
            press_close(cell, window),
            "the close went before its census was read"
        );
        assert_eq!(
            said.closed.recv_timeout(LEDGER_PATIENCE + ANSWER),
            Ok(()),
            "a close whose census never answered never went"
        );
        let waited = pressed.elapsed();
        assert!(
            waited >= LEDGER_PATIENCE,
            "the close went before the ledger's patience was out: {} ms",
            waited.as_millis()
        );
        assert!(
            said.asked.try_recv().is_err(),
            "a census that never answered asked"
        );
        let state = lock(cell);
        assert!(
            state.confirmed && state.question.is_none(),
            "the close that went is held: {state:?}"
        );
        assert_eq!(
            state.choice,
            Choice::Unread,
            "the goodbye would not say the census went unread"
        );
    }

    /// t-49062: a census that panics is one that did not answer — the close
    /// goes unasked on it at once, and nothing is left holding the next
    /// press.
    #[test]
    fn a_census_that_panics_lets_the_close_go_as_one_that_never_answers() {
        let cell = a_window();
        let (window, said) = Window::answering(Census::Panics);
        assert!(
            press_close(cell, window),
            "the close went before its census was read"
        );
        assert_eq!(
            said.closed.recv_timeout(ANSWER),
            Ok(()),
            "a close whose census panicked never went"
        );
        assert!(
            said.asked.try_recv().is_err(),
            "a census that panicked asked"
        );
        // The reading ends as the close's thread lets go of it.
        let deadline = std::time::Instant::now() + ANSWER;
        while lock(cell).reading && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        let state = lock(cell);
        assert!(
            !state.reading && state.confirmed,
            "a census that panicked left the close held: {state:?}"
        );
        assert_eq!(state.choice, Choice::Unread);
    }

    /// t-49062: a wait — 「끝나면」, which goes by itself with nobody pressing
    /// anything — whose census the ledger's actor never answers stands. The
    /// census is unknown and unknown counts as busy: never a gap, so the
    /// restart does not go on it, and when the wait's own patience is out it
    /// asks the person again rather than restarting. The log is told once,
    /// and the beats after it neither ask the actor again nor wait on it.
    /// (A close the person pressed goes instead — two tests back.)
    #[test]
    fn a_wait_whose_census_never_answers_stands_as_busy_and_is_said_once() {
        static OUT: AtomicBool = AtomicBool::new(false);
        let restart = Asking::Restart(None);
        let cell: &'static Mutex<Exiting> = Box::leak(Box::new(Mutex::new(waiting_on(restart))));
        assert_eq!(
            beat_on(cell, &OUT, MINUTE, never),
            ExitBeat::Unread(restart),
            "a census nobody answered did not stand the wait"
        );
        let again = std::time::Instant::now();
        assert_eq!(
            beat_on(cell, &OUT, MINUTE + 1_000, never),
            ExitBeat::Tell(WaitLine {
                road: "restart",
                running: 0,
                unknown: 1,
                waited_min: 1,
                wait_min: 30,
            }),
            "the wait's line does not say its census is unknown"
        );
        assert_eq!(
            beat_on(cell, &OUT, MINUTE + 2_000, never),
            ExitBeat::Nothing
        );
        assert!(
            again.elapsed() < PRESS,
            "a beat waited on a census it had already left out: {} ms",
            again.elapsed().as_millis()
        );
        assert_eq!(
            beat_on(cell, &OUT, 30 * MINUTE, never),
            ExitBeat::AskAgain(restart),
            "a restart whose census nobody read went by itself"
        );
        assert!(!lock(cell).confirmed, "a wait went on a census nobody read");
    }
}
