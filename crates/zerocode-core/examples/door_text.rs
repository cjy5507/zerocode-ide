//! The form bench's door to the words the window answers a web form with
//! (t-41387): what `zerocode-browser fields` and `fill` say is made by the
//! core (`browser_form`, `agent_browser`, `untrusted`), so the bench hands the
//! core what the page answered and prints what comes back — it keeps no copy
//! of a sentence an agent reads. One JSON object on stdin, one on stdout:
//! `"ok": false` is an answer the window gives on stderr (a refusal), and
//! `"words"` is what it says either way.
//!
//! ```text
//! echo '{"op":"parse","argv":["fill","browser-1","--value","{\"#a\":\"x\"}"]}' | door_text
//! echo '{"op":"fields","label":"browser-1","json":false,"read":{…}}'          | door_text
//! echo '{"op":"fill","label":"browser-1","text":"{…}","passes":[{"asked":["#a"],"pass":{…}}],"known":"…","buttons":[…]}' | door_text
//! echo '{"op":"press","label":"browser-1","read":{…},"known":"…","buttons":[…],"moving":false}' | door_text
//! echo '{"op":"fence","label":"browser-1","words":"…"}'                         | door_text
//! echo '{"op":"usage","door":"browser"}'                                        | door_text
//! ```

use std::io::Read as _;

use serde_json::{Value, json};
use zerocode_core::agent_browser::{self, ClickTarget, ScrollTarget};
use zerocode_core::browser_form::{
    self, FillLedger, FillPass, FormAction, FormRead, PressAfter, PressRead,
};
use zerocode_core::untrusted;

fn main() {
    let mut input = String::new();
    let asked: Value = std::io::stdin()
        .read_to_string(&mut input)
        .ok()
        .and_then(|_| serde_json::from_str(&input).ok())
        .unwrap_or_default();
    let answer = match asked["op"].as_str().unwrap_or_default() {
        "parse" => parse(&asked),
        "fields" => fields(&asked),
        "fill" => fill(&asked),
        "press" => press(&asked),
        "fence" => fence(&asked),
        "usage" => usage(&asked),
        other => json!({ "ok": false, "words": format!("door_text: no op `{other}`\n") }),
    };
    println!("{answer}");
}

/// The pane label a request names, `browser-1` when it names none.
fn label_of(asked: &Value) -> &str {
    asked["label"].as_str().unwrap_or("browser-1")
}

