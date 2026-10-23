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

/// A seat whose labels marked rows through a proxy says how many in every
/// agreement the JSON carries (t-60961): `proxied` counts the marks the judge
/// withholds from `compared` (t-26584, ①), so a seat that compared nothing is
/// told apart from one whose labels marked nothing. It counts marks and not
/// label rows: a proxy row that compared nothing counts in `notCompared` alone.
#[test]
fn a_summary_counts_the_marks_a_proxy_label_made() {
    use zerocode_core::jev::provenance::{stamp_label, LabelSource};
    use zerocode_core::worker_placement::UNSEEN;
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::PLACEMENT;
    let began_ms = core_types::date::unix_secs_from_rfc3339("2026-10-16T04:17:22Z").expect("a time") * 1000;
    let proxy_marks = [("w-1", true), ("w-2", false)];
    let executed_marks = [("w-3", true)];
    let unmarked = ["w-4"];
    let asked = |at: i64, worker: &str| {
        serde_json::json!({"at": at, "placement": worker, "outcome": "answered", "elapsedMs": 40, "requests": 1,
                           "chosen": "split", "confidence": 0.7, "rubricVersion": seat.rubric_version})
    };
    let stamped = |mut row: serde_json::Value, source: LabelSource| {
        stamp_label(&mut row, source, seat.id);
        row
    };
    let workers = proxy_marks.iter().chain(&executed_marks).map(|(worker, _)| *worker).chain(unmarked);
    let mut rows: Vec<serde_json::Value> =
        workers.zip(0..).map(|(worker, n)| asked(began_ms + n * 1_000, worker)).collect();
    let labelled_at = began_ms + 10_000;
    rows.extend(proxy_marks.iter().map(|(worker, agreed)| {
        stamped(serde_json::json!({"at": labelled_at, "label": worker, "agreed": agreed}), LabelSource::Proxy)
    }));
    rows.extend(executed_marks.iter().map(|(worker, agreed)| {
        stamped(serde_json::json!({"at": labelled_at, "label": worker, "agreed": agreed}), LabelSource::Executed)
    }));
    rows.extend(unmarked.iter().map(|worker| {
        stamped(serde_json::json!({"at": labelled_at, "label": worker, "notCompared": UNSEEN}), LabelSource::Proxy)
    }));
    std::fs::write(
        home.path().join(seat.ledger),
        rows.iter().map(|row| row.to_string() + "\n").collect::<String>(),
    )
    .expect("write");
    let seats = tools::jev_summary::report(&roots, None, None, began_ms + 39_000, 0);
    let value: serde_json::Value = serde_json::from_str(&render_json(&seats, false).to_string()).expect("json");
    let placement = value["seats"]
        .as_array()
        .expect("seats")
        .iter()
        .find(|row| row["id"] == seat.id)
        .expect("placement");
    println!("{placement}");
    assert_eq!(placement["agreementWeek"]["proxied"], proxy_marks.len(), "the week counts the marks a proxy label made");
    assert_eq!(placement["agreementWeek"]["compared"], executed_marks.len(), "and compares only the executed ones");
    assert_eq!(placement["agreementWeek"]["notCompared"], unmarked.len(), "a row that compared nothing is no mark");
    assert_eq!(placement["judged"]["agreement"]["proxied"], proxy_marks.len(), "the judged window");
    assert_eq!(placement["days"][6]["agreement"]["proxied"], proxy_marks.len(), "today");
}

