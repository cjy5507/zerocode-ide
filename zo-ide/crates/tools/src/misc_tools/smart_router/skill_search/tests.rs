//! What the seat promises: one row per search, in the words every Jev counter
//! reads; a shard's refusal that leaves the others standing; and a word match
//! under a `routeUse` nobody can mistake for a judgment.

use serde_json::{json, Value};
use zerocode_core::jev::summary::{self, LEDGER_KEYS};
use zerocode_core::jev::SKILLS;

use super::*;

fn candidates(count: usize) -> Vec<SkillCandidate> {
    let skills: Vec<SkillIndexEntry> = (0..count)
        .map(|at| {
            SkillIndexEntry::new(
                format!("skill-{at:03}"),
                Some(format!("does thing {at}")),
                PathBuf::from(format!("/skills/skill-{at:03}/SKILL.md")),
            )
        })
        .collect();
    runtime::skill_rank::skill_candidates(&skills)
}

fn key(candidates: &[SkillCandidate]) -> MemoKey {
    MemoKey::for_search("a task", candidates, jev_gate::model_key(SYSTEMONE_MODEL))
}

fn reading(candidate: &SkillCandidate, score: f64) -> SkillReading {
    SkillReading {
        position: candidate.position,
        name: candidate.name.clone(),
        score,
        normalised: score / 2.0,
        confidence: 0.9,
    }
}

/// A row carries the columns every Jev counter reads, spelled the way they
/// are spelled: the recall ledger forked three of these into snake case once
/// and nobody saw it for a year of rows.
#[test]
fn a_row_is_spelled_the_way_every_jev_counter_reads_it() {
    let held = candidates(3);
    let mut row = SkillSearchRow::new(key(&held), 3, 1, SKILL_OUTCOME_ANSWERED.to_string());
    row.elapsed_ms = 410;
    row.input_tokens = Some(38_600);
    row.route_use = ROUTE_USE_APPLIED.to_string();
    row.chosen = vec![Chosen::from(&reading(&held[0], 2.0))];
    let value: Value = serde_json::to_value(&row).expect("a row serializes");

    for spelled in ["at", "outcome", "elapsedMs", "requests", "redactedLines", "inputTokens"] {
        assert!(value.get(spelled).is_some(), "a row says `{spelled}`: {value}");
    }
    // And says them in the canonical spelling the shared reader looks for
    // first, not in one of the spellings it only tolerates.
    for lkey in LEDGER_KEYS {
        if let Some(read) = lkey.read(&value) {
            assert_eq!(
                Some(read),
                value.get(lkey.canonical),
                "`{}` is read from a spelling this row does not write",
                lkey.canonical
            );
        }
    }
    assert_eq!(value["routeUse"], json!(ROUTE_USE_APPLIED));
    assert_eq!(value["chosen"][0]["name"], json!("skill-000"));
}

/// The seat's rows stand in its own ledger and `zo jev summary` counts them —
/// the seat is in the table, so the counter that walks the table finds it
/// without being told anything about skills.
#[test]
fn zo_jev_summary_counts_the_skill_seat_from_its_own_ledger() {
    let root = tempfile::tempdir().expect("a temp root");
    let held = candidates(2);
    let mut answered = SkillSearchRow::new(key(&held), 2, 1, SKILL_OUTCOME_ANSWERED.to_string());
    answered.at = 1_700_000_000_000;
    answered.elapsed_ms = 512;
    answered.input_tokens = Some(38_600);
    answered.requests = Some(1);
    answered.route_use = ROUTE_USE_APPLIED.to_string();
    let mut refused = SkillSearchRow::new(key(&held), 2, 1, "no_key".to_string());
    refused.at = 1_700_000_001_000;

    let ledger = root.path().join(SKILL_SEARCH_FILE);
    for row in [&answered, &refused] {
        append_shadow_row(&ledger, row, SHADOW_LEDGER_MAX_BYTES).expect("append");
    }
    let roots = vec![root.path().to_path_buf()];
    let report = super::super::jev_summary::report(
        &roots,
        None,
        Some(&json!({"smart": {SKILLS.setting: "shadow"}})),
        1_700_000_002_000,
        0,
    );
    let seat = report
        .iter()
        .find(|seat| seat.id == SKILLS.id)
        .expect("the skill seat is in the report");
    assert_eq!(seat.setting, SKILLS.setting);
    assert_eq!(seat.mode, JevMode::Shadow);
    assert_eq!(seat.found.as_deref(), Some(ledger.as_path()));
    assert_eq!(seat.week.rows, 2);
    assert_eq!(seat.week.answered, 1);
    assert_eq!(seat.week.refused, 1, "a keyless row says nothing about the seat");
    assert_eq!(seat.week.input_tokens, 38_600);
    assert_eq!(seat.rise_floor_permille, SKILLS.answer_floor_permille);
    // The judgment window is the one the table names, and it reads this
    // seat's rows without a reader written for it.
    assert!(summary::asked_something(&serde_json::to_value(&answered).expect("row")).is_some());
}

/// A shard that was refused takes its own skills out of the ranking and
/// nothing else: the other request asked about different skills.
#[test]
fn one_refused_shard_leaves_the_other_shards_ranking_standing() {
    let held = candidates(4);
    let (first, second) = held.split_at(2);
    let answers = vec![
        Shard {
            readings: Ok(vec![reading(&first[0], 2.0), reading(&first[1], 0.0)]),
            call: None,
            withheld: 0,
        },
        Shard {
            readings: Err(Refusal {
                outcome: "schema".to_string(),
                rejected: Some("score_mismatch".to_string()),
                rejected_at: Some(3),
            }),
            call: None,
            withheld: 0,
        },
    ];
    let (row, ranked) = fold(key(&held), &held, &[first, second], answers);
    assert_eq!(row.outcome, SKILL_OUTCOME_ANSWERED);
    assert_eq!(row.shards, 2);
    assert_eq!(row.shards_answered, 1);
    assert_eq!(row.under_floor, 1, "the skill under the floor is counted, not hidden");
    assert_eq!(
        ranked.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
        vec!["skill-000"]
    );
    assert_eq!(row.chosen.len(), 1);
}

