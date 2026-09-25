//! The challenger arm's caller: one eligible spawn in five asks a model
//! nobody has evidence for the same design the routed model is about to
//! carry out, and puts the two to a blind comparison through the Jev door
//! (t-6263; the pure half is `zerocode_core::jev::challenger`).
//!
//! Where it stands in a spawn (`agent_tools::spawn::run_agent_job`):
//!
//! 1. **At the attempt's start** ([`Arm::open`]): the person's mode word and
//!    the cheap holds — a retry, a guarded flow, a person's pin, a role the
//!    arm does not touch, the four attempts in five that do not draw — read
//!    on the spawn's thread, which is all the spawn ever pays. Everything
//!    else runs on a thread of its own ([`Arm::draw`]): the task's head
//!    cleared by the seat's own door (no key, no consent, no budget: nothing
//!    is spent on a design nobody will judge), the challenger chosen from
//!    the connected inventory ([`pick_challenger`]), priced from the one
//!    price table, the day's share read and reserved under one lock
//!    ([`Reservation`]) — and then, the moment before the design request
//!    leaves, the person's word and the door asked again as they stand NOW,
//!    and only the words that door cleared sent ([`cleared_task`]).
//! 2. **After the attempt's first turn** ([`Drawn::designs_ready`]): the
//!    incumbent's design is its own first plan — the first text it wrote
//!    before its first tool call ([`first_design_text`]) — frozen once. The
//!    challenger's design is joined, and — the person's word, the door and
//!    the key read again, a first turn after the draw read them — both are
//!    put to the judge under no name in the blind's order; the reservation
//!    is settled at what the design and the comparison actually cost, and
//!    the row is written. A design that left before the person took the
//!    word back is settled at what it cost, and nothing more is sent.
//!    Nothing the attempt does waits on any of it: the incumbent's design is
//!    what runs, every time.
//! 3. **When a verdict lands** ([`note_challenger_verdicts`]): the
//!    verification loop's receipt for the attempt — a `verdict` row about
//!    that attempt's work, naming as the source it saw the very source the
//!    attempt handed in ([`OnRecord::receipt`]) — writes the label, with
//!    when that verdict was recorded. What a verifier saw is taken when it
//!    starts and held until its turns end ([`SourceWatch`]): a tree written
//!    under it, even back to the bytes it had — any file the tree is written
//!    from, the index's or not — is no source it saw. The verdict carries it
//!    whenever the comparison lands, so a label follows the later of the
//!    two. A finished attempt is not a receipt; a verifier that settled
//!    nothing, or that judged some other source, labels nothing; and a label
//!    that names no source, one that proves no binding once the record has
//!    forgotten the run, or one the record contradicts — now, or before it
//!    forgot the rows that said so, which a strike keeps, and which this
//!    process holds until the ledger shows it back — is read as no label at
//!    all ([`binding`], [`bound_rows`]).
//! 4. **Acting** (`auto` once the seat's own evidence has raised it, or a
//!    person's `on`): a labelled comparison whose receipt and judge agree
//!    becomes one verified sample of the challenger in the route-outcome
//!    ledger, under a `challenger` source and an attempt key of its own,
//!    while the seat acts and the challenger's standing for the role passes
//!    the incumbent's own learned rate — written once, and written late by
//!    the next call when a crash or a refused write left it out
//!    ([`note_verdicts_in`]). Every learner reads a sample only while that
//!    stays true at the moment it reads ([`admit_samples`]): switched off or
//!    fallen, the samples stay in the ledger and teach nothing; raised
//!    again, the same rows count under their own decay.
//!
//! What leaves the machine: the head of the task, as the door clears it,
//! and one design, to the challenger, on the person's own provider
//! credentials — a model the router could have routed the role to, from the
//! same connected inventory — and the task's head with two designs, to the
//! door. The design request is filed in the route-outcome ledger as tax
//! ([`RouteTaxCall::Challenger`]), which the learner already skips, so a
//! paragraph never teaches the router.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use api::{
    InputMessage, MessageRequest, ModelPrice, OutputContentBlock, SystemBlock, SystemOneClient,
    SystemOneConfig, SystemOneFailure, SystemOneRate, SYSTEMONE_MODEL,
};
use runtime::{
    ContentBlock, ConversationMessage, DecisionKind, LearnedSpecialtyHint, MessageRole, ModelBand,
    ModelInventory, RouteOutcomeRecord, RouteRole, RouteTaskRisk, RouteTaxCall, VerdictSubject,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use zerocode_core::jev::challenger::spend::{self, Book, Op};
use zerocode_core::jev::challenger::{
    self as arm, Attempt, Comparison, Designs, Held, Preferred, Receipt, Receipted, ATTEMPT,
    CHALLENGER_MODEL, COST_MICROS, INCUMBENT_MODEL, ROLE,
};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::summary::{AGREED, LABEL, OUTCOME};
use zerocode_core::jev::{
    count, digest_of, door, fingerprint_of, JevMode, CHALLENGER, CHALLENGER_APPLY_DEADLINE_MS,
    CHALLENGER_DESIGN_BYTE_CAP, CHALLENGER_DESIGN_CAP, CHALLENGER_DESIGN_MAX_TOKENS,
    CHALLENGER_DESIGN_WALL_MS,
};

use super::jev_gate::{self, JevDoor};
use super::settings::jev_challenger_mode_from;
use super::shadow_ledger::{
    append_shadow_row, judge_seat_rows, shadow_ledger_path, try_read_shadow_rows, SHADOW_LEDGER_MAX_BYTES,
};

/// The seat's ledger file — the Jev use table's name for it.
pub const CHALLENGER_FILE: &str = CHALLENGER.ledger;

/// The version of the comparison's question — the words the judge is asked
/// (`zerocode_core::jev::challenger::ask`) and the two designs' shape: the
/// core question table's own number, never a copy. Every row that asked
/// names it ([`ChallengerRow::rubric_version`]) and its request digest
/// carries it, so a later reading of the question starts a window of its
/// own (t-6263 R5).
pub const CHALLENGER_RUBRIC_VERSION: u32 = zerocode_core::jev::questions::CHALLENGER_RUBRIC_VERSION;

/// The wall one comparison waits — the use table's own number.
const COMPARISON_DEADLINE: Duration = Duration::from_millis(CHALLENGER_APPLY_DEADLINE_MS);
const _: () = assert!(
    matches!(CHALLENGER.apply_deadline_ms, Some(CHALLENGER_APPLY_DEADLINE_MS)),
    "the seat's row names the wall its comparison waits"
);

/// The wall one design request waits — the use table's own number.
const DESIGN_WALL: Duration = Duration::from_millis(CHALLENGER_DESIGN_WALL_MS);

/// What the challenger is told: a design, not a patch — the same yardstick
/// the judge applies (`zerocode_core::jev::challenger`'s question), so the
/// two designs are answers to one question.
const DESIGN_INSTRUCTION: &str = "You are asked for a design only. In one paragraph, state how \
you would carry out the request below: what you would change, where, and how you would verify \
it. Make the fewest assumptions the request allows and name the ones you make. Do not write \
code, do not run anything, and do not ask questions.";

/// The `routeSource` a verified challenger sample carries — never `auto`,
/// so a sample the arm wrote can be told from a route the selector took.
pub const CHALLENGER_ROUTE_SOURCE: &str = RouteTaxCall::Challenger.as_str();

/// The route target a verified challenger sample is filed under: a key of
/// its own, like the tax's, so it never lands in a work route's feedback
/// bucket — the learner pools by role, and reads it there.
const SAMPLE_TARGET: &str = CHALLENGER_ROUTE_SOURCE;

/// The attempt key a verified challenger sample carries: the compared
/// attempt's, marked — so the learning mask counts it as one sample of its
/// own and never as the incumbent's verdict on that attempt.
#[must_use]
pub fn sample_attempt_key(attempt: &str) -> String {
    format!("{attempt}~{CHALLENGER_ROUTE_SOURCE}")
}

/// Where a project's challenger ledger lives.
#[must_use]
pub fn challenger_path(cwd: &Path) -> PathBuf {
    shadow_ledger_path(cwd, CHALLENGER_FILE)
}

/* ---- what a spawn knows at its start ---------------------------------- */

/// What the spawn knows of the attempt when it starts — everything the
/// holds, the draw and the design request read. Owned, because the draw
/// runs on a thread of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttemptFacts {
    /// `<agentId>#<runGeneration>` (`runtime::spawn_attempt_key`).
    pub key: String,
    /// The route role's key, as the manifest carries it; `None` when the
    /// router decided nothing for this spawn.
    pub role: Option<String>,
    /// The route's risk label, likewise.
    pub risk: Option<String>,
    /// How the model was chosen (`routeSource`).
    pub route_source: Option<String>,
    /// The model the attempt acts on.
    pub incumbent_model: String,
    /// The task in the person's words — the spawn's prompt.
    pub task: String,
    /// The effort the route recommended, asked of the challenger too: the
    /// two designs are answers under the same constraint.
    pub effort: Option<api::EffortLevel>,
    /// A resume, a retry or a repair round.
    pub retry_or_handover: bool,
}

impl AttemptFacts {
    pub(crate) fn attempt(&self) -> Attempt<'_> {
        Attempt {
            key: &self.key,
            role: self.role.as_deref().unwrap_or_default(),
            retry_or_handover: self.retry_or_handover,
            guarded: self
                .risk
                .as_deref()
                .and_then(RouteTaskRisk::from_label)
                .is_some_and(|risk| matches!(risk, RouteTaskRisk::High | RouteTaskRisk::Critical)),
            pinned: self.route_source.as_deref().is_some_and(super::route_source_is_a_persons),
        }
    }
}

/* ---- the design request ------------------------------------------------ */

/// One design request, as the challenger is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DesignRequest {
    pub model: String,
    /// The head of the task exactly as the seat's door cleared it the
    /// moment before it left ([`cleared_task`]): every line that may carry a
    /// credential withheld, cut to [`zerocode_core::jev::CHALLENGER_TASK_CHAR_CAP`]
    /// characters. No other words of the person's are in a design request.
    pub task_head: String,
    pub effort: Option<api::EffortLevel>,
    pub max_tokens: u32,
}