/// A seat's numbers say when they begin (t-60961): `countedSince` is the time
/// of the earliest row the rolling week counted, so a ledger begun 39 s ago —
/// as a judgment server's new port begins one — reads from that row and not
/// from seven days back. A first row of the file older than the week is not
/// counted. A week that counted nothing names no time, and nor does one that
/// counted only a row with no time or one before the epoch: null, never 1970.
#[test]
fn a_summary_names_when_the_rows_it_counted_begin() {
    use zerocode_core::jev::{JevUse, COMMAND_GUARD, PLACEMENT};
    let began = "2026-10-16T04:17:22Z";
    let began_ms = core_types::date::unix_secs_from_rfc3339(began).expect("a time") * 1000;
    let now_ms = began_ms + 39_000;
    let day_ms = 24 * 60 * 60 * 1000;
    let before_the_week = now_ms - (tools::jev_summary::WINDOW_DAYS + 1) * day_ms;
    let asked = |seat: &JevUse, at: Option<i64>| {
        let mut row = serde_json::json!({"outcome": "answered", "elapsedMs": 40, "requests": 1,
                                         "rubricVersion": seat.rubric_version});
        if let Some(at) = at {
            row["at"] = serde_json::json!(at);
        }
        row
    };
    let write = |root: &Path, seat: &JevUse, rows: &[serde_json::Value]| {
        std::fs::write(root.join(seat.ledger), rows.iter().map(|row| row.to_string() + "\n").collect::<String>())
            .expect("write");
    };
    let seat_in = |root: &Path, now_ms: i64, id: &str| {
        let seats = tools::jev_summary::report(&[root.to_path_buf()], None, None, now_ms, 0);
        let value: serde_json::Value = serde_json::from_str(&render_json(&seats, false).to_string()).expect("json");
        value["seats"].as_array().expect("seats").iter().find(|row| row["id"] == id).expect("the seat").clone()
    };
    let home = tempfile::tempdir().expect("tmp");
    let in_the_week = [began_ms, began_ms + 5_000, began_ms + 30_000];
    let guard_rows: Vec<serde_json::Value> =
        std::iter::once(before_the_week).chain(in_the_week).map(|at| asked(&COMMAND_GUARD, Some(at))).collect();
    write(home.path(), &COMMAND_GUARD, &guard_rows);
    write(home.path(), &PLACEMENT, &[asked(&PLACEMENT, Some(before_the_week))]);
    let guard = seat_in(home.path(), now_ms, COMMAND_GUARD.id);
    println!("{guard}");
    assert_eq!(guard["countedSince"], began, "the week of the guard begins at the first row it counted");
    assert_eq!(guard["week"]["rows"], in_the_week.len(), "and the row before the week is not one");
    let unasked = zerocode_core::jev::JEV_USES
        .iter()
        .find(|seat| !home.path().join(seat.ledger).exists())
        .expect("a seat never asked");
    for (id, what) in [(PLACEMENT.id, "a week that counted nothing"), (unasked.id, "a seat never asked")] {
        let said = seat_in(home.path(), now_ms, id);
        assert_eq!(said.get("countedSince"), Some(&serde_json::Value::Null), "{what} names no time");
    }
    // Every counter reads a row with no time at the epoch: a week read one day after it reaches back past it,
    // counts such a row, and still names no time.
    let epoch = tempfile::tempdir().expect("tmp");
    let timeless = [asked(&PLACEMENT, None)];
    let early = [asked(&COMMAND_GUARD, Some(-1_000))];
    for (seat, rows, what) in
        [(&PLACEMENT, &timeless, "a row that names no time"), (&COMMAND_GUARD, &early, "a row before the epoch")]
    {
        write(epoch.path(), seat, rows);
        let read = seat_in(epoch.path(), day_ms, seat.id);
        assert_eq!(read["week"]["rows"], rows.len(), "{what} is counted");
        assert_eq!(read.get("countedSince"), Some(&serde_json::Value::Null), "{what} names no time, never 1970");
    }
}