/// Every shard refused: the row says the first refusal's word and which rule
/// broke, and nothing is ranked.
#[test]
fn a_search_nothing_answered_says_which_rule_refused_it() {
    let held = candidates(2);
    let answers = vec![Shard {
        readings: Err(Refusal {
            outcome: "schema".to_string(),
            rejected: Some("level_keys".to_string()),
            rejected_at: Some(1),
        }),
        call: None,
        withheld: 0,
    }];
    let (row, ranked) = fold(key(&held), &held, &[&held], answers);
    assert_eq!(row.outcome, "schema");
    assert_eq!(row.rejected.as_deref(), Some("level_keys"));
    assert_eq!(row.rejected_at, Some(1));
    assert_eq!(row.shards_answered, 0);
    assert!(ranked.is_empty());
}

/// The mark this seat's `auto` rises on: the turn loaded a skill the search
/// named, or it did not.
#[test]
fn the_agreed_mark_says_whether_the_turn_loaded_what_the_search_named() {
    let named: Vec<String> = ["dataviz", "docx", "pdf"].into_iter().map(str::to_string).collect();
    let request = SkillRequestName { task: 7, catalog: 11, at: 1_700_000_000_000 };
    let agreed = label_row("docx", &named, None, request);
    assert!(agreed.agreed);
    assert_eq!(agreed.rank, Some(1));
    let disagreed = label_row("second-brain", &named, None, request);
    assert!(!disagreed.agreed);
    assert_eq!(disagreed.rank, None);
    // The judge reads the mark off the row without knowing anything about
    // skills — it is the column every rising seat writes — and the request
    // it grades by the name and the time the row repeats (t-6877).
    let value = serde_json::to_value(&agreed).expect("a label row");
    assert_eq!(summary::AGREED.read(&value), Some(&json!(true)));
    assert_eq!(summary::LABEL.read(&value), Some(&json!("7:11")));
    assert_eq!(summary::REQUEST_AT.read(&value), Some(&json!(1_700_000_000_000_u64)));
    assert!(label_row("", &[], None, request).agreed, "no suggestion followed by no load is a negative match");
    assert!(!label_row("", &[String::from("docx")], None, request).agreed,
        "a suggestion ignored by the turn must be able to say no");
}

#[test]
fn a_loaded_skill_with_no_following_tool_or_body_quote_is_an_unused_proxy() {
    let load = ConversationMessage::assistant(vec![ContentBlock::ToolUse {
        id: "load-1".into(),
        name: "Skill".into(),
        input: json!({"skill":"docx"}).to_string(),
    }]);
    let body = ConversationMessage::tool_result(
        "load-1",
        "Skill",
        json!({"prompt":"Check the document layout before delivery.\nUse the template heading styles."}).to_string(),
        false,
    );
    let idle = [load.clone(), body.clone(), ConversationMessage::assistant(vec![
        ContentBlock::Text { text: "Done.".into() },
    ])];
    assert_eq!(skill_used_after_load(&idle, "docx"), Some(false));
    let quoted = [load.clone(), body, ConversationMessage::assistant(vec![
        ContentBlock::Text { text: "Check the document layout before delivery.".into() },
    ])];
    assert_eq!(skill_used_after_load(&quoted, "docx"), Some(true));
    let worked = [load, ConversationMessage::assistant(vec![ContentBlock::ToolUse {
        id: "read-1".into(), name: "read_file".into(), input: "{}".into(),
    }])];
    assert_eq!(skill_used_after_load(&worked, "docx"), Some(true));
    let only_more_loading = [
        idle[0].clone(),
        ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: "load-2".into(),
            name: "Skill".into(),
            input: json!({"skill":"pdf"}).to_string(),
        }]),
    ];
    assert_eq!(skill_used_after_load(&only_more_loading, "docx"), Some(false));
    assert_eq!(skill_used_after_load(&[], "docx"), None);

    let request = SkillRequestName { task: 1, catalog: 2, at: 3 };
    let label = label_turn(PendingSuggestion {
        generation: 0,
        suggested: Some("docx".into()),
        baseline: Some("docx".into()),
        loaded: Some("docx".into()),
        acting: false,
        judged: Some(request),
    }, request, &idle);
    assert!(label.agreed);
    assert_eq!(label.baseline_loaded.as_deref(), Some("docx"));
    assert_eq!(label.unused_load, Some(true));
    assert_eq!(label.baseline_unused_load, Some(true));
}

/// A recording turn does not wait for its judgment, so the judgment can land
/// after the turn has ended — or not at all. A turn whose entry was never
/// marked judged has nothing to compare its load with and gets no label; a
/// load seen before the judgment landed is still the turn's first load.
#[test]
fn a_turn_whose_judgment_never_landed_writes_no_label() {
    let root = tempfile::tempdir().expect("a temp root");
    let cwd = root.path().canonicalize().expect("a canonical root");
    turn_pending().lock().expect("pending lock").insert(
        cwd.clone(),
        PendingSuggestion { generation: 0, baseline: None, suggested: None, loaded: None, acting: false, judged: None },
    );
    note_loaded_skill(&cwd, "docx");
    let unjudged = turn_pending()
        .lock()
        .expect("pending lock")
        .remove(&cwd)
        .expect("the seated turn");
    assert_eq!(unjudged.loaded.as_deref(), Some("docx"), "the early load is kept");
    assert!(judged_label(unjudged, &[]).is_none(), "no judgment, no label");

    let request = SkillRequestName { task: 1, catalog: 2, at: 3 };
    let judged = PendingSuggestion {
        generation: 0,
        suggested: Some("docx".into()),
        baseline: Some("docx".into()),
        loaded: Some("docx".into()),
        acting: false,
        judged: Some(request),
    };
    let row = judged_label(judged, &[]).expect("the judged turn's label");
    assert!(row.agreed);
    assert_eq!(row.baseline_loaded.as_deref(), Some("docx"));
    assert_eq!((row.label.as_str(), row.request_at), ("1:2", 3), "the label names the judgment it grades");
}

