//! A check kept in one window start is read back in the next, against the
//! clock it is read with; nothing but state words, counts and times reaches
//! the file.

use super::*;

fn live(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

fn check(id: &str, generation: u64, state: Verdict, count: Option<u64>) -> CheckAnswer {
    CheckAnswer {
        id: id.to_string(),
        generation,
        state,
        count,
        forget_judgments: false,
    }
}

fn judgment(id: &str, generation: u64, model: &str, state: Verdict) -> JudgmentAnswer {
    JudgmentAnswer {
        id: id.to_string(),
        generation,
        model: model.to_string(),
        protocol: "openai_chat".to_string(),
        state,
    }
}

#[test]
fn a_kept_check_reads_back_in_the_next_start_with_its_time() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("model-connection-checks.json");
    let ids = live(&["gateway", "bench"]);
    remember_check(
        &path,
        &check("gateway", 2, Verdict::Ok, Some(12)),
        &ids,
        1_000,
    )
    .expect("ok");
    remember_check(
        &path,
        &check("bench", 1, Verdict::Down, Some(3)),
        &ids,
        2_000,
    )
    .expect("down");
    remember_judgment(
        &path,
        &judgment("gateway", 2, "vendor/model", Verdict::Schema),
        &ids,
        3_000,
    )
    .expect("judgment");

    // The next start: a new read of the same file against a later clock.
    let read = read(&path, 10_801_000);
    assert_eq!(read.now_ms, 10_801_000);
    assert_eq!(read.every_ms, QUIET_EVERY_MS);
    let gateway = &read.connections["gateway"];
    assert_eq!(
        (
            gateway.generation,
            gateway.state,
            gateway.count,
            gateway.checked_ms
        ),
        (2, Some(Verdict::Ok), Some(12), Some(1_000))
    );
    assert_eq!(
        gateway.judgments,
        vec![Judgment {
            model: "vendor/model".into(),
            protocol: "openai_chat".into(),
            state: Verdict::Schema,
            checked_ms: 3_000
        }]
    );
    // A failed check keeps no model count: there was no list.
    let bench = &read.connections["bench"];
    assert_eq!(
        (bench.state, bench.count, bench.checked_ms),
        (Some(Verdict::Down), None, Some(2_000))
    );
}

#[test]
fn a_check_of_another_generation_starts_the_record_over() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("checks.json");
    let ids = live(&["gateway"]);
    remember_check(
        &path,
        &check("gateway", 1, Verdict::Ok, Some(4)),
        &ids,
        1_000,
    )
    .expect("ok");
    remember_judgment(
        &path,
        &judgment("gateway", 1, "m", Verdict::Usable),
        &ids,
        1_500,
    )
    .expect("j");
    remember_judgment(
        &path,
        &judgment("gateway", 2, "m", Verdict::Down),
        &ids,
        2_000,
    )
    .expect("j2");
    let record = &read(&path, 3_000).connections["gateway"];
    assert_eq!(record.generation, 2);
    assert_eq!(
        record.state, None,
        "the old generation's list said nothing about this one"
    );
    assert_eq!(record.judgments.len(), 1);
    assert_eq!(record.judgments[0].state, Verdict::Down);
}

#[test]
fn a_press_forgets_the_judgments_and_a_quiet_check_keeps_them() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("checks.json");
    let ids = live(&["gateway"]);
    remember_judgment(
        &path,
        &judgment("gateway", 1, "m", Verdict::Schema),
        &ids,
        1_000,
    )
    .expect("j");
    remember_judgment(
        &path,
        &judgment("gateway", 1, "m", Verdict::Usable),
        &ids,
        1_100,
    )
    .expect("again");
    let kept = read(&path, 1_200).connections["gateway"].judgments.clone();
    assert_eq!(
        kept.len(),
        1,
        "the model's later answer replaces its earlier one"
    );
    assert_eq!(kept[0].state, Verdict::Usable);
    remember_check(
        &path,
        &check("gateway", 1, Verdict::Ok, Some(1)),
        &ids,
        2_000,
    )
    .expect("quiet");
    assert_eq!(read(&path, 2_100).connections["gateway"].judgments.len(), 1);
    let mut pressed = check("gateway", 1, Verdict::Ok, Some(1));
    pressed.forget_judgments = true;
    remember_check(&path, &pressed, &ids, 3_000).expect("pressed");
    assert!(
        read(&path, 3_100).connections["gateway"]
            .judgments
            .is_empty()
    );
}

