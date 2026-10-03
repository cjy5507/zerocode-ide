use super::*;
use std::io::Write as _;

/// A model this table prices flat: a dollar per million input tokens.
const MODEL: &str = "claude-haiku-4-5";

/// One assistant record of the call `message`, at `output` output tokens and a
/// million input tokens — a dollar and `output` times five millionths.
fn record(message: &str, output: i64) -> String {
    serde_json::json!({
        "type": "assistant",
        "sessionId": "session-1",
        "timestamp": "2026-10-03T00:00:00.000Z",
        "uuid": format!("uuid-{message}-{output}"),
        "requestId": "request-1",
        "message": {
            "id": message,
            "model": MODEL,
            "usage": {
                "input_tokens": 1_000_000,
                "output_tokens": output,
                "cache_read_input_tokens": 0,
                "cache_creation_input_tokens": 0,
            },
        },
    })
    .to_string()
}

fn append(path: &Path, text: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("the transcript opens");
    file.write_all(text.as_bytes())
        .expect("the transcript grows");
}

fn dollars(costs: &[CallCost]) -> Vec<f64> {
    costs.iter().filter_map(|cost| cost.usd()).collect()
}

fn transcript() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a transcript dir");
    let path = dir.path().join("session.jsonl");
    (dir, path)
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("a utf-8 path")
}

#[test]
fn a_look_finishes_every_call_but_the_one_still_being_written_and_the_next_look_finishes_it() {
    let (_dir, path) = transcript();
    append(
        &path,
        &format!("{}\n{}\n", record("m1", 0), record("m2", 200_000)),
    );
    let mut meter = Meter::new("claude");
    let first = meter.poll(Some(path_text(&path)), 10_000);
    assert_eq!(dollars(&first), vec![1.0], "m1 is finished by m2 beginning");
    // Nothing grew: the stream of m2 is over, and it is finished.
    let quiet = meter.poll(Some(path_text(&path)), 10_000 + POLL_MS);
    assert_eq!(
        dollars(&quiet),
        vec![2.0],
        "m2: a dollar and a dollar of output"
    );
    assert!(
        meter
            .poll(Some(path_text(&path)), 10_000 + 2 * POLL_MS)
            .is_empty(),
        "and not counted twice"
    );
}

#[test]
fn only_what_the_file_grew_by_is_read_on_the_next_look() {
    let (_dir, path) = transcript();
    append(&path, &format!("{}\n", record("m1", 0)));
    let mut meter = Meter::new("claude");
    assert!(meter.poll(Some(path_text(&path)), 0).is_empty());
    append(&path, &format!("{}\n", record("m2", 0)));
    assert_eq!(
        dollars(&meter.poll(Some(path_text(&path)), POLL_MS)),
        vec![1.0],
        "m1, finished by the growth"
    );
    append(&path, &format!("{}\n", record("m3", 0)));
    assert_eq!(
        dollars(&meter.poll(Some(path_text(&path)), 2 * POLL_MS)),
        vec![1.0],
        "m2, once; m1 is not read again"
    );
}

#[test]
fn a_look_sooner_than_the_poll_touches_nothing() {
    let (_dir, path) = transcript();
    let mut meter = Meter::new("claude");
    assert!(meter.poll(Some(path_text(&path)), 0).is_empty());
    append(
        &path,
        &format!("{}\n{}\n", record("m1", 0), record("m2", 0)),
    );
    assert!(
        meter.poll(Some(path_text(&path)), POLL_MS - 1).is_empty(),
        "too soon: the growth waits for its look"
    );
    assert_eq!(
        dollars(&meter.poll(Some(path_text(&path)), POLL_MS)),
        vec![1.0]
    );
}

#[test]
fn a_line_still_being_written_waits_for_its_end() {
    let (_dir, path) = transcript();
    let whole = record("m1", 0);
    let (head, tail) = whole.split_at(whole.len() / 2);
    let mut meter = Meter::new("claude");
    append(&path, &format!("{}\n{head}", record("m0", 0)));
    assert!(meter.poll(Some(path_text(&path)), 0).is_empty());
    append(&path, &format!("{tail}\n{}\n", record("m2", 0)));
    // m0 was finished when m1 completed, and m1 when m2 began.
    assert_eq!(
        dollars(&meter.poll(Some(path_text(&path)), POLL_MS)),
        vec![1.0, 1.0],
        "the half line was held until it was whole"
    );
}

#[test]
fn a_worker_first_seen_mid_run_is_read_from_the_end_of_its_file() {
    let (_dir, path) = transcript();
    // A past the window never looked at: about three mebibytes of calls, then
    // two that are the window's to read.
    let one = record("old0", 0).len() + 1;
    let copies = usize::try_from(3 * ATTACH_TAIL_BYTES).expect("fits") / one + 1;
    let mut written = String::new();
    for index in 0..copies {
        written.push_str(&record(&format!("old{index}"), 0));
        written.push('\n');
    }
    append(&path, &written);
    append(
        &path,
        &format!("{}\n{}\n", record("new1", 0), record("new2", 0)),
    );
    let mut meter = Meter::new("claude");
    let first = meter.poll(Some(path_text(&path)), 0);
    let tail_calls = usize::try_from(ATTACH_TAIL_BYTES).expect("fits") / one;
    assert!(
        first.len() <= tail_calls + 3,
        "{} calls from one look at a file of {copies}: only its tail",
        first.len()
    );
    assert!(
        first.len() + 100 >= tail_calls,
        "{} calls: the tail was read",
        first.len()
    );
}

#[test]
fn a_replaced_file_is_read_again_from_its_end() {
    let (_dir, path) = transcript();
    append(
        &path,
        &format!("{}\n{}\n", record("m1", 0), record("m2", 0)),
    );
    let mut meter = Meter::new("claude");
    meter.poll(Some(path_text(&path)), 0);
    meter.poll(Some(path_text(&path)), POLL_MS);
    std::fs::write(&path, format!("{}\n", record("n1", 0))).expect("a shorter file");
    assert!(
        meter.poll(Some(path_text(&path)), 2 * POLL_MS).is_empty(),
        "a lone call is open until it is finished"
    );
    assert_eq!(
        dollars(&meter.poll(Some(path_text(&path)), 3 * POLL_MS)),
        vec![1.0]
    );
}

#[test]
fn what_cannot_be_read_says_why_and_costs_nothing() {
    let (_dir, path) = transcript();
    let mut claude = Meter::new("claude");
    assert!(claude.poll(None, 0).is_empty());
    assert_eq!(claude.note(false), CostNote::NoTranscript);
    assert!(
        claude.poll(Some(path_text(&path)), 0).is_empty(),
        "a path with no file under it"
    );
    assert_eq!(claude.note(true), CostNote::Unreadable);
    append(&path, &format!("{}\n", record("m1", 0)));
    claude.poll(Some(path_text(&path)), POLL_MS);
    assert_eq!(claude.note(true), CostNote::Read);

    for agent in ["codex", "zo", "kimi", "grok", "antigravity"] {
        let mut meter = Meter::new(agent);
        append(&path, &format!("{}\n", record("m2", 0)));
        assert!(meter.poll(Some(path_text(&path)), 0).is_empty());
        assert_eq!(meter.note(true), CostNote::NoReader, "{agent}");
    }
}