/// What came back: the design, what it cost in tokens, whether the request
/// left the machine at all, and the call's own ending in the outcome
/// ledger's words.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DesignReply {
    pub text: Option<String>,
    pub usage: Option<api::Usage>,
    /// Whether the request was sent. One that never left (no client for the
    /// model, its provider parked behind a wall) cost nothing and releases
    /// its reservation; one that left and reported no usage may have been
    /// billed and settles at its ceiling.
    pub left: bool,
    /// `completed`, `failed` or `stopped` (a wall).
    pub status: &'static str,
    pub elapsed_ms: u64,
}

impl DesignReply {
    fn missing(left: bool, status: &'static str, elapsed_ms: u64) -> Self {
        Self {
            text: None,
            usage: None,
            left,
            status,
            elapsed_ms,
        }
    }
}

/// Who answers a design request — the provider wire in the product, a
/// scripted answer in a test. Blocking: it runs on the arm's own thread.
pub(crate) trait Designer: Send + Sync {
    fn design(&self, request: &DesignRequest) -> DesignReply;
}

/// The product's designer: the provider client the spawn path builds for
/// any model, one `send_message` inside [`DESIGN_WALL`].
struct WireDesigner;

impl Designer for WireDesigner {
    fn design(&self, request: &DesignRequest) -> DesignReply {
        let model = api::resolve_model_alias(&request.model);
        // A provider parked behind its own wall would only hear the wall
        // again: nothing is sent, and the reservation is released.
        if api::quota::rate_limit_cooldown_remaining_ms(api::detect_provider_kind(&model)) > 0 {
            return DesignReply::missing(false, runtime::OUTCOME_STOPPED, 0);
        }
        // The same bare client the routing probe sends on: no prompt-cache
        // recorder, so the request is never a row of the day's request
        // ledgers — the arm's spend enters the day's whole once, from its
        // own book ([`spend::day_spend`]).
        let Ok(client) = crate::misc_tools::agent_tools::build_provider_client_for_agent(&model) else {
            return DesignReply::missing(false, runtime::OUTCOME_FAILED, 0);
        };
        let message = MessageRequest {
            model,
            max_tokens: request.max_tokens,
            messages: vec![InputMessage::user_text(request.task_head.clone())],
            system: Some(vec![SystemBlock::Text {
                text: DESIGN_INSTRUCTION.to_string(),
                cache_control: None,
            }]),
            tools: None,
            tool_choice: None,
            stream: false,
            thinking: None,
            output_config: None,
            effort: request.effort,
            effort_band_ceiling: None,
        };
        let started = Instant::now();
        let handle = crate::misc_tools::agent_tools::shared_agent_runtime().handle().clone();
        let outcome = handle.block_on(async {
            tokio::time::timeout(DESIGN_WALL, client.send_message(&message)).await
        });
        let elapsed_ms = jev_gate::millis(started.elapsed());
        match outcome {
            Ok(Ok(response)) => {
                let text: String = response
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        OutputContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect();
                DesignReply {
                    text: Some(text.trim().to_string()).filter(|text| !text.is_empty()),
                    usage: Some(response.usage),
                    left: true,
                    status: runtime::OUTCOME_COMPLETED,
                    elapsed_ms,
                }
            }
            Ok(Err(_)) => DesignReply::missing(true, runtime::OUTCOME_FAILED, elapsed_ms),
            Err(_) => DesignReply::missing(true, runtime::OUTCOME_STOPPED, elapsed_ms),
        }
    }
}

/// The key a request body keeps what the seat's `sends` describe under —
/// the `/state/…` of every pointer in the seat's row.
const STATE_KEY: &str = "state";

/// The head of `task` as it may leave for the challenger's provider, asked
/// of `door` as it stands: the seat's own door and the seat's own row, so
/// the words a design request carries are cleared exactly as the
/// comparison's are — the key, the switch, the workspace's consent and the
/// day's budget asked, every line that may carry a credential withheld
/// (the one table every such road reads), the rest cut to the task's cap.
/// Nothing is counted in the day: the request goes to the person's own
/// provider, not to the judge. Answers the words and how many lines were
/// withheld from them.
///
/// # Errors
/// The door's refusal: nothing may leave.
pub(crate) fn cleared_task(door: &JevDoor, key: bool, task: &str) -> Result<(String, usize), Refused> {
    let body = json!({ STATE_KEY: { arm::STATE_KEYS[0]: task } });
    let cleared = door.clear(&CHALLENGER, key, body)?;
    let body: Value = serde_json::from_slice(cleared.bytes()).unwrap_or(Value::Null);
    let words = body[STATE_KEY][arm::STATE_KEYS[0]].as_str().unwrap_or_default().to_string();
    Ok((words, cleared.withheld_lines()))
}

/// What a design of `task_head` is expected to cost at `price`, as the
/// share is charged: every byte of the request a token — the bound that
/// holds for any byte-level tokenizer in any script — and the whole
/// `max_tokens` out, which the provider enforces. A ceiling, not an
/// estimate; what the design actually cost settles it ([`settled_design_micros`]).
#[must_use]
pub(crate) fn expected_design_micros(price: &ModelPrice, task_head: &str) -> u64 {
    let input_bound = u64::try_from(task_head.len() + DESIGN_INSTRUCTION.len()).unwrap_or(u64::MAX);
    let output_bound = u64::try_from(CHALLENGER_DESIGN_MAX_TOKENS).unwrap_or(u64::MAX);
    arm::expected_micros(input_bound, output_bound, price.input, price.output)
}

/// What the comparison is expected to cost at the judge's `rate`, as the
/// share is charged: the request with both designs empty, and each design
/// at the most the door lets through — a byte a token, as above. A System
/// One call bills input only.
#[must_use]
pub(crate) fn expected_comparison_micros(rate: &SystemOneRate, empty_request_bytes: usize) -> u64 {
    let bound = empty_request_bytes.saturating_add(CHALLENGER_DESIGN_CAP * CHALLENGER_DESIGN_BYTE_CAP);
    arm::expected_micros(u64::try_from(bound).unwrap_or(u64::MAX), 0, rate.input, 0.0)
}

/// What the comparison cost at `rate`, one send at a time: the send the
/// judge answered at the input it reported, and every other send that left
/// — a failed try the wire re-sent, a request that never came back — at the
/// bytes that left, a byte a token. A send whose bill nobody reported is
/// charged what it could have cost, never nothing: whether a refused or
/// failed request is billed is the vendor's to say, and it does not say.
/// Nothing for a comparison the door never let out. The reservation is one
/// request's ceiling, so a comparison the wire had to re-send settles above
/// it, and the next draw's share reads what it cost.
#[must_use]
fn settled_comparison_micros(rate: &SystemOneRate, wire: &Wire) -> u64 {
    if wire.requests == 0 {
        return 0;
    }
    let sent = u64::try_from(wire.sent_bytes).unwrap_or(u64::MAX);
    let (answered, unreported) = match wire.input_tokens {
        Some(tokens) => (tokens, wire.requests - 1),
        None => (0, wire.requests),
    };
    let tokens = answered.saturating_add(sent.saturating_mul(u64::from(unreported)));
    arm::expected_micros(tokens, 0, rate.input, 0.0)
}

/// What a design actually cost at `price`, from the usage the wire reported
/// — or, for a request that left and reported nothing, what was reserved for
/// it: a design that failed after leaving may have been billed, and the share
/// is charged the ceiling rather than nothing.
#[must_use]
pub(crate) fn settled_design_micros(price: &ModelPrice, reply: &DesignReply, expected: u64) -> u64 {
    match &reply.usage {
        Some(usage) => {
            let plain = arm::expected_micros(
                u64::from(usage.input_tokens),
                u64::from(usage.output_tokens),
                price.input,
                price.output,
            );
            let cached = arm::expected_micros(
                u64::from(usage.cache_read_input_tokens),
                u64::from(usage.cache_creation_input_tokens),
                price.cache_read,
                price.cache_write,
            );
            plain.saturating_add(cached)
        }
        None => expected,
    }
}

/* ---- choosing the challenger ----------------------------------------- */

/// The model to challenge `incumbent` with for `role`, and its price.
///
/// From `inventory` — the connected models the router itself may route to,
/// so no task leaves for a provider the person has not connected — every
/// model that is not the incumbent, stands in a band the router routes work
/// to (not a provider's top, which plans and verifies, and not a release its
/// successor superseded), and is short of evidence for the role: no learned
/// entry at the learner's own floor (`LearnedSpecialtyHint::compute` says
/// nothing of a pair under it). Newly discovered releases come first,
/// newest first as discovery orders them; then the newer release by the
/// catalog's own rank; then by name, so the same day's inventory picks the
/// same challenger. The first with a row in the price table wins; a
/// candidate the table does not price is stepped over, because unknown is
/// not free.
///
/// # Errors
/// [`Held::NoChallenger`] when nothing qualifies; [`Held::Unpriced`] when
/// something did and none of it is priced.
pub(crate) fn pick_challenger(
    inventory: &ModelInventory,
    records: &[RouteOutcomeRecord],
    now_secs: u64,
    role: RouteRole,
    incumbent: &str,
    discovered: &[String],
    price_of: impl Fn(&str) -> Option<ModelPrice>,
) -> Result<(String, ModelPrice), Held> {
    let canonical = super::canonicalize_route_model_id;
    let incumbent_id = canonical(incumbent);
    let learned = LearnedSpecialtyHint::compute(records, now_secs, canonical);
    let mut candidates: Vec<&runtime::ModelDescriptor> = inventory
        .models()
        .iter()
        .filter(|model| canonical(model.id()) != incumbent_id)
        .filter(|model| !matches!(model.band(), ModelBand::Top | ModelBand::Superseded))
        .filter(|model| learned.entry_for(role, &canonical(model.id())).is_none())
        .collect();
    if candidates.is_empty() {
        return Err(Held::NoChallenger);
    }
    let discovered_rank = |id: &str| discovered.iter().position(|new| new == id);
    candidates.sort_by(|left, right| {
        let newness = |model: &runtime::ModelDescriptor| discovered_rank(model.id()).unwrap_or(usize::MAX);
        newness(left)
            .cmp(&newness(right))
            .then_with(|| right.release_rank_value().cmp(&left.release_rank_value()))
            .then_with(|| left.id().cmp(right.id()))
    });
    candidates
        .iter()
        .find_map(|model| price_of(model.id()).map(|price| (model.id().to_string(), price)))
        .ok_or(Held::Unpriced)
}

