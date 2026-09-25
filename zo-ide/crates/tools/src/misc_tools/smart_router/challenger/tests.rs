//! The arm, held to its contract on a fake wire and a scripted challenger:
//! off is today to the byte, every hold writes its word and spends nothing,
//! the words a design request carries are the door's and the door is asked
//! as it stands the moment they leave, the share is read and reserved as one
//! step and settled in the day it was charged — never in a day that has
//! ended, never from a book that could not be read, and never behind the
//! lock of a holder that died — the judge sees two designs under no name,
//! asked of the door and the person's word as they stand when the
//! comparison is made, and the answer comes back to the right side, a
//! missing plan is compared with nothing, a receipt labels only when its
//! verifier saw the source the attempt handed in and a completion never
//! does, a label's sample follows it once however a write failed, and the
//! router learns from a sample only while the seat stands behind it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use api::{ModelPrice, SystemOneClient, Usage};
use runtime::{
    ContentBlock, ConversationMessage, DecisionKind, LearnedSpecialtyHint, ModelCapability, ModelDescriptor,
    ModelInventory, ModelTier, RouteOutcomeRecord, RouteRole, VerdictSubject,
};
use serde_json::{json, Value};
use zerocode_core::jev::challenger::spend;
use zerocode_core::jev::challenger::{draws, Blind, Held, Preferred, Receipt, Receipted, Side};
use zerocode_core::jev::door::JevSettings;
use zerocode_core::jev::promote::{FELL, ROSE};
use zerocode_core::jev::summary::{AGREED, LABEL, OUTCOME, RUBRIC_VERSION, TRANSITION};
use zerocode_core::jev::{
    fingerprint_of, JevMode, A_WINDOW_OF_COMPARISONS, CHALLENGER, CHALLENGER_TASK_CHAR_CAP, MODEL_SETTING,
    SMART_SETTINGS_KEY,
};

use super::super::jev_gate::JevDoor;
use super::super::jev_mock::Mock;
use super::super::shadow_ledger::{read_shadow_rows, shadow_ledger_dir};
use super::*;

pub(crate) const INCUMBENT: &str = "claude-fable-5-1";
pub(crate) const NEWCOMER: &str = "claude-opus-5-2";
const RIVAL: &str = "gpt-5.6-sol";

/// The source the attempts of these tests hand in, and the one their
/// verifiers saw.
const HANDED_IN: &str = "tree-handed-in";

/// When the verdicts of the labels these tests lay down were recorded.
const VERDICT_AT: u64 = 3;

/// A receipt on the source handed in, its verdict recorded at
/// [`VERDICT_AT`] — what a label the labeller wrote keeps.
fn receipted(receipt: Receipt) -> Receipted {
    Receipted {
        receipt,
        source: HANDED_IN.to_string(),
        verdict_at: VERDICT_AT,
    }
}

/// The one price every test model lists at — and none for a model named
/// unpriced, which the table does not name.
pub(super) fn priced(id: &str) -> Option<ModelPrice> {
    (id != "unpriced-coder").then_some(ModelPrice {
        input: 5.0,
        cache_read: 0.5,
        cache_write: 6.25,
        output: 25.0,
    })
}

fn nothing_priced(_: &str) -> Option<ModelPrice> {
    None
}

/// What the scripted challenger reports it cost: 400 in, 120 out.
const DESIGN_BILL: u64 = 400 * 5 + 120 * 25;

/// A challenger that answers from a script, counts how often it was asked,
/// and — when the test holds it — keeps its answer on the wire until it is
/// let go.
pub(crate) struct Scripted {
    text: Option<String>,
    usage: Option<Usage>,
    left: bool,
    status: &'static str,
    asked: AtomicUsize,
    seen: Mutex<Vec<DesignRequest>>,
    gate: Mutex<Option<mpsc::Receiver<()>>>,
}

impl Scripted {
    fn new(text: Option<&str>, usage: Option<Usage>, left: bool, status: &'static str) -> Arc<Self> {
        Arc::new(Self {
            text: text.map(str::to_string),
            usage,
            left,
            status,
            asked: AtomicUsize::new(0),
            seen: Mutex::new(Vec::new()),
            gate: Mutex::new(None),
        })
    }

    fn billed() -> Usage {
        Usage {
            input_tokens: 400,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            output_tokens: 120,
            output_tokens_details: None,
        }
    }

    pub(crate) fn answering(text: &str) -> Arc<Self> {
        Self::new(Some(text), Some(Self::billed()), true, runtime::OUTCOME_COMPLETED)
    }

    /// Answering — once the returned sender lets the answer go. The request
    /// has left by then: what happens meanwhile happens to a design in
    /// flight.
    pub(crate) fn answering_when_let_go(text: &str) -> (Arc<Self>, mpsc::Sender<()>) {
        let (let_go, gate) = mpsc::channel();
        let scripted = Self::new(Some(text), Some(Self::billed()), true, runtime::OUTCOME_COMPLETED);
        *scripted.gate.lock().expect("gate") = Some(gate);
        (scripted, let_go)
    }

    /// Left, and no answer came back inside the wall.
    fn silent() -> Arc<Self> {
        Self::new(None, None, true, runtime::OUTCOME_STOPPED)
    }

    /// Never left: no client for the model, or its provider parked.
    fn unsent() -> Arc<Self> {
        Self::new(None, None, false, runtime::OUTCOME_FAILED)
    }

    pub(crate) fn calls(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }

    fn seen(&self) -> Vec<DesignRequest> {
        self.seen.lock().map(|seen| seen.clone()).unwrap_or_default()
    }

    /// Wait until a design request is on the wire.
    pub(crate) fn wait_until_asked(&self) {
        let started = Instant::now();
        while self.calls() == 0 && started.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(self.calls() > 0, "the design request never left");
    }
}

impl Designer for Scripted {
    fn design(&self, request: &DesignRequest) -> DesignReply {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(request.clone());
        }
        self.asked.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().ok().and_then(|mut gate| gate.take());
        if let Some(gate) = gate {
            let _ = gate.recv_timeout(Duration::from_secs(20));
        }
        DesignReply {
            text: self.text.clone(),
            usage: self.usage,
            left: self.left,
            status: self.status,
            elapsed_ms: 7,
        }
    }
}

fn coder(id: &str, provider: &str, family: &str, release: u32) -> ModelDescriptor {
    ModelDescriptor::new(id, provider, family)
        .capabilities([ModelCapability::Coding, ModelCapability::Default])
        .tiers([ModelTier::Strong])
        .release_rank(release)
}

/// The incumbent and one newcomer of its own provider, both below the top.
pub(crate) fn inventory() -> ModelInventory {
    ModelInventory::new(INCUMBENT, vec![coder(INCUMBENT, "anthropic", "claude", 50), coder(NEWCOMER, "anthropic", "claude", 60)])
}

pub(crate) fn jev_answer(chosen: &str) -> String {
    json!({
        "model": "jev-test",
        "answers": {
            "preferred": {
                "type": "choice",
                "choice": chosen,
                "probabilities": { "first": 0.7, "second": 0.2, "neither": 0.1 },
                "confidence": 0.7
            }
        },
        "usage": {"input_tokens": 300, "output_tokens": 4}
    })
    .to_string()
}

/* ---- the rig: a machine of the test's own ------------------------------ */

const SEOUL_MINUTES: i32 = 540;

/// What the rig's settings file tells the door: its switch, the workspace's
/// consent, and the judge a person pinned.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DoorWords {
    pub(crate) enabled: bool,
    pub(crate) consented: bool,
    pub(crate) pin: Option<&'static str>,
}

impl DoorWords {
    pub(crate) const OPEN: Self = Self {
        enabled: true,
        consented: true,
        pin: None,
    };
}

/// A machine of the test's own: a temp config home holding the person's
/// settings file — the seat's word, the door's switch and consent — which
/// the arm reads the way the product does, through the product's own
/// readers, every time it asks; the Jev wire on a loopback mock; a scripted
/// challenger; and a clock the test may hold still — the environment held
/// for the rig's life. Shared with the spawn path's own end-to-end test
/// (`agent_tools::spawn`).
pub(crate) struct Rig {
    pub(crate) cwd: PathBuf,
    pub(crate) home: PathBuf,
    mock: Mock,
    pub(crate) designer: Arc<Scripted>,
    clock: Arc<AtomicU64>,
    _work: Option<tempfile::TempDir>,
    _home: tempfile::TempDir,
    _env: crate::tests::EnvGuard,
}

impl Rig {
    /// A rig whose workspace is a temp folder of its own.
    pub(crate) fn new(mode: JevMode, jev_body: &str, designer: Arc<Scripted>) -> Self {
        let work = tempfile::tempdir().expect("a workspace");
        let cwd = std::fs::canonicalize(work.path()).expect("the workspace resolved");
        Self::at(cwd, Some(work), mode, jev_body, designer)
    }

    /// A rig whose workspace is `cwd` — the process's own folder, for a test
    /// that goes through a recorder that reads the process's cwd. No
    /// verification loop is started behind a finished spawn here: its
    /// verifier would be a real provider's call.
    pub(crate) fn at(
        cwd: PathBuf,
        work: Option<tempfile::TempDir>,
        mode: JevMode,
        jev_body: &str,
        designer: Arc<Scripted>,
    ) -> Self {
        let mock = Mock::serving(200, jev_body.to_string());
        let home_dir = tempfile::tempdir().expect("a config home");
        let home = std::fs::canonicalize(home_dir.path()).expect("the home resolved");
        let env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", &home.to_string_lossy())
            .set_also("ZO_HOME", &home)
            .set_also(core_types::paths::ZO_STATE_DIR_ENV, &home)
            .set_also("HOME", &home)
            .set_also(api::SYSTEMONE_API_KEY_ENV, "test-key")
            .set_also(api::SYSTEMONE_BASE_URL_ENV, &mock.base_url)
            .set_also("ZO_AUTO_VERIFY", "0");
        let rig = Self {
            cwd,
            home,
            mock,
            designer,
            clock: Arc::new(AtomicU64::new(0)),
            _work: work,
            _home: home_dir,
            _env: env,
        };
        rig.write_settings(Some(mode), DoorWords::OPEN);
        rig
    }

    /// Write the person's settings file: the seat's word (none: the switch
    /// decides), and what the door is told.
    pub(crate) fn write_settings(&self, mode: Option<JevMode>, door: DoorWords) {
        let workspaces: Vec<String> = if door.consented { vec![door::resolved_path(&self.cwd)] } else { Vec::new() };
        let mut smart = json!({ "jev": { "enabled": door.enabled, "workspaces": workspaces } });
        if let Some(mode) = mode {
            smart[CHALLENGER.setting] = json!(mode.key());
        }
        if let Some(pin) = door.pin {
            smart[MODEL_SETTING] = json!(pin);
        }
        std::fs::write(self.home.join("settings.json"), json!({ SMART_SETTINGS_KEY: smart }).to_string())
            .expect("a settings file");
    }

    /// A settings file nobody can read.
    fn write_unreadable_settings(&self) {
        std::fs::write(self.home.join("settings.json"), "{ not a settings file").expect("a settings file");
    }

    /// The arm this rig stands.
    pub(crate) fn arm(&self) -> Arm {
        self.arm_on(&self.mock.base_url, Box::new(|_| inventory()))
    }

    /// The arm, with `picking` run while it picks its challenger — after the
    /// task's words were first cleared, before the share is reserved and the
    /// design leaves: the moment a person takes a word back mid-draw.
    fn arm_picking(&self, picking: impl Fn() + Send + Sync + 'static) -> Arm {
        self.arm_on(
            &self.mock.base_url,
            Box::new(move |_| {
                picking();
                inventory()
            }),
        )
    }

    fn arm_on(&self, wire: &str, inventory: Box<dyn Fn(&str) -> ModelInventory + Send + Sync>) -> Arm {
        let (mode_cwd, door_cwd) = (self.cwd.clone(), self.cwd.clone());
        let url = wire.to_string();
        let clock = Arc::clone(&self.clock);
        Arm::with(Scene {
            cwd: self.cwd.clone(),
            config_home: self.home.clone(),
            mode: Box::new(move || jev_challenger_mode_from(&runtime::ConfigLoader::default_for(&mode_cwd))),
            designer: Arc::clone(&self.designer) as Arc<dyn Designer>,
            inventory,
            door: Box::new(move || JevDoor::open(&door_cwd)),
            client: Box::new(move || Some(SystemOneClient::new(&url, "test-key"))),
            clock: Box::new(move || match clock.load(Ordering::SeqCst) {
                0 => Today::now(),
                held => Today::at(held, SEOUL_MINUTES),
            }),
        })
    }

    /// Hold the arm's clock at `now_ms`, in Seoul's day.
    fn hold_clock(&self, now_ms: u64) {
        self.clock.store(now_ms, Ordering::SeqCst);
    }

    /// A day that has bought a share: one session's request ledger under the
    /// home's prompt cache, 1,000,000 input tokens of the incumbent at the
    /// test's $5/M, stamped `at_ms` — the day's other spend 5,000,000
    /// micro-dollars and the share half a million.
    pub(crate) fn spent_today(&self, at_ms: u64) {
        let dir = self.home.join("cache").join("prompt-cache").join("s-work");
        std::fs::create_dir_all(&dir).expect("a session dir");
        let row = json!({"seq": 1, "ts_unix_ms": at_ms, "model": INCUMBENT, "provider": "anthropic",
                         "cache_creation": 0, "cache_read": 0, "input_uncached": 1_000_000, "output": 0,
                         "message_count": 1, "broke": false});
        std::fs::write(dir.join("requests.jsonl"), format!("{row}\n")).expect("a request ledger");
    }

    /// A window of comparisons the challenger won for the coding role — each
    /// labelled, when `labelled`, by a failing receipt the judge agreed with
    /// — and the incumbent's own eight runs at one half: a standing that
    /// passes the incumbent's rate. Written straight to the ledgers, so no
    /// judgment of the seat is taken on the way.
    pub(crate) fn seed_a_standing_window(&self, labelled: bool) {
        let ledger = challenger_path(&self.cwd);
        for n in 0..A_WINDOW_OF_COMPARISONS {
            let attempt = format!("window-{n}#1");
            append_shadow_row(&ledger, &request_row(&attempt, Preferred::Challenger), SHADOW_LEDGER_MAX_BYTES)
                .expect("a comparison");
            if labelled {
                let label = arm::label_row(&attempt, &receipted(Receipt::Failed), Preferred::Challenger, 1);
                append_shadow_row(&ledger, &label, SHADOW_LEDGER_MAX_BYTES).expect("a label");
            }
        }
        let now = now_ms() / 1_000;
        for n in 0..8 {
            runtime::record_route_outcome(&self.cwd, &incumbent_run(n, now)).expect("a peer");
        }
    }

    /// Mark the seat's own ledger with a transition, as its judgment writes
    /// one.
    fn transition(&self, word: &str) {
        let row = json!({ "at": now_ms(), (TRANSITION.canonical): word });
        append_shadow_row(&challenger_path(&self.cwd), &row, SHADOW_LEDGER_MAX_BYTES).expect("a transition");
    }

    pub(crate) fn rows(&self) -> Vec<Value> {
        read_shadow_rows(&challenger_path(&self.cwd))
    }

    pub(crate) fn spend(&self) -> String {
        let dir = self.home.join(zerocode_core::jev::count::REQUESTS_DIR);
        let Ok(entries) = std::fs::read_dir(dir) else {
            return String::new();
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(spend::SPEND_FILE_PREFIX))
            })
            .collect();
        files.sort();
        files.iter().filter_map(|path| std::fs::read_to_string(path).ok()).collect()
    }

    /// How many requests reached the judge's wire.
    pub(crate) fn judged(&self) -> usize {
        self.mock.requests().len()
    }

    fn spend_on(&self, day: &str) -> String {
        std::fs::read_to_string(spend::spend_path(&self.home, day)).unwrap_or_default()
    }

    /// Whether the arm has written `attempt`'s sample.
    fn sampled(&self, attempt: &str) -> bool {
        let key = sample_attempt_key(attempt);
        runtime::read_route_outcomes(&self.cwd)
            .expect("records")
            .iter()
            .any(|record| record.run_id.as_deref() == Some(key.as_str()))
    }

    /// The samples the arm has written to the route-outcome ledger.
    pub(crate) fn samples(&self) -> usize {
        runtime::read_route_outcomes(&self.cwd)
            .expect("records")
            .iter()
            .filter(|record| record.is_seat_sample())
            .count()
    }
}

fn facts(key: &str, task: &str) -> AttemptFacts {
    AttemptFacts {
        key: key.to_string(),
        role: Some("coding".to_string()),
        risk: Some("low".to_string()),
        route_source: Some("auto".to_string()),
        incumbent_model: INCUMBENT.to_string(),
        task: task.to_string(),
        effort: None,
        retry_or_handover: false,
    }
}

/// A spawn key that draws, and whose blind shows `first` first.
pub(crate) fn a_key_that_draws(first: Side) -> String {
    (0..10_000)
        .map(|n| format!("agent-{n}#1"))
        .find(|key| draws(key) && Blind::over(key).first() == first)
        .expect("some key draws with this order")
}

fn a_key_that_does_not_draw() -> String {
    (0..10_000)
        .map(|n| format!("agent-{n}#1"))
        .find(|key| !draws(key))
        .expect("some key does not draw")
}

fn word(row: &Value, key: &zerocode_core::jev::summary::LedgerKey) -> Option<String> {
    key.read(row).and_then(Value::as_str).map(str::to_string)
}

fn now_ms() -> u64 {
    Today::now().now_ms
}

/// A clock that reads noon of `day`, in Seoul.
fn noon_of(day: &str) -> impl Fn() -> Today {
    let noon = zerocode_core::civil::epoch_ms_of_iso(&format!("{day}T12:00:00+09:00")).expect("a day");
    let noon = u64::try_from(noon).expect("after the epoch");
    move || Today::at(noon, SEOUL_MINUTES)
}

/// The body the judge was sent in `request`, parsed.
fn body_of(request: &str) -> Value {
    serde_json::from_str(request.split("\r\n\r\n").last().expect("a body")).expect("json")
}

