use std::path::Path;

use super::*;
use crate::artifact::Limits;
use crate::evidence_digest::{Digest, Format, Steps, read, told};

/// Two cargo test binaries in one log: the first failed two of its tests, left
/// one ignored and three filtered out; the second passed. The failed tests'
/// output holds a line indented like a name, which is not one.
const CARGO_LOG: &str = "\
running 13 tests
test store::tests::a_row_is_written ... ok
test store::tests::a_title_is_read ... FAILED
test store::tests::a_slow_one ... ignored
test store::tests::a_kind_is_stated ... FAILED

failures:

---- store::tests::a_title_is_read stdout ----
thread 'store::tests::a_title_is_read' panicked at src/store.rs:10:5:
assertion `left == right` failed
  left: 1
 right: 2
    a line of the output, indented like a name

failures:
    store::tests::a_kind_is_stated
    store::tests::a_title_is_read

test result: FAILED. 10 passed; 2 failed; 1 ignored; 0 measured; 3 filtered out; finished in 0.52s

running 4 tests
test tests::it_reads ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";

/// One window suite run: two of its four checks failed, one with a detail.
const WINDOW_LOG: &str = "\
SUITE  artifact-cards
PASS  a card says its kind
FAIL  the drawer is one report  — bar height 52, wanted 40
PASS  the chip filters every tab
FAIL  an outline follows the reader

2/4 passed
";

fn read_tests(text: &str, limits: &Limits) -> Tests {
    let digest = read(Format::Text, text.as_bytes(), limits);
    assert!(
        matches!(digest, Some(Digest::Tests(_))),
        "a test log reads as tests: {digest:?}"
    );
    match digest {
        Some(Digest::Tests(tests)) => tests,
        _ => Tests::default(),
    }
}

/// What every digest of a test log must keep: the failed tests are the named
/// ones and the rest, never a third number.
fn the_failed_are_the_named_and_the_rest(tests: &Tests) {
    assert_eq!(
        tests.failed,
        tests.failed_names.len() + tests.more,
        "failed = named + more: {tests:?}"
    );
}

#[test]
fn a_cargo_log_is_summed_over_its_binaries_and_names_what_failed() {
    let tests = read_tests(CARGO_LOG, &Limits::default());
    assert_eq!(
        (tests.passed, tests.failed, tests.ignored, tests.filtered),
        (14, 2, 1, 3),
        "the counts of both binaries, summed"
    );
    assert_eq!(tests.suites, 2);
    assert_eq!(
        tests.failed_names,
        [
            "store::tests::a_title_is_read",
            "store::tests::a_kind_is_stated"
        ],
        "each failed test once, in the order the log first names it"
    );
    assert_eq!(tests.more, 0);
    assert!(!tests.cut);
    the_failed_are_the_named_and_the_rest(&tests);
}

#[test]
fn a_window_suite_log_is_its_summary_line_and_its_fail_lines() {
    let tests = read_tests(WINDOW_LOG, &Limits::default());
    assert_eq!((tests.passed, tests.failed, tests.suites), (2, 2, 1));
    assert_eq!(
        tests.failed_names,
        ["the drawer is one report", "an outline follows the reader"],
        "a failed check is named without its detail"
    );
    the_failed_are_the_named_and_the_rest(&tests);
}

#[test]
fn a_log_of_both_runners_adds_them_up() {
    let both = format!("{CARGO_LOG}\n{WINDOW_LOG}");
    let tests = read_tests(&both, &Limits::default());
    assert_eq!(
        (tests.passed, tests.failed, tests.ignored, tests.filtered),
        (16, 4, 1, 3)
    );
    assert_eq!(tests.suites, 3);
    assert_eq!(tests.failed_names.len(), 4);
    the_failed_are_the_named_and_the_rest(&tests);
}

