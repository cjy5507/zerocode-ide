use super::*;

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|word| (*word).to_string()).collect()
}

#[test]
fn the_usage_names_every_verb_and_flag() {
    for word in [
        "zo jev summary",
        "zo jev ask",
        "zo jev choose",
        "zo jev score",
        "--cwd",
        "--json",
        "--recent",
        "--act-lines",
        "--context",
        "--option",
        "--level",
        "--item",
        "--stdin",
        "Exit codes",
    ] {
        assert!(USAGE.contains(word), "usage says nothing of {word}");
    }
    assert_eq!(parse(&[]).unwrap_err(), usage(SUMMARY_REFUSED_EXIT));
    assert_eq!(parse(&["--help".to_string()]).unwrap_err(), usage(SUMMARY_REFUSED_EXIT));
}

#[test]
fn an_unknown_word_is_refused_with_the_usage_rather_than_guessed_at() {
    let refused = parse(&["summarise".to_string()]).unwrap_err();
    assert!(refused.message.starts_with("unknown verb 'summarise'"));
    assert!(refused.message.contains("zo jev summary"));
    assert_eq!(refused.exit, SUMMARY_REFUSED_EXIT);
    let flag = parse(&["summary".to_string(), "--week".to_string()]).unwrap_err();
    assert!(flag.message.starts_with("unknown argument '--week'"));
    assert_eq!(flag.exit, SUMMARY_REFUSED_EXIT, "summary refuses as it always has");
    assert_eq!(
        parse(&["summary".to_string(), "--cwd".to_string()]).unwrap_err(),
        Refused { message: "--cwd needs a directory".to_string(), exit: SUMMARY_REFUSED_EXIT }
    );
}

#[test]
fn the_flags_are_read_in_either_order() {
    let want = Request::Summary(SummaryRequest {
        cwd: Some(std::path::PathBuf::from("/tmp/x")),
        sessions: None,
        recent: 0,
        act_lines: false,
        json: true,
    });
    let one = parse(&words(&["summary", "--json", "--cwd", "/tmp/x"]));
    let two = parse(&words(&["summary", "--cwd", "/tmp/x", "--json"]));
    assert_eq!(one.as_ref(), Ok(&want));
    assert_eq!(two.as_ref(), Ok(&want));
}

/// The replay asks for every line of each seat's grid and the row it keeps
/// (t-9468); nobody else does.
#[test]
fn the_act_lines_are_read_off_their_flag() {
    let Ok(Request::Summary(asked)) = parse(&words(&["summary", "--json", "--act-lines"])) else {
        panic!("a summary");
    };
    assert!(asked.act_lines && asked.json);
    let Ok(Request::Summary(plain)) = parse(&words(&["summary", "--json"])) else {
        panic!("a summary");
    };
    assert!(!plain.act_lines);
}

#[test]
fn the_sessions_folder_is_read_off_its_flag() {
    let want = Request::Summary(SummaryRequest {
        cwd: None,
        sessions: Some(std::path::PathBuf::from("/tmp/cu")),
        recent: 0,
        act_lines: false,
        json: false,
    });
    assert_eq!(parse(&words(&["summary", "--computer-use", "/tmp/cu"])).as_ref(), Ok(&want));
    assert_eq!(
        parse(&words(&["summary", "--computer-use"])).unwrap_err().message,
        "--computer-use needs a directory"
    );
}

#[test]
fn the_recent_count_is_read_off_its_flag_and_refused_when_it_is_not_a_count() {
    let want = Request::Summary(SummaryRequest { cwd: None, sessions: None, recent: 12, act_lines: false, json: true });
    assert_eq!(parse(&words(&["summary", "--recent", "12", "--json"])).as_ref(), Ok(&want));
    assert_eq!(parse(&words(&["summary", "--recent"])).unwrap_err().message, "--recent needs a count");
    assert!(
        parse(&words(&["summary", "--recent", "many"]))
            .unwrap_err()
            .message
            .starts_with("--recent needs a count, not 'many'")
    );
}