/// A verifier's verdict about `attempt`'s work, naming the source it saw.
fn verdict_on(attempt: &str, status: &str, subject: VerdictSubject, at: u64, seen: Option<&str>) -> RouteOutcomeRecord {
    let mut record = RouteOutcomeRecord::new("subagent", "general-purpose", INCUMBENT, status)
        .with_signal("verdict")
        .with_decision(DecisionKind::Verify)
        .with_verdict_subject(subject)
        .with_attempt_key(attempt)
        .with_source(seen.map(str::to_string));
    record.recorded_at = at;
    record
}

/// A verdict about `attempt`'s work, its verifier having seen what the
/// attempt handed in.
pub(crate) fn verdict(attempt: &str, status: &str, subject: VerdictSubject, at: u64) -> RouteOutcomeRecord {
    verdict_on(attempt, status, subject, at, Some(HANDED_IN))
}

/// `attempt`'s own run row, naming the source it handed in.
fn handed_in(attempt: &str, source: Option<&str>) -> RouteOutcomeRecord {
    RouteOutcomeRecord::new("subagent", "general-purpose", INCUMBENT, runtime::OUTCOME_COMPLETED)
        .with_attempt_key(attempt)
        .with_source(source.map(str::to_string))
}

/// The product's sample writer, for `cwd`.
fn feed_into(cwd: &Path) -> impl Fn(&RouteOutcomeRecord) -> std::io::Result<()> + '_ {
    move |sample| runtime::record_route_outcome(cwd, sample)
}

/* ---- off, holds, the door -------------------------------------------- */

/// 꺼진 자리는 오늘과 바이트까지 같다: 행도, 예약도, 도전 설계 요청도, 문의 요청도, 세금 행도 없다.
#[test]
fn off_draws_nothing_designs_nothing_and_writes_nothing() {
    let rig = Rig::new(JevMode::Off, &jev_answer("first"), Scripted::answering("plan"));
    rig.spent_today(now_ms());
    let key = a_key_that_draws(Side::Challenger);
    assert!(rig.arm().open(facts(&key, "rename the flag")).is_none());
    rig.write_unreadable_settings();
    assert!(rig.arm().open(facts(&key, "t")).is_none(), "an unreadable setting is off");
    assert!(!challenger_path(&rig.cwd).exists(), "off writes no ledger");
    assert_eq!(rig.spend(), "", "off reserves nothing");
    assert_eq!(rig.designer.calls(), 0, "off asks no challenger");
    assert!(rig.mock.requests().is_empty(), "off asks the door nothing");
    assert!(runtime::read_route_outcomes(&rig.cwd).expect("records").is_empty(), "off files no tax");
    assert_eq!(note_challenger_verdicts(&rig.cwd), 0, "and labels nothing");
}

/// 붙들린 시도는 뽑혔을 때만 제 낱말을 적고 아무것도 쓰지 않는다 — 안 뽑힌 시도는 역할이 무엇이든 행이 없다.
#[test]
fn a_drawn_attempt_held_writes_its_word_and_an_undrawn_one_writes_nothing() {
    let rig = Rig::new(JevMode::Shadow, &jev_answer("first"), Scripted::answering("plan"));
    rig.spent_today(now_ms());
    let arm = rig.arm();
    let drawn = a_key_that_draws(Side::Challenger);
    let undrawn = a_key_that_does_not_draw();
    let held = |facts: AttemptFacts| assert!(arm.open(facts).is_none());

    held(AttemptFacts { role: None, ..facts(&drawn, "t") });
    held(AttemptFacts { route_source: Some("pin".to_string()), ..facts(&drawn, "t") });
    held(AttemptFacts { route_source: Some("explicit".to_string()), ..facts(&drawn, "t") });
    held(AttemptFacts { risk: Some("critical".to_string()), ..facts(&drawn, "t") });
    held(AttemptFacts { risk: Some("high".to_string()), ..facts(&drawn, "t") });
    held(AttemptFacts { retry_or_handover: true, ..facts(&drawn, "t") });
    held(AttemptFacts { role: Some("verifier".to_string()), ..facts(&drawn, "t") });
    held(AttemptFacts { role: None, ..facts(&undrawn, "t") });
    held(AttemptFacts { role: Some("verifier".to_string()), ..facts(&undrawn, "t") });
    held(facts(&undrawn, "t"));

    let words: Vec<String> = rig.rows().iter().filter_map(|row| word(row, &arm::HELD)).collect();
    assert_eq!(
        words,
        [Held::Role, Held::Pinned, Held::Pinned, Held::Guarded, Held::Guarded, Held::Retry, Held::Role]
            .map(|held| held.token().to_string())
            .to_vec(),
        "one row per drawn attempt held, and none for the three that did not draw"
    );
    for row in rig.rows() {
        assert!(OUTCOME.read(&row).is_none(), "a held row is not a request: {row}");
        assert_eq!(word(&row, &arm::ATTEMPT).as_deref(), Some(drawn.as_str()));
    }
    assert_eq!(rig.spend(), "");
    assert_eq!(rig.designer.calls(), 0);
    assert!(rig.mock.requests().is_empty());
}

/// 문이 비교를 거절할 판이면 설계도 안 산다 — 거절 낱말이 요청 행에 남고, 요청 0·예약 0.
#[test]
fn a_door_that_would_refuse_spends_nothing_on_a_design() {
    let rig = Rig::new(JevMode::Shadow, &jev_answer("first"), Scripted::answering("plan"));
    rig.spent_today(now_ms());
    rig.write_settings(Some(JevMode::Shadow), DoorWords { enabled: false, ..DoorWords::OPEN });
    let key = a_key_that_draws(Side::Challenger);
    rig.arm().open(facts(&key, "t")).expect("the cheap holds clear").finish(Some("INCUMBENT: a plan".to_string()));
    let rows = rig.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(word(&rows[0], &OUTCOME).as_deref(), Some(JevMode::Off.key()));
    assert_eq!(rows[0]["requests"], json!(0));
    assert_eq!(word(&rows[0], &arm::ATTEMPT).as_deref(), Some(key.as_str()));
    assert_eq!(rig.designer.calls(), 0, "no design is bought for a comparison the door refuses");
    assert_eq!(rig.spend(), "");
    assert!(rig.mock.requests().is_empty());
}

/// 도전할 모델이 없으면·있는데 가격이 없으면 붙든다 — 미상은 공짜가 아니다.
#[test]
fn no_challenger_and_an_unpriced_one_are_held_by_their_words() {
    let only_incumbent = ModelInventory::new(INCUMBENT, vec![coder(INCUMBENT, "anthropic", "claude", 50)]);
    let unpriced = ModelInventory::new(
        INCUMBENT,
        vec![coder(INCUMBENT, "anthropic", "claude", 50), coder("unpriced-coder", "anthropic", "claude", 60)],
    );
    let records = Vec::new();
    assert_eq!(
        pick_challenger(&only_incumbent, &records, 0, RouteRole::Coding, INCUMBENT, &[], priced).err(),
        Some(Held::NoChallenger)
    );
    assert_eq!(
        pick_challenger(&unpriced, &records, 0, RouteRole::Coding, INCUMBENT, &[], priced).err(),
        Some(Held::Unpriced)
    );
    assert_eq!(
        pick_challenger(&inventory(), &records, 0, RouteRole::Coding, INCUMBENT, &[], nothing_priced).err(),
        Some(Held::Unpriced)
    );
    let (picked, _) =
        pick_challenger(&inventory(), &records, 0, RouteRole::Coding, INCUMBENT, &[], priced).expect("a challenger");
    assert_eq!(picked, NEWCOMER);
}

/// 새로 발견된 모델이 먼저, 그다음 새 릴리스, 그다음 이름 — 그리고 표본이 찬 모델은 도전자가 아니다.
#[test]
fn a_challenger_is_the_newest_under_evidenced_model_the_router_could_route_to() {
    let inventory = ModelInventory::new(
        INCUMBENT,
        vec![
            coder(INCUMBENT, "anthropic", "claude", 50),
            coder(NEWCOMER, "anthropic", "claude", 60),
            coder(RIVAL, "openai", "gpt", 60),
            coder("zz-old-coder", "anthropic", "claude", 10),
        ],
    );
    let none: Vec<RouteOutcomeRecord> = Vec::new();
    let pick = |records: &[RouteOutcomeRecord], discovered: &[String]| {
        pick_challenger(&inventory, records, 1_000, RouteRole::Coding, INCUMBENT, discovered, priced)
            .expect("a challenger")
            .0
    };
    assert_eq!(pick(&none, &[]), NEWCOMER, "the newer release first, by name between equals");
    assert_eq!(pick(&none, &[RIVAL.to_string()]), RIVAL, "a discovered release comes first");
    let evidenced: Vec<RouteOutcomeRecord> = (0..16)
        .map(|n| {
            let model = if n % 2 == 0 { NEWCOMER } else { INCUMBENT };
            let mut record = RouteOutcomeRecord::new("subagent", "general-purpose", model, runtime::OUTCOME_COMPLETED)
                .with_role(Some("coding".to_string()))
                .with_route_source(Some("auto".to_string()))
                .with_attempt_key(format!("agent-{n}#1"));
            record.recorded_at = 1_000;
            record
        })
        .collect();
    assert_eq!(pick(&evidenced, &[]), RIVAL, "a model with a learned entry is no longer short of evidence");
}

/* ---- what leaves for the challenger's provider ---------------------------- */

