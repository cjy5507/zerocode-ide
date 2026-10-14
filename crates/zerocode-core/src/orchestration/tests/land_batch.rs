//! t-45315 (T3 stage 6): `land-batch` is the coordinator's verb. The ledger chooses the members and
//! hands the window its work; every record of a batch comes back as the window's receipt, judged
//! against the batch's records before the ledger writes it as its own letter to the coordinator.

use super::*;
use crate::orchestration::land_batch::{
    Action, BATCH_MEMBERS_MAX, BatchRecord, BatchState, INTEGRATION_BATCH_KEY, LAND_BATCH_VERB,
    LandBatchAsk, LandBatchReceipt, Landed, Member, Press, PressedBy, Record, Standing, Verdict,
    batches, bisect_next, integration_queue_supported, is_batch_id, operation_id, person_press,
    press,
};
use crate::orchestration::landing_watch::{
    CLEANABLE_AFTER_MS, GitSays, LandingWitness, MERGE_OVERDUE_MS, MERGED_UNRECORDED_GRACE_MS,
    RENOTIFY_MS, VERIFY_OVERDUE_MS,
};

const CHECKOUT: &str = "/r/checkout";
const BRANCH: &str = "refs/heads/main";
const BATCH: &str = "lb-1791940000000-1";

/// B: the base branch's commit the batch is chosen on.
fn base() -> String {
    format!("0951f1a83{:031x}", 1)
}

/// Where the base branch went while a batch waited.
fn moved_base() -> String {
    format!("77aa88b00{:031x}", 2)
}

/// S(step) of generation `generation`.
fn step_commit(generation: u32, step: u64) -> String {
    format!(
        "{:040x}",
        0x5000_0000_u64 + u64::from(generation) * 0x100 + step
    )
}

/// The tip's tree of generation `generation`.
fn tree_of(generation: u32) -> String {
    format!("{:040x}", 0x7ee0_0000_u64 + u64::from(generation))
}

/// An absolute path on either platform: `/abs/…` is no absolute path on Windows.
fn abs(rest: &str) -> String {
    if cfg!(windows) {
        format!("C:/abs/{rest}")
    } else {
        format!("/abs/{rest}")
    }
}

/// One handed-in task: the attempt that handed it in, the whole commit it handed in, and the pane
/// of the worker that did.
struct Ready {
    task: String,
    dispatch: String,
    head: String,
    pane: String,
}

/// A task a seated worker handed in at a whole commit id — reviewed `verified` by the coordinator
/// against that attempt and that head when `verified` says so.
fn handed(bench: &mut Bench, verified: bool) -> Ready {
    if bench.ledger.runs().is_empty() {
        bench.json("run-create --name batch");
    }
    let n = bench.ledger.runs()[0].tasks.len() as u64 + 1;
    let head = format!("{:040x}", 0xb000_0000_u64 + n);
    let task = bench.json(&format!("task-create --spec batch-member-{n}"))["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (_worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    assert!(
        bench.ledger.worker_seated(("team-1", &pane), CHECKOUT),
        "the worker was seated in its checkout"
    );
    let dispatch = bench.ledger.runs()[0]
        .dispatches
        .last()
        .expect("an attempt")
        .id
        .clone();
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{head}\"}}"),
    );
    if verified {
        bench.json(&format!(
            "task-update --task {task} --result {{\"verified\":true}} --attempt {dispatch} \
             --source {head}"
        ));
    }
    Ready {
        task,
        dispatch,
        head,
        pane,
    }
}

/// `count` tasks ready to be batched.
fn ready(bench: &mut Bench, count: usize) -> Vec<Ready> {
    (0..count).map(|_| handed(bench, true)).collect()
}

/// The ask the coordinator's verb planned, or a panic that says what it planned instead.
fn asked(planned: &Decided) -> LandBatchAsk {
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    match planned.effect.clone() {
        Effect::LandBatch(ask) => *ask,
        other => panic!("land-batch planned no batch work: {other:?}"),
    }
}

/// A refusal of `line` that names `reason` and hands the window nothing.
fn refused(bench: &mut Bench, line: &str, reason: &str) {
    let planned = bench.run(line);
    assert_ne!(planned.reply.exit_code, 0, "`{line}` was not refused");
    assert!(
        planned.reply.stderr.contains(reason),
        "`{line}` was refused, but not for {reason}: {}",
        planned.reply.stderr
    );
    assert!(
        matches!(planned.effect, Effect::None),
        "`{line}`: {:?}",
        planned.effect
    );
}

/// The bench's coordinator as a presser: its run and the generation of its seat.
fn coordinator(bench: &Bench) -> PressedBy {
    let run = &bench.ledger.runs()[0];
    let seat = run
        .coordinator_live()
        .expect("the bench's run has a live coordinator seat");
    PressedBy::Coordinator {
        run: run.id.clone(),
        seat: seat.seat.clone(),
        generation: seat.generation,
    }
}

/// The window's receipt for one record of [`BATCH`].
fn receipt(bench: &Bench, generation: u32, by: &PressedBy, record: Record) -> LandBatchReceipt {
    LandBatchReceipt {
        run: bench.ledger.runs()[0].id.clone(),
        batch: BATCH.to_string(),
        generation,
        by: by.clone(),
        record,
    }
}

