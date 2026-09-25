//! What a forked step may do, and what it may not (t-6044).

use std::collections::BTreeMap;

use serde_json::{Value, json};
use zerocode_core::branching::{BranchChoice, NextStep};
use zerocode_core::computer_recipe::RecipeStop;
use zerocode_core::jev::summary::{AGREED, CACHED};
use zerocode_core::screen_action::{ActionChoice, ActionRead, Chosen, option_of};

use super::super::tests::{FakeJudge, FakeWorld, goal, pick, stopped};
use super::super::{
    Barred, Judged, Mode, OVERLAP, OVERLAP_USED, Options, RESCUE, RESCUED_BY, RESCUED_BY_TEAM, run,
    run_with,
};
use super::{Branching, Compared};

const SHADOW: Branching = Branching {
    mode: Mode::Shadow,
    acting: false,
    act_line: None,
};
const RAISED: Branching = Branching {
    mode: Mode::Auto,
    acting: true,
    act_line: None,
};
const UNRAISED: Branching = Branching {
    mode: Mode::Auto,
    acting: false,
    act_line: None,
};

/// The screen seat's answer: `chosen` first, then the others by weight.
fn ranked(chosen: usize, spread: &[(usize, f64)]) -> Judged {
    let Judged::Chose(ActionRead { mut choice, .. }) = pick(chosen) else {
        unreachable!()
    };
    choice.probabilities = spread
        .iter()
        .map(|(mark, weight)| (option_of(*mark), *weight))
        .collect();
    choice.probabilities.insert("give_up".to_string(), 0.0);
    Judged::Chose(choice.into())
}

/// A comparison that names `mark` at `confidence`.
fn compared(mark: usize, confidence: f64) -> Compared {
    Compared::Chose(BranchChoice {
        mark,
        probabilities: BTreeMap::new(),
        confidence,
    })
}

fn control(mark: usize, label: &str) -> Value {
    json!({
        "mark": mark,
        "role": "button",
        "label": label,
        "centerX": 100.0 + mark as f64,
        "centerY": 40.0,
    })
}

/// A phone showing two controls, where pressing 2 leads to a screen with
/// nothing new and pressing 1 leads to the goal's screen.
fn phone() -> FakeWorld {
    let mut world = FakeWorld::android(&[1, 2]);
    world.leads_to.insert(
        1,
        vec![
            control(1, "Wi-Fi"),
            control(2, "Bluetooth"),
            control(3, "연결됨"),
        ],
    );
    world
}

/// A judge whose press ranks 2 first and 1 second, torn between them —
/// inside the table's margin (`BRANCHING_FORK_MARGIN_PERMILLE`), so the
/// step is one a fork is for.
fn judging() -> FakeJudge {
    let mut judge = FakeJudge::chose(&[]);
    judge.answers = vec![ranked(2, &[(2, 0.55), (1, 0.45)])];
    judge
}

/// The rows of a walk with the clock's stamps taken off, so two walks can
/// be compared word for word.
fn unstamped(rows: &[Value]) -> Vec<Value> {
    rows.iter()
        .map(|row| {
            let mut row = row.clone();
            if let Some(row) = row.as_object_mut() {
                row.remove("at");
                row.remove("elapsedMs");
            }
            row
        })
        .collect()
}

#[test]
fn off_is_todays_walk_byte_for_byte() {
    let mut plain_world = phone();
    let mut plain_judge = judging();
    let plain = run(Mode::On, true, &goal(1), &mut plain_judge, &mut plain_world);

    for off in [
        Branching::OFF,
        Branching {
            mode: Mode::Off,
            acting: true,
            act_line: None,
        },
    ] {
        let mut world = phone();
        let mut judge = judging();
        let walked = run_with(
            Mode::On,
            true,
            off,
            &goal(1),
            &mut judge,
            &mut world,
            Options::default(),
            None,
        );
        assert_eq!(unstamped(&walked.rows), unstamped(&plain.rows), "{off:?}");
        assert!(walked.forks.is_empty(), "{off:?}");
        assert_eq!(world.presses, plain_world.presses, "{off:?}");
        assert!(world.snapshot_log.is_empty(), "{off:?}");
        assert!(judge.compared.is_empty(), "{off:?}: nothing is asked");
    }
    assert_eq!(
        plain_world.presses,
        [2],
        "today presses the seat's first choice"
    );
}