/// 설계 요청이 공급자에게 싣는 과업은 문이 지운 말이다: 자격 증명 줄은 빠지고 캡에서 잘리며, 판정자에게 가는 과업도 같은 말이다.
/// 자격 증명이 없는 과업은 그대로 간다.
#[test]
fn a_design_request_masks_the_task_that_reaches_its_provider() {
    let secret = "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789";
    let task = format!("make the parser stricter\nexport OPENAI_API_KEY={secret}\nthen add a test for unknown keys");
    let rig = Rig::new(JevMode::Shadow, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.spent_today(now_ms());
    let key = a_key_that_draws(Side::Challenger);
    rig.arm().open(facts(&key, &task)).expect("drawn").finish(Some("INCUMBENT: a plan".to_string()));

    let seen = rig.designer.seen();
    assert_eq!(seen.len(), 1, "one design request");
    let sent = &seen[0].task_head;
    assert!(!sent.contains(secret), "the credential reached the challenger's provider: {sent}");
    assert!(sent.contains(door::WITHHELD_LINE), "the line is withheld in its place: {sent}");
    assert!(
        sent.starts_with("make the parser stricter\n") && sent.ends_with("then add a test for unknown keys"),
        "the rest of the task is the person's words: {sent}"
    );
    let judged = rig.mock.requests();
    assert_eq!(judged.len(), 1);
    assert!(!judged[0].contains(secret), "nor did it reach the judge");
    assert_eq!(body_of(&judged[0])["state"]["task"], json!(sent), "the judge reads the words the design was asked of");

    let long = "가".repeat(CHALLENGER_TASK_CHAR_CAP + 50);
    let door = JevDoor::open(&rig.cwd);
    let (cut, withheld) = cleared_task(&door, true, &long).expect("cleared");
    assert_eq!((cut.chars().count(), withheld), (CHALLENGER_TASK_CHAR_CAP, 0), "cut to the task's cap");
    assert_eq!(cleared_task(&door, true, "rename the flag").expect("cleared").0, "rename the flag");
}

/// 설계가 떠나기 직전에 문과 사람의 말을 다시 묻는다: 뽑는 사이에 동의를 거두거나, 자리를 끄거나, Jev를 끄면 설계 요청은
/// 떠나지 않고 예약은 풀리며 거절 낱말이 행에 남는다.
#[test]
fn a_word_taken_back_while_the_draw_picks_stops_the_design_before_it_leaves() {
    let cases: [(&str, Option<JevMode>, DoorWords, &str); 3] = [
        ("consent withdrawn", Some(JevMode::Shadow), DoorWords { consented: false, ..DoorWords::OPEN }, "not_consented"),
        ("the seat switched off", Some(JevMode::Off), DoorWords::OPEN, JevMode::Off.key()),
        ("Jev switched off", Some(JevMode::Shadow), DoorWords { enabled: false, ..DoorWords::OPEN }, JevMode::Off.key()),
    ];
    for (case, mode, door, refused) in cases {
        let rig = Rig::new(JevMode::Shadow, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
        rig.spent_today(now_ms());
        let settings = rig.home.join("settings.json");
        let cwd = rig.cwd.clone();
        let taken_back = move || {
            let workspaces: Vec<String> = if door.consented { vec![door::resolved_path(&cwd)] } else { Vec::new() };
            let mut smart = json!({ "jev": { "enabled": door.enabled, "workspaces": workspaces } });
            if let Some(mode) = mode {
                smart[CHALLENGER.setting] = json!(mode.key());
            }
            std::fs::write(&settings, json!({ SMART_SETTINGS_KEY: smart }).to_string()).expect("settings");
        };
        let key = a_key_that_draws(Side::Challenger);
        let drawn = rig.arm_picking(taken_back).open(facts(&key, "make the parser stricter")).expect("drawn");
        drawn.finish(Some("INCUMBENT: a plan".to_string()));
        assert_eq!(rig.designer.calls(), 0, "{case}: the design request did not leave");
        let book = spend::fold(&rig.spend());
        assert_eq!((book.reserved, book.settled, book.spent_micros), (0, 0, 0), "{case}: the reservation is released");
        let rows = rig.rows();
        assert_eq!(rows.len(), 1, "{case}: {rows:?}");
        assert_eq!(word(&rows[0], &OUTCOME).as_deref(), Some(refused), "{case}");
        assert_eq!(rows[0]["requests"], json!(0), "{case}");
        assert!(rig.mock.requests().is_empty(), "{case}: nothing reached the judge");
        assert!(runtime::read_route_outcomes(&rig.cwd).expect("records").is_empty(), "{case}: no tax for a request never sent");
    }
}

/// 설계가 나간 뒤 사람이 말을 거두면: 나간 설계는 치른 값으로 정산하되 비교도 표본도 보내지 않는다. 판정자 모델을 바꿨으면
/// 비교는 바뀐 모델에게 간다. 아무것도 안 바꿨으면 오늘처럼 비교한다.
#[test]
fn a_draw_finishing_after_off_or_consent_revocation_sends_no_comparison_or_sample() {
    // What the person changes while the design is on the wire, if anything.
    type Change = Option<(Option<JevMode>, DoorWords)>;
    let cases: [(&str, Change, Option<&str>); 5] = [
        ("nothing changed", None, Some(zerocode_core::jev::DEFAULT_MODEL)),
        ("the seat switched off", Some((Some(JevMode::Off), DoorWords::OPEN)), None),
        ("consent withdrawn", Some((Some(JevMode::On), DoorWords { consented: false, ..DoorWords::OPEN })), None),
        ("Jev switched off", Some((Some(JevMode::On), DoorWords { enabled: false, ..DoorWords::OPEN })), None),
        ("another judge pinned", Some((Some(JevMode::On), DoorWords { pin: Some("jev-1.14.0"), ..DoorWords::OPEN })), Some("jev-1.14.0")),
    ];
    for (case, change, asked) in cases {
        let (designer, let_go) = Scripted::answering_when_let_go("CHALLENGER: split the parser");
        let rig = Rig::new(JevMode::On, &jev_answer("first"), designer);
        rig.spent_today(now_ms());
        rig.seed_a_standing_window(true);
        let key = a_key_that_draws(Side::Challenger); // "first" → the challenger is preferred
        runtime::record_route_outcome(&rig.cwd, &handed_in(&key, Some(HANDED_IN))).expect("the attempt's run");
        runtime::record_route_outcome(&rig.cwd, &verdict(&key, runtime::OUTCOME_FAILED, VerdictSubject::Work, 5))
            .expect("its verdict");
        let drawn = rig.arm().open(facts(&key, "make the parser stricter")).expect("drawn");
        rig.designer.wait_until_asked();
        if let Some((mode, door)) = change {
            rig.write_settings(mode, door);
        }
        let_go.send(()).expect("let the design come back");
        drawn.finish(Some("INCUMBENT: reject unknown fields".to_string()));

        let book = spend::fold(&rig.spend());
        assert_eq!((book.reserved, book.settled), (0, 1), "{case}: a design that left is settled");
        assert!(book.spent_micros >= DESIGN_BILL, "{case}: at what it cost");
        let row = rig.rows().into_iter().find(|row| word(row, &arm::ATTEMPT).as_deref() == Some(key.as_str()));
        let row = row.expect("the attempt's row");
        let sent = rig.mock.requests();
        if let Some(model) = asked {
            assert_eq!(sent.len(), 1, "{case}: compared");
            assert_eq!(body_of(&sent[0])["model"], json!(model), "{case}: the judge pinned when it was asked");
            assert_eq!(word(&row, &OUTCOME).as_deref(), Some(door::ANSWERED_OUTCOME), "{case}");
            assert!(rig.sampled(&key), "{case}: an acting seat feeds the agreeing label");
        } else {
            assert!(sent.is_empty(), "{case}: no comparison was sent");
            assert_eq!(row["requests"], json!(0), "{case}");
            assert!(arm::PREFERRED.read(&row).is_none(), "{case}: nothing was preferred");
            assert_eq!(row[COST_MICROS.canonical].as_u64(), Some(book.spent_micros), "{case}: the design's cost, on its row");
            assert!(!rig.sampled(&key), "{case}: and no sample was written");
        }
    }
}

/* ---- the share --------------------------------------------------------- */

/// 같은 몫을 본 여럿이 둘 다 통과하지 않는다: 읽기→검사→예약은 한 걸음이다.
#[test]
fn the_share_admits_one_of_eight_draws_that_race_for_the_last_place() {
    let home = tempfile::tempdir().expect("a home");
    let racers: Vec<_> = (0..8)
        .map(|n| {
            let book = Reservation::for_day(home.path(), "2026-09-24");
            // A day that has bought a share of exactly one design: 10% of 100,000.
            std::thread::spawn(move || book.reserve(&format!("agent-{n}#1"), 10_000, 100_000, &noon_of("2026-09-24")).is_ok())
        })
        .collect();
    let admitted = racers
        .into_iter()
        .map(|racer| racer.join().expect("a racer"))
        .filter(|admitted| *admitted)
        .count();
    assert_eq!(admitted, 1, "the share had room for one design and admitted {admitted}");
    let book = Reservation::for_day(home.path(), "2026-09-24").book().expect("readable");
    assert_eq!((book.reserved, book.reserved_micros), (1, 10_000));
}

/// 한 시도는 하루에 한 번 잡힌다 — 같은 시도를 다시 열면 재시도로 붙든다.
#[test]
fn an_attempt_is_reserved_once_and_a_second_opening_is_a_retry() {
    let home = tempfile::tempdir().expect("a home");
    let today = noon_of("2026-09-24");
    let book = Reservation::for_day(home.path(), "2026-09-24");
    book.reserve("agent-1#1", 10, 1_000_000, &today).expect("room");
    assert_eq!(book.reserve("agent-1#1", 10, 1_000_000, &today).err(), Some(Held::Retry));
    book.settle("agent-1#1", 7);
    assert_eq!(book.reserve("agent-1#1", 10, 1_000_000, &today).err(), Some(Held::Retry), "settled is still named");
    let read = book.book().expect("readable");
    assert_eq!((read.spent_micros, read.reserved), (7, 0));
}

/// 정산은 예약을 끝내고 실비를 적는다; 같은 정산이 두 번 와도 한 번; 해제는 몫을 돌려준다; 재시작해도 장부는 같다.
#[test]
fn a_settlement_ends_the_reservation_and_a_restart_reads_the_same_book() {
    let home = tempfile::tempdir().expect("a home");
    let today = noon_of("2026-09-24");
    let book = Reservation::for_day(home.path(), "2026-09-24");
    book.reserve("agent-1#1", 10_000, 1_000_000, &today).expect("room");
    book.reserve("agent-2#1", 10_000, 1_000_000, &today).expect("room");
    book.settle("agent-1#1", 6_000);
    // A new process reading the same day: agent-2 died mid-draw and its
    // reservation still binds the share until the day ends.
    let restarted = Reservation::for_day(home.path(), "2026-09-24").book().expect("readable");
    assert_eq!((restarted.reserved_micros, restarted.spent_micros), (10_000, 6_000));
    let day = spend::day_spend(&restarted, 1_000_000);
    assert_eq!(day.day_micros, 1_006_000, "the arm's spend enters the whole once");
    book.settle("agent-1#1", 6_000);
    assert_eq!(book.book().expect("readable").spent_micros, 6_000, "a settlement written twice counts once");
    book.release("agent-2#1");
    let released = book.book().expect("readable");
    assert_eq!((released.reserved_micros, released.spent_micros), (0, 6_000));
}

/// 자정: 23:59:59에 잡은 예약은 00:00:01에 돌아온 정산도 그날 장부에 적힌다; 다음 날 장부는 비어 있고, 다음 날 첫 예약이 정산이
/// 끝난 지난날을 치운다.
#[test]
fn a_draw_across_midnight_settles_in_the_day_it_was_charged() {
    let rig = Rig::new(JevMode::Shadow, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    let seoul_now = Today::at(now_ms(), SEOUL_MINUTES);
    let last_second = seoul_now.start_ms + 86_400_000 - 1_000;
    let before = Today::at(last_second, SEOUL_MINUTES);
    let after = Today::at(last_second + 2_000, SEOUL_MINUTES);
    assert_ne!(before.day, after.day, "the two instants straddle a midnight");
    rig.spent_today(last_second);
    rig.hold_clock(last_second);
    let key = a_key_that_draws(Side::Challenger);
    let drawn = rig.arm().open(facts(&key, "t")).expect("drawn");
    let started = Instant::now();
    while !spend::names(&rig.spend_on(&before.day), &key) && started.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(spend::names(&rig.spend_on(&before.day), &key), "charged before midnight");
    rig.hold_clock(last_second + 2_000);
    drawn.finish(Some("INCUMBENT: a plan".to_string()));
    let charged = spend::fold(&rig.spend_on(&before.day));
    assert_eq!((charged.reserved, charged.settled), (0, 1), "reserved and settled in the day it was drawn");
    assert_eq!(rig.spend_on(&after.day), "", "the next day's book holds nothing of it");

    let next = after.clone();
    Reservation::for_day(&rig.home, &after.day)
        .reserve("agent-next#1", 1, 1_000_000, &move || next.clone())
        .expect("room");
    assert_eq!(rig.spend_on(&before.day), "", "the next day's first reservation forgets a settled day before");
}

/// 늦은 전날 예약은 새날 장부를 지우지 못한다: 자정 전에 날을 정한 뽑기가 자정 뒤에 장부에 닿으면, 끝난 날에는 적지 않고
/// 거절하며, 새날의 첫 예약과 그 몫은 그대로다.
#[test]
fn a_late_previous_day_reservation_cannot_delete_todays_book() {
    let home = tempfile::tempdir().expect("a home");
    let (ended, begun) = ("2026-09-24", "2026-09-25");
    Reservation::for_day(home.path(), begun)
        .reserve("agent-b#1", 10_000, 1_000_000, &noon_of(begun))
        .expect("the new day's first reservation");
    // A draw that read its day before midnight reaches the book after it.
    assert_eq!(
        Reservation::for_day(home.path(), ended).reserve("agent-a#1", 10_000, 1_000_000, &noon_of(begun)).err(),
        Some(Held::DayBudget),
        "a day that has ended is not charged"
    );
    let today = Reservation::for_day(home.path(), begun).book().expect("the new day's book");
    assert_eq!((today.reserved, today.reserved_micros), (1, 10_000), "the new day's book is as it was");
    // The new day's share is 100,000 of which 10,000 is taken: 95,000 more does not fit.
    assert_eq!(
        Reservation::for_day(home.path(), begun).reserve("agent-c#1", 95_000, 1_000_000, &noon_of(begun)).err(),
        Some(Held::DayBudget),
        "the reservation taken in the new day still binds its share"
    );
    assert!(
        Reservation::for_day(home.path(), begun).reserve("agent-d#1", 90_000, 1_000_000, &noon_of(begun)).is_ok(),
        "and what is left of it is still there"
    );
}

/// 읽을 수 없는 장부는 빈 예산이 아니다: 예약을 거절하고, 읽을 수 없는 동안 아무것도 청구하지 않는다.
#[cfg(unix)]
#[test]
fn an_unreadable_spend_book_never_becomes_an_empty_budget() {
    use std::os::unix::fs::PermissionsExt as _;
    let home = tempfile::tempdir().expect("a home");
    let day = "2026-09-24";
    let book = Reservation::for_day(home.path(), day);
    // A share of 100,000; 90,000 of it taken.
    book.reserve("agent-1#1", 90_000, 1_000_000, &noon_of(day)).expect("room");
    let path = spend::spend_path(home.path(), day);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).expect("write-only");
    let refused = book.reserve("agent-2#1", 50_000, 1_000_000, &noon_of(day));
    let unread = book.book();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("readable again");
    assert_eq!(refused.err(), Some(Held::DayBudget), "a book that cannot be read is not an empty day");
    assert!(unread.is_none(), "and reads as no book, not as an empty one");
    let after = book.book().expect("readable");
    assert_eq!((after.reserved, after.reserved_micros), (1, 90_000), "nothing was charged while it could not be read");
}

/// The name the test harness knows the lock holder by.
const LOCK_HOLDER_TEST: &str = "hold_a_spend_book_lock_until_killed";
/// Where the holder is told which book to lock, and where to say it holds it.
const HOLD_BOOK_ENV: &str = "ZO_TEST_CHALLENGER_HOLD_BOOK";
const HOLD_MARK_ENV: &str = "ZO_TEST_CHALLENGER_HOLD_MARK";

/// 장부의 잠금을 잡은 채 죽은 프로세스: 살아 있는 동안은 예약이 기다리다 거절하고, 실제로 죽인 뒤에는 잠금을 되찾아 같은
/// 장부를 그대로 읽는다 — 미결 예약은 그날 끝까지 몫을 물고, 죽은 잠금은 몫을 막지 않는다.
#[cfg(unix)]
#[test]
fn a_book_whose_holder_was_killed_is_taken_back_and_reads_the_same() {
    let home = tempfile::tempdir().expect("a home");
    let day = "2026-09-24";
    let book = Reservation::for_day(home.path(), day);
    book.reserve("agent-1#1", 10_000, 1_000_000, &noon_of(day)).expect("room");
    let path = spend::spend_path(home.path(), day);
    let lock = path.with_extension("json.lock");
    let mark = home.path().join("holding");
    let test_path = module_path!().split_once("::").map_or("", |(_, rest)| rest);
    let mut holder = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args([format!("{test_path}::{LOCK_HOLDER_TEST}").as_str(), "--exact", "--ignored", "--test-threads=1"])
        .env(HOLD_BOOK_ENV, &path)
        .env(HOLD_MARK_ENV, &mark)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("a holder process");
    let started = Instant::now();
    while !mark.exists() && started.elapsed() < Duration::from_secs(60) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(mark.exists(), "the holder took the lock");
    assert_eq!(
        book.reserve("agent-2#1", 10_000, 1_000_000, &noon_of(day)).err(),
        Some(Held::DayBudget),
        "a live holder's lock is not taken: the share cannot be kept while another holds the book"
    );
    holder.kill().expect("kill the holder");
    holder.wait().expect("the holder is gone");
    assert!(lock.exists(), "a killed holder never lets go of its own accord");
    let taken = book
        .reserve("agent-3#1", 10_000, 1_000_000, &noon_of(day))
        .expect("a dead holder's lock is taken back");
    assert_eq!((taken.reserved, taken.reserved_micros), (2, 20_000), "the book reads as it stood, and this one on it");
    assert!(!lock.exists(), "and let go again");
}

/// The other half of the test above: run as a child process, it takes the
/// book's lock, says so, and holds it until it is killed.
#[test]
#[ignore = "a child of a_book_whose_holder_was_killed_is_taken_back_and_reads_the_same: holds a lock until killed"]
fn hold_a_spend_book_lock_until_killed() {
    let (Some(book), Some(mark)) = (std::env::var_os(HOLD_BOOK_ENV), std::env::var_os(HOLD_MARK_ENV)) else {
        return;
    };
    let _held = runtime::SettingsFileLock::acquire(Path::new(&book)).expect("the book's lock");
    std::fs::write(mark, "").expect("say it is held");
    std::thread::sleep(Duration::from_secs(120));
}

/// 하루의 다른 지출은 오늘 건드린 세션의 요청 원장을 한 가격표로 값 매긴 합이다; 어제 행과 가격 없는 행은 들지 않는다.
#[test]
fn the_days_other_spend_is_todays_request_rows_priced_by_the_one_table() {
    let home = tempfile::tempdir().expect("a home");
    let root = home.path().join("cache").join("prompt-cache");
    let start_ms = 1_000_000_000_000;
    let row = |ts: u64, model: &str, input: u32, output: u32| {
        json!({"seq": 1, "ts_unix_ms": ts, "model": model, "provider": "x", "cache_creation": 0,
               "cache_read": 100, "input_uncached": input, "output": output, "message_count": 1, "broke": false})
        .to_string()
    };
    let session = |name: &str, lines: &[String]| {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).expect("a session dir");
        std::fs::write(dir.join("requests.jsonl"), lines.join("\n") + "\n").expect("a ledger");
    };
    session("s-today", &[row(start_ms + 5, INCUMBENT, 1_000, 100), row(start_ms - 5, INCUMBENT, 1_000_000, 0)]);
    session("s-unpriced", &[row(start_ms + 9, "unpriced-coder", 1_000_000, 1_000_000)]);
    // 1,000 in × $5 + 100 out × $25 + 100 cache reads × $0.5 = 5,000 + 2,500 + 50.
    assert_eq!(day_request_micros(std::slice::from_ref(&root), start_ms, priced), 7_550);
    assert_eq!(day_request_micros(&[root], u64::MAX, priced), 0, "a ledger not touched since the day began is not read");
}

/// 예약은 천장(바이트마다 한 토큰 + `max_tokens` 전부 + 비교의 상한)이고, 정산은 청구서다 — 청구서가 없으면 천장으로.
#[test]
fn the_reservation_is_a_ceiling_and_the_settlement_is_the_bill() {
    let rig = Rig::new(JevMode::Shadow, &jev_answer("first"), Scripted::answering("plan"));
    let price = priced("x").expect("a price");
    let (head, _) = cleared_task(&JevDoor::open(&rig.cwd), true, &"가".repeat(3_000)).expect("cleared");
    assert_eq!(head.chars().count(), CHALLENGER_TASK_CHAR_CAP);
    let expected = expected_design_micros(&price, &head);
    let input_bound = u64::try_from(head.len() + DESIGN_INSTRUCTION.len()).expect("small");
    let output_bound = u64::try_from(CHALLENGER_DESIGN_MAX_TOKENS).expect("small");
    assert_eq!(expected, arm::expected_micros(input_bound, output_bound, price.input, price.output));
    assert!(head.len() > head.chars().count(), "a script of three bytes a character is bounded by its bytes");
    let billed = DesignReply {
        text: Some("plan".to_string()),
        usage: Some(Usage {
            input_tokens: 400,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 20,
            output_tokens: 120,
            output_tokens_details: None,
        }),
        left: true,
        status: runtime::OUTCOME_COMPLETED,
        elapsed_ms: 1,
    };
    // 400×5 + 120×25 + 20×0.5 + 10×6.25 = 2,000 + 3,000 + 10 + 62.5 → 5,073.
    assert_eq!(settled_design_micros(&price, &billed, expected), 5_073);
    let unknown = DesignReply::missing(true, runtime::OUTCOME_FAILED, 1);
    assert_eq!(settled_design_micros(&price, &unknown, expected), expected, "a bill nobody reported settles at the ceiling");
    let rate = api::systemone_rate(zerocode_core::jev::DEFAULT_MODEL).expect("the judge is priced");
    let judge = expected_comparison_micros(&rate, 1_000);
    let bound = u64::try_from(1_000 + CHALLENGER_DESIGN_CAP * CHALLENGER_DESIGN_BYTE_CAP).expect("small");
    assert_eq!(judge, arm::expected_micros(bound, 0, rate.input, 0.0));
    assert!(judge > 0);
}

/// 비교는 나간 전송마다 정산한다: 답한 전송은 보고된 입력으로, 보고 없이 나간 나머지(선이 다시 보낸 실패·돌아오지 않은 요청)는
/// 보낸 바이트로 — 미상은 0이 아니다; 문이 내보내지 않은 비교는 0. 다시 보낸 비교는 한 요청의 천장인 예약을 넘을 수 있다.
#[test]
fn a_comparison_is_settled_send_by_send_and_an_unreported_send_at_its_bytes() {
    let rate = api::systemone_rate(zerocode_core::jev::DEFAULT_MODEL).expect("the judge is priced");
    let micros = |tokens: u64| arm::expected_micros(tokens, 0, rate.input, 0.0);
    let wire = |requests: u32, retries: u32, input_tokens: Option<u64>| {
        let mut wire = Wire::silent(String::new());
        wire.requests = requests;
        wire.retries = retries;
        wire.input_tokens = input_tokens;
        wire.sent_bytes = 1_000;
        wire
    };
    assert_eq!(settled_comparison_micros(&rate, &wire(0, 0, None)), 0, "nothing left, nothing charged");
    assert_eq!(settled_comparison_micros(&rate, &wire(1, 0, Some(300))), micros(300), "one send: the input it reported");
    assert_eq!(settled_comparison_micros(&rate, &wire(1, 0, None)), micros(1_000), "one send that reported nothing: its bytes");
    assert_eq!(
        settled_comparison_micros(&rate, &wire(3, 2, Some(300))),
        micros(300 + 2 * 1_000),
        "two failed tries re-sent before the answer: each at the bytes it carried"
    );
    assert_eq!(settled_comparison_micros(&rate, &wire(3, 2, None)), micros(3 * 1_000), "three sends and no answer: every one at its bytes");
    assert!(
        settled_comparison_micros(&rate, &wire(3, 2, Some(300))) > settled_comparison_micros(&rate, &wire(1, 0, Some(300))),
        "a re-sent comparison costs more than the one request its reservation was the ceiling of"
    );
}

/* ---- the comparison ---------------------------------------------------- */

/// 뽑힌 시도: 설계 요청 하나·예약·이름 없는 두 설계·자리 낱말로 온 답이 제 쪽으로 풀리고, 설계+비교의 실비로 정산한다 — 네 칸 모두.
#[test]
fn a_drawn_attempt_is_designed_reserved_compared_blind_and_read_back_to_its_side() {
    for (first, chosen, preferred) in [
        (Side::Challenger, "first", Preferred::Challenger),
        (Side::Incumbent, "first", Preferred::Incumbent),
        (Side::Challenger, "second", Preferred::Incumbent),
        (Side::Incumbent, "neither", Preferred::Neither),
    ] {
        let design = "CHALLENGER: split the parser and test each half";
        let rig = Rig::new(JevMode::Shadow, &jev_answer(chosen), Scripted::answering(design));
        rig.spent_today(now_ms());
        let key = a_key_that_draws(first);
        let task = "make the parser stricter";
        let incumbent_plan = "INCUMBENT: reject unknown fields at the root";
        rig.arm().open(facts(&key, task)).expect("a drawn attempt").finish(Some(incumbent_plan.to_string()));

        assert_eq!(rig.designer.calls(), 1, "one design request");
        let seen = rig.designer.seen();
        assert_eq!(seen[0].model, NEWCOMER);
        assert_eq!(seen[0].task_head, task);
        assert_eq!(seen[0].max_tokens, u32::try_from(CHALLENGER_DESIGN_MAX_TOKENS).expect("small"));

        let rows = rig.rows();
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert_eq!(word(row, &OUTCOME).as_deref(), Some(door::ANSWERED_OUTCOME));
        assert_eq!(word(row, &arm::ATTEMPT).as_deref(), Some(key.as_str()));
        assert_eq!(word(row, &arm::PREFERRED).as_deref(), Some(preferred.token()), "{first:?} {chosen}");
        assert_eq!(row[arm::WON.canonical], json!(preferred == Preferred::Challenger));
        assert_eq!(word(row, &arm::BLIND).as_deref(), Some(Blind::over(&key).token()));
        assert_eq!(word(row, &INCUMBENT_MODEL).as_deref(), Some(INCUMBENT));
        assert_eq!(word(row, &CHALLENGER_MODEL).as_deref(), Some(NEWCOMER));
        assert_eq!(word(row, &arm::INCUMBENT_DESIGN), Some(fingerprint_of(incumbent_plan)));
        assert_eq!(word(row, &arm::CHALLENGER_DESIGN), Some(fingerprint_of(design)));
        let rate = api::systemone_rate(zerocode_core::jev::DEFAULT_MODEL).expect("priced");
        let judge_bill = arm::expected_micros(300, 0, rate.input, 0.0);
        let expected = row[arm::EXPECTED_MICROS.canonical].as_u64().expect("expected");
        let cost = row[COST_MICROS.canonical].as_u64().expect("cost");
        assert_eq!(cost, DESIGN_BILL + judge_bill, "settled at the design's bill and the judge's");
        assert!(cost <= expected, "the bill fits the ceiling: {cost} <= {expected}");
        assert_eq!(row["requests"], json!(1));
        assert_eq!(row["model"], json!("jev-test"));
        assert!(row["requestDigest"].is_string());
        let text = row.to_string();
        assert!(!text.contains(task) && !text.contains(design) && !text.contains(incumbent_plan), "no words in the row: {row}");

        let sent = rig.mock.requests();
        assert_eq!(sent.len(), 1);
        let body = body_of(&sent[0]);
        let shown = body["state"]["designs"].as_array().expect("two designs");
        assert_eq!(shown.len(), 2);
        let first_body = shown[0]["body"].as_str().expect("first body");
        match first {
            Side::Challenger => assert_eq!(first_body, design),
            Side::Incumbent => assert_eq!(first_body, incumbent_plan),
        }
        assert_eq!(body["state"]["task"], json!(task));
        for name in [INCUMBENT, NEWCOMER, "incumbent", "challenger"] {
            assert!(!body.to_string().contains(name), "{name} reached the judge");
        }

        let book = spend::fold(&rig.spend());
        assert_eq!((book.reserved, book.settled, book.spent_micros), (0, 1, cost));

        let records = runtime::read_route_outcomes(&rig.cwd).expect("records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].decision_kind(), DecisionKind::Classify);
        assert_eq!(records[0].target, runtime::RouteTaxCall::Challenger.as_str());
        assert_eq!(records[0].run_id.as_deref(), Some(key.as_str()));
        assert_eq!(records[0].selected_model, NEWCOMER);
        assert!(records[0].decision_kind().is_bookkeeping(), "the learner skips it");
    }
}

/// 재시도해도 좌우는 바뀌지 않는다: 같은 시도는 같은 순서, 같은 요청.
#[test]
fn the_same_attempt_is_shown_in_the_same_order_every_time() {
    let designs = Designs { incumbent: "a plan", challenger: "another plan" };
    let key = a_key_that_draws(Side::Challenger);
    let once = arm::ask(&key, "t", &designs);
    let again = arm::ask(&key, "t", &designs);
    assert_eq!(request_body(&once), request_body(&again));
    assert_eq!(once.blind(), again.blind());
}

/// 설계 속 자격 증명 줄은 문이 가린다 — 판정자에게도 행에도 가지 않는다.
#[test]
fn a_credential_in_a_design_is_withheld_at_the_door() {
    let rig = Rig::new(JevMode::Shadow, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.spent_today(now_ms());
    let secret = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789";
    let key = a_key_that_draws(Side::Challenger);
    rig.arm()
        .open(facts(&key, "t"))
        .expect("drawn")
        .finish(Some(format!("INCUMBENT: set ANTHROPIC_API_KEY={secret}\nthen run the tests")));
    let sent = rig.mock.requests();
    assert_eq!(sent.len(), 1);
    assert!(!sent[0].contains(secret), "the credential left the machine");
    assert!(sent[0].contains("then run the tests"), "the line after it did not");
    let rows = rig.rows();
    assert_eq!(rows[0]["redactedLines"], json!(1));
    assert!(!rows[0].to_string().contains(secret));
}

/// 판정자가 벽을 넘기면 선호가 없고 승리도 라벨도 없다 — 보낸 바이트만큼은 비용으로 친다.
#[test]
fn a_comparison_that_misses_its_wall_prefers_nothing_and_is_never_labelled() {
    let rig = Rig::new(JevMode::Shadow, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.spent_today(now_ms());
    let silent = Mock::silent();
    let key = a_key_that_draws(Side::Challenger);
    rig.arm_on(&silent.base_url, Box::new(|_| inventory()))
        .open(facts(&key, "t"))
        .expect("drawn")
        .finish(Some("INCUMBENT: a plan".to_string()));
    let rows = rig.rows();
    assert_eq!(rows.len(), 1);
    assert_ne!(word(&rows[0], &OUTCOME).as_deref(), Some(door::ANSWERED_OUTCOME));
    assert!(arm::PREFERRED.read(&rows[0]).is_none() && arm::WON.read(&rows[0]).is_none());
    assert!(rows[0][COST_MICROS.canonical].as_u64().expect("cost") > DESIGN_BILL, "the bytes that left are charged");
    runtime::record_route_outcome(&rig.cwd, &handed_in(&key, Some(HANDED_IN))).expect("the run");
    runtime::record_route_outcome(&rig.cwd, &verdict(&key, runtime::OUTCOME_FAILED, VerdictSubject::Work, 5)).expect("a verdict");
    assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::Shadow), &feed_into(&rig.cwd)), 0, "no preference, no label");
}

/// 현직이 계획 없이 도구부터 썼으면 비교하지 않는다 — 빈 문자열과 견주어 이기는 쪽은 없고, 산 설계 값은 정산한다.
#[test]
fn a_missing_plan_is_compared_with_nothing_and_still_settles_its_cost() {
    let rig = Rig::new(JevMode::Shadow, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.spent_today(now_ms());
    let key = a_key_that_draws(Side::Challenger);
    rig.arm().open(facts(&key, "t")).expect("drawn").finish(None);
    let rows = rig.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(word(&rows[0], &arm::HELD).as_deref(), Some(Held::NoDesign.token()));
    assert!(OUTCOME.read(&rows[0]).is_none(), "not a request");
    assert_eq!(rows[0][COST_MICROS.canonical], json!(DESIGN_BILL), "the design was bought and is charged");
    assert!(rig.mock.requests().is_empty(), "the judge was not asked");
    assert_eq!(spend::fold(&rig.spend()).settled, 1);
}

/// 도전자의 설계가 나갔는데 안 돌아오면 천장으로 정산하고, 아예 안 나갔으면 예약을 풀어 0을 쓴다.
#[test]
fn a_design_that_left_settles_at_its_ceiling_and_one_that_never_left_is_released() {
    for (designer, left) in [(Scripted::silent(), true), (Scripted::unsent(), false)] {
        let rig = Rig::new(JevMode::Shadow, &jev_answer("first"), designer);
        rig.spent_today(now_ms());
        let key = a_key_that_draws(Side::Incumbent);
        rig.arm().open(facts(&key, "t")).expect("drawn").finish(Some("INCUMBENT: a plan".to_string()));
        let rows = rig.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(word(&rows[0], &arm::HELD).as_deref(), Some(Held::NoDesign.token()));
        let book = spend::fold(&rig.spend());
        assert_eq!(book.reserved, 0, "nothing stays reserved");
        let tax = runtime::read_route_outcomes(&rig.cwd).expect("records");
        if left {
            assert_eq!(book.settled, 1);
            assert!(book.spent_micros > DESIGN_BILL, "a design that left and reported nothing settles at its ceiling");
            assert_eq!(tax.len(), 1, "a request that left is tax");
        } else {
            assert_eq!((book.settled, book.spent_micros), (0, 0), "a request that never left costs nothing");
            assert!(tax.is_empty(), "and is no tax");
        }
        assert!(rig.mock.requests().is_empty());
    }
}

/// 현직의 설계는 첫 턴에서 도구를 부르기 전에 쓴 첫 글이다 — 도구가 먼저면 설계가 없고, 도구 결과는 설계가 아니다.
#[test]
fn the_incumbents_design_is_its_first_words_before_any_tool_call() {
    let text = |words: &str| ConversationMessage::assistant(vec![ContentBlock::Text { text: words.to_string() }]);
    let tool_use = || ContentBlock::ToolUse { id: "t".to_string(), name: "bash".to_string(), input: "{}".to_string() };
    assert_eq!(
        first_design_text(&[text("  I will split the parser.  "), ConversationMessage::assistant(vec![tool_use()])]),
        Some("I will split the parser.".to_string())
    );
    assert_eq!(first_design_text(&[text(""), text("second words")]), Some("second words".to_string()));
    assert_eq!(
        first_design_text(&[ConversationMessage::assistant(vec![tool_use()]), text("after acting")]),
        None,
        "a plan written after acting is not a plan"
    );
    assert_eq!(first_design_text(&[]), None);
    let planned = ConversationMessage::assistant(vec![ContentBlock::Text { text: "plan first".to_string() }, tool_use()]);
    assert_eq!(first_design_text(&[planned]), Some("plan first".to_string()));
    let acted = ConversationMessage::assistant(vec![tool_use(), ContentBlock::Text { text: "then words".to_string() }]);
    assert_eq!(first_design_text(&[acted]), None);
    let tool_result = ConversationMessage::tool_result("t", "bash", "a plan somebody else wrote", false);
    assert_eq!(first_design_text(&[tool_result]), None, "a tool's output is not the attempt's plan");
}

/* ---- the receipt ------------------------------------------------------- */

/// 영수증은 그 시도의 일에 대한 첫 verdict다 — 완료 행·검증자 제 잘못·정하지 못한 verdict·남의 시도는 영수증이 아니다.
#[test]
fn a_receipt_is_the_first_settled_verdict_on_the_attempts_work_and_never_a_completion() {
    let attempt = "agent-9#1";
    let run = || handed_in(attempt, Some(HANDED_IN));
    let receipt = |verdicts: &[RouteOutcomeRecord]| {
        let records: Vec<RouteOutcomeRecord> = std::iter::once(run()).chain(verdicts.iter().cloned()).collect();
        OnRecord::of(&records).receipt(attempt).map(|receipted| receipted.receipt)
    };
    assert_eq!(receipt(&[]), None, "a finished attempt is not a receipt");
    assert_eq!(
        receipt(&[verdict(attempt, runtime::OUTCOME_FAILED, VerdictSubject::Validator, 1)]),
        None,
        "the verifier's own fault says nothing of the work"
    );
    assert_eq!(
        receipt(&[verdict(attempt, runtime::OUTCOME_STOPPED, VerdictSubject::Work, 1)]),
        None,
        "a verifier that settled nothing labels nothing"
    );
    let failed_first = [
        verdict(attempt, runtime::OUTCOME_FAILED, VerdictSubject::Work, 5),
        verdict(attempt, runtime::OUTCOME_COMPLETED, VerdictSubject::Work, 9),
    ];
    assert_eq!(receipt(&failed_first), Some(Receipt::Failed), "the first verdict binds");
    let out_of_order = [
        verdict(attempt, runtime::OUTCOME_COMPLETED, VerdictSubject::Work, 9),
        verdict(attempt, runtime::OUTCOME_FAILED, VerdictSubject::Work, 5),
    ];
    assert_eq!(receipt(&out_of_order), Some(Receipt::Failed), "by the clock, not by the file's order");
    let records: Vec<RouteOutcomeRecord> = std::iter::once(run()).chain(out_of_order.iter().cloned()).collect();
    assert_eq!(
        OnRecord::of(&records).receipt(attempt).map(|receipted| receipted.verdict_at),
        Some(5),
        "and it keeps when the verdict that binds was recorded"
    );
    assert_eq!(
        receipt(&[verdict("agent-8#1", runtime::OUTCOME_FAILED, VerdictSubject::Work, 1)]),
        None,
        "another attempt's receipt is not this one's"
    );
}

/// 영수증은 검증자가 본 source가 그 시도가 넘긴 source와 같을 때만이다: 다른 source·source 없는 verdict는 판정 불가이고,
/// 넘긴 source를 모르는 시도에는 영수증이 없다. 맞는 첫 verdict가 묶고, 앞선 다른 source의 verdict는 그것을 막지 않는다.
#[test]
fn a_receipt_is_bound_to_the_source_the_attempt_handed_in() {
    let attempt = "agent-7#1";
    let with = |run: Option<&str>, verdicts: &[RouteOutcomeRecord]| {
        let records: Vec<RouteOutcomeRecord> =
            std::iter::once(handed_in(attempt, run)).chain(verdicts.iter().cloned()).collect();
        OnRecord::of(&records).receipt(attempt).map(|receipted| (receipted.receipt, receipted.source))
    };
    let seen = |source: Option<&str>, status: &str, at: u64| verdict_on(attempt, status, VerdictSubject::Work, at, source);
    assert_eq!(
        with(Some(HANDED_IN), &[seen(Some(HANDED_IN), runtime::OUTCOME_FAILED, 5)]),
        Some((Receipt::Failed, HANDED_IN.to_string())),
        "the same source: a receipt, and what it judged"
    );
    assert_eq!(
        with(Some(HANDED_IN), &[seen(Some("tree-after-another-edit"), runtime::OUTCOME_FAILED, 5)]),
        None,
        "a verdict about another source of the same attempt is not evaluable"
    );
    assert_eq!(
        with(Some(HANDED_IN), &[seen(None, runtime::OUTCOME_FAILED, 5)]),
        None,
        "a verdict that names no source is not evaluable"
    );
    assert_eq!(
        with(None, &[seen(Some(HANDED_IN), runtime::OUTCOME_FAILED, 5)]),
        None,
        "an attempt whose source nobody named has no receipt"
    );
    assert_eq!(
        with(
            Some(HANDED_IN),
            &[seen(Some("tree-elsewhere"), runtime::OUTCOME_COMPLETED, 3), seen(Some(HANDED_IN), runtime::OUTCOME_FAILED, 5)]
        ),
        Some((Receipt::Failed, HANDED_IN.to_string())),
        "the first verdict on the work handed in binds; one about other work does not stand in its way"
    );
}

/// 라벨은 영수증이 든 뒤 한 번: 완료만으로는 0, verdict가 오면 1(판정한 source와 함께), 다시·충돌하는 verdict·동시 호출이 와도 1.
#[test]
fn a_verdict_labels_a_comparison_once_and_a_completion_labels_it_never() {
    let rig = Rig::new(JevMode::Shadow, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.spent_today(now_ms());
    let key = a_key_that_draws(Side::Challenger); // "first" → the challenger is preferred
    rig.arm().open(facts(&key, "t")).expect("drawn").finish(Some("INCUMBENT: a plan".to_string()));
    assert_eq!(rig.rows().len(), 1);

    runtime::record_route_outcome(&rig.cwd, &handed_in(&key, Some(HANDED_IN))).expect("a completion");
    assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::Shadow), &feed_into(&rig.cwd)), 0);
    assert_eq!(rig.rows().len(), 1, "completion is not a receipt");

    runtime::record_route_outcome(&rig.cwd, &verdict(&key, runtime::OUTCOME_FAILED, VerdictSubject::Work, 5)).expect("a verdict");
    runtime::record_route_outcome(&rig.cwd, &verdict(&key, runtime::OUTCOME_COMPLETED, VerdictSubject::Work, 9)).expect("a later verdict");
    let racers: Vec<_> = (0..4)
        .map(|_| {
            let cwd = rig.cwd.clone();
            std::thread::spawn(move || note_verdicts_in(&cwd, Some(JevMode::Shadow), &feed_into(&cwd)))
        })
        .collect();
    let labelled: usize = racers.into_iter().map(|racer| racer.join().expect("a racer")).sum();
    assert_eq!(labelled, 1, "four callers racing for one attempt label it once");
    let rows = rig.rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(word(&rows[1], &LABEL).as_deref(), Some(key.as_str()));
    assert_eq!(word(&rows[1], &arm::VERIFIED).as_deref(), Some(Receipt::Failed.token()), "the first verdict, not the later pass");
    assert_eq!(word(&rows[1], &arm::VERIFIED_SOURCE).as_deref(), Some(HANDED_IN), "and the source it judged");
    assert_eq!(rows[1][arm::WON.canonical], json!(true));
    assert_eq!(rows[1][AGREED.canonical], json!(true));
    assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::Shadow), &feed_into(&rig.cwd)), 0, "and again, nothing");
    assert_eq!(rig.samples(), 0, "shadow feeds no sample");
}