/// A test that prints without a newline tears its own line: the name is then
/// on the list the binary prints under `failures:`, and a summary glued to the
/// print is still found.
#[test]
fn a_failed_test_whose_line_was_torn_is_named_by_the_binarys_list() {
    let log = "\
running 2 tests
test a::one ... {\"measured\":1}
FAILED
test a::two ... ok

failures:
    a::one

{\"measured\":2}test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
";
    let tests = read_tests(log, &Limits::default());
    assert_eq!((tests.passed, tests.failed), (1, 1));
    assert_eq!(tests.failed_names, ["a::one"]);
    the_failed_are_the_named_and_the_rest(&tests);
}

/// A binary that died before its summary still failed what it named, and a
/// summary that counts more failures than the log names says how many more.
#[test]
fn the_failed_count_is_always_the_names_and_the_rest() {
    let died = "\
running 3 tests
test a::one ... FAILED
test a::two ... ok
error: test failed, to rerun pass `-p store --lib`
";
    let tests = read_tests(died, &Limits::default());
    assert_eq!((tests.failed, tests.suites), (1, 0));
    assert_eq!(tests.failed_names, ["a::one"]);
    the_failed_are_the_named_and_the_rest(&tests);

    let unnamed = "\
test a::one ... FAILED
test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out
";
    let tests = read_tests(unnamed, &Limits::default());
    assert_eq!((tests.failed, tests.more), (3, 2));
    the_failed_are_the_named_and_the_rest(&tests);

    let limits = Limits {
        digest_failures_max: 2,
        digest_text_chars: 12,
        ..Limits::default()
    };
    let many: String = (1..=5)
        .map(|n| format!("FAIL  check {n} of a suite with a long name\n"))
        .chain(["0/5 passed\n".to_string()])
        .collect();
    let tests = read_tests(&many, &limits);
    assert_eq!((tests.failed, tests.more), (5, 3));
    assert_eq!(
        tests.failed_names,
        ["check 1 of a", "check 2 of a"],
        "the table's two names, each cut to the table's characters"
    );
    the_failed_are_the_named_and_the_rest(&tests);
}

/// The verdict of every case of the counts and of what was told: an intended
/// failure is said by the exit code or by the hand-in, and by nothing else.
#[test]
fn a_verdict_is_the_counts_the_exit_code_and_what_the_hand_in_said() {
    let failing = Tests {
        passed: 3,
        failed: 2,
        more: 2,
        ..Tests::default()
    };
    let clean = Tests {
        passed: 3,
        ..Tests::default()
    };
    let cut_clean = Tests {
        cut: true,
        ..clean.clone()
    };
    let nothing = Tests::default();
    let with = |rc: Option<i32>, meant_to_fail: bool| Told { rc, meant_to_fail };
    let cases = [
        (&failing, with(None, false), Verdict::Fail),
        (&failing, with(Some(0), false), Verdict::Intended),
        (&failing, with(Some(101), false), Verdict::Fail),
        (&failing, with(Some(101), true), Verdict::Intended),
        (&failing, with(None, true), Verdict::Intended),
        (&clean, with(None, false), Verdict::Pass),
        (&clean, with(Some(0), false), Verdict::Pass),
        (&clean, with(Some(1), false), Verdict::Fail),
        (&clean, with(Some(1), true), Verdict::Intended),
        // Meant to fail, and nothing failed: the run is what it is.
        (&clean, with(Some(0), true), Verdict::Pass),
        // What lies past the cut may have failed.
        (&cut_clean, with(None, false), Verdict::Unknown),
        (&cut_clean, with(Some(0), false), Verdict::Pass),
        (&nothing, with(None, false), Verdict::Unknown),
        (&nothing, with(Some(0), false), Verdict::Pass),
        (&nothing, with(Some(2), false), Verdict::Fail),
    ];
    for (tests, said, verdict) in cases {
        let judged = tests.clone().judged(said);
        assert_eq!(judged.verdict, verdict, "{tests:?} with {said:?}");
        assert_eq!(judged.rc, said.rc, "the exit code told is kept");
    }
}

/// A log read by itself is judged by its counts alone, so failures nothing
/// vouches for are a failure — whatever the file is called.
#[test]
fn a_log_alone_is_judged_by_its_counts_and_never_by_its_name() {
    assert_eq!(
        read_tests(CARGO_LOG, &Limits::default()).verdict,
        Verdict::Fail
    );
    let passing = "test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
    assert_eq!(
        read_tests(passing, &Limits::default()).verdict,
        Verdict::Pass
    );
    for name in ["red-run.log", "intended-red.out", "expected-fail.txt"] {
        assert_eq!(
            Format::of(Path::new(name)),
            Some(Format::Text),
            "{name} is read as text"
        );
    }
}

/// What is told from outside the log reaches the digest: a test log is judged
/// again, a file with nothing to count becomes a verdict once an exit code is
/// known for it, and a digest of another shape is left as it was.
#[test]
fn what_is_told_from_outside_the_log_reaches_its_digest() {
    let limits = Limits::default();
    let ended_as_expected = Told {
        rc: Some(0),
        meant_to_fail: false,
    };
    let red = told(
        read(Format::Text, CARGO_LOG.as_bytes(), &limits),
        ended_as_expected,
    );
    assert!(
        matches!(
            &red,
            Some(Digest::Tests(tests)) if tests.verdict == Verdict::Intended && tests.rc == Some(0)
        ),
        "failures under an exit code of 0 are an intended failure: {red:?}"
    );

    let build_log = b"   Compiling store v0.1.0\n    Finished `dev` profile in 2.1s\n";
    let plain = read(Format::Text, build_log.as_slice(), &limits);
    assert_eq!(plain, None, "a log with nothing to count has no digest");
    assert_eq!(told(None, Told::default()), None);
    let built = told(None, ended_as_expected);
    assert!(
        matches!(
            &built,
            Some(Digest::Tests(tests))
                if tests.verdict == Verdict::Pass && tests.rc == Some(0) && tests.suites == 0
        ),
        "an exit code alone is a verdict: {built:?}"
    );

    let steps = Some(Digest::Steps(Steps::default()));
    assert_eq!(told(steps.clone(), ended_as_expected), steps);
}

#[test]
fn an_exit_code_file_alone_is_a_verdict() {
    let limits = Limits::default();
    let of = |held: &str| match read(Format::ExitCode, held.as_bytes(), &limits) {
        Some(Digest::Tests(tests)) => Some((tests.rc, tests.verdict)),
        _ => None,
    };
    assert_eq!(of("0\n"), Some((Some(0), Verdict::Pass)));
    assert_eq!(of("1"), Some((Some(1), Verdict::Fail)));
    assert_eq!(of(" 101 \n"), Some((Some(101), Verdict::Fail)));
    for not_a_code in ["", "zero\n", "0\nand more\n", "00000000000000000000\n"] {
        assert_eq!(of(not_a_code), None, "{not_a_code:?} is not an exit code");
    }
    assert_eq!(Format::of(Path::new("gates.rc")), Some(Format::ExitCode));
    assert_eq!(Format::of(Path::new("GATES.LOG")), Some(Format::Text));
}

/// A log longer than the table reads says so, and a cut log that showed only
/// passes is not called passed.
#[test]
fn a_cut_log_says_it_was_cut_and_is_not_called_passed() {
    let first = "test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
    let second = "test result: FAILED. 0 passed; 9 failed; 0 ignored; 0 measured; 0 filtered out\n";
    let limits = Limits {
        digest_bytes_max: first.len() as u64 + 4,
        ..Limits::default()
    };
    let tests = read_tests(&format!("{first}{second}"), &limits);
    assert!(tests.cut, "the second summary lies past the table's bytes");
    assert_eq!((tests.passed, tests.failed, tests.suites), (4, 0, 1));
    assert_eq!(tests.verdict, Verdict::Unknown);
}

#[test]
fn the_wire_shape_of_a_test_log_is_tagged_and_flat() {
    let digest = told(
        read(Format::Text, WINDOW_LOG.as_bytes(), &Limits::default()),
        Told {
            rc: Some(0),
            meant_to_fail: false,
        },
    );
    assert_eq!(
        serde_json::to_value(&digest).unwrap_or_default(),
        serde_json::json!({
            "kind": "tests",
            "passed": 2,
            "failed": 2,
            "ignored": 0,
            "filtered": 0,
            "suites": 1,
            "failed_names": ["the drawer is one report", "an outline follows the reader"],
            "more": 0,
            "cut": false,
            "rc": 0,
            "verdict": "intended",
        })
    );
}

/// The preview cache counts a digest against its byte cap: a test log's digest
/// weighs the names it keeps, not the log it read.
#[test]
fn a_test_logs_digest_weighs_the_names_it_keeps() {
    let limits = Limits::default();
    let weight_of =
        |text: &str| read(Format::Text, text.as_bytes(), &limits).map(|held| held.weight());
    let named = weight_of(WINDOW_LOG);
    let padded = weight_of(&format!("{}{WINDOW_LOG}", "PASS  a check\n".repeat(2_000)));
    assert!(named.is_some(), "the log has a digest");
    assert_eq!(named, padded, "two thousand passed lines weigh nothing");
    let unnamed = weight_of("3/3 passed\n");
    assert!(
        named > unnamed,
        "the names are what a digest holds: {named:?} over {unnamed:?}"
    );
}
