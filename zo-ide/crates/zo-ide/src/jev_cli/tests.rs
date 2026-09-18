use super::*;

#[test]
fn the_usage_names_the_one_verb_and_both_flags() {
    for word in ["zo jev summary", "--cwd", "--json"] {
        assert!(USAGE.contains(word), "usage says nothing of {word}");
    }
    assert_eq!(parse(&[]).unwrap_err(), USAGE);
    assert_eq!(parse(&["--help".to_string()]).unwrap_err(), USAGE);
}

#[test]
fn an_unknown_word_is_refused_with_the_usage_rather_than_guessed_at() {
    let refused = parse(&["summarise".to_string()]).unwrap_err();
    assert!(refused.starts_with("unknown verb 'summarise'"));
    assert!(refused.contains("zo jev summary"));
    let flag = parse(&["summary".to_string(), "--week".to_string()]).unwrap_err();
    assert!(flag.starts_with("unknown argument '--week'"));
    assert_eq!(
        parse(&["summary".to_string(), "--cwd".to_string()]).unwrap_err(),
        "--cwd needs a directory"
    );
}

#[test]
fn the_flags_are_read_in_either_order() {
    let want = Request { cwd: Some(std::path::PathBuf::from("/tmp/x")), json: true };
    let one = parse(&["summary".into(), "--json".into(), "--cwd".into(), "/tmp/x".into()]);
    let two = parse(&["summary".into(), "--cwd".into(), "/tmp/x".into(), "--json".into()]);
    assert_eq!(one.as_ref(), Ok(&want));
    assert_eq!(two.as_ref(), Ok(&want));
}

#[test]
fn every_seat_the_table_names_reaches_the_json_with_its_own_numbers() {
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seats = tools::jev_summary::report(&roots, None, 1_000, 0);
    let value: serde_json::Value = serde_json::from_str(&render_json(&seats).to_string()).expect("json");
    let seats = value["seats"].as_array().expect("seats");
    assert_eq!(seats.len(), zerocode_core::jev::JEV_USES.len());
    for (row, seat) in seats.iter().zip(zerocode_core::jev::JEV_USES) {
        assert_eq!(row["id"], seat.id, "the table's order is the answer's order");
        assert_eq!(row["ledger"], seat.ledger);
        assert_eq!(row["found"], serde_json::Value::Null, "nothing was written here");
        assert_eq!(row["week"]["rows"], 0);
        assert_eq!(
            row["week"]["answeredShare"],
            serde_json::Value::Null,
            "never asked is not zero percent"
        );
        assert_eq!(row["riseFloorPermille"].is_null(), !seat.promotes);
    }
    assert_eq!(value["judgedEveryRows"], zerocode_core::jev::summary::JUDGED_EVERY_ROWS);
    assert_eq!(value["windowDays"], tools::jev_summary::WINDOW_DAYS);
}

#[test]
fn the_text_answer_names_every_seat_once() {
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let text = render_text(&tools::jev_summary::report(&roots, None, 1_000, 0));
    let report = Report { text };
    for seat in zerocode_core::jev::JEV_USES {
        assert_eq!(
            report.text.matches(seat.id).count(),
            1,
            "{} named {} times",
            seat.id,
            report.text.matches(seat.id).count()
        );
    }
    assert!(report.text.contains("never asked"));
}
