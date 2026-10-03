//! Turn-boundary project rule checks and one-time advice for zo's next turn.

use std::path::{Path, PathBuf};
use std::time::Duration;

use api::SystemOneConfig;
use runtime::{ContentBlock, ConversationMessage, MessageRole};
use serde_json::json;
use zerocode_core::jev::{self, JevMode, PROJECT_RULES, project_rules::{self as rules, Book, Facts, store}};

use super::jev_gate::{self, JevDoor};
use super::settings::merged_settings_root_from;
use super::shadow_ledger::{append_shadow_row, shadow_ledger_path, SHADOW_LEDGER_MAX_BYTES};

#[derive(Debug)]
struct Turn {
    task: String,
    edited: Vec<String>,
    evidence: String,
    final_text: String,
    facts: Facts,
}

fn words(message: &ConversationMessage, cap: usize) -> String {
    let mut output = String::new();
    let mut left = cap;
    for block in &message.blocks {
        let ContentBlock::Text { text } = block else { continue; };
        if left == 0 { break; }
        let (text, _) = jev::door::clear_text(text, jev::Cap::Chars(left));
        left = left.saturating_sub(text.chars().count() + 1);
        if !output.is_empty() { output.push('\n'); }
        output.push_str(&text);
    }
    output
}

fn turn(workspace: &Path, messages: &[ConversationMessage]) -> Turn {
    let root = store::workspace_root(workspace);
    let task = messages.iter().find(|message| message.role == MessageRole::User).map(|message| words(message, 2_000)).unwrap_or_default();
    let final_text = messages.iter().rev().find(|message| message.role == MessageRole::Assistant).map(|message| words(message, 4_000)).unwrap_or_default();
    let mut outside = false;
    let edited = runtime::edited_file_paths(messages).into_iter()
        .filter_map(|path| match store::edit_path(workspace, &path) {
            Ok(path) => Some(path),
            Err(rules::Invalid::Outside) => { outside = true; None },
            Err(_) => Some("..".into()),
        }).collect::<Vec<_>>();
    let mut parts = Vec::new();
    let mut left = 16 * 1024;
    for block in messages.iter().rev().flat_map(|message| message.blocks.iter().rev()) {
        let ContentBlock::ToolResult { tool_name, output, .. } = block else { continue; };
        if left < 128 { break; }
        // Do not parse/clone a giant patch merely to throw most of it away.
        // Missing evidence is explicit, so it cannot be counted as compliance.
        let body = if output.len() > left {
            "[tool output omitted: exceeds the rule-check evidence budget]".to_string()
        } else {
            runtime::patch_review::written_patch(tool_name, output).map_or_else(
                || jev::door::clear_text(output, jev::Cap::Bytes(2_048)).0,
                |(path, hunks)| format!("{path}\n{}", runtime::patch_review::unified_diff(&hunks)))
        };
        let part = jev::door::clear_text(&format!("{tool_name}: {body}"), jev::Cap::Bytes(left)).0;
        left = left.saturating_sub(part.len() + 1);
        parts.push(part);
    }
    parts.reverse();
    let evidence = parts.join("\n");
    let claims = runtime::claim_check::scan(messages, &final_text);
    let facts = Facts {
        // r43 can establish an unverified completion; its silence is not
        // evidence that checks actually ran after edits.
        checks_after_edits: runtime::claim_check::r43_would_reprompt(messages, &final_text).then_some(false),
        no_contradicted_completion: Some(!claims.iter().any(|claim| claim.code == jev::claim::CodeVerdict::Contradicted)),
        worktree_edits: if outside { None } else if edited.is_empty() { Some(true) } else {
            zerocode_core::git_dir::owning_checkout_of(&root).map(|owner| owner != root)
        },
    };
    Turn { task, edited, evidence, final_text, facts }
}

fn mode(workspace: &Path) -> JevMode {
    merged_settings_root_from(&runtime::ConfigLoader::default_for(workspace))
        .map_or(JevMode::Off, |root| PROJECT_RULES.mode_in(&root))
}

