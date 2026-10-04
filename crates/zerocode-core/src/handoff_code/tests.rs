//! What the person's-turn card may take (t-40807): the rule for a code, the
//! words a reason may not hold, the one table the card reads, and the flag
//! that asks for the line.

use super::*;
use crate::computer_recipe::{RecipeStop, recipe_step_stop};
use crate::computer_use::{
    COMPUTER_BRIDGE_GRACE_MS, COMPUTER_ENTER_GRACE_MS, COMPUTER_LONGEST_DEADLINE_MS,
    ComputerMethod, computer_deadline_ms, parse_command, usage,
};

fn words(argv: &[&str]) -> Vec<String> {
    argv.iter().map(|word| (*word).to_string()).collect()
}

/// What a code is: four to ten ASCII letters or digits, the person's own case
/// kept, and nothing else.
#[test]
fn a_code_is_four_to_ten_letters_or_digits() {
    for good in ["1234", "123456", "A1b2C3", "1234567890", "ABCDEFGHIJ"] {
        let parsed = OneTimeCode::parse(good);
        assert!(parsed.is_ok(), "{good} is a code: {parsed:?}");
        assert_eq!(
            parsed.map(|code| code.reveal().to_string()),
            Ok(good.to_string())
        );
    }
    for (typed, why) in [
        ("", CodeRefusal::Empty),
        ("   ", CodeRefusal::Empty),
        ("- -", CodeRefusal::Empty),
        ("123", CodeRefusal::TooShort),
        ("12345678901", CodeRefusal::TooLong),
        ("12!456", CodeRefusal::NotLettersOrDigits),
        ("１２３４５６", CodeRefusal::NotLettersOrDigits),
        ("인증번호", CodeRefusal::NotLettersOrDigits),
        ("12\n34", CodeRefusal::NotLettersOrDigits),
    ] {
        assert_eq!(OneTimeCode::parse(typed).err(), Some(why), "{typed:?}");
    }
}

/// A code is written in groups; the groups are one code, and the limit counts
/// the characters between the separators.
#[test]
fn a_code_written_in_groups_is_one_code() {
    for (typed, code) in [
        ("123 456", "123456"),
        ("123-456", "123456"),
        ("  123456  ", "123456"),
        ("12 34 56 78", "12345678"),
        ("G-123456", "G123456"),
    ] {
        assert_eq!(
            OneTimeCode::parse(typed)
                .map(|parsed| parsed.reveal().to_string())
                .ok()
                .as_deref(),
            Some(code),
            "{typed:?}"
        );
    }
    assert_eq!(
        OneTimeCode::parse("12345 67890").map(|code| code.chars()),
        Ok(10)
    );
    assert_eq!(
        OneTimeCode::parse("12345 678901").err(),
        Some(CodeRefusal::TooLong)
    );
}

/// A log line, a record or a panic message that holds a code holds its
/// length.
#[test]
fn a_code_never_prints_its_value() {
    let parsed = OneTimeCode::parse("493021");
    assert!(parsed.is_ok(), "a six-digit code is a code");
    let code = parsed.unwrap_or_else(|_| unreachable!("asserted above"));
    let said = format!("{code:?} {:?}", Some(&code));
    assert!(!said.contains("493021"), "the code is in its Debug: {said}");
    assert!(
        said.contains("6 chars"),
        "its Debug says how long it is: {said}"
    );
    assert_eq!(code.chars(), 6);
}

/// Reasons that name something that outlives one use, in the five languages
/// the window speaks: the card for these has no line to type in.
const SECRET_REASONS: &[(&str, &str)] = &[
    ("ko", "비밀번호를 입력해 주세요"),
    ("ko", "은행 계정 비밀번호"),
    ("ko", "카드번호 16자리"),
    ("ko", "카드 뒷면 보안코드(CVC)"),
    ("ko", "핀번호"),
    ("en", "Enter your password"),
    ("en", "the card number"),
    ("en", "CVC from the back of the card"),
    ("en", "security code on the card"),
    ("en", "your PIN."),
    ("en", "the backup codes"),
    ("en", "the one-time password and the card number"),
    ("en", "the card   number"),
    ("ko", "카드  번호"),
    ("ja", "パスワードを入力"),
    ("ja", "カード番号"),
    ("ja", "セキュリティコード"),
    ("ja", "暗証番号"),
    ("zh", "输入密码"),
    ("zh", "银行卡号"),
    ("zh", "卡片安全码"),
    ("zh", "助记词"),
    ("es", "la contraseña"),
    ("es", "número de tarjeta"),
    ("es", "código de seguridad"),
    ("es", "el PIN"),
];

#[test]
fn a_reason_that_names_a_secret_gets_no_line_in_five_languages() {
    for (language, reason) in SECRET_REASONS {
        assert!(
            reason_names_a_secret(reason),
            "[{language}] a code line for `{reason}`"
        );
    }
}