/// One receipt written the way the window writes it, answering the batch as it stands after.
fn file(bench: &mut Bench, receipt: &LandBatchReceipt) -> BatchState {
    bench.clock += 1;
    let at = bench.clock;
    bench
        .ledger
        .land_batched(receipt, at)
        .unwrap_or_else(|why| panic!("{} was refused: {why}", receipt.record.phase()))
        .state
}

/// The coordinator chooses `ready` in order, and the window writes the choice once its own checks
/// passed. Answers the presser and the batch.
fn chosen(bench: &mut Bench, ready: &[Ready]) -> (PressedBy, BatchState) {
    let names: Vec<&str> = ready.iter().map(|one| one.task.as_str()).collect();
    let ask = asked(&bench.run(&format!(
        "{LAND_BATCH_VERB} --members {} --message-dir {}",
        names.join(","),
        abs("messages")
    )));
    let Action::Choose {
        members, checkout, ..
    } = ask.action
    else {
        panic!("a choice was asked for");
    };
    let choice = receipt(
        bench,
        1,
        &ask.by,
        Record::Chosen {
            members,
            base_branch: BRANCH.to_string(),
            base: base(),
            repository: checkout,
        },
    );
    let state = file(bench, &choice);
    (ask.by, state)
}

/// The window stacks the batch's generation: one commit per member, the tip's tree.
fn stacked(bench: &mut Bench, by: &PressedBy, state: &BatchState) -> BatchState {
    let generation = state.generation;
    let run = bench.ledger.runs()[0].id.clone();
    let begun = receipt(
        bench,
        generation,
        by,
        Record::Stacking {
            operation: operation_id(&run, BATCH, generation),
            lease_until_ms: bench.clock + 60_000,
        },
    );
    file(bench, &begun);
    let steps: Vec<String> = (1..=state.members.len() as u64)
        .map(|step| step_commit(generation, step))
        .collect();
    let tip = steps.last().cloned().expect("a member");
    let done = receipt(
        bench,
        generation,
        by,
        Record::Stacked {
            steps,
            tip,
            tree: tree_of(generation),
            took_ms: 1_200,
            overlaps: 0,
        },
    );
    file(bench, &done)
}

/// The step the batch asks to gate next — the tip, or the next step of a search — prepared and
/// recorded through the coordinator's presses, with the window's receipts for what it saw and
/// `rc_of(step)` as the whole gate set's exit code. Answers the step and the batch after.
fn gated(bench: &mut Bench, by: &PressedBy, rc_of: &dyn Fn(u32) -> i32) -> (u32, BatchState) {
    let ask = asked(&bench.run(&format!("{LAND_BATCH_VERB} --batch {BATCH} --prepare")));
    let Action::Prepare {
        batch,
        step,
        target,
    } = ask.action
    else {
        panic!("a gate was asked for");
    };
    let check = format!("lc-{}-{step}", 1_791_940_000_000_i64 + bench.clock);
    let prepared = receipt(
        bench,
        batch.generation,
        by,
        Record::GatePrepared {
            check: check.clone(),
            step,
            target: target.clone(),
            tree: tree_of(batch.generation),
            folder: abs(&format!("land-check/work/{check}")),
            log: abs(&format!("land-check/logs/{check}.log")),
        },
    );
    file(bench, &prepared);
    let rc = rc_of(step);
    let summary = abs("gate/rc-summary.txt");
    let ask = asked(&bench.run(&format!(
        "{LAND_BATCH_VERB} --batch {BATCH} --record {check} --rc {rc} --log {summary} --took-ms \
         1834200"
    )));
    assert!(
        matches!(ask.action, Action::Record { .. }),
        "a record was asked for: {:?}",
        ask.action
    );
    let ended = receipt(
        bench,
        batch.generation,
        by,
        Record::Gated {
            check,
            step,
            target,
            tree: tree_of(batch.generation),
            verdict: Verdict::of(rc),
            rc,
            took_ms: Some(1_834_200),
            log: summary,
        },
    );
    (step, file(bench, &ended))
}

/// Each member as it landed: its head and the merge commit that carried it.
fn landed(state: &BatchState) -> Vec<Landed> {
    let steps = &state.stacked.as_ref().expect("stacked").steps;
    state
        .members
        .iter()
        .zip(steps)
        .map(|(member, step)| Landed {
            task: member.task.clone(),
            head: member.head.clone(),
            merged_as: step.clone(),
        })
        .collect()
}

/// The phases of every batch letter, oldest first.
fn phases(bench: &Bench) -> Vec<String> {
    bench.ledger.runs()[0]
        .messages()
        .iter()
        .filter_map(BatchRecord::of)
        .map(|record| record.record.phase().to_string())
        .collect()
}

/// The newest batch letter's line and payload record.
fn newest(bench: &Bench) -> (String, serde_json::Value) {
    let run = &bench.ledger.runs()[0];
    let row = run
        .messages()
        .iter()
        .rev()
        .find(|row| BatchRecord::of(row).is_some())
        .expect("a batch letter");
    let payload: serde_json::Value =
        serde_json::from_str(row.payload.as_str()).expect("the payload is JSON");
    (
        row.body.as_str().to_string(),
        payload[INTEGRATION_BATCH_KEY].clone(),
    )
}

