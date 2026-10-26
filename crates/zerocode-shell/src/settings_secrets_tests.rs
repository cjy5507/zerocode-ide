//! The move of a plain-text secret, against a fake zo (a script that writes
//! down its arguments and answers each door from a file), a fake settings
//! file under a temporary folder and `HeldKeys` for the keychain. No real zo,
//! keychain, `security` tool or settings file is reached.

use super::*;
use crate::api_routers::HeldKeys;
use std::os::unix::fs::PermissionsExt;

/// A value that stands in for a secret: forty characters, searched for in
/// everything the window says and never expected in any of it.
const VALUE: &str = "SENTINEL-not-a-secret-0123456789abcdefgh";
const NAME: &str = "FIXTURE_TOKEN";

fn listed(file: &Path) -> String {
    serde_json::json!({"literals": [{
        "file": file,
        "path": ["mcpServers", "fixture-server", "env", NAME],
        "name": NAME,
    }]})
    .to_string()
}

/// A folder holding the fake zo, its answers and the settings file.
struct Bench {
    dir: tempfile::TempDir,
}

impl Bench {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("bench");
        let bench = Self { dir };
        std::fs::write(
            bench.settings(),
            serde_json::to_string_pretty(&serde_json::json!({
                "mcpServers": {"fixture-server": {"command": "fixture", "env": {NAME: VALUE}}}
            }))
            .expect("settings"),
        )
        .expect("settings file");
        bench.answer("list", &listed(&bench.settings()), 0);
        bench.answer(
            "check",
            &serde_json::json!({"name": NAME, "resolves": true, "from": "keychain"}).to_string(),
            0,
        );
        bench.answer(
            "move",
            &serde_json::json!({"moved": true, "backup": bench.backup()}).to_string(),
            0,
        );
        bench.zo_script(&format!(
            "for a in \"$@\"; do printf '%s\\n' \"$a\" >> '{dir}/args.log'; done\n\
             printf -- '---\\n' >> '{dir}/args.log'\n\
             case \"$3\" in check|move) door=\"$3\";; *) door=list;; esac\n\
             cat '{dir}/'\"$door\".json\n\
             exit \"$(cat '{dir}/'\"$door\".code)\"\n",
            dir = bench.dir.path().display()
        ));
        bench
    }

    fn settings(&self) -> PathBuf {
        self.dir.path().join("settings.json")
    }

    fn backup(&self) -> PathBuf {
        self.dir.path().join("settings.json.bak-fixture")
    }

    fn bin(&self) -> PathBuf {
        self.dir.path().join("zo")
    }

    fn zo_script(&self, body: &str) {
        std::fs::write(self.bin(), format!("#!/bin/sh\n{body}")).expect("zo");
        std::fs::set_permissions(self.bin(), std::fs::Permissions::from_mode(0o755)).expect("mode");
    }

    fn answer(&self, door: &str, json: &str, code: i32) {
        std::fs::write(self.dir.path().join(format!("{door}.json")), json).expect("answer");
        std::fs::write(
            self.dir.path().join(format!("{door}.code")),
            code.to_string(),
        )
        .expect("code");
    }

    fn zo(&self) -> ZoDoor {
        ZoDoor::at(self.bin(), Vec::new())
    }

    /// Each call zo was given, as its arguments.
    fn calls(&self) -> Vec<Vec<String>> {
        let log = std::fs::read_to_string(self.dir.path().join("args.log")).unwrap_or_default();
        log.split("---\n")
            .filter(|call| !call.is_empty())
            .map(|call| call.lines().map(str::to_string).collect())
            .collect()
    }

    fn args_log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("args.log")).unwrap_or_default()
    }
}

fn strings(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_string()).collect()
}

fn list_call() -> Vec<String> {
    strings(&["settings", "secrets", "--json"])
}

fn check_call() -> Vec<String> {
    strings(&["settings", "secrets", "check", "--name", NAME, "--json"])
}