/// What people ask for when they ask for a code — including the words that
/// look like a secret and are not: a one-time password is a code, and `pin`
/// is not a part of `shipping` or `pinned`.
const CODE_REASONS: &[&str] = &[
    "카카오톡 인증번호",
    "휴대폰의 2FA 코드",
    "문자로 온 인증 번호",
    "이메일 확인 코드",
    "일회용 비밀번호(OTP)",
    "1회용 비밀번호",
    "verification code from the SMS",
    "the 2FA code from my phone",
    "one-time password",
    "OTP",
    "code from the shipping notification",
    "the code in the pinned message",
    "one time passcode",
    "SMS認証コード",
    "メールの確認コード",
    "ワンタイムパスワード",
    "短信验证码",
    "邮箱验证码",
    "一次性密码",
    "动态密码",
    "código de verificación del SMS",
    "contraseña de un solo uso",
    "el código que llegó por correo",
];

#[test]
fn a_reason_that_asks_for_a_code_is_never_taken_for_a_secret() {
    for reason in CODE_REASONS {
        assert!(
            !reason_names_a_secret(reason),
            "no code line for `{reason}`"
        );
    }
}

/// A language that loses its words stops guarding: one word from each is
/// pinned in each table.
#[test]
fn the_tables_speak_the_five_languages_of_the_window() {
    for word in [
        "카드번호",
        "card number",
        "カード番号",
        "银行卡号",
        "número de tarjeta",
    ] {
        assert!(SECRET_WORDS.contains(&word), "{word} left SECRET_WORDS");
    }
    for word in ["비밀번호", "password", "パスワード", "密码", "contraseña"] {
        assert!(PASSWORD_WORDS.contains(&word), "{word} left PASSWORD_WORDS");
    }
    for word in ["일회용", "one-time", "ワンタイム", "一次性", "un solo uso"] {
        assert!(ONE_TIME_WORDS.contains(&word), "{word} left ONE_TIME_WORDS");
    }
}

/// The card reads its limits from one table, and the rule that judges what
/// was typed is the same table.
#[test]
fn the_limits_the_card_shows_are_the_ones_the_rule_checks() {
    let table = CodeLimits::TABLE;
    assert_eq!(
        (table.min, table.max, table.typed_max),
        (CODE_MIN_CHARS, CODE_MAX_CHARS, CODE_TYPED_MAX_CHARS)
    );
    assert!(
        OneTimeCode::parse(&"7".repeat(table.min)).is_ok(),
        "the shortest the card says is a code"
    );
    assert_eq!(
        OneTimeCode::parse(&"7".repeat(table.min - 1)).err(),
        Some(CodeRefusal::TooShort)
    );
    assert!(
        OneTimeCode::parse(&"7".repeat(table.max)).is_ok(),
        "the longest the card says is a code"
    );
    assert_eq!(
        OneTimeCode::parse(&"7".repeat(table.max + 1)).err(),
        Some(CodeRefusal::TooLong)
    );
    assert_eq!(
        serde_json::to_value(table).expect("limits serialize"),
        serde_json::json!({ "min": 4, "max": 10, "typedMax": 20 })
    );
}

const INTO_SET_VALUE: &str = r#"["set-value","--app","Form","--element-index","3"]"#;
const INTO_TYPE_TEXT: &str = r#"["type-text","--app","Form","--restore-window"]"#;
const INTO_BROWSER: &str = r##"["browser","type","browser-1","#otp"]"##;
const INTO_BROWSER_SETTER: &str = r##"["browser","type","browser-1","#otp","--value"]"##;
const INTO_PHONE: &str = r#"["emulator","text","--platform","ios","--device","SIM-1"]"#;
const A_VALUE: &str = "ZC7TEST9";