/// Default off, the ledger's half: choosing writes nothing in the ledger — no batch task, no letter,
/// no record — and only hands the window its work. The window reads the repository's switch before
/// any receipt, so a repository that never turned the queue on refuses with nothing written; that
/// half is the window's test of the same name.
#[test]
fn default_off_refuses_and_writes_nothing() {
    let mut bench = Bench::new();
    let ready = ready(&mut bench, 2);
    let tasks = bench.ledger.runs()[0].tasks.len();
    let letters = bench.ledger.runs()[0].messages().len();
    let planned = bench.run(&format!(
        "{LAND_BATCH_VERB} --members {},{} --message-dir {}",
        ready[0].task,
        ready[1].task,
        abs("messages")
    ));
    let ask = asked(&planned);
    assert!(
        matches!(ask.action, Action::Choose { .. }),
        "{:?}",
        ask.action
    );
    assert_eq!(
        planned.reply.stdout.trim(),
        "null",
        "the answer is the window's to give"
    );
    let run = &bench.ledger.runs()[0];
    assert_eq!(
        run.tasks.len(),
        tasks,
        "no batch task before the window's receipt"
    );
    assert_eq!(
        run.messages().len(),
        letters,
        "no letter before the window's receipt"
    );
    assert!(
        batches(run).is_empty(),
        "no batch before the window's receipt"
    );
}

/// The choice is written once the window's checks passed: the members in the order named, each at
/// the whole head its worker handed in, the base branch and B, who pressed — the coordinator, with
/// its run and seat generation — and when, in the ledger's own letter to the coordinator about a
/// batch task the ledger opened and holds blocked.
#[test]
fn choosing_records_members_order_base_and_who() {
    let mut bench = Bench::new();
    let ready = ready(&mut bench, 3);
    let order = [&ready[2], &ready[0], &ready[1]];
    let named: Vec<&str> = order.iter().map(|one| one.task.as_str()).collect();
    let ask = asked(&bench.run(&format!(
        "{LAND_BATCH_VERB} --members {} --message-dir {}",
        named.join(","),
        abs("messages")
    )));
    let want: Vec<Member> = order
        .iter()
        .map(|one| Member {
            task: one.task.clone(),
            head: one.head.clone(),
        })
        .collect();
    let Action::Choose {
        members,
        checkout,
        message_dir,
    } = ask.action.clone()
    else {
        panic!("a choice was asked for: {:?}", ask.action);
    };
    assert_eq!(
        members, want,
        "the members in the order named, each at the head its worker handed in"
    );
    assert_eq!(checkout, CHECKOUT, "the checkout names the repository");
    assert_eq!(message_dir, abs("messages"));
    let who = coordinator(&bench);
    assert_eq!(
        ask.by, who,
        "the coordinator pressed, with its run and seat generation"
    );

    let choice = receipt(
        &bench,
        1,
        &ask.by,
        Record::Chosen {
            members: want.clone(),
            base_branch: BRANCH.to_string(),
            base: base(),
            repository: checkout,
        },
    );
    bench.clock += 1;
    let at = bench.clock;
    let written = bench
        .ledger
        .land_batched(&choice, at)
        .expect("the choice is written");
    assert_eq!(written.letters.len(), 1, "one letter for one record");
    let run = &bench.ledger.runs()[0];
    let letter = run
        .messages()
        .iter()
        .find(|row| row.id == written.letters[0])
        .expect("the letter");
    assert_eq!(letter.kind, MessageKind::LandCheck);
    assert_eq!(letter.from, LEDGER_ITSELF);
    assert_eq!(letter.to, run.address());
    assert_eq!(letter.created_ms, at, "written at the ledger's time");
    let body: serde_json::Value =
        serde_json::from_str(letter.payload.as_str()).expect("the payload is JSON");
    let record = &body[INTEGRATION_BATCH_KEY];
    assert_eq!(record["batch"], BATCH);
    assert_eq!(record["generation"], 1);
    assert_eq!(record["phase"], "chosen");
    assert_eq!(record["at"], at);
    let PressedBy::Coordinator {
        run: pressed_in,
        seat,
        generation,
    } = &who
    else {
        panic!("the coordinator pressed");
    };
    assert_eq!(
        record["by"],
        serde_json::json!({
            "kind": "coordinator",
            "run": pressed_in,
            "seat": seat,
            "generation": generation,
        })
    );
    let heads: Vec<serde_json::Value> = want
        .iter()
        .map(|one| serde_json::json!({"task": one.task, "head": one.head}))
        .collect();
    assert_eq!(
        record["value"]["members"],
        serde_json::Value::Array(heads),
        "the members in order, each at its whole head"
    );
    assert_eq!(record["value"]["baseBranch"], BRANCH);
    assert_eq!(record["value"]["base"], base());
    let task = run.task(&written.state.task).expect("the batch task");
    assert_eq!(
        task.title.as_str(),
        format!("묶음 {BATCH}: {}", named.join(", "))
    );
    assert_eq!(
        task.status,
        TaskStatus::Blocked,
        "held so no beat sends a worker at it"
    );
    assert_eq!(letter.task.as_deref(), Some(task.id.as_str()));
    assert!(
        letter.body.as_str().starts_with(&format!(
            "{LAND_BATCH_VERB} {BATCH}: chosen 3 tasks on main 0951f1a"
        )),
        "{}",
        letter.body.as_str()
    );
    assert_eq!(written.state.standing, Standing::Chosen);
    assert_eq!(written.state.members, want);
    assert_eq!(written.state.chosen_by, who);
}