#[test]
fn shadow_asks_over_actions_alone_and_presses_todays_number() {
    for recording in [SHADOW, UNRAISED] {
        let mut world = phone();
        let mut judge = judging();
        judge.compares = vec![compared(1, 0.9)];

        let walked = run_with(
            Mode::On,
            true,
            recording,
            &goal(1),
            &mut judge,
            &mut world,
            Options::default(),
            None,
        );

        assert_eq!(
            world.presses,
            [2],
            "{recording:?}: today's number is pressed"
        );
        assert!(
            world.snapshot_log.is_empty(),
            "{recording:?}: nothing is saved"
        );
        assert_eq!(judge.compared, [vec![2, 1]], "{recording:?}");
        let asked = &judge.compared_state[0];
        assert!(
            asked["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .all(|candidate| candidate.get("result").is_none()),
            "{recording:?}: the question carries actions alone:\n{asked}"
        );
        assert_eq!(asked["where"]["platform"], "android");
        assert_eq!(walked.forks.len(), 1, "{recording:?}");
        let row = &walked.forks[0];
        assert_eq!(row["mode"], json!(recording.mode.key()));
        assert_eq!(row["explored"], 0);
        assert_eq!(row["today"], "mark:2");
        assert_eq!(row["chosen"], "mark:1");
        assert_eq!(row["candidates"], 2);
        assert_eq!(row["routeUse"], "shadow");
        assert_eq!(row["reason"], "seat_recording");
        assert_eq!(row["platform"], "android");
        // Neither the device's name nor the goal's words are in the row
        // (t-6155 F12): their fingerprints are.
        assert_eq!(
            row["deviceFingerprint"],
            zerocode_core::jev::fingerprint_of("Pixel_6")
        );
        assert!(row.get("device").is_none() && row.get("flow").is_none());
        assert!(row["flowFingerprint"].is_string());
        let printed = row.to_string();
        assert!(
            !printed.contains("Pixel_6") && !printed.contains(goal(1).goal),
            "{printed}"
        );
        assert!(row["branching"].is_string() && row["at"].is_i64());
        assert!(
            walked.rows[0].get("forked").is_none(),
            "the screen seat's row pressed its own number"
        );
    }
}

#[test]
fn an_acting_seat_saves_tries_each_candidate_and_presses_the_comparisons_pick() {
    let mut world = phone();
    let mut judge = judging();
    judge.compares = vec![compared(1, 0.8)];

    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(1),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );

    assert_eq!(
        world.presses,
        [2, 1, 1],
        "each candidate once on the saved device, then the pick"
    );
    assert_eq!(
        world.snapshot_log,
        [
            ("save", "fake-0".to_string()),
            ("load", "fake-0".to_string()),
            ("load", "fake-0".to_string()),
            ("forget", "fake-0".to_string()),
        ],
        "one save, a load after every candidate, one forget"
    );
    assert_eq!(judge.compared, [vec![2, 1]]);
    let asked = &judge.compared_state[0];
    let candidates = asked["candidates"].as_array().unwrap();
    assert_eq!(candidates[0]["option"], "mark:2");
    assert_eq!(
        candidates[0]["result"]["moved"], false,
        "pressing 2 led nowhere"
    );
    assert_eq!(candidates[1]["result"]["moved"], true);
    assert_eq!(candidates[1]["result"]["count"], 3);
    assert!(
        candidates[1]["result"]["controls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().unwrap().contains("연결됨"))
    );
    assert_eq!(asked["before"].as_array().unwrap().len(), 2);
    let row = &walked.forks[0];
    assert_eq!(row["explored"], 2);
    assert_eq!(row["routeUse"], "applied");
    assert_eq!(row["chosen"], "mark:1");
    assert_eq!(row["today"], "mark:2");
    assert_eq!(row["outcome"], "answered");
    assert_eq!(row["confidence"], 0.8);
    assert_eq!(row["stepMs"].as_array().unwrap().len(), 2);
    assert_eq!(row["restoreMs"].as_array().unwrap().len(), 2);
    assert!(row["saveMs"].is_u64() && row["forkMs"].is_u64() && row["budgetMs"].is_u64());
    assert_eq!(
        walked.rows[0]["forked"], "mark:1",
        "the screen seat's row says which number the hand went out with"
    );
    assert_eq!(
        walked.rows[0]["chosen"], "mark:2",
        "and keeps its own answer"
    );
    // The exploratory presses carry the fork's context, the pick the walk's.
    let contexts: Vec<Value> = world
        .observed
        .iter()
        .map(|seen: &Option<Value>| seen.clone().unwrap_or(Value::Null))
        .collect();
    assert_eq!(contexts[0]["fork"]["candidate"], 0);
    assert_eq!(contexts[0]["fork"]["of"], 2);
    assert_eq!(contexts[1]["fork"]["mark"], 1);
    assert_eq!(contexts[2]["judgment"]["asked"], true);
}

#[test]
fn a_low_confidence_comparison_and_a_refusal_press_the_first_candidate() {
    for (answer, outcome, barred) in [
        (Some(compared(1, 0.3)), "answered", Some("low_confidence")),
        (None, "timeout", None),
    ] {
        let mut world = phone();
        let mut judge = judging();
        judge.compares = answer.into_iter().collect();

        let walked = run_with(
            Mode::On,
            true,
            RAISED,
            &goal(1),
            &mut judge,
            &mut world,
            Options::default(),
            None,
        );

        assert_eq!(
            world.presses,
            [2, 1, 2],
            "{outcome}: explored, then today's"
        );
        let row = &walked.forks[0];
        assert_eq!(row["outcome"], outcome);
        assert_eq!(row["routeUse"], "fallback");
        assert_eq!(row.get("barred").and_then(Value::as_str), barred);
        assert!(
            walked.rows[0].get("forked").is_none(),
            "{outcome}: the hand went out with the seat's own number"
        );
        assert_eq!(
            world.snapshot_log.last(),
            Some(&("forget", "fake-0".to_string())),
            "{outcome}: the saved state is let go of"
        );
    }
}

#[test]
fn a_device_that_cannot_be_saved_steps_back_to_a_single_press() {
    // An iOS simulator, and an Android device whose save failed, both
    // answer the walk with no saved state.
    let mut world = phone();
    world.snapshots = None;
    let mut judge = judging();
    judge.compares = vec![compared(1, 0.9)];

    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(1),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );

    assert_eq!(world.presses, [2]);
    assert!(
        judge.compared.is_empty(),
        "nothing explored, nothing compared"
    );
    assert_eq!(walked.forks[0]["barred"], "no_snapshot");
    assert_eq!(walked.forks[0]["routeUse"], "fallback");
    assert_eq!(walked.forks[0]["explored"], 0);
}

#[test]
fn a_page_or_the_desktop_is_never_forked() {
    let mut world = FakeWorld::showing(&[1, 2]);
    world.snapshots = Some(0);
    let mut judge = judging();
    judge.compares = vec![compared(1, 0.9)];

    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(1),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );

    assert_eq!(world.presses, [2]);
    assert!(walked.forks.is_empty(), "the seat is a phone step's");
    assert!(world.snapshot_log.is_empty());
}

#[test]
fn a_fork_without_clock_enough_presses_the_first_candidate_at_once() {
    let mut world = phone();
    world.snapshots = Some(1_000);
    world.look_ms = 1_000;
    // After the look and the save, three rounds of a step and a load plus
    // the wall is more than what is left. The step's own cost is read off the
    // walk's clock, which a fake world does not turn, so the budget here is
    // the loads and the wall alone.
    world.left_ms = 6_000;
    let mut judge = judging();
    judge.compares = vec![compared(1, 0.9)];

    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(1),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );

    assert_eq!(world.presses, [2]);
    assert_eq!(
        world.snapshot_log,
        [
            ("save", "fake-0".to_string()),
            ("forget", "fake-0".to_string())
        ],
        "the budget is read off the save, and the save is let go of"
    );
    assert_eq!(walked.forks[0]["barred"], "no_budget");
    assert_eq!(walked.forks[0]["saveMs"], 1_000);
    assert!(judge.compared.is_empty());
}

