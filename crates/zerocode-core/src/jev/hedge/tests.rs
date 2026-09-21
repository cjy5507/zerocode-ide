//! What the rule promises: a delay the sample actually holds, room the second
//! copy can land in, and a refusal wherever a hedge would be a guess.

use super::*;

/// The wall a routing judgment is used inside — `DECISION_ACTIVE_DEADLINE` in
/// zo's smart router, spelled here as the number those rows were measured
/// against rather than reached for across the workspace.
const WALL: Duration = Duration::from_millis(1_500);

/// A duration as the whole milliseconds these samples are written in.
fn ms(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).expect("a duration this file spells in millis")
}

/// Every answered routing judgment this machine's ledger held on 2026-09-18,
/// in milliseconds: 22 rows, `retries: 0` on all of them, bodies of 512–1,038
/// tokens. The sample the module's arithmetic was first quoted against.
const LEDGER: [u64; 22] = [
    210, 218, 241, 250, 292, 311, 349, 473, 487, 616, 636, 641, 702, 1_020, 1_570, 1_953, 2_257,
    2_596, 3_140, 4_259, 4_847, 6_798,
];

/// A window of the recall ledger the rule still hedges: the 16 answers
/// standing behind the firing of 2026-09-21 13:30:10, which the second copy
/// won.
const ROOMY: [u64; 16] = [
    238, 242, 245, 246, 270, 271, 272, 275, 301, 316, 336, 396, 490, 529, 556, 590,
];

/// A window of the recall ledger the rule now refuses: the 17 answers behind
/// the firing of 2026-09-18 22:53:53, whose call died at the wall with both
/// copies still out (`loser_ms` 642 against a 643 ms room).
const CRAMPED: [u64; 17] = [
    229, 244, 253, 262, 319, 342, 430, 625, 643, 692, 771, 930, 1_078, 1_404, 1_494, 1_902, 2_018,
];

/// The routing sample the module was written against is not hedged at all.
///
/// Its p75 is 2,257 ms against a 1,500 ms wall: the load rank the rule may
/// spend is already past the wall, so there is no delay to name and no second
/// copy to send. The rule this replaced answered 864 ms here — the wall's
/// term, earlier than the load rank, and so a hedge on 9 of 22 calls rather
/// than the quarter `MAX_EXTRA_LOAD` promises. The routing ledger then fired
/// five times and the second copy won none of them.
#[test]
fn a_service_whose_load_rank_is_past_the_wall_is_asked_once() {
    assert_eq!(percentile(&LEDGER, 1.0 - MAX_EXTRA_LOAD), 2_257);
    assert_eq!(plan(&LEDGER, WALL), None);

    /* The delay the folded rule would have named, and the room it left: one
     * median exactly, which is the cliff ANSWERS_OF_ROOM sits above. */
    let folded = ms(WALL) - percentile(&LEDGER, 0.5);
    assert_eq!(folded, 864);
    assert_eq!(ms(WALL) - folded, percentile(&LEDGER, 0.5));
}

/// A window with room for two ordinary answers is hedged, at the load's rank.
///
/// This is a real firing: the recall ledger's 2026-09-21 13:30:10, which the
/// second copy won. The delay is the sample's own p75 and the room is 1,104 ms
/// against a 275 ms median — four ordinary answers, and the copy took one.
#[test]
fn a_window_with_room_for_two_answers_is_hedged_at_the_loads_rank() {
    let plan = plan(&ROOMY, WALL).expect("16 answers with room to land");

    assert_eq!(plan.delay, Duration::from_millis(396));
    assert_eq!(plan.delay, Duration::from_millis(percentile(&ROOMY, 0.75)));
    assert_eq!(plan.room, Duration::from_millis(1_104));
    assert!(
        ms(plan.room) >= ANSWERS_OF_ROOM * percentile(&ROOMY, 0.5),
        "room {:?} against {} answers of {} ms",
        plan.room,
        ANSWERS_OF_ROOM,
        percentile(&ROOMY, 0.5)
    );

    /* The ledger recorded 396 ms for this firing, so the replay is reading
     * the same rule the wire ran: here the load rank bound both readings. */
    assert_eq!(
        ms(plan.delay),
        u64::min(
            percentile(&ROOMY, 1.0 - MAX_EXTRA_LOAD),
            ms(WALL) - percentile(&ROOMY, 0.5)
        ),
        "the folded rule named the same delay on this window"
    );
}

