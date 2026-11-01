use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{
    PENDING_FILE, SWEPT_KEY, forget_custom, forget_seat, lift_kept_seats, may_write, seat_deleted,
    tidy_at_start,
};
use crate::jev::builtin;
use crate::jev::door::JEV_SETTINGS_KEY;
use crate::jev::{COMMAND_GUARD, CUSTOM_DECISIONS, JevMode, NOTIFY, RECALL, ROUTING, STALL};

const PROVIDER: &str = "judgment-aaaa";
const OTHER_PROVIDER: &str = "judgment-bbbb";
const PROJECT: &str = "Users-dev-work-0123456789abcdef";

fn home() -> tempfile::TempDir {
    tempfile::tempdir().expect("a temporary home")
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// Every folder a seat's ledger can stand in under a zo home.
fn evidence_folders(home: &Path) -> Vec<PathBuf> {
    let jev = home.join("jev");
    let project = home
        .join("projects")
        .join(PROJECT)
        .join("state/smart-router");
    vec![
        jev.clone(),
        jev.join("providers").join(PROVIDER),
        jev.join("providers").join(OTHER_PROVIDER),
        jev.join("provider-unconfigured"),
        project.clone(),
        project.join("providers").join(PROVIDER),
        project.join("providers").join(OTHER_PROVIDER),
        project.join("provider-unconfigured"),
    ]
}

fn settings_with(home: &Path, smart: &Value) -> PathBuf {
    let path = home.join("settings.json");
    write(&path, &json!({ "smart": smart }).to_string());
    path
}

fn smart_of(path: &Path) -> Value {
    let root: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    root["smart"].clone()
}

#[test]
fn deleting_a_seat_removes_its_file_from_every_folder_and_no_other_file() {
    let home = home();
    let settings = settings_with(home.path(), &json!({}));
    for folder in evidence_folders(home.path()) {
        write(&folder.join(RECALL.ledger), "{\"at\":1}\n");
        write(&folder.join(NOTIFY.ledger), "{\"at\":2}\n");
        write(&folder.join("judgment-memo.jsonl"), "{\"at\":3}\n");
    }

    let removed = forget_seat(&settings, RECALL.id).expect("the records go");

    assert_eq!(removed, evidence_folders(home.path()).len());
    for folder in evidence_folders(home.path()) {
        assert!(!folder.join(RECALL.ledger).exists(), "{}", folder.display());
        assert!(folder.join(NOTIFY.ledger).exists(), "{}", folder.display());
        assert!(folder.join("judgment-memo.jsonl").exists());
    }
}

#[test]
fn deleting_a_seat_takes_its_line_out_of_the_thresholds_beside_it() {
    let home = home();
    let settings = settings_with(home.path(), &json!({}));
    let folder = home.path().join("jev/providers").join(PROVIDER);
    write(
        &folder.join("thresholds.json"),
        &json!([{"seat":RECALL.id,"n":1},{"seat":NOTIFY.id,"n":2}]).to_string(),
    );

    forget_seat(&settings, RECALL.id).expect("the records go");

    let kept: Value =
        serde_json::from_slice(&fs::read(folder.join("thresholds.json")).unwrap()).unwrap();
    assert_eq!(kept, json!([{"seat":NOTIFY.id,"n":2}]));
}

#[test]
fn deleting_a_person_s_decision_removes_only_its_rows() {
    let home = home();
    for folder in evidence_folders(home.path()) {
        write(
            &folder.join(CUSTOM_DECISIONS.ledger),
            "{\"decision\":\"gone\",\"at\":1}\n{\"decision\":\"kept\",\"at\":2}\nnot json\n{\"decision\":\"gone\",\"at\":3}\n",
        );
    }

    let removed = forget_custom(home.path(), "gone").expect("the rows go");

    assert_eq!(removed, 2 * evidence_folders(home.path()).len());
    for folder in evidence_folders(home.path()) {
        assert_eq!(
            fs::read_to_string(folder.join(CUSTOM_DECISIONS.ledger)).unwrap(),
            "{\"decision\":\"kept\",\"at\":2}\nnot json\n",
            "{}",
            folder.display()
        );
    }
}

#[test]
fn deleting_the_only_decision_in_a_ledger_leaves_no_file() {
    let home = home();
    let ledger = home.path().join("jev").join(CUSTOM_DECISIONS.ledger);
    write(&ledger, "{\"decision\":\"gone\",\"at\":1}\n");

    forget_custom(home.path(), "gone").expect("the rows go");

    assert!(!ledger.exists());
}

#[test]
fn no_row_reaches_a_deleted_seat_in_any_folder() {
    let home = home();
    settings_with(
        home.path(),
        &json!({(builtin::SETTING): {(RECALL.id): {"deleted": true}}}),
    );
    for folder in evidence_folders(home.path()) {
        assert!(
            !may_write(&folder.join(RECALL.ledger)),
            "{}",
            folder.display()
        );
        assert!(
            may_write(&folder.join(NOTIFY.ledger)),
            "{}",
            folder.display()
        );
        assert!(may_write(&folder.join("judgment-memo.jsonl")));
    }
}

#[test]
fn a_ledger_outside_a_zo_home_is_written_as_asked() {
    let home = home();
    assert!(may_write(&home.path().join(RECALL.ledger)));
    assert!(may_write(&home.path().join("jev").join(RECALL.ledger)));
}

/// This machine's own settings: both kept seats deleted with no mode saved,
/// each setting written `off` by the deletion, and the switch on.
fn as_the_person_left_them(home: &Path) -> PathBuf {
    settings_with(
        home,
        &json!({
            (JEV_SETTINGS_KEY): {"enabled": true},
            (STALL.setting): "off",
            (ROUTING.setting): "off",
            (RECALL.setting): "off",
            (builtin::SETTING): {
                (STALL.id): {"deleted": true, "mode": null},
                (ROUTING.id): {"deleted": true, "mode": null},
                (RECALL.id): {"deleted": true, "mode": null},
                (COMMAND_GUARD.id): {"name": "kept"},
            }
        }),
    )
}

fn root_of(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn the_kept_seats_are_lifted_once_and_every_other_mark_stands() {
    let home = home();
    let settings = as_the_person_left_them(home.path());
    let never_deleted = json!({"smart": {(JEV_SETTINGS_KEY): {"enabled": true}}});
    assert_eq!(STALL.mode_in(&root_of(&settings)), JevMode::Off);
    assert_eq!(ROUTING.mode_in(&root_of(&settings)), JevMode::Off);

    assert!(lift_kept_seats(&settings).expect("the first run"));

    let root = root_of(&settings);
    assert_eq!(STALL.mode_in(&root), STALL.mode_in(&never_deleted));
    assert_eq!(STALL.mode_in(&root), JevMode::Auto, "stall asks again");
    assert_eq!(
        ROUTING.mode_in(&root),
        JevMode::Off,
        "routing stays on the rule"
    );
    assert_eq!(builtin::in_root(&root, STALL.id), None);
    assert_eq!(
        builtin::in_root(&root, ROUTING.id),
        None,
        "routing is not deleted"
    );
    let lifted = smart_of(&settings);
    assert_eq!(lifted[ROUTING.setting], "off");
    assert_eq!(lifted[builtin::SETTING][RECALL.id]["deleted"], true);
    assert_eq!(
        lifted[RECALL.setting], "off",
        "another deleted seat stays off"
    );
    assert_eq!(RECALL.mode_in(&root), JevMode::Off);
    assert_eq!(lifted[builtin::SETTING][COMMAND_GUARD.id]["name"], "kept");

    let before = fs::read(&settings).unwrap();
    assert!(!lift_kept_seats(&settings).expect("the second run"));
    assert_eq!(
        fs::read(&settings).unwrap(),
        before,
        "the second run changes nothing"
    );
}

#[test]
fn a_kept_seat_returns_to_the_mode_it_was_deleted_at() {
    let home = home();
    let settings = settings_with(
        home.path(),
        &json!({
            (JEV_SETTINGS_KEY): {"enabled": true},
            (STALL.setting): "off",
            (builtin::SETTING): {(STALL.id): {"deleted": true, "mode": "shadow"}}
        }),
    );

    assert!(lift_kept_seats(&settings).expect("the lift"));

    assert_eq!(STALL.mode_in(&root_of(&settings)), JevMode::Shadow);
    assert_eq!(smart_of(&settings)[STALL.setting], "shadow");
}

#[test]
fn a_seat_deleted_again_after_the_lift_stays_deleted() {
    let home = home();
    let settings = settings_with(home.path(), &json!({}));
    lift_kept_seats(&settings).expect("the first run");

    let mut root: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    root["smart"][builtin::SETTING] = json!({(STALL.id): {"deleted": true}});
    fs::write(&settings, root.to_string()).unwrap();

    assert!(!lift_kept_seats(&settings).expect("the second run"));
    assert_eq!(
        smart_of(&settings)[builtin::SETTING][STALL.id]["deleted"],
        true
    );
}

#[test]
fn a_kept_seat_deleted_before_the_first_lift_is_not_brought_back() {
    let home = home();
    let settings = home.path().join("settings.json");

    write(&settings, "{}");
    forget_seat(&settings, STALL.id).expect("the records go");
    let mut root: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    root["smart"][builtin::SETTING] = json!({(STALL.id): {"deleted": true}});
    fs::write(&settings, root.to_string()).unwrap();

    assert!(!lift_kept_seats(&settings).expect("the lift"));
    assert_eq!(
        smart_of(&settings)[builtin::SETTING][STALL.id]["deleted"],
        true
    );
}

#[test]
fn a_kept_seat_with_a_typed_edit_keeps_its_edit() {
    let home = home();
    let edit = json!({"deleted": true, "questionSchema": 2, "questionDeleted": false, "questions": {"cause": {"type": "choice", "instructions": "edited", "criteria": {"a": "A", "b": "B"}}}});
    let settings = settings_with(
        home.path(),
        &json!({(builtin::SETTING): {(STALL.id): edit.clone()}}),
    );

    assert!(!lift_kept_seats(&settings).expect("the lift"));

    assert_eq!(smart_of(&settings)[builtin::SETTING][STALL.id], edit);
}

#[test]
fn no_settings_file_is_left_alone() {
    let home = home();
    let settings = home.path().join("settings.json");
    assert!(!lift_kept_seats(&settings).expect("nothing to lift"));
    assert!(!settings.exists());
}

/// Marks for two seats the person deleted, and the two the lift brings back.
fn settings_marking_four(home: &Path) -> PathBuf {
    settings_with(
        home,
        &json!({ builtin::SETTING: {
            RECALL.id: { "deleted": true },
            NOTIFY.id: { "deleted": true },
            STALL.id: { "deleted": true },
            ROUTING.id: { "deleted": true },
        } }),
    )
}

fn plant_ledgers(home: &Path, seats: &[&crate::jev::JevUse]) {
    for folder in evidence_folders(home) {
        for seat in seats {
            write(&folder.join(seat.ledger), "{\"at\":1}\n");
        }
    }
}

fn a_ledger_stands(home: &Path, seat: &crate::jev::JevUse) -> Vec<bool> {
    evidence_folders(home)
        .iter()
        .map(|folder| folder.join(seat.ledger).exists())
        .collect()
}

fn none_stands(home: &Path, seat: &crate::jev::JevUse) -> bool {
    a_ledger_stands(home, seat).iter().all(|stands| !stands)
}

fn all_stand(home: &Path, seat: &crate::jev::JevUse) -> bool {
    a_ledger_stands(home, seat).iter().all(|stands| *stands)
}

/// A folder where the ledger should be: it cannot be removed as a file, on any
/// platform, until the test takes it away.
fn jam_ledger(home: &Path, seat: &crate::jev::JevUse) -> PathBuf {
    let jammed = home.join("jev").join(seat.ledger);
    fs::create_dir_all(jammed.join("in-the-way")).unwrap();
    jammed
}

#[test]
fn the_first_start_erases_what_marked_seats_left_and_keeps_the_lifted_seats() {
    let home = home();
    let settings = settings_marking_four(home.path());
    plant_ledgers(
        home.path(),
        &[&RECALL, &NOTIFY, &STALL, &ROUTING, &COMMAND_GUARD],
    );

    let lift = lift_kept_seats(&settings);
    tidy_at_start(&settings, lift.is_ok());

    assert!(none_stands(home.path(), &RECALL));
    assert!(none_stands(home.path(), &NOTIFY));
    assert!(
        all_stand(home.path(), &STALL),
        "a lifted seat keeps its records"
    );
    assert!(
        all_stand(home.path(), &ROUTING),
        "a lifted seat keeps its records"
    );
    assert!(
        all_stand(home.path(), &COMMAND_GUARD),
        "a seat never deleted"
    );
    assert_eq!(smart_of(&settings)[SWEPT_KEY], true);
}

#[test]
fn a_failed_lift_erases_nothing() {
    let home = home();
    let settings = settings_marking_four(home.path());
    plant_ledgers(home.path(), &[&RECALL, &STALL]);
    let other_writer = crate::settings_file_lock::SettingsFileLock::acquire(&settings).unwrap();

    let lift = lift_kept_seats(&settings);
    assert!(lift.is_err(), "the lock was held");
    tidy_at_start(&settings, lift.is_ok());
    drop(other_writer);

    assert!(all_stand(home.path(), &RECALL));
    assert!(all_stand(home.path(), &STALL));
    assert!(smart_of(&settings).get(SWEPT_KEY).is_none());
}

#[test]
fn a_later_start_with_no_note_reads_no_folder() {
    let home = home();
    let settings = settings_marking_four(home.path());
    tidy_at_start(&settings, true);
    assert_eq!(smart_of(&settings)[SWEPT_KEY], true);

    plant_ledgers(home.path(), &[&RECALL]);
    tidy_at_start(&settings, true);

    assert!(
        all_stand(home.path(), &RECALL),
        "the later start went looking"
    );
}

#[test]
fn a_later_start_erases_only_what_the_note_names_and_strikes_the_note() {
    let home = home();
    let settings = settings_marking_four(home.path());
    tidy_at_start(&settings, true);
    let jammed = jam_ledger(home.path(), &RECALL);
    seat_deleted(&settings, RECALL.id);
    assert!(
        home.path().join(PENDING_FILE).exists(),
        "the failure left no note"
    );

    fs::remove_dir_all(jammed).unwrap();
    plant_ledgers(home.path(), &[&RECALL, &NOTIFY]);
    tidy_at_start(&settings, true);

    assert!(none_stands(home.path(), &RECALL), "the named seat stayed");
    assert!(
        all_stand(home.path(), &NOTIFY),
        "a seat the note does not name went"
    );
    assert!(!home.path().join(PENDING_FILE).exists(), "the note stayed");
}

#[test]
fn a_note_stays_while_the_record_still_will_not_go() {
    let home = home();
    let settings = settings_marking_four(home.path());
    tidy_at_start(&settings, true);
    jam_ledger(home.path(), &RECALL);
    seat_deleted(&settings, RECALL.id);

    tidy_at_start(&settings, true);

    assert!(home.path().join(PENDING_FILE).exists());
}

#[test]
fn a_note_naming_a_seat_no_longer_deleted_erases_nothing() {
    let home = home();
    let settings = settings_marking_four(home.path());
    tidy_at_start(&settings, true);
    let jammed = jam_ledger(home.path(), &COMMAND_GUARD);
    seat_deleted(&settings, COMMAND_GUARD.id);
    fs::remove_dir_all(jammed).unwrap();
    plant_ledgers(home.path(), &[&COMMAND_GUARD]);

    tidy_at_start(&settings, true);

    assert!(all_stand(home.path(), &COMMAND_GUARD));
    assert!(!home.path().join(PENDING_FILE).exists());
}

#[test]
fn a_seat_the_first_start_cannot_clear_is_noted_and_the_first_start_is_done() {
    let home = home();
    let settings = settings_marking_four(home.path());
    plant_ledgers(home.path(), &[&NOTIFY]);
    jam_ledger(home.path(), &RECALL);

    tidy_at_start(&settings, true);

    assert!(none_stands(home.path(), &NOTIFY));
    assert!(home.path().join(PENDING_FILE).exists());
    assert_eq!(smart_of(&settings)[SWEPT_KEY], true);
}
