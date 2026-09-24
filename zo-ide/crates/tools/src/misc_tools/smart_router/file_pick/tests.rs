use std::fs;

use serde_json::{json, Value};
use tempfile::tempdir;
use zerocode_core::jev::fingerprint_of;

use super::*;

#[test]
fn candidates_are_workspace_relative_and_send_only_a_comment_line() {
    let workspace = tempdir().expect("workspace");
    let src = workspace.path().join("src");
    fs::create_dir_all(&src).expect("source directory");
    fs::write(
        src.join("target.rs"),
        "//! The target module.\n// CODE-PICK-BODY-SENTINEL\npub fn target() {}\n",
    )
    .expect("source file");

    let batch = candidate_batch(
        workspace.path(),
        "fixture-session",
        "implement an uncommon recalculation flow",
        Some(vec!["src/target.rs".to_string(), "../outside.rs".to_string()]),
    );

    let target = batch
        .files
        .iter()
        .find(|candidate| candidate.path == "src/target.rs")
        .expect("recent edit stays among candidates");
    assert_eq!(target.about, "//! The target module.");
    assert!(batch.files.iter().all(|candidate| !Path::new(&candidate.path).is_absolute()));
    assert!(!target.about.contains("CODE-PICK-BODY-SENTINEL"));
    assert!(batch.files.iter().all(|candidate| workspace.path().join(&candidate.path).is_file()));
}

#[test]
fn grep_hits_add_paths_without_sending_the_matched_line() {
    let workspace = tempdir().expect("workspace");
    let src = workspace.path().join("src");
    fs::create_dir_all(&src).expect("source directory");
    fs::write(
        src.join("cache_key.rs"),
        "//! Cache key helpers.\npub fn adjust_cache_key() { let body = \"BODY_SENTINEL_FIXTURE\"; }\n",
    )
    .expect("matching source file");
    fs::write(src.join("other.rs"), "pub fn unrelated() {}\n").expect("other source file");

    let batch = candidate_batch(
        workspace.path(),
        "fixture-session",
        "debug adjust the cache key",
        Some(Vec::new()),
    );

    assert!(batch.search_candidates > 0);
    assert!(batch.files.iter().any(|candidate| candidate.path == "src/cache_key.rs"));
    assert!(batch.files.iter().all(|candidate| !candidate.about.contains("BODY_SENTINEL_FIXTURE")));
}

#[test]
fn hindsight_compares_top_three_recent_order_and_candidate_ceiling_without_paths() {
    let workspace = tempdir().expect("workspace");
    let state_home = tempdir().expect("state home");
    let state_path = state_home.path().to_string_lossy().to_string();
    let _env = crate::tests::EnvGuard::set(core_types::paths::ZO_STATE_DIR_ENV, &state_path);
    let ledger = file_pick_path(workspace.path());
    let files = vec![
        FilePickCandidate { path: "src/first.rs".to_string(), about: String::new() },
        FilePickCandidate { path: "src/second.rs".to_string(), about: String::new() },
        FilePickCandidate { path: "src/third.rs".to_string(), about: String::new() },
        FilePickCandidate { path: "src/fourth.rs".to_string(), about: String::new() },
    ];
    let ask = FilePickAsk {
        attempt: "replay-attempt-1".to_string(),
        session_id: "replay-session-1".to_string(),
        request: "fix the target lookup".to_string(),
    };
    let batch = CandidateBatch {
        files,
        recent_paths: vec!["src/old.rs".to_string()],
        search_candidates: 2,
        graph_candidates: 2,
    };
    let mut row = FilePickRow::new(&ask, &batch, 12);
    row.outcome = FILE_PICK_OUTCOME_ANSWERED.to_string();
    row.answers = Some(BTreeMap::from([
        ("any".to_string(), 0.91),
        ("F01".to_string(), 0.91),
        ("F02".to_string(), 0.88),
        ("F03".to_string(), 0.81),
        ("F04".to_string(), 0.75),
    ]));
    row.ranked_paths = vec![
        fingerprint_of("src/first.rs"),
        fingerprint_of("src/second.rs"),
        fingerprint_of("src/third.rs"),
    ];
    row.selected_paths = vec![
        fingerprint_of("src/first.rs"),
        fingerprint_of("src/second.rs"),
        fingerprint_of("src/third.rs"),
    ];
    write_request_and_judge(workspace.path(), &row);

    assert!(note_file_pick_turn(
        workspace.path(),
        &ask.attempt,
        &["src/second.rs".to_string()],
        Some(4),
    ));

    let rows: Vec<Value> = read_shadow_rows(&ledger);
    let label = rows
        .iter()
        .find(|row| row.get("kind").and_then(Value::as_str) == Some(runtime::LABEL_ROW_KIND))
        .expect("the edited-file label");
    assert_eq!(label["agreed"], json!(true));
    assert_eq!(label["baselineAgreed"], json!(false));
    assert_eq!(label["candidateCeilingHit"], json!(true));
    assert_eq!(label["searchCallsBeforeFirstEdit"], json!(4));
    let written = fs::read_to_string(ledger).expect("the local ledger");
    assert!(!written.contains("src/second.rs"));
    assert!(!written.contains("fix the target lookup"));
}

