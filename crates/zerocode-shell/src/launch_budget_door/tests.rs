use super::*;

const NOW: i64 = 1_800_000_000_000;
const MINUTE: i64 = 60_000;

fn answered(success: bool, stdout: &str, stderr: &str) -> Result<Once, OnceFailure> {
    Ok(Once {
        success,
        stdout: stdout.to_string(),
        stderr_tail: stderr.to_string(),
    })
}

fn launch(provider: &str, requested: bool) -> Launch<'_> {
    Launch {
        provider,
        job: None,
        fresh_ms: None,
        requested,
    }
}

fn unlimited() -> Limits {
    Limits {
        concurrent: None,
        per_hour: None,
        per_day: None,
    }
}

fn ran_ok(budgeted: &Budgeted) -> bool {
    matches!(budgeted, Budgeted::Ran(Ok(once)) if once.success)
}

#[test]
fn a_launch_that_is_let_through_runs_and_is_answered_as_it_always_was() {
    let budgeter = Budgeter::new(None, Limits::default());
    let budgeted = budgeter.run(
        &launch("claude", false),
        || NOW,
        || answered(true, "draft", ""),
    );
    assert!(ran_ok(&budgeted));
    assert_eq!(budgeter.counters(NOW).last_hour, 1);
    assert_eq!(
        budgeter.counters(NOW).active,
        0,
        "and its place is free again"
    );
}

#[test]
fn a_background_launch_is_held_to_the_ceilings_and_a_requested_one_is_not() {
    let budgeter = Budgeter::new(
        None,
        Limits {
            concurrent: None,
            per_hour: Some(2),
            per_day: None,
        },
    );
    for _ in 0..2 {
        assert!(ran_ok(&budgeter.run(
            &launch("claude", false),
            || NOW,
            || answered(true, "", "")
        )));
    }
    let refused = budgeter.run(&launch("claude", false), || NOW, || answered(true, "", ""));
    assert!(
        matches!(
            refused,
            Budgeted::Refused(Refusal::Hourly {
                used: 2,
                limit: 2,
                ..
            })
        ),
        "the third background launch of the hour"
    );
    assert!(
        ran_ok(&budgeter.run(&launch("claude", true), || NOW, || answered(true, "", ""))),
        "a person's own button is never held to the ceilings"
    );
    assert_eq!(budgeter.counters(NOW).last_hour, 3, "though it is counted");
}

#[test]
fn a_wall_rests_the_provider_for_every_launch_and_only_that_provider() {
    let budgeter = Budgeter::new(None, Limits::default());
    let wall = "You've hit your session limit · resets 4:10am";
    let stopped = budgeter.run(
        &launch("claude", false),
        || NOW,
        || answered(false, "", wall),
    );
    assert!(
        matches!(stopped, Budgeted::Ran(Ok(_))),
        "it ran, and it was the wall"
    );
    for requested in [false, true] {
        let refused = budgeter.run(
            &launch("claude", requested),
            || NOW + MINUTE,
            || answered(true, "", ""),
        );
        assert!(
            matches!(refused, Budgeted::Refused(Refusal::Resting { .. })),
            "requested={requested}: the provider rests"
        );
    }
    assert!(ran_ok(&budgeter.run(
        &launch("codex", false),
        || NOW + MINUTE,
        || answered(true, "", "")
    )));
    assert!(
        ran_ok(&budgeter.run(
            &launch("claude", false),
            || NOW + REST_UNKNOWN_MS,
            || answered(true, "", "")
        )),
        "and stops resting at its time"
    );
}

