use serde_json::json;

use super::*;

/// One line of a screen, as OCR boxes it.
fn line(text: &str, y: f64) -> ReadLine {
    ReadLine {
        text: text.to_string(),
        x: 100.0,
        y,
        width: 200.0,
        height: 17.0,
    }
}

fn screen(texts: &[&str]) -> Vec<ReadLine> {
    texts
        .iter()
        .enumerate()
        .map(|(at, text)| line(text, 40.0 * (at as f64 + 1.0)))
        .collect()
}

fn typed_answer(reason: &str) -> Value {
    json!({ "action": { "path": "synthetic", "verification": { "state": "unverified", "reason": reason } } })
}

/// A value that is on the screen matches, a suggestion after it included, and
/// neither case nor the lines it wraps over nor the spaces OCR drops count.
#[test]
fn a_value_the_screen_shows_matches() {
    for (typed, shown) in [
        ("Korea", vec!["Search", "Korea, Republic of"]),
        ("korea", vec!["Korea"]),
        ("KOREA", vec!["korea"]),
        ("Hello World", vec!["Hello", "World"]),
        ("Hello World", vec!["HelloWorld"]),
        ("안녕하세요", vec!["메모", "안녕하세요"]),
    ] {
        assert_eq!(
            judge(typed, &screen(&shown)),
            TypedCheck::Match,
            "{typed:?} on {shown:?}"
        );
    }
}

/// OCR writes `1` for `l`, `I` for `l`, `0` for `O` and back, in both
/// directions: none of those stops a walk.
#[test]
fn what_ocr_reads_alike_is_no_mismatch() {
    for (typed, shown) in [
        ("hello", "he11o"),
        ("he11o", "hello"),
        ("Ill1", "1lIl"),
        ("O0O0", "0O0O"),
        ("Order100", "0rderlOO"),
        ("bool", "b0ol"),
        ("user|name", "userlname"),
    ] {
        assert_eq!(
            judge(typed, &screen(&[shown])),
            TypedCheck::Match,
            "{typed:?} read as {shown:?}"
        );
    }
}

/// A phone's keyboard writes curly quotes and long dashes for the plain ones.
#[test]
fn a_phones_smart_punctuation_is_no_mismatch() {
    assert_eq!(judge("don't", &screen(&["don’t"])), TypedCheck::Match);
    assert_eq!(judge("a-b-c", &screen(&["a–b—c"])), TypedCheck::Match);
    assert_eq!(judge("\"go\"", &screen(&["“go”"])), TypedCheck::Match);
}

/// A real difference is a mismatch, and it names the character where the two
/// part, counted in the value as it was given, spaces included.
#[test]
fn a_real_difference_names_where_the_two_part() {
    for (typed, shown, at, of) in [
        ("Korea", "Korca", 4, 5),
        ("Korea", "orea", 1, 5),
        ("Korea", "Kor", 4, 5),
        ("ab cd", "ab ce", 5, 5),
        ("안녕하세요", "안넝하세요", 2, 5),
        ("hello", "ㅗ디ㅣㅣㅐ", 1, 5),
    ] {
        assert_eq!(
            judge(typed, &screen(&[shown])),
            TypedCheck::Mismatch { at, of },
            "{typed:?} read as {shown:?}"
        );
    }
}

/// A screen with no text says nothing; nor does a blank line.
#[test]
fn a_screen_with_no_text_is_unread() {
    for shown in [vec![], vec!["   "]] {
        assert_eq!(
            judge("Korea", &screen(&shown)),
            TypedCheck::Unread(UnreadWhy::NoText)
        );
    }
}

/// A long value shows only its tail in a field that scrolled to the caret:
/// that is no mismatch. Both its ends on the screen with the middle wrong is.
#[test]
fn a_long_value_that_runs_out_of_view_is_unread() {
    let long = "abcdefghij-klmnopqrst-uvwxyz";
    assert_eq!(
        judge(long, &screen(&["rst-uvwxyz"])),
        TypedCheck::Unread(UnreadWhy::RunsOutOfView)
    );
    assert_eq!(
        judge(long, &screen(&["abcdefghij-kXmnopqrst-uvwxyz"])),
        TypedCheck::Mismatch { at: 13, of: 28 }
    );
    assert_eq!(
        judge("abcdefghij", &screen(&["efghij"])),
        TypedCheck::Mismatch { at: 1, of: 10 },
        "a short value fits its field"
    );
}

/// What OCR cannot read back as written, or a field's next piece carried
/// elsewhere, is not read for at all.
#[test]
fn what_ocr_cannot_read_back_is_not_read_for() {
    let answer = typed_answer("synthetic_input");
    for (typed, why) in [
        ("ab", UnreadWhy::TooShort),
        ("1", UnreadWhy::TooShort),
        ("...", UnreadWhy::TooShort),
        ("hi 👋 there", UnreadWhy::NotPlainText),
        ("cafe\u{301} menu", UnreadWhy::NotPlainText),
        (
            "\u{1112}\u{1161}\u{11ab}\u{1100}\u{1173}\u{11af}",
            UnreadWhy::NotPlainText,
        ),
        ("user\tname", UnreadWhy::MovesFocus),
        ("one\ntwo", UnreadWhy::MovesFocus),
    ] {
        assert_eq!(before_reading(typed, &answer), Some(why), "{typed:?}");
    }
    for typed in ["Korea", "안녕하세요", "hello, world!", "010-1234-5678"] {
        assert_eq!(before_reading(typed, &answer), None, "{typed:?}");
    }
}

