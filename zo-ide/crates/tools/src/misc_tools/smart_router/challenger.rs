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
//!    else runs on a thread of its own ([`Arm::draw`]): the door asked
//!    whether it WOULD admit the comparison (no key, no consent, no budget:
//!    nothing is spent on a design nobody will judge), the challenger chosen
//!    from the connected inventory ([`pick_challenger`]), priced from the one
//!    price table, the day's share read and reserved under one lock
//!    ([`Reservation`]), and the design requested.
//! 2. **After the attempt's first turn** ([`Drawn::designs_ready`]): the
//!    incumbent's design is its own first plan — the first text it wrote
//!    before its first tool call ([`first_design_text`]) — frozen once. The
//!    challenger's design is joined, both are put to the judge under no name
//!    in the blind's order, the reservation is settled at what the design
//!    and the comparison actually cost, and the row is written. Nothing the
//!    attempt does waits on any of it: the incumbent's design is what runs,
//!    every time.
//! 3. **When a verdict lands** ([`note_challenger_verdicts`]): the
//!    verification loop's receipt for the attempt — a `verdict` row in the
//!    route-outcome ledger about that attempt's work — writes the label. A
//!    finished attempt is not a receipt; a verifier that settled nothing
//!    labels nothing.
//! 4. **Acting** (`auto` once the seat's own evidence has raised it, or a
//!    person's `on`): a labelled comparison whose challenger's standing for
//!    the role passes the incumbent's own learned rate becomes one verified
//!    sample of the challenger in the route-outcome ledger, under a
//!    `challenger` source and an attempt key of its own — the learner's own
//!    admission line then moves the role's model, and stops moving it when
//!    the samples stop. A seat only recording writes no sample.
//!
//! What leaves the machine: the head of the task and one design, to the
//! challenger, on the person's own provider credentials — a model the
//! router could have routed the role to, from the same connected inventory
//! — and the task's head with two designs, to the door. The design request
//! is filed in the route-outcome ledger as tax ([`RouteTaxCall::Challenger`]),
//! which the learner already skips, so a paragraph never teaches the router.

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
    self as arm, Attempt, Comparison, Designs, Held, Preferred, Receipt, ATTEMPT, CHALLENGER_MODEL,
    COST_MICROS, INCUMBENT_MODEL, ROLE,
};
use zerocode_core::jev::door::Refused;
use zerocode_core::jev::promote;
use zerocode_core::jev::summary::{LABEL, OUTCOME};
use zerocode_core::jev::{
    count, digest_of, door, fingerprint_of, Cap, JevMode, CHALLENGER, CHALLENGER_APPLY_DEADLINE_MS,
    CHALLENGER_DESIGN_BYTE_CAP, CHALLENGER_DESIGN_CAP, CHALLENGER_DESIGN_MAX_TOKENS,
    CHALLENGER_DESIGN_WALL_MS, CHALLENGER_TASK_CHAR_CAP,
};

use super::jev_gate::{self, JevDoor};
use super::settings::jev_challenger_mode_from;
use super::shadow_ledger::{
    append_shadow_row, judge_seat_ledger, read_shadow_rows, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES,
};

/// The seat's ledger file — the Jev use table's name for it.
pub const CHALLENGER_FILE: &str = CHALLENGER.ledger;

/// The version of the comparison's question — the words the judge is asked
/// (`zerocode_core::jev::challenger::ask`) and the two designs' shape. A
/// row's request digest carries it, so a later reading of the question
/// starts a window of its own.
pub const CHALLENGER_RUBRIC_VERSION: u32 = 1;

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
    fn attempt(&self) -> Attempt<'_> {
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
    /// The head of the task ([`CHALLENGER_TASK_CHAR_CAP`] characters).
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