/// Check after a completed turn. No turn/response is held open and no repair
/// loop is started. Missing or stale definitions produce no model request.
pub fn note_project_rule_turn(workspace: &Path, recipient: &str, turn_id: &str, messages: &[ConversationMessage]) {
    let home = runtime::default_config_home();
    let at = super::decision_shadow::unix_millis();
    if !matches!(store::begin_check(&home, workspace, recipient, at, turn_id), Ok(true)) { return; }
    let mode = mode(workspace);
    if !mode.asks() || !JevDoor::open(workspace).permits_application_now() { return; }
    let book = match store::load(&home, workspace) {
        Ok(Some(book)) => book,
        Ok(None) => return,
        Err(error) => {
            let row = json!({"at":super::decision_shadow::unix_millis(),"from":"zo","turn":turn_id,
                "rubricVersion":PROJECT_RULES.rubric_version,"outcome":"rules_unavailable",
                "requests":0,"applied":false,"reason":error.to_string()});
            let _ = append_shadow_row(&shadow_ledger_path(workspace, PROJECT_RULES.ledger), &row, SHADOW_LEDGER_MAX_BYTES);
            return;
        }
    };
    let look = turn(workspace, messages);
    let workspace = workspace.to_path_buf();
    let recipient = recipient.to_string();
    let turn_id = turn_id.to_string();
    super::patch_review::detach(async move {
        check(workspace, recipient, turn_id, book, look, mode, at).await;
    });
}

async fn check(workspace: PathBuf, recipient: String, turn_id: String, book: Book, mut look: Turn, initial: JevMode, at: u64) {
    let home = runtime::default_config_home();
    if !matches!(store::begin_check(&home, &workspace, &recipient, at, &turn_id), Ok(true)) { return; }
    let Ok(definition) = book.identity() else { return; };
    look.edited = rules::scope_paths(&book, look.edited);
    let mut readings = rules::deterministic(&book, &look.edited, look.facts);
    let mut row = json!({"at":at,"from":"zo","turn":turn_id,"definition":definition,
        "rubricVersion":PROJECT_RULES.rubric_version,"outcome":"control","requests":0,
        "retries":0,"inputTokens":0,"elapsedMs":0,"redactedLines":0,"applied":false,
        "routeUse":initial.key()});
    if let Err(error) = store::verify_scope(&workspace, &book, &look.edited) {
        row["outcome"] = json!("rules_unavailable");
        row["reason"] = json!(error.to_string());
        let _ = append_shadow_row(&shadow_ledger_path(&workspace, PROJECT_RULES.ledger), &row, SHADOW_LEDGER_MAX_BYTES);
        return;
    }
    if let Some(body) = rules::request(&book, &look.edited, &look.task, &look.evidence, &look.final_text) {
        let client = SystemOneConfig::from_env().ok().map(SystemOneConfig::into_client);
        let opened = workspace.clone();
        let Ok(door) = tokio::task::spawn_blocking(move || JevDoor::open(&opened)).await else { return; };
        match (door.pass(&PROJECT_RULES, client.is_some(), body), client) {
            (Ok(cleared), Some(client)) => {
                let cleared = cleared.with_review_origin("zo/session", &recipient);
                row["redactedLines"] = json!(cleared.withheld_lines());
                let call = jev_gate::send(&client, cleared,
                    Duration::from_millis(rules::REQUEST_DEADLINE_MS), None).await;
                row["requests"] = json!(call.requests);
                row["retries"] = json!(call.retries);
                row["elapsedMs"] = json!(jev_gate::millis(call.elapsed));
                match call.outcome {
                    Ok(response) => {
                        row["model"] = json!(response.model);
                        row["inputTokens"] = json!(response.usage.input_tokens);
                        let answers = json!(response.answers);
                        let semantic = book.rules.iter().filter(|rule| rule.applies_to(&look.edited))
                            .filter(|rule| matches!(rule.check, rules::Check::Model { .. }))
                            .map(|rule| rules::read(&answers, &rule.id)).collect::<Result<Vec<_>, _>>();
                        match semantic {
                            Ok(semantic) => { readings.extend(semantic); row["outcome"] = json!(jev::door::ANSWERED_OUTCOME); }
                            Err(_) => row["outcome"] = json!("schema"),
                        }
                    }
                    Err(failure) => row["outcome"] = json!(failure.ledger_token()),
                }
                if !door.permits_application_now() { row["withheld"] = json!("settings_changed"); }
            }
            (passed, _) => row["outcome"] = json!(passed.err().unwrap_or(jev::door::Refused::NoKey).token()),
        }
    }
    let still_on = initial == JevMode::On && mode(&workspace) == JevMode::On
        && JevDoor::open(&workspace).permits_application_now();
    if still_on && row.get("withheld").is_none() {
        let evidence = format!("{}\n{}\n{}", look.task, look.evidence, look.final_text);
        if let Ok(Some(advice)) = rules::advice(&book, &readings, &evidence, &look.edited, at, &turn_id) {
            match store::offer(&home, &workspace, &recipient, &advice) {
                Ok(true) => {
                    row["queued"] = json!(true);
                    row["deliveryKey"] = json!(rules::delivery_key(&advice, &recipient));
                }
                Ok(false) => {},
                Err(error) => row["withheld"] = json!(error.to_string()),
            }
        }
    }
    row["readings"] = json!(readings);
    let ledger = shadow_ledger_path(&workspace, PROJECT_RULES.ledger);
    let _ = tokio::task::spawn_blocking(move || append_shadow_row(&ledger, &row, SHADOW_LEDGER_MAX_BYTES)).await;
}

