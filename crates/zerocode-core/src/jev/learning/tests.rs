use super::*;
use crate::jev::{AGENT_TOOL, door};
use serde_json::json;

const WORKSPACE: &str = if cfg!(windows) { r"C:\work\project" } else { "/work/project" };

const WORKSPACE_ONE: &str = if cfg!(windows) { r"C:\work\one" } else { "/work/one" };
const WORKSPACE_TWO: &str = if cfg!(windows) { r"C:\work\two" } else { "/work/two" };

fn case_with(seat: &JevUse, state: &str, questions: Value, answers: Value, model: &str) -> Case {
    let settings =
        door::JevSettings::from_root(&json!({"smart":{"jev":{"enabled":true,"workspaces":["*"]}}}));
    let cleared = door::may_send(
        seat,
        &door::Asking {
            key: true,
            settings: &settings,
            workspace: Some(WORKSPACE),
            sent_today: 0,
        },
        json!({"model":"jev-latest","state":{"context":state},"questions":questions}),
    )
    .unwrap();
    Case::from_cleared(
        seat,
        &cleared,
        &json!({"model":model,"answers":answers}),
        10,
    )
    .unwrap()
}

fn case(state: &str, yes: f64) -> Case {
    case_with(
        &AGENT_TOOL,
        state,
        json!({"q":{"type":"noul","instructions":"Is the result useful?"}}),
        json!({"q":{"type":"noul","noul":yes}}),
        "jev-test-1",
    )
}

fn outcome(case: &Case, question: &str) -> Outcome {
    Outcome {
        case_id: case.id.clone(),
        question: question.into(),
        at: 20,
        correct: true,
        reviewer: Reviewer::Human,
        note: "Checked the original result.".into(),
    }
}

#[test]
fn local_origins_bind_case_identity_and_keep_distinct_states_of_a_session_together() {
    let original = case("first state", 0.5);
    let first = original.clone().with_origin("zo/session", "one").unwrap();
    let later = case("another state", 0.7)
        .with_origin("zo/session", "one")
        .unwrap();
    let other = original.clone().with_origin("zo/session", "two").unwrap();
    assert_ne!(
        first.group, later.group,
        "state duplicate identity stays separate"
    );
    assert_eq!(first.origin_group, later.origin_group);
    assert_ne!(first.origin_group, other.origin_group);
    assert_ne!(original.id, first.id);
    assert_ne!(first.id, other.id);
    assert_eq!(
        first.request, original.request,
        "local provenance never enters the wire"
    );
    let mut modified = first;
    modified.origin_group = other.origin_group;
    assert!(modified.validate().is_err());
}

#[test]
fn uncertainty_and_random_audits_are_disjoint_and_repeatable() {
    let cases = (0..50)
        .map(|at| case(&at.to_string(), if at < 4 { 0.5 } else { 1.0 }))
        .collect::<Vec<_>>();
    let selected = select(&cases, &[], 5, 1, 42).unwrap();
    assert_eq!(selected, select(&cases, &[], 5, 1, 42).unwrap());
    assert_eq!(
        selected
            .iter()
            .filter(|row| row.reason == Reason::Uncertain)
            .count(),
        4
    );
    assert!(
        selected[..4]
            .iter()
            .all(|row| row.case.answers["q"]["noul"] == 0.5)
    );
    assert_eq!(
        selected[4].case.answers["q"]["noul"], 1.0,
        "a confident answer is still audited"
    );
    assert_eq!(
        selected
            .iter()
            .map(|row| &row.case.id)
            .collect::<HashSet<_>>()
            .len(),
        5
    );
}

#[test]
fn case_identity_binds_the_question_model_and_rubric_but_not_the_observation_clock() {
    let original = case("one", 0.7);
    let mut later = original.clone();
    later.at += 1;
    assert!(later.validate().is_ok());
    let mut changed = original.clone();
    changed.model = "jev-test-2".into();
    assert_eq!(changed.validate(), Err(Invalid::Identity));
    let mut changed = original.clone();
    changed.rubric_version += 1;
    assert_eq!(changed.validate(), Err(Invalid::Identity));
    let different_answer = case("one", 0.3);
    assert_eq!(
        original.group, different_answer.group,
        "a changed answer is not a new state split"
    );
    assert_ne!(original.id, different_answer.id);
}

