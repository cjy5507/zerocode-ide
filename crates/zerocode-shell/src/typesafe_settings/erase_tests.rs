//! What the window's delete does to a decision's records (t-87506).

use std::fs;

use serde_json::json;
use zerocode_core::jev::erase::{PENDING_FILE, lift_kept_seats, tidy_at_start};
use zerocode_core::jev::{CUSTOM_DECISIONS, JevMode, PLACEMENT, RECALL, STALL, builtin};

use super::*;

const PROVIDER: &str = "judgment-aaaa";
const PROJECT: &str = "Users-dev-work-0123456789abcdef";
const THRESHOLDS: &str = "thresholds.json";

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A folder of each shape a ledger can stand in under a zo home.
fn evidence_folders(home: &Path) -> [std::path::PathBuf; 3] {
    let jev = home.join("jev");
    [
        jev.clone(),
        jev.join("providers").join(PROVIDER),
        home.join("projects")
            .join(PROJECT)
            .join("state/smart-router"),
    ]
}

fn settings_in(home: &Path, smart: &Value) -> std::path::PathBuf {
    let path = home.join("settings.json");
    write(&path, &json!({ SMART_SETTINGS_KEY: smart }).to_string());
    path
}

fn records_of_two_seats(home: &Path) {
    let thresholds = json!([{ "seat": PLACEMENT.id }, { "seat": RECALL.id }]).to_string();
    for folder in evidence_folders(home) {
        write(&folder.join(PLACEMENT.ledger), "{\"at\":1}\n");
        write(&folder.join(RECALL.ledger), "{\"at\":2}\n");
        write(&folder.join(THRESHOLDS), &thresholds);
    }
}

fn seats_in_thresholds(folder: &Path) -> Vec<String> {
    let rows: Value = serde_json::from_slice(&fs::read(folder.join(THRESHOLDS)).unwrap()).unwrap();
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| row["seat"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_windows_delete_takes_the_seats_records_and_no_other_seats() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings_in(home.path(), &json!({}));
    records_of_two_seats(home.path());

    update_builtin_decision(
        &settings,
        PLACEMENT.id,
        "delete",
        &builtin::Decision::default(),
        None,
    )
    .unwrap();

    for folder in evidence_folders(home.path()) {
        assert!(
            !folder.join(PLACEMENT.ledger).exists(),
            "the deleted seat's ledger stayed in {}",
            folder.display()
        );
        assert!(
            folder.join(RECALL.ledger).exists(),
            "another seat's ledger went from {}",
            folder.display()
        );
        assert_eq!(seats_in_thresholds(&folder), [RECALL.id]);
    }
}

#[test]
fn a_refused_delete_and_the_other_actions_leave_the_records() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings_in(home.path(), &json!({}));
    records_of_two_seats(home.path());
    let stale = builtin::Decision {
        name: Some("What the editor last saw".to_string()),
        ..builtin::Decision::default()
    };

    update_builtin_decision(&settings, PLACEMENT.id, "delete", &stale, None).unwrap_err();
    update_builtin_decision(
        &settings,
        PLACEMENT.id,
        "reset",
        &builtin::Decision::default(),
        None,
    )
    .unwrap();

    for folder in evidence_folders(home.path()) {
        assert!(folder.join(PLACEMENT.ledger).exists());
        assert_eq!(seats_in_thresholds(&folder), [PLACEMENT.id, RECALL.id]);
    }
}

/// The lift brings back a mark set before it ran. A person deleting the seat
/// through the window after this version starts is the later deletion, and it
/// stands.
#[test]
fn a_kept_seat_the_person_deletes_in_the_window_stays_deleted() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings_in(home.path(), &json!({}));

    update_builtin_decision(
        &settings,
        STALL.id,
        "delete",
        &builtin::Decision::default(),
        None,
    )
    .unwrap();
    lift_kept_seats(&settings).unwrap();

    let root: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert!(builtin::in_root(&root, STALL.id).is_some_and(|decision| decision.deleted));
}

