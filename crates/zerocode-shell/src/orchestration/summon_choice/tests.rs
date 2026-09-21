//! What a summons' row promises whoever reads the ledger back. Every case
//! here crosses a real socket or none at all.

use serde_json::json;
use zerocode_core::jev::JevMode;
use zerocode_core::jev::door::Refused;
use zerocode_core::orchestration::Pinned;
use zerocode_core::summon_choice::Summonable;

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
        options: vec![
            Summonable {
                id: "claude".to_string(),
                spent_percent: Some(61),
                window: Some("weekly"),
                launched: 0,
                recent_brief: None,
            },
            Summonable {
                id: "kimi".to_string(),
                spent_percent: None,
                window: None,
                launched: 0,
                recent_brief: None,
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
/// agreed.
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
    let shadow = shadow();
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
    let criteria = &sent["questions"]["summon"]["criteria"];
    assert!(
        criteria["claude"]
            .as_str()
            .expect("claude's room")
            .contains("61%")
    );
    assert!(
        criteria["kimi"]
            .as_str()
            .expect("kimi's room")
            .contains("no quota gauge")
    );
    assert!(
        !sent["state"].to_string().contains("opus"),
        "the state showed the answer somebody already wrote down: {}",
        sent["state"]
    );

    // What came back, beside what was actually summoned.
    assert_eq!(row["outcome"], json!("answered"));
    assert_eq!(row["chosen"], json!("kimi"));
    assert_eq!(row["agent"], json!("claude"));
    assert_eq!(row["model"], json!("claude-opus-5"));
    assert_eq!(row["effort"], json!("max"));
    assert_eq!(row["modelWasPinned"], json!(true));
    assert_eq!(row["agreed"], json!(false), "two agents, two answers");
    assert_eq!(row["confidence"], json!(0.64));
    assert_eq!(row["options"], json!(["claude", "kimi"]));
    assert_eq!(row[REQUESTS_KEY], json!(1));
    assert_eq!(row[REDACTED_LINES_KEY], json!(0));
    assert_eq!(row["summon"], json!("w-4711"));
    assert_eq!(row["run"], json!("run-4275"));
    assert_eq!(row["dispatch"], json!("dp-4712"));
    assert_eq!(row["task"], json!("t-4711"));
    assert_eq!(row["mode"], json!("shadow"));
    assert_eq!(row["rubricVersion"], json!(2));
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
    assert_eq!(row["options"], json!(["claude", "kimi"]));
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
        zerocode_core::jev::promote::Agreement::default(),
        "evidence about nothing reached the seat's agreement statistics"
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
