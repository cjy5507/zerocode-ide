use super::*;

const NOW: i64 = 1_800_000_000_000;
const MINUTE: i64 = 60_000;

fn limits(concurrent: Option<u32>, per_hour: Option<u32>, per_day: Option<u32>) -> Limits {
    Limits {
        concurrent,
        per_hour,
        per_day,
    }
}

fn unlimited() -> Limits {
    limits(None, None, None)
}

fn plain(provider: &str) -> Ask<'_> {
    Ask {
        provider,
        job: None,
        fresh_ms: None,
    }
}

fn job<'a>(provider: &'a str, key: &'a str, fresh_ms: Option<i64>) -> Ask<'a> {
    Ask {
        provider,
        job: Some(key),
        fresh_ms,
    }
}

/// The three published FNV-1a vectors (the empty string, `a` and `foobar`):
/// the job key is persisted, so the hash under it must be the specified one
/// and not whatever std's hasher is this release.
#[test]
fn the_job_hash_is_fnv1a_as_published() {
    assert_eq!(fnv1a("".bytes()), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a("a".bytes()), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a("foobar".bytes()), 0x8594_4171_f739_67e8);
}

#[test]
fn a_job_key_is_stable_and_every_part_of_the_job_changes_it() {
    let base = job_key("repo-a", "abc123", "draft", "cfg-1");
    assert_eq!(base.len(), 16, "sixteen hex characters: {base}");
    assert_eq!(base, job_key("repo-a", "abc123", "draft", "cfg-1"));
    for (name, other) in [
        ("repo", job_key("repo-b", "abc123", "draft", "cfg-1")),
        ("head", job_key("repo-a", "abc124", "draft", "cfg-1")),
        ("job", job_key("repo-a", "abc123", "review", "cfg-1")),
        ("config", job_key("repo-a", "abc123", "draft", "cfg-2")),
    ] {
        assert_ne!(base, other, "a different {name} is a different job");
    }
    assert_ne!(
        job_key("ab", "c", "", ""),
        job_key("a", "bc", "", ""),
        "where one part ends and the next begins is part of the key"
    );
}

#[test]
fn the_hourly_ceiling_counts_launches_over_a_sliding_hour() {
    let mut ledger = LaunchLedger::default();
    let limits = limits(None, Some(3), None);
    for minute in 0..3 {
        let permit = ledger
            .reserve(&plain("claude"), &limits, NOW + minute * MINUTE)
            .expect("under the ceiling");
        ledger.finish(permit, Outcome::Done, NOW + minute * MINUTE);
    }
    let refused = ledger
        .reserve(&plain("claude"), &limits, NOW + 3 * MINUTE)
        .expect_err("the fourth launch of the hour");
    assert_eq!(
        refused,
        Refusal::Hourly {
            used: 3,
            limit: 3,
            retry_at_ms: Some(NOW + MS_PER_HOUR),
        },
        "the hour frees a place when its oldest launch ages out"
    );
    assert!(
        ledger
            .reserve(&plain("claude"), &limits, NOW + MS_PER_HOUR)
            .is_ok(),
        "a launch exactly one hour old is out of the window"
    );
}

#[test]
fn the_daily_ceiling_holds_after_the_hourly_one_has_let_go() {
    let mut ledger = LaunchLedger::default();
    let limits = limits(None, None, Some(2));
    for hours in [0, 2] {
        let at = NOW + hours * MS_PER_HOUR;
        let permit = ledger
            .reserve(&plain("claude"), &limits, at)
            .expect("under the ceiling");
        ledger.finish(permit, Outcome::Done, at);
    }
    let refused = ledger
        .reserve(&plain("claude"), &limits, NOW + 3 * MS_PER_HOUR)
        .expect_err("the third launch of the day");
    assert_eq!(
        refused,
        Refusal::Daily {
            used: 2,
            limit: 2,
            retry_at_ms: Some(NOW + MS_PER_DAY),
        }
    );
    assert!(
        ledger
            .reserve(&plain("claude"), &limits, NOW + MS_PER_DAY)
            .is_ok()
    );
}

