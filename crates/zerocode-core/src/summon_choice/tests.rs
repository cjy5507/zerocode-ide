//! What the summons' question promises the caller that asks it.

use serde_json::json;

use super::*;

fn room(id: &str, spent: Option<u8>) -> Summonable {
    Summonable {
        id: id.to_string(),
        spent_percent: spent,
        window: spent.map(|_| "weekly"),
        record: AgentRecord::default(),
    }
}

/// One summons this ledger carried for `agent`, as the record fold reads it.
fn carried(agent: &str, started_ms: i64, ended: Option<(i64, bool)>) -> CarriedSummons {
    CarriedSummons {
        agent: agent.to_string(),
        started_ms,
        ended_ms: ended.map(|(ended_ms, _)| ended_ms),
        succeeded: ended.map(|(_, succeeded)| succeeded),
        title: Some(format!("{agent}'s task at {started_ms}")),
    }
}

fn look() -> SummonLook<'static> {
    SummonLook {
        brief: "터미널 그리기 경로의 프레임 시간을 재고, 숫자를 커밋 메시지에 남겨.",
        brief_chars: 37,
        worktree: true,
        replaces_an_attempt: false,
        carries_a_task: true,
        attempts: 0,
        failures: 0,
        pinned_model: None,
    }
}

#[test]
fn the_version_is_pinned_to_the_words() {
    // Changing a word of the question without bumping the version turns this
    // red: a judgment read under one wording is not evidence about another.
    // Version 6 moves each agent's room and record out of its option's
    // sentence and into the state's `agents` (t-9469): what the question
    // reads changed, so the series starts again.
    assert_eq!(SUMMON_CHOICE_RUBRIC_VERSION, 6);
    assert_eq!(
        crate::jev::rubric_fingerprint(rubric_words),
        "f8e0588f5fb7f459"
    );
}

/// The one entry `agents` holds for `id`.
fn entry<'a>(state: &'a Value, id: &str) -> &'a Value {
    state[AGENTS_KEY]
        .as_array()
        .expect("the agents offered")
        .iter()
        .find(|agent| agent[AGENT_KEYS[0]] == json!(id))
        .unwrap_or_else(|| panic!("no entry for {id}: {state}"))
}

/// A choice between one agent is not a choice. A summons on a machine with
/// one agent left standing gets no question at all, rather than a row saying
/// the judgment agreed with a decision nobody made.
#[test]
fn one_option_is_not_a_question() {
    assert_eq!(FEWEST_OPTIONS, 2);
    assert!(ask(&look(), &[]).is_none());
    assert!(ask(&look(), &[room("claude", Some(61))]).is_none());
    assert!(ask(&look(), &[room("claude", Some(61)), room("codex", None)]).is_some());
}

/// Each option says what choosing it means and nothing it weighs: the room
/// its provider has left is a field of its entry in `agents`, and an agent
/// whose gauge nobody read carries no number there — which is not the same
/// as one that has room, and not the same as one that is empty (t-9469).
#[test]
fn an_option_says_what_choosing_it_means_and_its_room_is_a_field() {
    let asked = ask(&look(), &[room("claude", Some(61)), room("cursor", None)])
        .expect("two agents are a question");
    assert_eq!(asked.options(), ["claude", "cursor"]);
    let criteria = &asked.questions["summon"]["criteria"];
    for id in ["claude", "cursor"] {
        assert_eq!(
            criteria[id],
            json!(OPTION_MEANS.replace("{agent}", id)),
            "an option's words are its meaning, the same sentence for every agent"
        );
        let said = criteria[id].as_str().expect("a description");
        assert!(
            !said.chars().any(|glyph| glyph.is_ascii_digit()),
            "an option carries no number: {said}"
        );
    }
    let claude = entry(&asked.state, "claude");
    assert_eq!(claude[AGENT_KEYS[1]], json!(61));
    assert_eq!(claude[AGENT_KEYS[2]], json!("weekly"));
    let cursor = entry(&asked.state, "cursor");
    assert_eq!(cursor[AGENT_KEYS[1]], Value::Null, "no gauge was read");
    assert_eq!(cursor[AGENT_KEYS[2]], Value::Null);
}

/// The endpoint reads a question's kind off its own tag: a body without one
/// is refused whole (`union_tag_not_found`, HTTP 422, measured against
/// api.typesafe.ai on 2026-09-18), which is a shape failure no answer ever
/// comes back from.
#[test]
fn the_question_wears_the_tag_the_endpoint_reads_its_kind_from() {
    let asked = ask(&look(), &[room("claude", Some(61)), room("codex", Some(9))])
        .expect("two agents are a question");
    assert_eq!(asked.questions["summon"]["type"], json!("choice"));
}

