//! A live reflex run as the window holds it (realtime v1 §6, t-9205): the door
//! a start passes, the start that hands the helper the plan with the window's
//! tables, and — for every run it started — one watch that keeps the run's
//! receipts on disk and asks the reflex decision of what it reads.
//!
//! The helper holds the hand for the run and answers the start at once; every
//! later request names its run (`run`) and the helper compares it under one
//! lock, so a late stop or status for one run never reaches another. The
//! receipts leave the helper only when the window acknowledges what it wrote:
//! one collector per run reads after the last number it has on disk, writes,
//! syncs, and only then acknowledges. On a run a person's plan started the
//! reflex decision (`zerocode_core::jev::REFLEX_DECIDE`) only records; on the
//! runs of an autopilot — a model's plan from a goal, `reflex-auto` — its
//! answer is carried out once the seat applies ([`autopilot`], t-10223).
//!
//! Whether a run could start here and whether the person lets it are two
//! words, never one: `capabilities` and `reflex-status` answer both
//! (`liveReflex {supported, enabled}`, [`standing`]).

use std::collections::BTreeMap;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::computer_flow::{Policy, parse_flow_with_reflex};
use zerocode_core::computer_use::{ComputerCommand, ComputerMethod, REFLEX_COLLECT_MS, eye_table};
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::computer_use_protocol::game_state;
use zerocode_core::computer_use_protocol::reflex::{
    self, RUN_POLICY_VERSION, ReflexCapability, RunPolicy, Surface, VERSION, ValidatedPlan,
};
use zerocode_core::jev::reflex_decide::{self, Decider, Offer, Pending, Stamp, Wired};
use zerocode_core::jev::{JevMode, REFLEX_DECIDE, REFLEX_DECIDE_DEADLINE_MS};

use super::ComputerUseError;
use crate::settings_runtime::setting_key::COMPUTER_LIVE_REFLEX;
use crate::systemone::{self, Spent, Wire};

/// What the door knows when a start comes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoorFacts {
    /// This build runs a macOS desktop and the capability table claims live
    /// reflex on it (`reflex::capability`).
    pub supported: bool,
    /// The person turned `computer_live_reflex` on in the settings.
    pub enabled: bool,
    /// Why the operator is stopped, when it is.
    pub stopped: Option<String>,
}

impl DoorFacts {
    /// This window, now: the table this build carries, the platform it runs
    /// on, the person's setting and the operator's stop.
    pub(crate) fn now(enabled: bool) -> Self {
        Self {
            supported: platform_runs_reflex(
                cfg!(target_os = "macos"),
                reflex::capability(Surface::MacosDesktop),
            ),
            enabled,
            stopped: super::guard::stopped_reason(),
        }
    }
}

/// Whether a platform runs live reflex at all: a macOS desktop whose row in
/// the capability table claims it. No other platform does, whatever a row
/// says — none has the live frames a run reads.
pub(crate) const fn platform_runs_reflex(macos: bool, desktop: ReflexCapability) -> bool {
    macos && desktop.live_reflex
}

/// The key `capabilities` and `reflex-status` carry [`standing`] under.
pub(crate) const LIVE_REFLEX: &str = "liveReflex";

/// The key the settings page's answer carries the last [`check`] under,
/// beside [`standing`] ([`settings_answer`]).
pub(crate) const CHECK: &str = "check";

/// Where the last [`check`] is kept, under the window's local data root —
/// beside the person's settings, never in them: the switch is the only road
/// into those.
pub(crate) const CHECK_FILE: &str = "computer-use/live-reflex-check.json";

/// What the door says to a platform whose table does not claim live reflex.
const NOT_HERE: &str = "live reflex runs only on a macOS desktop whose capability table claims it";

/// What a start says to a helper whose handshake does not read what it carries.
const HELPER_DOES_NOT_READ: &str =
    "this helper does not read run policy 1 with its kernel installed; restart Computer Use";

/// Live reflex as `capabilities` and `reflex-status` answer it, in two words
/// never folded into one: `supported` — this platform's row in the table,
/// the helper's kernel, and the plan contract and run policy it reads
/// ([`helper_reads_it`]) — and `enabled`, the person's setting.
pub(crate) fn standing(facts: &DoorFacts, handshake: &Value) -> Value {
    json!({
        "supported": facts.supported && helper_reads_it(handshake),
        "enabled": facts.enabled,
    })
}

