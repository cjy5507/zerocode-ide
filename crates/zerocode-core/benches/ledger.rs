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
    Launcher, Ledger, Message, MessageKind, next_dispatch, plan, receipt_actor,
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
/// for every seated holder, so its shape (two linear walks, never a walk per
/// question) is load-bearing — the unit tests pin the shape, this pins the
/// price.
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

criterion_group!(
    ledger,
    an_empty_check_answers,
    the_pointer_walks_a_full_mailbox,
    a_disarmed_run_short_circuits_the_tick,
    an_armed_tick_picks_the_oldest_of_a_thousand,
    a_replayed_mutation_answers_from_its_receipt,
    a_boot_validates_a_big_ledger
);
criterion_main!(ledger);