/// A member that is not ready refuses the whole choice and leaves nothing behind: no coordinator
/// review saying verified, no head handed in, already merged, in another open batch, or one task
/// too many. The window's receipt is judged by the same rules.
#[test]
fn choosing_refuses_unready_members_without_half_a_batch() {
    let mut bench = Bench::new();
    let good = ready(&mut bench, 2);
    let unverified = handed(&mut bench, false);
    let merged = handed(&mut bench, true);
    bench.json(&format!(
        "task-update --task {} --result {{\"verified\":true,\"merged\":true}} --attempt {} \
         --source {}",
        merged.task, merged.dispatch, merged.head
    ));
    // Still at work: its attempt has not ended, so nothing was handed in.
    let working = bench.json("task-create --spec still-working")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    bench.seat(&format!("worker-start --agent claude --task {working}"));
    let (by, _) = chosen(&mut bench, &good[..1]);
    let tasks = bench.ledger.runs()[0].tasks.len();
    let letters = bench.ledger.runs()[0].messages().len();
    let one_too_many: Vec<String> = (1..=BATCH_MEMBERS_MAX + 1)
        .map(|k| format!("t-{k}"))
        .collect();
    for (members, reason) in [
        (
            format!("{},{}", good[1].task, unverified.task),
            "not verified".to_string(),
        ),
        (
            format!("{},{working}", good[1].task),
            "handed-in head".to_string(),
        ),
        (
            format!("{},{}", good[1].task, merged.task),
            "already merged".to_string(),
        ),
        (
            format!("{},{}", good[1].task, good[0].task),
            "open batch".to_string(),
        ),
        (
            one_too_many.join(","),
            format!("at most {BATCH_MEMBERS_MAX}"),
        ),
    ] {
        refused(
            &mut bench,
            &format!(
                "{LAND_BATCH_VERB} --members {members} --message-dir {}",
                abs("messages")
            ),
            &reason,
        );
    }

    let mut half = receipt(
        &bench,
        1,
        &by,
        Record::Chosen {
            members: vec![
                Member {
                    task: good[1].task.clone(),
                    head: good[1].head.clone(),
                },
                Member {
                    task: unverified.task.clone(),
                    head: unverified.head.clone(),
                },
            ],
            base_branch: BRANCH.to_string(),
            base: base(),
            repository: CHECKOUT.to_string(),
        },
    );
    half.batch = "lb-1791940000000-2".to_string();
    bench.clock += 1;
    let refusal = bench
        .ledger
        .land_batched(&half, bench.clock)
        .expect_err("a choice with an unverified member is refused");
    assert!(refusal.contains("not verified"), "{refusal}");
    let run = &bench.ledger.runs()[0];
    assert_eq!(run.tasks.len(), tasks, "no batch task was left behind");
    assert_eq!(run.messages().len(), letters, "no letter was left behind");
    assert_eq!(
        batches(run).len(),
        1,
        "only the batch that was chosen stands"
    );
}

/// A launcher standing in for a machine the queue does not run on.
struct OffMac;

impl Launcher for OffMac {
    fn command_for(
        &self,
        agent: &str,
        _prompt: &str,
        _tuning: &[String],
    ) -> Result<String, String> {
        Err(format!("no agent here is called {agent}"))
    }

    fn integration_queue_supported(&self) -> bool {
        false
    }
}

/// Off macOS the verb refuses before anything is written. The platform judgment is one function,
/// read through the launcher so a test can stand in for either machine.
#[test]
fn off_macos_the_verb_refuses() {
    assert_eq!(
        integration_queue_supported(),
        cfg!(target_os = "macos"),
        "the one platform judgment answers macOS"
    );
    let mut bench = Bench::new();
    let ready = ready(&mut bench, 1);
    let line = format!(
        "{LAND_BATCH_VERB} --members {} --message-dir {}",
        ready[0].task,
        abs("messages")
    );
    let letters = bench.ledger.runs()[0].messages().len();
    let mut argv = words(&line);
    argv.push(RETRY_REQUEST.to_string());
    argv.push("off-macos-1".to_string());
    bench.clock += 1;
    let who = bench_actor(&bench.team.id, agent_teams::LEADER_PANE);
    let planned = plan_with_gate_job(
        &mut bench.ledger,
        &mut bench.team,
        &OffMac,
        &argv,
        agent_teams::LEADER_PANE,
        bench.clock,
        Some(&who),
        &crate::orchestration::evidence::ReceiptFiles::default(),
        None,
    );
    assert_ne!(
        planned.reply.exit_code, 0,
        "the press was not refused off macOS"
    );
    assert!(
        planned.reply.stderr.contains("only on macOS"),
        "{}",
        planned.reply.stderr
    );
    assert!(
        matches!(planned.effect, Effect::None),
        "{:?}",
        planned.effect
    );
    assert_eq!(
        bench.ledger.runs()[0].messages().len(),
        letters,
        "nothing was written"
    );
    let ask = asked(&bench.run(&line));
    assert!(
        matches!(ask.action, Action::Choose { .. }),
        "on a machine the queue runs on the same line is carried: {:?}",
        ask.action
    );
}

