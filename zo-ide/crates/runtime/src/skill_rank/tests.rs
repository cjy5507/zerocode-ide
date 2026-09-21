//! What the ranking promises: one question per skill over a shared state,
//! even shards with the offset mapping a split batch needs, a reply read only
//! when every rule holds, and a word match behind all of it.

use std::collections::BTreeMap;

use api::{SystemOneQuestionKind, SystemOneResponse, SystemOneUsage};
use serde_json::json;

use super::*;

fn entry(name: &str, description: &str) -> SkillIndexEntry {
    SkillIndexEntry::new(
        name.to_string(),
        (!description.is_empty()).then(|| description.to_string()),
        std::path::PathBuf::from(format!("/skills/{name}/SKILL.md")),
    )
}

fn catalog(count: usize) -> Vec<SkillIndexEntry> {
    (0..count)
        .map(|at| entry(&format!("skill-{at:03}"), &format!("does thing {at}")))
        .collect()
}

/// One score answer whose whole probability sits on `level`, with the legend
/// the contract echoes back.
fn answer(level: usize) -> serde_json::Value {
    let numbers: Vec<String> = (0..SKILL_LEVELS.len()).map(|n| n.to_string()).collect();
    let probabilities: serde_json::Map<String, serde_json::Value> = numbers
        .iter()
        .enumerate()
        .map(|(at, number)| (number.clone(), json!(if at == level { 1.0 } else { 0.0 })))
        .collect();
    let legend: serde_json::Map<String, serde_json::Value> = numbers
        .iter()
        .enumerate()
        .map(|(at, number)| (number.clone(), json!(SKILL_LEVELS[at])))
        .collect();
    json!({
        "type": "score",
        "score": f64::from(u8::try_from(level).expect("a short scale")),
        "confidence": 0.9,
        "legend": legend,
        "probabilities": probabilities,
    })
}

fn reply(levels: &BTreeMap<String, usize>) -> SystemOneResponse {
    SystemOneResponse {
        model: "jev-test".to_string(),
        answers: levels
            .iter()
            .map(|(id, level)| (id.clone(), answer(*level)))
            .collect(),
        usage: SystemOneUsage {
            input_tokens: 321,
            output_tokens: 12,
        },
    }
}

/// Every answer at `level`, for the questions this shard asks.
fn reply_at(shard: &[SkillCandidate], level: usize) -> SystemOneResponse {
    reply(
        &shard
            .iter()
            .map(|candidate| (candidate.question_id.clone(), level))
            .collect(),
    )
}

// ---- (a) the questions -----------------------------------------------------

#[test]
fn every_skill_gets_one_question_over_one_shared_task() {
    let skills = catalog(7);
    let candidates = skill_candidates(&skills);
    let shards = skill_shards(&candidates);
    assert_eq!(shards.len(), 1, "seven skills are one request");
    let questions = skill_questions(shards[0]);
    assert_eq!(questions.len(), skills.len());
    for (at, candidate) in candidates.iter().enumerate() {
        let question = questions
            .get(&candidate.question_id)
            .unwrap_or_else(|| panic!("no question for {}", candidate.name));
        assert_eq!(question.kind, SystemOneQuestionKind::Score);
        assert!(
            question.instructions.contains(&format!("skills[{at}]")),
            "{} names its place in the state: {}",
            candidate.name,
            question.instructions
        );
        // The id is for code; a question never sends one.
        assert!(!question.instructions.contains(&candidate.question_id));
    }
    // One state, read by every question in the shard.
    let state = skill_state("write a word document", shards[0]);
    assert_eq!(state["task"], json!("write a word document"));
    assert_eq!(
        state["skills"].as_array().map(Vec::len),
        Some(skills.len()),
        "one state carries the shard's whole list"
    );
}

/// The cut is the door's own, so the text a question carries and the text the
/// door would clear are the same text — a character cap cuts on a character
/// boundary and leaves no mark, which is what makes cutting an already cut
/// text a no-op.
#[test]
fn a_description_is_cut_at_the_tables_cap_by_the_doors_own_cutter() {
    let long = "가".repeat(SKILL_DESCRIPTION_CHAR_CAP * 2);
    let candidates = skill_candidates(&[entry("long", &long)]);
    let clipped = &candidates[0].description;
    assert_eq!(clipped.chars().count(), SKILL_DESCRIPTION_CHAR_CAP);
    assert_eq!(
        *clipped,
        zerocode_core::jev::door::cut(&long, Cap::Chars(SKILL_DESCRIPTION_CHAR_CAP)),
        "the question carries exactly what the door would clear"
    );
    // A description that fits is carried whole.
    let short = skill_candidates(&[entry("short", "a line")]);
    assert_eq!(short[0].description, "a line");
    // A skill with no description is still asked about.
    let none = skill_candidates(&[entry("bare", "")]);
    assert_eq!(none[0].description, "");
    assert_eq!(skill_questions(&none).len(), 1);
}

