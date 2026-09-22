//! The mention rerank seat: one page of the `@` popup, or of the `/resume`
//! list, put to a System One judgment — which row does the person mean, read
//! off the sentence they are writing — and reordered only while the
//! selection still sits on the first row (t-6042).
//!
//! The fuzzy page is drawn first, exactly as it was before this seat
//! existed; nothing here runs on the key path but a clone of one page of
//! names and a spawn. The question, the door, the row, the memo and the
//! label are the same shape as the recall seat next door
//! (`rerank_shadow.rs`), so a reader of one can read the other: one closed
//! choice over the page (`zerocode_core::jev::choice`), the one door
//! (`jev_gate`), the one ledger discipline (`shadow_ledger`), the one judge
//! (`judge_seat_ledger`).
//!
//! Putting a page to the judgment sends the head of the sentence a person
//! is writing and the names on the page. That is a different thing to
//! consent to than a task's text or the vault's summaries, so it has its own
//! switch, `smart.jevMentionRerank`, and is off unless a person writes one
//! of its other words.
//!
//! # One question in flight, ever
//!
//! A keystroke that changes the page asks again and aborts the older
//! question: its answer would be for a page the person is no longer
//! looking at. The surface remembers the ticket of the question it is
//! waiting on and drops an answer that names any other.
//!
//! # Nothing gets worse for asking
//!
//! A refusal, a failure, a timeout, a reply that breaks the contract, or an
//! answer for a page the person has already moved through leaves the fuzzy
//! page as it was, and the row says which reader the page came from
//! (`routeUse`). Under a recording mode the answer still reaches the
//! surface — marked as not to be applied — so the label can compare the
//! person's pick to what the judgment would have put first.
//!
//! # The label is a comparison the person makes
//!
//! One label per completion ([`MentionRerank::note_chosen`]): `agreed: true`
//! when the row the person took was the row the judgment put first, `false`
//! otherwise with `rank` the place the judgment gave the row they took, and
//! `notCompared` for a pick outside the page the judgment saw. The judge
//! counts those rows as this seat's agreement
//! (`zerocode_core::jev::summary::AGREED`), which is what `auto` rises on.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use api::{
    SystemOneCall, SystemOneClient, SystemOneConfig, SystemOneFailure, SystemOneQuestion,
    SystemOneRequest, SYSTEMONE_MODEL,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use zerocode_core::jev::choice::{self, Choice};
use zerocode_core::jev::door::{self, Refused};
use zerocode_core::jev::{
    Cap, JevMode, MENTION_APPLY_DEADLINE_MS, MENTION_CANDIDATE_CAP,
    MENTION_HEAD_BYTE_CAP, MENTION_INTENT_CHAR_CAP, MENTION_RERANK, ROUTE_USE_APPLIED,
    ROUTE_USE_FALLBACK,
};

use super::jev_gate::{self, JevDoor};
use super::probe_exec::{remember_bounded, task_fingerprint};
use super::settings::jev_mention_rerank_mode_from;
use super::shadow_ledger::{
    append_shadow_row, judge_seat_ledger, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES,
};
use crate::misc_tools::agent_tools::shared_agent_runtime;

/// The seat's ledger file — the Jev use table's name for it.
pub const MENTION_RERANK_FILE: &str = MENTION_RERANK.ledger;

/// Outcome of a row whose judgment answered and checked out — the door's
/// word, because the one counter every seat shares reads it.
pub const MENTION_OUTCOME_ANSWERED: &str = door::ANSWERED_OUTCOME;

/// The wall past which an answer is dropped — the use table's own number,
/// so the stage that waits and the judge that reads the wait cannot
/// disagree.
pub const MENTION_RERANK_DEADLINE: Duration = Duration::from_millis(MENTION_APPLY_DEADLINE_MS);
const _: () = assert!(
    matches!(MENTION_RERANK.apply_deadline_ms, Some(MENTION_APPLY_DEADLINE_MS)),
    "the seat's row names the wall its stage holds"
);