/// `--into` names where a code goes as the words of one input command — the
/// four shapes there are — and the door's own words type the value where the
/// command's verb takes one.
#[test]
fn into_names_where_a_code_goes_in_the_four_shapes() {
    for (raw, tool, verb, typed) in [
        (
            INTO_SET_VALUE,
            EnterTool::Computer,
            "set-value",
            vec![
                "set-value",
                "--app",
                "Form",
                "--element-index",
                "3",
                "--value",
                A_VALUE,
            ],
        ),
        (
            INTO_TYPE_TEXT,
            EnterTool::Computer,
            "type-text",
            vec![
                "type-text",
                "--app",
                "Form",
                "--restore-window",
                "--text",
                A_VALUE,
            ],
        ),
        (
            INTO_BROWSER,
            EnterTool::Browser,
            "type",
            vec!["type", "browser-1", "#otp", A_VALUE],
        ),
        (
            INTO_BROWSER_SETTER,
            EnterTool::Browser,
            "type",
            vec!["type", "browser-1", "#otp", "--value", A_VALUE],
        ),
        (
            INTO_PHONE,
            EnterTool::Emulator,
            "text",
            vec![
                "text",
                "--platform",
                "ios",
                "--device",
                "SIM-1",
                "--text",
                A_VALUE,
            ],
        ),
    ] {
        let parsed = EnterInto::parse(raw);
        assert!(parsed.is_ok(), "{raw} names a place: {parsed:?}");
        let into = parsed.unwrap_or_else(|_| unreachable!("asserted above"));
        assert_eq!(into.tool(), tool, "{raw}");
        assert_eq!(into.verb(), verb, "{raw}");
        assert_eq!(into.words_with(A_VALUE), words(&typed), "{raw}");
        let given: Vec<String> = serde_json::from_str(raw).expect("the flag is an array");
        assert_eq!(into.to_words(), given, "{raw}: the array as it was given");
    }
    assert_eq!(EnterTool::Computer.name(), "computer");
    assert_eq!(EnterTool::Browser.name(), "browser");
    assert_eq!(EnterTool::Emulator.name(), "emulator");
}

