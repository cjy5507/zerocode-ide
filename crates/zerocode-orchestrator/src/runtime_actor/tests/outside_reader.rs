//! t-37679: another process reading the window's ledger database must never
//! bring the window down — and what the request road costs per request.
//!
//! On 2026-10-04 four `sqlite3` reads of the live store from another process
//! were followed by the window dying of SIGBUS in the runtime actor
//! (`verify_authority` → `WorkflowStore::connection` → `walFindFrame`).
//!
//! Everything here is `#[ignore]`d and asked for by name on the build line:
//! the reproduction runs child processes for seconds, and the measurement
//! prints numbers rather than judging them. The role tests are each child's
//! whole job and return at once when the parent did not start them.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use super::*;
use crate::workflow_store::CONNECTIONS_OPENED;

/// How many times the outside reader opens the database, reads one row and
/// closes it again — the person's four `sqlite3` queries, fifty times over.
const OUTSIDE_READS: usize = 200;
/// The reader's pause between reads, so its opens spread across the
/// window's requests rather than landing between two of them.
const OUTSIDE_READ_PAUSE: Duration = Duration::from_millis(2);
/// What the outside reader asks: the ledger head and a scan of the runs the
/// window keeps adding — a person's look at a live ledger.
const OUTSIDE_QUERY: &str = "SELECT revision + (SELECT COUNT(*) FROM ledger_runs)
                               FROM orchestration_ledger_heads WHERE ledger_id = 'main-ledger'";
/// The reader waits on a busy database the way the store itself does, so a
/// refusal it counts is a fault and not a lost race for a lock.
const OUTSIDE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// The environment that turns a role test into a child's whole job.
const ROLE_STORE: &str = "ZEROCODE_T37679_STORE";
/// A file whose appearance tells the window role to finish.
const ROLE_STOP: &str = "ZEROCODE_T37679_STOP";
/// A file the window role leaves once its actor answers.
const ROLE_READY: &str = "ZEROCODE_T37679_READY";
/// The exit of a window role whose actor refused a request — a fault the
/// parent counts apart from a crash.
const WINDOW_REFUSED_EXIT: i32 = 3;
/// How often the parent looks at its children.
const CHILD_POLL: Duration = Duration::from_millis(5);
/// How long the parent waits for the first window to answer.
const WINDOW_READY_LIMIT: Duration = Duration::from_secs(60);
/// One request in this many writes the ledger; the rest read it, the way a
/// window's sweeps and views outnumber its verbs.
const WRITE_EVERY: u64 = 10;
/// Requests answered before the measurement starts, so it weighs the
/// steady road and not the boot.
const WARM_UP_REQUESTS: u64 = 50;
/// The measurement's request count — "per 1,000 requests" in the report.
const MEASURED_REQUESTS: u64 = 1_000;
/// Connections opened, read once and closed for the per-open cost.
const MEASURED_OPENS: u32 = 1_000;

/// One request on the actor's road: a view, a list, or every
/// [`WRITE_EVERY`]th a new run — each preceded by `verify_authority`.
fn one_request(actor: &RuntimeActor, tag: &str, n: u64) -> Result<(), RuntimeError> {
    let now_ms = wall_clock_ms();
    if n.is_multiple_of(WRITE_EVERY) {
        let name = format!("run-{tag}-{n}");
        let retry = format!("r-{tag}-{n}");
        let (decided, _) = actor.plan(a_command(
            &[
                "run-create",
                "--name",
                name.as_str(),
                "--retry-request",
                retry.as_str(),
            ],
            now_ms,
        ))?;
        return match decided.reply.exit_code {
            0 => Ok(()),
            _ => Err(RuntimeError::InvalidInput),
        };
    }
    if n.is_multiple_of(2) {
        actor.view().map(drop)
    } else {
        actor.plan(a_command(&["run-list"], now_ms)).map(drop)
    }
}

fn role_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

/// The window half of the reproduction: an actor answering requests as
/// fast as they come, until the parent leaves the stop file.
#[test]
#[ignore = "t-37679: a child role of an_outside_reader_never_brings_the_window_down"]
fn outside_reader_window_role() {
    let (Some(store), Some(stop), Some(ready)) = (
        role_path(ROLE_STORE),
        role_path(ROLE_STOP),
        role_path(ROLE_READY),
    ) else {
        return;
    };
    let store = WorkflowStore::open(&store).expect("the reproduction's store");
    let actor = RuntimeActor::start(
        &store,
        "main-ledger",
        RuntimeBoot::Reopen,
        4,
        TableFixture::holding(a_team()),
        Box::new(NoLauncher),
    )
    .expect("the window's actor");
    std::fs::write(&ready, b"").expect("the ready mark");
    let tag = std::process::id().to_string();
    let mut served = 0_u64;
    while !stop.exists() {
        if let Err(error) = one_request(&actor, &tag, served) {
            eprintln!("T37679 window request {served} refused: {error:?}");
            std::process::exit(WINDOW_REFUSED_EXIT);
        }
        served += 1;
    }
    println!("T37679 window {tag} served {served}");
    actor.shutdown().expect("the window's shutdown");
}