/// What the window's own parsers make of an argv: the pane label and the
/// verb's arguments, or the sentence they refuse with.
fn parse(asked: &Value) -> Value {
    let argv: Vec<String> = asked["argv"]
        .as_array()
        .map(|words| {
            words
                .iter()
                .filter_map(|word| word.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let refused =
        |why: String| json!({ "ok": false, "words": format!("zerocode-browser: {why}\n") });
    match argv.first().map(String::as_str) {
        Some("fields") => match agent_browser::parse_fields(&argv) {
            Ok(command) => json!({ "ok": true, "label": command.label, "json": command.json }),
            Err(why) => refused(why),
        },
        Some("fill") => match agent_browser::parse_fill(&argv) {
            Ok(command) => {
                json!({ "ok": true, "label": command.label, "entries": command.entries })
            }
            Err(why) => refused(why),
        },
        Some("click") => match agent_browser::parse_click(&argv) {
            Ok(ClickTarget::Css(css)) => json!({ "ok": true, "css": css }),
            Ok(ClickTarget::Mark(mark)) => json!({ "ok": true, "mark": mark }),
            Ok(ClickTarget::MarkSettleLater(mark)) => {
                json!({ "ok": true, "mark": mark, "later": true })
            }
            Err(why) => refused(why),
        },
        Some("read") => match agent_browser::parse_read(&argv) {
            Ok(command) => json!({
                "ok": true,
                "label": command.label,
                "selector": command.selector,
                "full": command.full,
            }),
            Err(why) => refused(why),
        },
        Some("scroll") => match argv.get(2).map(|word| agent_browser::parse_scroll(word)) {
            Some(Ok(ScrollTarget::Top)) => json!({ "ok": true, "kind": "top" }),
            Some(Ok(ScrollTarget::Bottom)) => json!({ "ok": true, "kind": "bottom" }),
            Some(Ok(ScrollTarget::PageUp)) => {
                json!({ "ok": true, "kind": "page", "direction": "up" })
            }
            Some(Ok(ScrollTarget::PageDown)) => {
                json!({ "ok": true, "kind": "page", "direction": "down" })
            }
            Some(Ok(ScrollTarget::By(dx, dy))) => {
                json!({ "ok": true, "kind": "by", "dx": dx, "dy": dy })
            }
            Some(Ok(ScrollTarget::Selector(selector))) => {
                json!({ "ok": true, "kind": "selector", "selector": selector })
            }
            Some(Err(why)) => refused(why),
            None => json!({ "ok": false, "words": format!("{}\n", agent_browser::usage()) }),
        },
        _ => json!({ "ok": false, "words": format!("{}\n", agent_browser::usage()) }),
    }
}

/// A `fields` answer: the read's lines inside the fence, or its JSON.
fn fields(asked: &Value) -> Value {
    let read: FormRead = serde_json::from_value(asked["read"].clone()).unwrap_or_default();
    let words = if asked["json"].as_bool().unwrap_or(false) {
        format!("{}\n", browser_form::fields_json(&read))
    } else {
        untrusted::fence(
            label_of(asked),
            &browser_form::fields_lines(&read),
            usize::MAX,
        )
    };
    json!({ "ok": true, "words": words })
}

/// A `fill` answer: the passes the page made, kept by the ledger the window
/// keeps and set against the form the agent read before them (`known`, its
/// fingerprint, and the `buttons` that read had), said the way the window
/// says them — fenced on stdout when every field took, fenced on stderr when
/// one did not, the host's own words when the form was not the one read.
fn fill(asked: &Value) -> Value {
    let Ok(entries) = browser_form::fill_entries(asked["text"].as_str().unwrap_or_default()) else {
        return json!({ "ok": false, "words": "door_text: the fill text is no bundle\n" });
    };
    let mut ledger = FillLedger::new(entries.clone());
    for round in asked["passes"].as_array().into_iter().flatten() {
        let handles: Vec<&str> = round["asked"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let sent: Vec<_> = entries
            .iter()
            .filter(|entry| handles.contains(&entry.handle.as_str()))
            .cloned()
            .collect();
        let pass: FillPass = serde_json::from_value(round["pass"].clone()).unwrap_or_default();
        ledger.record(&sent, pass);
    }
    let mut report = ledger.report();
    if !report.stale {
        let buttons: Vec<FormAction> =
            serde_json::from_value(asked["buttons"].clone()).unwrap_or_default();
        report = report.against(asked["known"].as_str(), &buttons);
    }
    let words = browser_form::fill_lines(&report);
    if report.stale {
        return json!({ "ok": false, "words": format!("zerocode-browser: {words}") });
    }
    let fenced = untrusted::fence(label_of(asked), &words, usize::MAX);
    json!({ "ok": report.all_took(), "words": fenced })
}

/// The lines under a click in a form the agent read: what the page said once the
/// press was made and it had settled (`read`), set against the form the agent read
/// before it (`known`, its fingerprint, and the `buttons` it had) and said the
/// way the window says them — inside the fence. `changed` says whether the form
/// moved on, so the caller keeps what the agent now knows.
fn press(asked: &Value) -> Value {
    let read: PressRead = serde_json::from_value(asked["read"].clone()).unwrap_or_default();
    let buttons: Vec<FormAction> =
        serde_json::from_value(asked["buttons"].clone()).unwrap_or_default();
    let moving = asked["moving"].as_bool().unwrap_or(false);
    let after = PressAfter::against(read, asked["known"].as_str(), &buttons, moving);
    let words = browser_form::press_lines(&after);
    json!({
        "ok": true,
        "words": untrusted::fence(label_of(asked), &words, usize::MAX),
        "changed": after.changed,
    })
}

/// Words a page wrote, inside the one fence every agent road uses.
fn fence(asked: &Value) -> Value {
    let words = asked["words"].as_str().unwrap_or_default();
    json!({ "ok": true, "words": untrusted::fence(label_of(asked), words, usize::MAX) })
}

/// What a door's `--help` says: the core's own manual, which the shim a pane
/// carries prints locally.
fn usage(asked: &Value) -> Value {
    let manual = match asked["door"].as_str() {
        Some("computer") => zerocode_core::computer_use::usage(),
        _ => agent_browser::usage(),
    };
    json!({ "ok": true, "words": format!("{manual}\n") })
}