/// A window with room for only one answer is not hedged, and the ledger says
/// why.
///
/// The folded rule hedged this one at 857 ms, leaving the copy 643 ms — one
/// median of a distribution whose median is 643 ms. Both copies then ran out
/// the wall: the row is a timeout carrying `loser_ms: 642`. Fourteen of this
/// machine's firings ended that way and not one of them was rescued.
#[test]
fn a_window_with_room_for_one_answer_is_asked_once() {
    assert_eq!(plan(&CRAMPED, WALL), None);

    let folded = ms(WALL) - percentile(&CRAMPED, 0.5);
    assert_eq!(folded, 857, "the delay the ledger recorded for this firing");
    let room = ms(WALL) - folded;
    assert_eq!(room, percentile(&CRAMPED, 0.5), "one median of room");
    assert!(
        room < ANSWERS_OF_ROOM * percentile(&CRAMPED, 0.5),
        "the room this rule refuses"
    );
}

/// The delay is never more than the share of requests this rule says it
/// prefers to double.
///
/// The rule this replaced took `min(load rank, wall - median)`, so the wall
/// could name a delay *earlier* than the load rank and spend past the
/// preference without saying so — it did, on 6 of this machine's 68 firings,
/// reaching 0.39 against a stated 0.25. Reading the delay from the rank alone
/// makes the promise structural.
#[test]
fn the_extra_load_never_exceeds_what_the_rule_prefers() {
    for sample in [LEDGER.as_slice(), ROOMY.as_slice(), CRAMPED.as_slice()] {
        for wall in [Duration::from_millis(400), WALL, Duration::from_secs(9)] {
            let Some(plan) = plan(sample, wall) else {
                continue;
            };
            assert!(
                plan.extra_load <= MAX_EXTRA_LOAD,
                "extra load {} on a {wall:?} wall",
                plan.extra_load
            );
            assert!(plan.delay < wall, "a hedge must leave inside the wall");
            assert_eq!(plan.room, wall - plan.delay, "the room is what is left");
        }
    }
}

/// A delay is a value the sample took, never one interpolated between two.
///
/// A judgment waits on a wire, and a rank the sample holds is a wait some
/// answer really needed; a number between two answers is a wait nothing ever
/// took. Nearest rank keeps the delay honest.
#[test]
fn the_delay_is_always_an_answer_the_sample_held() {
    let plan = plan(&ROOMY, WALL).expect("a plan");
    let delay = u64::try_from(plan.delay.as_millis()).expect("millis");
    assert!(
        ROOMY.contains(&delay),
        "{delay} is no answer of this sample"
    );
}

/// Too few answers is not a small sample — it is no sample.
///
/// The published adaptive design names low traffic as one of the places
/// hedging fails, so the rule declines rather than reading a percentile off
/// three numbers.
#[test]
fn a_sample_too_small_to_hold_a_rank_is_asked_once() {
    for count in 0..MIN_SAMPLES {
        let sample: Vec<u64> = ROOMY.iter().copied().take(count).collect();
        assert_eq!(plan(&sample, WALL), None, "{count} answers named a rank");
    }
    let enough: Vec<u64> = ROOMY.iter().copied().take(MIN_SAMPLES).collect();
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

    /* The boundary: a median exactly at the wall leaves no room at all. */
    let at_the_wall = [1_500_u64; 8];
    assert_eq!(plan(&at_the_wall, WALL), None);
}