/// The state is the summons' shape and not the summons — and above all it
/// does not carry the agent the coordinator typed. A question that shows the
/// answer somebody already wrote down is not a second opinion. The agents it
/// names are the options, every one of them, each once and in the order
/// offered, so nothing in it singles one out.
#[test]
fn the_state_is_the_shape_and_never_the_coordinators_own_choice() {
    let asked = ask(&look(), &[room("claude", Some(61)), room("codex", Some(9))])
        .expect("two agents are a question");
    let state = &asked.state;
    let keys: Vec<&str> = state
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    let mut expected = STATE_KEYS.to_vec();
    let mut seen = keys.clone();
    expected.sort_unstable();
    seen.sort_unstable();
    assert_eq!(seen, expected, "the state's keys are the fingerprint's");
    assert_eq!(state["worktree"], json!(true));
    assert_eq!(state["replaces"], json!(false));
    assert_eq!(state["task"], json!(true));
    assert_eq!(state["attempts"], json!(0));
    assert_eq!(state["failures"], json!(0));
    let named: Vec<&str> = state[AGENTS_KEY]
        .as_array()
        .expect("the agents offered")
        .iter()
        .map(|agent| agent[AGENT_KEYS[0]].as_str().expect("an id"))
        .collect();
    assert_eq!(named, asked.options(), "every option, once, in order");
    for agent in state[AGENTS_KEY].as_array().expect("the agents offered") {
        let mut keys: Vec<&str> = agent
            .as_object()
            .expect("an entry")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut expected = AGENT_KEYS.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected, "an entry's keys are the fingerprint's");
    }
    let said = state.to_string();
    for word in ["opus", "model", "effort"] {
        assert!(!said.contains(word), "the state named `{word}`: {said}");
    }
    // And the question names every key it reads, the state's and each
    // entry's, by its path (t-9469): a key the words never name is evidence
    // the model has to guess the meaning of.
    for key in STATE_KEYS.iter().chain(&AGENT_KEYS) {
        assert!(
            INSTRUCTIONS.contains(&format!("`{key}`")),
            "the question never names `{key}`"
        );
    }
}

/// The head is what leaves, cut to the table's cap here as well as at the
/// door — and how long the whole brief was survives the cut, because a brief
/// four times the cap and one that fits are different sizes of ask.
#[test]
fn the_brief_is_cut_to_the_tables_cap_and_its_whole_length_survives() {
    let long = "가".repeat(SUMMON_BRIEF_CHAR_CAP + 500);
    let (head, whole) = brief_shape(&long);
    assert_eq!(head.chars().count(), SUMMON_BRIEF_CHAR_CAP);
    assert_eq!(whole, SUMMON_BRIEF_CHAR_CAP + 500);
    assert_eq!(brief_shape("짧다"), ("짧다".to_string(), 2));

    let mut look = look();
    look.brief = &long;
    look.brief_chars = whole;
    let asked = ask(&look, &[room("claude", Some(1)), room("codex", Some(2))])
        .expect("two agents are a question");
    assert_eq!(
        asked.state["brief"]
            .as_str()
            .expect("the head")
            .chars()
            .count(),
        SUMMON_BRIEF_CHAR_CAP
    );
    assert_eq!(
        asked.state["briefChars"],
        json!(SUMMON_BRIEF_CHAR_CAP + 500)
    );
}

/// The reader judges against the set THIS question offered. An agent this
/// machine has but the gate held back — one at its wall — cannot be chosen by
/// an answer that names it anyway.
#[test]
fn an_answer_is_judged_against_the_set_that_was_asked() {
    let asked = ask(&look(), &[room("claude", Some(61)), room("kimi", Some(12))])
        .expect("two agents are a question");
    let walled = json!({
        "summon": {
            "type": "choice",
            "choice": "codex",
            "probabilities": { "claude": 0.5, "kimi": 0.5 },
            "confidence": 0.9,
        }
    });
    assert_eq!(
        asked.read(&walled),
        Err(crate::jev::choice::ChoiceRefusal::UnknownOption),
        "codex was not offered to this summons"
    );

    let honest = json!({
        "summon": {
            "type": "choice",
            "choice": "kimi",
            "probabilities": { "claude": 0.3, "kimi": 0.7 },
            "confidence": 0.62,
        }
    });
    let read = asked.read(&honest).expect("an offered agent reads");
    assert_eq!(read.chosen, "kimi");
    assert!((read.confidence - 0.62).abs() < 1e-9);
    assert_eq!(read.probabilities.len(), 2);

    // And the same set answers the other direction: whether the agent the
    // summons actually landed on was one of the things offered. A row about a
    // summons this question never carried an option for is not a comparison,
    // and the word it says so with is this module's ([`NOT_OFFERED`]).
    assert!(asked.offered("claude") && asked.offered("kimi"));
    assert!(!asked.offered("codex"), "codex was never offered");
    assert_eq!(NOT_OFFERED, "not_offered");
    assert_eq!(NOT_COMPARED_KEY, "notCompared");
}