fn request_row(attempt: &str, preferred: Preferred) -> Value {
    let mut row = json!({"at": 1, "outcome": door::ANSWERED_OUTCOME, "elapsedMs": 10, "model": "jev-test"});
    let columns = Comparison {
        attempt,
        role: "coding",
        incumbent_model: INCUMBENT,
        challenger_model: NEWCOMER,
        expected_micros: 1,
        cost_micros: 1,
        incumbent_design: "a",
        challenger_design: "b",
        blind: Blind::over(attempt),
        preferred: Some(preferred),
    }
    .columns();
    for (key, value) in columns {
        row[key] = value;
    }
    row
}

/// 영수증 셋 × 선호 셋의 표 전체를 실제 독자 `labels_due`·`labelled_in`이 핵심 규칙 `quality` 그대로 읽는다: 영수증 없음은 라벨
/// 없음, 「현직 통과 + 도전자 선호」는 판정 불가(agreed 없음), 표본은 판정자와 영수증이 맞은 칸에서만.
#[test]
fn the_whole_truth_table_is_read_through_the_labeller_and_only_agreeing_cells_feed_the_router() {
    for receipt in [None, Some(Receipt::Passed), Some(Receipt::Failed)] {
        for preferred in [Preferred::Incumbent, Preferred::Challenger, Preferred::Neither] {
            let mut rows = vec![request_row("agent-1#1", preferred)];
            let due = labels_due(&rows, |_| receipt.map(receipted), 7);
            let Some(receipt) = receipt else {
                assert!(due.is_empty(), "no receipt, no label ({preferred:?})");
                continue;
            };
            assert_eq!(due.len(), 1);
            let label = &due[0];
            let graded = arm::quality(Some(receipt), preferred);
            assert_eq!(label[arm::WON.canonical], json!(graded.won), "{receipt:?} {preferred:?}");
            assert_eq!(AGREED.read(label).and_then(Value::as_bool), graded.agreed, "{receipt:?} {preferred:?}");
            rows.extend(due);
            let labelled = labelled_in(&rows);
            assert_eq!(labelled.len(), 1);
            assert_eq!((labelled[0].won, labelled[0].agreed), (graded.won, graded.agreed));
            assert_eq!((labelled[0].incumbent.as_str(), labelled[0].challenger.as_str()), (INCUMBENT, NEWCOMER));
            let sample = sample_of(&labelled[0]);
            let fed = matches!(
                (receipt, preferred),
                (Receipt::Failed, Preferred::Challenger) | (Receipt::Passed, Preferred::Incumbent)
            );
            assert_eq!(sample.is_some(), fed, "{receipt:?} {preferred:?}");
            if let Some(sample) = sample {
                let won = sample.status == runtime::OUTCOME_COMPLETED;
                assert_eq!(won, preferred == Preferred::Challenger, "a vindicated challenger is a win, a vindicated incumbent a loss");
            }
        }
    }
    let passed_challenger =
        labels_due(&[request_row("agent-2#1", Preferred::Challenger)], |_| Some(receipted(Receipt::Passed)), 7);
    assert!(AGREED.read(&passed_challenger[0]).is_none(), "undecidable: no agreed mark");
    assert_eq!(passed_challenger[0][arm::WON.canonical], json!(false));
}