/// The gate's record, the ledger's half: `--record` names the prepared check, the whole gate set's
/// exit code and the per-job RC summary; the ledger writes the tip's verdict, and only rc 0 is green.
#[test]
fn record_writes_green_or_red_for_the_tip() {
    let mut bench = Bench::new();
    let ready = ready(&mut bench, 2);
    let (by, state) = chosen(&mut bench, &ready);
    let state = stacked(&mut bench, &by, &state);
    let tip = state.stacked.as_ref().expect("stacked").tip.clone();

    let ask = asked(&bench.run(&format!("{LAND_BATCH_VERB} --batch {BATCH} --prepare")));
    let Action::Prepare { step, target, .. } = ask.action else {
        panic!("a gate was asked for");
    };
    assert_eq!(
        (step, target.as_str()),
        (2, tip.as_str()),
        "the gate checks the tip, S(n)"
    );
    let check = "lc-1791940000000-7".to_string();
    let summary = abs("gate/rc-summary.txt");
    let prepared = receipt(
        &bench,
        1,
        &by,
        Record::GatePrepared {
            check: check.clone(),
            step,
            target: tip.clone(),
            tree: tree_of(1),
            folder: abs(&format!("land-check/work/{check}")),
            log: abs(&format!("land-check/logs/{check}.log")),
        },
    );
    let state = file(&mut bench, &prepared);
    assert_eq!(state.standing, Standing::Gating);

    refused(
        &mut bench,
        &format!(
            "{LAND_BATCH_VERB} --batch {BATCH} --record lc-1791940000000-9 --rc 0 --log {summary}"
        ),
        "no prepared gate",
    );
    let lying = receipt(
        &bench,
        1,
        &by,
        Record::Gated {
            check: check.clone(),
            step,
            target: tip.clone(),
            tree: tree_of(1),
            verdict: Verdict::Green,
            rc: 3,
            took_ms: None,
            log: summary.clone(),
        },
    );
    bench.clock += 1;
    let refusal = bench
        .ledger
        .land_batched(&lying, bench.clock)
        .expect_err("rc 3 written as green");
    assert!(refusal.contains("only rc 0 is green"), "{refusal}");

    let ask = asked(&bench.run(&format!(
        "{LAND_BATCH_VERB} --batch {BATCH} --record {check} --rc 3 --log {summary} --took-ms \
         1834200"
    )));
    let Action::Record {
        check: recorded,
        rc,
        log,
        took_ms,
        ..
    } = ask.action
    else {
        panic!("a record was asked for");
    };
    assert_eq!(
        (recorded.as_str(), rc, log.as_str(), took_ms),
        (check.as_str(), 3, summary.as_str(), Some(1_834_200))
    );
    let red = receipt(
        &bench,
        1,
        &by,
        Record::Gated {
            check: check.clone(),
            step,
            target: tip.clone(),
            tree: tree_of(1),
            verdict: Verdict::Red,
            rc: 3,
            took_ms: Some(1_834_200),
            log: summary.clone(),
        },
    );
    let state = file(&mut bench, &red);
    assert_eq!(state.standing, Standing::Red);
    let gate = state.tip_gate.as_ref().expect("the tip's gate");
    assert_eq!(
        (gate.verdict, gate.rc, gate.tree.as_str()),
        (Verdict::Red, 3, tree_of(1).as_str())
    );
    let (line, _) = newest(&bench);
    assert!(
        line.starts_with(&format!("{LAND_BATCH_VERB} {BATCH}: red · tree ")),
        "{line}"
    );

    let (_, state) = gated(&mut bench, &by, &|_: u32| 0);
    assert_eq!(state.standing, Standing::Green);
    assert_eq!(
        state.tip_gate.as_ref().map(|gate| gate.verdict),
        Some(Verdict::Green)
    );
    let (line, _) = newest(&bench);
    assert_eq!(
        line,
        format!(
            "{LAND_BATCH_VERB} {BATCH}: green · tree {} · rc 0 · 1834.2 s · log {summary}",
            &tree_of(1)[..7]
        )
    );
}