/// The rubric's version, pinned by a fingerprint of its words: a word
/// changed without a bump is a red test rather than a quiet drift.
pub const MENTION_RUBRIC_VERSION: u32 = 1;

/// The one question, by the id its answer comes back under.
const QUESTION: &str = "meant";

/// What the judgment is asked. The names in backticks are the state's own
/// keys, so the sentence reads the request rather than describing it.
const INSTRUCTIONS: &str = "The person is writing `intent` and has typed `query` to pick one row of \
`candidates`. Which candidate do they mean?";

/// The word a label row carries as its kind, so a reader sweeping the ledger
/// can tell a comparison from a reading without parsing both.
pub const LABEL_ROW_KIND: &str = "label";

const FAIL_SETTINGS_UNAVAILABLE: &str = "settings_unavailable";

/// Where a project's mention ledger lives.
#[must_use]
pub fn mention_rerank_path(cwd: &Path) -> PathBuf {
    shadow_ledger_path(cwd, MENTION_RERANK_FILE)
}

/// Every sentence the judgment is shown, in one string, so a fingerprint of
/// it pins them ([`MENTION_RUBRIC_VERSION`]).
#[cfg(test)]
#[must_use]
pub fn rubric_words() -> String {
    [QUESTION, INSTRUCTIONS].join("\n")
}

/// The fingerprint of [`rubric_words`] this version was pinned at.
#[cfg(test)]
#[must_use]
pub fn rubric_pin() -> String {
    zerocode_core::jev::rubric_fingerprint(rubric_words)
}

/// Which page asked: the composer's `@` popup or the `/resume` list. One
/// ledger for both, told apart by this word, because the question, the
/// caps and the label are the same and the two are one seat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MentionSurface {
    Mention,
    Resume,
}

impl MentionSurface {
    /// The word a ledger row carries.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Mention => "mention",
            Self::Resume => "resume",
        }
    }
}

/// One row of the page as the judgment sees it: its name — a path, a
/// `wiki/…` page, a skill, a session id — and the head the row already
/// shows beside it, or nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionCandidate {
    pub name: String,
    pub head: String,
}

/// One page put to the judgment, in the order the surface drew it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionAsk {
    pub surface: MentionSurface,
    /// The sentence being written, without the token; empty on a `/resume`
    /// search, where the words typed are the whole of the intent.
    pub intent: String,
    /// The `@` token, or the `/resume` search text.
    pub query: String,
    pub candidates: Vec<MentionCandidate>,
}

/// A judgment's answer, as the surface receives it: the page's positions in
/// the judgment's order, and whether the mode acts on it. A recording mode
/// still delivers the order — marked `applies: false` — so the surface can
/// remember what the judgment put first for the label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionAnswer {
    /// The question this answers; the surface drops any other ticket's.
    pub ticket: u64,
    pub surface: MentionSurface,
    /// Every position of the page exactly once, the judgment's first choice
    /// first.
    pub order: Vec<usize>,
    pub applies: bool,
}