#[test]
fn the_task_is_cut_at_the_tables_cap() {
    let long = "가".repeat(SKILL_TASK_CHAR_CAP * 2);
    let state = skill_state(&long, &skill_candidates(&catalog(1)));
    let carried = state["task"].as_str().expect("a task");
    assert_eq!(carried.chars().count(), SKILL_TASK_CHAR_CAP);
    assert_eq!(
        carried,
        zerocode_core::jev::door::cut(&long, Cap::Chars(SKILL_TASK_CHAR_CAP))
    );
}

/// The design's own example: a hundred and one skills are three even requests,
/// and a question's id names the skill's place in the CATALOG while its
/// instructions name its place in THIS shard's state.
#[test]
fn a_split_batch_maps_an_id_in_the_catalog_to_a_place_in_its_shard() {
    let skills = catalog(101);
    let candidates = skill_candidates(&skills);
    let shards = skill_shards(&candidates);
    let sizes: Vec<usize> = shards.iter().map(|shard| shard.len()).collect();
    assert_eq!(sizes, vec![34, 34, 33]);

    let third = shards[2];
    assert_eq!(third[0].position, 68);
    let questions = skill_questions(third);
    assert!(
        questions["s68"].instructions.contains("skills[0]"),
        "the first skill of the third shard sits at the head of that shard's state"
    );
    assert!(questions["s100"].instructions.contains("skills[32]"));
    // No two shards answer under the same id.
    let mut ids: Vec<&str> = shards
        .iter()
        .flat_map(|shard| shard.iter().map(|c| c.question_id.as_str()))
        .collect();
    let asked = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), asked, "an id names one skill across the search");
}

// ---- (b) reading the answers ----------------------------------------------

#[test]
fn a_reading_under_the_floor_is_not_handed_back() {
    let candidates = skill_candidates(&catalog(3));
    let levels: BTreeMap<String, usize> = candidates
        .iter()
        .zip([0_usize, 1, 2])
        .map(|(candidate, level)| (candidate.question_id.clone(), level))
        .collect();
    let readings = validate_skills(&candidates, &reply(&levels)).expect("a well-formed reply");
    assert_eq!(readings.len(), 3, "every skill is read");
    let ranked = rank(readings);
    assert_eq!(
        ranked.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        vec!["skill-002"],
        "1.4 of 2 keeps only the level the reference build's line keeps"
    );
}

#[test]
fn a_ranking_falls_to_confidence_then_to_name() {
    let readings = vec![
        SkillReading { position: 0, name: "beta".into(), score: 2.0, normalised: 1.0, confidence: 0.5 },
        SkillReading { position: 1, name: "alpha".into(), score: 2.0, normalised: 1.0, confidence: 0.5 },
        SkillReading { position: 2, name: "gamma".into(), score: 2.0, normalised: 1.0, confidence: 0.9 },
        SkillReading { position: 3, name: "delta".into(), score: 1.5, normalised: 0.75, confidence: 1.0 },
    ];
    let ranked = rank(readings);
    assert_eq!(
        ranked.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        vec!["gamma", "alpha", "beta", "delta"]
    );
}

