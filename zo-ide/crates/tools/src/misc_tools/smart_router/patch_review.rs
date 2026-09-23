//! The patch review seat: every patch an edit tool writes put to four Noul
//! questions through the Jev door before the model reads the result, and
//! hindsight's label on what became of the patch (t-6203).
//!
//! The questions, the state, the checks on a reply, the verdict, the note and
//! the hindsight walk belong to `runtime::patch_review`. This file owns only
//! what running it needs — the setting, the door, the wire, the row, the book
//! of patches waiting on their hindsight and the label rows — the shape of the
//! compaction seat next door (`compaction_seat.rs`), so a reader of one can
//! read the other.
//!
//! Putting a patch to the review sends the head of the person's words, the
//! patch's hunks, the newest lines of the output the edit followed and the
//! path's fingerprint. That is its own thing to consent to, so it has its own
//! switch, `smart.jevPatchReview`, off unless a person writes one of its
//! other words; and every request goes through the Jev door (`jev_gate`):
//! consent, budget, withheld lines and caps.
//!
//! # Recording never waits
//!
//! Under `shadow`, and under an `auto` its own evidence has not raised, the
//! review is asked beside the turn: the seat hands the result back at once
//! and writes its row when the answer comes, so the edit's result and its
//! timing are what they were before the seat. Only a seat that acts holds the
//! result — inside the row's wall — for the line it may add.
//!
//! # Nothing gets worse for asking
//!
//! A review the door refuses, that fails, misses the wall or breaks the
//! contract is `unavailable`, and the result reads as it did without the seat.
//! No second copy is ever sent (no hedge): a miss costs the edit nothing, so
//! there is nothing a second request would buy that is worth the person's
//! money.
//!
//! # The label is hindsight
//!
//! One label row per answered review, written by [`note_patch_review_turn`]
//! at a turn's end: the patch stood (a check green after the turn's last
//! edit, or its window of turns passed without its lines being edited again)
//! or it was regretted (an edit inside the window touched the same lines).
//! `agreed` is whether the verdict called it — a `permit` that stood, a
//! `proposal_only` that was regretted — and the judge counts those rows as
//! this seat's agreement (`zerocode_core::jev::summary::AGREED`).

use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use api::{SystemOneClient, SystemOneConfig, SystemOneFailure, SystemOneRequest, SYSTEMONE_MODEL};
use futures_util::future::BoxFuture;
use runtime::patch_review::{
    hindsight_of_turn, note, questions, read, state, unified_diff, Answers, Hindsight, Verdict,
    Watched,
};
use runtime::{ConversationMessage, PatchAsk, PatchReview, PatchReviewSeat, PATCH_REVIEW_RUBRIC_VERSION};
use serde::{Deserialize, Serialize};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::promote;
use zerocode_core::jev::{
    digest_of, fingerprint_of, Baseline, JevMode, PATCH_REVIEW, PATCH_REVIEW_APPLY_DEADLINE_MS,
    ROUTE_USE_APPLIED, ROUTE_USE_FALLBACK,
};

use super::jev_gate::{self, JevDoor};
use super::probe_exec::task_fingerprint;
use super::settings::jev_patch_review_mode_from;
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};

/// The seat's ledger file — the Jev use table's name for it.
pub const PATCH_REVIEW_FILE: &str = PATCH_REVIEW.ledger;

/// Outcome of a row whose review answered and checked out — the door's word,
/// because the one counter every seat shares reads it.
pub const PATCH_REVIEW_OUTCOME_ANSWERED: &str = zerocode_core::jev::door::ANSWERED_OUTCOME;

/// The wall one review waits — the use table's own number, so the stage that
/// waits and the judge that reads the wait cannot disagree.
pub const PATCH_REVIEW_DEADLINE: Duration = Duration::from_millis(PATCH_REVIEW_APPLY_DEADLINE_MS);
const _: () = assert!(
    matches!(PATCH_REVIEW.apply_deadline_ms, Some(PATCH_REVIEW_APPLY_DEADLINE_MS)),
    "the seat's row names the wall its stage waits"
);

