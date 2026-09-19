//! The live console reads the one evidence store and submits through the
//! existing ComputerRequest queue. A plain shell gains no agent capability.

use crate::browser_runtime::from_the_main_webview;
use crate::{AppState, ShellStateExt as _};
use serde_json::{Value, json};
use std::path::Path;

/// Read the folder already used by the report button. No writes, captures or
/// alternate log: late/reopened consoles catch up from the same evidence.
#[tauri::command(async)]
pub(crate) fn flow_evidence(
    webview: tauri::Webview,
    state: tauri::State<'_, AppState>,
    report: String,
) -> Result<Value, String> {
    from_the_main_webview(&webview)?;
    evidence_in(state.local_data_root(), Path::new(&report))
}

fn evidence_in(root: &Path, report: &Path) -> Result<Value, String> {
    let report = report.canonicalize().map_err(|error| error.to_string())?;
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let dir = report.parent().ok_or("an evidence report needs a folder")?;
    if report
        .file_name()
        .is_none_or(|name| name != crate::computer_use::report::REPORT_FILE)
        || !(dir.starts_with(root.join("automations"))
            || dir.starts_with(root.join(crate::computer_use::evidence::SESSIONS_DIR)))
    {
        return Err("the report is outside this window's evidence folders".into());
    }
    Ok(json!({
        "dir": dir, "report": report,
        "steps": crate::run_evidence::steps_in(dir),
        "walk": crate::run_evidence::walks_in(dir).last().map(|(_, report)| report),
    }))
}

type ComputerSender = tokio::sync::mpsc::UnboundedSender<zerocode_hookd::ComputerRequest>;
static COMPUTER: std::sync::OnceLock<ComputerSender> = std::sync::OnceLock::new();

/// Installed only after the window's bridge binds, with its existing sender.
pub(crate) fn install_sender(sender: ComputerSender) {
    let _ = COMPUTER.set(sender);
}

/// A person's Flow action takes the same dispatcher, sequence gate, runner and
/// writer as the CLI. It does not give a plain terminal agent capabilities.
#[tauri::command]
pub(crate) async fn flow_execute(
    webview: tauri::Webview,
    state: tauri::State<'_, AppState>,
    argv: Vec<String>,
    worktree: String,
) -> Result<Value, String> {
    from_the_main_webview(&webview)?;
    let sender = COMPUTER
        .get()
        .ok_or("the Computer Use bridge is unavailable")?;
    execute(
        sender,
        argv,
        state.active_root().to_string_lossy().into_owned(),
        &worktree,
    )
    .await
}

fn validate(argv: &[String]) -> Result<(), String> {
    if argv.iter().any(|word| word.chars().any(char::is_control)) {
        return Err("a Flow command must be one line without control characters".into());
    }
    let command = zerocode_core::computer_use::parse_command(argv)?;
    if !matches!(
        command.method,
        zerocode_core::computer_use::ComputerMethod::RecipeRun
            | zerocode_core::computer_use::ComputerMethod::Walk
    ) {
        return Err("the Flow console runs recipe-run or walk".into());
    }
    Ok(())
}