/// A seat several programs ask says in the JSON who asked it this week
/// (t-10916) — the name the window's dashboard reads it by.
#[test]
fn the_json_says_who_asked_a_seat_this_week() {
    let home = tempfile::tempdir().expect("tmp");
    let roots = [home.path().to_path_buf()];
    let seat = &zerocode_core::jev::COMMAND_GUARD;
    let asked = |at: i64, from: &str| {
        serde_json::json!({"at": at, "outcome": "answered", "elapsedMs": 40, "requests": 1, "from": from,
                           "rubricVersion": seat.rubric_version})
    };
    let rows = [asked(100, "zo"), asked(110, "claude"), asked(120, "claude")];
    std::fs::write(
        home.path().join(seat.ledger),
        rows.iter().map(|row| row.to_string() + "\n").collect::<String>(),
    )
    .expect("write");
    let seats = tools::jev_summary::report(&roots, None, None, 1_000, 0);
    let value: serde_json::Value = serde_json::from_str(&render_json(&seats, false).to_string()).expect("json");
    let guard = value["seats"]
        .as_array()
        .expect("seats")
        .iter()
        .find(|row| row["id"] == seat.id)
        .expect("the command guard");
    assert_eq!(guard["askersWeek"], serde_json::json!({ "claude": 2, "zo": 1 }));
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
        // The labels name their executed source: this test reads the act line
        // the labels draw, and the source rule is tested in provenance (ADR-171).
        serde_json::json!({"at": 2_000 + n, "label": format!("r-{n}"), "agreed": agreed, "baselineAgreed": false, "labelSource": "executed"})
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
    // The line acts on half the answers, above the abstain ceiling of a third: the
    // calibration holds it, however the pinned table reads (t-26584, ③).
    assert_eq!(after["verdict"]["verdict"], "hold", "{}", after["verdict"]);
    assert_eq!(after["verdict"]["line"], "calibration", "{}", after["verdict"]);
    assert_eq!(after["verdict"]["actLine"], 300);
    assert_eq!(after["judged"]["agreement"]["compared"], 50);
}

/// Every seat's line says which id it asks with and which version answered
/// (t-6187): the person's pin, or the alias, beside the `model` its newest
/// request named — and a verdict read on rows a change of version cut
/// names the version it cut away.
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
    assert_eq!(placement["costUsd"], serde_json::Value::Null, "a timeout with unknown usage is not free");
    assert_eq!(placement["week"]["unmeteredRequests"], 1);
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
            // The control row's own axes made the three marks, and no label did, so none is withheld (t-60961).
            "proxied": 0,
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

/// The value `name` holds in zo's door, read from its source (t-58193): a
/// rule's number is written once, there, and a test that names it moves with
/// it. A `Duration` is read in its seconds.
fn door_number(name: &str) -> i64 {
    let gate = include_str!("../../../tools/src/misc_tools/smart_router/jev_gate.rs");
    let line = gate
        .lines()
        .find(|line| line.starts_with(&format!("pub const {name}:")))
        .unwrap_or_else(|| panic!("the door names no `{name}`"));
    let value = line.split_once('=').expect("a value").1.trim().trim_end_matches(';');
    let value = value.trim_start_matches("Duration::from_secs(").trim_end_matches(')');
    value.split('*').map(|factor| factor.trim().parse::<i64>().expect("a number")).product()
}

/// `body` with zo's config home and its projects' state under `home`, under
/// the lock every test that moves them takes, and the process's own put back
/// after: after a failing `body` too, or the next test would read a folder
/// that is gone.
fn under_home<T>(home: &Path, body: impl FnOnce() -> T) -> T {
    under_home_with(home, &[], body)
}

