//! The pure ledger's hot paths, measured.
//!
//! "Rust must outperform" is this product's charter line, and the original
//! keeps its own stopwatch on exactly these roads (empty-dispatch
//! short-circuit, authority at scale, federated reads). A charter without a
//! measuring stick regresses one refactor at a time; the unit tests already
//! count operations on the quadratic-prone walks, and these benches are the
//! other half — wall-clock witnesses on the paths a WINDOW pays for on every
//! beat and every verb.
//!
//! Everything here drives the ledger the way production does: through
//! [`plan`] with argv, an actor, and a seat — no test-only doors, because a
//! bench through a side door measures the side door.

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use zerocode_core::SessionKey;
use zerocode_core::agent_teams::{LEADER_PANE, Team};
use zerocode_core::orchestration::{
    Draft, Ending, Launcher, Ledger, Message, MessageKind, Priority, Waiting, deadline_look,
    look_again, newest_wall, next_dispatch, plan, receipt_actor, worker_address,
};

/// A launcher that knows every agent. Arming a standing order validates the
/// agent's name through this trait, and these benches are about the ledger —
/// an agent that cannot actually start is some window's problem, not the
/// stopwatch's.
struct AnyAgent;

impl Launcher for AnyAgent {
    fn command_for(
        &self,
        agent: &str,
        _prompt: &str,
        _tuning: &[String],
    ) -> Result<String, String> {
        Ok(agent.to_string())
    }
}

/// One verb, spoken as production speaks it, asserted green.
fn spoke(ledger: &mut Ledger, team: &mut Team, actor: &str, line: &str) -> String {
    let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
    let decided = plan(ledger, team, &AnyAgent, &argv, LEADER_PANE, 0, Some(actor));
    assert_eq!(
        decided.reply.exit_code, 0,
        "bench fixture refused `{line}`: {}",
        decided.reply.stderr
    );
    decided.reply.stdout
}

/// A ledger with one bound run, ready for whatever a scenario seeds into it.
fn a_bound_run(name: &str) -> (Ledger, Team, String, String) {
    let mut ledger = Ledger::new();
    let mut team = Team::new(format!("bench-{name}"), "bench-capability", 1);
    let actor = receipt_actor("claude", SessionKey::SessionId, name);
    let said = spoke(
        &mut ledger,
        &mut team,
        &actor,
        &format!("run-create --name {name} --retry-request open-{name}"),
    );
    let opened: serde_json::Value = serde_json::from_str(&said).expect("a run answer");
    let run_id = opened["runId"].as_str().expect("a run id").to_string();
    (ledger, team, run_id, actor)
}

/// The road a coordinator's polling loop pays for: a `check` with nothing
/// waiting. This answer is given thousands of times for every one that
/// carries mail, so its cost IS the cost of advising `check --wait` freely.
fn an_empty_check_answers(bench: &mut Criterion) {
    let (mut ledger, mut team, _, actor) = a_bound_run("empty");
    let argv: Vec<String> = vec!["check".to_string()];
    bench.bench_function("an_empty_check_answers", |timed| {
        timed.iter(|| {
            black_box(plan(
                &mut ledger,
                &mut team,
                &AnyAgent,
                black_box(&argv),
                LEADER_PANE,
                0,
                Some(&actor),
            ))
        })
    });
}

/// The beat's own read: `pointer_wanted` over a mailbox holding ten thousand
/// messages, a fifth of them question/reply threads. This runs once a second
/// for every seated holder, so its shape (the queue counted, never a walk per
/// question — a question is no door since t-8938, so its threads cost no walk
/// at all) is load-bearing — the unit tests pin the shape, this pins the price.
fn the_pointer_walks_a_full_mailbox(bench: &mut Criterion) {
    let (mut ledger, _, run_id, _) = a_bound_run("mailbox");
    let address = format!("run:{run_id}");
    for at in 0..10_000u32 {
        let (kind, thread) = match at % 10 {
            8 => (MessageKind::Question, None),
            9 => (MessageKind::Status, Some(format!("mail{}", at - 1))),
            _ => (MessageKind::Status, None),
        };
        ledger
            .send(
                &run_id,
                Message {
                    id: format!("mail{at}"),
                    from: "worker:w-1".to_string(),
                    to: address.clone(),
                    kind,
                    body: String::from("a line of report").into(),
                    subject: Default::default(),
                    priority: Default::default(),
                    payload: Default::default(),
                    thread,
                    task: None,
                    dispatch: None,
                    author_seat: None,
                    created_ms: i64::from(at),
                },
            )
            .expect("seeded mail");
    }
    let run = ledger.run(&run_id).expect("the seeded run");
    bench.bench_function("the_pointer_walks_a_full_mailbox", |timed| {
        timed.iter(|| black_box(run.pointer_wanted(black_box(&address), None)))
    });
}