/* ---- the day's share ------------------------------------------------- */

/// The local day and its start, on this machine's clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Today {
    pub day: String,
    pub start_ms: u64,
    pub now_ms: u64,
}

impl Today {
    fn now() -> Self {
        let now_ms = u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
        )
        .unwrap_or(u64::MAX);
        let offset_minutes = i32::try_from(core_types::date::local_utc_offset_secs() / 60).unwrap_or(0);
        Self::at(now_ms, offset_minutes)
    }

    /// The day `now_ms` falls on for a clock `offset_minutes` ahead of UTC.
    #[must_use]
    pub(crate) fn at(now_ms: u64, offset_minutes: i32) -> Self {
        let now = i64::try_from(now_ms).unwrap_or(i64::MAX);
        let day = count::day_of(now, offset_minutes);
        let local = now.saturating_add(i64::from(offset_minutes) * 60_000);
        let day_start_local = local.div_euclid(86_400_000) * 86_400_000;
        let start = day_start_local.saturating_sub(i64::from(offset_minutes) * 60_000);
        Self {
            day,
            start_ms: u64::try_from(start).unwrap_or(0),
            now_ms,
        }
    }
}

/// The day's model spend outside the arm, in micro-dollars: every request
/// row the prompt caches under `roots` recorded since `start_ms`, priced by
/// `price_of` — the one price table — with a row the table does not price
/// left out (an unpriced row shrinks the share rather than padding it).
/// Only ledgers touched today are read: a session's ledger is append-only
/// and its file time is its last request's.
#[must_use]
pub(crate) fn day_request_micros(
    roots: &[PathBuf],
    start_ms: u64,
    price_of: impl Fn(&str) -> Option<ModelPrice>,
) -> u64 {
    let mut total: u64 = 0;
    for root in roots {
        let Ok(sessions) = std::fs::read_dir(root) else {
            continue;
        };
        for session in sessions.flatten() {
            let ledger = session.path().join(REQUESTS_LEDGER);
            if !touched_since(&ledger, start_ms) {
                continue;
            }
            for row in api::read_request_ledger(&ledger) {
                if row.ts_unix_ms < start_ms {
                    continue;
                }
                let Some(price) = price_of(&row.model) else {
                    continue;
                };
                let plain = arm::expected_micros(
                    u64::from(row.input_uncached),
                    u64::from(row.output),
                    price.input,
                    price.output,
                );
                let cached = arm::expected_micros(
                    u64::from(row.cache_read),
                    u64::from(row.cache_creation),
                    price.cache_read,
                    price.cache_write,
                );
                total = total.saturating_add(plain).saturating_add(cached);
            }
        }
    }
    total
}

/// The per-session request ledger's file name, as `api::PromptCachePaths`
/// spells it (`requests_path`).
const REQUESTS_LEDGER: &str = "requests.jsonl";

fn touched_since(path: &Path, start_ms: u64) -> bool {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .is_some_and(|since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX) >= start_ms)
}

/// The day file's reservations and settlements, read and written under one
/// lock — so two attempts drawn in the same moment cannot each see a share
/// with room for one and both take it.
pub(crate) struct Reservation {
    day: String,
    path: PathBuf,
}

impl Reservation {
    /// The book for `day` under `config_home`.
    #[must_use]
    pub(crate) fn for_day(config_home: &Path, day: &str) -> Self {
        Self {
            day: day.to_string(),
            path: spend::spend_path(config_home, day),
        }
    }

    /// The day's book as it stands: `None` when it cannot be read — which
    /// is not an empty day.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn book(&self) -> Option<Book> {
        self.text().ok().map(|text| spend::fold(&text))
    }

    /// The book's lines: none for a day nothing has been written in yet,
    /// and an error for a book that is there and cannot be read.
    fn text(&self) -> io::Result<String> {
        match std::fs::read_to_string(&self.path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
            read => read,
        }
    }

    /// Read the book, ask the share, and reserve `expected` for `attempt` if
    /// it fits — one critical section, under the cross-process lock the
    /// settings file already uses ([`runtime::SettingsFileLock`], which takes
    /// a lock back from an owner that died holding it), so the read and the
    /// write are one step. `other_micros` is the day's spend outside the arm,
    /// summed by the caller for this book's day before the lock is taken.
    ///
    /// The day is asked again under the lock (`clock`): a draw that read its
    /// day before midnight and reaches the book after it would be charging a
    /// day that has ended against a spend summed for it, so it refuses —
    /// the day that began has its own share, which nothing was summed for.
    /// The first reservation of a day forgets the books of earlier days that
    /// nothing will settle into again ([`spend::forget_settled_days`]), and
    /// never a later day's.
    ///
    /// # Errors
    /// [`Held::Retry`] for an attempt the book already names — a second
    /// opening of the same attempt is not a second draw; [`Held::DayBudget`]
    /// when the share has no room, the day has moved on, or the lock or the
    /// book could not be taken or read — a budget that cannot be kept
    /// refuses, and a book that cannot be read is not an empty day.
    pub(crate) fn reserve(
        &self,
        attempt: &str,
        expected: u64,
        other_micros: u64,
        clock: &dyn Fn() -> Today,
    ) -> Result<Book, Held> {
        let _lock = runtime::SettingsFileLock::acquire(&self.path).map_err(|_| Held::DayBudget)?;
        if clock().day != self.day {
            return Err(Held::DayBudget);
        }
        let text = self.text().map_err(|_| Held::DayBudget)?;
        if spend::names(&text, attempt) {
            return Err(Held::Retry);
        }
        let before = spend::fold(&text);
        let day = spend::day_spend(&before, other_micros);
        if !arm::within_day_budget(&day, expected) {
            return Err(Held::DayBudget);
        }
        self.append(&spend::line(attempt, Op::Reserve, expected))
            .map_err(|_| Held::DayBudget)?;
        if text.is_empty() {
            spend::forget_settled_days(&self.path);
        }
        Ok(Book {
            reserved_micros: before.reserved_micros.saturating_add(expected),
            reserved: before.reserved.saturating_add(1),
            ..before
        })
    }

    /// Settle `attempt` at what it cost. Appends only: a settlement checks
    /// no share.
    pub(crate) fn settle(&self, attempt: &str, micros: u64) {
        let _ = self.append(&spend::line(attempt, Op::Settle, micros));
    }

    /// Release `attempt`'s reservation: nothing left the machine.
    pub(crate) fn release(&self, attempt: &str) {
        let _ = self.append(&spend::line(attempt, Op::Release, 0));
    }

    fn append(&self, line: &str) -> io::Result<()> {
        use std::io::Write as _;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())
    }
}

/* ---- the arm ----------------------------------------------------------- */

/// Everything one project's arm reads off the machine, resolved once; the
/// tests hand in their own ([`Arm::with`]).
pub(crate) struct Arm {
    inner: Arc<Inner>,
}

pub(crate) type Clock = Box<dyn Fn() -> Today + Send + Sync>;

/// The person's mode word for the seat, as it stands at the moment it is
/// read — `None` when the settings cannot be read, which is off.
pub(crate) type ModeWord = Box<dyn Fn() -> Option<JevMode> + Send + Sync>;

struct Inner {
    cwd: PathBuf,
    config_home: PathBuf,
    /// Read at the attempt's start, again before a design leaves, again
    /// before a comparison does, and again before a sample is written: a
    /// word taken back mid-draw stops everything that has not left yet.
    mode: ModeWord,
    designer: Arc<dyn Designer>,
    inventory: Box<dyn Fn(&str) -> ModelInventory + Send + Sync>,
    discovered: Vec<String>,
    prompt_cache_roots: Vec<PathBuf>,
    price_of: fn(&str) -> Option<ModelPrice>,
    judge_rate_of: fn(&str) -> Option<SystemOneRate>,
    door: Box<dyn Fn() -> JevDoor + Send + Sync>,
    client: Box<dyn Fn() -> Option<SystemOneClient> + Send + Sync>,
    clock: Clock,
}

