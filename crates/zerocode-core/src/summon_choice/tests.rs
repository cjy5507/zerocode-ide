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
    assert_eq!(SUMMON_CHOICE_RUBRIC_VERSION, 4);
    assert_eq!(
        crate::jev::rubric_fingerprint(rubric_words),
        "1d5d6af8efffc1de"
    );
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

/// Each option says its own id and the room its provider has left, and an
/// agent whose gauge nobody read says exactly that — which is not the same
/// sentence as one that has room, and not the same as one that is empty.
#[test]
fn an_option_says_the_room_this_window_has_actually_read() {
    let asked = ask(&look(), &[room("claude", Some(61)), room("cursor", None)])
        .expect("two agents are a question");
    assert_eq!(asked.options(), ["claude", "cursor"]);
    let criteria = &asked.questions["summon"]["criteria"];
    assert_eq!(
        criteria["claude"],
        json!(
            "claude. 61% of its weekly quota is already spent on this machine. This window \
             has never summoned it, so nothing this machine measured says how it does."
        )
    );
    assert_eq!(
        criteria["cursor"],
        json!(
            "cursor. This machine has read no quota gauge for it, so how much room it has is \
             unknown. This window has never summoned it, so nothing this machine measured \
             says how it does."
        )
    );
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
/// answer somebody already wrote down is not a second opinion.
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
    let said = state.to_string();
    for word in ["claude", "codex", "opus", "model", "effort"] {
        assert!(!said.contains(word), "the state named `{word}`: {said}");
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

/// An option says what this ledger has summoned the agent for — the free
/// hindsight a coordinator's own choices leave behind — and says plainly
/// when it never has, instead of implying a history nobody wrote.
#[test]
fn an_option_carries_the_ledgers_own_summons_history() {
    let seasoned = Summonable {
        record: AgentRecord {
            launched: 12,
            recent_briefs: vec!["measure the seat's latency on the installed build".to_string()],
            ..AgentRecord::default()
        },
        ..room("claude", Some(61))
    };
    let said = seasoned.means(12);
    assert!(said.contains("summoned it 12 of the 12 times"), "{said}");
    assert!(said.contains("measure the seat's latency"), "{said}");
    let fresh = room("kimi", None).means(12);
    assert!(fresh.contains("never summoned it"), "{fresh}");
    let bare = Summonable {
        record: AgentRecord {
            launched: 1,
            ..AgentRecord::default()
        },
        ..room("codex", None)
    };
    assert!(bare.means(1).contains("no task"), "{}", bare.means(1));
}

/// An option says what became of the work, not only how often it was
/// chosen. A count alone left the judgment naming an agent this ledger had
/// never summoned in eleven of seventeen disagreements (t-5873), because
/// "never summoned" and "summoned 265 times" read as equally neutral facts
/// when neither says what came back.
#[test]
fn an_option_says_what_became_of_the_work_this_ledger_gave_it() {
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
    let said = Summonable {
        record: claude.clone(),
        ..room("claude", Some(61))
    }
    .means(5);
    assert!(
        said.contains("Of the 3 of those whose work has ended"),
        "{said}"
    );
    assert!(said.contains("2 reached worker_done"), "{said}");
    assert!(said.contains("median 30 minutes"), "{said}");

    // An agent summoned once, still running, has a history and no record —
    // and says so rather than showing a zero that reads as a failure.
    let codex = folded.get("codex").expect("a record");
    assert_eq!((codex.launched, codex.carried), (1, 0));
    assert_eq!(codex.median_minutes, None);
    let said = Summonable {
        record: codex.clone(),
        ..room("codex", None)
    }
    .means(5);
    assert!(said.contains("None of that work has ended yet"), "{said}");
}