/// An agent's entry says what this ledger has summoned it for — the free
/// hindsight a coordinator's own choices leave behind — against the offered
/// set's own total, and says plainly when it never has: a zero, and no task,
/// instead of a history nobody wrote.
#[test]
fn an_entry_carries_the_ledgers_own_summons_history() {
    let seasoned = Summonable {
        record: AgentRecord {
            launched: 12,
            recent_briefs: vec!["measure the seat's latency on the installed build".to_string()],
            ..AgentRecord::default()
        },
        ..room("claude", Some(61))
    };
    let bare = Summonable {
        record: AgentRecord {
            launched: 1,
            ..AgentRecord::default()
        },
        ..room("codex", None)
    };
    let asked =
        ask(&look(), &[seasoned, room("kimi", None), bare]).expect("three agents are a question");
    assert_eq!(asked.state[SUMMONED_ALL_KEY], json!(13), "12 + 0 + 1");
    let claude = entry(&asked.state, "claude");
    assert_eq!(claude[AGENT_KEYS[3]], json!(12));
    assert_eq!(
        claude[AGENT_KEYS[7]],
        json!(["measure the seat's latency on the installed build"])
    );
    let kimi = entry(&asked.state, "kimi");
    assert_eq!(kimi[AGENT_KEYS[3]], json!(0), "never summoned");
    assert_eq!(kimi[AGENT_KEYS[7]], json!([]));
    let codex = entry(&asked.state, "codex");
    assert_eq!(codex[AGENT_KEYS[3]], json!(1));
    assert_eq!(
        codex[AGENT_KEYS[7]],
        json!([]),
        "a pane summoned with no task"
    );
}

/// A task title is the coordinators' own words: each leaves cut to its cap,
/// at most the newest few, and only as a field of the state the door clears
/// (`crate::jev::SUMMON`'s `sends`) — never inside an option's sentence.
#[test]
fn a_task_title_is_a_field_cut_to_its_cap() {
    let long = "가".repeat(SUMMON_RECENT_BRIEF_CHAR_CAP + 40);
    let seasoned = Summonable {
        record: AgentRecord {
            launched: 3,
            recent_briefs: vec![long.clone(), "two".to_string(), "three".to_string()],
            ..AgentRecord::default()
        },
        ..room("claude", Some(61))
    };
    let asked = ask(&look(), &[seasoned, room("kimi", None)]).expect("two agents are a question");
    let titles = entry(&asked.state, "claude")[AGENT_KEYS[7]]
        .as_array()
        .expect("titles")
        .clone();
    assert_eq!(titles.len(), 3);
    assert_eq!(
        titles[0].as_str().map(|title| title.chars().count()),
        Some(SUMMON_RECENT_BRIEF_CHAR_CAP)
    );
    assert!(
        !asked.questions.to_string().contains("가"),
        "a title reached an option's words"
    );
}

/// An entry says what became of the work, not only how often it was chosen.
/// A count alone left the judgment naming an agent this ledger had never
/// summoned in eleven of seventeen disagreements (t-5873), because "never
/// summoned" and "summoned 265 times" read as equally neutral facts when
/// neither says what came back.
#[test]
fn an_entry_says_what_became_of_the_work_this_ledger_gave_it() {
    let folded = records(&[
        // Three ended: two reached `worker_done`, at 10, 30 and 50 minutes.
        carried("claude", 1_000, Some((1_000 + 10 * 60_000, true))),
        carried("claude", 2_000, Some((2_000 + 30 * 60_000, false))),
        carried("claude", 3_000, Some((3_000 + 50 * 60_000, true))),
        // And one still open, which is no outcome at all.
        carried("claude", 4_000, None),
        carried("codex", 9_000, None),
    ]);
    let claude = folded.get("claude").expect("a record");
    assert_eq!(
        (claude.launched, claude.carried, claude.finished),
        (4, 3, 2),
        "an open dispatch is counted as a summons and as no outcome"
    );
    assert_eq!(claude.median_minutes, Some(30));
    assert_eq!(
        claude.recent_briefs,
        [
            "claude's task at 4000",
            "claude's task at 3000",
            "claude's task at 2000"
        ],
        "the newest summonses name the newest tasks, newest first"
    );
    // An agent summoned once, still running, has a history and no record —
    // and says so rather than showing a median that reads as a failure.
    let codex = folded.get("codex").expect("a record");
    assert_eq!((codex.launched, codex.carried), (1, 0));
    assert_eq!(codex.median_minutes, None);
    let asked = ask(
        &look(),
        &[
            Summonable {
                record: claude.clone(),
                ..room("claude", Some(61))
            },
            Summonable {
                record: codex.clone(),
                ..room("codex", None)
            },
        ],
    )
    .expect("two agents are a question");
    let said = entry(&asked.state, "claude");
    assert_eq!(said[AGENT_KEYS[4]], json!(3), "ended");
    assert_eq!(said[AGENT_KEYS[5]], json!(2), "reached worker_done");
    assert_eq!(said[AGENT_KEYS[6]], json!(30), "median minutes");
    let said = entry(&asked.state, "codex");
    assert_eq!(said[AGENT_KEYS[4]], json!(0), "none of it has ended yet");
    assert_eq!(said[AGENT_KEYS[6]], Value::Null, "no median over nothing");
}