async fn execute(
    sender: &ComputerSender,
    mut argv: Vec<String>,
    cwd: String,
    worktree: &str,
) -> Result<Value, String> {
    // The requested root is a precondition, never a new execution directory.
    // Keep the native snapshot after this check so another tab switch cannot
    // change the workspace used by the queued walk's Jev consent door.
    if Path::new(&cwd) != Path::new(worktree) {
        return Err(
            "the active worktree changed; return to the original worktree before running this Flow"
                .into(),
        );
    }
    validate(&argv)?;
    if !argv.iter().any(|word| word == "--json") {
        argv.push("--json".into());
    }
    let (answer, waiting) = tokio::sync::oneshot::channel();
    sender
        .send(zerocode_hookd::ComputerRequest {
            argv,
            cwd: Some(cwd),
            evidence: None,
            answer,
        })
        .map_err(|_| "the Computer Use bridge is no longer listening".to_string())?;
    let answer = waiting
        .await
        .map_err(|_| "the Computer Use request ended without an answer".to_string())?;
    let envelope =
        zerocode_core::computer_use_protocol::answer_envelope(&answer.stdout, &answer.stderr)
            .ok_or("the Computer Use request did not return its JSON answer")?;
    if answer.exit_code != 0 {
        return Err(envelope
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or(answer.stderr.trim())
            .to_string());
    }
    Ok(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_console_read_is_fenced_and_does_not_modify_the_seal() {
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join(crate::computer_use::evidence::SESSIONS_DIR)
            .join("run");
        std::fs::create_dir_all(&dir).unwrap();
        crate::run_evidence::record(
            &dir,
            1,
            "browser",
            &["marks".into(), "page".into()],
            Ok(()),
            crate::run_evidence::Framing::None,
        );
        let report = crate::computer_use::report::write(&dir).unwrap().unwrap();
        let before = std::fs::read(dir.join("manifest.json")).unwrap();
        let read = evidence_in(root.path(), &report).unwrap();
        assert_eq!(read["steps"][0]["n"], 1);
        assert_eq!(before, std::fs::read(dir.join("manifest.json")).unwrap());
        assert!(crate::computer_use::report::verify(&dir).reproduced);
        let outside = tempfile::tempdir().unwrap();
        let foreign = outside
            .path()
            .join(crate::computer_use::report::REPORT_FILE);
        std::fs::write(&foreign, "outside").unwrap();
        assert!(evidence_in(root.path(), &foreign).is_err());
        #[cfg(unix)]
        {
            let alias = dir.parent().unwrap().join("foreign");
            std::os::unix::fs::symlink(outside.path(), &alias).unwrap();
            assert!(
                evidence_in(
                    root.path(),
                    &alias.join(crate::computer_use::report::REPORT_FILE)
                )
                .is_err()
            );
        }
    }

    #[tokio::test]
    async fn the_console_sends_opaque_arguments_to_the_existing_computer_queue() {
        let (bridge, _events, _teams, _browser, mut requests, _federation) =
            zerocode_hookd::BridgeState::new_with_computer("test", "browser", "computer");
        let sender = bridge.computer_requests();
        let argv: Vec<String> = [
            "walk",
            "--goal",
            "read $(touch /tmp/never) `whoami` \"quoted\" 'word'",
            "--pane",
            "p",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        let mut expected = argv.clone();
        expected.push("--json".into());
        let acting = async {
            let request = requests.recv().await.unwrap();
            assert_eq!(request.argv, expected);
            assert_eq!(request.cwd.as_deref(), Some("/fixture"));
            assert!(request.evidence.is_none());
            request
                .answer
                .send(zerocode_hookd::TeamAnswer {
                    stdout: json!({"ok": true, "result": {"done": true}}).to_string(),
                    stderr: String::new(),
                    exit_code: 0,
                })
                .unwrap();
        };
        let (answer, ()) = tokio::join!(
            execute(&sender, argv, "/fixture".into(), "/fixture"),
            acting
        );
        assert_eq!(answer.unwrap()["result"]["done"], true);
    }

    #[tokio::test]
    async fn a_queued_flow_cannot_borrow_another_worktrees_consent() {
        let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel();
        for asked in ["/first-worktree", ""] {
            assert!(
                execute(
                    &sender,
                    vec!["recipe-run".into(), "--name".into(), "sample".into()],
                    "/second-worktree".into(),
                    asked,
                )
                .await
                .is_err()
            );
            assert!(matches!(
                requests.try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ));
        }
    }

    #[tokio::test]
    async fn the_console_refuses_non_flow_commands_before_the_queue() {
        let (sender, mut requests) = tokio::sync::mpsc::unbounded_channel();
        assert!(
            execute(
                &sender,
                vec!["status".into()],
                "/fixture".into(),
                "/fixture"
            )
            .await
            .is_err()
        );
        assert!(
            execute(
                &sender,
                vec![
                    "walk".into(),
                    "--goal".into(),
                    "a\rb".into(),
                    "--pane".into(),
                    "p".into()
                ],
                "/fixture".into(),
                "/fixture",
            )
            .await
            .is_err()
        );
        assert!(requests.try_recv().is_err());
    }
}