impl Clone for Arm {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl Arm {
    /// The arm for `cwd` as the product stands it: the person's own mode
    /// word, the connected inventory, discovery's newest ids, the door, the
    /// key, the provider wire and this machine's clock. Off, it reads the
    /// mode word and nothing else.
    #[must_use]
    pub(crate) fn live(cwd: &Path) -> Self {
        let mode_cwd = cwd.to_path_buf();
        let mode: ModeWord = Box::new(move || jev_challenger_mode_from(&runtime::ConfigLoader::default_for(&mode_cwd)));
        let discovered = if mode().is_some_and(JevMode::asks) {
            runtime::model_discovery::current()
                .map(|catalog| {
                    runtime::model_discovery::new_models(&catalog)
                        .into_iter()
                        .map(|model| model.id)
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let door_cwd = cwd.to_path_buf();
        Self {
            inner: Arc::new(Inner {
                cwd: cwd.to_path_buf(),
                config_home: runtime::default_config_home(),
                mode,
                designer: Arc::new(WireDesigner),
                inventory: Box::new(runtime::connected_model_inventory),
                discovered,
                prompt_cache_roots: api::prompt_cache_roots(),
                price_of: api::model_price,
                judge_rate_of: api::systemone_rate,
                door: Box::new(move || JevDoor::open(&door_cwd)),
                client: Box::new(|| SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client)),
                clock: Box::new(Today::now),
            }),
        }
    }

    /// An arm told exactly these facts, for a test that must not read the
    /// machine.
    #[cfg(test)]
    pub(crate) fn with(scene: Scene) -> Self {
        Self {
            inner: Arc::new(Inner {
                prompt_cache_roots: vec![scene.config_home.join("cache").join("prompt-cache")],
                cwd: scene.cwd,
                config_home: scene.config_home,
                mode: scene.mode,
                designer: scene.designer,
                inventory: scene.inventory,
                discovered: Vec::new(),
                price_of: tests::priced,
                judge_rate_of: api::systemone_rate,
                door: scene.door,
                client: scene.client,
                clock: scene.clock,
            }),
        }
    }

    fn ledger(&self) -> PathBuf {
        challenger_path(&self.inner.cwd)
    }

    fn write(&self, row: &impl Serialize) {
        let _ = append_shadow_row(&self.ledger(), row, SHADOW_LEDGER_MAX_BYTES);
    }

    fn now_ms(&self) -> u64 {
        (self.inner.clock)().now_ms
    }

    /// The person's word for the seat as it stands now, when it asks
    /// anything at all.
    fn asking_now(&self) -> Option<JevMode> {
        (self.inner.mode)().filter(|mode| mode.asks())
    }

    /// The attempt's start, on the spawn's own thread: the mode word and the
    /// cheap holds, and — for an attempt that clears them — the draw handed
    /// to a thread of its own. `None` for an attempt the arm does nothing
    /// for. A held row is written only for an attempt the draw picked: the
    /// four in five that did not draw say nothing, and neither does a role
    /// the arm never touches unless its attempt drew — one row per drawn
    /// attempt, never one per spawn.
    pub(crate) fn open(&self, facts: AttemptFacts) -> Option<Drawn> {
        self.asking_now()?;
        let attempt = facts.attempt();
        if let Err(held) = arm::eligible(&attempt) {
            if held != Held::NotDrawn && arm::draws(attempt.key) {
                self.write(&arm::held_row(&attempt, held, unix_millis_i64(self.now_ms())));
            }
            return None;
        }
        let drawing = self.clone();
        let asked = facts.clone();
        let work = std::thread::Builder::new()
            .name(format!("zo-challenger-{}", fingerprint_of(&facts.key)))
            .spawn(move || drawing.draw(&asked))
            .ok()?;
        Some(Drawn {
            arm: self.clone(),
            facts,
            work,
        })
    }

    /// Everything after the cheap holds, off the spawn's thread: the words
    /// the door would let out, the challenger, the price, the share, and —
    /// the person's word and the door asked once more as they stand at that
    /// moment — the design itself. `None` for a draw the arm held or the
    /// door refused: its word is already in the ledger, and a reservation it
    /// took is released.
    fn draw(&self, facts: &AttemptFacts) -> Option<Designed> {
        let attempt = facts.attempt();
        let at = unix_millis_i64(self.now_ms());
        let hold = |why: Held| {
            self.write(&arm::held_row(&attempt, why, at));
            None
        };
        let refuse = |refused: Refused| {
            self.write(&refused_row(&attempt, &facts.incumbent_model, refused, self.now_ms()));
            None
        };
        let door = (self.inner.door)();
        let (task_head, _) = match cleared_task(&door, (self.inner.client)().is_some(), &facts.task) {
            Ok(cleared) => cleared,
            Err(refused) => return refuse(refused),
        };
        let Some(judge_rate) = (self.inner.judge_rate_of)(door.model()) else {
            return hold(Held::Unpriced);
        };
        let role = RouteRole::from_key(attempt.role)?;
        let today = (self.inner.clock)();
        let records = learning_records_in(&self.inner.cwd, || (self.inner.mode)(), today.now_ms / 1_000);
        let inventory = (self.inner.inventory)(&facts.incumbent_model);
        let (challenger, price) = match pick_challenger(
            &inventory,
            &records,
            today.now_ms / 1_000,
            role,
            &facts.incumbent_model,
            &self.inner.discovered,
            self.inner.price_of,
        ) {
            Ok(picked) => picked,
            Err(why) => return hold(why),
        };
        let design_expected = expected_design_micros(&price, &task_head);
        let empty = arm::ask(&facts.key, &task_head, &Designs { incumbent: "", challenger: "" });
        let comparison_expected = expected_comparison_micros(&judge_rate, request_body(&empty).to_string().len());
        let expected = design_expected.saturating_add(comparison_expected);
        let other_micros = day_request_micros(&self.inner.prompt_cache_roots, today.start_ms, self.inner.price_of);
        let reservation = Reservation::for_day(&self.inner.config_home, &today.day);
        if let Err(why) = reservation.reserve(&facts.key, expected, other_micros, &*self.inner.clock) {
            return hold(why);
        }
        // The moment before the design leaves: the person's word and the door
        // as they stand now — a draw is a pick and a reservation older than
        // the words it is about to send — and only what that door cleared.
        let leaving = match self.asking_now() {
            Some(_) => cleared_task(&(self.inner.door)(), (self.inner.client)().is_some(), &facts.task),
            None => Err(Refused::Off),
        };
        let task_head = match leaving {
            Ok((words, _)) => words,
            Err(refused) => {
                reservation.release(&facts.key);
                return refuse(refused);
            }
        };
        let request = DesignRequest {
            model: challenger.clone(),
            task_head: task_head.clone(),
            effort: facts.effort,
            max_tokens: u32::try_from(CHALLENGER_DESIGN_MAX_TOKENS).unwrap_or(u32::MAX),
        };
        let reply = self.inner.designer.design(&request);
        record_design_tax(&self.inner.cwd, &facts.key, &challenger, &reply);
        if !reply.left {
            reservation.release(&facts.key);
            return hold(Held::NoDesign);
        }
        Some(Designed {
            challenger,
            price,
            design_expected,
            expected,
            task_head,
            day: today.day,
            reply,
        })
    }
}

/// What a test stands an arm on ([`Arm::with`]).
#[cfg(test)]
pub(crate) struct Scene {
    pub cwd: PathBuf,
    pub config_home: PathBuf,
    pub mode: ModeWord,
    pub designer: Arc<dyn Designer>,
    /// Read when the draw picks its challenger — after the words were first
    /// cleared, before the share is reserved and the design leaves.
    pub inventory: Box<dyn Fn(&str) -> ModelInventory + Send + Sync>,
    pub door: Box<dyn Fn() -> JevDoor + Send + Sync>,
    pub client: Box<dyn Fn() -> Option<SystemOneClient> + Send + Sync>,
    pub clock: Clock,
}

/// A draw that bought a design: what the comparison and the settlement
/// need. No door, key or word of the person's is kept here — the comparison
/// asks them again when it is made.
struct Designed {
    challenger: String,
    price: ModelPrice,
    design_expected: u64,
    expected: u64,
    /// The words the design request carried, as the door cleared them.
    task_head: String,
    /// The day the share was charged in — where the settlement goes, even
    /// when it lands after midnight.
    day: String,
    reply: DesignReply,
}

/// An attempt that cleared the cheap holds: its draw on a thread of its
/// own, and everything the comparison will need once the incumbent has
/// written its own plan.
pub(crate) struct Drawn {
    arm: Arm,
    facts: AttemptFacts,
    work: JoinHandle<Option<Designed>>,
}

impl Drawn {
    /// The incumbent's design is in hand (or is not): finish the comparison
    /// on a thread of its own. Nothing waits on it.
    pub(crate) fn designs_ready(self, incumbent_design: Option<String>) {
        let _ = std::thread::Builder::new()
            .name(format!("zo-challenger-judge-{}", fingerprint_of(&self.facts.key)))
            .spawn(move || self.finish(incumbent_design));
    }

    /// Join the draw; ask the person's word, the door and the key as they
    /// stand now, a first turn after the draw read them; put the two designs
    /// to the judge under no name; settle the share at what the design and
    /// the comparison cost; write the row; and label whatever receipts are
    /// already in, writing samples only as the seat stands now. A design
    /// that left and cannot be compared — the word taken back, consent
    /// withdrawn, the judge's model no longer priced — is settled at what it
    /// cost, and nothing more is sent. On the calling thread — the seam a
    /// test holds to await the row.
    pub(crate) fn finish(self, incumbent_design: Option<String>) {
        let Self { arm, facts, work } = self;
        let Ok(Some(designed)) = work.join() else {
            return;
        };
        let arm = &arm;
        let reservation = Reservation::for_day(&arm.inner.config_home, &designed.day);
        let design_cost = settled_design_micros(&designed.price, &designed.reply, designed.design_expected);
        let (Some(incumbent), Some(challenger)) = (incumbent_design, designed.reply.text.as_deref()) else {
            held_after_the_design(arm, &facts, &reservation, &designed, Held::NoDesign, design_cost);
            return;
        };
        let door = (arm.inner.door)();
        let Some(judge_rate) = (arm.inner.judge_rate_of)(door.model()) else {
            held_after_the_design(arm, &facts, &reservation, &designed, Held::Unpriced, design_cost);
            return;
        };
        let designs = Designs {
            incumbent: &incumbent,
            challenger,
        };
        let asked = arm::ask(&facts.key, &designed.task_head, &designs);
        let wire = match arm.asking_now() {
            Some(_) => compare(&door, (arm.inner.client)().as_ref(), &asked),
            None => Wire::silent(Refused::Off.token().to_string()),
        };
        let cost = design_cost.saturating_add(settled_comparison_micros(&judge_rate, &wire));
        reservation.settle(&facts.key, cost);
        let comparison = Comparison {
            attempt: &facts.key,
            role: facts.attempt().role,
            incumbent_model: &facts.incumbent_model,
            challenger_model: &designed.challenger,
            expected_micros: designed.expected,
            cost_micros: cost,
            incumbent_design: &fingerprint_of(&incumbent),
            challenger_design: &fingerprint_of(challenger),
            blind: asked.blind(),
            preferred: wire.preferred,
        };
        let row = ChallengerRow {
            at: arm.now_ms(),
            outcome: wire.outcome,
            rubric_version: CHALLENGER_RUBRIC_VERSION,
            elapsed_ms: wire.elapsed_ms,
            requests: wire.requests,
            retries: wire.retries,
            redacted_lines: wire.redacted_lines,
            model: wire.model,
            input_tokens: wire.input_tokens,
            request_digest: wire.request_digest,
            rejected: wire.rejected,
            confidence: wire.confidence,
            comparison: comparison.columns(),
        };
        arm.write(&row);
        let cwd = &arm.inner.cwd;
        // A record or a ledger that cannot be read is not an empty one: no
        // label is read off it, and the seat is judged the next time both can
        // be.
        if let Ok(records) = runtime::read_route_outcomes(cwd) {
            let now_ms = unix_millis_i64(arm.now_ms());
            if let Ok(rows) = bound_rows(&arm.ledger(), &OnRecord::of(&records), now_ms) {
                let _ = judge_seat_rows(&CHALLENGER, &arm.ledger(), &rows, now_ms);
            }
        }
        let _ = note_verdicts_in(cwd, (arm.inner.mode)(), &|sample| runtime::record_route_outcome(cwd, sample));
    }
}

/// A design that left and will not be compared: settled at what it cost,
/// and its word written beside the two models and the price.
fn held_after_the_design(
    arm: &Arm,
    facts: &AttemptFacts,
    reservation: &Reservation,
    designed: &Designed,
    why: Held,
    cost: u64,
) {
    reservation.settle(&facts.key, cost);
    let mut row = arm::held_row(&facts.attempt(), why, unix_millis_i64(arm.now_ms()));
    row[INCUMBENT_MODEL.canonical] = Value::from(facts.incumbent_model.as_str());
    row[CHALLENGER_MODEL.canonical] = Value::from(designed.challenger.as_str());
    row[COST_MICROS.canonical] = Value::from(cost);
    arm.write(&row);
}

fn unix_millis_i64(now_ms: u64) -> i64 {
    i64::try_from(now_ms).unwrap_or(i64::MAX)
}

/// The design request's own row in the route-outcome ledger: tax, billed to
/// the attempt that drew, under the challenger's model — bookkeeping the
/// learner skips ([`DecisionKind::is_bookkeeping`]). A request that never
/// left writes nothing.
fn record_design_tax(cwd: &Path, attempt: &str, challenger: &str, reply: &DesignReply) {
    if !reply.left {
        return;
    }
    let record = RouteOutcomeRecord::route_tax(
        RouteTaxCall::Challenger,
        super::canonicalize_route_model_id(challenger),
        reply.status,
    )
    .with_attempt_key(attempt)
    .with_duration_ms(Some(reply.elapsed_ms))
    .with_output_tokens(reply.usage.as_ref().map_or(0, |usage| u64::from(usage.output_tokens)));
    let _ = runtime::record_route_outcome(cwd, &record);
}

/* ---- the row ------------------------------------------------------------- */

/// One comparison's row: the wire's own columns beside the table's
/// ([`Comparison::columns`]). No words: the designs are fingerprints, the
/// task is not here at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChallengerRow {
    pub at: u64,
    /// The door's word for an answered comparison, a failure's ledger token
    /// or the door's refusal.
    pub outcome: String,
    /// The version of the question it asked ([`CHALLENGER_RUBRIC_VERSION`]),
    /// under the one key every seat's row names it by
    /// (`zerocode_core::jev::summary::RUBRIC_VERSION`).
    pub rubric_version: u32,
    pub elapsed_ms: u64,
    pub requests: u32,
    pub retries: u32,
    pub redacted_lines: u32,
    /// The Jev version that answered, as the response named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
    /// How sure the judge was of its choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
    #[serde(flatten)]
    pub comparison: Map<String, Value>,
}

/// A row for a comparison the door refused before anything was spent: a
/// request the seat counts as a refusal, naming the attempt and the
/// incumbent, with no challenger chosen yet.
fn refused_row(attempt: &Attempt<'_>, incumbent: &str, refused: Refused, now_ms: u64) -> ChallengerRow {
    ChallengerRow {
        at: now_ms,
        outcome: refused.token().to_string(),
        rubric_version: CHALLENGER_RUBRIC_VERSION,
        elapsed_ms: 0,
        requests: 0,
        retries: 0,
        redacted_lines: 0,
        model: None,
        input_tokens: None,
        request_digest: None,
        rejected: None,
        confidence: None,
        comparison: Map::from_iter([
            (ATTEMPT.canonical.to_string(), Value::from(attempt.key)),
            (ROLE.canonical.to_string(), Value::from(attempt.role)),
            (INCUMBENT_MODEL.canonical.to_string(), Value::from(incumbent)),
        ]),
    }
}

/// What the wire said of one comparison.
struct Wire {
    outcome: String,
    elapsed_ms: u64,
    requests: u32,
    retries: u32,
    redacted_lines: u32,
    /// Bytes the door let out — the comparison's bill when the judge reports
    /// no usage.
    sent_bytes: usize,
    model: Option<String>,
    input_tokens: Option<u64>,
    request_digest: Option<String>,
    rejected: Option<String>,
    confidence: Option<f64>,
    preferred: Option<Preferred>,
}

impl Wire {
    fn silent(outcome: String) -> Self {
        Self {
            outcome,
            elapsed_ms: 0,
            requests: 0,
            retries: 0,
            redacted_lines: 0,
            sent_bytes: 0,
            model: None,
            input_tokens: None,
            request_digest: None,
            rejected: None,
            confidence: None,
            preferred: None,
        }
    }
}

/// The request body a comparison is asked with, before the door clears it.
fn request_body(asked: &arm::ComparisonAsk) -> Value {
    json!({
        STATE_KEY: asked.state,
        "model": SYSTEMONE_MODEL,
        "questions": asked.questions,
    })
}

/// One comparison through the door and down the wire, read back unblinded.
/// Writes nothing.
fn compare(door: &JevDoor, client: Option<&SystemOneClient>, asked: &arm::ComparisonAsk) -> Wire {
    let (cleared, client) = match (door.pass(&CHALLENGER, client.is_some(), request_body(asked)), client) {
        (Ok(cleared), Some(client)) => (cleared, client),
        (passed, _) => {
            let refusal = passed.err().unwrap_or(Refused::NoKey);
            return Wire::silent(refusal.token().to_string());
        }
    };
    let mut wire = Wire::silent(String::new());
    wire.redacted_lines = u32::try_from(cleared.withheld_lines()).unwrap_or(u32::MAX);
    wire.sent_bytes = cleared.bytes().len();
    wire.request_digest = Some(digest_of(
        CHALLENGER.id,
        CHALLENGER_RUBRIC_VERSION,
        door.model(),
        cleared.bytes(),
    ));
    let call = api::sync_bridge::run_blocking(jev_gate::send(client, cleared, COMPARISON_DEADLINE, None));
    wire.requests = call.requests;
    wire.retries = call.retries;
    wire.elapsed_ms = jev_gate::millis(call.elapsed);
    let response = match call.outcome {
        Ok(response) => response,
        Err(failure) => {
            wire.outcome = failure.ledger_token();
            return wire;
        }
    };
    wire.model = Some(response.model.clone());
    wire.input_tokens = Some(response.usage.input_tokens);
    let answers = serde_json::to_value(&response.answers).unwrap_or(Value::Null);
    match asked.read(&answers) {
        Ok(answer) => {
            wire.outcome = door::ANSWERED_OUTCOME.to_string();
            wire.preferred = Some(answer.preferred);
            wire.confidence = Some(answer.confidence);
        }
        Err(refusal) => {
            wire.outcome = SystemOneFailure::Schema.ledger_token();
            wire.rejected = Some(refusal.token().to_string());
        }
    }
    wire
}

/* ---- the incumbent's design ------------------------------------------- */

/// The incumbent's design: the first text the attempt wrote before its
/// first tool call, in its own first turn — its plan, frozen once. `None`
/// when it acted before it planned (a tool call came first) or wrote
/// nothing: a plan that is not there is not compared with anything, and an
/// empty string would be a design the judge could only lose to.
#[must_use]
pub(crate) fn first_design_text(assistant_messages: &[ConversationMessage]) -> Option<String> {
    for message in assistant_messages {
        if message.role != MessageRole::Assistant {
            continue;
        }
        for block in &message.blocks {
            match block {
                ContentBlock::Text { text } => {
                    let text = text.trim();
                    if !text.is_empty() {
                        return Some(text.to_string());
                    }
                }
                ContentBlock::ToolUse { .. } => return None,
                _ => {}
            }
        }
    }
    None
}

/* ---- the source a receipt is about --------------------------------------- */

/// Whether an attempt's own run row should name the source it handed in
/// ([`source_of`]): the draw picked it and it cleared every cheap hold — the
/// only attempts a comparison is ever made for — and the seat asks. The pure
/// questions first and the settings only then, so an off seat's rows stay
/// as they were, to the byte, and cost nothing to leave so; a tree is
/// written for no attempt the arm held.
#[must_use]
pub(crate) fn hands_in_a_source(cwd: &Path, facts: &AttemptFacts) -> bool {
    arm::eligible(&facts.attempt()).is_ok() && asks_in(cwd)
}

/// Whether a verifier bound to `attempt` is watched for the source it reads
/// ([`SourceWatch`]): the draw picked the attempt, the seat asks, and the
/// attempt's own run row names the source it handed in — the one kind of
/// attempt a receipt is ever read for, known before its verifier starts,
/// whenever its comparison lands: before the verdict or after it. Pure
/// first, the settings next, the route-outcome ledger last.
#[must_use]
pub(crate) fn judges_a_source(cwd: &Path, attempt: &str) -> bool {
    arm::draws(attempt)
        && asks_in(cwd)
        && runtime::read_route_outcomes(cwd).is_ok_and(|records| handed_in_among(&rows_about(&records, attempt)).is_some())
}

/// Whether the person's word for the seat under `cwd` asks anything.
fn asks_in(cwd: &Path) -> bool {
    jev_challenger_mode_from(&runtime::ConfigLoader::default_for(cwd)).is_some_and(JevMode::asks)
}

/// The source state of the work under `work_dir`: the git tree of its whole
/// working state — tracked, changed and new files alike, written the way
/// the undo snapshot writes it (`runtime::git_snapshot::compute_worktree_tree`) — or
/// `None` outside a repository or when git cannot say. What an attempt hands
/// in and what a verifier sees, in one spelling, so a receipt can hold the
/// two to be the same work ([`OnRecord::receipt`]).
#[must_use]
pub fn source_of(work_dir: &Path) -> Option<String> {
    let root = runtime::git_snapshot::read_git_root(work_dir)?;
    runtime::git_snapshot::compute_worktree_tree(&root).ok()
}

/// A bound verifier's watch over the source it reads, from the moment before
/// its first turn to the end of its last: the tree of the work as the
/// verifier starts — written the way [`source_of`] writes it, on a thread of
/// its own while the verifier works, with every path it holds
/// ([`runtime::git_snapshot::WorktreeSource`]) — and the stamp of every file
/// a tree of the work can be written from
/// ([`runtime::git_snapshot::WorktreeStamp`]), taken again when its turns
/// are over. The two stamps alike, the tree written from the HEAD the first
/// one saw and every path it holds among the files it stamped
/// ([`runtime::git_snapshot::WorktreeStamp::vouches_for`]): nothing in the
/// tree was written in between — not even an edit put back to the bytes it
/// replaced — so whatever the verifier read, it read that tree. Any change,
/// a path the stamp did not watch, or a stamp that cannot be taken, and the
/// watch names no source: a verdict that cannot say it saw the work handed
/// in is no receipt for it ([`OnRecord::receipt`]). What happens to the
/// tree once its turns are over is not what it saw, and moves nothing.
pub(crate) struct SourceWatch {
    root: PathBuf,
    stamp: runtime::git_snapshot::WorktreeStamp,
    tree: JoinHandle<Option<runtime::git_snapshot::WorktreeSource>>,
}

impl SourceWatch {
    /// Start watching the work under `work_dir`, before the verifier reads
    /// anything. `None` outside a repository, or where its files cannot be
    /// stamped.
    #[must_use]
    pub(crate) fn open(work_dir: &Path) -> Option<Self> {
        let root = runtime::git_snapshot::read_git_root(work_dir)?;
        let stamp = runtime::git_snapshot::worktree_stamp(&root).ok()?;
        let hashing = root.clone();
        let tree = std::thread::Builder::new()
            .name("zo-challenger-source".to_string())
            .spawn(move || {
                let source = runtime::git_snapshot::compute_worktree_source(&hashing).ok();
                #[cfg(test)]
                tests::tree_written(&hashing);
                source
            })
            .ok()?;
        Some(Self { root, stamp, tree })
    }