#[test]
fn a_removed_connection_takes_its_record_with_it() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("checks.json");
    remember_check(
        &path,
        &check("gone", 1, Verdict::Ok, Some(1)),
        &live(&["gone", "kept"]),
        1,
    )
    .expect("a");
    remember_check(
        &path,
        &check("kept", 1, Verdict::Ok, Some(1)),
        &live(&["kept"]),
        2,
    )
    .expect("b");
    assert_eq!(
        read(&path, 3).connections.keys().collect::<Vec<_>>(),
        vec!["kept"]
    );
}

/// The page answers a check with words; a key, a header value or what a
/// server said back has no field to travel in, and a field the record does
/// not know refuses the whole answer.
#[test]
fn nothing_secret_can_reach_the_file() {
    let secret = "sk-fixture-secret-value-0123456789";
    for smuggled in [
        serde_json::json!({"id": "gateway", "generation": 1, "state": "badkey", "raw": format!("서버 오류: HTTP 401 {secret}")}),
        serde_json::json!({"id": "gateway", "generation": 1, "state": "ok", "key": secret}),
        serde_json::json!({"id": "gateway", "generation": 1, "state": secret}),
        serde_json::json!({"id": "gateway", "generation": 1, "state": "checking"}),
    ] {
        assert!(
            serde_json::from_value::<CheckAnswer>(smuggled.clone()).is_err(),
            "{smuggled} was taken"
        );
    }
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("checks.json");
    let ids = live(&["gateway"]);
    let answer: CheckAnswer = serde_json::from_value(
        serde_json::json!({"id": "gateway", "generation": 1, "state": "badkey", "count": 9}),
    )
    .expect("answer");
    remember_check(&path, &answer, &ids, 5).expect("kept");
    remember_judgment(
        &path,
        &judgment("gateway", 1, "vendor/model", Verdict::Badkey),
        &ids,
        6,
    )
    .expect("j");
    let bytes = std::fs::read_to_string(&path).expect("file");
    assert!(!bytes.contains(secret));
    let file: serde_json::Value = serde_json::from_str(&bytes).expect("json");
    let fields = |value: &serde_json::Value| {
        let mut keys: Vec<String> = value.as_object().expect("object").keys().cloned().collect();
        keys.sort();
        keys
    };
    assert_eq!(
        fields(&file["connections"]["gateway"]),
        vec!["checkedMs", "generation", "judgments", "state"],
        "a field beyond the state words reached the file: {bytes}"
    );
    assert_eq!(
        fields(&file["connections"]["gateway"]["judgments"][0]),
        vec!["checkedMs", "model", "protocol", "state"]
    );
}

#[test]
fn a_bad_answer_or_a_foreign_file_keeps_nothing() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("checks.json");
    let ids = live(&["gateway"]);
    assert!(remember_check(&path, &check("../gateway", 1, Verdict::Ok, None), &ids, 1).is_err());
    assert!(remember_check(&path, &check("gateway", 1, Verdict::Usable, None), &ids, 1).is_err());
    assert!(
        remember_judgment(&path, &judgment("gateway", 1, "", Verdict::Usable), &ids, 1).is_err()
    );
    assert!(
        remember_judgment(
            &path,
            &judgment("gateway", 1, "m\n", Verdict::Usable),
            &ids,
            1
        )
        .is_err()
    );
    assert!(!path.exists(), "a refused answer wrote a file");
    std::fs::write(
        &path,
        br#"{"version":99,"connections":{"gateway":{"generation":1,"state":"ok"}}}"#,
    )
    .expect("foreign");
    assert!(read(&path, 1).connections.is_empty());
    std::fs::write(&path, b"not json").expect("torn");
    assert!(read(&path, 1).connections.is_empty());
}

/// The widest record the write limits allow: four connections whose judgment
/// checks are 256, 256, 256 and `last` long, every model name 256 characters (nine at the end is 262,022 bytes).
/// Model `n` of the file was checked at `CLOCK + n` (a real 13-digit clock
/// reading), so a smaller number is older.
const CLOCK: u64 = 1_790_000_000_000;

fn widest_file(path: &Path, last: usize) -> BTreeSet<String> {
    let ids = ["c1", "c2", "c3", "c4"];
    let sizes = [256, 256, 256, last];
    let mut stamp = 0_u64;
    let mut connections = BTreeMap::new();
    for (id, size) in ids.iter().zip(sizes) {
        let judgments = (0..size)
            .map(|_| {
                stamp += 1;
                Judgment {
                    model: format!("{stamp:0>256}"),
                    protocol: "openai_chat".into(),
                    state: Verdict::Schema,
                    checked_ms: CLOCK + stamp,
                }
            })
            .collect();
        connections.insert(
            (*id).to_string(),
            Record {
                generation: 1,
                judgments,
                ..Record::default()
            },
        );
    }
    let bytes = serde_json::to_vec(&File {
        version: VERSION,
        connections,
    })
    .expect("seed");
    std::fs::write(path, bytes).expect("seed file");
    live(&ids)
}