/// One page's row: what the page held, what the judgment put first, and
/// how the call went.
///
/// No intent, no query: the words are a fingerprint. The names stay — a
/// relative path, a page name, a session id — because an order is
/// unreadable without names, as the recall row keeps its slugs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionRerankRow {
    /// Unix milliseconds when the row was made.
    pub at: u64,
    pub surface: MentionSurface,
    /// Fingerprint of the intent and the query the page was judged against.
    pub query: u64,
    /// Fingerprint of the page's names in fuzzy order, so a label is only
    /// ever compared with the reading about the same page.
    pub notes: u64,
    pub rubric_version: u32,
    /// [`MENTION_OUTCOME_ANSWERED`], a failure's ledger token, or the door's
    /// refusal token.
    pub outcome: String,
    /// How many rows were put to the judgment.
    pub candidates: usize,
    /// True when the judgment was recalled from this process's memo rather
    /// than asked; the timing fields then say nothing.
    #[serde(default)]
    pub cached: bool,
    pub elapsed_ms: u64,
    pub retries: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Requests this page sent: none when the door refused it or the memo
    /// answered, one plus its retries when it left.
    pub requests: u32,
    /// Lines the door withheld from what was sent.
    pub redacted_lines: u32,
    /// Which of the reply's rules refused it, on a row whose `outcome` is
    /// `schema` — [`choice::ChoiceRefusal::token`]'s word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
    /// What the judgment said, once it checked out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judged: Option<MentionJudged>,
    /// Which reader the page's order came from: the judgment
    /// ([`ROUTE_USE_APPLIED`]) when the answer was handed to the surface
    /// under an acting mode, the mode's own word when it only records, and
    /// the fuzzy page ([`ROUTE_USE_FALLBACK`]) when nothing answered.
    pub route_use: String,
    /// Whether the answer was handed to the surface to act on. Whether the
    /// surface then DID reorder — it does not when the selection had moved —
    /// is the label's `applied`, written by the surface that knows.
    pub applied: bool,
}

/// A checked judgment beside the fuzzy page it was asked about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionJudged {
    /// The page in fuzzy order — what the person saw first.
    pub fuzzy: Vec<String>,
    /// The page in the judgment's order.
    pub proposed: Vec<String>,
    pub top_changed: bool,
    /// The judgment's confidence in its first choice — the shape of the
    /// distribution, not a rate of being right.
    pub confidence: f64,
}

impl MentionRerankRow {
    fn new(key: MemoKey, ask: &MentionAsk, outcome: String) -> Self {
        Self {
            at: super::decision_shadow::unix_millis(),
            surface: ask.surface,
            query: key.query,
            notes: key.notes,
            rubric_version: key.rubric,
            outcome,
            candidates: ask.candidates.len(),
            cached: false,
            elapsed_ms: 0,
            retries: 0,
            model: None,
            input_tokens: None,
            requests: 0,
            redacted_lines: 0,
            rejected: None,
            judged: None,
            route_use: ROUTE_USE_FALLBACK.to_string(),
            applied: false,
        }
    }

    /// What this row's call cost, as the client counted it — the same
    /// account the recall row keeps (`rerank_shadow::RerankShadowRow::spent`).
    fn spent(&mut self, call: &SystemOneCall, withheld: u32) {
        self.elapsed_ms = jev_gate::millis(call.elapsed);
        self.retries = call.retries;
        self.requests = call.requests;
        self.redacted_lines = withheld;
    }
}

/// The comparison the person made, as the ledger keeps it. Shaped like the
/// recall seat's label (`rerank_shadow::RerankLabelRow`): the reading it
/// grades under `label`, the mark under `agreed`, `rank` the place the
/// judgment gave the row taken, and `applied` — here, whether the page was
/// actually reordered before the pick.
///
/// `agreed` is absent, and `notCompared` present, for a pick outside the
/// page the judgment saw: the seat never offered that row, so the pick says
/// nothing about the seat (the summons seat's own rule,
/// `zerocode_core::summon_choice::NOT_OFFERED`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionLabelRow {
    pub kind: String,
    pub at: u64,
    pub surface: MentionSurface,
    /// The reading this row grades, spelled `<query>:<notes>` — the two
    /// fingerprints the answered row is named by.
    pub label: String,
    pub query: u64,
    pub notes: u64,
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_compared: Option<String>,
}

/// A page's judgment as this process remembers it. The key carries the
/// rubric version and the requested model — the door's, pinned or not
/// ([`jev_gate::model_key`]) — so a judgment made under other words or by
/// another model is never recalled for this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MemoKey {
    query: u64,
    notes: u64,
    rubric: u32,
    model: u64,
}

