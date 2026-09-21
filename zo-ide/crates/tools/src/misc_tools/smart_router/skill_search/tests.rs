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
    MemoKey::for_search("a task", candidates)
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
/// Ignored by default: it reads `~/.zo/skills` (or `ZO_SKILL_MEASURE_ROOT`),
/// so what it prints is a fact about a machine rather than a promise about
/// the code. Nothing leaves — the request bodies are built and measured, not
/// sent.
///
/// `cargo test -p tools --lib skill_search_request_cost -- --ignored --nocapture`
#[test]
#[ignore = "a measurement of this machine's catalog, not a contract"]
fn skill_search_request_cost() {
    let root = std::env::var("ZO_SKILL_MEASURE_ROOT").map_or_else(
        |_| runtime::default_config_home().join("skills"),
        PathBuf::from,
    );
    let skills: Vec<SkillIndexEntry> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let file = entry.path().join("SKILL.md");
            let text = std::fs::read_to_string(&file).ok()?;
            // The frontmatter block: everything between the opening `---` and
            // the next one.
            let front: String = text
                .lines()
                .skip(1)
                .take_while(|line| line.trim() != "---")
                .collect::<Vec<_>>()
                .join("\n");
            let field = |name: &str| {
                front
                    .lines()
                    .find_map(|line| line.strip_prefix(&format!("{name}:")))
                    .map(str::trim)
                    .map(str::to_string)
            };
            let name = field("name")?;
            Some(SkillIndexEntry::new(name, field("description"), file))
        })
        .collect();
    assert!(!skills.is_empty(), "no skills under {}", root.display());

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
}
