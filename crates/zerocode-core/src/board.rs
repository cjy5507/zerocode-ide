//! The agent board — which column an agent belongs in, and in what order.
//!
//! Orca's `AgentKanbanBoard` (AgentKanbanBoard-CzEPAxZN.js, 1.4.164): four
//! columns, every running agent a card, and the card moves column as the agent's
//! state changes. It opens two ways — beside the sidebar (`in-window`, the
//! default) or as a separate window (`popout`) — behind
//! `experimentalAgentDashboardMode`.
//!
//! ## What is here, and what is in the window
//!
//! The window assembles the cards, because the window is where the tabs, the
//! lanes and the worktree names already live. What is here is every **rule** the
//! board follows:
//!
//! - which column a state means ([`Bucket::of_word`]), from both of this
//!   product's state vocabularies at once,
//! - what order the columns come in ([`BUCKET_ORDER`]) and when `idle` is one of
//!   them,
//! - how cards sort inside a column (most recently changed first),
//! - what the search box matches.
//!
//! Those are the parts that must not drift, and they are the parts a test can
//! pin. Assembly is data-gathering; classification is a rule.
//!
//! ## Two vocabularies, one column
//!
//! An agent's state reaches this window two ways and they are spelled
//! differently: a lane the orchestrator supervises reports [`LaneState`], and an
//! agent in a terminal reports through its hooks as [`HookState`]. Orca has only
//! the first (`bucketForState`, App-BaqTRjaA.js:8362-8370). Both are mapped
//! here, in one function, so a `claude` started by hand and a supervised lane
//! cannot end up in different columns for the same situation.
//!
//! ## The word, and who is still behind it
//!
//! Both of those are the agent SPEAKING, and what it says is only ever as good
//! as its last chance to say it. A process that stops has no last word, so
//! either vocabulary can leave `working` standing over a terminal nobody is
//! waiting on any more. That was t-612: two workers the ledger held at
//! `release_unknown`, their checkouts already removed from disk, drawn as
//! "작업 중" because a hook had once said so.
//!
//! The orchestration ledger is not a third vocabulary — it never says what an
//! agent is DOING, and `active` is no more `working` than it is `waiting`. It
//! answers a different question, the one nothing else here can:
//! [`crate::orchestration::WorkerState::is_live`], is anybody still waiting on
//! that terminal. So it arrives as [`BoardCard::ledger`], beside the state
//! rather than mixed into it, and [`BoardCard::bucket_at`] lets it take a
//! claim away and never add one. The same asymmetry `Run::seen_state` keeps in
//! the ledger itself: the record is what was DECIDED, the table is what IS.
//!
//! It is a verdict, not a clock. [`STALE_AFTER_MS`] guesses from silence and
//! hides what it cannot vouch for; this KNOWS, and says so in the column a
//! person reads endings in.

use serde::{Deserialize, Serialize};

use crate::hook::HookState;
use crate::lane::LaneState;

/// A board column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Bucket {
    /// Stopped, and a person is the only thing that will move it.
    Attention,
    /// A turn is in flight.
    Working,
    /// Finished a turn, or the process ended.
    Done,
    /// Alive with nothing in flight.
    Idle,
}

/// Left to right. Orca's own order (`DASHBOARD_BUCKET_ORDER`,
/// AgentKanbanBoard-CzEPAxZN.js:68-73), and it is the order of decreasing claim
/// on the person looking: what is stuck comes before what is running, which
/// comes before what is finished.
pub const BUCKET_ORDER: [Bucket; 4] = [
    Bucket::Attention,
    Bucket::Working,
    Bucket::Done,
    Bucket::Idle,
];

impl Bucket {
    /// The wire name, which is also the class the window styles by.
    pub const fn slug(self) -> &'static str {
        match self {
            Bucket::Attention => "attention",
            Bucket::Working => "working",
            Bucket::Done => "done",
            Bucket::Idle => "idle",
        }
    }

    /// Which column a lane's state means.
    ///
    /// `AwaitingPermission` and `Blocked` are both attention, which is Orca's
    /// own pairing (`blocked`/`waiting` → `attention`): from the board's point of
    /// view they are the same situation, an agent that will not move until a
    /// person does something. They stay distinct on the card's own dot, because
    /// *what* to do differs.
    ///
    /// `Exited` is done rather than a column of its own. A finished agent and a
    /// dead one both have nothing left to produce, and a fifth column for
    /// "ended" would split the one place a person looks for finished work.
    pub const fn of_lane(state: LaneState) -> Self {
        match state {
            LaneState::Streaming => Bucket::Working,
            LaneState::AwaitingPermission | LaneState::Blocked => Bucket::Attention,
            LaneState::Exited => Bucket::Done,
            LaneState::Idle => Bucket::Idle,
        }
    }

    /// Which column a hook report means.
    pub const fn of_hook(state: HookState) -> Self {
        match state {
            HookState::Working => Bucket::Working,
            HookState::NeedsAttention => Bucket::Attention,
            HookState::Done => Bucket::Done,
            HookState::Idle => Bucket::Idle,
        }
    }

    /// Which column a state WORD means, whichever vocabulary wrote it.
    ///
    /// The window holds its cards' states as the words it already puts on rows
    /// and tabs, so this reads words rather than taking a typed state. Both
    /// vocabularies are in the one table, because `streaming` and `working` are
    /// one column, and `waiting`, `blocked` and `needs-attention` are one column.
    ///
    /// **The word is normalised first**, because the two enums disagree on
    /// casing: [`LaneState`] serialises `snake_case` (`awaiting_permission`) and
    /// [`HookState`] serialises `kebab-case` (`needs-attention`). That
    /// disagreement is worth settling — this crate's own rule is snake_case
    /// everywhere — but not by quietly changing a wire name the window and the
    /// stylesheet already spell out. So this normalises to one shape and the
    /// table is written once. A test pins each enum's real serialisation against
    /// this function, so the day either casing changes, it says so here.
    ///
    /// An unknown word is [`Bucket::Idle`], not a panic and not a fifth column: a
    /// state this function has not met is a state nobody has said needs
    /// attention, and a board is not the place to discover a typo by crashing.
    pub fn of_word(word: &str) -> Self {
        match word.trim().to_lowercase().replace('_', "-").as_str() {
            "streaming" | "working" => Bucket::Working,
            "waiting" | "blocked" | "needs-attention" | "awaiting-permission" => Bucket::Attention,
            "done" | "exited" => Bucket::Done,
            _ => Bucket::Idle,
        }
    }
}