const FAIL_SETTINGS_UNAVAILABLE: &str = "settings_unavailable";

/// Where a project's patch review ledger lives.
#[must_use]
pub fn patch_review_path(cwd: &Path) -> PathBuf {
    shadow_ledger_path(cwd, PATCH_REVIEW_FILE)
}

/// One review's row: what was asked, what came back, what the code made of
/// it, and whether the result the model read carried a line for it.
///
/// No words and no path: the path is a fingerprint, the patch is counted in
/// hunks and bytes, and the four answers are numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchReviewRow {
    /// Unix milliseconds when the row was made.
    pub at: u64,
    /// The turn the edit ran inside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    /// Fingerprint of the turn and the edit's call — what a label row names
    /// under `label`.
    pub judged: u64,
    /// The edit tool that wrote the patch.
    pub tool: String,
    /// Fingerprint of the file the patch was written to.
    pub path: String,
    pub hunks: usize,
    /// Bytes of the patch's hunks, before the row's cap.
    pub patch_bytes: usize,
    pub rubric_version: u32,
    /// [`PATCH_REVIEW_OUTCOME_ANSWERED`], a failure's ledger token, or the
    /// door's refusal token.
    pub outcome: String,
    /// `permit`, `proposal_only` or `unavailable` — the code's judgment.
    pub verdict: String,
    /// Each question's probability of `yes`, on an answered row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<BTreeMap<String, f64>>,
    /// Which reader the result came from: the review's ([`ROUTE_USE_APPLIED`]),
    /// a recording mode's word, or the tool alone ([`ROUTE_USE_FALLBACK`]).
    pub route_use: String,
    /// Whether the seat acted on an answered review.
    pub applied: bool,
    /// Whether a line joined the result the model read.
    pub noted: bool,
    #[serde(default)]
    pub cached: bool,
    pub elapsed_ms: u64,
    pub retries: u32,
    /// The model that answered, as the response named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Requests this review sent: none when the door refused it.
    pub requests: u32,
    /// Lines the door withheld from what was sent.
    pub redacted_lines: u32,
    /// The request's receipt ([`digest_of`]): the seat, the rubric version,
    /// the model asked for and the bytes the door let through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_digest: Option<String>,
    /// Which of a reply's rules refused it, on a row whose `outcome` is
    /// `schema`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
}

impl PatchReviewRow {
    fn new(ask: &PatchAsk) -> Self {
        Self {
            at: super::decision_shadow::unix_millis(),
            attempt: Some(ask.attempt.trim())
                .filter(|attempt| !attempt.is_empty())
                .map(str::to_string),
            judged: judged_key(ask),
            tool: ask.tool_name.clone(),
            path: fingerprint_of(&ask.path),
            hunks: ask.hunks.len(),
            patch_bytes: unified_diff(&ask.hunks).len(),
            rubric_version: PATCH_REVIEW_RUBRIC_VERSION,
            outcome: String::new(),
            verdict: Verdict::Unavailable.word().to_string(),
            answers: None,
            route_use: ROUTE_USE_FALLBACK.to_string(),
            applied: false,
            noted: false,
            cached: false,
            elapsed_ms: 0,
            retries: 0,
            model: None,
            input_tokens: None,
            requests: 0,
            redacted_lines: 0,
            request_digest: None,
            rejected: None,
        }
    }
}

/// The fingerprint a review's row and its label share: the turn and the call
/// that wrote the patch.
fn judged_key(ask: &PatchAsk) -> u64 {
    task_fingerprint(&ask.attempt, &ask.tool_use_id)
}

