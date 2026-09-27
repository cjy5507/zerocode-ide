//! Turn-start file-pick seat. Search results, this session's recent edits and
//! an already-built codegraph index form one bounded list; one shared Jev door
//! sends its paths and short descriptions in a single Noul batch.
//!
//! The rows, the label and the words a request is searched by live in the
//! core since t-11349 (`zerocode_core::jev::file_pick`), where the window asks
//! the same seat of its panes' agents; zo's own search engines, its codegraph
//! and its door stay here.

use std::collections::{BTreeMap, HashSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use api::{SystemOneConfig, SystemOneFailure, SystemOneRequest, SystemOneResponse, SYSTEMONE_MODEL};
use codegraph::{CodeGraph, Resolved};
use runtime::file_pick::{FilePickAsk, FilePickCandidate, FilePickHint, FilePickSeat};
use runtime::file_search::{self, FileSearchOptions, MatchType, SearchRoot};
use runtime::{grep_search, GrepSearchInput};
use serde_json::Value;
use zerocode_core::jev::door::{self, Refused};
use zerocode_core::jev::file_pick::{
    edited_fingerprints, interleave, label_row, search_terms, workspace_relative_path,
    CandidateBatch,
};
#[cfg(test)]
use zerocode_core::jev::file_pick::FILE_PICK_NO_EDIT_LABEL;
pub use zerocode_core::jev::file_pick::{FilePickLabelRow, FilePickRow};
use zerocode_core::jev::promote;
use zerocode_core::AgentKind;
use zerocode_core::jev::{
    digest_of, fingerprint_of, FILE_PICK, FILE_PICK_APPLY_DEADLINE_MS,
    FILE_PICK_CANDIDATE_CAP,
};

use super::jev_gate::{self, JevDoor};
#[cfg(test)]
use super::probe_exec::task_fingerprint;
use super::settings::jev_file_pick_mode_from;
use super::shadow_ledger::{append_shadow_row, read_shadow_rows, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};

/// The ledger the Jev use table names for file-pick requests and their labels.
pub const FILE_PICK_FILE: &str = FILE_PICK.ledger;
/// The outcome shared with the Jev answer-rate judge.
pub const FILE_PICK_OUTCOME_ANSWERED: &str = door::ANSWERED_OUTCOME;
/// The version attached to the one request rubric and state shape — the
/// seat's row's own (t-6877), read from the table and not respelled.
pub const FILE_PICK_RUBRIC_VERSION: u32 = zerocode_core::jev::file_pick::FILE_PICK_RUBRIC_VERSION;
const _: () = assert!(matches!(
    FILE_PICK.apply_deadline_ms,
    Some(FILE_PICK_APPLY_DEADLINE_MS)
));

const FILE_PICK_GRAPH_LINK_CAP: usize = 8;
const FILE_PICK_SOURCE_COUNT: usize = 3;
const FILE_PICK_SETTINGS_UNAVAILABLE: &str = "settings_unavailable";

/// A seat for the project rooted at `cwd`; its settings and ledger share the
/// existing project and Jev door, so there is no additional consent surface.
#[derive(Debug)]
pub struct FilePickJudge {
    cwd: PathBuf,
}

impl FilePickJudge {
    /// Build the seat for one workspace.
    #[must_use]
    pub fn at(cwd: &Path) -> Self {
        Self {
            cwd: cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()),
        }
    }
}

impl FilePickSeat for FilePickJudge {
    fn suggest(&self, ask: FilePickAsk) -> futures_util::future::BoxFuture<'_, Option<FilePickHint>> {
        Box::pin(run_at(self.cwd.clone(), ask))
    }

    fn label(&self, attempt: &str, edited_paths: &[String], search_calls_before_first_edit: Option<usize>) {
        let _ = note_file_pick_turn(
            &self.cwd,
            attempt,
            edited_paths,
            search_calls_before_first_edit,
        );
    }
}

/// Where this workspace's file-pick ledger lives.
#[must_use]
pub fn file_pick_path(cwd: &Path) -> PathBuf {
    shadow_ledger_path(cwd, FILE_PICK_FILE)
}

/// Judge the file-pick seat on its request and label rows.
#[must_use]
pub fn judge_ledger(ledger: &Path, now_ms: i64) -> Option<promote::Verdict> {
    super::shadow_ledger::judge_seat_ledger(&FILE_PICK, ledger, now_ms)
}

