//! What a deleted seat left outside its own ledger: its review cases, its memo
//! rows and the judgment cache's rows that name it (t-88024).

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use super::{
    PENDING_FILE, SWEPT_KEY, evidence_folders, forget_seat, home, lift_kept_seats, seat_deleted,
    settings_marking_four, settings_with, smart_of, tidy_at_start, write,
};
use crate::jev::erase::shared_ledger_lock;
use crate::jev::learning::Case;
use crate::jev::learning::store::{MAX_CASES, Recorded, Store};
use crate::jev::{
    AGENT_TOOL, COMMAND_GUARD, JUDGMENT_CACHE, JevUse, NOTIFY, RECALL, STALL, builtin, door, memo,
};

/// The word the sweep's mark is stored under, pinned as written.
const LEFTOVERS_KEY: &str = "deletedLeftoversSwept";
const ANSWER: &str = "{\"answers\":{}}";

fn review(home: &Path) -> PathBuf {
    home.join("jev/review")
}

fn memo_of(folder: &Path) -> PathBuf {
    folder.join(memo::MEMO_FILE)
}

fn cache_of(folder: &Path) -> PathBuf {
    folder.join(JUDGMENT_CACHE.ledger)
}

/// A case file as the store names it, holding only what a sweep reads.
fn plant_case(home: &Path, seat: &JevUse, index: usize) -> PathBuf {
    let path = review(home).join(format!("{:064x}", index + 1));
    let case = path.with_extension("json");
    write(&case, &json!({ "seat": seat.id }).to_string());
    case
}

/// A real case, as the door lets one be made.
fn real_case(seat: &JevUse, state: &str) -> Case {
    let settings =
        door::JevSettings::from_root(&json!({"smart":{"jev":{"enabled":true,"workspaces":["*"]}}}));
    let cleared = door::may_send(
        seat,
        &door::Asking {
            key: true,
            settings: &settings,
            workspace: Some(if cfg!(windows) {
                r"C:\work\project"
            } else {
                "/work/project"
            }),
            sent_today: 0,
        },
        json!({
            "model": "jev-latest",
            "state": { "context": state },
            "questions": { "q": { "type": "noul", "instructions": "Is it useful?" } },
        }),
    )
    .unwrap();
    Case::from_cleared(
        seat,
        &cleared,
        &json!({"model": "m", "answers": {"q": {"type": "noul", "noul": 0.5}}}),
        10,
    )
    .unwrap()
}

/// Rows of `gone` among rows that must survive byte for byte: another seat's in
/// an odd key order and spacing, one that is not JSON, one that is not UTF-8,
/// one that names no seat, and a last line with no newline.
fn rows_around(gone: &JevUse, other: &JevUse) -> (Vec<u8>, Vec<u8>) {
    let gone = format!("{{\"seat\":\"{}\",\"at\":1}}\n", gone.id).into_bytes();
    let kept: Vec<Vec<u8>> = vec![
        format!("{{ \"at\": 2,  \"seat\": \"{}\" }}\n", other.id).into_bytes(),
        b"not json\n".to_vec(),
        b"{\"seat\":\"\xff\"}\n".to_vec(),
        b"{\"at\":3}\n".to_vec(),
        format!("{{\"seat\":\"{}\",\"at\":4}}", other.id).into_bytes(),
    ];
    let mut whole = gone.clone();
    for (index, line) in kept.iter().enumerate() {
        whole.extend_from_slice(line);
        if index == 0 {
            whole.extend_from_slice(&gone);
        }
    }
    (whole, kept.concat())
}