/// The three question verbs: the question is the one bare word, every part
/// rides its own flag and may repeat, and a refusal exits with the same 2
/// an unanswered question does.
#[test]
fn a_question_verb_reads_its_parts_off_their_flags() {
    let judged = |args: &[&str]| match parse(&words(args)) {
        Ok(Request::Judge(request)) => request,
        other => panic!("a question: {other:?}"),
    };
    let ask = judged(&["ask", "Does it touch the wire?", "--context", "+ fn send", "--json"]);
    assert_eq!(ask.shape, JevShape::Ask);
    assert_eq!(ask.question, "Does it touch the wire?");
    assert_eq!(ask.context.as_deref(), Some("+ fn send"));
    assert!(ask.json && !ask.stdin);

    let choose = judged(&["choose", "Which?", "--option", "a", "--option", "b", "--cwd", "/work"]);
    assert_eq!(choose.shape, JevShape::Choose);
    assert_eq!(choose.options, words(&["a", "b"]));
    assert_eq!(choose.cwd, Some(std::path::PathBuf::from("/work")));

    let score = judged(&["score", "How risky?", "--level", "low", "--level", "high", "--stdin"]);
    assert_eq!(score.shape, JevShape::Score);
    assert_eq!(score.levels, words(&["low", "high"]));
    assert!(score.stdin && score.items.is_empty());

    let missing = parse(&words(&["ask"])).unwrap_err();
    assert!(missing.message.starts_with("ask needs a question"), "{}", missing.message);
    assert_eq!(missing.exit, NO_ANSWER_EXIT);
    let dangling = parse(&words(&["choose", "Which?", "--option"])).unwrap_err();
    assert_eq!(dangling, Refused { message: "--option needs a value".to_string(), exit: NO_ANSWER_EXIT });
    let twice = parse(&words(&["ask", "one", "two"])).unwrap_err();
    assert!(twice.message.starts_with("one question only; 'two' came after it"), "{}", twice.message);
    let stranger = parse(&words(&["score", "q", "--rubric", "x"])).unwrap_err();
    assert!(stranger.message.starts_with("unknown argument '--rubric'"), "{}", stranger.message);
    assert_eq!(stranger.exit, NO_ANSWER_EXIT);
}

/// Standard input carries the parts either way a script writes them.
#[test]
fn stdin_parts_are_lines_or_one_json_array() {
    assert_eq!(parts_from_text("a\n\n  b  \nc\n"), words(&["a", "b", "c"]));
    assert_eq!(parts_from_text("[\"x\", \"y z\"]\n"), words(&["x", "y z"]));
    assert_eq!(parts_from_text("[not json\nsecond"), words(&["[not json", "second"]), "a bracket alone is a line");
    assert!(parts_from_text("  \n").is_empty());
}

/// The seat's own check refuses a malformed question before anything is
/// asked, in the seat's words, and the shell exits 2 for it.
#[test]
fn a_question_the_seat_refuses_exits_two_before_anything_is_asked() {
    let request = match parse(&words(&["choose", "Which?", "--option", "only"])) {
        Ok(Request::Judge(request)) => request,
        other => panic!("{other:?}"),
    };
    let refused = run_judge(&request, Vec::new(), Path::new("/nowhere")).unwrap_err();
    assert_eq!(refused, Refused { message: "choose needs at least two options".to_string(), exit: NO_ANSWER_EXIT });
}

fn verdict(answer: Option<JevAnswer>) -> JevVerdict {
    JevVerdict {
        shape: JevShape::Ask,
        outcome: if answer.is_some() { "answered".to_string() } else { "off".to_string() },
        route_use: if answer.is_some() { "applied".to_string() } else { "off".to_string() },
        answer,
        elapsed_ms: 312,
        requests: 1,
        input_tokens: Some(321),
        cost_usd: Some(0.000_013),
        model: Some("jev-test".to_string()),
        note: "the note".to_string(),
    }
}