    /// The verifier's turns are over: the tree it started on, when nothing
    /// in it changed since — `None` otherwise.
    #[must_use]
    pub(crate) fn seen(self) -> Option<String> {
        let source = self.tree.join().ok().flatten()?;
        let now = runtime::git_snapshot::worktree_stamp(&self.root).ok()?;
        self.stamp.vouches_for(&source, &now).then_some(source.tree)
    }
}

/* ---- the receipt and the label ----------------------------------------- */

/// `attempt`'s own rows of `records`.
fn rows_about<'r>(records: &'r [RouteOutcomeRecord], attempt: &str) -> Vec<&'r RouteOutcomeRecord> {
    records.iter().filter(|record| record.run_id.as_deref() == Some(attempt)).collect()
}

/// An attempt's own run row: its model's work, with nobody's signal about it.
fn is_run_row(record: &RouteOutcomeRecord) -> bool {
    record.decision_kind() == DecisionKind::Model && record.signal.is_none()
}

/// The source an attempt handed in, among its own rows: what its run row
/// names.
fn handed_in_among<'r>(rows: &[&'r RouteOutcomeRecord]) -> Option<&'r str> {
    rows.iter().copied().filter(|record| is_run_row(record)).find_map(|record| record.source.as_deref())
}

/// What the verification loop said of an attempt's work, read among that
/// attempt's own route-outcome rows, and the source it said it of: the first
/// settled `verdict` about the attempt whose verifier saw the very source
/// the attempt handed in — the source the attempt's own run row names. A
/// verifier's pass or failure of the WORK: never the verifier's own fault,
/// never a verifier that settled nothing, and never a verdict about another
/// source of the same attempt or about a source nobody named — what a
/// verdict judged is half of what it says, and a verdict that cannot say it
/// judged this work is no receipt for it. Completion is not here: a spawn's
/// own `completed` row is not a verdict.
fn receipt_among(rows: &[&RouteOutcomeRecord]) -> Option<Receipted> {
    let handed_in = handed_in_among(rows)?;
    let mut verdicts: Vec<&RouteOutcomeRecord> =
        rows.iter().copied().filter(|record| record.source.as_deref() == Some(handed_in)).collect();
    verdicts.sort_by_key(|record| record.recorded_at);
    verdicts.into_iter().find_map(|record| {
        Some(Receipted {
            receipt: settled_by(record)?,
            source: handed_in.to_string(),
            verdict_at: record.recorded_at,
        })
    })
}