/* ---- acting ------------------------------------------------------------ */

fn labelled(attempt: &str) -> Labelled {
    Labelled {
        attempt: attempt.to_string(),
        role: "coding".to_string(),
        incumbent: INCUMBENT.to_string(),
        challenger: NEWCOMER.to_string(),
        won: true,
        agreed: Some(true),
    }
}

/// 두 선이 다 서야 움직인다: 자리가 Applying(원장 전이)이고 도전자의 전적이 현직 비율을 넘을 때만 — 한 선만으로는 0.
#[test]
fn a_roles_model_moves_only_when_the_seat_stands_and_the_standing_passes() {
    let rows: Vec<Value> = (0..A_WINDOW_OF_COMPARISONS)
        .map(|n| request_row(&format!("agent-{n}#1"), Preferred::Challenger))
        .collect();
    let won = labelled("agent-0#1");
    let rate = Some(0.5);
    assert!(!may_move(JevMode::Shadow, true, &rows, &won, rate), "shadow records");
    assert!(!may_move(JevMode::Auto, false, &rows, &won, rate), "auto not raised: a Rise this window is not a standing");
    assert!(may_move(JevMode::Auto, true, &rows, &won, rate), "auto raised and the standing passes");
    assert!(may_move(JevMode::On, false, &rows, &won, rate), "a person's on is the seat's word");
    assert!(!may_move(JevMode::On, true, &rows, &won, None), "no incumbent rate: held to nothing, moves nothing");
    let thin: Vec<Value> = rows[..A_WINDOW_OF_COMPARISONS - 1].to_vec();
    assert!(!may_move(JevMode::Auto, true, &thin, &won, rate), "a window short of comparisons passes nothing");
    let losing: Vec<Value> = (0..A_WINDOW_OF_COMPARISONS)
        .map(|n| request_row(&format!("agent-{n}#1"), Preferred::Incumbent))
        .collect();
    assert!(!may_move(JevMode::Auto, true, &losing, &won, Some(0.1)), "a standing under the incumbent's rate moves nothing");
    let sample = sample_of(&won).expect("an agreeing label");
    assert_eq!(sample.selected_model, NEWCOMER);
    assert_eq!(sample.role.as_deref(), Some("coding"));
    assert_eq!(sample.route_source.as_deref(), Some(CHALLENGER_ROUTE_SOURCE));
    assert_eq!(sample.run_id.as_deref(), Some(sample_attempt_key("agent-0#1").as_str()));
    assert_eq!(sample.decision_kind(), DecisionKind::Model);
    assert!(!sample.decision_kind().is_bookkeeping(), "a verified sample is what the learner reads");
    assert!(sample.is_seat_sample(), "and one a seat stands behind or not");
}

fn incumbent_run(n: usize, now_secs: u64) -> RouteOutcomeRecord {
    let status = if n.is_multiple_of(2) { runtime::OUTCOME_COMPLETED } else { runtime::OUTCOME_FAILED };
    let mut record = RouteOutcomeRecord::new("subagent", "general-purpose", INCUMBENT, status)
        .with_role(Some("coding".to_string()))
        .with_route_source(Some("auto".to_string()))
        .with_attempt_key(format!("peer-{n}#1"));
    record.recorded_at = now_secs;
    record
}

/// 학습기는 자리가 세운 도전자 표본만 기존 learned 문 그대로 읽는다: 세운 표본은 표본이고, 세우지 않은 표본과 도전 설계의 세금
/// 행은 아무것도 가르치지 않는다; 현직 비율은 학습기의 바닥을 따르며 표본에 흔들리지 않는다.
#[test]
fn the_learner_reads_the_samples_its_seat_stands_behind_and_never_the_tax() {
    let now = 2_000_000_000;
    let peers: Vec<RouteOutcomeRecord> = (0..8).map(|n| incumbent_run(n, now)).collect();
    let tax: Vec<RouteOutcomeRecord> = (0..8)
        .map(|n| {
            let mut record = RouteOutcomeRecord::route_tax(runtime::RouteTaxCall::Challenger, NEWCOMER, runtime::OUTCOME_COMPLETED)
                .with_attempt_key(format!("agent-{n}#1"));
            record.recorded_at = now;
            record
        })
        .collect();
    let samples = |admitted: bool| -> Vec<RouteOutcomeRecord> {
        (0..4)
            .map(|n| {
                let mut record = sample_of(&labelled(&format!("agent-{n}#1"))).expect("a sample");
                record.recorded_at = now;
                record.admitted = admitted;
                record
            })
            .collect()
    };
    let canonical = super::super::canonicalize_route_model_id;
    let entry = |records: &[RouteOutcomeRecord]| {
        LearnedSpecialtyHint::compute(records, now, canonical).entry_for(RouteRole::Coding, NEWCOMER)
    };
    let with_tax: Vec<RouteOutcomeRecord> = peers.iter().chain(&tax).cloned().collect();
    assert!(entry(&with_tax).is_none(), "tax teaches nothing");
    let unadmitted: Vec<RouteOutcomeRecord> = peers.iter().chain(&tax).chain(&samples(false)).cloned().collect();
    assert!(entry(&unadmitted).is_none(), "a sample no reader admitted teaches nothing");
    let admitted: Vec<RouteOutcomeRecord> = peers.iter().chain(&tax).chain(&samples(true)).cloned().collect();
    let learned = entry(&admitted).expect("four verified samples (weight 2 each) clear the learner's floor");
    assert!(learned.model_adjustment > 0, "wins against a peer at one half: {learned:?}");
    assert_eq!(runtime::learned_rate(&peers, now, RouteRole::Coding, INCUMBENT, canonical), Some(0.5));
    assert_eq!(
        runtime::learned_rate(&admitted, now, RouteRole::Coding, INCUMBENT, canonical),
        Some(0.5),
        "the incumbent's rate is its own runs"
    );
    assert_eq!(
        runtime::learned_rate(&peers[..3], now, RouteRole::Coding, INCUMBENT, canonical),
        None,
        "under the learner's floor: no rate to be held to"
    );
}

/// 라우팅 소비자가 읽는 순간의 자리 자격이 도전자 표본의 영향을 정한다(m-7953): on이면 들고, off면 0, auto는 원장이 세웠을
/// 때만, 떨어지면 0, 다시 세우면 같은 행이 같은 감쇠 그대로 돌아온다; 전적이 현직을 못 넘으면 0; 다른 출처의 학습은 늘 그대로.
#[test]
fn the_arms_samples_teach_the_router_only_while_the_seat_stands_behind_them() {
    let rig = Rig::new(JevMode::On, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.seed_a_standing_window(true);
    for labelled in labelled_in(&rig.rows()) {
        runtime::record_route_outcome(&rig.cwd, &sample_of(&labelled).expect("an agreeing label")).expect("a sample");
    }
    let canonical = super::super::canonicalize_route_model_id;
    let read = || {
        let records = read_learning_outcomes(&rig.cwd).expect("records");
        let now = now_ms() / 1_000;
        let challenger = LearnedSpecialtyHint::compute(&records, now, canonical)
            .entry_for(RouteRole::Coding, NEWCOMER)
            .map(|entry| entry.model_adjustment);
        let others: Vec<RouteOutcomeRecord> = records.into_iter().filter(|record| !record.is_seat_sample()).collect();
        (challenger, others)
    };
    let (acting, others) = read();
    let moved = acting.expect("a person's on: the samples teach");
    assert!(moved > 0, "the challenger won its window: {moved}");

    rig.write_settings(Some(JevMode::Off), DoorWords::OPEN);
    let (off, off_others) = read();
    assert_eq!(off, None, "switched off: the samples stay in the ledger and teach nothing");
    assert_eq!(off_others, others, "and every other source's learning is as it was");

    rig.write_settings(Some(JevMode::Shadow), DoorWords::OPEN);
    assert_eq!(read().0, None, "recording: nothing");
    rig.write_settings(Some(JevMode::Auto), DoorWords::OPEN);
    assert_eq!(read().0, None, "an auto its ledger has not raised: nothing");
    rig.transition(ROSE);
    assert_eq!(read().0, Some(moved), "raised: the same rows, under their own decay");
    rig.transition(FELL);
    assert_eq!(read().0, None, "fallen: nothing");
    rig.transition(ROSE);
    assert_eq!(read().0, Some(moved), "raised again: back, as they were");
}

/// 전적이 현직 비율을 못 넘는 도전자의 표본은 자리가 행동해도 아무것도 가르치지 않는다 — 학습기는 그 도전자를 모른다.
#[test]
fn a_standing_under_the_incumbents_rate_admits_no_sample() {
    let losing = Rig::new(JevMode::On, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    losing.seed_a_standing_window(false);
    let ledger = challenger_path(&losing.cwd);
    for n in 0..A_WINDOW_OF_COMPARISONS {
        let attempt = format!("window-{n}#1");
        let label = arm::label_row(&attempt, &receipted(Receipt::Passed), Preferred::Incumbent, 2);
        append_shadow_row(&ledger, &label, SHADOW_LEDGER_MAX_BYTES).expect("a label");
    }
    for labelled in labelled_in(&losing.rows()) {
        runtime::record_route_outcome(&losing.cwd, &sample_of(&labelled).expect("an agreeing label")).expect("a sample");
    }
    let records = read_learning_outcomes(&losing.cwd).expect("records");
    assert_eq!(records.iter().filter(|record| record.is_seat_sample()).count(), A_WINDOW_OF_COMPARISONS);
    assert!(
        records.iter().filter(|record| record.is_seat_sample()).all(|record| !record.admitted),
        "a standing under the incumbent's rate stands behind no sample"
    );
    let learned = LearnedSpecialtyHint::compute(&records, now_ms() / 1_000, super::super::canonicalize_route_model_id);
    assert!(
        learned.entry_for(RouteRole::Coding, NEWCOMER).is_none(),
        "and the router learns nothing of the challenger from them, for or against"
    );
}

/// 행동하는 자리에서 동의 칸 라벨은 표본 하나가 된다 — 기록만 하던 때 붙은 라벨도, 자리가 이제 세우면; 다시 불러도 하나다.
#[test]
fn an_acting_seat_feeds_one_sample_per_agreeing_label_and_a_recording_one_feeds_none() {
    let rig = Rig::new(JevMode::On, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.seed_a_standing_window(false);
    let now = now_ms() / 1_000;
    for attempt in ["window-0#1", "window-1#1"] {
        runtime::record_route_outcome(&rig.cwd, &handed_in(attempt, Some(HANDED_IN))).expect("a run");
    }
    runtime::record_route_outcome(&rig.cwd, &verdict("window-0#1", runtime::OUTCOME_FAILED, VerdictSubject::Work, now)).expect("a verdict");
    assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::Shadow), &feed_into(&rig.cwd)), 1, "recording labels");
    assert_eq!(rig.samples(), 0, "and feeds nothing");
    runtime::record_route_outcome(&rig.cwd, &verdict("window-1#1", runtime::OUTCOME_FAILED, VerdictSubject::Work, now)).expect("a verdict");
    assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)), 1);
    assert_eq!(rig.samples(), 2, "an acting seat feeds every agreeing label it stands behind — the one labelled while recording too");
    assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)), 0);
    assert_eq!(rig.samples(), 2, "once");
}

/// 라벨이 먼저 남고 표본은 따라온다: 표본 쓰기가 거절되거나 라벨을 쓴 직후 멈췄어도, 다음 부름(재시작 뒤 포함)이 그 표본을 정확히
/// 한 번 쓰고, 이미 쓴 표본은 몇이 동시에 불러도 다시 쓰지 않는다.
#[test]
fn a_label_whose_sample_was_lost_gets_it_from_the_next_call_once() {
    let rig = Rig::new(JevMode::On, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.seed_a_standing_window(false);
    let now = now_ms() / 1_000;
    runtime::record_route_outcome(&rig.cwd, &handed_in("window-0#1", Some(HANDED_IN))).expect("a run");
    runtime::record_route_outcome(&rig.cwd, &verdict("window-0#1", runtime::OUTCOME_FAILED, VerdictSubject::Work, now)).expect("a verdict");
    let refused = |_: &RouteOutcomeRecord| Err(std::io::Error::other("the outcome ledger refused the write"));
    assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &refused), 1, "the label is the durable fact");
    assert_eq!(rig.samples(), 0, "its sample was refused — or the process stopped right after the label");
    assert!(
        rig.rows().iter().any(|row| word(row, &LABEL).as_deref() == Some("window-0#1")),
        "the label stands"
    );
    // A new call is a restart: everything it reads is on disk.
    assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)), 0, "no second label");
    assert_eq!(rig.samples(), 1, "the lost sample is written now");
    let racers: Vec<_> = (0..4)
        .map(|_| {
            let cwd = rig.cwd.clone();
            std::thread::spawn(move || note_verdicts_in(&cwd, Some(JevMode::On), &feed_into(&cwd)))
        })
        .collect();
    for racer in racers {
        racer.join().expect("a racer");
    }
    assert_eq!(rig.samples(), 1, "and never twice");
}

/* ---- a label's source ---------------------------------------------------- */

/// A label as the arm wrote it before labels named the source their receipt
/// judged: every word of today's but that one.
fn unsourced_label(attempt: &str, receipt: Receipt, preferred: Preferred, at: i64) -> Value {
    let mut label = arm::label_row(attempt, &receipted(receipt), preferred, at);
    label.as_object_mut().expect("a row").remove(arm::VERIFIED_SOURCE.canonical);
    label
}

/// The samples of `cwd`'s route-outcome ledger the router may learn from
/// now, as the router's own reader marks them.
fn admitted(cwd: &Path) -> Vec<String> {
    read_learning_outcomes(cwd)
        .expect("records")
        .into_iter()
        .filter(|record| record.is_seat_sample() && record.admitted)
        .filter_map(|record| record.run_id)
        .collect()
}

/// source를 적지 않은 라벨(옛 형식)은 판정할 수 없다(t-6263 R4c): 행동하는 자리·통과하는 전적·옛 라벨이 써 둔 표본이 다 있어도
/// 그 표본은 하나도 인정되지 않고, 새 표본도 쓰이지 않으며, 라우터는 도전자를 배우지 않는다 — 옛 행은 그대로 남는다. 그 시도가
/// 넘긴 source에 묶인 영수증이 기록에 있으면 새 라벨이 옛 행 옆에 보태지고, 그 시도만 다시 표본이 된다.
#[test]
fn a_label_that_names_no_source_teaches_nothing_until_a_receipt_on_its_source_supplements_it() {
    let rig = Rig::new(JevMode::On, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.seed_a_standing_window(false);
    let ledger = challenger_path(&rig.cwd);
    for n in 0..A_WINDOW_OF_COMPARISONS {
        let attempt = format!("window-{n}#1");
        let label = unsourced_label(&attempt, Receipt::Failed, Preferred::Challenger, 1);
        append_shadow_row(&ledger, &label, SHADOW_LEDGER_MAX_BYTES).expect("an old label");
        let sample = sample_of(&Labelled { attempt, ..labelled("") }).expect("an agreeing label");
        runtime::record_route_outcome(&rig.cwd, &sample).expect("the sample it was given");
    }
    let old = rig.rows();
    assert_eq!(rig.samples(), A_WINDOW_OF_COMPARISONS);
    assert!(admitted(&rig.cwd).is_empty(), "a label that names no source is not evaluable: none of its samples is admitted");
    assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)), 0, "no receipt on record: nothing to label");
    assert_eq!(rig.samples(), A_WINDOW_OF_COMPARISONS, "and no sample is written from an old label");
    let learned = LearnedSpecialtyHint::compute(
        &read_learning_outcomes(&rig.cwd).expect("records"),
        now_ms() / 1_000,
        super::super::canonicalize_route_model_id,
    );
    assert!(learned.entry_for(RouteRole::Coding, NEWCOMER).is_none(), "the router learns nothing of the challenger from them");

    // A receipt on the source the attempt handed in, on record now.
    let now = now_ms() / 1_000;
    runtime::record_route_outcome(&rig.cwd, &handed_in("window-0#1", Some(HANDED_IN))).expect("a run");
    runtime::record_route_outcome(&rig.cwd, &verdict("window-0#1", runtime::OUTCOME_FAILED, VerdictSubject::Work, now))
        .expect("a verdict");
    assert_eq!(
        note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)),
        1,
        "a label on the source it judged supplements the old one"
    );
    let rows = rig.rows();
    assert_eq!(&rows[..old.len()], old.as_slice(), "every old row stands as it was written");
    assert_eq!(word(&rows[old.len()], &LABEL).as_deref(), Some("window-0#1"));
    assert_eq!(word(&rows[old.len()], &arm::VERIFIED_SOURCE).as_deref(), Some(HANDED_IN));
    assert_eq!(admitted(&rig.cwd), [sample_attempt_key("window-0#1")], "that attempt, and only it, is a sample again");
    assert_eq!(rig.samples(), A_WINDOW_OF_COMPARISONS, "its sample was there already: none is written twice");
}

