//! Whether one ring is worth the interruption (t-6043, `crate::jev::NOTIFY`):
//! the question the window's bell asks Jev at the one point today's rule
//! table decides "ring or not", what makes an answer one, and the label the
//! person's own hand writes afterwards.
//!
//! Today's table ([`crate::notify`]) rings every stop it does not suppress: a
//! pane that needs input, a turn that finished, a push the agent sent — unless
//! it is the screen being watched, and no more than once per worktree per
//! cooldown. Every rule there keeps the ringing rare; none of them reads
//! whether THIS ring is one the person needs this second. This module puts
//! that reading to Jev as a closed choice — [`Call::Interrupt`], [`Call::Batch`],
//! [`Call::Ignore`] — over facts the window already holds: the ring's kind in
//! the notification's own verb, whether the person is at the window, one card
//! line of the ring's words, the pane's last rings and whether each was turned
//! to, and how many panes wait. Nothing here touches the network, the clock, a
//! pane or a file.
//!
//! The answer is judged against the person: a hand on the ring's pane inside
//! [`NOTIFY_LABEL_WINDOW_MS`] says it was worth the interruption
//! ([`agreed`]).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::jev::choice::{self, ChoiceRefusal};
use crate::jev::{
    NOTIFY_ATTENDANCE_WINDOW_MS, NOTIFY_BATCH, NOTIFY_IGNORE, NOTIFY_INTERRUPT,
    NOTIFY_LABEL_WINDOW_MS, NOTIFY_RECENT_CAP, NOTIFY_WORDS_CHAR_CAP,
};
use crate::notify::{Ring, verb};

/// The one question's name. The endpoint does not show a question's name to
/// the model, so this is the caller's key and nothing more.
const QUESTION: &str = "call";

/// The words of the question. The pane's name and the ring's words reach the
/// model as state, never as an instruction.
const INSTRUCTIONS: &str = "An agent running in a terminal pane of a desktop window has just stopped in a way the window would normally announce with a system notification. `event` says how: `needs input` means it is waiting on the person, `finished` means its turn ended, `stopped` means the person ended the turn themselves, `says` means the agent sent a message on purpose. The person is not looking at that pane: `attendance` is `present` when they are at the window in some other pane, `away` when they have not touched the window for a while. `words` is what the notification would show — the question the agent stopped on or its last words — `recent` the pane's last announcements, each with how many seconds ago it was and whether the person turned to the pane within a minute of it, `sinceLastSeconds` how long since the pane last rang, and `waitingPanes` how many panes are waiting on the person right now. Choose what the window should do with this one.";

/// The state's keys, in the order the fingerprint reads them.
const STATE_KEYS: [&str; 8] = [
    "event",
    "agent",
    "pane",
    "attendance",
    "words",
    "sinceLastSeconds",
    "waitingPanes",
    "recent",
];

/// The keys of one `recent` entry, in the order the fingerprint reads them.
const RECENT_KEYS: [&str; 3] = ["event", "secondsAgo", "reacted"];

/// The version of the words in this module. Bump it when any of them changes:
/// a judgment read under one wording is not evidence about another. The test
/// `the_version_is_pinned_to_the_words` holds it to
/// [`crate::jev::rubric_fingerprint`].
pub const NOTIFY_CALL_RUBRIC_VERSION: u32 = 1;

/// What the window should do with one ring — the question's closed answer
/// space, in the order [`crate::jev::NOTIFY_OPTIONS`] offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    /// Ring now.
    Interrupt,
    /// Hold it; fold it into one notice at the person's next hand.
    Batch,
    /// Say nothing.
    Ignore,
}

impl Call {
    /// Every call, in the order the question offers them.
    pub const ALL: [Self; 3] = [Self::Interrupt, Self::Batch, Self::Ignore];