/// What may not be an input for a code: one that carries a value of its own,
/// one that goes by the clipboard (it stays there for `clipboard-read`), one
/// that lifts a guard or asks to be confirmed, one with no field named, and
/// anything that is not an input.
#[test]
fn into_refuses_what_could_hold_or_leak_a_code() {
    let accepted = EnterInto::parse(INTO_SET_VALUE);
    assert!(
        accepted.is_ok(),
        "the plain shape is accepted: {accepted:?}"
    );
    let refuse = |raw: &str, why: &str| {
        let refused = EnterInto::parse(raw);
        assert!(
            refused
                .as_ref()
                .err()
                .is_some_and(|said| said.contains(why)),
            "{raw} is refused for {why:?}: {refused:?}"
        );
    };
    // A value of its own: the code is the value, and only the window writes it.
    refuse(
        r#"["set-value","--app","Form","--element-index","3","--value","1234"]"#,
        "--value",
    );
    refuse(r#"["type-text","--app","Form","--text","1234"]"#, "--text");
    refuse(
        r#"["set-value","--app","Form","--element-index","3","--value-stdin"]"#,
        "--value-stdin",
    );
    refuse(
        r#"["type-text","--app","Form","--text-stdin"]"#,
        "--text-stdin",
    );
    refuse(
        r##"["browser","type","browser-1","#otp","hunter2"]"##,
        "browser",
    );
    refuse(
        r##"["browser","type","browser-1","#otp","--value-stdin"]"##,
        "--value-stdin",
    );
    refuse(
        r#"["emulator","text","--platform","ios","--device","SIM-1","--text","1234"]"#,
        "--text",
    );
    // A second thing in the one command: an answer's shape, a guard lifted, a press confirmed.
    refuse(
        r#"["set-value","--app","Form","--element-index","3","--json"]"#,
        "--json",
    );
    refuse(
        r#"["set-value","--app","Form","--element-index","3","--confirming","payment"]"#,
        "--confirming",
    );
    refuse(
        r#"["type-text","--app","Form","--allow-self"]"#,
        "--allow-self",
    );
    // The clipboard keeps what it is given, and `clipboard-read` gives it back.
    refuse(r#"["paste-text","--app","Form"]"#, "paste-text");
    refuse(r#"["clipboard-write"]"#, "clipboard-write");
    // Not an input, or no field named.
    refuse(r#"["type"]"#, "type");
    refuse(r#"["key","--key","return"]"#, "key");
    refuse(r#"["click","--app","Form","--text","Send"]"#, "click");
    refuse(r#"["set-value","--element-index","3"]"#, "--app");
    refuse(r#"["set-value","--app","Form"]"#, "--element-index");
    refuse(r#"["type-text"]"#, "--app");
    refuse(r#"["browser","type","browser-1"]"#, "browser");
    refuse(r##"["browser","click","browser-1","#otp"]"##, "browser");
    refuse(r#"["emulator","text"]"#, "emulator");
    refuse(
        r#"["emulator","tap","--platform","ios","--device","SIM-1"]"#,
        "emulator",
    );
    refuse("[]", "--into");
    refuse(r#"[""]"#, "--into");
    refuse(r#"{"into":"set-value"}"#, "--into");
    refuse("set-value --app Form", "--into");
    refuse("[1,2]", "--into");
}

/// A card for a code says where the code goes, and a place is only named for
/// a card that has a line: the parse refuses either alone, and keeps the
/// words in the params as an array the window reads again.
#[test]
fn a_handoff_asks_a_code_only_with_into_and_into_only_with_a_code() {
    let asked = parse_command(&words(&[
        "handoff",
        "--ask-code",
        "--reason",
        "카카오톡 인증번호",
        "--timeout-ms",
        "180000",
        "--into",
        INTO_SET_VALUE,
        "--json",
    ]));
    assert!(asked.is_ok(), "handoff --ask-code --into parses: {asked:?}");
    let asked = asked.unwrap_or_else(|_| unreachable!("asserted above"));
    assert_eq!(asked.method, ComputerMethod::Handoff);
    assert_eq!(asked.params["askCode"], true);
    assert_eq!(asked.params["reason"], "카카오톡 인증번호");
    assert_eq!(asked.params["timeoutMs"], 180_000);
    assert_eq!(
        asked.params["into"],
        serde_json::json!(["set-value", "--app", "Form", "--element-index", "3"])
    );

    let plain = parse_command(&words(&["handoff", "--reason", "휴대폰의 2FA 코드"]));
    assert!(plain.is_ok(), "a plain handoff still parses");
    let plain = plain.unwrap_or_else(|_| unreachable!("asserted above"));
    assert!(
        plain.params.get("askCode").is_none() && plain.params.get("into").is_none(),
        "a plain handoff asks for no code and names no place"
    );

    let without_a_place = parse_command(&words(&["handoff", "--ask-code", "--reason", "x"]));
    assert!(
        without_a_place
            .as_ref()
            .err()
            .is_some_and(|why| why.contains("--into")),
        "a code has nowhere to go without --into: {without_a_place:?}"
    );
    let without_a_code = parse_command(&words(&[
        "handoff",
        "--reason",
        "x",
        "--into",
        INTO_SET_VALUE,
    ]));
    assert!(
        without_a_code
            .as_ref()
            .err()
            .is_some_and(|why| why.contains("--ask-code")),
        "a place is named for a card that takes a code: {without_a_code:?}"
    );
    let not_an_input = parse_command(&words(&[
        "handoff",
        "--ask-code",
        "--reason",
        "x",
        "--into",
        r#"["paste-text","--app","Form"]"#,
    ]));
    assert!(
        not_an_input
            .as_ref()
            .err()
            .is_some_and(|why| why.contains("paste-text")),
        "the place is checked before the card is shown: {not_an_input:?}"
    );
    let elsewhere = parse_command(&words(&["wait", "--ms", "1", "--into", INTO_SET_VALUE]));
    assert!(elsewhere.is_err(), "no other verb takes --into");
    let elsewhere = parse_command(&words(&["wait", "--ms", "1", "--ask-code"]));
    assert!(elsewhere.is_err(), "no other verb takes --ask-code");
}

#[test]
fn the_manual_names_the_code_card_and_where_its_code_goes() {
    let manual = usage();
    assert!(
        manual.contains(
            "handoff --reason <what the person must do> [--ask-code --into '<json words>']"
        ),
        "the usage line for handoff lost --ask-code --into:\n{manual}"
    );
    assert!(
        !manual.contains("the answer is that code"),
        "the manual still says the agent is handed the code:\n{manual}"
    );
}

/// A code card waits as long as any person's turn, and then as long as the
/// typing takes: the one ladder the bridge, the shim and zo read
/// (`computer_deadline_ms`), no second clock.
#[test]
fn a_code_card_waits_for_its_person_and_then_for_its_typing() {
    let ask = words(&[
        "handoff",
        "--ask-code",
        "--reason",
        "x",
        "--timeout-ms",
        "180000",
        "--into",
        INTO_SET_VALUE,
    ]);
    let plain = words(&["handoff", "--reason", "x", "--timeout-ms", "180000"]);
    assert_eq!(
        computer_deadline_ms(&plain),
        180_000 + COMPUTER_BRIDGE_GRACE_MS,
        "a plain turn waits for its person and no longer"
    );
    assert_eq!(
        computer_deadline_ms(&ask),
        180_000 + COMPUTER_BRIDGE_GRACE_MS + COMPUTER_ENTER_GRACE_MS,
        "a turn that types the code waits for the typing too"
    );
    let longest = words(&[
        "handoff",
        "--ask-code",
        "--reason",
        "x",
        "--into",
        INTO_SET_VALUE,
    ]);
    assert!(
        computer_deadline_ms(&longest) <= COMPUTER_LONGEST_DEADLINE_MS,
        "the longest turn still fits the shim's own clock"
    );
}

/// A recipe that saved a code card walks to it and stops: a walk has nobody
/// to hand a value to, so it is the person's turn like any other.
#[test]
fn a_saved_code_card_is_the_persons_turn_in_a_walk() {
    assert_eq!(
        recipe_step_stop(&words(&["handoff", "--ask-code", "--reason", "x"])),
        Some(RecipeStop::PersonsTurn)
    );
}