#[test]
fn a_load_that_fails_leaves_the_device_on_that_candidates_screen_and_says_so() {
    let mut world = phone();
    world.restore_takes = false;
    let mut judge = judging();
    judge.compares = vec![compared(1, 0.9)];

    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(1),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );

    assert_eq!(
        world.presses,
        [2],
        "the first candidate's press is the step: nothing else is pressed"
    );
    let row = &walked.forks[0];
    assert_eq!(row["outcome"], "restore_failed");
    assert_eq!(row["chosen"], "mark:2");
    assert_eq!(row["explored"], 1);
    assert_eq!(row["routeUse"], "fallback");
    assert!(judge.compared.is_empty());
    assert_eq!(
        world.snapshot_log,
        [
            ("save", "fake-0".to_string()),
            ("load", "fake-0".to_string()),
            ("forget", "fake-0".to_string())
        ]
    );
    assert_eq!(walked.pressed, 1, "the walk counts the press it made");
}

#[test]
fn one_candidate_is_no_fork() {
    let mut world = phone();
    let mut judge = FakeJudge::chose(&[]);
    judge.answers = vec![ranked(2, &[(2, 1.0), (1, 0.0)])];
    judge.compares = vec![compared(1, 0.9)];

    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(1),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );

    assert_eq!(world.presses, [2]);
    assert!(
        walked.forks.is_empty(),
        "a press nobody else was weighed against is a step"
    );
    assert!(world.snapshot_log.is_empty());
}

#[test]
fn a_step_at_the_persons_turn_is_never_forked() {
    let mut world = phone();
    let mut judge = judging();
    judge.compares = vec![compared(1, 0.9)];

    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &stopped(RecipeStop::PersonsTurn),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );

    assert!(world.presses.is_empty() && world.snapshot_log.is_empty());
    assert!(walked.forks.is_empty());
    assert_eq!(walked.rows[0]["barred"], "not_recoverable");
}

/// A goal walk of two steps whose second judgment is `second`.
fn two_steps(second: Judged, compare: Compared) -> (FakeWorld, FakeJudge) {
    let world = phone();
    let mut judge = judging();
    judge.answers.push(second);
    judge.compares = vec![compare];
    (world, judge)
}

#[test]
fn the_walks_next_step_grades_the_fork() {
    // Reached at once: the caller's own condition held after the pick.
    let (mut world, mut judge) = two_steps(pick(3), compared(1, 0.8));
    world.reached = vec![true];
    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(2),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );
    assert_eq!(walked.forks[0]["next"], "reached");
    assert_eq!(walked.forks[0][AGREED.canonical], true);
    assert_eq!(
        walked.forks[0]["rescued"], true,
        "a pick that differed from today's and got there rescued the step"
    );

    // Moved on: the next look is a new screen and the judgment presses on.
    let (mut world, mut judge) = two_steps(pick(3), compared(1, 0.8));
    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(2),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );
    assert_eq!(walked.forks[0]["next"], "moved_on");
    assert_eq!(walked.forks[0][AGREED.canonical], true);
    assert_eq!(walked.forks[0]["rescued"], true);

    // Gave up: the next judgment sees nothing worth pressing where the pick led.
    let (mut world, mut judge) = two_steps(
        Judged::Chose(
            ActionChoice {
                chosen: Chosen::GiveUp,
                probabilities: BTreeMap::new(),
                confidence: 0.7,
                guard: None,
            }
            .into(),
        ),
        compared(1, 0.8),
    );
    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(2),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );
    assert_eq!(walked.forks[0]["next"], "gave_up");
    assert_eq!(walked.forks[0][AGREED.canonical], false);
    assert_eq!(walked.forks[0]["rescued"], false);

    // The same screen: the pick did nothing, and what follows is a retry.
    let (mut world, mut judge) = two_steps(pick(1), compared(2, 0.8));
    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(2),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );
    assert_eq!(walked.forks[0]["chosen"], "mark:2");
    assert_eq!(walked.forks[0]["next"], "same_screen");
    assert_eq!(walked.forks[0][AGREED.canonical], false);

    // The walk ended before a next look: nothing is shown either way.
    let (mut world, mut judge) = two_steps(pick(3), compared(1, 0.8));
    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(1),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );
    assert!(walked.forks[0].get("next").is_none());
    assert!(walked.forks[0].get(AGREED.canonical).is_none());
}

#[test]
fn under_shadow_the_mark_reads_the_comparison_against_todays_press() {
    // The same pick shares today's fate.
    let (mut world, mut judge) = two_steps(pick(3), compared(2, 0.8));
    world.leads_to.insert(2, vec![control(3, "다음")]);
    let walked = run_with(
        Mode::On,
        true,
        SHADOW,
        &goal(2),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );
    assert_eq!(world.presses, [2, 3]);
    assert_eq!(walked.forks[0]["next"], "moved_on");
    assert_eq!(walked.forks[0][AGREED.canonical], true);
    assert_eq!(walked.forks[0]["rescued"], false, "nothing was applied");

    // A different pick is wrong when today's press went on fine.
    let (mut world, mut judge) = two_steps(pick(3), compared(1, 0.8));
    world.leads_to.insert(2, vec![control(3, "다음")]);
    let walked = run_with(
        Mode::On,
        true,
        SHADOW,
        &goal(2),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );
    assert_eq!(walked.forks[0][AGREED.canonical], false);

    // And says nothing when today's press failed: nobody tried the other.
    let (mut world, mut judge) = two_steps(pick(1), compared(1, 0.8));
    let walked = run_with(
        Mode::On,
        true,
        SHADOW,
        &goal(2),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );
    assert_eq!(walked.forks[0]["next"], "same_screen");
    assert!(walked.forks[0].get(AGREED.canonical).is_none());
}