#[test]
fn a_turn_that_edits_no_file_is_not_a_comparison() {
    let workspace = tempdir().expect("workspace");
    let state_home = tempdir().expect("state home");
    let state_path = state_home.path().to_string_lossy().to_string();
    let _env = crate::tests::EnvGuard::set(core_types::paths::ZO_STATE_DIR_ENV, &state_path);
    let ask = FilePickAsk {
        attempt: "replay-attempt-2".to_string(),
        session_id: "replay-session-2".to_string(),
        request: "fix a lookup".to_string(),
    };
    let batch = CandidateBatch {
        files: vec![FilePickCandidate {
            path: "src/lookup.rs".to_string(),
            about: String::new(),
        }],
        recent_paths: vec![],
        search_candidates: 1,
        graph_candidates: 0,
    };
    let mut row = FilePickRow::new(&ask, &batch, 1);
    row.outcome = FILE_PICK_OUTCOME_ANSWERED.to_string();
    row.answers = Some(BTreeMap::from([
        ("any".to_string(), 0.93),
        ("F01".to_string(), 0.93),
    ]));
    row.ranked_paths = vec![fingerprint_of("src/lookup.rs")];
    row.selected_paths = vec![fingerprint_of("src/lookup.rs")];
    write_request_and_judge(workspace.path(), &row);

    assert!(note_file_pick_turn(workspace.path(), &ask.attempt, &[], None));
    let rows: Vec<Value> = read_shadow_rows(&file_pick_path(workspace.path()));
    let label = rows
        .into_iter()
        .find(|row| row.get("kind").and_then(Value::as_str) == Some(runtime::LABEL_ROW_KIND))
        .expect("the label says it was not compared");
    assert_eq!(label["notCompared"], FILE_PICK_NO_EDIT_LABEL);
    assert!(label.get("agreed").is_none());
}

/* ---- the replay: start-of-turn file candidates from this machine ----------- */

const REPLAY_SEED_ENV: &str = "ZEROCODE_FILE_PICK_REPLAY_SEED";
const REPLAY_LIMIT_ENV: &str = "ZEROCODE_FILE_PICK_REPLAY_LIMIT";
const REPLAY_OUT_ENV: &str = "ZEROCODE_FILE_PICK_REPLAY_OUT";
const REPLAY_MAX_TURNS: usize = 50;
const REPLAY_SPEND_CAP_USD: f64 = 0.02;

struct ReplayPoint {
    cwd: PathBuf,
    ask: FilePickAsk,
    recent_paths: Vec<String>,
    edited_paths: Vec<String>,
    search_calls_before_first_edit: Option<usize>,
}

struct ReplaySample {
    points: Vec<ReplayPoint>,
    sample: String,
    eligible: usize,
    read: usize,
    skipped: usize,
}

struct ReplayMeasurement {
    spent: f64,
    rows: Vec<Value>,
    labels: Vec<Value>,
}

fn replay_points(
    history: &[runtime::ConversationMessage],
    cwd: &Path,
    session_id: &str,
) -> Vec<ReplayPoint> {
    let mut points = Vec::new();
    let mut recent_paths = Vec::new();
    for (turn_index, turn) in runtime::patch_review::persons_turns(history).into_iter().enumerate() {
        let request = runtime::patch_review::persons_words(turn);
        let edited_paths = runtime::edited_file_paths(turn);
        if !edited_paths.is_empty() && runtime::file_pick::is_code_edit_intent(&request) {
            let attempt = format!(
                "replay-{:016x}",
                task_fingerprint(session_id, &turn_index.to_string())
            );
            points.push(ReplayPoint {
                cwd: cwd.to_path_buf(),
                ask: FilePickAsk {
                    attempt,
                    session_id: session_id.to_string(),
                    request,
                },
                recent_paths: recent_paths.clone(),
                edited_paths: edited_paths.clone(),
                search_calls_before_first_edit: runtime::file_pick::search_calls_before_first_edit(turn),
            });
        }
        for path in edited_paths {
            recent_paths.retain(|recent| recent != &path);
            recent_paths.insert(0, path);
        }
    }
    points
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        let part = f64::from(u32::try_from(part).expect("replay count fits u32"));
        let whole = f64::from(u32::try_from(whole).expect("replay count fits u32"));
        part * 100.0 / whole
    }
}

