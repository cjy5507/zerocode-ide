//! The move of a plain-text secret, against a fake zo (a script that writes
//! down its arguments and answers each door from a file), a fake settings
//! file under a temporary folder standing for the home folder and `HeldKeys`
//! for the keychain. No real zo, keychain, `security` tool or settings file is
//! reached. The watch over a door is tried on a made-up process and clock:
//! no real signal is sent and no real wait is timed.

use super::*;
use crate::api_routers::HeldKeys;
use std::os::unix::fs::PermissionsExt;

/// A value that stands in for a secret: forty characters, searched for in
/// everything the window says and never expected in any of it.
const VALUE: &str = "SENTINEL-not-a-secret-0123456789abcdefgh";
const NAME: &str = "FIXTURE_TOKEN";
/// The value a project's own file holds in the same cell — never the person's.
const PROJECT_VALUE: &str = "PROJECT-not-a-secret-9876543210zyxwvuts";
/// A second plain-text secret in the same file.
const VALUE_2: &str = "SENTINEL-two-not-a-secret-zyxwvuts98765";
const NAME_2: &str = "FIXTURE_TOKEN_2";

fn listed(file: &Path) -> String {
    serde_json::json!({"literals": [{
        "file": file,
        "path": ["mcpServers", "fixture-server", "env", NAME],
        "name": NAME,
    }]})
    .to_string()
}

/// A folder standing for the home folder: the fake zo, its answers, and the
/// settings file at `.zo/settings.json`.
struct Bench {
    dir: tempfile::TempDir,
}

impl Bench {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("bench");
        std::fs::create_dir(dir.path().join(".zo")).expect(".zo");
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
             if [ -e '{dir}/.zo/settings.json.lock' ]; then echo \"$door\" >> '{dir}/lock.log'; fi\n\
             cat '{dir}/'\"$door\".json\n\
             if [ -f '{dir}/'\"$door\".then ]; then . '{dir}/'\"$door\".then; fi\n\
             exit \"$(cat '{dir}/'\"$door\".code)\"\n",
            dir = bench.dir.path().display()
        ));
        // A move that answers it moved rewrites the cell as zo does; one that
        // answers it did not leaves the file alone.
        bench.then(
            "move",
            &format!(
                "if grep -q '\"moved\":true' '{dir}/move.json'; then\n{rewrite}fi\n",
                dir = bench.dir.path().display(),
                rewrite = rewrite(&bench, "mv")
            ),
        );
        bench
    }

    fn settings(&self) -> PathBuf {
        self.dir.path().join(".zo").join("settings.json")
    }

    /// The settings file, trusted from the bench as the home folder.
    fn file(&self) -> SettingsFile {
        SettingsFile::under(self.dir.path().to_path_buf(), self.settings()).expect("below")
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

    /// What the door does after it has answered, as shell lines — a change
    /// to the settings file made between two steps of the window's.
    fn then(&self, door: &str, lines: &str) {
        std::fs::write(self.dir.path().join(format!("{door}.then")), lines).expect("then");
    }

    /// A file of the project's, holding a value of its own in the same cell.
    fn project(&self) -> PathBuf {
        let project = self.dir.path().join("project.json");
        std::fs::write(
            &project,
            serde_json::json!({"mcpServers": {"fixture-server": {"env": {NAME: PROJECT_VALUE}}}})
                .to_string(),
        )
        .expect("project");
        project
    }

    /// The doors zo was asked while the window held the settings file's lock.
    fn asked_under_the_lock(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("lock.log")).unwrap_or_default()
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
    let notice = notice(&bench.zo(), &bench.file());
    assert_eq!(
        notice,
        Some(SecretNotice {
            names: vec![NAME.to_string()],
            ..SecretNotice::default()
        })
    );
    assert_eq!(bench.calls(), vec![list_call()]);
    said_nowhere(&bench, &[serde_json::to_string(&notice).expect("json")]);
}

#[test]
fn consent_puts_the_value_in_the_keychain_then_checks_then_moves() {
    let bench = Bench::new();
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
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
    assert_eq!(
        bench.asked_under_the_lock(),
        "",
        "zo is asked with the settings lock let go, so it can take it"
    );
    said_nowhere(&bench, &[serde_json::to_string(&outcome).expect("json")]);
}

#[test]
fn a_different_value_under_the_name_is_not_overwritten() {
    let bench = Bench::new();
    let keys = HeldKeys::default();
    keys.write(&keychain_service(NAME), "fixture-other-value")
        .expect("held");
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
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
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
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
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
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
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
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
    let outcome = move_all(&bench.zo(), &bench.file(), &HeldKeys::default());
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
    assert_eq!(notice(&bench.zo(), &bench.file()), None);
    let missing = ZoDoor::at(bench.dir.path().join("no-zo-here"), Vec::new());
    assert_eq!(notice(&missing, &bench.file()), None);
    let outcome = move_all(&bench.zo(), &bench.file(), &HeldKeys::default());
    assert!(outcome.moved.is_empty());
    assert_eq!(
        outcome.stopped.as_ref().map(|stop| stop.reason),
        Some("zo_silent")
    );
}

#[test]
fn a_zo_that_says_the_move_is_not_supported_here_shows_nothing() {
    let bench = Bench::new();
    let row = serde_json::json!({
        "file": bench.settings(),
        "path": ["mcpServers", "fixture-server", "env", NAME],
        "name": NAME,
    });
    for literals in [Vec::new(), vec![row]] {
        bench.answer(
            "list",
            &serde_json::json!({
                "literals": literals,
                "supported": false,
                "why": "fixture: no keychain here",
            })
            .to_string(),
            0,
        );
        assert_eq!(notice(&bench.zo(), &bench.file()), None, "{literals:?}");
        let keys = HeldKeys::default();
        let outcome = move_all(&bench.zo(), &bench.file(), &keys);
        assert_eq!(reason(&outcome), Some("zo_silent"));
        nothing_kept(&bench, &keys, &outcome);
    }
}