/// The one path through which this feature asks Jev: mode and consent from the
/// person's settings, candidates from local workspace data, and a single
/// batched Noul request through the shared door.
async fn run_at(cwd: PathBuf, ask: FilePickAsk) -> Option<FilePickHint> {
    run_at_with_recent(cwd, ask, None).await
}

async fn run_at_with_recent(
    cwd: PathBuf,
    ask: FilePickAsk,
    recent_override: Option<Vec<String>>,
) -> Option<FilePickHint> {
    let mode = asking_mode(&cwd)?;
    if !mode.asks() {
        return None;
    }
    let acting = if mode.automatic() {
        mode.applies_with(runtime::jev_seat_applies(&cwd, &FILE_PICK))
    } else {
        mode.applies()
    };

    let candidate_started = Instant::now();
    let cwd_for_candidates = cwd.clone();
    let request = ask.request.clone();
    let session_id = ask.session_id.clone();
    let batch = tokio::task::spawn_blocking(move || {
        candidate_batch(&cwd_for_candidates, &session_id, &request, recent_override)
    })
    .await
    .unwrap_or_default();
    let candidate_elapsed_ms = jev_gate::millis(candidate_started.elapsed());

    let (mut row, answers) =
        score_candidates(&cwd, &ask, &batch, candidate_elapsed_ms).await?;
    row.outcome = FILE_PICK_OUTCOME_ANSWERED.to_string();
    row.answers = Some(answers.probabilities.clone());
    let ranked = runtime::file_pick::rank_candidates(&batch.files, &answers.value);
    row.ranked_paths = ranked.iter().map(|path| fingerprint_of(path)).collect();
    let selected = runtime::file_pick::select_candidates(&batch.files, &answers.value);
    row.selected_paths = selected
        .iter()
        .map(|path| fingerprint_of(path))
        .collect();
    let hint = acting.then(|| runtime::file_pick::hint(&selected)).flatten();
    row.applied = hint.is_some();
    row.noted = hint.is_some();
    row.route_use = if row.applied {
        zerocode_core::jev::ROUTE_USE_APPLIED.to_string()
    } else {
        mode.key().to_string()
    };
    write_request_and_judge(&cwd, &row);
    hint
}

async fn score_candidates(
    cwd: &Path,
    ask: &FilePickAsk,
    batch: &CandidateBatch,
    candidate_elapsed_ms: u64,
) -> Option<(FilePickRow, CheckedAnswers)> {
    let state = runtime::file_pick::state(&ask.request, &batch.files);
    let questions = runtime::file_pick::questions(&batch.files);
    let request = SystemOneRequest {
        state: &state,
        model: SYSTEMONE_MODEL,
        questions: &questions,
    };
    let mut row = FilePickRow::new(ask, batch, candidate_elapsed_ms);
    row.from = Some(AgentKind::Zo.slug().to_string());
    row.pane = super::tool_guard::pane_of(cwd);
    let Some(body) = jev_gate::body_of(&request) else {
        row.outcome = SystemOneFailure::InvalidRequest.ledger_token();
        write_request_and_judge(cwd, &row);
        return None;
    };

    let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
    let opened_at = cwd.to_path_buf();
    let Ok(door) = tokio::task::spawn_blocking(move || JevDoor::open(&opened_at)).await else {
        row.outcome = FILE_PICK_SETTINGS_UNAVAILABLE.to_string();
        write_request_and_judge(cwd, &row);
        return None;
    };
    let (cleared, client) = match (door.pass(&FILE_PICK, client.is_some(), body), client) {
        (Ok(cleared), Some(client)) => (cleared, client),
        (passed, _) => {
            let refusal = passed.err().unwrap_or(Refused::NoKey);
            row.outcome = refusal.token().to_string();
            row.redacted_lines = 0;
            write_request_and_judge(cwd, &row);
            return None;
        }
    };

    row.redacted_lines = u32::try_from(cleared.withheld_lines()).unwrap_or(u32::MAX);
    row.request_digest = Some(digest_of(
        FILE_PICK.id,
        FILE_PICK_RUBRIC_VERSION,
        door.model(),
        cleared.bytes(),
    ));
    let deadline = Duration::from_millis(FILE_PICK_APPLY_DEADLINE_MS);
    let call = jev_gate::send(&client, cleared, deadline, None).await;
    row.requests = call.requests;
    row.retries = call.retries;
    row.elapsed_ms = jev_gate::millis(call.elapsed);
    let response = match call.outcome {
        Ok(response) => response,
        Err(failure) => {
            row.outcome = failure.ledger_token();
            write_request_and_judge(cwd, &row);
            return None;
        }
    };

    row.model = Some(response.model.clone());
    row.input_tokens = Some(response.usage.input_tokens);
    row.output_tokens = Some(response.usage.output_tokens);
    let answers = match read_answers(&response, &batch.files) {
        Ok(answers) => answers,
        Err((question, refusal)) => {
            row.outcome = SystemOneFailure::Schema.ledger_token();
            row.rejected = Some(format!("{question}: {}", refusal.token()));
            write_request_and_judge(cwd, &row);
            return None;
        }
    };
    Some((row, answers))
}