#[test]
fn the_walls_own_words_are_read_in_the_result_and_in_the_error_stream() {
    let wall = "You've hit your session limit · resets 4:10am";
    let result = format!(r#"{{"type":"result","is_error":true,"result":"{wall}"}}"#);
    for (success, stdout, stderr) in [
        (false, "", wall),
        (false, result.as_str(), ""),
        (true, result.as_str(), ""),
    ] {
        assert!(
            matches!(
                outcome_of("claude", &answered(success, stdout, stderr), NOW),
                Outcome::Rested { .. }
            ),
            "{stdout}{stderr}"
        );
    }
    assert!(matches!(
        outcome_of(
            "codex",
            &answered(false, "", "You've hit your usage limit."),
            NOW
        ),
        Outcome::Rested { .. }
    ));
}

#[test]
fn a_failure_that_is_not_a_wall_rests_nobody_and_a_good_run_is_done() {
    assert_eq!(
        outcome_of("claude", &answered(true, "a draft", ""), NOW),
        Outcome::Done
    );
    assert_eq!(
        outcome_of(
            "claude",
            &answered(false, "", "claude: command not found"),
            NOW
        ),
        Outcome::Failed
    );
    assert_eq!(
        outcome_of(
            "claude",
            &answered(true, r#"{"is_error":true,"result":"boom"}"#, ""),
            NOW
        ),
        Outcome::Failed
    );
    assert_eq!(
        outcome_of("claude", &Err(OnceFailure::TimedOut), NOW),
        Outcome::Failed
    );
}

#[test]
fn the_same_job_is_refused_while_it_runs_and_the_lock_is_not_held_meanwhile() {
    let budgeter = Budgeter::new(None, unlimited());
    let job = zerocode_core::launch_budget::job_key("/repo", "", "draft", "the prompt");
    let same = Launch {
        provider: "claude",
        job: Some(&job),
        fresh_ms: None,
        requested: true,
    };
    let outer = budgeter.run(
        &same,
        || NOW,
        || {
            let inner = budgeter.run(&same, || NOW, || answered(true, "", ""));
            assert!(
                matches!(inner, Budgeted::Refused(Refusal::Running)),
                "a double click while the first still runs"
            );
            answered(true, "the draft", "")
        },
    );
    assert!(ran_ok(&outer));
    assert!(
        ran_ok(&budgeter.run(&same, || NOW, || answered(true, "", ""))),
        "a regenerated draft is a new draft"
    );
}

#[test]
fn the_ledger_is_kept_in_its_file_and_read_back_after_a_restart() {
    let dir = tempfile::tempdir().expect("a config root");
    let file = dir.path().join(FILE);
    let first = Budgeter::new(Some(file.clone()), Limits::default());
    first.run(&launch("claude", false), || NOW, || answered(true, "", ""));
    first.run(
        &launch("codex", false),
        || NOW,
        || answered(false, "", "You've hit your usage limit."),
    );
    assert!(file.is_file(), "the ledger is written down");
    let restarted = Budgeter::new(Some(file), Limits::default());
    let counters = restarted.counters(NOW + MINUTE);
    assert_eq!(counters.last_hour, 2, "the launches are kept");
    assert_eq!(counters.resting.len(), 1, "and the wall");
    assert_eq!(counters.resting[0].provider, "codex");
    assert_eq!(counters.active, 0);
}

#[test]
fn a_ceiling_a_person_just_set_applies_to_the_next_launch() {
    let budgeter = Budgeter::new(None, Limits::default());
    assert!(ran_ok(&budgeter.run(
        &launch("claude", false),
        || NOW,
        || answered(true, "", "")
    )));
    budgeter.set_limits(Limits {
        concurrent: None,
        per_hour: Some(1),
        per_day: None,
    });
    assert!(matches!(
        budgeter.run(&launch("claude", false), || NOW, || answered(true, "", "")),
        Budgeted::Refused(Refusal::Hourly { .. })
    ));
}

#[test]
fn every_refusal_says_itself_in_a_sentence_with_its_numbers() {
    let now = crate::now_epoch_ms();
    let said = |refusal: Refusal| refusal_said(&refusal);
    assert!(
        said(Refusal::Resting {
            until_ms: now + 20 * MINUTE
        })
        .contains("쉬는 중")
    );
    assert!(said(Refusal::Running).contains("이미 돌고"));
    assert!(said(Refusal::Fresh { age_ms: 4 * MINUTE }).contains("4분"));
    assert!(
        said(Refusal::Concurrent {
            active: 4,
            limit: 4
        })
        .contains('4')
    );
    assert!(
        said(Refusal::Hourly {
            used: 300,
            limit: 300,
            retry_at_ms: None
        })
        .contains("300")
    );
    assert!(
        said(Refusal::Daily {
            used: 2_000,
            limit: 2_000,
            retry_at_ms: None
        })
        .contains("2000")
    );
}