impl MemoKey {
    fn for_page(ask: &MentionAsk, model: u64) -> Self {
        let mut names = String::new();
        for candidate in &ask.candidates {
            names.push_str(&candidate.name);
            names.push('\u{1f}');
        }
        Self {
            query: task_fingerprint(&ask.intent, &ask.query),
            notes: task_fingerprint(ask.surface.key(), &names),
            rubric: MENTION_RUBRIC_VERSION,
            model,
        }
    }
}

#[derive(Debug, Clone)]
struct Remembered {
    model: String,
    judged: MentionJudged,
    order: Vec<usize>,
}

fn memo() -> &'static Mutex<HashMap<MemoKey, Remembered>> {
    static MEMO: OnceLock<Mutex<HashMap<MemoKey, Remembered>>> = OnceLock::new();
    MEMO.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What one page's judgment settled on, kept for the label the person's
/// pick writes. In memory and not on disk, for the reason the recall seat
/// gives: the mark is whether THIS pick took what THIS reading put first.
#[derive(Debug, Clone)]
struct Settled {
    ticket: u64,
    surface: MentionSurface,
    query: u64,
    notes: u64,
    /// The page's positions in the judgment's order.
    order: Vec<usize>,
}

/// The seat as it stands while a page is open: what was read at the
/// boundary, once, so no keystroke reads a settings file, a ledger, or the
/// keychain.
struct Armed {
    mode: JevMode,
    /// Whether the mode acts — `on`, or an `auto` its own ledger raised.
    acting: bool,
    door: Arc<JevDoor>,
    client: Option<SystemOneClient>,
}

/// How an answer reaches the surface: a closure the host hands in once,
/// which sends on whatever the host's loop selects on.
type Deliver = Arc<dyn Fn(MentionAnswer) + Send + Sync>;

/// The seat beside the `@` popup and the `/resume` list. Seated once per
/// host at the project's `cwd`, where the setting and the ledger are; armed
/// when a page opens, asked as the page changes, told what the person took.
pub struct MentionRerank {
    cwd: PathBuf,
    deliver: Deliver,
    armed: Arc<Mutex<Option<Armed>>>,
    /// Which arming is current: a boundary read that lands after the page
    /// closed, or after a newer page opened, is thrown away.
    arming: Arc<AtomicU64>,
    tickets: AtomicU64,
    in_flight: Mutex<Option<tokio::task::JoinHandle<()>>>,
    settled: Arc<Mutex<Option<Settled>>>,
}

impl std::fmt::Debug for MentionRerank {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MentionRerank").field("cwd", &self.cwd).finish_non_exhaustive()
    }
}