#[test]
fn a_completed_review_is_excluded_but_an_invalid_review_hides_nothing() {
    let cases = vec![case("one", 0.5), case("two", 0.7)];
    let mut invalid = outcome(&cases[1], "missing");
    invalid.at = 0;
    let selected = select(&cases, &[outcome(&cases[0], "q"), invalid], 10, 2, 1).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].case.id, cases[1].id);
}

#[test]
fn partial_reviews_keep_the_unreviewed_question_and_duplicate_cases_do_not_fill_a_batch() {
    let one = case_with(
        &AGENT_TOOL,
        "two questions",
        json!({"a":{"type":"noul"},"b":{"type":"noul"}}),
        json!({"a":{"type":"noul","noul":0.5},"b":{"type":"noul","noul":0.8}}),
        "jev-test",
    );
    let mut later = one.clone();
    later.at = 30;
    let cases = [later, one.clone()];
    let selected = select(&cases, &[outcome(&one, "a")], 10, 2, 0).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].case.at, 10);
    assert_eq!(selected[0].remaining, ["b"]);
}

#[test]
fn imported_modified_evidence_never_receives_a_review_slot() {
    let mut changed = case("one", 0.5);
    changed.answers["q"]["noul"] = json!(0.9);
    assert!(select(&[changed], &[], 5, 1, 0).unwrap().is_empty());
}

#[test]
fn a_reworded_question_on_the_same_state_stays_in_the_same_evaluation_group() {
    let first = case("same evidence", 0.5);
    let changed = case_with(
        &AGENT_TOOL,
        "same evidence",
        json!({"q":{"type":"noul","instructions":"Does this evidence support the next step?"}}),
        json!({"q":{"type":"noul","noul":0.8}}),
        "jev-test-2",
    );
    assert_eq!(
        first.group, changed.group,
        "question or model changes cannot make held-out evidence new"
    );
    assert_ne!(
        first.id, changed.id,
        "a review still belongs to its exact question and answer"
    );
}

#[test]
fn a_saved_case_has_only_the_state_the_door_cleared() {
    let case = case(
        "ordinary text\nAuthorization: Bearer sample-private-value\nlast line",
        0.5,
    );
    let body = case.request.to_string();
    assert!(!body.contains("sample-private-value"));
    assert!(body.contains("ordinary text"));
}

#[test]
fn malformed_scores_use_the_same_rules_as_the_runtime() {
    let score = json!({"type":"score","score":0.0,"confidence":0.9,
        "legend":{"0":"low","1":"high"},"probabilities":{"0":0.0,"1":1.0}});
    assert_eq!(
        super::super::score::read_value(&score, 2),
        Err(super::super::score::ScoreRule::ScoreMismatch)
    );
    let questions = json!({"q":{"type":"score","criteria":["low","high"]}});
    assert_eq!(
        certainty(&questions, &json!({"q":score}), "q"),
        Err(Invalid::Answer)
    );
}

#[test]
fn zero_oversized_and_overallocated_batches_are_refused() {
    for (limit, audit) in [(0, 0), (MAX_BATCH + 1, 1), (2, 3)] {
        assert_eq!(select(&[], &[], limit, audit, 0), Err(Invalid::Batch));
    }
}

#[test]
fn negative_zero_confidence_is_as_uncertain_as_zero() {
    let zero = case_with(
        &AGENT_TOOL,
        "uncertain choice",
        json!({"q":{"type":"choice","criteria":{"a":"first","b":"second"}}}),
        json!({"q":{"type":"choice","choice":"a","probabilities":{"a":0.5,"b":0.5},"confidence":-0.0}}),
        "jev-test",
    );
    let cases = [case("confident fact", 0.9), zero];
    let chosen = select(&cases, &[], 1, 0, 0).unwrap();
    assert_eq!(chosen[0].case.id, cases[1].id);
}

