//! The agent question joins the assign moment's one request (t-16578).
use super::*;
use crate::summon_choice::{AgentRecord, SummonLook, Summonable};
use crate::summon_difficulty::lineup::Lineup;
use serde_json::json;

fn room(id: &str) -> Summonable {
    Summonable {
        id: id.to_string(),
        spent_percent: None,
        window: None,
        record: AgentRecord::default(),
    }
}

fn agent_ask(agents: &[&str]) -> SummonAsk {
    let rooms: Vec<Summonable> = agents.iter().map(|id| room(id)).collect();
    crate::summon_choice::ask(
        &SummonLook {
            brief: "measure the frame time",
            brief_chars: 22,
            worktree: true,
            replaces_an_attempt: false,
            carries_a_task: true,
            attempts: 2,
            failures: 1,
            pinned_model: None,
        },
        &rooms,
    )
    .expect("two agents are a question")
}

fn look() -> Look {
    Look {
        title: "task".into(),
        spec: "measure the frame time".into(),
        attempt: 2,
        failures: 1,
        retry_of: false,
    }
}

/// `agent`'s pair question over `models` models of the efforts the ladder takes.
fn pair(agent: &str, models: usize) -> ModelAsk {
    let catalog: Vec<Value> = (0..models)
        .map(|n| json!({"provider": agent, "id": format!("{agent}-m{n}"), "builtin": true}))
        .collect();
    let lineup = Lineup::from_catalog(&json!({ "models": catalog }), agent).unwrap();
    let options = crate::summon_model::options(
        agent,
        &lineup,
        None,
        &Default::default(),
        &["low", "medium", "high"],
        |_| None,
        0,
    );
    crate::summon_model::ask_for(agent, &look(), &options).unwrap()
}

fn joined() -> AssignAsk {
    AssignAsk {
        agent: Some(agent_ask(&["claude", "codex"])),
        difficulty: Some(look()),
        model: None,
        pairs: vec![pair("claude", 3), pair("codex", 4)],
    }
}

/// A summons with the agent and the dials open is ONE request: three
/// seats' questions — the agent, the difficulty, and a pair question for each
/// agent the answer may choose — over one state, each keyed for itself.
#[test]
fn the_agent_difficulty_and_pair_questions_ride_one_request() {
    let asked = joined();
    assert_eq!(asked.seats(), 3);
    assert!(asked.shared());
    let questions = asked.questions();
    let mut names: Vec<&str> = questions
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "summon",
            "summon_difficulty",
            "summon_model_claude",
            "summon_model_codex"
        ]
    );
    let state = asked.state();
    for key in [
        "brief",
        "agents",
        "title",
        "spec",
        "attempt",
        "models_claude",
        "models_codex",
    ] {
        assert!(state.get(key).is_some(), "{key} rides the one state");
    }
    assert_eq!(state["failures"], json!(1), "the facts they share agree");
    assert_eq!(state["models_claude"].as_array().unwrap().len(), 3);
    assert_eq!(state["models_codex"].as_array().unwrap().len(), 4);
}

/// The endpoint's answer to one pair question: `chosen`, with every offered
/// word given an equal share.
fn answer_to(ask: &ModelAsk, question: &str, chosen: &str) -> Value {
    let words: Vec<String> = ask
        .offered()
        .iter()
        .map(|(model, effort)| crate::summon_model::option_word(model, effort))
        .chain([crate::summon_model::ABSTAIN.to_string()])
        .collect();
    let share = 1.0 / words.len() as f64;
    json!({ question: {
        "type": "choice", "choice": chosen, "confidence": 0.8,
        "probabilities": words.iter().map(|word| (word.clone(), json!(share))).collect::<serde_json::Map<_, _>>(),
    }})
}

/// Only the chosen agent's pair is read, and each pair is read under its own
/// question: another agent's answer, or its option words, are refused.
#[test]
fn a_pair_is_read_only_under_its_own_agents_question() {
    let asked = joined();
    let claude = asked.pair_of("claude").unwrap();
    let codex = asked.pair_of("codex").unwrap();
    assert_eq!(claude.scope(), Some("claude"));
    let answers = answer_to(claude, "summon_model_claude", "claude-m1|medium");
    let read = claude.read(&answers).expect("its own question's answer");
    assert_eq!(
        read.chosen,
        Some(("claude-m1".to_string(), "medium".to_string()))
    );
    assert!(
        codex.read(&answers).is_err(),
        "codex's question was not answered"
    );
    assert!(asked.pair_of("kimi").is_none());
}

/// What the join costs is a number the code can say: the pair questions'
/// words and model lists, and no more — dropping the pairs leaves the agent's
/// and the difficulty's request whole.
#[test]
fn the_cost_of_joining_the_pairs_is_counted_in_bytes() {
    let asked = joined();
    let alone = asked.only(true, true, false);
    assert!(alone.pairs.is_empty());
    assert!(asked.join_bytes() > 0);
    assert_eq!(alone.join_bytes(), 0);
    let size = |ask: &AssignAsk| ask.state().to_string().len() + ask.questions().to_string().len();
    assert!(
        size(&asked) >= size(&alone) + asked.join_bytes() / 2,
        "the pairs add their bytes: {} over {}",
        size(&asked),
        size(&alone)
    );
}

/// Every row of the agent catalog is accounted for: the agent question
/// offers it by its id, and its row says whether a pair question rides for it
/// or why none does.
#[test]
fn every_catalog_agent_is_reachable_or_says_why_not() {
    let ids: Vec<&str> = crate::agent::AGENT_SPECS
        .iter()
        .map(|spec| spec.id)
        .collect();
    let rooms: Vec<Summonable> = ids.iter().map(|id| room(id)).collect();
    let ask = crate::summon_choice::ask(
        &SummonLook {
            brief: "x",
            brief_chars: 1,
            worktree: false,
            replaces_an_attempt: false,
            carries_a_task: false,
            attempts: 0,
            failures: 0,
            pinned_model: None,
        },
        &rooms,
    )
    .unwrap();
    for id in ids {
        assert!(ask.offered(id), "{id} is an option of the agent question");
        match Reach::of(id) {
            Reach::Pair => assert!(Reach::of(id).why_not().is_none()),
            Reach::AgentOnly => assert!(
                Reach::of(id).why_not().is_some_and(|why| !why.is_empty()),
                "{id} says why it has no pair"
            ),
        }
    }
}