fn a_decision_in(home: &Path) -> String {
    let settings = json!({ SMART_SETTINGS_KEY: {} });
    let option = |name: &str, means: &str| ChoiceOption {
        name: name.to_string(),
        means: means.to_string(),
    };
    let draft = DecisionDraft {
        id: None,
        name: "Release risk".to_string(),
        question: "How risky is this change to ship today?".to_string(),
        answer: AnswerDraft::Choice {
            options: vec![
                option("hold", "Wait for a person."),
                option("ship", "Ship it now."),
            ],
        },
        at: custom::Point::TurnEnd,
        reads: BTreeSet::from([custom::Input::Task]),
        folders: None,
        act_from: custom::ACT_FROM_DEFAULT,
        max_per_hour: custom::HOURLY_DEFAULT,
    };
    save_custom_decision_at(home, draft, 1_000).unwrap();
    custom_decisions_at(home, &settings).unwrap().decisions[0]
        .draft
        .id
        .clone()
        .unwrap()
}

#[test]
fn the_windows_delete_of_a_persons_decision_takes_its_rows_and_no_others() {
    let home = tempfile::tempdir().unwrap();
    let id = a_decision_in(home.path());
    let other = "{\"decision\":\"another\",\"at\":2}\n";
    for folder in evidence_folders(home.path()) {
        let row = json!({ "decision": id, "at": 1 });
        write(
            &folder.join(CUSTOM_DECISIONS.ledger),
            &format!("{row}\n{other}"),
        );
    }

    delete_custom_decision_at(home.path(), &id).unwrap();

    for folder in evidence_folders(home.path()) {
        assert_eq!(
            fs::read_to_string(folder.join(CUSTOM_DECISIONS.ledger)).unwrap(),
            other,
            "in {}",
            folder.display()
        );
    }
}

#[test]
fn a_delete_the_store_refuses_leaves_the_rows() {
    let home = tempfile::tempdir().unwrap();
    let row = "{\"decision\":\"never-kept\",\"at\":1}\n";
    let ledger = home.path().join("jev").join(CUSTOM_DECISIONS.ledger);
    write(&ledger, row);

    assert!(matches!(
        delete_custom_decision_at(home.path(), "never-kept"),
        Err(custom::store::Error::Missing)
    ));

    assert_eq!(fs::read_to_string(&ledger).unwrap(), row);
}

/// The window's two start passes run on different threads and neither waits:
/// the store's tidy keeps to the store, and the lift to the settings.
#[test]
fn the_start_tidy_and_the_lift_end_alike_in_either_order() {
    let run = |tidy_first: bool| {
        let home = tempfile::tempdir().unwrap();
        let settings = settings_in(
            home.path(),
            &json!({ builtin::SETTING: { STALL.id: { "deleted": true, "mode": JevMode::Shadow.key() } } }),
        );
        let id = a_decision_in(home.path());
        let trash = custom::store::root(home.path()).join("trash");
        write(&trash.join("old.json"), "{}");
        if tidy_first {
            tidy_decision_store(home.path());
            lift_kept_seats(&settings).unwrap();
        } else {
            lift_kept_seats(&settings).unwrap();
            tidy_decision_store(home.path());
        }
        let root: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
        let listed = custom_decisions_at(home.path(), &json!({})).unwrap();
        assert_eq!(listed.decisions[0].draft.id.as_deref(), Some(id.as_str()));
        assert!(!trash.exists());
        root
    };
    let lifted = run(true);
    assert_eq!(lifted, run(false));
    assert_eq!(
        lifted[SMART_SETTINGS_KEY][STALL.setting],
        JevMode::Shadow.key()
    );
}

/// A folder where the ledger should be: on no platform can it be removed as a
/// file or read as one.
fn jam(ledger: &Path) {
    fs::create_dir_all(ledger.join("in-the-way")).unwrap();
}