#[test]
fn a_late_judgment_of_an_ended_turn_does_not_label_the_next_turn() {
    let root = tempfile::tempdir().expect("a temp root");
    let _env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", root.path().to_str().expect("UTF-8 temp root"));
    let cwd = root.path().canonicalize().expect("a canonical root");
    let late_before = LATE_SUGGESTION_JUDGMENTS.load(Ordering::Relaxed);
    let first = start_pending_suggestion(&cwd, false).expect("first turn");
    let (release, waiting) = tokio::sync::oneshot::channel::<()>();
    let (landed, done) = std::sync::mpsc::channel();
    let delayed_cwd = cwd.clone();
    super::super::patch_review::detach(async move {
        waiting.await.expect("release first judgment");
        mark_pending_suggestion_judged(&delayed_cwd, first, Some("docx"), SkillRequestName { task: 1, catalog: 2, at: 3 });
        landed.send(()).expect("signal settled judgment");
    });

    finish_turn_suggestion(&cwd, &[]);
    let next = start_pending_suggestion(&cwd, false).expect("next turn");
    note_loaded_skill(&cwd, "pdf");
    release.send(()).expect("release judgment");
    done.recv_timeout(Duration::from_secs(5)).expect("detached judgment completed");

    let pending = turn_pending().lock().expect("pending lock");
    let current = pending.get(&cwd).expect("next turn pending");
    assert_eq!(current.generation, next);
    assert!(current.judged.is_none(), "the next turn has received no judgment");
    assert!(current.suggested.is_none());
    assert_eq!(current.loaded.as_deref(), Some("pdf"));
    assert!(LATE_SUGGESTION_JUDGMENTS.load(Ordering::Relaxed) > late_before);
    drop(pending);
    finish_turn_suggestion(&cwd, &[]);
    assert!(!skill_suggestion_path(&cwd).exists(), "neither turn gets a label row");
}

#[test]
fn a_judgment_that_lands_in_its_own_turn_still_labels_it() {
    let root = tempfile::tempdir().expect("a temp root");
    let _env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", root.path().to_str().expect("UTF-8 temp root"));
    let cwd = root.path().canonicalize().expect("a canonical root");
    let generation = start_pending_suggestion(&cwd, false).expect("turn");
    let (release, waiting) = tokio::sync::oneshot::channel::<()>();
    let (landed, done) = std::sync::mpsc::channel();
    let delayed_cwd = cwd.clone();
    super::super::patch_review::detach(async move {
        waiting.await.expect("release judgment");
        mark_pending_suggestion_judged(&delayed_cwd, generation, Some("docx"), SkillRequestName { task: 1, catalog: 2, at: 3 });
        landed.send(()).expect("signal settled judgment");
    });

    note_loaded_skill(&cwd, "docx");
    release.send(()).expect("release judgment");
    done.recv_timeout(Duration::from_secs(5)).expect("detached judgment completed");
    finish_turn_suggestion(&cwd, &[]);
    let rows = super::super::jev_summary::read_rows(&skill_suggestion_path(&cwd));
    assert_eq!(rows.len(), 1, "the suggestion's label goes to the suggestion's ledger (t-6877)");
    assert_eq!(summary::AGREED.read(&rows[0]), Some(&json!(true)));
    assert!(!skill_search_path(&cwd).exists(), "and not to the search's");
}

/// The `agreed` mark shares a file with the rows it is about, so it has to be
/// invisible to every counter that counts requests — a label counted as a
/// request would halve the answer rate the seat is promoted on.
#[test]
fn a_label_row_is_not_counted_as_a_request() {
    let root = tempfile::tempdir().expect("a temp root");
    let held = candidates(1);
    let ledger = root.path().join(SKILL_SEARCH_FILE);
    // Three searches, each labelled by the load that followed it — a label
    // names the search it grades (t-6877), so three labels of one search
    // would be one comparison, the newest.
    for (n, loaded) in ["skill-000", "somewhere-else", "skill-000"].into_iter().enumerate() {
        let mut answered = SkillSearchRow::new(key(&held), 1, 1, SKILL_OUTCOME_ANSWERED.to_string());
        answered.at = 1_700_000_000_000 + n as u64;
        append_shadow_row(&ledger, &answered, SHADOW_LEDGER_MAX_BYTES).expect("the search row");
        append_shadow_row(
            &ledger,
            &label_row(loaded, &["skill-000".to_string()], None, SkillRequestName::of(&answered)),
            SHADOW_LEDGER_MAX_BYTES,
        )
        .expect("a label row");
    }
    let rows = super::super::jev_summary::read_rows(&ledger);
    assert_eq!(rows.len(), 6, "the file holds both kinds");
    let tally = summary::summarize(&rows, 0);
    assert_eq!(tally.rows, 3, "three requests, three labels");
    assert_eq!(tally.answered, 3);
    // And the judge still reads every mark.
    let judged = zerocode_core::jev::promote::judge_seat(&SKILLS, &rows).expect("a rising seat");
    assert_eq!(judged.agreement.compared, 3);
    assert_eq!(judged.agreement.agreed, 2);
}

/// The path is the use table's: a ledger the summary looks for by name.
#[test]
fn the_ledger_is_the_one_the_table_names() {
    assert_eq!(SKILL_SEARCH_FILE, SKILLS.ledger);
    let cwd = std::path::Path::new("/tmp/does-not-matter");
    assert!(skill_search_path(cwd).ends_with(SKILLS.ledger));
}

/// A search with nothing to ask about answers from the word match and never
/// writes a row: a question nobody could have asked is not evidence about the
/// seat.
#[test]
fn nothing_to_ask_is_answered_without_a_row() {
    let root = tempfile::tempdir().expect("a temp root");
    let searched = search(root.path(), "a task", &[]);
    assert_eq!(searched.route_use, ROUTE_USE_FALLBACK);
    assert_eq!(searched.outcome, NOTHING_TO_ASK);
    assert!(searched.ranked.is_empty());
    assert!(!searched.judged());
    assert!(
        searched.judged_names.is_none(),
        "no judgment, so nothing for a later load to agree with"
    );
    assert!(
        !skill_search_path(root.path()).exists(),
        "no row for a search that could not be made"
    );
}