/// The tick's short-circuit: a run nobody armed answers `None` before it
/// looks at anything. The original benches its own version of this door
/// ("db-empty-dispatch-shortcircuit"); ours has no db to skip, and this holds
/// it to that.
fn a_disarmed_run_short_circuits_the_tick(bench: &mut Criterion) {
    let (mut ledger, mut team, run_id, actor) = a_bound_run("disarmed");
    for at in 0..1_000u32 {
        spoke(
            &mut ledger,
            &mut team,
            &actor,
            &format!("task-create --spec slice-{at} --retry-request task-{at}"),
        );
    }
    let run = ledger.run(&run_id).expect("the seeded run");
    bench.bench_function("a_disarmed_run_short_circuits_the_tick", |timed| {
        timed.iter(|| black_box(next_dispatch(black_box(run))))
    });
}

/// The armed tick over a thousand ready tasks: one full scan for the oldest.
/// This is the price of "the queue drains in the order it was written", paid
/// once a second while a standing order runs.
fn an_armed_tick_picks_the_oldest_of_a_thousand(bench: &mut Criterion) {
    let (mut ledger, mut team, run_id, actor) = a_bound_run("armed");
    for at in 0..1_000u32 {
        spoke(
            &mut ledger,
            &mut team,
            &actor,
            &format!("task-create --spec slice-{at} --retry-request task-{at}"),
        );
    }
    spoke(
        &mut ledger,
        &mut team,
        &actor,
        "run-auto --agent claude --max 4 --retry-request arm",
    );
    let run = ledger.run(&run_id).expect("the seeded run");
    bench.bench_function("an_armed_tick_picks_the_oldest_of_a_thousand", |timed| {
        timed.iter(|| black_box(next_dispatch(black_box(run)).expect("a dispatchable")))
    });
}

/// The exactly-once road on its replay side: the same retry name, answered
/// from the receipt. Every crash-recovery story this ledger tells rests on
/// this being cheap enough to advise freely.
fn a_replayed_mutation_answers_from_its_receipt(bench: &mut Criterion) {
    let (mut ledger, mut team, _, actor) = a_bound_run("replay");
    spoke(
        &mut ledger,
        &mut team,
        &actor,
        "task-create --spec once --retry-request task-once",
    );
    let line = "task-create --spec once --retry-request task-once";
    let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
    bench.bench_function("a_replayed_mutation_answers_from_its_receipt", |timed| {
        timed.iter(|| {
            let replayed = plan(
                &mut ledger,
                &mut team,
                &AnyAgent,
                black_box(&argv),
                LEADER_PANE,
                0,
                Some(&actor),
            );
            assert_eq!(replayed.reply.exit_code, 0, "{}", replayed.reply.stderr);
            black_box(replayed)
        })
    });
}

