//! `recipe-run --repeat` (docs/design/flow-engine-operator-and-qa.md §2 —
//! the `event` and `schedule` triggers; §3, an episode per trigger): rounds
//! of the one walk, each a `run()` of the same document with the values the
//! caller gave. A round begins when the Flow's `## Trigger` line flips past
//! where it stood after the last round — an event, so the same notification
//! never starts two rounds and nothing is ever read off it — or at once when
//! the document has no trigger (a schedule: the caller's clock is the
//! trigger). Nothing here walks a step: the walk is the core's one loop
//! inside `run()`, the trigger is asked down the check road the oracle takes
//! (`recipe_run::ask`), and this layer only decides when the next round
//! begins and when there is none — the `--until` bound, the fail ceiling,
//! the person's stop or hand, the caller gone, or the call's own clock.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::computer_flow::{Check, Observed, Presence, judge};
use zerocode_core::computer_recipe::{RecipeStop, RecipeTool};
use zerocode_core::computer_use::{
    COMPUTER_WAIT_FOR_POLL_MS, FLOW_BASELINE_PROBE_MS, FLOW_REPEAT_MAX_FAILS, FLOW_TRIGGER_MS,
    RepeatUntil, walk_budget_ms,
};
use zerocode_core::computer_use_protocol::error_code;
use zerocode_hookd::TeamAnswer;

use super::ComputerUseError;
use super::recipe_run::{Desk, ask, verdict_of};

/// The word a repeat's answer carries as its `kind`.
pub const ANSWER_KIND: &str = "repeat";

/// What a repeat is told once: the trigger line (none for a schedule), where
/// it ends, the call's deadline, and the local clock's offset from UTC for
/// an `--until HH:MM`.
pub struct Plan<'a> {
    pub trigger: Option<&'a Check>,
    pub until: Option<RepeatUntil>,
    pub deadline_ms: u64,
    pub local_offset_secs: i64,
}

/// Why a repeat ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The `--until` time of day came.
    Clock,
    /// The `--until` count of rounds was walked.
    Rounds,
    /// `FLOW_REPEAT_MAX_FAILS` rounds failed in a row.
    Fails,
    /// The call's own clock: no round fits before the bridge gives up.
    Deadline,
    /// The caller stopped waiting.
    CallerGone,
    /// The operator was stopped, or the person is being asked: the door is
    /// shut.
    Door,
    /// A round's walk stopped for someone — the person's turn or last step,
    /// their hand, the operator's stop — or for the call's clock; the stop's
    /// own word.
    Stopped(RecipeStop),
}

impl Ended {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Clock => "until",
            Self::Rounds => "rounds",
            Self::Fails => "fails",
            Self::Deadline => "deadline",
            Self::CallerGone => "caller_gone",
            Self::Door => RecipeStop::Stopped.as_str(),
            Self::Stopped(kind) => kind.as_str(),
        }
    }

    /// The sentence a person reads.
    #[must_use]
    pub fn said(self) -> String {
        match self {
            Self::Clock => "the --until time came".to_string(),
            Self::Rounds => "the --until rounds were walked".to_string(),
            Self::Fails => format!(
                "{FLOW_REPEAT_MAX_FAILS} rounds failed in a row: a loop, not progress — look, then run again"
            ),
            Self::Deadline => {
                "the call's own time ran out before another round: run again to go on".to_string()
            }
            Self::CallerGone => "the caller stopped waiting".to_string(),
            Self::Door => {
                "the operator is stopped (or the person is being asked): resume, then run again"
                    .to_string()
            }
            Self::Stopped(kind) => kind.advice().to_string(),
        }
    }

    /// Whether the repeat ended as asked — its bound, the person, the
    /// clock — rather than on its own failures.
    #[must_use]
    pub const fn as_asked(self) -> bool {
        !matches!(self, Self::Fails)
    }
}

/// What a repeat came to: the rounds walked, the rounds that passed (the
/// Flow's verdict, or the walk's end without one), why it ended, and the
/// last round's report — whose folder and verdict the caller reads as a
/// lone walk's.
#[derive(Debug, Clone, PartialEq)]
pub struct Repeated {
    pub rounds: u32,
    pub passed: u32,
    pub ended: Ended,
    pub last: Option<Value>,
}