/// One review's hindsight, as the ledger keeps it. Shaped like the other
/// hindsight seats' labels: the row it grades under `label`, the mark under
/// `agreed`, and `applied` from the row, so an acted review and a recorded one
/// are compared on the same mark.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchReviewLabelRow {
    pub kind: String,
    pub at: u64,
    /// The row this grades — its `judged` fingerprint, spelled as text.
    pub label: String,
    /// Fingerprint of the file.
    pub path: String,
    /// The verdict the row carried.
    pub verdict: String,
    pub applied: bool,
    /// Whether the verdict called what became of the patch.
    pub agreed: bool,
    /// Whether the seat's baseline — the one verdict the table holds it
    /// against — called it too (`zerocode_core::jev::summary::BASELINE_AGREED`,
    /// t-6342).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_agreed: Option<bool>,
    /// What settled it: `receipt` (a check green after the turn's last
    /// edit) and `window` (its turns passed quietly) are a patch that stood,
    /// `regret` one whose lines were edited again (`Hindsight::word`).
    pub hindsight: String,
    /// Turns after the patch at which the label was decided.
    pub turns_later: u32,
}

/// The seat a host installs on the runtime.
#[derive(Debug)]
pub struct PatchReviewJudge {
    cwd: PathBuf,
}

impl PatchReviewJudge {
    /// The seat for a project — its setting and its ledger live under the
    /// project's working directory.
    #[must_use]
    pub fn at(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
        }
    }
}

impl PatchReviewSeat for PatchReviewJudge {
    fn review(&self, ask: PatchAsk) -> BoxFuture<'_, PatchReview> {
        Box::pin(review_at(self.cwd.clone(), ask))
    }
}

/// The mode this project's patches are reviewed under, or `None` when they
/// are not reviewed at all: an ablation holding it out, an unreadable
/// setting, or a mode that asks nothing.
fn asking_mode(cwd: &Path) -> Option<JevMode> {
    if telemetry::attest_ablated(telemetry::HarnessFeature::PatchReview) {
        return None;
    }
    let Some(mode) = jev_patch_review_mode_from(&runtime::ConfigLoader::default_for(cwd)) else {
        telemetry::attest_failed(telemetry::HarnessFeature::PatchReview, FAIL_SETTINGS_UNAVAILABLE);
        return None;
    };
    if !mode.asks() {
        telemetry::attest_declined(telemetry::HarnessFeature::PatchReview, mode.key());
        return None;
    }
    Some(mode)
}

/// Review one patch on the road this project's setting names: acting, the
/// review is waited for inside the wall and its line handed back; recording,
/// it is asked beside the turn and nothing waits.
async fn review_at(cwd: PathBuf, ask: PatchAsk) -> PatchReview {
    let Some(mode) = asking_mode(&cwd) else {
        return PatchReview::default();
    };
    // The standing is read only where it decides anything — `auto` — since
    // it reads the seat's whole ledger and a patch is a frequent thing.
    let acting = if mode.automatic() {
        mode.applies_with(runtime::jev_seat_applies(&cwd, &PATCH_REVIEW))
    } else {
        mode.applies()
    };
    let opened_at = cwd.clone();
    let Ok(door) = tokio::task::spawn_blocking(move || JevDoor::open(&opened_at)).await else {
        telemetry::attest_failed(telemetry::HarnessFeature::PatchReview, FAIL_SETTINGS_UNAVAILABLE);
        return PatchReview::default();
    };
    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
    // Everything the review reads off the machine is read here, before it may
    // be sent off on its own: the door, the key, the ledger's place.
    let ledger = patch_review_path(&cwd);
    watch(&cwd, &ask, &judged_key(&ask).to_string());
    let reviewed = review_and_write(cwd, ledger, door, client, ask, mode, acting);
    if acting {
        return reviewed.await;
    }
    detach(async move {
        let _ = reviewed.await;
    });
    PatchReview::default()
}