/// The exit code says the answer: yes 0, no 1, nothing 2; a pick and a
/// grading are 0 once there is one.
#[test]
fn the_exit_code_says_the_answer() {
    assert_eq!(exit_of(&verdict(Some(JevAnswer::Ask { yes: true, p_yes: 0.9, confidence: 0.8 }))), 0);
    assert_eq!(exit_of(&verdict(Some(JevAnswer::Ask { yes: false, p_yes: 0.1, confidence: 0.8 }))), ASK_NO_EXIT);
    assert_eq!(exit_of(&verdict(None)), NO_ANSWER_EXIT);
    let chosen = JevAnswer::Choose { chosen: "b".to_string(), index: 1, probabilities: vec![0.2, 0.8], confidence: 0.6 };
    assert_eq!(exit_of(&verdict(Some(chosen))), 0);
    assert_eq!(exit_of(&verdict(Some(JevAnswer::Score { items: Vec::new() }))), 0);
}

/// The text form leads with the answer and puts the numbers under it; the
/// JSON form is the verdict as the tool returns it.
#[test]
fn the_text_leads_with_the_answer_and_the_json_is_the_verdict() {
    let yes = report_of(&verdict(Some(JevAnswer::Ask { yes: true, p_yes: 0.93, confidence: 0.87 })), false);
    assert_eq!(yes.exit, 0);
    assert_eq!(yes.text, "yes\np(yes) 0.93 · confidence 0.87 · 312 ms · 321 tokens · $0.000013\nthe note");

    let chosen = JevAnswer::Choose { chosen: "release".to_string(), index: 2, probabilities: vec![0.1, 0.1, 0.8], confidence: 0.7 };
    let pick = report_of(&verdict(Some(chosen)), false);
    assert_eq!(pick.text, "release\noption 3 of 3 · p 0.80 · confidence 0.70 · 312 ms · 321 tokens · $0.000013\nthe note");

    let scored = JevAnswer::Score {
        items: vec![tools::ScoredItem { index: 0, item: "a change".to_string(), score: 1.5, normalised: 0.75, confidence: 0.4 }],
    };
    let graded = report_of(&verdict(Some(scored)), false);
    assert_eq!(graded.text, "1.50\t0.75\t0.40\ta change\n1 scored · 312 ms · 321 tokens · $0.000013\nthe note");

    let none = report_of(&verdict(None), false);
    assert_eq!(none.exit, NO_ANSWER_EXIT);
    assert_eq!(none.text, "no answer: off\nthe note");

    let json = report_of(&verdict(None), true);
    let value: serde_json::Value = serde_json::from_str(&json.text).expect("the verdict is JSON");
    assert_eq!(value["outcome"], "off");
    assert_eq!(value["routeUse"], "off");
    assert_eq!(value.get("answer"), None);
    assert_eq!(value["inputTokens"], 321);
    assert_eq!(value["shape"], "ask");
}

