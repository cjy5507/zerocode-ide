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