/// 라벨이 적은 source를 기록이 반박하면 그 라벨은 판정할 수 없다(t-6263 R4c): 시도의 실행 행이 다른 source를 넘겼거나, 넘긴
/// source의 첫 영수증이 라벨과 다른 말을 하거나, 영수증이 아예 없으면 그 시도의 표본은 인정되지 않는다 — 기록이 옳은 영수증을
/// 들고 있으면 그것이 새 라벨이 된다. 시도가 보존 한도로 기록에서 빠진 뒤에는 라벨이 쓰일 때 검사받은 결합의 증거로 선다.
#[test]
fn a_label_whose_source_the_record_contradicts_is_no_label() {
    let rig = Rig::new(JevMode::On, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.seed_a_standing_window(true);
    for labelled in labelled_in(&rig.rows()) {
        runtime::record_route_outcome(&rig.cwd, &sample_of(&labelled).expect("an agreeing label")).expect("a sample");
    }
    assert_eq!(
        admitted(&rig.cwd).len(),
        A_WINDOW_OF_COMPARISONS,
        "attempts no longer on record: each label stands on the source it was bound to when it was written"
    );
    let now = now_ms() / 1_000;
    let record = |row: RouteOutcomeRecord| runtime::record_route_outcome(&rig.cwd, &row).expect("a row");
    // Its run handed in another source.
    record(handed_in("window-0#1", Some("tree-another")));
    // The first receipt on its source passed the work; the label says it failed.
    record(handed_in("window-1#1", Some(HANDED_IN)));
    record(verdict("window-1#1", runtime::OUTCOME_COMPLETED, VerdictSubject::Work, now));
    // It handed in the label's source, and no verdict on it is on record.
    record(handed_in("window-2#1", Some(HANDED_IN)));
    // The record says what the label says.
    record(handed_in("window-3#1", Some(HANDED_IN)));
    record(verdict("window-3#1", runtime::OUTCOME_FAILED, VerdictSubject::Work, now));
    let now_admitted = admitted(&rig.cwd);
    for contradicted in ["window-0#1", "window-1#1", "window-2#1"] {
        assert!(!now_admitted.contains(&sample_attempt_key(contradicted)), "{contradicted}: the record contradicts its label");
    }
    assert!(now_admitted.contains(&sample_attempt_key("window-3#1")), "the record confirms this one");
    assert_eq!(now_admitted.len(), A_WINDOW_OF_COMPARISONS - 3);

    let before = rig.rows().len();
    assert_eq!(
        note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)),
        1,
        "the record's own receipt labels window-1 anew, beside the label it contradicts"
    );
    let rows = rig.rows();
    assert_eq!(word(&rows[before], &LABEL).as_deref(), Some("window-1#1"));
    assert_eq!(word(&rows[before], &arm::VERIFIED).as_deref(), Some(Receipt::Passed.token()));
    assert!(
        !admitted(&rig.cwd).contains(&sample_attempt_key("window-1#1")),
        "a passing incumbent says nothing of a challenger the judge preferred: the old sample stays unadmitted"
    );
    assert_eq!(rig.samples(), A_WINDOW_OF_COMPARISONS, "and nothing new is sampled");
}

/// 라벨이 기록에 서는 방식은 한 곳에서 읽는다(t-6263 R4c): source를 적지 않은 라벨은 「source 없음」이고, 기록이 그 시도의
/// 실행 행을 들고 있으면 넘긴 source의 첫 영수증과 같을 때만 서며 아니면 반박된다; 기록에서 빠진 시도의 라벨은 쓰일 때 검사받은
/// 결합의 증거(영수증 verdict의 기록 시각)를 들고 있을 때만 서고, 없으면 증명되지 않은 라벨이다(R4c-1). 독자가 읽는 원장은
/// 서는 라벨만 남긴다 — 파일은 그대로다.
#[test]
fn a_labels_binding_is_read_against_the_record_in_one_place() {
    let records = vec![
        handed_in("ran#1", Some(HANDED_IN)),
        verdict("ran#1", runtime::OUTCOME_FAILED, VerdictSubject::Work, 5),
        handed_in("ran-elsewhere#1", Some("tree-another")),
        handed_in("ran-unverified#1", Some(HANDED_IN)),
    ];
    let on_record = OnRecord::of(&records);
    let label = |attempt: &str, receipt: Receipt| arm::label_row(attempt, &receipted(receipt), Preferred::Challenger, 1);
    let unsourced = unsourced_label("ran#1", Receipt::Failed, Preferred::Challenger, 1);
    assert_eq!(binding(&unsourced, "ran#1", &on_record), Binding::Unsourced);
    assert_eq!(binding(&label("ran#1", Receipt::Failed), "ran#1", &on_record), Binding::Bound);
    assert_eq!(binding(&label("ran#1", Receipt::Passed), "ran#1", &on_record), Binding::Contradicted, "another receipt");
    for attempt in ["ran-elsewhere#1", "ran-unverified#1"] {
        assert_eq!(binding(&label(attempt, Receipt::Failed), attempt, &on_record), Binding::Contradicted, "{attempt}");
    }
    assert_eq!(
        binding(&label("aged-out#1", Receipt::Failed), "aged-out#1", &on_record),
        Binding::Bound,
        "no longer on record: the binding it was written on, which it keeps, stands"
    );
    assert_eq!(
        binding(&unproven_label("aged-out#1", Receipt::Failed, Preferred::Challenger, 1), "aged-out#1", &on_record),
        Binding::Unproven,
        "and one that keeps no evidence of it proves nothing"
    );
    let mut rows = vec![request_row("ran#1", Preferred::Challenger), unsourced, label("ran#1", Receipt::Failed), label("ran-elsewhere#1", Receipt::Failed)];
    keep_bound_labels(&mut rows, &on_record);
    assert_eq!(rows.len(), 2, "the comparison and the one label that stands: {rows:?}");
    assert_eq!(word(&rows[1], &arm::VERIFIED_SOURCE).as_deref(), Some(HANDED_IN));
}

/// A label as the arm wrote it before labels kept the evidence of the
/// binding they were written on: every word of today's but that one.
fn unproven_label(attempt: &str, receipt: Receipt, preferred: Preferred, at: i64) -> Value {
    let mut label = arm::label_row(attempt, &receipted(receipt), preferred, at);
    label.as_object_mut().expect("a row").remove(arm::VERIFIED_AT.canonical);
    label
}

/// 기록이 시도의 실행 행을 잊었다는 것은 결합의 증명이 아니다(t-6263 R4c-1): 실행 행이 빠진 기록에서 라벨은 제가 쓰일 때
/// 묶인 증거(영수증 verdict의 기록 시각)를 들고 있고, 기록에 남은 그 source의 verdict 어느 것도 다른 말을 하지 않을 때만
/// 선다 — 먼저 통과한 verdict든 나중 것이든(실행 행 없이는 어느 것이 첫 verdict였는지 기록이 말하지 못한다). 증거가 없는
/// 라벨은 실행 행이 빠지면 증명되지 않은 라벨이다. 다른 source의 verdict는 이 라벨에 대해 아무 말도 하지 않는다.
#[test]
fn a_label_off_the_record_never_stands_against_a_verdict_still_on_it() {
    let attempt = "aged-out#1";
    let label = arm::label_row(attempt, &receipted(Receipt::Failed), Preferred::Challenger, 1);
    let unproven = unproven_label(attempt, Receipt::Failed, Preferred::Challenger, 1);
    let on = |records: &[RouteOutcomeRecord], label: &Value| binding(label, attempt, &OnRecord::of(records));
    let seen = |status: &str, at: u64, source: &str| verdict_on(attempt, status, VerdictSubject::Work, at, Some(source));
    let passed_first = seen(runtime::OUTCOME_COMPLETED, VERDICT_AT - 1, HANDED_IN);
    let passed_later = seen(runtime::OUTCOME_COMPLETED, VERDICT_AT + 9, HANDED_IN);
    let its_own = seen(runtime::OUTCOME_FAILED, VERDICT_AT, HANDED_IN);
    let elsewhere = seen(runtime::OUTCOME_COMPLETED, VERDICT_AT - 1, "tree-another");

    assert_eq!(on(std::slice::from_ref(&passed_first), &label), Binding::Contradicted, "a verdict before it says the work passed");
    assert_eq!(on(std::slice::from_ref(&passed_later), &label), Binding::Contradicted, "and one after it: the record cannot say which was first");
    assert_eq!(on(&[its_own.clone(), passed_later.clone()], &label), Binding::Contradicted);
    assert_eq!(on(&[], &label), Binding::Bound, "the record has forgotten the attempt: the label stands on its evidence");
    assert_eq!(on(std::slice::from_ref(&its_own), &label), Binding::Bound, "what is left of the record says what it says");
    assert_eq!(on(std::slice::from_ref(&elsewhere), &label), Binding::Bound, "a verdict on other work says nothing of it");

    assert_eq!(on(&[], &unproven), Binding::Unproven, "a run gone from the record proves nothing");
    assert_eq!(on(std::slice::from_ref(&its_own), &unproven), Binding::Unproven);
    assert_eq!(on(std::slice::from_ref(&passed_first), &unproven), Binding::Contradicted);
    let ran = [handed_in(attempt, Some(HANDED_IN)), its_own];
    assert_eq!(on(&ran, &unproven), Binding::Bound, "the record still binds it itself");
}

/// The route-outcome ledger with the incumbent's own runs appended until it
/// holds no row about any of `attempts`: the bucket keeps its newest rows
/// only, so what the attempts' runs and verdicts said ages out, as it does
/// on a machine that goes on working.
fn age_out(cwd: &Path, attempts: &[&str]) {
    age_out_rows(cwd, |record| record.run_id.as_deref().is_some_and(|run| attempts.contains(&run)));
}

/// The route-outcome ledger with the incumbent's own runs appended until it
/// holds no row `gone` picks — the oldest rows of the bucket leave first.
fn age_out_rows(cwd: &Path, gone: impl Fn(&RouteOutcomeRecord) -> bool) {
    let now = now_ms() / 1_000;
    for n in 0.. {
        let records = runtime::read_route_outcomes(cwd).expect("records");
        if !records.iter().any(&gone) {
            return;
        }
        assert!(n < 10_000, "the ledger never let go of the rows");
        runtime::record_route_outcome(cwd, &incumbent_run(n, now)).expect("a later run");
    }
}

/// The attempts [`contradicted_window`] contradicts.
const CONTRADICTED: [&str; 3] = ["window-0#1", "window-1#1", "window-2#1"];

/// A standing window laid down in `rig`'s ledgers — every comparison
/// labelled by a failing receipt the judge agreed with, each label keeping
/// the evidence it was bound on but the [`CONTRADICTED`] ones' where
/// `kept_its_evidence` is false, and each followed by its sample — and the
/// record contradicting the three: a run that handed in another source, a
/// first receipt that passed the work, a run no verdict followed. The labels
/// of `held_back` are not appended but answered, in the window's order, for
/// the test to append when it will.
fn contradicted_window(rig: &Rig, kept_its_evidence: bool, held_back: &[&str]) -> Vec<Value> {
    rig.seed_a_standing_window(false);
    let ledger = challenger_path(&rig.cwd);
    let mut held = Vec::new();
    for n in 0..A_WINDOW_OF_COMPARISONS {
        let attempt = format!("window-{n}#1");
        let label = if kept_its_evidence || !CONTRADICTED.contains(&attempt.as_str()) {
            arm::label_row(&attempt, &receipted(Receipt::Failed), Preferred::Challenger, 1)
        } else {
            unproven_label(&attempt, Receipt::Failed, Preferred::Challenger, 1)
        };
        if held_back.contains(&attempt.as_str()) {
            held.push(label);
        } else {
            append_shadow_row(&ledger, &label, SHADOW_LEDGER_MAX_BYTES).expect("a label");
        }
        let sample = sample_of(&Labelled { attempt, ..labelled("") }).expect("an agreeing label");
        runtime::record_route_outcome(&rig.cwd, &sample).expect("the sample it was given");
    }
    let record = |row: RouteOutcomeRecord| runtime::record_route_outcome(&rig.cwd, &row).expect("a row");
    record(handed_in("window-0#1", Some("tree-another")));
    record(handed_in("window-1#1", Some(HANDED_IN)));
    record(verdict("window-1#1", runtime::OUTCOME_COMPLETED, VerdictSubject::Work, VERDICT_AT - 1));
    record(handed_in("window-2#1", Some(HANDED_IN)));
    held
}

/// That the router's reader admits no sample of a [`CONTRADICTED`] label,
/// and every other one of the window's.
fn assert_the_contradicted_unadmitted(rig: &Rig, when: &str) {
    let admitted = admitted(&rig.cwd);
    for attempt in CONTRADICTED {
        assert!(!admitted.contains(&sample_attempt_key(attempt)), "{when}: {attempt}'s label was contradicted");
    }
    assert_eq!(admitted.len(), A_WINDOW_OF_COMPARISONS - CONTRADICTED.len(), "{when}: every other label stands");
}

/// The attempts the strikes in `rig`'s ledger name, in the order written.
fn struck_on_file(rig: &Rig) -> Vec<String> {
    rig.rows().iter().filter_map(|row| word(row, &arm::STRUCK)).collect()
}

thread_local! {
    /// How this thread's next strike appends are refused
    /// ([`refused_strike`]): with an error of this kind, or — `None` — with
    /// an `Ok` that wrote nothing, as a write the shadow ledger declines
    /// answers ([`append_shadow_row`]).
    static REFUSED_STRIKES: std::cell::RefCell<std::collections::VecDeque<Option<std::io::ErrorKind>>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

/// The refusal queued for this thread's next strike append (`append_strike`).
pub(super) fn refused_strike() -> Option<std::io::Result<()>> {
    REFUSED_STRIKES
        .with(|queued| queued.borrow_mut().pop_front())
        .map(|refusal| refusal.map_or(Ok(()), |kind| Err(kind.into())))
}

/// Refuse this thread's next `times` strike appends as `refusal` says.
fn refuse_strikes(times: usize, refusal: Option<std::io::ErrorKind>) {
    REFUSED_STRIKES.with(|queued| queued.borrow_mut().extend(std::iter::repeat_n(refusal, times)));
}

/// Whether every refusal this thread queued was spent.
fn every_refusal_spent() -> bool {
    REFUSED_STRIKES.with(|queued| queued.borrow().is_empty())
}

thread_local! {
    /// How this thread's next readings of a challenger ledger fail
    /// ([`failed_reading`]).
    static FAILED_READINGS: std::cell::RefCell<std::collections::VecDeque<std::io::ErrorKind>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
    /// Where this thread's next reading of a challenger ledger stops once it
    /// has read ([`reading_taken`]): it says so on the first, and goes on
    /// when the second says to.
    static HELD_READING: std::cell::RefCell<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>> =
        const { std::cell::RefCell::new(None) };
}

/// The failure queued for this thread's next reading of a challenger ledger
/// (`Reading::of`).
pub(super) fn failed_reading() -> Option<std::io::Error> {
    FAILED_READINGS.with(|queued| queued.borrow_mut().pop_front()).map(std::io::Error::from)
}

/// Fail this thread's next `times` readings of a challenger ledger with
/// `kind`.
fn fail_readings(times: usize, kind: std::io::ErrorKind) {
    FAILED_READINGS.with(|queued| queued.borrow_mut().extend(std::iter::repeat_n(kind, times)));
}

/// Whether every failure this thread queued was spent.
fn every_failure_spent() -> bool {
    FAILED_READINGS.with(|queued| queued.borrow().is_empty())
}

/// A reading of a challenger ledger has read it, and neither ended nor
/// settled a thing on it (`Reading::of`): if this thread asked to be held
/// there, say so and wait to be let go.
pub(super) fn reading_taken() {
    if let Some((reached, go)) = HELD_READING.with(std::cell::RefCell::take) {
        reached.send(()).expect("the test waits for the reading");
        go.recv_timeout(Duration::from_secs(60)).expect("the test lets the reading go");
    }
}

/// Hold this thread's next reading of a challenger ledger once it has read
/// ([`reading_taken`]).
fn hold_the_next_reading(reached: mpsc::Sender<()>, go: mpsc::Receiver<()>) {
    HELD_READING.with(|held| *held.borrow_mut() = Some((reached, go)));
}

/// How many strikes this process holds for `ledger` that it has not seen
/// back yet (`UNSEEN_STRIKES`).
fn held_strikes(ledger: &Path) -> usize {
    UNSEEN_STRIKES
        .get()
        .and_then(|book| book.lock().ok().and_then(|book| book.get(ledger).map(std::collections::BTreeMap::len)))
        .unwrap_or_default()
}

/// How many trees a [`SourceWatch`] has written, by the root it watched —
/// what a test holds an edit on until the watch's tree is written, since
/// the tree is written on a thread of its own while the verifier works and
/// the order of the two is not promised.
static WATCHED_TREES: (Mutex<std::collections::BTreeMap<PathBuf, usize>>, std::sync::Condvar) =
    (Mutex::new(std::collections::BTreeMap::new()), std::sync::Condvar::new());

/// A watch over `root` has written its tree.
pub(super) fn tree_written(root: &Path) {
    let (written, changed) = &WATCHED_TREES;
    *written.lock().unwrap_or_else(std::sync::PoisonError::into_inner).entry(root.to_path_buf()).or_default() += 1;
    changed.notify_all();
}

/// How many trees watches over `root` have written.
pub(crate) fn trees_written(root: &Path) -> usize {
    WATCHED_TREES.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(root).copied().unwrap_or_default()
}

/// Wait until watches over `root` have written `count` trees.
pub(crate) fn wait_for_trees_written(root: &Path, count: usize) {
    let (written, changed) = &WATCHED_TREES;
    let started = Instant::now();
    let mut seen = written.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    while seen.get(root).copied().unwrap_or_default() < count {
        assert!(started.elapsed() < Duration::from_secs(60), "no watch over {} wrote its tree", root.display());
        seen = changed
            .wait_timeout(seen, Duration::from_millis(100))
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
    }
}

/// The three ways a strike can fail to land: two errors, and an `Ok` that
/// wrote nothing.
const STRIKE_REFUSALS: [Option<std::io::ErrorKind>; 3] =
    [Some(std::io::ErrorKind::PermissionDenied), Some(std::io::ErrorKind::WriteZero), None];

/// 기록이 반박한 라벨은 기록이 그 시도를 잊은 뒤에도 서지 않는다(t-6263 R4c-1): 다른 source를 넘긴 실행·먼저 통과한
/// verdict·영수증 없는 실행이 라벨을 반박한 뒤, 실행 행과 verdict가 보존 한도로 기록에서 빠져도 그 라벨의 표본은 인정되지
/// 않고 새 표본도 없다 — 반박되지 않은 라벨은 그대로 선다. 두 원장 모두: 증거를 적지 않은 옛 라벨(ddb7a8b3의 기록기가 쓴
/// 모양)은 반박을 읽은 독자가 없었어도 기록이 잊으면 증명되지 않은 라벨이고, 증거를 적은 라벨은 반박을 읽은 독자(학습기의
/// 읽기)가 그 자리에서 그어 둔다 — 옛 행은 바이트 그대로다. 잊힌 뒤 그 source에 적법한 영수증이 다시 오면 그 시도만 다시
/// 표본이 된다: 기록이 옛 라벨을 스스로 묶거나, 그어진 라벨 옆에 새 라벨이 보태진다.
#[test]
fn a_label_the_record_contradicted_stays_unadmitted_once_the_record_forgets_the_run() {
    for kept_its_evidence in [false, true] {
        let rig = Rig::new(JevMode::On, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
        contradicted_window(&rig, kept_its_evidence, &[]);
        let now = now_ms() / 1_000;
        let record = |row: RouteOutcomeRecord| runtime::record_route_outcome(&rig.cwd, &row).expect("a row");
        let written = rig.rows();
        let look = |when: &str| assert_the_contradicted_unadmitted(&rig, &format!("{kept_its_evidence} {when}"));
        if kept_its_evidence {
            // A reader sees the contradiction while the record still says it.
            look("on record");
        }
        age_out(&rig.cwd, &CONTRADICTED);
        look("aged out");
        assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)), 0, "no receipt: nothing to label");
        look("after the writer");
        assert_eq!(rig.samples(), A_WINDOW_OF_COMPARISONS, "{kept_its_evidence}: and no sample is written");
        let rows = rig.rows();
        assert_eq!(&rows[..written.len()], written.as_slice(), "{kept_its_evidence}: every row stands as it was written");
        let owed: &[&str] = if kept_its_evidence { &CONTRADICTED } else { &[] };
        assert_eq!(struck_on_file(&rig), owed, "each contradiction a reader saw written down once, beside its label");

        // A lawful receipt on the source handed in, on record again.
        record(handed_in("window-2#1", Some(HANDED_IN)));
        record(verdict("window-2#1", runtime::OUTCOME_FAILED, VerdictSubject::Work, now));
        assert_eq!(
            note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)),
            usize::from(kept_its_evidence),
            "{kept_its_evidence}: the record binds the old label itself, or a label joins the struck one"
        );
        assert!(admitted(&rig.cwd).contains(&sample_attempt_key("window-2#1")), "{kept_its_evidence}: that attempt is a sample again");
        assert_eq!(rig.samples(), A_WINDOW_OF_COMPARISONS, "{kept_its_evidence}: its sample was there already");
    }
}