/// The boot road: `validate_loaded` over a ledger holding a thousand tasks
/// and ten thousand messages. This is the one function whose input is a FILE,
/// so it is the one place a quadratic regression would greet the user at
/// startup — the unit tests keep it sets-not-walks, this keeps it honest on
/// the clock.
fn a_boot_validates_a_big_ledger(bench: &mut Criterion) {
    let (mut ledger, mut team, run_id, actor) = a_bound_run("boot");
    for at in 0..1_000u32 {
        spoke(
            &mut ledger,
            &mut team,
            &actor,
            &format!("task-create --spec slice-{at} --retry-request task-{at}"),
        );
    }
    let address = format!("run:{run_id}");
    for at in 0..10_000u32 {
        ledger
            .send(
                &run_id,
                Message {
                    id: format!("mail{at}"),
                    from: "worker:w-1".to_string(),
                    to: address.clone(),
                    kind: MessageKind::Status,
                    body: String::from("a line of report").into(),
                    subject: Default::default(),
                    priority: Default::default(),
                    payload: Default::default(),
                    thread: None,
                    task: None,
                    dispatch: None,
                    author_seat: None,
                    created_ms: i64::from(at),
                },
            )
            .expect("seeded mail");
    }
    bench.bench_function("a_boot_validates_a_big_ledger", |timed| {
        timed.iter(|| ledger.validate_loaded().expect("a coherent ledger"))
    });
}

/// The stall sweep's wall question for a quiet worker whose attempt never
/// walled (t-6427): one scan of the run's mail, newest first, that finds no
/// `quota_walled` row. The sweep asks it once a second for every quiet
/// worker, so on the beat with no walls this is the whole of what the wait
/// rung costs — ten thousand rows, a month of a busy run's mail.
fn a_wallless_attempt_reads_its_wall_in_one_scan(bench: &mut Criterion) {
    let (mut ledger, _, run_id, _) = a_bound_run("wallless");
    let address = format!("run:{run_id}");
    for at in 0..10_000u32 {
        ledger
            .send(
                &run_id,
                Message {
                    id: format!("mail{at}"),
                    from: "worker:w-1".to_string(),
                    to: address.clone(),
                    kind: MessageKind::Status,
                    body: String::from("a line of report").into(),
                    subject: Default::default(),
                    priority: Default::default(),
                    payload: Default::default(),
                    thread: None,
                    task: None,
                    dispatch: Some(format!("dp-{}", at % 7)),
                    author_seat: None,
                    created_ms: i64::from(at),
                },
            )
            .expect("seeded mail");
    }
    let run = ledger.run(&run_id).expect("the seeded run");
    bench.bench_function("a_wallless_attempt_reads_its_wall_in_one_scan", |timed| {
        timed.iter(|| black_box(newest_wall(black_box(run), black_box("dp-3"))))
    });
}

/// A run with ten thousand rows of mail, one worker seated at `%2`, and one
/// open question to it from the coordinator — the shape every receiver
/// notice (t-6740) is written against. Answers the ledger, the seat, the
/// question's id and the asker's wait.
fn a_busy_run_with_one_open_question() -> (Ledger, (String, String), String, Waiting) {
    let (mut ledger, team, run_id, _) = a_bound_run("receiver");
    let seat = (team.id.clone(), "%2".to_string());
    let started = ledger
        .start_worker(&run_id, "claude", (&seat.0, &seat.1), None, 0)
        .expect("a seated receiver");
    let address = format!("run:{run_id}");
    for at in 0..10_000u32 {
        ledger
            .send(
                &run_id,
                Message {
                    id: format!("mail{at}"),
                    from: "worker:w-9".to_string(),
                    to: address.clone(),
                    kind: MessageKind::Status,
                    body: String::from("a line of report").into(),
                    subject: Default::default(),
                    priority: Default::default(),
                    payload: Default::default(),
                    thread: None,
                    task: None,
                    dispatch: Some(format!("dp-{}", at % 7)),
                    author_seat: None,
                    created_ms: i64::from(at),
                },
            )
            .expect("seeded mail");
    }
    let question = ledger
        .post(
            &run_id,
            Draft {
                from: address.clone(),
                to: worker_address(&started.worker),
                kind: MessageKind::Question,
                body: String::from("which branch?").into(),
                subject: Default::default(),
                priority: Priority::Normal,
                payload: Default::default(),
                thread: None,
                task: None,
                dispatch: None,
            },
            10_001,
        )
        .expect("the coordinator's question");
    let waiting = Waiting {
        run: run_id,
        address,
        kinds: Vec::new(),
        peek: false,
        deadline_ms: None,
        acked: false,
        format: false,
        thread: Some(question.clone()),
        seat: format!("{}/{LEADER_PANE}", team.id),
    };
    (ledger, seat, question, waiting)
}