/// A promote needs a green gate of this generation's exact tip tree: no gate, a red one, or another
/// generation's green are each refused; the green tip is written as promoting first, and a second
/// press waits for it.
#[test]
fn promote_requires_a_green_gate_for_this_exact_tree() {
    let mut bench = Bench::new();
    let ready = ready(&mut bench, 2);
    let (by, state) = chosen(&mut bench, &ready);
    stacked(&mut bench, &by, &state);
    let promote = format!("{LAND_BATCH_VERB} --batch {BATCH} --promote");
    refused(&mut bench, &promote, "green");
    gated(&mut bench, &by, &|_: u32| 1);
    refused(&mut bench, &promote, "green");
    let (_, state) = gated(&mut bench, &by, &|_: u32| 0);
    let ask = asked(&bench.run(&promote));
    assert!(
        matches!(ask.action, Action::Promote { .. }),
        "{:?}",
        ask.action
    );

    let restacked = receipt(
        &bench,
        2,
        &by,
        Record::Restacked {
            members: state.members.clone(),
            dropped: Vec::new(),
            base: moved_base(),
        },
    );
    let state = file(&mut bench, &restacked);
    stacked(&mut bench, &by, &state);
    refused(&mut bench, &promote, "green");
    let stale = receipt(
        &bench,
        1,
        &by,
        Record::Promoting {
            target: step_commit(1, 2),
            expect: base(),
            base_branch: BRANCH.to_string(),
            lease_until_ms: bench.clock + 60_000,
        },
    );
    bench.clock += 1;
    let refusal = bench
        .ledger
        .land_batched(&stale, bench.clock)
        .expect_err("generation 1's promote after generation 2 was stacked");
    assert!(refusal.contains("generation"), "{refusal}");

    let (_, state) = gated(&mut bench, &by, &|_: u32| 0);
    let tip = state.stacked.as_ref().expect("stacked").tip.clone();
    asked(&bench.run(&promote));
    let promoting = receipt(
        &bench,
        2,
        &by,
        Record::Promoting {
            target: tip,
            expect: moved_base(),
            base_branch: BRANCH.to_string(),
            lease_until_ms: bench.clock + 60_000,
        },
    );
    let state = file(&mut bench, &promoting);
    assert_eq!(state.standing, Standing::Promoting);
    refused(&mut bench, &promote, "already promoting");
}

/// The ledger writes who pressed: the coordinator's verb as the coordinator with its run and seat
/// generation, the window's button as the person — kept on the promoted record — and a worker's
/// pane cannot press at all.
#[test]
fn the_ledger_records_who_pressed() {
    let mut bench = Bench::new();
    let ready = ready(&mut bench, 1);
    let line = format!(
        "{LAND_BATCH_VERB} --members {} --message-dir {}",
        ready[0].task,
        abs("messages")
    );
    let planned = bench.at(&ready[0].pane, &line);
    assert_ne!(
        planned.reply.exit_code, 0,
        "a worker pressed the coordinator's verb"
    );
    assert!(
        planned.reply.stderr.contains("coordinator's verb"),
        "{}",
        planned.reply.stderr
    );
    assert!(
        matches!(planned.effect, Effect::None),
        "{:?}",
        planned.effect
    );

    let (by, state) = chosen(&mut bench, &ready);
    assert_eq!(
        by,
        coordinator(&bench),
        "the coordinator's verb presses as the coordinator"
    );
    stacked(&mut bench, &by, &state);
    let (_, state) = gated(&mut bench, &by, &|_: u32| 0);

    assert_eq!(person_press(), PressedBy::Person);
    let run = bench.ledger.runs()[0].id.clone();
    let ask = press(&bench.ledger, &run, BATCH, Press::Promote, person_press())
        .expect("the person may promote a green batch");
    assert_eq!(ask.by, PressedBy::Person);
    let tip = state.stacked.as_ref().expect("stacked").tip.clone();
    let promoting = receipt(
        &bench,
        1,
        &ask.by,
        Record::Promoting {
            target: tip.clone(),
            expect: base(),
            base_branch: BRANCH.to_string(),
            lease_until_ms: bench.clock + 60_000,
        },
    );
    file(&mut bench, &promoting);
    let (line, record) = newest(&bench);
    assert!(line.ends_with("pressed by the person"), "{line}");
    assert_eq!(record["by"], serde_json::json!({"kind": "person"}));

    let promoted = receipt(
        &bench,
        1,
        &ask.by,
        Record::Promoted {
            from: base(),
            to: tip,
            members: landed(&state),
            settled_later: false,
        },
    );
    let state = file(&mut bench, &promoted);
    assert_eq!(
        state.promotion.as_ref().map(|promotion| &promotion.by),
        Some(&PressedBy::Person),
        "the presser is kept"
    );
    let (line, record) = newest(&bench);
    assert_eq!(record["phase"], "promoted");
    assert_eq!(record["by"], serde_json::json!({"kind": "person"}));
    assert!(
        line.contains("pressed by the person") && line.contains("--record-merged"),
        "{line}"
    );
}