impl MentionRerank {
    /// The seat for a project, delivering every answer through `deliver`.
    pub fn at(cwd: &Path, deliver: impl Fn(MentionAnswer) + Send + Sync + 'static) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
            deliver: Arc::new(deliver),
            armed: Arc::new(Mutex::new(None)),
            arming: Arc::new(AtomicU64::new(0)),
            tickets: AtomicU64::new(0),
            in_flight: Mutex::new(None),
            settled: Arc::new(Mutex::new(None)),
        }
    }

    /// The project this seat stands in.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// The host moved to another project (`/resume`, `/new` elsewhere): the
    /// seat's setting and ledger are that project's from now, and whatever
    /// page was open is gone.
    pub fn move_to(&mut self, cwd: &Path) {
        self.disarm();
        self.cwd = cwd.to_path_buf();
    }

    /// Read the boundary once, as a page opens: the setting, the door, the
    /// key and — under `auto` — the standing the seat's own ledger recorded.
    /// Off the key path: the read is a settings file, two ledger tails and,
    /// the first time in a process, the keychain, so it lands on a blocking
    /// task and a page asked about before it lands asks nothing. A seat
    /// whose word is `off` arms nothing, and every ask until
    /// [`Self::disarm`] costs a mutex read and no more.
    pub fn arm(&self) {
        let arming = self.arming.fetch_add(1, Ordering::SeqCst) + 1;
        let cwd = self.cwd.clone();
        let armed = Arc::clone(&self.armed);
        let current = Arc::clone(&self.arming);
        shared_agent_runtime().spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                let read = arm_at(&cwd);
                if current.load(Ordering::SeqCst) != arming {
                    return;
                }
                if let Ok(mut held) = armed.lock() {
                    *held = read;
                }
            })
            .await;
        });
    }

    /// [`Self::arm`], waited for — a test's road, and a host that opens a
    /// page it will ask about at once.
    pub fn arm_now(&self) {
        let arming = self.arming.fetch_add(1, Ordering::SeqCst) + 1;
        let read = arm_at(&self.cwd);
        if self.arming.load(Ordering::SeqCst) != arming {
            return;
        }
        if let Ok(mut held) = self.armed.lock() {
            *held = read;
        }
    }

    /// The page closed: the question in flight is abandoned, and nothing
    /// asked before the next [`Self::arm`] leaves the machine.
    pub fn disarm(&self) {
        self.arming.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut held) = self.armed.lock() {
            *held = None;
        }
        self.abandon_in_flight();
        if let Ok(mut settled) = self.settled.lock() {
            *settled = None;
        }
    }

    /// Whether the seat is armed to ask — for a surface deciding whether to
    /// build a page at all.
    #[must_use]
    pub fn asks(&self) -> bool {
        self.armed.lock().is_ok_and(|held| held.is_some())
    }

    /// Put one page to the judgment, off the calling thread, and hand back
    /// the question's ticket — `None` when nothing will be asked: the seat is
    /// not armed, the query is empty, or there is only one row to order.
    /// The question before this one is abandoned.
    pub fn ask(&self, ask: MentionAsk) -> Option<u64> {
        let (mode, acting, door, client) = {
            let held = self.armed.lock().ok()?;
            let armed = held.as_ref()?;
            (armed.mode, armed.acting, Arc::clone(&armed.door), armed.client.clone())
        };
        if ask.query.trim().is_empty() || ask.candidates.len() < 2 {
            return None;
        }
        let ticket = self.tickets.fetch_add(1, Ordering::Relaxed) + 1;
        self.abandon_in_flight();
        let shot = Shot {
            ledger: mention_rerank_path(&self.cwd),
            door,
            client,
            ask: cut_to_caps(ask),
            mode,
            acting,
            ticket,
            settled: Arc::clone(&self.settled),
            deliver: Arc::clone(&self.deliver),
        };
        let handle = shared_agent_runtime().spawn(run(shot));
        if let Ok(mut in_flight) = self.in_flight.lock() {
            if let Some(older) = in_flight.replace(handle) {
                older.abort();
            }
        }
        Some(ticket)
    }

    /// The person took a row: write the comparison against the judgment
    /// that settled for `ticket`, if one did. `chosen` is the row's position
    /// on the page as the judgment saw it, `None` for a row outside that
    /// page; `reordered` is whether the page had taken the judgment's order
    /// before the pick. Answers whether a label was written — nothing is,
    /// when no judgment settled for this ticket.
    #[must_use]
    pub fn note_chosen(&self, ticket: u64, chosen: Option<usize>, reordered: bool) -> bool {
        let Some(settled) = self.settled.lock().ok().and_then(|mut held| held.take()) else {
            return false;
        };
        if settled.ticket != ticket {
            return false;
        }
        let row = label_row(&settled, chosen, reordered);
        let ledger = mention_rerank_path(&self.cwd);
        let written = append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES).is_ok();
        // A label may be the mark that clears the seat's agreement line:
        // judge now, off the key path, so a rise the labels earned is read by
        // the next page that opens.
        judge_detached(ledger);
        written
    }

    fn abandon_in_flight(&self) {
        if let Some(older) = self.in_flight.lock().ok().and_then(|mut held| held.take()) {
            older.abort();
        }
    }
}

