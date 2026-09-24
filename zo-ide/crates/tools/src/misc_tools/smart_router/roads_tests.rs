//! The routing seat's roads, driven through the two entries whose shape the
//! second version did not change (t-4727, t-6346): a turn's
//! [`super::assess_turn_probed`] and a spawn batch's
//! [`super::apply::apply_smart_models_to_spawn_input`]. Each case stands on a
//! machine of its own — a config home holding the seat's word with the
//! working directory consented, a key, a fake System One and a fake chat
//! probe — so what a case counts is exactly what the entry asked.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use runtime::RouteTaskComplexity as C;
use serde_json::{json, Value};
use zerocode_core::jev::questions::{
    ROUTING_COMPLEXITY_ID, ROUTING_FACTS, ROUTING_INTENTS, ROUTING_INTENT_ID, ROUTING_REASONING,
    ROUTING_REASONING_ID, ROUTING_RISK_ID,
};

use super::decision_shadow::decision_shadow_path;
use super::jev_mock::Mock;
use super::turn::AssessmentReaders;

/// The one model every case routes under.
const PARENT: &str = "gpt-5.5-codex";

/// How long a case waits for a recording judgment's row: it is detached.
const ROW_WALL: Duration = Duration::from_secs(10);

/// Words no other case asked — both memos are process-wide, and a recalled
/// judgment sends nothing — and that the control sample leaves alone, so a
/// case that counts probe requests counts the routing road's only.
fn unique(words: &str) -> String {
    unique_for("", words)
}

/// [`unique`] for a spawn's prompt under `description`: the sample is drawn
/// on the fingerprint of both fields.
fn unique_for(description: &str, words: &str) -> String {
    loop {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |since| since.as_nanos());
        let text = format!("{words} #{nanos}");
        if !super::decision_shadow::control_sampled(super::probe_exec::task_fingerprint(description, &text)) {
            return text;
        }
    }
}

/// A second-version answer: complexity at `level` with `confidence`, a low
/// risk, a debugging intent, a search, and every fact a clear no.
fn answer(level: usize, confidence: f64) -> String {
    let mut levels = [0.02; 4];
    levels[level] = 0.94;
    #[allow(clippy::cast_precision_loss)]
    let score: f64 = levels.iter().enumerate().map(|(at, share)| at as f64 * share).sum();
    let spread = |words: &[&str], chosen: &str, leading: f64| {
        #[allow(clippy::cast_precision_loss)]
        let rest = (1.0 - leading) / (words.len() - 1) as f64;
        words
            .iter()
            .map(|word| ((*word).to_string(), json!(if *word == chosen { leading } else { rest })))
            .collect::<serde_json::Map<String, Value>>()
    };
    let intents: Vec<&str> = ROUTING_INTENTS.iter().map(|intent| intent.option.word).collect();
    let kinds: Vec<&str> = ROUTING_REASONING.iter().map(|kind| kind.word).collect();
    let mut answers = json!({
        ROUTING_COMPLEXITY_ID: {
            "type": "score", "score": (score * 100.0).round() / 100.0, "confidence": confidence,
            "legend": {"0": "a", "1": "b", "2": "c", "3": "d"},
            "probabilities": {"0": levels[0], "1": levels[1], "2": levels[2], "3": levels[3]},
        },
        ROUTING_RISK_ID: {
            "type": "score", "score": 0.1, "confidence": 0.9, "legend": {"0": "a", "1": "b", "2": "c", "3": "d"},
            "probabilities": {"0": 0.9, "1": 0.1, "2": 0.0, "3": 0.0},
        },
        ROUTING_INTENT_ID: {"type": "choice", "choice": "debugging", "confidence": 0.8,
            "probabilities": spread(&intents, "debugging", 0.82)},
        ROUTING_REASONING_ID: {"type": "choice", "choice": "search", "confidence": 0.66,
            "probabilities": spread(&kinds, "search", 0.7)},
    });
    for fact in &ROUTING_FACTS {
        answers[fact.id] = json!({"type": "noul", "noul": 0.05});
    }
    json!({"model": "jev-1.13.0", "answers": answers, "usage": {"input_tokens": 900, "output_tokens": 0}}).to_string()
}