/// A value that is a secret by its kind, and keys the helper says went under
/// the keyboard's secure mode, are never compared.
#[test]
fn a_secret_is_not_compared() {
    let answer = typed_answer("synthetic_input");
    for typed in [
        "4242 4242 4242 4242",
        "4242-4242-4242-4242",
        "sk-ant-abcdefgh",
        "abcdefghij1234567890abcd",
    ] {
        assert_eq!(
            before_reading(typed, &answer),
            Some(UnreadWhy::Secret),
            "{typed:?}"
        );
    }
    let secure = json!({ "action": {
        "verification": { "state": "unverified", "reason": "synthetic_input" },
        "secureInput": { "on": true },
    } });
    assert_eq!(before_reading("Korea", &secure), Some(UnreadWhy::Secret));
}

/// A screen that asks for a secret is not compared either, whatever it shows:
/// its field masks what it is given, so a difference there means nothing.
#[test]
fn a_screen_that_asks_for_a_secret_is_not_compared() {
    for shown in [
        vec!["Password", "•••••"],
        vec!["비밀번호", "•••••"],
        vec!["카드 번호"],
        vec!["CVC"],
        vec!["Enter your verification code"],
        vec!["Passcode"],
    ] {
        assert_eq!(
            judge("Korea", &screen(&shown)),
            TypedCheck::Unread(UnreadWhy::Secret),
            "{shown:?}"
        );
    }
}

/// Only keys nobody read back ask the screen: a field the helper read has its
/// own word.
#[test]
fn only_keys_nobody_read_back_ask_a_look() {
    assert!(asks_a_look(&typed_answer("synthetic_input")));
    for reason in ["value_mismatch", "value_unchanged", "secret_field"] {
        assert!(!asks_a_look(&typed_answer(reason)), "{reason}");
    }
    assert!(!asks_a_look(&json!({
        "action": { "verification": { "state": "verified", "property": "focusedText" } }
    })));
    assert!(!asks_a_look(
        &json!({ "action": { "path": "accessibility" } })
    ));
}

/// The three answers say the three words, and none carries the value or what
/// the screen read.
#[test]
fn the_answer_is_one_of_three_words_and_holds_no_value() {
    let secret = "Xyzzy-secret-value";
    let read = "Xyzzy-screen-words";
    let check = judge(secret, &screen(&[read]));
    let said = check.answer().to_string();
    assert_eq!(check.answer()["verdict"], "mismatch");
    assert_eq!(check.answer()["at"], 8);
    assert_eq!(check.answer()["of"], 18);
    for fragment in ["Xyzzy", "secret", "screen", "value", "words"] {
        assert!(!said.contains(fragment), "{said} holds {fragment}");
    }
    assert_eq!(TypedCheck::Match.answer(), json!({ "verdict": "match" }));
    assert_eq!(
        TypedCheck::Unread(UnreadWhy::NoText).answer(),
        json!({ "verdict": "unread", "why": "no_text" })
    );
    let stop = stop_sentence(&json!({ INPUT_CHECK_KEY: check.answer() })).expect("it stops");
    assert!(stop.contains("character 8 of 18"), "{stop}");
    for fragment in ["Xyzzy", "secret", "screen-words", "value"] {
        assert!(!stop.contains(fragment), "{stop} holds {fragment}");
    }
}

/// Only a mismatch stops a walk: an answer without a check goes on, and so
/// does a check that could not say.
#[test]
fn only_a_mismatch_stops_a_walk() {
    for check in [TypedCheck::Match, TypedCheck::Unread(UnreadWhy::ReadFailed)] {
        assert_eq!(
            stop_sentence(&json!({ INPUT_CHECK_KEY: check.answer() })),
            None
        );
    }
    assert_eq!(stop_sentence(&json!({ "action": {} })), None);
}

fn read_of(text: &str) -> Value {
    json!({ "lines": [{ "text": text, "x": 1.0, "y": 2.0, "width": 30.0, "height": 17.0 }] })
}

/// The screen catches up within the budget: the first reading that shows the
/// value ends the check, and it waited only for those before it.
#[test]
fn a_screen_that_catches_up_matches_at_once() {
    let mut readings = vec![read_of("Kor"), read_of("Kore"), read_of("Korea")].into_iter();
    let mut pauses = Vec::new();
    let check = read_back("Korea", &mut || readings.next(), &mut |ms| pauses.push(ms));
    assert_eq!(check, TypedCheck::Match);
    assert_eq!(pauses, [READ_BACK_POLL_MS; 2]);
}

/// A screen that never shows the value is a mismatch once the budget passes,
/// from its last reading, and the budget is a bound.
#[test]
fn a_screen_that_never_shows_it_is_a_mismatch_at_the_budget() {
    let mut reads = 0;
    let mut waited = 0;
    let check = read_back(
        "Korea",
        &mut || {
            reads += 1;
            Some(read_of("Korca"))
        },
        &mut |ms| waited += ms,
    );
    assert_eq!(check, TypedCheck::Mismatch { at: 4, of: 5 });
    assert_eq!(waited, READ_BACK_BUDGET_MS);
    assert_eq!(reads, READ_BACK_BUDGET_MS / READ_BACK_POLL_MS + 1);
}

/// A window that cannot be read says so at once, and a secret screen ends the
/// reading: it is not read again.
#[test]
fn a_failed_reading_or_a_secret_screen_ends_the_check_at_once() {
    let mut paused = false;
    assert_eq!(
        read_back("Korea", &mut || None, &mut |_| paused = true),
        TypedCheck::Unread(UnreadWhy::ReadFailed)
    );
    let mut reads = 0;
    assert_eq!(
        read_back(
            "Korea",
            &mut || {
                reads += 1;
                Some(read_of("Password"))
            },
            &mut |_| paused = true,
        ),
        TypedCheck::Unread(UnreadWhy::Secret)
    );
    assert_eq!(reads, 1);
    assert!(!paused);
}