#[test]
fn a_ceiling_of_nothing_never_frees_and_no_ceiling_never_refuses() {
    let mut ledger = LaunchLedger::default();
    assert_eq!(
        ledger
            .reserve(&plain("claude"), &limits(None, Some(0), None), NOW)
            .expect_err("nothing may launch"),
        Refusal::Hourly {
            used: 0,
            limit: 0,
            retry_at_ms: None,
        }
    );
    for step in 0..50 {
        let permit = ledger
            .reserve(&plain("claude"), &unlimited(), NOW + step)
            .expect("no ceiling");
        ledger.finish(permit, Outcome::Done, NOW + step);
    }
}

#[test]
fn a_finished_launch_frees_its_place_but_the_hour_still_counts_it() {
    let mut ledger = LaunchLedger::default();
    let limits = limits(Some(1), Some(2), None);
    let first = ledger
        .reserve(&plain("claude"), &limits, NOW)
        .expect("the first");
    assert_eq!(
        ledger
            .reserve(&plain("claude"), &limits, NOW)
            .expect_err("one place, held"),
        Refusal::Concurrent {
            active: 1,
            limit: 1,
        }
    );
    ledger.finish(first, Outcome::Done, NOW + MINUTE);
    let second = ledger
        .reserve(&plain("claude"), &limits, NOW + MINUTE)
        .expect("the place is free");
    ledger.finish(second, Outcome::Done, NOW + 2 * MINUTE);
    assert!(
        matches!(
            ledger.reserve(&plain("claude"), &limits, NOW + 2 * MINUTE),
            Err(Refusal::Hourly { used: 2, .. })
        ),
        "two launches in the hour, however fast they ended"
    );
}

#[test]
fn a_wall_rests_its_provider_until_its_time_and_no_other() {
    let mut ledger = LaunchLedger::default();
    let permit = ledger
        .reserve(&plain("claude"), &unlimited(), NOW)
        .expect("before the wall");
    ledger.finish(
        permit,
        Outcome::Rested {
            until_ms: NOW + 30 * MINUTE,
        },
        NOW + MINUTE,
    );
    assert_eq!(
        ledger
            .reserve(&plain("claude"), &unlimited(), NOW + 2 * MINUTE)
            .expect_err("the provider rests"),
        Refusal::Resting {
            until_ms: NOW + 30 * MINUTE,
        }
    );
    assert!(
        ledger
            .reserve(&plain("codex"), &unlimited(), NOW + 2 * MINUTE)
            .is_ok(),
        "another provider's door stays open"
    );
    assert!(
        ledger
            .reserve(&plain("claude"), &unlimited(), NOW + 30 * MINUTE)
            .is_ok(),
        "the wall stops standing at its time"
    );
}

#[test]
fn a_second_wall_never_shortens_the_rest() {
    let mut ledger = LaunchLedger::default();
    // Two launches in flight when the walls fall; the first wall stands longer.
    let first = ledger
        .reserve(&plain("claude"), &unlimited(), NOW)
        .expect("the first");
    let second = ledger
        .reserve(&plain("claude"), &unlimited(), NOW)
        .expect("the second");
    ledger.finish(
        first,
        Outcome::Rested {
            until_ms: NOW + 60 * MINUTE,
        },
        NOW + MINUTE,
    );
    ledger.finish(
        second,
        Outcome::Rested {
            until_ms: NOW + 10 * MINUTE,
        },
        NOW + MINUTE,
    );
    assert_eq!(
        ledger.counters(NOW + 2 * MINUTE).resting,
        vec![Resting {
            provider: "claude".into(),
            until_ms: NOW + 60 * MINUTE,
        }]
    );
}