/// The dashboard's fields ride the JSON: the applied count and the door's
/// refusals in every tally, seven days per seat, and the recent list when
/// asked — read off the same rows as the numbers.
#[test]
fn the_json_carries_the_days_the_refusals_the_applied_count_and_the_recent_list() {
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::PLACEMENT;
    std::fs::write(
        home.path().join(seat.ledger),
        [
            serde_json::json!({"at": 900, "outcome": "answered", "elapsedMs": 40, "requests": 1, "chosen": "split",
                               "applied": true, "confidence": 0.6, "task": "t-1", "worker": "w-1",
                               "rubricVersion": seat.rubric_version}),
            serde_json::json!({"at": 950, "outcome": "not_consented", "requests": 0, "rubricVersion": seat.rubric_version}),
        ]
        .iter()
        .map(|row| row.to_string() + "\n")
        .collect::<String>(),
    )
    .expect("write");
    let seats = tools::jev_summary::report_with_recent(&roots, None, None, 1_000, 0, 3);
    let value: serde_json::Value = serde_json::from_str(&render_json(&seats, false).to_string()).expect("json");
    let placement = value["seats"]
        .as_array()
        .expect("seats")
        .iter()
        .find(|row| row["id"] == seat.id)
        .expect("placement");
    assert_eq!(placement["week"]["applied"], 1);
    assert_eq!(placement["week"]["refusals"], serde_json::json!([{"token": "not_consented", "rows": 1}]));
    assert_eq!(placement["week"]["failures"], serde_json::json!([{"token": "not_consented", "rows": 1}]));
    // A seat whose rows name no control and no guard carries the counts, as
    // zeros (t-6277 D6) — the drawer reads a screen seat off `named`.
    assert_eq!(placement["week"]["guards"], serde_json::json!({"instructed": 0, "walled": 0}));
    assert_eq!(placement["week"]["controls"], serde_json::json!({"named": 0, "destructiveHeld": 0}));
    assert_eq!(placement["days"].as_array().map(Vec::len), Some(7));
    assert_eq!(placement["days"][6]["tally"]["rows"], 2, "today is the last day");
    assert_eq!(placement["days"][6]["agreement"]["compared"], 0);
    let recent = placement["recent"].as_array().expect("recent");
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0]["outcome"], "not_consented");
    assert_eq!(recent[1]["answered"], "split");
    assert_eq!(recent[1]["applied"], true);
    assert_eq!(recent[1]["asked"], serde_json::json!({"task": "t-1", "worker": "w-1"}));
    assert_eq!(recent[1]["confidence"], 0.6);
    let unasked = tools::jev_summary::report(&roots, None, None, 1_000, 0);
    let without: serde_json::Value = serde_json::from_str(&render_json(&unasked, false).to_string()).expect("json");
    let placement = without["seats"].as_array().expect("seats").iter().find(|row| row["id"] == seat.id).expect("placement");
    assert_eq!(placement["recent"], serde_json::json!([]), "nothing listed unless asked");
    assert_eq!(placement["days"].as_array().map(Vec::len), Some(7), "the days always ride");
}

/// Every agreement the JSON carries says why its rows compare nothing, word
/// by word (t-9556): the words the label rows wrote, each with its count, so
/// a screen says "not carried 2 · unseen 1" and not only "3". A row that
/// carries a mark is a mark, whatever else it spells.
#[test]
fn the_json_says_why_the_rows_compare_nothing_word_by_word() {
    use zerocode_core::worker_placement::{NOT_CARRIED, UNSEEN};
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::PLACEMENT;
    let asked = |at: i64, worker: &str| {
        serde_json::json!({"at": at, "placement": worker, "outcome": "answered", "elapsedMs": 40, "requests": 1,
                           "chosen": "split", "confidence": 0.7, "rubricVersion": seat.rubric_version})
    };
    let rows = [
        asked(100, "w-1"),
        asked(110, "w-2"),
        asked(120, "w-3"),
        asked(130, "w-4"),
        serde_json::json!({"at": 200, "label": "w-1", "notCompared": UNSEEN}),
        serde_json::json!({"at": 210, "label": "w-2", "notCompared": NOT_CARRIED}),
        serde_json::json!({"at": 220, "label": "w-3", "notCompared": NOT_CARRIED}),
        serde_json::json!({"at": 230, "label": "w-4", "agreed": true, "notCompared": UNSEEN}),
    ];
    std::fs::write(
        home.path().join(seat.ledger),
        rows.iter().map(|row| row.to_string() + "\n").collect::<String>(),
    )
    .expect("write");
    let seats = tools::jev_summary::report(&roots, None, None, 1_000, 0);
    let value: serde_json::Value = serde_json::from_str(&render_json(&seats, false).to_string()).expect("json");
    let placement = value["seats"]
        .as_array()
        .expect("seats")
        .iter()
        .find(|row| row["id"] == seat.id)
        .expect("placement");
    let words = serde_json::json!({ NOT_CARRIED: 2, UNSEEN: 1 });
    assert_eq!(placement["agreementWeek"]["notCompared"], 3);
    assert_eq!(placement["agreementWeek"]["notComparedBy"], words, "the week");
    assert_eq!(placement["judged"]["agreement"]["notComparedBy"], words, "the judged window");
    assert_eq!(placement["days"][6]["agreement"]["notComparedBy"], words, "today");
    assert_eq!(
        placement["days"][0]["agreement"]["notComparedBy"],
        serde_json::json!({}),
        "a day nothing was withheld on says so"
    );
}

