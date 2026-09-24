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
    let agreed = label_row("docx", &named);
    assert!(agreed.agreed);
    assert_eq!(agreed.rank, Some(1));
    let disagreed = label_row("second-brain", &named);
    assert!(!disagreed.agreed);
    assert_eq!(disagreed.rank, None);
    // The judge reads the mark off the row without knowing anything about
    // skills — it is the column every rising seat writes.
    let value = serde_json::to_value(&agreed).expect("a label row");
    assert_eq!(summary::AGREED.read(&value), Some(&json!(true)));
    assert!(label_row("", &[]).agreed, "no suggestion followed by no load is a negative match");
    assert!(!label_row("", &[String::from("docx")]).agreed,
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

    let label = label_turn(PendingSuggestion {
        generation: 0,
        suggested: Some("docx".into()),
        loaded: Some("docx".into()),
        acting: false,
        judged: true,
    }, &idle);
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
        PendingSuggestion { generation: 0, suggested: None, loaded: None, acting: false, judged: false },
    );
    note_loaded_skill(&cwd, "docx");
    let unjudged = turn_pending()
        .lock()
        .expect("pending lock")
        .remove(&cwd)
        .expect("the seated turn");
    assert_eq!(unjudged.loaded.as_deref(), Some("docx"), "the early load is kept");
    assert!(judged_label(unjudged, &[]).is_none(), "no judgment, no label");

    let judged = PendingSuggestion {
        generation: 0,
        suggested: Some("docx".into()),
        loaded: Some("docx".into()),
        acting: false,
        judged: true,
    };
    let row = judged_label(judged, &[]).expect("the judged turn's label");
    assert!(row.agreed);
    assert_eq!(row.baseline_loaded.as_deref(), Some("docx"));
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
        mark_pending_suggestion_judged(&delayed_cwd, first, Some("docx"));
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
    assert!(!current.judged, "the next turn has received no judgment");
    assert!(current.suggested.is_none());
    assert_eq!(current.loaded.as_deref(), Some("pdf"));
    assert!(LATE_SUGGESTION_JUDGMENTS.load(Ordering::Relaxed) > late_before);
    drop(pending);
    finish_turn_suggestion(&cwd, &[]);
    assert!(!skill_search_path(&cwd).exists(), "neither turn gets a label row");
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
        mark_pending_suggestion_judged(&delayed_cwd, generation, Some("docx"));
        landed.send(()).expect("signal settled judgment");
    });

    note_loaded_skill(&cwd, "docx");
    release.send(()).expect("release judgment");
    done.recv_timeout(Duration::from_secs(5)).expect("detached judgment completed");
    finish_turn_suggestion(&cwd, &[]);
    let rows = super::super::jev_summary::read_rows(&skill_search_path(&cwd));
    assert_eq!(rows.len(), 1);
    assert_eq!(summary::AGREED.read(&rows[0]), Some(&json!(true)));
}

/// The `agreed` mark shares a file with the rows it is about, so it has to be
/// invisible to every counter that counts requests — a label counted as a
/// request would halve the answer rate the seat is promoted on.
#[test]
fn a_label_row_is_not_counted_as_a_request() {
    let root = tempfile::tempdir().expect("a temp root");
    let held = candidates(1);
    let mut answered = SkillSearchRow::new(key(&held), 1, 1, SKILL_OUTCOME_ANSWERED.to_string());
    answered.at = 1_700_000_000_000;
    let ledger = root.path().join(SKILL_SEARCH_FILE);
    append_shadow_row(&ledger, &answered, SHADOW_LEDGER_MAX_BYTES).expect("the search row");
    for loaded in ["skill-000", "somewhere-else", "skill-000"] {
        append_shadow_row(
            &ledger,
            &label_row(loaded, &["skill-000".to_string()]),
            SHADOW_LEDGER_MAX_BYTES,
        )
        .expect("a label row");
    }
    let rows = super::super::jev_summary::read_rows(&ledger);
    assert_eq!(rows.len(), 4, "the file holds both kinds");
    let tally = summary::summarize(&rows, 0);
    assert_eq!(tally.rows, 1, "one request, three labels");
    assert_eq!(tally.answered, 1);
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
    let duplicate_state = runtime::skill_rank::skill_state(task, &held);
    let duplicate_wide = jev_gate::body_of(&SystemOneRequest {
        state: &duplicate_state,
        model: SYSTEMONE_MODEL,
        questions: &wide_questions,
    }).expect("duplicate catalog request");
    println!(
        "wide state once: {} B versus catalog duplicated: {} B",
        wide.to_string().len(), duplicate_wide.to_string().len()
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