/// After a red gate nothing starts by itself: no search, no gate prepared. The search begins when
/// somebody presses, and even then the first gate waits for its own press.
#[test]
fn a_red_gate_starts_no_search_until_pressed() {
    let mut bench = Bench::new();
    let ready = ready(&mut bench, 4);
    let (by, state) = chosen(&mut bench, &ready);
    stacked(&mut bench, &by, &state);
    let (_, state) = gated(&mut bench, &by, &|_: u32| 1);
    assert_eq!(state.standing, Standing::Red);
    assert!(
        state.search.is_none(),
        "a red gate started a search by itself"
    );
    assert!(
        state.prepared.is_none(),
        "a red gate prepared another gate by itself"
    );
    assert_eq!(
        phases(&bench).last().map(String::as_str),
        Some("gated"),
        "nothing was written after the red gate"
    );

    let ask = asked(&bench.run(&format!("{LAND_BATCH_VERB} --batch {BATCH} --bisect")));
    assert!(
        matches!(ask.action, Action::Bisect { .. }),
        "{:?}",
        ask.action
    );
    let started = receipt(
        &bench,
        1,
        &ask.by,
        Record::BisectStarted {
            lo: 0,
            hi: 4,
            next: bisect_next(0, 4),
        },
    );
    let state = file(&mut bench, &started);
    assert_eq!(state.standing, Standing::Searching);
    assert_eq!(
        state.search.as_ref().and_then(|search| search.next),
        Some(2)
    );
    assert!(
        state.prepared.is_none(),
        "the search prepared a gate before anybody pressed"
    );
    assert_eq!(
        phases(&bench).last().map(String::as_str),
        Some("bisect_started")
    );
    let ask = asked(&bench.run(&format!("{LAND_BATCH_VERB} --batch {BATCH} --prepare")));
    let Action::Prepare { step, target, .. } = ask.action else {
        panic!("a gate was asked for");
    };
    assert_eq!(
        (step, target),
        (2, step_commit(1, 2)),
        "the press prepares S(2), the middle"
    );
}

/// The search halves the stack with the base taken as green and the tip as red, and names the
/// breaking member in at most ⌈log₂ n⌉ gates: eight tasks with the fifth breaking gate S4, S6, S5.
/// A culprit that is the first member is also what a red base looks like, and the letter says so.
#[test]
fn the_search_finds_the_breaking_member_in_log2_gates() {
    assert_eq!(bisect_next(0, 8), Some(4));
    assert_eq!(bisect_next(4, 6), Some(5));
    assert_eq!(bisect_next(4, 5), None);
    for (size, culprit) in [
        (8_usize, 5_u32),
        (8, 1),
        (8, 8),
        (4, 3),
        (2, 1),
        (2, 2),
        (1, 1),
    ] {
        let mut bench = Bench::new();
        let ready = ready(&mut bench, size);
        let (by, state) = chosen(&mut bench, &ready);
        stacked(&mut bench, &by, &state);
        gated(&mut bench, &by, &|_: u32| 1);
        asked(&bench.run(&format!("{LAND_BATCH_VERB} --batch {BATCH} --bisect")));
        let hi = u32::try_from(size).expect("a small batch");
        let started = receipt(
            &bench,
            1,
            &by,
            Record::BisectStarted {
                lo: 0,
                hi,
                next: bisect_next(0, hi),
            },
        );
        let mut state = file(&mut bench, &started);
        let mut gates: Vec<u32> = Vec::new();
        while state.culprit.is_none() {
            assert!(
                gates.len() < size,
                "the search of {size} gated {gates:?} and named nobody"
            );
            let (step, after) = gated(&mut bench, &by, &|step: u32| {
                if step < culprit { 0 } else { 1 }
            });
            gates.push(step);
            state = after;
        }
        let found = state.culprit.as_ref().expect("a culprit");
        let at = usize::try_from(culprit).expect("a step") - 1;
        assert_eq!(
            (found.task.as_str(), found.step),
            (ready[at].task.as_str(), culprit),
            "{size} tasks, culprit {culprit}"
        );
        assert_eq!(found.main_first, culprit == 1);
        let bound = (usize::BITS - (size - 1).leading_zeros()) as usize;
        assert!(
            gates.len() <= bound,
            "{size} tasks took {} gates: {gates:?}",
            gates.len()
        );
        if (size, culprit) == (8, 5) {
            assert_eq!(gates, [4, 6, 5], "the design's example: S4, S6, S5");
        }
        let (line, _) = newest(&bench);
        assert!(
            line.contains(&format!(
                "culprit {} (step {culprit} of {size})",
                ready[at].task
            )),
            "{line}"
        );
        assert_eq!(
            line.contains("check main itself first"),
            culprit == 1,
            "{line}"
        );
        assert_eq!(state.standing, Standing::CulpritFound);
    }
}