/// A seat's act line, end to end (t-9468): the summary says the line the
/// seat's graded answers draw and the row the replay would keep for it; the
/// row, kept beside the ledger, is the line the product then reads — the
/// seat is judged on the answers that line lets act, rises where its whole
/// record holds, and the JSON says what it acts on and how often that is
/// wrong beside the baseline.
#[test]
fn the_act_line_the_labels_draw_is_the_line_the_seat_reads_once_the_table_keeps_it() {
    use zerocode_core::jev::threshold::THRESHOLDS_FILE;
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::NOTIFY;
    // Fifty confident rings, three of them wrong; fifty timid ones, half
    // wrong; the baseline wrong on every one.
    let mut rows: Vec<serde_json::Value> = (0..100)
        .map(|n| {
            serde_json::json!({"at": 1_000 + n, "notify": format!("r-{n}"), "outcome": "answered", "elapsedMs": 40,
                               "requests": 1, "confidence": if n % 2 == 0 { 0.9 } else { 0.2 },
                               "rubricVersion": seat.rubric_version})
        })
        .collect();
    rows.extend((0..100).map(|n| {
        let agreed = if n % 2 == 0 { n % 34 != 0 } else { n % 4 == 1 };
        serde_json::json!({"at": 2_000 + n, "label": format!("r-{n}"), "agreed": agreed, "baselineAgreed": false})
    }));
    let ledger = home.path().join(seat.ledger);
    std::fs::write(&ledger, rows.iter().map(|row| row.to_string() + "\n").collect::<String>()).expect("write");
    let notify = |value: &serde_json::Value| -> serde_json::Value {
        value["seats"].as_array().expect("seats").iter().find(|row| row["id"] == seat.id).expect("notify").clone()
    };
    let read = || {
        let seats = tools::jev_summary::report(&roots, None, None, 10_000, 0);
        let value: serde_json::Value = serde_json::from_str(&render_json(&seats, true).to_string()).expect("json");
        let plain: serde_json::Value = serde_json::from_str(&render_json(&seats, false).to_string()).expect("json");
        assert!(
            notify(&plain)["calibration"].get("grid").is_none() && notify(&plain)["calibration"].get("row").is_none(),
            "the grid and the row ride only when --act-lines asks"
        );
        (value.clone(), notify(&value))
    };

    let (value, before) = read();
    assert_eq!(value["thresholdsFile"], THRESHOLDS_FILE);
    let calibration = &before["calibration"];
    assert_eq!(calibration["readsActLine"], true);
    assert_eq!(calibration["actFromPermille"], 300, "the lowest line the confident rings pass on");
    assert_eq!(calibration["reason"], serde_json::Value::Null);
    assert_eq!(calibration["grid"].as_array().map(Vec::len), Some(9));
    assert_eq!(calibration["fixed"]["fromPermille"], 850, "the bands' own act line");
    assert_eq!(calibration["drawn"]["marks"], 50);
    assert_eq!(calibration["tableLine"], serde_json::Value::Null);
    assert_eq!(before["applyShare"], serde_json::Value::Null, "no line is read yet");
    assert_eq!(before["verdict"]["verdict"], "hold");
    assert_eq!(before["verdict"]["line"], "agreement", "the whole record, timid rings and all");
    assert_eq!(before["verdict"]["actLine"], serde_json::Value::Null);

    // The replay keeps the row beside the ledger; the product reads it.
    let row = calibration["row"].clone();
    assert_eq!(row["actFromPermille"], 300);
    std::fs::write(home.path().join(THRESHOLDS_FILE), serde_json::json!([row]).to_string()).expect("the table");
    let (_, after) = read();
    assert_eq!(after["calibration"]["tableLine"], 300);
    assert_eq!(after["applyShare"], 0.5);
    assert_eq!(after["appliedErrorPermille"], 60, "three of the fifty it acts on");
    assert_eq!(after["baselineErrorPermille"], 1_000);
    assert_eq!(after["verdict"]["verdict"], "rise", "{}", after["verdict"]);
    assert_eq!(after["verdict"]["actLine"], 300);
    assert_eq!(after["judged"]["agreement"]["compared"], 50);
}

