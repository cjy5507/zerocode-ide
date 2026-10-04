use super::*;

/// A small table, so a short log already folds: two steps from the start, one
/// from the end, one on each side of a failure.
fn small() -> Limits {
    Limits {
        digest_head_steps: 2,
        digest_tail_steps: 1,
        digest_context_steps: 1,
        digest_rows_max: 8,
        digest_text_chars: 24,
        digest_verbs_max: 2,
        digest_facts_max: 24,
        ..Limits::default()
    }
}

/// One line as the window's recorder writes it: the verb first in `argv`, a
/// measured time, and — on a failed step — the refusal and its code.
fn step(n: usize, verb: &str, ok: bool) -> String {
    let refusal = if ok {
        String::new()
    } else {
        r#","error":"the selector never showed","code":"timeout""#.to_string()
    };
    format!(
        r#"{{"n":{n},"at_epoch_ms":{at},"tool":"browser","verb":"{verb}","argv":["{verb}","browser-1","the go button"],"ok":{ok},"observation":{{"act_ms":{ms}}},"frame":{{"scale":2.0}}{refusal}}}"#,
        at = 1_000 + n * 10,
        ms = n * 2,
    )
}

fn log(steps: &[(usize, &str, bool)]) -> String {
    steps
        .iter()
        .map(|(n, verb, ok)| step(*n, verb, *ok) + "\n")
        .collect()
}

fn read_steps(text: &str, limits: &Limits) -> Steps {
    let digest = read(Format::Lines, text.as_bytes(), limits);
    assert!(
        matches!(digest, Some(Digest::Steps(_))),
        "a step log reads as steps: {digest:?}"
    );
    match digest {
        Some(Digest::Steps(steps)) => steps,
        _ => Steps::default(),
    }
}

/// What a row shows, in a word: `7!` a failed step, `…3` a fold of three.
fn shape(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            Row::Step(step) if step.ok => step.n.to_string(),
            Row::Step(step) => format!("{}!", step.n),
            Row::Fold { steps, failed: 0 } => format!("…{steps}"),
            Row::Fold { steps, failed } => format!("…{steps}/{failed}!"),
        })
        .collect()
}

/// The whole log is counted; the list is its first steps, every failed step
/// with its neighbours, and its last step, with the runs between folded.
#[test]
fn a_step_log_is_counted_whole_and_shown_in_part() {
    let verbs: Vec<(usize, &str, bool)> = (1..=12)
        .map(|n| match n {
            1 => (n, "open", true),
            7 => (n, "wait", false),
            12 => (n, "close", true),
            _ => (n, "click", true),
        })
        .collect();
    let steps = read_steps(&log(&verbs), &small());
    assert_eq!((steps.total, steps.failed, steps.skipped), (12, 1, 0));
    assert!(!steps.cut);
    assert_eq!(
        (steps.started_ms, steps.ended_ms),
        (Some(1_010), Some(1_120))
    );
    assert_eq!(
        shape(&steps.rows),
        ["1", "2", "…3", "6", "7!", "8", "…3", "12"],
        "the first two, the failed one between its neighbours, the last"
    );
    // The verbs used most, named to the table's count; the rest are one sum.
    assert_eq!(
        steps.verbs,
        [
            VerbCount {
                verb: "click".into(),
                steps: 9
            },
            VerbCount {
                verb: "close".into(),
                steps: 1
            },
        ]
    );
    assert_eq!(steps.other_verbs, 2);
    // A row is the step in plain fields: its target without the verb, its
    // time, and a refusal cut to the table's characters.
    let failed = steps.rows.iter().find_map(|row| match row {
        Row::Step(step) if !step.ok => Some(step.clone()),
        _ => None,
    });
    assert_eq!(
        failed,
        Some(StepRow {
            n: 7,
            at_ms: 1_070,
            tool: "browser".into(),
            verb: "wait".into(),
            target: "browser-1 the go button".into(),
            ok: false,
            ms: Some(14),
            error: Some("the selector never showe".into()),
            code: Some("timeout".into()),
            framed: false,
        })
    );
}

/// Past the table's rows nothing more is listed, and every failure is still
/// counted — in the totals and on the fold that hides it.
#[test]
fn a_log_past_the_table_folds_the_rest_and_still_counts_every_failure() {
    let limits = Limits {
        digest_rows_max: 3,
        ..small()
    };
    let all_failed: Vec<(usize, &str, bool)> = (1..=6).map(|n| (n, "click", false)).collect();
    let steps = read_steps(&log(&all_failed), &limits);
    assert_eq!((steps.total, steps.failed), (6, 6));
    assert_eq!(shape(&steps.rows), ["1!", "2!", "3!", "…3/3!"]);
}