/// A start the door let through: the validated plan, the run's policy, the
/// display it reads and the folder its Flow document sits in — where the
/// words the reflex decision sends come from, which is what the Jev door's
/// consent is read against.
#[derive(Debug, Clone)]
pub(crate) struct Admitted {
    pub plan: ValidatedPlan,
    pub policy: RunPolicy,
    pub display: u64,
    pub workspace: Option<PathBuf>,
}

fn refused(code: &str, message: impl Into<String>) -> ComputerUseError {
    ComputerUseError::new(code, message)
}

/// The door's own questions, before any plan is read: the operator not
/// stopped, a platform whose table claims live reflex, the person's setting
/// on.
pub(crate) fn door_opens(facts: &DoorFacts) -> Result<(), ComputerUseError> {
    if let Some(reason) = &facts.stopped {
        return Err(super::guard::refusal(reason));
    }
    if !facts.supported {
        return Err(refused(error_code::UNSUPPORTED_CAPABILITY, NOT_HERE));
    }
    if !facts.enabled {
        return Err(refused(
            error_code::UNSUPPORTED_CAPABILITY,
            format!("live reflex is off: `{COMPUTER_LIVE_REFLEX}` in the settings turns it on"),
        ));
    }
    Ok(())
}

/// The door every start passes before anything reaches a helper: the
/// door's own questions ([`door_opens`]), a Flow document that reads and
/// carries its reflex sections, no money line and no `guarded` policy, and
/// then what every plan passes ([`admit_plan`]). A target the helper cannot
/// resolve, or ZeroCode itself, is the helper's to refuse before its eye or
/// its hand — the window keeps no copy of that check.
pub(crate) fn admit(
    params: &Value,
    read: impl FnOnce(&Path) -> std::io::Result<String>,
    facts: &DoorFacts,
) -> Result<Admitted, ComputerUseError> {
    door_opens(facts)?;
    let path = Path::new(
        params
            .get("flow")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    let text = read(path).map_err(|error| {
        ComputerUseError::invalid_argument(format!("the Flow document does not read: {error}"))
    })?;
    let parsed = parse_flow_with_reflex(&text)
        .map_err(ComputerUseError::invalid_argument)?
        .ok_or_else(|| ComputerUseError::invalid_argument("the document is not a Flow"))?;
    let Some(plan) = parsed.reflex else {
        return Err(ComputerUseError::invalid_argument(
            "the Flow carries no reflex sections to run",
        ));
    };
    if parsed.flow.money.is_some() || parsed.flow.policy == Policy::Guarded {
        return Err(ComputerUseError::invalid_argument(
            "a Flow that moves money, or a guarded one, never runs as a reflex",
        ));
    }
    admitted(plan, params, path.parent().map(Path::to_path_buf))
}

/// A plan the window wrote rather than read from a person's Flow — the
/// autopilot's (§2.3) — through the same door: its own questions asked now,
/// and what every plan passes. Such a plan has no Flow document and so no
/// money line or policy to weigh; the contract's reader has read its
/// sections already.
pub(crate) fn admit_plan(
    plan: ValidatedPlan,
    params: &Value,
    workspace: Option<PathBuf>,
    facts: &DoorFacts,
) -> Result<Admitted, ComputerUseError> {
    door_opens(facts)?;
    admitted(plan, params, workspace)
}

/// What every plan passes, whoever wrote it: a plan for the macOS desktop,
/// and the run's policy.
fn admitted(
    plan: ValidatedPlan,
    params: &Value,
    workspace: Option<PathBuf>,
) -> Result<Admitted, ComputerUseError> {
    if plan.plan().scope.surface != Surface::MacosDesktop {
        return Err(refused(
            error_code::UNSUPPORTED_CAPABILITY,
            "this window runs reflex plans on the macOS desktop only",
        ));
    }
    let seconds = params.get("seconds").and_then(Value::as_u64).unwrap_or(0);
    let renew = params.get("renew").and_then(Value::as_bool) == Some(true);
    let policy = RunPolicy::for_seconds(seconds, renew)
        .map_err(|_| ComputerUseError::invalid_argument("--seconds is outside the table"))?;
    Ok(Admitted {
        plan,
        policy,
        display: params.get("display").and_then(Value::as_u64).unwrap_or(0),
        workspace,
    })
}

/// Whether a helper's handshake says it reads what a start carries: its
/// kernel installed, this plan contract and run policy 1. A helper that does
/// not say so is never sent a start.
pub(crate) fn helper_reads_it(handshake: &Value) -> bool {
    let reflex = &handshake["supports"]["desktop"]["reflex"];
    reflex["kernel"] == json!(true)
        && reflex["planVersion"].as_u64() == Some(VERSION.into())
        && reflex["runPolicy"].as_u64() == Some(RUN_POLICY_VERSION.into())
}

/// A run's id: unique in this window, in the plan's identifier alphabet.
fn new_run_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "rx-{}-{}",
        crate::project_runtime::now_epoch_ms(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// The start the helper is sent: the plan's canonical wire, the window's
/// reflex, perception and eye tables, the run's policy and the capability
/// table — every number the run keeps is the window's.
fn start_params(admitted: &Admitted, run_id: &str) -> Value {
    let text = |bytes: Vec<u8>| String::from_utf8(bytes).unwrap_or_default();
    json!({
        "runId": run_id,
        "plan": text(reflex::wire_bytes(admitted.plan.plan())),
        "limits": text(reflex::limits_wire()),
        "perception": text(game_state::limits_wire(&game_state::LIMITS)),
        "eye": Value::Object(eye_table()),
        "runPolicy": text(admitted.policy.wire()),
        "capability": text(reflex::capability_wire().to_vec()),
        "display": admitted.display,
    })
}

/// Where a run's receipts are kept in an evidence folder.
pub(crate) fn receipts_file(dir: &Path, run: &str) -> PathBuf {
    dir.join(format!("reflex-{run}.jsonl"))
}

/// The helper's road, as a start, a status, a stop and a run's watch call it.
pub(crate) type Call<'a> = &'a mut dyn FnMut(&str, Value) -> Result<Value, ComputerUseError>;

/// A start: the door, the helper's handshake, then the helper's own start —
/// answered at once with the run's id while the helper's hand runs the plan.
pub(crate) fn start(
    params: &Value,
    read: impl FnOnce(&Path) -> std::io::Result<String>,
    facts: &DoorFacts,
    call: Call<'_>,
) -> Result<(Value, Admitted), ComputerUseError> {
    let admitted = admit(params, read, facts)?;
    let answer = launch(&admitted, call)?;
    Ok((answer, admitted))
}

/// An admitted plan to the helper: its handshake, then its start under a
/// new run's id — every start's second half, whoever's plan it runs.
pub(crate) fn launch(admitted: &Admitted, call: Call<'_>) -> Result<Value, ComputerUseError> {
    let handshake = call("handshake", json!({}))?;
    if !helper_reads_it(&handshake) {
        return Err(refused(
            error_code::UNSUPPORTED_CAPABILITY,
            HELPER_DOES_NOT_READ,
        ));
    }
    let run_id = new_run_id();
    call("reflexStart", start_params(admitted, &run_id))
}

/// Where one run stands, from the helper — read, never taking a receipt —
/// beside what its watch kept on disk.
pub(crate) fn status(
    params: &Value,
    facts: &DoorFacts,
    call: Call<'_>,
) -> Result<Value, ComputerUseError> {
    let run = params
        .get("run")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let handshake = call("handshake", json!({}))?;
    let mut answer = call("reflexStatus", json!({ "run": run }))?;
    if let Some(fields) = answer.as_object_mut() {
        if let Some(report) = watch_report(run) {
            fields.insert("collector".into(), report);
        }
        // A run of an autopilot's says the whole autopilot: its plans, its
        // decisions and why it ended, whichever of its runs is named.
        if let Some(report) = autopilot::report(run) {
            fields.insert(autopilot::AUTOPILOT.into(), report);
        }
        fields.insert(LIVE_REFLEX.into(), standing(facts, &handshake));
    }
    Ok(answer)
}

/// The helper's handshake as `capabilities` answers it, with [`standing`]
/// beside what the helper says it reads.
pub(crate) fn capabilities(
    params: &Value,
    facts: &DoorFacts,
    call: Call<'_>,
) -> Result<Value, ComputerUseError> {
    let mut handshake = call("handshake", params.clone())?;
    let standing = standing(facts, &handshake);
    if let Some(fields) = handshake.as_object_mut() {
        fields.insert(LIVE_REFLEX.into(), standing);
    }
    Ok(handshake)
}

/// Which helper a handshake came from: its version and the plan contract it
/// reads — a [`check`] vouches for this helper and no other.
pub(crate) fn helper_identity(handshake: &Value) -> Value {
    json!({
        "version": handshake["providerVersion"],
        "planVersion": handshake["supports"]["desktop"]["reflex"]["planVersion"],
    })
}

/// The settings page's check before the switch may turn on (t-10221): what
/// a start's door and handshake read — nobody stopped, in the window or in
/// the helper; this platform's row; a helper whose kernel reads this plan
/// contract and run policy — and nothing a run does: no plan, no frame, no
/// input. A failing road says the sentence the door says for it. Whether the
/// kernel sees and hits is the bench's to measure, not this check's.
pub(crate) fn check(
    facts: &DoorFacts,
    handshake: &Value,
    call: Call<'_>,
    at_ms: i64,
) -> Result<Value, ComputerUseError> {
    let reason = if let Some(reason) = &facts.stopped {
        Some(super::guard::refusal(reason).message)
    } else if !facts.supported {
        Some(NOT_HERE.to_string())
    } else if !helper_reads_it(handshake) {
        Some(HELPER_DOES_NOT_READ.to_string())
    } else {
        // The helper's own stop — the person's chord heard on the desktop.
        let helper = call("status", json!({}))?;
        (helper["stopped"] == json!(true))
            .then(|| super::guard::refusal(helper["reason"].as_str().unwrap_or_default()).message)
    };
    Ok(json!({
        "ok": reason.is_none(),
        "at_ms": at_ms,
        "helper": helper_identity(handshake),
        "reason": reason,
    }))
}

/// `capabilities` as the settings page reads it: the helper's handshake with
/// [`standing`], and under [`CHECK`] the check made now (`now`) and kept at
/// `kept`, or — with no `now` — the one kept there, only while it names the
/// helper that answered: an old check never vouches for another helper.
pub(crate) fn settings_answer(
    facts: &DoorFacts,
    call: Call<'_>,
    kept: &Path,
    now: Option<i64>,
) -> Result<Value, String> {
    let mut answer = capabilities(&json!({}), facts, call).map_err(|error| error.to_string())?;
    let checked = match now {
        Some(at_ms) => {
            let made = check(facts, &answer, call, at_ms).map_err(|error| error.to_string())?;
            keep_check(kept, &made).map_err(|error| error.to_string())?;
            made
        }
        None => std::fs::read(kept)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(|made| made["helper"] == helper_identity(&answer))
            .unwrap_or(Value::Null),
    };
    if let Some(fields) = answer.as_object_mut() {
        fields.insert(CHECK.into(), checked);
    }
    Ok(answer)
}

/// Keep a check whole or not at all: written beside, then renamed over.
fn keep_check(kept: &Path, made: &Value) -> std::io::Result<()> {
    if let Some(folder) = kept.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let beside = kept.with_extension("json.tmp");
    std::fs::write(&beside, made.to_string())?;
    std::fs::rename(&beside, kept)
}

/// End one run, and no other: the helper compares the run it names. A run
/// of an autopilot's ends the autopilot too — it writes no plan after a
/// person's stop — and the run it moved on to, if it did.
pub(crate) fn stop(params: &Value, call: Call<'_>) -> Result<Value, ComputerUseError> {
    let run = params
        .get("run")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if let Some(Some(current)) = autopilot::stop_owner(run)
        && current != run
    {
        let _ = call("reflexStop", json!({ "run": current }));
    }
    call("reflexStop", json!({ "run": run }))
}

/// The four verbs and `capabilities` as the window answers them: `enabled`
/// reads the person's setting, asked by a start, an autopilot, a status and
/// the capabilities, and by nothing else; `generator` reads the road the
/// person chose for the autopilot's plans, asked by an autopilot alone.
pub(crate) fn answer(
    command: &ComputerCommand,
    enabled: impl FnOnce() -> bool,
    generator: impl FnOnce() -> super::errand::value::Setup,
) -> Result<Value, ComputerUseError> {
    let mut call = |method: &str, params: Value| super::call(method, params);
    match command.method {
        ComputerMethod::ReflexStart => {
            let facts = DoorFacts::now(enabled());
            let (answer, admitted) = start(
                &command.params,
                |path| std::fs::read_to_string(path),
                &facts,
                &mut call,
            )?;
            let run = answer
                .get("runId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let evidence = super::evidence::session_dir(crate::project_runtime::now_epoch_ms())
                .map(|dir| receipts_file(&dir, &run));
            watch_in_the_background(&run, evidence.clone(), admitted.workspace);
            Ok(json!({
                "runId": run,
                "state": answer.get("state").cloned().unwrap_or(Value::Null),
                "renew": admitted.policy.renew,
                "runNs": admitted.policy.run_ns,
                "evidence": evidence,
            }))
        }
        ComputerMethod::ReflexAuto => autopilot::begin(
            &command.params,
            DoorFacts::now(enabled()),
            generator(),
            &mut call,
        ),
        ComputerMethod::ReflexStatus => {
            status(&command.params, &DoorFacts::now(enabled()), &mut call)
        }
        ComputerMethod::ReflexStop => stop(&command.params, &mut call),
        ComputerMethod::Capabilities => {
            capabilities(&command.params, &DoorFacts::now(enabled()), &mut call)
        }
        _ => Err(ComputerUseError::invalid_argument("not a reflex verb")),
    }
}

// ---- a run's watch: its receipts on disk, and the reflex decision --------------

/// Where a run's receipts are kept: truncate to the length already durable
/// (so a write that failed halfway leaves nothing twice), append, sync, and
/// answer the new durable length.
pub(crate) trait ReceiptSink {
    fn keep(&mut self, durable: u64, lines: &[u8]) -> std::io::Result<u64>;
}

/// The run's evidence file in the session folder — or none, when the window
/// has no evidence folder: then nothing is kept, nothing is acknowledged, and
/// the helper's queue ends the run before it acts with nowhere to write.
pub(crate) struct FileSink(pub Option<PathBuf>);

impl ReceiptSink for FileSink {
    fn keep(&mut self, durable: u64, lines: &[u8]) -> std::io::Result<u64> {
        let Some(path) = &self.0 else {
            return Err(std::io::Error::other(
                "no evidence folder to keep receipts in",
            ));
        };
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        file.set_len(durable)?;
        file.seek(SeekFrom::Start(durable))?;
        file.write_all(lines)?;
        file.sync_all()?;
        Ok(durable + lines.len() as u64)
    }
}

/// What a run's watch has done, for a status and for the evidence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Report {
    pub evidence: Option<PathBuf>,
    /// Receipts on disk, by the helper's last number among them.
    pub through: u64,
    /// Writes that failed (each retried from the last durable length).
    pub write_failures: u64,
    /// The run ended and every receipt it issued is on disk, and it did not
    /// end for want of room: nothing it did went unrecorded.
    pub verified: Option<bool>,
    pub ended: bool,
    /// Reflex decisions, by the road each took, and the requests they sent.
    pub roads: BTreeMap<String, u64>,
    pub attempts: u64,
}

impl Report {
    fn rendered(&self) -> Value {
        json!({
            "evidence": self.evidence,
            "through": self.through,
            "writeFailures": self.write_failures,
            "verified": self.verified,
            "ended": self.ended,
            "decisions": self.roads,
            "attempts": self.attempts,
        })
    }
}

/// One question on the wire as the watch hands it to a thread of its own: the
/// state goes, what the wire came to — and the door's and the version's
/// account of it — comes back.
pub(crate) type Asker = Arc<dyn Fn(Value) -> (Wired, Spent) + Send + Sync>;

/// Whoever carries a run's decisions out, as the run's watch meets them: the
/// mode each pass asks under, the steady clock a reading is stamped on, and
/// — for every question that settles — the row's word on what became of its
/// answer. A run nobody carries out (`reflex-start`, the bench) is only
/// recorded ([`Recording`]); an autopilot's runs are carried
/// ([`autopilot`]).
pub(crate) trait Carrier {
    /// The reflex decision's word this pass.
    fn mode(&mut self) -> JevMode;
    /// Milliseconds on the window's steady clock.
    fn now_ms(&self) -> u64;
    /// A question came back: its row, stamped with the run's provenance and
    /// the pass's mode, is this carrier's to judge before it is recorded.
    fn settled(&mut self, pending: &Pending, row: &mut Value);
}

/// A run nobody carries out: each pass asks under the seat's word, and every
/// row says so and that nothing was applied.
pub(crate) struct Recording<F: Fn() -> JevMode>(pub F);

impl<F: Fn() -> JevMode> Carrier for Recording<F> {
    fn mode(&mut self) -> JevMode {
        (self.0)()
    }

    fn now_ms(&self) -> u64 {
        steady_ms()
    }

    fn settled(&mut self, _pending: &Pending, _row: &mut Value) {}
}

/// Milliseconds since this window first asked: the steady clock a reading's
/// age is read on ([`reflex_decide::age_at`]).
pub(crate) fn steady_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    u64::try_from(EPOCH.get_or_init(Instant::now).elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// What one pass came to: whether the watch is done, and the run's status as
/// the pass read it — the receipts it kept among it — when the helper
/// answered.
pub(crate) struct Passed {
    pub done: bool,
    pub read: Option<Value>,
}

/// One run's watch. Each pass reads the run's receipts after the last number
/// on disk — the read carries the run's status — keeps them, and only then
/// acknowledges them; a batch read again is written once. The same status is
/// the reflex decision's reading: at most one question in flight, the newest
/// reading waiting behind it, every one it replaced recorded as merged.
pub(crate) struct Watch {
    run: String,
    durable: u64,
    report: Report,
    decider: Decider,
    in_flight: Option<(Pending, mpsc::Receiver<(Wired, Spent)>)>,
    /// What an autopilot remembers of the run beside every question it asks
    /// (t-10223 §2.2): none for a run a person's plan started.
    stamp: Option<Stamp>,
}

impl Watch {
    pub(crate) fn new(run: &str, evidence: Option<PathBuf>) -> Self {
        Self {
            run: run.to_string(),
            durable: 0,
            report: Report {
                evidence,
                ..Report::default()
            },
            decider: Decider::new(),
            in_flight: None,
            stamp: None,
        }
    }

    /// The watch of an autopilot's run, every row it leaves carrying `stamp`.
    pub(crate) fn stamped(run: &str, evidence: Option<PathBuf>, stamp: Stamp) -> Self {
        Self {
            stamp: Some(stamp),
            ..Self::new(run, evidence)
        }
    }

    pub(crate) fn report(&self) -> &Report {
        &self.report
    }

    /// One pass; true once the run has ended with every receipt it issued on
    /// disk and no question in flight. `mode` is the reflex decision's word
    /// now; `record` takes the decision rows to write. Nothing is carried
    /// out.
    pub(crate) fn pass(
        &mut self,
        call: Call<'_>,
        sink: &mut dyn ReceiptSink,
        mode: impl Fn() -> JevMode,
        ask: &Asker,
        record: &mut dyn FnMut(Vec<Value>),
    ) -> bool {
        self.pass_carried(call, sink, &mut Recording(mode), ask, record)
            .done
    }

    /// [`Self::pass`], with `carrier` judging every question that settles
    /// before its row is recorded, and the pass's reading handed back.
    pub(crate) fn pass_carried(
        &mut self,
        call: Call<'_>,
        sink: &mut dyn ReceiptSink,
        carrier: &mut dyn Carrier,
        ask: &Asker,
        record: &mut dyn FnMut(Vec<Value>),
    ) -> Passed {
        let Ok(read) = call(
            "reflexReceipts",
            json!({ "run": self.run, "after": self.report.through }),
        ) else {
            return Passed {
                done: false,
                read: None,
            };
        };
        let state = read
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mode = carrier.mode();
        if state == reflex_missing() {
            // The helper does not know the run: what it did cannot be read.
            self.report.ended = true;
            self.report.verified = Some(false);
            return Passed {
                done: self.finish(record, mode),
                read: Some(read),
            };
        }
        self.keep(&read, call, sink);
        let mut rows = Vec::new();
        let asks = mode.asks();
        let ended = state == "stopped";
        // A run that ended is asked about no more.
        if asks && !ended {
            let mut snapshot = reflex_decide::snapshot_of(&read);
            snapshot.read_ms = Some(carrier.now_ms());
            match self.decider.offer(snapshot) {
                Offer::Same => {}
                Offer::Ask(pending) => self.send(pending, ask),
                Offer::Waiting { coalesced } => {
                    rows.extend(coalesced.map(|merged| {
                        self.count(reflex_decide::ROAD_COALESCED, 0);
                        let mut row = reflex_decide::coalesced_row(&self.run, &merged);
                        self.stamp(&mut row, mode);
                        row
                    }));
                }
            }
        }
        rows.extend(self.collect(&read, asks, ended, ask, carrier, mode));
        if !rows.is_empty() {
            record(rows);
        }
        let issued = read
            .get("receiptsIssued")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if ended && self.report.through >= issued && self.in_flight.is_none() {
            self.report.ended = true;
            let overflowed = read.get("reason").and_then(Value::as_str) == Some("overflow");
            self.report.verified = Some(!overflowed);
            return Passed {
                done: self.finish(record, mode),
                read: Some(read),
            };
        }
        Passed {
            done: false,
            read: Some(read),
        }
    }

    /// Write what came after the last durable receipt, then acknowledge it: a
    /// write that failed acknowledges nothing, and the same receipts come back.
    fn keep(&mut self, read: &Value, call: Call<'_>, sink: &mut dyn ReceiptSink) {
        let fresh: Vec<&Value> = read
            .get("receipts")
            .and_then(Value::as_array)
            .map(|receipts| {
                receipts
                    .iter()
                    .filter(|receipt| {
                        receipt
                            .get("seq")
                            .and_then(Value::as_u64)
                            .is_some_and(|seq| seq > self.report.through)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let Some(last) = fresh.last().and_then(|receipt| receipt["seq"].as_u64()) else {
            return;
        };
        let mut lines = Vec::new();
        for receipt in &fresh {
            let mut line = (*receipt).clone();
            line["run"] = json!(self.run);
            lines.extend_from_slice(line.to_string().as_bytes());
            lines.push(b'\n');
        }
        match sink.keep(self.durable, &lines) {
            Ok(durable) => {
                self.durable = durable;
                self.report.through = last;
                // A lost acknowledgement only means the same receipts are read
                // again, and written once.
                let _ = call("reflexAck", json!({ "run": self.run, "through": last }));
            }
            Err(_) => self.report.write_failures += 1,
        }
    }

    fn send(&mut self, pending: Pending, ask: &Asker) {
        let (answered, answer) = mpsc::channel();
        let state = pending.snapshot.state.clone();
        let ask = Arc::clone(ask);
        std::thread::spawn(move || {
            let _ = answered.send(ask(state));
        });
        self.in_flight = Some((pending, answer));
    }

    /// The question in flight, if it came back: its row, judged by `carrier`,
    /// and the reading waiting behind it sent next.
    fn collect(
        &mut self,
        read: &Value,
        asks: bool,
        ended: bool,
        ask: &Asker,
        carrier: &mut dyn Carrier,
        mode: JevMode,
    ) -> Vec<Value> {
        let Some((pending, answer)) = self.in_flight.take() else {
            return Vec::new();
        };
        let Ok((wired, spent)) = answer.try_recv() else {
            self.in_flight = Some((pending, answer));
            return Vec::new();
        };
        let now = reflex_decide::snapshot_of(read).scene;
        let mut row =
            reflex_decide::asked_row(&self.run, &pending, &wired, now.as_ref(), asks, ended);
        spent.stamp(&mut row);
        self.stamp(&mut row, mode);
        carrier.settled(&pending, &mut row);
        let road = row["road"].as_str().unwrap_or_default().to_string();
        self.count(&road, u64::from(wired.attempts));
        if let Some(next) = self.decider.settled(pending.id) {
            self.send(next, ask);
        }
        vec![row]
    }

    /// The run's provenance and the pass's mode on a row it leaves.
    fn stamp(&self, row: &mut Value, mode: JevMode) {
        if let Some(stamp) = &self.stamp {
            stamp.stamp(row);
        }
        row["mode"] = json!(mode.key());
    }

    fn count(&mut self, road: &str, attempts: u64) {
        *self.report.roads.entry(road.to_string()).or_default() += 1;
        self.report.attempts += attempts;
    }

    /// The run ended: the reading still waiting is recorded as merged.
    fn finish(&mut self, record: &mut dyn FnMut(Vec<Value>), mode: JevMode) -> bool {
        if let Some(waiting) = self.decider.close() {
            self.count(reflex_decide::ROAD_COALESCED, 0);
            let mut row = reflex_decide::coalesced_row(&self.run, &waiting);
            self.stamp(&mut row, mode);
            record(vec![row]);
        }
        true
    }
}

/// The helper's word for a run it does not know (`ReflexRuntimeHost.missing`).
const fn reflex_missing() -> &'static str {
    "missing"
}

/// The one question, asked down `wire` for the words of `workspace`, bounded
/// by one lease: what the door let through, and what came back.
pub(crate) fn asker(wire: Wire, workspace: Option<PathBuf>) -> Asker {
    Arc::new(move |state: Value| {
        let body = systemone::request_body(&state, &reflex_decide::questions());
        let began = Instant::now();
        let asked = wire.ask(
            &REFLEX_DECIDE,
            workspace.as_deref(),
            body,
            Duration::from_millis(REFLEX_DECIDE_DEADLINE_MS),
        );
        let wired = Wired {
            answer: asked.answer,
            attempts: asked.spent.requests,
            request_bytes: asked.request_bytes,
            rtt_ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
        };
        (wired, asked.spent)
    })
}

fn watches() -> &'static Mutex<BTreeMap<String, Report>> {
    static WATCHES: OnceLock<Mutex<BTreeMap<String, Report>>> = OnceLock::new();
    WATCHES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn watch_report(run: &str) -> Option<Value> {
    watches()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(run)
        .map(Report::rendered)
}

/// A run's watch on a thread of its own, every `REFLEX_COLLECT_MS`, while
/// the window's helper session stands — never starting one.
fn watch_in_the_background(run: &str, evidence: Option<PathBuf>, workspace: Option<PathBuf>) {
    let run = run.to_string();
    std::thread::spawn(move || {
        let wire = Wire::of_this_machine();
        let ledger = systemone::ledger_of(&wire, &REFLEX_DECIDE);
        let ask = asker(wire, workspace);
        let mut sink = FileSink(evidence.clone());
        let mut watch = Watch::new(&run, evidence);
        let mut record = |rows: Vec<Value>| record_decisions(ledger.as_deref(), rows);
        loop {
            if !super::session_stands() {
                let mut report = watch.report().clone();
                report.ended = true;
                report.verified = Some(false);
                publish(&run, report);
                return;
            }
            let mut call = |method: &str, params: Value| super::call(method, params);
            let done = watch.pass(
                &mut call,
                &mut sink,
                || super::errand::mode_now(&REFLEX_DECIDE),
                &ask,
                &mut record,
            );
            publish(&run, watch.report().clone());
            if done {
                return;
            }
            std::thread::sleep(Duration::from_millis(REFLEX_COLLECT_MS));
        }
    });
}

/// The reflex decision's rows into its ledger, each at the moment it is
/// written unless its carrier named the moment already — a label names its
/// request by it — and judged as every window seat's rows are
/// ([`systemone::record_rows`]).
pub(crate) fn record_decisions(ledger: Option<&Path>, rows: Vec<Value>) {
    let Some(ledger) = ledger else {
        return;
    };
    let now = crate::project_runtime::now_epoch_ms();
    let rows: Vec<Value> = rows
        .into_iter()
        .map(|mut row| {
            if row.get("at").is_none() {
                row["at"] = json!(now);
            }
            row
        })
        .collect();
    systemone::record_rows(&REFLEX_DECIDE, ledger, &rows, now);
}

pub(crate) fn publish(run: &str, report: Report) {
    watches()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(run.to_string(), report);
}

pub(crate) mod autopilot;
#[cfg(test)]
mod bench;
pub(crate) mod plan;
#[cfg(test)]
mod tests;