/// The fork's cost on a fake desk whose clock is this machine's own: a look
/// of 1,386 ms and a press of 1,420 ms (the emulator walk this window
/// recorded on 2026-09-20, `computer-use/sessions/20260920-125305-1482`),
/// and 1,700 ms per snapshot save or load — inside the band the live AVD on
/// this machine answered on 2026-09-22 (`a_live_avds_snapshot_round_trip`
/// on emulator-5554: saves 2,394 and 921 ms, loads 1,338 and 1,409 ms, the
/// delete 101 ms) and the 1.4–2.0 s saves `emulator::android::SNAPSHOT_SAVE_LIMIT`'s
/// note measured. Printed so a report can carry the multiple; asserted so
/// the arithmetic cannot drift from the road.
#[test]
fn measure_forked_steps_on_a_fake_desk() {
    const LOOK_MS: u64 = 1_386;
    const PRESS_MS: u64 = 1_420;
    const SNAPSHOT_MS: u64 = 1_700;
    let desk = || {
        let mut world = phone();
        world.look_ms = LOOK_MS;
        world.press_ms = PRESS_MS;
        world.snapshots = Some(SNAPSHOT_MS);
        world.left_ms = 600_000;
        world
    };
    // One step, today: a look and a press.
    let mut single = desk();
    let mut judge = judging();
    run_with(
        Mode::On,
        true,
        Branching::OFF,
        &goal(1),
        &mut judge,
        &mut single,
        Options::default(),
        None,
    );
    let single_ms = single.spent_ms;
    assert_eq!(single_ms, LOOK_MS + PRESS_MS);

    // The same step, forked over k = 2: a save, two presses with a look and
    // a load each, and the pick.
    let mut forked = desk();
    let mut judge = judging();
    judge.compares = vec![compared(1, 0.8)];
    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(1),
        &mut judge,
        &mut forked,
        Options::default(),
        None,
    );
    let forked_ms = forked.spent_ms;
    assert_eq!(
        forked_ms,
        LOOK_MS + SNAPSHOT_MS + 2 * (PRESS_MS + LOOK_MS + SNAPSHOT_MS) + PRESS_MS
    );
    assert_eq!(walked.forks[0]["explored"], 2);

    // Rescued steps over a set of forks where today's press leads nowhere
    // and the alternate leads to the goal, judged by a comparison that reads
    // the results as scripted: every one of them.
    let mut rescued = 0;
    let mut forks = 0;
    for _ in 0..10 {
        let mut world = desk();
        world.reached = vec![true];
        let mut judge = judging();
        judge.compares = vec![compared(1, 0.8)];
        let walked = run_with(
            Mode::On,
            true,
            RAISED,
            &goal(2),
            &mut judge,
            &mut world,
            Options::default(),
            None,
        );
        forks += 1;
        if walked.forks[0]["rescued"] == json!(true) {
            rescued += 1;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    let multiple = forked_ms as f64 / single_ms as f64;
    println!(
        "MEASURE fake desk: single step {single_ms} ms, forked step (k=2) {forked_ms} ms, x{multiple:.2}; rescued {rescued}/{forks} steps of a scripted world where today's press led nowhere and the comparison read the script (the script's number, not a claim)"
    );
    assert_eq!(rescued, forks);
    assert!(multiple > 1.0);

    // The gate's bite (t-6155 F3): over a leader-and-runner-up grid of the
    // seat's answers — the leader from 0.30 to 0.95 in steps of 0.05, the
    // rest on the runner-up — how many steps fork now that a clear lead is a
    // single step. Before the margin every one of them forked, and the fork
    // above was the clock of every step.
    let grid: Vec<f64> = (6_u8..=19).map(|n| f64::from(n) / 20.0).collect();
    let forking = grid
        .iter()
        .filter(|leader| {
            let Judged::Chose(ActionRead { choice, .. }) =
                ranked(2, &[(2, **leader), (1, 1.0 - **leader)])
            else {
                unreachable!()
            };
            zerocode_core::branching::fork_wanted(&choice).len() >= 2
        })
        .count();
    println!(
        "MEASURE fork gate: {forking}/{} steps of a leader/runner-up grid (0.30..=0.95 by 0.05) fork under the margin; {}/{} did before it",
        grid.len(),
        grid.len(),
        grid.len()
    );
    assert!(forking > 0 && forking < grid.len());
}

/* ---- the replay: the fake desk's forks, asked of the real endpoint ---- */

/// The seed `tools/branching-replay/seed.py` wrote (optional: it counts how
/// many of this machine's phone presses a fork would have been offered at).
const SEED_ENV: &str = "ZEROCODE_BRANCHING_REPLAY_SEED";
/// How many times each fork is asked (default 1): passes beyond the first
/// measure the seat's repeatability, not more evidence.
const RUNS_ENV: &str = "ZEROCODE_BRANCHING_REPLAY_RUNS";

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplaySeed {
    k: usize,
    k_cap: usize,
    apply_deadline_ms: u64,
    rows: Vec<ReplayRow>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplayRow {
    platform: Option<String>,
    probabilities: BTreeMap<String, f64>,
    chosen: Option<String>,
    look_ms: Option<u64>,
    act_ms: Option<u64>,
}

/// One fork of the fake desk: a goal, the screen before, the candidates with
/// the screens they led to, and which candidate the scenario says is right.
struct Scenario {
    goal: &'static str,
    before: &'static [&'static str],
    candidates: &'static [(usize, &'static str, bool, &'static [&'static str])],
    right: usize,
}