/// A torn line and a record that is not a step are skipped and counted; a
/// blank line is nothing.
#[test]
fn lines_that_are_not_steps_are_skipped_and_counted() {
    let text = format!(
        "{}\n{{\"n\":\n{{\"note\":\"between\"}}\n\n{}\n",
        step(1, "open", true),
        step(2, "click", true)
    );
    let steps = read_steps(&text, &small());
    assert_eq!((steps.total, steps.failed, steps.skipped), (2, 0, 2));
    assert_eq!(shape(&steps.rows), ["1", "2"]);
}

/// A log longer than the table reads is counted as far as it was read, and
/// says so; the line the reading stopped in is not a torn line.
#[test]
fn a_log_longer_than_the_table_reads_says_it_was_cut() {
    let text = log(&[(1, "open", true), (2, "click", true), (3, "click", true)]);
    let first_line = text.find('\n').map_or(0, |at| at + 1);
    let limits = Limits {
        digest_bytes_max: first_line as u64 + 10,
        ..small()
    };
    let steps = read_steps(&text, &limits);
    assert_eq!((steps.total, steps.skipped), (1, 0));
    assert!(steps.cut, "the reading stopped inside the second line");
    // A log that ends exactly where the table does was not cut.
    let exact = Limits {
        digest_bytes_max: text.len() as u64,
        ..small()
    };
    let whole = read_steps(&text, &exact);
    assert_eq!(whole.total, 3);
    assert!(!whole.cut);
}

/// The operator's state record is read by its keys, whatever its layout.
#[test]
fn the_operators_state_record_is_read_by_its_keys() {
    let compact = r#"{"actions":7,"lastVerb":"click","lastOk":false,"lastError":"the selector never showed","lastAtMs":5,"consecutiveFailures":2,"recent":["look","click"],"unchangedLooks":1,"stuck":true}"#;
    let expected = State {
        actions: 7,
        last_verb: Some("click".into()),
        last_ok: Some(false),
        last_error: Some("the selector never showe".into()),
        last_at_ms: 5,
        consecutive_failures: 2,
        unchanged_looks: 1,
        stuck: true,
        recent: vec!["look".into(), "click".into()],
    };
    assert_eq!(
        read(Format::Document, compact.as_bytes(), &small()),
        Some(Digest::State(expected.clone()))
    );
    let value: serde_json::Value = serde_json::from_str(compact).expect("the fixture is JSON");
    let pretty = serde_json::to_string_pretty(&value).expect("serialises");
    assert_eq!(
        read(Format::Document, pretty.as_bytes(), &small()),
        Some(Digest::State(expected)),
        "a record written over many lines is the same record"
    );
}

/// Any other JSON is its top-level values: a text cut to the table, a number
/// as written, and a list or a nested record as its size.
#[test]
fn any_other_record_is_its_top_level_values() {
    let record = r#"{"goal":"walk the form to its very last page","steps":[1,2,3],"meta":{"a":1,"b":2},"done":true,"left":null,"n":4}"#;
    let digest = read(Format::Document, record.as_bytes(), &small());
    assert!(
        matches!(digest, Some(Digest::Facts(_))),
        "a record that is no step log and no state reads as facts: {digest:?}"
    );
    let Some(Digest::Facts(facts)) = digest else {
        return;
    };
    assert_eq!((facts.records, facts.more, facts.cut), (1, 0, false));
    let value_of = |key: &str| {
        facts
            .facts
            .iter()
            .find(|fact| fact.key == key)
            .map(|fact| fact.value.clone())
    };
    assert_eq!(
        value_of("goal"),
        Some(FactValue::Text("walk the form to its ver".into()))
    );
    assert_eq!(value_of("steps"), Some(FactValue::List(3)));
    assert_eq!(value_of("meta"), Some(FactValue::Record(2)));
    assert_eq!(value_of("done"), Some(FactValue::Flag(true)));
    assert_eq!(value_of("left"), Some(FactValue::Nothing));
    assert_eq!(value_of("n"), Some(FactValue::Number("4".into())));

    // The table bounds how many are listed, and the rest are counted.
    let two = Limits {
        digest_facts_max: 2,
        ..small()
    };
    let Some(Digest::Facts(bounded)) = read(Format::Document, record.as_bytes(), &two) else {
        panic!("the same record under a smaller table still reads as facts");
    };
    assert_eq!((bounded.facts.len(), bounded.more), (2, 4));

    // A log of records that are not steps is counted by its lines, and shows
    // the first of them; a top-level list by its items.
    let lines = "{\"walk\":1,\"judgment\":\"go\"}\n{\"walk\":2,\"judgment\":\"stop\"}\n";
    let Some(Digest::Facts(counted)) = read(Format::Lines, lines.as_bytes(), &small()) else {
        panic!("a log of other records reads as facts");
    };
    assert_eq!(counted.records, 2);
    assert!(
        counted
            .facts
            .iter()
            .any(|fact| fact.key == "judgment" && fact.value == FactValue::Text("go".into()))
    );
    let Some(Digest::Facts(listed)) = read(
        Format::Document,
        br#"[{"walk":1},{"walk":2},{"walk":3}]"#.as_slice(),
        &small(),
    ) else {
        panic!("a top-level list of records reads as facts");
    };
    assert_eq!(listed.records, 3);
}

