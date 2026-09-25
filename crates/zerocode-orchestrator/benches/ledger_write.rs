//! The durable ledger's write and read costs at the size a busy window holds.
//!
//! Keep the fixture explicit: these counts are the contract for the measuring
//! stick, and changing one should be a deliberate change to the result being
//! reported. The write is wrapped in the same immediate transaction the
//! runtime uses, so the number includes the durability boundary without
//! measuring fixture setup.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use rusqlite::{Connection, TransactionBehavior};
use zerocode_core::orchestration::{
    AckedRow, DispatchRow, InboxRow, LedgerProjectionV1, PROJECTION_SCHEMA, RunRow, ServedAnswer,
    ServedRow, TaskRow, Text, WorkerRow,
};
use zerocode_core::orchestration::{TaskStatus, WorkerState};
use zerocode_orchestrator::ledger_store::{LEDGER_TABLES_SQL, read, write};

const TASK_COUNT: usize = 50;
const DISPATCH_COUNT: usize = 100;
const WORKER_COUNT: usize = 20;
const SERVED_COUNT: usize = 10_000;
const LEDGER_ID: &str = "bench-ledger";

fn a_store() -> Connection {
    let connection = Connection::open_in_memory().expect("an in-memory database");
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .expect("foreign keys");
    connection
        .execute_batch(LEDGER_TABLES_SQL)
        .expect("ledger tables");
    connection
}

fn a_projection() -> LedgerProjectionV1 {
    let run = "run-0".to_string();
    LedgerProjectionV1 {
        schema: PROJECTION_SCHEMA,
        next_id: (TASK_COUNT + DISPATCH_COUNT + SERVED_COUNT) as u64,
        runs: vec![RunRow {
            id: run.clone(),
            name: Text::from("benchmark run"),
            created_ms: 1,
            auto: None,
            summary: None,
            coordinator: None,
            handover: None,
        }],
        tasks: (0..TASK_COUNT)
            .map(|at| TaskRow {
                run: run.clone(),
                id: format!("task-{at}"),
                spec: Text::from(format!("perform benchmark task {at}")),
                title: Text::from(format!("task {at}")),
                deps: Vec::new(),
                parent: None,
                status: TaskStatus::Completed,
                result: Text::from("done"),
                failures: 0,
                created_ms: at as i64,
                result_author: None,
            })
            .collect(),
        dispatches: (0..DISPATCH_COUNT)
            .map(|at| DispatchRow {
                run: run.clone(),
                id: format!("dispatch-{at}"),
                task: format!("task-{}", at % TASK_COUNT),
                worker: format!("worker-{}", at % WORKER_COUNT),
                started_ms: at as i64,
                ended_ms: Some(at as i64 + 1),
                succeeded: Some(true),
                retry_of: None,
                remote: None,
                source: None,
            })
            .collect(),
        workers: (0..WORKER_COUNT)
            .map(|at| WorkerRow {
                run: run.clone(),
                id: format!("worker-{at}"),
                team: "bench-team".to_string(),
                agent: "bench-agent".to_string(),
                pane: format!("%{at}"),
                state: WorkerState::Reclaimable,
                started_ms: at as i64,
                dispatch: Some(format!("dispatch-{}", at % DISPATCH_COUNT)),
                model: Some("bench-model".to_string()),
                effort: Some("high".to_string()),
                session: None,
                ready_by_ms: None,
                hook_unreachable_since_ms: None,
                pane_missing_since_ms: None,
                adopted_by: None,
                on_quota_wall: None,
                quota_wait: false,
                exit_unconfirmed: None,
                taken_over: false,
                checkout: None,
                quiet_at: Some(at as i64 + 1),
                archive: Some(Text::from("worker output")),
                // Populated so the row this benchmark writes is the size a
                // real one is. A column left NULL measures a narrower write
                // than the ledger actually performs.
                started_by: Some(format!("worker-{}", at.saturating_sub(1))),
            })
            .collect(),
        messages: Vec::new(),
        inboxes: vec![InboxRow {
            run: run.clone(),
            address: "run:run-0".to_string(),
            pending: Vec::new(),
            open: None,
        }],
        bound: Vec::new(),
        served: (0..SERVED_COUNT)
            .map(|at| ServedRow {
                caller: Some(Text::from("bench-caller")),
                request: Text::from(format!("request-{at}")),
                answer: ServedAnswer::Inline("benchmark answer".to_string()),
                fingerprint: Some(Text::from(format!("fingerprint-{at:064}"))),
                verb: Some("run-use".to_string()),
                filed_ms: Some(at as i64),
                expired: false,
            })
            .collect(),
        acked: Vec::<AckedRow>::new(),
        gates: Vec::new(),
        attachments: Vec::new(),
        retention_days: 30,
        swept_at_ms: 0,
        verb_tallies: Vec::new(),
    }
}

fn seed(connection: &Connection, projection: &LedgerProjectionV1) {
    write(connection, LEDGER_ID, 0, 1, projection, 1).expect("seed ledger");
}

fn a_write_of_a_busy_ledger(criterion: &mut Criterion) {
    let projection = a_projection();
    let mut changed_projection = projection.clone();
    changed_projection.served[0].request = Text::from("a changed request");
    let mut connection = a_store();
    seed(&connection, &projection);
    let mut revision = 1_u64;

    criterion.bench_function(
        "ledger_write_50_tasks_100_dispatches_20_workers_10k_served",
        |bench| {
            bench.iter(|| {
                let transaction = connection
                    .transaction_with_behavior(TransactionBehavior::Immediate)
                    .expect("begin immediate transaction");
                write(
                    &transaction,
                    LEDGER_ID,
                    revision,
                    revision + 1,
                    black_box(if revision.is_multiple_of(2) {
                        &projection
                    } else {
                        &changed_projection
                    }),
                    2,
                )
                .expect("write ledger");
                transaction.commit().expect("commit ledger");
                revision += 1;
            })
        },
    );
}

fn a_read_of_a_busy_ledger(criterion: &mut Criterion) {
    let projection = a_projection();
    let connection = a_store();
    seed(&connection, &projection);

    criterion.bench_function(
        "ledger_read_50_tasks_100_dispatches_20_workers_10k_served",
        |bench| {
            bench.iter(|| {
                black_box(read(&connection, LEDGER_ID, PROJECTION_SCHEMA))
                    .expect("read ledger")
                    .expect("seeded ledger");
            })
        },
    );
}

criterion_group!(
    ledger_write,
    a_write_of_a_busy_ledger,
    a_read_of_a_busy_ledger
);
criterion_main!(ledger_write);