impl Repeated {
    /// The answer's `result`.
    #[must_use]
    pub fn value(&self) -> Value {
        json!({
            "kind": ANSWER_KIND,
            "rounds": self.rounds,
            "passed": self.passed,
            "ended": self.ended.as_str(),
            "said": self.ended.said(),
            "last": self.last,
        })
    }

    /// The answer as a person reads it in a terminal.
    #[must_use]
    pub fn text(&self) -> String {
        format!(
            "{ANSWER_KIND}: {} round(s), {} passed — ended: {} ({})",
            self.rounds,
            self.passed,
            self.ended.as_str(),
            self.ended.said()
        )
    }
}

/// Rounds of the one walk. Each round: the trigger, when the document has
/// one, is asked with the trigger's wait until it flips past where it stood
/// after the last round (`flipped`) — a look that did not flip waits out
/// the rest of the window before the next, so a check that answers at once
/// is one look a window, not a poll — then `round` walks once with a desk
/// of its own (`desk_of`) and the time the call has left. The repeat ends
/// at the plan's bound, after `FLOW_REPEAT_MAX_FAILS` failed rounds in a
/// row (a failed oracle is a failed round; a trigger that could not be
/// asked counts too), when a round's walk stopped for someone, when the
/// door is shut, when the caller leaves, or when no round fits the call's
/// clock. The values a round walks with are the caller's; nothing is read
/// off the trigger.
pub fn repeat<D: Desk, S: FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer>(
    plan: &Plan<'_>,
    mut desk_of: impl FnMut() -> D,
    mut step: S,
    mut round: impl FnMut(&mut S, D, u64) -> Result<Value, ComputerUseError>,
    door_shut: impl Fn() -> bool,
    caller_waits: impl Fn() -> bool,
) -> Repeated {
    // The repeat's own clock: the trigger's waits and the call's deadline
    // are read off it; each round gets a fresh desk.
    let clock = RefCell::new(desk_of());
    let began_epoch_ms = clock.borrow().now_epoch_ms();
    let ends_at = plan
        .until
        .and_then(|until| until.ends_at_epoch_ms(began_epoch_ms, plan.local_offset_secs));
    let rounds_asked = plan.until.and_then(RepeatUntil::rounds);
    let remaining = || plan.deadline_ms.saturating_sub(clock.borrow().elapsed_ms());
    let mut repeated = Repeated {
        rounds: 0,
        passed: 0,
        ended: Ended::Rounds,
        last: None,
    };
    let mut fails: u32 = 0;
    // Where the trigger stood after the last round — asked afresh, with no
    // wait, before the next round may begin; forgotten after each round.
    let mut stood: Option<Option<Presence>> = None;
    loop {
        let ended = if !caller_waits() {
            Some(Ended::CallerGone)
        } else if rounds_asked.is_some_and(|asked| u64::from(repeated.rounds) >= asked) {
            Some(Ended::Rounds)
        } else if ends_at.is_some_and(|at| clock.borrow().now_epoch_ms() >= at) {
            Some(Ended::Clock)
        } else if fails >= FLOW_REPEAT_MAX_FAILS {
            Some(Ended::Fails)
        } else if door_shut() {
            Some(Ended::Door)
        } else if walk_budget_ms(remaining()) == 0 {
            Some(Ended::Deadline)
        } else {
            None
        };
        if let Some(ended) = ended {
            repeated.ended = ended;
            return repeated;
        }
        if let Some(trigger) = plan.trigger {
            let stand = *stood.get_or_insert_with(|| {
                presence(ask(
                    trigger,
                    FLOW_BASELINE_PROBE_MS,
                    plan.deadline_ms,
                    &mut step,
                    &clock,
                    &caller_waits,
                ))
            });
            let asking = clock.borrow().elapsed_ms();
            let seen = ask(
                trigger,
                FLOW_TRIGGER_MS.min(walk_budget_ms(remaining())),
                plan.deadline_ms,
                &mut step,
                &clock,
                &caller_waits,
            );
            let flipped = match seen {
                Observed::Seen(now) => {
                    let flipped = flipped(trigger, stand, now);
                    if !flipped {
                        stood = Some(Some(now));
                    }
                    flipped
                }
                Observed::NotEvaluable(_) => {
                    fails += 1;
                    false
                }
            };
            if !flipped {
                let took = clock.borrow().elapsed_ms().saturating_sub(asking);
                wait_out(
                    FLOW_TRIGGER_MS.saturating_sub(took).min(remaining()),
                    &clock,
                    &caller_waits,
                );
                continue;
            }
        }
        repeated.rounds += 1;
        match round(&mut step, desk_of(), remaining()) {
            Ok(report) => {
                let stop = stop_of(&report);
                if round_passed(&report) {
                    repeated.passed += 1;
                    fails = 0;
                } else {
                    fails += 1;
                }
                repeated.last = Some(report);
                if let Some(kind) = stop.filter(|kind| ends_the_repeat(*kind)) {
                    repeated.ended = Ended::Stopped(kind);
                    return repeated;
                }
            }
            Err(refused) => {
                fails += 1;
                let door = matches!(
                    refused.code.as_str(),
                    error_code::STOPPED | error_code::PERSON_ASKED | error_code::BUDGET_EXCEEDED
                );
                repeated.last = Some(json!({
                    "ok": false,
                    "error": { "code": refused.code, "message": refused.message },
                }));
                if door {
                    repeated.ended = Ended::Door;
                    return repeated;
                }
            }
        }
        stood = None;
    }
}

