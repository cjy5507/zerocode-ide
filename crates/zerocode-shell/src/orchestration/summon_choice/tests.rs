//! What a summons' row promises whoever reads the ledger back. Every case
//! here crosses a real socket or none at all.

use serde_json::json;
use zerocode_core::jev::JevMode;
use zerocode_core::jev::door::Refused;
use zerocode_core::orchestration::Pinned;
use zerocode_core::summon_choice::{AgentRecord, Summonable};

use super::*;
use crate::systemone::tests::Endpoint;

/// The summons every case here is about: a `--worktree` task summoned on
/// `claude`, with `kimi` the other agent that had room.
fn shadow() -> SummonShadow {
    SummonShadow {
        pinned: Pinned {
            agent: "claude".to_string(),
            model: Some("claude-opus-5".to_string()),
            effort: Some("max".to_string()),
        },
        model_was_pinned: true,
        auto: false,
        brief: "measure the terminal's frame time again and put the numbers in the commit"
            .to_string(),
        brief_chars: 2_480,
        worktree: true,
        replaces_an_attempt: false,
        carries_a_task: true,
        attempts: 1,
        failures: 0,
        options: vec![
            Summonable {
                id: "claude".to_string(),
                spent_percent: Some(61),
                window: Some("weekly"),
                record: AgentRecord {
                    launched: 265,
                    carried: 137,
                    finished: 121,
                    median_minutes: Some(28),
                    recent_briefs: vec!["measure the seat's own latency".to_string()],
                },
            },
            Summonable {
                id: "kimi".to_string(),
                spent_percent: None,
                window: None,
                record: AgentRecord::default(),
            },
        ],
    }
}

fn seat(shadow: &SummonShadow) -> Value {
    opened(
        &Seat {
            run: "run-4275",
            worker: "w-4711",
            dispatch: Some("dp-4712"),
            task: Some("t-4711"),
        },
        shadow,
        JevMode::Shadow.key(),
        1_789_600_000_000,
    )
}

/// The endpoint's answer: `chosen`, with the rest of the room going to the
/// other agent.
fn an_agent_answer(chosen: &str, other: &str) -> String {
    json!({
        "model": "jev-1.13.0",
        "answers": {
            "summon": {
                "type": "choice",
                "choice": chosen,
                "probabilities": { chosen: 0.81, other: 0.19 },
                "confidence": 0.64,
            }
        },
        "usage": { "input_tokens": 900, "output_tokens": 0 },
    })
    .to_string()
}

/// zo's settings in a folder of the case's own, consenting to `consented`.
/// Answers the settings file's path.
fn settings_consenting_to(home: &tempfile::TempDir, consented: &str) -> std::path::PathBuf {
    use zerocode_core::jev::SMART_SETTINGS_KEY;
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({
            SMART_SETTINGS_KEY: {
                SUMMON.setting: JevMode::Shadow.key(),
                "jev": { "workspaces": [consented] },
            }
        })
        .to_string(),
    )
    .expect("zo's settings");
    settings
}

/// The option ids one row names, in the order the question offered them.
fn option_ids(row: &Value) -> Vec<String> {
    row["options"]
        .as_array()
        .expect("the options the answer was judged against")
        .iter()
        .map(|option| option["id"].as_str().expect("an id").to_string())
        .collect()
}

/// The request body a heard request carried.
fn heard_body(request: &str) -> Value {
    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("a request body");
    serde_json::from_str(body).expect("a json body")
}