/// What a page's opening reads, or `None` when the seat is to ask nothing:
/// an ablation holding it out, an unreadable setting, or a mode that asks
/// nothing.
fn arm_at(cwd: &Path) -> Option<Armed> {
    if telemetry::attest_ablated(telemetry::HarnessFeature::MentionRerank) {
        return None;
    }
    let Some(mode) = jev_mention_rerank_mode_from(&runtime::ConfigLoader::default_for(cwd)) else {
        telemetry::attest_failed(telemetry::HarnessFeature::MentionRerank, FAIL_SETTINGS_UNAVAILABLE);
        return None;
    };
    if !mode.asks() {
        telemetry::attest_declined(telemetry::HarnessFeature::MentionRerank, mode.key());
        return None;
    }
    // Read only under `auto`: a person's `on` needs no ledger, and `shadow`
    // reads none.
    let raised = mode == JevMode::Auto && runtime::jev_seat_applies(cwd, &MENTION_RERANK);
    Some(Armed {
        mode,
        acting: mode.applies_with(raised),
        door: Arc::new(JevDoor::open(cwd)),
        client: SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client),
    })
}

/// The page as the row records it and the wire receives it: the caps the
/// row declares, applied once here so the fingerprint and the request are
/// of the same words. The door cuts again, and finds nothing to cut.
fn cut_to_caps(mut ask: MentionAsk) -> MentionAsk {
    ask.intent = door::cut(&ask.intent, Cap::Chars(MENTION_INTENT_CHAR_CAP));
    ask.query = door::cut(&ask.query, Cap::Chars(MENTION_INTENT_CHAR_CAP));
    ask.candidates.truncate(MENTION_CANDIDATE_CAP);
    for candidate in &mut ask.candidates {
        candidate.head = door::cut(&candidate.head, Cap::Bytes(MENTION_HEAD_BYTE_CAP));
    }
    ask
}

/// Everything one page's judgment carries off the calling thread.
struct Shot {
    ledger: PathBuf,
    door: Arc<JevDoor>,
    client: Option<SystemOneClient>,
    ask: MentionAsk,
    mode: JevMode,
    acting: bool,
    ticket: u64,
    settled: Arc<Mutex<Option<Settled>>>,
    deliver: Deliver,
}

/// Judge the page, hand the surface what it may act on, and write the row.
async fn run(shot: Shot) {
    let Shot { ledger, door, client, ask, mode, acting, ticket, settled, deliver } = shot;
    let (mut row, order) = judge(&door, client.as_ref(), &ask).await;
    let answered = row.outcome == MENTION_OUTCOME_ANSWERED;
    row.applied = answered && acting;
    row.route_use = if row.applied {
        ROUTE_USE_APPLIED.to_string()
    } else if answered {
        mode.key().to_string()
    } else {
        ROUTE_USE_FALLBACK.to_string()
    };
    if let Some(order) = order {
        if let Ok(mut held) = settled.lock() {
            *held = Some(Settled {
                ticket,
                surface: ask.surface,
                query: row.query,
                notes: row.notes,
                order: order.clone(),
            });
        }
        deliver(MentionAnswer { ticket, surface: ask.surface, order, applies: row.applied });
    }
    let _ = tokio::task::spawn_blocking(move || {
        let written = append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES);
        let _ = judge_ledger(&ledger, super::decision_shadow::now_ms());
        written
    })
    .await;
}

/// The state one request carries: the sentence, the token, and the page.
/// A row with no head carries none, so the door finds nothing there to cut.
fn state_of(ask: &MentionAsk) -> Value {
    let candidates: Vec<Value> = ask
        .candidates
        .iter()
        .map(|candidate| {
            let mut row = Map::from_iter([("name".to_string(), Value::from(candidate.name.as_str()))]);
            if !candidate.head.is_empty() {
                row.insert("head".to_string(), Value::from(candidate.head.as_str()));
            }
            Value::Object(row)
        })
        .collect();
    Value::Object(Map::from_iter([
        ("intent".to_string(), Value::from(ask.intent.as_str())),
        ("query".to_string(), Value::from(ask.query.as_str())),
        ("candidates".to_string(), Value::Array(candidates)),
    ]))
}