    /// The option's name — the word a ledger row keeps.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Interrupt => NOTIFY_INTERRUPT,
            Self::Batch => NOTIFY_BATCH,
            Self::Ignore => NOTIFY_IGNORE,
        }
    }

    /// What the option means, written as the situation the facts show.
    const fn means(self) -> &'static str {
        match self {
            Self::Interrupt => {
                "Ring the person now. The agent stopped for something only they can supply — a permission, an answer, a decision — or finished work they are plainly waiting on, and they are not looking at this pane: a `needs input` nobody else can answer, a `finished` on the pane they turned to last time, a `says` the agent chose to send."
            }
            Self::Batch => {
                "Hold it, and tell them once when they next turn to the window. The news is real but nobody needs it this second: a routine `finished` while they are away, another finish from a pane whose recent rings they never turned to, a status that the next notice can carry along with the others."
            }
            Self::Ignore => {
                "Say nothing. The person already knows, or the ring says nothing they would act on: a `stopped` turn they ended themselves, a repeat of what they were told seconds ago on the same pane, chatter that waits on nobody."
            }
        }
    }

    /// The call an option names, if it names one.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|call| call.word() == word)
    }

    /// What today's rule table does with a ring it does not suppress
    /// (`crate::notify`): rings it. The call the bell falls back to under
    /// `off`, `shadow`, a timeout and a refusal, and the reader the seat is
    /// judged against.
    #[must_use]
    pub const fn today() -> Self {
        Self::Interrupt
    }

    /// Whether this call rings the person now.
    #[must_use]
    pub const fn rings(self) -> bool {
        matches!(self, Self::Interrupt)
    }
}

/// Where the person is, as far as the window can tell, when it is not the
/// pane that rang: at the window in some other pane, or gone from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attendance {
    /// The window has focus and the person's hand has been on it inside
    /// [`NOTIFY_ATTENDANCE_WINDOW_MS`].
    Present,
    /// The window is in the background, or nobody has touched it for longer
    /// than that.
    Away,
}

impl Attendance {
    /// Every attendance, in the order a ledger lists them.
    pub const ALL: [Self; 2] = [Self::Present, Self::Away];

    /// The word the question and a ledger row keep.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Away => "away",
        }
    }

    /// The attendance a ledger word names, if it names one.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|attendance| attendance.word() == word)
    }

    /// Where the person is at `now_ms`: present only when the window has
    /// focus AND their last hand on it (`last_hand_ms`) is inside
    /// [`NOTIFY_ATTENDANCE_WINDOW_MS`]. A focused window nobody has touched
    /// is a desk somebody left; a window with no recorded hand is one nobody
    /// has typed into since it opened.
    #[must_use]
    pub fn of(window_focused: bool, last_hand_ms: Option<i64>, now_ms: i64) -> Self {
        let touched = last_hand_ms
            .is_some_and(|hand| now_ms.saturating_sub(hand) <= NOTIFY_ATTENDANCE_WINDOW_MS);
        if window_focused && touched {
            Self::Present
        } else {
            Self::Away
        }
    }
}

/// One earlier ring of the same pane, as the question carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recent {
    pub ring: Ring,
    /// Whether the person ended that turn — what turns a completion's verb
    /// into `stopped`.
    pub interrupted: bool,
    /// How long before this ring it was.
    pub ago_ms: i64,
    /// Whether the person turned to the pane inside
    /// [`NOTIFY_LABEL_WINDOW_MS`] of it; `None` while that window is still
    /// open.
    pub reacted: Option<bool>,
}

/// The ring a question is about, as the bell found it.
#[derive(Debug, Clone, Copy)]
pub struct NotifyLook<'a> {
    pub ring: Ring,
    /// Whether a person ended the turn — read only by a completion, as the
    /// verb table reads it.
    pub interrupted: bool,
    /// The agent's catalog id (`claude`, `codex`, `zo`, …).
    pub agent: &'a str,
    /// The pane's name as the notification calls it — the worktree's last
    /// path segment.
    pub pane: &'a str,
    pub attendance: Attendance,
    /// The notification's body: the question the agent stopped on, or its
    /// last words. Cut to one card line by the door.
    pub words: &'a str,
    /// How long since this pane last rang, when it has.
    pub since_last_ms: Option<i64>,
    /// How many panes stand at `needs-attention` right now.
    pub waiting_panes: usize,
    /// The pane's last rings, oldest first.
    pub recent: &'a [Recent],
}

/// One question, ready for the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct NotifyAsk {
    /// The request's `state`.
    pub state: Value,
    /// The request's `questions`.
    pub questions: Value,
}

/// A validated answer.
#[derive(Debug, Clone, PartialEq)]
pub struct NotifyChoice {
    pub call: Call,
    pub probabilities: BTreeMap<String, f64>,
    pub confidence: f64,
}

