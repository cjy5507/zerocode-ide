use serde_json::json;

use super::*;

fn entry(handle: &str, value: FormValue) -> FillEntry {
    FillEntry {
        handle: handle.to_string(),
        value,
    }
}

fn text(words: &str) -> FormValue {
    FormValue::Text(words.to_string())
}

fn said(handle: &str, status: FillStatus) -> FillResult {
    FillResult {
        handle: handle.to_string(),
        status,
        ..FillResult::default()
    }
}

#[test]
fn a_bundle_keeps_the_order_it_was_written_in() {
    // A date before the time it loads: the page's order is the writer's.
    let written = fill_entries(r##"{"#zeta": "2026-10-05", "#alpha": true, "#mid": 3}"##)
        .expect("an object bundle");
    assert_eq!(
        written,
        [
            entry("#zeta", text("2026-10-05")),
            entry("#alpha", FormValue::Flag(true)),
            entry("#mid", text("3")),
        ]
    );
    let listed = fill_entries(
        r##"[{"handle": "#zeta", "value": "2026-10-05"}, {"handle": "#alpha", "value": true}, {"handle": "#mid", "value": 3}]"##,
    )
    .expect("a list bundle");
    assert_eq!(listed, written, "the two shapes say the same bundle");
    let framed = fill_entries(r##"{"iframe#pay >> #card": "4000"}"##).expect("a framed handle");
    assert_eq!(framed[0].handle, "iframe#pay >> #card");
}

#[test]
fn a_bundle_that_cannot_be_filled_is_refused_by_name() {
    for (bundle, why) in [
        ("", "not JSON"),
        ("\"#a\"", "a bare string"),
        ("{}", "empty"),
        ("[]", "empty list"),
        (r##"{"#a": null}"##, "a null value"),
        (r##"{"#a": ["x"]}"##, "a list value"),
        (r##"{"#a": "x", "#a": "y"}"##, "a handle twice"),
        (r##"{"  ": "x"}"##, "an empty handle"),
        (
            r##"[{"handle": "#a", "value": "x", "extra": 1}]"##,
            "an unknown key",
        ),
    ] {
        assert!(fill_entries(bundle).is_err(), "{why}: {bundle}");
    }
    let long = format!(
        r##"{{"#a": "{}"}}"##,
        "x".repeat(BROWSER_FILL_VALUE_CAP + 1)
    );
    let refused = fill_entries(&long).expect_err("a value past the cap");
    assert!(refused.contains("#a"), "{refused}");
    let many: serde_json::Map<String, serde_json::Value> = (0..=BROWSER_FORM_FIELD_CAP)
        .map(|at| (format!("#f{at}"), json!("x")))
        .collect();
    assert!(fill_entries(&serde_json::Value::Object(many).to_string()).is_err());
}

#[test]
fn a_fill_tries_again_only_what_the_page_may_still_bring() {
    let bundle = vec![
        entry("#name", text("Kim")),
        entry("#time", text("09:00")),
        entry("#plan", text("Indoor")),
        entry("#pw", text("hidden")),
    ];
    let mut ledger = FillLedger::new(bundle.clone());
    assert_eq!(ledger.next(), bundle, "the first pass writes everything");
    ledger.record(
        &bundle,
        FillPass {
            results: vec![
                said("#name", FillStatus::Set),
                said("#time", FillStatus::NoOption),
                said("#plan", FillStatus::NotFound),
                said("#pw", FillStatus::Secret),
            ],
            left: vec![FormField {
                handle: "#time".into(),
                required: true,
                ..FormField::default()
            }],
            ..FillPass::default()
        },
    );
    let second = ledger.next();
    assert_eq!(
        second.iter().map(|e| e.handle.as_str()).collect::<Vec<_>>(),
        ["#time", "#plan"],
        "what took and what never will are not written again"
    );
    ledger.record(
        &second,
        FillPass {
            results: vec![
                said("#time", FillStatus::Set),
                said("#plan", FillStatus::Mismatch),
            ],
            left: Vec::new(),
            ..FillPass::default()
        },
    );
    let third = ledger.next();
    assert_eq!(third, [entry("#plan", text("Indoor"))]);
    ledger.record(
        &third,
        FillPass {
            results: vec![said("#plan", FillStatus::Same)],
            left: Vec::new(),
            ..FillPass::default()
        },
    );
    assert!(ledger.next().is_empty());
    let report = ledger.report();
    assert_eq!(
        report
            .results
            .iter()
            .map(|r| (r.handle.as_str(), r.status))
            .collect::<Vec<_>>(),
        [
            ("#name", FillStatus::Set),
            ("#time", FillStatus::Set),
            ("#plan", FillStatus::Same),
            ("#pw", FillStatus::Secret),
        ],
        "the bundle's order, each field's last word"
    );
    assert_eq!(report.passes, 3);
    assert_eq!((report.took(), report.all_took()), (3, false));
    assert!(
        report.left.is_empty(),
        "what is left is the last pass's word"
    );
}

#[test]
fn a_fill_stops_chasing_a_value_the_page_keeps_rewriting() {
    let bundle = vec![entry("#phone", text("01055500123"))];
    let mut ledger = FillLedger::new(bundle);
    let mut passes = 0;
    loop {
        let asked = ledger.next();
        if asked.is_empty() {
            break;
        }
        passes += 1;
        assert!(passes <= BROWSER_FILL_PASSES, "the passes are bounded");
        ledger.record(
            &asked,
            FillPass {
                results: vec![said("#phone", FillStatus::Mismatch)],
                left: Vec::new(),
                ..FillPass::default()
            },
        );
    }
    assert_eq!(passes, BROWSER_FILL_PASSES);
    assert_eq!(ledger.report().results[0].status, FillStatus::Mismatch);
}

#[test]
fn a_field_the_page_did_not_answer_reads_unread() {
    let bundle = vec![entry("#a", text("x")), entry("#b", text("y"))];
    let mut ledger = FillLedger::new(bundle.clone());
    let pass: FillPass = serde_json::from_value(json!({
        "results": [{ "handle": "#a", "status": "exploded", "now": "x" }],
    }))
    .expect("a pass with a word this door does not know");
    ledger.record(&bundle, pass);
    let report = ledger.report();
    assert_eq!(
        report
            .results
            .iter()
            .map(|r| (r.handle.as_str(), r.status))
            .collect::<Vec<_>>(),
        [("#a", FillStatus::Unread), ("#b", FillStatus::Unread)]
    );
}

/// A read as the page script answers it: two sections, a choice past the
/// cap, a secret, a button and a frame of another origin.
fn booking() -> FormRead {
    serde_json::from_value(json!({
        "fields": [
            { "handle": "#in-date", "kind": "date", "label": "Entry date", "section": "Schedule",
              "value": "", "required": true },
            { "handle": "select[name=\"in-time\"]", "kind": "select", "label": "Entry time",
              "section": "Schedule", "value": "", "required": true,
              "options": ["06:00", "06:30"], "moreOptions": 8 },
            { "handle": "#phone-1", "kind": "tel", "label": "Mobile (1/3)", "section": "Driver",
              "value": "010", "maxLength": 3 },
            { "handle": "#pw", "kind": "password", "label": "Password", "section": "Driver",
              "value": "", "masked": true, "required": true },
            { "handle": "#agree", "kind": "checkbox", "label": "I agree", "section": "Driver",
              "value": true, "required": true, "error": "" },
        ],
        "actions": [
            { "handle": "#send-code", "label": "Send code" },
            { "handle": "form > button", "label": "Book", "disabled": true },
        ],
        "more": 2,
        "sealedFrames": ["pay.example.com"],
    }))
    .expect("the page's read")
}

#[test]
fn a_fields_read_says_each_field_under_its_section_with_its_choices() {
    let lines = fields_lines(&booking());
    for expected in [
        "양식 칸 5개 (필수 4, 비어 있는 필수 3)",
        "[Schedule]\n  #in-date · date · Entry date * = \"\"\n",
        "  select[name=\"in-time\"] · select · Entry time * = \"\" ▸ 06:00 | 06:30 (+8)",
        "[Driver]\n  #phone-1 · tel · Mobile (1/3) = \"010\" 최대 3자",
        "  #pw · password · Password * = (가림)",
        "  #agree · checkbox · I agree * = true",
        "(+2칸 더",
        "버튼: #send-code 「Send code」 · form > button 「Book」 (꺼짐)",
        "읽지 못한 다른 출처의 틀: pay.example.com",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
    assert_eq!(lines.matches("[Driver]").count(), 1, "one header a section");
}

#[test]
fn a_json_read_is_the_read_flagged_as_the_pages_words() {
    let answer = fields_json(&booking());
    assert_eq!(answer[crate::untrusted::JSON_FLAG], json!(true));
    assert_eq!(answer["fields"][1]["moreOptions"], json!(8));
    assert_eq!(answer["fields"][3]["value"], json!(""));
    assert_eq!(answer["actions"][0]["handle"], json!("#send-code"));
}

#[test]
fn a_fill_report_names_what_took_and_why_the_rest_did_not() {
    let report = FillReport {
        results: vec![
            FillResult {
                now: text("Kim"),
                label: "Name".into(),
                ..said("#name", FillStatus::Set)
            },
            FillResult {
                now: FormValue::Flag(true),
                label: "I agree".into(),
                ..said("#agree", FillStatus::Same)
            },
            FillResult {
                label: "Entry time".into(),
                options: vec!["06:00".into(), "06:30".into()],
                ..said("#time", FillStatus::NoOption)
            },
            FillResult {
                label: "Mobile".into(),
                now: text("010-555"),
                ..said("#phone", FillStatus::Mismatch)
            },
            FillResult {
                label: "Password".into(),
                ..said("#pw", FillStatus::Secret)
            },
        ],
        left: vec![FormField {
            handle: "#code".into(),
            kind: "text".into(),
            label: "Code".into(),
            required: true,
            ..FormField::default()
        }],
        passes: 2,
        ..FillReport::default()
    };
    let lines = fill_lines(&report);
    for expected in [
        "채움 2/5칸 (2회)",
        "  ✓ #name Name = \"Kim\"",
        "  ✓ #agree I agree = true",
        "  ✗ #time Entry time: 그 값의 선택지가 없음 ▸ 06:00 | 06:30",
        "  ✗ #phone Mobile: 다시 읽으니 다른 값 (\"010-555\")",
        "  ✗ #pw Password: 비밀 칸은 fill이 쓰지 않음",
        "남은 칸:\n  #code · text · Code — 필수, 비어 있음",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
}

/// A fill on a form that changed since its agent read it (t-37883, m-40824):
/// the page said so before writing anything — the fill ends there, every
/// field unwritten, and the agent is told to read the form again.
#[test]
fn a_fill_on_a_form_changed_since_its_read_ends_at_once_and_writes_nothing() {
    let bundle = vec![entry("#name", text("Kim")), entry("#time", text("09:00"))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(
        &bundle,
        FillPass {
            stale: true,
            fingerprint: "11:abc".into(),
            ..FillPass::default()
        },
    );
    assert!(ledger.next().is_empty(), "a stale form is not tried again");
    let report = ledger.report();
    assert!(report.stale && !report.all_took(), "{report:?}");
    assert_eq!(report.fingerprint, "11:abc");
    assert!(
        report
            .results
            .iter()
            .all(|result| result.status == FillStatus::Unread),
        "nothing was written"
    );
    let said = fill_lines(&report);
    assert!(
        said.starts_with(crate::computer_use_protocol::error_code::FORM_STALE)
            && said.contains("fields"),
        "{said}"
    );
}

/// A date the fill could not pick on the page's calendar is said with the
/// calendar as the page shows it — what the agent presses to finish it.
#[test]
fn a_calendar_the_fill_could_not_pick_on_is_said_with_its_heading_pagers_and_days() {
    let pass: FillPass = serde_json::from_value(json!({
        "results": [{ "handle": "#out", "status": "no_option", "label": "Return",
            "widget": { "heading": "2026년 10월", "month": "2026-10", "pagers": ["#prev", "#next"], "days": "div.days" } }],
    }))
    .expect("a pass with a calendar");
    let bundle = vec![entry("#out", text("2026-12-03"))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    let report = ledger.report();
    assert_eq!(
        report.results[0]
            .widget
            .as_ref()
            .map(|widget| widget.days.as_str()),
        Some("div.days")
    );
    let lines = fill_lines(&report);
    assert!(
        lines.contains("달력 「2026년 10월」 넘김 #prev · #next · 날짜 칸 div.days"),
        "{lines}"
    );
}

/// What a person can press that the read named no kind for is said with its
/// words and caption, and how to go on: press it, read again.
#[test]
fn a_pressable_thing_without_a_kind_is_said_in_the_read() {
    let read: FormRead = serde_json::from_value(json!({
        "fields": [{ "handle": "#name", "kind": "text", "label": "Name", "value": "" }],
        "unknowns": [
            { "handle": "#region", "label": "Pick a region ▾", "caption": "Region" },
            { "handle": "div.chip", "label": "Breakfast", "caption": "" },
        ],
    }))
    .expect("a read with things of no kind");
    let lines = fields_lines(&read);
    assert!(
        lines.contains(
            "종류를 모르는 조작: #region 「Pick a region ▾」 (Region) · div.chip 「Breakfast」 — click 뒤 fields로 다시 읽기"
        ),
        "{lines}"
    );
}

/// A field that is off or cannot be written says the words the page gives
/// about it, in the read (t-41387): the agent learns what switches it on.
/// A field that is on says nothing more.
#[test]
fn a_field_that_is_off_says_the_words_the_page_gives_about_it() {
    let read: FormRead = serde_json::from_value(json!({
        "fields": [
            { "handle": "#state", "kind": "select", "label": "State", "value": "", "disabled": true,
              "hint": "Choose a country first." },
            { "handle": "#ref", "kind": "text", "label": "Reference", "value": "X1", "readOnly": true,
              "hint": "Filled in by the shop; you cannot change it." },
            { "handle": "#nick", "kind": "text", "label": "Nickname", "value": "" },
        ],
    }))
    .expect("a read with a hint");
    let lines = fields_lines(&read);
    for expected in [
        "  #state · select · State = \"\" (꺼짐) — 안내: Choose a country first.",
        "  #ref · text · Reference = \"X1\" (직접 못 씀) — 안내: Filled in by the shop; you cannot change it.",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
    assert_eq!(
        lines.matches("안내").count(),
        2,
        "a field that is on says no more:\n{lines}"
    );
}

/// A box the page lets scroll that has more to read below what is shown is
/// said, so the agent knows what to read to its end when a field stays off.
#[test]
fn a_box_with_more_to_read_below_it_is_said_in_the_read() {
    let read: FormRead = serde_json::from_value(json!({
        "fields": [{ "handle": "#name", "kind": "text", "label": "Name", "value": "" }],
        "scrollBoxes": [
            { "handle": "#licence", "label": "Licence terms 1 Licence terms 2" },
            { "handle": "div.policy", "label": "Privacy" },
        ],
    }))
    .expect("a read with boxes to scroll");
    let lines = fields_lines(&read);
    assert!(
        lines.contains(
            "끝까지 안 내린 스크롤 상자: #licence 「Licence terms 1 Licence terms 2」 · div.policy 「Privacy」 — 꺼진 칸이 있으면 끝까지 내려 보고 fields로 다시 읽기"
        ),
        "{lines}"
    );
    assert!(
        !fields_lines(&booking()).contains("스크롤 상자"),
        "a read with no such box says nothing"
    );
}

/// A fill that meets a field that is off says why, in the page's words.
#[test]
fn a_fill_that_meets_a_field_that_is_off_says_the_words_the_page_gives_about_it() {
    let pass: FillPass = serde_json::from_value(json!({
        "results": [{ "handle": "#accept", "status": "disabled", "label": "I have read the licence",
            "hint": "Scroll the licence to its end to switch this on." }],
    }))
    .expect("a pass with a field that is off");
    let bundle = vec![entry("#accept", FormValue::Flag(true))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    let lines = fill_lines(&ledger.report());
    assert!(
        lines.contains("  ✗ #accept I have read the licence: 꺼져 있음 — 안내: Scroll the licence to its end to switch this on."),
        "{lines}"
    );
}