#[test]
fn a_delete_whose_records_will_not_go_still_succeeds_and_leaves_a_note() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings_in(home.path(), &json!({}));
    jam(&home.path().join("jev").join(PLACEMENT.ledger));

    update_builtin_decision(
        &settings,
        PLACEMENT.id,
        "delete",
        &builtin::Decision::default(),
        None,
    )
    .expect("the mark is written, so the delete happened");

    let root: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert!(builtin::in_root(&root, PLACEMENT.id).is_some_and(|decision| decision.deleted));
    assert!(home.path().join(PENDING_FILE).exists());
}

#[test]
fn a_delete_of_a_persons_decision_whose_rows_will_not_go_still_succeeds_and_leaves_a_note() {
    let home = tempfile::tempdir().unwrap();
    let id = a_decision_in(home.path());
    jam(&home.path().join("jev").join(CUSTOM_DECISIONS.ledger));

    delete_custom_decision_at(home.path(), &id).expect("the list changed, so the delete happened");

    let settings = json!({ SMART_SETTINGS_KEY: {} });
    let listed = custom_decisions_at(home.path(), &settings).unwrap();
    assert!(listed.decisions.is_empty());
    assert!(home.path().join(PENDING_FILE).exists());
}

fn rows_of(ids: &[&str]) -> String {
    ids.iter()
        .map(|id| format!("{}\n", json!({ "decision": id, "at": 1 })))
        .collect()
}

#[test]
fn the_first_start_takes_the_rows_no_kept_decision_names() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings_in(home.path(), &json!({}));
    let id = a_decision_in(home.path());
    let no_decision = "{\"at\":3}\n";
    let ledger = home.path().join("jev").join(CUSTOM_DECISIONS.ledger);
    write(
        &ledger,
        &format!("{}{no_decision}", rows_of(&[&id, "d-gone"])),
    );

    tidy_at_start(&settings, true);

    assert_eq!(
        fs::read_to_string(&ledger).unwrap(),
        format!("{}{no_decision}", rows_of(&[&id]))
    );
}

#[test]
fn a_store_its_last_delete_emptied_still_judges_the_rows() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings_in(home.path(), &json!({}));
    let id = a_decision_in(home.path());
    delete_custom_decision_at(home.path(), &id).unwrap();
    let ledger = home.path().join("jev").join(CUSTOM_DECISIONS.ledger);
    write(&ledger, &rows_of(&["d-gone"]));

    tidy_at_start(&settings, true);

    assert!(!ledger.exists());
}

#[test]
fn a_store_that_cannot_be_read_keeps_every_row_and_the_next_start_tries_again() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings_in(home.path(), &json!({}));
    let id = a_decision_in(home.path());
    let ledger = home.path().join("jev").join(CUSTOM_DECISIONS.ledger);
    let rows = rows_of(&[&id, "d-gone"]);
    write(&ledger, &rows);
    let index = custom::store::root(home.path()).join("index.json");
    let whole = fs::read(&index).unwrap();
    fs::write(&index, b"{ torn").unwrap();

    tidy_at_start(&settings, true);
    assert_eq!(fs::read_to_string(&ledger).unwrap(), rows);

    fs::write(&index, whole).unwrap();
    tidy_at_start(&settings, true);
    assert_eq!(fs::read_to_string(&ledger).unwrap(), rows_of(&[&id]));
}

#[test]
fn a_home_with_no_store_keeps_every_row() {
    let home = tempfile::tempdir().unwrap();
    let settings = settings_in(home.path(), &json!({}));
    let ledger = home.path().join("jev").join(CUSTOM_DECISIONS.ledger);
    let rows = rows_of(&["d-unknown"]);
    write(&ledger, &rows);

    tidy_at_start(&settings, true);

    assert_eq!(fs::read_to_string(&ledger).unwrap(), rows);
}