fn plant_rows(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// `seat`'s case and rows, beside those of a seat that stays. Returns the case.
fn plant_leftovers(home: &Path, seat: &JevUse) -> PathBuf {
    let (rows, _) = rows_around(seat, &COMMAND_GUARD);
    for folder in evidence_folders(home) {
        plant_rows(&memo_of(&folder), &rows);
        plant_rows(&cache_of(&folder), &rows);
    }
    plant_case(home, seat, 0)
}

fn leftovers_stand(home: &Path, seat: &JevUse, case: &Path) -> bool {
    let (rows, _) = rows_around(seat, &COMMAND_GUARD);
    case.exists()
        && evidence_folders(home).iter().all(|folder| {
            fs::read(memo_of(folder)).unwrap() == rows
                && fs::read(cache_of(folder)).unwrap() == rows
        })
}

fn leftovers_gone(home: &Path, seat: &JevUse, case: &Path) -> bool {
    let (_, kept) = rows_around(seat, &COMMAND_GUARD);
    !case.exists()
        && evidence_folders(home).iter().all(|folder| {
            fs::read(memo_of(folder)).unwrap() == kept
                && fs::read(cache_of(folder)).unwrap() == kept
        })
}

/// Settings as a machine stands after B: the first tidy done and `seat` marked.
fn settings_after_the_first_tidy(home: &Path, seat: &JevUse) -> PathBuf {
    settings_with(
        home,
        &json!({
            "keptSeatsLifted": true,
            (SWEPT_KEY): true,
            (builtin::SETTING): { (seat.id): { "deleted": true } },
        }),
    )
}

/// A folder where the memo should be: it cannot be read as a file, on any
/// platform, until the test takes it away.
fn jam_memo(home: &Path) -> PathBuf {
    let jammed = memo_of(&home.join("jev"));
    if jammed.exists() {
        fs::remove_file(&jammed).unwrap();
    }
    fs::create_dir_all(jammed.join("in-the-way")).unwrap();
    jammed
}

#[test]
fn deleting_a_seat_takes_its_cases_and_rows_and_leaves_the_rest_to_the_byte() {
    let home = home();
    let settings = settings_with(home.path(), &json!({}));
    let (rows, kept) = rows_around(&RECALL, &NOTIFY);
    for folder in evidence_folders(home.path()) {
        plant_rows(&memo_of(&folder), &rows);
        plant_rows(&cache_of(&folder), &rows);
    }
    let gone = plant_case(home.path(), &RECALL, 0);
    let labels = gone.with_extension("outcomes");
    write(&labels, "[]");
    let stays = plant_case(home.path(), &NOTIFY, 1);
    let stays_labels = stays.with_extension("outcomes");
    write(&stays_labels, "[]");
    let unreadable = review(home.path()).join(format!("{:064x}.json", 3));
    write(&unreadable, "not a case");
    let case_bytes = fs::read(&stays).unwrap();

    forget_seat(&settings, RECALL.id).expect("the records go");

    assert!(!gone.exists(), "the deleted seat's case stayed");
    assert!(!labels.exists(), "the deleted seat's labels stayed");
    assert_eq!(fs::read(&stays).unwrap(), case_bytes);
    assert!(stays_labels.exists());
    assert!(unreadable.exists(), "a case nobody can read names no seat");
    for folder in evidence_folders(home.path()) {
        assert_eq!(fs::read(memo_of(&folder)).unwrap(), kept, "{folder:?}");
        assert_eq!(fs::read(cache_of(&folder)).unwrap(), kept, "{folder:?}");
    }
}

#[test]
fn a_real_case_and_its_labels_go_with_their_seat() {
    let home = home();
    let settings = settings_with(home.path(), &json!({}));
    let store = Store::at(home.path());
    let case = real_case(&AGENT_TOOL, "a result");
    assert_eq!(store.record(&case).unwrap(), Recorded::Saved);
    let path = review(home.path()).join(format!("{}.json", case.id));
    write(&path.with_extension("outcomes"), "[]");

    forget_seat(&settings, AGENT_TOOL.id).expect("the records go");

    assert!(store.snapshot().unwrap().cases.is_empty());
    assert!(!path.with_extension("outcomes").exists());
}

#[test]
fn a_memo_left_with_no_row_leaves_no_file() {
    let home = home();
    let settings = settings_with(home.path(), &json!({}));
    let memo = memo_of(&home.path().join("jev"));
    plant_rows(&memo, b"{\"seat\":\"recall\"}\n");

    forget_seat(&settings, RECALL.id).expect("the rows go");

    assert!(!memo.exists());
}

#[test]
fn a_home_that_never_kept_them_gains_no_folder_and_no_lock() {
    let home = home();
    let settings = settings_with(home.path(), &json!({}));
    fs::create_dir_all(home.path().join("jev")).unwrap();

    forget_seat(&settings, RECALL.id).expect("nothing to do");

    assert!(!review(home.path()).exists(), "the store was created");
    let names: Vec<_> = fs::read_dir(home.path().join("jev")).unwrap().collect();
    assert!(names.is_empty(), "{names:?}");
}

/// The cache's own seat keeps a file of its own, and that goes. The memo does
/// not go with it: the pane guards read it whatever the cache's seat says.
#[test]
fn deleting_the_cache_seat_leaves_the_memo_rows_of_the_other_seats() {
    let home = home();
    let settings = settings_with(home.path(), &json!({}));
    let folder = home.path().join("jev");
    let row = b"{\"seat\":\"command_guard\"}\n";
    plant_rows(&memo_of(&folder), row);
    plant_rows(&cache_of(&folder), row);

    forget_seat(&settings, JUDGMENT_CACHE.id).expect("the records go");

    assert_eq!(fs::read(memo_of(&folder)).unwrap(), row);
    assert!(!cache_of(&folder).exists());
}

/// What `builtin::in_root` reads as deleted so as not to bring a decision back
/// by accident. None of them is a person's delete, so none costs a record.
#[test]
fn a_mark_that_is_not_a_persons_delete_erases_no_leftover() {
    let shapes = [
        json!(null),
        json!([]),
        json!({ RECALL.id: null }),
        json!({ RECALL.id: "deleted" }),
        json!({ RECALL.id: { "deleted": "yes" } }),
        json!({ RECALL.id: { "deleted": true, "questionSchema": 3, "questionDeleted": true } }),
        json!({ RECALL.id: { "deleted": true, "questionSchema": 2 } }),
        json!({ RECALL.id: { "deleted": true, "questionSchema": 2, "questionDeleted": false } }),
    ];
    for decisions in shapes {
        let home = home();
        let settings = settings_with(home.path(), &json!({ builtin::SETTING: decisions }));
        let case = plant_leftovers(home.path(), &RECALL);

        let lift = lift_kept_seats(&settings);
        tidy_at_start(&settings, lift.is_ok());

        assert!(
            leftovers_stand(home.path(), &RECALL, &case),
            "under {decisions}"
        );
        assert_eq!(
            smart_of(&settings)[LEFTOVERS_KEY],
            true,
            "under {decisions}"
        );
    }
}

#[test]
fn a_person_s_delete_of_a_typed_edit_erases_the_leftovers() {
    let home = home();
    let settings = settings_with(
        home.path(),
        &json!({ builtin::SETTING: {
            RECALL.id: { "deleted": true, "questionSchema": 2, "questionDeleted": true },
        } }),
    );
    let case = plant_leftovers(home.path(), &RECALL);

    tidy_at_start(&settings, true);

    assert!(leftovers_gone(home.path(), &RECALL, &case));
}

/// The sweep has its own mark: a machine past the first tidy still has what
/// the seats it deleted left in these three places.
#[test]
fn a_machine_past_the_first_tidy_still_sweeps_once() {
    let home = home();
    let settings = settings_after_the_first_tidy(home.path(), &RECALL);
    let case = plant_leftovers(home.path(), &RECALL);

    tidy_at_start(&settings, true);

    assert!(leftovers_gone(home.path(), &RECALL, &case));
    assert_eq!(smart_of(&settings)[LEFTOVERS_KEY], true);
}

#[test]
fn the_sweep_keeps_the_lifted_seats_and_the_seats_never_deleted() {
    let home = home();
    let settings = settings_marking_four(home.path());
    let gone = plant_case(home.path(), &RECALL, 0);
    let lifted = plant_case(home.path(), &STALL, 1);
    let never = plant_case(home.path(), &COMMAND_GUARD, 2);

    let lift = lift_kept_seats(&settings);
    tidy_at_start(&settings, lift.is_ok());

    assert!(!gone.exists());
    assert!(lifted.exists(), "a lifted seat keeps its cases");
    assert!(never.exists());
}

#[test]
fn the_second_start_reads_no_folder() {
    let home = home();
    let settings = settings_after_the_first_tidy(home.path(), &RECALL);
    tidy_at_start(&settings, true);
    assert_eq!(smart_of(&settings)[LEFTOVERS_KEY], true);

    let case = plant_leftovers(home.path(), &RECALL);
    tidy_at_start(&settings, true);

    assert!(
        leftovers_stand(home.path(), &RECALL, &case),
        "the later start went looking"
    );
}

#[test]
fn a_failed_lift_sweeps_nothing() {
    let home = home();
    let settings = settings_marking_four(home.path());
    let case = plant_leftovers(home.path(), &RECALL);
    let other_writer = crate::settings_file_lock::SettingsFileLock::acquire(&settings).unwrap();

    let lift = lift_kept_seats(&settings);
    assert!(lift.is_err(), "the lock was held");
    tidy_at_start(&settings, lift.is_ok());
    drop(other_writer);

    assert!(leftovers_stand(home.path(), &RECALL, &case));
    assert!(smart_of(&settings).get(LEFTOVERS_KEY).is_none());
}

#[test]
fn a_memo_that_will_not_go_leaves_a_debt_and_no_mark_and_the_next_start_finishes() {
    let home = home();
    let settings = settings_after_the_first_tidy(home.path(), &RECALL);
    let case = plant_leftovers(home.path(), &RECALL);
    let jammed = jam_memo(home.path());

    tidy_at_start(&settings, true);

    assert!(smart_of(&settings).get(LEFTOVERS_KEY).is_none());
    assert!(home.path().join(PENDING_FILE).exists(), "no debt was noted");
    let note: Value =
        serde_json::from_slice(&fs::read(home.path().join(PENDING_FILE)).unwrap()).unwrap();
    assert_eq!(note["seats"], json!([RECALL.id]));
    assert!(!case.exists(), "what could go waits for nothing");

    fs::remove_dir_all(&jammed).unwrap();
    let (rows, kept) = rows_around(&RECALL, &COMMAND_GUARD);
    plant_rows(&jammed, &rows);
    tidy_at_start(&settings, true);

    assert_eq!(fs::read(&jammed).unwrap(), kept);
    assert_eq!(smart_of(&settings)[LEFTOVERS_KEY], true);
    assert!(!home.path().join(PENDING_FILE).exists(), "the debt stayed");
}

#[test]
fn a_store_held_by_another_writer_leaves_a_debt_and_no_mark() {
    let home = home();
    let settings = settings_after_the_first_tidy(home.path(), &RECALL);
    let case = plant_case(home.path(), &RECALL, 0);
    let held = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(review(home.path()).join(".lock"))
        .unwrap();
    held.try_lock().unwrap();

    tidy_at_start(&settings, true);

    assert!(case.exists());
    assert!(smart_of(&settings).get(LEFTOVERS_KEY).is_none());
    assert!(home.path().join(PENDING_FILE).exists());

    held.unlock().unwrap();
    tidy_at_start(&settings, true);

    assert!(!case.exists());
    assert_eq!(smart_of(&settings)[LEFTOVERS_KEY], true);
    assert!(!home.path().join(PENDING_FILE).exists());
}

#[test]
fn a_delete_that_left_a_memo_row_owes_it_to_the_next_start() {
    let home = home();
    let settings = settings_after_the_first_tidy(home.path(), &RECALL);
    tidy_at_start(&settings, true);
    let jammed = jam_memo(home.path());

    seat_deleted(&settings, RECALL.id);
    assert!(home.path().join(PENDING_FILE).exists(), "no debt was noted");

    fs::remove_dir_all(&jammed).unwrap();
    let (rows, kept) = rows_around(&RECALL, &COMMAND_GUARD);
    plant_rows(&jammed, &rows);
    tidy_at_start(&settings, true);

    assert_eq!(fs::read(&jammed).unwrap(), kept);
    assert!(!home.path().join(PENDING_FILE).exists());
}

#[test]
fn a_broken_note_brings_the_sweep_back_once() {
    let home = home();
    let settings = settings_after_the_first_tidy(home.path(), &RECALL);
    tidy_at_start(&settings, true);
    assert_eq!(smart_of(&settings)[LEFTOVERS_KEY], true);
    let case = plant_leftovers(home.path(), &RECALL);
    fs::write(home.path().join(PENDING_FILE), "{ torn").unwrap();

    tidy_at_start(&settings, true);

    assert!(
        leftovers_gone(home.path(), &RECALL, &case),
        "what the note named is unknown"
    );
    assert_eq!(smart_of(&settings)[LEFTOVERS_KEY], true);
}

#[test]
fn the_memo_and_the_cache_are_rewritten_under_a_lock_and_no_other_ledger_is() {
    let home = home();
    let folder = home.path().join("jev");
    fs::create_dir_all(&folder).unwrap();

    assert!(shared_ledger_lock(&memo_of(&folder)).unwrap().is_some());
    assert!(shared_ledger_lock(&cache_of(&folder)).unwrap().is_some());
    assert!(
        shared_ledger_lock(&folder.join(RECALL.ledger))
            .unwrap()
            .is_none()
    );
}

#[test]
fn remember_waits_for_a_rewrite_to_finish() {
    let home = home();
    let memo = memo_of(&home.path().join("jev"));
    fs::create_dir_all(memo.parent().unwrap()).unwrap();
    fs::write(&memo, "").unwrap();
    let held = shared_ledger_lock(&memo).unwrap();
    assert!(held.is_some(), "the memo takes no lock");

    let remembering = {
        let memo = memo.clone();
        thread::spawn(move || memo::remember(&memo, &NOTIFY, "k", ANSWER, 1))
    };
    thread::sleep(Duration::from_millis(100));
    assert_eq!(memo::rows(&memo), 0, "remember went past the lock");
    drop(held);

    remembering.join().unwrap().unwrap();
    assert_eq!(memo::rows(&memo), 1);
}

/// Only the row is lost: the answer was already in hand, and the caller goes on
/// with it. A holder keeps the lock for one append or one rewrite, so this is
/// a stuck writer.
#[test]
fn remember_gives_up_on_a_lock_held_too_long_and_writes_no_row() {
    let home = home();
    let memo = memo_of(&home.path().join("jev"));
    fs::create_dir_all(memo.parent().unwrap()).unwrap();
    fs::write(&memo, "").unwrap();
    let held = shared_ledger_lock(&memo).unwrap();
    assert!(held.is_some(), "the memo takes no lock");

    let error = memo::remember(&memo, &NOTIFY, "k", ANSWER, 1).expect_err("the lock stayed held");

    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    assert_eq!(memo::rows(&memo), 0);
    drop(held);
    memo::remember(&memo, &NOTIFY, "k", ANSWER, 1).expect("the lock is free");
    assert_eq!(memo::rows(&memo), 1);
}

/// A rewrite replaces the memo whole: a row remembered between its read and its
/// rename would go with the old file. Each round gives the eraser a row to take,
/// so it rewrites all through the appends.
#[test]
fn a_row_remembered_while_the_memo_is_rewritten_is_kept() {
    const ROUNDS: usize = 600;
    let home = home();
    let settings = settings_with(home.path(), &json!({}));
    let memo = memo_of(&home.path().join("jev"));
    let remembering = {
        let memo = memo.clone();
        thread::spawn(move || {
            for round in 0..ROUNDS {
                memo::remember(&memo, &NOTIFY, &format!("kept-{round}"), ANSWER, 1).unwrap();
                memo::remember(&memo, &RECALL, "gone", ANSWER, 1).unwrap();
            }
        })
    };
    while !remembering.is_finished() {
        forget_seat(&settings, RECALL.id).unwrap();
        thread::sleep(Duration::from_micros(200));
    }
    remembering.join().unwrap();
    forget_seat(&settings, RECALL.id).unwrap();

    assert_eq!(memo::rows(&memo), ROUNDS, "rows went with a rewrite");
}

/// This machine stood here on 11-01: a full store, 88 of its 512 cases the
/// deleted seats', and every new case dropped as `Full`.
#[test]
fn a_full_store_takes_a_new_case_once_a_deleted_seat_s_cases_go() {
    let home = home();
    let settings = settings_with(home.path(), &json!({}));
    for index in 0..MAX_CASES {
        let seat = if index < 88 { &RECALL } else { &NOTIFY };
        plant_case(home.path(), seat, index);
    }
    let store = Store::at(home.path());
    let next = real_case(&AGENT_TOOL, "a new result");
    assert_eq!(store.record(&next).unwrap(), Recorded::Full);

    forget_seat(&settings, RECALL.id).expect("the cases go");

    assert_eq!(store.record(&next).unwrap(), Recorded::Saved);
    let left = fs::read_dir(review(home.path()))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .count();
    assert_eq!(left, MAX_CASES - 88 + 1);
}

/// The door refuses a deleted seat before it asks, but an answer already on its
/// way when the person deletes arrives after the sweep. It must not bring back
/// what the delete took.
#[test]
fn an_answer_arriving_after_the_delete_writes_nothing() {
    let home = home();
    settings_with(
        home.path(),
        &json!({ builtin::SETTING: { AGENT_TOOL.id: { "deleted": true } } }),
    );
    let store = Store::at(home.path());

    let _ = store.record(&real_case(&AGENT_TOOL, "a late result"));
    assert!(store.snapshot().unwrap().cases.is_empty(), "a late case");

    let memo = memo_of(&home.path().join("jev"));
    memo::remember(&memo, &AGENT_TOOL, "k", ANSWER, 1).expect("nothing to write");
    assert_eq!(memo::rows(&memo), 0, "a late memo row");

    memo::remember(&memo, &NOTIFY, "k", ANSWER, 1).unwrap();
    assert_eq!(memo::rows(&memo), 1, "another seat still remembers");
}