/// The autonomous work zo is pursuing in one pane.
///
/// This mirrors the deliberately small `session_status.autonomy` channel
/// contract. It lives in core because the board grouping round trip must know
/// the field to preserve it; the window does not otherwise depend on zo's
/// implementation crate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardCardAutonomy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<BoardCardGoal>,
    #[serde(default)]
    pub loops: Vec<BoardCardLoop>,
    #[serde(default)]
    pub budget: BoardCardAutonomyBudget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardCardGoal {
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<u64>,
    pub gates_passed: u32,
    pub gates_total: u32,
    pub action_turns: u32,
    pub stalled_turns: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardCardLoop {
    pub id: String,
    pub phase: String,
    pub trigger: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<u64>,
    pub runs: u32,
    pub max_runs: u32,
    pub quiet: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoardCardAutonomyBudget {
    pub continuations: u32,
    pub max_continuations: u32,
    pub assistant_turns: u32,
    pub max_assistant_turns: u32,
    pub output_tokens: u64,
    pub max_output_tokens: u64,
    pub active_millis: u64,
    pub max_wall_clock_secs: u64,
}

/// One agent, as the board shows it.
///
/// Orca's card carries more (`sameCard`, :115) — the last user and agent
/// messages, a review pill, a repo icon. Each of those needs a source this
/// window does not have yet, and each is named in the ledger with what would
/// have to exist first (docs/reverse/orca-ui-inventory.md 1-bd). What is here
/// is what can be filled truthfully. Subagent lineage was on that list until
/// a pane could say which pane started it; it is [`BoardCard::parent`] now,
/// and the shape it makes is [`crate::agent_lineage`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardCard {
    /// What the window opens when the card is clicked. Its own id for the pane —
    /// a term id, or a lane id.
    pub pane: String,
    /// Agent slug, for the mark.
    pub agent: String,
    /// The state word, in either vocabulary. Kept as well as the bucket because
    /// the card's dot says the precise state while the column says the coarse
    /// one — `waiting` and `blocked` share a column and not a dot.
    pub state: String,
    /// What the orchestration ledger last said about this card's WORKER —
    /// [`crate::orchestration::WorkerState`]'s own word, verbatim.
    ///
    /// The second authority, and the only one that can answer the question
    /// [`BoardCard::state`] cannot: whether anybody is still waiting on the
    /// terminal behind the card. A hook says what the agent was doing when it
    /// last managed to speak; the ledger owns the worker's LIFE — it is what
    /// recorded the release nobody answered — so the two disagree exactly when
    /// it matters, and until t-612 the board read only the first of them.
    ///
    /// Empty for every card no ledger row names, which is every agent a person
    /// started by hand and every supervised lane: absence of a verdict, never
    /// a verdict of absence.
    ///
    /// A WORD rather than the typed state, for the reason [`Bucket::of_word`]
    /// reads words — the window holds what the backend handed it. Typed, a
    /// state the ledger grows and this build has not met would fail the whole
    /// card's deserialisation and take the board down with it; as a word, an
    /// unrecognised verdict is simply no verdict.
    #[serde(default)]
    pub ledger: String,
    /// The card's own line: a conversation name if the agent has one, otherwise
    /// the workspace it runs in.
    pub heading: String,
    /// The workspace, which moves to the footer when the heading is a
    /// conversation name — Orca's `worktreeInFooter` (:176).
    #[serde(default)]
    pub worktree: String,
    #[serde(default)]
    pub project: String,
    /// What it was asked to do, when that is known.
    #[serde(default)]
    pub task: String,
    /// The conversation's two lines — Orca's `lastUserMessage` and
    /// `lastAgentMessage` (I18nProvider-4EBrmTGg.js:96432-96442). When either
    /// is present they take the task's spot on the card, the way Orca's do.
    #[serde(default)]
    pub you: String,
    #[serde(default)]
    pub said: String,
    /// The question the agent is stuck on — Orca's `askSummary`, which its
    /// card computes only for the attention bucket (:26400) and draws as the
    /// amber box that replaces the state dot (:96430, :96444).
    #[serde(default)]
    pub ask: String,
    /// The question's full shape when it has one — what the answer panel
    /// draws. Carried through the column grouping untouched: the columns
    /// read the summary, and dropping this here would strip the card of its
    /// choices on the way to the screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_prompt: Option<crate::ask::AskPrompt>,
    /// The tool waiting on permission, carried the same way for the same
    /// reason — the card's Allow and Deny are drawn from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<crate::ask::ApprovalPrompt>,
    /// Which pane started this one, spelled the way [`BoardCard::pane`] is —
    /// so a parent is found by looking for a card that calls itself this.
    /// Empty when nobody did, which is every agent a person opened.
    ///
    /// Orca reaches the same fact three ways (`resolveAgentRowParentPaneKey`,
    /// useWorktreeAgentRows-BZzQmOQU.js:14-30) because it has three places
    /// that record it. This window has one, so this is one field, and an
    /// answer that names nobody in the list is handled where every other bad
    /// edge is — in [`crate::agent_lineage`], by promotion, never by dropping
    /// the row.
    #[serde(default)]
    pub parent: String,
    /// Where this card sits in its column's tree.
    ///
    /// **Filled by [`columns`], never by the caller.** It arrives from the
    /// window as the default — a row standing alone — and is overwritten with
    /// the answer, because the tree is a fact about a card's neighbours and
    /// only the grouping has all of them.
    #[serde(default)]
    pub lineage: crate::agent_lineage::Lineage,
    /// Nobody has looked at this since it last changed.
    #[serde(default)]
    pub unseen: bool,
    /// When the state last changed, in milliseconds. The sort key.
    #[serde(default)]
    pub changed_at: i64,
    /// What the footer's relative time counts from.
    #[serde(default)]
    pub at: i64,
    /// Goal/loop status reported by zo. Absent for every other agent and for
    /// older snapshots, which is why this is an optional forward-compatible
    /// board fact rather than another lifecycle state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy: Option<BoardCardAutonomy>,
}

/// How long a state stays believed without a fresh report.
///
/// `AGENT_STATUS_STALE_AFTER_MS` (I18nProvider-4EBrmTGg.js:50363). Half an
/// hour of silence from a "working" agent means the process died, the hook
/// broke, or the machine slept — and a board that keeps saying "working" about
/// it is telling the person their work is progressing when nobody knows.
///
/// Left at Orca's measurement rather than shortened, because shortening it
/// answers a different question badly. This is the guess for a card NOBODY
/// speaks for; where somebody does — [`BoardCard::ledger`] — the answer is
/// read instead of estimated, and no length of clock could have reached the
/// cards that started t-612 anyway: a pane that never sent a hook carries
/// `changed_at == 0`, which never decays at all.
pub const STALE_AFTER_MS: i64 = 30 * 60 * 1000;

impl BoardCardAutonomy {
    pub fn paused(&self) -> bool {
        self.goal
            .as_ref()
            .is_some_and(|goal| goal.phase == "paused")
            || self.loops.iter().any(|one| one.phase == "paused")
    }

    pub fn running(&self) -> bool {
        self.goal
            .as_ref()
            .is_some_and(|goal| goal.phase == "running")
            || self.loops.iter().any(|one| one.phase == "running")
    }
}

impl BoardCard {
    pub fn bucket(&self) -> Bucket {
        Bucket::of_word(&self.state)
    }

    /// Has the authority that owns this card's terminal ended its run?
    ///
    /// [`crate::orchestration::WorkerState::is_live`] is the ledger's own
    /// answer to "does this terminal still hold a process worth reading",
    /// which is word for word what the board is asking. Asking it there
    /// rather than listing the finished words here keeps ONE table of which
    /// states are over, in the module that owns them — the day the ledger
    /// grows a state, nothing here has to be told.
    ///
    /// A card with no ledger row, and one carrying a word this build does not
    /// know, are both un-retired. Silence is not a verdict, and the direction
    /// every branch on this surface leans is toward drawing what is there.
    fn retired(&self) -> bool {
        self.ledger
            .parse::<crate::orchestration::WorkerState>()
            .is_ok_and(|state| !state.is_live())
    }

    /// The column, with the clock's — and the person's — opinion.
    ///
    /// A finished card somebody has SEEN settles into [`Bucket::Idle`]: the
    /// bucket is Orca's DISPLAY state, not the raw one
    /// (`dashboardCardDisplayState` reads done-and-not-unseen as idle,
    /// dashboard-snapshot.ts:38-44, and the bucket takes that reading,
    /// build-dashboard-snapshot.ts:236-240). Bucketing the raw state left
    /// every acknowledged run standing in Done, and the column grew into a
    /// receipt pile — the map's P0-5. An UNSEEN done never decays: finished
    /// work waits in Done for the person however long ago it finished.
    ///
    /// A **non-done** card whose last word is older than [`STALE_AFTER_MS`]
    /// buckets as [`Bucket::Idle`] — Orca's own decay, applied to non-done rows
    /// only (index-ftls8Hg_.js:25808-25843).
    ///
    /// A card with no timestamp does not decay either — `changed_at == 0`
    /// means "clock unknown", and measured against `now` it would be stale the
    /// moment it appeared.
    ///
    /// A card whose worker the LEDGER has retired ([`BoardCard::ledger`])
    /// stops claiming work in flight. That rule runs first and only ever
    /// downward: it moves a card that says `working` or `waiting` into
    /// [`Bucket::Done`] — the column whose own word for this is "the process
    /// ended" — and touches nothing else, so a verdict can take a claim away
    /// and never invent one.
    ///
    /// The two rules below are skipped for it, each for its own reason.
    /// `unseen` says whether the person has seen this card's own last
    /// REPORT; the ledger's verdict is not that report, so settling on that
    /// flag would hide the ending in the same beat it arrived — and a board
    /// that hides an ending is the same lie as one that hides a death. The
    /// clock is skipped because decay is what this does when NOBODY knows,
    /// and here somebody does: the pile that leaves is bounded by the panes
    /// still open, and a retired worker's open pane is exactly the thing a
    /// person is meant to see and close.
    ///
    /// Autonomy ([`BoardCard::autonomy`]) speaks only for an un-retired card,
    /// and only about turns: a RUNNING goal or loop keeps a `done` turn in
    /// [`Bucket::Working`], because the continuation is the work. A PAUSED
    /// goal or loop is not a question — nobody is being asked anything, and
    /// the pause is the autonomy badge's fact, not the column's — so it never
    /// reaches [`Bucket::Attention`]. A turn still in flight keeps its own
    /// word; a finished turn with a pause behind it settles into
    /// [`Bucket::Idle`] whether or not it was seen, since what is unfinished
    /// there is the goal, and the goal is carried on the card.
    pub fn bucket_at(&self, now_ms: i64) -> Bucket {
        let plain = self.bucket();
        if matches!(plain, Bucket::Working | Bucket::Attention) && self.retired() {
            return Bucket::Done;
        }
        // A continuation outlives a turn, not its process or a question.
        if !self.retired()
            && matches!(self.state.as_str(), "working" | "streaming" | "done")
            && self
                .autonomy
                .as_ref()
                .is_some_and(BoardCardAutonomy::running)
        {
            return Bucket::Working;
        }
        // A pause is not a question. The turn is over and the autonomy badge
        // says what stopped; a card in Attention here would sit in front of
        // the agents actually working, wearing the mark of one that is asking.
        if !self.retired()
            && self.state == "done"
            && self
                .autonomy
                .as_ref()
                .is_some_and(BoardCardAutonomy::paused)
        {
            return Bucket::Idle;
        }
        if plain == Bucket::Done && !self.unseen {
            return Bucket::Idle;
        }
        let aged = self.changed_at > 0 && now_ms.saturating_sub(self.changed_at) > STALE_AFTER_MS;
        if aged && plain != Bucket::Done {
            return Bucket::Idle;
        }
        plain
    }

    /// The text the search box looks in.
    ///
    /// Orca searches the same fields (`cardSearchText`) and lowercases both
    /// sides — including the conversation's two lines and the standing
    /// question, so a word the agent JUST SAID finds its card. Two of Orca's
    /// entries have no line here by design: subagent names, because ours are
    /// cards of their own and match by their own heading; and `#review`,
    /// because the review is the window's fact and this crate never hears it
    /// (기록된 이탈, 1-g78b's ownership rule). Assembled per query rather
    /// than stored on the card: a board holds tens of cards, and a stored
    /// copy is a second thing to keep true.
    fn haystack(&self) -> String {
        [
            self.heading.as_str(),
            self.worktree.as_str(),
            self.project.as_str(),
            self.task.as_str(),
            self.agent.as_str(),
            self.you.as_str(),
            self.said.as_str(),
            self.ask.as_str(),
        ]
        .join("\u{1f}")
        .to_lowercase()
    }

    /// Does this card answer `query`?
    ///
    /// An empty query matches everything, which is what makes the filter one
    /// path instead of two.
    pub fn matches(&self, query: &str) -> bool {
        let wanted = query.trim().to_lowercase();
        wanted.is_empty() || self.haystack().contains(&wanted)
    }
}

/// One column, filled.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardColumn {
    pub bucket: Bucket,
    pub cards: Vec<BoardCard>,
}

