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
use std::sync::Mutex;

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
}

/// A restart waits half an hour for the first gap, then asks again. Measured
/// over the workers' own transcripts (09-16 – 09-24, 780 background
/// commands): half end within 3.2 minutes, nine in ten within 14.7, and 97 %
/// within 30 — a turn is not waited out (half run past 56 minutes), only
/// what runs under it.
const RESTART_PATIENCE: Patience = Patience {
    gap_wait_ms: 30 * MINUTE_MS,
    answer_ms: None,
    overdue_leaves: false,
};

/// A close waits ten minutes (82 % of those commands end within it) and then
/// goes, because closing is the person leaving; and its question stands one
/// minute, so a close nobody is there to answer still closes.
const CLOSE_PATIENCE: Patience = Patience {
    gap_wait_ms: 10 * MINUTE_MS,
    answer_ms: Some(MINUTE_MS),
    overdue_leaves: true,
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
}

/// Each choice's word in the goodbye's line, in one table.
const CHOICES: [(Choice, &str); 5] = [
    (Choice::Unasked, "unasked"),
    (Choice::Now, "now"),
    (Choice::Gap, "gap"),
    (Choice::Overdue, "overdue"),
    (Choice::Unanswered, "unanswered"),
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

/// What the beat does about the way out, this second.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExitBeat {
    Nothing,
    /// The wait's line moved.
    Tell(WaitLine),
    /// The wait ran out on a road that asks again.
    AskAgain(Asking),
    /// Go, by this road.
    Leave(Asking),
}

/// The window's way out, as far as it has got.
#[derive(Debug, Default)]
struct Exiting {
    road: Option<ExitRoad>,
    /// A question standing, and since when.
    question: Option<(Asking, i64)>,
    wait: Option<Wait>,
    choice: Choice,
    /// The person chose to go, or the road went for them: the next close
    /// passes without a question.
    confirmed: bool,
}

static EXITING: Mutex<Exiting> = Mutex::new(Exiting {
    road: None,
    question: None,
    wait: None,
    choice: Choice::Unasked,
    confirmed: false,
});

fn exiting() -> std::sync::MutexGuard<'static, Exiting> {
    EXITING.lock().unwrap_or_else(|held| held.into_inner())
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

/// What the person chose on the way out, for the goodbye's line.
pub(crate) fn choice() -> Choice {
    exiting().choice
}

/// A question stands on this road, from now: the close's minute runs.
pub(crate) fn ask(asking: Asking, now_ms: i64) {
    let mut state = exiting();
    state.question = Some((asking, now_ms));
    state.wait = None;
}

/// 「끝나면」: wait on this road for the first gap, from now.
pub(crate) fn arm(asking: Asking, now_ms: i64) {
    let mut state = exiting();
    state.question = None;
    state.wait = Some(Wait {
        asking,
        since_ms: now_ms,
        told: None,
    });
}

/// 「지금」: this road goes now, and says so.
pub(crate) fn leave_now(asking: Asking) {
    let _ = settle(&mut exiting(), asking, Choice::Now);
}

/// 「취소」: no question stands and nothing waits.
pub(crate) fn cancel() {
    let mut state = exiting();
    state.question = None;
    state.wait = None;
}

/// Whether a close passes without a question: the person chose to go, or
/// the road went for them.
pub(crate) fn confirmed() -> bool {
    exiting().confirmed
}

/// Whether the close already has a question standing or a wait armed: a
/// second press of the close keeps it rather than asking twice.
pub(crate) fn holding_close() -> bool {
    holds_close(&exiting())
}

fn holds_close(state: &Exiting) -> bool {
    state
        .question
        .is_some_and(|(asking, _)| asking == Asking::Close)
        || state
            .wait
            .as_ref()
            .is_some_and(|wait| wait.asking == Asking::Close)
}

/// Whether anything on the way out needs the beat: a question standing or
/// a wait armed.
pub(crate) fn watching() -> bool {
    let state = exiting();
    state.question.is_some() || state.wait.is_some()
}

/// Whether a wait is armed — the one state the beat reads a census for.
pub(crate) fn waiting() -> bool {
    exiting().wait.is_some()
}

/// The beat's second (t-6428): `busy` is the census, read only while a wait
/// is armed.
pub(crate) fn beat(
    now_ms: i64,
    busy: Option<crate::orchestration::restart_census::Busy>,
) -> ExitBeat {
    step(&mut exiting(), now_ms, busy)
}

fn step(
    state: &mut Exiting,
    now_ms: i64,
    busy: Option<crate::orchestration::restart_census::Busy>,
) -> ExitBeat {
    if let Some((asking, since_ms)) = state.question
        && asking
            .patience()
            .answer_ms
            .is_some_and(|limit| now_ms.saturating_sub(since_ms) >= limit)
    {
        return settle(state, asking, Choice::Unanswered);
    }
    let (Some(wait), Some(busy)) = (state.wait.clone(), busy) else {
        return ExitBeat::Nothing;
    };
    if busy.gap {
        return settle(state, wait.asking, Choice::Gap);
    }
    let patience = wait.asking.patience();
    if now_ms.saturating_sub(wait.since_ms) >= patience.gap_wait_ms {
        if patience.overdue_leaves {
            return settle(state, wait.asking, Choice::Overdue);
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

fn settle(state: &mut Exiting, asking: Asking, choice: Choice) -> ExitBeat {
    state.question = None;
    state.wait = None;
    state.choice = choice;
    state.confirmed = true;
    ExitBeat::Leave(asking)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::restart_census::Busy;

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
        // The first gap: the road armed goes, and the goodbye says why.
        assert_eq!(
            step(&mut state, MINUTE + 6_000, Some(census(0, 0))),
            ExitBeat::Leave(toast)
        );
        assert_eq!(state.choice, Choice::Gap);
        assert!(state.confirmed && state.wait.is_none());
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
}