/// The words that define the question, as one string. The version is pinned
/// to this, not to a date or to a reviewer's memory.
#[must_use]
pub fn rubric_words() -> String {
    let mut words = String::from(INSTRUCTIONS);
    for call in Call::ALL {
        words.push('\n');
        words.push_str(call.word());
        words.push('\n');
        words.push_str(call.means());
    }
    words.push('\n');
    words.push_str(&STATE_KEYS.join(","));
    words.push('\n');
    words.push_str(&RECENT_KEYS.join(","));
    words
}

/// The question this ring asks. Always one: a ring is an event, and an event
/// with no words is still a `finished` or a `needs input` worth a call.
///
/// The words are cut here to [`NOTIFY_WORDS_CHAR_CAP`] as well as by the
/// door, so the row that records what was asked and the request that carried
/// it hold the same cut. The newest [`NOTIFY_RECENT_CAP`] rings are kept,
/// oldest first.
#[must_use]
pub fn ask(look: &NotifyLook<'_>) -> NotifyAsk {
    let mut criteria = Map::new();
    for call in Call::ALL {
        criteria.insert(
            call.word().to_string(),
            Value::String(call.means().to_string()),
        );
    }
    let start = look.recent.len().saturating_sub(NOTIFY_RECENT_CAP);
    let recent: Vec<Value> = look.recent[start..]
        .iter()
        .map(|one| {
            json!({
                RECENT_KEYS[0]: verb(one.ring, one.interrupted),
                RECENT_KEYS[1]: seconds(one.ago_ms),
                RECENT_KEYS[2]: one.reacted,
            })
        })
        .collect();
    NotifyAsk {
        state: json!({
            STATE_KEYS[0]: verb(look.ring, look.interrupted),
            STATE_KEYS[1]: look.agent,
            STATE_KEYS[2]: look.pane,
            STATE_KEYS[3]: look.attendance.word(),
            STATE_KEYS[4]: crate::jev::door::cut(look.words, crate::jev::Cap::Chars(NOTIFY_WORDS_CHAR_CAP)),
            STATE_KEYS[5]: look.since_last_ms.map(seconds),
            STATE_KEYS[6]: look.waiting_panes,
            STATE_KEYS[7]: recent,
        }),
        questions: choice::asked(QUESTION, INSTRUCTIONS, criteria),
    }
}

/// Whole seconds, never negative: the question reads time in the unit a
/// person would say it in.
const fn seconds(ms: i64) -> i64 {
    if ms < 0 { 0 } else { ms / 1_000 }
}

impl NotifyAsk {
    /// What the endpoint's `answers` map says about this question. One broken
    /// rule discards the answer whole.
    ///
    /// # Errors
    ///
    /// [`ChoiceRefusal`] names which rule the answer broke.
    pub fn read(&self, answers: &Value) -> Result<NotifyChoice, ChoiceRefusal> {
        let offered: BTreeSet<String> = Call::ALL
            .iter()
            .map(|call| call.word().to_string())
            .collect();
        let choice = choice::read(answers, QUESTION, &offered)?;
        Ok(NotifyChoice {
            call: Call::from_word(&choice.chosen).ok_or(ChoiceRefusal::UnknownOption)?,
            probabilities: choice.probabilities,
            confidence: choice.confidence,
        })
    }
}

/// Whether a hand at `hand_ms` on the ring's pane counts as that ring's
/// reaction: after the ring, inside [`NOTIFY_LABEL_WINDOW_MS`].
#[must_use]
pub const fn reacted_within(asked_ms: i64, hand_ms: i64) -> bool {
    hand_ms >= asked_ms && hand_ms - asked_ms <= NOTIFY_LABEL_WINDOW_MS
}

/// Whether the label window of a ring asked at `asked_ms` has closed by
/// `now_ms` — the moment a ring nobody turned to is labeled.
#[must_use]
pub const fn window_closed(asked_ms: i64, now_ms: i64) -> bool {
    now_ms - asked_ms > NOTIFY_LABEL_WINDOW_MS
}