/// What the board asks for.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BoardQuery {
    /// The search box.
    #[serde(default)]
    pub query: String,
    /// Project paths to keep. Empty means every project — Orca's own convention
    /// for these (`filters.projects.length === 0 || …`), and the right one: an
    /// empty filter list is "no filter", never "nothing".
    #[serde(default)]
    pub projects: Vec<String>,
    /// Whether `idle` is a column at all. Orca hides it unless
    /// `experimentalAgentDashboardShowIdle` (:1280) — an idle agent is not work
    /// in progress, and a fourth column of them buries the three that matter.
    #[serde(default)]
    pub show_idle: bool,
}

/// One classified board snapshot.
///
/// The window needs the filtered cards for its surface and the unfiltered
/// attention count for the closed-door badge at the same moment. Returning
/// both from one pass keeps those two surfaces on one clock and prevents a
/// hook beat from classifying every card twice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoardSnapshot {
    pub columns: Vec<BoardColumn>,
    pub attention_count: usize,
    pub total_count: usize,
}

/// Group cards into the columns the board draws.
///
/// Every column in [`BUCKET_ORDER`] comes back, empty ones included: a board
/// whose columns appear and disappear as work moves is a board whose layout
/// jumps under the pointer. Orca draws the empty ones too, with `None` in them.
pub fn columns(cards: &[BoardCard], query: &BoardQuery, now_ms: i64) -> Vec<BoardColumn> {
    snapshot(cards, query, now_ms).columns
}