/// What a `verdict` row about an attempt's work settled — a pass or a
/// failure of the WORK; `None` for any other row, for the verifier's own
/// fault, and for a verifier that settled nothing.
fn settled_by(record: &RouteOutcomeRecord) -> Option<Receipt> {
    if record.signal.as_deref() != Some(VERDICT_SIGNAL)
        || record.decision_kind() != DecisionKind::Verify
        || record.verdict_subject_kind() != VerdictSubject::Work
    {
        return None;
    }
    match record.status.as_str() {
        runtime::OUTCOME_COMPLETED => Some(Receipt::Passed),
        runtime::OUTCOME_FAILED => Some(Receipt::Failed),
        _ => None,
    }
}

/// The `signal` word a verdict row carries, as the attribution recorders
/// spell it (`workflow_tools::engine::attribution`).
const VERDICT_SIGNAL: &str = "verdict";

/// The route-outcome ledger's rows, grouped by the attempt they are about —
/// read once, for every label a reading holds to the record ([`binding`]).
pub(crate) struct OnRecord<'r> {
    rows_of: std::collections::HashMap<&'r str, Vec<&'r RouteOutcomeRecord>>,
}

impl<'r> OnRecord<'r> {
    #[must_use]
    pub(crate) fn of(records: &'r [RouteOutcomeRecord]) -> Self {
        let mut rows_of: std::collections::HashMap<&str, Vec<&RouteOutcomeRecord>> = std::collections::HashMap::new();
        for record in records {
            if let Some(attempt) = record.run_id.as_deref() {
                rows_of.entry(attempt).or_default().push(record);
            }
        }
        Self { rows_of }
    }

    /// `attempt`'s receipt ([`receipt_among`]).
    #[must_use]
    pub(crate) fn receipt(&self, attempt: &str) -> Option<Receipted> {
        self.rows_of.get(attempt).and_then(|rows| receipt_among(rows))
    }

    /// Whether `attempt`'s own run row is still on record.
    fn ran(&self, attempt: &str) -> bool {
        self.rows_of.get(attempt).is_some_and(|rows| rows.iter().any(|record| is_run_row(record)))
    }

    /// Whether a verdict still on record about `attempt`'s work on `source`
    /// settled it other than `verified` says ([`settled_by`]) — whenever it
    /// was recorded: with the run gone, the record can no longer say which
    /// verdict on the source was the first, only that one of them disagrees.
    fn gainsays(&self, attempt: &str, source: &str, verified: Option<&str>) -> bool {
        self.rows_of.get(attempt).is_some_and(|rows| {
            rows.iter()
                .filter(|record| record.source.as_deref() == Some(source))
                .filter_map(|record| settled_by(record))
                .any(|receipt| Some(receipt.token()) != verified)
        })
    }
}

/// How a label row stands to the record now — the one reading every reader
/// of a label takes it through: the labeller, the standing, the samples and
/// the seat's own judgment (t-6263).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Binding {
    /// It names the source its receipt judged, and the record says the same;
    /// or the record no longer holds the attempt's run — the route-outcome
    /// ledger keeps a bucket's newest rows only — and the label keeps the
    /// evidence it was bound on ([`arm::VERIFIED_AT`]), which no verdict
    /// still on record gainsays.
    Bound,
    /// It names no source ([`arm::label_source`]): written before labels
    /// carried one. Not evaluable — kept as written, read as no label, and
    /// joined by a label on its source once a receipt for it is on record.
    Unsourced,
    /// It names a source, but the record no longer holds the attempt's run
    /// and the label keeps no evidence of the binding it was written on —
    /// written before labels kept it. A run gone from the record proves
    /// nothing: not evaluable, and read as no label.
    Unproven,
    /// The record says otherwise: it holds the attempt's run, which handed
    /// in another source, or whose first receipt on that source is not the
    /// one the label carries, or which has none; or, the run gone, it holds
    /// a verdict on the label's source that settled it otherwise.
    Contradicted,
}

/// How the label row `label` of `attempt` stands to `on_record` ([`Binding`]).
#[must_use]
pub(crate) fn binding(label: &Value, attempt: &str, on_record: &OnRecord<'_>) -> Binding {
    let Some(source) = arm::label_source(label) else {
        return Binding::Unsourced;
    };
    let verified = arm::VERIFIED.read(label).and_then(Value::as_str);
    if on_record.ran(attempt) {
        return match on_record.receipt(attempt) {
            Some(receipted) if receipted.source == source && verified == Some(receipted.receipt.token()) => {
                Binding::Bound
            }
            _ => Binding::Contradicted,
        };
    }
    if on_record.gainsays(attempt, source, verified) {
        Binding::Contradicted
    } else if arm::label_verdict_at(label).is_some() {
        Binding::Bound
    } else {
        Binding::Unproven
    }
}

/// `rows` less every label that does not stand ([`binding`]) or that a
/// strike took back ([`arm::strike_row`]) — the ledger as its readers read
/// it. Nothing is taken out of the file.
pub(crate) fn keep_bound_labels(rows: &mut Vec<Value>, on_record: &OnRecord<'_>) {
    let struck: std::collections::BTreeSet<String> =
        arm::struck_prints(rows).into_iter().map(str::to_string).collect();
    rows.retain(|row| {
        LABEL.read(row).and_then(Value::as_str).is_none_or(|attempt| {
            binding(row, attempt, on_record) == Binding::Bound
                && (struck.is_empty() || !struck.contains(&arm::label_print(row)))
        })
    });
}

/// The strikes `rows` owe: one for every label the record contradicts now
/// ([`Binding::Contradicted`]) that no strike has taken back yet — so a
/// record that later forgets the rows that contradicted it gives the label
/// nothing back. Pure; the caller appends them.
fn strikes_due(rows: &[Value], on_record: &OnRecord<'_>, now_ms: i64) -> Vec<Value> {
    let mut struck: std::collections::BTreeSet<String> =
        arm::struck_prints(rows).into_iter().map(str::to_string).collect();
    rows.iter()
        .filter(|row| {
            LABEL
                .read(row)
                .and_then(Value::as_str)
                .is_some_and(|attempt| binding(row, attempt, on_record) == Binding::Contradicted)
        })
        .filter(|row| struck.insert(arm::label_print(row)))
        .filter_map(|row| arm::strike_row(row, now_ms))
        .collect()
}