/// `body` as [`under_home`] runs it, with each of `also` set beside the home
/// and put back the same way.
fn under_home_with<T>(home: &Path, also: &[(&str, &std::ffi::OsStr)], body: impl FnOnce() -> T) -> T {
    let _env = crate::test_env_lock();
    let mut names = vec![("ZO_CONFIG_HOME", home.as_os_str()), (core_types::paths::ZO_STATE_DIR_ENV, home.as_os_str())];
    names.extend_from_slice(also);
    let before: Vec<_> = names.iter().map(|(name, _)| std::env::var_os(name)).collect();
    for (name, value) in &names {
        std::env::set_var(name, value);
    }
    let said = std::panic::catch_unwind(std::panic::AssertUnwindSafe(body));
    for ((name, _), value) in names.iter().zip(before) {
        match value {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
    }
    match said {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// A seat the door rests says until when, why, and after how many
/// unanswered requests (t-58193), in one field that the panel's chip and
/// sentence both read: the door's own reading of the project's ledger, its
/// count the rule's own number. A seat whose rest has passed says nothing of
/// one, and nor does any seat that never rested.
#[test]
fn a_resting_seat_says_until_when_why_and_after_how_many() {
    use std::fmt::Write as _;
    use zerocode_core::jev::RECALL;
    let home = tempfile::tempdir().expect("a config home");
    let project = tempfile::tempdir().expect("a project");
    let unanswered = door_number("UNANSWERED_BEFORE_REST");
    let rest_ms = door_number("REST") * 1_000;
    under_home(home.path(), || {
        let settings = json!({"smart": {RECALL.setting: "shadow"}});
        std::fs::write(home.path().join("settings.json"), settings.to_string()).expect("settings");
        let ledger = jev_summary::ledger_roots(project.path())[1].join(RECALL.ledger);
        std::fs::create_dir_all(ledger.parent().expect("a folder")).expect("the folder");
        let timed_out = |newest: i64| {
            let mut rows = String::new();
            for before in (0..unanswered).rev() {
                let row = json!({"at": newest - before * 1_000, "outcome": "timeout",
                    "requests": 1, "rubricVersion": RECALL.rubric_version});
                let _ = writeln!(rows, "{row}");
            }
            std::fs::write(&ledger, rows).expect("the ledger");
        };
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        let now = i64::try_from(since.expect("after 1970").as_millis()).expect("a time");
        let request =
            SummaryRequest { cwd: None, sessions: None, recent: 0, act_lines: false, json: true };
        let said_rests = || -> Vec<(String, Value)> {
            let text = run_summary(&request, project.path(), now, 0).text;
            let said: Value = serde_json::from_str(&text).expect("json");
            let seats = said["seats"].as_array().expect("seats").iter();
            seats
                .map(|seat| {
                    let id = seat["id"].as_str().unwrap_or_default().to_string();
                    (id, seat["resting"].clone())
                })
                .collect()
        };
        let recall_of = |rests: &[(String, Value)]| {
            rests.iter().find(|(id, _)| id == RECALL.id).map(|(_, rest)| rest.clone())
        };

        timed_out(now - 60_000);
        let resting = said_rests();
        let until = now - 60_000 + rest_ms;
        let expected = json!({"untilMs": until, "cause": "timeout", "unanswered": unanswered});
        assert_eq!(recall_of(&resting), Some(expected), "the recall rests");
        let others: Vec<&String> = resting
            .iter()
            .filter(|(id, rest)| id != RECALL.id && !rest.is_null())
            .map(|(id, _)| id)
            .collect();
        assert!(others.is_empty(), "these seats never rested: {others:?}");

        timed_out(now - rest_ms - 60_000);
        assert_eq!(recall_of(&said_rests()), Some(Value::Null), "a rest that has passed");
    });
}

/// A stand-in for the judgment provider on this machine's loopback that
/// answers every question a request asks with a yes as likely as `yes`: its
/// origin, and how many requests it has answered.
fn provider_answering(yes: f64) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::io::{Read as _, Write as _};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a socket for the stand-in provider");
    let origin = format!("http://{}", listener.local_addr().expect("its address"));
    let answered = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&answered);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut read = Vec::new();
            let mut chunk = [0u8; 4096];
            let asked = loop {
                let count = stream.read(&mut chunk).unwrap_or(0);
                if count == 0 {
                    break None;
                }
                read.extend_from_slice(&chunk[..count]);
                let text = String::from_utf8_lossy(&read).into_owned();
                let Some((head, body)) = text.split_once("\r\n\r\n") else { continue };
                let length = head
                    .to_ascii_lowercase()
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:").and_then(|value| value.trim().parse::<usize>().ok()))
                    .unwrap_or(0);
                if body.len() >= length {
                    break serde_json::from_str::<Value>(body).ok();
                }
            };
            let Some(asked) = asked else { continue };
            let answers: serde_json::Map<String, Value> = asked["questions"]
                .as_object()
                .into_iter()
                .flat_map(serde_json::Map::keys)
                .map(|id| (id.clone(), json!({"type": "noul", "noul": yes})))
                .collect();
            let body = json!({"model": "jev-test", "answers": answers, "usage": {"input_tokens": 20, "output_tokens": 0}})
                .to_string();
            counted.fetch_add(1, Ordering::SeqCst);
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (origin, answered)
}

/// `zo jev decide <name>` asks the person's own decision of that name
/// (t-62170): one that is on prints its line and exits 0; one that records is
/// asked all the same, prints no line — `null` in its JSON — and exits 2; and
/// a name no decision has is refused with exit 2 before anything is asked.
#[test]
fn decide_prints_an_on_decisions_line_and_nothing_of_one_that_records() {
    use std::ffi::OsStr;
    use std::sync::atomic::Ordering;
    use zerocode_core::jev::custom::{self, store, Answer, Definition, Point};
    use zerocode_core::jev::{door, JevMode, AGENT_TOOL_ASK_OPTIONS, CUSTOM_DECISIONS, SMART_SETTINGS_KEY};
    let home = tempfile::tempdir().expect("a config home");
    let work = tempfile::tempdir().expect("a project");
    let project = std::fs::canonicalize(work.path()).expect("the project resolved");
    let (origin, answered) = provider_answering(0.9);
    let also = [
        ("HOME", home.path().as_os_str()),
        ("ZO_HOME", home.path().as_os_str()),
        (api::SYSTEMONE_API_KEY_ENV, OsStr::new("test-key")),
        (api::SYSTEMONE_BASE_URL_ENV, OsStr::new(&origin)),
    ];
    under_home_with(home.path(), &also, || {
        let consented = json!({door::ENABLED_SETTING: true, door::WORKSPACES_SETTING: [project.to_string_lossy()]});
        let settings =
            json!({SMART_SETTINGS_KEY: {CUSTOM_DECISIONS.setting: JevMode::On.key(), door::JEV_SETTINGS_KEY: consented}});
        std::fs::write(home.path().join("settings.json"), settings.to_string()).expect("settings");
        for (name, mode) in [("Ship", JevMode::On), ("Wait", JevMode::Shadow)] {
            let answer = Answer::YesNo { yes: "It is ready.".to_string(), no: "It needs more work.".to_string() };
            let question = "Is the change ready to ship?".to_string();
            let definition = Definition::new(custom::new_id(), name.to_string(), question, answer, Point::AgentAsks);
            store::create(home.path(), &definition, mode, 1).expect("the decision kept");
        }
        let decide = |name: &str, json: bool| {
            let mut args = words(&["decide", name, "--context", "the diff adds one test"]);
            if json {
                args.push("--json".to_string());
            }
            run(&args, &project, 1, 0)
        };
        let yes = AGENT_TOOL_ASK_OPTIONS[0];
        let on = decide("Ship", false).expect("a report");
        assert_eq!(on, Report { text: custom::line("Ship", yes, 0.9), exit: HEADLESS_LOOP_EXIT_DONE });
        let recording = decide("Wait", false).expect("a report");
        assert_eq!(recording.exit, NO_ANSWER_EXIT);
        assert!(!recording.text.contains(&custom::line("Wait", yes, 0.9)), "{}", recording.text);
        let said: Value = serde_json::from_str(&decide("Wait", true).expect("a report").text).expect("json");
        assert_eq!((&said["line"], &said["mode"]), (&Value::Null, &json!(JevMode::Shadow.key())), "{said}");
        assert_eq!(answered.load(Ordering::SeqCst), 3, "each was asked");
        let unknown = decide("Nobody", false).expect_err("no decision has the name");
        assert_eq!(unknown.exit, NO_ANSWER_EXIT);
        assert!(unknown.message.contains("Nobody"), "{}", unknown.message);
        assert_eq!(answered.load(Ordering::SeqCst), 3, "an unknown name asks nothing");
    });
}

/// The summary's JSON carries the person's own decisions under
/// `customDecisions` (t-62170): each with its id, name and mode, the version
/// it asks with now, and every earlier version a row names counted apart.
#[test]
fn the_summary_names_each_decision_and_counts_its_versions_apart() {
    use std::fmt::Write as _;
    use zerocode_core::jev::custom::{self, store, Answer, Definition, Point};
    use zerocode_core::jev::door::ANSWERED_OUTCOME;
    use zerocode_core::jev::{JevMode, CUSTOM_DECISIONS};
    let home = tempfile::tempdir().expect("a config home");
    let project = tempfile::tempdir().expect("a project");
    under_home(home.path(), || {
        let answer = Answer::YesNo { yes: "A visible change.".to_string(), no: "No visible change.".to_string() };
        let question = "Did the product change?".to_string();
        let mut decision = Definition::new(custom::new_id(), "Release notes".to_string(), question, answer, Point::TurnEnd);
        let first = store::create(home.path(), &decision, JevMode::Shadow, 1).expect("the decision kept");
        decision.question = "Did what a person sees change?".to_string();
        let second = store::edit(home.path(), &decision, 2).expect("the decision changed");
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
        let now = i64::try_from(since.expect("after 1970").as_millis()).expect("a time");
        let ledger = jev_summary::ledger_roots(project.path())[1].join(CUSTOM_DECISIONS.ledger);
        let mut rows = String::new();
        for (version, at) in [(&first, now - 2_000), (&first, now - 1_000), (&second, now - 500)] {
            let row = json!({"at": at, "decision": decision.id, "definition": version, "outcome": ANSWERED_OUTCOME,
                "requests": 1, "rubricVersion": CUSTOM_DECISIONS.rubric_version});
            let _ = writeln!(rows, "{row}");
        }
        std::fs::create_dir_all(ledger.parent().expect("a folder")).expect("the folder");
        std::fs::write(&ledger, rows).expect("the ledger");
        let request = SummaryRequest { cwd: None, sessions: None, recent: 0, act_lines: false, json: true };
        let said: Value = serde_json::from_str(&run_summary(&request, project.path(), now, 0).text).expect("json");
        let decisions = said["customDecisions"].as_array().expect("the person's decisions");
        assert_eq!(decisions.len(), 1, "{said}");
        let one = &decisions[0];
        let named = (&one["id"], &one["name"], &one["mode"]);
        assert_eq!(named, (&json!(decision.id), &json!("Release notes"), &json!(JevMode::Shadow.key())), "{one}");
        assert_eq!((&one["definition"], &one["requestsWeek"]), (&json!(second), &json!(1)), "{one}");
        let earlier: Vec<(&Value, &Value)> = one["earlier"]
            .as_array()
            .expect("its earlier versions")
            .iter()
            .map(|version| (&version["definition"], &version["requestsWeek"]))
            .collect();
        assert_eq!(earlier, [(&json!(first), &json!(2))], "{one}");
    });
}


#[test]
fn every_builtin_definition_is_read_without_running_a_judgment() {
    for seat in zerocode_core::jev::JEV_USES.iter().filter(|row| row.id != zerocode_core::jev::CUSTOM_DECISIONS.id) {
        let args = vec!["definition".to_string(), seat.id.to_string(), "--json".to_string()];
        let report = super::run(&args, std::path::Path::new("."), 0, 0).expect("shipped definition");
        assert_eq!(report.exit, 0);
        let value: serde_json::Value = serde_json::from_str(&report.text).unwrap();
        assert_eq!(value["id"], seat.id);
        assert!(value["words"].is_string());
    }
}

#[test]
fn every_builtin_editor_exports_typed_questions_and_criteria() {
    use zerocode_core::jev::{self, builtin};
    for seat in jev::JEV_USES.iter().filter(|seat| seat.id != jev::CUSTOM_DECISIONS.id) {
        let args = vec!["definition".to_string(), seat.id.to_string(), "--json".to_string()];
        let report = super::run(&args, std::path::Path::new("."), 0, 0).unwrap();
        let value: serde_json::Value = serde_json::from_str(&report.text).unwrap();
        let questions: Vec<builtin::QuestionDefinition> = serde_json::from_value(value["questions"].clone()).unwrap();
        assert_eq!(questions.is_empty(), [jev::AGENT_TOOL.id, jev::PROJECT_RULES.id, jev::JUDGMENT_CACHE.id].contains(&seat.id), "{}", seat.id);
        let edits = questions.iter().map(|question| (question.id.clone(), question.question.clone())).collect();
        builtin::validate_against(&edits, &questions).unwrap_or_else(|error| panic!("{}: {error}", seat.id));
    }
}