/// Ask, settle, write the row, and hand the verdict to the book — the whole
/// of one review, whether the turn waits on it or not.
async fn review_and_write(
    cwd: PathBuf,
    ledger: PathBuf,
    door: JevDoor,
    client: Option<SystemOneClient>,
    ask: PatchAsk,
    mode: JevMode,
    acting: bool,
) -> PatchReview {
    let (row, answers) = judge(&door, client.as_ref(), &ask).await;
    let (row, verdict, note) = settle(mode, acting, row, answers.as_ref());
    settle_verdict(&cwd, &ledger, &row.judged.to_string(), verdict, row.applied);
    // The row and the judge after it read and write the ledger — the whole of
    // it, for the judge — and nothing the result carries waits on either.
    drop(tokio::task::spawn_blocking(move || {
        let _ = append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES);
        let _ = judge_ledger(&ledger, super::decision_shadow::now_ms());
    }));
    PatchReview { note }
}

/// Run `review` to its end without anybody waiting on it: on the ambient
/// runtime when it has worker threads to run it, else on a thread of its own
/// — a current-thread runtime would run it only while somebody blocks on it.
pub(super) fn detach(review: impl Future<Output = ()> + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            drop(handle.spawn(review));
        }
        _ => drop(std::thread::spawn(move || api::sync_bridge::run_blocking(review))),
    }
}

/// What the row says and what the runtime is told: the verdict always; the
/// review's own reader ([`ROUTE_USE_APPLIED`]) and its line when it answered
/// and the mode acts; the mode's word when it answered and only records; the
/// tool alone when nothing answered.
fn settle(
    mode: JevMode,
    acting: bool,
    mut row: PatchReviewRow,
    answers: Option<&Answers>,
) -> (PatchReviewRow, Verdict, Option<String>) {
    let verdict = runtime::patch_review::verdict(answers);
    let answered = verdict != Verdict::Unavailable;
    row.verdict = verdict.word().to_string();
    row.applied = answered && acting;
    row.route_use = if row.applied {
        ROUTE_USE_APPLIED.to_string()
    } else if answered {
        mode.key().to_string()
    } else {
        ROUTE_USE_FALLBACK.to_string()
    };
    let line = answers.filter(|_| row.applied).and_then(note);
    row.noted = line.is_some();
    (row, verdict, line)
}

/// One review's row and its answers: refused at the door, or asked and
/// checked. Writes nothing.
pub(super) async fn judge(
    door: &JevDoor,
    client: Option<&SystemOneClient>,
    ask: &PatchAsk,
) -> (PatchReviewRow, Option<Answers>) {
    let mut row = PatchReviewRow::new(ask);
    let state = state(ask);
    let questions = questions();
    let request = SystemOneRequest {
        state: &state,
        model: SYSTEMONE_MODEL,
        questions: &questions,
    };
    let Some(body) = jev_gate::body_of(&request) else {
        let failure = SystemOneFailure::InvalidRequest;
        telemetry::attest_failed(telemetry::HarnessFeature::PatchReview, failure.token());
        row.outcome = failure.ledger_token();
        return (row, None);
    };
    let (cleared, client) = match (door.pass(&PATCH_REVIEW, client.is_some(), body), client) {
        (Ok(cleared), Some(client)) => (cleared, client),
        (passed, _) => {
            let refusal = passed.err().unwrap_or(Refused::NoKey);
            telemetry::attest_declined(telemetry::HarnessFeature::PatchReview, refusal.token());
            row.outcome = refusal.token().to_string();
            return (row, None);
        }
    };
    row.redacted_lines = u32::try_from(cleared.withheld_lines()).unwrap_or(u32::MAX);
    row.request_digest = Some(digest_of(
        PATCH_REVIEW.id,
        PATCH_REVIEW_RUBRIC_VERSION,
        door.model(),
        cleared.bytes(),
    ));
    let call = jev_gate::send(client, cleared, PATCH_REVIEW_DEADLINE, None).await;
    row.requests = call.requests;
    row.retries = call.retries;
    row.elapsed_ms = jev_gate::millis(call.elapsed);
    let response = match call.outcome {
        Ok(response) => response,
        Err(failure) => {
            telemetry::attest_failed(telemetry::HarnessFeature::PatchReview, failure.token());
            row.outcome = failure.ledger_token();
            return (row, None);
        }
    };
    row.model = Some(response.model.clone());
    row.input_tokens = Some(response.usage.input_tokens);
    match read(&response) {
        Ok(answers) => {
            telemetry::attest_fired(telemetry::HarnessFeature::PatchReview);
            row.outcome = PATCH_REVIEW_OUTCOME_ANSWERED.to_string();
            row.answers = Some(answers.by_id());
            (row, Some(answers))
        }
        Err(rejection) => {
            let failure = SystemOneFailure::Schema;
            telemetry::attest_failed(telemetry::HarnessFeature::PatchReview, failure.token());
            row.outcome = failure.ledger_token();
            row.rejected = Some(rejection.rule().to_string());
            (row, None)
        }
    }
}