/// A batch task is the ledger's bookkeeping, never work a worker handed in: promoted and completed,
/// it raises none of the landing alerts, however late and whatever git says.
#[test]
fn a_batch_task_raises_no_landing_alert() {
    let mut bench = Bench::new();
    let ready = ready(&mut bench, 2);
    let (by, state) = chosen(&mut bench, &ready);
    stacked(&mut bench, &by, &state);
    let (_, state) = gated(&mut bench, &by, &|_: u32| 0);
    let tip = state.stacked.as_ref().expect("stacked").tip.clone();
    let promoting = receipt(
        &bench,
        1,
        &by,
        Record::Promoting {
            target: tip.clone(),
            expect: base(),
            base_branch: BRANCH.to_string(),
            lease_until_ms: bench.clock + 60_000,
        },
    );
    file(&mut bench, &promoting);
    let promoted = receipt(
        &bench,
        1,
        &by,
        Record::Promoted {
            from: base(),
            to: tip.clone(),
            members: landed(&state),
            settled_later: false,
        },
    );
    let state = file(&mut bench, &promoted);
    let batch_task = state.task.clone();
    assert_eq!(
        bench.ledger.runs()[0]
            .task(&batch_task)
            .map(|task| task.status),
        Some(TaskStatus::Completed)
    );
    let start = bench.clock;
    for after in [
        0,
        VERIFY_OVERDUE_MS,
        MERGE_OVERDUE_MS + 1,
        MERGED_UNRECORDED_GRACE_MS * 3,
        CLEANABLE_AFTER_MS * 2,
        RENOTIFY_MS * 2,
    ] {
        for git in [
            GitSays::Landed { at_ms: Some(start) },
            GitSays::Unlanded {
                behind: Some(3),
                far_behind: false,
                conflict: None,
            },
            GitSays::Silent,
        ] {
            let witness = LandingWitness {
                checkout: CHECKOUT.to_string(),
                branch: Some("wt/batch".to_string()),
                head: tip.clone(),
                compare_ref: "origin/main".to_string(),
                compare_oid: base(),
                git,
                occupied: false,
                beside: Vec::new(),
            };
            bench.ledger.landing_watch(&[witness], start + after);
        }
    }
    let told: Vec<&Message> = bench.ledger.runs()[0]
        .messages()
        .iter()
        .filter(|row| {
            matches!(
                row.kind,
                MessageKind::LandingStalled | MessageKind::BranchDrifted
            ) && row.task.as_deref() == Some(batch_task.as_str())
        })
        .collect();
    assert!(
        told.is_empty(),
        "the batch task raised landing alerts: {told:?}"
    );
}

/// A batch id is `lb-<digits>-<digits>` and nothing else: the window builds refs and paths from it.
#[test]
fn a_batch_id_is_the_prefix_and_two_numbers_and_nothing_else() {
    assert!(is_batch_id(BATCH));
    assert!(is_batch_id("lb-0-0"));
    for bad in [
        "",
        "lb-",
        "lb-1",
        "lb-1-2-3",
        "lb-a-1",
        "lb--1",
        "LB-1-2",
        "lb-1-2/",
        "../lb-1-2",
        "lb-1-2 ",
        "lc-1-2",
    ] {
        assert!(!is_batch_id(bad), "{bad:?} is not a batch id");
    }
}

/// Whether the coordinator's review of `task` says merged.
fn merged(bench: &Bench, task: &str) -> bool {
    let run = &bench.ledger.runs()[0];
    run.task(task)
        .is_some_and(|held| run.review_of(held).merged)
}

/// After a promote each member's record says merged, in the coordinator's voice: the coordinator's
/// own promote writes them with the same press; after the person's, the coordinator writes them with
/// `--record-merged`, judged the way `task-update` judges a review, and only once.
#[test]
fn the_merged_records_follow_a_promote_in_the_coordinators_voice() {
    for pressed_by_the_person in [false, true] {
        let mut bench = Bench::new();
        let ready = ready(&mut bench, 2);
        let (by, state) = chosen(&mut bench, &ready);
        stacked(&mut bench, &by, &state);
        let (_, state) = gated(&mut bench, &by, &|_: u32| 0);
        let presser = if pressed_by_the_person {
            person_press()
        } else {
            by.clone()
        };
        let tip = state.stacked.as_ref().expect("stacked").tip.clone();
        let promoting = receipt(
            &bench,
            1,
            &presser,
            Record::Promoting {
                target: tip.clone(),
                expect: base(),
                base_branch: BRANCH.to_string(),
                lease_until_ms: bench.clock + 60_000,
            },
        );
        file(&mut bench, &promoting);
        let promoted = receipt(
            &bench,
            1,
            &presser,
            Record::Promoted {
                from: base(),
                to: tip.clone(),
                members: landed(&state),
                settled_later: false,
            },
        );
        file(&mut bench, &promoted);
        let written_by_the_promote = ready.iter().all(|one| merged(&bench, &one.task));
        assert_eq!(
            written_by_the_promote, !pressed_by_the_person,
            "the coordinator's promote writes the merged records, the person's leaves them to the \
             coordinator"
        );
        if pressed_by_the_person {
            let said = bench.json(&format!(
                "{LAND_BATCH_VERB} --batch {BATCH} --record-merged"
            ));
            assert_eq!(said["batch"], BATCH);
            assert!(ready.iter().all(|one| merged(&bench, &one.task)));
            refused(
                &mut bench,
                &format!("{LAND_BATCH_VERB} --batch {BATCH} --record-merged"),
                "already written",
            );
        }
        assert_eq!(
            phases(&bench).last().map(String::as_str),
            Some("merged_recorded")
        );
        let run = &bench.ledger.runs()[0];
        for (one, step) in ready
            .iter()
            .zip(&state.stacked.as_ref().expect("stacked").steps)
        {
            let result: serde_json::Value =
                serde_json::from_str(run.task(&one.task).expect("a member").result.as_str())
                    .expect("a JSON result");
            assert_eq!(result["mergedAs"], step.as_str());
            assert_eq!(result["mainAt"], tip.as_str());
        }
    }
}
