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
        (r##"{"#a": [null]}"##, "a list holding a null"),
        (r##"{"#a": [true]}"##, "a list holding a flag"),
        (r##"{"#a": [["x"]]}"##, "a list holding a list"),
        (r##"{"#a": [{"b": 1}]}"##, "a list holding an object"),
        (r##"{"#a": {"b": 1}}"##, "an object value"),
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

/// A group of buttons that say whether they are pressed is one field (t-41656): its value is the
/// list of the words of the buttons pressed, and a bundle gives the list it wants held — words, or
/// numbers written as words; none at all is an empty list; a list of anything else is refused.
#[test]
fn a_list_of_words_is_a_value_a_bundle_gives_a_group_of_chips() {
    let written = fill_entries(r##"{"#colours": ["Red", "Blue"], "#none": [], "#n": ["1", 2]}"##);
    assert!(written.is_ok(), "a list of words is a value: {written:?}");
    let written = written.unwrap_or_default();
    assert_eq!(written.len(), 3, "{written:?}");
    let values: Vec<serde_json::Value> = written
        .iter()
        .map(|one| serde_json::to_value(&one.value).unwrap_or_default())
        .collect();
    assert_eq!(
        values[0],
        json!(["Red", "Blue"]),
        "the words, in the order given"
    );
    assert_eq!(
        values[1],
        json!([]),
        "none is an empty list, the group with nothing pressed"
    );
    assert_eq!(
        values[2],
        json!(["1", "2"]),
        "a number is written as its words"
    );
    let long = format!(
        r##"{{"#a": ["{}"]}}"##,
        "x".repeat(BROWSER_FILL_VALUE_CAP + 1)
    );
    let refused = fill_entries(&long).expect_err("a word past the cap");
    assert!(refused.contains("#a"), "{refused}");
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
        "양식 칸 5개 (필수 4, 비어 있는 필수 2, 값을 읽지 않는 필수 1)",
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

/// A fill that read its buttons after the last pass, as the page answers it.
fn filled_with_buttons(fingerprint: &str) -> FillReport {
    let pass: FillPass = serde_json::from_value(json!({
        "results": [{ "handle": "#mail", "status": "set", "label": "Contact email",
            "now": "kim@example.com" }],
        "fingerprint": fingerprint,
        "actions": [
            { "handle": "#go", "label": "Continue", "disabled": false },
            { "handle": "#back", "label": "Go back", "disabled": true },
        ],
    }))
    .expect("a pass with the buttons after it");
    let bundle = vec![entry("#mail", text("kim@example.com"))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    ledger.report()
}

fn button(handle: &str, label: &str, disabled: bool) -> FormAction {
    FormAction {
        handle: handle.into(),
        label: label.into(),
        disabled,
        ..FormAction::default()
    }
}

/// A fill says, at its end, the state of the buttons the form has after it
/// (t-41387): the agent presses the step's button with no read between.
#[test]
fn a_fill_says_the_state_of_each_button_after_it() {
    let lines = fill_lines(&filled_with_buttons("5:aaa"));
    assert!(
        lines.ends_with("버튼: #go 「Continue」 켜짐, #back 「Go back」 꺼짐\n"),
        "{lines}"
    );
}

/// Against the form the agent read before the fill: the same fields are
/// "그대로" and a button the page took away is said as hidden; other fields
/// are "바뀜" and the agent reads again — the old form's buttons are not said.
#[test]
fn a_fill_says_whether_the_form_it_read_changed_and_which_button_went_away() {
    let before = vec![
        button("#go", "Continue", true),
        button("#back", "Go back", false),
        button("#later", "Skip this step", false),
    ];
    let stayed = fill_lines(&filled_with_buttons("5:aaa").against(Some("5:aaa"), &before));
    assert!(
        stayed.ends_with(
            "양식 그대로 — 버튼: #go 「Continue」 켜짐, #back 「Go back」 꺼짐; 숨김: #later 「Skip this step」\n"
        ),
        "{stayed}"
    );
    let changed = fill_lines(&filled_with_buttons("5:bbb").against(Some("5:aaa"), &before));
    assert!(
        changed.ends_with(
            "양식 바뀜(fields로 다시 읽기) — 버튼: #go 「Continue」 켜짐, #back 「Go back」 꺼짐\n"
        ),
        "{changed}"
    );
}

/// A form the agent never read has no "before" to be the same as: nothing is
/// said of a change, and the buttons are said all the same.
#[test]
fn a_fill_of_a_form_the_agent_never_read_says_nothing_of_a_change() {
    let lines = fill_lines(&filled_with_buttons("5:aaa").against(None, &[]));
    assert!(
        !lines.contains("양식 그대로") && !lines.contains("양식 바뀜"),
        "{lines}"
    );
    assert!(
        lines.contains("버튼: #go 「Continue」 켜짐"),
        "the buttons are said all the same:\n{lines}"
    );
}

/// A pass as the page answers it once it has settled (or not): the one field
/// took, the form's two buttons, and what the settle heard.
fn pass_heard(moving: Option<bool>) -> FillPass {
    serde_json::from_value(json!({
        "results": [{ "handle": "#mail", "status": "set", "label": "Contact email",
            "now": "kim@example.com" }],
        "fingerprint": "5:aaa",
        "moving": moving,
        "actions": [
            { "handle": "#go", "label": "Continue", "disabled": false },
            { "handle": "#back", "label": "Go back", "disabled": true },
        ],
    }))
    .expect("a pass the settle was heard on")
}

/// A fill read while the page was still changing does not call its form "the
/// same" or its buttons settled: it says so, and says what to do (t-41387).
#[test]
fn a_fill_that_ended_while_the_page_was_still_changing_says_so() {
    let bundle = vec![entry("#mail", text("kim@example.com"))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass_heard(Some(true)));
    let lines = fill_lines(&ledger.report().against(Some("5:aaa"), &[]));
    assert!(
        lines.ends_with(
            "아직 바뀌는 중(fields로 다시 읽기) — 버튼: #go 「Continue」 켜짐, #back 「Go back」 꺼짐\n"
        ),
        "{lines}"
    );
    assert!(!lines.contains("양식 그대로"), "{lines}");
}

/// The last word a settle gave stands for the fill: a pass that wrote nothing
/// heard none, so it keeps the one before it; a pass that settled replaces it.
#[test]
fn the_last_word_a_settle_gave_is_what_a_fill_ends_with() {
    let bundle = vec![entry("#mail", text("kim@example.com"))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass_heard(Some(true)));
    ledger.record(&bundle, pass_heard(None));
    assert!(
        fill_lines(&ledger.clone().report()).contains("아직 바뀌는 중"),
        "a pass that waited for nothing leaves the last word"
    );
    ledger.record(&bundle, pass_heard(Some(false)));
    assert!(
        !fill_lines(&ledger.report()).contains("아직 바뀌는 중"),
        "a pass whose page stood still ends it"
    );
}

/// A write that took the page to another document leaves nothing to read back:
/// the field is said to have been replaced, not "not found" or "set".
#[test]
fn a_field_the_page_replaced_after_the_write_says_so() {
    let pass: FillPass = serde_json::from_value(json!({
        "results": [{ "handle": "#mail", "status": "replaced", "label": "Contact email" }],
    }))
    .expect("a pass whose page was replaced");
    let bundle = vec![entry("#mail", text("kim@example.com"))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    let lines = fill_lines(&ledger.report());
    assert!(
        lines.contains(
            "  ✗ #mail Contact email: 쓴 뒤 페이지가 다른 문서로 바뀜 — fields로 다시 읽기"
        ),
        "{lines}"
    );
}

/// A button with no id or name is found by its place in the page: when the fill
/// shows a banner above it the place moves and the button is the same — one
/// that has the same words now is not one the page took away.
#[test]
fn a_button_whose_place_moved_is_not_said_to_be_hidden() {
    let before = vec![
        button("div:nth-of-type(2) > button", "다음", false),
        button("div:nth-of-type(3) > button", "이전", false),
    ];
    let pass: FillPass = serde_json::from_value(json!({
        "results": [{ "handle": "#mail", "status": "set", "label": "Contact email",
            "now": "kim@example.com" }],
        "fingerprint": "5:aaa",
        "actions": [
            { "handle": "div:nth-of-type(3) > button", "label": "다음", "disabled": false },
        ],
    }))
    .expect("a pass with the buttons moved");
    let bundle = vec![entry("#mail", text("kim@example.com"))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    let lines = fill_lines(&ledger.report().against(Some("5:aaa"), &before));
    assert!(
        lines.contains("숨김: div:nth-of-type(3) > button 「이전」"),
        "{lines}"
    );
    assert!(
        !lines.contains("숨김: div:nth-of-type(2) > button"),
        "the button that stands somewhere else now is not hidden:\n{lines}"
    );
}

/// A read made once the page has stood still for the settle's quiet window cannot
/// see what shows later — a check that waits, a timer — and the answer does not
/// say it is the page's last word: it says what was waited for and to read again
/// before submitting (t-41387, m-41513). A pass that waited for nothing, or a page
/// still changing (which already says to read again), makes no such claim.
#[test]
fn a_fill_that_waited_for_the_page_says_what_such_a_read_cannot_see() {
    let quiet = crate::agent_browser::BROWSER_SETTLE_QUIET_MS;
    let note = format!(
        "※ 쓴 뒤 {quiet} ms 동안 가만히 있는 것을 보고 읽었습니다. 그 뒤에 뜨는 오류는 못 봅니다 — 제출 전에 fields로 다시 읽으세요"
    );
    let bundle = vec![entry("#mail", text("kim@example.com"))];
    for (heard, says) in [(Some(false), true), (None, false), (Some(true), false)] {
        let mut ledger = FillLedger::new(bundle.clone());
        ledger.record(&bundle, pass_heard(heard));
        let lines = fill_lines(&ledger.report().against(Some("5:aaa"), &[]));
        assert_eq!(lines.contains(&note), says, "{heard:?}:\n{lines}");
        if says {
            assert!(
                lines.ends_with(&format!("{note}\n")),
                "the note is the last line:\n{lines}"
            );
        }
    }
}

/// A field whose value the door never reads — a password — is told as unread,
/// never counted as required and empty: the count line says how many required
/// fields have no value to look at, and the fill's list of what is left names a
/// field only for what the page said is wrong with it.
#[test]
fn a_secret_field_the_door_never_reads_is_told_as_unread_not_as_empty() {
    let read: FormRead = serde_json::from_value(json!({
        "fields": [
            { "handle": "#mail", "kind": "email", "label": "Email", "value": "", "required": true },
            { "handle": "#pw", "kind": "password", "label": "Password", "value": "", "masked": true, "required": true },
            { "handle": "#pw2", "kind": "password", "label": "Repeat", "value": "", "masked": true, "required": true },
        ],
    }))
    .expect("a read with two secrets");
    let lines = fields_lines(&read);
    assert!(
        lines.contains("양식 칸 3개 (필수 3, 비어 있는 필수 1, 값을 읽지 않는 필수 2)"),
        "{lines}"
    );
    let plain: FormRead = serde_json::from_value(json!({
        "fields": [{ "handle": "#mail", "kind": "email", "label": "Email", "value": "", "required": true }],
    }))
    .expect("a read with no secret");
    assert!(
        fields_lines(&plain).contains("양식 칸 1개 (필수 1, 비어 있는 필수 1)\n"),
        "a read with no secret says no more than it did"
    );
}

/// A button that opens a list says whether the list is open, and a name told by
/// number says it is only an order — in the words beside the field.
#[test]
fn a_list_field_says_whether_it_is_open_and_a_number_in_a_name_says_it_is_only_an_order() {
    let read: FormRead = serde_json::from_value(json!({
        "fields": [
            { "handle": "#branch", "kind": "combobox", "label": "Branch", "value": "Seoul", "open": true,
              "options": ["Hanbit", "Seoul"] },
            { "handle": "#prefix", "kind": "combobox", "label": "Phone prefix", "value": "US +1", "open": false },
            { "handle": "#n1", "kind": "text", "label": "Note #1", "value": "", "ordinal": true },
            { "handle": "#n2", "kind": "text", "label": "Note #2", "value": "", "ordinal": true },
            { "handle": "#nick", "kind": "text", "label": "Nickname", "value": "" },
        ],
    }))
    .expect("a read with a list button and numbered names");
    let lines = fields_lines(&read);
    for expected in [
        "  #branch · combobox · Branch = \"Seoul\" (열림) ▸ Hanbit | Seoul",
        "  #prefix · combobox · Phone prefix = \"US +1\" (닫힘)\n",
        "  #n1 · text · Note #1 = \"\" (같은 이름이라 번호만 붙임)",
        "  #n2 · text · Note #2 = \"\" (같은 이름이라 번호만 붙임)",
        "  #nick · text · Nickname = \"\"\n",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
    let json = fields_json(&read);
    assert_eq!(json["fields"][0]["open"], json!(true));
    assert!(
        json["fields"][4].get("open").is_none() && json["fields"][4].get("ordinal").is_none(),
        "a field with neither says neither in the JSON: {json}"
    );
}

/// A new kind of field is a row of the controls table: a button that opens a list is one.
#[test]
fn a_button_that_opens_a_list_is_one_of_the_controls_a_read_looks_at() {
    assert!(
        BROWSER_FORM_CONTROLS.contains(&"[aria-haspopup=listbox]"),
        "{BROWSER_FORM_CONTROLS:?}"
    );
}

/// A fill says the text that stands new beside a field it wrote — as the page
/// wrote it, never as an error — the field the page marked invalid and said
/// nothing about for what it is, and the notices the page's live regions made.
#[test]
fn a_fill_says_the_text_that_appeared_beside_a_field_and_a_silent_invalid_field_for_what_it_is() {
    let pass: FillPass = serde_json::from_value(json!({
        "results": [
            { "handle": "#mail", "status": "set", "label": "Email", "now": "kim",
              "fresh": ["Enter a valid email", "Try again"] },
            { "handle": "#age", "status": "set", "label": "Age", "now": "12",
              "error": "aria-invalid", "silent": true },
            { "handle": "#code", "status": "set", "label": "Code", "now": "1" },
        ],
        "left": [
            { "handle": "#age", "kind": "number", "label": "Age", "value": "12",
              "error": "aria-invalid", "silent": true },
            { "handle": "#code2", "kind": "text", "label": "Code 2", "value": "", "required": true,
              "fresh": ["Needed"] },
        ],
        "alerts": ["Code sent again"],
        "moving": false,
    }))
    .expect("a pass with text that appeared");
    let bundle = vec![
        entry("#mail", text("kim")),
        entry("#age", text("12")),
        entry("#code", text("1")),
    ];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    let lines = fill_lines(&ledger.report());
    for expected in [
        "  ✓ #mail Email = \"kim\" — 새로 뜬 글: 「Enter a valid email」 「Try again」",
        "  ✓ #age Age = \"12\" ⚠ aria-invalid — 페이지가 이유를 말하지 않음",
        "  ✓ #code Code = \"1\"\n",
        "  #age · number · Age — aria-invalid — 페이지가 이유를 말하지 않음",
        "  #code2 · text · Code 2 — 필수, 비어 있음 · 새로 뜬 글: 「Needed」",
        "새로 뜬 알림: 「Code sent again」",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
    assert_eq!(
        lines.matches("새로 뜬 글").count(),
        2,
        "a field with nothing new says nothing:\n{lines}"
    );
}

/// Text that stands new in the form beside no single field — the reason under a
/// value made of several fields, a sentence under a group of buttons — is said
/// apart from the fields, after a fill and after a press, and only while the
/// form is the one the agent read: a form that moved on has all its text new.
#[test]
fn a_fill_and_a_press_say_text_beside_no_single_field_only_while_the_form_stayed() {
    let pass: FillPass = serde_json::from_value(json!({
        "results": [{ "handle": "#id", "status": "set", "label": "Email name", "now": "kim" }],
        "outside": ["Check the email address", "Choose a diet"],
        "moving": false,
    }))
    .expect("a pass with text beside no field");
    let bundle = vec![entry("#id", text("kim"))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    let report = ledger.report();
    let said = fill_lines(&report.clone().against(Some("9:read"), &[]));
    let same = FillReport {
        fingerprint: "9:read".into(),
        ..report.clone()
    }
    .against(Some("9:read"), &[]);
    let lines = fill_lines(&same);
    assert!(
        lines.contains("새로 뜬 글(칸 밖): 「Check the email address」 「Choose a diet」"),
        "missing the text beside no field in\n{lines}"
    );
    assert!(
        !said.contains("칸 밖"),
        "a form that is not the one read says none of it:\n{said}"
    );
    let read: PressRead = serde_json::from_value(json!({
        "fingerprint": "9:read",
        "actions": [{ "handle": "#go", "label": "Go" }],
        "outside": ["Check the email address"],
    }))
    .expect("what a page said after a press");
    let known = [button("#go", "Go", false)];
    let stayed = press_lines(&PressAfter::against(
        read.clone(),
        Some("9:read"),
        &known,
        false,
    ));
    assert!(
        stayed.contains("새로 뜬 글(칸 밖): 「Check the email address」"),
        "missing the text beside no field in\n{stayed}"
    );
    let moved = press_lines(&PressAfter::against(read, Some("12:other"), &known, false));
    assert!(
        !moved.contains("칸 밖"),
        "a form that moved on says only so:\n{moved}"
    );
}

/// A press is set against the form the agent read before it, as a fill is:
/// the same form or another, and the button the page took away — none when
/// the form changed, which has lost every button of the old one.
#[test]
fn a_press_is_set_against_the_form_that_was_read_before_it() {
    let read = PressRead {
        fingerprint: "9:read".into(),
        actions: vec![button("#next", "Next", false)],
        ..PressRead::default()
    };
    let known = [
        button("#next", "Next", false),
        button("#skip", "Skip", false),
    ];
    let same = PressAfter::against(read.clone(), Some("9:read"), &known, false);
    assert_eq!(same.changed, Some(false));
    assert_eq!(same.hidden, vec![button("#skip", "Skip", false)]);
    assert!(same.settled && !same.moving, "{same:?}");
    let other = PressAfter::against(
        PressRead {
            fingerprint: "12:other".into(),
            ..read.clone()
        },
        Some("9:read"),
        &known,
        false,
    );
    assert_eq!(other.changed, Some(true));
    assert!(other.hidden.is_empty(), "{other:?}");
    assert_eq!(
        PressAfter::against(read.clone(), None, &[], false).changed,
        None
    );
    assert!(PressAfter::against(read, Some("9:read"), &known, true).moving);
}

/// What a press in a known form says: whether the form stayed, its buttons on
/// and off, what stands left and what appeared beside it — the reason a "next"
/// that did not move on gives — the notices of the page, and that the page was
/// read after it stood still. A form that moved on says only that, and its buttons.
#[test]
fn a_press_says_the_form_its_buttons_what_is_new_beside_a_field_and_what_is_left() {
    let read: PressRead = serde_json::from_value(json!({
        "fingerprint": "9:read",
        "actions": [
            { "handle": "#next", "label": "Next" },
            { "handle": "#back", "label": "Back", "disabled": true },
        ],
        "noted": [
            { "handle": "#mail", "kind": "email", "label": "Email", "value": "", "required": true,
              "fresh": ["Enter a valid email"] },
            { "handle": "#age", "kind": "number", "label": "Age", "value": "12",
              "error": "aria-invalid", "silent": true },
        ],
        "alerts": ["Saved as a draft"],
    }))
    .expect("what a page said after a press");
    let known = [
        button("#next", "Next", false),
        button("#back", "Back", true),
    ];
    let after = PressAfter::against(read.clone(), Some("9:read"), &known, false);
    let lines = press_lines(&after);
    for expected in [
        "양식 그대로 — 버튼: #next 「Next」 켜짐, #back 「Back」 꺼짐",
        "남은 칸:",
        "  #mail · email · Email — 필수, 비어 있음 · 새로 뜬 글: 「Enter a valid email」",
        "  #age · number · Age — aria-invalid — 페이지가 이유를 말하지 않음",
        "새로 뜬 알림: 「Saved as a draft」",
        "※ 누른 뒤 50 ms 동안 가만히 있는 것을 보고 읽었습니다. 그 뒤에 뜨는 오류는 못 봅니다 — 제출 전에 fields로 다시 읽으세요",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
    let moved = PressAfter::against(
        PressRead {
            fingerprint: "12:other".into(),
            ..read.clone()
        },
        Some("9:read"),
        &known,
        false,
    );
    let said = press_lines(&moved);
    assert!(
        said.starts_with(
            "양식 바뀜(fields로 다시 읽기) — 버튼: #next 「Next」 켜짐, #back 「Back」 꺼짐"
        ),
        "{said}"
    );
    assert!(
        !said.contains("남은 칸") && !said.contains("새로 뜬") && !said.contains('※'),
        "a form that moved on says only so:\n{said}"
    );
    let still = press_lines(&PressAfter::against(read, Some("9:read"), &known, true));
    assert!(
        still.contains("아직 바뀌는 중(fields로 다시 읽기)") && !still.contains('※'),
        "a page still changing says so, and no word of having stood still:\n{still}"
    );
}

/// A group of buttons that say whether they are pressed is one field of kind `chips` (t-41656): its
/// value is the list of the pressed buttons' words, its options the buttons' words, and a group the
/// page gives no title says it has none — a name left empty would read as a field that has a name.
#[test]
fn a_group_of_chips_is_one_field_that_holds_a_list_and_a_group_with_no_title_says_it_has_none() {
    let read: Result<FormRead, _> = serde_json::from_value(json!({
        "fields": [
            { "handle": "#colours", "kind": "chips", "label": "Pick colours",
              "value": ["Green"], "options": ["Red", "Green", "Blue"], "moreOptions": 2 },
            { "handle": "#size", "kind": "chips", "label": "Size", "value": [], "required": true,
              "options": ["Small", "Medium"] },
            { "handle": "#row", "kind": "chips", "label": "", "value": [],
              "options": ["Yes", "No"] },
        ],
    }));
    assert!(
        read.is_ok(),
        "a group's value is a list of words: {:?}",
        read.as_ref().err()
    );
    let read = read.unwrap_or_default();
    let lines = fields_lines(&read);
    for expected in [
        "양식 칸 3개 (필수 1, 비어 있는 필수 1)",
        "  #colours · chips · Pick colours = [\"Green\"] ▸ Red | Green | Blue (+2)",
        "  #size · chips · Size * = [] ▸ Small | Medium",
        "  #row · chips · (제목 없음) = [] ▸ Yes | No",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
    let json = fields_json(&read);
    assert_eq!(json["fields"][0]["value"], json!(["Green"]));
    assert_eq!(json["fields"][1]["value"], json!([]));
}

/// A fill of a group says the list it holds now, and the list it holds instead when it did not take.
#[test]
fn a_fill_of_a_group_of_chips_says_the_list_it_holds_now_and_the_one_it_holds_instead() {
    let pass: Result<FillPass, _> = serde_json::from_value(json!({
        "results": [
            { "handle": "#colours", "status": "set", "label": "Colours", "now": ["Red", "Blue"] },
            { "handle": "#size", "status": "mismatch", "label": "Size", "now": ["Medium"] },
            { "handle": "#fit", "status": "no_option", "label": "Fit", "options": ["S", "M"] },
        ],
    }));
    assert!(
        pass.is_ok(),
        "what a group holds now is a list of words: {:?}",
        pass.as_ref().err()
    );
    let bundle = vec![
        entry("#colours", text("Red, Blue")),
        entry("#size", text("Small")),
        entry("#fit", text("XL")),
    ];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass.unwrap_or_default());
    let lines = fill_lines(&ledger.report());
    for expected in [
        "  ✓ #colours Colours = [\"Red\", \"Blue\"]",
        "  ✗ #size Size: 다시 읽으니 다른 값 ([\"Medium\"])",
        "  ✗ #fit Fit: 그 값의 선택지가 없음 ▸ S | M",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
}

/// A page that marks what is required only with a star in the words of the field declares nothing
/// (no `required`, no `aria-required`): a read that says "필수 0" there is false reassurance, so
/// it says the page did not declare it and how many fields carry a star; a page that declares some
/// and stars others says the stars apart; a page with neither says "필수 0" as it did.
#[test]
fn a_page_that_marks_required_only_with_a_star_is_not_told_it_has_no_required_field() {
    let read = |fields: serde_json::Value| -> FormRead {
        serde_json::from_value(json!({ "fields": fields })).expect("a read")
    };
    let starred = read(json!([
        { "handle": "#name", "kind": "text", "label": "Name *", "value": "" },
        { "handle": "#mail", "kind": "email", "label": "Email *", "value": "kim@example.com" },
        { "handle": "#pw", "kind": "password", "label": "Password *", "value": "", "masked": true },
        { "handle": "#memo", "kind": "textarea", "label": "Memo", "value": "" },
        { "handle": "#rate", "kind": "text", "label": "Rate x*y", "value": "" },
    ]));
    let lines = fields_lines(&starred);
    assert!(
        lines.contains(
            "양식 칸 5개 (필수 표시 없음 — 이름에 *가 있는 칸 3, 그중 비어 있는 1, 값을 읽지 않는 1)"
        ),
        "a star that stands at an end of the words or alone is the page's mark, one inside a word is not:\n{lines}"
    );
    assert!(!lines.contains("필수 0"), "no false reassurance:\n{lines}");
    let mixed = read(json!([
        { "handle": "#a", "kind": "text", "label": "A *", "value": "", "required": true },
        { "handle": "#b", "kind": "text", "label": "* B", "value": "" },
        { "handle": "#c", "kind": "text", "label": "C", "value": "" },
    ]));
    let said = fields_lines(&mixed);
    assert!(
        said.contains("양식 칸 3개 (필수 1, 비어 있는 필수 1, 이름에만 *가 있는 칸 1)"),
        "{said}"
    );
    let plain = read(json!([{ "handle": "#c", "kind": "text", "label": "C", "value": "" }]));
    assert!(
        fields_lines(&plain).contains("양식 칸 1개 (필수 0, 비어 있는 필수 0)\n"),
        "a page with no star and no required field says what it said"
    );
}

/// A button the page declares a submit (`type=submit`) is marked in the buttons of a read, of a fill's
/// end and of a press's, so a driver or an agent picks the button that moves a form on by what the
/// page says and not by a list of words; a button that declares none is not marked.
#[test]
fn an_explicit_submit_button_is_marked_in_the_buttons_of_a_read_a_fill_and_a_press() {
    let actions = json!([
        { "handle": "#back", "label": "Back" },
        { "handle": "#go", "label": "Book", "disabled": true, "submit": true },
        { "handle": "#send", "label": "Send", "submit": true },
    ]);
    let read: FormRead = serde_json::from_value(json!({
        "fields": [{ "handle": "#a", "kind": "text", "label": "A", "value": "" }],
        "actions": actions,
    }))
    .expect("a read with buttons");
    let lines = fields_lines(&read);
    assert!(
        lines.contains(
            "버튼: #back 「Back」 · #go 「Book」 (꺼짐) (제출 단추) · #send 「Send」 (제출 단추)"
        ),
        "{lines}"
    );
    let json = fields_json(&read);
    assert_eq!(json["actions"][1]["submit"], json!(true));
    assert!(
        json["actions"][0].get("submit").is_none(),
        "a button that declares none says none in the JSON: {json}"
    );
    let pass: FillPass = serde_json::from_value(json!({
        "results": [{ "handle": "#a", "status": "set", "label": "A", "now": "x" }],
        "fingerprint": "5:aaa",
        "actions": actions,
    }))
    .expect("a pass with buttons");
    let bundle = vec![entry("#a", text("x"))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    let filled = fill_lines(&ledger.report());
    assert!(
        filled.contains("버튼: #back 「Back」 켜짐, #go 「Book」 꺼짐 (제출 단추), #send 「Send」 켜짐 (제출 단추)"),
        "{filled}"
    );
    let after = PressAfter::against(
        PressRead {
            fingerprint: "5:aaa".into(),
            actions: read.actions.clone(),
            ..PressRead::default()
        },
        Some("5:aaa"),
        &[],
        false,
    );
    let pressed = press_lines(&after);
    assert!(
        pressed.contains("버튼: #back 「Back」 켜짐, #go 「Book」 꺼짐 (제출 단추), #send 「Send」 켜짐 (제출 단추)"),
        "{pressed}"
    );
}

/// A field the page gave no words says it has none: a name left empty would read as a field that has a
/// name, and the words the page gave only as a placeholder stay an example beside it. A group of chips
/// with no title keeps its own words for the same thing.
#[test]
fn a_field_the_page_gave_no_name_says_it_has_none_and_keeps_its_placeholder_as_an_example() {
    let read: FormRead = serde_json::from_value(json!({
        "fields": [
            { "handle": "#unit", "kind": "text", "label": "", "value": "", "placeholder": "Floor or flat" },
            { "handle": "#bare", "kind": "text", "label": "", "value": "" },
            { "handle": "#row", "kind": "chips", "label": "", "value": [], "options": ["Yes", "No"] },
            { "handle": "#street", "kind": "text", "label": "Street", "value": "", "placeholder": "Lane" },
        ],
    }))
    .expect("a read of fields with no name");
    let lines = fields_lines(&read);
    for expected in [
        "  #unit · text · (이름 없음) = \"\" 예: Floor or flat",
        "  #bare · text · (이름 없음) = \"\"\n",
        "  #row · chips · (제목 없음) = [] ▸ Yes | No",
        "  #street · text · Street = \"\" 예: Lane",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
    let pass: FillPass = serde_json::from_value(json!({
        "results": [
            { "handle": "#bare", "status": "set", "kind": "text", "label": "", "now": "x" },
            { "handle": "#row", "status": "set", "kind": "chips", "label": "", "now": ["Yes"] },
        ],
    }))
    .expect("a pass over fields with no name");
    let bundle = vec![entry("#bare", text("x")), entry("#row", text("Yes"))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    let filled = fill_lines(&ledger.report());
    for expected in [
        "  ✓ #bare (이름 없음) = \"x\"",
        "  ✓ #row (제목 없음) = [\"Yes\"]",
    ] {
        assert!(
            filled.contains(expected),
            "missing {expected:?} in\n{filled}"
        );
    }
}

/// A fill that reads back a text unlike the one it was given, where the page's own state does not say
/// whether it is the same value (a list button that shows its choice shortened, a date drawn in another
/// shape), says it cannot see: neither "다른 값" nor "들어감". It is not tried again — the page will draw
/// it the same way — and it is not counted as taken.
#[test]
fn a_fill_that_cannot_see_whether_the_text_read_back_is_the_same_value_says_so_and_does_not_try_again()
 {
    let pass: FillPass = serde_json::from_value(json!({
        "results": [
            { "handle": "#when", "status": "unseen", "kind": "text", "label": "Date", "now": "02/17" },
            { "handle": "#rate", "status": "unseen", "kind": "combobox", "label": "Rating", "now": "5★",
              "options": ["Five stars", "Four stars"] },
            { "handle": "#size", "status": "mismatch", "kind": "text", "label": "Size", "now": "M" },
            { "handle": "#name", "status": "set", "kind": "text", "label": "Name", "now": "Kim" },
        ],
    }))
    .expect("a pass with a text it cannot tell");
    let bundle = vec![
        entry("#when", text("2027-02-17")),
        entry("#rate", text("Five stars")),
        entry("#size", text("S")),
        entry("#name", text("Kim")),
    ];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    assert_eq!(
        ledger
            .next()
            .iter()
            .map(|one| one.handle.as_str())
            .collect::<Vec<_>>(),
        ["#size"],
        "only the text that reads back as another value is tried again"
    );
    let report = ledger.report();
    assert_eq!(
        (report.took(), report.all_took()),
        (1, false),
        "a text the door cannot tell is not counted as taken"
    );
    let lines = fill_lines(&report);
    for expected in [
        "채움 1/4칸 (1회)",
        "  ? #when Date: 다시 읽은 글과 글자는 다르나 같은지는 볼 수 없음 (\"02/17\")",
        "  ? #rate Rating: 다시 읽은 글과 글자는 다르나 같은지는 볼 수 없음 (\"5★\")",
        "  ✗ #size Size: 다시 읽으니 다른 값 (\"M\")",
        "  ✓ #name Name = \"Kim\"",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
    assert!(
        !lines.contains("? #when Date: 다시 읽으니 다른 값") && !lines.contains("들어감"),
        "neither a difference nor a sameness is claimed:\n{lines}"
    );
}

/// A value split across fields says, beside each part that has one after it, the short symbol the page
/// draws between the parts (an @, a dash, a colon, a slash), so whoever writes the value does not guess
/// where to cut it. A part with nothing after it says nothing, and the JSON carries the symbol as `joint`.
#[test]
fn a_part_of_a_split_value_says_the_symbol_the_page_draws_after_it() {
    let read: FormRead = serde_json::from_value(json!({
        "fields": [
            { "handle": "#sa", "kind": "text", "label": "Score (1/3)", "value": "", "joint": "/" },
            { "handle": "#sb", "kind": "text", "label": "Score (2/3)", "value": "", "joint": "~" },
            { "handle": "#sc", "kind": "text", "label": "Score (3/3)", "value": "" },
        ],
    }))
    .expect("a read of split parts");
    let lines = fields_lines(&read);
    for expected in [
        "  #sa · text · Score (1/3) = \"\" (다음 칸 앞에 「/」)",
        "  #sb · text · Score (2/3) = \"\" (다음 칸 앞에 「~」)",
        "  #sc · text · Score (3/3) = \"\"\n",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
    let json = fields_json(&read);
    assert_eq!(json["fields"][0]["joint"], json!("/"));
    assert_eq!(json["fields"][1]["joint"], json!("~"));
    assert!(
        json["fields"][2].get("joint").is_none(),
        "a part with nothing after it says none in the JSON: {json}"
    );
}

/// A button that stands beside one field says whose it is — in the buttons of a read, of a fill's end and
/// of a press's — so a reader ties "Apply" to the code it applies without a list of words; a button that
/// stands beside no one field says nothing, in the words and in the JSON.
#[test]
fn a_button_beside_a_field_is_said_to_be_that_fields_in_the_buttons_of_a_read_a_fill_and_a_press() {
    let actions = json!([
        { "handle": "#apply", "label": "Apply", "beside": "#code" },
        { "handle": "#less", "label": "Less", "disabled": true, "beside": "#qty" },
        { "handle": "#go", "label": "Go", "submit": true },
    ]);
    let read: FormRead = serde_json::from_value(json!({
        "fields": [{ "handle": "#code", "kind": "text", "label": "Code", "value": "" }],
        "actions": actions,
    }))
    .expect("a read with buttons beside fields");
    let lines = fields_lines(&read);
    assert!(
        lines.contains(
            "버튼: #apply 「Apply」 (#code 칸 곁) · #less 「Less」 (꺼짐) (#qty 칸 곁) · #go 「Go」 (제출 단추)"
        ),
        "{lines}"
    );
    let json = fields_json(&read);
    assert_eq!(json["actions"][0]["beside"], json!("#code"));
    assert!(
        json["actions"][2].get("beside").is_none(),
        "a button beside no one field says none in the JSON: {json}"
    );
    let pass: FillPass = serde_json::from_value(json!({
        "results": [{ "handle": "#code", "status": "set", "label": "Code", "now": "x" }],
        "fingerprint": "5:aaa",
        "actions": actions,
    }))
    .expect("a pass with buttons");
    let bundle = vec![entry("#code", text("x"))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    let filled = fill_lines(&ledger.report());
    assert!(
        filled.contains(
            "버튼: #apply 「Apply」 켜짐 (#code 칸 곁), #less 「Less」 꺼짐 (#qty 칸 곁), #go 「Go」 켜짐 (제출 단추)"
        ),
        "{filled}"
    );
    let after = PressAfter::against(
        PressRead {
            fingerprint: "5:aaa".into(),
            actions: read.actions.clone(),
            ..PressRead::default()
        },
        Some("5:aaa"),
        &[],
        false,
    );
    let pressed = press_lines(&after);
    assert!(
        pressed.contains(
            "버튼: #apply 「Apply」 켜짐 (#code 칸 곁), #less 「Less」 꺼짐 (#qty 칸 곁), #go 「Go」 켜짐 (제출 단추)"
        ),
        "{pressed}"
    );
}

/// A secret field is never written by `fill`; the answer sends the agent to `type` — but `type` names a field by a selector of the page's own document,
/// and the handle of a field inside a frame is no such selector. Said for such a field, the way to `type` is false: the answer says the field is out of
/// reach of both, and a secret in the page itself still names `type`.
#[test]
fn a_secret_field_inside_a_frame_is_said_to_be_out_of_reach_of_both_fill_and_type() {
    let pass: FillPass = serde_json::from_value(json!({
        "results": [
            { "handle": "#card >> #cvc", "status": "secret", "kind": "password", "label": "CVC" },
            { "handle": "#pw", "status": "secret", "kind": "password", "label": "Password" },
        ],
    }))
    .expect("a pass with two secrets");
    let bundle = vec![entry("#card >> #cvc", text("742")), entry("#pw", text("x"))];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    let lines = fill_lines(&ledger.report());
    assert!(
        lines.contains("  ✗ #card >> #cvc CVC: 비밀 칸이 틀 안에 있음 — fill은 쓰지 않고 type은 틀 안의 칸을 손잡이로 가리키지 못함"),
        "a secret in a frame says both roads are shut:\n{lines}"
    );
    assert!(
        lines.contains(
            "  ✗ #pw Password: 비밀 칸은 fill이 쓰지 않음 — type <label> <손잡이> --value-stdin"
        ),
        "a secret in the page itself still names type:\n{lines}"
    );
}

/// A field the page keeps from being typed in and fills from a window a button opens (a pick from a list the page searches) says which button opens
/// it, so the agent presses that one and does not guess; a read-only field with no such button says no more than that it cannot be written.
#[test]
fn a_read_only_field_says_the_button_that_opens_it() {
    let read: FormRead = serde_json::from_value(json!({
        "fields": [
            { "handle": "#locker", "kind": "text", "label": "Locker", "value": "", "readOnly": true,
              "opens": { "handle": "#pick", "label": "Pick a locker" } },
            { "handle": "#spot", "kind": "text", "label": "Parcel point", "value": "", "readOnly": true,
              "opens": { "handle": "#pick", "label": "Pick a locker" } },
            { "handle": "#ref", "kind": "text", "label": "Reference", "value": "X1", "readOnly": true },
        ],
    }))
    .expect("a read with an opener");
    let lines = fields_lines(&read);
    for expected in [
        "  #locker · text · Locker = \"\" (직접 못 씀 — 열 단추: #pick 「Pick a locker」)",
        "  #spot · text · Parcel point = \"\" (직접 못 씀 — 열 단추: #pick 「Pick a locker」)",
    ] {
        assert!(lines.contains(expected), "missing {expected:?} in\n{lines}");
    }
    assert!(
        lines
            .lines()
            .any(|line| line == "  #ref · text · Reference = \"X1\" (직접 못 씀)"),
        "a read-only field with no opener says no more:\n{lines}"
    );
}

/// The answer of a fill that the page keeps from writing names the same button, in the words that say to open the field.
#[test]
fn a_fill_that_the_page_keeps_from_writing_names_the_button_that_opens_the_field() {
    let pass: FillPass = serde_json::from_value(json!({
        "results": [
            { "handle": "#spot", "status": "read_only", "kind": "text", "label": "Parcel point",
              "opens": { "handle": "#pick", "label": "Pick a locker" } },
            { "handle": "#ref", "status": "read_only", "kind": "text", "label": "Reference" },
        ],
    }))
    .expect("a pass with two read-only fields");
    let bundle = vec![
        entry("#spot", text("North Gate 9")),
        entry("#ref", text("X2")),
    ];
    let mut ledger = FillLedger::new(bundle.clone());
    ledger.record(&bundle, pass);
    let lines = fill_lines(&ledger.report());
    assert!(
        lines.contains("  ✗ #spot Parcel point: 페이지가 직접 쓰지 못하게 막음 — click으로 열어 고를 것 (열 단추: #pick 「Pick a locker」)"),
        "a fill that cannot write names the button that opens the field:\n{lines}"
    );
    assert!(
        lines.lines().any(|line| line
            == "  ✗ #ref Reference: 페이지가 직접 쓰지 못하게 막음 — click으로 열어 고를 것"),
        "and says no more where the read named none:\n{lines}"
    );
}

/// A button the page says opens a dialog (`aria-haspopup="dialog"`) says the name of the dialog in the read; a dialog with no name says only that it opens one, and any other button says nothing more.
#[test]
fn a_button_that_opens_a_dialog_says_the_name_of_the_dialog() {
    let read: FormRead = serde_json::from_value(json!({
        "fields": [],
        "actions": [
            { "handle": "#from", "label": "Start date", "dialog": "Stay dates" },
            { "handle": "#help", "label": "Help", "dialog": "" },
            { "handle": "#next", "label": "Next" },
        ],
    }))
    .expect("a read with buttons that open dialogs");
    let lines = fields_lines(&read);
    assert!(
        lines.contains("#from 「Start date」 (대화상자 「Stay dates」을 엶)"),
        "a named dialog is said by its name:\n{lines}"
    );
    assert!(
        lines.contains("#help 「Help」 (대화상자를 엶)"),
        "one with no name is said to open one:\n{lines}"
    );
    assert!(
        lines.contains("#next 「Next」") && !lines.contains("#next 「Next」 (대화상자"),
        "and any other button says nothing of one:\n{lines}"
    );
}
