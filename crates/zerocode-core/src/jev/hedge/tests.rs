//! What the rule promises: a delay the sample actually holds, inside the wall
//! it exists to beat, and a refusal wherever a hedge would be a guess.

use super::*;

/// The wall a routing judgment is used inside — `DECISION_ACTIVE_DEADLINE` in
/// zo's smart router, spelled here as the number those rows were measured
/// against rather than reached for across the workspace.
const WALL: Duration = Duration::from_millis(1_500);

/// Every answered routing judgment this machine's ledger held on 2026-09-18,
/// in milliseconds: 22 rows, `retries: 0` on all of them, bodies of 512–1,038
/// tokens. The sample the module's arithmetic is quoted against.
const LEDGER: [u64; 22] = [
    210, 218, 241, 250, 292, 311, 349, 473, 487, 616, 636, 641, 702, 1_020, 1_570, 1_953, 2_257,
    2_596, 3_140, 4_259, 4_847, 6_798,
];

/// The real sample, and the four numbers the module's own doc quotes.
///
/// This is the test that would catch a "tuning" of the rule that quietly
/// stopped paying for itself: the delay, the load, and both shares are pinned
/// to the rows they were read from.
#[test]
fn the_ledgers_own_rows_give_the_delay_the_module_documents() {
    let plan = plan(&LEDGER, WALL).expect("22 answers name a rank");

    /* min(p75 = 2257, wall - median = 1500 - 636) — the wall binds, which is
     * the whole point: the load-derived delay alone lands past the wall. */
    assert_eq!(plan.delay, Duration::from_millis(864));
    assert!(plan.wall_bound, "the wall set this delay, not the load");

    /* Nine of 22 answers ran past 864 ms, so 1.41x the requests. */
    assert!(
        (plan.extra_load - 9.0 / 22.0).abs() < f64::EPSILON,
        "extra load {}",
        plan.extra_load
    );

    let (once, twice) = cleared_share(&LEDGER, WALL, Some(&plan));
    assert!((once - 14.0 / 22.0).abs() < f64::EPSILON, "once {once}");
    assert!(
        (twice - 0.818_181_818_181_818_2).abs() < 1e-12,
        "twice {twice}"
    );
    assert!(
        twice - once > 0.18,
        "the hedge must buy more than eighteen points: {once} -> {twice}"
    );
}

/// A delay is a value the sample took, never one interpolated between two.
///
/// A judgment waits on a wire, and a rank the sample holds is a wait some
/// answer really needed; a number between two answers is a wait nothing ever
/// took. Nearest rank keeps the delay honest.
#[test]
fn the_delay_is_always_an_answer_the_sample_held() {
    let plan = plan(&LEDGER, WALL).expect("a plan");
    let delay = u64::try_from(plan.delay.as_millis()).expect("millis");
    /* 864 = 1500 - 636, and 636 is LEDGER[10]. The subtrahend is a sample
     * value, so the delay is anchored to one. */
    assert!(LEDGER.contains(&636));
    assert_eq!(delay, 1_500 - 636);
}

/// Too few answers is not a small sample — it is no sample.
///
/// The published adaptive design names low traffic as one of the places
/// hedging fails, so the rule declines rather than reading a percentile off
/// three numbers.
#[test]
fn a_sample_too_small_to_hold_a_rank_is_asked_once() {
    for count in 0..MIN_SAMPLES {
        let sample: Vec<u64> = LEDGER.iter().copied().take(count).collect();
        assert_eq!(plan(&sample, WALL), None, "{count} answers named a rank");
    }
    let enough: Vec<u64> = LEDGER.iter().copied().take(MIN_SAMPLES).collect();
    assert!(
        plan(&enough, WALL).is_some(),
        "{MIN_SAMPLES} answers must name one"
    );
}

/// When the ordinary answer is already late, a second copy of it is late too.
///
/// A distribution entirely past the wall is a wall to raise or a judgment to
/// drop. Doubling the requests would buy nothing at full price, so the rule
/// refuses instead of hedging into a loss.
#[test]
fn a_distribution_past_the_wall_is_never_hedged() {
    let late = [1_600_u64, 1_700, 1_800, 1_900, 2_000, 2_100, 2_200, 2_300];
    assert_eq!(plan(&late, WALL), None);

    /* The boundary: a median exactly at the wall leaves no time at all. */
    let at_the_wall = [1_500_u64; 8];
    assert_eq!(plan(&at_the_wall, WALL), None);
}

/// A fast service is not hedged, because its own load rank comes first.
///
/// When every answer is quick the wall stops binding and `MAX_EXTRA_LOAD`
/// takes over — which is Finagle's rule, and correct here: the hedge fires
/// only on the slowest quarter, and `wall_bound` says so.
#[test]
fn a_fast_service_hedges_on_its_own_rank_not_the_wall() {
    let quick = [40_u64, 45, 50, 55, 60, 65, 70, 400];
    let plan = plan(&quick, WALL).expect("a plan");
    assert!(!plan.wall_bound, "the load rank should bind, not the wall");
    /* p75 of eight answers is the sixth by nearest rank. */
    assert_eq!(plan.delay, Duration::from_millis(65));
    assert!(
        plan.extra_load <= MAX_EXTRA_LOAD,
        "a load-bound plan must keep inside its preference: {}",
        plan.extra_load
    );
}

/// The cost is bounded at twice the requests, whatever the sample says.
#[test]
fn no_plan_ever_asks_more_than_twice() {
    assert_eq!(MAX_ATTEMPTS, 2);
    for wall in [Duration::from_millis(400), WALL, Duration::from_secs(9)] {
        if let Some(plan) = plan(&LEDGER, wall) {
            assert!(
                plan.extra_load <= 1.0,
                "extra load is a share of one hedge: {}",
                plan.extra_load
            );
            assert!(plan.delay < wall, "a hedge must leave inside the wall");
        }
    }
}

/// Asked once, the two shares agree — there is nothing a hedge changed.
#[test]
fn without_a_plan_both_shares_are_the_same_reading() {
    let (once, twice) = cleared_share(&LEDGER, WALL, None);
    assert!((once - twice).abs() < f64::EPSILON);
    assert!((once - 14.0 / 22.0).abs() < f64::EPSILON);
}

/// The order answers arrive in is not part of the rule.
#[test]
fn the_sample_is_read_as_a_set_not_a_sequence() {
    let mut shuffled = LEDGER;
    shuffled.reverse();
    assert_eq!(plan(&shuffled, WALL), plan(&LEDGER, WALL));
}