struct CheckedAnswers {
    value: Value,
    probabilities: BTreeMap<String, f64>,
}

fn read_answers(
    response: &SystemOneResponse,
    candidates: &[FilePickCandidate],
) -> Result<CheckedAnswers, (String, zerocode_core::jev::noul::NoulRefusal)> {
    let value = serde_json::to_value(&response.answers).map_err(|_| {
        (
            "answers".to_string(),
            zerocode_core::jev::noul::NoulRefusal::NoAnswer,
        )
    })?;
    let readings = zerocode_core::jev::file_pick::read_answers_naming(candidates, &value)?;
    let probabilities = zerocode_core::jev::file_pick::probabilities(readings);
    Ok(CheckedAnswers { value, probabilities })
}

fn write_request_and_judge(cwd: &Path, row: &FilePickRow) {
    let ledger = file_pick_path(cwd);
    let _ = append_shadow_row(&ledger, row, SHADOW_LEDGER_MAX_BYTES);
    let _ = judge_ledger(&ledger, super::decision_shadow::now_ms());
}

/// Label one answered request with the file set the same public turn wrote.
#[must_use]
pub fn note_file_pick_turn(
    cwd: &Path,
    attempt: &str,
    edited_paths: &[String],
    search_calls_before_first_edit: Option<usize>,
) -> bool {
    let ledger = file_pick_path(cwd);
    let rows: Vec<Value> = read_shadow_rows(&ledger);
    let Some(request) = rows.iter().rev().find(|row| {
        row.get("attempt").and_then(Value::as_str) == Some(attempt)
            && row.get("outcome").and_then(Value::as_str) == Some(FILE_PICK_OUTCOME_ANSWERED)
    }) else {
        return false;
    };
    let Some(judged) = request.get("judged").and_then(Value::as_u64) else {
        return false;
    };
    let label = judged.to_string();
    if rows.iter().any(|row| {
        row.get("kind").and_then(Value::as_str) == Some(runtime::LABEL_ROW_KIND)
            && row.get("label").and_then(Value::as_str) == Some(label.as_str())
    }) {
        return false;
    }

    let normalized_edits = edited_fingerprints(&[cwd], edited_paths);
    let label_row = label_row(
        request,
        judged,
        &normalized_edits,
        search_calls_before_first_edit,
        super::decision_shadow::unix_millis(),
    );
    let written = append_shadow_row(&ledger, &label_row, SHADOW_LEDGER_MAX_BYTES).is_ok();
    if written {
        let _ = judge_ledger(&ledger, super::decision_shadow::now_ms());
    }
    written
}

fn asking_mode(cwd: &Path) -> Option<super::settings::DecisionShadowMode> {
    jev_file_pick_mode_from(&runtime::ConfigLoader::default_for(cwd))
}