/// 라벨러가 기록에 묶어 쓴 라벨은 기록이 그 시도를 잊은 뒤에도 제 증거로 선다(t-6263 R4c-1 양성): 실행 행과 영수증이
/// 보존 한도로 빠져도 그 표본은 그대로 인정되고, 새 라벨·새 표본은 없다.
#[test]
fn a_label_the_labeller_bound_stands_on_its_evidence_once_the_record_forgets_the_run() {
    let rig = Rig::new(JevMode::On, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.seed_a_standing_window(false);
    let ledger = challenger_path(&rig.cwd);
    for n in 1..A_WINDOW_OF_COMPARISONS {
        let label = arm::label_row(&format!("window-{n}#1"), &receipted(Receipt::Failed), Preferred::Challenger, 1);
        append_shadow_row(&ledger, &label, SHADOW_LEDGER_MAX_BYTES).expect("a label");
    }
    let now = now_ms() / 1_000;
    runtime::record_route_outcome(&rig.cwd, &handed_in("window-0#1", Some(HANDED_IN))).expect("a run");
    runtime::record_route_outcome(&rig.cwd, &verdict("window-0#1", runtime::OUTCOME_FAILED, VerdictSubject::Work, now))
        .expect("a verdict");
    assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)), 1, "the labeller binds it");
    let label = rig.rows().into_iter().rev().find(|row| word(row, &LABEL).as_deref() == Some("window-0#1")).expect("its label");
    assert_eq!(arm::VERIFIED_AT.read(&label).and_then(Value::as_u64), Some(now), "with when its receipt was recorded");
    let sample = sample_attempt_key("window-0#1");
    assert!(admitted(&rig.cwd).contains(&sample), "its sample is admitted");
    assert_eq!(rig.samples(), A_WINDOW_OF_COMPARISONS, "an acting seat fed one sample for each agreeing label");

    age_out(&rig.cwd, &["window-0#1"]);
    let before = rig.rows().len();
    assert_eq!(admitted(&rig.cwd).len(), A_WINDOW_OF_COMPARISONS, "the record forgot the attempt: the label stands on its evidence");
    assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)), 0, "and nothing is labelled again");
    assert_eq!(rig.rows().len(), before, "nor struck");
    assert!(rig.sampled("window-0#1"));
    assert_eq!(rig.samples(), A_WINDOW_OF_COMPARISONS, "and no sample written twice");
}

/// 독자가 읽은 반박은 줄긋기 저장이 실패해도 사라지지 않는다(t-6263 R4c-2): 기록이 라벨을 반박하는 동안 학습기의 읽기가
/// 그 반박을 봤는데 줄긋기 append가 거절되면(권한 없음·쓰인 바이트 0, 또는 shadow 원장이 사양하는 쓰기처럼 `Ok`인데
/// 아무것도 안 쓰임), 그 반박은 원장이 되돌려 보여 줄 때까지 붙잡힌다 — 그 사이 실행 행과 verdict가 보존 한도로 기록에서
/// 빠져도 라벨의 표본은 인정되지 않고 새 표본도 없으며, 쓰기가 돌아오면 그 줄긋기가 원 행 옆에 한 번 적힌다. 옛 행은
/// 바이트 그대로다.
#[test]
fn a_contradiction_a_reader_saw_is_held_until_its_strike_lands() {
    for refusal in STRIKE_REFUSALS {
        let rig = Rig::new(JevMode::On, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
        contradicted_window(&rig, true, &[]);
        let written = rig.rows();
        refuse_strikes(CONTRADICTED.len(), refusal);
        assert_the_contradicted_unadmitted(&rig, &format!("{refusal:?} on record"));
        assert!(every_refusal_spent(), "{refusal:?}: the reader tried to write every strike it owed");
        assert_eq!(rig.rows(), written, "{refusal:?}: and none landed");

        age_out(&rig.cwd, &CONTRADICTED);
        assert_the_contradicted_unadmitted(&rig, &format!("{refusal:?} aged out"));
        assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)), 0, "no receipt: nothing to label");
        assert_the_contradicted_unadmitted(&rig, &format!("{refusal:?} after the writer"));
        assert_eq!(rig.samples(), A_WINDOW_OF_COMPARISONS, "{refusal:?}: and no sample is written");
        let rows = rig.rows();
        assert_eq!(&rows[..written.len()], written.as_slice(), "{refusal:?}: every row stands as it was written");
        assert_eq!(struck_on_file(&rig), CONTRADICTED, "{refusal:?}: each held strike written down once, beside its label");
    }
}

/// How a reading of the challenger's ledger fails in these tests: a ledger
/// nobody may read, and one that is not text.
const READING_FAILURES: [std::io::ErrorKind; 2] =
    [std::io::ErrorKind::PermissionDenied, std::io::ErrorKind::InvalidData];

/// 읽지 못한 원장은 빈 원장이 아니다(t-6263 R4c-3 A): 기록이 라벨을 반박하는 동안 독자가 그 반박을 봤고 줄긋기 저장이
/// 거절돼(세 변형) 반박이 붙잡혀 있을 때, 그다음 원장 읽기가 실패하면(권한 없음·텍스트 아님) 학습기는 어떤 표본도
/// 인정하지 않고 기록기는 아무것도 적지 않으며, 붙잡힌 반박은 하나도 풀리지 않는다 — 읽기가 돌아오고 실행 행과 verdict가
/// 보존 한도로 빠진 뒤에도 라벨의 표본은 인정되지 않고 새 표본도 없으며, 줄긋기는 원 행 옆에 한 번 적히고, 원장이 그것을
/// 되돌려 보여 준 뒤에 장부를 떠난다.
#[test]
fn a_held_strike_outlives_a_reading_that_failed() {
    for failure in READING_FAILURES {
        for refusal in STRIKE_REFUSALS {
            let case = format!("{failure:?} after {refusal:?}");
            let rig = Rig::new(JevMode::On, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
            contradicted_window(&rig, true, &[]);
            let ledger = challenger_path(&rig.cwd);
            let written = rig.rows();
            refuse_strikes(CONTRADICTED.len(), refusal);
            assert_the_contradicted_unadmitted(&rig, &format!("{case} on record"));
            assert!(every_refusal_spent(), "{case}: the reader tried to write every strike it owed");
            assert_eq!(held_strikes(&ledger), CONTRADICTED.len(), "{case}: and holds every one");

            fail_readings(1, failure);
            assert!(admitted(&rig.cwd).is_empty(), "{case}: a ledger that cannot be read stands behind no sample");
            fail_readings(1, failure);
            assert_eq!(
                note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)),
                0,
                "{case}: and labels nothing"
            );
            assert!(every_failure_spent(), "{case}: both readers met the failure");
            assert_eq!(rig.rows(), written, "{case}: nothing was written on it");

            age_out(&rig.cwd, &CONTRADICTED);
            assert_the_contradicted_unadmitted(&rig, &format!("{case} aged out"));
            assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)), 0, "{case}: nothing to label");
            assert_the_contradicted_unadmitted(&rig, &format!("{case} after the writer"));
            assert_eq!(rig.samples(), A_WINDOW_OF_COMPARISONS, "{case}: and no sample is written");
            let rows = rig.rows();
            assert_eq!(&rows[..written.len()], written.as_slice(), "{case}: every row stands as it was written");
            assert_eq!(struck_on_file(&rig), CONTRADICTED, "{case}: each held strike written down once, beside its label");
            assert_eq!(held_strikes(&ledger), 0, "{case}: and let go once the ledger showed it back");
        }
    }
}

/// 붙잡힌 반박보다 오래된 읽기는 그것을 풀지 못한다(t-6263 R4c-3 B): 한 독자(학습기)가 반박될 라벨들이 아직 없는 원장을
/// 읽고 장부에 닿기 전에 멈춘 사이 라벨들이 적히고, 다른 독자가 기록의 반박을 읽어 줄긋기를 붙잡는데 그 저장이
/// 거절되면(세 변형) — 먼저 읽은 독자가 다시 가서 제 옛 읽기에 그 라벨이 없다는 이유로 붙잡힌 반박을 풀면 안 된다. 실행
/// 행과 verdict가 보존 한도로 빠진 뒤에도 표본은 인정되지 않고 새 표본도 없으며, 줄긋기는 원 행 옆에 한 번 적힌다.
#[test]
fn a_reading_older_than_a_held_strike_never_lets_it_go() {
    for refusal in STRIKE_REFUSALS {
        let rig = Rig::new(JevMode::On, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
        let late = contradicted_window(&rig, true, &CONTRADICTED);
        let ledger = challenger_path(&rig.cwd);
        let (reached, read) = mpsc::channel();
        let (let_go, go) = mpsc::channel();
        let cwd = rig.cwd.clone();
        let older = std::thread::spawn(move || {
            hold_the_next_reading(reached, go);
            admitted(&cwd)
        });
        read.recv_timeout(Duration::from_secs(60)).expect("the older reading has read the ledger");

        for label in &late {
            append_shadow_row(&ledger, label, SHADOW_LEDGER_MAX_BYTES).expect("a label");
        }
        let written = rig.rows();
        refuse_strikes(CONTRADICTED.len(), refusal);
        assert_the_contradicted_unadmitted(&rig, &format!("{refusal:?} on record"));
        assert!(every_refusal_spent(), "{refusal:?}: the reader tried to write every strike it owed");
        assert_eq!(rig.rows(), written, "{refusal:?}: and none landed");
        assert_eq!(held_strikes(&ledger), CONTRADICTED.len(), "{refusal:?}: it holds every one");

        let_go.send(()).expect("the older reading waits");
        let saw = older.join().expect("the older reading");
        for attempt in CONTRADICTED {
            assert!(!saw.contains(&sample_attempt_key(attempt)), "{refusal:?}: the older reading held no label of {attempt}");
        }

        age_out(&rig.cwd, &CONTRADICTED);
        assert_the_contradicted_unadmitted(&rig, &format!("{refusal:?} aged out"));
        assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)), 0, "{refusal:?}: nothing to label");
        assert_the_contradicted_unadmitted(&rig, &format!("{refusal:?} after the writer"));
        assert_eq!(rig.samples(), A_WINDOW_OF_COMPARISONS, "{refusal:?}: and no sample is written");
        let rows = rig.rows();
        assert_eq!(&rows[..written.len()], written.as_slice(), "{refusal:?}: every row stands as it was written");
        assert_eq!(struck_on_file(&rig), CONTRADICTED, "{refusal:?}: each held strike written down once, beside its label");
        assert_eq!(held_strikes(&ledger), 0, "{refusal:?}: and let go once the ledger showed it back");
    }
}

/// 붙잡힌 줄긋기는 그것을 빚진 가장 새 읽기가 끝난 뒤 시작한 읽기만 정산한다(t-6263 R4c-3): 그보다 먼저 시작한 읽기는
/// 라벨이 없든 줄긋기를 보여 주든 아무것도 풀지 못하고, 그 뒤 읽기는 줄긋기가 보이거나(원장이 되돌려 보여 줌) 라벨이
/// 원장에서 잘려 나갔으면(보존 한도) 푼다 — 라벨이 있고 줄긋기가 없으면 다시 쓸 빚으로 돌려준다.
#[test]
fn a_held_strike_is_settled_only_on_a_reading_begun_after_it_was_owed() {
    let ledger = PathBuf::from("a-ledger-this-test-only-names").join(CHALLENGER_FILE);
    let label = arm::label_row("window-0#1", &receipted(Receipt::Failed), Preferred::Challenger, 1);
    let strike = arm::strike_row(&label, 2).expect("a strike");
    let settle = |rows: &[&Value], begun: u64, ended: u64, due: &[&Value]| {
        let reading = Reading {
            rows: rows.iter().copied().cloned().collect(),
            begun,
            ended,
        };
        let owed = unseen_strikes(&ledger, &reading, due.iter().copied().cloned().collect());
        (owed.len(), held_strikes(&ledger))
    };

    assert_eq!(settle(&[&label], 9, 10, &[&strike]), (1, 1), "a reading that owes it holds it and writes it");
    assert_eq!(settle(&[], 8, 11, &[]), (0, 1), "a reading begun before it was owed holds no label, and lets nothing go");
    assert_eq!(settle(&[&label, &strike], 7, 12, &[]), (0, 1), "nor does one that shows the strike");
    assert_eq!(settle(&[&label], 13, 14, &[]), (1, 1), "a later reading that holds the label and no strike owes it again");
    assert_eq!(settle(&[&label], 15, 20, &[&strike]), (1, 1), "and one that finds it due holds it as of its own end");
    assert_eq!(settle(&[&label, &strike], 16, 21, &[]), (0, 1), "so a reading begun before that end lets nothing go");
    assert_eq!(settle(&[&label, &strike], 22, 23, &[]), (0, 0), "a reading begun after it that shows the strike lets it go");

    assert_eq!(settle(&[&label], 24, 25, &[&strike]), (1, 1));
    assert_eq!(settle(&[], 26, 27, &[]), (0, 0), "as does one begun after it that no longer holds the label");
}

/// 라벨러가 적법하게 묶은 라벨도 같다(t-6263 R4c-2): 영수증(실패)으로 라벨과 표본이 선 뒤 같은 source에 나중 verdict(통과)가
/// 기록되고 실행 행만 먼저 보존 한도로 빠지면 남은 verdict가 라벨을 반박한다 — 그때 줄긋기 저장이 실패하고 나머지 verdict까지
/// 빠져도 그 표본은 다시 인정되지 않으며, 쓰기가 돌아오면 줄긋기가 한 번 적힌다.
#[test]
fn a_lawful_label_a_later_verdict_gainsays_stays_unadmitted_whatever_its_strike_met() {
    let attempt = "window-0#1";
    for refusal in STRIKE_REFUSALS {
        let rig = Rig::new(JevMode::On, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
        rig.seed_a_standing_window(false);
        let ledger = challenger_path(&rig.cwd);
        for n in 1..A_WINDOW_OF_COMPARISONS {
            let label = arm::label_row(&format!("window-{n}#1"), &receipted(Receipt::Failed), Preferred::Challenger, 1);
            append_shadow_row(&ledger, &label, SHADOW_LEDGER_MAX_BYTES).expect("a label");
        }
        let now = now_ms() / 1_000;
        let record = |row: RouteOutcomeRecord| runtime::record_route_outcome(&rig.cwd, &row).expect("a row");
        record(handed_in(attempt, Some(HANDED_IN)));
        record(verdict(attempt, runtime::OUTCOME_FAILED, VerdictSubject::Work, now));
        assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)), 1, "{refusal:?}: the labeller binds it");
        let sample = sample_attempt_key(attempt);
        assert!(admitted(&rig.cwd).contains(&sample), "{refusal:?}: its sample is admitted");

        // A later verdict on the same source says the work passed; the run
        // leaves the record first.
        record(verdict(attempt, runtime::OUTCOME_COMPLETED, VerdictSubject::Work, now + 9));
        age_out_rows(&rig.cwd, |row| row.run_id.as_deref() == Some(attempt) && is_run_row(row));
        let written = rig.rows();
        refuse_strikes(1, refusal);
        assert!(!admitted(&rig.cwd).contains(&sample), "{refusal:?}: a verdict still on record gainsays its label");
        assert!(every_refusal_spent(), "{refusal:?}: the reader tried to write its strike");
        assert_eq!(rig.rows(), written, "{refusal:?}: and it did not land");

        age_out(&rig.cwd, &[attempt]);
        assert!(!admitted(&rig.cwd).contains(&sample), "{refusal:?}: the record forgot the verdict; the label stays taken back");
        assert_eq!(note_verdicts_in(&rig.cwd, Some(JevMode::On), &feed_into(&rig.cwd)), 0, "{refusal:?}: nothing to label");
        assert!(!admitted(&rig.cwd).contains(&sample), "{refusal:?}: after the writer");
        assert_eq!(struck_on_file(&rig), [attempt], "{refusal:?}: its strike written down once, beside its label");
        assert_eq!(rig.samples(), A_WINDOW_OF_COMPARISONS, "{refusal:?}: and no sample written twice");
    }
}