/// What one search costs on the wire, for the catalog on the machine it is
/// run on — the number the design's measurement table carries.
///
/// Ignored by default: it discovers the catalog for the current directory (or
/// `ZO_SKILL_MEASURE_CWD`),
/// so what it prints is a fact about a machine rather than a promise about
/// the code. Nothing leaves — the request bodies are built and measured, not
/// sent.
///
/// `cargo test -p tools --lib skill_search_request_cost -- --ignored --nocapture`
#[test]
#[ignore = "a measurement of this machine's catalog, not a contract"]
fn skill_search_request_cost() {
    let cwd = std::env::var("ZO_SKILL_MEASURE_CWD").map_or_else(
        |_| std::env::current_dir().expect("cwd"),
        PathBuf::from,
    );
    let skills = runtime::discover_skills(&cwd);
    assert!(!skills.is_empty(), "no skills discovered");

    let held = runtime::skill_rank::skill_candidates(&skills);
    let shards = runtime::skill_rank::skill_shards(&held);
    let task = "find the skill that covers writing a Word document for the team";
    let mut total = 0;
    let started = std::time::Instant::now();
    for shard in &shards {
        let state = runtime::skill_rank::skill_state(task, shard);
        let questions = runtime::skill_rank::skill_questions(shard);
        let request = SystemOneRequest {
            state: &state,
            model: SYSTEMONE_MODEL,
            questions: &questions,
        };
        let body = jev_gate::body_of(&request).expect("a request serializes");
        let bytes = body.to_string().len();
        total += bytes;
        println!(
            "shard of {:2} skills: {bytes:6} bytes (~{} Jev input tokens at chars/4)",
            shard.len(),
            bytes / 4
        );
    }
    println!(
        "{} skills in {} request(s): {total} bytes total (~{} Jev input tokens), assembled in {:?}",
        held.len(),
        shards.len(),
        total / 4,
        started.elapsed()
    );
    let wide_state = runtime::skill_rank::wide_state(task, &held);
    let wide_questions = runtime::skill_rank::wide_questions(&held);
    let wide = jev_gate::body_of(&SystemOneRequest {
        state: &wide_state,
        model: SYSTEMONE_MODEL,
        questions: &wide_questions,
    }).expect("wide request");
    println!(
        "wide: state {} B (every skill's name and description), questions {} B (a place and a name an option)",
        wide["state"].to_string().len(), wide["questions"].to_string().len()
    );
    let details: Vec<runtime::skill_rank::SkillDetail> = skills.iter().take(
        zerocode_core::jev::SKILL_SUGGESTION_SHORTLIST
    ).enumerate().map(|(position, skill)| runtime::skill_rank::SkillDetail {
        position,
        excerpt: std::fs::read_to_string(&skill.path).unwrap_or_default(),
    }).collect();
    let narrow_state = runtime::skill_rank::narrow_state(task, &held, &details);
    let narrow_questions = runtime::skill_rank::narrow_questions(&held, &details);
    let narrow = jev_gate::body_of(&SystemOneRequest {
        state: &narrow_state,
        model: SYSTEMONE_MODEL,
        questions: &narrow_questions,
    }).expect("narrow request");
    let bytes = wide.to_string().len() + narrow.to_string().len();
    println!(
        "suggestion: {} skills, wide {} B, narrow {} B, both {} B (~{} tokens; 300 turns/day ~${:.5}/day at $0.042/M)",
        held.len(), wide.to_string().len(), narrow.to_string().len(), bytes, bytes / 4,
        f64::from(u32::try_from(bytes).expect("a bounded catalog request")) / 4.0
            * 300.0 * 0.042 / 1_000_000.0
    );
}

/* ---- two seats, two ledgers, two standings (t-6877 round 2) ------------------ */

/// A skills request row as either seat's writer files it, reduced to what
/// the judge reads: its name (the task and catalog fingerprints), its
/// rubric, the version that answered, and its outcome.
fn skills_request(at: u64, task: u64, rubric: u32) -> Value {
    json!({
        "at": at, "task": task, "catalog": 1, "rubricVersion": rubric, "outcome": SKILL_OUTCOME_ANSWERED,
        "elapsedMs": 400, "requests": 1, "model": "jev-1.13.0",
    })
}

/// A full window answered under `rubric`, then the marks that clear every
/// line of the seat, each naming a request of the window and the time it
/// was asked, as `label_row` names them.
fn skills_window_that_rises(seat: &zerocode_core::jev::JevUse, rubric: u32, first: u64, at: u64) -> Vec<Value> {
    use zerocode_core::jev::promote::{marks_that_can_clear, window_wanted_for};
    let wanted = u64::try_from(window_wanted_for(seat).expect("the seat rises")).expect("small");
    let misses = u64::try_from(seat.negatives_wanted.expect("negatives")).expect("small");
    let marks = u64::try_from(marks_that_can_clear(seat).expect("a width")).expect("small");
    let mut rows: Vec<Value> = (0..wanted).map(|n| skills_request(at + n, first + n, rubric)).collect();
    rows.extend((0..marks).map(|n| {
        json!({
            "at": at + wanted + n, "label": format!("{}:1", first + n), "requestAt": at + n, "loaded": "docx",
            "agreed": n >= misses, "baselineAgreed": n % 2 == 0,
        })
    }));
    rows
}

fn ledger_with(path: &Path, rows: &[Value]) {
    for row in rows {
        append_shadow_row(path, row, SHADOW_LEDGER_MAX_BYTES).expect("a row");
    }
}

#[test]
fn a_search_label_that_completes_the_evidence_records_its_rise() {
    let root = tempfile::tempdir().expect("a temp root");
    let _env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", root.path().to_str().expect("UTF-8 temp root"));
    let cwd = root.path().canonicalize().expect("a canonical root");
    let mut rows = skills_window_that_rises(&SKILLS, SKILLS.rubric_version, 1, 0);
    let last = rows.pop().expect("the last comparison");
    let asked = rows.iter().find(|row| row["at"] == last["requestAt"]).expect("the request");
    let request = SkillRequestName {
        task: asked["task"].as_u64().expect("task"),
        catalog: asked["catalog"].as_u64().expect("catalog"),
        at: asked["at"].as_u64().expect("at"),
    };
    ledger_with(&skill_search_path(&cwd), &rows);
    assert!(!runtime::jev_seat_applies(&cwd, &SKILLS));

    // The completing label marks the baseline as the comparison it replaces did.
    let baseline = if last["baselineAgreed"].as_bool() == Some(true) {
        [String::from("docx")]
    } else {
        [String::from("pdf")]
    };
    note_search_answer(&cwd, &[String::from("docx")], &baseline, 1, request);
    note_loaded_skill(&cwd, "docx");

    assert!(runtime::jev_seat_applies(&cwd, &SKILLS), "the writer must judge the evidence it completed");
    assert!(!runtime::jev_seat_applies(&cwd, &SKILL_SUGGESTION));
}