fn candidate_batch(
    cwd: &Path,
    session_id: &str,
    request: &str,
    recent_override: Option<Vec<String>>,
) -> CandidateBatch {
    let Ok(root) = cwd.canonicalize() else {
        return CandidateBatch::default();
    };
    let terms = search_terms(request);
    let recent_source = recent_override.unwrap_or_else(|| runtime::turn_trace::session_edited_files(&root, session_id));
    let recent_paths = recent_source
        .iter()
        .filter_map(|path| workspace_relative_path(&root, Path::new(path)))
        .filter(|path| root.join(path).is_file())
        .collect::<Vec<_>>();
    let search_paths = search_paths(&root, &terms);
    let graph_paths = graph_paths(&root, &terms, &search_paths, &recent_paths);

    let sources: [&[String]; FILE_PICK_SOURCE_COUNT] =
        [&search_paths, &recent_paths, &graph_paths];
    CandidateBatch {
        files: interleave(&root, &sources),
        recent_paths,
        search_candidates: search_paths.len(),
        graph_candidates: graph_paths.len(),
    }
}

fn search_paths(root: &Path, terms: &[String]) -> Vec<String> {
    if terms.is_empty() {
        return Vec::new();
    }
    let Some(limit) = NonZeroUsize::new(FILE_PICK_CANDIDATE_CAP) else {
        return Vec::new();
    };
    // `search_terms` has already split on every regex metacharacter, leaving
    // only alphanumeric words and `_` before we join them as alternatives.
    let pattern = terms.join("|");
    let grep = grep_search(&GrepSearchInput {
        pattern,
        path: Some(root.to_string_lossy().into_owned()),
        glob: None,
        output_mode: Some("files_with_matches".to_string()),
        before: None,
        after: None,
        context_short: None,
        context: None,
        line_numbers: None,
        case_insensitive: Some(true),
        file_type: None,
        head_limit: Some(FILE_PICK_CANDIDATE_CAP),
        offset: None,
        multiline: Some(false),
    })
    .map(|output| output.filenames)
    .unwrap_or_default();

    let query = terms.join(" ");
    let options = FileSearchOptions {
        limit,
        ..FileSearchOptions::default()
    };
    let file_name_hits = file_search::run(&query, vec![SearchRoot::repo(root)], options, None)
        .map(|results| {
            results
                .matches
                .into_iter()
                .filter(|hit| hit.match_type == MatchType::File)
                .map(|hit| hit.full_path.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for hit in grep.into_iter().chain(file_name_hits) {
        let Some(path) = workspace_relative_path(root, Path::new(&hit)) else {
            continue;
        };
        if root.join(&path).is_file() && seen.insert(path.clone()) {
            paths.push(path);
        }
        if paths.len() == FILE_PICK_CANDIDATE_CAP {
            break;
        }
    }
    paths
}

fn graph_paths(root: &Path, terms: &[String], search: &[String], recent: &[String]) -> Vec<String> {
    let cache = crate::codegraph_cache_path(root);
    let Ok(Some(mut graph)) = CodeGraph::open_existing(root, cache) else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    if let Ok(resolved) = graph.resolve_mentions(terms) {
        for item in resolved.into_iter().flatten() {
            let path = match item {
                Resolved::File(path) => path,
                Resolved::Symbol(symbol) => symbol.file,
            };
            if let Some(path) = workspace_relative_path(root, &root.join(path)) {
                if seen.insert(path.clone()) {
                    paths.push(path);
                }
            }
        }
    }
    let seeds = search
        .iter()
        .chain(recent.iter())
        .map(|path| root.join(path))
        .collect::<Vec<_>>();
    for seed in seeds {
        let Ok(Some(links)) = graph.file_links(&seed, FILE_PICK_GRAPH_LINK_CAP) else {
            continue;
        };
        for linked in links.uses.iter().chain(links.used_by.iter()) {
            let Some(path) = workspace_relative_path(root, &root.join(&linked.file)) else {
                continue;
            };
            if root.join(&path).is_file() && seen.insert(path.clone()) {
                paths.push(path);
            }
            if paths.len() == FILE_PICK_CANDIDATE_CAP {
                return paths;
            }
        }
    }
    paths
}

/// zo's start of a request's row: the core's, made now
/// ([`FilePickRow::of_batch`]).
trait NewFilePickRow {
    fn new(ask: &FilePickAsk, batch: &CandidateBatch, candidate_elapsed_ms: u64) -> Self;
}

impl NewFilePickRow for FilePickRow {
    fn new(ask: &FilePickAsk, batch: &CandidateBatch, candidate_elapsed_ms: u64) -> Self {
        Self::of_batch(super::decision_shadow::unix_millis(), ask, batch, candidate_elapsed_ms)
    }
}

#[cfg(test)]
mod tests;
