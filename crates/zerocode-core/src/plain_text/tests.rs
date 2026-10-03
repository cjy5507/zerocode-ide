//! The plain-writing lint, held to the numbers it gives on the synthetic
//! reports, the person's own corrections and the edges of the counting.
use super::*;

const BEFORE_KO: &str = include_str!("../../fixtures/plain-text/before.ko.md");
const AFTER_KO: &str = include_str!("../../fixtures/plain-text/after.ko.md");
const BEFORE_EN: &str = include_str!("../../fixtures/plain-text/before.en.md");
const AFTER_EN: &str = include_str!("../../fixtures/plain-text/after.en.md");

/// The table is the one place the words live: every row of the file loads,
/// names what to write instead, and cannot be counted twice by two of its own
/// spellings.
#[test]
fn the_table_loads_every_row_and_each_row_names_what_to_write_instead() {
    let rows = RULES_JSON.matches("\"lang\"").count();
    assert!(rows > 0, "the table file has no rows");
    assert_eq!(
        rules().len(),
        rows,
        "a row of the table file did not load as a rule"
    );
    for rule in rules() {
        assert!(!rule.finds.is_empty(), "{rule:?} names no spelling");
        assert!(!rule.shown.trim().is_empty(), "{rule:?} has no name");
        assert!(
            !rule.plain.trim().is_empty(),
            "{rule:?} says nothing to write instead"
        );
        assert!(!rule.note.trim().is_empty(), "{rule:?} does not say why");
        for find in &rule.finds {
            assert_eq!(find, &find.to_lowercase(), "{rule:?} was not lower-cased");
        }
        // Two spellings of one row where the first begins the second would count
        // one place twice. A whole English word is safe: its right edge is
        // checked, so `utilize` does not stand inside `utilizes`.
        let whole_english = rule.language == Language::English && rule.mode == Match::Word;
        if !whole_english {
            for one in &rule.finds {
                for other in &rule.finds {
                    assert!(
                        one == other || !other.starts_with(one.as_str()),
                        "{one:?} begins {other:?} in one row, so one place would count twice"
                    );
                }
            }
        }
    }
    for language in [Language::Korean, Language::English] {
        for kind in [RuleKind::Word, RuleKind::Pattern] {
            assert!(
                rules()
                    .iter()
                    .any(|rule| rule.language == language && rule.kind == kind),
                "the table has no {kind:?} row for {language:?}"
            );
        }
    }
}

/// The person's own correction (09-21), one sentence for each of its seven
/// phrases: each is found, and each is answered with the plain word.
#[test]
fn each_phrase_the_person_corrected_is_found_and_answered_with_the_plain_word() {
    let corrected = [
        ("Jev 자리의 판정이 느리다.", "Jev 자리", "Jev 기능"),
        ("규칙 표를 읽는다.", "규칙 표", "규칙"),
        ("스윕 박자를 조정했다.", "박자", "주기"),
        ("진입점 문으로 들어온다.", "진입점 문", "진입점·경로"),
        ("설정값 낱말을 바꿨다.", "낱말", "설정값 또는 단어"),
        ("그 항목은 손을 든다.", "손을 든다", "적용하지 않음"),
        ("그 항목은 표만 돈다.", "표만 돈다", "기록만 한다"),
    ];
    for (sentence, shown, plain) in corrected {
        let found = lint(sentence);
        assert_eq!(found.lang, Language::Korean, "{sentence}");
        assert_eq!(found.words, 1, "{sentence}: {found:?}");
        assert_eq!(found.patterns, 0, "{sentence}: {found:?}");
        let hit = found
            .hits
            .first()
            .unwrap_or_else(|| panic!("{sentence} hit nothing"));
        assert_eq!(hit.kind, RuleKind::Word, "{sentence}");
        assert_eq!(hit.find, shown, "{sentence}");
        assert_eq!(hit.plain, plain, "{sentence}");
    }
}