/// A fast service is hedged on its own rank, and has room to spare.
#[test]
fn a_fast_service_hedges_on_its_own_rank() {
    let quick = [40_u64, 45, 50, 55, 60, 65, 70, 400];
    let plan = plan(&quick, WALL).expect("a plan");
    /* p75 of eight answers is the sixth by nearest rank. */
    assert_eq!(plan.delay, Duration::from_millis(65));
    assert_eq!(plan.room, Duration::from_millis(1_435));
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
        if let Some(plan) = plan(&ROOMY, wall) {
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

/// What independence would buy, on a window the rule does hedge — kept as the
/// upper bound it is now known to be (see [`cleared_share`]).
#[test]
fn the_independent_reading_is_an_upper_bound_the_ledger_beat_down() {
    let plan = plan(&ROOMY, WALL).expect("a plan");
    let (once, twice) = cleared_share(&ROOMY, WALL, Some(&plan));
    assert!((once - 1.0).abs() < f64::EPSILON, "once {once}");
    assert!((twice - 1.0).abs() < f64::EPSILON, "twice {twice}");

    /* And on a window it refuses, the arithmetic still promises a gain — the
     * promise the 14 timeouts disproved. This is the number the module's own
     * doc now reads as a bound rather than a forecast. */
    let folded = HedgePlan {
        delay: Duration::from_millis(857),
        room: Duration::from_millis(643),
        extra_load: 0.0,
    };
    let (once, twice) = cleared_share(&CRAMPED, WALL, Some(&folded));
    assert!((once - 15.0 / 17.0).abs() < 1e-12, "once {once}");
    assert!(
        twice > once,
        "independence expected a gain here: {once} -> {twice}"
    );
}

/// The order answers arrive in is not part of the rule.
#[test]
fn the_sample_is_read_as_a_set_not_a_sequence() {
    let mut shuffled = ROOMY;
    shuffled.reverse();
    assert_eq!(plan(&shuffled, WALL), plan(&ROOMY, WALL));
}

/// Every hedge this machine has fired, replayed against the rule.
///
/// The seed is `tools/hedge-replay/seed.py`'s, which only *reads* the ledgers
/// — it names no delay and counts no win, because the arithmetic is
/// [`plan`]'s and lives here (the discipline `tools/summon-replay` keeps).
/// Each firing carries the sample the wire read at that moment, the delay it
/// actually used, and what became of the call, so the "before" column is the
/// ledger's own record rather than a second copy of the old rule.
///
/// Ignored because it needs a person's ledgers:
///
/// ```sh
/// python3 tools/hedge-replay/seed.py --out /tmp/hedge-replay/seed.json
/// ZEROCODE_HEDGE_REPLAY_SEED=/tmp/hedge-replay/seed.json \
///   cargo test -p zerocode-core jev::hedge::tests::the_firings_that_already_happened \
///   -- --ignored --nocapture
/// ```
#[test]
#[ignore = "reads this machine's Jev ledgers through tools/hedge-replay/seed.py"]
fn the_firings_that_already_happened() {
    let Ok(path) = std::env::var("ZEROCODE_HEDGE_REPLAY_SEED") else {
        panic!("ZEROCODE_HEDGE_REPLAY_SEED names the seed to replay");
    };
    let text = std::fs::read_to_string(&path).expect("the seed");
    let seed: serde_json::Value = serde_json::from_str(&text).expect("seed json");
    let wall = Duration::from_millis(seed["wallMs"].as_u64().expect("wallMs"));
    let firings = seed["firings"].as_array().expect("firings");

    let mut tally: std::collections::BTreeMap<(String, bool), [usize; 4]> = Default::default();
    for firing in firings {
        let seat = firing["seat"].as_str().unwrap_or("?").to_string();
        let samples: Vec<u64> = firing["samples"]
            .as_array()
            .expect("samples")
            .iter()
            .filter_map(serde_json::Value::as_u64)
            .collect();
        let missed = firing["outcome"].as_str() == Some("timeout");
        let won = firing["won"].as_bool().unwrap_or(false);
        // The ledger's own delay is the "before": what the wire really did.
        let before = firing["recordedDelayMs"].is_u64();
        let after = plan(&samples, wall).is_some();
        for (arm, fires) in [(false, before), (true, after)] {
            if !fires {
                continue;
            }
            let row = tally.entry((seat.clone(), arm)).or_default();
            row[0] += 1;
            row[1] += usize::from(won);
            row[2] += usize::from(missed);
            row[3] += usize::from(!won && !missed);
        }
    }
    println!("seat            arm     fires  won  first  both-died");
    for ((seat, arm), row) in &tally {
        let arm = if *arm { "now" } else { "was" };
        println!(
            "{seat:14} {arm:6} {:6} {:4} {:6} {:10}",
            row[0], row[1], row[3], row[2]
        );
    }
    assert!(!tally.is_empty(), "the seed held no firings");
}