fn spread_points(points: Vec<ReplayPoint>, limit: usize) -> Vec<ReplayPoint> {
    if limit == 0 || points.len() <= limit {
        return points;
    }
    let every = points.len().div_ceil(limit);
    points.into_iter().step_by(every).take(limit).collect()
}


/// Replay a bounded sample against the real TypeSafe endpoint. The caller
/// supplies a temporary `ZO_CONFIG_HOME`; the Jev door still reads the copied
/// person's consent, and the seed itself carries no prompt text or edited-file paths.
#[test]
#[ignore = "reads this machine's recent edit transcripts and asks the real endpoint through the Jev door"]
fn the_file_pick_this_machine_would_have_shown() {
    let seed_path = std::env::var(REPLAY_SEED_ENV)
        .expect("ZEROCODE_FILE_PICK_REPLAY_SEED names the private seed");
    let limit = std::env::var(REPLAY_LIMIT_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(REPLAY_MAX_TURNS)
        .min(REPLAY_MAX_TURNS);
    let sample = load_replay_sample(&seed_path, limit);
    println!(
        "--- file-pick replay: {} of {} eligible edit turns, sample {}",
        sample.points.len(),
        sample.eligible,
        sample.sample
    );
    let measurement = measure_replay(&sample.points);
    report_measurement(&sample, &measurement);
}

fn load_replay_sample(seed_path: &str, limit: usize) -> ReplaySample {
    let seed: Value = serde_json::from_str(
        &fs::read_to_string(seed_path).expect("the seed is readable"),
    )
    .expect("the seed parses");
    let mut points = Vec::new();
    let (mut read, mut skipped) = (0usize, 0usize);
    for transcript in seed["transcripts"].as_array().expect("transcripts") {
        let Some(path) = transcript["path"].as_str() else {
            skipped += 1;
            continue;
        };
        let Some(cwd) = transcript["cwd"].as_str() else {
            skipped += 1;
            continue;
        };
        let cwd = PathBuf::from(cwd);
        if !cwd.is_dir() {
            skipped += 1;
            continue;
        }
        let Ok(session) = runtime::Session::load_from_path(path) else {
            skipped += 1;
            continue;
        };
        let Some(history) = super::super::replay_support::history_as_it_stood(&session) else {
            skipped += 1;
            continue;
        };
        read += 1;
        points.extend(replay_points(&history, &cwd, &session.session_id));
    }
    let eligible = points.len();
    let points = spread_points(points, limit);
    let sample = fingerprint_of(
        &points
            .iter()
            .map(|point| point.ask.attempt.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    );
    ReplaySample {
        points,
        sample,
        eligible,
        read,
        skipped,
    }
}

fn measure_replay(points: &[ReplayPoint]) -> ReplayMeasurement {
    let rate = api::systemone_rate(SYSTEMONE_MODEL).expect("the System One price is known");
    let mut spent = 0.0;
    let mut rows: Vec<Value> = Vec::new();
    let mut labels: Vec<Value> = Vec::new();
    for point in points.iter().take(REPLAY_MAX_TURNS) {
        if spent >= REPLAY_SPEND_CAP_USD {
            break;
        }
        let _ = api::sync_bridge::run_blocking(run_at_with_recent(
            point.cwd.clone(),
            point.ask.clone(),
            Some(point.recent_paths.clone()),
        ));
        let ledger = file_pick_path(&point.cwd);
        let request_rows: Vec<Value> = read_shadow_rows(&ledger);
        let request = request_rows
            .into_iter()
            .rev()
            .find(|row| row.get("attempt").and_then(Value::as_str) == Some(point.ask.attempt.as_str()));
        let Some(request) = request else {
            println!("stopped before a request row; the seat is off or has no file-pick consent");
            break;
        };
        let outcome = request.get("outcome").and_then(Value::as_str).unwrap_or_default();
        if outcome != FILE_PICK_OUTCOME_ANSWERED {
            println!("stopped on first unavailable response (outcome={outcome})");
            break;
        }
        let tokens = request.get("inputTokens").and_then(Value::as_u64).unwrap_or(0);
        spent += rate.input_cost_usd(tokens);
        let _ = note_file_pick_turn(
            &point.cwd,
            &point.ask.attempt,
            &point.edited_paths,
            point.search_calls_before_first_edit,
        );
        if let Some(label) = request.get("judged").and_then(Value::as_u64) {
            let label = label.to_string();
            let ledger_rows: Vec<Value> = read_shadow_rows(&ledger);
            if let Some(row) = ledger_rows.into_iter().find(|row| {
                row.get("kind").and_then(Value::as_str) == Some(runtime::LABEL_ROW_KIND)
                    && row.get("label").and_then(Value::as_str) == Some(label.as_str())
            }) {
                labels.push(row);
            }
        }
        rows.push(request);
    }
    ReplayMeasurement {
        spent,
        rows,
        labels,
    }
}

fn report_measurement(sample: &ReplaySample, measurement: &ReplayMeasurement) {
    use zerocode_core::jev::summary::{percentile, wilson_lower, WILSON_Z_95};

    let spent = measurement.spent;
    let rows = &measurement.rows;
    let labels = &measurement.labels;
    let comparable: Vec<&Value> = labels
        .iter()
        .filter(|row| row.get("notCompared").is_none())
        .collect();
    let hit_count = |key: &str| {
        comparable
            .iter()
            .filter(|row| row.get(key).and_then(Value::as_bool) == Some(true))
            .count()
    };
    let top3 = hit_count("agreed");
    let baseline = hit_count("baselineAgreed");
    let hinted = hit_count("eligibleHintAgreed");
    let ceiling = hit_count("candidateCeilingHit");
    let lower = |count: usize| 100.0 * wilson_lower(count, comparable.len(), WILSON_Z_95);
    let mut request_ms: Vec<u64> = rows
        .iter()
        .filter_map(|row| row.get("elapsedMs").and_then(Value::as_u64))
        .collect();
    let mut candidate_ms: Vec<u64> = rows
        .iter()
        .filter_map(|row| row.get("candidateElapsedMs").and_then(Value::as_u64))
        .collect();
    request_ms.sort_unstable();
    candidate_ms.sort_unstable();
    let mut search_counts: Vec<u64> = labels
        .iter()
        .filter_map(|row| row.get("searchCallsBeforeFirstEdit").and_then(Value::as_u64))
        .collect();
    search_counts.sort_unstable();
    println!(
        "answered turns={} requests={} cost_usd={spent:.6} input_tokens={}",
        rows.len(),
        rows.iter()
            .map(|row| row.get("requests").and_then(Value::as_u64).unwrap_or(0))
            .sum::<u64>(),
        rows.iter()
            .map(|row| row.get("inputTokens").and_then(Value::as_u64).unwrap_or(0))
            .sum::<u64>()
    );
    println!(
        "candidate ceiling={ceiling}/{} ({:.1}%, Wilson lower {:.1}%), ranked_top3={top3}/{} ({:.1}%, Wilson lower {:.1}%), recent_edit_top3={baseline}/{} ({:.1}%), thresholded_hint_top3={hinted}/{} ({:.1}%)",
        comparable.len(),
        percent(ceiling, comparable.len()),
        lower(ceiling),
        comparable.len(),
        percent(top3, comparable.len()),
        lower(top3),
        comparable.len(),
        percent(baseline, comparable.len()),
        comparable.len(),
        percent(hinted, comparable.len())
    );
    println!(
        "p50 candidate_ms={} type_safe_ms={} search_calls_before_first_edit_p50={} transcripts_read={} skipped={} labels_not_compared={}",
        percentile(&candidate_ms, 0.5).unwrap_or(0),
        percentile(&request_ms, 0.5).unwrap_or(0),
        percentile(&search_counts, 0.5).unwrap_or(0),
        sample.read,
        sample.skipped,
        labels.len().saturating_sub(comparable.len())
    );
    if let Ok(path) = std::env::var(REPLAY_OUT_ENV) {
        let summary = json!({
            "sample": sample.sample,
            "asked": rows.len(),
            "requests": rows.iter().map(|row| row.get("requests").and_then(Value::as_u64).unwrap_or(0)).sum::<u64>(),
            "inputTokens": rows.iter().map(|row| row.get("inputTokens").and_then(Value::as_u64).unwrap_or(0)).sum::<u64>(),
            "costUsd": spent,
            "compared": comparable.len(),
            "candidateCeilingHits": ceiling,
            "top3Hits": top3,
            "recentEditBaselineHits": baseline,
            "thresholdedHintHits": hinted,
            "p50CandidateMs": percentile(&candidate_ms, 0.5).unwrap_or(0),
            "p50TypeSafeMs": percentile(&request_ms, 0.5).unwrap_or(0),
            "p50SearchCallsBeforeFirstEdit": percentile(&search_counts, 0.5).unwrap_or(0),
        });
        fs::write(path, format!("{summary}\n")).expect("the aggregate replay row is written");
    }
}
