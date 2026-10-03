use super::*;
use crate::hook::Tool;

const T0: i64 = 1_800_000_000_000;

/// A call a second: the fastest cadence the gate's lookahead is sized for.
const STEP_MS: i64 = 1_000;

fn activity(phase: Phase) -> Activity {
    Activity {
        verb: Tool::Other("tool".into()),
        target: None,
        phase,
        reads: Vec::new(),
        writes: Vec::new(),
        vcs: Vec::new(),
        cwd: None,
    }
}

/// A tool event's payload as a hook delivers it.
fn payload(tool: &str, input: &str) -> String {
    format!(r#"{{"hook_event_name":"PreToolUse","tool_name":"{tool}","tool_input":{input}}}"#)
}

fn read_of(path: &str, offset: usize) -> (&'static str, String) {
    (
        "Read",
        format!(r#"{{"file_path":"{path}","offset":{offset},"limit":2000}}"#),
    )
}

/// A synthetic worker, driven the way the window drives a real one: the hook
/// says a tool was called, the transcript says what the model's call cost, and
/// the beat judges.
struct Drive {
    book: StepBook,
    now_ms: i64,
}

impl Drive {
    fn new() -> Self {
        Self {
            book: StepBook::default(),
            now_ms: T0,
        }
    }

    fn call(&mut self, tool: &str, input: &str) {
        self.now_ms += STEP_MS;
        self.book
            .note_activity(&activity(Phase::Started), &payload(tool, input));
    }

    fn call_of(&mut self, (tool, input): (&str, String)) {
        self.call(tool, &input);
    }

    fn fail(&mut self) {
        self.book.note_activity(&activity(Phase::Failed), "{}");
    }

    fn prompt(&mut self) {
        self.book.note_activity(&activity(Phase::Prompted), "{}");
    }

    fn cost(&mut self, usd: f64) {
        self.book.note_cost(Some(usd));
    }

    fn judge(&self) -> Judgement {
        self.book.judge(&Allowance::default(), self.now_ms)
    }

    /// Saves a checkpoint the moment one is due, as the window does.
    fn checkpoint_when_due(&mut self) {
        if self
            .judge()
            .reasons
            .iter()
            .any(|reason| reason.code == Code::CheckpointDue)
        {
            let now = self.now_ms;
            self.book.checkpointed(now);
        }
    }

    /// The task's budget as the window fills it in for this worker alone.
    fn judge_within(&self, task_usd: f64) -> Judgement {
        let allowance = Allowance {
            task: Some(Cap {
                limit_usd: task_usd,
                spent_usd: self.book.spent_usd().unwrap_or(0.0),
                ahead_usd: self.book.ahead_usd(),
            }),
            day: None,
        };
        self.book.judge(&allowance, self.now_ms)
    }
}

fn codes(judgement: &Judgement) -> Vec<Code> {
    judgement.reasons.iter().map(|reason| reason.code).collect()
}

#[test]
fn a_steady_worker_is_left_alone_until_a_checkpoint_is_due() {
    let mut drive = Drive::new();
    for step in 1..CHECKPOINT_EVERY_STEPS as usize {
        drive.call_of(read_of(&format!("/repo/src/f{step}.rs"), 0));
        drive.cost(0.2);
        let judgement = drive.judge();
        assert_eq!(
            judgement.verdict,
            Verdict::Continue,
            "step {step}: {:?}",
            judgement.reasons
        );
    }
    drive.call_of(read_of("/repo/src/last.rs", 0));
    drive.cost(0.2);
    let judgement = drive.judge();
    assert_eq!(judgement.verdict, Verdict::Checkpoint);
    assert_eq!(codes(&judgement), vec![Code::CheckpointDue]);
    assert_eq!(
        judgement.reasons[0].value,
        f64::from(CHECKPOINT_EVERY_STEPS)
    );
    assert_eq!(judgement.metrics.steps, CHECKPOINT_EVERY_STEPS);
}

#[test]
fn a_checkpoint_resets_the_count_and_is_not_due_again_inside_the_gap() {
    let mut drive = Drive::new();
    for step in 0..CHECKPOINT_EVERY_STEPS as usize {
        drive.call_of(read_of("/repo/src/a.rs", step));
    }
    assert_eq!(drive.judge().verdict, Verdict::Checkpoint);
    drive.book.checkpointed(drive.now_ms);
    assert_eq!(drive.judge().verdict, Verdict::Continue);
    assert_eq!(drive.judge().metrics.checkpoints, 1);
    assert_eq!(drive.judge().metrics.since_checkpoint, 0);

    // Two hundred calls a second apart are well inside the five minutes.
    for step in 0..CHECKPOINT_EVERY_STEPS as usize {
        drive.call_of(read_of("/repo/src/b.rs", step));
    }
    assert_eq!(
        drive.judge().verdict,
        Verdict::Continue,
        "the steps are there, the gap is not"
    );
    drive.now_ms += CHECKPOINT_MIN_GAP_MS;
    assert_eq!(drive.judge().verdict, Verdict::Checkpoint);
}

#[test]
fn reads_of_one_file_at_different_offsets_are_not_a_loop() {
    let mut drive = Drive::new();
    for chunk in 0..60 {
        drive.call_of(read_of("/repo/src/orchestration.rs", chunk * 2_000));
    }
    let judgement = drive.judge();
    assert_eq!(judgement.metrics.rework_permille, Some(0));
    assert!(judgement.verdict <= Verdict::Checkpoint);
}

#[test]
fn the_same_call_over_and_over_pauses_once_the_window_can_judge() {
    let mut drive = Drive::new();
    for step in 1..REWORK_MIN_STEPS {
        drive.call("Bash", r#"{"command":"cargo test -p zerocode-core"}"#);
        let judgement = drive.judge();
        assert_eq!(judgement.metrics.rework_permille, None, "step {step}");
        assert_eq!(judgement.verdict, Verdict::Continue, "step {step}");
    }
    drive.call("Bash", r#"{"command":"cargo test -p zerocode-core"}"#);
    let judgement = drive.judge();
    assert_eq!(judgement.verdict, Verdict::Pause);
    assert_eq!(codes(&judgement), vec![Code::ReworkLoop]);
    // Fifteen of sixteen calls repeated the one before.
    assert_eq!(judgement.reasons[0].value, 937.0);
    assert_eq!(judgement.reasons[0].limit, f64::from(REWORK_LIMIT_PERMILLE));
}

#[test]
fn a_prompt_ends_a_run_of_repeats() {
    let mut drive = Drive::new();
    for _ in 0..REWORK_WINDOW_STEPS {
        drive.call("Bash", r#"{"command":"zerocode-orc check"}"#);
        drive.prompt();
    }
    assert_eq!(drive.judge().metrics.rework_permille, Some(0));
}

#[test]
fn a_call_that_failed_is_rework_and_a_few_failures_are_not_a_loop() {
    let mut few = Drive::new();
    for step in 0..REWORK_WINDOW_STEPS {
        few.call_of(read_of(&format!("/repo/src/f{step}.rs"), 0));
        if step.is_multiple_of(12) {
            few.fail();
        }
    }
    let judgement = few.judge();
    assert_eq!(judgement.metrics.rework_permille, Some(83), "2 of 24");
    assert_eq!(judgement.verdict, Verdict::Continue);

    let mut many = Drive::new();
    for step in 0..REWORK_WINDOW_STEPS {
        many.call_of(read_of(&format!("/repo/src/f{step}.rs"), 0));
        if step.is_multiple_of(3) {
            many.fail();
        }
    }
    let judgement = many.judge();
    assert_eq!(judgement.metrics.rework_permille, Some(333), "8 of 24");
    assert_eq!(judgement.verdict, Verdict::Pause);
    assert_eq!(codes(&judgement), vec![Code::ReworkLoop]);
}

#[test]
fn a_failure_on_a_call_that_also_repeated_is_one_rework_and_not_two() {
    let mut drive = Drive::new();
    drive.call("Bash", r#"{"command":"make"}"#);
    drive.call("Bash", r#"{"command":"make"}"#);
    drive.fail();
    for step in 0..REWORK_MIN_STEPS {
        drive.call_of(read_of(&format!("/repo/src/f{step}.rs"), 0));
    }
    assert_eq!(drive.judge().metrics.rework_permille, Some(55), "1 of 18");
}

#[test]
fn the_rework_ratio_is_read_over_the_latest_calls_and_forgets() {
    let mut drive = Drive::new();
    for step in 0..500 {
        drive.call_of(read_of(&format!("/repo/src/f{step}.rs"), 0));
        drive.checkpoint_when_due();
    }
    assert_eq!(drive.judge().verdict, Verdict::Continue);
    let mut paused_after = None;
    for repeat in 1..=REWORK_WINDOW_STEPS {
        drive.call("Bash", r#"{"command":"cargo build"}"#);
        if paused_after.is_none() && drive.judge().verdict == Verdict::Pause {
            paused_after = Some(repeat);
        }
    }
    // The first call of the loop is not a repeat; the eighth repeat — the
    // ninth call — is what takes eight of twenty-four past three in ten.
    assert_eq!(paused_after, Some(9));
    for step in 0..REWORK_WINDOW_STEPS {
        drive.call_of(read_of(&format!("/repo/src/g{step}.rs"), 0));
    }
    let judgement = drive.judge();
    assert!(
        !codes(&judgement).contains(&Code::ReworkLoop),
        "a worker that has stopped looping is not held to its loop: {:?}",
        judgement.reasons
    );
}

#[test]
fn a_rising_cost_asks_for_a_checkpoint_and_not_a_look() {
    let mut drive = Drive::new();
    for (index, usd) in [0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5]
        .into_iter()
        .enumerate()
    {
        drive.call_of(read_of("/repo/src/a.rs", index * 2_000));
        drive.cost(usd);
    }
    let judgement = drive.judge();
    assert_eq!(judgement.verdict, Verdict::Checkpoint);
    assert_eq!(codes(&judgement), vec![Code::CostRising]);
    assert_eq!(judgement.reasons[0].value, 2_500.0);
    assert_eq!(judgement.metrics.rise_permille, Some(2_500));
}

#[test]
fn one_dear_call_is_not_a_trend_and_cheap_calls_are_not_worth_calling_one() {
    let mut spike = Drive::new();
    for usd in [0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 0.2, 3.0] {
        spike.cost(usd);
    }
    assert_eq!(spike.judge().verdict, Verdict::Continue);
    assert_eq!(spike.judge().metrics.rise_permille, Some(1_000));

    let mut cheap = Drive::new();
    for usd in [
        0.01, 0.01, 0.01, 0.01, 0.01, 0.01, 0.05, 0.05, 0.05, 0.05, 0.05, 0.05,
    ] {
        cheap.cost(usd);
    }
    assert_eq!(cheap.judge().metrics.rise_permille, Some(5_000));
    assert_eq!(cheap.judge().verdict, Verdict::Continue);
}

/// The task's own words: a worker is reproduced running away — from the thirtieth
/// call it repeats one failing call while each model call costs a tenth more
/// than the last, the shape of a loop that floods its own context — and the
/// budget a person set for the task is twenty dollars.
#[test]
fn a_runaway_is_stopped_before_it_spends_the_budget() {
    const BUDGET: f64 = 20.0;
    const RUNS_AWAY_AT: usize = 30;
    /// Model calls spent between the decision and the stop landing.
    const LAG_CALLS: usize = 3;
    const GROWTH: f64 = 1.12;

    let mut drive = Drive::new();
    let mut usd = 0.30;
    let (mut first_pause, mut stopped) = (None, None);
    for step in 1..=200 {
        if step < RUNS_AWAY_AT {
            drive.call_of(read_of(&format!("/repo/src/f{step}.rs"), 0));
        } else {
            drive.call("Bash", r#"{"command":"cargo test"}"#);
            drive.fail();
            usd *= GROWTH;
        }
        drive.cost(usd);
        match drive.judge_within(BUDGET).verdict {
            Verdict::Stop => {
                stopped = Some(step);
                break;
            }
            Verdict::Pause => first_pause = first_pause.or(Some(step)),
            Verdict::Continue | Verdict::Checkpoint => {}
        }
    }
    let stopped = stopped.expect("the runaway is stopped");
    let first_pause = first_pause.expect("and was asked to be looked at first");
    assert!(
        RUNS_AWAY_AT < first_pause && first_pause < stopped,
        "ran away at {RUNS_AWAY_AT}, paused at {first_pause}, stopped at {stopped}"
    );

    let mut spent = drive.book.spent_usd().expect("priced");
    let at_decision = spent;
    for _ in 0..LAG_CALLS {
        usd *= GROWTH;
        spent += usd;
    }
    assert!(
        spent < BUDGET,
        "stopped at call {stopped} with ${at_decision:.2} spent; ${spent:.2} once the stop lands, \
         against a budget of ${BUDGET:.2}"
    );
    let judgement = drive.judge_within(BUDGET);
    assert_eq!(judgement.reasons[0].code, Code::TaskBudgetStop);
    assert_eq!(judgement.reasons[0].limit, BUDGET);
}

/// A small deterministic generator, so a "normal worker" is the same worker on
/// every machine and every run.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
}

/// Commands a worker runs again and again over a day.
const COMMANDS: [&str; 6] = [
    "cargo test -p zerocode-core",
    "git status --short",
    "cargo fmt --all",
    "just pii-check",
    "git diff --stat",
    "ls crates",
];

/// One call of an honest worker: mostly reads of many files and of one file in
/// chunks, edits, commands it runs again after an edit, searches. Nobody who is
/// making progress makes the very same call twice with nothing between — that
/// is the rule the gate reads — so a call identical to the last is replaced by
/// the edit that real work puts between them.
fn normal_call(rng: &mut Lcg, step: usize, previous: &str) -> (&'static str, String) {
    let (tool, input) = match rng.below(100) {
        0..=54 => (
            "Read",
            format!(
                r#"{{"file_path":"/repo/src/f{}.rs","offset":{},"limit":2000}}"#,
                rng.below(300),
                rng.below(10) * 2_000
            ),
        ),
        55..=74 => (
            "Edit",
            format!(
                r#"{{"file_path":"/repo/src/f{}.rs","old_string":"a{step}","new_string":"b{step}"}}"#,
                rng.below(300)
            ),
        ),
        75..=89 => (
            "Bash",
            format!(
                r#"{{"command":"{}"}}"#,
                COMMANDS[usize::try_from(rng.below(6)).unwrap_or(0)]
            ),
        ),
        _ => (
            "Grep",
            format!(r#"{{"pattern":"needle{}"}}"#, rng.below(500)),
        ),
    };
    if input == previous {
        (
            "Edit",
            format!(r#"{{"file_path":"/repo/src/f{step}.rs","old_string":"x","new_string":"y"}}"#),
        )
    } else {
        (tool, input)
    }
}

/// What a normal worker's model call costs: a context that grows to three times
/// its size over the day, and the one call in forty that pays for a cache that
/// expired eight times over.
fn normal_cost(step: usize, steps: usize) -> f64 {
    let growth = 1.0 + step as f64 / steps as f64 * 2.0;
    let expired = if step % 40 == 39 { 8.0 } else { 1.0 };
    0.15 * growth * expired
}

/// One honest worker's day, call by call, a failed call now and then and a
/// prompt now and then; from call `loops_from` — when there is one — it does
/// nothing but run one command, over and over. `beat` is the window's beat,
/// told the step and the worker as it stands.
fn a_day(
    seed: u64,
    steps: usize,
    loops_from: Option<usize>,
    mut beat: impl FnMut(usize, &mut Drive),
) {
    let mut drive = Drive::new();
    let mut rng = Lcg(seed);
    let mut previous = String::new();
    for step in 0..steps {
        let (tool, input) = if loops_from.is_some_and(|from| step >= from) {
            ("Bash", r#"{"command":"cargo build"}"#.to_string())
        } else {
            normal_call(&mut rng, step, &previous)
        };
        previous.clone_from(&input);
        drive.call(tool, &input);
        if rng.below(100) < 2 {
            drive.fail();
        }
        if rng.below(150) == 0 {
            drive.prompt();
        }
        drive.cost(normal_cost(step, steps));
        beat(step, &mut drive);
    }
}

/// The task's acceptance, the half that must hold as firmly as the first: a
/// worker doing ordinary work — on any of eight different days — is never
/// paused or stopped, at any call, with a budget it stays inside or none at
/// all. A checkpoint is the most the gate ever asks of it.
#[test]
fn a_normal_worker_is_never_paused_or_stopped_at_any_call_of_its_day() {
    for seed in 1..=8_u64 {
        a_day(seed, 1_500, None, |step, drive| {
            let spent = drive.book.spent_usd().unwrap_or(0.0);
            let ahead = drive.book.ahead_usd();
            let budgeted = Allowance {
                task: Some(Cap {
                    limit_usd: 1_000.0,
                    spent_usd: spent,
                    ahead_usd: ahead,
                }),
                day: Some(Cap {
                    limit_usd: 5_000.0,
                    spent_usd: spent,
                    ahead_usd: ahead,
                }),
            };
            for allowance in [&budgeted, &Allowance::default()] {
                let judgement = drive.book.judge(allowance, drive.now_ms);
                assert!(
                    judgement.verdict <= Verdict::Checkpoint,
                    "seed {seed} step {step}: {:?} {:?}",
                    judgement.verdict,
                    judgement.reasons
                );
            }
            drive.checkpoint_when_due();
        });
    }
}

/// And the same days are not blind: the loop that the ordinary day never makes
/// is found in the same worker within a window of its starting.
#[test]
fn the_same_worker_is_paused_within_a_window_of_its_day_turning_into_a_loop() {
    const LOOPS_FROM: usize = 300;
    for seed in 1..=8_u64 {
        let mut paused_at = None;
        a_day(seed, 400, Some(LOOPS_FROM), |step, drive| {
            drive.checkpoint_when_due();
            if drive.judge().verdict == Verdict::Pause {
                paused_at.get_or_insert(step);
            }
        });
        let paused_at = paused_at.unwrap_or_else(|| panic!("seed {seed}: never paused"));
        assert!(
            (LOOPS_FROM..=LOOPS_FROM + REWORK_WINDOW_STEPS).contains(&paused_at),
            "seed {seed}: the loop began at {LOOPS_FROM} and was seen at {paused_at}"
        );
    }
}

#[test]
fn a_budget_nearly_spent_asks_for_a_look_and_says_which_budget() {
    let mut drive = Drive::new();
    drive.cost(0.1);
    let task = Allowance {
        task: Some(Cap {
            limit_usd: 10.0,
            spent_usd: 8.5,
            ahead_usd: 0.1,
        }),
        day: Some(Cap {
            limit_usd: 100.0,
            spent_usd: 10.0,
            ahead_usd: 0.1,
        }),
    };
    let judgement = drive.book.judge(&task, drive.now_ms);
    assert_eq!(judgement.verdict, Verdict::Pause);
    assert_eq!(codes(&judgement), vec![Code::TaskBudgetNear]);
    assert_eq!(judgement.reasons[0].value, 8.5);
    assert_eq!(judgement.reasons[0].limit, 10.0);

    let both = Allowance {
        task: task.task,
        day: Some(Cap {
            limit_usd: 100.0,
            spent_usd: 85.0,
            ahead_usd: 0.1,
        }),
    };
    assert_eq!(
        codes(&drive.book.judge(&both, drive.now_ms)),
        vec![Code::TaskBudgetNear, Code::DayBudgetNear]
    );
}

#[test]
fn the_day_budget_stops_on_the_whole_days_projection_not_this_workers_alone() {
    let drive = Drive::new();
    let allowance = Allowance {
        task: None,
        day: Some(Cap {
            limit_usd: 50.0,
            spent_usd: 40.0,
            // Five workers running, each about to spend two dollars.
            ahead_usd: 10.0,
        }),
    };
    let judgement = drive.book.judge(&allowance, drive.now_ms);
    assert_eq!(judgement.verdict, Verdict::Stop);
    assert_eq!(codes(&judgement), vec![Code::DayBudgetStop]);
    assert_eq!(judgement.reasons[0].value, 50.0);
}

#[test]
fn a_budget_already_spent_stops_even_with_nothing_to_project_from() {
    let drive = Drive::new();
    let allowance = Allowance {
        task: Some(Cap {
            limit_usd: 5.0,
            spent_usd: 5.0,
            ahead_usd: 0.0,
        }),
        day: None,
    };
    assert_eq!(
        drive.book.judge(&allowance, drive.now_ms).verdict,
        Verdict::Stop
    );
}

#[test]
fn a_budget_of_nothing_is_no_budget() {
    let drive = Drive::new();
    let allowance = Allowance {
        task: Some(Cap {
            limit_usd: 0.0,
            spent_usd: 5.0,
            ahead_usd: 5.0,
        }),
        day: None,
    };
    assert_eq!(
        drive.book.judge(&allowance, drive.now_ms).verdict,
        Verdict::Continue
    );
}

#[test]
fn the_projection_waits_for_enough_calls_and_a_first_dear_call_is_not_eight() {
    let mut drive = Drive::new();
    assert_eq!(drive.book.ahead_usd(), 0.0);
    drive.cost(2.0);
    drive.cost(2.0);
    assert_eq!(drive.book.ahead_usd(), 0.0, "two calls are not a rate");
    drive.cost(0.2);
    drive.cost(0.2);
    drive.cost(0.2);
    // The median of [2.0, 2.0, 0.2, 0.2, 0.2] is the cheap call.
    assert!((drive.book.ahead_usd() - 0.2 * f64::from(PROJECTION_STEPS)).abs() < 1e-9);
}

#[test]
fn reasons_come_most_serious_first_and_carry_their_numbers() {
    let mut drive = Drive::new();
    for step in 0..CHECKPOINT_EVERY_STEPS {
        drive.call("Bash", r#"{"command":"cargo test"}"#);
        drive.cost(if step < 194 { 0.2 } else { 0.6 });
    }
    let allowance = Allowance {
        task: Some(Cap {
            limit_usd: 1_000.0,
            spent_usd: 900.0,
            ahead_usd: 0.0,
        }),
        day: None,
    };
    let judgement = drive.book.judge(&allowance, drive.now_ms);
    assert_eq!(judgement.verdict, Verdict::Pause);
    assert_eq!(
        codes(&judgement),
        vec![
            Code::TaskBudgetNear,
            Code::ReworkLoop,
            Code::CostRising,
            Code::CheckpointDue
        ]
    );
    let verdicts: Vec<_> = judgement
        .reasons
        .iter()
        .map(|reason| reason.code.verdict())
        .collect();
    assert!(verdicts.windows(2).all(|pair| pair[0] >= pair[1]));
}

#[test]
fn an_unpriced_model_counts_its_calls_and_no_dollars() {
    let mut drive = Drive::new();
    for _ in 0..3 {
        drive.book.note_cost(None);
    }
    drive.book.note_cost(Some(f64::NAN));
    drive.book.note_cost(Some(-1.0));
    let metrics = drive.judge().metrics;
    assert_eq!(metrics.spent_usd, None);
    assert_eq!(metrics.unpriced_calls, 5);
    assert_eq!(drive.book.ahead_usd(), 0.0);
    drive.cost(1.5);
    assert_eq!(drive.book.spent_usd(), Some(1.5));
    assert_eq!(drive.judge().metrics.unpriced_calls, 5, "short by five");
}

#[test]
fn a_call_is_the_tool_and_the_whole_of_its_input() {
    let print = |tool: &str, input: &str| call_print(&payload(tool, input));
    let base = print("Read", r#"{"file_path":"/a.rs","offset":0}"#);
    assert!(base.is_some());
    assert_eq!(base, print("Read", r#"{"file_path":"/a.rs","offset":0}"#));
    assert_ne!(
        base,
        print("Read", r#"{"file_path":"/a.rs","offset":2000}"#)
    );
    assert_ne!(base, print("Read", r#"{"file_path":"/b.rs","offset":0}"#));
    assert_ne!(base, print("Grep", r#"{"file_path":"/a.rs","offset":0}"#));
    assert_eq!(call_print(r#"{"hook_event_name":"PreToolUse"}"#), None);
    assert_eq!(call_print("not json"), None);
    assert!(
        print("Bash", "{}").is_some(),
        "a call with no input is a call"
    );
}

#[test]
fn only_the_first_bytes_of_an_input_make_its_print() {
    let long = |tail: &str| {
        let words = "x".repeat(PRINT_BYTES);
        call_print(&payload(
            "Write",
            &format!(r#"{{"content":"{words}{tail}"}}"#),
        ))
    };
    assert_eq!(long("one ending"), long("another ending"));
    let short = |words: &str| call_print(&payload("Write", &format!(r#"{{"content":"{words}"}}"#)));
    assert_ne!(short("one"), short("another"));
}

#[test]
fn what_a_book_remembers_is_bounded() {
    let mut drive = Drive::new();
    for step in 0..50_000_usize {
        drive.call("Read", &format!(r#"{{"file_path":"/f{}"}}"#, step % 97));
        drive.cost(0.1);
        if step.is_multiple_of(11) {
            drive.fail();
        }
    }
    assert!(drive.book.window.len() <= REWORK_WINDOW_STEPS);
    assert!(drive.book.costs.len() <= COST_RING);
    assert_eq!(drive.judge().metrics.steps, 50_000);
}

#[test]
fn the_verdicts_rise_in_the_order_of_what_they_ask() {
    assert!(Verdict::Continue < Verdict::Checkpoint);
    assert!(Verdict::Checkpoint < Verdict::Pause);
    assert!(Verdict::Pause < Verdict::Stop);
    assert_eq!(Verdict::default(), Verdict::Continue);
    let words: Vec<_> = [
        Verdict::Continue,
        Verdict::Checkpoint,
        Verdict::Pause,
        Verdict::Stop,
    ]
    .iter()
    .map(|verdict| verdict.word())
    .collect();
    assert_eq!(words, ["continue", "checkpoint", "pause", "stop"]);
    assert_eq!(
        serde_json::to_string(&Verdict::Checkpoint).expect("serializes"),
        r#""checkpoint""#
    );
}

#[test]
fn every_reason_earns_the_verdict_its_words_say() {
    for (code, verdict) in [
        (Code::CheckpointDue, Verdict::Checkpoint),
        (Code::CostRising, Verdict::Checkpoint),
        (Code::ReworkLoop, Verdict::Pause),
        (Code::TaskBudgetNear, Verdict::Pause),
        (Code::DayBudgetNear, Verdict::Pause),
        (Code::TaskBudgetStop, Verdict::Stop),
        (Code::DayBudgetStop, Verdict::Stop),
    ] {
        assert_eq!(code.verdict(), verdict, "{code:?}");
    }
}

#[test]
fn the_gate_tells_and_does_not_end_work_unless_a_person_said_so() {
    assert_eq!(Mode::default(), Mode::Notify);
    let settings = Settings::default();
    assert_eq!(settings.mode, Mode::Notify);
    assert_eq!(settings.task_usd, None);
    assert_eq!(settings.day_usd, None);
}

#[test]
fn settings_are_read_field_by_field_and_a_budget_must_be_one() {
    let read = |value: serde_json::Value| Settings::parse(&value);
    assert_eq!(
        read(serde_json::json!({"mode":"stop","task_usd":12.5,"day_usd":80})),
        Settings {
            mode: Mode::Stop,
            task_usd: Some(12.5),
            day_usd: Some(80.0),
        }
    );
    assert_eq!(read(serde_json::json!({"mode":"off"})).mode, Mode::Off);
    assert_eq!(read(serde_json::json!({"mode":"bogus"})).mode, Mode::Notify);
    let strange = read(serde_json::json!({
        "mode": "stop",
        "task_usd": -3,
        "day_usd": "five",
    }));
    assert_eq!(
        strange.mode,
        Mode::Stop,
        "a bad budget does not take the mode"
    );
    assert_eq!(strange.task_usd, None);
    assert_eq!(strange.day_usd, None);
    for typo in [0.0, -1.0, BUDGET_USD_MAX + 1.0] {
        assert_eq!(read(serde_json::json!({ "task_usd": typo })).task_usd, None);
    }
    assert_eq!(read(serde_json::Value::Null), Settings::default());
}

/// What the settings file holds is read the way what the settings pane sends is:
/// field by field. A budget of nothing, or less, is no budget — read as one it
/// would stop a worker on its first call — and a field of the wrong type must not
/// make the whole file unreadable, because the settings document is one file and
/// an unreadable one is set aside with every other setting in it.
#[test]
fn a_stored_budget_that_is_not_a_budget_is_none_and_a_stored_mess_still_reads() {
    let stored = |json: &str| serde_json::from_str::<Settings>(json).ok();
    assert_eq!(
        stored(r#"{"mode":"stop","task_usd":0,"day_usd":-3}"#),
        Some(Settings {
            mode: Mode::Stop,
            task_usd: None,
            day_usd: None,
        }),
        "a budget of nothing is none, not a stop on the first call"
    );
    assert_eq!(
        stored(r#"{"mode":"stop","task_usd":20.5}"#),
        Some(Settings {
            mode: Mode::Stop,
            task_usd: Some(20.5),
            day_usd: None,
        })
    );
    assert_eq!(
        stored(r#"{"mode":"bogus","task_usd":"lots","day_usd":1e9}"#),
        Some(Settings::default()),
        "a field that is not what it should be falls back alone and the read goes on"
    );
    assert_eq!(
        stored("5"),
        Some(Settings::default()),
        "and so does a value that is not even an object"
    );
}

#[test]
fn every_code_has_the_word_it_is_serialized_as() {
    for code in Code::ALL {
        assert_eq!(
            serde_json::to_string(&code).expect("serializes"),
            format!("\"{}\"", code.word()),
            "{code:?}"
        );
    }
}

#[test]
fn every_verdict_has_the_word_it_is_serialized_as_and_they_rise_in_the_order_of_all() {
    for verdict in Verdict::ALL {
        assert_eq!(
            serde_json::to_string(&verdict).expect("serializes"),
            format!("\"{}\"", verdict.word()),
            "{verdict:?}"
        );
    }
    assert!(
        Verdict::ALL.windows(2).all(|pair| pair[0] < pair[1]),
        "ALL is lowest first: the board lists them that way"
    );
}