#[test]
fn stored_cases_and_outcomes_round_trip_without_duplicating_a_retry() {
    let home = tempfile::tempdir().unwrap();
    let store = store::Store::at(home.path());
    let one = case("a result", 0.5);
    assert_eq!(store.record(&one).unwrap(), store::Recorded::Saved);
    assert_eq!(store.record(&one).unwrap(), store::Recorded::Existing);
    let reviewed = outcome(&one, "q");
    store.outcome(reviewed.clone()).unwrap();
    store.outcome(reviewed.clone()).unwrap();
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.cases, [one]);
    assert_eq!(snapshot.outcomes, [reviewed]);
    assert_eq!(snapshot.invalid, 0);
}

#[test]
fn workspace_provenance_binds_the_case_but_does_not_split_identical_evidence_groups() {
    let original = case("shared evidence", 0.5);
    let response = json!({"model":original.model,"answers":original.answers});
    let first = Case::from_request(
        &original.seat,
        original.rubric_version,
        original.request.clone(),
        &response,
        original.at,
        Some(WORKSPACE_ONE.into()),
    )
    .unwrap();
    let mut second = Case::from_request(
        &original.seat,
        original.rubric_version,
        original.request,
        &response,
        original.at,
        Some(WORKSPACE_TWO.into()),
    )
    .unwrap();
    assert_ne!(
        first.id, second.id,
        "an outcome belongs to its source workspace"
    );
    assert_eq!(
        first.group, second.group,
        "the same input must remain in one evaluation split"
    );
    second.workspace = first.workspace;
    assert_eq!(second.validate(), Err(Invalid::Identity));
    assert!(
        second
            .with_workspace(std::path::Path::new(WORKSPACE_ONE))
            .is_err(),
        "binding a workspace must not launder already-modified evidence"
    );
}