/// One summons, asked and written down: what left the machine is the shape
/// and not the agent already chosen, and the row carries both answers beside
/// each other with the one number this ledger exists for — whether they
/// agreed. A summons that named no model of its own: a pinned one grades
/// nothing (`a_summons_whose_model_was_pinned_grades_nothing`).
#[test]
fn a_summons_row_carries_both_answers_and_says_whether_they_agreed() {
    let work = tempfile::tempdir().expect("a checkout");
    let home = tempfile::tempdir().expect("a zo home");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", an_agent_answer("kimi", "claude"), 0);
    let wire = Wire::at(
        &endpoint.base(),
        "test-key",
        Some(settings_consenting_to(
            &home,
            &work.path().display().to_string(),
        )),
    );
    let shadow = SummonShadow {
        model_was_pinned: false,
        ..shadow()
    };
    let ask =
        summon_choice::ask(&shadow.look(), &shadow.options).expect("two agents are a question");
    let row = settle(&wire, seat(&shadow), &ask, &shadow, Some(work.path()));

    // What was sent: the brief's head, the shape, and the two agents that
    // could have carried it — never the agent the coordinator typed.
    let heard = endpoint.asked();
    assert_eq!(heard.len(), 1, "asked {} times", heard.len());
    let sent = heard_body(&heard[0]);
    assert_eq!(
        sent["state"]["brief"],
        json!("measure the terminal's frame time again and put the numbers in the commit")
    );
    assert_eq!(sent["state"]["briefChars"], json!(2_480));
    assert_eq!(sent["state"]["worktree"], json!(true));
    // Each agent's room and record went as fields of its own entry, in the
    // order offered, and an option's words say nothing it weighs (t-9469).
    let agents = sent["state"]["agents"]
        .as_array()
        .expect("the agents offered");
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0]["id"], json!("claude"));
    assert_eq!(agents[0]["quotaSpentPercent"], json!(61));
    assert_eq!(agents[0]["summoned"], json!(265));
    assert_eq!(agents[0]["reachedWorkerDone"], json!(121));
    assert_eq!(
        agents[0]["newestTasks"],
        json!(["measure the seat's own latency"])
    );
    assert_eq!(agents[1]["id"], json!("kimi"));
    assert_eq!(agents[1]["quotaSpentPercent"], Value::Null, "no gauge read");
    assert_eq!(sent["state"]["summonedAll"], json!(265));
    let criteria = &sent["questions"]["summon"]["criteria"];
    for id in ["claude", "kimi"] {
        let said = criteria[id].as_str().expect("an option's words");
        assert!(
            !said.chars().any(|glyph| glyph.is_ascii_digit()),
            "an option carries no number: {said}"
        );
    }
    // The model the coordinator pinned is a constraint the options were
    // already narrowed by, and the state says it (t-6342); the agent it typed
    // is still nowhere in it.
    assert_eq!(sent["state"]["pinnedModel"], json!("claude-opus-5"));
    assert!(
        sent["state"].get("agent").is_none(),
        "the state showed the answer somebody already wrote down: {}",
        sent["state"]
    );

    // What came back, beside what was actually summoned.
    assert_eq!(row["outcome"], json!("answered"));
    assert_eq!(row["chosen"], json!("kimi"));
    assert_eq!(row["agent"], json!("claude"));
    assert_eq!(row[WORKER_MODEL_KEY], json!("claude-opus-5"));
    assert_eq!(
        row[zerocode_core::jev::summary::MODEL.canonical],
        json!("jev-1.13.0"),
        "the Jev version that judged the summons, beside the worker's model"
    );
    assert_eq!(row["effort"], json!("max"));
    assert_eq!(row["modelWasPinned"], json!(false));
    assert_eq!(row["agreed"], json!(false), "two agents, two answers");
    assert_eq!(row["confidence"], json!(0.64));
    assert_eq!(option_ids(&row), ["claude", "kimi"]);
    assert_eq!(row[REQUESTS_KEY], json!(1));
    assert_eq!(row[REDACTED_LINES_KEY], json!(0));
    assert_eq!(row["summon"], json!("w-4711"));
    assert_eq!(row["run"], json!("run-4275"));
    assert_eq!(row["dispatch"], json!("dp-4712"));
    assert_eq!(row["task"], json!("t-4711"));
    assert_eq!(row["mode"], json!("shadow"));
    assert_eq!(
        row["rubricVersion"],
        json!(zerocode_core::summon_choice::SUMMON_CHOICE_RUBRIC_VERSION)
    );
    // The task's own history travelled with the shape, so a reader of the
    // row sees the difficulty grade the question was given.
    assert_eq!(row["attempts"], json!(1));
    assert_eq!(row["failures"], json!(0));
    assert!(row["elapsedMs"].is_u64() && row["requestBytes"].as_u64() > Some(0));

    // The same question answered the coordinator's own way is the row that
    // says so — one word apart, and it is the word the evidence is made of.
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", an_agent_answer("claude", "kimi"), 0);
    let wire = Wire::at(
        &endpoint.base(),
        "test-key",
        Some(settings_consenting_to(
            &home,
            &work.path().display().to_string(),
        )),
    );
    let agreed = settle(&wire, seat(&shadow), &ask, &shadow, Some(work.path()));
    assert_eq!(agreed["chosen"], json!("claude"));
    assert_eq!(agreed["agreed"], json!(true));
    assert_eq!(
        zerocode_core::jev::summary::agreement_since(std::slice::from_ref(&agreed), 0),
        zerocode_core::jev::promote::Agreement {
            compared: 1,
            agreed: 1,
            // The pinned model's own vendor CLI named the same agent: the
            // seat's baseline on the same summons (t-6342).
            baseline_compared: 1,
            baseline_agreed: 1,
            not_compared: 0,
        },
        "a marked row is the one comparison the seat rises on"
    );
}