/// Every seat's line says which id it asks with and which version answered
/// (t-6187): the person's pin, or the alias, beside the `model` its newest
/// request named — and a verdict read on rows a change of version cut
/// names the version it cut away. A pinned version is still priced: it
/// bills at its family's row.
#[test]
fn the_json_names_the_asked_model_the_answering_version_and_the_cut() {
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::PLACEMENT;
    std::fs::write(
        home.path().join(seat.ledger),
        [
            serde_json::json!({"at": 900, "outcome": "answered", "elapsedMs": 40, "requests": 1,
                               "inputTokens": 1000, "model": "jev-1.12.0", "rubricVersion": seat.rubric_version}),
            serde_json::json!({"at": 950, "outcome": "answered", "elapsedMs": 40, "requests": 1,
                               "inputTokens": 1000, "model": "jev-1.13.0", "rubricVersion": seat.rubric_version}),
            serde_json::json!({"at": 960, "outcome": "timeout", "requests": 1, "rubricVersion": seat.rubric_version}),
        ]
        .iter()
        .map(|row| row.to_string() + "\n")
        .collect::<String>(),
    )
    .expect("write");
    let placement_of = |settings: Option<&serde_json::Value>| {
        let seats = tools::jev_summary::report(&roots, None, settings, 1_000, 0);
        let value: serde_json::Value = serde_json::from_str(&render_json(&seats, false).to_string()).expect("json");
        let placement = value["seats"]
            .as_array()
            .expect("seats")
            .iter()
            .find(|row| row["id"] == seat.id)
            .expect("placement")
            .clone();
        (placement, render_text(&seats))
    };

    let pinned = serde_json::json!({"smart": {"jevModel": "jev-1.13.0"}});
    let (placement, text) = placement_of(Some(&pinned));
    assert_eq!(placement["askedModel"], "jev-1.13.0", "the id the seat asks with is the pin");
    assert_eq!(placement["model"], "jev-1.13.0", "the version the newest answer named");
    assert_eq!(placement["verdict"]["cutModel"], "jev-1.12.0", "the thin window says why it is thin");
    assert!(placement["costUsd"].as_f64().is_some_and(|usd| usd > 0.0), "a pinned version is priced");
    let line = text.lines().find(|line| line.starts_with(seat.id)).expect("the placement line");
    assert!(line.contains("asks jev-1.13.0 · answered by jev-1.13.0"), "{line}");
    assert!(line.contains("cut at jev-1.12.0"), "{line}");

    let (placement, _) = placement_of(None);
    assert_eq!(placement["askedModel"], zerocode_core::jev::DEFAULT_MODEL, "unpinned, the alias");
    let summon = {
        let seats = tools::jev_summary::report(&roots, None, None, 1_000, 0);
        render_json(&seats, false)["seats"]
            .as_array()
            .expect("seats")
            .iter()
            .find(|row| row["id"] == zerocode_core::jev::SUMMON.id)
            .expect("summon")
            .clone()
    };
    assert_eq!(summon["model"], serde_json::Value::Null, "a seat nothing answered names no version");
}

#[test]
fn every_seat_the_table_names_reaches_the_json_with_its_own_numbers() {
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seats = tools::jev_summary::report(&roots, None, None, 1_000, 0);
    let value: serde_json::Value = serde_json::from_str(&render_json(&seats, false).to_string()).expect("json");
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
    let text = render_text(&tools::jev_summary::report(&roots, None, None, 1_000, 0));
    let report = Report { text, exit: 0 };
    for seat in zerocode_core::jev::JEV_USES {
        assert_eq!(
            named(&report.text, seat.id),
            1,
            "{} named {} times",
            seat.id,
            named(&report.text, seat.id)
        );
    }
    assert!(report.text.contains("never asked"));
}