/// The road every turn end in the window pays for once the seat has an
/// asker (t-6740): the receivers at the seat are found, the open questions
/// to them walked, and — the common beat — the same fact already told, so
/// nothing is written.
fn a_receivers_turn_end_is_told_once_against_ten_thousand_rows(bench: &mut Criterion) {
    let (mut ledger, seat, _, _) = a_busy_run_with_one_open_question();
    assert_eq!(
        ledger.receivers_told_turn_ended((&seat.0, &seat.1), 20_000, false, 20_001),
        1,
        "the first turn end tells the asker"
    );
    bench.bench_function(
        "a_receivers_turn_end_is_told_once_against_ten_thousand_rows",
        |timed| {
            timed.iter(|| {
                black_box(ledger.receivers_told_turn_ended(
                    (&seat.0, &seat.1),
                    black_box(20_000),
                    false,
                    black_box(20_002),
                ))
            })
        },
    );
}

/// The rare beat: a fresh fact, and one line written for the asker.
fn a_receivers_fresh_fact_writes_one_line(bench: &mut Criterion) {
    let (ledger, seat, _, _) = a_busy_run_with_one_open_question();
    let projected = ledger.export();
    bench.bench_function("a_receivers_fresh_fact_writes_one_line", |timed| {
        timed.iter_batched(
            || Ledger::rebuild(projected.clone()).expect("the seeded run rebuilds"),
            |mut fresh| {
                black_box(fresh.receivers_told_turn_ended(
                    (&seat.0, &seat.1),
                    black_box(20_000),
                    false,
                    black_box(20_001),
                ))
            },
            criterion::BatchSize::LargeInput,
        )
    });
}

/// The woken look of a blocked `ask` whose receiver is gone (t-6740): a
/// read that ends the wait, paid once per bell.
fn a_gone_receiver_wakes_the_asker_in_one_look(bench: &mut Criterion) {
    let (mut ledger, seat, _, waiting) = a_busy_run_with_one_open_question();
    let worker = ledger
        .run(&waiting.run)
        .and_then(|run| run.worker_in_pane(&seat.0, &seat.1))
        .map(|held| held.id.clone())
        .expect("the receiver");
    ledger
        .end_attempt(&worker, Ending::Stopped, "bench", 20_000)
        .expect("the receiver stopped");
    assert!(look_again(&mut ledger, &waiting).is_some(), "the wake");
    bench.bench_function("a_gone_receiver_wakes_the_asker_in_one_look", |timed| {
        timed.iter(|| black_box(look_again(&mut ledger, black_box(&waiting))))
    });
}

/// The deadline answer of a blocked `ask`: the woken look once more, then
/// the last word about the receiver, read off the thread.
fn a_timed_out_ask_reads_its_receivers_last_word(bench: &mut Criterion) {
    let (mut ledger, seat, _, waiting) = a_busy_run_with_one_open_question();
    ledger.receivers_told_turn_ended((&seat.0, &seat.1), 20_000, false, 20_001);
    bench.bench_function("a_timed_out_ask_reads_its_receivers_last_word", |timed| {
        timed.iter(|| black_box(deadline_look(&ledger, black_box(&waiting))))
    });
}

criterion_group!(
    ledger,
    an_empty_check_answers,
    the_pointer_walks_a_full_mailbox,
    a_disarmed_run_short_circuits_the_tick,
    an_armed_tick_picks_the_oldest_of_a_thousand,
    a_replayed_mutation_answers_from_its_receipt,
    a_boot_validates_a_big_ledger,
    a_wallless_attempt_reads_its_wall_in_one_scan,
    a_receivers_turn_end_is_told_once_against_ten_thousand_rows,
    a_receivers_fresh_fact_writes_one_line,
    a_gone_receiver_wakes_the_asker_in_one_look,
    a_timed_out_ask_reads_its_receivers_last_word
);
criterion_main!(ledger);