/// A summons whose own agent was never among the options is no comparison at
/// all: the row leaves `agreed` unwritten, says in a word why, and the seat's
/// agreement statistics pass it by.
///
/// The accident this closes: 2026-09-19 18:49, the summon seat's first row out
/// of `never asked` (w-4837) offered five agents and not the codex the summons
/// had actually landed on. Jev chose from a set the real answer was missing
/// from, and `agreed: false` put that into the very statistics the seat rises
/// on. The filter that dropped codex is fixed one crate over
/// (`GaugeReading::wall_to_act_on`); this is the second half, because a row is
/// evidence about what it was asked, and a judgment that was never offered the
/// answer did not disagree with it.
#[test]
fn a_summons_the_options_never_offered_is_no_comparison_at_all() {
    let work = tempfile::tempdir().expect("a checkout");
    let home = tempfile::tempdir().expect("a zo home");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", an_agent_answer("kimi", "claude"), 0);
    let wire = Wire::at(
        &endpoint.base(),
        "test-key",
        Some(settings_consenting_to(
            &home,
            &work.path().display().to_string(),
        )),
    );
    // The summons landed on codex; the options are the two agents the quota
    // gate said had room. The row this makes is the 09-19 row.
    let shadow = SummonShadow {
        pinned: Pinned {
            agent: "codex".to_string(),
            ..shadow().pinned
        },
        ..shadow()
    };
    let ask =
        summon_choice::ask(&shadow.look(), &shadow.options).expect("two agents are a question");
    let row = settle(&wire, seat(&shadow), &ask, &shadow, Some(work.path()));

    // The question was asked and answered in shape — that much is a request
    // like any other, and the row still says what came back.
    assert_eq!(row["outcome"], json!("answered"));
    assert_eq!(row["chosen"], json!("kimi"));
    assert_eq!(row["agent"], json!("codex"));
    assert_eq!(option_ids(&row), ["claude", "kimi"]);
    assert_eq!(row["confidence"], json!(0.64));
    // What it does NOT say: that the two disagreed.
    assert!(
        row["agreed"].is_null(),
        "a judgment never offered codex was marked as disagreeing with it: {row}"
    );
    assert_eq!(
        row[summon_choice::NOT_COMPARED_KEY],
        json!(summon_choice::NOT_OFFERED),
        "the row kept no word for why it carries no mark: {row}"
    );
    // And the judge reads it the way the row means it.
    assert_eq!(
        zerocode_core::jev::summary::agreement_since(std::slice::from_ref(&row), 0),
        zerocode_core::jev::promote::Agreement {
            not_compared: 1,
            ..zerocode_core::jev::promote::Agreement::default()
        },
        "evidence about nothing reached the seat's agreement statistics, or went uncounted as such"
    );
}