#[test]
fn a_machine_that_keeps_no_key_does_not_ask_zo_at_all() {
    let bench = Bench::new();
    let off = || bench.zo().supported(false);
    let before = std::fs::read(bench.settings()).expect("settings");
    assert_eq!(notice(&off(), &bench.file()), None);
    assert_eq!(off().ask(&["settings", "secrets", "--json"]), None);
    let keys = HeldKeys::default();
    let outcome = move_all(&off(), &bench.file(), &keys);
    assert_eq!(reason(&outcome), Some("zo_silent"));
    assert!(outcome.moved.is_empty() && outcome.left.is_empty());
    assert_eq!(keys.held(&keychain_service(NAME)), None);
    assert_eq!(
        bench.calls(),
        Vec::<Vec<String>>::new(),
        "no door of zo's was asked"
    );
    assert_eq!(std::fs::read(bench.settings()).expect("settings"), before);
    // The same door, on a machine that keeps keys, is asked as before.
    assert!(notice(&bench.zo().supported(true), &bench.file()).is_some());
    assert_eq!(bench.calls(), vec![list_call()]);
}

#[test]
fn the_window_asks_zo_only_where_a_keychain_keeps_the_key() {
    let config = tempfile::tempdir().expect("config");
    assert_eq!(
        ZoDoor::of_this_window(config.path()).map(|zo| zo.supported),
        Some(crate::accounts::KEYCHAIN_AVAILABLE)
    );
}