/// A delivery is prepared only while the existing switch/mode and source
/// hashes still allow it. The caller acknowledges after adding it to context.
#[must_use]
pub fn pending_project_rule_advice(workspace: &Path, recipient: &str, room: usize) -> Option<rules::Advice> {
    if mode(workspace) != JevMode::On || !JevDoor::open(workspace).permits_application_now() { return None; }
    store::pending(&runtime::default_config_home(), workspace, recipient, room).ok().flatten()
}

pub fn project_rule_advice_delivered(workspace: &Path, recipient: &str, advice: &rules::Advice) {
    let receipt = rules::delivery_receipt(advice, recipient, i64::try_from(super::decision_shadow::unix_millis()).unwrap_or(i64::MAX));
    // Keep pending state if its receipt cannot be recorded. A retry is
    // idempotent in the shared reader and spends no second model request.
    if append_shadow_row(&shadow_ledger_path(workspace, PROJECT_RULES.ledger), &receipt, SHADOW_LEDGER_MAX_BYTES).is_ok() {
        let _ = store::delivered(&runtime::default_config_home(), workspace, recipient, &advice.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::jev_mock::{machine, Mock};
    use std::sync::{Arc, Mutex};

    fn compile(workspace: &Path) -> Book {
        std::fs::write(workspace.join("AGENTS.md"), "Avoid duplicating existing business logic.\n").unwrap();
        let book = Book { schema_version: 1, rules: vec![rules::Rule { id: "shared_logic".into(),
            source: rules::Source { path: "AGENTS.md".into(), sha256: String::new(), first_line: 1,
                last_line: 1, text: "Avoid duplicating existing business logic.".into() },
            paths: Vec::new(), check: rules::Check::Model { question: "Is the same business logic newly implemented twice?".into() } }] };
        store::compile(&runtime::default_config_home(), workspace, book).unwrap();
        store::load(&runtime::default_config_home(), workspace).unwrap().unwrap()
    }

    fn response() -> String {
        json!({"model":"jev-test","answers":{"shared_logic":{"type":"choice","choice":"violated",
            "confidence":0.8,"probabilities":{"violated":0.9,"followed":0.0,"not_applicable":0.0,"unknown":0.1}}},
            "usage":{"input_tokens":20,"output_tokens":0}}).to_string()
    }

    #[test]
    fn silence_from_the_completion_gate_is_not_proof_of_a_green_check() {
        let workspace = tempfile::tempdir().unwrap();
        let look = turn(workspace.path(), &[ConversationMessage::user_text("Inspect the code.")]);
        assert_eq!(look.facts.checks_after_edits, None);
    }

    #[test]
    fn semantic_rules_record_in_shadow_and_offer_once_when_on() {
        for word in ["shadow", "on"] {
            let mock = Mock::serving(200, response());
            machine(&PROJECT_RULES, word, &mock.base_url, |workspace| {
                compile(workspace);
                let messages = [ConversationMessage::user_text("Review the patch."),
                    ConversationMessage::assistant(vec![ContentBlock::Text { text: "The change is ready.".into() }])];
                note_project_rule_turn(workspace, "session", "turn", &messages);
                let ledger = shadow_ledger_path(workspace, PROJECT_RULES.ledger);
                let until = std::time::Instant::now() + Duration::from_secs(5);
                let rows = loop {
                    let rows = super::super::jev_summary::read_rows(&ledger);
                    if !rows.is_empty() { break rows; }
                    assert!(std::time::Instant::now() < until, "the turn entrypoint did not finish its check");
                    std::thread::sleep(Duration::from_millis(10));
                };
                assert_eq!(mock.requests().len(), 1);
                let advice = pending_project_rule_advice(workspace, "session", rules::ADVICE_CHAR_CAP);
                assert_eq!(advice.is_some(), word == "on");
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0]["applied"], false, "queuing is not delivery");
                if let Some(advice) = advice {
                    project_rule_advice_delivered(workspace, "session", &advice);
                    project_rule_advice_delivered(workspace, "session", &advice);
                    assert!(pending_project_rule_advice(workspace, "session", rules::ADVICE_CHAR_CAP).is_none());
                    let delivered = super::super::jev_summary::read_rows(&ledger);
                    let tally = jev::summary::summarize(&delivered, 0);
                    assert_eq!((tally.rows, tally.requests, tally.applied), (1, 1, 1));
                }
            });
        }
    }

    #[test]
    fn a_late_rule_answer_cannot_apply_after_its_mode_or_source_changes() {
        for change in ["off", "source", "pin"] {
            let root = Arc::new(Mutex::new(None::<PathBuf>));
            let changed = Arc::clone(&root);
            let mock = Mock::answering(move |_| {
                let root = changed.lock().unwrap().clone().unwrap();
                if change == "source" {
                    std::fs::write(root.join("AGENTS.md"), "Changed instructions.\n").unwrap();
                } else {
                    let path = runtime::default_config_home().join("settings.json");
                    let mut settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                    if change == "off" { settings["smart"][PROJECT_RULES.setting] = json!("off"); }
                    else { settings["smart"]["jevModel"] = json!("jev-another-pin"); }
                    std::fs::write(path, settings.to_string()).unwrap();
                }
                (200, response())
            });
            machine(&PROJECT_RULES, "on", &mock.base_url, |workspace| {
                *root.lock().unwrap() = Some(workspace.to_path_buf());
                let book = compile(workspace);
                let messages = [ConversationMessage::user_text("Review the patch.")];
                api::sync_bridge::run_blocking(check(workspace.to_path_buf(), "session".into(), "turn".into(), book,
                    turn(workspace, &messages), JevMode::On, 1));
                assert!(pending_project_rule_advice(workspace, "session", rules::ADVICE_CHAR_CAP).is_none(), "{change}");
                let rows = super::super::jev_summary::read_rows(&shadow_ledger_path(workspace, PROJECT_RULES.ledger));
                assert_eq!(rows[0]["applied"], false, "{change}");
            });
        }
    }

    #[test]
    fn off_and_missing_definitions_never_ask_and_evidence_memory_is_bounded() {
        let mock = Mock::serving(200, response());
        machine(&PROJECT_RULES, "off", &mock.base_url, |workspace| {
            compile(workspace);
            note_project_rule_turn(workspace, "session", "turn", &[ConversationMessage::user_text("Task")]);
            assert!(mock.requests().is_empty());
            assert!(!shadow_ledger_path(workspace, PROJECT_RULES.ledger).exists());
        });
        machine(&PROJECT_RULES, "on", &mock.base_url, |workspace| {
            note_project_rule_turn(workspace, "session", "turn", &[ConversationMessage::user_text("Task")]);
            assert!(mock.requests().is_empty());
            let messages = [ConversationMessage::tool_result("read", "Read", "x".repeat(1024 * 1024), false)];
            let look = turn(workspace, &messages);
            assert!(look.evidence.len() <= 16 * 1024);
            assert!(look.evidence.contains("omitted"));
        });
    }
}