/// Forks a phone walk meets: a settings tree, a chat list, a form, a dialog.
/// Each candidate is the legend line pressed, whether the screen moved and
/// the legend of the screen it led to; the first candidate is the one the
/// emulator seat ranked first.
const SCENARIOS: [Scenario; 8] = [
    Scenario {
        goal: "Wi-Fi 설정을 열어라",
        before: &[
            "1 button Wi-Fi @101,40",
            "2 button 설정 @102,40",
            "3 button 검색 @103,40",
        ],
        candidates: &[
            (
                2,
                "2 button 설정 @102,40",
                true,
                &[
                    "4 button 네트워크 및 인터넷 @104,40",
                    "5 button 연결된 기기 @105,40",
                    "6 button 앱 @106,40",
                ],
            ),
            (
                1,
                "1 button Wi-Fi @101,40",
                true,
                &[
                    "7 switch 무선 랜 사용 @107,40",
                    "8 button 저장된 네트워크 @108,40",
                ],
            ),
        ],
        right: 1,
    },
    Scenario {
        goal: "홍길동에게 보낼 채팅방을 열어라",
        before: &[
            "1 button 홍길동 @101,40",
            "2 button 김철수 @102,40",
            "3 button 새 채팅 @103,40",
        ],
        candidates: &[
            (
                3,
                "3 button 새 채팅 @103,40",
                true,
                &["4 edit 받는 사람 @104,40", "5 button 취소 @105,40"],
            ),
            (
                1,
                "1 button 홍길동 @101,40",
                true,
                &[
                    "6 edit 메시지 입력 @106,40",
                    "7 button 보내기 @107,40",
                    "8 text 마지막 접속 방금 전 @108,10",
                ],
            ),
        ],
        right: 1,
    },
    Scenario {
        goal: "알림을 끄고 저장하라",
        before: &[
            "1 switch 푸시 수신 @101,40",
            "2 button 적용 @102,40",
            "3 button 뒤로 @103,40",
        ],
        candidates: &[
            (
                2,
                "2 button 적용 @102,40",
                false,
                &[
                    "1 switch 푸시 수신 @101,40",
                    "2 button 적용 @102,40",
                    "3 button 뒤로 @103,40",
                ],
            ),
            (
                1,
                "1 switch 푸시 수신 @101,40",
                true,
                &[
                    "1 switch 푸시 수신 해제됨 @101,40",
                    "2 button 적용 @102,40",
                    "3 button 뒤로 @103,40",
                ],
            ),
        ],
        right: 1,
    },
    Scenario {
        goal: "계정에서 로그아웃하라",
        before: &[
            "1 button 프로필 @101,40",
            "2 button 설정 @102,40",
            "3 button 도움말 @103,40",
        ],
        candidates: &[
            (
                1,
                "1 button 프로필 @101,40",
                true,
                &[
                    "4 text 홍길동 @104,10",
                    "5 button 프로필 수정 @105,40",
                    "6 button 이 기기에서 나가기 @106,40",
                ],
            ),
            (
                2,
                "2 button 설정 @102,40",
                true,
                &[
                    "7 button 알림 @107,40",
                    "8 button 개인정보 @108,40",
                    "9 button 정보 @109,40",
                ],
            ),
        ],
        right: 1,
    },
    Scenario {
        goal: "사진을 한 장 첨부하라",
        before: &[
            "1 button 카메라 @101,40",
            "2 button 갤러리 @102,40",
            "3 button 파일 @103,40",
        ],
        candidates: &[
            (
                3,
                "3 button 파일 @103,40",
                true,
                &[
                    "4 button 최근 @104,40",
                    "5 button 다운로드 @105,40",
                    "6 button 문서 @106,40",
                ],
            ),
            (
                2,
                "2 button 갤러리 @102,40",
                true,
                &[
                    "7 image IMG_0001 @107,40",
                    "8 image IMG_0002 @108,40",
                    "9 button 선택 @109,40",
                ],
            ),
            (
                1,
                "1 button 카메라 @101,40",
                true,
                &["10 button 촬영 @110,40", "11 button 전환 @111,40"],
            ),
        ],
        right: 2,
    },
    Scenario {
        goal: "확인 대화상자를 닫고 목록으로 돌아가라",
        before: &[
            "1 button 취소 @101,40",
            "2 button 삭제 @102,40",
            "3 text 정말 삭제할까요? @103,10",
        ],
        candidates: &[
            (
                2,
                "2 button 삭제 @102,40",
                true,
                &[
                    "4 text 삭제되었습니다 @104,10",
                    "5 button 실행 취소 @105,40",
                ],
            ),
            (
                1,
                "1 button 취소 @101,40",
                true,
                &[
                    "6 button 항목 1 @106,40",
                    "7 button 항목 2 @107,40",
                    "8 button 항목 3 @108,40",
                ],
            ),
        ],
        right: 1,
    },
    Scenario {
        goal: "다크 모드를 켜라",
        before: &[
            "1 button 디스플레이 @101,40",
            "2 button 소리 @102,40",
            "3 button 배터리 @103,40",
        ],
        candidates: &[
            (
                3,
                "3 button 배터리 @103,40",
                true,
                &["4 switch 절전 모드 @104,40", "5 text 배터리 82% @105,10"],
            ),
            (
                1,
                "1 button 디스플레이 @101,40",
                true,
                &[
                    "6 switch 어두운 테마 @106,40",
                    "7 button 밝기 @107,40",
                    "8 button 글자 크기 @108,40",
                ],
            ),
        ],
        right: 1,
    },
    Scenario {
        goal: "이번 달 청구서를 열어라",
        before: &[
            "1 button 홈 @101,40",
            "2 button 청구 @102,40",
            "3 button 더보기 @103,40",
        ],
        candidates: &[
            (
                3,
                "3 button 더보기 @103,40",
                true,
                &[
                    "4 button 설정 @104,40",
                    "5 button 고객센터 @105,40",
                    "6 button 로그아웃 @106,40",
                ],
            ),
            (
                2,
                "2 button 청구 @102,40",
                true,
                &[
                    "7 button 2026년 9월분 명세 @107,40",
                    "8 button 2026년 8월분 명세 @108,40",
                    "9 button 자동 납부 @109,40",
                ],
            ),
        ],
        right: 2,
    },
];

/// The stems of a goal's words: each whitespace token with one trailing
/// particle taken off, two characters and longer. A harness rule and not a
/// product one — it exists so a scenario cannot hand the comparison its
/// answer as a string.
fn goal_stems(goal: &str) -> Vec<String> {
    const PARTICLES: [&str; 14] = [
        "에서", "에게", "으로", "하라", "어라", "을", "를", "이", "가", "에", "의", "로", "고",
        "라",
    ];
    goal.split_whitespace()
        .map(|word| {
            PARTICLES
                .iter()
                .find_map(|particle| word.strip_suffix(particle))
                .unwrap_or(word)
                .to_string()
        })
        .filter(|stem| stem.chars().count() >= 2)
        .collect()
}