#[test]
fn the_same_job_is_refused_while_it_runs_and_while_its_answer_stands() {
    let mut ledger = LaunchLedger::default();
    let key = job_key("repo-a", "abc123", "draft", "cfg");
    let fresh = Some(10 * MINUTE);
    let first = ledger
        .reserve(&job("claude", &key, fresh), &unlimited(), NOW)
        .expect("the first run");
    assert_eq!(
        ledger
            .reserve(&job("claude", &key, fresh), &unlimited(), NOW + MINUTE)
            .expect_err("it is running"),
        Refusal::Running
    );
    let other = job_key("repo-a", "abc124", "draft", "cfg");
    let other = ledger
        .reserve(&job("claude", &other, fresh), &unlimited(), NOW + MINUTE)
        .expect("another commit is another job");
    ledger.finish(other, Outcome::Failed, NOW + MINUTE);
    ledger.finish(first, Outcome::Done, NOW + MINUTE);
    assert_eq!(
        ledger
            .reserve(&job("claude", &key, fresh), &unlimited(), NOW + 5 * MINUTE)
            .expect_err("its answer stands"),
        Refusal::Fresh { age_ms: 4 * MINUTE }
    );
    assert!(
        ledger
            .reserve(&job("claude", &key, fresh), &unlimited(), NOW + 11 * MINUTE)
            .is_ok(),
        "the answer went stale"
    );
}

#[test]
fn a_caller_that_cannot_reuse_an_answer_is_refused_only_while_the_job_runs() {
    let mut ledger = LaunchLedger::default();
    let key = job_key("repo-a", "abc123", "draft", "cfg");
    let first = ledger
        .reserve(&job("claude", &key, None), &unlimited(), NOW)
        .expect("the first run");
    ledger.finish(first, Outcome::Done, NOW + MINUTE);
    assert!(
        ledger
            .reserve(&job("claude", &key, None), &unlimited(), NOW + MINUTE)
            .is_ok(),
        "a regenerated draft is a new draft"
    );
}

#[test]
fn a_failed_run_does_not_stand_as_an_answer() {
    let mut ledger = LaunchLedger::default();
    let key = job_key("repo-a", "abc123", "draft", "cfg");
    let fresh = Some(10 * MINUTE);
    let first = ledger
        .reserve(&job("claude", &key, fresh), &unlimited(), NOW)
        .expect("the first run");
    ledger.finish(first, Outcome::Failed, NOW + MINUTE);
    assert!(
        ledger
            .reserve(&job("claude", &key, fresh), &unlimited(), NOW + 2 * MINUTE)
            .is_ok()
    );
}

#[test]
fn a_launch_that_never_finished_lapses_instead_of_tightening_the_ceiling_forever() {
    let mut ledger = LaunchLedger::default();
    let limits = limits(Some(1), None, None);
    let _lost = ledger
        .reserve(&plain("claude"), &limits, NOW)
        .expect("the first");
    assert!(
        ledger
            .reserve(&plain("claude"), &limits, NOW + MINUTE)
            .is_err()
    );
    assert!(
        ledger
            .reserve(&plain("claude"), &limits, NOW + PERMIT_STALE_MS)
            .is_ok(),
        "the window that held the place is long gone"
    );
}

#[test]
fn the_ledger_outlives_the_process_but_a_running_launch_does_not() {
    let mut ledger = LaunchLedger::default();
    let key = job_key("repo-a", "abc123", "draft", "cfg");
    let fresh = Some(10 * MINUTE);
    let done = ledger
        .reserve(&job("claude", &key, fresh), &unlimited(), NOW)
        .expect("a run");
    ledger.finish(done, Outcome::Done, NOW + MINUTE);
    let walled = ledger
        .reserve(&plain("codex"), &unlimited(), NOW + MINUTE)
        .expect("a run that meets a wall");
    ledger.finish(
        walled,
        Outcome::Rested {
            until_ms: NOW + 40 * MINUTE,
        },
        NOW + 2 * MINUTE,
    );
    let _running = ledger
        .reserve(&plain("claude"), &unlimited(), NOW + 3 * MINUTE)
        .expect("one still running");

    let kept: LaunchLedger =
        serde_json::from_str(&serde_json::to_string(&ledger).expect("serializes"))
            .expect("reads back");
    let counters = kept.counters(NOW + 4 * MINUTE);
    assert_eq!(counters.last_hour, 3, "the launches are kept");
    assert_eq!(counters.active, 0, "a running launch does not survive");
    assert_eq!(counters.resting.len(), 1, "the wall is kept");
    let mut kept = kept;
    assert_eq!(
        kept.reserve(&job("claude", &key, fresh), &unlimited(), NOW + 4 * MINUTE)
            .expect_err("the finished job's answer is kept"),
        Refusal::Fresh { age_ms: 3 * MINUTE }
    );
}