/// Build the window's complete board answer in one classification pass.
///
/// Search and project filters affect `columns` but never the attention badge.
/// The badge is an interruption signal, so narrowing the open surface cannot
/// hide somebody who is waiting for the person.
pub fn snapshot(cards: &[BoardCard], query: &BoardQuery, now_ms: i64) -> BoardSnapshot {
    let mut attention_count = 0;
    let mut grouped: std::collections::HashMap<Bucket, Vec<&BoardCard>> = BUCKET_ORDER
        .into_iter()
        .map(|bucket| (bucket, Vec::new()))
        .collect();

    for card in cards {
        let bucket = card.bucket_at(now_ms);
        if bucket == Bucket::Attention {
            attention_count += 1;
        }
        if !card.matches(&query.query)
            || (!query.projects.is_empty() && !query.projects.contains(&card.project))
        {
            continue;
        }
        if let Some(rows) = grouped.get_mut(&bucket) {
            rows.push(card);
        }
    }

    let columns = BUCKET_ORDER
        .iter()
        .filter(|bucket| **bucket != Bucket::Idle || query.show_idle)
        .map(|bucket| {
            let mut chosen = grouped.remove(bucket).unwrap_or_default();
            chosen.sort_by(|left, right| {
                right
                    .changed_at
                    .cmp(&left.changed_at)
                    .then_with(|| left.pane.cmp(&right.pane))
            });
            let rows: Vec<crate::agent_lineage::Row<'_>> = chosen
                .iter()
                .map(|card| crate::agent_lineage::Row {
                    key: card.pane.as_str(),
                    parent: card.parent.as_str(),
                })
                .collect();
            let cards = crate::agent_lineage::arrange(&rows)
                .into_iter()
                .map(|placed| {
                    let mut card = chosen[placed.index].clone();
                    card.lineage = placed.lineage;
                    card
                })
                .collect();
            BoardColumn {
                bucket: *bucket,
                cards,
            }
        })
        .collect();

    BoardSnapshot {
        columns,
        attention_count,
        total_count: cards.len(),
    }
}