/// A summons whose model was pinned is no comparison either (t-9087): the pin
/// is the person's word, an apply stage leaves such a summons alone, and the
/// agent the coordinator typed beside it is the pin's own CLI — on this
/// machine's ledger (2026-09-25) every one of the 121 marked summonses of the
/// seat's current words was pinned, and the pin's CLI carried all 121, so the
/// seat's baseline stood at 1,000‰ and no answer could beat it. The row asks
/// and records what came back, and says why it carries no mark.
#[test]
fn a_summons_whose_model_was_pinned_grades_nothing() {
    let work = tempfile::tempdir().expect("a checkout");
    let home = tempfile::tempdir().expect("a zo home");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", an_agent_answer("claude", "kimi"), 0);
    let wire = Wire::at(
        &endpoint.base(),
        "test-key",
        Some(settings_consenting_to(
            &home,
            &work.path().display().to_string(),
        )),
    );
    let shadow = shadow();
    assert!(shadow.model_was_pinned);
    let ask =
        summon_choice::ask(&shadow.look(), &shadow.options).expect("two agents are a question");
    let row = settle(&wire, seat(&shadow), &ask, &shadow, Some(work.path()));

    // Asked and answered as any summons is.
    assert_eq!(endpoint.asked().len(), 1);
    assert_eq!(row["outcome"], json!("answered"));
    assert_eq!(row["chosen"], json!("claude"));
    assert_eq!(row["modelWasPinned"], json!(true));
    // No mark, for the seat or its baseline, and a word for why.
    assert!(
        row["agreed"].is_null(),
        "a pinned summons was graded: {row}"
    );
    assert!(
        row[zerocode_core::jev::summary::BASELINE_AGREED.canonical].is_null(),
        "{row}"
    );
    assert_eq!(
        row[summon_choice::NOT_COMPARED_KEY],
        json!(summon_choice::PINNED),
        "{row}"
    );
    assert_eq!(
        zerocode_core::jev::summary::agreement_since(std::slice::from_ref(&row), 0),
        zerocode_core::jev::promote::Agreement {
            not_compared: 1,
            ..zerocode_core::jev::promote::Agreement::default()
        }
    );
}

/// A checkout the person never consented to sends nothing at all, and the row
/// says which of the door's four questions stopped it — with the two numbers
/// every row written since the door carries.
#[test]
fn a_workspace_nobody_consented_to_is_the_rows_outcome_and_nothing_leaves() {
    let work = tempfile::tempdir().expect("a checkout");
    let home = tempfile::tempdir().expect("a zo home");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", an_agent_answer("kimi", "claude"), 0);
    let wire = Wire::at(
        &endpoint.base(),
        "test-key",
        Some(settings_consenting_to(&home, "/somewhere/else")),
    );
    let shadow = shadow();
    let ask =
        summon_choice::ask(&shadow.look(), &shadow.options).expect("two agents are a question");
    let row = settle(&wire, seat(&shadow), &ask, &shadow, Some(work.path()));

    assert!(
        endpoint.asked().is_empty(),
        "words left an unconsented tree"
    );
    assert_eq!(row["outcome"], json!(Refused::NotConsented.token()));
    assert_eq!(row[REQUESTS_KEY], json!(0));
    assert_eq!(row["requestBytes"], json!(0));
    assert!(row["chosen"].is_null() && row["agreed"].is_null());
}

/// An answer that names an agent this summons could not have landed on is
/// discarded whole: the row is `schema`, not a chosen agent nobody offered.
#[test]
fn an_answer_naming_an_agent_that_was_not_offered_says_nothing() {
    let work = tempfile::tempdir().expect("a checkout");
    let home = tempfile::tempdir().expect("a zo home");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", an_agent_answer("codex", "claude"), 0);
    let wire = Wire::at(
        &endpoint.base(),
        "test-key",
        Some(settings_consenting_to(
            &home,
            &work.path().display().to_string(),
        )),
    );
    let shadow = shadow();
    let ask =
        summon_choice::ask(&shadow.look(), &shadow.options).expect("two agents are a question");
    let row = settle(&wire, seat(&shadow), &ask, &shadow, Some(work.path()));
    assert_eq!(row["outcome"], json!(SCHEMA));
    assert!(row["chosen"].is_null());
    assert_eq!(row[REQUESTS_KEY], json!(1), "the request was still spent");
}