/// No scenario hands the comparison its answer as a string (t-6155 F3): the
/// screen the right candidate leads to carries none of the goal's words.
/// A wrong candidate's screen may — a decoy is what a real screen does —
/// and the controls pressed may, since the emulator seat saw those too.
#[test]
fn no_scenario_leaks_its_goal_into_the_right_candidates_result() {
    for scenario in &SCENARIOS {
        let stems = goal_stems(scenario.goal);
        assert!(!stems.is_empty(), "{}", scenario.goal);
        let right = scenario
            .candidates
            .iter()
            .find(|(mark, ..)| *mark == scenario.right)
            .expect("the right candidate is one of the candidates");
        for line in right.3 {
            for stem in &stems {
                assert!(
                    !line.contains(stem.as_str()),
                    "{:?}: the right result {line:?} carries the goal's {stem:?}",
                    scenario.goal
                );
            }
        }
    }
    // The reader takes the stems a person would: nouns and verbs, not their
    // particles, and nothing of one character.
    assert_eq!(goal_stems("알림을 끄고 저장하라"), ["알림", "저장"]);
    assert_eq!(goal_stems("Wi-Fi 설정을 열어라"), ["Wi-Fi", "설정"]);
}

fn scenario_ask(scenario: &Scenario) -> zerocode_core::branching::BranchAsk {
    use zerocode_core::branching::{BranchLook, Candidate, Outcome, ask};
    let candidates: Vec<Candidate> = scenario
        .candidates
        .iter()
        .map(|(mark, action, moved, controls)| Candidate {
            mark: *mark,
            action: (*action).to_string(),
            result: Some(Outcome {
                moved: *moved,
                controls: controls.iter().map(|line| (*line).to_string()).collect(),
                count: controls.len(),
            }),
        })
        .collect();
    let before: Vec<String> = scenario
        .before
        .iter()
        .map(|line| (*line).to_string())
        .collect();
    ask(&BranchLook {
        goal: scenario.goal,
        at: zerocode_core::screen_action::Where::Phone {
            platform: "android",
            device: "Pixel_6",
        },
        before: &before,
        candidates: &candidates,
    })
    .expect("every scenario has two candidates")
}