#[test]
fn a_suggestion_label_that_completes_the_evidence_records_its_own_rise() {
    let root = tempfile::tempdir().expect("a temp root");
    let _env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", root.path().to_str().expect("UTF-8 temp root"));
    let cwd = root.path().canonicalize().expect("a canonical root");
    let mut rows = skills_window_that_rises(&SKILL_SUGGESTION, SKILL_SUGGESTION.rubric_version, 1, 0);
    let last = rows.pop().expect("the last comparison");
    let asked = rows.iter().find(|row| row["at"] == last["requestAt"]).expect("the request");
    let request = SkillRequestName {
        task: asked["task"].as_u64().expect("task"),
        catalog: asked["catalog"].as_u64().expect("catalog"),
        at: asked["at"].as_u64().expect("at"),
    };
    ledger_with(&skill_suggestion_path(&cwd), &rows);
    assert!(!runtime::jev_seat_applies(&cwd, &SKILL_SUGGESTION));

    let generation = start_pending_suggestion(&cwd, false).expect("turn");
    mark_pending_suggestion_judged(&cwd, generation, Some("docx"), request);
    note_loaded_skill(&cwd, "docx");
    finish_turn_suggestion(&cwd, &[]);

    assert!(runtime::jev_seat_applies(&cwd, &SKILL_SUGGESTION), "the writer must judge its own seat");
    assert!(!runtime::jev_seat_applies(&cwd, &SKILLS));
}