/// The rows go where the day's count goes, under zo's own config home.
#[test]
fn the_rows_sit_beside_the_days_count() {
    let home = tempfile::tempdir().expect("a zo home");
    let wire = Wire::at(
        "http://127.0.0.1:1",
        "k",
        Some(home.path().join("settings.json")),
    );
    assert_eq!(
        crate::systemone::ledger_of(&wire, &SUMMON),
        Some(
            home.path()
                .join(zerocode_core::jev::count::REQUESTS_DIR)
                .join("summon-choice.jsonl")
        )
    );
    assert_eq!(SUMMON.ledger, "summon-choice.jsonl");
}

/* ---- the seat's accuracy, replayed (t-5873) ---------------------------- */

/// The seed the replay reads: the ledger's own summonses, the gauges this
/// window has, and one entry per row to replay.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplaySeed {
    gauges: std::collections::BTreeMap<String, ReplayGauge>,
    replays: Vec<Replay>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplayGauge {
    spent_percent: Option<u8>,
    window: Option<String>,
}

/// One summons already decided, as its row and its task remember it.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Replay {
    /// When the summons was decided. Every fact the replay hands its
    /// question is read as of this moment and never later: a row replayed
    /// against a ledger that has moved on is asked about evidence the real
    /// question could not have had, and the answer is neither faithful nor
    /// reproducible — the same rubric came back 36 of 54 and then 24 of 54
    /// four hours apart, because four summonses had landed in between and
    /// changed what the options said (2026-09-22).
    at: i64,
    /// The dispatch snapshot at this row, including only outcomes already
    /// known then. Older seeds must be regenerated rather than replaying
    /// today's completion status as yesterday's evidence.
    carried: Vec<zerocode_core::summon_choice::CarriedSummons>,
    task: String,
    rubric_version: Option<u32>,
    /// The agent the summons actually landed on — the label
    /// ([`zerocode_core::jev::SUMMON`]'s `agreed` rule).
    label: String,
    /// Whether production's own answer agreed, read off the row.
    agreed_in_production: bool,
    options: Vec<String>,
    worktree: bool,
    replaces: bool,
    carries_a_task: bool,
    attempts: usize,
    failures: u32,
    title: String,
    spec: String,
}

/// Which arm the replay asks under, named by what each offered agent's entry
/// in the state carries (its option's words until t-9469).
const ARM_ENV: &str = "ZEROCODE_SUMMON_REPLAY_ARM";
/// Where the seed is.
const SEED_ENV: &str = "ZEROCODE_SUMMON_REPLAY_SEED";
/// The arm whose entries carry the quota gauge and no record — the evidence
/// the rubric had before the record was added.
const ARM_ROOM: &str = "room";
/// The arm whose entries carry this ledger's record as well.
const ARM_RECORD: &str = "record";
/// How many times each row is asked.
///
/// One is not enough and that is a measured fact, not a precaution: the same
/// rubric, the same seed and the same labels came back 36 of 54 and then 24
/// of 54 on two consecutive runs (2026-09-22). A single pass cannot tell one
/// wording from another at this sample size, so every arm is asked several
/// times and the table carries the pooled count and the spread.
const RUNS_ENV: &str = "ZEROCODE_SUMMON_REPLAY_RUNS";
/// How many times a row is asked when nobody says.
const RUNS_DEFAULT: usize = 3;
/// Which quota gauge the replayed entries carry.
///
/// The gauge is the strongest term in the question and the one the rows do
/// not record, so a replay reads it from a cache that keeps moving: across
/// one afternoon `claude` went 75% → 89% → 100% spent, and at 100% the
/// judgment correctly refuses the very agent that is the label on 40 of 54
/// rows. Every arm-to-arm difference measured against a live gauge is that
/// number's and not the rubric's. `seed` carries what the seed captured;
/// `unread` holds every entry at "no gauge read", which is a state the
/// product really has, is identical for every option and every arm, and
/// leaves the record as the only thing that differs between arms. Rows
/// written from now on carry their own gauge ([`offered`]), which is what
/// makes a faithful replay possible later.
const GAUGE_ENV: &str = "ZEROCODE_SUMMON_REPLAY_GAUGE";
/// The gauge word that holds every option at "nobody read one".
const GAUGE_UNREAD: &str = "unread";