/// What a look at the trigger saw, if it could be read.
fn presence(seen: Observed) -> Option<Presence> {
    match seen {
        Observed::Seen(presence) => Some(presence),
        Observed::NotEvaluable(_) => None,
    }
}

/// Whether the trigger flipped past its stand: the event judged as the
/// oracle judges one — absent then present, or one more than before; what
/// already stood is stale. Without a stand nothing can have flipped.
fn flipped(trigger: &Check, stand: Option<Presence>, now: Presence) -> bool {
    let baseline: BTreeMap<usize, Presence> = stand
        .map(|presence| (trigger.id, presence))
        .into_iter()
        .collect();
    judge(
        std::slice::from_ref(trigger),
        &baseline,
        &BTreeMap::from([(trigger.id, Observed::Seen(now))]),
    )
    .pass
}

/// The rest of a trigger's window, waited out in the check table's own
/// paces so the caller's leaving is seen within one.
fn wait_out(mut rest_ms: u64, clock: &RefCell<impl Desk>, caller_waits: &impl Fn() -> bool) {
    while rest_ms > 0 && caller_waits() {
        let pause = rest_ms.min(COMPUTER_WAIT_FOR_POLL_MS);
        clock.borrow_mut().pause(Duration::from_millis(pause));
        rest_ms -= pause;
    }
}

/// The stop a round's report names, if the walk stopped.
fn stop_of(report: &Value) -> Option<RecipeStop> {
    let word = report.pointer("/stop/kind")?.as_str()?;
    RecipeStop::ALL
        .into_iter()
        .find(|kind| kind.as_str() == word)
}

/// The stops that end a repeat: the walk stopped for someone — the person's
/// turn or last step, their hand, the operator's stop — or for the call's
/// clock. Any other stop is that round's failure, and the next round may go.
const fn ends_the_repeat(kind: RecipeStop) -> bool {
    matches!(
        kind,
        RecipeStop::PersonsTurn
            | RecipeStop::PersonsLastStep
            | RecipeStop::PersonMoved
            | RecipeStop::Stopped
            | RecipeStop::Budget
    )
}

/// Whether a round passed: its Flow's verdict when it walked one, else that
/// it walked to its end.
fn round_passed(report: &Value) -> bool {
    verdict_of(report).map_or(report["done"] == Value::Bool(true), |(pass, _)| pass)
}

#[cfg(test)]
mod tests {
    use super::super::ComputerUseError;
    use super::super::recipe_run::bench::*;
    use super::super::recipe_run::{Run, run};
    use super::*;
    use serde_json::{Value, json};
    use std::collections::{BTreeSet, VecDeque};
    use zerocode_core::computer_flow::{
        Check, CheckKind, EvidenceLevel, Fingerprint, FlowSpec, Policy,
    };
    use zerocode_core::computer_recipe::{RecipeStop, RecipeTool};
    use zerocode_core::computer_use::{
        COMPUTER_LONGEST_DEADLINE_MS, COMPUTER_USE_PROTOCOL_VERSION, FLOW_BASELINE_PROBE_MS,
        FLOW_REPEAT_MAX_FAILS, FLOW_TRIGGER_MS, RepeatUntil,
    };
    use zerocode_hookd::TeamAnswer;