#[test]
fn nothing_written_as_plain_text_shows_nothing() {
    let bench = Bench::new();
    bench.answer("list", r#"{"literals":[]}"#, 0);
    assert_eq!(notice(&bench.zo(), &bench.file()), None);
}

#[test]
fn a_cell_in_another_file_is_neither_named_nor_read() {
    let bench = Bench::new();
    let other = bench.dir.path().join("other.json");
    std::fs::copy(bench.settings(), &other).expect("other");
    bench.answer("list", &listed(&other), 0);
    assert_eq!(notice(&bench.zo(), &bench.file()), None);
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert!(outcome.moved.is_empty() && outcome.stopped.is_none());
    assert_eq!(keys.held(&keychain_service(NAME)), None);
}

#[test]
fn a_cell_that_changed_since_the_listing_stops_before_the_keychain() {
    let bench = Bench::new();
    std::fs::write(bench.settings(), r#"{"mcpServers":{}}"#).expect("changed");
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
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
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(
        outcome.stopped.as_ref().map(|stop| stop.reason),
        Some("padded")
    );
    assert_eq!(keys.held(&keychain_service(NAME)), None);
    said_nowhere(&bench, &[serde_json::to_string(&outcome).expect("json")]);
}

/// A keychain double that does `meanwhile` the first time it is read — a
/// change to the settings file landing between the window's read of the cell
/// and its keychain write — and notes, at each read and write, whether the
/// window's one guard over keychain items was held.
struct Watched {
    keys: HeldKeys,
    meanwhile: std::cell::RefCell<Option<Box<dyn FnOnce()>>>,
    guarded: std::cell::RefCell<Vec<(&'static str, bool)>>,
}

impl Watched {
    fn new(meanwhile: impl FnOnce() + 'static) -> Self {
        Self {
            keys: HeldKeys::default(),
            meanwhile: std::cell::RefCell::new(Some(Box::new(meanwhile))),
            guarded: std::cell::RefCell::default(),
        }
    }
}

impl RouterKeys for Watched {
    fn read(&self, service: &str) -> Result<Option<String>, crate::api_routers::RouterRefusal> {
        self.guarded
            .borrow_mut()
            .push(("read", crate::typesafe_settings::key_items_held_here()));
        let meanwhile = self.meanwhile.borrow_mut().take();
        if let Some(meanwhile) = meanwhile {
            meanwhile();
        }
        self.keys.read(service)
    }
    fn write(&self, service: &str, secret: &str) -> Result<(), crate::api_routers::RouterRefusal> {
        self.guarded
            .borrow_mut()
            .push(("write", crate::typesafe_settings::key_items_held_here()));
        self.keys.write(service, secret)
    }
    fn delete(&self, service: &str) -> Result<(), crate::api_routers::RouterRefusal> {
        self.keys.delete(service)
    }
}

fn reason(outcome: &MoveOutcome) -> Option<&'static str> {
    outcome.stopped.as_ref().map(|stop| stop.reason)
}

/// Nothing reached the keychain, zo was asked for the listing alone, and
/// neither the person's value nor the project's is in anything said.
fn nothing_kept(bench: &Bench, keys: &HeldKeys, outcome: &MoveOutcome) {
    assert_eq!(
        keys.held(&keychain_service(NAME)),
        None,
        "no keychain write"
    );
    assert!(outcome.moved.is_empty());
    assert!(bench.calls().iter().all(|call| *call == list_call()));
    let said = serde_json::to_string(outcome).expect("json");
    said_nowhere(bench, std::slice::from_ref(&said));
    assert!(!said.contains(PROJECT_VALUE) && !bench.args_log().contains(PROJECT_VALUE));
}

#[test]
fn a_settings_path_that_is_a_link_is_neither_named_nor_read() {
    let bench = Bench::new();
    let project = bench.project();
    std::fs::remove_file(bench.settings()).expect("remove");
    std::os::unix::fs::symlink(&project, bench.settings()).expect("link");
    assert_eq!(notice(&bench.zo(), &bench.file()), None);
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(reason(&outcome), Some("file_swapped"));
    nothing_kept(&bench, &keys, &outcome);
}

/// A settings file laid by stow or home-manager: its own name is a link to
/// the person's dotfiles, every folder above it a folder of its own.
fn linked_by_stow(bench: &Bench) -> PathBuf {
    let dotfiles = bench.dir.path().join("dotfiles");
    std::fs::create_dir(&dotfiles).expect("dotfiles");
    let target = dotfiles.join("zo-settings.json");
    std::fs::rename(bench.settings(), &target).expect("move into dotfiles");
    std::os::unix::fs::symlink(&target, bench.settings()).expect("link");
    target
}

/// t-77216: zo reads a settings file whose own name is a link through to
/// its target when the target passes its checks (t-78117), and says so on
/// the row (`via_link`). The line names what zo read there, beside the file
/// the link stands on — and the window moves none of it: its move refuses a
/// link before a byte is read. A row zo read through a link at any other
/// path, or one claiming a link where the settings file is a plain one, is
/// not named. Before, the link dropped the file out of the line with nothing
/// said.
#[test]
fn a_settings_file_linked_by_stow_names_what_zo_read_through_it_and_moves_none() {
    let bench = Bench::new();
    let target = linked_by_stow(&bench);
    let row = |file: &Path| {
        serde_json::json!({
            "file": file,
            "path": ["mcpServers", "fixture-server", "env", NAME],
            "name": NAME,
            "via_link": target,
        })
    };
    bench.answer(
        "list",
        &serde_json::json!({"literals": [row(&bench.settings())], "skipped": []}).to_string(),
        0,
    );
    let shown = notice(&bench.zo(), &bench.file());
    assert_eq!(
        shown,
        Some(SecretNotice {
            linked: vec![NAME.to_string()],
            linked_to: Some(target.display().to_string()),
            ..SecretNotice::default()
        })
    );
    said_nowhere(&bench, &[serde_json::to_string(&shown).expect("json")]);
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(reason(&outcome), Some("file_swapped"));
    nothing_kept(&bench, &keys, &outcome);

    // At another path, it is not the person's file the line speaks of.
    let other = bench.dir.path().join("other.json");
    bench.answer(
        "list",
        &serde_json::json!({"literals": [row(&other)]}).to_string(),
        0,
    );
    assert_eq!(notice(&bench.zo(), &bench.file()), None);

    // A plain settings file is pinned and read by the window itself: a row
    // claiming a link there is not taken on zo's word.
    let plain = Bench::new();
    plain.answer(
        "list",
        &serde_json::json!({"literals": [{
            "file": plain.settings(),
            "path": ["mcpServers", "fixture-server", "env", NAME],
            "name": NAME,
            "via_link": target,
        }]})
        .to_string(),
        0,
    );
    assert_eq!(notice(&plain.zo(), &plain.file()), None);
}

/// t-77216: a settings file zo did not read — its link's target another
/// user's, writable by others, gone, in the workspace, unreadable — is said
/// with zo's word for why (t-78117's five), which the page turns into its
/// own sentence, and zo's sentence for a word the page does not know. A
/// file outside the person's folder, or a row without a word, is left out.
#[test]
fn a_settings_file_zo_did_not_read_is_said_with_its_word_for_why() {
    let bench = Bench::new();
    linked_by_stow(&bench);
    let skipped = |file: &Path, reason: &str| serde_json::json!({"file": file, "reason": reason, "why": "fixture: the link's target is writable by others"});
    bench.answer(
        "list",
        &serde_json::json!({
            "literals": [],
            "skipped": [
                skipped(&bench.settings(), "link_target_writable_by_others"),
                skipped(Path::new("/elsewhere/settings.json"), "unreadable"),
                skipped(&bench.settings(), "Not A Word"),
                {"file": bench.settings(), "why": "fixture: no word"},
            ],
        })
        .to_string(),
        0,
    );
    assert_eq!(
        notice(&bench.zo(), &bench.file()),
        Some(SecretNotice {
            skipped: vec![Skipped {
                file: bench.settings().display().to_string(),
                reason: "link_target_writable_by_others".to_string(),
                why: "fixture: the link's target is writable by others".to_string(),
            }],
            ..SecretNotice::default()
        })
    );
    // An older zo says nothing of what it skipped: nothing to show.
    bench.answer("list", r#"{"literals":[]}"#, 0);
    assert_eq!(notice(&bench.zo(), &bench.file()), None);
}

#[test]
fn a_link_put_in_place_after_the_listing_is_refused_before_the_read() {
    let bench = Bench::new();
    let project = bench.project();
    bench.then(
        "list",
        &format!(
            "rm '{settings}'\nln -s '{project}' '{settings}'\n",
            settings = bench.settings().display(),
            project = project.display()
        ),
    );
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(reason(&outcome), Some("file_swapped"));
    nothing_kept(&bench, &keys, &outcome);
}

#[test]
fn a_file_put_in_place_of_the_settings_after_the_listing_is_refused_before_the_read() {
    let bench = Bench::new();
    let project = bench.project();
    bench.then(
        "list",
        &format!(
            "cp '{project}' '{settings}.new'\nmv '{settings}.new' '{settings}'\n",
            settings = bench.settings().display(),
            project = project.display()
        ),
    );
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(reason(&outcome), Some("file_swapped"));
    nothing_kept(&bench, &keys, &outcome);
}

#[test]
fn a_cell_that_changes_between_the_read_and_the_keychain_write_is_not_kept() {
    for (label, cell) in [
        (
            "another value",
            "fixture-another-value-written-meanwhile".to_string(),
        ),
        ("its reference", format!("${{{NAME}}}")),
    ] {
        let bench = Bench::new();
        let settings = bench.settings();
        let keys = Watched::new(move || {
            std::fs::write(
                &settings,
                serde_json::json!({"mcpServers": {"fixture-server": {"env": {NAME: cell}}}})
                    .to_string(),
            )
            .expect("changed in place");
        });
        let outcome = move_all(&bench.zo(), &bench.file(), &keys);
        assert_eq!(reason(&outcome), Some("cell_changed"), "{label}");
        nothing_kept(&bench, &keys.keys, &outcome);
    }
}

#[test]
fn a_file_swapped_between_the_read_and_the_keychain_write_is_not_kept() {
    let bench = Bench::new();
    let settings = bench.settings();
    let project = bench.project();
    let keys = Watched::new(move || {
        let fresh = settings.with_extension("fresh");
        std::fs::copy(&project, &fresh).expect("copy");
        std::fs::rename(&fresh, &settings).expect("swap");
    });
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(reason(&outcome), Some("file_swapped"));
    nothing_kept(&bench, &keys.keys, &outcome);
}

#[test]
fn a_cell_that_already_refers_to_its_name_is_refused() {
    let bench = Bench::new();
    std::fs::write(
        bench.settings(),
        serde_json::json!({"mcpServers": {"fixture-server": {"env": {NAME: format!("${{{NAME}}}")}}}})
            .to_string(),
    )
    .expect("reference");
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(reason(&outcome), Some("already_reference"));
    nothing_kept(&bench, &keys, &outcome);
}

#[test]
fn the_keychain_is_read_and_written_under_the_guard_every_key_save_takes() {
    let bench = Bench::new();
    let keys = Watched::new(|| {});
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(outcome.stopped, None);
    assert_eq!(
        *keys.guarded.borrow(),
        vec![("read", true), ("write", true)],
        "the check for another value and the write are one step"
    );
    let saved = Watched::new(|| {});
    crate::typesafe_settings::save_key_at("fixture.service", "fixture-key", "empty", &saved)
        .expect("saved");
    crate::typesafe_settings::remove_key_at("fixture.service", &saved).expect("removed");
    assert_eq!(
        *saved.guarded.borrow(),
        vec![("write", true)],
        "the window's other key saves take the same guard"
    );
    assert!(!crate::typesafe_settings::key_items_held_here());
}

#[test]
fn a_second_move_of_the_same_file_while_one_runs_is_refused() {
    let bench = Bench::new();
    let (bin, settings) = (bench.bin(), bench.file());
    let second = std::sync::Arc::new(std::sync::Mutex::new(None));
    let seen = second.clone();
    let keys = Watched::new(move || {
        let outcome = std::thread::spawn(move || {
            move_all(
                &ZoDoor::at(bin, Vec::new()),
                &settings,
                &HeldKeys::default(),
            )
        })
        .join()
        .expect("second move");
        *seen.lock().expect("seen") = Some(outcome);
    });
    let first = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(first.stopped, None);
    let second = second.lock().expect("second").take().expect("ran");
    assert_eq!(reason(&second), Some("already_moving"));
    assert!(second.moved.is_empty());
    assert_eq!(
        bench.calls(),
        vec![list_call(), check_call(), move_call(&bench)],
        "the second move asked zo nothing"
    );
    let after = move_all(&bench.zo(), &bench.file(), &HeldKeys::default());
    assert_ne!(
        reason(&after),
        Some("already_moving"),
        "the guard goes with the move"
    );
}

#[test]
fn a_listing_of_another_shape_is_not_trusted_and_nothing_is_read() {
    let settings_with_note = serde_json::json!({
        "metadata": {"note": VALUE},
        "mcpServers": {"fixture-server": {"env": {NAME: VALUE, "OTHER": VALUE}, "headers": {NAME: VALUE}}}
    });
    let good = serde_json::json!(["mcpServers", "fixture-server", "env", NAME]);
    for (label, rows) in [
        (
            "not under mcpServers",
            vec![serde_json::json!(["metadata", "note"])],
        ),
        (
            "the last key is not the name",
            vec![serde_json::json!([
                "mcpServers",
                "fixture-server",
                "env",
                "OTHER"
            ])],
        ),
        (
            "not an env cell",
            vec![serde_json::json!([
                "mcpServers",
                "fixture-server",
                "headers",
                NAME
            ])],
        ),
        (
            "deeper than an env cell",
            vec![serde_json::json!([
                "mcpServers",
                "fixture-server",
                "env",
                NAME,
                "x"
            ])],
        ),
        (
            "one row of another shape among good ones",
            vec![good.clone(), serde_json::json!(["metadata", "note"])],
        ),
    ] {
        let bench = Bench::new();
        std::fs::write(bench.settings(), settings_with_note.to_string()).expect("settings");
        let literals = rows
            .into_iter()
            .map(|path| serde_json::json!({"file": bench.settings(), "path": path, "name": NAME}))
            .collect::<Vec<_>>();
        bench.answer(
            "list",
            &serde_json::json!({"literals": literals}).to_string(),
            0,
        );
        assert_eq!(notice(&bench.zo(), &bench.file()), None, "{label}");
        let keys = HeldKeys::default();
        let outcome = move_all(&bench.zo(), &bench.file(), &keys);
        assert_eq!(reason(&outcome), Some("zo_silent"), "{label}");
        nothing_kept(&bench, &keys, &outcome);
    }
}

/// Two plain-text secrets in one file, each listed, and a zo whose move
/// rewrites the cell it is named as zo does: a new file renamed over the old.
fn two_cells() -> Bench {
    let bench = Bench::new();
    std::fs::write(
        bench.settings(),
        serde_json::to_string_pretty(&serde_json::json!({
            "mcpServers": {
                "fixture-server": {"command": "fixture", "env": {NAME: VALUE}},
                "second-server": {"command": "fixture", "env": {NAME_2: VALUE_2}},
            }
        }))
        .expect("settings"),
    )
    .expect("settings file");
    bench.answer(
        "list",
        &serde_json::json!({"literals": [
            {"file": bench.settings(), "path": ["mcpServers", "fixture-server", "env", NAME], "name": NAME},
            {"file": bench.settings(), "path": ["mcpServers", "second-server", "env", NAME_2], "name": NAME_2},
        ]})
        .to_string(),
        0,
    );
    bench.then("move", &rewrite(&bench, "mv"));
    bench
}

/// Shell lines that write the settings file with the cell named by the move's
/// `--name` turned into its reference: renamed over the file with `mv`, or
/// copied into it in place with `cat`. `also` is a further `sed` expression.
fn rewrite_with(bench: &Bench, how: &str, also: &str) -> String {
    let settings = bench.settings().display().to_string();
    let put = match how {
        "mv" => format!("mv '{settings}.zo-new' '{settings}'"),
        _ => format!("cat '{settings}.zo-new' > '{settings}'; rm '{settings}.zo-new'"),
    };
    format!(
        "case \"$9\" in {NAME}) v='{VALUE}';; {NAME_2}) v='{VALUE_2}';; esac\n\
         sed -e \"s/\\\"$v\\\"/\\\"\\${{$9}}\\\"/\"{also} '{settings}' > '{settings}.zo-new'\n\
         {put}\n"
    )
}

fn rewrite(bench: &Bench, how: &str) -> String {
    rewrite_with(bench, how, "")
}

fn check_call_of(name: &str) -> Vec<String> {
    strings(&["settings", "secrets", "check", "--name", name, "--json"])
}

fn move_call_of(bench: &Bench, server: &str, name: &str) -> Vec<String> {
    vec![
        "settings".to_string(),
        "secrets".to_string(),
        "move".to_string(),
        "--file".to_string(),
        bench.settings().display().to_string(),
        "--path".to_string(),
        format!(r#"["mcpServers","{server}","env","{name}"]"#),
        "--name".to_string(),
        name.to_string(),
        "--json".to_string(),
    ]
}

#[test]
fn a_settings_folder_that_is_a_link_to_a_project_is_neither_named_nor_read() {
    let bench = Bench::new();
    let project = bench.dir.path().join("project");
    std::fs::create_dir(&project).expect("project");
    std::fs::write(
        project.join("settings.json"),
        serde_json::json!({"mcpServers": {"fixture-server": {"env": {NAME: PROJECT_VALUE}}}})
            .to_string(),
    )
    .expect("project settings");
    std::fs::remove_dir_all(bench.dir.path().join(".zo")).expect("remove .zo");
    std::os::unix::fs::symlink(&project, bench.dir.path().join(".zo")).expect("link .zo");
    assert_eq!(notice(&bench.zo(), &bench.file()), None);
    let keys = Watched::new(|| {});
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(reason(&outcome), Some("file_swapped"));
    assert!(
        keys.guarded.borrow().is_empty(),
        "the keychain is neither read nor written"
    );
    nothing_kept(&bench, &keys.keys, &outcome);
}

#[test]
fn a_settings_folder_turned_into_a_link_after_the_listing_is_refused_before_the_read() {
    let bench = Bench::new();
    let project = bench.dir.path().join("project");
    std::fs::create_dir(&project).expect("project");
    std::fs::write(
        project.join("settings.json"),
        serde_json::json!({"mcpServers": {"fixture-server": {"env": {NAME: PROJECT_VALUE}}}})
            .to_string(),
    )
    .expect("project settings");
    bench.then(
        "list",
        &format!(
            "mv '{home}/.zo' '{home}/.zo-was'\nln -s '{project}' '{home}/.zo'\n",
            home = bench.dir.path().display(),
            project = project.display()
        ),
    );
    let keys = Watched::new(|| {});
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(reason(&outcome), Some("file_swapped"));
    assert!(keys.guarded.borrow().is_empty());
    nothing_kept(&bench, &keys.keys, &outcome);
}

#[test]
fn a_path_that_leaves_the_folder_it_is_trusted_from_is_not_taken() {
    let home = PathBuf::from("/fixture-home");
    for path in [
        "/elsewhere/.zo/settings.json",
        "/fixture-home/../elsewhere/settings.json",
        "/fixture-home",
    ] {
        assert!(
            SettingsFile::under(home.clone(), PathBuf::from(path)).is_none(),
            "{path}"
        );
    }
    assert!(SettingsFile::under(home, PathBuf::from("/fixture-home/.zo/settings.json")).is_some());
}

#[test]
fn every_cell_is_moved_through_the_file_each_move_leaves() {
    let bench = two_cells();
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(outcome.stopped, None, "{outcome:?}");
    assert_eq!(
        outcome
            .moved
            .iter()
            .map(|moved| moved.name.as_str())
            .collect::<Vec<_>>(),
        vec![NAME, NAME_2]
    );
    assert_eq!(keys.held(&keychain_service(NAME)).as_deref(), Some(VALUE));
    assert_eq!(
        keys.held(&keychain_service(NAME_2)).as_deref(),
        Some(VALUE_2)
    );
    assert_eq!(
        bench.calls(),
        vec![
            list_call(),
            check_call_of(NAME),
            move_call_of(&bench, "fixture-server", NAME),
            check_call_of(NAME_2),
            move_call_of(&bench, "second-server", NAME_2),
        ]
    );
    let left = std::fs::read_to_string(bench.settings()).expect("left");
    assert!(
        left.contains(&format!("${{{NAME}}}")) && left.contains(&format!("${{{NAME_2}}}")),
        "both cells are references now"
    );
    let said = serde_json::to_string(&outcome).expect("json");
    said_nowhere(&bench, std::slice::from_ref(&said));
    assert!(!said.contains(VALUE_2) && !bench.args_log().contains(VALUE_2));
}

#[test]
fn a_move_that_leaves_more_changed_than_its_cell_is_not_gone_on_from() {
    for how in ["mv", "in place"] {
        let bench = two_cells();
        // zo's move also puts the project's value in the second cell.
        bench.then(
            "move",
            &rewrite_with(&bench, how, &format!(" -e 's/{VALUE_2}/{PROJECT_VALUE}/'")),
        );
        let keys = HeldKeys::default();
        let outcome = move_all(&bench.zo(), &bench.file(), &keys);
        assert_eq!(outcome.moved.len(), 1, "{how}");
        assert_eq!(
            outcome.stopped,
            Some(Stopped {
                name: NAME_2.to_string(),
                reason: "file_swapped",
                why: None,
                keychain_kept: false,
            }),
            "{how}"
        );
        assert_eq!(keys.held(&keychain_service(NAME_2)), None, "{how}");
        assert_eq!(
            bench.calls().len(),
            3,
            "{how}: nothing asked for the second"
        );
        let said = serde_json::to_string(&outcome).expect("json");
        assert!(!said.contains(PROJECT_VALUE) && !said.contains(VALUE_2));
    }
}

#[test]
fn a_file_put_in_place_while_zo_checks_is_not_handed_to_zos_move() {
    for (label, meanwhile, expected) in [
        (
            "a new plain file with the very same bytes",
            "cp '{s}' '{s}.fresh'\nmv '{s}.fresh' '{s}'\n",
            "file_swapped",
        ),
        (
            "the same file with another cell changed",
            "sed -e 's/\"command\": \"fixture\"/\"command\": \"other\"/' '{s}' > '{s}.edit'\ncat '{s}.edit' > '{s}'\n",
            "cell_changed",
        ),
        (
            "the same file given a second name",
            "ln '{s}' '{s}.linked'\n",
            "file_swapped",
        ),
    ] {
        let bench = Bench::new();
        let before = std::fs::read(bench.settings()).expect("before");
        bench.then(
            "check",
            &meanwhile.replace("{s}", &bench.settings().display().to_string()),
        );
        let keys = HeldKeys::default();
        let outcome = move_all(&bench.zo(), &bench.file(), &keys);
        assert_eq!(
            outcome.stopped,
            Some(Stopped {
                name: NAME.to_string(),
                reason: expected,
                why: None,
                keychain_kept: true,
            }),
            "{label}"
        );
        assert_eq!(
            bench.calls(),
            vec![list_call(), check_call()],
            "{label}: move is not asked"
        );
        if label.starts_with("a new") {
            assert_eq!(std::fs::read(bench.settings()).expect("after"), before);
        }
        said_nowhere(&bench, &[serde_json::to_string(&outcome).expect("json")]);
    }
}

/// A file of the project's standing on the settings file's own inode — a hard
/// link, which no folder handle or `O_NOFOLLOW` can see — one way or the
/// other: the settings path linked to the project's file, or the project's
/// path linked to the settings file.
fn hard_linked(bench: &Bench, settings_is_the_link: bool) {
    if settings_is_the_link {
        let project = bench.project();
        std::fs::remove_file(bench.settings()).expect("remove");
        std::fs::hard_link(&project, bench.settings()).expect("hard link");
    } else {
        std::fs::hard_link(bench.settings(), bench.dir.path().join("project.json"))
            .expect("hard link");
    }
}

#[test]
fn a_settings_file_hard_linked_to_a_project_file_is_neither_named_nor_read() {
    for settings_is_the_link in [true, false] {
        let bench = Bench::new();
        hard_linked(&bench, settings_is_the_link);
        let label = if settings_is_the_link {
            "the settings path is a hard link to the project's file"
        } else {
            "the project's path is a hard link to the settings file"
        };
        assert_eq!(notice(&bench.zo(), &bench.file()), None, "{label}");
        let keys = Watched::new(|| {});
        let outcome = move_all(&bench.zo(), &bench.file(), &keys);
        assert_eq!(reason(&outcome), Some("file_swapped"), "{label}");
        assert!(
            keys.guarded.borrow().is_empty(),
            "{label}: the keychain is neither read nor written"
        );
        nothing_kept(&bench, &keys.keys, &outcome);
    }
}

#[test]
fn a_spelling_of_the_settings_file_from_outside_its_root_is_not_taken() {
    let bench = Bench::new();
    // Another folder, outside the home folder, whose `alias` is a link to the
    // person's `.zo`: zo's spelling through it stands on the very same file.
    let outside = tempfile::tempdir().expect("outside");
    std::os::unix::fs::symlink(bench.dir.path().join(".zo"), outside.path().join("alias"))
        .expect("alias");
    bench.answer(
        "list",
        &listed(&outside.path().join("alias").join("settings.json")),
        0,
    );
    assert_eq!(notice(&bench.zo(), &bench.file()), None);
    let keys = Watched::new(|| {});
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert!(outcome.moved.is_empty() && outcome.stopped.is_none());
    assert!(
        keys.guarded.borrow().is_empty(),
        "the keychain is neither read nor written"
    );
    assert_eq!(bench.calls(), vec![list_call(), list_call()]);
}

#[test]
fn a_hard_link_made_after_the_listing_is_refused_before_the_read() {
    let bench = Bench::new();
    bench.then(
        "list",
        &format!(
            "ln '{settings}' '{project}'\n",
            settings = bench.settings().display(),
            project = bench.dir.path().join("project.json").display()
        ),
    );
    let keys = Watched::new(|| {});
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(reason(&outcome), Some("file_swapped"));
    assert!(
        keys.guarded.borrow().is_empty(),
        "the keychain is neither read nor written"
    );
    nothing_kept(&bench, &keys.keys, &outcome);
}

#[test]
fn a_hard_link_made_between_the_read_and_the_keychain_write_is_not_kept() {
    let bench = Bench::new();
    let (settings, project) = (bench.settings(), bench.dir.path().join("project.json"));
    let keys = Watched::new(move || {
        std::fs::hard_link(&settings, &project).expect("hard link");
    });
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(reason(&outcome), Some("file_swapped"));
    assert_eq!(
        *keys.guarded.borrow(),
        vec![("read", true)],
        "the keychain is read for another value, and never written"
    );
    nothing_kept(&bench, &keys.keys, &outcome);
}

#[test]
fn a_file_zo_left_with_a_second_name_is_not_gone_on_from() {
    let bench = two_cells();
    bench.then(
        "move",
        &format!(
            "{rewrite}ln '{settings}' '{project}'\n",
            rewrite = rewrite(&bench, "mv"),
            settings = bench.settings().display(),
            project = bench.dir.path().join("project.json").display()
        ),
    );
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(outcome.moved.len(), 1);
    assert_eq!(
        outcome.stopped,
        Some(Stopped {
            name: NAME_2.to_string(),
            reason: "file_swapped",
            why: None,
            keychain_kept: false,
        })
    );
    assert_eq!(keys.held(&keychain_service(NAME_2)), None);
    assert_eq!(bench.calls().len(), 3, "nothing asked for the second");
}

/// The last cell's move answered, and the file it left is not the one read
/// with that cell turned into its reference: the move is not reported
/// finished. The cell zo said it moved is said moved, with its backup; the
/// keychain item stays.
fn unconfirmed(name: &str) -> Option<Stopped> {
    Some(Stopped {
        name: name.to_string(),
        reason: "unconfirmed",
        why: None,
        keychain_kept: true,
    })
}

#[test]
fn a_single_cell_zo_says_it_moved_but_left_as_it_was_is_not_reported_finished() {
    let bench = Bench::new();
    bench.then("move", "");
    let before = std::fs::read(bench.settings()).expect("before");
    let keys = HeldKeys::default();
    let outcome = move_all(&bench.zo(), &bench.file(), &keys);
    assert_eq!(
        outcome,
        MoveOutcome {
            moved: vec![Moved {
                name: NAME.to_string(),
                backup: bench.backup().display().to_string(),
            }],
            stopped: unconfirmed(NAME),
            left: Vec::new(),
        }
    );
    assert_eq!(std::fs::read(bench.settings()).expect("after"), before);
    assert_eq!(keys.held(&keychain_service(NAME)).as_deref(), Some(VALUE));
    said_nowhere(&bench, &[serde_json::to_string(&outcome).expect("json")]);
}

#[test]
fn a_last_cell_whose_move_changed_more_than_the_cell_is_not_reported_finished() {
    for how in ["mv", "in place"] {
        let bench = two_cells();
        // The second move also changes the first server's command.
        bench.then(
            "move",
            &format!(
                "if [ \"$9\" = '{NAME_2}' ]; then\n{also}else\n{plain}fi\n",
                also = rewrite_with(
                    &bench,
                    how,
                    " -e 's/\"command\": \"fixture\"/\"command\": \"other\"/'"
                ),
                plain = rewrite(&bench, how),
            ),
        );
        let keys = HeldKeys::default();
        let outcome = move_all(&bench.zo(), &bench.file(), &keys);
        assert_eq!(
            outcome
                .moved
                .iter()
                .map(|moved| moved.name.as_str())
                .collect::<Vec<_>>(),
            vec![NAME, NAME_2],
            "{how}"
        );
        assert_eq!(outcome.stopped, unconfirmed(NAME_2), "{how}");
        assert!(outcome.left.is_empty(), "{how}");
        let said = serde_json::to_string(&outcome).expect("json");
        said_nowhere(&bench, std::slice::from_ref(&said));
        assert!(!said.contains(VALUE_2));
    }
}

#[test]
fn a_last_cell_whose_file_is_given_a_second_name_is_not_reported_finished() {
    let bench = Bench::new();
    bench.then(
        "move",
        &format!(
            "{rewrite}ln '{settings}' '{project}'\n",
            rewrite = rewrite(&bench, "mv"),
            settings = bench.settings().display(),
            project = bench.dir.path().join("project.json").display()
        ),
    );
    let outcome = move_all(&bench.zo(), &bench.file(), &HeldKeys::default());
    assert_eq!(outcome.moved.len(), 1);
    assert_eq!(outcome.stopped, unconfirmed(NAME));
}

/// A made-up door: a clock that moves only when the watch waits, and a zo
/// that closes its output and ends at the times given (never, for `None`).
/// It notes each kill of zo's group, with the time and whether zo had been
/// reaped by then — once reaped, zo's number may be another group's.
struct MadeUp {
    start: Instant,
    at: Duration,
    output_closes: Option<Duration>,
    ends: Option<Duration>,
    wait_fails: bool,
    reaped: bool,
    kills: Vec<(Duration, bool)>,
    reaps: Vec<Duration>,
}

impl MadeUp {
    fn new(output_closes: Option<Duration>, ends: Option<Duration>) -> Self {
        Self {
            start: Instant::now(),
            at: Duration::ZERO,
            output_closes,
            ends,
            wait_fails: false,
            reaped: false,
            kills: Vec::new(),
            reaps: Vec::new(),
        }
    }
}

impl Answering for MadeUp {
    fn now(&self) -> Instant {
        self.start + self.at
    }

    fn printed(&mut self, tick: Duration) -> Option<Option<String>> {
        match self.output_closes {
            Some(closes) if closes <= self.at + tick => {
                self.at = self.at.max(closes);
                Some(Some(r#"{"literals":[]}"#.to_string()))
            }
            _ => {
                self.at += tick;
                None
            }
        }
    }

    fn reaped(&mut self) -> std::io::Result<Option<ExitStatus>> {
        use std::os::unix::process::ExitStatusExt;
        if self.wait_fails {
            return Err(std::io::Error::from_raw_os_error(libc::ECHILD));
        }
        if self.ends.is_some_and(|ends| ends <= self.at) && !self.reaped {
            self.reaped = true;
            self.reaps.push(self.at);
        }
        Ok(self.reaped.then(|| ExitStatus::from_raw(0)))
    }

    fn give_up(&mut self) {
        self.kills.push((self.at, self.reaped));
        if !self.reaped {
            self.reaped = true;
            self.reaps.push(self.at);
        }
    }

    fn pause(&mut self, tick: Duration) {
        self.at += tick;
    }
}

const WAIT: Duration = Duration::from_secs(10);

/// The one kill came at the wait, to a zo not yet reaped.
fn killed_once_at_the_wait_unreaped(door: &MadeUp) {
    assert_eq!(door.kills.len(), 1, "{:?}", door.kills);
    let (at, reaped) = door.kills[0];
    assert!(at >= WAIT && at < WAIT + TICK, "at the wait: {at:?}");
    assert!(!reaped, "the group is killed while zo's number is its own");
    assert_eq!(door.reaps, vec![at], "reaped once, after the kill");
}

#[test]
fn a_zo_that_ended_while_a_child_holds_its_output_is_killed_before_it_is_reaped() {
    // zo ends at once; a child of its — in its group, or one that left it —
    // keeps the output open past the wait.
    let mut door = MadeUp::new(None, Some(Duration::ZERO));
    assert_eq!(watch(&mut door, WAIT), None);
    killed_once_at_the_wait_unreaped(&door);
}

#[test]
fn a_zo_that_closes_its_output_and_keeps_running_is_killed_at_the_wait() {
    let mut door = MadeUp::new(Some(Duration::ZERO), None);
    assert_eq!(watch(&mut door, WAIT), None);
    killed_once_at_the_wait_unreaped(&door);
}

#[test]
fn a_zo_that_answers_and_ends_within_the_wait_is_reaped_and_sent_nothing() {
    let mut door = MadeUp::new(
        Some(Duration::from_millis(40)),
        Some(Duration::from_millis(70)),
    );
    assert_eq!(
        watch(&mut door, WAIT),
        Some((0, serde_json::json!({"literals": []})))
    );
    assert!(door.kills.is_empty(), "{:?}", door.kills);
    assert_eq!(door.reaps.len(), 1);
    assert!(door.reaps[0] >= Duration::from_millis(70) && door.reaps[0] < WAIT);
}

#[test]
fn a_zo_the_window_can_no_longer_wait_on_is_sent_nothing() {
    let mut door = MadeUp::new(Some(Duration::ZERO), Some(Duration::ZERO));
    door.wait_fails = true;
    assert_eq!(watch(&mut door, WAIT), None);
    assert!(
        door.kills.is_empty(),
        "a number the window cannot wait on may be another's: {:?}",
        door.kills
    );
}

/// The made-up zo and clock, with zo's output a real pipe whose write end a
/// child that left zo's group still holds: the pipe is read as a real zo's
/// is, and no real process or signal is involved.
struct Held {
    output: Output,
    door: MadeUp,
}

impl Answering for Held {
    fn now(&self) -> Instant {
        self.door.now()
    }

    fn printed(&mut self, tick: Duration) -> Option<Option<String>> {
        let printed = self.output.printed(Duration::ZERO);
        if printed.is_none() {
            self.door.pause(tick);
        }
        printed
    }

    fn reaped(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.door.reaped()
    }

    fn give_up(&mut self) {
        self.output.close();
        self.door.give_up();
    }

    fn pause(&mut self, tick: Duration) {
        self.door.pause(tick);
    }
}

/// Whether the window still holds its end of the pipe: a write to the other
/// end goes through while it does, and breaks once it is let go.
fn still_read(writer: &mut std::io::PipeWriter) -> bool {
    use std::io::Write;
    match writer.write(b"x") {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => false,
        Err(error) => panic!("the pipe: {error}"),
    }
}

#[test]
fn an_output_a_child_that_left_zos_group_holds_is_let_go_at_the_wait() {
    let (reader, mut writer) = std::io::pipe().expect("pipe");
    let mut door = Held {
        output: Output::of(reader).expect("output"),
        // zo ends at once; its detached child holds the output open.
        door: MadeUp::new(None, Some(Duration::ZERO)),
    };
    assert!(still_read(&mut writer), "read while the door is watched");
    assert_eq!(watch(&mut door, WAIT), None);
    killed_once_at_the_wait_unreaped(&door.door);
    assert!(
        let_go(&mut writer),
        "the window's end of the output is closed at the wait: no reader, no descriptor is left"
    );
}

/// Whether the window's end of the pipe is let go: writes to the other end
/// until one breaks, up to twice [`PRINTED_BYTES`]. A process another test
/// started in the moment the pipe was made may hold a copy of the window's
/// end (`pipe` and its close-on-exec flag are two steps on macOS) and keep a
/// write going until it ends; a window still reading takes every byte.
fn let_go(writer: &mut std::io::PipeWriter) -> bool {
    use std::io::Write;
    let chunk = [b'x'; 8192];
    let mut written = 0;
    while written < 2 * PRINTED_BYTES {
        match writer.write(&chunk) {
            Ok(wrote) => written += wrote,
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => return true,
            Err(error) => panic!("the pipe: {error}"),
        }
    }
    false
}

#[test]
fn an_output_past_its_bound_is_let_go_and_not_read_as_an_answer() {
    let (reader, mut writer) = std::io::pipe().expect("pipe");
    // A child that left zo's group and prints without end, ending only when
    // its output breaks — or, for a window that reads it all, after twice the
    // bound, closing it.
    let printing = std::thread::spawn(move || {
        use std::io::Write;
        let chunk = [b'x'; 8192];
        let mut written = 0;
        while written < 2 * PRINTED_BYTES {
            match writer.write(&chunk) {
                Ok(wrote) => written += wrote,
                Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                    return (written, true);
                }
                Err(error) => panic!("the pipe: {error}"),
            }
        }
        (written, false)
    });
    // Each look waits up to a tick for the output and drains it to its end
    // or its bound; the looks go on until the output is done with.
    let mut output = Output::of(reader).expect("output");
    let answered = loop {
        if let Some(printed) = output.printed(TICK) {
            break printed;
        }
    };
    let (written, broke) = printing.join().expect("printing");
    assert_eq!(answered, None, "an output past its bound is no answer");
    assert!(broke, "the window let its end go: {written} bytes written");
    assert!(
        written <= PRINTED_BYTES + 1024 * 1024,
        "read to the bound, not to the end: {written} bytes"
    );
}