/// This seat's agreement, replayed over the summonses that already happened
/// (t-5873, docs/design/jev-dashboard-and-perfection-20260921.md §3 J2).
///
/// Every row whose `agreed` mark production wrote is asked again under
/// today's words, against the same option set and the same label, and the
/// table it prints is what the before/after in the report is read off.
///
/// **Not a check.** It crosses a real socket and spends one Jev request per
/// row, so it is `#[ignore]`d and run by hand. It asks under a zo home of its
/// own, so the person's own day count and their own ledgers are untouched.
///
/// ```sh
/// python3 tools/summon-replay/seed.py \
///   --ledger ~/.zo/jev/summon-choice.jsonl \
///   --store "$HOME/Library/Application Support/dev.zerocode.app/authority/authority.sqlite" \
///   --out /tmp/summon-replay/seed.json
/// TYPESAFE_API_KEY=$(security find-generic-password \
///     -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w) \
/// ZEROCODE_SUMMON_REPLAY_SEED=/tmp/summon-replay/seed.json \
/// ZEROCODE_SUMMON_REPLAY_ARM=record \
///   cargo test -p zerocode-shell --bin zerocode-shell \
///   orchestration::summon_choice::tests::the_seats_agreement_over_the_rows_that_already_happened \
///   -- --ignored --nocapture
/// ```
#[test]
#[ignore = "a measurement against api.typesafe.ai, printed; not a check"]
fn the_seats_agreement_over_the_rows_that_already_happened() {
    let seed_at = std::env::var(SEED_ENV)
        .unwrap_or_else(|_| panic!("{SEED_ENV} names the seed tools/summon-replay/seed.py wrote"));
    let arm = std::env::var(ARM_ENV).unwrap_or_else(|_| ARM_RECORD.to_string());
    assert!(
        arm == ARM_ROOM || arm == ARM_RECORD,
        "{ARM_ENV} is `{ARM_ROOM}` or `{ARM_RECORD}`, not `{arm}`"
    );
    let gauge_word = std::env::var(GAUGE_ENV).unwrap_or_else(|_| GAUGE_UNREAD.to_string());
    let read_gauges = gauge_word != GAUGE_UNREAD;
    let seed: ReplaySeed =
        serde_json::from_str(&std::fs::read_to_string(&seed_at).expect("the seed reads"))
            .expect("the seed's shape");

    let work = tempfile::tempdir().expect("a checkout");
    let home = tempfile::tempdir().expect("a zo home of this measurement's own");
    // The key comes from the environment and not from the keychain: inside a
    // test build `accounts::security_command` is a fixture holding a map, so
    // a `Wire::of_this_machine()` here would read a keychain that does not
    // exist. The runner lifts it out of the real keychain in the command
    // line above, where no file and no log ever sees it.
    let key = std::env::var(zerocode_harness::TYPESAFE_API_KEY_ENV).unwrap_or_else(|_| {
        panic!(
            "{} carries this machine's TypeSafe key",
            zerocode_harness::TYPESAFE_API_KEY_ENV
        )
    });
    let wire = Wire::at(
        crate::systemone::SYSTEMONE_BASE_URL,
        &key,
        Some(settings_consenting_to(
            &home,
            &work.path().display().to_string(),
        )),
    );

    let runs: usize = std::env::var(RUNS_ENV)
        .ok()
        .and_then(|said| said.parse().ok())
        .unwrap_or(RUNS_DEFAULT)
        .max(1);
    // One agreement per pass, pooled at the end: the spread across passes is
    // what says whether a difference between two arms is a difference at all.
    let mut passes = vec![zerocode_core::jev::promote::Agreement::default(); runs];
    // How often the same question came back with the same answer — the
    // seat's own repeatability, which caps how high any agreement can go.
    let mut repeated = zerocode_core::jev::promote::Agreement::default();
    let mut agreement = zerocode_core::jev::promote::Agreement::default();
    let mut production = zerocode_core::jev::promote::Agreement::default();
    // The same two counts per rubric version the row was written under: the
    // seat's baseline was measured over its v2 rows, and a before/after read
    // over a different population is not a before/after.
    let mut by_version: std::collections::BTreeMap<
        String,
        (
            zerocode_core::jev::promote::Agreement,
            zerocode_core::jev::promote::Agreement,
        ),
    > = std::collections::BTreeMap::new();
    let mut elapsed = Vec::new();
    let mut refusals = 0usize;
    let mut confusion: std::collections::BTreeMap<(String, String), usize> =
        std::collections::BTreeMap::new();
    println!(
        "arm={arm} gauge={gauge} seed={seed_at} rows={rows} runs={runs}",
        gauge = if read_gauges { "seed" } else { GAUGE_UNREAD },
        rows = seed.replays.len()
    );
    for replay in &seed.replays {
        // The ledger as it stood when this summons was decided — the one
        // fold, over the summonses that had already happened. `<` and not
        // `<=`, for the reason the product reads its options before the
        // reservation: this summons must not count itself or quote its own
        // task (t-4839).
        let records = zerocode_core::summon_choice::records(
            &replay
                .carried
                .iter()
                .filter(|carried| carried.started_ms < replay.at)
                .cloned()
                .collect::<Vec<_>>(),
        );
        let options: Vec<Summonable> = replay
            .options
            .iter()
            .map(|id| {
                let gauge = seed.gauges.get(id).filter(|_| read_gauges);
                Summonable {
                    id: id.clone(),
                    spent_percent: gauge.and_then(|gauge| gauge.spent_percent),
                    // The option's window word is `'static` in the product
                    // because the quota table owns it; the seed carries the
                    // same word and the measurement matches it back rather
                    // than inventing a leak.
                    window: gauge
                        .and_then(|gauge| gauge.window.as_deref())
                        .and_then(quota_window_word),
                    record: match arm.as_str() {
                        ARM_RECORD => records.get(id).cloned().unwrap_or_default(),
                        _ => zerocode_core::summon_choice::AgentRecord::default(),
                    },
                }
            })
            .collect();
        let words = match replay.title.is_empty() || replay.spec.starts_with(&replay.title) {
            true if replay.spec.is_empty() => replay.title.clone(),
            true => replay.spec.clone(),
            false => format!("{}\n\n{}", replay.title, replay.spec),
        };
        let (brief, brief_chars) = summon_choice::brief_shape(&words);
        let look = zerocode_core::summon_choice::SummonLook {
            brief: &brief,
            brief_chars,
            worktree: replay.worktree,
            replaces_an_attempt: replay.replaces,
            carries_a_task: replay.carries_a_task,
            attempts: replay.attempts,
            failures: replay.failures,
            pinned_model: None,
        };
        let Some(ask) = summon_choice::ask(&look, &options) else {
            continue;
        };
        let mut chose = Vec::new();
        for (pass, tally) in passes.iter_mut().enumerate() {
            let began = Instant::now();
            let answer = wire.ask(
                &SUMMON,
                Some(work.path()),
                request_body(&ask.state, &ask.questions),
                SUMMON_CHOICE_DEADLINE,
            );
            let took = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
            let read = answer
                .answer
                .ok()
                .and_then(|body| serde_json::from_str::<Value>(&body).ok())
                .and_then(|parsed| ask.read(parsed.get("answers")?).ok());
            let Some(pick) = read else {
                refusals += 1;
                println!("  {task} REFUSED", task = replay.task);
                continue;
            };
            elapsed.push(took);
            let agreed = pick.chosen == replay.label;
            agreement.compared += 1;
            agreement.agreed += usize::from(agreed);
            tally.compared += 1;
            tally.agreed += usize::from(agreed);
            production.compared += 1;
            production.agreed += usize::from(replay.agreed_in_production);
            let held = by_version
                .entry(
                    replay
                        .rubric_version
                        .map_or_else(|| "?".to_string(), |version| format!("v{version}")),
                )
                .or_default();
            held.0.compared += 1;
            held.0.agreed += usize::from(replay.agreed_in_production);
            held.1.compared += 1;
            held.1.agreed += usize::from(agreed);
            *confusion
                .entry((replay.label.clone(), pick.chosen.clone()))
                .or_default() += 1;
            chose.push(pick.chosen.clone());
            println!(
                "  {task:<9} v{was} #{pass} label={label:<9} chose={chosen:<11} \
                 conf={conf:.2} {mark} (production {before}) {took} ms",
                task = replay.task,
                was = replay
                    .rubric_version
                    .map_or_else(|| "?".to_string(), |version| version.to_string()),
                label = replay.label,
                chosen = pick.chosen,
                conf = pick.confidence,
                mark = if agreed { "AGREE" } else { "differ" },
                before = if replay.agreed_in_production {
                    "agreed"
                } else {
                    "differed"
                },
            );
        }
        // Every pass after the first, against the first: how often this seat
        // says the same thing twice about one unchanged question.
        for said in chose.iter().skip(1) {
            repeated.compared += 1;
            repeated.agreed += usize::from(Some(said) == chose.first());
        }
    }
    elapsed.sort_unstable();
    println!("\nRepeated asks are dependent observations; pooled shares have no Wilson interval.");
    println!("| arm | compared | agreed | share | p50 ms | p95 ms |");
    println!("| --- | --- | --- | --- | --- | --- |");
    for (name, held, times) in [
        ("production", &production, &[][..]),
        (arm.as_str(), &agreement, elapsed.as_slice()),
    ] {
        println!(
            "| {name} | {compared} | {agreed} | {share} | {p50} | {p95} |",
            compared = held.compared,
            agreed = held.agreed,
            share = share_of(held),
            p50 = percentile_of(times, 0.50),
            p95 = percentile_of(times, 0.95),
        );
    }
    println!("\n| rows written under | production | {arm} |");
    println!("| --- | --- | --- |");
    for (version, (before, after)) in &by_version {
        println!(
            "| {version} (n={n}) | {before_share} | {after_share} |",
            n = before.compared,
            before_share = share_of(before),
            after_share = share_of(after),
        );
    }
    println!("\n| pass | compared | agreed | share | Wilson lower (one ask per row) |");
    println!("| --- | --- | --- | --- | --- |");
    for (pass, held) in passes.iter().enumerate() {
        println!(
            "| #{pass} | {compared} | {agreed} | {share} | {bound} |",
            compared = held.compared,
            agreed = held.agreed,
            share = share_of(held),
            bound = bound_of(held),
        );
    }
    println!(
        "\nthe same question answered the same way again: {agreed}/{compared} ({share})",
        agreed = repeated.agreed,
        compared = repeated.compared,
        share = share_of(&repeated),
    );
    println!("\nrefused: {refusals}");
    println!("\n| label | chose | rows |");
    println!("| --- | --- | --- |");
    for ((label, chose), rows) in &confusion {
        println!("| {label} | {chose} | {rows} |");
    }
}

