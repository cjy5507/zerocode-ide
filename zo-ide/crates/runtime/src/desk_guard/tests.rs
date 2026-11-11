//! The desk guard (t-104888), on the commands of design §7
//! (`reports/desk-guard/DESIGN.md`) and on every name in its table. Nothing
//! runs: each command is only judged.

use super::{caught, refusal, refused, with_session, Caught, Kind, SETTING, TABLE};
use crate::kill_guard::Shell;
use crate::permission_enforcer::{EnforcementResult, PermissionEnforcer};
use crate::{PermissionMode, PermissionOutcome, PermissionPolicy, RuntimePermissionRuleConfig};

fn bash(command: &str) -> Option<Caught> {
    caught(Shell::Bash, command)
}

fn kind_of(command: &str) -> Option<Kind> {
    bash(command).map(|caught| caught.kind)
}

/* ---- design §7: what it refuses ---------------------------------------------- */

#[test]
fn an_input_tool_is_caught_wherever_the_shell_runs_it() {
    for command in [
        "cliclick c:10,20",
        "/opt/homebrew/bin/cliclick -m verbose c:1,1",
        "true && cliclick p",
        "FOO=1 cliclick p",
        "bash -c 'cliclick p'",
        "sudo nice -n 5 cliclick p",
    ] {
        let caught = bash(command).unwrap_or_else(|| panic!("{command} must be caught"));
        assert_eq!(caught.kind, Kind::InputTool, "{command}");
        assert_eq!(caught.words, "cliclick", "{command}");
    }
    assert_eq!(kind_of("xdotool click 1"), Some(Kind::InputTool));
    assert_eq!(kind_of("nircmd.exe sendkeypress ctrl+a"), Some(Kind::InputTool));
}