    type Road<'a> = Box<dyn FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer + 'a>;

    const TRIGGER: [&str; 5] = ["wait-for", "--app", "Mail", "--text", "입금"];
    const ORACLE: [&str; 5] = ["wait-for", "--app", "Mail", "--text", "Done"];

    fn check(id: usize, kind: CheckKind, line: &[&str]) -> Check {
        Check {
            id,
            kind,
            required: true,
            tool: RecipeTool::Computer,
            argv: words(line),
        }
    }

    /// A Flow whose oracle is one state line, with or without a trigger.
    fn flow(trigger: Option<Check>) -> FlowSpec {
        FlowSpec {
            money: None,
            confirm: Default::default(),
            policy: Policy::Dry,
            evidence: EvidenceLevel::Off,
            fingerprint: Fingerprint {
                apps: ["com.apple.mail".to_string()].into_iter().collect(),
                hosts: BTreeSet::new(),
                protocol: COMPUTER_USE_PROTOCOL_VERSION,
                assets: BTreeSet::new(),
            },
            checks: vec![check(1, CheckKind::State, &ORACLE)],
            trigger,
        }
    }

    fn document(trigger: bool) -> String {
        flow_doc(
            &[(RecipeTool::Computer, "key --key tab")],
            &flow(trigger.then(|| check(1, CheckKind::Event, &TRIGGER))),
        )
    }