/// How many cards want a person, for the badge on the board's own door.
///
/// Same clock as [`columns`], so the number on the door and the length of the
/// column behind it cannot disagree about a stale card.
pub fn attention_count(cards: &[BoardCard], now_ms: i64) -> usize {
    cards
        .iter()
        .filter(|card| card.bucket_at(now_ms) == Bucket::Attention)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(pane: &str, state: &str, changed_at: i64) -> BoardCard {
        BoardCard {
            pane: pane.into(),
            agent: "claude".into(),
            state: state.into(),
            ledger: String::new(),
            heading: pane.into(),
            worktree: "main".into(),
            project: "/p".into(),
            task: String::new(),
            you: String::new(),
            said: String::new(),
            ask: String::new(),
            ask_prompt: None,
            approval: None,
            parent: String::new(),
            lineage: crate::agent_lineage::Lineage::default(),
            unseen: false,
            changed_at,
            at: changed_at,
            autonomy: None,
        }
    }

    #[test]
    fn autonomy_keeps_a_continuation_visible_but_not_a_dead_or_waiting_process() {
        let mut held = card("term:1", "done", 1);
        held.autonomy = Some(
            serde_json::from_value(serde_json::json!({
                "goal": { "phase": "running", "next_at": 9_999_999,
                    "gates_passed": 0, "gates_total": 1, "action_turns": 1, "stalled_turns": 0 }
            }))
            .unwrap(),
        );
        let now = STALE_AFTER_MS + 2;
        assert_eq!(held.bucket_at(now), Bucket::Working);
        held.state = "needs-attention".into();
        held.changed_at = now;
        assert_eq!(held.bucket_at(now), Bucket::Attention);
        for state in ["idle", "exited", "error", "failed"] {
            held.state = state.into();
            assert_ne!(held.bucket_at(now), Bucket::Working, "{state}");
        }
        held.state = "working".into();
        for ledger in ["released", "release_unknown", "sleeping"] {
            held.ledger = ledger.into();
            assert_eq!(held.bucket_at(now), Bucket::Done, "{ledger}");
        }
        held.ledger.clear();
        held.state = "done".into();
        let autonomy = held.autonomy.as_mut().unwrap();
        autonomy.goal.as_mut().unwrap().phase = "paused".into();
        autonomy.goal.as_mut().unwrap().pause_reason = Some("permission required".into());
        assert_eq!(
            held.bucket_at(now),
            Bucket::Idle,
            "a paused goal is not a question, whatever it was paused for"
        );
        held.autonomy.as_mut().unwrap().loops.push(BoardCardLoop {
            id: "loop-1".into(),
            phase: "running".into(),
            trigger: "every 600s".into(),
            next_at: Some(9_999_999),
            runs: 1,
            max_runs: 10,
            quiet: 0,
            reason: None,
        });
        assert_eq!(held.bucket_at(now), Bucket::Working);
        let wire = serde_json::to_value(&held).unwrap();
        assert_eq!(
            wire["autonomy"]["goal"]["pause_reason"],
            "permission required"
        );
        let restored: BoardCard = serde_json::from_value(wire).unwrap();
        assert_eq!(restored, held);
        held.autonomy = None;
        assert_eq!(held.bucket_at(now), Bucket::Idle);
    }

    /// A `/goal` that stopped short — budget spent, gates not yet passed.
    fn paused_goal() -> BoardCardAutonomy {
        BoardCardAutonomy {
            goal: Some(BoardCardGoal {
                phase: "paused".into(),
                next_at: None,
                gates_passed: 1,
                gates_total: 3,
                action_turns: 4,
                stalled_turns: 0,
                pause_reason: Some("continuations: 8/8".into()),
            }),
            ..BoardCardAutonomy::default()
        }
    }

    /// A `/loop` that stopped short — quota wall, quiet runs, whichever.
    fn paused_loop() -> BoardCardAutonomy {
        BoardCardAutonomy {
            loops: vec![BoardCardLoop {
                id: "loop-1".into(),
                phase: "paused".into(),
                trigger: "every 600s".into(),
                next_at: None,
                runs: 3,
                max_runs: 10,
                quiet: 1,
                reason: Some("quota wall".into()),
            }],
            ..BoardCardAutonomy::default()
        }
    }

    /// A paused goal or loop is a pause, not a question.
    ///
    /// A `/goal` that ran out of continuations left its card in Attention —
    /// drawn with the `?` the column keeps for an agent that is asking, and
    /// stacked ahead of the agents actually working. Nobody had asked
    /// anything. The pause is the autonomy badge's to say; the column only
    /// says whether a turn is in flight, over, or stuck on a person.
    #[test]
    fn a_paused_goal_or_loop_is_not_a_question() {
        let now = 100_000_000;
        let fresh = now - 60_000;
        let stale = now - 31 * 60 * 1000;
        let both = BoardCardAutonomy {
            goal: paused_goal().goal,
            ..paused_loop()
        };
        for (name, autonomy) in [
            ("goal", paused_goal()),
            ("loop", paused_loop()),
            ("both", both),
        ] {
            assert!(autonomy.paused() && !autonomy.running(), "{name}");
            let held = |state: &str| BoardCard {
                autonomy: Some(autonomy.clone()),
                ..card("term:7", state, fresh)
            };

            // A turn still in flight keeps its column: a pause says what
            // happens after the turn, not whether it is running.
            for said in ["working", "streaming"] {
                assert_eq!(held(said).bucket_at(now), Bucket::Working, "{name}/{said}");
            }
            // A real question is still a question — the state word carries
            // it, and the pause neither adds nor removes one.
            for said in ["needs-attention", "waiting", "blocked"] {
                assert_eq!(
                    held(said).bucket_at(now),
                    Bucket::Attention,
                    "{name}/{said}"
                );
            }
            // The finished turn settles, seen or not. What is unfinished is
            // the goal, and the goal rides on the card for the badge to draw.
            assert_eq!(held("done").bucket_at(now), Bucket::Idle, "{name}");
            let unread = BoardCard {
                unseen: true,
                ..held("done")
            };
            assert_eq!(unread.bucket_at(now), Bucket::Idle, "{name}/unseen");
            let wire = serde_json::to_value(&unread).expect("a paused card serialises");
            assert_eq!(wire["autonomy"], serde_json::to_value(&autonomy).unwrap());
            let restored: BoardCard = serde_json::from_value(wire).unwrap();
            assert_eq!(restored, unread, "{name}: the pause was lost on the wire");

            // The ledger and the clock outrank a pause exactly as they
            // outrank the card's own word.
            assert_eq!(
                BoardCard {
                    ledger: "release_unknown".into(),
                    ..held("working")
                }
                .bucket_at(now),
                Bucket::Done,
                "{name}/retired"
            );
            assert_eq!(
                BoardCard {
                    ledger: "released".into(),
                    ..unread.clone()
                }
                .bucket_at(now),
                Bucket::Done,
                "{name}/retired-unseen: the ending was hidden"
            );
            assert_eq!(
                BoardCard {
                    changed_at: stale,
                    ..held("working")
                }
                .bucket_at(now),
                Bucket::Idle,
                "{name}/stale"
            );
            // `exited` and `idle` never hear about a pause at all.
            assert_eq!(
                BoardCard {
                    unseen: true,
                    ..held("exited")
                }
                .bucket_at(now),
                Bucket::Done,
                "{name}/exited"
            );
            assert_eq!(held("exited").bucket_at(now), Bucket::Idle, "{name}");
            assert_eq!(held("idle").bucket_at(now), Bucket::Idle, "{name}");
        }

        // A pause beside something still running is not a pause.
        let mut mixed = BoardCard {
            autonomy: Some(paused_goal()),
            unseen: true,
            ..card("term:7", "done", fresh)
        };
        mixed.autonomy.as_mut().unwrap().loops = paused_loop().loops;
        mixed.autonomy.as_mut().unwrap().loops[0].phase = "running".into();
        assert_eq!(
            mixed.bucket_at(now),
            Bucket::Working,
            "a running loop lost to a paused goal"
        );
        mixed.autonomy.as_mut().unwrap().loops[0].phase = "paused".into();
        mixed
            .autonomy
            .as_mut()
            .unwrap()
            .goal
            .as_mut()
            .unwrap()
            .phase = "running".into();
        assert_eq!(
            mixed.bucket_at(now),
            Bucket::Working,
            "a running goal lost to a paused loop"
        );

        // The badge rings for the question and not for the pause, and the
        // paused card does not stand in front of the one that is working.
        let held = [
            BoardCard {
                autonomy: Some(paused_goal()),
                unseen: true,
                ..card("paused", "done", fresh)
            },
            BoardCard {
                autonomy: Some(paused_loop()),
                ..card("busy", "working", fresh)
            },
            card("asking", "waiting", fresh),
        ];
        let answer = snapshot(&held, &BoardQuery::default(), now);
        assert_eq!(answer.attention_count, 1, "the pause rang the badge");
        assert_eq!(attention_count(&held, now), 1);
        assert_eq!(answer.total_count, 3);
        assert_eq!(
            answer
                .columns
                .iter()
                .map(|column| (
                    column.bucket.slug(),
                    column
                        .cards
                        .iter()
                        .map(|one| one.pane.as_str())
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("attention", vec!["asking"]),
                ("working", vec!["busy"]),
                ("done", vec![]),
            ]
        );
        // And it is still on the board when idle is drawn, pause intact.
        let with_idle = snapshot(
            &held,
            &BoardQuery {
                show_idle: true,
                ..BoardQuery::default()
            },
            now,
        );
        let parked = &with_idle.columns[3];
        assert_eq!(parked.bucket, Bucket::Idle);
        assert_eq!(
            parked
                .cards
                .iter()
                .map(|one| one.pane.as_str())
                .collect::<Vec<_>>(),
            ["paused"]
        );
        assert!(
            parked.cards[0]
                .autonomy
                .as_ref()
                .is_some_and(BoardCardAutonomy::paused)
        );
    }

    /// The same card, started by another pane.
    fn child(pane: &str, state: &str, changed_at: i64, parent: &str) -> BoardCard {
        BoardCard {
            parent: parent.into(),
            ..card(pane, state, changed_at)
        }
    }

    /// Both vocabularies land in the same column for the same situation. This is
    /// the whole reason the mapping is one function: a lane says `streaming` and
    /// a hook says `working`, and they are one thing.
    #[test]
    fn a_lane_and_a_hook_agree_on_the_column() {
        assert_eq!(
            Bucket::of_lane(LaneState::Streaming),
            Bucket::of_hook(HookState::Working)
        );
        assert_eq!(
            Bucket::of_lane(LaneState::AwaitingPermission),
            Bucket::of_hook(HookState::NeedsAttention)
        );
        assert_eq!(
            Bucket::of_lane(LaneState::Blocked),
            Bucket::of_hook(HookState::NeedsAttention)
        );
        assert_eq!(
            Bucket::of_lane(LaneState::Exited),
            Bucket::of_hook(HookState::Done)
        );
        assert_eq!(Bucket::of_lane(LaneState::Idle), Bucket::Idle);
        // And the state nobody sends: a pane whose agent left is the same
        // column as a lane that has nothing running in it.
        assert_eq!(
            Bucket::of_lane(LaneState::Idle),
            Bucket::of_hook(HookState::Idle)
        );

        // And the word form agrees with both, for every word either half writes.
        for (word, bucket) in [
            ("streaming", Bucket::Working),
            ("working", Bucket::Working),
            ("waiting", Bucket::Attention),
            ("blocked", Bucket::Attention),
            ("needs-attention", Bucket::Attention),
            // Both casings of the same state, which is the disagreement
            // `of_word` normalises away.
            ("awaiting-permission", Bucket::Attention),
            ("awaiting_permission", Bucket::Attention),
            ("done", Bucket::Done),
            ("exited", Bucket::Done),
            ("idle", Bucket::Idle),
        ] {
            assert_eq!(Bucket::of_word(word), bucket, "{word}");
        }
        // The hook and lane slugs themselves must be words this reads — the two
        // enums serialise into the very strings the window holds.
        for state in [
            HookState::Working,
            HookState::NeedsAttention,
            HookState::Done,
            HookState::Idle,
        ] {
            let word = serde_json::to_value(state).expect("a hook state serialises");
            assert_eq!(
                Bucket::of_word(word.as_str().unwrap_or_default()),
                Bucket::of_hook(state),
                "the word form disagrees with the typed form for {state:?}"
            );
        }
        for state in [
            LaneState::Idle,
            LaneState::Streaming,
            LaneState::AwaitingPermission,
            LaneState::Blocked,
            LaneState::Exited,
        ] {
            let word = serde_json::to_value(state).expect("a lane state serialises");
            assert_eq!(
                Bucket::of_word(word.as_str().unwrap_or_default()),
                Bucket::of_lane(state),
                "the word form disagrees with the typed form for {state:?}"
            );
        }
    }

    /// An unknown word is idle, not a crash and not a column of its own.
    #[test]
    fn a_state_nobody_taught_this_is_idle() {
        assert_eq!(Bucket::of_word("hibernating"), Bucket::Idle);
        assert_eq!(Bucket::of_word(""), Bucket::Idle);
        // And it does not vanish from the board — it is a card in a column.
        let held = [card("a", "hibernating", 1)];
        let drawn = columns(
            &held,
            &BoardQuery {
                show_idle: true,
                ..Default::default()
            },
            0,
        );
        assert_eq!(drawn.last().expect("a column").cards.len(), 1);
    }

    /// Every column is drawn, in order, empty ones included — a board whose
    /// columns come and go moves the others under the pointer.
    #[test]
    fn the_columns_are_always_the_same_columns() {
        let drawn = columns(&[], &BoardQuery::default(), 0);
        assert_eq!(
            drawn.iter().map(|one| one.bucket).collect::<Vec<_>>(),
            vec![Bucket::Attention, Bucket::Working, Bucket::Done],
            "idle is drawn without being asked for"
        );
        assert!(drawn.iter().all(|one| one.cards.is_empty()));

        let with_idle = columns(
            &[],
            &BoardQuery {
                show_idle: true,
                ..Default::default()
            },
            0,
        );
        assert_eq!(with_idle.len(), 4);
        assert_eq!(with_idle[3].bucket, Bucket::Idle);
    }

    /// Inside a column, the most recently changed card is first, and two cards
    /// that changed in the same millisecond keep a stable order.
    #[test]
    fn a_column_puts_the_freshest_first_and_never_flickers() {
        let held = [
            card("term-1", "working", 100),
            card("term-2", "working", 300),
            card("term-3", "working", 300),
        ];
        let working = columns(&held, &BoardQuery::default(), 0)
            .into_iter()
            .find(|one| one.bucket == Bucket::Working)
            .expect("a working column");
        assert_eq!(
            working
                .cards
                .iter()
                .map(|one| one.pane.as_str())
                .collect::<Vec<_>>(),
            vec!["term-2", "term-3", "term-1"]
        );
        // Reversed input, same output: the tie is broken by something stable
        // rather than by which card happened to be gathered first.
        let flipped = [
            card("term-3", "working", 300),
            card("term-2", "working", 300),
            card("term-1", "working", 100),
        ];
        let again = columns(&flipped, &BoardQuery::default(), 0)
            .into_iter()
            .find(|one| one.bucket == Bucket::Working)
            .expect("a working column");
        assert_eq!(
            again.cards, working.cards,
            "the order depends on input order"
        );
    }

    /// Search reads the fields Orca's does, case-insensitively, and an empty
    /// query is not a filter.
    #[test]
    fn search_looks_where_the_words_are() {
        let mut held = card("term-1", "idle", 1);
        held.heading = "Rewrite the parser".into();
        held.worktree = "feature/parse".into();
        held.project = "/repos/zerocode".into();
        held.task = "make it read TOML".into();
        held.agent = "codex".into();
        held.you = "머지 충돌 정리해줘".into();
        held.said = "the fixture drifted from the default".into();
        held.ask = "overwrite the lockfile?".into();

        for query in [
            "", "  ", "REWRITE", "parse", "zerocode", "toml", "codex", "충돌", "drifted",
            "lockfile",
        ] {
            assert!(held.matches(query), "{query:?} did not match");
        }
        // The joined fields must not let a match straddle two of them: "parser"
        // ends one field and "feature" begins the next.
        assert!(!held.matches("parserfeature"));
    }

    /// An empty project filter means every project, never none.
    #[test]
    fn an_empty_filter_is_not_a_filter() {
        let held = [card("a", "working", 1)];
        let all = columns(&held, &BoardQuery::default(), 0);
        assert_eq!(all.iter().map(|one| one.cards.len()).sum::<usize>(), 1);

        let kept = columns(
            &held,
            &BoardQuery {
                projects: vec!["/p".into()],
                ..Default::default()
            },
            0,
        );
        assert_eq!(kept.iter().map(|one| one.cards.len()).sum::<usize>(), 1);

        let dropped = columns(
            &held,
            &BoardQuery {
                projects: vec!["/elsewhere".into()],
                ..Default::default()
            },
            0,
        );
        assert_eq!(dropped.iter().map(|one| one.cards.len()).sum::<usize>(), 0);
    }

    /// A hidden idle column hides its cards too — otherwise they would fall into
    /// whichever column was drawn last.
    #[test]
    fn hiding_idle_hides_its_cards_rather_than_moving_them() {
        let held = [card("a", "idle", 1), card("b", "working", 2)];
        let drawn = columns(&held, &BoardQuery::default(), 0);
        assert_eq!(drawn.iter().map(|one| one.cards.len()).sum::<usize>(), 1);
        assert!(
            drawn
                .iter()
                .all(|one| one.cards.iter().all(|card| card.pane != "a")),
            "an idle card landed in a visible column: {drawn:?}"
        );
    }

    /// Half an hour of silence and a card stops claiming its state. A board
    /// that says "working" about a process that died at lunch is telling the
    /// person their work is progressing when nobody knows — and `done` never
    /// decays, because finished work stays finished however long ago.
    #[test]
    fn a_silent_half_hour_reads_as_idle() {
        // Literal milliseconds, never derived from the constant: a boundary
        // test whose fixtures scale with the threshold cannot notice the
        // threshold slipping a unit.
        let now = 100_000_000;
        let fresh = now - 29 * 60 * 1000; // 29 minutes ago
        let stale = now - 31 * 60 * 1000; // 31 minutes ago

        // The boundary, from both sides.
        assert_eq!(card("a", "working", fresh).bucket_at(now), Bucket::Working);
        assert_eq!(card("a", "working", stale).bucket_at(now), Bucket::Idle);
        // Exactly at the threshold is not yet past it.
        assert_eq!(
            card("a", "working", now - 30 * 60 * 1000).bucket_at(now),
            Bucket::Working
        );

        // Attention decays too — Orca decays every non-done state. A question
        // asked half an hour ago with nobody there is not a live question.
        assert_eq!(card("b", "waiting", stale).bucket_at(now), Bucket::Idle);
        assert_eq!(card("b", "blocked", stale).bucket_at(now), Bucket::Idle);

        // An UNSEEN done never decays — finished work waits in Done for the
        // person, however long ago it finished.
        let unread = |state: &str| BoardCard {
            unseen: true,
            ..card("c", state, stale)
        };
        assert_eq!(unread("done").bucket_at(now), Bucket::Done);
        assert_eq!(unread("exited").bucket_at(now), Bucket::Done);
        // A SEEN done settles into idle at once, fresh or stale: the bucket
        // is Orca's display state (done and not unseen reads as idle), so an
        // acknowledged run sails instead of piling up in Done.
        assert_eq!(card("c", "done", stale).bucket_at(now), Bucket::Idle);
        assert_eq!(card("c", "done", fresh).bucket_at(now), Bucket::Idle);
        assert_eq!(card("c", "exited", fresh).bucket_at(now), Bucket::Idle);

        // No clock, no decay: zero means "unknown", and measured against now
        // it would be stale the moment it appeared.
        assert_eq!(card("d", "working", 0).bucket_at(now), Bucket::Working);

        // The column grouping and the badge read the same clock.
        let held = [
            card("live", "working", fresh),
            card("quiet", "working", stale),
            card("stuck", "waiting", stale),
        ];
        let drawn = columns(&held, &BoardQuery::default(), now);
        let working: Vec<&str> = drawn
            .iter()
            .find(|one| one.bucket == Bucket::Working)
            .expect("working column")
            .cards
            .iter()
            .map(|one| one.pane.as_str())
            .collect();
        assert_eq!(working, ["live"], "the stale card kept its claim");
        assert_eq!(
            attention_count(&held, now),
            0,
            "a stale question still rang the badge"
        );
    }

    /// The second authority, and the bug it was written for.
    ///
    /// Two `antigravity` workers sat in the ledger at `release_unknown` with
    /// their checkouts removed from disk, and the board drew both of them as
    /// "작업 중" — because the only thing it read was the word their hooks had
    /// last sent. The ledger knew, and had known all along
    /// ([`crate::orchestration::WorkerState::is_live`]); nothing carried the
    /// answer here (t-612).
    #[test]
    fn a_worker_the_ledger_retired_stops_claiming_work() {
        let now = 100_000_000;
        let fresh = now - 60_000;
        let held = |state: &str, word: &str| BoardCard {
            ledger: word.into(),
            ..card("term:9", state, fresh)
        };

        // The measured shape: a fresh `working` report from a worker whose
        // release was asked for and never answered.
        assert_eq!(
            held("working", "release_unknown").bucket_at(now),
            Bucket::Done
        );
        // Every word the ledger has for a terminal that is over.
        for word in ["release_pending", "release_unknown", "released", "sleeping"] {
            for said in ["working", "streaming"] {
                assert_eq!(
                    held(said, word).bucket_at(now),
                    Bucket::Done,
                    "{word}/{said}"
                );
            }
            // And a question nobody is left to answer does not want a person.
            for said in ["waiting", "blocked", "needs-attention"] {
                assert_eq!(
                    held(said, word).bucket_at(now),
                    Bucket::Done,
                    "{word}/{said}"
                );
            }
        }
        // Every word for a terminal that still holds a process leaves the
        // card's own report exactly where it was: the verdict takes a claim
        // away and never adds one. `orphaned` is among them on purpose: a
        // leader exiting took the road home, not the process, and drawing
        // that card as finished is the same lie in the other direction.
        for word in ["active", "reclaimable", "retained", "orphaned"] {
            assert_eq!(
                held("working", word).bucket_at(now),
                Bucket::Working,
                "{word}"
            );
            assert_eq!(
                held("waiting", word).bucket_at(now),
                Bucket::Attention,
                "{word}"
            );
        }
        // No row at all, and a word this build has never met, are both
        // silence — and silence is not a verdict.
        assert_eq!(held("working", "").bucket_at(now), Bucket::Working);
        assert_eq!(
            held("working", "hibernating").bucket_at(now),
            Bucket::Working
        );
    }

    /// A retired worker is DRAWN as ended, never quietly dropped.
    ///
    /// "Nobody is working on this" and "this was never here" are different
    /// sentences and only one of them is true. `done` is the column a person
    /// looks in for work that stopped, and it is drawn whether or not `idle`
    /// was asked for — which is the whole reason the verdict lands there and
    /// not in `idle`, where the default board would have swallowed it.
    #[test]
    fn a_retired_worker_is_drawn_rather_than_hidden() {
        let now = 100_000_000;
        let dead = BoardCard {
            ledger: "release_unknown".into(),
            ..card("term:9", "working", now - 60_000)
        };
        // Looked at once already, while it still said `working`. That is the
        // flag which settles a REPORTED `done` into the hidden idle column,
        // and the one that must not settle a verdict the person has not seen.
        assert!(!dead.unseen);
        let live = card("term:12", "working", now - 60_000);

        let drawn = columns(&[dead.clone(), live], &BoardQuery::default(), now);
        assert_eq!(
            drawn
                .iter()
                .map(|column| (
                    column.bucket.slug(),
                    column
                        .cards
                        .iter()
                        .map(|one| one.pane.as_str())
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("attention", vec![]),
                ("working", vec!["term:12"]),
                ("done", vec!["term:9"]),
            ],
            "the ended worker is not where a person would look for it"
        );
        // And it does not ring the badge on the way there.
        assert_eq!(attention_count(&[dead], now), 0);
    }

    /// The verdict takes a claim away and never gives one back.
    #[test]
    fn a_verdict_never_resurrects_a_card_somebody_finished_with() {
        let now = 100_000_000;
        let over = |state: &str| BoardCard {
            ledger: "released".into(),
            ..card("term:9", state, now - 60_000)
        };
        // A run the card itself reported finished and the person acknowledged
        // has already sailed into `idle`. The ledger agreeing that its
        // terminal is over is not news, and putting the card back into `done`
        // would hand somebody a receipt they filed an hour ago.
        assert_eq!(over("done").bucket_at(now), Bucket::Idle);
        assert_eq!(over("exited").bucket_at(now), Bucket::Idle);
        assert_eq!(over("idle").bucket_at(now), Bucket::Idle);
        // And an unseen `done` stays exactly where it was, too.
        assert_eq!(
            BoardCard {
                unseen: true,
                ..over("done")
            }
            .bucket_at(now),
            Bucket::Done
        );
    }

    /// Why a shorter clock could not have answered this.
    ///
    /// The decay reads `changed_at`, and the panes this bug is about have
    /// none: the backend stamps zero for a pane that never sent a hook and
    /// calls it `working` because its pty is alive, and a zero clock never
    /// decays — measured against `now` it would be stale the millisecond it
    /// appeared. So NO value of [`STALE_AFTER_MS`], thirty minutes or thirty
    /// seconds, moves that card, and the verdict moves it at any clock.
    ///
    /// Where the clock does fire the two say different things, and that is
    /// the second reason: decay hides the card in `idle` because nobody
    /// knows, and the ledger draws it in `done` because somebody does.
    #[test]
    fn the_clock_could_not_have_answered_this() {
        let now = 100_000_000;
        let unclocked = card("term:9", "working", 0);
        assert_eq!(unclocked.bucket_at(now), Bucket::Working);
        assert_eq!(
            unclocked.bucket_at(now + 1000 * STALE_AFTER_MS),
            Bucket::Working,
            "a clockless card decayed after all"
        );
        assert_eq!(
            BoardCard {
                ledger: "release_unknown".into(),
                ..unclocked
            }
            .bucket_at(now),
            Bucket::Done
        );

        let silent = card("term:9", "working", now - 31 * 60 * 1000);
        assert_eq!(silent.bucket_at(now), Bucket::Idle);
        assert_eq!(
            BoardCard {
                ledger: "release_unknown".into(),
                ..silent
            }
            .bucket_at(now),
            Bucket::Done
        );
    }

    /// Every word the ledger can write is a word this reads, and the line
    /// between "over" and "still running" is asked for rather than copied —
    /// so the day the ledger grows a state, it is decided in one place.
    #[test]
    fn the_ledger_vocabulary_is_read_from_the_ledger() {
        use crate::orchestration::WorkerState;

        for state in WorkerState::ALL {
            let held = BoardCard {
                ledger: state.as_str().into(),
                ..card("term:9", "working", 1)
            };
            assert_eq!(
                held.bucket_at(2),
                if state.is_live() {
                    Bucket::Working
                } else {
                    Bucket::Done
                },
                "{state:?}"
            );
            // The word the window is handed is the word this parses: the
            // ledger's serialisation and its `as_str` are one spelling.
            let wire = serde_json::to_value(state).expect("a worker state serialises");
            assert_eq!(wire.as_str(), Some(state.as_str()), "{state:?}");
        }
    }

    /// A column comes back as a tree: a child follows its parent, one level in,
    /// and the parent says how many it has.
    ///
    /// The sort still decides everything it can — `term:9` changed last and is
    /// therefore the first ROOT, while its own children stay in freshest-first
    /// order among themselves.
    #[test]
    fn a_column_indents_the_agents_a_pane_started() {
        let held = [
            card("term:1", "working", 100),
            child("term:2", "working", 200, "term:1"),
            child("term:3", "working", 300, "term:1"),
            card("term:9", "working", 400),
        ];
        let working = columns(&held, &BoardQuery::default(), 0)
            .into_iter()
            .find(|one| one.bucket == Bucket::Working)
            .expect("a working column");
        assert_eq!(
            working
                .cards
                .iter()
                .map(|one| (one.pane.as_str(), one.lineage.depth))
                .collect::<Vec<_>>(),
            vec![("term:9", 0), ("term:1", 0), ("term:3", 1), ("term:2", 1)]
        );
        let parent = &working.cards[1];
        assert_eq!(parent.lineage.child_count, 2);
        assert!(working.cards[2].lineage.is_first_sibling);
        assert!(working.cards[3].lineage.is_last_sibling);
    }

    /// A parent in another column is a parent this column does not have — and
    /// the child is drawn as a root there rather than vanishing between the
    /// two. This is the invariant that matters most on this surface: the child
    /// is often the agent that is stuck, and the column it is stuck in is the
    /// one somebody is looking at.
    #[test]
    fn a_child_whose_parent_sits_in_another_column_is_still_drawn() {
        let held = [
            card("term:1", "working", 100),
            child("term:2", "waiting", 200, "term:1"),
        ];
        let drawn = columns(&held, &BoardQuery::default(), 0);
        assert_eq!(
            drawn.iter().map(|one| one.cards.len()).sum::<usize>(),
            2,
            "a card was lost between the columns: {drawn:?}"
        );
        let stuck = drawn
            .iter()
            .find(|one| one.bucket == Bucket::Attention)
            .expect("attention column");
        assert_eq!(stuck.cards[0].pane, "term:2");
        assert_eq!(stuck.cards[0].lineage, crate::agent_lineage::ROOT_LINEAGE);
        // And the parent, alone in its own column, does not claim a child it
        // cannot show.
        let busy = drawn
            .iter()
            .find(|one| one.bucket == Bucket::Working)
            .expect("working column");
        assert_eq!(busy.cards[0].lineage.child_count, 0);
    }

    /// The window's own answer is never believed: whatever `lineage` arrives
    /// as, the grouping overwrites it. Otherwise a webview could claim a depth
    /// and indent a card under a card that never started it.
    #[test]
    fn the_grouping_owns_the_lineage_it_ships() {
        let mut lying = card("term:1", "working", 100);
        lying.lineage = crate::agent_lineage::Lineage {
            depth: 7,
            is_first_sibling: false,
            is_last_sibling: false,
            child_count: 42,
        };
        let drawn = columns(&[lying], &BoardQuery::default(), 0)
            .into_iter()
            .find(|one| one.bucket == Bucket::Working)
            .expect("a working column");
        assert_eq!(drawn.cards[0].lineage, crate::agent_lineage::ROOT_LINEAGE);
    }

    /// The badge counts what wants a person, and only that.
    #[test]
    fn the_badge_counts_only_what_is_stuck() {
        let held = [
            card("a", "blocked", 1),
            card("b", "waiting", 2),
            card("c", "working", 3),
            card("d", "done", 4),
            card("e", "idle", 5),
        ];
        assert_eq!(attention_count(&held, 0), 2);
        assert_eq!(attention_count(&[], 0), 0);
    }

    #[test]
    fn one_snapshot_feeds_the_filtered_surface_and_unfiltered_badge() {
        let held = [
            card("wanted", "working", 3),
            card("hidden-attention", "waiting", 2),
        ];
        let answer = snapshot(
            &held,
            &BoardQuery {
                query: "wanted".into(),
                ..BoardQuery::default()
            },
            0,
        );
        assert_eq!(answer.total_count, 2);
        assert_eq!(answer.attention_count, 1);
        assert_eq!(
            answer
                .columns
                .iter()
                .flat_map(|column| &column.cards)
                .map(|card| card.pane.as_str())
                .collect::<Vec<_>>(),
            ["wanted"]
        );
    }

    #[test]
    fn board_card_autonomy_is_optional_for_older_snapshots() {
        let legacy: BoardCard = serde_json::from_value(serde_json::json!({
            "pane": "term:4",
            "agent": "zo",
            "state": "working",
            "heading": "Zo"
        }))
        .expect("a pre-autonomy card still deserializes");
        assert_eq!(legacy.autonomy, None);
    }

    #[test]
    fn board_card_autonomy_round_trips_the_full_status() {
        let value = serde_json::json!({
            "pane": "term:4",
            "agent": "zo",
            "state": "working",
            "heading": "Zo",
            "autonomy": {
                "goal": {
                    "phase": "running",
                    "next_at": 42_000,
                    "gates_passed": 1,
                    "gates_total": 2,
                    "action_turns": 3,
                    "stalled_turns": 0
                },
                "loops": [{
                    "id": "loop-1",
                    "phase": "running",
                    "trigger": "every 600s",
                    "next_at": 42_000,
                    "runs": 2,
                    "max_runs": 50,
                    "quiet": 3,
                    "reason": "watching CI"
                }],
                "budget": {
                    "continuations": 2,
                    "max_continuations": 8,
                    "assistant_turns": 3,
                    "max_assistant_turns": 12,
                    "output_tokens": 900,
                    "max_output_tokens": 32_000,
                    "active_millis": 4_500,
                    "max_wall_clock_secs": 1_800
                }
            }
        });
        let card: BoardCard = serde_json::from_value(value.clone())
            .expect("the session-status autonomy contract is a board fact");
        assert_eq!(
            serde_json::to_value(card).expect("the card serializes")["autonomy"],
            value["autonomy"]
        );
    }
}