/// One case's machine: settings, key and both fake endpoints in the
/// environment for as long as it stands, under the crate's environment lock.
struct Machine {
    _home: tempfile::TempDir,
    ledger: PathBuf,
    _env: crate::tests::EnvGuard,
}

impl Machine {
    /// `smart` as given, with the working directory consented at the door.
    fn new(mut smart: Value, judgment: &Mock, probe: &Mock) -> Self {
        let home = tempfile::tempdir().expect("a config home");
        let cwd = std::env::current_dir().expect("a working directory");
        smart["enabled"] = json!(true);
        smart["jev"] = json!({ "workspaces": [zerocode_core::jev::door::resolved_path(&cwd)] });
        std::fs::write(home.path().join("settings.json"), json!({ "smart": smart }).to_string())
            .expect("a settings file");
        let env = crate::tests::EnvGuard::set(core_types::paths::ZO_CONFIG_HOME_ENV, &home.path().to_string_lossy())
            .set_also(core_types::paths::ZO_HOME_ENV, home.path())
            .set_also("HOME", home.path())
            .set_also(core_types::paths::ZO_STATE_DIR_ENV, home.path())
            .set_also(api::SYSTEMONE_API_KEY_ENV, "test-key")
            .set_also(api::SYSTEMONE_BASE_URL_ENV, &judgment.base_url)
            .set_also("ZO_PROBE_BASE_URL", &probe.base_url);
        Self { ledger: decision_shadow_path(&cwd), _home: home, _env: env }
    }

    /// The ledger's rows as written — read as JSON, so a case says what a
    /// row carries by the key a reader sweeps for.
    fn rows(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.ledger)
            .map(|text| text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
            .unwrap_or_default()
    }