/// The quota table's own word for a window, matched back from the seed — so
/// a replayed option says the same word a live one would and the harness
/// spells none of them itself.
fn quota_window_word(said: &str) -> Option<&'static str> {
    use zerocode_core::orchestration::QuotaWindow;
    [
        QuotaWindow::Session,
        QuotaWindow::Weekly,
        QuotaWindow::Monthly,
    ]
    .into_iter()
    .map(QuotaWindow::as_str)
    .find(|word| *word == said)
}

/// One agreement's 95% Wilson lower bound, as a line prints it.
fn bound_of(held: &zerocode_core::jev::promote::Agreement) -> String {
    held.lower_bound()
        .map_or_else(|| "—".to_string(), |bound| format!("{:.1}%", bound * 100.0))
}

/// One agreement's share, as a line prints it.
fn share_of(held: &zerocode_core::jev::promote::Agreement) -> String {
    match held.compared {
        0 => "—".to_string(),
        #[allow(clippy::cast_precision_loss)]
        compared => format!("{:.1}%", held.agreed as f64 / compared as f64 * 100.0),
    }
}

/// One percentile of a sorted population, as a line prints it.
fn percentile_of(sorted: &[u64], share: f64) -> String {
    zerocode_core::jev::summary::percentile(sorted, share)
        .map_or_else(|| "—".to_string(), |held| held.to_string())
}