fn move_call(bench: &Bench) -> Vec<String> {
    vec![
        "settings".to_string(),
        "secrets".to_string(),
        "move".to_string(),
        "--file".to_string(),
        bench.settings().display().to_string(),
        "--path".to_string(),
        format!(r#"["mcpServers","fixture-server","env","{NAME}"]"#),
        "--name".to_string(),
        NAME.to_string(),
        "--json".to_string(),
    ]
}

/// The value is in none of what the window said or handed zo.
fn said_nowhere(bench: &Bench, said: &[String]) {
    for text in said.iter().chain(std::iter::once(&bench.args_log())) {
        assert!(
            !text.contains(VALUE),
            "the value was said: {} bytes searched",
            text.len()
        );
    }
}

#[test]
fn a_plain_text_secret_is_named_and_its_value_said_nowhere() {
    let bench = Bench::new();
    let notice = notice(&bench.zo(), &bench.settings());
    assert_eq!(
        notice,
        Some(SecretNotice {
            names: vec![NAME.to_string()]
        })
    );
    assert_eq!(bench.calls(), vec![list_call()]);
    said_nowhere(&bench, &[serde_json::to_string(&notice).expect("json")]);
}

#[test]
fn consent_puts_the_value_in_the_keychain_then_checks_then_moves() {
    let bench = Bench::new();
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.settings(), &keys);
    assert_eq!(
        outcome,
        MoveOutcome {
            moved: vec![Moved {
                name: NAME.to_string(),
                backup: bench.backup().display().to_string(),
            }],
            stopped: None,
            left: Vec::new(),
        }
    );
    assert_eq!(
        keys.held(&format!("dev.zerocode.key.{NAME}")).as_deref(),
        Some(VALUE),
        "the item zo reads holds exactly the value"
    );
    assert_eq!(
        bench.calls(),
        vec![list_call(), check_call(), move_call(&bench)]
    );
    said_nowhere(&bench, &[serde_json::to_string(&outcome).expect("json")]);
}

#[test]
fn a_different_value_under_the_name_is_not_overwritten() {
    let bench = Bench::new();
    let keys = HeldKeys::default();
    keys.write(&keychain_service(NAME), "fixture-other-value")
        .expect("held");
    let outcome = move_all(&bench.zo(), &bench.settings(), &keys);
    assert_eq!(
        outcome.stopped,
        Some(Stopped {
            name: NAME.to_string(),
            reason: "keychain_differs",
            why: None,
            keychain_kept: false,
        })
    );
    assert!(outcome.moved.is_empty());
    assert_eq!(
        keys.held(&keychain_service(NAME)).as_deref(),
        Some("fixture-other-value")
    );
    assert_eq!(
        bench.calls(),
        vec![list_call()],
        "neither check nor move is asked"
    );
    said_nowhere(&bench, &[serde_json::to_string(&outcome).expect("json")]);
}

#[test]
fn the_same_value_already_in_the_keychain_is_used_as_it_is() {
    let bench = Bench::new();
    let keys = HeldKeys::default();
    keys.write(&keychain_service(NAME), VALUE).expect("held");
    let outcome = move_all(&bench.zo(), &bench.settings(), &keys);
    assert_eq!(outcome.stopped, None);
    assert_eq!(outcome.moved.len(), 1);
    assert_eq!(
        bench.calls(),
        vec![list_call(), check_call(), move_call(&bench)]
    );
}

#[test]
fn a_name_that_does_not_resolve_is_not_moved() {
    let bench = Bench::new();
    bench.answer(
        "check",
        &serde_json::json!({"name": NAME, "resolves": false, "why": "fixture: no item answered"})
            .to_string(),
        1,
    );
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.settings(), &keys);
    assert_eq!(
        outcome.stopped,
        Some(Stopped {
            name: NAME.to_string(),
            reason: "not_resolved",
            why: Some("fixture: no item answered".to_string()),
            keychain_kept: true,
        })
    );
    assert_eq!(
        bench.calls(),
        vec![list_call(), check_call()],
        "move is not asked"
    );
    said_nowhere(&bench, &[serde_json::to_string(&outcome).expect("json")]);
}