    /// The judgment rows once `count` have landed, or what is there at the
    /// wall — a recording judgment runs detached.
    fn rows_after(&self, count: usize) -> Vec<Value> {
        let started = Instant::now();
        loop {
            let rows = self.rows();
            if rows.len() >= count || started.elapsed() > ROW_WALL {
                return rows;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn answering(body: String) -> Mock {
    Mock::serving(200, body)
}

/// The fake chat probe: its answer does not matter to these cases — only
/// whether it was asked — so it answers what no probe parses and the turn
/// keeps the tables' verdict wherever the probe is its only reader.
fn probe() -> Mock {
    Mock::serving(200, "{}".to_string())
}

fn turn(text: &str, verify_leg: bool) -> super::TurnProbeAssessment {
    super::assess_turn_probed(text, PARENT, AssessmentReaders { verify_leg }, "turn-9@1")
}

/// A `Large` brief — every orchestration brief is one — was never judged: the
/// probe's gate declined it and the judgment sat behind that gate (0 of 25
/// rows applied, t-4727). A seat that only records judges it now, detached,
/// and its row says the probe was not asked.
#[test]
fn a_large_brief_is_judged_when_the_seat_only_records() {
    let judgment = answering(answer(3, 0.9));
    let chat = probe();
    let machine = Machine::new(json!({ "decisionShadow": "shadow" }), &judgment, &chat);
    let brief = unique("design and build a landing page across the whole repo");
    assert_eq!(super::assess_turn_complexity(&brief), C::Large, "premise: the tables read it Large");

    let assessment = turn(&brief, false);

    assert_eq!(assessment.complexity, C::Large, "a recording seat routes nothing");
    assert!(!assessment.judged);
    let rows = machine.rows_after(1);
    assert_eq!(rows.len(), 1, "the brief was judged once: {rows:?}");
    assert_eq!(judgment.requests().len(), 1);
    assert!(chat.requests().is_empty(), "the probe's gate still declines a Large brief");
    let row = &rows[0];
    assert_eq!(row["outcome"], "answered", "{row}");
    assert_eq!(row["routeUse"], "record_only");
    assert_eq!(row["attempt"], "turn-9@1");
    assert_eq!(row["probe"], "not_asked", "the probe was not asked, and the row says so");
    assert_eq!(row["band"], "act", "what acting would have done");
    assert!(row["reading"].is_object(), "every answer is kept for the reader that will weigh it");
    assert_eq!(row["rule"]["complexity"], "large", "the tables' own reading, beside the judgment's");
}

/// When the seat acts and the turn's verdict has a reader, every band is
/// judged once — the easy band the probe would have taken, the verb-matched
/// `Medium` and the `Large` brief it never took — and an answer sure enough
/// to act routes the turn with no chat probe at all.
#[test]
fn every_turn_asks_once_when_the_setting_is_on() {
    let judgment = answering(answer(2, 0.9));
    let chat = probe();
    let machine = Machine::new(json!({ "decisionShadow": "on" }), &judgment, &chat);
    let turns = [
        (unique("이 설정값이 뭔지 알려줘"), C::Small),
        (unique("이 함수의 버그를 수정해줘"), C::Medium),
        (unique("design and build a landing page across the whole repo"), C::Large),
    ];
    for (asked, (text, band)) in turns.iter().enumerate() {
        assert_eq!(super::assess_turn_complexity(text), *band, "premise: {text}");
        let assessment = turn(text, true);
        assert!(assessment.judged, "{text}: the judgment routed it");
        assert_eq!(judgment.requests().len(), asked + 1, "{text}: asked exactly once");
    }
    assert!(chat.requests().is_empty(), "an acting answer leaves the probe unasked");
    let rows = machine.rows();
    assert_eq!(rows.len(), turns.len());
    assert!(rows.iter().all(|row| row["routeUse"] == "applied"), "{rows:?}");
}

/// The chat probe is asked only where the judgment abstains (t-6346): the
/// same easy turn, which the probe's gate admits, costs no probe when the
/// answer is sure and one probe when it is not — and its row says it
/// abstained rather than failed.
#[test]
fn the_probe_is_called_only_inside_the_abstain_band() {
    {
        let judgment = answering(answer(1, 0.9));
        let chat = probe();
        let machine = Machine::new(json!({ "decisionShadow": "on" }), &judgment, &chat);
        let assessment = turn(&unique("이 설정값이 뭔지 알려줘"), true);
        assert!(assessment.judged);
        assert!(chat.requests().is_empty(), "a sure answer spends no probe");
        assert_eq!(machine.rows()[0]["routeUse"], "applied");
    }
    {
        let judgment = answering(answer(1, 0.4));
        let chat = probe();
        let machine = Machine::new(json!({ "decisionShadow": "on" }), &judgment, &chat);
        let assessment = turn(&unique("이 설정값이 뭔지 알려줘"), true);
        assert!(!assessment.judged, "an abstaining answer routes nothing");
        assert_eq!(chat.requests().len(), 1, "the probe is asked in its place");
        let rows = machine.rows();
        assert_eq!(rows[0]["routeUse"], "abstained");
        assert_eq!(rows[0]["outcome"], "answered", "an abstention is an answer, not a failure");
        assert_eq!(rows[0]["band"], "abstain");
    }
}

/// Where the probe's gate declines the turn, an abstaining answer leaves the
/// keyword tables standing — no probe is bought for a band the gate refused.
#[test]
fn an_abstaining_answer_on_a_band_the_probe_declines_keeps_the_tables() {
    let judgment = answering(answer(0, 0.4));
    let chat = probe();
    let machine = Machine::new(json!({ "decisionShadow": "on" }), &judgment, &chat);
    let brief = unique("design and build a landing page across the whole repo");

    let assessment = turn(&brief, true);

    assert_eq!(assessment.complexity, C::Large);
    assert!(!assessment.judged);
    assert!(chat.requests().is_empty());
    assert_eq!(machine.rows()[0]["routeUse"], "abstained");
}

/// A person who named the model in the turn is never routed below it: the
/// Architect's implementer is walked down by the band, so a judgment sure
/// enough to lower the band leaves it where the tables put it — and lowers
/// it for the same words with no model named.
#[test]
fn a_person_pinned_model_is_never_routed_below() {
    let judgment = answering(answer(0, 0.95));
    let chat = probe();
    let _machine = Machine::new(json!({ "decisionShadow": "on" }), &judgment, &chat);
    let named = unique("opus로 이 설정값이 뭔지 알려줘");
    let unnamed = unique("이 설정값이 뭔지 알려줘");
    assert_eq!(super::turn::user_named_model(&named), Some("claude-opus-5"), "premise: the turn names a model");
    assert_eq!(super::assess_turn_complexity(&named), C::Small, "premise: the tables read it Small");
    assert_eq!(super::assess_turn_complexity(&unnamed), C::Small);

    let pinned = turn(&named, true);
    let free = turn(&unnamed, true);

    assert!(pinned.judged && free.judged);
    assert_eq!(pinned.complexity, C::Small, "the person's pick is not walked down");
    assert_eq!(free.complexity, C::Trivial, "the same answer lowers an unnamed turn one band");
}

/// A spawn is judged whatever the classifier word, as long as automatic
/// routing runs and the seat asks: the probe waits on the person's `probed`,
/// the judgment no longer does (t-4727: `autoClassifier` defaults to
/// deterministic, so the spawn road had never asked it).
#[test]
fn a_spawn_is_judged_under_the_deterministic_classifier() {
    let judgment = answering(answer(2, 0.9));
    let chat = probe();
    let machine = Machine::new(
        json!({ "decisionShadow": "on", "autoClassifier": "deterministic" }),
        &judgment,
        &chat,
    );
    let mut input = crate::misc_tools::SpawnMultiAgentInput {
        agents: vec![json!({"description": "verify", "prompt": unique_for("verify", "run the tests and report")})],
        concurrency: None,
        parent_session_id: None,
        plan_shape: None,
        registry: None,
        tool_call_id: None,
        mcp_passthrough: None,
        parent_permission_mode: None,
    };

    super::apply::apply_smart_models_to_spawn_input(Some("claude-sonnet-main"), &mut input);

    assert_eq!(judgment.requests().len(), 1, "the spawn's task was judged");
    assert!(chat.requests().is_empty(), "the deterministic word asks no probe");
    let rows = machine.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["routeUse"], "applied");
}

/// The memo is keyed by the facts beside the words (run-6774 V1): a spawn
/// retried after a failed attempt carries `retry_of_failed_attempt`, and the
/// complexity question reads it — so the same description and prompt under
/// that fact is another state, and the first attempt's remembered answer is
/// not recalled for it. The same words under the same facts are.
#[test]
fn a_retry_of_a_failed_attempt_is_judged_again_not_recalled() {
    let judgment = answering(answer(2, 0.9));
    let chat = probe();
    let machine = Machine::new(json!({ "decisionShadow": "on" }), &judgment, &chat);
    let inventory = runtime::ModelInventory::new(PARENT, Vec::new());
    let prompt = unique_for("implement", "write the parser and its tests");
    let judged = |retry_of_failed_attempt: bool| {
        super::probe_exec::route_probe_assessment(
            &inventory,
            PARENT,
            "implement",
            &prompt,
            runtime::RoutingFacts { retry_of_failed_attempt },
            "turn-9@1",
            super::probe_exec::Admitted { probe: true, read: true },
        )
    };

    judged(false);
    assert_eq!(judgment.requests().len(), 1, "the first attempt was judged");
    judged(false);
    assert_eq!(judgment.requests().len(), 1, "the same words under the same facts are recalled, not asked");
    judged(true);
    assert_eq!(
        judgment.requests().len(),
        2,
        "a retry of a failed attempt is another state: the question reads the fact, so it is asked again"
    );
    let rows = machine.rows_after(3);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[1]["cached"], true, "the second first-attempt row came from the memo");
    assert_ne!(rows[2]["cached"], true, "the retry's row did not");
}

/// The route-use words a routing row carries are the table's own, so the
/// judge and every sweep of the ledger count them under one spelling.
#[test]
fn a_routing_rows_route_use_words_are_the_tables() {
    use super::decision_shadow::DecisionRouteUse;
    let word = |route_use: DecisionRouteUse| serde_json::to_value(route_use).expect("a word");
    assert_eq!(word(DecisionRouteUse::Applied), zerocode_core::jev::ROUTE_USE_APPLIED);
    assert_eq!(word(DecisionRouteUse::Fallback), zerocode_core::jev::ROUTE_USE_FALLBACK);
    assert_eq!(word(DecisionRouteUse::Abstained), zerocode_core::jev::ROUTE_USE_ABSTAINED);
}

/// What each word of the seat adds to a turn's start — the time
/// [`super::assess_turn_probed`] holds the turn before its first request —
/// on a fake System One that answers at once, so what is printed is this
/// side's own cost; the wire's own time is the replay's
/// (`tools/routing-replay/README.md`). A measurement, not a check: run with
/// `--ignored --nocapture`.
#[test]
#[ignore = "a measurement: prints what each word adds to a turn's start"]
fn what_each_word_adds_to_a_turns_start() {
    const TURNS: usize = 200;
    for word in ["off", "shadow", "auto", "on"] {
        let judgment = answering(answer(2, 0.9));
        let chat = probe();
        let machine = Machine::new(json!({ "decisionShadow": word }), &judgment, &chat);
        let mut took: Vec<u128> = (0..TURNS)
            .map(|_| {
                let text = unique("이 함수의 버그를 수정해줘");
                let started = Instant::now();
                let _ = turn(&text, true);
                started.elapsed().as_micros()
            })
            .collect();
        // A recording judgment lands after the turn has gone on: wait for
        // it, so the next word's machine starts on a quiet port.
        let landed = if word == "off" { 0 } else { machine.rows_after(TURNS).len() };
        took.sort_unstable();
        let at = |share: f64| {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
            let index = ((took.len() - 1) as f64 * share).round() as usize;
            took[index]
        };
        println!(
            "{word}: p50 {} µs · p95 {} µs · max {} µs · rows {landed} · requests {}",
            at(0.5),
            at(0.95),
            took[took.len() - 1],
            judgment.requests().len()
        );
    }
}

/// What reading the seat's standing costs once its ledger is full — `auto`
/// reads it on every turn the seat asks about, to know whether it may act
/// ([`super::decision_shadow::acts_here`]). The rows are the second
/// version's own, repeated to the ledger's cap. A measurement, not a check.
#[test]
#[ignore = "a measurement: prints what the standing read costs on a full ledger"]
fn what_a_full_ledger_costs_the_seats_standing() {
    const TURNS: usize = 20;
    let judgment = answering(answer(2, 0.9));
    let chat = probe();
    let machine = Machine::new(json!({ "decisionShadow": "shadow" }), &judgment, &chat);
    for _ in 0..TURNS {
        let _ = turn(&unique("이 함수의 버그를 수정해줘"), true);
    }
    let lines: Vec<String> = machine.rows_after(TURNS).iter().map(Value::to_string).collect();
    let row_bytes = lines.iter().map(String::len).sum::<usize>() / lines.len().max(1);
    let cap = usize::try_from(super::shadow_ledger::SHADOW_LEDGER_MAX_BYTES).expect("the cap fits");
    let mut text = String::with_capacity(cap + row_bytes * TURNS);
    while text.len() < cap {
        for line in &lines {
            text.push_str(line);
            text.push('\n');
        }
    }
    let full = tempfile::NamedTempFile::new().expect("a ledger file");
    std::fs::write(full.path(), &text).expect("the ledger writes");
    let every_row = || {
        let rows = super::jev_summary::read_rows(full.path());
        zerocode_core::jev::promote::stand_from(&rows) == zerocode_core::jev::promote::Stand::Applying
    };
    let timed = |read: &dyn Fn() -> bool| {
        let mut took: Vec<u128> = (0..20)
            .map(|_| {
                let started = Instant::now();
                let _ = read();
                started.elapsed().as_micros()
            })
            .collect();
        took.sort_unstable();
        (took[took.len() / 2], took[took.len() - 1])
    };
    assert_eq!(super::decision_shadow::raised_at(full.path()), every_row(), "both reads stand the seat alike");
    let (whole_p50, whole_max) = timed(&every_row);
    let (lines_p50, lines_max) = timed(&|| super::decision_shadow::raised_at(full.path()));
    println!(
        "row {row_bytes} B · ledger {} B, {} rows · every row parsed p50 {whole_p50} µs (max {whole_max}) · transition lines only p50 {lines_p50} µs (max {lines_max})",
        text.len(),
        text.lines().count(),
    );
}