/// The head of a task as the challenger reads it and the judge is shown
/// it: the seat's own cap, cut as the door cuts.
#[must_use]
pub(crate) fn task_head(task: &str) -> String {
    door::cut(task, Cap::Chars(CHALLENGER_TASK_CHAR_CAP))
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

/// What the comparison cost at `rate`: the input the judge reported, or —
/// for a request that left and reported nothing — the bytes that left, a
/// byte a token; nothing for one the door never let out.
#[must_use]
fn settled_comparison_micros(rate: &SystemOneRate, wire: &Wire) -> u64 {
    if wire.requests == 0 {
        return 0;
    }
    let tokens = wire
        .input_tokens
        .unwrap_or_else(|| u64::try_from(wire.sent_bytes).unwrap_or(u64::MAX));
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
    path: PathBuf,
}

impl Reservation {
    /// The book for `day` under `config_home`.
    #[must_use]
    pub(crate) fn for_day(config_home: &Path, day: &str) -> Self {
        Self {
            path: spend::spend_path(config_home, day),
        }
    }

    /// The day's book as it stands.
    #[must_use]
    pub(crate) fn book(&self) -> Book {
        spend::fold(&std::fs::read_to_string(&self.path).unwrap_or_default())
    }

    /// Read the book, ask the share, and reserve `expected` for `attempt` if
    /// it fits — one critical section, under the cross-process lock the
    /// settings file already uses ([`runtime::SettingsFileLock`]), so the
    /// read and the write are one step. `other_micros` is the day's spend
    /// outside the arm, summed by the caller before the lock is taken. The
    /// first reservation of a day forgets the books of earlier days.
    ///
    /// # Errors
    /// [`Held::Retry`] for an attempt the book already names — a second
    /// opening of the same attempt is not a second draw; [`Held::DayBudget`]
    /// when the share has no room, or the lock or the file could not be
    /// taken — a budget that cannot be kept refuses.
    pub(crate) fn reserve(&self, attempt: &str, expected: u64, other_micros: u64) -> Result<Book, Held> {
        let _lock = runtime::SettingsFileLock::acquire(&self.path).map_err(|_| Held::DayBudget)?;
        let first_of_the_day = !self.path.exists();
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        if spend::names(&text, attempt) {
            return Err(Held::Retry);
        }
        let day = spend::day_spend(&spend::fold(&text), other_micros);
        if !arm::within_day_budget(&day, expected) {
            return Err(Held::DayBudget);
        }
        self.append(&spend::line(attempt, Op::Reserve, expected))
            .map_err(|_| Held::DayBudget)?;
        if first_of_the_day {
            spend::forget_other_days(&self.path);
        }
        Ok(self.book())
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

struct Inner {
    cwd: PathBuf,
    config_home: PathBuf,
    mode: Option<JevMode>,
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
        let mode = jev_challenger_mode_from(&runtime::ConfigLoader::default_for(cwd));
        let discovered = if mode.is_some_and(JevMode::asks) {
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
        let inventory = scene.inventory;
        Self {
            inner: Arc::new(Inner {
                prompt_cache_roots: vec![scene.config_home.join("cache").join("prompt-cache")],
                cwd: scene.cwd,
                config_home: scene.config_home,
                mode: scene.mode,
                designer: scene.designer,
                inventory: Box::new(move |_| inventory.clone()),
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

    /// The attempt's start, on the spawn's own thread: the mode word and the
    /// cheap holds, and — for an attempt that clears them — the draw handed
    /// to a thread of its own. `None` for an attempt the arm does nothing
    /// for. A held row is written only for an attempt the draw picked: the
    /// four in five that did not draw say nothing, and neither does a role
    /// the arm never touches unless its attempt drew — one row per drawn
    /// attempt, never one per spawn.
    pub(crate) fn open(&self, facts: AttemptFacts) -> Option<Drawn> {
        let mode = self.inner.mode.filter(|mode| mode.asks())?;
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
            mode,
            work,
        })
    }

    /// Everything after the cheap holds, off the spawn's thread: the door's
    /// word, the challenger, the price, the share, and the design itself.
    /// `None` for a draw the arm held — its word is already in the ledger.
    fn draw(&self, facts: &AttemptFacts) -> Option<Designed> {
        let attempt = facts.attempt();
        let at = unix_millis_i64(self.now_ms());
        let hold = |why: Held| {
            self.write(&arm::held_row(&attempt, why, at));
            None
        };
        let door = (self.inner.door)();
        let client = (self.inner.client)();
        if let Err(refused) = door.would_admit(&CHALLENGER, client.is_some()) {
            self.write(&refused_row(&attempt, &facts.incumbent_model, refused, self.now_ms()));
            return None;
        }
        let Some(judge_rate) = (self.inner.judge_rate_of)(door.model()) else {
            return hold(Held::Unpriced);
        };
        let role = RouteRole::from_key(attempt.role)?;
        let records = runtime::read_route_outcomes(&self.inner.cwd).unwrap_or_default();
        let inventory = (self.inner.inventory)(&facts.incumbent_model);
        let today = (self.inner.clock)();
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
        let task_head = task_head(&facts.task);
        let design_expected = expected_design_micros(&price, &task_head);
        let empty = arm::ask(&facts.key, &task_head, &Designs { incumbent: "", challenger: "" });
        let comparison_expected = expected_comparison_micros(&judge_rate, request_body(&empty).to_string().len());
        let expected = design_expected.saturating_add(comparison_expected);
        let other_micros = day_request_micros(&self.inner.prompt_cache_roots, today.start_ms, self.inner.price_of);
        let reservation = Reservation::for_day(&self.inner.config_home, &today.day);
        if let Err(why) = reservation.reserve(&facts.key, expected, other_micros) {
            return hold(why);
        }
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
            door,
            client,
            judge_rate,
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
    pub mode: Option<JevMode>,
    pub designer: Arc<dyn Designer>,
    pub inventory: ModelInventory,
    pub door: Box<dyn Fn() -> JevDoor + Send + Sync>,
    pub client: Box<dyn Fn() -> Option<SystemOneClient> + Send + Sync>,
    pub clock: Clock,
}

/// A draw that bought a design: what the comparison and the settlement need.
struct Designed {
    door: JevDoor,
    client: Option<SystemOneClient>,
    judge_rate: SystemOneRate,
    challenger: String,
    price: ModelPrice,
    design_expected: u64,
    expected: u64,
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
    mode: JevMode,
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

    /// Join the draw; put the two designs to the judge under no name; settle
    /// the share at what the design and the comparison cost; write the row;
    /// and label whatever receipts are already in. On the calling thread —
    /// the seam a test holds to await the row.
    pub(crate) fn finish(self, incumbent_design: Option<String>) {
        let Ok(Some(designed)) = self.work.join() else {
            return;
        };
        let arm = &self.arm;
        let attempt = self.facts.attempt();
        let reservation = Reservation::for_day(&arm.inner.config_home, &designed.day);
        let design_cost = settled_design_micros(&designed.price, &designed.reply, designed.design_expected);
        let now_ms = arm.now_ms();
        let (Some(incumbent), Some(challenger)) = (incumbent_design, designed.reply.text.as_deref()) else {
            reservation.settle(&self.facts.key, design_cost);
            let mut row = arm::held_row(&attempt, Held::NoDesign, unix_millis_i64(now_ms));
            row[INCUMBENT_MODEL.canonical] = Value::from(self.facts.incumbent_model.as_str());
            row[CHALLENGER_MODEL.canonical] = Value::from(designed.challenger.as_str());
            row[COST_MICROS.canonical] = Value::from(design_cost);
            arm.write(&row);
            return;
        };
        let designs = Designs {
            incumbent: &incumbent,
            challenger,
        };
        let asked = arm::ask(&self.facts.key, &designed.task_head, &designs);
        let wire = compare(&designed.door, designed.client.as_ref(), &asked);
        let cost = design_cost.saturating_add(settled_comparison_micros(&designed.judge_rate, &wire));
        reservation.settle(&self.facts.key, cost);
        let comparison = Comparison {
            attempt: &self.facts.key,
            role: attempt.role,
            incumbent_model: &self.facts.incumbent_model,
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
        let ledger = arm.ledger();
        let _ = judge_seat_ledger(&CHALLENGER, &ledger, unix_millis_i64(now_ms));
        let _ = note_verdicts_in(&arm.inner.cwd, &ledger, self.mode);
    }
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
        "state": asked.state,
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

/* ---- the receipt and the label ----------------------------------------- */

/// The verification loop's word on `attempt`'s work, read off the
/// route-outcome ledger: the first settled `verdict` about that attempt —
/// a verifier's pass or failure of the WORK, never the verifier's own fault
/// and never a verifier that settled nothing. Completion is not here: a
/// spawn's own `completed` row is not a verdict.
#[must_use]
pub(crate) fn receipt_for(records: &[RouteOutcomeRecord], attempt: &str) -> Option<Receipt> {
    let mut verdicts: Vec<&RouteOutcomeRecord> = records
        .iter()
        .filter(|record| record.run_id.as_deref() == Some(attempt))
        .filter(|record| record.signal.as_deref() == Some(VERDICT_SIGNAL))
        .filter(|record| record.decision_kind() == DecisionKind::Verify)
        .filter(|record| record.verdict_subject_kind() == VerdictSubject::Work)
        .collect();
    verdicts.sort_by_key(|record| record.recorded_at);
    verdicts.into_iter().find_map(|record| match record.status.as_str() {
        runtime::OUTCOME_COMPLETED => Some(Receipt::Passed),
        runtime::OUTCOME_FAILED => Some(Receipt::Failed),
        _ => None,
    })
}

/// The `signal` word a verdict row carries, as the attribution recorders
/// spell it (`workflow_tools::engine::attribution`).
const VERDICT_SIGNAL: &str = "verdict";

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
/// has a receipt and no label yet. Pure; the caller appends them.
#[must_use]
pub(crate) fn labels_due(
    rows: &[Value],
    receipt_of: impl Fn(&str) -> Option<Receipt>,
    now_ms: i64,
) -> Vec<(Value, Labelled)> {
    let mut labelled: std::collections::BTreeSet<&str> = rows
        .iter()
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
        let Some(receipt) = receipt_of(attempt) else {
            continue;
        };
        labelled.insert(attempt);
        let graded = arm::quality(Some(receipt), preferred);
        let text = |key: &zerocode_core::jev::summary::LedgerKey| {
            key.read(row).and_then(Value::as_str).unwrap_or_default().to_string()
        };
        due.push((
            arm::label_row(attempt, receipt, preferred, now_ms),
            Labelled {
                attempt: attempt.to_string(),
                role: text(&ROLE),
                incumbent: text(&INCUMBENT_MODEL),
                challenger: text(&CHALLENGER_MODEL),
                won: graded.won,
                agreed: graded.agreed,
            },
        ));
    }
    due
}

/// Whether the arm may move a role's model right now: the seat acts (a
/// person's `on`, or an `auto` its own ledger has raised to applying — the
/// standing, read back from the transitions, not this window's verdict) AND
/// the challenger's record for the role passes the incumbent's own learned
/// rate. Either alone moves nothing.
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

/// Whether `attempt`'s sample is already in the ledger.
fn already_fed(records: &[RouteOutcomeRecord], attempt: &str) -> bool {
    let key = sample_attempt_key(attempt);
    records.iter().any(|record| record.run_id.as_deref() == Some(key.as_str()))
}

/// Label every comparison of `cwd`'s whose receipt is in, and — where the
/// arm acts — write its sample. Called where a verdict is recorded and
/// where a comparison's row is written, so a receipt that came before the
/// row and one that comes after both land. Answers how many labels were
/// written.
#[must_use]
pub fn note_challenger_verdicts(cwd: &Path) -> usize {
    let Some(mode) = jev_challenger_mode_from(&runtime::ConfigLoader::default_for(cwd)) else {
        return 0;
    };
    if !mode.asks() {
        return 0;
    }
    note_verdicts_in(cwd, &challenger_path(cwd), mode)
}

/// [`note_challenger_verdicts`] on `ledger` — the seam a test hands a path
/// of its own. Under the ledger's lock: a verdict landing while a
/// comparison's row is being written reads the rows once, and one attempt
/// is labelled once and fed once however many callers race for it.
pub(crate) fn note_verdicts_in(cwd: &Path, ledger: &Path, mode: JevMode) -> usize {
    if !ledger.exists() {
        return 0;
    }
    let Ok(_lock) = runtime::SettingsFileLock::acquire(ledger) else {
        return 0;
    };
    let records = runtime::read_route_outcomes(cwd).unwrap_or_default();
    let mut rows: Vec<Value> = read_shadow_rows(ledger);
    let now = Today::now();
    let due = labels_due(&rows, |attempt| receipt_for(&records, attempt), unix_millis_i64(now.now_ms));
    if due.is_empty() {
        return 0;
    }
    let seat_applies = mode.automatic() && promote::stand_from(&rows) == promote::Stand::Applying;
    let mut written = 0;
    for (label, labelled) in due {
        if append_shadow_row(ledger, &label, SHADOW_LEDGER_MAX_BYTES).is_err() {
            continue;
        }
        written += 1;
        rows.push(label);
        let incumbent_rate = RouteRole::from_key(&labelled.role).and_then(|role| {
            runtime::learned_rate(
                &records,
                now.now_ms / 1_000,
                role,
                &labelled.incumbent,
                super::canonicalize_route_model_id,
            )
        });
        if !may_move(mode, seat_applies, &rows, &labelled, incumbent_rate)
            || already_fed(&records, &labelled.attempt)
        {
            continue;
        }
        if let Some(sample) = sample_of(&labelled) {
            let _ = runtime::record_route_outcome(cwd, &sample);
        }
    }
    let _ = judge_seat_ledger(&CHALLENGER, ledger, unix_millis_i64(now.now_ms));
    written
}

#[cfg(test)]
pub(crate) mod tests;