#[test]
fn a_refused_move_says_why_and_leaves_the_file_as_it_was() {
    let bench = Bench::new();
    bench.answer(
        "move",
        &serde_json::json!({"moved": false, "why": "fixture: the file changed after it was read"})
            .to_string(),
        1,
    );
    let before = std::fs::read(bench.settings()).expect("before");
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.settings(), &keys);
    assert_eq!(
        outcome.stopped,
        Some(Stopped {
            name: NAME.to_string(),
            reason: "move_refused",
            why: Some("fixture: the file changed after it was read".to_string()),
            keychain_kept: true,
        })
    );
    assert_eq!(std::fs::read(bench.settings()).expect("after"), before);
    assert_eq!(
        keys.held(&keychain_service(NAME)).as_deref(),
        Some(VALUE),
        "the item stays for the next try, as the page says"
    );
    said_nowhere(&bench, &[serde_json::to_string(&outcome).expect("json")]);
}

#[test]
fn a_reason_from_zo_that_carries_the_value_is_not_said() {
    let bench = Bench::new();
    bench.answer(
        "move",
        &serde_json::json!({"moved": false, "why": format!("fixture: {VALUE} differs")})
            .to_string(),
        1,
    );
    let outcome = move_all(&bench.zo(), &bench.settings(), &HeldKeys::default());
    assert_eq!(
        outcome.stopped.as_ref().map(|stop| stop.reason),
        Some("move_refused")
    );
    assert_eq!(
        outcome.stopped.as_ref().and_then(|stop| stop.why.clone()),
        None
    );
    assert!(
        !serde_json::to_string(&outcome)
            .expect("json")
            .contains(VALUE)
    );
}

#[test]
fn a_zo_without_the_door_shows_nothing() {
    let bench = Bench::new();
    bench.zo_script("echo \"unknown argument '$1'\" >&2\nexit 2\n");
    assert_eq!(notice(&bench.zo(), &bench.settings()), None);
    let missing = ZoDoor::at(bench.dir.path().join("no-zo-here"), Vec::new());
    assert_eq!(notice(&missing, &bench.settings()), None);
    let outcome = move_all(&bench.zo(), &bench.settings(), &HeldKeys::default());
    assert!(outcome.moved.is_empty());
    assert_eq!(
        outcome.stopped.as_ref().map(|stop| stop.reason),
        Some("zo_silent")
    );
}

#[test]
fn nothing_written_as_plain_text_shows_nothing() {
    let bench = Bench::new();
    bench.answer("list", r#"{"literals":[]}"#, 0);
    assert_eq!(notice(&bench.zo(), &bench.settings()), None);
}

#[test]
fn a_cell_in_another_file_is_neither_named_nor_read() {
    let bench = Bench::new();
    let other = bench.dir.path().join("other.json");
    std::fs::copy(bench.settings(), &other).expect("other");
    bench.answer("list", &listed(&other), 0);
    assert_eq!(notice(&bench.zo(), &bench.settings()), None);
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.settings(), &keys);
    assert!(outcome.moved.is_empty() && outcome.stopped.is_none());
    assert_eq!(keys.held(&keychain_service(NAME)), None);
}

#[test]
fn a_cell_that_changed_since_the_listing_stops_before_the_keychain() {
    let bench = Bench::new();
    std::fs::write(bench.settings(), r#"{"mcpServers":{}}"#).expect("changed");
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.settings(), &keys);
    assert_eq!(
        outcome.stopped.as_ref().map(|stop| stop.reason),
        Some("cell_changed")
    );
    assert_eq!(keys.held(&keychain_service(NAME)), None);
    assert_eq!(bench.calls(), vec![list_call()]);
}

#[test]
fn a_value_with_spaces_around_it_is_not_trimmed_into_another() {
    let bench = Bench::new();
    std::fs::write(
        bench.settings(),
        serde_json::json!({"mcpServers": {"fixture-server": {"env": {NAME: format!(" {VALUE} ")}}}})
            .to_string(),
    )
    .expect("padded");
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.settings(), &keys);
    assert_eq!(
        outcome.stopped.as_ref().map(|stop| stop.reason),
        Some("padded")
    );
    assert_eq!(keys.held(&keychain_service(NAME)), None);
    said_nowhere(&bench, &[serde_json::to_string(&outcome).expect("json")]);
}