/// The challenger's ledger at `ledger` as its readers read it
/// ([`keep_bound_labels`]) — every contradiction the reading finds written
/// down first ([`strikes_due`]): one `O_APPEND` line each, beside the label,
/// never over it; a strike two readers both wrote is one strike. A strike
/// is kept only once the ledger shows it back: until a reading finds it
/// there, it is held apart from the record whose retention would forget
/// what contradicted the label ([`UNSEEN_STRIKES`]), written again by every
/// reading, and taken back by every reading meanwhile — an append that
/// failed, or that answered `Ok` and wrote nothing, as a write the shadow
/// ledger declines does ([`append_shadow_row`]), never gives a contradicted
/// label back.
///
/// # Errors
/// The ledger could not be read ([`Reading::of`]): no rows, and nothing held
/// is settled on it — a ledger that could not be read is not one that
/// holds nothing.
fn bound_rows(ledger: &Path, on_record: &OnRecord<'_>, now_ms: i64) -> io::Result<Vec<Value>> {
    let reading = Reading::of(ledger)?;
    let due = strikes_due(&reading.rows, on_record, now_ms);
    let owed = unseen_strikes(ledger, &reading, due);
    let mut rows = reading.rows;
    for strike in owed {
        let _ = append_strike(ledger, &strike);
        rows.push(strike);
    }
    keep_bound_labels(&mut rows, on_record);
    Ok(rows)
}

/// One whole reading of the challenger's ledger, and where it stands in the
/// order of this process's readings ([`READINGS`]): begun at one tick and
/// ended at a later one. A reading begun after another ended holds every
/// row the other held but the ones the ledger's retention has cut since —
/// what lets a strike held on one reading be settled only on a reading that
/// is not older than it ([`unseen_strikes`]).
struct Reading {
    rows: Vec<Value>,
    begun: u64,
    ended: u64,
}

/// The ticks this process's readings of a challenger ledger begin and end
/// at ([`Reading`]) — one order for every reader, whichever thread.
static READINGS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Reading {
    /// Read `ledger` whole, between two ticks of [`READINGS`]; under a
    /// test, first whatever failure the test's thread queued
    /// (`tests::failed_reading`), and once read, wherever the test's thread
    /// asked to be held (`tests::reading_taken`).
    ///
    /// # Errors
    /// The ledger is there and could not be read to its end
    /// ([`try_read_shadow_rows`]).
    fn of(ledger: &Path) -> io::Result<Self> {
        let tick = || READINGS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let begun = tick();
        #[cfg(test)]
        if let Some(failed) = tests::failed_reading() {
            return Err(failed);
        }
        let rows = try_read_shadow_rows(ledger)?;
        #[cfg(test)]
        tests::reading_taken();
        Ok(Self { rows, begun, ended: tick() })
    }
}

/// A strike a reading owed and the ledger has not yet shown back
/// ([`UNSEEN_STRIKES`]).
struct UnseenStrike {
    strike: Value,
    /// The tick the newest reading that owed it ended at ([`Reading::ended`]).
    owed_at: u64,
}

/// Every strike a reading of this process owed a ledger that the ledger
/// has not yet shown back, by ledger, each under the print of the label it
/// takes back ([`bound_rows`]). In this process only: the record's
/// retention cannot reach it, and a strike that lands leaves it at the next
/// reading begun after it was owed.
static UNSEEN_STRIKES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, std::collections::BTreeMap<String, UnseenStrike>>>,
> = std::sync::OnceLock::new();

/// The strikes `ledger` still owes once `reading` — the ledger as it was
/// just read — and the strikes `due` on it are taken into account, in the
/// order the ledger holds the labels they take back: each held
/// ([`UNSEEN_STRIKES`]) until a reading shows it, or no longer holds the
/// label it takes back. Only a reading begun after the newest reading that
/// owed a strike ended may settle it: an older one — taken before the label
/// was appended, and brought to the book only after — has not seen the
/// label at all, and its missing it says nothing.
fn unseen_strikes(ledger: &Path, reading: &Reading, due: Vec<Value>) -> Vec<Value> {
    let shown = arm::struck_prints(&reading.rows);
    let labels: Vec<String> =
        reading.rows.iter().filter(|row| LABEL.read(row).is_some()).map(arm::label_print).collect();
    let mut book = UNSEEN_STRIKES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let unseen = book.entry(ledger.to_path_buf()).or_default();
    for strike in due {
        if let Some(print) = arm::STRUCK_PRINT.read(&strike).and_then(Value::as_str) {
            let held = unseen.entry(print.to_string()).or_insert(UnseenStrike {
                strike,
                owed_at: reading.ended,
            });
            held.owed_at = held.owed_at.max(reading.ended);
        }
    }
    let mut owing = std::collections::BTreeSet::new();
    let mut owed = Vec::new();
    for print in labels {
        if shown.contains(print.as_str()) || owing.contains(&print) {
            continue;
        }
        if let Some(held) = unseen.get(&print) {
            owed.push(held.strike.clone());
            owing.insert(print);
        }
    }
    // What the ledger shows, or no longer holds a label for, leaves the book
    // — on a reading no older than the strike.
    unseen.retain(|print, held| owing.contains(print) || held.owed_at > reading.begun);
    if unseen.is_empty() {
        book.remove(ledger);
    }
    owed
}

/// Append `strike` beside the label it takes back — the shadow ledger's one
/// append; under a test, first whatever refusal the test's thread queued
/// (`tests::refused_strike`).
fn append_strike(ledger: &Path, strike: &Value) -> io::Result<()> {
    #[cfg(test)]
    if let Some(refused) = tests::refused_strike() {
        return refused;
    }
    append_shadow_row(ledger, strike, SHADOW_LEDGER_MAX_BYTES)
}

/// One labelled comparison, for the sample it may become.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Labelled {
    pub attempt: String,
    pub role: String,
    pub incumbent: String,
    pub challenger: String,
    /// The challenger's word by the receipt ([`arm::Quality::won`]).
    pub won: bool,
    /// Whether the judge named what the receipt vindicated
    /// ([`arm::Quality::agreed`]); `None` where the receipt cannot say.
    pub agreed: Option<bool>,
}

/// The label rows due on `rows`: one per answered comparison whose attempt
/// has a receipt and no label that stands yet, carrying the source the
/// receipt judged and when its verdict was recorded ([`arm::Receipted`]).
/// Pure; the caller appends them, and hands in the rows as its readers read
/// them ([`keep_bound_labels`]).
#[must_use]
pub(crate) fn labels_due(
    rows: &[Value],
    receipt_of: impl Fn(&str) -> Option<Receipted>,
    now_ms: i64,
) -> Vec<Value> {
    let mut labelled: std::collections::BTreeSet<&str> = rows
        .iter()
        .filter(|row| arm::label_source(row).is_some())
        .filter_map(|row| LABEL.read(row).and_then(Value::as_str))
        .collect();
    let mut due = Vec::new();
    for row in rows {
        let Some(attempt) = ATTEMPT.read(row).and_then(Value::as_str) else {
            continue;
        };
        if OUTCOME.read(row).is_none() || labelled.contains(attempt) {
            continue;
        }
        let Some(preferred) = arm::PREFERRED
            .read(row)
            .and_then(Value::as_str)
            .and_then(Preferred::from_token)
        else {
            continue;
        };
        let Some(receipted) = receipt_of(attempt) else {
            continue;
        };
        labelled.insert(attempt);
        due.push(arm::label_row(attempt, &receipted, preferred, now_ms));
    }
    due
}

/// Every labelled comparison in `rows`: each label row read beside the
/// request row it names — the role and the two models from the request,
/// the challenger's word and the agreement from the label. A label naming
/// no request in the ledger, or no source ([`arm::label_source`]), says
/// nothing.
#[must_use]
pub(crate) fn labelled_in(rows: &[Value]) -> Vec<Labelled> {
    let requests: std::collections::BTreeMap<&str, &Value> = rows
        .iter()
        .filter(|row| OUTCOME.read(row).is_some())
        .filter_map(|row| ATTEMPT.read(row).and_then(Value::as_str).map(|attempt| (attempt, row)))
        .collect();
    rows.iter()
        .filter(|label| arm::label_source(label).is_some())
        .filter_map(|label| {
            let attempt = LABEL.read(label).and_then(Value::as_str)?;
            let request = requests.get(attempt)?;
            let text = |key: &zerocode_core::jev::summary::LedgerKey| {
                key.read(request).and_then(Value::as_str).unwrap_or_default().to_string()
            };
            Some(Labelled {
                attempt: attempt.to_string(),
                role: text(&ROLE),
                incumbent: text(&INCUMBENT_MODEL),
                challenger: text(&CHALLENGER_MODEL),
                won: arm::WON.read(label).and_then(Value::as_bool).unwrap_or(false),
                agreed: AGREED.read(label).and_then(Value::as_bool),
            })
        })
        .collect()
}

/// Whether the arm may move a role's model right now: the seat acts (a
/// person's `on`, or an `auto` its own ledger has raised to applying — the
/// standing, read back from the transitions, not this window's verdict) AND
/// the challenger's record for the role passes the incumbent's own learned
/// rate. Either alone moves nothing. The one line a sample is written on
/// ([`note_verdicts_in`]) and read on ([`admit_samples`]).
#[must_use]
pub(crate) fn may_move(
    mode: JevMode,
    seat_applies: bool,
    rows: &[Value],
    labelled: &Labelled,
    incumbent_rate: Option<f64>,
) -> bool {
    if !mode.applies_with(seat_applies) {
        return false;
    }
    let Some(rate) = incumbent_rate else {
        return false;
    };
    arm::standing(rows, &labelled.role, &labelled.challenger).passes(rate)
}

/// The mode word a seat acts under right now, and whether its `auto` stands
/// raised — read back from its ledger's transitions by the one reader every
/// seat's standing is read with ([`runtime::jev_seat_applies`]), and only for
/// an `auto`, the one word the standing decides. `None` for a seat that asks
/// nothing, or acts on nothing.
fn acting_now(cwd: &Path, mode: Option<JevMode>) -> Option<(JevMode, bool)> {
    let mode = mode.filter(|mode| mode.asks())?;
    let raised = mode.automatic() && runtime::jev_seat_applies(cwd, &CHALLENGER);
    mode.applies_with(raised).then_some((mode, raised))
}