#[test]
fn streaming_review_selects_the_same_evidence_and_respects_its_workspace() {
    let home = tempfile::tempdir().unwrap();
    let store = store::Store::at(home.path());
    for index in 0..16 {
        let mut one = case(
            &format!("result {index}"),
            if index % 3 == 0 { 0.5 } else { 1.0 },
        );
        one.workspace = Some(
            if index % 2 == 0 {
                WORKSPACE_ONE
            } else {
                WORKSPACE_TWO
            }
            .into(),
        );
        one.id = one.identity().unwrap();
        store.record(&one).unwrap();
        if index % 4 == 0 {
            store.outcome(outcome(&one, "q")).unwrap();
        }
    }
    std::fs::write(
        home.path()
            .join("jev/review")
            .join(format!("{}.json", "a".repeat(64))),
        b"{torn",
    )
    .unwrap();
    let snapshot = store.snapshot().unwrap();
    for (limit, audit) in [(1, 0), (5, 1), (5, 5), (100, 10)] {
        let expected = select(&snapshot.cases, &snapshot.outcomes, limit, audit, 42).unwrap();
        let review = store.review(None, None, limit, audit, 42).unwrap();
        assert_eq!(
            serde_json::to_value(review.samples).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        assert_eq!((review.total, review.cases, review.invalid), (17, 16, 1));
    }
    let scoped = store
        .review(Some(AGENT_TOOL.id), Some(WORKSPACE_ONE), 5, 1, 42)
        .unwrap();
    assert_eq!((scoped.total, scoped.cases, scoped.invalid), (17, 8, 1));
    assert_eq!(
        scoped.samples.len(),
        4,
        "four of this workspace's cases were already reviewed"
    );
    assert!(
        scoped
            .samples
            .iter()
            .all(|sample| sample.case.workspace.as_deref() == Some(WORKSPACE_ONE))
    );
    assert!(store.review(None, None, 0, 0, 42).is_err());
}

#[test]
fn an_old_or_unrelated_outcome_cannot_replace_the_review() {
    let home = tempfile::tempdir().unwrap();
    let store = store::Store::at(home.path());
    let one = case("a result", 0.5);
    store.record(&one).unwrap();
    let reviewed = outcome(&one, "q");
    store.outcome(reviewed.clone()).unwrap();
    let mut stale = reviewed.clone();
    stale.correct = false;
    stale.at -= 1;
    assert!(store.outcome(stale).is_err());
    let mut wrong = reviewed.clone();
    wrong.question = "unasked".into();
    assert!(store.outcome(wrong).is_err());
    assert_eq!(store.snapshot().unwrap().outcomes, [reviewed]);
}

#[test]
fn human_agent_and_execution_reviews_keep_their_own_labels_and_clocks() {
    let home = tempfile::tempdir().unwrap();
    let store = store::Store::at(home.path());
    let one = case("a result", 0.5);
    store.record(&one).unwrap();
    let human = outcome(&one, "q");
    store.outcome(human.clone()).unwrap();
    let agent = Outcome {
        reviewer: Reviewer::Agent,
        at: 15,
        correct: false,
        ..human.clone()
    };
    let execution = Outcome {
        reviewer: Reviewer::Execution,
        at: 16,
        ..human.clone()
    };
    store.outcome(agent.clone()).unwrap();
    store.outcome(execution.clone()).unwrap();
    assert_eq!(
        store.snapshot().unwrap().outcomes,
        [human.clone(), agent.clone(), execution.clone()]
    );
    let changed = Outcome {
        at: 21,
        correct: false,
        ..human.clone()
    };
    store.outcome(changed.clone()).unwrap();
    assert!(store.outcome(human).is_err());
    assert_eq!(
        store.snapshot().unwrap().outcomes,
        [changed, agent, execution]
    );
}

#[test]
fn archiving_preserves_all_reviewers_and_releases_capacity_without_overwriting_a_file() {
    let home = tempfile::tempdir().unwrap();
    let exports = tempfile::tempdir().unwrap();
    let store = store::Store::at(home.path());
    let one = case("reviewed evidence", 0.5);
    store.record(&one).unwrap();
    let human = outcome(&one, "q");
    let agent = Outcome {
        reviewer: Reviewer::Agent,
        at: 21,
        correct: false,
        ..human.clone()
    };
    store.outcome(human.clone()).unwrap();
    store.outcome(agent.clone()).unwrap();
    let directory = home.path().join("jev/review");
    for index in 1..store::MAX_CASES {
        std::fs::write(
            directory.join(format!("{index:064x}.json")),
            "broken record",
        )
        .unwrap();
    }
    let next = case("new model evidence", 0.5);
    assert_eq!(store.record(&next).unwrap(), store::Recorded::Full);
    let path = exports.path().join("archive.json");
    let archived = store
        .archive(&path, Some(&one.seat), Some(one.rubric_version), false)
        .unwrap();
    assert_eq!(
        (archived.archived, archived.retired, archived.invalid),
        (1, 1, store::MAX_CASES - 1)
    );
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["cases"][0]["id"], one.id);
    let labels: Vec<Outcome> = serde_json::from_value(saved["outcomes"].clone()).unwrap();
    assert_eq!(labels, [human, agent]);
    assert_eq!(store.record(&next).unwrap(), store::Recorded::Saved);
    assert!(
        store.archive(&path, None, None, true).is_err(),
        "an existing archive is not replaced"
    );
    assert!(directory.join(format!("{}.json", next.id)).exists());
}

#[test]
fn unreviewed_evidence_requires_an_explicit_archive_choice_and_failed_exports_remove_nothing() {
    let home = tempfile::tempdir().unwrap();
    let exports = tempfile::tempdir().unwrap();
    let store = store::Store::at(home.path());
    let one = case("awaiting review", 0.5);
    store.record(&one).unwrap();
    assert_eq!(
        store
            .archive(&exports.path().join("reviewed.json"), None, None, false)
            .unwrap()
            .retired,
        0
    );
    assert!(store.archive(exports.path(), None, None, true).is_err());
    assert_eq!(store.snapshot().unwrap().cases.len(), 1);
    assert_eq!(
        store
            .archive(&exports.path().join("all.json"), None, None, true)
            .unwrap()
            .retired,
        1
    );
    assert!(store.snapshot().unwrap().cases.is_empty());
}