/// The fake desk's forks, put to the real endpoint as the shipped question
/// (`branching::ask`), and read against the scenario's own right answer:
/// how often the comparison named it — pass by pass, a Wilson bound on the
/// first alone — how often it named what the emulator seat would have
/// pressed (the first candidate), its latency and what it cost. The right
/// answers are the scenarios' own, a synthetic golden and not this machine's
/// walks. With a seed, also how many of this machine's phone presses a fork
/// would have been offered at (`branching::fork_wanted` over the seat's own
/// probabilities) and what those steps' looks and presses cost.
///
/// The person's ledger and day count are never touched — the question
/// leaves from a temporary home. The key comes from the environment
/// (`tools/branching-replay/README.md`).
#[test]
#[ignore = "crosses the real endpoint; run by hand with the key in the environment"]
fn the_forks_this_desk_would_take() {
    use zerocode_core::branching::fork_wanted;
    use zerocode_core::jev::{BRANCHING, BRANCHING_APPLY_DEADLINE_MS, JevMode, SMART_SETTINGS_KEY};

    if let Ok(seed_at) = std::env::var(SEED_ENV) {
        let seed: ReplaySeed =
            serde_json::from_str(&std::fs::read_to_string(&seed_at).expect("the seed reads"))
                .expect("the seed's shape");
        assert_eq!(
            (seed.k, seed.k_cap, seed.apply_deadline_ms),
            (
                zerocode_core::jev::BRANCHING_K,
                zerocode_core::jev::BRANCHING_K_CAP,
                BRANCHING_APPLY_DEADLINE_MS
            ),
            "a seed made for another table is refused"
        );
        let mut would_fork = 0usize;
        let mut looks = Vec::new();
        let mut acts = Vec::new();
        for row in &seed.rows {
            let chosen = row
                .chosen
                .as_deref()
                .and_then(zerocode_core::screen_action::mark_of)
                .map_or(Chosen::GiveUp, Chosen::Mark);
            let choice = ActionChoice {
                chosen,
                probabilities: row.probabilities.clone(),
                confidence: 0.0,
                guard: None,
            };
            if fork_wanted(&choice).len() >= 2 {
                would_fork += 1;
            }
            looks.extend(row.look_ms);
            acts.extend(row.act_ms);
        }
        looks.sort_unstable();
        acts.sort_unstable();
        println!(
            "SEED {seed_at}: {} phone presses on this machine, {would_fork} would have forked (k={}); look p50 {:?} ms, press p50 {:?} ms; platforms {:?}",
            seed.rows.len(),
            seed.k,
            zerocode_core::jev::summary::percentile(&looks, 0.5),
            zerocode_core::jev::summary::percentile(&acts, 0.5),
            seed.rows
                .iter()
                .filter_map(|row| row.platform.clone())
                .collect::<std::collections::BTreeSet<_>>(),
        );
    }

    let key = std::env::var(zerocode_harness::TYPESAFE_API_KEY_ENV).unwrap_or_else(|_| {
        panic!(
            "{} carries this machine's TypeSafe key",
            zerocode_harness::TYPESAFE_API_KEY_ENV
        )
    });
    let runs: usize = std::env::var(RUNS_ENV)
        .ok()
        .and_then(|said| said.parse().ok())
        .unwrap_or(1)
        .max(1);
    let home = tempfile::tempdir().expect("a zo home of this measurement's own");
    let work = tempfile::tempdir().expect("a checkout");
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({
            SMART_SETTINGS_KEY: {
                BRANCHING.setting: JevMode::Shadow.key(),
                "jev": { "workspaces": [work.path().display().to_string()] },
            }
        })
        .to_string(),
    )
    .expect("zo's settings");
    let wire =
        crate::systemone::Wire::at(crate::systemone::SYSTEMONE_BASE_URL, &key, Some(settings));
    let rate = model_prices::systemone_rate(crate::systemone::SYSTEMONE_MODEL);
    println!(
        "scenarios={} runs={runs} wall={}ms",
        SCENARIOS.len(),
        BRANCHING_APPLY_DEADLINE_MS
    );

    // Per pass, as the notify harness counts (t-6155 F2): only the first
    // pass is an independent sample and carries a Wilson bound; a later pass
    // asks the same eight questions again and says only whether the answer
    // repeated.
    let mut right_by_pass = vec![zerocode_core::jev::promote::Agreement::default(); runs];
    let mut as_today_by_pass = vec![0usize; runs];
    let mut elapsed = Vec::new();
    let mut refusals: BTreeMap<String, usize> = BTreeMap::new();
    let mut input_tokens = 0u64;
    let mut first: Vec<Option<usize>> = vec![None; SCENARIOS.len()];
    let mut repeated = zerocode_core::jev::promote::Agreement::default();
    for pass in 0..runs {
        for (index, scenario) in SCENARIOS.iter().enumerate() {
            let asked = scenario_ask(scenario);
            let began = std::time::Instant::now();
            let answered = wire.ask(
                &BRANCHING,
                Some(work.path()),
                crate::systemone::request_body(&asked.state, &asked.questions),
                std::time::Duration::from_millis(BRANCHING_APPLY_DEADLINE_MS),
            );
            let took = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
            elapsed.push(took);
            let read = answered.answer.and_then(|body| {
                let parsed: Value =
                    serde_json::from_str(&body).map_err(|_| "schema".to_string())?;
                input_tokens += parsed["usage"]["input_tokens"].as_u64().unwrap_or_default();
                asked
                    .read(parsed.get("answers").unwrap_or(&Value::Null))
                    .map_err(|why| why.token().to_string())
            });
            match read {
                Ok(choice) => {
                    right_by_pass[pass].compared += 1;
                    right_by_pass[pass].agreed += usize::from(choice.mark == scenario.right);
                    as_today_by_pass[pass] += usize::from(choice.mark == scenario.candidates[0].0);
                    if pass == 0 {
                        first[index] = Some(choice.mark);
                    } else if let Some(was) = first[index] {
                        repeated.compared += 1;
                        repeated.agreed += usize::from(was == choice.mark);
                    }
                    println!(
                        "  {pass}/{index} {:<24} chose mark:{} (right mark:{}, today mark:{}) conf {:.2} {took} ms",
                        scenario.goal,
                        choice.mark,
                        scenario.right,
                        scenario.candidates[0].0,
                        choice.confidence
                    );
                }
                Err(token) => {
                    *refusals.entry(token.clone()).or_default() += 1;
                    println!(
                        "  {pass}/{index} {:<24} refused: {token} {took} ms",
                        scenario.goal
                    );
                }
            }
        }
    }
    elapsed.sort_unstable();
    let asked_count = elapsed.len();
    let cost = rate.map(|rate| rate.input_cost_usd(input_tokens));
    #[allow(clippy::cast_precision_loss)]
    let share = |part: usize, whole: usize| {
        if whole == 0 {
            0.0
        } else {
            100.0 * part as f64 / whole as f64
        }
    };
    println!(
        "\nRepeated asks are dependent observations; only the first pass carries a Wilson bound."
    );
    println!("| pass | compared | right | share | Wilson lower | named today's press |");
    println!("| --- | --- | --- | --- | --- | --- |");
    for (pass, (right, today)) in right_by_pass.iter().zip(&as_today_by_pass).enumerate() {
        let bound = if pass == 0 {
            right
                .lower_bound()
                .map_or_else(|| "—".to_string(), |bound| format!("{:.1}%", bound * 100.0))
        } else {
            "repeated".to_string()
        };
        println!(
            "| #{pass} | {} | {} | {:.1}% | {bound} | {today} |",
            right.compared,
            right.agreed,
            share(right.agreed, right.compared)
        );
    }
    println!(
        "repeat {}/{} · refusals {refusals:?}",
        repeated.agreed, repeated.compared,
    );
    println!(
        "latency p50 {:?} ms · p95 {:?} ms · max {:?} ms over {asked_count} asks; input tokens {input_tokens}; cost {}",
        zerocode_core::jev::summary::percentile(&elapsed, 0.5),
        zerocode_core::jev::summary::percentile(&elapsed, 0.95),
        elapsed.last(),
        cost.map_or_else(
            || "unpriced".to_string(),
            |usd| format!(
                "${usd:.4} total, ${:.6} per ask",
                usd / asked_count.max(1) as f64
            )
        )
    );
}

// ---- beside the walk's own switches (t-6132) --------------------------------
//
// The order of one step, where the two meet: look, the judgment (asked in
// turn, or the one begun on the last look), the second rung, the fork, the
// question begun ahead, the press — and on the next look the fork's mark and
// the memo's word both on their rows.

/// A forked step asks ahead ([`Options::overlap`]) with the number the hand
/// goes out with — the comparison's pick, not the seat's own — its legend
/// among `pressed`; and the fork's own presses and looks begin nothing.
#[test]
fn a_forked_step_asks_ahead_with_the_canonical_number() {
    let mut world = phone();
    let mut judge = judging();
    judge.compares = vec![compared(1, 0.8)];
    // One answer for the question begun ahead, one for the screen the pick
    // leads to.
    judge.answers.extend([pick(3), pick(3)]);

    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(2),
        &mut judge,
        &mut world,
        Options {
            overlap: true,
            rescue: false,
            act_line: None,
        },
        None,
    );

    assert_eq!(
        world.presses[..3],
        [2, 1, 1],
        "each candidate on the saved device, then the pick"
    );
    assert_eq!(
        judge.begun,
        [vec![2]],
        "begun once, after the fork: the pick (1) is the number spent, not today's (2)"
    );
    let legend = |mark: usize| {
        zerocode_core::computer_use_protocol::marks::legend_line(&control(mark, "저장"))
            .expect("a legend line")
    };
    assert_eq!(
        judge.begun_state[0]["pressed"],
        json!([legend(1)]),
        "the legend among `pressed` is the pick's, not today's ({})",
        legend(2)
    );
    assert_eq!(walked.forks[0]["chosen"], "mark:1");
    assert_eq!(walked.rows[0]["forked"], "mark:1");
    // Pressing 1 leads to a screen that moved: the next look asks another
    // question, the answer in flight is dropped, and the fork's mark is the
    // walk going on.
    assert_eq!((walked.overlapped, walked.discarded), (0, 1));
    assert_eq!(walked.forks[0]["next"], NextStep::MovedOn.word());
    assert_eq!(walked.forks[0][AGREED.canonical], true);
}