/// The outside half: open the database, read the ledger head, close —
/// [`OUTSIDE_READS`] times, each a separate SQLite connection the way each
/// `sqlite3` invocation is.
#[test]
#[ignore = "t-37679: a child role of an_outside_reader_never_brings_the_window_down"]
fn outside_reader_reader_role() {
    let Some(store) = role_path(ROLE_STORE) else {
        return;
    };
    let mut refused = 0_usize;
    for _ in 0..OUTSIDE_READS {
        let read = rusqlite::Connection::open(&store).and_then(|connection| {
            connection.busy_timeout(OUTSIDE_BUSY_TIMEOUT)?;
            connection.query_row(OUTSIDE_QUERY, [], |row| row.get::<_, i64>(0))
        });
        if let Err(error) = read {
            refused += 1;
            eprintln!("T37679 reader refused: {error}");
        }
        std::thread::sleep(OUTSIDE_READ_PAUSE);
    }
    println!("T37679 reader refused {refused} of {OUTSIDE_READS}");
    assert_eq!(refused, 0, "the outside reader was refused");
}

fn spawn_role(role: &str, store: &Path, stop: &Path, ready: &Path) -> Child {
    let exact = format!("runtime_actor::tests::outside_reader::{role}");
    Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            exact.as_str(),
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROLE_STORE, store)
        .env(ROLE_STOP, stop)
        .env(ROLE_READY, ready)
        .spawn()
        .expect("a role process")
}

/// The window's request road keeps running while another process reads
/// its database [`OUTSIDE_READS`] times; a window that dies is started
/// again and counted. Zero crashes, zero refusals, an intact store.
#[test]
#[ignore = "t-37679 reproduction: child processes for seconds; the build line runs it by name"]
fn an_outside_reader_never_brings_the_window_down() {
    use std::os::unix::process::ExitStatusExt as _;

    let fixture = Fixture::new();
    start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 4)
        .shutdown()
        .expect("a ledger for the window to reopen");
    let store = fixture.store.private_path().to_path_buf();
    let private = store.parent().expect("the store's directory");
    let (stop, ready) = (private.join("window-stop"), private.join("window-ready"));

    let mut window = spawn_role("outside_reader_window_role", &store, &stop, &ready);
    let waited = Instant::now();
    while !ready.exists() && waited.elapsed() < WINDOW_READY_LIMIT {
        std::thread::sleep(CHILD_POLL);
    }
    assert!(ready.exists(), "the window never answered");

    let mut reader = spawn_role("outside_reader_reader_role", &store, &stop, &ready);
    let mut endings: Vec<ExitStatus> = Vec::new();
    let reader_ended = loop {
        if let Some(status) = reader.try_wait().expect("the reader's state") {
            break status;
        }
        if let Some(status) = window.try_wait().expect("the window's state") {
            endings.push(status);
            window = spawn_role("outside_reader_window_role", &store, &stop, &ready);
        }
        std::thread::sleep(CHILD_POLL);
    };
    std::fs::write(&stop, b"").expect("the stop mark");
    endings.push(window.wait().expect("the last window"));

    let crashes = endings
        .iter()
        .filter(|ended| ended.signal().is_some())
        .count();
    let sigbus = endings
        .iter()
        .filter(|ended| ended.signal() == Some(libc::SIGBUS))
        .count();
    let refused = endings
        .iter()
        .filter(|ended| ended.code().is_some_and(|code| code != 0))
        .count();
    let integrity: String = rusqlite::Connection::open(&store)
        .and_then(|connection| connection.query_row("PRAGMA integrity_check", [], |row| row.get(0)))
        .unwrap_or_else(|error| error.to_string());
    println!(
        "T37679-REPRO {{\"reads\":{OUTSIDE_READS},\"windows\":{},\"crashes\":{crashes},\
         \"sigbus\":{sigbus},\"refused\":{refused},\"reader_ok\":{},\"integrity\":{integrity:?}}}",
        endings.len(),
        reader_ended.success(),
    );
    assert_eq!(
        (crashes, sigbus, refused),
        (0, 0, 0),
        "the window died or refused while another process read its database: {endings:?}"
    );
    assert!(reader_ended.success(), "the outside reader was refused");
    assert_eq!(integrity, "ok");
}