/// A sentence ends where a reader stops. The point in a number or a version
/// does not end one, code is one character, and what is not prose is not read.
#[test]
fn sentences_end_where_a_reader_stops_and_markup_is_not_prose() {
    let decimals = lint("시간이 7.4초에서 2.1초로 줄었다. 버전은 1.1.50이다.");
    assert_eq!(
        (decimals.sentences, decimals.longest),
        (2, 21),
        "{decimals:?}"
    );

    let code = lint("`cargo test -p zerocode-core`를 돌렸다.");
    assert_eq!(
        (code.sentences, code.longest),
        (1, 7),
        "code counts as one character"
    );

    let link = lint("[보고서](https://example.com/a.b)를 읽었다.");
    assert_eq!(
        (link.sentences, link.longest),
        (1, 9),
        "a link keeps its words, not its address"
    );

    let markdown = "---\nname: x\n---\n# 제목\n\n첫 문장이다. 두 번째 문장이다.\n둘째 줄은 같은 문단이다.\n\n- 목록 하나\n- 목록 둘\n\n| 항목 | 값 |\n| --- | --- |\n| a | b |\n\n```\n코드 문장입니다.\n```\n\n> 인용이다.\n";
    let structured = lint(markdown);
    assert_eq!(
        (structured.sentences, structured.longest, structured.avg_len),
        (6, 14, 8),
        "metadata, headings, table rows and fenced code are not prose: {structured:?}"
    );
}

/// The limit is the language's own: a Korean sentence of 60 characters is
/// fine and one of 61 is long; an English one of 25 words is fine and 26 is long.
#[test]
fn a_sentence_is_long_one_unit_past_the_limit_of_its_language() {
    let korean = |chars: usize| lint(&format!("{}.", "가".repeat(chars - 1)));
    let at_limit = korean(KO_SENTENCE_CHARS_MAX);
    assert_eq!(
        (at_limit.longest, at_limit.long_sentences),
        (60, 0),
        "{at_limit:?}"
    );
    let past = korean(KO_SENTENCE_CHARS_MAX + 1);
    assert_eq!(
        (past.longest, past.long_sentences, past.limit),
        (61, 1, 60),
        "{past:?}"
    );

    let english = |words: usize| lint(&format!("{} end.", vec!["word"; words - 1].join(" ")));
    let at_limit = english(EN_SENTENCE_WORDS_MAX);
    assert_eq!(
        (at_limit.longest, at_limit.long_sentences),
        (25, 0),
        "{at_limit:?}"
    );
    let past = english(EN_SENTENCE_WORDS_MAX + 1);
    assert_eq!(
        (past.longest, past.long_sentences, past.limit),
        (26, 1, 25),
        "{past:?}"
    );
}

/// The language follows the letters, not the markup around them.
#[test]
fn the_language_follows_the_letters_and_other_scripts_get_no_counts() {
    assert_eq!(
        lint("`cargo` `test` `clippy` 시험을 돌렸다.").lang,
        Language::Korean,
        "identifiers in code do not make a Korean sentence English"
    );
    assert_eq!(
        lint("The retry count fell from 1,204 to 312 per day 줄었다.").lang,
        Language::English,
        "one Korean word does not make an English sentence Korean"
    );
    for silent in [
        "1234 5678",
        "これは日本語です。",
        "",
        "```\ncode\n```",
        "| a | b |\n| --- | --- |",
    ] {
        let found = lint(silent);
        assert_eq!(
            found,
            TextLint::default(),
            "{silent:?} should have no counts"
        );
    }
    assert_eq!(
        lint("The check was performed by the worker. We utilize it."),
        TextLint {
            lang: Language::English,
            sentences: 2,
            avg_len: 5,
            longest: 7,
            limit: 25,
            long_sentences: 0,
            words: 1,
            patterns: 1,
            hits: vec![
                RuleHit {
                    kind: RuleKind::Word,
                    find: "utilize".into(),
                    plain: "use".into(),
                    count: 1,
                },
                RuleHit {
                    kind: RuleKind::Pattern,
                    find: "is performed".into(),
                    plain: "name who acts and use an active verb".into(),
                    count: 1,
                },
            ],
            cut: false,
        },
        "English text is read with the English rows, in table order for equal counts"
    );
}

/// The same report before and after the rule, from the lint itself: the
/// numbers the report quotes.
#[test]
fn the_same_report_before_and_after_the_rule_moves_every_number() {
    let numbers = |text: &str| {
        let found = lint(text);
        (
            found.sentences,
            found.avg_len,
            found.longest,
            found.long_sentences,
            found.words,
            found.patterns,
        )
    };
    // (sentences, mean length, longest, long sentences, words, patterns)
    assert_eq!(numbers(BEFORE_KO), (6, 61, 77, 4, 10, 13), "Korean, before");
    assert_eq!(numbers(AFTER_KO), (9, 24, 30, 0, 0, 0), "Korean, after");
    assert_eq!(numbers(BEFORE_EN), (6, 25, 47, 3, 6, 7), "English, before");
    assert_eq!(numbers(AFTER_EN), (7, 9, 11, 0, 0, 0), "English, after");
    for (name, text) in [
        ("ko-before", BEFORE_KO),
        ("ko-after", AFTER_KO),
        ("en-before", BEFORE_EN),
        ("en-after", AFTER_EN),
    ] {
        let found = lint(text);
        eprintln!(
            "PLAIN_TEXT_NUMBERS {name} lang={:?} sentences={} mean={} longest={} limit={} long={} words={} patterns={}",
            found.lang,
            found.sentences,
            found.avg_len,
            found.longest,
            found.limit,
            found.long_sentences,
            found.words,
            found.patterns
        );
    }
}