/// The routing seat with one active row and its control row: the judgment
/// named what the probe named on every axis. Both rows are the rubric that
/// asks now — the judge windows the seat by it (t-6346).
fn routing_ledger_with_a_control_row(home: &std::path::Path) {
    use zerocode_core::jev::questions::ROUTING_RUBRIC_VERSION;
    use zerocode_core::jev::summary::CONTROL;
    let axis = |choice: &str| serde_json::json!({"choice": choice, "probabilities": {}, "confidence": 0.9});
    let jev = serde_json::json!({"complexity": axis("large"), "risk": axis("low"), "intent": axis("analysis")});
    let active = serde_json::json!({
        "at": 1, "task": "0000000000000005", "rubricVersion": ROUTING_RUBRIC_VERSION, "outcome": "answered",
        "elapsedMs": 400, "retries": 0, "cached": false, "requests": 1, "routeUse": "applied", "probe": "not_run",
        "jev": jev,
    });
    let control = serde_json::json!({
        "at": 2, "task": "0000000000000005", "rubricVersion": ROUTING_RUBRIC_VERSION, "outcome": CONTROL, "elapsedMs": 0,
        "retries": 0, "cached": false, "requests": 0, "routeUse": CONTROL,
        "probe": {"complexity": "large", "risk": "low", "intent": "analysis", "confidence": "high"}, "jev": jev,
    });
    std::fs::write(
        home.join(zerocode_core::jev::ROUTING.ledger),
        format!("{active}\n{control}\n"),
    )
    .expect("write");
}

#[test]
fn the_control_rows_the_agreement_borrowed_are_named_in_both_answers() {
    let home = tempfile::tempdir().expect("tmp");
    routing_ledger_with_a_control_row(home.path());
    let roots = [home.path().to_path_buf()];
    // A clock just past the rows, so the day holds them.
    let seats = tools::jev_summary::report(&roots, None, None, 1_000, 0);

    let value: serde_json::Value = serde_json::from_str(&render_json(&seats, false).to_string()).expect("json");
    let routing = value["seats"]
        .as_array()
        .expect("seats")
        .iter()
        .find(|seat| seat["id"] == "routing")
        .expect("routing");
    assert_eq!(routing["judged"]["window"]["rows"], 1, "the control row was counted in the window");
    assert_eq!(routing["today"]["rows"], 1, "the control row was counted in the day");
    // The countdown is the judge's own: the first judgment waits for the
    // window routing's floor can be cleared on, and this row took one off it.
    let owed = zerocode_core::jev::promote::rows_to_next_judgment(&zerocode_core::jev::ROUTING, 1)
        .expect("routing rises");
    assert_eq!(
        routing["rowsToNextJudgment"],
        serde_json::json!(owed),
        "the control row moved the cadence"
    );
    assert_eq!(
        routing["judged"]["agreement"],
        serde_json::json!({
            "compared": 3,
            "agreed": 3,
            "lowerBound": zerocode_core::jev::summary::wilson_lower(3, 3, zerocode_core::jev::summary::WILSON_Z_95),
            "controlRows": 1,
            // The routing writer stamps no baseline mark yet (t-6342).
            "baselineCompared": 0,
            "baselineAgreed": 0,
            "baselineShare": null,
            "notCompared": 0,
            "notComparedBy": {},
        })
    );

    let text = render_text(&seats);
    let line = text.lines().find(|line| line.starts_with("routing")).expect("the routing line");
    assert!(line.contains("agrees 3 of 3 (1 control row)"), "{line}");
    assert!(line.contains(&format!("{owed} rows to judgment")), "{line}");
    // A seat that borrowed nothing says nothing of it.
    let summon = text.lines().find(|line| line.starts_with("summon")).expect("the summon line");
    assert!(!summon.contains("control row"), "{summon}");
}

/// How many times `id` stands in `text` as a whole word — `effort` inside
/// `step_effort` is that seat's name, not this one's.
fn named(text: &str, id: &str) -> usize {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(id)
        .filter(|(at, _)| {
            let before = text[..*at].chars().next_back();
            let after = text[at + id.len()..].chars().next();
            !before.is_some_and(is_word) && !after.is_some_and(is_word)
        })
        .count()
}