/// Judge the seat on what it has just written, and write down a rise or a
/// fall — the one judge every seat that carries its own `agreed` marks takes
/// (`shadow_ledger::judge_seat_ledger`).
#[must_use]
pub fn judge_ledger(ledger: &Path, now_ms: i64) -> Option<promote::Verdict> {
    super::shadow_ledger::judge_seat_ledger(&PATCH_REVIEW, ledger, now_ms)
}

/* ---- the label: what became of the lines a patch wrote --------------------- */

/// One reviewed patch in the book: its hindsight so far, and its verdict
/// once the review has answered — which may be after the hindsight, when a
/// recording review is still on the wire at the turn's end.
#[derive(Debug, Clone)]
struct Waiting {
    label: String,
    watched: Watched,
    verdict: Option<Verdict>,
    applied: bool,
    decided: Option<Hindsight>,
}

type Book = HashMap<PathBuf, Vec<Waiting>>;

/// The patches still waiting on their label, per project. In memory and not
/// on disk, for the reason the other hindsight seats' books are: the mark is
/// whether THIS session's next turns went back to THIS patch's lines, and a
/// patch read back off a ledger row could be another session's. A process
/// that ends before a window closes labels nothing.
fn book() -> &'static Mutex<Book> {
    static BOOK: OnceLock<Mutex<Book>> = OnceLock::new();
    BOOK.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Put a patch in the book as it is asked about: its hindsight starts with
/// the turn it was written in, whatever the review is still doing.
fn watch(cwd: &Path, ask: &PatchAsk, label: &str) {
    if let Ok(mut book) = book().lock() {
        book.entry(cwd.to_path_buf()).or_default().push(Waiting {
            label: label.to_string(),
            watched: Watched::of(ask),
            verdict: None,
            applied: false,
            decided: None,
        });
    }
}

/// The review of the patch `label` names has answered — or has not. A review
/// that reached no verdict is taken out of the book: there is nothing to
/// grade. One whose hindsight is already in writes its label now.
fn settle_verdict(cwd: &Path, ledger: &Path, label: &str, verdict: Verdict, applied: bool) {
    let ready = {
        let Ok(mut book) = book().lock() else {
            return;
        };
        let Some(waiting) = book.get_mut(cwd) else {
            return;
        };
        let Some(at) = waiting.iter().position(|one| one.label == label) else {
            return;
        };
        if verdict == Verdict::Unavailable {
            waiting.remove(at);
            None
        } else {
            waiting[at].verdict = Some(verdict);
            waiting[at].applied = applied;
            waiting[at].decided.is_some().then(|| waiting.remove(at))
        }
    };
    if let Some(done) = ready {
        write_labels(ledger, vec![done]);
    }
}