/// A result names the rules hit most often, most first, and at most the cap;
/// the totals still count every rule.
#[test]
fn the_rules_a_result_names_are_the_most_frequent_first_and_capped() {
    let found = lint(BEFORE_KO);
    assert_eq!(found.hits.len(), MAX_REPORTED_RULES, "{found:?}");
    assert!(
        found
            .hits
            .windows(2)
            .all(|pair| pair[0].count >= pair[1].count),
        "{:?}",
        found.hits
    );
    assert_eq!(found.hits[0].count, 2);
    let named: u32 = found.hits.iter().map(|hit| hit.count).sum();
    assert!(
        named < found.words + found.patterns,
        "the totals count the rules the cap left out"
    );
    // Equal counts keep the table's order: the first Korean word row with two hits.
    assert_eq!(found.hits[0].find, "박자");
}

/// A text longer than the cap is read to the cap, on a whole character, and
/// says so; a text under it says nothing.
#[test]
fn a_text_past_the_cap_is_read_to_the_cap_and_says_so() {
    assert!(!lint(AFTER_KO).cut);
    let long = "가".repeat(LINT_TEXT_BYTES_MAX);
    let found = lint(&long);
    assert!(found.cut, "{found:?}");
    assert_eq!(found.sentences, 1);
    // "가" is three bytes: the cap falls inside one, and the cut backs up to its start.
    assert_eq!(found.longest as usize, LINT_TEXT_BYTES_MAX / 3);
}

/// Worked out once per text, remembered while the text stays, let go of when a
/// beat does not ask.
#[test]
fn a_lint_is_worked_out_once_until_its_text_changes_and_let_go_of_when_not_asked() {
    let mut memo = LintMemo::default();
    memo.begin();
    let first = memo.lint_of("run-1", "t-1", BEFORE_KO);
    let again = memo.lint_of("run-1", "t-1", BEFORE_KO);
    assert_eq!(first, again);
    assert_eq!(memo.worked(), 1, "the same text was worked out twice");
    let changed = memo.lint_of("run-1", "t-1", AFTER_KO);
    assert_eq!(memo.worked(), 2, "a changed text was not worked out again");
    assert_ne!(first, changed);
    memo.lint_of("run-1", "t-2", AFTER_EN);
    memo.end();
    assert_eq!(memo.len(), 2);

    memo.begin();
    memo.lint_of("run-1", "t-2", AFTER_EN);
    memo.end();
    assert_eq!(
        memo.worked(),
        3,
        "a lint nobody changed was worked out again"
    );
    assert_eq!(
        memo.len(),
        1,
        "the lint nobody asked for in the beat was kept"
    );
    assert!(!memo.is_empty());
}

/// The cost of one lint on a report of about 20 KB, for the report to quote.
/// Run on the normal profile and under `taskpolicy -b`:
/// `cargo test -p zerocode-core --lib plain_text::tests::the_cost -- --ignored --nocapture`.
#[test]
#[ignore = "a measurement, run on purpose"]
fn the_cost_of_one_lint_on_a_20_kb_report() {
    const REPORT_BYTES: usize = 20 * 1024;
    const RUNS: usize = 200;
    let mut report = String::new();
    while report.len() < REPORT_BYTES {
        report.push_str(BEFORE_KO);
        report.push('\n');
    }
    let sentences = lint(&report).sentences;
    let mut micros: Vec<u128> = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = std::time::Instant::now();
        let found = lint(std::hint::black_box(&report));
        micros.push(started.elapsed().as_micros());
        assert_eq!(found.sentences, sentences);
    }
    micros.sort_unstable();
    let bytes = report.len();
    let (median, p95, max) = (micros[RUNS / 2], micros[RUNS * 95 / 100], micros[RUNS - 1]);
    eprintln!(
        "PLAIN_TEXT_NUMBERS cost bytes={bytes} sentences={sentences} runs={RUNS} median_us={median} p95_us={p95} max_us={max}"
    );
}
