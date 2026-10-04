use serde_json::json;

use super::*;

/// A phone form as OCR reads it in the mirrored window (screen points):
/// two yes/no questions, a choice row, a field label and the submit button —
/// listed out of reading order, as recognition answers them.
fn form() -> Vec<ReadLine> {
    lines(&json!({ "lines": [
        { "text": "No", "x": 1060.0, "y": 420.0, "width": 20.0, "height": 17.0 },
        { "text": "Have you visited a farm in the last 30 days?", "x": 760.0, "y": 380.0, "width": 380.0, "height": 17.0 },
        { "text": "Yes", "x": 860.0, "y": 420.0, "width": 26.0, "height": 17.0 },
        { "text": "Are you carrying food, plants or animal products?", "x": 760.0, "y": 300.0, "width": 400.0, "height": 17.0 },
        { "text": "Yes", "x": 860.0, "y": 340.0, "width": 26.0, "height": 17.0 },
        { "text": "No", "x": 1060.0, "y": 341.0, "width": 20.0, "height": 17.0 },
        { "text": "Nationality", "x": 760.0, "y": 200.0, "width": 90.0, "height": 20.0 },
        { "text": "Select", "x": 1080.0, "y": 201.0, "width": 50.0, "height": 20.0 },
        { "text": "Submit registration", "x": 880.0, "y": 900.0, "width": 160.0, "height": 20.0 },
        { "text": "Your other nationality", "x": 760.0, "y": 600.0, "width": 180.0, "height": 20.0 },
    ] }))
}

fn at(line: &ReadLine) -> (f64, f64) {
    line.center()
}

#[test]
fn the_helpers_lines_are_read_in_the_order_a_person_reads_them() {
    let read: Vec<String> = form().iter().map(|line| line.text.clone()).collect();
    assert_eq!(
        read,
        [
            "Nationality",
            "Select",
            "Are you carrying food, plants or animal products?",
            "Yes",
            "No",
            "Have you visited a farm in the last 30 days?",
            "Yes",
            "No",
            "Your other nationality",
            "Submit registration",
        ],
        "a row shared a point apart is still one row, read left to right"
    );
}

#[test]
fn a_press_by_words_takes_the_one_line_that_reads_them_and_its_centre() {
    let lines = form();
    let submit = choose(&lines, "submit registration", None).expect("one line reads it");
    assert_eq!(at(submit), (960.0, 910.0));
    // The words a person types are enough when only one line holds them.
    assert_eq!(
        choose(&lines, "Submit", None).expect("one").text,
        "Submit registration"
    );
    // Case and runs of space do not matter.
    assert_eq!(
        choose(&lines, "  SUBMIT   registration ", None)
            .expect("one")
            .text,
        "Submit registration"
    );
}

#[test]
fn the_line_that_reads_the_words_exactly_wins_over_lines_that_hold_them() {
    let lines = form();
    let picked = choose(&lines, "Nationality", None).expect("the exact line");
    assert_eq!(picked.text, "Nationality", "not 'Your other nationality'");
}

#[test]
fn two_lines_that_read_the_words_are_named_and_neither_is_pressed() {
    let lines = form();
    let refused = choose(&lines, "No", None).expect_err("two questions answer No");
    assert_eq!(refused.code, error_code::AMBIGUOUS_TARGET);
    assert!(
        refused.message.contains("2 lines read 'no'"),
        "{}",
        refused.message
    );
    assert!(
        refused.message.contains("(1070, 350)") && refused.message.contains("(1070, 428)"),
        "{}",
        refused.message
    );
}

#[test]
fn a_word_that_recurs_is_told_apart_by_the_line_it_follows() {
    let lines = form();
    let farm =
        choose(&lines, "No", Some("visited a farm")).expect("the No after the farm question");
    assert_eq!(at(farm), (1070.0, 428.5));
    let food =
        choose(&lines, "yes", Some("animal products")).expect("the Yes after the food question");
    assert_eq!(at(food), (873.0, 348.5));
}

#[test]
fn words_nobody_shows_are_refused_with_what_was_read() {
    let lines = form();
    let refused = choose(&lines, "Passport number", None).expect_err("not on this screen");
    assert_eq!(refused.code, error_code::ELEMENT_NOT_FOUND);
    assert!(
        refused.message.contains("'Nationality'"),
        "says what it read: {}",
        refused.message
    );
    let anchorless = choose(&lines, "No", Some("criminal record")).expect_err("no such question");
    assert_eq!(anchorless.code, error_code::ELEMENT_NOT_FOUND);
    let none_after =
        choose(&lines, "Nationality", Some("Submit")).expect_err("nothing after the last line");
    assert_eq!(none_after.code, error_code::ELEMENT_NOT_FOUND);
    assert_eq!(
        choose(&lines, "   ", None).expect_err("empty").code,
        error_code::INVALID_ARGUMENT
    );
}

#[test]
fn a_line_missing_its_frame_is_not_one_a_press_can_land_on() {
    let read = lines(
        &json!({ "lines": [{ "text": "Done" }, { "text": "Done", "x": 1.0, "y": 2.0, "width": 40.0, "height": 20.0 }] }),
    );
    assert_eq!(read.len(), 1);
    assert_eq!(
        at(choose(&read, "Done", None).expect("the framed one")),
        (21.0, 12.0)
    );
}

/// The measurement (t-37883; run with `--ignored --nocapture`, on the normal
/// profile and under `taskpolicy -b`): a phone screen's worth of OCR lines
/// read into reading order and chosen from — the window's own share of a
/// press by words, beside the helper's OCR read.
#[test]
#[ignore = "a measurement: run with --ignored --nocapture"]
fn measure_a_choice_on_a_full_screen() {
    const LINES: u32 = 60;
    const ROUNDS: u32 = 10_000;
    let answer = json!({ "lines": (0..LINES).rev().map(|row| json!({
        "text": format!("Line {row} of the form"),
        "x": 760.0, "y": 80.0 + 15.0 * f64::from(row), "width": 200.0, "height": 14.0,
    })).collect::<Vec<_>>() });
    let started = std::time::Instant::now();
    for _ in 0..ROUNDS {
        let read = lines(&answer);
        assert!(choose(&read, "Line 42 of the form", Some("Line 41")).is_ok());
    }
    let micros = started.elapsed().as_secs_f64() * 1e6 / f64::from(ROUNDS);
    println!("{{\"measure\":\"words-choose\",\"lines\":{LINES},\"perChoiceMicros\":{micros:.2}}}");
}