/// 결과가 있는 도전 행은 모두 물은 말의 버전을 적는다(t-6263 R5): 답한 비교·벽을 넘긴 비교·검사에 걸린 답·문이
/// 거절한 비교 — 버전은 core 질문 표의 챌린저 값 그대로다.
#[test]
fn every_row_that_asked_names_the_rubric_it_asked_by() {
    let rows_of = |jev_body: &str, door: DoorWords, silent: bool| {
        let rig = Rig::new(JevMode::Shadow, jev_body, Scripted::answering("CHALLENGER: a plan"));
        rig.spent_today(now_ms());
        rig.write_settings(Some(JevMode::Shadow), door);
        let key = a_key_that_draws(Side::Challenger);
        let quiet = Mock::silent();
        let arm = if silent { rig.arm_on(&quiet.base_url, Box::new(|_| inventory())) } else { rig.arm() };
        arm.open(facts(&key, "t")).expect("drawn").finish(Some("INCUMBENT: a plan".to_string()));
        rig.rows()
    };
    let answered = rows_of(&jev_answer("first"), DoorWords::OPEN, false);
    let timed_out = rows_of(&jev_answer("first"), DoorWords::OPEN, true);
    let malformed = rows_of(&jev_answer("fourth"), DoorWords::OPEN, false);
    let refused = rows_of(&jev_answer("first"), DoorWords { enabled: false, ..DoorWords::OPEN }, false);
    for (case, rows) in [("answered", answered), ("timed out", timed_out), ("malformed", malformed), ("refused", refused)] {
        assert_eq!(rows.len(), 1, "{case}: {rows:?}");
        assert!(OUTCOME.read(&rows[0]).is_some(), "{case}: a row that asked");
        assert_eq!(
            RUBRIC_VERSION.read(&rows[0]),
            Some(&json!(zerocode_core::jev::questions::CHALLENGER_RUBRIC_VERSION)),
            "{case}: {}",
            rows[0]
        );
        assert!(rows[0].get(RUBRIC_VERSION.canonical).is_some(), "{case}: under its canonical spelling");
    }
    assert_eq!(CHALLENGER_RUBRIC_VERSION, zerocode_core::jev::questions::CHALLENGER_RUBRIC_VERSION, "the table's own number");
}

/// 팔이 찍는 버전이 곧 표의 챌린저 행이 판정받는 창이다(t-6877 r5, t-6263 R5): 팔이 적은 비교와 그 라벨은 모두 표가
/// 오늘 묻는 버전의 창에 들고, 그 창은 자리를 세운다. 질문을 바꿔 표의 버전을 올린 날에는 그 옛 행이 새 버전의 창에
/// 하나도 들지 않는다 — 창은 빈 채로 새로 열리고, 새 말로 물은 행만 그 창을 채우며, 판정 박자에 못 미친 얇은 창은
/// 판정받지 않는다.
#[test]
fn the_arms_rows_fill_the_window_its_row_is_judged_on_and_a_new_question_opens_a_new_one() {
    use zerocode_core::jev::promote::{marks_that_can_clear, on_the_newest_version, window_wanted_for};
    use zerocode_core::jev::summary::{AT, JUDGED_EVERY_ROWS};
    use zerocode_core::jev::JevUse;

    // One comparison as the arm files it: the version it names is the writer's own stamp.
    let rig = Rig::new(JevMode::Auto, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.spent_today(now_ms());
    rig.arm()
        .open(facts(&a_key_that_draws(Side::Challenger), "t"))
        .expect("drawn")
        .finish(Some("INCUMBENT: a plan".to_string()));
    let filed = rig.rows().pop().expect("the arm filed its comparison");
    let ledger = challenger_path(&rig.cwd);
    std::fs::remove_file(&ledger).expect("the ledger starts again from the arm's one row");
    let as_filed = |at: usize, attempt: &str| {
        let mut row = filed.clone();
        row[AT.canonical] = json!(at);
        row[ATTEMPT.canonical] = json!(attempt);
        row
    };
    let file = |row: &Value| append_shadow_row(&ledger, row, SHADOW_LEDGER_MAX_BYTES).expect("a row");
    let rose = |rows: &[Value]| rows.iter().filter(|row| word(row, &TRANSITION).as_deref() == Some(ROSE)).count();

    // A window's worth of them and a receipt's label on every one the judge needs, ending on a
    // judgment's boundary; the judge got the first few wrong.
    let wanted = window_wanted_for(&CHALLENGER).expect("the challenger rises");
    let marks = marks_that_can_clear(&CHALLENGER).expect("a width");
    let misses = CHALLENGER.negatives_wanted.expect("negatives");
    let thin = JUDGED_EVERY_ROWS;
    assert!(thin < wanted, "a judgment's worth of requests is short of a window");
    let asked = wanted + marks.saturating_sub(wanted).div_ceil(thin) * thin;
    for n in 0..asked {
        file(&as_filed(n, &format!("before-{n}#1")));
    }
    for n in 0..marks {
        let preferred = if n < misses { Preferred::Incumbent } else { Preferred::Challenger };
        let at = i64::try_from(asked + n).expect("small");
        file(&arm::label_row(&format!("before-{n}#1"), &receipted(Receipt::Failed), preferred, at));
    }

    // Every row the arm wrote is in the window the table's row reads today …
    let rows = rig.rows();
    let today = on_the_newest_version(&CHALLENGER, &rows);
    assert_eq!(today.asked(), asked, "every comparison the arm filed is asked under the words its row asks");
    assert_eq!(today.rows.len(), rows.len(), "and every label grading one");
    // … and the day the question moves on, none of them is in the new one.
    let tomorrow = JevUse { rubric_version: CHALLENGER.rubric_version + 1, ..CHALLENGER };
    assert!(on_the_newest_version(&tomorrow, &rows).rows.is_empty(), "no row of the words before in the new window");

    // The window is one the arm's own judge raises the seat on …
    let _ = note_verdicts_in(&rig.cwd, Some(JevMode::Auto), &feed_into(&rig.cwd));
    assert_eq!(rose(&rig.rows()), 1, "the arm's rows rise under the words they were asked in");

    // … and the new question's window holds only what it asked: a judgment's worth of requests
    // filed as the arm files them once its words move on, which is no window yet.
    for n in 0..thin {
        let mut row = as_filed(asked + marks + n, &format!("tomorrow-{n}#1"));
        row[RUBRIC_VERSION.canonical] = json!(tomorrow.rubric_version);
        file(&row);
    }
    let rows = rig.rows();
    let opened = on_the_newest_version(&tomorrow, &rows);
    assert_eq!(opened.asked(), thin, "the new window counts the new question's requests alone");
    assert_eq!(opened.rows.len(), thin, "and nothing of the words before");
    assert_eq!(
        judge_seat_rows(&tomorrow, &ledger, &rows, 99_999),
        None,
        "{thin} requests are not a window of {wanted}"
    );
    assert_eq!(rose(&rig.rows()), 1, "no rise is written on the evidence of the words before");
}

/// source 없는 라벨의 합의 표식은 자리를 세우지 못한다(t-6263 R4c): 자리 판정기가 세울 원장이라도 라벨이 source를 적지
/// 않았으면 그 표식은 판정에 들지 않아 자리는 오르지 않는다; 같은 원장의 라벨이 source를 적었으면 오른다.
#[test]
fn marks_of_labels_that_name_no_source_never_raise_the_seat() {
    let window = zerocode_core::jev::promote::window_wanted_for(&CHALLENGER).expect("the seat rises");
    // A judgment falls due every window's-worth of rows past the first window.
    let requests =
        window + A_WINDOW_OF_COMPARISONS * 80usize.saturating_sub(window).div_ceil(A_WINDOW_OF_COMPARISONS);
    let labels = 45;
    let rose = |sourced: bool| {
        let rig = Rig::new(JevMode::Auto, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
        let ledger = challenger_path(&rig.cwd);
        for n in 0..requests {
            append_shadow_row(&ledger, &request_row(&format!("seat-{n}#1"), Preferred::Challenger), SHADOW_LEDGER_MAX_BYTES)
                .expect("a comparison");
        }
        for n in requests - labels..requests {
            // Three the judge got wrong: the receipt failed the incumbent it preferred.
            let preferred = if n - (requests - labels) < zerocode_core::jev::NEGATIVES_WANTED {
                Preferred::Incumbent
            } else {
                Preferred::Challenger
            };
            let at = i64::try_from(requests + n).expect("small");
            let attempt = format!("seat-{n}#1");
            let label = if sourced {
                arm::label_row(&attempt, &receipted(Receipt::Failed), preferred, at)
            } else {
                unsourced_label(&attempt, Receipt::Failed, preferred, at)
            };
            append_shadow_row(&ledger, &label, SHADOW_LEDGER_MAX_BYTES).expect("a label");
        }
        let _ = note_verdicts_in(&rig.cwd, Some(JevMode::Auto), &feed_into(&rig.cwd));
        rig.rows().iter().any(|row| word(row, &TRANSITION).as_deref() == Some(ROSE))
    };
    assert!(rose(true), "labels bound to their sources raise the seat: a ledger the judge raises");
    assert!(!rose(false), "the same marks from labels that name no source raise nothing");
}

/* ---- the dashboard ----------------------------------------------------- */

/// 대시보드의 자리 행은 이 원장을 읽는다: 물은 수·답한 수·Jev 비용 — 붙들린 행은 물은 것이 아니다.
#[test]
fn the_dashboard_row_counts_the_arms_comparisons_and_not_its_holds() {
    let rig = Rig::new(JevMode::Shadow, &jev_answer("first"), Scripted::answering("CHALLENGER: a plan"));
    rig.spent_today(now_ms());
    let arm = rig.arm();
    let key = a_key_that_draws(Side::Challenger);
    arm.open(facts(&key, "t")).expect("drawn").finish(Some("INCUMBENT: a plan".to_string()));
    assert!(arm.open(AttemptFacts { role: None, ..facts(&a_key_that_draws(Side::Incumbent), "t") }).is_none());
    assert_eq!(rig.rows().len(), 2);
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(rig.home.join("settings.json")).expect("settings")).expect("json");
    let now = i64::try_from(now_ms()).expect("now");
    let reports = super::super::jev_summary::report(&[shadow_ledger_dir(&rig.cwd)], None, Some(&settings), now, 0);
    let seat = reports.iter().find(|report| report.id == CHALLENGER.id).expect("the challenger's row");
    assert_eq!(seat.found.as_deref(), Some(challenger_path(&rig.cwd).as_path()));
    assert_eq!(seat.mode, JevMode::Shadow);
    assert_eq!(seat.week.asked(), 1, "one comparison asked; the hold is not a request");
    assert_eq!(seat.week.answered, 1);
    assert_eq!(seat.week.input_tokens, 300);
    assert!(seat.cost_usd.is_some_and(|cost| cost > 0.0));
    assert_eq!(seat.asked_toward_judgment, 1);
}

/* ---- the clock --------------------------------------------------------- */

/// 하루의 경계: 날짜와 시작 시각은 그 시계의 것.
#[test]
fn today_is_the_local_day_and_its_start() {
    // 2026-09-24T23:30:00Z at +09:00 is 2026-09-25 08:30 local.
    let now_ms = 1_790_292_600_000;
    let seoul = Today::at(now_ms, SEOUL_MINUTES);
    assert_eq!(seoul.day, "2026-09-25");
    assert_eq!(seoul.start_ms, 1_790_262_000_000, "2026-09-24T15:00:00Z is Seoul's midnight");
    let utc = Today::at(now_ms, 0);
    assert_eq!(utc.day, "2026-09-24");
    assert_eq!(utc.start_ms, 1_790_208_000_000);
}

/// 이 기계의 요청 원장·인벤토리·결과 원장을 읽기만 하며 하루 몫과 읽기 비용을 잰다 — 아무것도 쓰지 않는다.
/// `ZEROCODE_CHALLENGER_MEASURE_DAYS_BACK` 만큼 앞선 날의 시작부터 센다(자정 직후의 빈 날 대신 어제를 재려고).
#[test]
#[ignore = "reads this machine's ledgers; run by hand"]
fn the_days_share_and_the_arms_reads_on_this_machine() {
    let home = PathBuf::from(std::env::var_os("ZEROCODE_CHALLENGER_MEASURE_HOME").expect("the zo home to read"));
    let cwd = PathBuf::from(std::env::var_os("ZEROCODE_CHALLENGER_MEASURE_CWD").expect("a project folder"));
    let days_back: u64 = std::env::var("ZEROCODE_CHALLENGER_MEASURE_DAYS_BACK")
        .ok()
        .and_then(|days| days.parse().ok())
        .unwrap_or(0);
    let root = home.join("cache").join("prompt-cache");
    let today = Today::now();
    let start_ms = today.start_ms.saturating_sub(days_back * 86_400_000);
    let started = Instant::now();
    let other = day_request_micros(std::slice::from_ref(&root), start_ms, api::model_price);
    let read_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let records = runtime::read_route_outcomes(&cwd).unwrap_or_default();
    let records_ms = started.elapsed().as_millis();
    let mut coding: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for record in &records {
        if record.role.as_deref() == Some("coding") && record.decision_kind() == DecisionKind::Model {
            *coding.entry(record.selected_model.clone()).or_default() += 1;
        }
    }
    let incumbent = coding.iter().max_by_key(|(_, count)| **count).map(|(model, _)| model.clone());
    let rate = incumbent.as_deref().and_then(|model| {
        runtime::learned_rate(&records, today.now_ms / 1_000, RouteRole::Coding, model, super::super::canonicalize_route_model_id)
    });
    let started = Instant::now();
    let inventory = runtime::connected_model_inventory(incumbent.as_deref().unwrap_or(INCUMBENT));
    let inventory_ms = started.elapsed().as_millis();
    let started = Instant::now();
    let picked = pick_challenger(
        &inventory,
        &records,
        today.now_ms / 1_000,
        RouteRole::Coding,
        incumbent.as_deref().unwrap_or(INCUMBENT),
        &[],
        api::model_price,
    );
    let pick_ms = started.elapsed().as_millis();
    // The cap's worth of plain words clears to itself.
    let ceiling = picked
        .as_ref()
        .ok()
        .map(|(_, price)| expected_design_micros(price, &"x".repeat(CHALLENGER_TASK_CHAR_CAP)));
    let judge = api::systemone_rate(zerocode_core::jev::DEFAULT_MODEL)
        .map(|rate| expected_comparison_micros(&rate, 1_024));
    let share = spend::day_spend(&spend::Book::default(), other).day_micros / 10;
    println!(
        "{}",
        json!({
            "from": start_ms,
            "dayOtherMicros": other,
            "shareMicros": share,
            "dayRequestReadMs": read_ms,
            "records": records.len(),
            "recordsMs": records_ms,
            "codingModels": coding,
            "incumbent": incumbent,
            "incumbentLearnedRate": rate,
            "inventoryModels": inventory.models().len(),
            "inventoryMs": inventory_ms,
            "pick": picked.as_ref().map(|(model, _)| model.clone()).map_err(|held| held.token()),
            "pickMs": pick_ms,
            "designCeilingMicros": ceiling,
            "judgeCeilingMicros": judge,
            "designsTheShareBuys": ceiling.map(|one| share / one.max(1)),
        })
    );
}

/// The comparisons the live check puts to the real endpoint: one pair where
/// one design answers the request and the other does not, one where both do
/// — synthetic words, nobody's work.
const LIVE_PAIRS: [(&str, &str, &str); 2] = [
    (
        "Make the config loader reject unknown keys instead of ignoring them.",
        "Add a deny-unknown-fields check to the config struct's deserializer, return an error that names the key and its file, and add one test that loads a file with a misspelled key and asserts the error names it.",
        "Rewrite the logging module to use structured JSON output and bump every dependency to its latest version.",
    ),
    (
        "Cache the parsed index so the second search does not re-read the files.",
        "Keep the parsed index in a process-wide map keyed by the root path and its newest mtime, rebuild it when the mtime moves, and test that a second search reads no file.",
        "Store the parsed index beside the root in a small binary file, reload it when every source file is older than it, and test that the second search opens only that file.",
    ),
];

/// 실제 Jev 끝점에 비교 질문 넷(쌍 둘 × 순서 둘)을 보내 모양이 받아들여지고 눈가림이 두 순서에서 같은 설계로 풀리는지 잰다 —
/// 키는 이 명령의 환경에서만, 홈은 임시, 원장·하루 셈은 사람의 것이 아니다. 호출 상한 넷.
#[test]
#[ignore = "sends four requests to the real endpoint; run by hand with the key in the command's environment"]
fn the_comparison_on_the_real_wire() {
    let home = tempfile::tempdir().expect("a config home");
    let work = tempfile::tempdir().expect("a workspace");
    let cwd = std::fs::canonicalize(work.path()).expect("resolved");
    let settings = JevSettings {
        enabled: true,
        workspaces: vec![door::resolved_path(&cwd)],
        daily_requests: Some(4),
        model: zerocode_core::jev::DEFAULT_MODEL.to_string(),
    };
    let door = JevDoor::at(settings, &cwd, home.path());
    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client).expect("TYPESAFE_API_KEY in the environment");
    let rate = api::systemone_rate(door.model()).expect("the judge is priced");
    let (mut sent, mut micros, mut consistent) = (0_u32, 0_u64, 0_usize);
    for (pair, (task, good, other)) in LIVE_PAIRS.iter().enumerate() {
        let mut preferred = Vec::new();
        for first in [Side::Challenger, Side::Incumbent] {
            let key = a_key_that_draws(first);
            // The challenger's design is the first of the pair, the incumbent's the second.
            let asked = arm::ask(&key, task, &Designs { incumbent: other, challenger: good });
            let wire = compare(&door, Some(&client), &asked);
            sent += wire.requests;
            micros += settled_comparison_micros(&rate, &wire);
            println!(
                "{}",
                json!({
                    "pair": pair,
                    "blind": asked.blind().token(),
                    "outcome": wire.outcome,
                    "preferred": wire.preferred.map(Preferred::token),
                    "confidence": wire.confidence,
                    "inputTokens": wire.input_tokens,
                    "elapsedMs": wire.elapsed_ms,
                    "rejected": wire.rejected,
                })
            );
            preferred.push(wire.preferred);
        }
        consistent += usize::from(preferred[0].is_some() && preferred[0] == preferred[1]);
    }
    println!("{}", json!({ "requests": sent, "costMicros": micros, "consistentAcrossOrders": consistent, "pairs": LIVE_PAIRS.len() }));
    assert!(sent <= 4, "the call cap");
}