/// Write this seat's hindsight for the turn that just ended, judged on `turn`
/// — the messages the turn appended, already in memory — against every patch
/// of `cwd`'s still waiting. `None` is a cancelled turn, which is not a turn
/// of any window: what it wrote is behind the next turn, and nothing is
/// labeled. Answers how many label rows were written.
#[must_use]
pub fn note_patch_review_turn(cwd: &Path, turn: Option<&[ConversationMessage]>) -> usize {
    label_turn(cwd, &patch_review_path(cwd), turn)
}

/// [`note_patch_review_turn`], writing to `ledger` — the seam a test hands a
/// path of its own, so no row lands in the person's home.
fn label_turn(cwd: &Path, ledger: &Path, turn: Option<&[ConversationMessage]>) -> usize {
    let done = {
        let Ok(mut book) = book().lock() else {
            return 0;
        };
        let Some(waiting) = book.get_mut(cwd) else {
            return 0;
        };
        let Some(turn) = turn else {
            for one in waiting.iter_mut() {
                one.watched.behind();
            }
            return 0;
        };
        let mut open: Vec<Watched> = waiting
            .iter()
            .filter(|one| one.decided.is_none())
            .map(|one| one.watched.clone())
            .collect();
        let decided = hindsight_of_turn(&mut open, turn);
        for one in waiting.iter_mut().filter(|one| one.decided.is_none()) {
            if let Some(now) = open.iter().find(|now| now.tool_use_id == one.watched.tool_use_id) {
                one.watched = now.clone();
            }
        }
        for (watched, hindsight) in decided {
            if let Some(one) = waiting
                .iter_mut()
                .find(|one| one.watched.tool_use_id == watched.tool_use_id)
            {
                one.watched = watched;
                one.decided = Some(hindsight);
            }
        }
        let (done, still): (Vec<Waiting>, Vec<Waiting>) = waiting
            .drain(..)
            .partition(|one| one.decided.is_some() && one.verdict.is_some());
        *waiting = still;
        if waiting.is_empty() {
            book.remove(cwd);
        }
        done
    };
    write_labels(ledger, done)
}

/// The verdict the seat's baseline would have given every patch — the table's
/// own word (`PATCH_REVIEW.baseline`), read rather than spelled here.
fn baseline_verdict() -> Option<Verdict> {
    let Baseline::AlwaysSame(word) = PATCH_REVIEW.baseline else {
        return None;
    };
    [Verdict::Permit, Verdict::ProposalOnly]
        .into_iter()
        .find(|verdict| verdict.word() == word)
}

/// One label row per decided review, and the judge run over what they add.
fn write_labels(ledger: &Path, done: Vec<Waiting>) -> usize {
    let at = super::decision_shadow::unix_millis();
    let rows: Vec<PatchReviewLabelRow> = done
        .into_iter()
        .filter_map(|one| {
            let (verdict, hindsight) = (one.verdict?, one.decided?);
            Some(PatchReviewLabelRow {
                kind: runtime::LABEL_ROW_KIND.to_string(),
                at,
                label: one.label,
                path: fingerprint_of(&one.watched.path),
                verdict: verdict.word().to_string(),
                applied: one.applied,
                agreed: hindsight.agrees_with(verdict)?,
                baseline_agreed: baseline_verdict().and_then(|baseline| hindsight.agrees_with(baseline)),
                hindsight: hindsight.word().to_string(),
                turns_later: one.watched.turns,
            })
        })
        .collect();
    if rows.is_empty() {
        return 0;
    }
    let written = rows
        .iter()
        .filter(|row| append_shadow_row(ledger, row, SHADOW_LEDGER_MAX_BYTES).is_ok())
        .count();
    let _ = judge_ledger(ledger, super::decision_shadow::now_ms());
    written
}

/// The book for `cwd`, emptied — for a test that begins from nothing.
#[cfg(test)]
fn forget_waiting(cwd: &Path) {
    if let Ok(mut book) = book().lock() {
        book.remove(cwd);
    }
}

#[cfg(test)]
mod tests;