#[test]
fn an_explicit_search_rechecks_settings_after_the_reply_and_before_reusing_a_memo() {
    use super::super::jev_mock::{machine, Mock};
    use std::sync::Arc;
    let mut failures = Vec::new();
    for (initial, change, applied) in [
        ("on", "none", true), ("on", "off", false), ("on", "shadow", false),
        ("on", "global_off", false), ("on", "consent", false), ("on", "model", false),
        ("on", "auto", false), ("shadow", "on", false),
    ] {
        let path = Arc::new(Mutex::new(None::<PathBuf>));
        let settings = Arc::clone(&path);
        let mock = Mock::answering(move |body| {
            let path = settings.lock().unwrap().clone().unwrap();
            let mut root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            match change {
                "none" => {},
                "global_off" => root["smart"]["jev"]["enabled"] = json!(false),
                "consent" => root["smart"]["jev"]["workspaces"] = json!([]),
                "model" => root["smart"]["jevModel"] = json!("jev-new-pin"),
                _ => root["smart"][SKILLS.setting] = json!(change),
            }
            std::fs::write(path, root.to_string()).unwrap();
            let request: Value = serde_json::from_str(body).unwrap();
            let legend: serde_json::Map<String, Value> = zerocode_core::jev::SKILL_LEVELS
                .iter().enumerate().map(|(i, text)| (i.to_string(), json!(text))).collect();
            let answers: serde_json::Map<String, Value> = request["questions"].as_object().unwrap()
                .keys().map(|id| (id.clone(), json!({"type":"score","score":2.0,"confidence":1.0,
                    "legend":legend,"probabilities":{"0":0.0,"1":0.0,"2":1.0}}))).collect();
            (200, json!({"model":"jev-test","answers":answers,
                "usage":{"input_tokens":10,"output_tokens":0}}).to_string())
        });
        machine(&SKILLS, initial, &mock.base_url, |cwd| {
            let settings_path = runtime::default_config_home().join("settings.json");
            *path.lock().unwrap() = Some(settings_path.clone());
            let original: Value = serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
            let skills = [SkillIndexEntry::new("docx".into(), Some("Write Word files".into()), cwd.join("SKILL.md"))];
            let task = format!("explicit settings regression {initial} {change}");
            let baseline = lexical_rank(&task, &skill_candidates(&skills));
            let searched = search_for_session(cwd, &task, &skills, Some("settings-session"));
            assert_eq!(searched.outcome, SKILL_OUTCOME_ANSWERED, "{initial}/{change}: {searched:?}");
            assert_eq!(mock.requests().len(), 1, "{initial}/{change}");
            let rows = super::super::jev_summary::read_rows(&skill_search_path(cwd));
            assert_eq!(rows.len(), 1);
            let correct_ranking = if applied {
                searched.ranked.first().is_some_and(|reading| reading.name == "docx")
            } else { searched.ranked == baseline };
            let correct = searched.judged() == applied && correct_ranking
                && (rows[0]["routeUse"] == ROUTE_USE_APPLIED) == applied;
            println!("explicit settings case {initial}/{change}: {correct}");
            if !correct { failures.push(format!("{initial}/{change}: {searched:?}")); }
            if change == "none" {
                // The exact successful ranking is now memoized. Turning off
                // either permission must also withhold that zero-request road.
                for revoke in ["global", "consent"] {
                    let mut root = original.clone();
                    if revoke == "global" { root["smart"]["jev"]["enabled"] = json!(false); }
                    else { root["smart"]["jev"]["workspaces"] = json!([]); }
                    std::fs::write(&settings_path, root.to_string()).unwrap();
                    let cached = search_for_session(cwd, &task, &skills, Some("settings-session"));
                    assert_eq!(mock.requests().len(), 1, "a withheld memo sends nothing");
                    let rows = super::super::jev_summary::read_rows(&skill_search_path(cwd));
                    assert_eq!(rows.last().unwrap()["cached"], true);
                    assert_eq!(rows.last().unwrap()["requests"], 0);
                    let correct = !cached.judged() && cached.ranked == baseline
                        && rows.last().unwrap()["routeUse"] != ROUTE_USE_APPLIED;
                    println!("explicit settings case memo/{revoke}: {correct}");
                    if !correct { failures.push(format!("memo/{revoke}: {cached:?}")); }
                }
            }
        });
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn an_in_flight_suggestion_cannot_outlive_its_settings() {
    use super::super::jev_mock::{machine_words, Mock};
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    for (change, stage, expected_requests, applied) in [
        ("none", 0, 2, true), ("provenance", 0, 2, true), ("off", 1, 1, false),
        ("shadow", 2, 2, false), ("global_off", 1, 1, false),
        ("model", 1, 1, false),
    ] {
        let path = Arc::new(Mutex::new(None::<PathBuf>));
        let settings = Arc::clone(&path);
        let requests = AtomicUsize::new(0);
        let mock = Mock::answering(move |body| {
            let number = requests.fetch_add(1, Ordering::SeqCst) + 1;
            if number == stage {
                let path = settings.lock().unwrap().clone().unwrap();
                let mut root: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                match change {
                    "global_off" => root["smart"]["jev"]["enabled"] = json!(false),
                    "model" => root["smart"]["jevModel"] = json!("jev-new-pin"),
                    _ => root["smart"][SKILL_SUGGESTION.setting] = json!(change),
                }
                std::fs::write(path, root.to_string()).unwrap();
            }
            let request: Value = serde_json::from_str(body).unwrap();
            let answers: serde_json::Map<String, Value> = request["questions"].as_object().unwrap()
                .keys().map(|id| {
                    let answer = if id == "which" {
                        let confidence = if change == "provenance" { 0.55 } else { 1.0 };
                        json!({"type":"choice","choice":"s0","confidence":confidence,
                            "probabilities":{"s0":confidence,"__no_skill__":1.0-confidence}})
                    } else { json!({"type":"noul","noul":if id == "prose_suffices" { 0.0 } else { 1.0 }}) };
                    (id.clone(), answer)
                }).collect();
            (200, json!({"model":"jev-test","answers":answers,
                "usage":{"input_tokens":10,"output_tokens":0}}).to_string())
        });
        machine_words(&[(SKILL_SUGGESTION.setting, "on")], &mock.base_url, |cwd| {
            *path.lock().unwrap() = Some(runtime::default_config_home().join("settings.json"));
            if change == "provenance" {
                let file = path.lock().unwrap().clone().unwrap();
                let mut root: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
                root["smart"]["jev"]["labelDrafts"] = json!(true);
                std::fs::write(file, root.to_string()).unwrap();
            }
            let skill = cwd.join(".zo/skills/docx");
            std::fs::create_dir_all(&skill).unwrap();
            std::fs::write(skill.join("SKILL.md"),
                "---\nname: docx\ndescription: Write Word documents.\n---\nCreate the Word file.\n").unwrap();
            let session = Arc::new(Mutex::new(None));
            let judge = SkillSuggestionJudge::at(cwd).with_session_source(Arc::clone(&session));
            *session.lock().unwrap() = Some("suggestion-session".to_string());
            let note = api::sync_bridge::run_blocking(judge.suggest("write a Word document".into()));
            assert_eq!(note.is_some(), applied, "{change}: {note:?}");
            assert_eq!(mock.requests().len(), expected_requests, "{change}");
            let rows = super::super::jev_summary::read_rows(&skill_suggestion_path(cwd));
            assert_eq!(rows.len(), 1, "{change}: {rows:?}");
            assert_eq!(rows[0]["routeUse"] == ROUTE_USE_APPLIED, applied, "{change}: {rows:?}");
            if change == "provenance" {
                let captured = zerocode_core::jev::learning::store::Store::at(&runtime::default_config_home()).snapshot().unwrap();
                let origin = zerocode_core::jev::learning::origin_group("zo/session", "suggestion-session").unwrap();
                assert_eq!(captured.cases.len(), 2, "both request stages retain the actual session origin");
                assert!(captured.cases.iter().all(|case| case.origin_group.as_ref() == Some(&origin)));
                assert!(mock.requests().iter().all(|body| !body.contains("suggestion-session") && !body.contains("originGroup")));
            }
            finish_turn_suggestion(cwd, &[]);
        });
    }
}

/// A rise decided on two questions at once stands for neither (t-6877
/// round 2, astra R3): the search's ledger holds the search's full window
/// and marks and the rise the judge wrote while the skills seat asked two
/// rubrics into one ledger — naming both. The search's cached `auto`, read
/// where the tool result and the prompt's index road read it, does not
/// act on it; a rise naming the search's own words alone does.
#[test]
fn a_rise_decided_on_two_questions_stands_for_neither() {
    use zerocode_core::jev::promote::ROSE;
    use zerocode_core::jev::summary::TRANSITION;
    let root = tempfile::tempdir().expect("a temp root");
    let _env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", root.path().to_str().expect("UTF-8 temp root"));
    let cwd = root.path().canonicalize().expect("a canonical root");
    let mut rows = skills_window_that_rises(&SKILLS, zerocode_core::jev::questions::SKILL_SEARCH_RUBRIC_VERSION, 1, 0);
    rows.push(json!({"at": 5_000, (TRANSITION.canonical): ROSE, "rubricVersions": [
        zerocode_core::jev::questions::SKILL_SEARCH_RUBRIC_VERSION,
        zerocode_core::jev::questions::SKILL_SUGGESTION_RUBRIC_VERSION,
    ]}));
    let ledger = skill_search_path(&cwd);
    ledger_with(&ledger, &rows);
    assert!(
        !runtime::jev_seat_applies(&cwd, &SKILLS),
        "a rise decided on the search's and the suggestion's words together is not the search's"
    );
    append_shadow_row(
        &ledger,
        &json!({"at": 6_000, (TRANSITION.canonical): ROSE, "rubricVersions": [zerocode_core::jev::questions::SKILL_SEARCH_RUBRIC_VERSION]}),
        SHADOW_LEDGER_MAX_BYTES,
    )
    .expect("a rise");
    assert!(runtime::jev_seat_applies(&cwd, &SKILLS), "a rise naming the search's words alone stands");
}

/// The suggestion rises on its own ledger and names its rubric, and the
/// search's ledger says nothing about it (t-6877 round 2, astra R3): the
/// search's window, marks and rise on the search's ledger and nothing on
/// the suggestion's — the suggestion's cached `auto`, read where the turn
/// boundary reads it, records. Its own window and marks on its own ledger
/// take it up through the one judge, and the rise it writes names its
/// words alone.
#[test]
fn the_suggestion_rises_on_its_own_ledger_and_stands_on_nothing_of_the_searchs() {
    use zerocode_core::jev::promote::{Verdict, ROSE};
    use zerocode_core::jev::summary::TRANSITION;
    let root = tempfile::tempdir().expect("a temp root");
    let _env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", root.path().to_str().expect("UTF-8 temp root"));
    let cwd = root.path().canonicalize().expect("a canonical root");
    let mut search = skills_window_that_rises(&SKILLS, SKILLS.rubric_version, 1, 0);
    search.push(json!({"at": 5_000, (TRANSITION.canonical): ROSE, "rubricVersions": [SKILLS.rubric_version]}));
    ledger_with(&skill_search_path(&cwd), &search);
    assert!(runtime::jev_seat_applies(&cwd, &SKILLS));
    assert!(
        !runtime::jev_seat_applies(&cwd, &SKILL_SUGGESTION),
        "the search's rise is not the suggestion's, and the suggestion has no ledger yet"
    );
    // Twenty of the suggestion's own: not a window, nothing judged.
    let thin: Vec<Value> = (0..20).map(|n| skills_request(10_000 + n, 1_000 + n, SKILL_SUGGESTION.rubric_version)).collect();
    let ledger = skill_suggestion_path(&cwd);
    ledger_with(&ledger, &thin);
    assert_eq!(super::super::shadow_ledger::judge_seat_ledger(&SKILL_SUGGESTION, &ledger, 99_999), None);
    assert!(!runtime::jev_seat_applies(&cwd, &SKILL_SUGGESTION));
    // Its own full window and marks: judged, and up.
    std::fs::remove_file(&ledger).expect("start over");
    ledger_with(&ledger, &skills_window_that_rises(&SKILL_SUGGESTION, SKILL_SUGGESTION.rubric_version, 1_000, 10_000));
    let verdict = super::super::shadow_ledger::judge_seat_ledger(&SKILL_SUGGESTION, &ledger, 99_999);
    assert_eq!(verdict, Some(Verdict::Rise), "the suggestion's own window and marks");
    let rows = super::super::jev_summary::read_rows(&ledger);
    let written: Vec<&Value> = rows.iter().filter(|row| TRANSITION.read(row).is_some()).collect();
    assert_eq!(written.len(), 1);
    assert_eq!(written[0]["rubricVersions"], json!([SKILL_SUGGESTION.rubric_version]), "the rise names the suggestion's words");
    assert!(runtime::jev_seat_applies(&cwd, &SKILL_SUGGESTION));
    assert!(runtime::jev_seat_applies(&cwd, &SKILLS), "and the search stands where it stood");
}

/// The search's ledger the two questions were written into before they
/// were two seats reads as the search's series from the last suggestion
/// row on (t-6877 round 2): the suggestion's rows are not the search's
/// requests, and the search's requests asked since are its window.
#[test]
fn a_legacy_mixed_ledger_reads_as_the_searchs_series_after_the_last_suggestion_row() {
    use zerocode_core::jev::promote::{judge_seat, on_the_newest_version, Line, Verdict};
    let mut rows: Vec<Value> = (0..20).map(|n| skills_request(n, n, zerocode_core::jev::questions::SKILL_SEARCH_RUBRIC_VERSION)).collect();
    rows.extend((20..40).map(|n| skills_request(n, n, zerocode_core::jev::questions::SKILL_SUGGESTION_RUBRIC_VERSION)));
    rows.extend((40..43).map(|n| skills_request(n, n, zerocode_core::jev::questions::SKILL_SEARCH_RUBRIC_VERSION)));
    assert_eq!(on_the_newest_version(&SKILLS, &rows).asked(), 3, "the search's requests since the last suggestion row");
    let judged = judge_seat(&SKILLS, &rows).expect("judged");
    assert!(matches!(judged.verdict, Verdict::Hold(Line::TooFewRows { rows: 3, .. })), "{judged:?}");
}

/// The suggestion keeps the word its person wrote for the search until they
/// write one of its own (t-6877 round 3, the coordinator's migration
/// contract m-8181), at the seat's own door — the turn-boundary judge a
/// session holds: a file that turned the search off by hand asks no
/// suggestion after the update (no turn seated, no request, no row); the
/// same file with the suggestion's own `auto` asks it; and a file with
/// neither word asks it as the search's recommendation did before the
/// split.
#[test]
fn the_suggestion_asks_nothing_where_its_person_turned_the_search_off() {
    use super::super::jev_mock::{machine_words, Mock};
    // Whether the turn was seated, how many requests left, and how many
    // rows the suggestion's ledger holds once its judgment has landed.
    let ask = |words: &[(&str, &str)]| -> (bool, usize, usize) {
        let mock = Mock::serving(200, "{}".to_string());
        machine_words(words, &mock.base_url, |cwd| {
            let skill = cwd.join(".zo").join("skills").join("docx");
            std::fs::create_dir_all(&skill).expect("a skill folder");
            std::fs::write(
                skill.join("SKILL.md"),
                "---\nname: docx\ndescription: Create and edit Word documents.\n---\n# docx\n",
            )
            .expect("a skill");
            let judge = SkillSuggestionJudge::at(cwd);
            let note = api::sync_bridge::run_blocking(judge.suggest("draft the quarterly report as a Word file".to_string()));
            assert_eq!(note, None, "a recording seat hands the turn nothing");
            let seated = turn_pending().lock().expect("pending lock").contains_key(cwd);
            let ledger = skill_suggestion_path(cwd);
            let started = Instant::now();
            while seated && !ledger.exists() && started.elapsed() < Duration::from_secs(15) {
                std::thread::sleep(Duration::from_millis(20));
            }
            let rows = super::super::jev_summary::read_rows(&ledger).len();
            finish_turn_suggestion(cwd, &[]);
            (seated, mock.requests().len(), rows)
        })
    };
    assert_eq!(
        ask(&[(SKILLS.setting, JevMode::Off.key())]),
        (false, 0, 0),
        "a search its person turned off asks no suggestion after the update"
    );
    let own = ask(&[(SKILLS.setting, JevMode::Off.key()), (SKILL_SUGGESTION.setting, JevMode::Auto.key())]);
    assert!(own.0 && own.1 >= 1 && own.2 >= 1, "the suggestion's own auto asks: {own:?}");
    let neither = ask(&[]);
    assert!(neither.0 && neither.1 >= 1 && neither.2 >= 1, "neither word: asked as before the split: {neither:?}");
}

#[test]
fn judging_a_skill_label_does_not_invent_a_missing_baseline() {
    use zerocode_core::jev::promote::{judge_seat, Line, Verdict};
    let root = tempfile::tempdir().expect("a temp root");
    let _env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", root.path().to_str().expect("UTF-8 temp root"));
    let cwd = root.path().canonicalize().expect("a canonical root");
    let mut rows = skills_window_that_rises(&SKILLS, SKILLS.rubric_version, 1, 0);
    for row in &mut rows {
        row.as_object_mut().expect("a ledger row").remove("baselineAgreed");
    }
    let last = rows.pop().expect("the last comparison");
    let asked = rows.iter().find(|row| row["at"] == last["requestAt"]).expect("the request");
    let request = SkillRequestName {
        task: asked["task"].as_u64().expect("task"),
        catalog: asked["catalog"].as_u64().expect("catalog"),
        at: asked["at"].as_u64().expect("at"),
    };
    let ledger = skill_search_path(&cwd);
    ledger_with(&ledger, &rows);
    note_search_answer(&cwd, &[String::from("docx")], &[String::from("docx")], 1, request);
    note_loaded_skill(&cwd, "docx");
    let recorded = super::super::jev_summary::read_rows(&ledger);
    assert!(matches!(judge_seat(&SKILLS, &recorded).expect("judged").verdict,
        Verdict::Hold(Line::TooFewBaseline { compared: 1, .. })));
    assert!(!runtime::jev_seat_applies(&cwd, &SKILLS));
}

#[test]
fn skill_search_labels_compare_the_word_match_on_the_same_loaded_skill() {
    let request = SkillRequestName { task: 1, catalog: 2, at: 3 };
    for (named, baseline, loaded, expected) in [
        (vec!["pdf".to_string()], vec!["docx".to_string()], "docx", (false, Some(true))),
        (vec!["docx".to_string()], vec!["pdf".to_string()], "docx", (true, Some(false))),
        (vec!["docx".to_string()], vec!["docx".to_string()], "docx", (true, Some(true))),
        (vec!["pdf".to_string()], vec!["pdf".to_string()], "docx", (false, Some(false))),
        (Vec::new(), Vec::new(), "", (true, Some(true))),
    ] {
        let label = label_row(loaded, &named, Some(&baseline), request);
        assert_eq!((label.agreed, label.baseline_agreed), expected);
    }
}

#[test]
fn skill_suggestion_labels_compare_the_lexical_first_choice_on_the_same_turn() {
    let request = SkillRequestName { task: 1, catalog: 2, at: 3 };
    for (suggested, baseline, loaded, expected) in [
        (Some("pdf"), Some("docx"), Some("docx"), (false, Some(true))),
        (Some("docx"), Some("pdf"), Some("docx"), (true, Some(false))),
        (Some("docx"), Some("docx"), Some("docx"), (true, Some(true))),
        (Some("pdf"), Some("pdf"), Some("docx"), (false, Some(false))),
        (None, None, None, (true, Some(true))),
    ] {
        let label = label_turn(PendingSuggestion {
            generation: 0, suggested: suggested.map(str::to_string),
            baseline: baseline.map(str::to_string), loaded: loaded.map(str::to_string),
            acting: true, judged: Some(request),
        }, request, &[]);
        assert_eq!((label.agreed, label.baseline_agreed), expected);
    }
}

#[test]
fn skill_search_cuts_both_rankings_at_the_requested_width() {
    let cwd = tempfile::tempdir().expect("project");
    let request = SkillRequestName { task: 1, catalog: 2, at: 3 };
    let named = vec!["docx".to_string(), "pdf".to_string()];
    let baseline = vec!["pdf".to_string(), "docx".to_string()];
    note_search_answer(cwd.path(), &named, &baseline, 1, request);
    let (named, baseline, request) = last_answer().lock().expect("answer book").remove(cwd.path()).expect("answer");
    let label = label_row("docx", &named, Some(&baseline), request);
    assert_eq!((label.agreed, label.baseline_agreed), (true, Some(false)), "the load below lexical k is a baseline miss");
}

#[test]
fn a_skill_load_labels_its_explicit_search_even_while_a_suggestion_waits() {
    let root = tempfile::tempdir().expect("home");
    let _env = crate::tests::EnvGuard::set("ZO_CONFIG_HOME", root.path().to_str().expect("UTF-8 home"));
    let cwd = root.path().canonicalize().expect("project");
    let request = SkillRequestName { task: 1, catalog: 2, at: 3 };
    start_pending_suggestion(&cwd, false).expect("turn");
    note_search_answer(&cwd, &["docx".to_string()], &["pdf".to_string()], 1, request);
    note_loaded_skill(&cwd, "docx");
    assert!(skill_search_path(&cwd).is_file(), "the pending suggestion swallowed the search label");
    finish_turn_suggestion(&cwd, &[]);
}

/// A narrow request that never got a reply — the wire timed out or the
/// service answered 503 — is a fallback, not a malformed reply: the judge
/// holds a seat from rising for any `schema` row in its window and forgives
/// one wire failure (t-19255). Every `schema` row the suggestion ledgers on
/// this machine carried a wire word under `rejected`.
#[test]
fn a_narrow_request_that_got_no_reply_is_not_written_as_a_refused_reply() {
    for wire in ["timeout", "http_503", "deadline"] {
        let mut row = SkillSearchRow::new(key(&candidates(1)), 1, 2, String::new());
        settle_narrow_failure(&mut row, NarrowFailure::Wire(wire.to_string()));
        assert_eq!(row.outcome, wire);
        assert!(!zerocode_core::jev::promote::names_a_schema_failure(&row.outcome));
        assert_eq!(row.rejected, None);
    }
    let mut row = SkillSearchRow::new(key(&candidates(1)), 1, 2, String::new());
    settle_narrow_failure(&mut row, NarrowFailure::Refused("score_mismatch".to_string()));
    assert_eq!(row.outcome, "schema");
    assert_eq!(row.rejected.as_deref(), Some("score_mismatch"));
}