/// The mark the judge counts (§4 of the settings design), from what the
/// person did: a hand on the pane inside the window says the ring was worth
/// the interruption, so the call agreed iff it rang; no hand while the person
/// was present at the window says it was not, so the call agreed iff it did
/// not ring; no hand while they were away says nothing — a person who was
/// not there could not have turned to it — and leaves no mark.
#[must_use]
pub const fn agreed(call: Call, reacted: bool, attendance: Attendance) -> Option<bool> {
    match (reacted, attendance) {
        (true, _) => Some(call.rings()),
        (false, Attendance::Present) => Some(!call.rings()),
        (false, Attendance::Away) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jev::{NOTIFY_OPTIONS, rubric_fingerprint};

    /// The version is pinned to the words: a wording change without a bump
    /// is a red test, not a quiet drift.
    #[test]
    fn the_version_is_pinned_to_the_words() {
        assert_eq!(NOTIFY_CALL_RUBRIC_VERSION, 1);
        assert_eq!(rubric_fingerprint(rubric_words), "82bfdac9ae456c94");
    }

    /// The options are the table's three words, in the table's order, and
    /// each reads back.
    #[test]
    fn the_calls_are_the_tables_words() {
        let words: Vec<&str> = Call::ALL.iter().map(|call| call.word()).collect();
        assert_eq!(words, NOTIFY_OPTIONS);
        for call in Call::ALL {
            assert_eq!(Call::from_word(call.word()), Some(call));
        }
        assert_eq!(Call::from_word("ring"), None);
        assert_eq!(Call::today(), Call::Interrupt);
        assert!(Call::Interrupt.rings());
        assert!(!Call::Batch.rings() && !Call::Ignore.rings());
    }

    fn look<'a>(recent: &'a [Recent]) -> NotifyLook<'a> {
        NotifyLook {
            ring: Ring::Attention,
            interrupted: false,
            agent: "claude",
            pane: "api",
            attendance: Attendance::Present,
            words: "rm -rf build 를 실행할까요?",
            since_last_ms: Some(42_500),
            waiting_panes: 2,
            recent,
        }
    }

    /// The state carries the verb table's words, seconds a person would say,
    /// and the pane's newest rings — never more than the cap, oldest first.
    #[test]
    fn the_question_carries_the_verb_the_attendance_and_the_newest_rings() {
        let recent: Vec<Recent> = (0..NOTIFY_RECENT_CAP + 2)
            .map(|at| Recent {
                ring: if at % 2 == 0 {
                    Ring::Completion
                } else {
                    Ring::Attention
                },
                interrupted: at == 1,
                ago_ms: i64::try_from((NOTIFY_RECENT_CAP + 2 - at) * 10_000).unwrap(),
                reacted: (at % 3 != 0).then_some(at % 2 == 1),
            })
            .collect();
        let asked = ask(&look(&recent));
        assert_eq!(asked.state["event"], "needs input");
        assert_eq!(asked.state["agent"], "claude");
        assert_eq!(asked.state["pane"], "api");
        assert_eq!(asked.state["attendance"], "present");
        assert_eq!(asked.state["words"], "rm -rf build 를 실행할까요?");
        assert_eq!(asked.state["sinceLastSeconds"], 42);
        assert_eq!(asked.state["waitingPanes"], 2);
        let carried = asked.state["recent"].as_array().expect("a list");
        assert_eq!(carried.len(), NOTIFY_RECENT_CAP, "the newest rings only");
        assert_eq!(carried[0]["secondsAgo"], 60, "oldest kept first");
        assert_eq!(carried[NOTIFY_RECENT_CAP - 1]["secondsAgo"], 10);
        // The oldest kept ring is the third (its window closed, no hand); the
        // one after it is still open and says nothing either way.
        assert_eq!(carried[0]["reacted"], false);
        assert_eq!(
            carried[1]["reacted"],
            Value::Null,
            "a window still open says nothing"
        );
        let questions = asked.questions.as_object().expect("questions");
        assert_eq!(questions.len(), 1);
        assert_eq!(asked.questions[QUESTION]["type"], "choice");
        let criteria = asked.questions[QUESTION]["criteria"]
            .as_object()
            .expect("criteria");
        assert_eq!(criteria.len(), Call::ALL.len());

        // A stopped turn wears the verb table's word for it, and a pane that
        // never rang carries no interval.
        let stopped = ask(&NotifyLook {
            ring: Ring::Completion,
            interrupted: true,
            since_last_ms: None,
            ..look(&[])
        });
        assert_eq!(stopped.state["event"], "stopped");
        assert_eq!(stopped.state["sinceLastSeconds"], Value::Null);
        assert_eq!(stopped.state["recent"], json!([]));
    }

    /// The words are cut to one card line here, so the row and the request
    /// hold the same cut.
    #[test]
    fn the_words_are_one_card_line() {
        let long = "가".repeat(NOTIFY_WORDS_CHAR_CAP + 40);
        let asked = ask(&NotifyLook {
            words: &long,
            ..look(&[])
        });
        assert_eq!(
            asked.state["words"]
                .as_str()
                .map(|words| words.chars().count()),
            Some(NOTIFY_WORDS_CHAR_CAP)
        );
    }

    fn answer(chosen: &str, spread: [f64; 3]) -> Value {
        json!({
            QUESTION: {
                "type": "choice",
                "choice": chosen,
                "probabilities": {
                    NOTIFY_INTERRUPT: spread[0],
                    NOTIFY_BATCH: spread[1],
                    NOTIFY_IGNORE: spread[2],
                },
                "confidence": 0.7,
            }
        })
    }

    /// An answer is read under the closed choice's rules, and one broken
    /// rule discards it whole.
    #[test]
    fn an_answer_names_one_of_the_three_or_is_refused_whole() {
        let asked = ask(&look(&[]));
        let read = asked
            .read(&answer(NOTIFY_BATCH, [0.2, 0.7, 0.1]))
            .expect("in shape");
        assert_eq!(read.call, Call::Batch);
        assert_eq!(read.confidence, 0.7);
        assert_eq!(read.probabilities.len(), 3);
        assert_eq!(
            asked.read(&answer("snooze", [0.2, 0.7, 0.1])),
            Err(ChoiceRefusal::UnknownOption)
        );
        assert_eq!(
            asked.read(&answer(NOTIFY_IGNORE, [0.5, 0.5, 0.5])),
            Err(ChoiceRefusal::NotOne)
        );
        assert_eq!(asked.read(&json!({})), Err(ChoiceRefusal::NoAnswer));
    }

    /// Present is focus AND a recent hand; either missing is away.
    #[test]
    fn present_is_a_focused_window_somebody_recently_touched() {
        let now = 1_000_000;
        assert_eq!(
            Attendance::of(true, Some(now - 1_000), now),
            Attendance::Present
        );
        assert_eq!(
            Attendance::of(true, Some(now - NOTIFY_ATTENDANCE_WINDOW_MS), now),
            Attendance::Present,
            "on the line is inside"
        );
        assert_eq!(
            Attendance::of(true, Some(now - NOTIFY_ATTENDANCE_WINDOW_MS - 1), now),
            Attendance::Away
        );
        assert_eq!(Attendance::of(false, Some(now - 1), now), Attendance::Away);
        assert_eq!(Attendance::of(true, None, now), Attendance::Away);
        for attendance in Attendance::ALL {
            assert_eq!(Attendance::from_word(attendance.word()), Some(attendance));
        }
    }

    /// The label window is one minute after the ring, closed on the far
    /// side and open on the near one.
    #[test]
    fn a_hand_inside_the_minute_is_the_reaction_and_after_it_is_not() {
        let asked = 5_000;
        assert!(reacted_within(asked, asked));
        assert!(reacted_within(asked, asked + NOTIFY_LABEL_WINDOW_MS));
        assert!(!reacted_within(asked, asked + NOTIFY_LABEL_WINDOW_MS + 1));
        assert!(
            !reacted_within(asked, asked - 1),
            "a hand before the ring answered nothing"
        );
        assert!(!window_closed(asked, asked + NOTIFY_LABEL_WINDOW_MS));
        assert!(window_closed(asked, asked + NOTIFY_LABEL_WINDOW_MS + 1));
    }

    /// The mark: a hand says the ring was worth it; no hand while present
    /// says it was not; no hand while away says nothing.
    #[test]
    fn the_persons_hand_writes_the_mark_and_an_absent_person_writes_none() {
        for attendance in Attendance::ALL {
            assert_eq!(agreed(Call::Interrupt, true, attendance), Some(true));
            assert_eq!(agreed(Call::Batch, true, attendance), Some(false));
            assert_eq!(agreed(Call::Ignore, true, attendance), Some(false));
        }
        assert_eq!(
            agreed(Call::Interrupt, false, Attendance::Present),
            Some(false)
        );
        assert_eq!(agreed(Call::Batch, false, Attendance::Present), Some(true));
        assert_eq!(agreed(Call::Ignore, false, Attendance::Present), Some(true));
        for call in Call::ALL {
            assert_eq!(agreed(call, false, Attendance::Away), None);
        }
    }
}