/// What is not JSON has no digest: the drawer shows it as the text it is.
#[test]
fn a_file_that_holds_no_record_has_no_digest() {
    for (format, text) in [
        (Format::Document, "plain words, not a record"),
        (Format::Document, ""),
        (Format::Document, "42"),
        (Format::Lines, "plain words\nmore words\n"),
        (Format::Lines, ""),
    ] {
        assert_eq!(read(format, text.as_bytes(), &small()), None, "{text:?}");
    }
    // A document longer than the table reads is not read in part.
    let long = Limits {
        digest_bytes_max: 8,
        ..small()
    };
    assert_eq!(
        read(
            Format::Document,
            br#"{"goal":"walk the form"}"#.as_slice(),
            &long
        ),
        None
    );
}

/// The extension says how the bytes are read, and nothing about what they hold.
#[test]
fn the_extension_names_the_format() {
    let cases: &[(&str, Option<Format>)] = &[
        ("steps.jsonl", Some(Format::Lines)),
        ("/a/b/browser-action.JSONL", Some(Format::Lines)),
        ("state.json", Some(Format::Document)),
        ("walk-002.Json", Some(Format::Document)),
        ("run.log", None),
        ("report.md", None),
        ("notes", None),
    ];
    for (name, expected) in cases {
        assert_eq!(Format::of(Path::new(name)), *expected, "{name}");
    }
}

/// The window reads these shapes: the tag says which digest and which row, and
/// what a step did not have is not written.
#[test]
fn the_wire_shape_tags_the_digest_and_its_rows() {
    let text = log(&[
        (1, "open", true),
        (2, "click", true),
        (3, "click", true),
        (4, "click", true),
        (5, "click", true),
        (6, "click", true),
    ]);
    let wire =
        serde_json::to_value(read(Format::Lines, text.as_bytes(), &small())).expect("serialises");
    assert_eq!(wire["kind"], "steps");
    assert_eq!(wire["total"], 6);
    assert_eq!(wire["rows"][0]["row"], "step");
    assert_eq!(wire["rows"][0]["verb"], "open");
    assert_eq!(wire["rows"][0]["ms"], 2);
    assert!(
        wire["rows"][0].get("error").is_none() && wire["rows"][0].get("framed").is_none(),
        "a passed step with no picture writes neither: {}",
        wire["rows"][0]
    );
    assert_eq!(
        wire["rows"][2],
        serde_json::json!({ "row": "fold", "steps": 3, "failed": 0 })
    );
    let state = serde_json::to_value(read(
        Format::Document,
        br#"{"actions":1,"consecutiveFailures":0,"lastOk":true}"#.as_slice(),
        &small(),
    ))
    .expect("serialises");
    assert_eq!(state["kind"], "state");
    assert_eq!(state["last_ok"], true);
    assert_eq!(state["consecutive_failures"], 0);
    let facts = serde_json::to_value(read(
        Format::Document,
        br#"{"goal":"walk"}"#.as_slice(),
        &small(),
    ))
    .expect("serialises");
    assert_eq!(facts["kind"], "facts");
    assert_eq!(
        facts["facts"][0],
        serde_json::json!({ "key": "goal", "value": { "is": "text", "value": "walk" } })
    );
}

/// What the preview cache counts a digest as: more for more rows, never
/// nothing, and never the file's own size.
#[test]
fn a_digest_weighs_what_it_keeps_and_not_what_it_read() {
    let short = log(&[(1, "open", true)]);
    let long: Vec<(usize, &str, bool)> = (1..=400).map(|n| (n, "click", n % 50 != 0)).collect();
    let long = log(&long);
    let weigh =
        |text: &str| read(Format::Lines, text.as_bytes(), &small()).map(|held| held.weight());
    let (one, many) = (weigh(&short).unwrap_or(0), weigh(&long).unwrap_or(0));
    assert!(one > 0, "a digest of one step weighs something");
    assert!(many > one, "more rows weigh more: {one} then {many}");
    assert!(
        many < long.len() as u64 / 4,
        "a digest is bounded by the table, not by the log: {many} of {} bytes",
        long.len()
    );
}