    fn plan<'a>(trigger: Option<&'a Check>, until: Option<RepeatUntil>) -> Plan<'a> {
        Plan {
            trigger,
            until,
            deadline_ms: COMPUTER_LONGEST_DEADLINE_MS,
            local_offset_secs: 0,
        }
    }

    /// The hold a trigger ask was given, read off its own words.
    fn budget_of(argv: &[String]) -> Option<&str> {
        argv.iter()
            .position(|word| word == "--timeout-ms")
            .and_then(|at| argv.get(at + 1))
            .map(String::as_str)
    }

    fn is_trigger(argv: &[String]) -> bool {
        argv.first().is_some_and(|verb| verb == "wait-for") && argv.contains(&"입금".to_string())
    }

    /// The trigger answered in turn from `answers` (present = ok, absent =
    /// timeout), the oracle present, every step ok — and counted.
    struct Answers<'a> {
        answers: VecDeque<bool>,
        asked: &'a std::cell::RefCell<Vec<Vec<String>>>,
        walked: &'a std::cell::Cell<usize>,
    }

    impl Answers<'_> {
        fn answer(&mut self, argv: &[String]) -> TeamAnswer {
            if is_trigger(argv) {
                self.asked.borrow_mut().push(argv.to_vec());
                return match self.answers.pop_front() {
                    Some(true) => ok(),
                    _ => refused("timeout"),
                };
            }
            if argv.first().is_some_and(|verb| verb == "key") {
                self.walked.set(self.walked.get() + 1);
            }
            ok()
        }
    }

    /// One round: the one walk over `text` from `command`.
    fn walk<'a, D: Desk>(
        command: &'a zerocode_core::computer_use::ComputerCommand,
        text: &'a str,
    ) -> impl FnMut(&mut Road<'_>, D, u64) -> Result<Value, ComputerUseError> + 'a {
        move |step, desk, deadline_ms| {
            run(
                &Run {
                    command,
                    resume_from: None,
                    file: "f.md",
                    text,
                    deadline_ms,
                    cwd: None,
                    evidence_dir: None,
                },
                &mut **step,
                |_, _, _| {},
                |_| {},
                desk,
                || true,
            )
            .map_err(|refused| ComputerUseError::new(refused.code, refused.message))
        }
    }

    fn road<'a>(
        answers: &[bool],
        asked: &'a std::cell::RefCell<Vec<Vec<String>>>,
        walked: &'a std::cell::Cell<usize>,
    ) -> Road<'a> {
        let mut road = Answers {
            answers: answers.iter().copied().collect(),
            asked,
            walked,
        };
        Box::new(move |_, argv, _| road.answer(argv))
    }

    /// A document with no trigger on a bench, repeated as the test arranges:
    /// its bound, its deadline, the door, the caller.
    struct Rig<'a> {
        bench: &'a Bench,
        command: &'a zerocode_core::computer_use::ComputerCommand,
        text: &'a str,
    }

    impl Rig<'_> {
        fn ends(
            &self,
            until: Option<RepeatUntil>,
            deadline_ms: u64,
            road: Road<'_>,
            door_shut: bool,
            waits: bool,
        ) -> Repeated {
            let mut plan = plan(None, until);
            plan.deadline_ms = deadline_ms;
            repeat(
                &plan,
                || self.bench.desk(false, None),
                road,
                walk(self.command, self.text),
                || door_shut,
                || waits,
            )
        }
    }

    /// The trigger is an event: it must flip past where it stood after the
    /// last round. A notification already there when the repeat begins is
    /// not its round's, one that stays is never a second round's, and one
    /// that went away and came back is a new one. Every ask takes the check
    /// road with the trigger's own wait; a document without a trigger walks
    /// its rounds on the caller's clock.
    #[test]
    fn a_repeat_waits_for_its_trigger_to_flip_after_the_last_round_then_walks_once() {
        let bench = Bench::new();
        let text = document(true);
        let command = command(&[]);
        let trigger = check(1, CheckKind::Event, &TRIGGER);
        let asked = std::cell::RefCell::new(Vec::new());
        let walked = std::cell::Cell::new(0);
        // Already there at the start (stale), still there (stale), gone,
        // there again (a new one): one round.
        let repeated = repeat(
            &plan(Some(&trigger), Some(RepeatUntil::Rounds(1))),
            || bench.desk(false, None),
            road(&[true, true, false, true], &asked, &walked),
            walk(&command, &text),
            || false,
            || true,
        );
        assert_eq!(
            (repeated.rounds, repeated.passed, repeated.ended),
            (1, 1, Ended::Rounds),
            "{}",
            repeated.text()
        );
        assert_eq!(walked.get(), 1, "the round walked once, after the flip");
        let asked_now = asked.borrow();
        assert_eq!(asked_now.len(), 4, "{asked_now:?}");
        assert_eq!(
            budget_of(&asked_now[0]),
            Some(FLOW_BASELINE_PROBE_MS.to_string().as_str()),
            "the stand is asked with no wait"
        );
        for ask in &asked_now[1..] {
            assert_eq!(
                budget_of(ask),
                Some(FLOW_TRIGGER_MS.to_string().as_str()),
                "one trigger wait: {ask:?}"
            );
        }
        assert!(
            bench.clock.get() >= 2 * FLOW_TRIGGER_MS,
            "a look that did not flip waits out the trigger's window before the next: {} ms",
            bench.clock.get()
        );
        drop(asked_now);
        assert!(
            repeated
                .last
                .as_ref()
                .is_some_and(|last| last["done"] == json!(true)),
            "{:?}",
            repeated.last
        );

        // The same notification never starts two rounds: after round one it
        // stands present; only a fresh flip starts round two.
        let bench = Bench::new();
        let asked = std::cell::RefCell::new(Vec::new());
        let walked = std::cell::Cell::new(0);
        let repeated = repeat(
            &plan(Some(&trigger), Some(RepeatUntil::Rounds(2))),
            || bench.desk(false, None),
            road(&[false, true, true, true, false, true], &asked, &walked),
            walk(&command, &text),
            || false,
            || true,
        );
        assert_eq!(
            (repeated.rounds, repeated.passed),
            (2, 2),
            "{}",
            repeated.text()
        );
        assert_eq!(walked.get(), 2);
        assert_eq!(asked.borrow().len(), 6, "{:?}", asked.borrow());

        // No trigger: rounds walk back to back — a schedule's call.
        let bench = Bench::new();
        let text = document(false);
        let asked = std::cell::RefCell::new(Vec::new());
        let walked = std::cell::Cell::new(0);
        let repeated = repeat(
            &plan(None, Some(RepeatUntil::Rounds(2))),
            || bench.desk(false, None),
            road(&[], &asked, &walked),
            walk(&command, &text),
            || false,
            || true,
        );
        assert_eq!(
            (repeated.rounds, repeated.passed, repeated.ended),
            (2, 2, Ended::Rounds)
        );
        assert_eq!(walked.get(), 2);
        assert!(asked.borrow().is_empty());
        assert_eq!(
            bench.clock.get(),
            0,
            "no trigger, no wait: {} ms",
            bench.clock.get()
        );
        let value = repeated.value();
        assert_eq!(
            (&value["rounds"], &value["passed"], &value["ended"]),
            (&json!(2), &json!(2), &json!("rounds"))
        );
        assert!(value["last"].is_object(), "{value}");
    }

    /// Where a repeat ends: the count of rounds, the clock, the fail ceiling
    /// (`FLOW_REPEAT_MAX_FAILS` failed rounds in a row — a failed oracle is
    /// a failed round), the operator's stop, the person's hand, the shut
    /// door, the caller gone, and the call's own deadline.
    #[test]
    fn a_repeat_ends_at_until_or_after_the_fail_ceiling_or_when_the_person_stops_it() {
        let text = document(false);
        let command = command(&[]);
        let never = std::cell::RefCell::new(Vec::new());
        fn rig<'a>(
            bench: &'a Bench,
            command: &'a zerocode_core::computer_use::ComputerCommand,
            text: &'a str,
        ) -> Rig<'a> {
            Rig {
                bench,
                command,
                text,
            }
        }
        let until = |n: u64| Some(RepeatUntil::Rounds(n));

        let bench = Bench::new();
        let walked = std::cell::Cell::new(0);
        let three = rig(&bench, &command, &text).ends(
            until(3),
            COMPUTER_LONGEST_DEADLINE_MS,
            road(&[], &never, &walked),
            false,
            true,
        );
        assert_eq!(
            (three.rounds, three.passed, three.ended),
            (3, 3, Ended::Rounds)
        );
        assert_eq!(walked.get(), 3);

        // The clock: 00:01 on a clock that reads 00:00:01 at the start, each
        // round taking twenty seconds.
        let bench = Bench::new();
        let ticking: Road<'_> = Box::new(|_, argv, _| {
            if argv.first().is_some_and(|verb| verb == "key") {
                bench.clock.set(bench.clock.get() + 20_000);
            }
            ok()
        });
        let clocked = rig(&bench, &command, &text).ends(
            Some(RepeatUntil::Clock { hour: 0, minute: 1 }),
            COMPUTER_LONGEST_DEADLINE_MS,
            ticking,
            false,
            true,
        );
        assert_eq!(
            (clocked.rounds, clocked.ended),
            (3, Ended::Clock),
            "{}",
            clocked.text()
        );

        // The fail ceiling: a step refused each round.
        let bench = Bench::new();
        let failing: Road<'_> = Box::new(|_, _, _| refused("element_not_found"));
        let failed = rig(&bench, &command, &text).ends(
            None,
            COMPUTER_LONGEST_DEADLINE_MS,
            failing,
            false,
            true,
        );
        assert_eq!(
            (failed.rounds, failed.passed, failed.ended),
            (FLOW_REPEAT_MAX_FAILS, 0, Ended::Fails),
            "{}",
            failed.text()
        );
        assert!(
            failed
                .last
                .as_ref()
                .is_some_and(|last| last["stop"]["code"] == json!("element_not_found")),
            "the last round's report rides the answer: {:?}",
            failed.last
        );
        // A failed oracle is a failed round, though every step walked.
        let bench = Bench::new();
        let unmet: Road<'_> = Box::new(|_, argv, _| {
            if argv.first().is_some_and(|verb| verb == "wait-for") {
                refused("timeout")
            } else {
                ok()
            }
        });
        let unmet = rig(&bench, &command, &text).ends(
            None,
            COMPUTER_LONGEST_DEADLINE_MS,
            unmet,
            false,
            true,
        );
        assert_eq!(
            (unmet.rounds, unmet.passed, unmet.ended),
            (FLOW_REPEAT_MAX_FAILS, 0, Ended::Fails)
        );
        // A pass in between resets the count.
        let bench = Bench::new();
        let every_other = std::cell::Cell::new(0u32);
        let alternating: Road<'_> = Box::new(|_, argv, _| {
            if argv.first().is_some_and(|verb| verb == "key") {
                every_other.set(every_other.get() + 1);
                if every_other.get() % 2 == 1 {
                    return refused("element_not_found");
                }
            }
            ok()
        });
        let alternated = rig(&bench, &command, &text).ends(
            until(6),
            COMPUTER_LONGEST_DEADLINE_MS,
            alternating,
            false,
            true,
        );
        assert_eq!(
            (alternated.rounds, alternated.passed, alternated.ended),
            (6, 3, Ended::Rounds)
        );

        // The operator's stop, the person's hand, the shut door.
        let bench = Bench::new();
        let stopped: Road<'_> = Box::new(|_, _, _| refused("stopped"));
        let stopped = rig(&bench, &command, &text).ends(
            None,
            COMPUTER_LONGEST_DEADLINE_MS,
            stopped,
            false,
            true,
        );
        assert_eq!(
            (stopped.rounds, stopped.ended),
            (1, Ended::Stopped(RecipeStop::Stopped))
        );
        assert_eq!(stopped.ended.as_str(), "stopped");
        let bench = Bench::at(Some((0.0, 0.0)));
        let moving: Road<'_> = Box::new(|_, _, _| {
            bench.pointer.set(Some((100.0, 100.0)));
            ok()
        });
        let moved = rig(&bench, &command, &text).ends(
            None,
            COMPUTER_LONGEST_DEADLINE_MS,
            moving,
            false,
            true,
        );
        assert_eq!(
            (moved.rounds, moved.ended),
            (1, Ended::Stopped(RecipeStop::PersonMoved))
        );
        let bench = Bench::new();
        let walked = std::cell::Cell::new(0);
        let shut = rig(&bench, &command, &text).ends(
            None,
            COMPUTER_LONGEST_DEADLINE_MS,
            road(&[], &never, &walked),
            true,
            true,
        );
        assert_eq!((shut.rounds, shut.ended), (0, Ended::Door));
        assert_eq!(walked.get(), 0, "a shut door walks nothing");

        // The caller gone, and the call's own deadline.
        let bench = Bench::new();
        let gone = rig(&bench, &command, &text).ends(
            None,
            COMPUTER_LONGEST_DEADLINE_MS,
            road(&[], &never, &walked),
            false,
            false,
        );
        assert_eq!((gone.rounds, gone.ended), (0, Ended::CallerGone));
        let bench = Bench::new();
        let late: Road<'_> = Box::new(|_, argv, _| {
            if argv.first().is_some_and(|verb| verb == "key") {
                bench.clock.set(bench.clock.get() + DEADLINE);
            }
            ok()
        });
        let late = rig(&bench, &command, &text).ends(None, DEADLINE, late, false, true);
        assert_eq!(
            (late.rounds, late.ended),
            (1, Ended::Deadline),
            "{}",
            late.text()
        );
        assert_eq!(late.ended.as_str(), "deadline");
        assert_eq!(
            Ended::Stopped(RecipeStop::PersonMoved).as_str(),
            "person_moved"
        );
    }

    /// The repeat layer's own cost per round, the trigger's wait left out:
    /// the same walk called directly, against the same walk through
    /// `repeat` — printed, not checked.
    #[test]
    #[ignore = "a measurement, printed; not a check"]
    fn measure_repeat_overhead_per_round_ms() {
        const ROUNDS: u64 = 500;
        let bench = Bench::new();
        let text = document(false);
        let command = command(&[]);
        let never = std::cell::RefCell::new(Vec::new());
        let walked = std::cell::Cell::new(0);
        let direct = std::time::Instant::now();
        let mut lone = road(&[], &never, &walked);
        for _ in 0..ROUNDS {
            walk(&command, &text)(
                &mut lone,
                bench.desk(false, None),
                COMPUTER_LONGEST_DEADLINE_MS,
            )
            .unwrap();
        }
        let direct_ms = direct.elapsed().as_secs_f64() * 1_000.0;
        let through = std::time::Instant::now();
        let repeated = repeat(
            &plan(None, Some(RepeatUntil::Rounds(ROUNDS))),
            || bench.desk(false, None),
            road(&[], &never, &walked),
            walk(&command, &text),
            || false,
            || true,
        );
        let through_ms = through.elapsed().as_secs_f64() * 1_000.0;
        assert_eq!(repeated.rounds as u64, ROUNDS);
        println!(
            "measure: rounds={ROUNDS} direct_ms={direct_ms:.2} repeat_ms={through_ms:.2} overhead_per_round_ms={:.4} ({})",
            (through_ms - direct_ms) / ROUNDS as f64,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
    }
}