/// The incumbent's own learned rate for the labelled comparison's role:
/// read from `records` as the learner weighs them — the arm's samples
/// unadmitted, so the bar is the incumbent's own runs, never the evidence it
/// is the bar for.
fn incumbent_rate(records: &[RouteOutcomeRecord], labelled: &Labelled, now_secs: u64) -> Option<f64> {
    let role = RouteRole::from_key(&labelled.role)?;
    runtime::learned_rate(records, now_secs, role, &labelled.incumbent, super::canonicalize_route_model_id)
}

/// The labelled comparisons of `rows` the seat stands behind now: [`may_move`]
/// asked once for each role, incumbent and challenger the labels name — a
/// reading of the whole ledger and the whole outcome log each, so once per
/// pair and not once per label.
fn stood_behind(
    acting: JevMode,
    raised: bool,
    rows: &[Value],
    records: &[RouteOutcomeRecord],
    now_secs: u64,
) -> Vec<Labelled> {
    let mut asked: std::collections::BTreeMap<(String, String, String), bool> = std::collections::BTreeMap::new();
    labelled_in(rows)
        .into_iter()
        .filter(|labelled| {
            let pair = (labelled.role.clone(), labelled.incumbent.clone(), labelled.challenger.clone());
            *asked.entry(pair).or_insert_with(|| {
                may_move(acting, raised, rows, labelled, incumbent_rate(records, labelled, now_secs))
            })
        })
        .collect()
}

/// The verified sample a labelled comparison becomes when the arm acts —
/// `None` for a label whose receipt and judge do not agree: only there does
/// the comparison's word about the challenger stand confirmed (a failed
/// incumbent the judge ranked below the challenger is a win; a passing one
/// the judge preferred is a loss). A judge the receipt contradicts, a
/// passing incumbent against a preferred challenger, and a judge who named
/// neither say nothing about the challenger's work, and teach the router
/// nothing. The sample carries the challenger's model and the role, under
/// the arm's own source and attempt key — so the learner counts it once, as
/// the challenger's, and never as the incumbent's.
#[must_use]
pub(crate) fn sample_of(labelled: &Labelled) -> Option<RouteOutcomeRecord> {
    (labelled.agreed == Some(true)).then(|| {
        RouteOutcomeRecord::new(
            "subagent",
            SAMPLE_TARGET,
            super::canonicalize_route_model_id(&labelled.challenger),
            if labelled.won {
                runtime::OUTCOME_COMPLETED
            } else {
                runtime::OUTCOME_FAILED
            },
        )
        .with_decision(DecisionKind::Model)
        .with_role(Some(labelled.role.clone()))
        .with_route_source(Some(CHALLENGER_ROUTE_SOURCE.to_string()))
        .with_signal(VERDICT_SIGNAL)
        .with_signal_weight(Some(1.0))
        .with_attempt_key(sample_attempt_key(&labelled.attempt))
    })
}

/// Bring the route-outcome ledger's samples up to what the seat stands
/// behind now: one sample for every labelled comparison whose receipt and
/// judge agree ([`sample_of`]), while the seat acts and the challenger's
/// standing passes ([`may_move`]) — the one already there never written
/// again, and the one a crash or a refused write left out written now. The
/// label is the durable fact; the sample follows it, and nothing is ever
/// taken back — a sample the seat no longer stands behind, or one another
/// label than the one that stands wrote, stays in the ledger and teaches
/// nothing ([`admit_samples`]). `rows` are the ledger as its readers read it
/// ([`keep_bound_labels`]). Answers how many were written.
fn feed_samples(
    cwd: &Path,
    mode: Option<JevMode>,
    rows: &[Value],
    records: &[RouteOutcomeRecord],
    now_secs: u64,
    feed: &dyn Fn(&RouteOutcomeRecord) -> io::Result<()>,
) -> usize {
    let Some((acting, raised)) = acting_now(cwd, mode) else {
        return 0;
    };
    let mut sampled: std::collections::BTreeSet<(String, String)> = records
        .iter()
        .filter(|record| record.is_seat_sample())
        .filter_map(|record| Some((record.run_id.clone()?, record.status.clone())))
        .collect();
    let mut written = 0;
    for sample in stood_behind(acting, raised, rows, records, now_secs).iter().filter_map(sample_of) {
        let Some(key) = sample.run_id.clone() else {
            continue;
        };
        let said = (key, sample.status.clone());
        if !sampled.contains(&said) && feed(&sample).is_ok() {
            sampled.insert(said);
            written += 1;
        }
    }
    written
}

/// Label every comparison of `cwd`'s whose receipt is in, and — where the
/// arm acts now — bring its samples up to date. Called where a verdict is
/// recorded and where a comparison's row is written, so a receipt that came
/// before the row and one that comes after both land. Answers how many
/// labels were written.
#[must_use]
pub fn note_challenger_verdicts(cwd: &Path) -> usize {
    let mode = jev_challenger_mode_from(&runtime::ConfigLoader::default_for(cwd));
    note_verdicts_in(cwd, mode, &|sample| runtime::record_route_outcome(cwd, sample))
}

/// [`note_challenger_verdicts`] with the mode word as it stands and the
/// sample writer handed in — the seam a test hands a refusing writer to.
/// Under the ledger's lock: a verdict landing while a comparison's row is
/// being written reads the rows once, one attempt is labelled once, and one
/// sample is written once however many callers race for it. A route-outcome
/// ledger that cannot be read is not an empty one, nor is a challenger
/// ledger that cannot be read ([`bound_rows`]): nothing is labelled, fed,
/// struck or judged until both can be read.
pub(crate) fn note_verdicts_in(
    cwd: &Path,
    mode: Option<JevMode>,
    feed: &dyn Fn(&RouteOutcomeRecord) -> io::Result<()>,
) -> usize {
    if !mode.is_some_and(JevMode::asks) {
        return 0;
    }
    let ledger = challenger_path(cwd);
    if !ledger.exists() {
        return 0;
    }
    let Ok(_lock) = runtime::SettingsFileLock::acquire(&ledger) else {
        return 0;
    };
    let Ok(records) = runtime::read_route_outcomes(cwd) else {
        return 0;
    };
    let on_record = OnRecord::of(&records);
    let now = Today::now();
    let Ok(mut rows) = bound_rows(&ledger, &on_record, unix_millis_i64(now.now_ms)) else {
        return 0;
    };
    let mut written = 0;
    for label in labels_due(&rows, |attempt| on_record.receipt(attempt), unix_millis_i64(now.now_ms)) {
        if append_shadow_row(&ledger, &label, SHADOW_LEDGER_MAX_BYTES).is_ok() {
            written += 1;
            rows.push(label);
        }
    }
    let _fed = feed_samples(cwd, mode, &rows, &records, now.now_ms / 1_000, feed);
    let _ = judge_seat_rows(&CHALLENGER, &ledger, &rows, unix_millis_i64(now.now_ms));
    written
}

/* ---- what the learners read ------------------------------------------------ */

/// Mark which of the arm's samples in `records` the router may learn from
/// right now ([`RouteOutcomeRecord::admitted`]) — the reading every
/// learner's records pass through (t-6263; the coordinator's m-7953): a
/// sample counts while its seat acts NOW and its challenger's standing for
/// the role passes the incumbent's own learned rate — [`may_move`], the same
/// line it was written on — and only as the sample its label, standing to
/// the record now ([`binding`]), makes: a label that names no source, that
/// proves no binding, or that the record contradicts — now, or before it
/// forgot the rows that said so ([`bound_rows`]) — admits nothing. Switched
/// off or fallen, none is admitted, and the router learns as if the arm had
/// never written; raised again, the same rows count under their own decay.
/// A ledger that cannot be read stands behind no sample. Nothing is removed
/// from the ledger. A record set with no sample in it asks nothing
/// more, not even the mode word (`mode_of`).
pub(crate) fn admit_samples(
    cwd: &Path,
    mode_of: impl FnOnce() -> Option<JevMode>,
    records: &mut [RouteOutcomeRecord],
    now_secs: u64,
) {
    if !records.iter().any(RouteOutcomeRecord::is_seat_sample) {
        return;
    }
    let Some((acting, raised)) = acting_now(cwd, mode_of()) else {
        return;
    };
    let now_ms = i64::try_from(now_secs.saturating_mul(1_000)).unwrap_or(i64::MAX);
    let Ok(rows) = bound_rows(&challenger_path(cwd), &OnRecord::of(records), now_ms) else {
        return;
    };
    let behind: std::collections::BTreeMap<String, String> = stood_behind(acting, raised, &rows, records, now_secs)
        .iter()
        .filter_map(sample_of)
        .filter_map(|sample| Some((sample.run_id?, sample.status)))
        .collect();
    for record in records.iter_mut().filter(|record| record.is_seat_sample()) {
        record.admitted = record.run_id.as_deref().and_then(|key| behind.get(key)) == Some(&record.status);
    }
}

/// The route-outcome ledger of `cwd` as every learner reads it: every row,
/// the arm's samples admitted only as the seat stands now
/// ([`admit_samples`]).
fn learning_records_in(
    cwd: &Path,
    mode_of: impl FnOnce() -> Option<JevMode>,
    now_secs: u64,
) -> Vec<RouteOutcomeRecord> {
    let mut records = runtime::read_route_outcomes(cwd).unwrap_or_default();
    admit_samples(cwd, mode_of, &mut records, now_secs);
    records
}

/// `learning_records_in` with the person's own mode word, read only when
/// the ledger holds a sample, and this machine's clock — what the router's
/// outcome learning reads (`apply::SmartRouteContext`), and the plan
/// scorer's shadow.
///
/// # Errors
/// The ledger could not be read.
pub fn read_learning_outcomes(cwd: &Path) -> io::Result<Vec<RouteOutcomeRecord>> {
    let mut records = runtime::read_route_outcomes(cwd)?;
    admit_samples(
        cwd,
        || jev_challenger_mode_from(&runtime::ConfigLoader::default_for(cwd)),
        &mut records,
        Today::now().now_ms / 1_000,
    );
    Ok(records)
}

#[cfg(test)]
pub(crate) mod tests;