#[test]
fn a_malformed_answer_refuses_the_whole_shard_and_says_which_rule() {
    let candidates = skill_candidates(&catalog(3));
    let mut response = reply_at(&candidates, 2);
    response
        .answers
        .insert("s1".into(), json!({"type": "score", "score": 9.0}));
    let refused = validate_skills(&candidates, &response).expect_err("a broken answer");
    assert_eq!(refused.rule(), "not_a_score");
    assert_eq!(refused.position(), Some(1));

    // An answer nobody asked for says the reply is not about this shard.
    let mut stray = reply_at(&candidates, 2);
    stray.answers.insert("s99".into(), answer(2));
    let refused = validate_skills(&candidates, &stray).expect_err("a stray answer");
    assert_eq!(refused.rule(), "unknown_answer");
    assert_eq!(refused.position(), None);

    // A skill asked about and not answered.
    let mut missing = reply_at(&candidates, 2);
    missing.answers.remove("s2");
    let refused = validate_skills(&candidates, &missing).expect_err("a missing answer");
    assert_eq!(refused.rule(), "missing_answer");
    assert_eq!(refused.position(), Some(2));

    // An arithmetic failure carries the score rule's own word.
    let mut wrong = reply_at(&candidates, 2);
    wrong.answers.insert(
        "s0".into(),
        json!({
            "type": "score",
            "score": 0.0,
            "confidence": 0.9,
            "legend": {"0": SKILL_LEVELS[0], "1": SKILL_LEVELS[1], "2": SKILL_LEVELS[2]},
            "probabilities": {"0": 0.0, "1": 0.0, "2": 1.0},
        }),
    );
    assert_eq!(
        validate_skills(&candidates, &wrong)
            .expect_err("a mismatched score")
            .rule(),
        "score_mismatch"
    );
}

/// A shard's refusal leaves the other shards' rankings standing: the shards
/// are separate requests over disjoint skills.
#[test]
fn one_shards_refusal_leaves_the_others_alone() {
    let candidates = skill_candidates(&catalog(101));
    let shards = skill_shards(&candidates);
    let good = validate_skills(shards[0], &reply_at(shards[0], 2)).expect("the first shard");
    let mut broken = reply_at(shards[1], 2);
    broken.answers.remove(&shards[1][0].question_id);
    assert!(validate_skills(shards[1], &broken).is_err());
    assert_eq!(good.len(), 34);
    assert_eq!(rank(good).len(), 34);
}

// ---- (e) the word match behind it -----------------------------------------

#[test]
fn the_word_match_ranks_by_shared_vocabulary_and_invents_nothing() {
    let skills = vec![
        entry("docx", "Create and edit Word documents and templates"),
        entry("pdf", "Read, merge and split PDF files"),
        entry("dataviz", "Charts, graphs and dashboards"),
    ];
    let candidates = skill_candidates(&skills);
    let ranked = lexical_rank("edit a word document for the team", &candidates);
    assert_eq!(
        ranked.first().map(|r| r.name.as_str()),
        Some("docx"),
        "the skill sharing the task's words comes first: {ranked:?}"
    );
    assert!(
        ranked.iter().all(|r| r.name != "dataviz"),
        "a skill sharing no word is not ranked: {ranked:?}"
    );
    assert!(
        lexical_rank("", &candidates).is_empty(),
        "no task, no ranking"
    );
    assert!(
        lexical_rank("zzzz qqqq", &candidates).is_empty(),
        "nothing shared, nothing invented"
    );
    // Deterministic: the same catalog and task rank the same way twice.
    assert_eq!(
        lexical_rank("edit a word document for the team", &candidates),
        ranked
    );
}

// ---- (c) names, and the typos people make of them --------------------------

#[test]
fn a_name_resolves_whatever_its_case_and_separators() {
    let names: Vec<String> = ["anthropic-skills:docx", "second-brain", "dataviz"]
        .into_iter()
        .map(str::to_string)
        .collect();
    for asked in [
        "anthropic-skills:docx",
        "Anthropic Skills: DOCX",
        "anthropic_skills_docx",
        "ANTHROPICSKILLSDOCX",
    ] {
        assert_eq!(
            resolve_skill_names(&[asked.to_string()], &names),
            vec![NameMatch::Found("anthropic-skills:docx".to_string())],
            "{asked}"
        );
    }
}

#[test]
fn a_typo_comes_back_with_the_names_it_was_probably_meant_to_be() {
    let names: Vec<String> = ["second-brain", "dataviz", "code-review"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let NameMatch::Unknown { asked, suggestions } =
        resolve_skill_names(&["dataviznn".to_string()], &names)
            .pop()
            .expect("one answer")
    else {
        panic!("a name nothing answers to is unknown");
    };
    assert_eq!(asked, "dataviznn");
    assert_eq!(suggestions, vec!["dataviz".to_string()]);

    // A name nothing is close to is answered with no guess at all, rather
    // than with whatever happened to sort first.
    let NameMatch::Unknown { suggestions, .. } =
        resolve_skill_names(&["qqqqqqqqqq".to_string()], &names)
            .pop()
            .expect("one answer")
    else {
        panic!("unknown");
    };
    assert!(suggestions.is_empty(), "{suggestions:?}");
}