#[test]
fn what_the_ledger_remembers_is_bounded() {
    let mut ledger = LaunchLedger::default();
    for at in 0..i64::try_from(LAUNCHES_KEPT_MAX + 100).expect("fits") {
        let permit = ledger
            .reserve(&plain("claude"), &unlimited(), NOW + at)
            .expect("no ceiling");
        ledger.finish(permit, Outcome::Done, NOW + at);
    }
    assert!(ledger.launches.len() <= LAUNCHES_KEPT_MAX + 1);
    for index in 0..DONE_KEPT_MAX + 50 {
        let key = job_key("repo", &index.to_string(), "draft", "cfg");
        let at = NOW + i64::try_from(index).expect("fits");
        let permit = ledger
            .reserve(&job("claude", &key, Some(MINUTE)), &unlimited(), at)
            .expect("a new job");
        ledger.finish(permit, Outcome::Done, at);
    }
    assert!(ledger.done.len() <= DONE_KEPT_MAX);
}

#[test]
fn counters_say_what_is_held_and_which_walls_still_stand() {
    let mut ledger = LaunchLedger::default();
    let permit = ledger
        .reserve(&plain("claude"), &unlimited(), NOW)
        .expect("a run");
    let _held = ledger
        .reserve(&plain("claude"), &unlimited(), NOW + MINUTE)
        .expect("another");
    ledger.finish(
        permit,
        Outcome::Rested {
            until_ms: NOW + 5 * MINUTE,
        },
        NOW + MINUTE,
    );
    let counters = ledger.counters(NOW + 2 * MINUTE);
    assert_eq!(counters.active, 1);
    assert_eq!(counters.last_hour, 2);
    assert_eq!(counters.last_day, 2);
    assert_eq!(counters.resting.len(), 1);
    assert!(
        ledger.counters(NOW + 6 * MINUTE).resting.is_empty(),
        "a wall that stopped standing is not said"
    );
}

#[test]
fn the_ceilings_default_and_a_settings_file_may_leave_any_of_them_out_or_lift_it() {
    assert_eq!(
        serde_json::from_str::<Limits>("{}").expect("empty"),
        Limits::default()
    );
    let lifted = serde_json::from_str::<Limits>(r#"{"per_hour":null}"#).expect("lifted");
    assert_eq!(lifted.per_hour, None);
    assert_eq!(lifted.concurrent, Some(DEFAULT_CONCURRENT));
    assert_eq!(lifted.per_day, Some(DEFAULT_PER_DAY));
}

#[test]
fn every_refusal_has_its_own_word() {
    let words: std::collections::BTreeSet<_> = [
        Refusal::Resting { until_ms: 0 },
        Refusal::Running,
        Refusal::Fresh { age_ms: 0 },
        Refusal::Concurrent {
            active: 0,
            limit: 0,
        },
        Refusal::Hourly {
            used: 0,
            limit: 0,
            retry_at_ms: None,
        },
        Refusal::Daily {
            used: 0,
            limit: 0,
            retry_at_ms: None,
        },
    ]
    .iter()
    .map(Refusal::word)
    .collect();
    assert_eq!(words.len(), 6);
}