fn prepare(home: &std::path::Path) -> store::Pending {
    let settings =
        door::JevSettings::from_root(&json!({"smart":{"jev":{"enabled":true,"workspaces":["*"]}}}));
    let cleared = door::may_send(
        &AGENT_TOOL,
        &door::Asking {
            key: true,
            settings: &settings,
            workspace: Some(WORKSPACE),
            sent_today: 0,
        },
        json!({"state":{"context":"a result"},"questions":{"q":{"type":"noul"}}}),
    )
    .unwrap();
    cleared
        .with_review(home, &AGENT_TOOL, WORKSPACE)
        .with_review_origin("zo/session", "session-one")
        .prepare_review()
        .unwrap()
}

#[test]
fn existing_settings_disable_retention_even_after_the_request_was_prepared() {
    for root in [
        json!({"smart":{"jev":{"enabled":false,"labelDrafts":true,"workspaces":["*"]}}}),
        json!({"smart":{"jev":{"enabled":true,"labelDrafts":false,"workspaces":["*"]}}}),
        json!({"smart":{"agentTool":"off","jev":{"enabled":true,"labelDrafts":true,"workspaces":["*"]}}}),
        json!({"smart":{"jev":{"enabled":true,"labelDrafts":true,"workspaces":[]}}}),
    ] {
        let home = tempfile::tempdir().unwrap();
        let pending = prepare(home.path());
        std::fs::write(home.path().join("settings.json"), root.to_string()).unwrap();
        assert_eq!(
            pending
                .finish(
                    &json!({"model":"jev-test","answers":{"q":{"type":"noul","noul":0.5}}}),
                    10
                )
                .unwrap(),
            None
        );
        assert!(
            !home.path().join("jev/review").exists(),
            "off must not create a store"
        );
    }
}

#[test]
fn recording_and_applying_both_collect_evidence_under_the_existing_preference() {
    for mode in ["shadow", "on"] {
        let home = tempfile::tempdir().unwrap();
        let root = json!({"smart":{"agentTool":mode,"jev":{"enabled":true,"labelDrafts":true,"workspaces":["*"]}}});
        std::fs::write(home.path().join("settings.json"), root.to_string()).unwrap();
        let pending = prepare(home.path());
        assert_eq!(
            pending
                .finish(
                    &json!({"model":"jev-test","answers":{"q":{"type":"noul","noul":0.5}}}),
                    10
                )
                .unwrap(),
            Some(store::Recorded::Saved)
        );
        let cases = store::Store::at(home.path()).snapshot().unwrap().cases;
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].workspace.as_deref(), Some(WORKSPACE));
        assert_eq!(
            cases[0].origin_group,
            Some(origin_group("zo/session", "session-one").unwrap())
        );
        assert!(!cases[0].request.to_string().contains("session-one"));
        assert!(
            cases[0].request.get("workspace").is_none(),
            "local provenance is not a wire field"
        );
    }
}

#[test]
fn the_case_size_limit_does_not_reject_the_persons_unrelated_or_managed_settings() {
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    let root = json!({"unrelated": "x".repeat(MAX_CASE_BYTES * 2),
        "smart":{"agentTool":"on","jev":{"enabled":true,"labelDrafts":true,"workspaces":["*"]}}});
    std::fs::write(&settings, root.to_string()).unwrap();
    let response = json!({"model":"jev-test","answers":{"q":{"type":"noul","noul":0.5}}});
    assert_eq!(
        prepare(home.path()).finish(&response, 10).unwrap(),
        Some(store::Recorded::Saved)
    );
    #[cfg(unix)]
    {
        let managed = home.path().join("managed-settings.json");
        std::fs::rename(&settings, &managed).unwrap();
        std::os::unix::fs::symlink(&managed, &settings).unwrap();
        assert_eq!(
            prepare(home.path()).finish(&response, 10).unwrap(),
            Some(store::Recorded::Existing)
        );
    }
}

#[test]
fn a_missing_draft_preference_prepares_no_copy_at_the_door() {
    let settings = door::JevSettings::from_root(&json!({"smart":{"jev":{"workspaces":["*"]}}}));
    let cleared = door::may_send(
        &AGENT_TOOL,
        &door::Asking {
            key: true,
            settings: &settings,
            workspace: Some(WORKSPACE),
            sent_today: 0,
        },
        json!({"state":{},"questions":{}}),
    )
    .unwrap();
    assert!(cleared.prepare_review().is_none());
}