/// The second rung comes before the fork: a step the second reader pressed
/// for is forked on the second reader's own ranking — its pick is the first
/// candidate, `today` on the fork's row — and the comparison's pick is the
/// number the hand goes out with. Without the rung the walk steps back to
/// the person before any candidate exists.
#[test]
fn a_rescued_step_forks_on_the_second_readers_ranking() {
    let seat = || {
        let mut judge = FakeJudge::chose(&[]);
        let Judged::Chose(ActionRead {
            choice: mut unsure, ..
        }) = ranked(2, &[(2, 0.6), (1, 0.4)])
        else {
            unreachable!()
        };
        unsure.confidence = 0.29;
        judge.answers = vec![Judged::Chose(unsure.into())];
        judge.compares = vec![compared(2, 0.8)];
        judge
    };
    let mut world = phone();
    let mut judge = seat();
    let mut team = FakeJudge::chose(&[]);
    // Torn between the two — inside the fork margin (t-6155 F3), so the
    // rescued step is one a fork is for.
    team.answers = vec![ranked(1, &[(1, 0.55), (2, 0.45)])];

    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(1),
        &mut judge,
        &mut world,
        Options {
            overlap: false,
            rescue: true,
            act_line: None,
        },
        Some(&mut team),
    );

    assert_eq!(team.asked, [vec![1, 2]], "the same closed choice");
    assert_eq!(
        judge.compared,
        [vec![1, 2]],
        "the fork's candidates are the second reader's, its pick first"
    );
    assert_eq!(
        world.presses,
        [1, 2, 2],
        "explored in the second reader's order, then the comparison's pick"
    );
    let row = &walked.rows[0];
    assert_eq!(row[RESCUE]["outcome"], "pressed");
    assert_eq!(row[RESCUED_BY], RESCUED_BY_TEAM);
    assert_eq!(row["chosen"], "mark:1", "the second reader's number");
    assert_eq!(
        row["forked"], "mark:2",
        "and the fork's, which the hand went out with"
    );
    assert_eq!(row["pressed"], true);
    let fork = &walked.forks[0];
    assert_eq!(fork["today"], "mark:1");
    assert_eq!(fork["chosen"], "mark:2");
    assert_eq!(fork["routeUse"], "applied");
    assert_eq!(
        (walked.rescued, walked.rescue_failed, walked.pressed),
        (1, 0, 1)
    );

    let mut world = phone();
    let mut judge = seat();
    let unhelped = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(1),
        &mut judge,
        &mut world,
        Options::default(),
        None,
    );
    assert!(
        world.presses.is_empty(),
        "the person's step: nothing pressed"
    );
    assert!(judge.compared.is_empty() && world.snapshot_log.is_empty());
    assert!(unhelped.forks.is_empty());
    assert_eq!(
        unhelped.rows[0]["barred"],
        json!(Barred::LowConfidence.as_str())
    );
}

/// On the next look both seats write: the fork's row learns what the pick
/// led to, and the screen seat's row says the judgment begun ahead was used
/// and came from the memo.
#[test]
fn the_next_look_settles_the_fork_and_says_the_memo_answered_ahead() {
    let mut world = phone();
    let mut judge = judging();
    // The comparison keeps today's 2, which leads nowhere: the screen stays,
    // and the question begun ahead is the one the next look asks.
    judge.compares = vec![compared(2, 0.8)];
    judge.answers.push(pick(1));
    judge.cached = true;

    let walked = run_with(
        Mode::On,
        true,
        RAISED,
        &goal(3),
        &mut judge,
        &mut world,
        Options {
            overlap: true,
            rescue: false,
            act_line: None,
        },
        None,
    );

    assert_eq!(
        world.presses,
        [2, 1, 2, 1],
        "the fork's two, the pick, then the answer begun ahead"
    );
    assert_eq!(judge.begun, [vec![1]], "begun once, 2 spent");
    assert_eq!(walked.forks.len(), 1);
    let fork = &walked.forks[0];
    assert_eq!(fork["chosen"], "mark:2");
    assert_eq!(fork["next"], NextStep::SameScreen.word());
    assert_eq!(fork[AGREED.canonical], false);
    assert_eq!((walked.overlapped, walked.discarded), (1, 0));
    let row = &walked.rows[1];
    assert_eq!(row[OVERLAP], json!(OVERLAP_USED));
    assert!(row["hiddenMs"].is_u64());
    assert_eq!(row[CACHED.canonical], true);
    assert_eq!(row["chosen"], "mark:1");
    assert!(row.get("forked").is_none(), "one candidate: no fork");
}

/// A fork presses each candidate for real before a snapshot puts the device
/// back, and a snapshot cannot take back what left the device: a torn step
/// whose runner-up is a control a press cannot take back is not forked — the
/// seat's own pick is pressed once, as today (t-6187). The same torn step
/// over two plain controls forks.
#[test]
fn a_fork_never_explores_a_control_a_press_cannot_take_back() {
    let torn_over = |runner_up: &str| {
        let mut world = FakeWorld::android(&[1, 2]);
        let mut screen = world.look_now();
        screen.items = vec![control(1, runner_up), control(2, "다음")];
        world.screen_is(screen);
        let mut judge = judging();
        judge.compares = vec![compared(1, 0.9)];
        let walked = run_with(
            Mode::On,
            true,
            RAISED,
            &goal(1),
            &mut judge,
            &mut world,
            Options::default(),
            None,
        );
        (walked, world, judge)
    };
    let (walked, world, judge) = torn_over("계정 삭제");
    assert_eq!(
        world.presses,
        [2],
        "the delete was pressed to see what it does"
    );
    assert!(world.snapshot_log.is_empty(), "{:?}", world.snapshot_log);
    assert!(walked.forks.is_empty() && judge.compared.is_empty());

    let (walked, world, _) = torn_over("설정");
    assert!(!world.snapshot_log.is_empty(), "two plain controls fork");
    assert_eq!(walked.forks.len(), 1);
}