/// The option a page position is offered under. A position and not the
/// name, so a name with a quote or a slash in it is never an option id.
fn option_id(position: usize) -> String {
    format!("c{position}")
}

/// The closed choice over the page, and the options it offered in page
/// order. Built as zo's typed question (`SystemOneQuestion::choice`, which
/// carries the union tag), read back through the shared reader
/// (`choice::read`), so the rules a closed choice keeps are kept once.
fn questions_of(ask: &MentionAsk) -> (BTreeMap<String, SystemOneQuestion>, Vec<String>) {
    let offered: Vec<String> = (0..ask.candidates.len()).map(option_id).collect();
    let means: Vec<String> = (0..ask.candidates.len()).map(|position| format!("`candidates[{position}]`")).collect();
    let question = SystemOneQuestion::choice(
        INSTRUCTIONS,
        offered.iter().zip(&means).map(|(id, means)| (id.as_str(), Some(means.as_str()))),
    );
    (BTreeMap::from_iter([(QUESTION.to_string(), question)]), offered)
}

/// The page's positions in the judgment's order: by probability, highest
/// first, and the fuzzy order between equals — a stable sort, so the
/// judgment moves only what it had an opinion about.
fn order_of(choice: &Choice, offered: &[String]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..offered.len()).collect();
    order.sort_by(|left, right| {
        let of = |position: &usize| choice.probabilities.get(&offered[*position]).copied().unwrap_or(0.0);
        of(right).partial_cmp(&of(left)).unwrap_or(std::cmp::Ordering::Equal)
    });
    order
}