#[test]
fn a_system_events_screen_script_is_caught() {
    for (command, words) in [
        (r#"osascript -e 'tell application "System Events" to keystroke "a"'"#, "keystroke"),
        (r#"osascript -e 'tell application "System Events" to key code 36'"#, "key code"),
        (r#"osascript -e 'tell application "System Events" to click at {10, 10}'"#, "click"),
    ] {
        let caught = bash(command).unwrap_or_else(|| panic!("{command} must be caught"));
        assert_eq!(caught.kind, Kind::ScreenScript, "{command}");
        assert_eq!(caught.words, words, "{command}");
    }
}

#[test]
fn a_script_that_changes_an_apps_windows_or_tabs_is_caught() {
    for (command, words) in [
        (r#"osascript -e 'tell application "Google Chrome" to activate'"#, "activate"),
        (
            r#"osascript -e 'tell application "Google Chrome"' -e 'set w to window 2' -e 'set index of w to 1' -e 'end tell'"#,
            "set index",
        ),
        (
            r#"osascript -e 'tell application "Google Chrome" to set active tab index of front window to 3'"#,
            "set active tab index",
        ),
        (
            "osascript <<'EOF'\ntell application \"Google Chrome\"\n  execute front window's active tab javascript \"document.title\"\nend tell\nEOF",
            "execute",
        ),
        (
            "osascript <<'EOF'\ntell application \"Google Chrome\"\n  set the URL of active tab of front window to \"http://localhost:3000\"\nend tell\nEOF",
            "set url",
        ),
    ] {
        let caught = bash(command).unwrap_or_else(|| panic!("{command} must be caught"));
        assert_eq!(caught.kind, Kind::AppWindows, "{command}");
        assert_eq!(caught.words, words, "{command}");
    }
}

#[test]
fn a_one_line_program_that_injects_input_is_caught() {
    assert_eq!(
        bash(r#"python3 -c "import pyautogui; pyautogui.click()""#),
        Some(Caught {
            kind: Kind::OneLiner,
            words: "pyautogui".to_string(),
        })
    );
    let send_keys = "[System.Windows.Forms.SendKeys]::SendWait('a')";
    assert_eq!(
        kind_of(&format!(r#"powershell -Command "{send_keys}""#)),
        Some(Kind::OneLiner)
    );
    assert_eq!(
        caught(Shell::PowerShell, send_keys).map(|caught| caught.kind),
        Some(Kind::OneLiner),
        "the PowerShell tool's command is itself the program"
    );
}

/* ---- design §7: what it lets through ----------------------------------------- */

#[test]
fn a_read_a_notice_or_a_name_only_written_is_let_through() {
    for command in [
        r#"osascript -e 'tell application "Google Chrome" to get URL of active tab of front window'"#,
        r#"osascript -e 'tell application "Google Chrome" to count windows'"#,
        r#"osascript -e 'tell application "System Events" to get name of every process whose frontmost is true'"#,
        r#"osascript -e 'set res to ""'"#,
        r#"osascript -e 'display notification "activate done"'"#,
        "echo cliclick",
        "grep -rn xdotool .",
        "which cliclick",
        "brew info cliclick",
        "open http://localhost:3000",
        "screencapture -x out.png",
        "python3 -c 'print(1)'",
        "cat pyautogui_notes.md",
        "cargo test -p runtime",
    ] {
        assert_eq!(bash(command), None, "{command} must be let through");
    }
}

#[test]
fn a_session_the_guard_does_not_stand_in_refuses_nothing() {
    // No Computer tool to go back to, or the person's `deskGuard` off.
    with_session(false, || assert_eq!(refused(Shell::Bash, "cliclick p"), None));
    with_session(true, || assert!(refused(Shell::Bash, "cliclick p").is_some()));
}

/* ---- the table: every name has a command it catches -------------------------- */

#[test]
fn every_name_in_the_table_catches_a_command() {
    for tool in TABLE.input_tools {
        let caught = bash(&format!("{tool} x")).unwrap_or_else(|| panic!("{tool}"));
        assert_eq!((caught.kind, caught.words.as_str()), (Kind::InputTool, *tool));
    }
    for verb in TABLE.screen_verbs {
        let verb = verb.join(" ");
        let command = format!(r#"osascript -e 'tell application "System Events" to {verb} x'"#);
        let caught = bash(&command).unwrap_or_else(|| panic!("{command}"));
        assert_eq!((caught.kind, caught.words.as_str()), (Kind::ScreenScript, verb.as_str()));
    }
    for verb in TABLE.window_verbs {
        let verb = verb.join(" ");
        let command = format!(r#"osascript -e 'tell application "Finder" to {verb} x'"#);
        let caught = bash(&command).unwrap_or_else(|| panic!("{command}"));
        assert_eq!((caught.kind, caught.words.as_str()), (Kind::AppWindows, verb.as_str()));
    }
    for property in TABLE.set_properties {
        let property = property.join(" ");
        let command =
            format!(r#"osascript -e 'tell application "Finder" to set {property} of window 1 to 1'"#);
        let caught = bash(&command).unwrap_or_else(|| panic!("{command}"));
        let words = format!("{} {property}", TABLE.set_verb);
        assert_eq!((caught.kind, caught.words.as_str()), (Kind::AppWindows, words.as_str()));
    }
    for flag in TABLE.code_flags {
        for name in TABLE.code_names {
            let command = format!(r#"interpreter {flag} "use {name}""#);
            let caught = bash(&command).unwrap_or_else(|| panic!("{command}"));
            assert_eq!((caught.kind, caught.words.as_str()), (Kind::OneLiner, *name));
        }
    }
}

/* ---- the refusal and where it stands ------------------------------------------ */

#[test]
fn the_refusal_names_what_it_saw_the_computer_tool_the_person_and_the_setting() {
    let text = refusal(&bash("cliclick p").expect("caught"));
    for part in ["`cliclick`", "Computer tool", "zerocode-computer", "tell the person", SETTING] {
        assert!(text.contains(part), "{part} missing from: {text}");
    }
}

fn cliclick_allowed() -> RuntimePermissionRuleConfig {
    RuntimePermissionRuleConfig::new(vec!["bash(cliclick:*)".to_string()], Vec::new(), Vec::new())
}

#[test]
fn full_access_and_an_allow_rule_do_not_answer_it() {
    let input = r#"{"command":"cliclick c:10,20"}"#;
    for mode in [PermissionMode::DangerFullAccess, PermissionMode::WorkspaceWrite] {
        let rules = cliclick_allowed();
        let policy = PermissionPolicy::new(mode).with_permission_rules(&rules);
        with_session(true, || match policy.authorize("bash", input, None) {
            PermissionOutcome::Deny { reason } => assert!(reason.contains("Computer tool"), "{reason}"),
            PermissionOutcome::Allow => panic!("{mode:?} with an allow rule ran it"),
        });
        // Without the guard standing, the same rule lets it run.
        with_session(false, || assert_eq!(policy.authorize("bash", input, None), PermissionOutcome::Allow));
    }
}

#[test]
fn check_bash_refuses_it_where_nobody_can_be_asked() {
    let enforcer = PermissionEnforcer::new(PermissionPolicy::new(PermissionMode::DangerFullAccess));
    with_session(true, || match enforcer.check_bash("bash -c 'cliclick p'") {
        EnforcementResult::Denied { reason, .. } => assert!(reason.contains("Computer tool"), "{reason}"),
        EnforcementResult::Allowed => panic!("check_bash ran it"),
    });
    with_session(true, || {
        assert_eq!(enforcer.check_bash("echo cliclick"), EnforcementResult::Allowed);
    });
}