fn kept_judgments(read: &Remembered) -> usize {
    read.connections.values().map(|one| one.judgments.len()).sum()
}

#[test]
fn a_write_just_inside_the_read_limit_keeps_every_check() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("checks.json");
    let ids = widest_file(&path, 9);
    let before = std::fs::metadata(&path).expect("seed").len();
    assert!(before <= MAX_BYTES, "the seed is not inside the limit: {before}");
    // The same model answers again: the file does not grow, nothing is lost.
    let newest = format!("{:0>256}", 777);
    let wrote = remember_judgment(
        &path,
        &judgment("c4", 1, &newest, Verdict::Down),
        &ids,
        CLOCK + 10_000,
    )
    .expect("a write inside the limit");
    let on_disk = std::fs::metadata(&path).expect("file").len();
    assert!(on_disk <= MAX_BYTES, "the file passes the read limit: {on_disk}");
    assert_eq!(kept_judgments(&wrote), 256 * 3 + 9);
    assert_eq!(read(&path, CLOCK + 10_000).connections, wrote.connections);
}

#[test]
fn a_write_past_the_read_limit_drops_the_oldest_checks_not_the_whole_file() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("checks.json");
    let ids = widest_file(&path, 9);
    // The tenth check of the fourth connection: every count and length is
    // inside its own limit, and the file is over the limit by 212 bytes.
    let fresh = format!("{:0>256}", 9_999);
    let before = std::fs::metadata(&path).expect("seed").len();
    let one = serde_json::to_vec(&Judgment {
        model: fresh.clone(),
        protocol: "openai_chat".into(),
        state: Verdict::Down,
        checked_ms: CLOCK + 10_000,
    })
    .expect("one")
    .len() as u64;
    assert!(
        before <= MAX_BYTES && before + one + 1 > MAX_BYTES,
        "the seed is not at the edge of the limit: {before} + {one}"
    );
    let wrote = remember_judgment(
        &path,
        &judgment("c4", 1, &fresh, Verdict::Down),
        &ids,
        CLOCK + 10_000,
    )
    .expect("a write past the limit is trimmed, not kept torn");
    let on_disk = std::fs::metadata(&path).expect("file").len();
    assert!(on_disk <= MAX_BYTES, "the file passes the read limit: {on_disk}");
    // What the next start reads is what the write answered with.
    let next = read(&path, CLOCK + 10_001);
    assert_eq!(next.connections, wrote.connections);
    assert!(kept_judgments(&next) >= 256 * 3, "the next read lost the history");
    // The oldest check went; the newest stayed.
    let c1 = &next.connections["c1"].judgments;
    assert!(c1.iter().all(|one| one.checked_ms > CLOCK + 1), "the oldest check stayed");
    let c4 = &next.connections["c4"].judgments;
    assert_eq!(c4.last().map(|one| one.model.as_str()), Some(fresh.as_str()));
    // And the save after it still starts from that history.
    let again = remember_check(&path, &check("c2", 1, Verdict::Ok, Some(4)), &ids, CLOCK + 10_002)
        .expect("the next save");
    assert!(kept_judgments(&again) >= 256 * 3, "the next save started empty");
}

#[test]
fn a_model_name_of_wide_characters_still_leaves_a_file_the_read_accepts() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("checks.json");
    let ids = live(&["c1"]);
    // 256 four-byte characters: one name is 1 KiB, so 256 of them are a
    // quarter of a megabyte by themselves.
    for n in 0..300_u64 {
        let model: String = std::iter::repeat_n(char::from_u32(0x1F600 + (n % 60) as u32).expect("char"), 255)
            .chain(char::from_u32(0x1F700 + n as u32))
            .collect();
        remember_judgment(&path, &judgment("c1", 1, &model, Verdict::Schema), &ids, n + 1)
            .expect("a write");
        let on_disk = std::fs::metadata(&path).expect("file").len();
        assert!(on_disk <= MAX_BYTES, "write {n} left {on_disk} bytes");
    }
    let next = read(&path, 1_000);
    assert!(!next.connections.is_empty(), "the next read found no record");
    assert_eq!(
        next.connections["c1"].judgments.last().map(|one| one.checked_ms),
        Some(300)
    );
}