/// One page's row and the order it settled on: recalled from the memo,
/// refused at the door, or asked and checked. Writes nothing.
pub(super) async fn judge(
    door: &JevDoor,
    client: Option<&SystemOneClient>,
    ask: &MentionAsk,
) -> (MentionRerankRow, Option<Vec<usize>>) {
    let key = MemoKey::for_page(ask, door.model_key());
    let recalled = memo().lock().ok().and_then(|memo| memo.get(&key).cloned());
    if let Some(remembered) = recalled {
        telemetry::attest_fired(telemetry::HarnessFeature::MentionRerank);
        let mut row = MentionRerankRow::new(key, ask, MENTION_OUTCOME_ANSWERED.to_string());
        row.cached = true;
        row.model = Some(remembered.model);
        row.judged = Some(remembered.judged);
        return (row, Some(remembered.order));
    }
    let state = state_of(ask);
    let (questions, offered) = questions_of(ask);
    let request = SystemOneRequest {
        state: &state,
        model: SYSTEMONE_MODEL,
        questions: &questions,
    };
    let Some(body) = jev_gate::body_of(&request) else {
        let failure = SystemOneFailure::InvalidRequest;
        telemetry::attest_failed(telemetry::HarnessFeature::MentionRerank, failure.token());
        return (MentionRerankRow::new(key, ask, failure.ledger_token()), None);
    };
    let (cleared, client) = match (door.pass(&MENTION_RERANK, client.is_some(), body), client) {
        (Ok(cleared), Some(client)) => (cleared, client),
        (passed, _) => {
            let refused = passed.err().unwrap_or(Refused::NoKey);
            telemetry::attest_declined(telemetry::HarnessFeature::MentionRerank, refused.token());
            return (MentionRerankRow::new(key, ask, refused.token().to_string()), None);
        }
    };
    let withheld = u32::try_from(cleared.withheld_lines()).unwrap_or(u32::MAX);
    // No hedge: nothing waits on this seat, so a second copy would buy an
    // answer inside a wall nobody is standing at — the person's money for
    // nothing, at a seat asked on every keystroke.
    let call = jev_gate::send(client, cleared, MENTION_RERANK_DEADLINE, None).await;
    let (mut row, order) = match &call.outcome {
        Ok(response) => {
            let answers = Value::Object(
                response.answers.iter().map(|(name, value)| (name.clone(), value.clone())).collect(),
            );
            let offered_set: BTreeSet<String> = offered.iter().cloned().collect();
            let (mut row, order) = match choice::read(&answers, QUESTION, &offered_set) {
                Ok(choice) => {
                    telemetry::attest_fired(telemetry::HarnessFeature::MentionRerank);
                    let order = order_of(&choice, &offered);
                    let fuzzy: Vec<String> = ask.candidates.iter().map(|candidate| candidate.name.clone()).collect();
                    let proposed: Vec<String> = order.iter().map(|position| fuzzy[*position].clone()).collect();
                    let judged = MentionJudged {
                        top_changed: order.first() != Some(&0),
                        confidence: choice.confidence,
                        fuzzy,
                        proposed,
                    };
                    if let Ok(mut memo) = memo().lock() {
                        remember_bounded(
                            &mut memo,
                            vec![(
                                key,
                                Remembered { model: response.model.clone(), judged: judged.clone(), order: order.clone() },
                            )],
                        );
                    }
                    let mut row = MentionRerankRow::new(key, ask, MENTION_OUTCOME_ANSWERED.to_string());
                    row.judged = Some(judged);
                    (row, Some(order))
                }
                Err(refused) => {
                    telemetry::attest_failed(
                        telemetry::HarnessFeature::MentionRerank,
                        SystemOneFailure::Schema.token(),
                    );
                    let mut row = MentionRerankRow::new(key, ask, SystemOneFailure::Schema.ledger_token());
                    row.rejected = Some(refused.token().to_string());
                    (row, None)
                }
            };
            // An answer that arrived billed, whether or not it checked out.
            row.model = Some(response.model.clone());
            row.input_tokens = Some(response.usage.input_tokens);
            (row, order)
        }
        Err(failure) => {
            telemetry::attest_failed(telemetry::HarnessFeature::MentionRerank, failure.token());
            (MentionRerankRow::new(key, ask, failure.ledger_token()), None)
        }
    };
    row.spent(&call, withheld);
    (row, order)
}

/// The mark itself.
fn label_row(settled: &Settled, chosen: Option<usize>, reordered: bool) -> MentionLabelRow {
    let rank = chosen.and_then(|chosen| settled.order.iter().position(|position| *position == chosen));
    MentionLabelRow {
        kind: LABEL_ROW_KIND.to_string(),
        at: super::decision_shadow::unix_millis(),
        surface: settled.surface,
        label: format!("{}:{}", settled.query, settled.notes),
        query: settled.query,
        notes: settled.notes,
        applied: reordered,
        agreed: chosen.map(|_| rank == Some(0)),
        rank,
        not_compared: chosen.is_none().then(|| zerocode_core::summon_choice::NOT_OFFERED.to_string()),
    }
}

/// Judge the seat on what it has just written, and write down a rise or a
/// fall — the one judge every seat that carries its own `agreed` marks
/// takes.
#[must_use]
pub fn judge_ledger(ledger: &Path, now_ms: i64) -> Option<zerocode_core::jev::promote::Verdict> {
    judge_seat_ledger(&MENTION_RERANK, ledger, now_ms)
}

/// The judge, off the key path: a label is written on the key that
/// completed a mention, and a whole-ledger parse does not belong there.
fn judge_detached(ledger: PathBuf) {
    shared_agent_runtime().spawn(async move {
        let _ = tokio::task::spawn_blocking(move || judge_ledger(&ledger, super::decision_shadow::now_ms())).await;
    });
}

#[cfg(test)]
mod tests;