/// What this process has written, as macOS's disk-writes report counts it:
/// logical bytes (dirtied file-backed memory included) and bytes that
/// reached the disk.
#[cfg(target_os = "macos")]
fn bytes_written() -> Option<(u64, u64)> {
    // SAFETY: zero is a valid `rusage_info_v4` (integers and a byte array).
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: the buffer is a live `rusage_info_v4`, the size the V4 flavor
    // writes; the call reads nothing else of ours.
    let answer = unsafe {
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V4,
            std::ptr::from_mut(&mut info).cast(),
        )
    };
    (answer == 0).then_some((info.ri_logical_writes, info.ri_diskio_byteswritten))
}

#[cfg(not(target_os = "macos"))]
fn bytes_written() -> Option<(u64, u64)> {
    None
}

fn written_since(before: Option<(u64, u64)>) -> String {
    match (before, bytes_written()) {
        (Some((logical, disk)), Some((logical_now, disk_now))) => format!(
            "\"logical_bytes\":{},\"disk_bytes\":{}",
            logical_now.saturating_sub(logical),
            disk_now.saturating_sub(disk)
        ),
        _ => "\"logical_bytes\":null,\"disk_bytes\":null".to_string(),
    }
}

/// The sample at `percent` of the way through `sorted`, nearest rank.
fn micros_at(sorted: &[Duration], percent: usize) -> u128 {
    let last = sorted.len().saturating_sub(1);
    let rank = (last * percent + 50) / 100;
    sorted.get(rank).map_or(0, Duration::as_micros)
}

fn timing(mut samples: Vec<Duration>) -> String {
    samples.sort_unstable();
    let total: Duration = samples.iter().sum();
    let count = u32::try_from(samples.len()).unwrap_or(u32::MAX).max(1);
    format!(
        "\"mean_us\":{},\"p50_us\":{},\"p99_us\":{}",
        (total / count).as_micros(),
        micros_at(&samples, 50),
        micros_at(&samples, 99)
    )
}

/// One connection's cost: open it, read one header field (which is what
/// attaches it to the WAL index), close it.
fn one_open_cost(store: &WorkflowStore, label: &str) {
    let before = bytes_written();
    let samples = (0..MEASURED_OPENS)
        .map(|_| {
            let began = Instant::now();
            let connection = store.connection().expect("a connection");
            let _: i64 = connection
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .expect("a header read");
            drop(connection);
            began.elapsed()
        })
        .collect();
    println!(
        "T37679-MEASURE {{\"what\":\"open\",\"beside\":{label:?},\"opens\":{MEASURED_OPENS},{},{}}}",
        timing(samples),
        written_since(before)
    );
}

/// The request road, measured: time per request, connections the actor
/// opened, how often a request ended with the WAL index gone (so the next
/// one rebuilt it), and the bytes written — per [`MEASURED_REQUESTS`].
/// Then one connection's cost alone and beside a connection held open.
#[test]
#[ignore = "t-37679 measurement: run by name on the build line, normal and taskpolicy -b"]
fn measure_the_request_road() {
    let fixture = Fixture::new();
    let actor = start(&fixture, RuntimeBoot::Fresh { now_ms: 10 }, 4);
    for n in 0..WARM_UP_REQUESTS {
        one_request(&actor, "warm", n).expect("a warm-up request");
    }
    let mut shm = fixture.store.private_path().as_os_str().to_os_string();
    shm.push("-shm");
    let shm = PathBuf::from(shm);

    let opened = CONNECTIONS_OPENED.load(std::sync::atomic::Ordering::Relaxed);
    let before = bytes_written();
    let mut samples = Vec::new();
    let mut index_gone = 0_u64;
    for n in WARM_UP_REQUESTS..WARM_UP_REQUESTS + MEASURED_REQUESTS {
        let began = Instant::now();
        one_request(&actor, "measured", n).expect("a measured request");
        samples.push(began.elapsed());
        if !shm.exists() {
            index_gone += 1;
        }
    }
    let written = written_since(before);
    let connections = CONNECTIONS_OPENED
        .load(std::sync::atomic::Ordering::Relaxed)
        .saturating_sub(opened);
    println!(
        "T37679-MEASURE {{\"what\":\"requests\",\"requests\":{MEASURED_REQUESTS},\
         \"writes\":{},\"connections\":{connections},\"index_gone_after\":{index_gone},{},{written}}}",
        MEASURED_REQUESTS / WRITE_EVERY,
        timing(samples),
    );
    actor.shutdown().expect("the measured actor's shutdown");

    one_open_cost(&fixture.store, "nothing");
    let held = fixture.store.connection().expect("a held connection");
    let _: i64 = held
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("the held connection attaches");
    one_open_cost(&fixture.store, "a held connection");
}
