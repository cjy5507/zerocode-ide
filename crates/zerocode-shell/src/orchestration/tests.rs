const TEST_HEADROOM_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// One snapshot as the window's poll writes it: a session and a week,
/// each `(used_percent, resets_at)`, or neither for a read that failed.
fn usage_snapshot(
    provider: &str,
    session: Option<(u8, Option<i64>)>,
    weekly: Option<(u8, Option<i64>)>,
    updated_at: i64,
) -> crate::usage::ProviderUsage {
    let window = |(used_percent, resets_at): (u8, Option<i64>)| crate::usage::UsageWindow {
        used_percent,
        window_minutes: 0,
        resets_at,
        reset_description: None,
    };
    crate::usage::ProviderUsage {
        provider: provider.to_string(),
        session: session.map(window),
        weekly: weekly.map(window),
        fable_weekly: None,
        monthly: None,
        buckets: None,
        updated_at,
        error: None,
        status: "ok".to_string(),
        failure_kind: None,
        retry_at_ms: None,
        plan_type: None,
        reset_credits: None,
        account: None,
    }
}

/// The ledger's quota probe reads the cache it is handed and nothing
/// else — no scan, no subprocess, no request (the source contract pins
/// the road; this pins the answer) — and a provider this window keeps
/// no gauge for answers "nobody knows", never 0%.
#[test]
fn provider_headroom_reads_the_injected_cache_and_never_the_network() {
    let catalog = super::LiveCatalog {
        overrides: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        ledger_dir: std::path::PathBuf::from("/ledger"),
        headroom: super::HeadroomSource::Fixed(TEST_HEADROOM_BYTES),
        usage: super::UsageSource::fixed(vec![
            (
                "codex",
                usage_snapshot(
                    "codex",
                    Some((40, Some(9_000))),
                    Some((98, Some(50_000))),
                    1_234,
                ),
            ),
            ("claude", usage_snapshot("claude", None, None, 5)),
        ]),
    };
    // The binding window is the fuller one, with its own reset.
    let read = catalog
        .provider_headroom("codex", None)
        .expect("codex's gauge was handed over");
    assert_eq!(
        (
            read.provider.as_str(),
            read.used_percent,
            read.window,
            read.resets_at_ms,
            read.updated_at_ms,
            read.status.as_str(),
        ),
        (
            "codex",
            98,
            zerocode_core::orchestration::QuotaWindow::Weekly,
            Some(50_000),
            1_234,
            "ok",
        )
    );
    // zo draws on its model's gauge.
    assert_eq!(
        catalog
            .provider_headroom("zo", Some("gpt-5-codex"))
            .map(|held| held.used_percent),
        Some(98)
    );
    // Gemini has no gauge in this window: None, never a percentage.
    assert!(
        catalog
            .provider_headroom("zo", Some("gemini-2.5-pro"))
            .is_none()
    );
    // A snapshot with no window figures — a failed read — is unknown too.
    assert!(catalog.provider_headroom("claude", None).is_none());
    // And so is a cache nobody filled.
    assert!(catalog.provider_headroom("kimi", None).is_none());
}

/// The fullest window binds — session first on a tie, a month when a
/// provider reports only that.
#[test]
fn the_binding_window_is_the_fullest_one() {
    use zerocode_core::orchestration::QuotaWindow;
    let pick = |snapshot: &crate::usage::ProviderUsage| {
        super::headroom_of(snapshot).map(|held| (held.window, held.used_percent))
    };
    assert_eq!(
        pick(&usage_snapshot(
            "codex",
            Some((40, None)),
            Some((98, None)),
            1
        )),
        Some((QuotaWindow::Weekly, 98))
    );
    assert_eq!(
        pick(&usage_snapshot(
            "codex",
            Some((98, None)),
            Some((98, None)),
            1
        )),
        Some((QuotaWindow::Session, 98))
    );
    assert_eq!(
        pick(&usage_snapshot("codex", Some((12, None)), None, 1)),
        Some((QuotaWindow::Session, 12))
    );
    let mut monthly = usage_snapshot("opencode-go", None, None, 1);
    monthly.monthly = Some(crate::usage::UsageWindow {
        used_percent: 73,
        window_minutes: 0,
        resets_at: Some(77),
        reset_description: None,
    });
    assert_eq!(pick(&monthly), Some((QuotaWindow::Monthly, 73)));
    assert_eq!(
        super::headroom_of(&monthly).and_then(|held| held.resets_at_ms),
        Some(77)
    );
    assert_eq!(pick(&usage_snapshot("codex", None, None, 1)), None);
}

/// One agent's session, as a test host answers for it.
///
/// Per TERMINAL, because that is what the real one is keyed by and what
/// makes two panes two actors — a constant would make the sibling tests
/// pass for the wrong reason.
fn test_actor(term: u32) -> String {
    // Shaped like a real one — see the core bench's own note.
    zerocode_core::orchestration::receipt_actor(
        "claude",
        zerocode_core::provider_session::SessionKey::SessionId,
        &format!("test-session-of-term-{term}"),
    )
}

/// The capability every test pane holds.
///
/// A real pane's is minted from `/dev/urandom` at spawn and remembered
/// beside the team table; a test's only has to be the same string on both
/// sides of the door. Named rather than spelled at each call so that the
/// one test which presents the WRONG one is visibly presenting a different
/// thing.
const TEST_CAPABILITY: &str = "pane-capability";

/* The mid-settlement stand — a thread-local hook `terminal_gone` and
 * `pane_turn_ended` called between reading a seat and settling it — is
 * gone WITH the gap it stood in: the actor resolves the seat and writes
 * the settlement inside one `with_seat_of_term` borrow, on one thread,
 * so there is no between for anything to stand in. The tests that stood
 * there are listed where they fell, below. */

/// A team whose leader pane holds a capability, which is what a real one
/// has: `open_team` mints the leader's token in the same breath as the team
/// (`agent_teams.rs:293`). A test team without one is a team no verb can
/// enter, so this is not test convenience — it is the same two facts.
fn seat_a_team(id: &str, leader_term: u32) {
    crate::agent_teams::teams().insert(
        id.to_string(),
        zerocode_core::agent_teams::Team::new(id.to_string(), TEST_CAPABILITY, leader_term),
    );
    let _ = crate::agent_teams::remember_pane_token(
        id,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY.to_string(),
    );
}

/// Stand the one durable authority up, once, for every test that drives
/// the production doors.
///
/// The rows are shareable: distinct team ids and terminals keep tests in
/// their own records. Host effects are deliberately not — the journal has
/// one in-flight cut for the whole window. [`the_window`] therefore keeps
/// one test turn across each production-road scenario so default Rust test
/// parallelism cannot make an unrelated split look like the product
/// refusal being asserted. A test that needs a runtime of its own — a disk
/// it can break — swaps the cell under [`the_window_to_itself`] and puts
/// the shared one back when it is done.
fn the_window_is_open() {
    static STOOD: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    STOOD.get_or_init(|| {
        assert_eq!(
            super::open_with_headroom(
                process_data_root(),
                1,
                super::HeadroomSource::Fixed(TEST_HEADROOM_BYTES),
            ),
            Restarted::default(),
            "the bench window opens"
        );
        assert!(
            super::unavailable().is_none(),
            "the bench window is degraded: {:?}",
            super::unavailable()
        );
    });
}

/// This test process's own orchestration data root, made once per
/// PROCESS and kept for its lifetime.
///
/// Once per process and never per test: `open` sets `BLACKBOX` once, and
/// every scenario in this binary reads its federation book and its
/// authority under it. Its own and not the app's: a root under the
/// support directory, or a fixed name under the temp directory, is one
/// a second test process on the same machine — another worktree's
/// `cargo test` — writes into at the same moment, and `tempfile` mints a
/// name no other process holds. This keeps PROCESSES apart; the
/// in-process order is still `shared_window_scenarios`'s. The gate that
/// runs two such processes side by side is
/// `scripts/test-shell-concurrent.sh`.
pub(crate) fn process_data_root() -> &'static std::path::Path {
    static ROOT: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| tempfile::tempdir().expect("a data root of this process's own"))
        .path()
}

/// The scenarios' files live under this process's own root — not under
/// the app's support directory, not under a name another process could
/// hold — so a second `cargo test -p zerocode-shell` on the machine
/// cannot reach them.
#[test]
fn the_shared_window_keeps_its_files_under_this_processs_own_root() {
    let _window = the_window();
    let root = process_data_root();
    let blackbox = super::BLACKBOX.get().expect("the window set its blackbox");
    assert_eq!(blackbox.as_path(), root);
    let federation = super::federation_root().expect("the window has a data root");
    assert!(federation.starts_with(root), "{}", federation.display());
    assert!(
        root.join("authority").join("authority.sqlite").is_file(),
        "the authority store stands under the process root"
    );
}

/// The root the product boots through: the window's own, byte for byte,
/// unless the environment names another — and an empty name is no name.
#[test]
fn the_data_root_is_the_windows_own_unless_the_environment_names_another() {
    let app = std::path::Path::new("/Library/Application Support/dev.zerocode.app");
    assert_eq!(super::resolve_orchestration_data_root(app, None), app);
    assert_eq!(
        super::resolve_orchestration_data_root(app, Some(std::ffi::OsString::new())),
        app
    );
    let named = process_data_root();
    assert_eq!(
        super::resolve_orchestration_data_root(app, Some(named.as_os_str().to_owned())),
        named
    );
    assert_eq!(
        super::ORCHESTRATION_DATA_ROOT_ENV,
        "ZEROCODE_ORCHESTRATION_DATA_ROOT"
    );
}

/// Every reader/swapper of the runtime cell, ordered.
///
/// Tests that USE the shared runtime hold it shared; the few that swap a
/// private one in hold it exclusively, so a swap cannot land in the
/// middle of somebody else's verb.
fn window_turns() -> &'static std::sync::RwLock<()> {
    static TURNS: std::sync::OnceLock<std::sync::RwLock<()>> = std::sync::OnceLock::new();
    TURNS.get_or_init(|| std::sync::RwLock::new(()))
}

/// The durable journal permits one host effect at a time. Tests exercise
/// that same journal, so their scenarios take turns even though their
/// ledger rows have distinct team ids.
fn shared_window_scenarios() -> &'static Mutex<()> {
    static TURNS: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
    TURNS.get_or_init(|| Mutex::new(()))
}

pub(crate) struct SharedWindow {
    _scenario: std::sync::MutexGuard<'static, ()>,
    _window: std::sync::RwLockReadGuard<'static, ()>,
}

/// The shared window, ready to drive. `pub(crate)`: the one
/// orchestration-road test in `agent_teams` holds the same window the
/// tests here do, instead of hoping a sibling opened it first.
pub(crate) fn the_window() -> SharedWindow {
    let scenario = shared_window_scenarios()
        .lock()
        .unwrap_or_else(|held| held.into_inner());
    let window = window_turns()
        .read()
        .unwrap_or_else(|held| held.into_inner());
    the_window_is_open();
    SharedWindow {
        _scenario: scenario,
        _window: window,
    }
}

/// A window whose runtime is this test's own — its store is the test's
/// to break — with the shared one restored when the guard drops.
struct PrivateWindow {
    _root: tempfile::TempDir,
    previous: Option<super::LiveRuntime>,
    _turn: std::sync::RwLockWriteGuard<'static, ()>,
}

impl PrivateWindow {
    fn boot() -> (Self, zerocode_orchestrator::workflow_store::WorkflowStore) {
        Self::boot_with_usage(Vec::new())
    }

    /// The same private window, whose usage cache holds these gauges —
    /// what a quota gate or a wall witness reads, handed over rather
    /// than read off the developer's account.
    fn boot_with_usage(
        usage: Vec<(&'static str, crate::usage::ProviderUsage)>,
    ) -> (Self, zerocode_orchestrator::workflow_store::WorkflowStore) {
        let usage = super::UsageSource::fixed(usage);
        let turn = window_turns()
            .write()
            .unwrap_or_else(|held| held.into_inner());
        /* The shared window stands BEFORE the private one is swapped in,
         * under this same exclusive turn. Two things this closes at once.
         * `previous` below is then always the shared runtime, so the
         * swap-back restores a window rather than `None`. And the shared
         * window's one-time boot can never run beside a private test:
         * `the_window_is_open` installs a runtime when it initialises,
         * and a lock-less first call from another test used to land that
         * install in the middle of a private test's verbs — the private
         * test read "no run in use" from a runtime that was not its own,
         * then put `None` back on its way out, and every shared test
         * after it read "the runtime never started" (measured 2026-09-05:
         * 48 of 110 red in parallel, 0 serial). */
        the_window_is_open();
        let _ = super::HOST_EPOCH.set(super::digest(
            b"zerocode.orchestration.test-host-epoch.v1",
            &[b"private-window"],
        ));
        let root = tempfile::tempdir().expect("private window root");
        let vault = root.path().join("authority");
        std::fs::create_dir(&vault).expect("private authority directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&vault, std::fs::Permissions::from_mode(0o700))
                .expect("private authority permissions");
        }
        let store = zerocode_orchestrator::workflow_store::WorkflowStore::open(
            vault.join("authority.sqlite"),
        )
        .expect("private authority store");
        let overrides: super::LiveOverrides =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let actor = super::RuntimeActor::start(
            &store,
            "main-ledger",
            super::RuntimeBoot::Cutover {
                legacy: None,
                now_ms: 1,
            },
            super::MAX_RUNTIME_MAILBOX,
            Box::new(super::ShellPaneTable),
            Box::new(super::LiveCatalog {
                overrides: std::sync::Arc::clone(&overrides),
                ledger_dir: vault.clone(),
                headroom: super::HeadroomSource::Fixed(TEST_HEADROOM_BYTES),
                usage: usage.clone(),
            }),
        )
        .expect("private runtime");
        let previous = super::runtime_cell()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .take();
        super::install_runtime(actor, overrides, usage);
        forget_the_goodbyes_note();
        (
            Self {
                _root: root,
                previous,
                _turn: turn,
            },
            store,
        )
    }
}

/// A private window is a window of its own (t-7812): a goodbye's note left
/// in this process's shared data root by another scenario names another
/// ledger's workers, whose ids this one mints again from `w-1`. Forgotten as
/// the window opens and again as it closes, so no scenario's owed words
/// reach another's wake.
fn forget_the_goodbyes_note() {
    if let Some(root) = super::BLACKBOX.get() {
        let _ = super::restart_census::leave_cut(
            root,
            &super::restart_census::RestartCensus::default(),
            &|_| false,
        );
    }
}

impl PrivateWindow {
    /// The provider's number moving under this window's live panes.
    fn set_usage(&self, rows: Vec<(&'static str, crate::usage::ProviderUsage)>) {
        super::runtime()
            .expect("this window's runtime")
            .usage
            .replace(rows);
    }
}

impl Drop for PrivateWindow {
    fn drop(&mut self) {
        forget_the_goodbyes_note();
        *super::runtime_cell()
            .lock()
            .unwrap_or_else(|held| held.into_inner()) = self.previous.take();
    }
}

/// The clock a bench verb stamps with.
///
/// The store refuses a timestamp behind its head, and two production
/// roads (`kill-pane`, `respawn-pane`) stamp with the wall clock — so
/// every verb driven at the SHARED window has to ride the same clock, or
/// the first wall-clock write would refuse every small literal after
/// it. Tests on a private window may keep little numbers; tests on the
/// shared one call this.
fn clock() -> i64 {
    crate::now_epoch_ms()
}

/// Everything the rows say about ONE run, as one comparable string —
/// the "my run did not move" question, asked without reading anybody
/// else's rows into the answer.
fn a_runs_shadow(id: &str) -> String {
    let rows = the_rows();
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}",
        rows.runs
            .iter()
            .filter(|one| one.id == id)
            .collect::<Vec<_>>(),
        rows.tasks
            .iter()
            .filter(|one| one.run == id)
            .collect::<Vec<_>>(),
        rows.dispatches
            .iter()
            .filter(|one| one.run == id)
            .collect::<Vec<_>>(),
        rows.workers
            .iter()
            .filter(|one| one.run == id)
            .collect::<Vec<_>>(),
        rows.messages
            .iter()
            .filter(|one| one.run == id)
            .collect::<Vec<_>>(),
    )
}

/// The rows, as the runtime shows them to everybody.
fn the_rows() -> zerocode_core::orchestration::LedgerProjectionV1 {
    super::runtime()
        .expect("a runtime")
        .actor
        .view()
        .expect("the rows")
        .projection()
        .clone()
}

#[test]
fn runtime_report_exposes_only_the_requested_claude_peer_name() {
    let mut ledger = super::Ledger::new();
    let run = ledger.create_run("provider peer report", 1);
    let claude = ledger
        .start_worker(&run, "claude", ("team-report", "%2"), None, 2)
        .expect("the claude worker")
        .worker;
    let codex = ledger
        .start_worker(&run, "codex", ("team-report", "%3"), None, 3)
        .expect("the codex worker")
        .worker;
    let private_session = zerocode_core::ProviderSession {
        key: zerocode_core::provider_session::SessionKey::SessionId,
        id: "private-provider-session-socket-token-registry-key".to_string(),
        transcript_path: Some("/private/registry/socket/token/session.jsonl".to_string()),
    };
    assert!(ledger.worker_session_reported(("team-report", "%2"), private_session));

    let report = super::runtime_report_value(7, &ledger);
    let workers = report["runs"][0]["workers"]
        .as_array()
        .expect("runtime worker rows");
    let row = |id: &str| {
        workers
            .iter()
            .find(|worker| worker["worker_id"] == id)
            .unwrap_or_else(|| panic!("missing worker {id}"))
    };
    let expected = serde_json::to_value(
        zerocode_core::orchestration::provider_peer("claude", &run, &claude)
            .expect("claude peer projection"),
    )
    .expect("provider peer JSON");

    assert_eq!(row(&claude)["providerPeer"], expected);
    assert!(row(&codex)["providerPeer"].is_null());
    let rendered = report.to_string();
    assert!(
        !rendered.contains("private-provider-session-socket-token-registry-key")
            && !rendered.contains("/private/registry/socket/token/session.jsonl"),
        "private provider state escaped through {rendered}",
    );
}

#[test]
fn runtime_report_uses_the_single_core_provider_peer_projection() {
    // The tests live in orchestration/tests.rs since 2026-09-11 (P4-1): the
    // production source is the whole file.
    let shipped = include_str!("../orchestration.rs");
    assert!(
        shipped.contains("\n#[cfg(test)]\npub(crate) mod tests;\n"),
        "the orchestration tests moved back inline — this pin reads the whole file as shipped code"
    );
    let report = shipped
        .split_once("fn runtime_report_value(")
        .expect("the runtime report")
        .1
        .split_once("\n}\n\npub(crate) fn runtime_report(")
        .expect("the end of the runtime report")
        .0;

    assert_eq!(
        report
            .matches("zerocode_core::orchestration::provider_peer(")
            .count(),
        1,
        "runtime state bypassed or duplicated the core provider peer projection",
    );
    assert_eq!(report.matches("\"providerPeer\"").count(), 1);
    assert!(!report.contains("WorkerPeerName::for_worker"));
}

#[test]
fn a_ledger_image_cache_hits_until_its_revision_moves() {
    let mut cache = super::LedgerCache::default();
    let projection = super::Ledger::new().export();

    let first = cache.ledger(7, &projection).expect("the first image");
    let same = cache.ledger(7, &projection).expect("the cached image");
    assert!(
        std::sync::Arc::ptr_eq(&first, &same),
        "the same revision was rebuilt instead of shared"
    );

    let moved = cache.ledger(8, &projection).expect("the next image");
    assert!(
        !std::sync::Arc::ptr_eq(&first, &moved),
        "a new revision reused the old image"
    );
}

#[test]
fn a_runtime_report_tree_is_cached_with_its_ledger_revision() {
    let mut cache = super::LedgerCache::default();
    let projection = super::Ledger::new().export();
    let ledger = cache.ledger(11, &projection).expect("the report image");

    let first = cache.report(11, &ledger);
    let same = cache.report(11, &ledger);
    assert!(
        std::sync::Arc::ptr_eq(&first, &same),
        "the report tree was built again for the same revision"
    );

    let moved = cache
        .ledger(12, &projection)
        .expect("the next report image");
    let next = cache.report(12, &moved);
    assert!(
        !std::sync::Arc::ptr_eq(&first, &next),
        "a new revision reused the old report tree"
    );
}

#[test]
fn ledger_image_cache_reports_baseline_and_hit_timings() {
    const TASKS: usize = 256;
    const SAMPLES: usize = 32;
    let mut ledger = super::Ledger::new();
    let run = ledger.create_run("cache timing", 1);
    for task in 0..TASKS {
        ledger
            .create_task(
                &run,
                format!("task {task}"),
                String::new(),
                Vec::new(),
                None,
                task as i64,
            )
            .expect("the timing task");
    }
    let projection = ledger.export();

    let mut baseline = std::time::Duration::ZERO;
    for _ in 0..SAMPLES {
        let started = std::time::Instant::now();
        std::hint::black_box(
            super::Ledger::rebuild(projection.clone()).expect("the baseline image"),
        );
        baseline += started.elapsed();
    }

    let mut cache = super::LedgerCache::default();
    let started = std::time::Instant::now();
    std::hint::black_box(cache.ledger(17, &projection).expect("the cache miss"));
    let miss = started.elapsed();
    let started = std::time::Instant::now();
    for _ in 0..SAMPLES {
        std::hint::black_box(cache.ledger(17, &projection).expect("the cache hit"));
    }
    let hits = started.elapsed();

    eprintln!(
        concat!(
            "ledger image cache ({} tasks, {} samples): baseline rebuild avg={}us, ",
            "first miss={}us, cached hit avg={}ns",
        ),
        TASKS,
        SAMPLES,
        baseline.as_micros() / SAMPLES as u128,
        miss.as_micros(),
        hits.as_nanos() / SAMPLES as u128,
    );
}

#[test]
fn ledger_image_cache_serializes_one_rebuild_per_revision() {
    let cache = std::sync::Arc::new(Mutex::new(super::LedgerCache::default()));
    let projection = std::sync::Arc::new(super::Ledger::new().export());
    let mut workers = Vec::new();
    for _ in 0..8 {
        let cache = std::sync::Arc::clone(&cache);
        let projection = std::sync::Arc::clone(&projection);
        workers.push(std::thread::spawn(move || {
            cache
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .ledger(13, &projection)
                .expect("the concurrent image")
        }));
    }
    let images: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("the cache reader"))
        .collect();
    // One rebuild, shared: a second rebuild would be a second allocation,
    // and every reader holding the same pointer is the proof there was one.
    assert!(
        images
            .iter()
            .all(|image| std::sync::Arc::ptr_eq(image, &images[0])),
        "concurrent readers did not share one image"
    );
}

/// Only a private SQLite backup is opened; never point this at live authority.
#[test]
#[ignore = "20 one-second polls against a private authority backup"]
fn measure_board_ledger_polls() {
    use std::time::{Duration, Instant};
    const SAMPLES: usize = 20;
    const PERIOD: Duration = Duration::from_secs(1);
    const SCALE_TASKS: usize = 3200;
    let backup = std::env::var("T3232_AUTHORITY_BACKUP").expect("private backup path");
    let (_window, _store) = PrivateWindow::boot();
    for scale in [false, true] {
        let root = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let db = root.path().join("authority.sqlite");
        std::fs::copy(&backup, &db).unwrap();
        let store = super::WorkflowStore::open(&db).unwrap();
        let overrides: super::LiveOverrides = Default::default();
        let start = |store: &super::WorkflowStore, legacy| {
            super::RuntimeActor::start(
                store,
                "main-ledger",
                super::RuntimeBoot::Cutover {
                    legacy,
                    now_ms: crate::now_epoch_ms(),
                },
                super::MAX_RUNTIME_MAILBOX,
                Box::new(super::ShellPaneTable),
                Box::new(super::LiveCatalog {
                    overrides: overrides.clone(),
                    ledger_dir: root.path().to_path_buf(),
                    headroom: super::HeadroomSource::Fixed(TEST_HEADROOM_BYTES),
                    usage: super::UsageSource::fixed(Vec::new()),
                }),
            )
            .unwrap()
        };
        let mut actor = start(&store, None);
        if scale {
            let projection = actor.shutdown().unwrap();
            let mut ledger = super::Ledger::rebuild(projection.projection().clone()).unwrap();
            let count: usize = ledger.runs().iter().map(|r| r.tasks.len()).sum();
            let run = ledger.runs()[0].id.clone();
            for n in count..SCALE_TASKS {
                ledger
                    .create_task(
                        &run,
                        format!("fixture task {n}"),
                        String::new(),
                        Vec::new(),
                        None,
                        crate::now_epoch_ms(),
                    )
                    .unwrap();
            }
            let expanded = super::WorkflowStore::open(root.path().join("scaled.sqlite")).unwrap();
            actor = start(&expanded, Some(Box::new((ledger.export(), "a".repeat(64)))));
        }
        let image = actor.view().unwrap();
        let tasks = image.projection().tasks.len();
        let workers = image.projection().workers.len();
        super::install_runtime(actor, overrides, super::UsageSource::fixed(Vec::new()));
        let mut timings = Vec::new();
        let mut refreshes = Vec::new();
        for poll in 0..SAMPLES {
            let tick = Instant::now();
            super::refresh_board_ledger();
            refreshes.push(tick.elapsed().as_secs_f64() * 1000.0);
            let started = Instant::now();
            let rows = crate::ledger_agents();
            let elapsed = started.elapsed();
            timings.push(elapsed.as_secs_f64() * 1000.0);
            println!(
                "poll tasks={tasks} workers={workers} n={} rows={} ms={:.6}",
                poll + 1,
                rows.len(),
                timings[poll]
            );
            std::thread::sleep(PERIOD.saturating_sub(tick.elapsed()));
        }
        timings.sort_by(f64::total_cmp);
        refreshes.sort_by(f64::total_cmp);
        println!(
            "SUMMARY tasks={tasks} workers={workers} samples={SAMPLES} p50_ms={:.6} p99_ms={:.6} refresh_p50_ms={:.6} refresh_p99_ms={:.6}",
            (timings[9] + timings[10]) / 2.0,
            timings[19],
            (refreshes[9] + refreshes[10]) / 2.0,
            refreshes[19]
        );
    }
}

#[test]
#[ignore = "measures an injected authority-cache stall on old and current command roads"]
fn measure_board_ledger_contention() {
    const STALL: std::time::Duration = std::time::Duration::from_millis(2200);
    let (_window, _store) = PrivateWindow::boot();
    let held = super::runtime().unwrap();
    for cached in [false, true] {
        let lock = held.cache.lock().unwrap();
        let (ready, started) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let at = std::time::Instant::now();
            ready.send(()).unwrap();
            if cached {
                std::hint::black_box(crate::ledger_agents());
            } else {
                std::hint::black_box(super::ledger_agents());
            }
            at.elapsed()
        });
        started.recv().unwrap();
        std::thread::sleep(STALL);
        drop(lock);
        let elapsed = reader.join().unwrap();
        println!(
            "contention cached={cached} held_ms={} command_ms={:.6}",
            STALL.as_millis(),
            elapsed.as_secs_f64() * 1000.0
        );
        if cached {
            assert!(elapsed < STALL);
        } else {
            assert!(elapsed >= STALL);
        }
    }
}

#[test]
fn board_snapshot_reuses_the_revision_cache_and_publishes_complete_answers() {
    let (_window, _store) = PrivateWindow::boot();
    let held = super::runtime().unwrap();
    let image = held.actor.view().unwrap();
    let ledger = super::cached_ledger(&held, &image).unwrap();
    let before = super::board_ledger_snapshot();
    super::refresh_board_ledger();
    let after = super::board_ledger_snapshot();
    assert!(!std::sync::Arc::ptr_eq(&before, &after));
    assert_eq!(before.agents, after.agents);
    assert!(std::sync::Arc::ptr_eq(
        &ledger,
        &super::cached_ledger(&held, &image).unwrap()
    ));
    assert!(std::sync::Arc::ptr_eq(
        &after,
        &super::board_ledger_snapshot()
    ));
}

#[test]
fn board_publication_changes_after_the_beat_but_restore_reads_the_live_seat() {
    const LEADER: u32 = 95_700;
    const WORKER: u32 = 95_701;
    const CHECKOUT: &str = "/tmp/board-publication";
    let (_window, _store) = PrivateWindow::boot();
    let team = "team-board-publication";
    seat_a_team(team, LEADER);
    let host = Seating {
        onto: WORKER,
        checkout: CHECKOUT,
    };
    for line in [
        "run-create --name board-publication",
        "worker-start --agent codex",
    ] {
        let said = run(
            &host,
            Vec::new(),
            team,
            zerocode_core::agent_teams::LEADER_PANE,
            TEST_CAPABILITY,
            &words(line),
            clock(),
        );
        assert_eq!(said.exit_code, 0, "{}", said.stderr);
    }
    // The board is periodic. Restore must not launch a duplicate during
    // the interval between this spawn and the next publication.
    assert!(super::board_ledger_snapshot().agents.is_empty());
    assert_eq!(
        super::last_agent_in_checkout(CHECKOUT).unwrap().term,
        Some(WORKER)
    );
    super::refresh_board_ledger();
    let published = super::board_ledger_snapshot();
    assert_eq!(published.agents.len(), 1);
    assert_eq!(published.agents[0].term, Some(WORKER));
    assert!(published.states.contains_key(&WORKER));
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

#[test]
fn board_ledger_command_does_not_wait_for_a_revision_rebuild() {
    let (_window, _store) = PrivateWindow::boot();
    let held = super::runtime().unwrap();
    let cache = held.cache.lock().unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let rows = crate::ledger_agents();
        sent.send(rows.len()).unwrap();
    });
    let answer = received.recv_timeout(std::time::Duration::from_millis(100));
    drop(cache);
    reader.join().unwrap();
    assert!(answer.is_ok(), "the UI waited for the revision cache owner");
}

#[test]
fn all_ledger_image_readers_use_the_revision_cache() {
    // The tests live in orchestration/tests.rs since 2026-09-11 (P4-1): the
    // production source is the whole file.
    let shipped = include_str!("../orchestration.rs");
    assert!(
        shipped.contains("\n#[cfg(test)]\npub(crate) mod tests;\n"),
        "the orchestration tests moved back inline — this pin reads the whole file as shipped code"
    );
    assert_eq!(
        shipped.matches("Ledger::rebuild(").count(),
        1,
        "a reader rebuilt an image outside the shared cache"
    );
    // Pin each reader's route, not the number of call sites: a reader can
    // legitimately recheck a later revision at an asynchronous boundary.
    for reader in [
        "seat_assignment(",
        "bound_run(",
        // The restore road's reading lives in the in-line walk since the
        // account switch holds the restore line across it (t-7538); the
        // switch's own readers take the same door.
        "reseat_sleeping_in_line(",
        "walled_claude_workers(",
        "rest_worker_for_switch(",
        "switch_move_ready(",
        "worker_now(",
        "last_agent_in_checkout(",
        "settled_checkouts(",
        "runtime_report(",
        // The board's two readings share one door. It takes the image and
        // the pane table in the one order that does not meet the actor's
        // own planning head-on, which is a thing to get right once.
        "with_ledger_seats<T>(",
        "point_at_waiting_mail(",
        "beat(",
        "reconcile_pane_liveness(",
        "notify_stalled_workers(",
        "resume_stalled_workers(",
        "file_crash_task(",
        "current_handover_wall(",
        "handover_order_still_current(",
        "settle_handover_terminal(",
        "walk_handovers(",
        "sleeper_awaiting(",
        "expire_sleepers(",
        "open_task_titled(",
    ] {
        let start = shipped
            .find(&format!("fn {reader}"))
            .unwrap_or_else(|| panic!("missing reader {reader}"));
        let body = &shipped[start..]
            .split_once("\nfn ")
            .map_or_else(|| &shipped[start..], |(body, _)| body);
        assert!(
            body.contains("cached_ledger(&held, &image)"),
            "{reader} bypasses the revision cache"
        );
    }
    // The background publisher and two fresh readers reach the same door. Named here
    // rather than left out, because "does not call `Ledger::rebuild`" is
    // satisfied by a reader that calls nothing at all.
    for delegating in [
        "refresh_board_ledger",
        "ledger_agents",
        "seated_live_workers",
    ] {
        let start = shipped
            .find(&format!("fn {delegating}()"))
            .unwrap_or_else(|| panic!("missing reader {delegating}"));
        let body = &shipped[start..]
            .split_once("\n}")
            .map_or_else(|| &shipped[start..], |(body, _)| body);
        assert!(
            body.contains("with_ledger_seats("),
            "{delegating} reaches a ledger without the cached door"
        );
    }
    let states = shipped
        .split_once("pub(crate) fn ledger_states_by_term()")
        .unwrap()
        .1
        .split_once("\n}")
        .unwrap()
        .0;
    assert!(states.contains("board_ledger_snapshot()"));
    // And the reclaim sweep's single-checkout re-check, which is the same
    // rule under a different door: it asks the listing rather than
    // reaching for an image of its own.
    let start = shipped
        .find("fn checkout_is_settled(")
        .expect("missing reader checkout_is_settled");
    let body = &shipped[start..]
        .split_once("\n}")
        .map_or_else(|| &shipped[start..], |(body, _)| body);
    assert!(
        body.contains("settled_checkouts()"),
        "checkout_is_settled reaches a ledger without the cached door"
    );
}

/// The pane table protects live host state, not retained ledger history.
/// Once its small seat index exists, walking the 30-day worker history
/// under that lock only makes pane planning wait behind board paint.
#[test]
fn ledger_state_walk_releases_the_team_table_after_indexing_seats() {
    let source = include_str!("../orchestration.rs");
    let body = source
        .split_once("pub(crate) fn ledger_states_by_term()")
        .expect("the ledger state reader")
        .1
        .split_once("pub(crate) fn runtime_report()")
        .expect("the next reader")
        .0;
    let indexed = body
        .find("let seats = index_team_seats(&teams)")
        .expect("the live seats are indexed");
    let viewed = body
        .find("held.actor.view()")
        .expect("the actor image is read");
    let table = body
        .find("crate::agent_teams::teams()")
        .expect("the team table is read");
    let released = body
        .find("drop(teams)")
        .expect("the team table is released after the seat snapshot");
    let history = body
        .find("for run in ledger.runs()")
        .expect("the retained workers are walked");
    assert!(
        viewed < table && indexed < released && released < history,
        "board paint holds the pane table while walking retained workers:\n{body}"
    );
}

/// Run order is creation order, but a pane can be reused by a worker in
/// an older run after a newer run's worker has left it. Paint must answer
/// with the current occupant, not whichever history row happens to be in
/// the run visited last.
#[test]
fn ledger_state_for_a_reused_seat_comes_from_its_current_worker() {
    use zerocode_core::agent_teams::{Direction, Team};
    use zerocode_core::orchestration::{Ledger, WorkerState};

    const TEAM: &str = "team-reused-seat";
    const PANE: &str = "%2";
    const TERM: u32 = 98_765;
    let mut ledger = Ledger::new();
    let older_run = ledger.create_run("created first", 1);
    let newer_run = ledger.create_run("created second", 2);
    let departed = ledger
        .start_worker(&newer_run, "claude", (TEAM, PANE), None, 3)
        .expect("the first occupant")
        .worker;
    ledger.begin_release(&departed).expect("release begins");
    assert_eq!(
        ledger.finish_release(&departed, None),
        WorkerState::Released
    );
    ledger
        .start_worker(&older_run, "codex", (TEAM, PANE), None, 4)
        .expect("the current occupant");

    let mut team = Team::new(TEAM, "unused", TERM - 1);
    team.record_split(
        PANE,
        TERM,
        zerocode_core::agent_teams::LEADER_PANE,
        Direction::Vertical,
    );
    let teams = std::collections::HashMap::from([(TEAM.to_string(), team)]);

    assert_eq!(
        super::ledger_states_for_seats(&ledger, &super::index_team_seats(&teams))
            .get(&TERM)
            .map(|one| one.ledger.as_str()),
        Some(WorkerState::Active.as_str()),
        "a released history row overrode the worker currently in the pane"
    );
}

/// The desk's letters follow the coordinator's own inbox (t-6588): a
/// question and a notice stand `pending` until the coordinator's `check`
/// hands them over, `delivered` in the batch it holds (named, with its size —
/// the unit an acknowledgement takes), and after the ack the notice is gone
/// while the question stays, `acked`, until the coordinator's `reply`
/// answers it. The question is the 「답할 우편」 and the notice the 「소식」
/// (t-9456).
#[test]
fn the_desks_mail_is_the_coordinators_inbox_letter_by_letter() {
    use zerocode_core::agent_teams::Team;
    use zerocode_core::orchestration::{Draft, MessageKind, Priority, Text};
    const LEADER_TERM: u32 = 9_410;
    /// One verb from the leader's seat, as the coordinator types it.
    fn verb(held: &mut Ledger, table: &mut Team, argv: &[String], at: i64) -> String {
        let decided = zerocode_core::orchestration::plan(
            held,
            table,
            &Catalog::new(Vec::new()),
            argv,
            zerocode_core::agent_teams::LEADER_PANE,
            at,
            Some(&test_actor(LEADER_TERM)),
        );
        assert_eq!(
            decided.reply.exit_code, 0,
            "{argv:?}: {}",
            decided.reply.stderr
        );
        decided.reply.stdout
    }
    let mut held = Ledger::new();
    let mut table = Team::new("team-desk-mail", TEST_CAPABILITY, LEADER_TERM);
    let opened: serde_json::Value = serde_json::from_str(&verb(
        &mut held,
        &mut table,
        &words("run-create --name desk-mail"),
        1_000,
    ))
    .expect("a run");
    let run_id = opened["runId"].as_str().expect("a run id").to_string();
    let address = held.run(&run_id).expect("the run").address();
    // From a teammate's seat rather than a worker row: the reply below is
    // filed to whoever asked, and a seat is answerable without a summons.
    let letter = |kind: MessageKind, body: &str| Draft {
        from: "pane:team-desk-mail/%7".to_string(),
        to: address.clone(),
        kind,
        body: Text::from(body),
        subject: Text::default(),
        priority: Priority::Normal,
        payload: Text::default(),
        thread: None,
        task: None,
        dispatch: None,
    };
    let question = held
        .post(
            &run_id,
            letter(MessageKind::Question, "main에 올려도 될까요?"),
            1_100,
        )
        .expect("a question");
    held.post(
        &run_id,
        letter(
            MessageKind::QuotaWalled,
            r#"{"workerId":"w-9","reason":"quota_wall","resetsAtMs":9000}"#,
        ),
        1_200,
    )
    .expect("a notice");
    // Every letter the desk draws, the answers first and the news after.
    let mail = |ledger: &Ledger| {
        let owed = super::desk::desk_letters(ledger.run(&run_id).expect("the run"), 1_600);
        [owed.mail, owed.news].concat()
    };

    let pending = mail(&held);
    assert_eq!(pending.len(), 2, "{pending:?}");
    assert!(pending.iter().all(|one| one.delivery == "pending"));
    assert_eq!(pending[1].reason.as_deref(), Some("quota_wall"));
    assert_eq!(pending[1].resets_at_ms, Some(9_000));
    assert_eq!(pending[1].worker.as_deref(), Some("w-9"));

    let looked: serde_json::Value =
        serde_json::from_str(&verb(&mut held, &mut table, &words("check"), 1_300))
            .expect("a delivery");
    let delivery = looked["deliveryId"]
        .as_str()
        .expect("a delivery id")
        .to_string();
    let handed = mail(&held);
    assert!(
        handed.iter().all(|one| one.delivery == "delivered"
            && one.delivery_id.as_deref() == Some(delivery.as_str())
            && one.batch == Some(2)),
        "{handed:?}"
    );

    verb(
        &mut held,
        &mut table,
        &words(&format!("check --ack {delivery} --peek")),
        1_400,
    );
    let acked = mail(&held);
    assert_eq!(
        acked.len(),
        1,
        "the notice left and the question stayed: {acked:?}"
    );
    assert_eq!(acked[0].id, question);
    assert_eq!(acked[0].delivery, "acked");

    verb(
        &mut held,
        &mut table,
        &[
            "reply".to_string(),
            "--to-message".to_string(),
            question.clone(),
            "--body".to_string(),
            "올리세요".to_string(),
            "--retry-request".to_string(),
            "desk-mail-reply".to_string(),
        ],
        1_500,
    );
    assert!(
        mail(&held).is_empty(),
        "an answered question is owed nothing"
    );
}

/// A classifier's decline and the switch of model it caused reach the desk
/// as news owed an acknowledgement (t-6747), with the facts their second
/// lines are written from: the category, whether it is routed and on which
/// rung the notice stands, and the model that answered in the bound one's
/// place, for how long.
#[test]
fn a_decline_and_a_switch_of_model_are_owed_on_the_desk() {
    use zerocode_core::orchestration::{Draft, MessageKind, Priority, Text};
    let mut held = Ledger::new();
    let run_id = held.create_run("desk-decline", 1);
    let address = held.run(&run_id).expect("the run").address();
    let notice = |kind: MessageKind, body: &str| Draft {
        from: zerocode_core::orchestration::LEDGER_ITSELF.to_string(),
        to: address.clone(),
        kind,
        body: Text::from(body),
        subject: Text::default(),
        priority: Priority::Normal,
        payload: Text::default(),
        thread: None,
        task: None,
        dispatch: None,
    };
    held.post(
        &run_id,
        notice(
            MessageKind::ClassifierDeclined,
            r#"{"workerId":"w-9","category":"reasoning_extraction","routed":false,"rung":"notify"}"#,
        ),
        1_100,
    )
    .expect("a decline");
    held.post(
        &run_id,
        notice(
            MessageKind::ModelDeviated,
            r#"{"workerId":"w-8","category":"cyber","from":"claude-fable-5-1","to":"claude-opus-4-8","scope":"session","rung":"fallback"}"#,
        ),
        1_200,
    )
    .expect("a switch");

    let owed = super::desk::desk_letters(held.run(&run_id).expect("the run"), 1_300);
    assert!(owed.mail.is_empty(), "a notice is not owed an answer");
    let mail = owed.news;

    assert_eq!(mail.len(), 2, "{mail:?}");
    assert_eq!(mail[0].kind, "classifier_declined");
    assert_eq!(mail[0].worker.as_deref(), Some("w-9"));
    assert_eq!(mail[0].category.as_deref(), Some("reasoning_extraction"));
    assert_eq!(mail[0].routed, Some(false));
    assert_eq!(mail[0].rung.as_deref(), Some("notify"));
    assert_eq!(mail[1].kind, "model_deviated");
    assert_eq!(mail[1].category.as_deref(), Some("cyber"));
    assert_eq!(mail[1].switched_to.as_deref(), Some("claude-opus-4-8"));
    assert_eq!(mail[1].scope.as_deref(), Some("session"));
    assert!(mail.iter().all(|one| one.delivery == "pending"));
}

/// The switch scan's cursor moves only past switches the ledger holds
/// (t-7153, P2): a chunk the actor refuses keeps the cursor where it was
/// and answers the chunks not held as WAITING, with the cursor they earn
/// once held; nothing to hold moves the cursor at once; a refusal after
/// some chunks landed keeps the cursor and holds back only what did not
/// land, and the landed rows dedupe when the same lines are asked again.
/// And a switch is asked only for the attempt that was open when it was
/// written (R3): one dated before the attempt began, or undated, is asked
/// of nobody, and the cursor moves past it; what is asked carries the
/// worker, the attempt and the file it was read from.
#[test]
fn the_switch_scans_cursor_moves_only_past_what_the_ledger_holds() {
    use crate::quota_wall::{DeviationScan, ScanCursor};
    use zerocode_core::orchestration::{MAX_LIST, ModelDeviation};
    const ATTEMPT_MS: i64 = 1_000_000;
    let binding = super::SwitchBinding {
        worker: "w-1",
        dispatch: "dp-1",
        source: "/t/w-1.jsonl",
        attempt_started_ms: ATTEMPT_MS,
    };
    let switch_at = |key: &str, at_ms: Option<i64>| ModelDeviation {
        worker: String::new(),
        dispatch: String::new(),
        source: String::new(),
        key: key.to_string(),
        from: "claude-fable-5-1".to_string(),
        to: "claude-opus-4-8".to_string(),
        category: Some("cyber".to_string()),
        scope: Some("session".to_string()),
        at_ms,
    };
    let switch = |key: &str| switch_at(key, Some(ATTEMPT_MS + 1));
    let at = |offset: u64| ScanCursor {
        offset,
        ..ScanCursor::default()
    };
    let scan = |switches: Vec<ModelDeviation>, next: u64| DeviationScan {
        switches,
        next: at(next),
    };
    let bound = |switch: ModelDeviation| ModelDeviation {
        worker: "w-1".to_string(),
        dispatch: "dp-1".to_string(),
        source: "/t/w-1.jsonl".to_string(),
        ..switch
    };
    let waiting = |switches: Vec<ModelDeviation>, next: u64| {
        Some(super::PendingSwitches {
            switches: switches.into_iter().map(bound).collect(),
            source: "/t/w-1.jsonl".to_string(),
            next: at(next),
        })
    };
    let record_scanned =
        |scan: DeviationScan,
         held_at: u64,
         record: &mut dyn FnMut(Vec<ModelDeviation>) -> Result<bool, super::RuntimeError>| {
            super::record_scanned_switches(&binding, scan, at(held_at), record)
        };

    // Nothing to hold: the cursor moves, and nobody is asked.
    let mut asked = 0;
    assert_eq!(
        record_scanned(scan(Vec::new(), 40), 10, &mut |_| {
            asked += 1;
            Ok(true)
        }),
        (at(40), None, false)
    );
    // Nothing of this attempt's to hold — a switch dated before it began,
    // one with no date — moves the cursor too, and asks nobody.
    assert_eq!(
        record_scanned(
            scan(
                vec![
                    switch_at("s-earlier", Some(ATTEMPT_MS - 1)),
                    switch_at("s-undated", None),
                ],
                90
            ),
            40,
            &mut |_| {
                asked += 1;
                Ok(true)
            }
        ),
        (at(90), None, false),
        "another attempt's switch was asked of the ledger"
    );
    assert_eq!(asked, 0);

    // The ledger's own dedupe stands in for the ledger: a key it holds
    // answers false. Refused once, the cursor stays and the reading waits;
    // asked again with the same lines, the switch lands and the cursor
    // moves.
    let mut held: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut refuse_next = true;
    let mut record = |switches: Vec<ModelDeviation>| -> Result<bool, super::RuntimeError> {
        if refuse_next {
            refuse_next = false;
            return Err(super::RuntimeError::RecoveryRequired);
        }
        let mut told = false;
        for one in switches {
            assert_eq!(
                (
                    one.worker.as_str(),
                    one.dispatch.as_str(),
                    one.source.as_str()
                ),
                ("w-1", "dp-1", "/t/w-1.jsonl"),
                "named for its worker, its attempt and its file"
            );
            told |= held.insert(one.key);
        }
        Ok(told)
    };
    assert_eq!(
        record_scanned(scan(vec![switch("s-1")], 700), 0, &mut record),
        (at(0), waiting(vec![switch("s-1")], 700), false),
        "a refused row moved the cursor, or was let go"
    );
    assert_eq!(
        record_scanned(scan(vec![switch("s-1")], 700), 0, &mut record),
        (at(700), None, true)
    );
    assert_eq!(held.len(), 1);

    // Two chunks, the second refused: the cursor stays and the second
    // chunk waits; asked again, the first chunk dedupes and the second
    // lands — each switch once.
    let many: Vec<ModelDeviation> = (0..=MAX_LIST)
        .map(|index| switch(&format!("m-{index}")))
        .collect();
    let mut chunks = 0;
    let mut record = |switches: Vec<ModelDeviation>| -> Result<bool, super::RuntimeError> {
        chunks += 1;
        if chunks == 2 {
            return Err(super::RuntimeError::Closed);
        }
        let mut told = false;
        for one in switches {
            told |= held.insert(one.key);
        }
        Ok(told)
    };
    assert_eq!(
        record_scanned(scan(many.clone(), 9_000), 700, &mut record),
        (at(700), waiting(many[MAX_LIST..].to_vec(), 9_000), true),
        "a partly refused reading moved the cursor, or held back what landed"
    );
    assert_eq!(
        record_scanned(scan(many, 9_000), 700, &mut record),
        (at(9_000), None, true)
    );
    assert_eq!(held.len(), 1 + MAX_LIST + 1, "each switch once");

    // What waits is asked a list's worth at a time — the actor refuses a
    // longer request for good — and lands whole; the cursor then stands
    // where the waiting reading ended.
    let longer: Vec<ModelDeviation> = (0..=MAX_LIST)
        .map(|index| switch(&format!("p-{index}")))
        .collect();
    let mut listed = |switches: Vec<ModelDeviation>| -> Result<bool, super::RuntimeError> {
        if switches.len() > MAX_LIST {
            return Err(super::RuntimeError::InvalidInput);
        }
        let mut told = false;
        for one in switches {
            told |= held.insert(one.key);
        }
        Ok(told)
    };
    let nowhere = std::path::Path::new("/nonexistent/t-7153/w-1.jsonl");
    assert_eq!(
        super::scan_switches_a_beat(
            &binding,
            nowhere,
            at(700),
            waiting(longer, 12_000),
            &mut listed
        ),
        (at(12_000), None, true),
        "a waiting reading longer than a list was never held"
    );
    assert_eq!(held.len(), 2 * (MAX_LIST + 1) + 1, "each switch once");

    // A row the ledger could never hold — a switch naming no model it went
    // to — is asked of nobody and holds nothing back: the rows beside it
    // land and the cursor moves.
    let unfit = ModelDeviation {
        to: String::new(),
        ..switch("s-unfit")
    };
    let mut fitting = |switches: Vec<ModelDeviation>| -> Result<bool, super::RuntimeError> {
        if switches.iter().any(|one| !one.fits()) {
            return Err(super::RuntimeError::InvalidInput);
        }
        let mut told = false;
        for one in switches {
            told |= held.insert(one.key);
        }
        Ok(told)
    };
    assert_eq!(
        record_scanned(
            scan(vec![unfit, switch("s-fit")], 13_000),
            12_000,
            &mut fitting
        ),
        (at(13_000), None, true),
        "a row the ledger can never hold held back the rows read with it"
    );
    assert!(held.contains("s-fit") && !held.contains("s-unfit"));
}

/// A worker's row carries what the task board's roster reads its health from
/// (t-6588): the question it is waiting on (`Run::awaiting_reply`), the newest
/// quota wall its attempt met as the ledger reads it back (`newest_wall`), and
/// its pane inside the team — and nothing about a wall another attempt met.
#[test]
fn a_workers_row_carries_its_question_its_wall_and_its_pane() {
    use zerocode_core::orchestration::{Draft, MessageKind, Priority, Text};
    let mut ledger = Ledger::new();
    let run = ledger.create_run("roster", 1);
    let task = ledger
        .create_task(&run, "do".into(), "roster task".into(), vec![], None, 2)
        .expect("a task");
    let started = ledger
        .start_worker(&run, "codex", ("team-roster", "%2"), Some(&task), 3)
        .expect("a worker");
    let dispatch = started.dispatch.clone().expect("it carries the task");
    let address = ledger.run(&run).expect("the run").address();
    let row = |ledger: &Ledger| {
        super::ledger_agents_for_seats(ledger, &super::TeamSeatIndex::new())
            .into_iter()
            .find(|one| one.worker == started.worker)
            .expect("the worker's row")
    };
    let quiet = row(&ledger);
    assert!(!quiet.asking && quiet.wall.is_none(), "{quiet:?}");
    assert_eq!(quiet.pane, "%2");

    let from = zerocode_core::orchestration::worker_address(&started.worker);
    let letter = |kind: MessageKind, body: String| Draft {
        from: from.clone(),
        to: address.clone(),
        kind,
        body: Text::from(body),
        subject: Text::default(),
        priority: Priority::Normal,
        payload: Text::default(),
        thread: None,
        task: Some(task.clone()),
        dispatch: Some(dispatch.clone()),
    };
    ledger
        .post(
            &run,
            letter(MessageKind::Question, "which base?".into()),
            10,
        )
        .expect("a question");
    ledger
        .post(
            &run,
            letter(
                MessageKind::QuotaWalled,
                serde_json::json!({ "observedAtMs": 20, "resetsAtMs": 2_700_020 }).to_string(),
            ),
            20,
        )
        .expect("a wall");
    let walled = row(&ledger);
    assert!(walled.asking, "{walled:?}");
    let wall = walled.wall.expect("the wall is read back");
    assert_eq!(wall.resets_at_ms, Some(2_700_020));
    assert!(wall.stands_until_ms > 2_700_020, "{wall:?}");
}

/// The desk calls a run's coordinator seat this window's only where the pane
/// the seat names (`team/pane`) is in this window's pane table (t-6588) —
/// the seat a reply from the board is written as.
#[test]
fn a_seat_is_held_only_where_this_window_holds_its_pane() {
    let seats = super::TeamSeatIndex::from([(
        "team-1".to_string(),
        std::collections::HashMap::from([("%0".to_string(), 7_u32)]),
    )]);
    assert!(super::seat_is_held(&seats, "team-1/%0"));
    assert!(!super::seat_is_held(&seats, "team-1/%2"));
    assert!(!super::seat_is_held(&seats, "team-2/%0"));
    assert!(!super::seat_is_held(&seats, "team-1"));
}

/// What the board's ledger readings cost, on this window's real shape.
///
/// In the tree and `#[ignore]`d on purpose. A ledger read was put on the
/// paint path once before and taken back off it (a1e2ec3) because it was
/// O(workers x panes) with the pane table held; the number that decision
/// turned on has to be re-askable by whoever changes this next, and a
/// sentence in a commit message is not re-askable. It asserts nothing —
/// a wall-clock threshold on a shared machine is a flaky test, not a rule.
///
/// `cargo test -p zerocode-shell --bin zerocode-shell -- --ignored \
/// --nocapture measure_ledger_agents_walk`
#[test]
#[ignore = "measurement, not a rule"]
fn measure_ledger_agents_walk() {
    use zerocode_core::agent_teams::{Direction, Team};
    use zerocode_core::orchestration::Ledger;

    // This window's real shape tonight: nine runs, one of them holding 65
    // released rows beside two live ones, ten panes on the strip.
    let mut ledger = Ledger::new();
    let mut teams = std::collections::HashMap::new();
    for r in 0..9 {
        let run = ledger.create_run(&format!("run {r}"), 1);
        for w in 0..67 {
            let team = format!("team-{r}");
            let pane = format!("%{w}");
            let born = ledger
                .start_worker(&run, "claude", (&team, &pane), None, 2)
                .expect("worker")
                .worker;
            if w < 65 {
                ledger.begin_release(&born).expect("release");
                ledger.finish_release(&born, None);
            }
        }
    }
    for t in 0..10u32 {
        let id = format!("team-{t}");
        let mut team = Team::new(&id, "unused", 1000 + t);
        team.record_split(
            "%65",
            2000 + t,
            zerocode_core::agent_teams::LEADER_PANE,
            Direction::Vertical,
        );
        teams.insert(id, team);
    }
    let seats = super::index_team_seats(&teams);
    let rounds = 200u32;
    let mut total = 0usize;
    let was = std::time::Instant::now();
    for _ in 0..rounds {
        total += super::ledger_states_for_seats(&ledger, &seats).len();
    }
    let was = was.elapsed() / rounds;
    let added = std::time::Instant::now();
    for _ in 0..rounds {
        total += super::ledger_agents_for_seats(&ledger, &seats).len();
    }
    let added = added.elapsed() / rounds;
    println!(
        "rows={} old-walk={was:?} added-walk={added:?} (9 runs x 67 workers, 10 seats)",
        total / rounds as usize,
    );
    let indexing = std::time::Instant::now();
    for _ in 0..rounds {
        std::hint::black_box(super::index_team_seats(&teams));
    }
    println!(
        "index (the only work under the lock)={:?}",
        indexing.elapsed() / rounds
    );
}

/// A worker this window holds no seat for is still NAMED.
///
/// The failure the board showed: its two card sources both ask "what is
/// this window holding", so a summons whose pane this window does not have
/// appeared nowhere — not a card, not a row, nothing — while `worker-list`
/// printed it the whole time. What must come back is the worker itself,
/// with the seat it does not have said out loud rather than guessed at.
#[test]
fn a_worker_with_no_seat_in_this_window_is_still_named() {
    use zerocode_core::agent_teams::{Direction, Team};
    use zerocode_core::orchestration::{Ledger, WorkerState};

    const SEATED_TEAM: &str = "team-seated";
    const LOST_TEAM: &str = "team-lost";
    const PANE: &str = "%2";
    const TERM: u32 = 55_501;

    let mut ledger = Ledger::new();
    let run_id = ledger.create_run("two workers, one seat", 1);
    ledger
        .start_worker(&run_id, "claude", (SEATED_TEAM, PANE), None, 2)
        .expect("the seated worker");
    ledger
        .start_worker(&run_id, "codex", (LOST_TEAM, PANE), None, 3)
        .expect("the worker whose window this is not");
    // And one that ENDED. A summons somebody answered is not a summons
    // anybody is still owed, and the board is not a receipt pile.
    let gone = ledger
        .start_worker(&run_id, "claude", ("team-gone", PANE), None, 4)
        .expect("the third worker")
        .worker;
    ledger.begin_release(&gone).expect("release begins");
    assert_eq!(ledger.finish_release(&gone, None), WorkerState::Released);

    // This window holds exactly one of the three seats.
    let mut seated = Team::new(SEATED_TEAM, "unused", TERM - 1);
    seated.record_split(
        PANE,
        TERM,
        zerocode_core::agent_teams::LEADER_PANE,
        Direction::Vertical,
    );
    let teams = std::collections::HashMap::from([(SEATED_TEAM.to_string(), seated)]);
    let listed = super::ledger_agents_for_seats(&ledger, &super::index_team_seats(&teams));

    assert_eq!(
        listed.len(),
        2,
        "the released row is on the board, or a live one is missing: {listed:?}"
    );
    let seated = listed
        .iter()
        .find(|one| one.agent == "claude")
        .expect("the seated worker is listed");
    assert_eq!(seated.term, Some(TERM), "the seat this window holds");
    assert_eq!(
        seated.ledger,
        WorkerState::Active.as_str(),
        "a seated worker reads as the ledger recorded it"
    );

    let lost = listed
        .iter()
        .find(|one| one.agent == "codex")
        .expect("the worker with no seat here is listed at all");
    assert_eq!(lost.term, None, "a seat was invented for a pane we lack");
    assert_eq!(
        lost.state, "working",
        "the coordinator is still waiting on this summons"
    );
    /* And the two authorities are BOTH on the card. The record says the
     * coordinator is owed a report; the reading says the terminal is not
     * there. `zerocode_core::board` resolves that pair — the verdict may
     * take the claim of work away and never add one — so this row lands
     * in the column endings are read in rather than drawing as "작업 중"
     * over a terminal nobody can find (t-612). */
    assert_eq!(
        lost.ledger,
        WorkerState::Released.as_str(),
        "a worker with no seat was reported as if its terminal were up"
    );
    assert_eq!(lost.run, run_id, "the run to go and ask about it");
}

/// A seat whose dispatch the ledger has closed says WHEN the work ended,
/// and one still open says nothing — the one stored fact that can call a
/// pane `done` when its agent speaks no hooks.
#[test]
fn a_closed_dispatch_stamps_when_the_seats_work_ended() {
    const PANE: &str = "%2";
    let mut ledger = Ledger::new();
    let run = ledger.create_run("hookless", 1);
    let task = ledger
        .create_task(
            &run,
            "probe".to_string(),
            String::new(),
            Vec::new(),
            None,
            2,
        )
        .expect("a task");
    let started = ledger
        .start_worker(&run, "zo", ("team-a", PANE), Some(&task), 3)
        .expect("a zo worker");
    let mut team = zerocode_core::agent_teams::Team::new("team-a", "unused", 100);
    team.record_split(
        PANE,
        101,
        "%1",
        zerocode_core::agent_teams::Direction::Vertical,
    );
    let teams = std::collections::HashMap::from([("team-a".to_string(), team)]);
    let seats = super::index_team_seats(&teams);

    let open = super::ledger_states_for_seats(&ledger, &seats);
    assert_eq!(
        open.get(&101).and_then(|one| one.work_ended_ms),
        None,
        "an open dispatch claimed the work had ended"
    );

    ledger
        .post(
            &run,
            zerocode_core::orchestration::Draft {
                from: format!("worker:{}", started.worker),
                to: format!("run:{run}"),
                kind: zerocode_core::orchestration::MessageKind::WorkerDone,
                body: "{\"ok\":true}".to_string().into(),
                subject: Default::default(),
                priority: zerocode_core::orchestration::Priority::Normal,
                payload: Default::default(),
                thread: None,
                task: None,
                dispatch: started.dispatch.clone(),
            },
            9,
        )
        .expect("the worker reports");
    let ended = super::ledger_states_for_seats(&ledger, &seats);
    let seat = ended.get(&101).expect("still seated: its terminal is up");
    assert_eq!(seat.ledger, WorkerState::Reclaimable.as_str());
    assert_eq!(seat.work_ended_ms, Some(9));
}

#[test]
fn the_same_pane_id_in_two_teams_keeps_two_distinct_terms() {
    use zerocode_core::agent_teams::{Direction, Team};
    use zerocode_core::orchestration::{Ledger, WorkerState};

    const PANE: &str = "%2";
    let mut ledger = Ledger::new();
    let first = ledger.create_run("first", 1);
    let second = ledger.create_run("second", 2);
    ledger
        .start_worker(&first, "codex", ("team-a", PANE), None, 3)
        .expect("team a worker");
    let pending = ledger
        .start_worker(&second, "claude", ("team-b", PANE), None, 4)
        .expect("team b worker")
        .worker;
    ledger.begin_release(&pending).expect("release is pending");

    let mut team_a = Team::new("team-a", "unused", 100);
    team_a.record_split(PANE, 101, "%1", Direction::Vertical);
    let mut team_b = Team::new("team-b", "unused", 200);
    team_b.record_split(PANE, 201, "%1", Direction::Vertical);
    let teams = std::collections::HashMap::from([
        ("team-a".to_string(), team_a),
        ("team-b".to_string(), team_b),
    ]);
    let states = super::ledger_states_for_seats(&ledger, &super::index_team_seats(&teams));

    assert_eq!(
        states.get(&101).map(|one| one.ledger.as_str()),
        Some(WorkerState::Active.as_str())
    );
    assert_eq!(
        states.get(&201).map(|one| one.ledger.as_str()),
        Some(WorkerState::ReleasePending.as_str())
    );
}

#[test]
fn aliased_terms_keep_the_existing_last_worker_winner() {
    use zerocode_core::agent_teams::{Direction, Team};
    use zerocode_core::orchestration::{Ledger, WorkerState};

    const TERM: u32 = 301;
    let mut ledger = Ledger::new();
    let first = ledger.create_run("first", 1);
    let second = ledger.create_run("second", 2);
    ledger
        .start_worker(&first, "codex", ("team-a", "%2"), None, 3)
        .expect("first worker");
    let later = ledger
        .start_worker(&second, "claude", ("team-b", "%7"), None, 4)
        .expect("later worker")
        .worker;
    ledger.begin_release(&later).expect("release is pending");

    let mut team_a = Team::new("team-a", "unused", 300);
    team_a.record_split("%2", TERM, "%1", Direction::Vertical);
    let mut team_b = Team::new("team-b", "unused", 400);
    team_b.record_split("%7", TERM, "%1", Direction::Vertical);
    let teams = std::collections::HashMap::from([
        ("team-a".to_string(), team_a),
        ("team-b".to_string(), team_b),
    ]);

    assert_eq!(
        super::ledger_states_for_seats(&ledger, &super::index_team_seats(&teams))
            .get(&TERM)
            .map(|one| one.ledger.as_str()),
        Some(WorkerState::ReleasePending.as_str()),
        "the worker visited last no longer wins an aliased host term"
    );
}

thread_local! {
    /// The word a degraded window answers with, for the tests that need
    /// one — and where a boot a test runs for real records its own
    /// ([`super::stand_down`]).
    ///
    /// Per-thread because the real flag is process-global — a window has
    /// one boot — and a test that set it would put every other test in
    /// this binary into a window that refuses everything. The old
    /// write-refusing switch that lived beside this is gone with the
    /// hand-rolled save it reached into: a disk that refuses is injected
    /// at the STORE now (a trigger in a private window's own database),
    /// which is the same seam the actor's own battery uses.
    static UNAVAILABLE_HERE: std::cell::RefCell<Option<super::StoodDown>> =
        const { std::cell::RefCell::new(None) };
}

/// What a degraded window would say on THIS thread, if it is one.
pub(super) fn stood_down_here() -> Option<super::StoodDown> {
    UNAVAILABLE_HERE.with(|held| held.borrow().clone())
}

/// A boot this test ran stood down: its word, on this thread.
pub(super) fn stand_down_here(stood: super::StoodDown) {
    UNAVAILABLE_HERE.with(|held| *held.borrow_mut() = Some(stood));
}

/// Make this window degraded, and give it back on the way out.
struct Degraded;

impl Degraded {
    fn begin() -> Self {
        // The sentence a boot says of a ledger file on its disk it cannot
        // read — so the same file is a durable ledger left unread.
        let said = super::ledger_is_unavailable(
            "its 12 bytes are not a ledger this window can read (test)",
        );
        stand_down_here(super::StoodDown {
            unread: Some(said.clone()),
            said,
        });
        Self
    }
}

impl Degraded {
    /// A boot of a window over `root`, run for real — the product's own
    /// `open` — that stood down (t-7538, astra R3-1): its word stays on
    /// this thread until the guard goes.
    fn booted_over(root: &Path) -> Self {
        assert!(
            std::env::var_os(super::ORCHESTRATION_DATA_ROOT_ENV).is_none(),
            "the boot would read another root than the one it was given"
        );
        assert_eq!(
            super::open_with_headroom(
                root,
                clock(),
                super::HeadroomSource::Fixed(TEST_HEADROOM_BYTES),
            ),
            Restarted::default()
        );
        assert!(stood_down_here().is_some(), "the boot stood the ledger up");
        Self
    }
}

impl Drop for Degraded {
    fn drop(&mut self) {
        UNAVAILABLE_HERE.with(|held| *held.borrow_mut() = None);
    }
}

/// The sentences a caller has always read stay word for word: a write
/// the disk refused, and a seat that cannot prove itself — whatever
/// error type now stands behind them.
#[test]
fn a_refusal_keeps_the_sentences_this_window_has_always_used() {
    let not_durable = refused_by_runtime(super::RuntimeError::NotDurable);
    assert!(
        not_durable
            .stderr
            .contains("the ledger could not be written — the change is not durable"),
        "{}",
        not_durable.stderr
    );
    let unauthorized = refused_by_runtime(super::RuntimeError::UnauthorizedSeat);
    assert!(
        unauthorized
            .stderr
            .contains("stale or unauthorized agent pane"),
        "{}",
        unauthorized.stderr
    );
}

/* Two tests died here with the hand-rolled save they were about, and
 * their names deserve their successors said aloud:
 *
 * · `a_commit_that_only_became_visible_is_not_a_commit_that_landed` —
 *   the `landed`/`VisibleButSyncFailed` door guarded the JSON file's
 *   rename; the authority is a SQLite store now, whose durability is the
 *   store's own battery (`workflow_store.rs`), not a rename this file
 *   performs.
 *
 * · `a_write_that_failed_is_not_answered_as_a_success` — the write-door
 *   ordering (snapshot inside the writer lock) is structural in the
 *   actor: one thread, one write-through per mutation, no second saver
 *   to order against. The refusal half lives on twice over — the actor's
 *   own `a_host_fact_the_disk_refuses_lands_nothing_and_recovers` and
 *   this file's `a_disk_that_refuses_a_write_still_answers_a_look_from_
 *   the_disks_word` — and the sentence the caller reads is pinned by
 *   `a_refusal_keeps_the_sentences_this_window_has_always_used`.
 */
use super::*;
use zerocode_core::launch::split_command_line;

/// A window that opens nothing.
///
/// Every verb this file's own tests drive either changes only the ledger or
/// is expected to be refused, so a host that answers "no" to everything is
/// the honest stand-in — one that pretended to cut panes would be testing
/// the pretence.
/// A host that does nothing and remembers being asked.
///
/// `Nowhere` answers "no" to everything, which cannot tell "the road
/// stopped before the host" from "the host declined" — and that is exactly
/// the difference a degraded window has to make.
#[derive(Default)]
struct Counting {
    effects: std::sync::atomic::AtomicUsize,
    panes: Mutex<std::collections::HashSet<u32>>,
    actors: Mutex<std::collections::HashMap<u32, String>>,
}

impl Counting {
    fn calls(&self) -> usize {
        self.effects.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn asked(&self) {
        self.effects
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn set_pane(&self, term: u32, present: bool) {
        let mut panes = self.panes.lock().unwrap_or_else(|held| held.into_inner());
        if present {
            panes.insert(term);
        } else {
            panes.remove(&term);
        }
    }

    fn set_actor(&self, term: u32, actor: &str) {
        self.actors
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(term, actor.to_string());
    }
}

impl Host for Counting {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        _token: &str,
    ) -> Option<u32> {
        self.asked();
        None
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        self.asked();
        false
    }
    fn pane_exists(&self, term: u32) -> bool {
        self.panes
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .contains(&term)
    }
    fn capture(&self, _term: u32) -> Option<String> {
        self.asked();
        None
    }
    fn focus(&self, _term: u32) -> bool {
        self.asked();
        false
    }
    fn close(&self, _term: u32) {
        self.asked();
    }
    fn actor_for(&self, term: u32) -> Option<String> {
        // NOT counted. It is a question about who is sitting there, asked
        // of state the window already has, and it changes nothing — the
        // pin is about EFFECTS.
        self.actors
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&term)
            .cloned()
            .or_else(|| Some(test_actor(term)))
    }
}

struct Nowhere;

impl Host for Nowhere {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        _token: &str,
    ) -> Option<u32> {
        None
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        false
    }
    fn capture(&self, _term: u32) -> Option<String> {
        None
    }
    fn focus(&self, _term: u32) -> bool {
        false
    }
    fn close(&self, _term: u32) {}
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
}

/// The two tests that beat standing orders take turns.
///
/// A beat walks EVERY run in a process-wide ledger and holds a
/// process-wide door, so two of them running at once read each other's
/// runs and each other's door. Not a flaw in either test: it is what the
/// tick is — one window, one ledger, one beat — and a test of it has to be
/// the only one beating.
fn one_beat_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static TURNS: Mutex<()> = Mutex::new(());
    TURNS.lock().unwrap_or_else(|held| held.into_inner())
}

thread_local! {
    static RECONCILE_PANES_HERE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The boot this thread's tests measure the sleepers' grace from
    /// (t-3058). `None` — every test that does not name one — leaves the
    /// production cell, which the test binary never sets, so no sleeper
    /// another scenario left in the shared runtime is ended by a beat
    /// here.
    static BOOTED_AT_HERE: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

pub(super) fn pane_reconciliation_enabled() -> bool {
    RECONCILE_PANES_HERE.with(std::cell::Cell::get)
}

pub(super) fn booted_at_override() -> Option<i64> {
    BOOTED_AT_HERE.with(std::cell::Cell::get)
}

/// Name this thread's boot for the grace, and forget it on drop.
struct BootedHere;

impl BootedHere {
    fn at(booted_ms: i64) -> Self {
        BOOTED_AT_HERE.with(|cell| cell.set(Some(booted_ms)));
        Self
    }
}

impl Drop for BootedHere {
    fn drop(&mut self) {
        BOOTED_AT_HERE.with(|cell| cell.set(None));
    }
}

struct ReconcilePanesHere;

impl ReconcilePanesHere {
    fn begin() -> Self {
        RECONCILE_PANES_HERE.with(|enabled| enabled.set(true));
        Self
    }
}

impl Drop for ReconcilePanesHere {
    fn drop(&mut self) {
        RECONCILE_PANES_HERE.with(|enabled| enabled.set(false));
    }
}

#[test]
fn a_missing_pane_needs_five_consecutive_beats_and_retries_as_one_fact() {
    use zerocode_core::agent_teams::{Direction, Team};
    use zerocode_core::orchestration::Ledger;

    const TEAM: &str = "team-pane-reconcile";
    const PANE: &str = "%2";
    const TERM: u32 = 81_002;
    let mut ledger = Ledger::new();
    let run = ledger.create_run("pane reconciliation", 1);
    let worker = ledger
        .start_worker(&run, "codex", (TEAM, PANE), None, 2)
        .expect("a worker")
        .worker;
    let mut team = Team::new(TEAM, "unused", TERM - 1);
    team.record_split(PANE, TERM, "%1", Direction::Vertical);
    let mapped = index_team_seats(&std::collections::HashMap::from([(TEAM.to_string(), team)]));
    // A restarted window can have no in-memory pane table at all. With no
    // current or remembered term there is no machine question to ask, so
    // five beats of that shape remain unknown rather than becoming a
    // fictional absence.
    let mut seats = TeamSeatIndex::new();
    let host = Counting::default();
    let mut reconciler = PaneReconciler::default();

    for now_ms in [1_000, 2_000, 3_000, 4_000, 5_000] {
        assert_eq!(
            reconciler.observe(&host, &ledger, &seats, now_ms),
            PaneReconciliation::default()
        );
    }
    seats = mapped;
    for now_ms in [6_000, 7_000, 8_000, 9_000] {
        assert_eq!(
            reconciler.observe(&host, &ledger, &seats, now_ms),
            PaneReconciliation::default()
        );
    }
    let missing = PaneReconciliation {
        missing: vec![(worker.clone(), 6_000)],
        seen: Vec::new(),
    };
    assert_eq!(reconciler.observe(&host, &ledger, &seats, 10_000), missing);
    // Level-triggered until the actor accepts it: a refused durable write
    // gets another chance, while the ledger de-duplicates the event.
    assert_eq!(reconciler.observe(&host, &ledger, &seats, 11_000), missing);

    host.set_pane(TERM, true);
    assert_eq!(
        reconciler.observe(&host, &ledger, &seats, 12_000),
        PaneReconciliation {
            missing: Vec::new(),
            seen: vec![worker.clone()],
        }
    );
    host.set_pane(TERM, false);
    for now_ms in [13_000, 14_000, 15_000, 16_000] {
        assert_eq!(
            reconciler.observe(&host, &ledger, &seats, now_ms),
            PaneReconciliation::default()
        );
    }
    assert_eq!(
        reconciler.observe(&host, &ledger, &seats, 17_000),
        PaneReconciliation {
            missing: vec![(worker, 13_000)],
            seen: Vec::new(),
        }
    );
}

#[test]
fn a_reseat_or_a_non_live_state_discards_an_old_missing_streak() {
    use zerocode_core::agent_teams::{Direction, Team};
    use zerocode_core::orchestration::Ledger;

    const TEAM: &str = "team-pane-reseat";
    const PANE: &str = "%2";
    const FIRST_TERM: u32 = 82_002;
    const SECOND_TERM: u32 = 82_003;
    let mut ledger = Ledger::new();
    let run = ledger.create_run("pane reseat", 1);
    let worker = ledger
        .start_worker(&run, "codex", (TEAM, PANE), None, 2)
        .expect("a worker")
        .worker;
    let mut team = Team::new(TEAM, "unused", FIRST_TERM - 1);
    team.record_split(PANE, FIRST_TERM, "%1", Direction::Vertical);
    let mut seats = index_team_seats(&std::collections::HashMap::from([(TEAM.to_string(), team)]));
    let host = Counting::default();
    let mut reconciler = PaneReconciler::default();

    for now_ms in [1_000, 2_000, 3_000, 4_000] {
        assert!(
            reconciler
                .observe(&host, &ledger, &seats, now_ms)
                .missing
                .is_empty()
        );
    }
    seats
        .get_mut(TEAM)
        .expect("the team")
        .insert(PANE.to_string(), SECOND_TERM);
    for now_ms in [5_000, 6_000, 7_000, 8_000] {
        assert!(
            reconciler
                .observe(&host, &ledger, &seats, now_ms)
                .missing
                .is_empty(),
            "the old term helped its replacement reach the threshold"
        );
    }
    assert_eq!(
        reconciler.observe(&host, &ledger, &seats, 9_000).missing,
        vec![(worker.clone(), 5_000)]
    );

    ledger.begin_release(&worker).expect("release begins");
    assert_eq!(
        reconciler.observe(&host, &ledger, &seats, 10_000),
        PaneReconciliation::default()
    );
    assert!(
        reconciler.missing.is_empty(),
        "a worker that no longer claims a live pane kept its old evidence"
    );
}

#[test]
fn an_orphan_uses_its_last_known_term_and_an_unknown_one_is_not_declared_dead() {
    use zerocode_core::agent_teams::{Direction, Team};
    use zerocode_core::orchestration::Ledger;

    const TEAM: &str = "team-orphan-probe";
    const PANE: &str = "%2";
    const TERM: u32 = 82_102;
    let mut ledger = Ledger::new();
    let run = ledger.create_run("orphan probe", 1);
    let task = ledger
        .create_task(
            &run,
            "keep working".to_string(),
            String::new(),
            Vec::new(),
            None,
            2,
        )
        .expect("task");
    let worker = ledger
        .start_worker(&run, "codex", (TEAM, PANE), Some(&task), 3)
        .expect("worker")
        .worker;
    assert!(ledger.worker_seated((TEAM, PANE), "/wt/orphan"));
    let mut team = Team::new(TEAM, "unused", TERM - 1);
    team.record_split(PANE, TERM, "%1", Direction::Vertical);
    let seats = index_team_seats(&std::collections::HashMap::from([(TEAM.to_string(), team)]));
    let host = Counting::default();
    host.set_pane(TERM, true);
    let mut reconciler = PaneReconciler::default();
    assert_eq!(
        reconciler.observe(&host, &ledger, &seats, 1_000).seen,
        vec![worker.clone()],
        "the live term was not learned"
    );

    assert_eq!(ledger.team_dissolved(TEAM, 2_000).workers, 1);
    let no_seats = TeamSeatIndex::new();
    assert_eq!(
        reconciler.observe(&host, &ledger, &no_seats, 3_000).seen,
        vec![worker.clone()],
        "dropping the leader's routing table hid a live orphan pane"
    );

    host.set_pane(TERM, false);
    for now_ms in [4_000, 5_000, 6_000, 7_000] {
        assert!(
            reconciler
                .observe(&host, &ledger, &no_seats, now_ms)
                .missing
                .is_empty()
        );
    }
    assert_eq!(
        reconciler.observe(&host, &ledger, &no_seats, 8_000).missing,
        vec![(worker.clone(), 4_000)],
        "a genuinely vanished orphan was not confirmed"
    );

    // A recycled terminal number is not the old worker's pane. The actor
    // identity learned while the team table proved ownership fences it.
    host.set_pane(TERM, true);
    let mut recycled = PaneReconciler::default();
    assert_eq!(
        recycled.observe(&host, &ledger, &seats, 8_500).seen,
        vec![worker.clone()]
    );
    host.set_actor(TERM, "another-pane-incarnation");
    assert_eq!(
        recycled.observe(&host, &ledger, &no_seats, 9_000),
        PaneReconciliation::default(),
        "a recycled term was accepted as the orphan's pane"
    );
    assert!(recycled.known.is_empty(), "the recycled term stayed cached");

    host.set_pane(TERM, false);
    let mut never_saw_term = PaneReconciler::default();
    for now_ms in [10_000, 11_000, 12_000, 13_000, 14_000] {
        assert!(
            never_saw_term
                .observe(&host, &ledger, &no_seats, now_ms)
                .missing
                .is_empty(),
            "an orphan with no machine address was guessed dead"
        );
    }
}

#[test]
fn the_real_host_checks_the_terminal_map_without_copying_the_grid() {
    let host = include_str!("../agent_tools_runtime.rs")
        .split_once("impl agent_teams::Host for TeamWindow {")
        .expect("the real team host")
        .1;
    let probe = host
        .split_once("fn pane_exists(&self, term: TermId) -> bool {")
        .expect("the pane existence probe")
        .1
        .split_once("fn capture(&self, term: TermId)")
        .expect("capture follows the probe")
        .0;
    assert!(
        probe.contains(".terminals()") && probe.contains(".contains_key(&term)"),
        "the production probe stopped asking the terminal map:\n{probe}"
    );
    assert!(
        !probe.contains("visible_text") && !probe.contains("capture("),
        "the cheap probe copied pane bytes:\n{probe}"
    );
    let quiet = host
        .split_once("fn quiet_since(")
        .expect("the production stall probe")
        .1
        .split_once("fn capture(&self, term: TermId)")
        .expect("capture follows the stall probe")
        .0;
    let shared_quiet = include_str!("../agent_tools_runtime.rs")
        .split_once("fn quiet_since_output(")
        .expect("the shared quiet rule")
        .1
        .split_once("impl agent_teams::Host for TeamWindow {")
        .expect("the host follows the shared rule")
        .0;
    assert!(
        quiet.contains("HookState::Working")
            && quiet.contains("last_output_epoch_ms()")
            && !quiet.contains("last_output_at()")
            && quiet.contains("quiet_since_output(last_output, worker_started_ms, now_ms)")
            && shared_quiet.contains("QUIET_GRACE_MS")
            && shared_quiet.contains("worker_started_ms"),
        "the stall probe lost hook, PTY, grace, or never-output fallback evidence:\n{quiet}"
    );
}

/// A host whose panes are all quiet, and whose screens say what a test
/// tells them to — the two host halves of a quota-wall witness.
struct AtTheWall {
    closed: Mutex<Vec<u32>>,
    busy: Mutex<bool>,
    onto: Mutex<u32>,
    checkout: &'static str,
    markers: Mutex<std::collections::HashMap<u32, zerocode_core::orchestration::QuotaWallMarker>>,
    /// Every gauge the beat asked the window to read again (t-6427).
    asked: Mutex<Vec<String>>,
    /// What each pane shows and records of a classifier decline (t-6747).
    declines: Mutex<std::collections::HashMap<u32, crate::quota_wall::DeclineReading>>,
    /// How long the pty has been silent while the hook still says `working`
    /// — a pause dialog's pane (t-6747); `None` is a pty that just wrote.
    pty_quiet_ms: Mutex<Option<i64>>,
    /// A pane's visible text, when a test hands the real screen reader a
    /// screen instead of a reading (t-7153): the decline reading is then
    /// `quota_wall::decline_reading_in`'s, off these words.
    screens: Mutex<std::collections::HashMap<u32, String>>,
    /// What the retirement fence reads, when it is to read something OTHER
    /// than the beat did (t-7153): the change between a plan and its last
    /// boundary, made deterministic.
    fence_reading: Mutex<Option<crate::quota_wall::DeclineReading>>,
}

impl AtTheWall {
    fn seating_onto(&self, term: u32) {
        *self.onto.lock().unwrap_or_else(|held| held.into_inner()) = term;
    }

    fn screen_says(&self, term: u32, source: &str, line: &str) {
        self.markers
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(
                term,
                zerocode_core::orchestration::QuotaWallMarker {
                    source: source.to_string(),
                    line: zerocode_core::orchestration::Text::from(line),
                },
            );
    }

    /// The pane's pty has been silent `quiet_ms` while its hook may still
    /// say `working`.
    fn pty_silent_for(&self, quiet_ms: i64) {
        *self.pty_quiet_ms.lock().unwrap() = Some(quiet_ms);
    }

    /// The pane stands at a classifier decline: what it shows and records.
    fn declined(&self, term: u32, reading: crate::quota_wall::DeclineReading) {
        self.declines
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(term, reading);
    }

    /// The pane's screen shows exactly these words (t-7153): the decline
    /// reading is the real reader's, off them, with no transcript.
    fn shows(&self, term: u32, screen: &str) {
        self.screens
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(term, screen.to_string());
    }

    /// What the retirement fence reads instead of the beat's reading
    /// (t-7153); `None` puts the fence back on the beat's reading.
    fn read_at_the_fence(&self, reading: Option<crate::quota_wall::DeclineReading>) {
        *self
            .fence_reading
            .lock()
            .unwrap_or_else(|held| held.into_inner()) = reading;
    }

    /// The beat's reading of `term`: the real reader over a screen a test
    /// handed it, else the reading a test handed it.
    fn decline_reading_of(&self, term: u32) -> Option<crate::quota_wall::DeclineReading> {
        if let Some(screen) = self
            .screens
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&term)
        {
            return Some(crate::quota_wall::decline_reading_in(
                "claude",
                Some(screen),
                None,
            ));
        }
        self.declines
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&term)
            .cloned()
    }

    /// The pane's own words moved past its wall.
    fn screen_clears(&self, term: u32) {
        self.markers
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .remove(&term);
    }

    fn asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    }
}

impl Host for AtTheWall {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        token: &str,
    ) -> Option<u32> {
        crate::agent_teams::place_seat_checkout(token, self.checkout.to_string());
        Some(*self.onto.lock().unwrap_or_else(|held| held.into_inner()))
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        true
    }
    fn pane_exists(&self, _term: u32) -> bool {
        true
    }
    fn quiet_since(&self, _term: u32, _worker_started_ms: i64, now_ms: i64) -> Option<i64> {
        if *self.busy.lock().unwrap() {
            return None;
        }
        Some(now_ms - zerocode_core::orchestration::QUIET_GRACE_MS)
    }
    fn decline_quiet_since(
        &self,
        term: u32,
        worker_started_ms: i64,
        now_ms: i64,
        hook_outlived_ms: i64,
    ) -> Option<i64> {
        self.quiet_since(term, worker_started_ms, now_ms)
            .or_else(|| {
                self.pty_quiet_ms
                    .lock()
                    .unwrap()
                    .filter(|quiet| *quiet >= hook_outlived_ms)
                    .map(|quiet| now_ms - quiet)
            })
    }
    fn quota_wall_marker(
        &self,
        term: u32,
        _agent: &str,
    ) -> Option<zerocode_core::orchestration::QuotaWallMarker> {
        self.markers
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&term)
            .cloned()
    }
    fn with_quota_wall_observation(
        &self,
        term: u32,
        _worker_started_ms: i64,
        _agent: &str,
        commit: &mut dyn FnMut(zerocode_core::orchestration::QuotaWallMarker),
    ) {
        let busy = self.busy.lock().unwrap();
        let markers = self.markers.lock().unwrap();
        if !*busy && let Some(marker) = markers.get(&term) {
            commit(marker.clone());
        }
    }
    fn classifier_decline_reading(
        &self,
        term: u32,
        _agent: &str,
    ) -> Option<crate::quota_wall::DeclineReading> {
        self.decline_reading_of(term)
    }
    fn with_classifier_decline_observation(
        &self,
        term: u32,
        _worker_started_ms: i64,
        _agent: &str,
        commit: &mut dyn FnMut(crate::quota_wall::DeclineReading, i64),
    ) {
        let busy = self.busy.lock().unwrap();
        let pty_quiet = self.pty_quiet_ms.lock().unwrap();
        // Quiet as the window's fence reads it: a hook at rest, or a pty
        // silent past the dialog's term whatever the hook says.
        let quiet_ms = if *busy {
            pty_quiet.filter(|quiet| {
                *quiet >= zerocode_core::orchestration::DECLINE_DIALOG_UNANSWERED_MS
            })
        } else {
            Some(zerocode_core::orchestration::QUIET_GRACE_MS)
        };
        // The fence reads what the beat read — unless a test moved the
        // world between the plan and this boundary (t-7153).
        let reading = self
            .fence_reading
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
            .or_else(|| self.decline_reading_of(term));
        if let (Some(quiet_ms), Some(reading)) = (quiet_ms, reading) {
            commit(reading, crate::now_epoch_ms() - quiet_ms);
        }
    }
    fn capture(&self, _term: u32) -> Option<String> {
        Some(String::new())
    }
    fn focus(&self, _term: u32) -> bool {
        true
    }
    fn close(&self, term: u32) {
        self.closed.lock().unwrap().push(term);
    }
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
    fn ask_usage(&self, gauge: &str) {
        self.asked
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push(gauge.to_string());
    }
}

/// A door that records what the walk says, commits as told, and speaks
/// the verbs through the real `run` — so the three argv steps are seen
/// AND land.
type WallChange<'a> = (usize, Box<dyn FnOnce() + 'a>);

struct Recording<'a> {
    wall_reads: std::cell::Cell<usize>,
    on_wall: std::cell::RefCell<Option<WallChange<'a>>>,
    host: &'a dyn Host,
    argv: Mutex<Vec<Vec<String>>>,
    commit: Mutex<Result<Option<String>, String>>,
    committed: Mutex<Vec<(std::path::PathBuf, String)>>,
    /// What `predecessor_tail` answers — nothing unless a test says so.
    tail: Option<(String, Vec<String>)>,
    /// How many verbs had run each time the walk asked for the tail.
    tail_asked: Mutex<Vec<usize>>,
}

impl<'a> Recording<'a> {
    fn change_on_wall(self, reading: usize, change: impl FnOnce() + 'a) -> Self {
        let _ = self.on_wall.replace(Some((reading, Box::new(change))));
        self
    }
    fn through(host: &'a dyn Host, commit: Result<Option<String>, String>) -> Self {
        Self {
            wall_reads: std::cell::Cell::new(0),
            on_wall: std::cell::RefCell::new(None),
            host,
            argv: Mutex::new(Vec::new()),
            commit: Mutex::new(commit),
            committed: Mutex::new(Vec::new()),
            tail: None,
            tail_asked: Mutex::new(Vec::new()),
        }
    }

    fn with_tail(self, transcript: &str, tail: Vec<String>) -> Self {
        Self {
            tail: Some((transcript.to_string(), tail)),
            ..self
        }
    }

    fn argv(&self) -> Vec<Vec<String>> {
        self.argv
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    }
}

impl HandoverDoor for Recording<'_> {
    fn current_wall(
        &self,
        plan: &zerocode_core::orchestration::HandoverPlan,
        now_ms: i64,
    ) -> Option<HandoverWall> {
        let reading = self.wall_reads.get() + 1;
        self.wall_reads.set(reading);
        let change = {
            let mut pending = self.on_wall.borrow_mut();
            if pending.as_ref().is_some_and(|(at, _)| *at == reading) {
                pending.take()
            } else {
                None
            }
        };
        if let Some((_, change)) = change {
            change();
        }
        current_handover_wall(self.host, plan, now_ms)
    }

    fn wip_commit(&self, checkout: &Path, message: &str) -> Result<Option<String>, String> {
        self.committed
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push((checkout.to_path_buf(), message.to_string()));
        self.commit
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    }

    fn predecessor_tail(
        &self,
        _plan: &zerocode_core::orchestration::HandoverPlan,
    ) -> Option<(String, Vec<String>)> {
        let verbs = self
            .argv
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .len();
        self.tail_asked
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push(verbs);
        self.tail.clone()
    }

    fn verb(
        &self,
        team: &str,
        pane: &str,
        capability: &str,
        argv: &[String],
        now_ms: i64,
        authority: (&zerocode_core::orchestration::HandoverPlan, &HandoverWall),
    ) -> zerocode_hookd::TeamAnswer {
        self.argv
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push(argv.to_vec());
        run_from_seat(
            self.host,
            Vec::new(),
            (team, pane, capability),
            argv,
            now_ms,
            Some(authority),
        )
    }
}

/// A private window with a codex worker at the wall in `checkout`, its
/// coordinator seated, and the run's handover policy as `policy` says —
/// everything a walk needs, before the walk.
struct Walled {
    window: PrivateWindow,
    _store: zerocode_orchestrator::workflow_store::WorkflowStore,
    _beat: std::sync::MutexGuard<'static, ()>,
    host: AtTheWall,
    team: String,
    task: String,
    worker: String,
    dispatch: String,
    began: i64,
}

impl Walled {
    fn stand(leader_term: u32, checkout: &'static str, policy: &str) -> Self {
        let began = clock();
        let gauges = |codex_used: u8, claude_used: u8| {
            vec![
                (
                    "codex",
                    usage_snapshot(
                        "codex",
                        Some((codex_used, Some(began + 42 * 60_000))),
                        Some((40, None)),
                        began - 3 * 60_000,
                    ),
                ),
                (
                    "claude",
                    usage_snapshot(
                        "claude",
                        Some((claude_used, Some(began + 9 * 60_000))),
                        Some((30, None)),
                        began - 60_000,
                    ),
                ),
            ]
        };
        let (window, store) = PrivateWindow::boot_with_usage(gauges(40, 50));
        let beat = one_beat_at_a_time();
        let team = format!("team-handover-{leader_term}");
        seat_a_team(&team, leader_term);
        let host = AtTheWall {
            closed: Mutex::new(Vec::new()),
            busy: Mutex::new(false),
            onto: Mutex::new(leader_term + 1),
            checkout,
            markers: Mutex::new(std::collections::HashMap::new()),
            asked: Mutex::new(Vec::new()),
            declines: Mutex::new(std::collections::HashMap::new()),
            pty_quiet_ms: Mutex::new(None),
            screens: Mutex::new(std::collections::HashMap::new()),
            fence_reading: Mutex::new(None),
        };
        let leader = zerocode_core::agent_teams::LEADER_PANE;
        let verb = |line: &str, at: i64| {
            let said = run(
                &host,
                Vec::new(),
                &team,
                leader,
                TEST_CAPABILITY,
                &words(line),
                at,
            );
            assert_eq!(said.exit_code, 0, "`{line}`: {}", said.stderr);
            serde_json::from_str::<serde_json::Value>(&said.stdout).expect("json")
        };
        verb("run-create --name handover", began);
        let task = verb("task-create --spec build-it", began + 1)["taskId"]
            .as_str()
            .expect("a task")
            .to_string();
        let codex = verb(
            &format!("worker-start --agent codex --task {task}"),
            began + 2,
        );
        let worker = codex["workerId"].as_str().expect("a worker").to_string();
        let dispatch = codex["dispatchId"]
            .as_str()
            .expect("a dispatch")
            .to_string();
        if !policy.is_empty() {
            verb(&format!("handover-policy {policy}"), began + 3);
        }
        host.seating_onto(leader_term + 2);
        host.screen_says(
            leader_term + 1,
            "screen",
            "You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage",
        );
        window.set_usage(gauges(98, 50));
        Self {
            window,
            _store: store,
            _beat: beat,
            host,
            team,
            task,
            worker,
            dispatch,
            began,
        }
    }

    fn verb(&self, line: &str, at: i64) -> zerocode_hookd::TeamAnswer {
        run(
            &self.host,
            Vec::new(),
            &self.team,
            zerocode_core::agent_teams::LEADER_PANE,
            TEST_CAPABILITY,
            &words(line),
            at,
        )
    }

    fn json(&self, line: &str, at: i64) -> serde_json::Value {
        let said = self.verb(line, at);
        assert_eq!(said.exit_code, 0, "`{line}`: {}", said.stderr);
        serde_json::from_str(&said.stdout).expect("json")
    }

    /// The `quota_walled` news, written by the stall sweep — the sweep
    /// alone, not a tick: a tick would also WALK the handover with the
    /// production door, and these scenarios walk it with their own.
    fn wall_it(&self) {
        notify_stalled_workers(&self.host, self.began + 10_000);
        assert_eq!(
            self.json("check --peek --types quota_walled", self.began + 10_001)["count"],
            1
        );
    }

    fn plan(&self) -> zerocode_core::orchestration::HandoverPlan {
        let held = super::runtime().expect("the runtime");
        let image = held.actor.view().expect("the image");
        let rows = super::cached_ledger(&held, &image).expect("the rows");
        rows.runs()
            .iter()
            .find_map(|run| zerocode_core::orchestration::next_handover(run, self.began + 20_000))
            .expect("a handover to walk")
    }

    fn receipts(&self, at: i64) -> Vec<serde_json::Value> {
        self.json("check --peek --types handover", at)["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| {
                serde_json::from_str(message["body"].as_str().expect("a body")).expect("json")
            })
            .collect()
    }

    fn worker_state(&self, worker: &str) -> String {
        let rows = the_rows();
        let held = rows
            .workers
            .iter()
            .find(|one| one.id == worker)
            .expect("the worker");
        format!("{:?}", held.state)
    }
}

/// The whole road on the beat itself, with the production door: a codex
/// worker walled in a REAL repository with uncommitted work, a policy
/// with `--wip-commit`, one tick — and the tree is committed under the
/// walk's message, the worker stopped, a claude replacement seated in
/// the same checkout on a dispatch linked to the ended one, and the
/// receipt delivered `done` with three steps.
#[test]
fn the_beat_walks_a_handover_through_git_and_the_one_door_and_leaves_a_receipt() {
    const LEADER_TERM: u32 = 85_000;
    let repo = tempfile::tempdir().expect("a checkout");
    let git = |args: &[&str]| {
        let output = crate::proc::quiet_command("git")
            .arg("-C")
            .arg(repo.path())
            .args(args)
            .output()
            .expect("git");
        assert!(output.status.success(), "git {args:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "ZeroCode Test"]);
    git(&["config", "user.email", "test@zerocode"]);
    git(&["commit", "--allow-empty", "-q", "-m", "first"]);
    std::fs::write(repo.path().join("half.txt"), "half done\n").expect("write");
    let checkout: &'static str =
        Box::leak(repo.path().to_string_lossy().into_owned().into_boxed_str());
    let stood = Walled::stand(
        LEADER_TERM,
        checkout,
        "--on-quota-wall claude:fable-5-1 --wip-commit",
    );
    // One tick: the sweep writes the wall, the walk takes it.
    tick(&stood.host, &[], stood.began + 10_000);
    let receipts = stood.receipts(stood.began + 10_001);
    assert_eq!(receipts.len(), 1, "{receipts:?}");
    let receipt = &receipts[0];
    assert_eq!(receipt["status"], "done", "{receipt}");
    assert_eq!(receipt["from"]["workerId"], stood.worker);
    assert_eq!(receipt["from"]["dispatchId"], stood.dispatch);
    assert_eq!(receipt["to"]["agent"], "claude");
    let steps = receipt["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 3, "{steps:?}");
    assert!(steps.iter().all(|step| step["ok"] == true), "{steps:?}");
    let sha = git(&["rev-parse", "--short", "HEAD"]).trim().to_string();
    assert!(
        steps[0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains(&sha)),
        "{}",
        steps[0]
    );
    let subject = git(&["log", "-1", "--format=%s"]);
    assert!(
        subject.starts_with(&format!(
            "wip(handover): {} stopped at codex wall, resets ",
            stood.worker
        )),
        "{subject}"
    );
    assert!(git(&["status", "--porcelain"]).trim().is_empty());
    // The ledger: old attempt ended (a stop leaves `Released`), the
    // replacement on a linked attempt in the same checkout.
    assert_eq!(stood.worker_state(&stood.worker), "Released");
    let replacement = receipt["to"]["workerId"].as_str().expect("a replacement");
    assert_eq!(stood.worker_state(replacement), "Active");
    let rows = the_rows();
    let seated = rows
        .workers
        .iter()
        .find(|one| one.id == replacement)
        .expect("the replacement");
    assert_eq!(seated.agent, "claude");
    assert_eq!(seated.model.as_deref(), Some("fable-5-1"));
    assert_eq!(seated.checkout.as_deref(), Some(checkout));
    let linked = rows
        .dispatches
        .iter()
        .find(|one| Some(one.id.as_str()) == seated.dispatch.as_deref())
        .expect("the replacement's attempt");
    assert_eq!(linked.retry_of.as_deref(), Some(stood.dispatch.as_str()));
    assert_eq!(linked.task, stood.task);
    // Once: the next tick has nothing to walk.
    tick(&stood.host, &[], stood.began + 11_000);
    assert_eq!(stood.receipts(stood.began + 11_001).len(), 1);
}

impl Walled {
    /// A claude worker stopped at a safety classifier's decline (t-6747):
    /// its screen shows Claude Code's own sentence and its transcript's last
    /// record is the decline, in `category`.
    fn stand_declined(
        leader_term: u32,
        checkout: &'static str,
        category: &str,
        policy: &str,
    ) -> Self {
        let said = "API Error: Fable 5.1's safeguards flagged this message \
                    (https://www.anthropic.com/legal/aup).";
        Self::stand_declined_with(
            leader_term,
            checkout,
            policy,
            crate::quota_wall::DeclineReading {
                screen: Some(zerocode_core::orchestration::DeclineScreen {
                    line: zerocode_core::orchestration::Text::from(said),
                    dialog: false,
                    category: None,
                }),
                record: Some(zerocode_core::orchestration::ClassifierDeclineMarker {
                    source: "transcript".to_string(),
                    line: zerocode_core::orchestration::Text::from(said),
                    category: Some(category.to_string()),
                    key: "decline-record-1".to_string(),
                }),
                fallbacks: Vec::new(),
            },
        )
    }

    /// The worker stands at Claude Code's pause dialog (t-6747): the dialog
    /// on its screen, no record — the dialog writes none until a key answers
    /// it — and a hook that still says `working`, because the dialog ended
    /// no turn. How long its pty has been silent is the test's to say.
    fn stand_paused(leader_term: u32, checkout: &'static str, policy: &str) -> Self {
        let stood = Self::stand_declined_with(
            leader_term,
            checkout,
            policy,
            crate::quota_wall::DeclineReading {
                screen: Some(zerocode_core::orchestration::DeclineScreen {
                    line: zerocode_core::orchestration::Text::from(
                        "Fable 5.1's safeguards flagged this message.",
                    ),
                    dialog: true,
                    category: Some("cyber".to_string()),
                }),
                record: None,
                fallbacks: Vec::new(),
            },
        );
        *stood.host.busy.lock().unwrap() = true;
        stood
    }

    /// A claude worker whose pane shows exactly `screen` under a hook that
    /// still says `working` (t-7153): what the pane is at is the real screen
    /// reader's to say, with no transcript record behind it.
    fn stand_showing(leader_term: u32, checkout: &'static str, policy: &str, screen: &str) -> Self {
        let stood = Self::stand_declined_with(
            leader_term,
            checkout,
            policy,
            crate::quota_wall::DeclineReading::default(),
        );
        stood.host.shows(leader_term + 1, screen);
        *stood.host.busy.lock().unwrap() = true;
        stood
    }

    fn stand_declined_with(
        leader_term: u32,
        checkout: &'static str,
        policy: &str,
        reading: crate::quota_wall::DeclineReading,
    ) -> Self {
        let began = clock();
        let (window, store) = PrivateWindow::boot();
        let beat = one_beat_at_a_time();
        let team = format!("team-decline-{leader_term}");
        seat_a_team(&team, leader_term);
        let host = AtTheWall {
            closed: Mutex::new(Vec::new()),
            busy: Mutex::new(false),
            onto: Mutex::new(leader_term + 1),
            checkout,
            markers: Mutex::new(std::collections::HashMap::new()),
            asked: Mutex::new(Vec::new()),
            declines: Mutex::new(std::collections::HashMap::new()),
            pty_quiet_ms: Mutex::new(None),
            screens: Mutex::new(std::collections::HashMap::new()),
            fence_reading: Mutex::new(None),
        };
        let verb = |line: &str, at: i64| {
            let said = run(
                &host,
                Vec::new(),
                &team,
                zerocode_core::agent_teams::LEADER_PANE,
                TEST_CAPABILITY,
                &words(line),
                at,
            );
            assert_eq!(said.exit_code, 0, "`{line}`: {}", said.stderr);
            serde_json::from_str::<serde_json::Value>(&said.stdout).expect("json")
        };
        verb("run-create --name declined", began);
        let task = verb("task-create --spec build-it", began + 1)["taskId"]
            .as_str()
            .expect("a task")
            .to_string();
        let claude = verb(
            &format!("worker-start --agent claude --model fable --effort max --task {task}"),
            began + 2,
        );
        let worker = claude["workerId"].as_str().expect("a worker").to_string();
        let dispatch = claude["dispatchId"]
            .as_str()
            .expect("a dispatch")
            .to_string();
        if !policy.is_empty() {
            verb(&format!("handover-policy {policy}"), began + 3);
        }
        host.seating_onto(leader_term + 2);
        host.declined(leader_term + 1, reading);
        Self {
            window,
            _store: store,
            _beat: beat,
            host,
            team,
            task,
            worker,
            dispatch,
            began,
        }
    }
}

/// A worker whose provider's classifier declined it in a routed category,
/// and whose CLI could not continue, is handed on in one tick (t-6747): the
/// two witnesses become one notice, and under the run's declared
/// `--on-classifier-decline` the task goes to a fresh worker in the SAME
/// checkout on a dispatch linked to the ended one — the declared model, the
/// effort the declaration left out the worker's own — and the receipt names
/// the category and the model binding the replacement leaves.
#[test]
fn a_declined_worker_is_resummoned_on_the_declared_rung_with_its_checkout() {
    const LEADER_TERM: u32 = 85_300;
    let stood = Walled::stand_declined(
        LEADER_TERM,
        "/tmp",
        "cyber",
        "--on-classifier-decline claude:claude-opus-4-8",
    );
    tick(&stood.host, &[], stood.began + 10_000);
    let news = stood.json(
        "check --peek --types classifier_declined",
        stood.began + 10_001,
    );
    assert_eq!(news["count"], 1, "{news}");
    let receipts = stood.receipts(stood.began + 10_001);
    assert_eq!(receipts.len(), 1, "{receipts:?}");
    let receipt = &receipts[0];
    assert_eq!(receipt["status"], "done", "{receipt}");
    assert_eq!(receipt["reason"], "classifier-decline");
    assert_eq!(receipt["category"], "cyber");
    assert_eq!(receipt["rung"], "handover");
    assert_eq!(receipt["to"]["model"], "claude-opus-4-8");
    assert_eq!(receipt["to"]["effort"], "max");
    assert_eq!(receipt["modelDeviation"]["from"]["model"], "fable");
    assert_eq!(receipt["modelDeviation"]["to"]["model"], "claude-opus-4-8");
    assert_eq!(stood.worker_state(&stood.worker), "Released");
    let replacement = receipt["to"]["workerId"].as_str().expect("a replacement");
    let rows = the_rows();
    let seated = rows
        .workers
        .iter()
        .find(|one| one.id == replacement)
        .expect("the replacement");
    assert_eq!(seated.agent, "claude");
    assert_eq!(seated.model.as_deref(), Some("claude-opus-4-8"));
    assert_eq!(seated.effort.as_deref(), Some("max"));
    assert_eq!(seated.checkout.as_deref(), Some("/tmp"));
    let linked = rows
        .dispatches
        .iter()
        .find(|one| Some(one.id.as_str()) == seated.dispatch.as_deref())
        .expect("the replacement's attempt");
    assert_eq!(linked.retry_of.as_deref(), Some(stood.dispatch.as_str()));
    assert_eq!(linked.task, stood.task);
    // Once: the next tick has nothing to walk.
    tick(&stood.host, &[], stood.began + 11_000);
    assert_eq!(stood.receipts(stood.began + 11_001).len(), 1);
}

/// A pause dialog stands behind a hook that still says `working` (t-6747):
/// Claude Code ends no turn while its dialog waits, and says so only through
/// a `Notification` hook this window does not install, so the stall sweep
/// never reads the pane. Once its pty has been silent past every dialog a
/// person answered here, the dialog on screen and that silence are told —
/// once, as DIAGNOSTIC news (t-7153, P1-3): a screen is not a witness that
/// ends a worker, so even under the run's declared order nothing is walked;
/// the notice says so and names the hand road. A shorter silence is
/// nothing: a person may be about to answer.
#[test]
fn a_pause_dialog_behind_a_working_hook_is_diagnostic_news_once_and_ends_no_worker() {
    const LEADER_TERM: u32 = 85_500;
    const MINUTE: i64 = 60_000;
    let stood = Walled::stand_paused(
        LEADER_TERM,
        "/tmp",
        "--on-classifier-decline claude:claude-opus-4-8",
    );
    let told = |at: i64| stood.json("check --peek --types classifier_declined", at);

    stood.host.pty_silent_for(9 * MINUTE);
    tick(&stood.host, &[], stood.began + 10_000);
    assert_eq!(told(stood.began + 10_001)["count"], 0);
    assert!(stood.receipts(stood.began + 10_001).is_empty());

    stood.host.pty_silent_for(11 * MINUTE);
    tick(&stood.host, &[], stood.began + 20_000);
    let news = told(stood.began + 20_001);
    assert_eq!(news["count"], 1, "{news}");
    let body: serde_json::Value =
        serde_json::from_str(news["messages"][0]["body"].as_str().expect("a body")).expect("json");
    assert_eq!(
        body["record"]["source"],
        zerocode_core::orchestration::DECLINE_DIALOG_SOURCE,
        "{body}"
    );
    assert_eq!(body["category"], "cyber", "{body}");
    assert_eq!(body["screenOnly"], true, "{body}");
    assert_eq!(
        body["rung"], "notify",
        "a screen-only decline armed a stop: {body}"
    );
    assert!(
        body["next"]
            .as_str()
            .is_some_and(|next| next.contains("worker-read")),
        "{body}"
    );
    assert!(
        stood.receipts(stood.began + 20_001).is_empty(),
        "a screen-only decline was walked to a handover"
    );
    assert_eq!(stood.worker_state(&stood.worker), "Active");
    assert!(stood.host.closed.lock().unwrap().is_empty());

    // Told once: the next beat has nothing to tell, and still nothing to walk.
    tick(&stood.host, &[], stood.began + 30_000);
    assert_eq!(told(stood.began + 30_001)["count"], 1);
    assert!(stood.receipts(stood.began + 30_001).is_empty());
}

/// The dialog's words, quoted — a tool result a worker read, under the
/// turn's own spinner — are no dialog (t-7153, P1-3): the real screen
/// reader finds no CLI dialog in them, so a pane showing them under a
/// `working` hook, silent past the dialog's term, is told about nothing and
/// nothing is walked. The same pane showing the CLI's own dialog is
/// diagnostic news, and still walks nowhere.
#[test]
fn a_screen_quoting_the_dialogs_words_is_nothing_and_the_cli_dialog_is_diagnostic_news() {
    const MINUTE: i64 = 60_000;
    const QUOTED: &str = "\
⏺ Read(notes/declines.md)
  ⎿  Read 5 lines
     Session paused
     Fable 5.1's safeguards flagged this message. Our intentionally broad safeguards…
     Details: `[cyber]`
     1. Switch to Opus 4.8
     2. Edit prompt and retry

✻ Thinking… (esc to interrupt)
";
    const DIALOG: &str = "\
 Session paused
 Fable 5.1's safeguards flagged this message. Our intentionally broad safeguards allow us to deliver more capabilities faster.
   Details: `[cyber]`
 ❯ 1. Switch to Opus 4.8
   2. Edit prompt and retry
";
    for (leader_term, screen, told_count) in [(85_600, QUOTED, 0), (85_700, DIALOG, 1)] {
        let stood = Walled::stand_showing(
            leader_term,
            "/tmp",
            "--on-classifier-decline claude:claude-opus-4-8",
            screen,
        );
        stood.host.pty_silent_for(11 * MINUTE);
        tick(&stood.host, &[], stood.began + 20_000);
        let news = stood.json(
            "check --peek --types classifier_declined",
            stood.began + 20_001,
        );
        assert_eq!(news["count"], told_count, "{screen}\n{news}");
        if told_count == 1 {
            let body: serde_json::Value =
                serde_json::from_str(news["messages"][0]["body"].as_str().expect("a body"))
                    .expect("json");
            assert_eq!(body["rung"], "notify", "{body}");
            assert_eq!(body["screenOnly"], true, "{body}");
        }
        assert!(
            stood.receipts(stood.began + 20_001).is_empty(),
            "{screen}\nwas walked to a handover"
        );
        assert_eq!(stood.worker_state(&stood.worker), "Active");
    }
}

/// A handover ends a worker only on the decline it was planned for (t-7153,
/// P1-4): the plan carries the record that witnessed it, and the last
/// boundary — inside the actor's fence, where the stop is committed —
/// re-reads the pane and commits only when the SAME record, in the SAME
/// routed category, still stands. A category that reads as none, another
/// request's record, or a category the provider routes nowhere at that
/// boundary settles nothing: the attempt stays open, the pane stays, the
/// receipt says the witness changed, and no replacement is summoned. When
/// the boundary reads the same decline again, the walk lands.
#[test]
fn a_decline_that_reads_differently_at_the_last_boundary_ends_no_worker() {
    let changed = [
        (85_800, a_printed_decline(None, "decline-record-1")),
        (85_900, a_printed_decline(Some("cyber"), "decline-record-2")),
        (
            86_000,
            a_printed_decline(Some("reasoning_extraction"), "decline-record-1"),
        ),
    ];
    for (leader_term, at_the_fence) in changed {
        let stood = Walled::stand_declined(
            leader_term,
            "/tmp",
            "cyber",
            "--on-classifier-decline claude:claude-opus-4-8",
        );
        stood.host.read_at_the_fence(Some(at_the_fence.clone()));
        tick(&stood.host, &[], stood.began + 10_000);
        let receipts = stood.receipts(stood.began + 10_001);
        assert!(
            receipts.iter().all(|receipt| receipt["status"] != "done"),
            "{at_the_fence:?}\nwas settled: {receipts:?}"
        );
        assert_eq!(
            stood.worker_state(&stood.worker),
            "Active",
            "{at_the_fence:?}\nended the worker"
        );
        assert!(stood.host.closed.lock().unwrap().is_empty());
        let rows = the_rows();
        assert_eq!(
            rows.dispatches
                .iter()
                .filter(|one| one.task == stood.task)
                .count(),
            1,
            "a replacement attempt was opened: {:?}",
            rows.dispatches
        );
        // The same decline again at the boundary: the walk lands.
        stood.host.read_at_the_fence(None);
        tick(&stood.host, &[], stood.began + 20_000);
        let receipts = stood.receipts(stood.began + 20_001);
        assert!(
            receipts.iter().any(|receipt| receipt["status"] == "done"),
            "{receipts:?}"
        );
        assert_eq!(stood.worker_state(&stood.worker), "Released");
    }
}

/// A pane at a printed decline: the CLI's sentence on screen, and its
/// transcript's last record the decline, in `category`, keyed `key`.
fn a_printed_decline(category: Option<&str>, key: &str) -> crate::quota_wall::DeclineReading {
    const SAID: &str = "API Error: Fable 5.1's safeguards flagged this message.";
    crate::quota_wall::DeclineReading {
        screen: Some(zerocode_core::orchestration::DeclineScreen {
            line: zerocode_core::orchestration::Text::from(SAID),
            dialog: false,
            category: None,
        }),
        record: Some(zerocode_core::orchestration::ClassifierDeclineMarker {
            source: "transcript".to_string(),
            line: zerocode_core::orchestration::Text::from(SAID),
            category: category.map(str::to_string),
            key: key.to_string(),
        }),
        fallbacks: Vec::new(),
    }
}

/// A dialog's diagnostic notice closes nothing (t-7153, R4): once the same
/// attempt's pane stands at a RECORD — the CLI printed the error, its
/// transcript's last record is the decline in a routed category — that
/// record is news of its own, told on the handover rung under the declared
/// order and walked exactly once, while the hook STILL says `working` and
/// the pty is still silent: the pane never becomes the quiet sweep's.
/// Before this one notice, whatever witnessed it, closed the paused
/// sweep's reading of the attempt, and a record behind a hook that stayed
/// `working` was never told nor planned by either sweep.
/// The hook's two roads both lead there: a hook that comes to rest hands
/// the pane to the quiet sweep, and one that stays `working` leaves it to
/// the paused sweep, which reads it again.
#[test]
fn a_dialogs_diagnostic_notice_does_not_hide_the_record_that_follows_it() {
    const MINUTE: i64 = 60_000;
    for (leader_term, hook_rests) in [(86_100, false), (86_150, true)] {
        let stood = Walled::stand_paused(
            leader_term,
            "/tmp",
            "--on-classifier-decline claude:claude-opus-4-8",
        );
        let told = |at: i64| stood.json("check --peek --types classifier_declined", at);
        stood.host.pty_silent_for(11 * MINUTE);
        tick(&stood.host, &[], stood.began + 20_000);
        assert_eq!(told(stood.began + 20_001)["count"], 1);
        assert!(stood.receipts(stood.began + 20_001).is_empty());

        // The record, on the same open attempt.
        stood.host.declined(
            leader_term + 1,
            a_printed_decline(Some("cyber"), "decline-record-1"),
        );
        *stood.host.busy.lock().unwrap() = !hook_rests;
        tick(&stood.host, &[], stood.began + 30_000);
        let news = told(stood.began + 30_001);
        assert_eq!(
            news["count"], 2,
            "hook at rest {hook_rests}: the record after the dialog's notice was not told: {news}"
        );
        let receipts = stood.receipts(stood.began + 30_001);
        assert_eq!(receipts.len(), 1, "hook at rest {hook_rests}: {receipts:?}");
        assert_eq!(receipts[0]["status"], "done", "{}", receipts[0]);
        assert_eq!(receipts[0]["reason"], "classifier-decline");
        assert_eq!(
            receipts[0]["recordKey"], "decline-record-1",
            "{}",
            receipts[0]
        );
        assert_eq!(stood.worker_state(&stood.worker), "Released");

        // Once: the next beat tells nothing more and walks nothing more.
        tick(&stood.host, &[], stood.began + 40_000);
        assert_eq!(told(stood.began + 40_001)["count"], 2);
        assert_eq!(stood.receipts(stood.began + 40_001).len(), 1);
    }
}

/// Behind a hook that never stops saying `working`, every record is news
/// of its own (t-7153, R4): the dialog's diagnostic notice, then a record
/// once a key answered it, then another request's record — each told once
/// on its own key, none twice, with no order declared and the pane's pty
/// silent throughout; and under an order declared after all three, the
/// walk lands on the record the pane stands at now, exactly once. The
/// hook stays `working` for the whole road: nothing here is the quiet
/// sweep's.
#[test]
fn records_behind_a_hook_that_stays_working_are_told_and_planned_each_on_its_own_key() {
    const LEADER_TERM: u32 = 86_300;
    const MINUTE: i64 = 60_000;
    let stood = Walled::stand_paused(LEADER_TERM, "/tmp", "");
    let told = |at: i64| stood.json("check --peek --types classifier_declined", at);
    let keys = |news: &serde_json::Value| -> Vec<String> {
        news["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| {
                serde_json::from_str::<serde_json::Value>(message["body"].as_str().expect("a body"))
                    .expect("json")["record"]["key"]
                    .as_str()
                    .expect("a key")
                    .to_string()
            })
            .collect()
    };
    stood.host.pty_silent_for(11 * MINUTE);
    tick(&stood.host, &[], stood.began + 20_000);
    assert_eq!(told(stood.began + 20_001)["count"], 1);

    stood.host.declined(
        LEADER_TERM + 1,
        a_printed_decline(Some("cyber"), "decline-record-1"),
    );
    assert!(*stood.host.busy.lock().unwrap());
    tick(&stood.host, &[], stood.began + 30_000);
    let news = told(stood.began + 30_001);
    assert_eq!(
        news["count"], 2,
        "the record behind the working hook was not told: {news}"
    );
    tick(&stood.host, &[], stood.began + 31_000);
    assert_eq!(
        told(stood.began + 31_001)["count"],
        2,
        "the same record was told twice"
    );

    stood.host.declined(
        LEADER_TERM + 1,
        a_printed_decline(Some("cyber"), "decline-record-2"),
    );
    tick(&stood.host, &[], stood.began + 40_000);
    let news = told(stood.began + 40_001);
    assert_eq!(news["count"], 3, "{news}");
    assert_eq!(
        keys(&news),
        [
            zerocode_core::orchestration::decline_dialog_key(&stood.dispatch),
            "decline-record-1".to_string(),
            "decline-record-2".to_string()
        ]
    );
    assert!(
        stood.receipts(stood.began + 40_001).is_empty(),
        "no order declared"
    );

    stood.json(
        "handover-policy --on-classifier-decline claude:claude-opus-4-8",
        stood.began + 41_000,
    );
    tick(&stood.host, &[], stood.began + 50_000);
    let receipts = stood.receipts(stood.began + 50_001);
    let done: Vec<&serde_json::Value> = receipts
        .iter()
        .filter(|receipt| receipt["status"] == "done")
        .collect();
    assert_eq!(done.len(), 1, "{receipts:?}");
    assert_eq!(done[0]["recordKey"], "decline-record-2", "{}", done[0]);
    assert_eq!(stood.worker_state(&stood.worker), "Released");
    tick(&stood.host, &[], stood.began + 60_000);
    assert_eq!(
        stood
            .receipts(stood.began + 60_001)
            .iter()
            .filter(|receipt| receipt["status"] == "done")
            .count(),
        1,
        "walked twice"
    );
}

/// Behind a hook that never stops saying `working`, a record ends a worker
/// only as it would behind a hook at rest (t-7153, R4): under the exact
/// order the run declares, on the record the pane stands at when the stop
/// is committed, and never past the handover's own refusals. A plan whose
/// record the pane no longer stands at by its last boundary ends nobody,
/// and the record it stands at then is told and walked once; an order
/// withdrawn after the dialog's notice walks nothing, its record told on
/// the notify rung; a pane the person took is told nothing more and walked
/// nowhere; and past the table's ceiling a record is news only. The hook
/// stays `working` and the pty silent on every road: nothing here is the
/// quiet sweep's.
#[test]
fn behind_a_working_hook_the_handovers_own_refusals_hold() {
    const MINUTE: i64 = 60_000;
    const ORDER: &str = "--on-classifier-decline claude:claude-opus-4-8";
    let told = |stood: &Walled, at: i64| stood.json("check --peek --types classifier_declined", at);
    let done = |stood: &Walled, at: i64| -> Vec<serde_json::Value> {
        stood
            .receipts(at)
            .into_iter()
            .filter(|receipt| receipt["status"] == "done")
            .collect()
    };
    let paused = |leader_term: u32| {
        let stood = Walled::stand_paused(leader_term, "/tmp", ORDER);
        stood.host.pty_silent_for(11 * MINUTE);
        tick(&stood.host, &[], stood.began + 20_000);
        assert_eq!(told(&stood, stood.began + 20_001)["count"], 1);
        assert!(stood.receipts(stood.began + 20_001).is_empty());
        stood
    };

    // An old plan: by the stop's fence the pane stands at another record.
    let stood = paused(86_400);
    stood
        .host
        .declined(86_401, a_printed_decline(Some("cyber"), "decline-record-1"));
    stood
        .host
        .read_at_the_fence(Some(a_printed_decline(Some("cyber"), "decline-record-2")));
    tick(&stood.host, &[], stood.began + 30_000);
    assert!(*stood.host.busy.lock().unwrap());
    assert_eq!(told(&stood, stood.began + 30_001)["count"], 2);
    assert!(
        done(&stood, stood.began + 30_001).is_empty(),
        "an old plan ended the worker"
    );
    assert_eq!(stood.worker_state(&stood.worker), "Active");
    assert!(stood.host.closed.lock().unwrap().is_empty());
    assert_eq!(
        the_rows()
            .dispatches
            .iter()
            .filter(|one| one.task == stood.task)
            .count(),
        1,
        "an old plan opened a replacement attempt"
    );
    stood
        .host
        .declined(86_401, a_printed_decline(Some("cyber"), "decline-record-2"));
    stood.host.read_at_the_fence(None);
    tick(&stood.host, &[], stood.began + 40_000);
    assert_eq!(told(&stood, stood.began + 40_001)["count"], 3);
    let walked = done(&stood, stood.began + 40_001);
    assert_eq!(walked.len(), 1, "{walked:?}");
    assert_eq!(walked[0]["recordKey"], "decline-record-2", "{}", walked[0]);
    assert_eq!(stood.worker_state(&stood.worker), "Released");
    drop(stood);

    // An order withdrawn after the dialog's notice.
    let stood = paused(86_500);
    stood.json("handover-policy --off", stood.began + 21_000);
    stood
        .host
        .declined(86_501, a_printed_decline(Some("cyber"), "decline-record-1"));
    tick(&stood.host, &[], stood.began + 30_000);
    let news = told(&stood, stood.began + 30_001);
    assert_eq!(news["count"], 2, "{news}");
    let body: serde_json::Value =
        serde_json::from_str(news["messages"][1]["body"].as_str().expect("a body")).expect("json");
    assert_eq!(body["record"]["key"], "decline-record-1", "{body}");
    assert_eq!(body["rung"], "notify", "{body}");
    assert!(
        stood.receipts(stood.began + 30_001).is_empty(),
        "a withdrawn order walked"
    );
    assert_eq!(stood.worker_state(&stood.worker), "Active");
    drop(stood);

    // A pane the person took after the dialog's notice.
    let stood = paused(86_600);
    super::pane_taken_over(86_601, stood.began + 21_000);
    stood
        .host
        .declined(86_601, a_printed_decline(Some("cyber"), "decline-record-1"));
    tick(&stood.host, &[], stood.began + 30_000);
    assert_eq!(
        told(&stood, stood.began + 30_001)["count"],
        1,
        "a pane the person took was told about"
    );
    assert!(
        stood.receipts(stood.began + 30_001).is_empty(),
        "a pane the person took was walked"
    );
    assert_eq!(stood.worker_state(&stood.worker), "Active");
    drop(stood);

    // The table's ceiling: the task walked as often as it may, and the
    // record its last worker stands at is news only.
    let stood = paused(86_700);
    let mut worker = stood.worker.clone();
    for walk in 1..=zerocode_core::orchestration::QUOTA_POLICY.handover_max {
        let term = 86_700 + u32::try_from(walk).expect("small");
        let at = stood.began + 20_000 + 10_000 * i64::try_from(walk).expect("small");
        stood.host.seating_onto(term + 1);
        stood.host.declined(
            term,
            a_printed_decline(Some("cyber"), &format!("decline-record-{walk}")),
        );
        tick(&stood.host, &[], at);
        let walked = done(&stood, at + 1);
        assert_eq!(walked.len(), walk, "{walked:?}");
        assert_eq!(stood.worker_state(&worker), "Released");
        worker = walked[walk - 1]["to"]["workerId"]
            .as_str()
            .expect("a replacement")
            .to_string();
    }
    let last = 86_701
        + u32::try_from(zerocode_core::orchestration::QUOTA_POLICY.handover_max).expect("small");
    stood.host.declined(
        last,
        a_printed_decline(Some("cyber"), "decline-record-last"),
    );
    tick(&stood.host, &[], stood.began + 90_000);
    let news = told(&stood, stood.began + 90_001);
    assert_eq!(
        news["count"],
        zerocode_core::orchestration::QUOTA_POLICY.handover_max + 2,
        "the record past the ceiling was not even news: {news}"
    );
    assert_eq!(
        done(&stood, stood.began + 90_001).len(),
        zerocode_core::orchestration::QUOTA_POLICY.handover_max,
        "the ceiling was walked past"
    );
    assert_eq!(stood.worker_state(&worker), "Active");
}

/// A later decline of the same attempt is told on its own record and
/// planned on it (t-7153, R4): the same record on the next beat is told
/// once; another record — a later request declined — is news again; and
/// under an order declared after both, the walk lands on the record the
/// pane stands at now, exactly once, never on the one it left behind.
#[test]
fn a_later_decline_of_the_same_attempt_is_told_and_planned_on_its_own_record() {
    const LEADER_TERM: u32 = 86_200;
    let stood = Walled::stand_declined(LEADER_TERM, "/tmp", "cyber", "");
    let told = |at: i64| stood.json("check --peek --types classifier_declined", at);
    tick(&stood.host, &[], stood.began + 10_000);
    assert_eq!(told(stood.began + 10_001)["count"], 1);
    tick(&stood.host, &[], stood.began + 11_000);
    assert_eq!(
        told(stood.began + 11_001)["count"],
        1,
        "the same record was told twice"
    );

    stood.host.declined(
        LEADER_TERM + 1,
        a_printed_decline(Some("cyber"), "decline-record-2"),
    );
    tick(&stood.host, &[], stood.began + 20_000);
    let news = told(stood.began + 20_001);
    assert_eq!(
        news["count"], 2,
        "a later decline on another record was not told: {news}"
    );
    let keys: Vec<String> = news["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| {
            serde_json::from_str::<serde_json::Value>(message["body"].as_str().expect("a body"))
                .expect("json")["record"]["key"]
                .as_str()
                .expect("a key")
                .to_string()
        })
        .collect();
    assert_eq!(keys, ["decline-record-1", "decline-record-2"]);
    tick(&stood.host, &[], stood.began + 21_000);
    assert_eq!(
        told(stood.began + 21_001)["count"],
        2,
        "the second record was told twice"
    );
    assert!(
        stood.receipts(stood.began + 21_001).is_empty(),
        "no order declared"
    );

    stood.json(
        "handover-policy --on-classifier-decline claude:claude-opus-4-8",
        stood.began + 22_000,
    );
    tick(&stood.host, &[], stood.began + 30_000);
    let receipts = stood.receipts(stood.began + 30_001);
    let done: Vec<&serde_json::Value> = receipts
        .iter()
        .filter(|receipt| receipt["status"] == "done")
        .collect();
    assert_eq!(done.len(), 1, "{receipts:?}");
    assert_eq!(done[0]["recordKey"], "decline-record-2", "{}", done[0]);
    assert_eq!(stood.worker_state(&stood.worker), "Released");
    tick(&stood.host, &[], stood.began + 40_000);
    assert_eq!(
        stood
            .receipts(stood.began + 40_001)
            .iter()
            .filter(|receipt| receipt["status"] == "done")
            .count(),
        1,
        "walked twice"
    );
}

/// A category the provider routes nowhere is news and walks nowhere, even
/// under a declared order: the provider's answer stands (t-6747).
#[test]
fn an_unrouted_decline_is_told_and_walked_nowhere() {
    const LEADER_TERM: u32 = 85_400;
    let stood = Walled::stand_declined(
        LEADER_TERM,
        "/tmp",
        "reasoning_extraction",
        "--on-classifier-decline claude:claude-opus-4-8",
    );
    tick(&stood.host, &[], stood.began + 10_000);
    let news = stood.json(
        "check --peek --types classifier_declined",
        stood.began + 10_001,
    );
    assert_eq!(news["count"], 1, "{news}");
    let body: serde_json::Value =
        serde_json::from_str(news["messages"][0]["body"].as_str().expect("a body")).expect("json");
    assert_eq!(body["routed"], false, "{body}");
    assert_eq!(body["rung"], "notify", "{body}");
    assert!(stood.receipts(stood.began + 10_001).is_empty());
    assert_eq!(stood.worker_state(&stood.worker), "Active");
}

/// The category the beat and the fence read is joined on BOTH of the
/// error record's ids (t-7153, R1), through the real transcript reader: a
/// category record the error's request names but whose uuid is not the
/// parent the error names is a contradiction — no category, so the decline
/// is unrouted news and nothing walks, where before the request's word was
/// borrowed and the worker handed over on it; the same contradiction read
/// at the last boundary, under the same record key, settles nothing; and
/// the record both ids name is this decline's, walked once.
#[test]
fn a_declines_category_is_joined_on_both_ids_at_the_beat_and_at_the_fence() {
    const SCREEN: &str = "\
⎿  API Error: Fable 5.1's safeguards flagged this message (https://www.anthropic.com/legal/aup).
❯ ";
    let system = |uuid: &str, request: &str| {
        format!(
            r#"{{"parentUuid":"p","isSidechain":false,"type":"system","subtype":"model_refusal_no_fallback","content":"","level":"warning","originalModel":"claude-fable-5-1","requestId":"{request}","apiRefusalCategory":"cyber","refusedUserMessageUuid":"q","isMeta":false,"uuid":"{uuid}","timestamp":"2026-09-24T04:29:48.712Z","version":"2.1.281"}}"#
        )
    };
    let error = |uuid: &str, request: &str, parent: &str| {
        format!(
            r#"{{"parentUuid":"{parent}","isSidechain":false,"type":"assistant","uuid":"{uuid}","timestamp":"2026-09-24T04:29:48.711Z","message":{{"id":"e","model":"<synthetic>","role":"assistant","stop_reason":"stop_sequence","type":"message","content":[{{"type":"text","text":"API Error: Fable 5.1's safeguards flagged this message (https://www.anthropic.com/legal/aup)."}}]}},"requestId":"{request}","error":"invalid_request","isApiErrorMessage":true}}"#
        )
    };
    let reading =
        |held: &[String]| crate::quota_wall::decline_reading_in("claude", Some(SCREEN), Some(held));
    let earlier = [
        system("sys-1", "req_1"),
        error("declined-1", "req_1", "sys-1"),
    ];
    // Both ids agree: `sys-2` answers `req_2`, and `declined-2` names it.
    let agreeing: Vec<String> = earlier
        .iter()
        .cloned()
        .chain([
            system("sys-2", "req_2"),
            error("declined-2", "req_2", "sys-2"),
        ])
        .collect();
    // The request agrees, the parent disputes: `declined-3` is `req_2`'s
    // but names `sys-1` — an earlier request's record — as its parent.
    let disputing: Vec<String> = earlier
        .iter()
        .cloned()
        .chain([
            system("sys-2", "req_2"),
            error("declined-3", "req_2", "sys-1"),
        ])
        .collect();
    // The same record key as `agreeing`, but a later category record on
    // the same request whose uuid is not the parent named.
    let disputing_at_the_fence: Vec<String> = agreeing
        .iter()
        .cloned()
        .chain([system("sys-3", "req_2")])
        .collect();
    let policy = "--on-classifier-decline claude:claude-opus-4-8";
    let category_of = |held: &[String]| reading(held).record.expect("the decline").category;
    assert_eq!(category_of(&agreeing).as_deref(), Some("cyber"));
    assert_eq!(category_of(&disputing), None);
    assert_eq!(category_of(&disputing_at_the_fence), None);
    assert_eq!(
        reading(&disputing_at_the_fence)
            .record
            .expect("the decline")
            .key,
        "declined-2"
    );

    // At the beat: a contradiction is unrouted news, and walks nowhere.
    let stood = Walled::stand_declined_with(86_300, "/tmp", policy, reading(&disputing));
    tick(&stood.host, &[], stood.began + 10_000);
    let news = stood.json(
        "check --peek --types classifier_declined",
        stood.began + 10_001,
    );
    assert_eq!(news["count"], 1, "{news}");
    let body: serde_json::Value =
        serde_json::from_str(news["messages"][0]["body"].as_str().expect("a body")).expect("json");
    assert_eq!(
        body["category"],
        serde_json::Value::Null,
        "a category was borrowed: {body}"
    );
    assert_eq!(body["routed"], false, "{body}");
    assert!(
        stood.receipts(stood.began + 10_001).is_empty(),
        "a decline with a contradicted category was walked"
    );
    assert_eq!(stood.worker_state(&stood.worker), "Active");
    drop(stood);

    // At the fence: the same key, the category now contradicted — nothing
    // settles; read the same again, the walk lands.
    let stood = Walled::stand_declined_with(86_400, "/tmp", policy, reading(&agreeing));
    stood
        .host
        .read_at_the_fence(Some(reading(&disputing_at_the_fence)));
    tick(&stood.host, &[], stood.began + 10_000);
    assert!(
        stood
            .receipts(stood.began + 10_001)
            .iter()
            .all(|receipt| receipt["status"] != "done"),
        "settled on a category the fence read as contradicted"
    );
    assert_eq!(stood.worker_state(&stood.worker), "Active");
    stood.host.read_at_the_fence(None);
    tick(&stood.host, &[], stood.began + 20_000);
    let receipts = stood.receipts(stood.began + 20_001);
    let done: Vec<&serde_json::Value> = receipts
        .iter()
        .filter(|receipt| receipt["status"] == "done")
        .collect();
    assert_eq!(done.len(), 1, "{receipts:?}");
    assert_eq!(done[0]["category"], "cyber");
    assert_eq!(done[0]["recordKey"], "declined-2");
    assert_eq!(stood.worker_state(&stood.worker), "Released");
}

/// The three steps in order, through the door, each named to the
/// ledger: a clean tree is a skipped ① (said so), ② ends the attempt,
/// and ③ carries `--retry-of` + `--inherit-checkout` + the handover
/// paragraph at the head of the prompt. When the alternative is itself
/// at the wall the ledger's gate refuses ③ by name, and the receipt
/// settles `failed` at that step — the attempt is ended and the task
/// is `ready` for a person to place (the design's "stop and leave news").
#[test]
fn a_walled_alternative_fails_the_third_step_by_name_and_the_receipt_says_so() {
    const LEADER_TERM: u32 = 85_100;
    let stood = Walled::stand(
        LEADER_TERM,
        "/tmp",
        "--on-quota-wall claude:fable-5-1:high --wip-commit",
    );
    stood.wall_it();
    // The alternative reaches its own wall before the walk.
    let began = stood.began;
    stood.window.set_usage(vec![
        (
            "codex",
            usage_snapshot("codex", Some((98, None)), None, began - 60_000),
        ),
        (
            "claude",
            usage_snapshot(
                "claude",
                Some((99, Some(began + 9 * 60_000))),
                None,
                began - 60_000,
            ),
        ),
    ]);
    let plan = stood.plan();
    assert_eq!(plan.worker, stood.worker);
    let door = Recording::through(&stood.host, Ok(None));
    let held = super::runtime().expect("the runtime");
    let walked = walk_handover(&held.actor, &door, &plan, TEST_CAPABILITY, began + 20_000);
    let Walked::Failed { at, .. } = walked else {
        panic!("{walked:?}");
    };
    assert_eq!(at, "worker-start");
    let argv = door.argv();
    assert_eq!(argv.len(), 2, "{argv:?}");
    assert_eq!(
        &argv[0][..5],
        [
            "worker-stop",
            "--worker",
            stood.worker.as_str(),
            "--reason",
            "quota-wall"
        ]
    );
    assert!(argv[0].contains(&"--retry-request".to_string()));
    let start = &argv[1];
    assert_eq!(start[0], "worker-start");
    for (flag, value) in [
        ("--agent", "claude"),
        ("--model", "fable-5-1"),
        ("--effort", "high"),
        ("--task", stood.task.as_str()),
        ("--retry-of", stood.dispatch.as_str()),
    ] {
        let at = start
            .iter()
            .position(|word| word == flag)
            .unwrap_or_else(|| panic!("{flag} is missing from {start:?}"));
        assert_eq!(start[at + 1], value, "{flag}");
    }
    assert!(start.contains(&"--inherit-checkout".to_string()));
    let prompt_at = start
        .iter()
        .position(|word| word == "--prompt")
        .expect("a prompt");
    let prompt = &start[prompt_at + 1];
    assert!(prompt.starts_with("Handover:"), "{prompt}");
    assert!(prompt.contains(&stood.worker) && prompt.contains("codex quota wall"));
    assert!(prompt.ends_with("build-it"), "{prompt}");
    let receipts = stood.receipts(began + 20_001);
    assert_eq!(receipts.len(), 1);
    let steps = receipts[0]["steps"].as_array().expect("steps");
    assert_eq!(receipts[0]["status"], "failed");
    assert_eq!(
        steps
            .iter()
            .map(|step| (
                step["name"].as_str().unwrap_or_default(),
                step["ok"] == true
            ))
            .collect::<Vec<_>>(),
        vec![
            ("wip-commit", true),
            ("worker-stop", true),
            ("worker-start", false)
        ]
    );
    assert_eq!(steps[0]["detail"], "skipped: the checkout was clean");
    assert!(
        steps[2]["detail"]
            .as_str()
            .is_some_and(|why| why.contains("claude is at 99%")),
        "{}",
        steps[2]
    );
    assert!(receipts[0]["to"]["workerId"].is_null());
    assert_eq!(stood.worker_state(&stood.worker), "Released");
    let rows = the_rows();
    let task = rows
        .tasks
        .iter()
        .find(|one| one.id == stood.task)
        .expect("the task");
    assert_eq!(format!("{:?}", task.status), "Ready");
    // Walked once: a second walk of the same attempt is refused at the
    // reservation and moves nothing.
    assert_eq!(
        walk_handover(&held.actor, &door, &plan, TEST_CAPABILITY, began + 21_000),
        Walked::Nothing
    );
    assert_eq!(door.argv().len(), 2);
}

/// The replacement is briefed with where the walled worker left off: the
/// walk reads its transcript's tail while its pane still stands — before
/// ② closes it — and the paragraph carries its last words and latest tool
/// calls inside the untrusted fence, after the paragraph and before the
/// spec. A door that cannot see transcripts briefs exactly as before
/// (every other walk test here).
#[test]
fn the_replacement_is_briefed_with_where_the_walled_worker_left_off() {
    const LEADER_TERM: u32 = 85_150;
    let stood = Walled::stand(LEADER_TERM, "/tmp", "--on-quota-wall claude:fable-5-1");
    stood.wall_it();
    let plan = stood.plan();
    let tail = vec![
        r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Lexer patched; the parser suite is next."}]}}"#.to_string(),
        r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test -p parser\"}","call_id":"c1"}}"#.to_string(),
    ];
    let door =
        Recording::through(&stood.host, Ok(None)).with_tail("/tmp/rollout-walled.jsonl", tail);
    let held = super::runtime().expect("the runtime");
    let _ = walk_handover(
        &held.actor,
        &door,
        &plan,
        TEST_CAPABILITY,
        stood.began + 20_000,
    );

    // Asked once, before any verb ran — ② had not yet closed the pane.
    assert_eq!(
        *door
            .tail_asked
            .lock()
            .unwrap_or_else(|held| held.into_inner()),
        vec![0]
    );
    let argv = door.argv();
    let start = argv
        .iter()
        .find(|one| one.first().map(String::as_str) == Some("worker-start"))
        .expect("a worker-start");
    let prompt_at = start
        .iter()
        .position(|word| word == "--prompt")
        .expect("a prompt");
    let prompt = &start[prompt_at + 1];
    assert!(prompt.starts_with("Handover:"), "{prompt}");
    for needed in [
        format!("Where worker {} left off", stood.worker),
        "/tmp/rollout-walled.jsonl".to_string(),
        "Last words: Lexer patched; the parser suite is next.".to_string(),
        "cargo test -p parser".to_string(),
    ] {
        assert!(prompt.contains(&needed), "`{needed}` is missing:\n{prompt}");
    }
    assert_eq!(
        prompt.matches(zerocode_core::untrusted::PHRASE).count(),
        2,
        "{prompt}"
    );
    assert!(prompt.ends_with("build-it"), "{prompt}");
}

/// A WIP commit that fails aborts the walk before any verb: the receipt
/// settles `failed` at step ①, the worker keeps its pane and its
/// attempt, and the task stays carried — a tree the window cannot
/// commit is not a tree it ends a worker in. And a policy WITHOUT
/// `--wip-commit` never asks git at all: step ① is "not asked", and the
/// replacement's paragraph says the tree was left as it was.
#[test]
fn a_failed_wip_commit_aborts_with_news_and_an_unasked_one_is_skipped() {
    const LEADER_TERM: u32 = 85_200;
    let stood = Walled::stand(LEADER_TERM, "/tmp", "--on-quota-wall claude --wip-commit");
    stood.wall_it();
    let began = stood.began;
    let plan = stood.plan();
    let refusing = Recording::through(
        &stood.host,
        Err("git commit failed: not a git repository".to_string()),
    );
    let held = super::runtime().expect("the runtime");
    let walked = walk_handover(
        &held.actor,
        &refusing,
        &plan,
        TEST_CAPABILITY,
        began + 20_000,
    );
    assert!(
        matches!(walked, Walked::Failed { ref at, .. } if at == "wip-commit"),
        "{walked:?}"
    );
    assert!(
        refusing.argv().is_empty(),
        "a verb was issued after a failed commit"
    );
    let receipts = stood.receipts(began + 20_001);
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["status"], "failed");
    let steps = receipts[0]["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0]["name"], "wip-commit");
    assert_eq!(steps[0]["ok"], false);
    assert!(
        steps[0]["detail"]
            .as_str()
            .is_some_and(|why| why.contains("not a git repository"))
    );
    assert_eq!(stood.worker_state(&stood.worker), "Active");
    let rows = the_rows();
    let dispatch = rows
        .dispatches
        .iter()
        .find(|one| one.id == stood.dispatch)
        .expect("the attempt");
    assert!(
        dispatch.ended_ms.is_none(),
        "a failed commit ended the attempt"
    );

    // A second window, no `--wip-commit`: git is never asked.
    drop(refusing);
    drop(stood);
    let stood = Walled::stand(LEADER_TERM + 10, "/tmp", "--on-quota-wall claude");
    stood.wall_it();
    let began = stood.began;
    let plan = stood.plan();
    assert!(!plan.wip_commit);
    let never = Recording::through(&stood.host, Err("must not be asked".to_string()));
    let held = super::runtime().expect("the runtime");
    let walked = walk_handover(&held.actor, &never, &plan, TEST_CAPABILITY, began + 20_000);
    let Walked::Done { replacement, .. } = walked else {
        panic!("{walked:?}");
    };
    assert!(
        never
            .committed
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .is_empty(),
        "git was asked without --wip-commit"
    );
    let receipts = stood.receipts(began + 20_001);
    assert_eq!(receipts[0]["status"], "done");
    assert_eq!(receipts[0]["to"]["workerId"], replacement);
    let steps = receipts[0]["steps"].as_array().expect("steps");
    assert!(
        steps[0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.starts_with("skipped: not asked")),
        "{}",
        steps[0]
    );
    let start = &never.argv()[1];
    let prompt = &start[start
        .iter()
        .position(|word| word == "--prompt")
        .expect("a prompt")
        + 1];
    assert!(prompt.contains("still in the tree"), "{prompt}");
    assert_eq!(stood.worker_state(&replacement), "Active");
}

/// The beat writes `quota_walled` only for a quiet pane with BOTH
/// witnesses — the screen's words and the cache's number at the wall —
/// and writes it once; a quiet pane with only one of them stays the
/// `went_quiet` it always was. Nothing is settled either way.
///
/// The cache is the private window's own (`boot_with_usage`): codex at
/// 98% three minutes ago, claude at 50%. Two workers, both quiet, both
/// with a wall on screen; only the codex one has the number.
#[test]
fn the_beat_writes_quota_walled_news_only_with_both_witnesses_and_settles_nothing() {
    const LEADER_TERM: u32 = 84_000;
    const CODEX_TERM: u32 = 84_001;
    const CLAUDE_TERM: u32 = 84_002;
    let began = clock();
    // Summoned with room, walled later: the gate would refuse a codex
    // summons at 98%, and a wall is something a running worker hits.
    let gauges = |codex_used: u8| {
        vec![
            (
                "codex",
                usage_snapshot(
                    "codex",
                    Some((codex_used, Some(began + 42 * 60_000))),
                    Some((40, None)),
                    began - 3 * 60_000,
                ),
            ),
            (
                "claude",
                usage_snapshot("claude", Some((50, None)), Some((30, None)), began - 60_000),
            ),
        ]
    };
    let (window, _store) = PrivateWindow::boot_with_usage(gauges(40));
    let _beat = one_beat_at_a_time();
    let team = format!("team-quota-wall-{LEADER_TERM}");
    seat_a_team(&team, LEADER_TERM);
    let host = AtTheWall {
        closed: Mutex::new(Vec::new()),
        busy: Mutex::new(false),
        onto: Mutex::new(CODEX_TERM),
        checkout: "/wt/walled",
        markers: Mutex::new(std::collections::HashMap::new()),
        asked: Mutex::new(Vec::new()),
        declines: Mutex::new(std::collections::HashMap::new()),
        pty_quiet_ms: Mutex::new(None),
        screens: Mutex::new(std::collections::HashMap::new()),
        fence_reading: Mutex::new(None),
    };
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let verb = |line: &str, at: i64| {
        let said = run(
            &host,
            Vec::new(),
            &team,
            leader,
            TEST_CAPABILITY,
            &words(line),
            at,
        );
        assert_eq!(said.exit_code, 0, "`{line}`: {}", said.stderr);
        serde_json::from_str::<serde_json::Value>(&said.stdout).expect("json")
    };
    verb("run-create --name walls", began);
    let first = verb("task-create --spec build-it", began + 1)["taskId"]
        .as_str()
        .expect("a task")
        .to_string();
    let second = verb("task-create --spec test-it", began + 2)["taskId"]
        .as_str()
        .expect("a task")
        .to_string();
    let codex = verb(
        &format!("worker-start --agent codex --task {first}"),
        began + 3,
    );
    let codex_worker = codex["workerId"].as_str().expect("a worker").to_string();
    host.seating_onto(CLAUDE_TERM);
    let claude = verb(
        &format!("worker-start --agent claude --task {second}"),
        began + 4,
    );
    let claude_worker = claude["workerId"].as_str().expect("a worker").to_string();
    host.screen_says(
        CODEX_TERM,
        "screen",
        "You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage",
    );
    host.screen_says(
        CLAUDE_TERM,
        "screen",
        "You've hit your session limit · resets 4:10am (Asia/Seoul)",
    );
    let lifecycle = |worker: &str| {
        let rows = the_rows();
        let held = rows
            .workers
            .iter()
            .find(|one| one.id == worker)
            .expect("the worker");
        let dispatch = rows
            .dispatches
            .iter()
            .find(|one| Some(one.id.as_str()) == held.dispatch.as_deref())
            .expect("its dispatch");
        let task = rows
            .tasks
            .iter()
            .find(|one| one.id == dispatch.task)
            .expect("its task");
        format!("{:?}|{:?}|{:?}", held.state, dispatch.ended_ms, task.status)
    };
    let before = (lifecycle(&codex_worker), lifecycle(&claude_worker));
    window.set_usage(gauges(98));
    let news = |kind: &str, at: i64| -> Vec<serde_json::Value> {
        verb(&format!("check --peek --types {kind}"), at)["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| {
                let mut body: serde_json::Value =
                    serde_json::from_str(message["body"].as_str().expect("a body")).expect("json");
                body["from"] = message["from"].clone();
                body["type"] = message["type"].clone();
                body
            })
            .collect()
    };

    tick(&host, &[], began + 10_000);
    let walled = news("quota_walled", began + 10_001);
    assert_eq!(walled.len(), 1, "{walled:?}");
    assert_eq!(
        walled[0]["from"],
        zerocode_core::orchestration::LEDGER_ITSELF
    );
    assert_eq!(walled[0]["workerId"], codex_worker);
    assert_eq!(walled[0]["taskId"], first);
    assert_eq!(walled[0]["provider"], "codex");
    assert_eq!(walled[0]["usedPercent"], 98);
    assert_eq!(walled[0]["resetsAtMs"], began + 42 * 60_000);
    assert_eq!(walled[0]["checkout"], "/wt/walled");
    assert_eq!(walled[0]["marker"]["source"], "screen");
    let quiet = news("went_quiet", began + 10_002);
    assert_eq!(
        quiet.len(),
        1,
        "a wall with one witness was not a silence: {quiet:?}"
    );
    assert_eq!(quiet[0]["workerId"], claude_worker);
    assert_eq!(quiet[0]["reason"], "stalled");
    assert_eq!(
        (lifecycle(&codex_worker), lifecycle(&claude_worker)),
        before,
        "the news settled work"
    );

    // The same wall on the next beat is the same fact.
    tick(&host, &[], began + 11_000);
    assert_eq!(news("quota_walled", began + 11_001).len(), 1);
    assert_eq!(
        (lifecycle(&codex_worker), lifecycle(&claude_worker)),
        before
    );
}

/// A wall stands until its reset and the slack after it, and no longer
/// (t-6427). Every worker this machine walled continued by itself a minute
/// after its reset, and two of them later died on a network error the sweep
/// never reported: the attempt's wall row silenced it for good (dp-6390 and
/// dp-6393, 2026-09-23 — 42 and 77 minutes until somebody looked). While the
/// wall stands its silence is the wall's; after it, a worker whose own words
/// moved past the wall is a quiet worker like any other, and one walled
/// again in the next window is walled news again.
#[test]
fn a_walled_attempts_later_silence_is_news_once_its_wall_stops_standing() {
    const LEADER_TERM: u32 = 85_600;
    const WORKER_TERM: u32 = LEADER_TERM + 1;
    let stood = Walled::stand(LEADER_TERM, "/wt/after-the-wall", "");
    stood.wall_it();
    let began = stood.began;
    let reset = began + 42 * 60_000;
    let next_reset = reset + 5 * 60 * 60_000;
    let stops_standing = reset + zerocode_core::orchestration::QUOTA_WAIT_POLICY.slack_ms;
    let stalls = |at: i64| -> Vec<serde_json::Value> {
        stood.json("check --peek --types went_quiet", at)["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| {
                serde_json::from_str::<serde_json::Value>(message["body"].as_str().expect("a body"))
                    .expect("json")
            })
            .filter(|body| body["workerId"] == stood.worker.as_str())
            .collect()
    };
    let codex_at = |used: u8, updated: i64| {
        vec![(
            "codex",
            usage_snapshot(
                "codex",
                Some((used, Some(next_reset))),
                Some((40, None)),
                updated,
            ),
        )]
    };

    // While the wall stands, the silence is the wall's.
    notify_stalled_workers(&stood.host, stops_standing - 1);
    assert!(stalls(stops_standing - 1).is_empty());

    // It went on after its reset and stopped on something else.
    stood.host.screen_clears(WORKER_TERM);
    stood.window.set_usage(codex_at(3, reset + 60_000));
    notify_stalled_workers(&stood.host, stops_standing);
    let told = stalls(stops_standing + 1);
    assert_eq!(
        told.len(),
        1,
        "a silence after the wall was not news: {told:?}"
    );
    assert_eq!(told[0]["reason"], "stalled");

    // And the next window's wall is news of its own.
    stood.host.screen_says(
        WORKER_TERM,
        "screen",
        "You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage",
    );
    stood
        .window
        .set_usage(codex_at(98, stops_standing + 60_000));
    notify_stalled_workers(&stood.host, stops_standing + 120_000);
    assert_eq!(
        stood.json(
            "check --peek --types quota_walled",
            stops_standing + 120_001
        )["count"],
        2
    );
}

/// Under a declared wait, a worker still stopped at its wall once the wall
/// has lifted is the coordinator's news, once (t-6427) — Claude Code
/// continues by itself about a minute after its reset, so this is the notice
/// for the walls nobody continued: a CLI that does not wait, or a countdown
/// somebody cancelled. From the reset on, the beat asks the window's gauge
/// for a reading after it; with none yet it says nothing and waits for one,
/// and with a number under the wall it tells the lift with that number.
#[test]
fn a_worker_still_at_its_wall_after_the_lift_is_told_once_under_a_wait() {
    const LEADER_TERM: u32 = 85_700;
    let stood = Walled::stand(LEADER_TERM, "/wt/lifted", "--on-quota-wall wait");
    stood.wall_it();
    let began = stood.began;
    let reset = began + 42 * 60_000;
    let stops_standing = reset + zerocode_core::orchestration::QUOTA_WAIT_POLICY.slack_ms;
    let notices = |at: i64| -> Vec<serde_json::Value> {
        stood.json("check --peek --types went_quiet", at)["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| {
                serde_json::from_str::<serde_json::Value>(message["body"].as_str().expect("a body"))
                    .expect("json")
            })
            .filter(|body| body["workerId"] == stood.worker.as_str())
            .collect()
    };

    // Before the reset nothing is asked; from it, the gauge is.
    notify_stalled_workers(&stood.host, reset - 1_000);
    assert!(stood.host.asked().is_empty(), "{:?}", stood.host.asked());
    notify_stalled_workers(&stood.host, reset + 1_000);
    assert_eq!(stood.host.asked(), vec!["codex".to_string()]);

    // Past the slack, with no reading since the reset: waited for, unsaid.
    notify_stalled_workers(&stood.host, stops_standing);
    assert!(
        notices(stops_standing).is_empty(),
        "{:?}",
        notices(stops_standing)
    );
    assert_eq!(stood.host.asked().len(), 2);

    // The reading comes, under the wall: told once, with its number.
    stood.window.set_usage(vec![(
        "codex",
        usage_snapshot(
            "codex",
            Some((3, Some(reset + 5 * 60 * 60_000))),
            Some((40, None)),
            stops_standing + 1_000,
        ),
    )]);
    notify_stalled_workers(&stood.host, stops_standing + 2_000);
    let told = notices(stops_standing + 2_001);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(
        told[0]["reason"],
        zerocode_core::orchestration::QUOTA_LIFTED_REASON
    );
    assert_eq!(told[0]["rung"], "wait");
    // The binding window after the reset is the fuller one: the week's.
    assert_eq!(told[0]["gauge"]["usedPercent"], 40);
    assert_eq!(told[0]["gauge"]["window"], "weekly");
    notify_stalled_workers(&stood.host, stops_standing + 3_000);
    assert_eq!(notices(stops_standing + 3_001).len(), 1, "told twice");
}

/// The wait rung waits for a number read after the reset exactly as long as
/// a re-read it asks for can take to be allowed (t-6427): the ledger's table
/// and the window's refetch floor are one number.
#[test]
fn the_wait_rung_waits_one_refetch_floor_for_a_number_read_after_the_reset() {
    assert_eq!(
        zerocode_core::orchestration::QUOTA_WAIT_POLICY.lift_read_ms,
        i64::try_from(crate::usage::MIN_REFETCH.as_millis()).expect("a floor in ms"),
    );
}

/// Every gauge the ledger's table can name for an agent is one the beat can
/// ask the window to read again (t-6427): a gauge with no ask would leave a
/// lifted wall unread until the rung stopped waiting for its number.
#[test]
fn every_gauge_the_ledger_reads_is_one_the_window_can_ask_again() {
    let asks: Vec<&str> = crate::cmd::usage::USAGE_ASKS
        .iter()
        .map(|(gauge, _)| *gauge)
        .collect();
    let mut named = 0;
    for spec in zerocode_core::agent::AGENT_SPECS {
        for model in [None, Some("claude"), Some("gpt")] {
            if let Some(gauge) = zerocode_core::orchestration::quota_gauge_for(spec.id, model) {
                named += 1;
                assert!(
                    asks.contains(&gauge),
                    "{} names the gauge `{gauge}`, which nothing asks again",
                    spec.id
                );
            }
        }
    }
    assert!(named > 0, "no agent named a gauge");
}

/* ---- the transient-error continuation (t-4537) ----------------------- */

/// A host whose worker pane is quiet and whose worker's transcript is a
/// real file the test writes — a stopped worker as the window finds one —
/// and which remembers every keystroke. `busy` stands for a turn the worker
/// is running; `refusing` for a pane whose door types nothing (its `send`
/// refuses, so the pointer's default road answers a timed-out receipt).
struct Stopped {
    worker_term: u32,
    transcript: std::path::PathBuf,
    busy: Mutex<bool>,
    refusing: Mutex<bool>,
    sent: Mutex<Vec<(u32, String)>>,
    /// The worker pane's visible screen.
    screen: Mutex<String>,
    /// Where this window's System One wire points — an origin and zo's
    /// settings file — when the case is about a Jev question (t-4538).
    jev: Mutex<Option<(String, std::path::PathBuf)>>,
}

impl Stopped {
    fn typed_at(&self, term: u32) -> Vec<String> {
        self.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .iter()
            .filter(|(at, _)| *at == term)
            .map(|(_, text)| text.clone())
            .collect()
    }
}

impl Host for Stopped {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        token: &str,
    ) -> Option<u32> {
        crate::agent_teams::place_seat_checkout(token, "/wt/stopped".to_string());
        Some(self.worker_term)
    }
    fn send(&self, term: u32, text: &str) -> bool {
        if *self
            .refusing
            .lock()
            .unwrap_or_else(|held| held.into_inner())
        {
            return false;
        }
        self.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push((term, text.to_string()));
        true
    }
    fn pane_exists(&self, _term: u32) -> bool {
        true
    }
    fn quiet_since(&self, _term: u32, _worker_started_ms: i64, now_ms: i64) -> Option<i64> {
        (!*self.busy.lock().unwrap_or_else(|held| held.into_inner()))
            .then_some(now_ms - zerocode_core::orchestration::QUIET_GRACE_MS)
    }
    /// The wall witness the real host reads, off the same file.
    fn quota_wall_marker(
        &self,
        term: u32,
        agent: &str,
    ) -> Option<zerocode_core::orchestration::QuotaWallMarker> {
        (term == self.worker_term)
            .then(|| crate::quota_wall::marker_for(agent, None, Some(&self.transcript)))
            .flatten()
    }
    fn provider_session(&self, term: u32) -> Option<zerocode_core::ProviderSession> {
        (term == self.worker_term).then(|| zerocode_core::ProviderSession {
            key: zerocode_core::provider_session::SessionKey::SessionId,
            id: "d4e81823-3b9a-4bf1-a54b-b4366117b514".to_string(),
            transcript_path: Some(self.transcript.to_string_lossy().into_owned()),
        })
    }
    fn capture(&self, term: u32) -> Option<String> {
        Some(if term == self.worker_term {
            self.screen
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .clone()
        } else {
            String::new()
        })
    }
    fn focus(&self, _term: u32) -> bool {
        true
    }
    fn close(&self, _term: u32) {}
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
    fn jev_wire(&self) -> Option<crate::systemone::Wire> {
        self.jev
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .as_ref()
            .map(|(base, settings)| {
                crate::systemone::Wire::at(base, "test-key", Some(settings.clone()))
            })
    }
}

/// A private window with one claude worker summoned under `policy` (not
/// asserted here: the red half of this road is a verb that did not exist),
/// its pane at `leader_term + 1` and its transcript in a temp file.
struct StoppedWorker {
    window: PrivateWindow,
    _store: zerocode_orchestrator::workflow_store::WorkflowStore,
    _beat: std::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    host: Stopped,
    team: String,
    worker: String,
    began: i64,
}

impl StoppedWorker {
    fn stand(
        leader_term: u32,
        policy: &str,
        usage: Vec<(&'static str, crate::usage::ProviderUsage)>,
    ) -> Self {
        Self::stand_with(leader_term, "--agent claude", policy, usage)
    }

    /// The same window with the worker summoned by `summons` — the words
    /// after `worker-start`, so a case can name the agent and its dial.
    fn stand_with(
        leader_term: u32,
        summons: &str,
        policy: &str,
        usage: Vec<(&'static str, crate::usage::ProviderUsage)>,
    ) -> Self {
        let began = clock();
        let (window, store) = PrivateWindow::boot_with_usage(usage);
        let beat = one_beat_at_a_time();
        let team = format!("team-stopped-{leader_term}");
        seat_a_team(&team, leader_term);
        let dir = tempfile::tempdir().expect("a transcript dir");
        let host = Stopped {
            worker_term: leader_term + 1,
            transcript: dir
                .path()
                .join("d4e81823-3b9a-4bf1-a54b-b4366117b514.jsonl"),
            busy: Mutex::new(false),
            refusing: Mutex::new(false),
            sent: Mutex::new(Vec::new()),
            screen: Mutex::new(String::new()),
            jev: Mutex::new(None),
        };
        let mut stood = Self {
            window,
            _store: store,
            _beat: beat,
            _dir: dir,
            host,
            team,
            worker: String::new(),
            began,
        };
        stood.json("run-create --name stopped", began);
        let task = stood.json("task-create --spec build-it", began + 1)["taskId"]
            .as_str()
            .expect("a task")
            .to_string();
        stood.worker =
            stood.json(&format!("worker-start {summons} --task {task}"), began + 2)["workerId"]
                .as_str()
                .expect("a worker")
                .to_string();
        if !policy.is_empty() {
            let _ = stood.verb(&format!("handover-policy {policy}"), began + 3);
        }
        stood
    }

    fn verb(&self, line: &str, at: i64) -> zerocode_hookd::TeamAnswer {
        run(
            &self.host,
            Vec::new(),
            &self.team,
            zerocode_core::agent_teams::LEADER_PANE,
            TEST_CAPABILITY,
            &words(line),
            at,
        )
    }

    fn json(&self, line: &str, at: i64) -> serde_json::Value {
        let said = self.verb(line, at);
        assert_eq!(said.exit_code, 0, "`{line}`: {}", said.stderr);
        serde_json::from_str(&said.stdout).expect("json")
    }

    /// The worker's turn ends on `last` — its transcript's newest records,
    /// the hook's `StopFailure` read as a finished turn, the pane at rest.
    fn stops_on(&self, last: &[&str], at: i64) {
        std::fs::write(&self.host.transcript, format!("{}\n", last.join("\n")))
            .expect("the transcript");
        *self
            .host
            .busy
            .lock()
            .unwrap_or_else(|held| held.into_inner()) = false;
        super::pane_turn_ended(self.host.worker_term, at - 1, false, at);
    }

    /// The bodies of this run's pending news of `kind`, about this worker.
    fn news(&self, kind: &str, at: i64) -> Vec<serde_json::Value> {
        self.json(&format!("check --peek --types {kind}"), at)["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| {
                serde_json::from_str::<serde_json::Value>(message["body"].as_str().expect("a body"))
                    .expect("json")
            })
            .filter(|body| body["workerId"] == self.worker.as_str())
            .collect()
    }

    /// The attempt this worker carries now, as the ledger holds it.
    fn attempt(&self) -> zerocode_core::orchestration::DispatchRow {
        let rows = the_rows();
        let held = rows
            .workers
            .iter()
            .find(|one| one.id == self.worker)
            .expect("the worker");
        let open = held.dispatch.as_deref().expect("an open attempt");
        rows.dispatches
            .iter()
            .find(|one| one.id == open)
            .expect("the attempt")
            .clone()
    }

    /// The worker reports done, and its pane is handed a second task —
    /// the same worker, its summons time standing, on a new attempt
    /// (t-7153, R3). Answers the new attempt.
    fn handed_a_second_task(&self, at: i64) -> zerocode_core::orchestration::DispatchRow {
        let pane = the_rows()
            .workers
            .iter()
            .find(|one| one.id == self.worker)
            .expect("the worker")
            .pane
            .clone();
        let held = crate::agent_teams::current_pane_capability(&self.team, &pane)
            .expect("the worker's capability");
        let done = run(
            &self.host,
            Vec::new(),
            &self.team,
            &pane,
            &held,
            &words(&format!(
                "send --type worker_done --body {{\"ok\":true}} --retry-request done-{}-{at}",
                self.worker
            )),
            at,
        );
        assert_eq!(done.exit_code, 0, "{}", done.stderr);
        let task = self.json("task-create --spec build-it-again", at + 1)["taskId"]
            .as_str()
            .expect("a task")
            .to_string();
        self.json(
            &format!("dispatch --task {task} --to {pane} --inject --retry-request re-{task}"),
            at + 2,
        );
        let attempt = self.attempt();
        assert_eq!(attempt.started_ms, at + 2, "the second attempt's own start");
        attempt
    }

    /// One more record at the end of the worker's transcript.
    fn appends(&self, line: &str) {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.host.transcript)
            .expect("append");
        writeln!(file, "{line}").expect("write");
    }

    /// This worker's `model_deviated` rows: the attempt each was written
    /// for, and the switch's key, oldest first.
    fn switches_written(&self, at: i64) -> Vec<(String, String)> {
        self.news("model_deviated", at)
            .iter()
            .map(|body| {
                (
                    body["dispatchId"].as_str().expect("an attempt").to_string(),
                    body["key"].as_str().expect("a key").to_string(),
                )
            })
            .collect()
    }
}

/// The same stop, a later error: w-4525's record under a new identity.
fn transient_record_keyed(key: &str) -> String {
    crate::quota_wall::tests::CLAUDE_TRANSIENT_RECORD
        .replace("8ff6adcf-19c1-45a5-b351-c3aa70bd37b7", key)
}

/// w-4525, 2026-09-17: a claude worker's first turn died on "API Error: The
/// response stopped arriving", its pane sat at an empty composer for
/// minutes, and only a coordinator's mail typed the line that woke it.
/// Under a declared `--on-transient-error resume` the beat types the
/// continuation itself — once, however many beats see the same stop — and
/// the stop is not also `went_quiet` news.
#[test]
fn a_worker_stopped_by_a_transient_api_error_is_typed_at_once_under_a_declared_order() {
    use crate::quota_wall::tests::{
        CLAUDE_PLAIN_RECORD, CLAUDE_TRANSIENT_RECORD, CLAUDE_TURN_DURATION,
    };
    let stood = StoppedWorker::stand(96_000, "--on-transient-error resume", Vec::new());
    stood.stops_on(
        &[
            CLAUDE_PLAIN_RECORD,
            CLAUDE_TRANSIENT_RECORD,
            CLAUDE_TURN_DURATION,
        ],
        stood.began + 9_000,
    );
    for beat in [10_000, 11_000, 12_000] {
        tick(&stood.host, &[], stood.began + beat);
    }
    let typed = stood.host.typed_at(96_001);
    assert_eq!(
        typed.len(),
        2,
        "not one continuation and its Enter: {typed:?}"
    );
    assert!(
        typed[0].contains("continue from where you left off"),
        "{typed:?}"
    );
    assert_eq!(typed[1], "\r");
    assert_eq!(
        stood.news("went_quiet", stood.began + 12_001),
        Vec::<serde_json::Value>::new(),
        "a stop the beat answered was also a silence"
    );
}

/// A switch of model is read off the FILE the cursor counted into, and is
/// the worker's whose attempt was open when it was written (t-7153, R3).
/// A transcript replaced under its path by another — as long as the old
/// one, with a switch in its first bytes; or longer — is read from its
/// start and the switch written, where before the old offset stood and a
/// same-length replacement read as unchanged; a switch dated before the
/// worker's summons is not written for it; and a worker seated later at the
/// same path — a session resumed — is written only the switches after its
/// own summons, never the earlier worker's as its own. A restart that
/// forgets the cursors reads the file again and writes nothing twice.
#[test]
fn a_switch_in_a_transcript_replaced_under_its_path_is_written_and_an_earlier_workers_is_not() {
    use crate::quota_wall::tests::{CLAUDE_FALLBACK, CLAUDE_PLAIN_RECORD, a_transcript_as_long_as};
    use zerocode_core::civil::iso_utc_of;
    const LEADER_TERM: u32 = 97_300;
    let stood = StoppedWorker::stand(LEADER_TERM, "", Vec::new());
    let switch = |key: &str, at_ms: i64| {
        CLAUDE_FALLBACK
            .replace("switch-1", key)
            .replace("2026-09-21T14:06:16.566Z", &iso_utc_of(at_ms))
    };
    let replace = |content: &str| {
        let fresh = stood._dir.path().join("next.jsonl");
        std::fs::write(&fresh, content).expect("the replacement");
        std::fs::rename(&fresh, &stood.host.transcript).expect("atomic replace");
    };
    let keys = |worker: &str, at: i64| -> Vec<String> {
        stood.json("check --peek --types model_deviated", at)["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| {
                serde_json::from_str::<serde_json::Value>(message["body"].as_str().expect("a body"))
                    .expect("json")
            })
            .filter(|body| body["workerId"] == worker)
            .map(|body| body["key"].as_str().expect("a key").to_string())
            .collect()
    };

    replace(&format!(
        "{CLAUDE_PLAIN_RECORD}\n{}\n",
        switch("switch-1", stood.began + 5_000)
    ));
    tick(&stood.host, &[], stood.began + 10_000);
    tick(&stood.host, &[], stood.began + 11_000);
    assert_eq!(keys(&stood.worker, stood.began + 11_001), ["switch-1"]);
    let old_len = std::fs::metadata(&stood.host.transcript)
        .expect("size")
        .len();

    // Replaced under its path, as long as before, a new switch first.
    replace(&a_transcript_as_long_as(
        &switch("switch-2", stood.began + 12_000),
        old_len,
    ));
    tick(&stood.host, &[], stood.began + 13_000);
    assert_eq!(
        keys(&stood.worker, stood.began + 13_001),
        ["switch-1", "switch-2"],
        "a switch in a transcript replaced as long as the old one was never read"
    );

    // Replaced by a longer one, a new switch first.
    replace(&format!(
        "{}\n{CLAUDE_PLAIN_RECORD}\n{CLAUDE_PLAIN_RECORD}\n",
        switch("switch-3", stood.began + 14_000)
    ));
    tick(&stood.host, &[], stood.began + 15_000);
    assert_eq!(
        keys(&stood.worker, stood.began + 15_001),
        ["switch-1", "switch-2", "switch-3"],
        "a switch in a longer replacement's first bytes was never read"
    );

    // A switch dated before the summons is not this worker's.
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&stood.host.transcript)
        .expect("append");
    use std::io::Write;
    writeln!(file, "{}", switch("switch-0", stood.began - 60_000)).expect("write");
    drop(file);
    tick(&stood.host, &[], stood.began + 16_000);
    assert_eq!(
        keys(&stood.worker, stood.began + 16_001),
        ["switch-1", "switch-2", "switch-3"],
        "a switch written before the worker's summons was written as its own"
    );

    // Another worker seated at the same path: only what follows its summons.
    let earlier = stood.worker.clone();
    stood.json(
        &format!("worker-stop --worker {earlier} --reason done --retry-request stop-{earlier}"),
        stood.began + 17_000,
    );
    let task = stood.json("task-create --spec build-it-again", stood.began + 17_001)["taskId"]
        .as_str()
        .expect("a task")
        .to_string();
    let later = stood.json(
        &format!("worker-start --agent claude --task {task}"),
        stood.began + 18_000,
    )["workerId"]
        .as_str()
        .expect("a worker")
        .to_string();
    tick(&stood.host, &[], stood.began + 19_000);
    tick(&stood.host, &[], stood.began + 20_000);
    assert_eq!(
        keys(&later, stood.began + 20_001),
        Vec::<String>::new(),
        "an earlier worker's switches were written as the later worker's own"
    );
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&stood.host.transcript)
        .expect("append");
    writeln!(file, "{}", switch("switch-4", stood.began + 21_000)).expect("write");
    drop(file);
    tick(&stood.host, &[], stood.began + 22_000);
    tick(&stood.host, &[], stood.began + 23_000);
    assert_eq!(keys(&later, stood.began + 23_001), ["switch-4"]);
    assert_eq!(
        keys(&earlier, stood.began + 23_001),
        ["switch-1", "switch-2", "switch-3"],
        "the ended worker was written more"
    );

    // A window restart forgets every cursor: the same attempt's file is
    // read again from its start, and the ledger — not the scan — writes
    // nothing twice.
    super::deviation_scans()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .clear();
    tick(&stood.host, &[], stood.began + 24_000);
    tick(&stood.host, &[], stood.began + 25_000);
    assert_eq!(
        keys(&later, stood.began + 25_001),
        ["switch-4"],
        "a switch read again after a restart was written twice"
    );
    assert_eq!(
        keys(&earlier, stood.began + 25_001),
        ["switch-1", "switch-2", "switch-3"]
    );
}

/// A transcript that grows a window every beat is still checked whole
/// (t-7153 r4, R3a, after astra t-6963 on 2e4ea291): its MIDDLE window
/// rewritten in place — the same inode, its first line, its length and the
/// bytes before the cursor kept, a switch where a record as long stood —
/// is found within `2N − 1` readings for the `N` windows the scan had
/// counted, the file growing a window a beat all the while; the switch is
/// read on the next reading and written once; and a file only appended to
/// is never read again from its start. Rewritten on the beat the scan
/// caught up, and again after three beats of growth, when the round under
/// way had passed the middle window. Before this a reading checked its
/// count over the windows counted NOW, both moving one a beat, and the
/// middle window was never checked again. Through the real beat and the
/// real ledger.
#[test]
fn a_middle_window_rewritten_in_place_is_found_while_the_transcript_grows_a_window_a_beat() {
    use crate::quota_wall::tests::{CLAUDE_FALLBACK, SLOT_AT, overwrite_at, records_with_a_slot};
    use zerocode_core::civil::iso_utc_of;
    use zerocode_core::transcript::MAX_TAIL_BYTES;
    let window = usize::try_from(MAX_TAIL_BYTES).expect("small");
    for (leader_term, grown_first) in [(97_830, 0), (97_840, 3)] {
        let stood = StoppedWorker::stand(leader_term, "", Vec::new());
        let switch = CLAUDE_FALLBACK
            .replace("switch-1", "switch-mid")
            .replace("2026-09-21T14:06:16.566Z", &iso_utc_of(stood.began + 5_000));
        let run = records_with_a_slot(window, switch.len());
        std::fs::write(&stood.host.transcript, run.repeat(3)).expect("three windows");
        let grow = || {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(&stood.host.transcript)
                .expect("append")
                .write_all(run.as_bytes())
                .expect("a window more");
        };
        let cursor = || {
            super::deviation_scans()
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .get(&stood.worker)
                .map_or(0, |held| held.cursor.offset)
        };
        let mut now = stood.began + 10_000;
        let mut beat = || {
            now += 1_000;
            tick(&stood.host, &[], now);
            now
        };

        // Read to its end, a window a beat.
        for _ in 0..8 {
            if cursor() == 3 * MAX_TAIL_BYTES {
                break;
            }
            beat();
        }
        assert_eq!(cursor(), 3 * MAX_TAIL_BYTES, "not read to its end");
        // Grown a window a beat, never rewritten: read on, a window a beat.
        for _ in 0..grown_first {
            let was = cursor();
            grow();
            beat();
            assert_eq!(
                cursor(),
                was + MAX_TAIL_BYTES,
                "a file only appended to was read again from its start, or not read on"
            );
        }

        // The middle window's slot becomes the switch, in place; the file
        // grows on, a window a beat.
        let counted = cursor().div_ceil(MAX_TAIL_BYTES);
        let bound = 2 * counted - 1;
        overwrite_at(
            &stood.host.transcript,
            MAX_TAIL_BYTES + SLOT_AT as u64,
            &switch,
        );
        let (mut started_over, mut written, mut readings) = (None, None, 0);
        while written.is_none() && readings < 2 * bound {
            let was = cursor();
            grow();
            let at = beat();
            readings += 1;
            if started_over.is_none() && cursor() < was {
                started_over = Some(readings);
            }
            if !stood.switches_written(at + 1).is_empty() {
                written = Some(readings);
            }
        }
        assert!(
            started_over.is_some_and(|after| after <= bound),
            "grown {grown_first} beats first: the middle of {counted} windows, rewritten in place, \
             was checked after {started_over:?} of {readings} readings of a file growing a window \
             a beat — the bound is {bound}"
        );
        assert_eq!(
            written,
            started_over.map(|after| after + 1),
            "its switch was not read on the next reading"
        );
        let attempt = stood.attempt().id;
        beat();
        let at = beat();
        assert_eq!(
            stood.switches_written(at + 1),
            [(attempt, "switch-mid".to_string())],
            "written other than once"
        );
    }
}

/// A switch is written for the ATTEMPT it was read under, and the same
/// worker's next attempt begins its own count (t-7153, R3): a worker
/// reports done and its pane is handed a second task — its summons time
/// stands, so a time filter on the summons would take the first attempt's
/// switch as the second's; a window restart then forgets the cursors and
/// the file is read again from its start. The first attempt's switch is
/// not written for the second; the second attempt's own switch lands once,
/// under the second. Through the real beat and the real ledger.
#[test]
fn a_switch_of_a_workers_earlier_attempt_is_never_written_as_its_next_attempts() {
    use crate::quota_wall::tests::{CLAUDE_FALLBACK, CLAUDE_PLAIN_RECORD};
    use zerocode_core::civil::iso_utc_of;
    const LEADER_TERM: u32 = 97_400;
    let stood = StoppedWorker::stand(LEADER_TERM, "", Vec::new());
    let switch = |key: &str, at_ms: i64| {
        CLAUDE_FALLBACK
            .replace("switch-1", key)
            .replace("2026-09-21T14:06:16.566Z", &iso_utc_of(at_ms))
    };
    std::fs::write(
        &stood.host.transcript,
        format!(
            "{CLAUDE_PLAIN_RECORD}\n{}\n",
            switch("switch-1", stood.began + 5_000)
        ),
    )
    .expect("the transcript");
    tick(&stood.host, &[], stood.began + 10_000);
    tick(&stood.host, &[], stood.began + 11_000);
    let first = stood.attempt();
    assert_eq!(
        stood.switches_written(stood.began + 11_001),
        [(first.id.clone(), "switch-1".to_string())]
    );

    let second = stood.handed_a_second_task(stood.began + 12_000);
    assert_ne!(second.id, first.id);

    // A restart forgets every cursor: the file is read again from its
    // start, and what it holds of the first attempt is not the second's.
    super::deviation_scans()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .clear();
    tick(&stood.host, &[], stood.began + 14_000);
    tick(&stood.host, &[], stood.began + 15_000);
    assert_eq!(
        stood.switches_written(stood.began + 15_001),
        [(first.id.clone(), "switch-1".to_string())],
        "the first attempt's switch was written as the second attempt's"
    );

    // The second attempt's own switch: once, under the second.
    stood.appends(&switch("switch-2", stood.began + 16_000));
    tick(&stood.host, &[], stood.began + 17_000);
    tick(&stood.host, &[], stood.began + 18_000);
    assert_eq!(
        stood.switches_written(stood.began + 18_001),
        [
            (first.id.clone(), "switch-1".to_string()),
            (second.id.clone(), "switch-2".to_string())
        ]
    );
}

/// A reading the ledger moved past between the scan and the fence is
/// written nowhere (t-7153, R3): the beat read the file under the first
/// attempt; before the ledger was asked, the worker reported done and its
/// pane was handed a second task. The reading carries the attempt it was
/// read under, and the ledger's fence — the real actor — writes it for
/// neither: not for the ended attempt, not for the one the worker carries
/// now. The second attempt's own reading, bound to it, lands.
#[test]
fn a_reading_the_ledger_moved_past_between_the_scan_and_the_fence_is_written_nowhere() {
    use crate::quota_wall::tests::{CLAUDE_FALLBACK, CLAUDE_PLAIN_RECORD};
    use crate::quota_wall::{ScanCursor, scan_fallbacks};
    use zerocode_core::civil::iso_utc_of;
    const LEADER_TERM: u32 = 97_500;
    let stood = StoppedWorker::stand(LEADER_TERM, "", Vec::new());
    let switch = |key: &str, at_ms: i64| {
        CLAUDE_FALLBACK
            .replace("switch-1", key)
            .replace("2026-09-21T14:06:16.566Z", &iso_utc_of(at_ms))
    };
    std::fs::write(
        &stood.host.transcript,
        format!(
            "{CLAUDE_PLAIN_RECORD}\n{}\n",
            switch("switch-1", stood.began + 5_000)
        ),
    )
    .expect("the transcript");
    let source = stood.host.transcript.to_string_lossy().into_owned();
    let first = stood.attempt();
    let held = super::runtime().expect("the runtime");
    let scan = scan_fallbacks(&stood.host.transcript, &ScanCursor::default());
    assert_eq!(scan.switches.len(), 1);
    let binding = super::SwitchBinding {
        worker: &stood.worker,
        dispatch: &first.id,
        source: &source,
        attempt_started_ms: first.started_ms,
    };
    let mut second = None;
    let (keep, waiting, moved) =
        super::record_scanned_switches(&binding, scan, ScanCursor::default(), &mut |switches| {
            // Between the reading and the fence: the attempt ends and the
            // pane is handed another task.
            second = Some(stood.handed_a_second_task(stood.began + 12_000));
            held.actor
                .model_deviations(switches, stood.began + 13_000)
                .map(|(told, _)| told)
        });
    let second = second.expect("the second attempt");
    assert!(!moved, "a reading of the ended attempt was written");
    assert_eq!(waiting, None, "the ledger answered; nothing waits");
    assert_eq!(
        stood.switches_written(stood.began + 13_001),
        Vec::<(String, String)>::new(),
        "a reading of the first attempt was written as the second's"
    );

    // The second attempt's own reading lands, bound to it.
    stood.appends(&switch("switch-2", stood.began + 14_000));
    let scan = scan_fallbacks(&stood.host.transcript, &keep);
    let binding = super::SwitchBinding {
        worker: &stood.worker,
        dispatch: &second.id,
        source: &source,
        attempt_started_ms: second.started_ms,
    };
    let (keep, waiting, moved) =
        super::record_scanned_switches(&binding, scan, keep.clone(), &mut |switches| {
            held.actor
                .model_deviations(switches, stood.began + 15_000)
                .map(|(told, _)| told)
        });
    assert!(moved);
    assert_eq!(waiting, None);
    assert_eq!(
        stood.switches_written(stood.began + 15_001),
        [(second.id.clone(), "switch-2".to_string())]
    );

    // The file swapped between the reading and the fence: the ledger learns
    // the worker's CLI now writes another conversation, and the reading of
    // the file it left is written nowhere.
    stood.appends(&switch("switch-3", stood.began + 16_000));
    let scan = scan_fallbacks(&stood.host.transcript, &keep);
    assert_eq!(scan.switches.len(), 1);
    let (_, waiting, moved) =
        super::record_scanned_switches(&binding, scan, keep.clone(), &mut |switches| {
            let (moved, _) = held
                .actor
                .worker_session_reported(
                    stood.host.worker_term,
                    zerocode_core::ProviderSession {
                        key: zerocode_core::provider_session::SessionKey::SessionId,
                        id: "another-conversation".to_string(),
                        transcript_path: Some(format!("{source}.another")),
                    },
                    stood.began + 16_500,
                )
                .expect("the session report");
            assert!(moved, "the ledger learned the new file");
            held.actor
                .model_deviations(switches, stood.began + 17_000)
                .map(|(told, _)| told)
        });
    assert!(!moved, "a reading of a file the worker left was written");
    assert_eq!(waiting, None);
    assert_eq!(
        stood.switches_written(stood.began + 17_001),
        [(second.id.clone(), "switch-2".to_string())],
        "a reading of another file was written as this conversation's"
    );
}

/// A worker's switches are read off the file the LEDGER names for its
/// conversation, and off the pane's own report only while the ledger has
/// heard none (t-7153, R3): the ledger's fence writes a switch only for
/// the file it names, so a file the pane reported first — a session the
/// ledger has not heard of yet — is not read, and once the ledger names it
/// it is read from its start and its switch lands, once. Read before the
/// ledger named it, the switch would have been refused at the fence and
/// its cursor moved past it for good.
#[test]
fn a_switch_is_read_off_the_file_the_ledger_names_and_lands_once_it_names_it() {
    use crate::quota_wall::tests::{CLAUDE_FALLBACK, CLAUDE_PLAIN_RECORD};
    use zerocode_core::civil::iso_utc_of;
    const LEADER_TERM: u32 = 97_800;
    let stood = StoppedWorker::stand(LEADER_TERM, "", Vec::new());
    let held = super::runtime().expect("the runtime");
    let named = |path: &std::path::Path, at: i64| {
        held.actor
            .worker_session_reported(
                stood.host.worker_term,
                zerocode_core::ProviderSession {
                    key: zerocode_core::provider_session::SessionKey::SessionId,
                    id: "d4e81823-3b9a-4bf1-a54b-b4366117b514".to_string(),
                    transcript_path: Some(path.to_string_lossy().into_owned()),
                },
                at,
            )
            .expect("the session report")
            .0
    };
    let elsewhere = stood._dir.path().join("elsewhere.jsonl");
    std::fs::write(&elsewhere, format!("{CLAUDE_PLAIN_RECORD}\n")).expect("the named file");
    assert!(
        named(&elsewhere, stood.began + 1_000),
        "the ledger named a file"
    );
    std::fs::write(
        &stood.host.transcript,
        format!(
            "{CLAUDE_PLAIN_RECORD}\n{}\n",
            CLAUDE_FALLBACK
                .replace("switch-1", "switch-pane")
                .replace("2026-09-21T14:06:16.566Z", &iso_utc_of(stood.began + 5_000))
        ),
    )
    .expect("the pane's file");
    tick(&stood.host, &[], stood.began + 10_000);
    tick(&stood.host, &[], stood.began + 11_000);
    assert_eq!(
        stood.switches_written(stood.began + 11_001),
        Vec::<(String, String)>::new()
    );

    assert!(named(&stood.host.transcript, stood.began + 12_000));
    tick(&stood.host, &[], stood.began + 13_000);
    tick(&stood.host, &[], stood.began + 14_000);
    assert_eq!(
        stood.switches_written(stood.began + 14_001),
        [(stood.attempt().id, "switch-pane".to_string())],
        "a switch read before the ledger named its file was lost at the fence"
    );
}

/// What waits on the ledger is bound to the file it was read from, and a
/// session that moves under the same attempt neither drops it nor lends it
/// the new file's cursor (t-7153, R3): the refused reading of file A is
/// asked first on the beat that finds the CLI writing file B, under its own
/// file; B is then read from its start, and its cursor is B's.
#[test]
fn a_waiting_reading_is_asked_under_its_own_file_when_the_session_moves() {
    use crate::quota_wall::ScanCursor;
    use crate::quota_wall::tests::{CLAUDE_FALLBACK, CLAUDE_PLAIN_RECORD};
    let dir = tempfile::tempdir().expect("a dir");
    let first = dir.path().join("first.jsonl");
    let second = dir.path().join("second.jsonl");
    let switch = |key: &str| CLAUDE_FALLBACK.replace("switch-1", key);
    std::fs::write(
        &first,
        format!("{CLAUDE_PLAIN_RECORD}\n{}\n", switch("switch-a")),
    )
    .expect("a");
    std::fs::write(
        &second,
        format!("{}\n{CLAUDE_PLAIN_RECORD}\n", switch("switch-b")),
    )
    .expect("b");
    let (first_source, second_source) = (
        first.to_string_lossy().into_owned(),
        second.to_string_lossy().into_owned(),
    );
    fn bound(source: &str) -> super::SwitchBinding<'_> {
        super::SwitchBinding {
            worker: "w-1",
            dispatch: "dp-1",
            source,
            attempt_started_ms: 0,
        }
    }
    let asked: std::cell::RefCell<Vec<(String, String)>> = std::cell::RefCell::new(Vec::new());
    let record = |refuse: bool| {
        let asked = &asked;
        move |switches: Vec<zerocode_core::orchestration::ModelDeviation>| {
            if refuse {
                return Err(super::RuntimeError::RecoveryRequired);
            }
            asked
                .borrow_mut()
                .extend(switches.into_iter().map(|one| (one.source, one.key)));
            Ok(true)
        }
    };

    let (held_at, waiting, _) = super::scan_switches_a_beat(
        &bound(&first_source),
        &first,
        ScanCursor::default(),
        None,
        &mut record(true),
    );
    assert_eq!(held_at, ScanCursor::default());
    assert!(waiting.is_some(), "the refused reading was let go");

    // The session moved: the beat reads B with a fresh cursor, and asks the
    // waiting reading of A first, under A.
    let (keep, waiting, moved) = super::scan_switches_a_beat(
        &bound(&second_source),
        &second,
        ScanCursor::default(),
        waiting,
        &mut record(false),
    );
    assert!(moved);
    assert_eq!(waiting, None);
    assert_eq!(
        *asked.borrow(),
        [
            (first_source.clone(), "switch-a".to_string()),
            (second_source.clone(), "switch-b".to_string())
        ],
        "the waiting reading was dropped, or asked under the new file"
    );
    assert_eq!(
        keep.offset,
        std::fs::metadata(&second).expect("size").len(),
        "the new file's cursor is not its own"
    );
}

/// A switch the ledger refused is held over and lands once the ledger can
/// hold it, even when the file it was read from is gone by then (t-7153,
/// R3): the disk refuses the beat's row; the file is replaced under its
/// path — as long as the cursor, a new switch first — and the disk comes
/// back; the next beat writes the refused switch, then reads the new file
/// from its start. Each switch once, through the real beat and the real
/// ledger. Before this the refused switch went with the old file.
#[test]
fn a_switch_the_ledger_refused_lands_once_after_the_file_it_was_read_from_is_replaced() {
    use crate::quota_wall::tests::{CLAUDE_FALLBACK, CLAUDE_PLAIN_RECORD, a_transcript_as_long_as};
    use zerocode_core::civil::iso_utc_of;
    const LEADER_TERM: u32 = 97_600;
    let stood = StoppedWorker::stand(LEADER_TERM, "", Vec::new());
    let switch = |key: &str, at_ms: i64| {
        CLAUDE_FALLBACK
            .replace("switch-1", key)
            .replace("2026-09-21T14:06:16.566Z", &iso_utc_of(at_ms))
    };
    let keys = |at: i64| -> Vec<String> {
        stood
            .switches_written(at)
            .into_iter()
            .map(|(_, key)| key)
            .collect()
    };
    std::fs::write(
        &stood.host.transcript,
        format!(
            "{CLAUDE_PLAIN_RECORD}\n{}\n",
            switch("switch-1", stood.began + 5_000)
        ),
    )
    .expect("the transcript");
    tick(&stood.host, &[], stood.began + 10_000);
    tick(&stood.host, &[], stood.began + 11_000);
    assert_eq!(keys(stood.began + 11_001), ["switch-1"]);
    let old_len = std::fs::metadata(&stood.host.transcript)
        .expect("size")
        .len();

    // The disk refuses the beat that reads the next switch.
    stood.appends(&switch("switch-refused", stood.began + 12_000));
    let connection = stood
        ._store
        .fault_connection_for_tests()
        .expect("fault connection");
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_switches
                    AFTER UPDATE OF revision ON orchestration_ledger_heads
                    BEGIN SELECT RAISE(ABORT, 'injected disk refusal'); END;",
        )
        .expect("a disk that refuses");
    tick(&stood.host, &[], stood.began + 13_000);
    connection
        .execute_batch("DROP TRIGGER refuse_switches;")
        .expect("the disk comes back");
    assert_eq!(
        keys(stood.began + 13_001),
        ["switch-1"],
        "the refused row landed"
    );

    // Replaced under its path before the next beat, as long as the cursor.
    let fresh = stood._dir.path().join("next.jsonl");
    std::fs::write(
        &fresh,
        a_transcript_as_long_as(&switch("switch-2", stood.began + 14_000), old_len),
    )
    .expect("the replacement");
    std::fs::rename(&fresh, &stood.host.transcript).expect("atomic replace");
    tick(&stood.host, &[], stood.began + 15_000);
    tick(&stood.host, &[], stood.began + 16_000);
    assert_eq!(
        keys(stood.began + 16_001),
        ["switch-1", "switch-refused", "switch-2"],
        "a switch the ledger refused was lost with the file it was read from"
    );
    tick(&stood.host, &[], stood.began + 17_000);
    assert_eq!(
        keys(stood.began + 17_001),
        ["switch-1", "switch-refused", "switch-2"],
        "written twice"
    );
}

/// A reading that waits on the ledger dies with its attempt (t-7153, R3):
/// the disk refuses the beat's row, the worker reports done and its pane is
/// handed a second task before the disk comes back — the waiting reading
/// was the first attempt's, and is written for neither the ended attempt
/// nor the new one; the new attempt's own switch lands once.
#[test]
fn a_waiting_reading_dies_with_its_attempt_and_is_never_the_next_attempts() {
    use crate::quota_wall::tests::{CLAUDE_FALLBACK, CLAUDE_PLAIN_RECORD};
    use zerocode_core::civil::iso_utc_of;
    const LEADER_TERM: u32 = 97_700;
    let stood = StoppedWorker::stand(LEADER_TERM, "", Vec::new());
    let switch = |key: &str, at_ms: i64| {
        CLAUDE_FALLBACK
            .replace("switch-1", key)
            .replace("2026-09-21T14:06:16.566Z", &iso_utc_of(at_ms))
    };
    std::fs::write(
        &stood.host.transcript,
        format!(
            "{CLAUDE_PLAIN_RECORD}\n{}\n",
            switch("switch-1", stood.began + 5_000)
        ),
    )
    .expect("the transcript");
    tick(&stood.host, &[], stood.began + 10_000);
    tick(&stood.host, &[], stood.began + 11_000);
    let first = stood.attempt();
    assert_eq!(
        stood.switches_written(stood.began + 11_001),
        [(first.id.clone(), "switch-1".to_string())]
    );

    stood.appends(&switch("switch-waiting", stood.began + 12_000));
    let connection = stood
        ._store
        .fault_connection_for_tests()
        .expect("fault connection");
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_switches
                    AFTER UPDATE OF revision ON orchestration_ledger_heads
                    BEGIN SELECT RAISE(ABORT, 'injected disk refusal'); END;",
        )
        .expect("a disk that refuses");
    tick(&stood.host, &[], stood.began + 13_000);
    connection
        .execute_batch("DROP TRIGGER refuse_switches;")
        .expect("the disk comes back");

    let second = stood.handed_a_second_task(stood.began + 14_000);
    tick(&stood.host, &[], stood.began + 15_000);
    tick(&stood.host, &[], stood.began + 16_000);
    assert_eq!(
        stood.switches_written(stood.began + 16_001),
        [(first.id.clone(), "switch-1".to_string())],
        "a reading of the ended attempt was written as the next attempt's"
    );
    stood.appends(&switch("switch-2", stood.began + 17_000));
    tick(&stood.host, &[], stood.began + 18_000);
    tick(&stood.host, &[], stood.began + 19_000);
    assert_eq!(
        stood.switches_written(stood.began + 19_001),
        [
            (first.id.clone(), "switch-1".to_string()),
            (second.id.clone(), "switch-2".to_string())
        ]
    );
}

/// A refused beat's switches wait, bound to their file, and are held before
/// anything more is read (t-7153, R3): the ledger refuses a beat's rows —
/// the cursor stays where the ledger holds rows of the old file and the
/// beat's reading waits beside it, and while it waits the beat reads
/// nothing more, so what waits is ever one beat's reading; the file is then
/// replaced under its path by one exactly as long as that cursor, a switch
/// first — the next beat asks the waiting reading first, and only once it
/// is held reads the new file from its start. Each switch lands once.
/// Before this the refused reading was let go to be read again from the
/// file, and the rotation took it with it.
#[test]
fn a_refused_readings_switches_wait_and_land_before_the_rotation_is_read() {
    use crate::quota_wall::ScanCursor;
    use crate::quota_wall::tests::{
        CLAUDE_FALLBACK, CLAUDE_PLAIN_RECORD, a_transcript_as_long_as, placed,
    };
    use std::io::Write;
    let dir = tempfile::tempdir().expect("a dir");
    let path = dir.path().join("session.jsonl");
    let switch = |key: &str| CLAUDE_FALLBACK.replace("switch-1", key);
    let replace = |content: &str| {
        let fresh = dir.path().join("session.jsonl.next");
        std::fs::write(&fresh, content).expect("the replacement");
        std::fs::rename(&fresh, &path).expect("atomic replace");
    };
    let append = |line: &str| {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("append");
        writeln!(file, "{line}").expect("write");
    };
    let source = path.to_string_lossy().into_owned();
    let binding = super::SwitchBinding {
        worker: "w-1",
        dispatch: "dp-1",
        source: &source,
        attempt_started_ms: 0,
    };
    let held: std::cell::RefCell<Vec<String>> = std::cell::RefCell::new(Vec::new());
    let beat = |cursor: &ScanCursor, pending: Option<super::PendingSwitches>, refuse: bool| {
        let (keep, pending, _) = super::scan_switches_a_beat(
            &binding,
            &path,
            cursor.clone(),
            pending,
            &mut |switches| {
                if refuse {
                    return Err(super::RuntimeError::RecoveryRequired);
                }
                held.borrow_mut()
                    .extend(switches.into_iter().map(|one| one.key));
                Ok(true)
            },
        );
        (keep, pending)
    };

    replace(&format!("{CLAUDE_PLAIN_RECORD}\n{}\n", switch("switch-1")));
    let (first, nothing) = beat(&ScanCursor::default(), None, false);
    let old_len = std::fs::metadata(&path).expect("size").len();
    assert_eq!(first.offset, old_len);
    assert_eq!(nothing, None);

    // A switch appended, and refused: the cursor stays in the old file and
    // the reading waits, bound to it.
    append(&switch("switch-refused"));
    let (refused, waiting) = beat(&first, None, true);
    assert_eq!(refused, first, "a refused beat moved the cursor");
    let waiting_keys = |waiting: &Option<super::PendingSwitches>| -> Vec<String> {
        waiting
            .as_ref()
            .map(|held| held.switches.iter().map(|one| one.key.clone()).collect())
            .unwrap_or_default()
    };
    assert_eq!(
        waiting_keys(&waiting),
        ["switch-refused"],
        "the refused reading was let go"
    );
    assert_eq!(*held.borrow(), ["switch-1"]);

    // Refused again: still one beat's reading waits, and nothing more is
    // read behind it — a switch written to the old file meanwhile is not
    // taken up until the waiting one is held.
    append(&switch("switch-later"));
    let (still, waiting) = beat(&refused, waiting, true);
    assert_eq!(still, first);
    assert_eq!(
        waiting_keys(&waiting),
        ["switch-refused"],
        "the beat read on while a reading waited"
    );

    // Replaced by a file as long as the cursor's offset, a switch first:
    // the waiting reading lands first, then the new file from its start.
    replace(&a_transcript_as_long_as(&switch("switch-2"), old_len));
    let (rotated, nothing) = beat(&refused, waiting, false);
    assert_eq!(rotated.offset, old_len);
    assert_ne!(rotated.file, first.file, "another file");
    assert_eq!(nothing, None);
    assert_eq!(
        *held.borrow(),
        ["switch-1", "switch-refused", "switch-2"],
        "a switch the ledger refused was lost with the file it was read from"
    );
    let (after, nothing) = beat(&rotated, None, false);
    assert_eq!(
        (placed(&after), nothing),
        (placed(&rotated), None),
        "an unchanged file moved the cursor"
    );
}

/// The whole road, attempt by attempt. The declaration reads back with its
/// ceiling; a continuation settles `submitted` only on the provider's own
/// prompt report and its receipt reaches the coordinator then; a later stop
/// is a new marker and a new attempt; a delivered continuation nobody
/// reported is `not_submitted` with its words possibly on the line, and that
/// marker is never typed at again — its silence is `went_quiet` news; past
/// `attempts_max` nothing is typed at all, and the silence is news again.
#[test]
fn a_continuation_settles_on_the_prompt_report_counts_attempts_and_stops_at_the_ceiling() {
    use crate::quota_wall::tests::{CLAUDE_PLAIN_RECORD, CLAUDE_TURN_DURATION};
    use zerocode_core::orchestration::{RESUME_LINE, RESUME_POLICY};
    const WORKER_TERM: u32 = 96_101;
    let stood = StoppedWorker::stand(96_100, "--on-transient-error resume", Vec::new());
    let shown = stood.json("run-show", stood.began + 4);
    assert_eq!(shown["handover"]["onTransientError"]["action"], "resume");
    assert_eq!(
        shown["handover"]["onTransientError"]["attemptsMax"],
        RESUME_POLICY.attempts_max
    );
    assert!(shown["handover"]["onQuotaWall"].is_null(), "{shown}");
    let at = |ms: i64| stood.began + ms;
    let spacing = RESUME_POLICY.retry_after_ms;
    let stop = |key: &str, when: i64| {
        let record = transient_record_keyed(key);
        stood.stops_on(
            &[CLAUDE_PLAIN_RECORD, record.as_str(), CLAUDE_TURN_DURATION],
            when,
        );
    };
    let receipts = |when: i64| stood.news("resumed", when);

    // ① The first stop: typed once, not yet settled, not yet news.
    stop("key-1", at(9_000));
    tick(&stood.host, &[], at(10_000));
    assert_eq!(
        stood.host.typed_at(WORKER_TERM),
        vec![RESUME_LINE.to_string(), "\r".to_string()]
    );
    assert!(
        receipts(at(10_001)).is_empty(),
        "an unsettled row was delivered"
    );
    tick(&stood.host, &[], at(11_000));
    assert_eq!(
        stood.host.typed_at(WORKER_TERM).len(),
        2,
        "typed twice in flight"
    );
    // The provider's prompt report: settled, delivered, the worker at work.
    super::pane_prompt_submitted(WORKER_TERM);
    *stood.host.busy.lock().unwrap() = true;
    tick(&stood.host, &[], at(12_000));
    let settled = receipts(at(12_001));
    assert_eq!(settled.len(), 1, "{settled:?}");
    assert_eq!(settled[0]["status"], "submitted");
    assert_eq!(settled[0]["typed"], true);
    assert_eq!(settled[0]["attempt"], 1);
    assert_eq!(settled[0]["attemptsMax"], RESUME_POLICY.attempts_max);
    assert_eq!(settled[0]["marker"]["key"], "key-1");
    assert_eq!(settled[0]["marker"]["source"], "transcript");
    assert_eq!(settled[0]["line"], RESUME_LINE);
    assert!(stood.news("went_quiet", at(12_002)).is_empty());

    // ② A later stop is a new marker: attempt 2, and nobody reports it.
    stop("key-2", at(12_000 + spacing));
    tick(&stood.host, &[], at(13_000 + spacing));
    assert_eq!(stood.host.typed_at(WORKER_TERM).len(), 4);
    tick(&stood.host, &[], at(14_000 + spacing));
    let budget = i64::try_from(zerocode_pty::ready::SUBMIT_ACK_TIMEOUT.as_millis()).unwrap();
    tick(&stood.host, &[], at(14_000 + spacing + budget));
    let settled = receipts(at(14_001 + spacing + budget));
    assert_eq!(settled.len(), 2, "{settled:?}");
    assert_eq!(settled[1]["status"], "not_submitted");
    assert_eq!(settled[1]["typed"], true);
    assert_eq!(settled[1]["attempt"], 2);
    assert!(settled[1].get("ceilingReached").is_none(), "{}", settled[1]);
    // The same marker, its words maybe on the line: never typed at again,
    // however long it stands — the silence is news instead.
    tick(&stood.host, &[], at(20_000 + 2 * spacing));
    assert_eq!(
        stood.host.typed_at(WORKER_TERM).len(),
        4,
        "a marker was typed at twice"
    );
    assert_eq!(stood.news("went_quiet", at(20_001 + 2 * spacing)).len(), 1);

    // ③ The third stop is the table's last try, and says so.
    stop("key-3", at(30_000 + 3 * spacing));
    tick(&stood.host, &[], at(31_000 + 3 * spacing));
    assert_eq!(stood.host.typed_at(WORKER_TERM).len(), 6);
    tick(&stood.host, &[], at(32_000 + 3 * spacing));
    tick(&stood.host, &[], at(32_000 + 3 * spacing + budget));
    let settled = receipts(at(32_001 + 3 * spacing + budget));
    assert_eq!(settled.len(), 3, "{settled:?}");
    assert_eq!(settled[2]["attempt"], RESUME_POLICY.attempts_max);
    assert_eq!(settled[2]["ceilingReached"], true);
    assert!(
        settled[2]["next"]
            .as_str()
            .is_some_and(|next| next.contains("went_quiet"))
    );

    // ④ Past the ceiling a new stop types nothing; it is a silence.
    stop("key-4", at(40_000 + 4 * spacing));
    tick(&stood.host, &[], at(41_000 + 4 * spacing));
    assert_eq!(
        stood.host.typed_at(WORKER_TERM).len(),
        6,
        "typed past the ceiling"
    );
    assert_eq!(receipts(at(41_001 + 4 * spacing)).len(), 3);
    let quiet = stood.news("went_quiet", at(41_002 + 4 * spacing));
    assert!(!quiet.is_empty(), "the ceiling left no news");
}

/// Nothing is typed where nothing was declared, where the pane holds a
/// question of its agent's own, where somebody already answered the stop
/// with a prompt, or where a person's hand ended the turn — each is the
/// silence it always was. And the wall's road wins over the continuation:
/// a claude record at its limit with the provider's number at the wall is
/// `quota_walled` news, typed at by nobody.
#[test]
fn a_stop_nobody_declared_one_a_person_or_a_question_holds_and_a_wall_are_never_typed_at() {
    use crate::quota_wall::tests::{
        CLAUDE_PLAIN_RECORD, CLAUDE_POINTER_PROMPT, CLAUDE_TRANSIENT_RECORD, CLAUDE_TURN_DURATION,
        CLAUDE_WALL_RECORD,
    };
    let stopped = [
        CLAUDE_PLAIN_RECORD,
        CLAUDE_TRANSIENT_RECORD,
        CLAUDE_TURN_DURATION,
    ];
    let silent = |stood: &StoppedWorker, why: &str| {
        tick(&stood.host, &[], stood.began + 10_000);
        assert!(
            stood.host.typed_at(stood.host.worker_term).is_empty(),
            "{why}: a continuation was typed"
        );
        assert!(
            stood.news("resumed", stood.began + 10_001).is_empty(),
            "{why}"
        );
        assert_eq!(
            stood.news("went_quiet", stood.began + 10_002).len(),
            1,
            "{why}: the stop was not a silence"
        );
    };
    {
        let stood = StoppedWorker::stand(96_200, "", Vec::new());
        stood.stops_on(&stopped, stood.began + 9_000);
        silent(&stood, "no order");
    }
    {
        let stood = StoppedWorker::stand(96_210, "--on-quota-wall codex", Vec::new());
        stood.stops_on(&stopped, stood.began + 9_000);
        silent(&stood, "a wall order only");
    }
    {
        let stood = StoppedWorker::stand(96_220, "--on-transient-error resume", Vec::new());
        stood.stops_on(&stopped, stood.began + 9_000);
        // Its own question parked in the composer: the hook said so.
        super::pane_turn_began(stood.host.worker_term, stood.began + 9_000);
        silent(&stood, "a question of its own");
    }
    {
        let stood = StoppedWorker::stand(96_230, "--on-transient-error resume", Vec::new());
        stood.stops_on(
            &[
                CLAUDE_TRANSIENT_RECORD,
                CLAUDE_TURN_DURATION,
                CLAUDE_POINTER_PROMPT,
            ],
            stood.began + 9_000,
        );
        silent(&stood, "a prompt after the stop");
    }
    {
        let stood = StoppedWorker::stand(96_240, "--on-transient-error resume", Vec::new());
        std::fs::write(&stood.host.transcript, stopped.join("\n")).expect("the transcript");
        super::pane_turn_ended(
            stood.host.worker_term,
            stood.began + 8_999,
            true,
            stood.began + 9_000,
        );
        silent(&stood, "a turn a person interrupted");
    }
    let began = clock();
    let gauge = |used: u8| {
        vec![(
            "claude",
            usage_snapshot(
                "claude",
                Some((used, Some(began + 60 * 60_000))),
                None,
                began - 60_000,
            ),
        )]
    };
    let stood = StoppedWorker::stand(96_250, "--on-transient-error resume", gauge(40));
    stood.window.set_usage(gauge(98));
    stood.stops_on(
        &[CLAUDE_PLAIN_RECORD, CLAUDE_WALL_RECORD],
        stood.began + 9_000,
    );
    tick(&stood.host, &[], stood.began + 10_000);
    assert!(
        stood.host.typed_at(stood.host.worker_term).is_empty(),
        "typed at a wall"
    );
    assert_eq!(stood.news("quota_walled", stood.began + 10_001).len(), 1);
    assert!(stood.news("went_quiet", stood.began + 10_002).is_empty());
}

/// A door that typed NOTHING is the one outcome that lets the same stop be
/// typed at again — after the table's spacing, as the attempt's next try.
#[test]
fn a_continuation_the_door_never_typed_is_tried_again_after_the_spacing() {
    use crate::quota_wall::tests::{
        CLAUDE_PLAIN_RECORD, CLAUDE_TRANSIENT_RECORD, CLAUDE_TURN_DURATION,
    };
    use zerocode_core::orchestration::{RESUME_LINE, RESUME_POLICY};
    let stood = StoppedWorker::stand(96_300, "--on-transient-error resume", Vec::new());
    let term = stood.host.worker_term;
    let at = |ms: i64| stood.began + ms;
    stood.stops_on(
        &[
            CLAUDE_PLAIN_RECORD,
            CLAUDE_TRANSIENT_RECORD,
            CLAUDE_TURN_DURATION,
        ],
        at(9_000),
    );
    *stood.host.refusing.lock().unwrap() = true;
    tick(&stood.host, &[], at(10_000));
    tick(&stood.host, &[], at(11_000));
    let refused = stood.news("resumed", at(11_001));
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0]["status"], "not_submitted");
    assert_eq!(refused[0]["typed"], false);
    // Inside the spacing the stop is a silence, and nothing is typed.
    tick(&stood.host, &[], at(12_000));
    assert!(stood.host.typed_at(term).is_empty());
    *stood.host.refusing.lock().unwrap() = false;
    tick(&stood.host, &[], at(10_000 + RESUME_POLICY.retry_after_ms));
    assert_eq!(
        stood.host.typed_at(term),
        vec![RESUME_LINE.to_string(), "\r".to_string()],
        "the same stop was not typed at once the door would take it"
    );
}

/* ---- the stall-cause question (t-4538) -------------------------------- */

/// A claude pane that finished and never reported, as a released screen on
/// this machine shows one — the summary's end, the turn footer, the empty
/// composer and the mode line — under a scrollback longer than any cap, with
/// a credential on the screen.
fn a_finished_screen_under_a_long_scrollback() -> String {
    let mut screen = String::new();
    for line in 0..400 {
        screen.push_str(&format!(
            "  running cargo test -p zerocode-shell --bins, pass {line} of the gate    \n"
        ));
    }
    screen.push_str("  export OPENAI_API_KEY=sk-proj-SCREEN-SENTINEL\n");
    screen.push_str(
        "  All runs had no extra invokes, the same first card node, focus kept and no errors.\n",
    );
    screen.push_str("\n✻ Worked for 27m 0s · done 2:11 AM\n\n");
    screen.push_str("❯ \n");
    screen.push_str("  ⏵⏵ bypass permissions on (shift+tab to cycle)\n\n\n");
    screen
}

/// A tool result in the transcript tail carrying a credential, in the shape
/// Claude Code writes one.
const CLAUDE_TOOL_RESULT_WITH_A_TOKEN: &str = r#"{"parentUuid":"v","isSidechain":false,"type":"user","uuid":"t","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"curl -H 'Authorization: Bearer TRANSCRIPT-SENTINEL' https://api.test"}]}}"#;

/// The endpoint's answer: `chosen` with the rest of the causes spread evenly.
fn a_cause_answer(chosen: &str) -> String {
    use zerocode_core::stall_cause::Cause;
    let rest = 0.1 / 6.0;
    let probabilities: serde_json::Map<String, serde_json::Value> = Cause::ALL
        .iter()
        .map(|cause| {
            let share = if cause.word() == chosen { 0.9 } else { rest };
            (cause.word().to_string(), serde_json::json!(share))
        })
        .collect();
    serde_json::json!({
        "model": "jev-1.13.0",
        "answers": {
            "cause": {
                "type": "choice",
                "choice": chosen,
                "probabilities": probabilities,
                "confidence": 0.62,
            }
        },
        "usage": { "input_tokens": 1800, "output_tokens": 0 },
    })
    .to_string()
}

impl StoppedWorker {
    /// Point this window's wire at `endpoint`, with zo's settings — `mode` for
    /// the stall row and `consented` as the one workspace root — in a folder of
    /// the case's own. Answers that folder: zo's config home.
    fn asks_jev(
        &self,
        endpoint: &crate::systemone::tests::Endpoint,
        mode: zerocode_core::jev::JevMode,
        consented: &str,
    ) -> tempfile::TempDir {
        use zerocode_core::jev::{SMART_SETTINGS_KEY, STALL};
        let home = tempfile::tempdir().expect("a zo home");
        let settings = home.path().join("settings.json");
        std::fs::write(
            &settings,
            serde_json::json!({
                SMART_SETTINGS_KEY: {
                    STALL.setting: mode.key(),
                    "jev": { "workspaces": [consented] },
                }
            })
            .to_string(),
        )
        .expect("zo's settings");
        *self.host.jev.lock().unwrap() = Some((endpoint.base(), settings));
        home
    }

    /// The stall ledger's rows under zo's config home `home`, oldest first.
    fn stall_rows(home: &tempfile::TempDir) -> Vec<serde_json::Value> {
        Self::rows_of(home, &zerocode_core::jev::STALL)
    }

    /// One seat's ledger rows under zo's config home `home`, oldest first.
    fn rows_of(
        home: &tempfile::TempDir,
        seat: &zerocode_core::jev::JevUse,
    ) -> Vec<serde_json::Value> {
        let path = home
            .path()
            .join(zerocode_core::jev::count::REQUESTS_DIR)
            .join(seat.ledger);
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).expect("a json row"))
            .collect()
    }

    /// [`Self::asks_jev`] for any seat: point the wire at `endpoint` with
    /// `seat` at `mode` and `consented` as the one workspace root, in a home
    /// of the case's own.
    fn asks_jev_for(
        &self,
        seat: &zerocode_core::jev::JevUse,
        endpoint: &crate::systemone::tests::Endpoint,
        mode: zerocode_core::jev::JevMode,
        consented: &str,
    ) -> tempfile::TempDir {
        use zerocode_core::jev::SMART_SETTINGS_KEY;
        let home = tempfile::tempdir().expect("a zo home");
        let settings = home.path().join("settings.json");
        std::fs::write(
            &settings,
            serde_json::json!({
                SMART_SETTINGS_KEY: {
                    seat.setting: mode.key(),
                    "jev": { "workspaces": [consented] },
                }
            })
            .to_string(),
        )
        .expect("zo's settings");
        *self.host.jev.lock().unwrap() = Some((endpoint.base(), settings));
        home
    }

    /// Re-point the wire at another endpoint, keeping the home and settings.
    fn answers_from(&self, endpoint: &crate::systemone::tests::Endpoint) {
        let mut held = self.host.jev.lock().unwrap();
        if let Some((base, _)) = held.as_mut() {
            *base = endpoint.base();
        }
    }

    /// Everything the host was asked to send at the worker's pane, in order.
    fn sent_at_worker(&self) -> Vec<String> {
        self.host
            .sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .iter()
            .filter(|(term, _)| *term == self.host.worker_term)
            .map(|(_, text)| text.clone())
            .collect()
    }
}

/// The request body a heard request carried.
fn heard_body(request: &str) -> serde_json::Value {
    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("a request body");
    serde_json::from_str(body).expect("a json body")
}

/// A silence the marker table cannot name, under a person's `shadow`: Jev is
/// asked once however many beats see it, the answer is a row beside the
/// silence, and the silence is still the `went_quiet` news it always was.
/// What left the machine carried no credential line and fit its caps, and
/// the newest words of the screen were the ones kept.
#[test]
fn an_unmarked_stall_is_asked_about_once_and_recorded_beside_the_silence() {
    use crate::quota_wall::tests::{CLAUDE_PLAIN_RECORD, CLAUDE_TURN_DURATION};
    use zerocode_core::jev::{JevMode, STALL_SCREEN_BYTE_CAP, STALL_TRANSCRIPT_BYTE_CAP};
    let stood = StoppedWorker::stand(96_400, "", Vec::new());
    let endpoint = crate::systemone::tests::Endpoint::serving(
        "HTTP/1.1 200 OK",
        a_cause_answer("finished_without_report"),
        0,
    );
    let home = stood.asks_jev(&endpoint, JevMode::Shadow, "/wt");
    *stood.host.screen.lock().unwrap() = a_finished_screen_under_a_long_scrollback();
    stood.stops_on(
        &[
            CLAUDE_PLAIN_RECORD,
            CLAUDE_TOOL_RESULT_WITH_A_TOKEN,
            CLAUDE_PLAIN_RECORD,
            CLAUDE_TURN_DURATION,
        ],
        stood.began + 9_000,
    );
    for beat in [10_000, 11_000, 12_000] {
        tick(&stood.host, &[], stood.began + beat);
    }

    let heard = endpoint.asked();
    assert_eq!(heard.len(), 1, "asked {} times", heard.len());
    assert!(
        !heard[0].contains("SENTINEL"),
        "a credential left the machine: {}",
        heard[0]
    );
    let sent = heard_body(&heard[0]);
    let screen = sent["state"]["screen"].as_str().expect("the screen");
    let transcript = sent["state"]["transcript"]
        .as_str()
        .expect("the transcript");
    assert!(screen.len() <= STALL_SCREEN_BYTE_CAP, "{}", screen.len());
    assert!(
        transcript.len() <= STALL_TRANSCRIPT_BYTE_CAP,
        "{}",
        transcript.len()
    );
    assert!(
        screen.ends_with("⏵⏵ bypass permissions on (shift+tab to cycle)"),
        "the newest lines were not the ones kept: {screen}"
    );
    assert!(screen.contains("✻ Worked for 27m 0s · done 2:11 AM"));
    assert!(transcript.contains("Continuing with the migration."));

    let rows = StoppedWorker::stall_rows(&home);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row["worker"], stood.worker.as_str());
    assert!(row["dispatch"].as_str().is_some_and(|id| !id.is_empty()));
    assert_eq!(row["agent"], "claude");
    assert_eq!(row["mode"], JevMode::Shadow.key());
    assert_eq!(row["outcome"], "answered");
    // The request carries the words the seat's row asks, so its label joins
    // the series of those words and no older one (t-6877; t-9087 moved them
    // to version 4).
    assert_eq!(
        row[zerocode_core::jev::summary::RUBRIC_VERSION.canonical],
        zerocode_core::jev::STALL.rubric_version
    );
    assert_eq!(row["cause"], "finished_without_report");
    assert_eq!(
        row[zerocode_core::jev::summary::MODEL.canonical],
        "jev-1.13.0",
        "the version that answered"
    );
    assert_eq!(row["confidence"], 0.62);
    assert_eq!(
        row["probabilities"].as_object().map(|all| all.len()),
        Some(zerocode_core::stall_cause::Cause::ALL.len()),
        "one probability per cause the rubric offers"
    );
    assert_eq!(row["requests"], 1);
    assert!(
        row["redactedLines"]
            .as_u64()
            .is_some_and(|lines| lines >= 2),
        "{row}"
    );
    assert!(
        row["requestBytes"].as_u64().is_some_and(|bytes| bytes > 0),
        "{row}"
    );
    assert!(row["elapsedMs"].is_u64(), "{row}");
    assert!(
        row.get("screen").is_none() && row.get("transcript").is_none(),
        "a row keeps no words: {row}"
    );

    assert_eq!(
        stood.news("went_quiet", stood.began + 12_001).len(),
        1,
        "shadow changed what the coordinator hears"
    );
}

/// Nothing is asked where nothing should be: a person's `off`, and a silence
/// the marker table already names — a transient error ending the record (with
/// no order to resume it) and the wall's own words (with no provider number to
/// make it news). Each stays the silence it was, and no row is written.
#[test]
fn off_and_a_silence_the_marker_table_names_ask_nothing() {
    use crate::quota_wall::tests::{
        CLAUDE_PLAIN_RECORD, CLAUDE_TRANSIENT_RECORD, CLAUDE_TURN_DURATION, CLAUDE_WALL_RECORD,
    };
    use zerocode_core::jev::JevMode;
    let endpoint =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_cause_answer("unknown"), 0);
    for (leader, mode, last, why) in [
        (
            96_410,
            JevMode::Off,
            vec![CLAUDE_PLAIN_RECORD, CLAUDE_TURN_DURATION],
            "off",
        ),
        (
            96_420,
            JevMode::Shadow,
            vec![
                CLAUDE_PLAIN_RECORD,
                CLAUDE_TRANSIENT_RECORD,
                CLAUDE_TURN_DURATION,
            ],
            "a transient error",
        ),
        (
            96_430,
            JevMode::Shadow,
            vec![CLAUDE_PLAIN_RECORD, CLAUDE_WALL_RECORD],
            "the wall's words",
        ),
    ] {
        let stood = StoppedWorker::stand(leader, "", Vec::new());
        let home = stood.asks_jev(&endpoint, mode, "/wt");
        *stood.host.screen.lock().unwrap() = a_finished_screen_under_a_long_scrollback();
        stood.stops_on(&last, stood.began + 9_000);
        tick(&stood.host, &[], stood.began + 10_000);
        tick(&stood.host, &[], stood.began + 11_000);
        assert!(endpoint.asked().is_empty(), "{why}: Jev was asked");
        assert!(
            StoppedWorker::stall_rows(&home).is_empty(),
            "{why}: a row was written"
        );
    }
}

/// A workspace the person never consented to sends nothing — the door refuses
/// before a socket opens — and the refusal is the row.
#[test]
fn a_stall_in_a_workspace_nobody_consented_to_writes_only_its_refusal() {
    use crate::quota_wall::tests::{CLAUDE_PLAIN_RECORD, CLAUDE_TURN_DURATION};
    use zerocode_core::jev::JevMode;
    use zerocode_core::jev::door::Refused;
    let stood = StoppedWorker::stand(96_440, "", Vec::new());
    let endpoint =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_cause_answer("unknown"), 0);
    let home = stood.asks_jev(&endpoint, JevMode::Auto, "/elsewhere");
    *stood.host.screen.lock().unwrap() = a_finished_screen_under_a_long_scrollback();
    stood.stops_on(
        &[CLAUDE_PLAIN_RECORD, CLAUDE_TURN_DURATION],
        stood.began + 9_000,
    );
    tick(&stood.host, &[], stood.began + 10_000);
    tick(&stood.host, &[], stood.began + 11_000);
    assert!(endpoint.asked().is_empty(), "nothing left the door");
    let rows = StoppedWorker::stall_rows(&home);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["outcome"], Refused::NotConsented.token());
    assert_eq!(rows[0]["requests"], 0);
    assert_eq!(rows[0]["mode"], JevMode::Auto.key());
    assert!(rows[0].get("cause").is_none(), "{}", rows[0]);
}

/// What the coordinator did next is written beside the answer, on the beat
/// after it happened: nothing while the window is open and nothing has, then
/// the coordinator's mail, keyed to the question and timed from it — once.
#[test]
fn what_the_coordinator_did_next_is_written_beside_the_answer() {
    use crate::quota_wall::tests::{CLAUDE_PLAIN_RECORD, CLAUDE_TURN_DURATION};
    use zerocode_core::jev::JevMode;
    let stood = StoppedWorker::stand(96_450, "", Vec::new());
    let endpoint = crate::systemone::tests::Endpoint::serving(
        "HTTP/1.1 200 OK",
        a_cause_answer("finished_without_report"),
        0,
    );
    let home = stood.asks_jev(&endpoint, JevMode::Shadow, "/wt");
    *stood.host.screen.lock().unwrap() = a_finished_screen_under_a_long_scrollback();
    stood.stops_on(
        &[CLAUDE_PLAIN_RECORD, CLAUDE_TURN_DURATION],
        stood.began + 9_000,
    );
    let at = |ms: i64| stood.began + ms;
    tick(&stood.host, &[], at(10_000));
    tick(&stood.host, &[], at(11_000));
    let rows = StoppedWorker::stall_rows(&home);
    assert_eq!(rows.len(), 1, "nothing followed yet: {rows:?}");
    let asked = &rows[0];
    assert_eq!(asked["outcome"], "answered");

    let said = stood.verb(
        &format!(
            "send --to worker:{} --type status --body nudge",
            stood.worker
        ),
        at(60_000),
    );
    assert_eq!(said.exit_code, 0, "{}", said.stderr);
    tick(&stood.host, &[], at(61_000));
    tick(&stood.host, &[], at(62_000));
    let rows = StoppedWorker::stall_rows(&home);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let label = &rows[1];
    assert_eq!(label["label"], asked["stall"]);
    assert_eq!(label["worker"], stood.worker.as_str());
    assert_eq!(label["dispatch"], asked["dispatch"]);
    assert_eq!(label["followed"], "mail");
    assert_eq!(label["followedAtMs"], at(60_000));
    assert_eq!(label["afterMs"], 50_000);
    assert_eq!(endpoint.asked().len(), 1, "the label asked Jev again");
}

#[test]
fn the_beat_reports_one_missing_pane_event_without_settling_any_work() {
    const LEADER_TERM: u32 = 83_000;
    const WORKER_TERM: u32 = 83_001;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    let _reconcile = ReconcilePanesHere::begin();
    let team = format!("team-pane-news-{LEADER_TERM}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKER_TERM);
    let host = Counting::default();
    let seat = zerocode_core::agent_teams::LEADER_PANE;

    // The observation stamp may move; the lifecycle must not. Keep only
    // the worker/task/dispatch fields this round promises not to repair.
    let lifecycle = || {
        let rows = the_rows();
        let held = rows
            .workers
            .iter()
            .find(|one| one.run == run_id && one.id == worker)
            .expect("the worker");
        let dispatch_id = held.dispatch.as_deref().expect("the open dispatch");
        let dispatch = rows
            .dispatches
            .iter()
            .find(|one| one.run == run_id && one.id == dispatch_id)
            .expect("the dispatch");
        let task = rows
            .tasks
            .iter()
            .find(|one| one.run == run_id && one.id == dispatch.task)
            .expect("the task");
        format!(
            "{:?}|{:?}|{:?}|{:?}",
            held.state, held.dispatch, dispatch.ended_ms, task.status
        )
    };
    let before = lifecycle();

    let read_news = |now_ms: i64| -> Vec<serde_json::Value> {
        let said = run(
            &host,
            Vec::new(),
            &team,
            seat,
            TEST_CAPABILITY,
            &words("check --peek --types status"),
            now_ms,
        );
        assert_eq!(said.exit_code, 0, "{}", said.stderr);
        let mail: serde_json::Value =
            serde_json::from_str(&said.stdout).expect("the coordinator inbox");
        mail["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .filter_map(|message| {
                let body =
                    serde_json::from_str::<serde_json::Value>(message["body"].as_str()?).ok()?;
                (body["reason"] == "pane_missing").then(|| {
                    serde_json::json!({
                        "from": message["from"],
                        "source": message["source"],
                        "trust": message["trust"],
                        "body": body,
                    })
                })
            })
            .collect()
    };

    let began = clock();
    for beat in 1..MISSING_PANE_CONFIRMATIONS {
        tick(&host, &[], began + i64::from(beat) * 1_000);
    }
    assert!(
        read_news(began + 4_500).is_empty(),
        "a transient absence was reported before five probes"
    );

    let first_missing = began + 1_000;
    tick(
        &host,
        &[],
        began + i64::from(MISSING_PANE_CONFIRMATIONS) * 1_000,
    );
    let news = read_news(began + 5_500);
    assert_eq!(
        news.len(),
        1,
        "the confirmed contradiction was not one event"
    );
    let told = &news[0];
    assert_eq!(told["from"], zerocode_core::orchestration::LEDGER_ITSELF);
    assert_eq!(told["source"], "ledger");
    assert_eq!(told["trust"], "observation");
    assert_eq!(told["body"]["workerId"], worker);
    assert_eq!(told["body"]["team"], team);
    assert_eq!(told["body"]["pane"], pane);
    assert_eq!(told["body"]["missingSinceMs"], first_missing);
    assert_eq!(
        lifecycle(),
        before,
        "reporting the contradiction settled work"
    );

    // Still absent is still the same event, however many beats say so.
    tick(&host, &[], began + 6_000);
    tick(&host, &[], began + 7_000);
    assert_eq!(read_news(began + 7_500).len(), 1, "one event flooded mail");

    // A seen pane closes the episode. Its next five misses are new news.
    host.set_pane(WORKER_TERM, true);
    tick(&host, &[], began + 8_000);
    host.set_pane(WORKER_TERM, false);
    for beat in 9..=13 {
        tick(&host, &[], began + beat * 1_000);
    }
    let news = read_news(began + 13_500);
    assert_eq!(news.len(), 2, "a returned pane never opened a new event");
    assert_eq!(news[1]["body"]["missingSinceMs"], began + 9_000);
    assert_eq!(lifecycle(), before, "the second report settled work");
}

/// How many beats have actually reached the ledger, ever.
///
/// The tick guard is defence in depth — a beat that got past it would be
/// refused again inside `run` — and defence in depth is exactly the kind of
/// line a test cannot see the outside of. Counted where the beat begins so
/// that "it did not run" is a fact and not an inference from a zero that
/// would be zero either way.
static BEATS_BEGUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Called by the road, once, when a beat is certainly under way.
pub(super) fn a_beat_has_begun() {
    BEATS_BEGUN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// How many callers have gone to sleep on a wait, ever.
///
/// Counted rather than flagged: a flag set and cleared while somebody was
/// looking is a signal they sleep straight through, and the same reasoning
/// the road itself uses for its mail bell.
static WAITS_BEGUN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Called by the road, once, when a caller is certainly asleep.
pub(super) fn a_wait_has_begun() {
    WAITS_BEGUN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

/// The waits run one at a time, so the count above means this test's
/// waiter and not somebody else's. The team table and the ledger are
/// process-wide here and the beat tests already take this precaution.
fn one_wait_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static TURNS: Mutex<()> = Mutex::new(());
    TURNS.lock().unwrap_or_else(|held| held.into_inner())
}

/// Block until a waiter has actually gone to sleep.
///
/// This is the difference between a test that proves something and a test
/// that hopes: a message posted before the wait begins is answered by the
/// FIRST look, and the road never reaches the loop where the bug lived.
fn until_a_wait_has_begun(before: u64) {
    const GIVING_UP_AT: std::time::Duration = std::time::Duration::from_secs(10);
    let began = std::time::Instant::now();
    while WAITS_BEGUN.load(std::sync::atomic::Ordering::SeqCst) == before {
        assert!(
            began.elapsed() < GIVING_UP_AT,
            "no caller reached the wait at all — the message below would \
                 have been answered by the first look, proving nothing"
        );
        std::thread::yield_now();
    }
}

/// An argv the way a shim hands one over.
/// A command line, with the retry name the road requires if it changes
/// anything and the line did not say one.
///
/// Every test here speaks whole verbs, so this is where a real caller's
/// obligation belongs. Named from a counter rather than a constant: one
/// shared name would make the second verb a replay of the first.
fn words(line: &str) -> Vec<String> {
    static NAMED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let mut argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
    if zerocode_core::orchestration::needs_a_retry_name(&argv) {
        let at = NAMED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        argv.push("--retry-request".to_string());
        argv.push(format!("test-{at}"));
    }
    argv
}

/// A worker's command line is the launch that window would make.
///
/// The pin under the comment on [`Catalog`]: the permission answer a person
/// stored is the permission answer a summoned worker gets, and the default
/// is reached only by a person who never said otherwise.
#[test]
fn a_summoned_worker_launches_on_the_terms_its_owner_set() {
    let plain = Catalog::new(Vec::new());
    let loose = plain
        .command_for("claude", "", &[])
        .expect("claude is in the catalog");
    assert_eq!(
        split_command_line(&loose).expect("a well-formed line"),
        std::iter::once("claude".to_string())
            .chain(launch_plan("claude", None).args)
            .collect::<Vec<_>>(),
        "a worker's line drifted from the launch plan's"
    );

    // And a person who wrote their own arguments is obeyed, defaults and
    // all — including a person who emptied the field, which is an answer.
    let held = Catalog::new(vec![(
        "claude".to_string(),
        LaunchOverride {
            args: Some(String::new()),
            env: None,
        },
    )]);
    let careful = held
        .command_for("claude", "", &[])
        .expect("claude is in the catalog");
    assert_eq!(
        split_command_line(&careful).expect("a well-formed line"),
        vec!["claude".to_string()],
        "an emptied args field was overruled by the measured default"
    );
    assert_ne!(
        careful, loose,
        "the override changed nothing, so the person was not heard"
    );
}

#[test]
fn a_restarted_claude_worker_keeps_the_name_it_was_summoned_under() {
    let peer_name = "zc-r7-w9";
    let tuning = vec!["--name".to_string(), peer_name.to_string()];
    let catalog = Catalog::new(Vec::new());
    let fresh = catalog
        .command_for("claude", "", &tuning)
        .expect("a fresh claude command");
    let session = zerocode_core::ProviderSession {
        key: zerocode_core::provider_session::SessionKey::SessionId,
        id: "claude-session-name".to_string(),
        transcript_path: None,
    };
    let resumed = catalog
        .command_for_resume("claude", &session, "", &tuning)
        .expect("a resumed claude command");
    let name_in = |command: &str| {
        split_command_line(command)
            .expect("a well-formed command")
            .windows(2)
            .find(|pair| pair[0] == "--name")
            .map(|pair| pair[1].clone())
    };

    assert_eq!(
        [name_in(&fresh), name_in(&resumed)],
        [Some(peer_name.to_string()), Some(peer_name.to_string())]
    );
}

/// The resume command written into a split effect is already the final
/// spawn argv: launch defaults, durable model/effort and provider selector.
/// The continuation prose is deliberately withheld until the durable
/// reseat, so the host only decodes this string and never appends a second
/// selector after the journal has named the command.
#[test]
fn the_reseat_journal_names_the_exact_resume_command_the_host_runs() {
    let session = zerocode_core::ProviderSession {
        key: zerocode_core::provider_session::SessionKey::SessionId,
        id: "0199-resume-worker".to_string(),
        transcript_path: None,
    };
    let tuning = vec![
        "--model".to_string(),
        "gpt-restart".to_string(),
        "-c".to_string(),
        "model_reasoning_effort=high".to_string(),
    ];
    let nudge = "continue the interrupted worker turn";
    let command = Catalog::new(Vec::new())
        .command_for_resume("codex", &session, "", &tuning)
        .expect("codex resume command");
    let effect = zerocode_core::agent_teams::Effect::Split {
        pane: "%2".to_string(),
        from: "%1".to_string(),
        direction: zerocode_core::agent_teams::Direction::Horizontal,
        command: command.clone(),
        helper: None,
    };
    let journaled = match effect {
        zerocode_core::agent_teams::Effect::Split { command, .. } => command,
        _ => unreachable!("the fixture is a split"),
    };
    assert!(
        !journaled.contains(nudge),
        "the interrupted work reached the process before durable reseat"
    );
    let host_argv = crate::split_command(&journaled);
    let mut expected = vec!["codex".to_string()];
    expected.extend(launch_plan("codex", None).args);
    expected.extend(tuning);
    expected.extend(["resume".to_string(), session.id]);
    assert_eq!(host_argv, expected);

    let host = include_str!("../agent_tools_runtime.rs")
        .split_once("impl agent_teams::Host for TeamWindow {")
        .expect("the split host")
        .1;
    assert!(
        host.contains("let words = split_command(command);")
            && !host
                .split_once("let words = split_command(command);")
                .expect("the command decoder")
                .1
                .chars()
                .take(1_000)
                .collect::<String>()
                .contains("resume_argv"),
        "the host stopped executing the journal's command verbatim"
    );
}

/// Cwd never crosses the renderer resume wire. Interactive resume keeps
/// the active root, while worker resume carries the ledger's checkout only
/// as typed host placement.
#[test]
fn a_worker_resume_root_comes_only_from_the_durable_checkout() {
    let board = include_str!("../cmd/board.rs");
    for door in [
        "pub(crate) fn resume_command(",
        "pub(crate) fn resume_session(",
    ] {
        let signature = board
            .split_once(door)
            .unwrap_or_else(|| panic!("{door} disappeared"))
            .1
            .split_once('{')
            .expect("function body")
            .0;
        assert!(
            !signature.contains("cwd") && !signature.contains("checkout"),
            "{door} opened a renderer/path input: {signature}"
        );
    }
    let resume_wrapper = board
        .split_once("pub(crate) fn resume_session(")
        .expect("resume wrapper")
        .1;
    assert!(resume_wrapper.contains("let root = state.active_root();"));

    let durable = tempfile::tempdir().expect("durable checkout");
    let placement = crate::agent_teams::WorkerHostPlacement::Existing(
        durable.path().to_string_lossy().into_owned(),
    );
    assert!(matches!(
        &placement,
        crate::agent_teams::WorkerHostPlacement::Existing(path)
            if std::path::Path::new(path) == durable.path()
    ));
    let host = include_str!("../agent_tools_runtime.rs");
    let existing = host
        .split_once("agent_teams::WorkerHostPlacement::Existing(path) =>")
        .expect("typed existing-checkout host arm")
        .1
        .chars()
        .take(700)
        .collect::<String>();
    assert!(
        existing.contains("PathBuf::from(&path)")
            && existing.contains("root.is_dir()")
            && !existing.contains("active_root"),
        "the existing-checkout arm derives worker cwd from another source:\n{existing}"
    );
}

/// `agent-list` is answered from the PATH a worker is LAUNCHED with, and
/// the look is taken without waiting for one.
///
/// Two failures are pinned here, and they pull in opposite directions.
/// Measure some other PATH and the verb answers honestly about a machine
/// nobody launches on — `hooks::pty_env` hands the child
/// `shell_path::hydrated()`, so that is the list a name has to resolve
/// against. Call `hydrate` instead and the verb spawns a login shell with
/// a five-second ceiling while holding the ledger, which would make the
/// cheapest read in the table the one that blocks every other verb.
#[test]
fn presence_is_read_from_the_launch_path_and_never_waits_for_it() {
    let listed = Catalog::new(Vec::new())
        .presence()
        .expect("this test process has a PATH");
    assert_eq!(
        listed.len(),
        zerocode_core::AGENT_SPECS.len(),
        "an agent the catalog knows went unanswered for"
    );
    let source = include_str!("../orchestration.rs");
    let body = source
        .split_once("fn machine_presence()")
        .expect("the measurement behind the verb")
        .1
        .chars()
        .take(400)
        .collect::<String>();
    // The one launch-PATH reader (t-3996 folded the readiness probe's copy
    // into it): hydrated when the shell has answered, the process's own when
    // it has not, and never a wait.
    assert!(
        body.contains("shell_path::launch_path()"),
        "presence is measured against a PATH no worker is launched with:\n{body}"
    );
    let reading = include_str!("../shell_path.rs")
        .split_once("pub fn launch_path()")
        .expect("the one launch-PATH reader")
        .1
        .chars()
        .take(200)
        .collect::<String>();
    assert!(
        reading.contains("hydrated()") && !reading.contains("hydrate("),
        "the launch PATH waits on a login shell, or reads something other \
         than the shell's answer:\n{reading}"
    );
}

/// A prompt survives the trip out and back, whatever is in it.
#[test]
fn a_prompt_arrives_as_one_argument_however_it_is_written() {
    let catalog = Catalog::new(Vec::new());
    for prompt in [
        "고쳐",
        "fix the parser",
        "it's a 'quoted' mess",
        "a \"double\" and a \\ backslash",
        "line one\nline two",
        // The briefing every summoned worker carries. It is the worst
        // shape this quoter ever sees — single quotes around a JSON body
        // with double quotes inside it — and if it came apart the worker
        // would start with half a protocol on its command line.
        &format!(
            "{}파서를 고쳐라",
            zerocode_core::orchestration::worker_briefing("t-7", "parser repair")
        ),
    ] {
        let line = catalog
            .command_for("codex", prompt, &[])
            .expect("codex is in the catalog");
        let words = split_command_line(&line).expect("a well-formed line");
        assert_eq!(
            words.last().map(String::as_str),
            Some(prompt),
            "`{prompt}` came apart on the way: {words:?}"
        );
        assert_eq!(words[0], "codex");
    }
}

/// The two vocabularies are told apart by the table that lists one of them.
#[test]
fn the_door_sends_each_verb_to_the_dialect_that_owns_it() {
    let said = |line: &str| {
        speaks_here(
            &line
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>(),
        )
    };
    for verb in ["run-create", "task-list", "worker-start", "send", "check"] {
        assert!(said(verb), "`{verb}` is advertised and not routed here");
    }
    for verb in [
        "split-window",
        "send-keys",
        "capture-pane",
        "list-panes",
        "kill-pane",
        "display-message",
        "select-layout",
        "respawn-pane",
        "-V",
    ] {
        assert!(!said(verb), "`{verb}` is tmux's and was taken from it");
    }
    assert!(!said(""), "an empty line belongs to nobody");
}

/// An agent whose prompt cannot ride the command line is refused in words a
/// coordinator can act on, rather than started without it.
#[test]
fn an_agent_that_reads_its_prompt_later_says_so_instead_of_starting_empty() {
    let catalog = Catalog::new(Vec::new());
    let typed: Vec<&'static str> = zerocode_core::agent::AGENT_SPECS
        .iter()
        .filter(|spec| matches!(prompt_injection(spec, "일"), Some(Injected::AfterStart(_))))
        .map(|spec| spec.id)
        .collect();
    assert!(!typed.is_empty(), "no agent takes the typed road any more");
    for agent in typed {
        let refused = catalog
            .command_for(agent, "일", &[])
            .expect_err("started anyway");
        assert!(refused.contains(agent), "{refused}");
        // Bare, the same agent starts fine — the refusal is about the
        // prompt, not about the agent.
        assert!(catalog.command_for(agent, "", &[]).is_ok(), "{agent}");
    }
}

/// A name nobody knows is a different refusal from a platform that cannot
/// run it, because a coordinator does different things about them.
#[test]
fn the_two_ways_a_worker_cannot_start_are_told_apart() {
    let catalog = Catalog::new(Vec::new());
    let unknown = catalog
        .command_for("nosuchagent", "", &[])
        .expect_err("started something that does not exist");
    assert!(unknown.contains("nosuchagent"), "{unknown}");

    let elsewhere: Vec<&'static str> = zerocode_core::agent::AGENT_SPECS
        .iter()
        .filter(|spec| !spec.runs_on(std::env::consts::OS))
        .map(|spec| spec.id)
        .collect();
    for agent in elsewhere {
        let refused = catalog
            .command_for(agent, "", &[])
            .expect_err("started an agent that cannot run here");
        assert!(
            refused.contains(std::env::consts::OS),
            "the refusal does not name the platform: {refused}"
        );
    }
}

/// A terminal becomes a seat, or the ledger never hears about it.
///
/// This is the half of the supervision road that only this file can get
/// wrong. A hook report names a TERM — a number this window mints — and the
/// ledger names a `(team, pane)`, where the pane id is only unique INSIDE
/// its team (two leaders both cut a `%2`). Everything either side of the
/// translation is measured in core; the translation itself was held up by a
/// text pin alone, which cannot tell a correct lookup from one that reads
/// the right shape out of the wrong table.
///
/// A pane nobody's worker sits in is the ordinary case, not a failure — the
/// window is full of them — so it has to be silent rather than loud.
#[test]
fn a_terms_silence_reaches_the_worker_sitting_in_it_and_nobody_elses() {
    // A base no other test uses: the team table is process-wide, and the
    // neighbouring suite hands out one base per test for the same reason.
    const LEADER_TERM: u32 = 9_100;
    const WORKER_TERM: u32 = 9_101;
    let _window = the_window();
    let team = format!("team-quiet-{LEADER_TERM}");
    let (run_id, worker, _pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKER_TERM);

    let said = |run_id: &str| {
        the_rows()
            .messages
            .iter()
            .filter(|one| {
                one.run == run_id
                    && one.kind == zerocode_core::orchestration::MessageKind::WentQuiet
            })
            .count()
    };

    // A person's Ctrl+C is not this pane going quiet on anybody.
    let turn_started = clock();
    pane_turn_ended(WORKER_TERM, turn_started, true, clock());
    assert_eq!(said(&run_id), 0, "an interrupted turn was reported");

    // A term this window has no team for — the ordinary case.
    pane_turn_ended(WORKER_TERM + 77, turn_started, false, clock());
    assert_eq!(said(&run_id), 0, "a pane nobody sits in was reported");

    // And the real thing, once — the SAME turn twice carries one stamp.
    pane_turn_ended(WORKER_TERM, turn_started, false, clock());
    pane_turn_ended(WORKER_TERM, turn_started, false, clock());
    assert_eq!(
        said(&run_id),
        1,
        "the seat lookup lost the worker, or spoke twice"
    );
    let rows = the_rows();
    let body = rows
        .messages
        .iter()
        .find(|one| {
            one.run == run_id && one.kind == zerocode_core::orchestration::MessageKind::WentQuiet
        })
        .map(|one| one.body.as_str().to_string())
        .expect("a notice");
    assert!(
        body.contains(&worker),
        "the notice named somebody else's worker: {body}"
    );
}

/// Zo's `turn/start` reaches this door instead of a hook. Once heard, the
/// same readiness sweep must retire the minute-long never-spoke window
/// before its deadline can notify about a live worker.
#[test]
fn a_turn_start_retires_the_workers_never_spoke_window() {
    const LEADER_TERM: u32 = 9_110;
    const WORKER_TERM: u32 = 9_111;
    let _window = the_window();
    let _beat = one_beat_at_a_time();
    let team = format!("team-turn-start-{LEADER_TERM}");
    let (run_id, worker, _pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKER_TERM);
    let deadline = the_rows()
        .workers
        .iter()
        .find(|row| row.id == worker)
        .and_then(|row| row.ready_by_ms)
        .expect("the worker's readiness deadline");

    pane_turn_began(WORKER_TERM, clock());
    tick(&Nowhere, &[], deadline.saturating_add(1));

    let rows = the_rows();
    let ready = rows
        .workers
        .iter()
        .find(|row| row.id == worker)
        .and_then(|row| row.ready_by_ms);
    let never_spoke = rows.messages.iter().any(|message| {
        message.run == run_id
            && message.kind == zerocode_core::orchestration::MessageKind::WentQuiet
            && message.body.contains("never_spoke")
    });
    assert_eq!((ready, never_spoke), (None, false));

    crate::agent_teams::forget_term(WORKER_TERM);
    crate::agent_teams::forget_term(LEADER_TERM);
}

/// Turn-end rows stay silent until the existing window beat measures a
/// real stall. This scenario holds the shared-window mutex because both
/// the runtime and team table are process-wide.
#[test]
fn the_beat_notifies_one_stalled_worker_without_turn_end_nags() {
    const LEADER_TERM: u32 = 9_120;
    const WORKER_TERM: u32 = 9_121;
    let _window = the_window();
    let _beat = one_beat_at_a_time();
    let team = format!("team-stall-{LEADER_TERM}");
    let (_run_id, _worker, _pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKER_TERM);

    struct Resting;
    impl Host for Resting {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, _term: u32, _text: &str) -> bool {
            false
        }
        fn quiet_since(&self, term: u32, _worker_started_ms: i64, _now_ms: i64) -> Option<i64> {
            (term == WORKER_TERM).then_some(1)
        }
        fn capture(&self, _term: u32) -> Option<String> {
            Some(String::new())
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let read = |now_ms: i64| {
        let said = run(
            &Resting,
            Vec::new(),
            &team,
            zerocode_core::agent_teams::LEADER_PANE,
            TEST_CAPABILITY,
            &words("check --peek --types went_quiet"),
            now_ms,
        );
        assert_eq!(said.exit_code, 0, "{}", said.stderr);
        serde_json::from_str::<serde_json::Value>(&said.stdout).expect("quiet mail")
    };

    pane_turn_ended(WORKER_TERM, 10, false, 20);
    assert_eq!(read(21)["count"], 0, "turn end itself rang");
    tick(&Resting, &[], 180_020);
    assert_eq!(read(180_021)["count"], 1, "the measured stall did not ring");
    tick(&Resting, &[], 180_022);
    assert_eq!(
        read(180_023)["count"],
        1,
        "the same stall rang on consecutive beats"
    );

    crate::agent_teams::forget_term(WORKER_TERM);
    crate::agent_teams::forget_term(LEADER_TERM);
}

/// A death travels the whole road: a term that exits reaches the
/// coordinator's INBOX, once, saying what to ask for again.
///
/// Measured here rather than only in core because only this file can get
/// the middle wrong. Core knows a `(team, pane)`; this window knows a
/// TERM, and the settlement it triggers has to survive the actor's
/// write-through and come back out of a durable read as mail somebody can
/// filter for. The nine notices that hid a real death were all delivered
/// perfectly well by this road; what was missing was the tenth, and a
/// notice posted into a ledger nobody can read it back out of would be the
/// same silence wearing a fix.
///
/// The inbox is read through the production `check` verb for that reason —
/// not by looking at the projection's message rows, which is a question
/// about storage rather than about whether anybody was told.
#[test]
fn a_dead_term_carrying_work_reaches_its_coordinators_inbox_once() {
    // Its own band: the team table is process-wide, and a term two tests
    // share is two tests settling each other's seats.
    const LEADER_TERM: u32 = 10_800;
    const WORKER_TERM: u32 = 10_801;
    let _window = the_window();
    let team = format!("team-death-{LEADER_TERM}");
    let (run_id, worker, _pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKER_TERM);
    let seat = zerocode_core::agent_teams::LEADER_PANE;

    let read = |line: &str| -> serde_json::Value {
        let said = run(
            &Nowhere,
            Vec::new(),
            &team,
            seat,
            TEST_CAPABILITY,
            &words(line),
            clock(),
        );
        assert_eq!(said.exit_code, 0, "`{line}`: {}", said.stderr);
        serde_json::from_str(&said.stdout).unwrap_or_else(|_| panic!("`{line}`: {}", said.stdout))
    };

    assert_eq!(
        read("check --peek --types worker_died")["count"],
        0,
        "the run was already carrying a death"
    );

    // The pane's process exits. This is the event the window watched
    // happen nine times over and never passed on.
    terminal_gone(WORKER_TERM, clock());

    let mail = read("check --peek --types worker_died");
    let messages = mail["messages"].as_array().expect("a list");
    assert_eq!(messages.len(), 1, "expected exactly one death: {mail}");
    let told = &messages[0];
    assert_eq!(told["from"], "ledger", "a worker was quoted: {told}");
    assert_eq!(
        told["trust"], "observation",
        "the ledger's own notice arrived as somebody's instruction: {told}"
    );
    let body: serde_json::Value =
        serde_json::from_str(told["body"].as_str().expect("a body")).expect("JSON");
    assert_eq!(body["workerId"], worker, "{body}");
    assert_eq!(body["taskStatus"], "ready", "{body}");
    // What a replacement is built from, and the ledger agrees it is a real
    // dispatch of this run rather than an id the notice invented.
    let named = body["dispatchId"].as_str().expect("a dispatch to name");
    let rows = the_rows();
    let held = rows
        .dispatches
        .iter()
        .find(|one| one.run == run_id && one.id == named)
        .expect("the notice named a dispatch this run never had");
    assert!(
        held.ended_ms.is_some(),
        "the notice named an attempt that is still open: {named}"
    );
    assert_eq!(body["taskId"], held.task, "{body}");

    // And once. A term that has already been settled says nothing more.
    terminal_gone(WORKER_TERM, clock());
    assert_eq!(
        read("check --peek --types worker_died")["count"],
        1,
        "one death was announced twice"
    );
}

/// A silence that has to be written down does not lock the window shut.
///
/// The lock this guards moved twice and the guard is still worth having:
/// first it was a ledger guard held across its own save; now the risk is
/// a caller that holds the TEAM TABLE while waiting on the actor's
/// mailbox — the actor takes that table itself, and the pair would hold
/// the window shut for good. The settlement road is driven end to end
/// under a deadline, because a deadlock cannot be asserted, only waited
/// for: this pin turns "forever" into a sentence.
#[test]
fn a_silence_that_must_be_written_down_does_not_hold_the_ledger_against_itself() {
    // A base no other test in this file uses. The team table is
    // process-wide and the seat lookup answers with the FIRST team
    // holding the term, so two tests sharing a base send one of them to
    // the other's seat — which is how this pin first failed.
    const LEADER_TERM: u32 = 9_700;
    const WORKER_TERM: u32 = 9_701;
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(20);
    let _window = the_window();
    let team = format!("team-nodeadlock-{LEADER_TERM}");
    let (run_id, _worker, _pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKER_TERM);

    // Off this thread, so a road that never returns is a deadline that
    // passes rather than a suite that stops.
    let (done, finished) = std::sync::mpsc::channel();
    let turn_started = clock();
    let carried = std::thread::spawn(move || {
        pane_turn_ended(WORKER_TERM, turn_started, false, clock());
        let _ = done.send(());
    });
    assert!(
        finished.recv_timeout(PATIENCE).is_ok(),
        "a worker's silence never came back — a guard was held across the \
             one thread that needed it"
    );
    carried.join().expect("the road");

    // And it did the thing, rather than returning early past the write.
    let said = the_rows()
        .messages
        .iter()
        .filter(|one| {
            one.run == run_id && one.kind == zerocode_core::orchestration::MessageKind::WentQuiet
        })
        .count();
    assert_eq!(said, 1, "the silence was never written down");
}

/// A verb presenting the wrong capability moves nothing at all.
///
/// The hole the Codex session found and this closes: authorization used to
/// happen in `answer_team_command`, which let the team table GO before this
/// road took it again to plan. A pane respawned in that gap rotates its
/// capability, so a request proved against the old incarnation would land
/// its effect on the new one — a time-of-check/time-of-use gap that no
/// amount of care between the two calls can close.
///
/// What is asserted is not only the refusal but the LEDGER: a door that
/// says no after writing something down is not a door. And the three ways
/// to be wrong — unknown team, unknown pane, wrong capability — answer with
/// one sentence on purpose, because three sentences would tell a caller
/// which of the three it had guessed.
#[test]
fn a_verb_presenting_the_wrong_capability_changes_nothing() {
    const LEADER_TERM: u32 = 9_800;
    const WRONG: &str = "not-this-panes-capability";
    let _window = the_window();
    let team = format!("team-fenced-{LEADER_TERM}");
    seat_a_team(&team, LEADER_TERM);
    let seat = zerocode_core::agent_teams::LEADER_PANE;

    let opened = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-create --name fenced"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let said: serde_json::Value = serde_json::from_str(&opened.stdout).expect("a run");
    let run_id = said["runId"].as_str().expect("a run id").to_string();

    let tasks = |id: &str| {
        the_rows()
            .tasks
            .iter()
            .filter(|task| task.run == id)
            .count()
    };
    let before = tasks(&run_id);

    for (pane, capability, why) in [
        (seat, WRONG, "a wrong capability"),
        (
            "%no-such-pane",
            TEST_CAPABILITY,
            "a pane this team has not got",
        ),
    ] {
        let turned = run(
            &Nowhere,
            Vec::new(),
            &team,
            pane,
            capability,
            &words("task-create --spec sneak-this-in"),
            clock(),
        );
        assert_eq!(turned.exit_code, 1, "{why} was let in: {turned:?}");
        assert!(
            turned.stderr.contains("stale or unauthorized agent pane"),
            "{why} was told which of the three it got wrong: {}",
            turned.stderr
        );
        assert_eq!(
            tasks(&run_id),
            before,
            "{why} was refused AFTER the ledger had already moved"
        );
    }

    // And the right one still works, so what was measured is a door and
    // not a wall.
    let allowed = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("task-create --spec the-real-one"),
        clock(),
    );
    assert_eq!(allowed.exit_code, 0, "{}", allowed.stderr);
    assert_eq!(tasks(&run_id), before + 1);
}

/// A wait that is answered comes back, and its receipt is there to replay.
///
/// The deadlock this closes, found by the Codex session: the loop read
/// `if let Some(reply) = look_again(&mut ledger(), …)`, and the guard a
/// scrutinee makes lives for the whole THEN body — where the receipt was
/// filed by taking the ledger AGAIN. One thread, one non-reentrant mutex,
/// no way out. It bit only the caller that both waited and named a retry,
/// which is the careful caller: the one that did the thing this protocol
/// asks for.
///
/// A deadlock cannot be asserted, only waited for, so the answer comes back
/// through a channel with a deadline rather than through `join`. And the
/// replay at the end is the other half: the fix moved the receipt under the
/// same guard as the look, so being answered and being able to ask again
/// are one transition rather than two.
#[test]
fn a_wait_that_is_answered_comes_back_and_leaves_a_receipt() {
    let _window = the_window();
    const LEADER_TERM: u32 = 10_000;
    const WORKER_TERM: u32 = 10_001;
    // Well under the road's own ceiling, which is what makes a timeout here
    // mean "it never came back" rather than "it waited its full turn".
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);
    const NAMED: &str = "check --wait --retry-request r-waited";
    let _turn = one_wait_at_a_time();
    let team = format!("team-waitreceipt-{LEADER_TERM}");
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    let (_run_id, worker, pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKER_TERM);

    let (came_back, answered) = std::sync::mpsc::channel();
    let before = WAITS_BEGUN.load(std::sync::atomic::Ordering::SeqCst);
    let asking = {
        let team = team.clone();
        std::thread::spawn(move || {
            let said = run(
                &Nowhere,
                Vec::new(),
                &team,
                seat,
                TEST_CAPABILITY,
                &words(NAMED),
                clock(),
            );
            let _ = came_back.send(said);
        })
    };

    // Proven, not assumed. Until this returns, the message below would be
    // read by the first look and the loop that held the deadlock would
    // never run.
    until_a_wait_has_begun(before);
    let held =
        crate::agent_teams::current_pane_capability(&team, &pane).expect("the worker capability");
    let posted = run(
        &Nowhere,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type status --body listen --retry-request waited-{worker}"
        )),
        clock(),
    );
    assert_eq!(posted.exit_code, 0, "{}", posted.stderr);

    let said = answered.recv_timeout(PATIENCE).expect(
        "the answered wait never came back — it took the ledger while its own \
             guard was still alive",
    );
    asking.join().expect("the sleeper");
    assert_eq!(said.exit_code, 0, "{}", said.stderr);
    assert!(
        said.stdout.contains("listen"),
        "the sleeper woke without its message: {}",
        said.stdout
    );

    // And the receipt stands: the same request asked again is answered from
    // it, at once, rather than sleeping for another whole ceiling.
    let again = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words(NAMED),
        clock(),
    );
    assert_eq!(
        again.stdout, said.stdout,
        "the answer a wait found was not filed under the name it was asked in"
    );
}

/// A disk that refuses a write still answers a look — from the disk's
/// word, not from the ghost of the change it refused.
///
/// The commonest disk fault is a disk that cannot be written and can
/// still be read — full, quota, mounted read-only — and this window's
/// answer to it is BOTH halves of what two earlier designs each had one
/// of. The change is refused (the old hand-rolled save answered success
/// and let a coordinator believe in a worker a restart never saw), and
/// the very next look answers — recovered from the store's own rows, so
/// what it shows is what a restart would show, not the change that never
/// landed. The old name for this test said "stops changes and not
/// looks"; the looks half survives, and it is more honest than it was.
#[test]
fn a_disk_that_refuses_a_write_still_answers_a_look_from_the_disks_word() {
    const LEADER_TERM: u32 = 10_100;
    let (window, store) = PrivateWindow::boot();
    let team = format!("team-refusing-{LEADER_TERM}");
    seat_a_team(&team, LEADER_TERM);
    let seat = zerocode_core::agent_teams::LEADER_PANE;

    let opened = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-create --name refusing"),
        1_000,
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);

    let connection = store
        .fault_connection_for_tests()
        .expect("fault connection");
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_writes
                    AFTER UPDATE OF revision ON orchestration_ledger_heads
                    BEGIN SELECT RAISE(ABORT, 'injected disk refusal'); END;",
        )
        .expect("a disk that refuses");

    let changing = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("task-create --spec phantom"),
        1_001,
    );
    assert_eq!(
        changing.exit_code, 1,
        "a change the disk refused was answered as a success: {changing:?}"
    );
    assert!(
        changing.stderr.contains("not durable"),
        "the refusal did not say why: {}",
        changing.stderr
    );

    for line in ["run-list", "run-current", "task-list", "worker-list"] {
        let looking = run(
            &Nowhere,
            Vec::new(),
            &team,
            seat,
            TEST_CAPABILITY,
            &words(line),
            1_002,
        );
        assert_eq!(
            looking.exit_code, 0,
            "`{line}` is a look and was refused because the DISK is unwell: {}",
            looking.stderr
        );
        // And what it shows is the DISK's word: the refused task is not
        // in it. The old window answered looks from a memory the disk
        // had refused — availability bought with a ghost.
        assert!(
            !looking.stdout.contains("phantom"),
            "`{line}` showed a change the disk refused: {}",
            looking.stdout
        );
    }

    /* And the SECOND try of the same request is refused too: the change
     * it repeats is still not on the disk that just refused it. Under
     * the actor there is no receipt to replay — the refused write took
     * the receipt down with it — so the retry RE-RUNS and meets the same
     * disk. */
    let named = "task-create --spec durable --retry-request r-durable";
    let first = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words(named),
        1_004,
    );
    assert_eq!(first.exit_code, 1, "{first:?}");
    let replayed = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words(named),
        1_005,
    );
    assert_eq!(
        replayed.exit_code, 1,
        "the retry of a change the disk refused was answered as a success \
             while that change was still not written: {replayed:?}"
    );

    connection
        .execute_batch("DROP TRIGGER refuse_writes;")
        .expect("the disk comes back");

    // Once the disk comes back the same request lands — once. The
    // refused attempts left no rows behind, so this is the FIRST time
    // the task exists rather than a second copy beside a ghost.
    let arrived = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words(named),
        1_006,
    );
    assert_eq!(arrived.exit_code, 0, "{}", arrived.stderr);
    let standing = the_rows()
        .tasks
        .iter()
        .filter(|task| task.spec.as_str() == "durable")
        .count();
    assert_eq!(standing, 1, "the refused attempts left rows behind");

    // And a change lands again — what was measured is a disk fault, not
    // a road that stopped working.
    let after = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("task-create --spec later"),
        1_007,
    );
    assert_eq!(after.exit_code, 0, "{}", after.stderr);
    drop(window);
}

/// A store that cannot be READ answers nothing — the narrow half of the
/// old availability, and the honest one.
///
/// When the rows themselves cannot be read back there is no "disk's
/// word" to answer from, and a window that answered from memory instead
/// would be showing a ledger nothing can verify. Every verb refuses,
/// reads included.
#[test]
fn a_store_that_cannot_be_read_answers_nothing() {
    const LEADER_TERM: u32 = 10_120;
    let (window, store) = PrivateWindow::boot();
    let team = format!("team-unreadable-{LEADER_TERM}");
    seat_a_team(&team, LEADER_TERM);
    let seat = zerocode_core::agent_teams::LEADER_PANE;

    let opened = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-create --name unreadable"),
        1_000,
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);

    // The rows disappear out from under the runtime — the shape of a
    // corrupted or vanished database, injected the only way a test can.
    store
        .fault_connection_for_tests()
        .expect("fault connection")
        .execute_batch("DROP TABLE ledger_runs;")
        .expect("an unreadable store");

    let changing = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("task-create --spec work"),
        1_001,
    );
    assert_eq!(
        changing.exit_code, 1,
        "a change on an unreadable store was answered as a success: {changing:?}"
    );
    let looking = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-list"),
        1_002,
    );
    assert_eq!(
        looking.exit_code, 1,
        "a look was answered from rows nothing can read back: {}",
        looking.stdout
    );
    drop(window);
}

/// The batch a refused write left is handed over again, not lost.
///
/// Raised by the Codex session, and the reason the durability question
/// moved from the VERB to the ANSWER. `check` is a read by the table, so a
/// disk fault used to let it through: the batch moved from pending to open
/// in memory, the caller was told an id and told to ack it, and none of
/// that reached the file. A restart there hands back a lease nobody can
/// ack and mail nobody will be given again.
///
/// The road it walks is refuse → repeat → recover, and all three are
/// asserted. The middle one is the load-bearing half: the batch has to
/// still be THERE after the refusal, or a fault would have swallowed the
/// mail on the way to telling the caller about it.
#[test]
fn a_delivery_the_disk_refused_is_handed_over_again_when_it_comes_back() {
    const LEADER_TERM: u32 = 10_150;
    const WORKER_TERM: u32 = 10_151;
    let (window, store) = PrivateWindow::boot();
    let team = format!("team-leased-{LEADER_TERM}");
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    let (_run_id, worker, pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKER_TERM);
    let mut clock = 1_000;
    let mut ask = |line: &str| {
        clock += 1;
        run(
            &Nowhere,
            Vec::new(),
            &team,
            seat,
            TEST_CAPABILITY,
            &words(line),
            clock,
        )
    };

    let held =
        crate::agent_teams::current_pane_capability(&team, &pane).expect("the worker capability");
    let laid = run(
        &Nowhere,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type status --body one --retry-request leased-{worker}"
        )),
        1_001,
    );
    assert_eq!(laid.exit_code, 0, "{}", laid.stderr);

    let connection = store
        .fault_connection_for_tests()
        .expect("fault connection");
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_lease
                    AFTER UPDATE OF revision ON orchestration_ledger_heads
                    BEGIN SELECT RAISE(ABORT, 'injected disk refusal'); END;",
        )
        .expect("a disk that refuses");

    let handing = ask("check");
    assert_eq!(
        handing.exit_code, 1,
        "a delivery lease the disk refused was answered as a success: {handing:?}"
    );
    assert!(
        handing.stderr.contains("not durable"),
        "the refusal did not say why: {}",
        handing.stderr
    );

    // The look that hands nothing over is still a look, even now. This is
    // the line the fix must not overshoot: `check` did not become a change,
    // its ANSWER did, and a coordinator polling a quiet inbox through a
    // disk fault is still owed an answer.
    let looking = ask("run-list");
    assert_eq!(
        looking.exit_code, 0,
        "a look was refused because the disk is unwell: {}",
        looking.stderr
    );

    connection
        .execute_batch("DROP TRIGGER refuse_lease;")
        .expect("the disk comes back");

    // Repeat, and recover: the mail is still there, under a lease the
    // caller can now be told about and can act on.
    let arrived = ask("check");
    assert_eq!(arrived.exit_code, 0, "{}", arrived.stderr);
    let mail: serde_json::Value = serde_json::from_str(&arrived.stdout).expect("json");
    assert_eq!(mail["count"], 1, "the refused check swallowed the mail");
    assert_eq!(mail["messages"][0]["body"], "one");

    let delivery = mail["deliveryId"].as_str().expect("an id").to_string();
    let acked = ask(&format!("check --ack {delivery}"));
    assert_eq!(
        acked.exit_code, 0,
        "the id the caller was finally given could not be acked: {}",
        acked.stderr
    );
    let after: serde_json::Value = serde_json::from_str(&acked.stdout).expect("json");
    assert_eq!(after["count"], 0, "and nothing is left behind it");
    drop(window);
}

/// A wait that finds mail is held to the same disk as a look that does.
///
/// The reason `look_again` answers a whole [`Decided`] and not a bare
/// reply. A wait's first look found nothing — it was free, and the plan it
/// started from says so — and then the SECOND look, half a minute later,
/// takes the delivery lease. If the requirement is read off the plan the
/// wait began with, that lease is handed over without ever waiting for the
/// disk. Reading it off the answer the wait came back with is the fix; the
/// alternative was parsing our own JSON back to see if there was a
/// `deliveryId` in it, which is a second place for the two to disagree.
///
/// The wait is deliberately UNNAMED. A `--retry-request` would file a
/// receipt, and a receipt already demands the disk on its own — the
/// careless caller is the one that has to be caught here, not the careful
/// one.
///
/// The disk refuses for the whole of the sleeping thread's life, which
/// makes the pre-sleep flush fail too. That one is a note and not a
/// refusal, and there is nothing to acknowledge in a plain `check --wait`,
/// so it changes nothing about what is being measured.
#[test]
fn a_wait_that_wakes_to_a_delivery_is_refused_when_the_disk_is() {
    // Its own band; see the note in
    // `a_leader_that_exits_leaves_no_child_holding_a_dispatch_nobody_can_reach`
    // for why a shared term is a shared team table and not a shared name.
    const LEADER_TERM: u32 = 10_600;
    const WORKER_TERM: u32 = 10_601;
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);
    let (window, store) = PrivateWindow::boot();
    let _turn = one_wait_at_a_time();
    let team = format!("team-waitleased-{LEADER_TERM}");
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    let (_run_id, worker, pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKER_TERM);
    let revision = runtime()
        .expect("the private runtime")
        .actor
        .view()
        .expect("the standing image")
        .revision();

    /* The disk refuses exactly the WOKEN look's write and nothing else.
     * The sender's own write has to land — it is what wakes the sleeper
     * — so the trigger is armed by revision: the run stands at 1, the
     * send will land at 2, and the woken delivery lease is the write
     * that would make 3. */
    let connection = store
        .fault_connection_for_tests()
        .expect("fault connection");
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER refuse_woken_lease
                    AFTER UPDATE OF revision ON orchestration_ledger_heads
                    WHEN NEW.revision = {}
                    BEGIN SELECT RAISE(ABORT, 'injected disk refusal'); END;",
            revision + 2
        ))
        .expect("a disk that refuses the third write");

    let (came_back, answered) = std::sync::mpsc::channel();
    let before = WAITS_BEGUN.load(std::sync::atomic::Ordering::SeqCst);
    let asking = {
        let team = team.clone();
        std::thread::spawn(move || {
            let said = run(
                &Nowhere,
                Vec::new(),
                &team,
                seat,
                TEST_CAPABILITY,
                &words("check --wait"),
                2_000,
            );
            let _ = came_back.send(said);
        })
    };

    until_a_wait_has_begun(before);
    let held =
        crate::agent_teams::current_pane_capability(&team, &pane).expect("the worker capability");
    let posted = run(
        &Nowhere,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type status --body listen --retry-request waitleased-{worker}"
        )),
        2_100,
    );
    assert_eq!(posted.exit_code, 0, "{}", posted.stderr);

    let said = answered
        .recv_timeout(PATIENCE)
        .expect("the woken wait never came back");
    asking.join().expect("the sleeper");
    assert_eq!(
        said.exit_code, 1,
        "a wait handed a delivery lease the disk refused was answered as a \
             success: {said:?}"
    );
    assert!(
        said.stderr.contains("not durable"),
        "the refusal did not say why: {}",
        said.stderr
    );

    connection
        .execute_batch("DROP TRIGGER refuse_woken_lease;")
        .expect("the disk comes back");

    // And the mail is still there for the next asking, which is what makes
    // the refusal honest rather than merely loud.
    let arrived = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("check"),
        2_200,
    );
    assert_eq!(arrived.exit_code, 0, "{}", arrived.stderr);
    let mail: serde_json::Value = serde_json::from_str(&arrived.stdout).expect("json");
    assert_eq!(mail["count"], 1, "the refused wait swallowed the mail");
    assert_eq!(mail["messages"][0]["body"], "listen");
    drop(window);
}

/// A missing ledger is a first boot; a damaged one is not.
///
/// The pure half of the fix, tested without a window around it. `open` used
/// to fold three answers into one — no file, unreadable file, unparseable
/// file — and then write an empty ledger back over whichever of them it
/// had. That is not a degraded boot, it is deletion: one bad byte, one
/// wrong permission, one power cut mid-write, and every run, task, dispatch
/// and message the person had was gone with nothing to restore from.
///
/// So the bytes are read back after every failing case. That assertion is
/// the whole point of the slice.
#[test]
fn a_ledger_that_is_missing_is_not_a_ledger_that_is_damaged() {
    let root = tempfile::tempdir().expect("temp directory");
    let path = root.path().join(LEDGER_FILE);

    assert!(
        matches!(read_ledger(&path), Loaded::Fresh),
        "a window with no ledger yet has to be able to start"
    );

    // A real one round-trips, which is the case the other two must not be
    // confused with. Written from a ledger of its OWN — the window's is
    // shared with every other test in this binary, and borrowing it to
    // write a fixture is how a probe comes to change somebody else's
    // answer.
    let good = serde_json::to_vec(&Ledger::new()).expect("bytes");
    std::fs::write(&path, &good).expect("write");
    assert!(
        matches!(read_ledger(&path), Loaded::Read(_, _)),
        "a ledger this window wrote could not be read back"
    );

    for (which, bytes) in [
        (
            "truncated mid-write",
            b"{\"runs\":[{\"id\":\"r-1\"".to_vec(),
        ),
        (
            "not json at all",
            b"\n\n<html>the proxy ate it</html>\n".to_vec(),
        ),
        ("json but not a ledger", b"[1,2,3]".to_vec()),
        // Invalid UTF-8. It reaches the same answer through the same call,
        // which is why the file is read as bytes and not as a string.
        ("invalid utf-8", vec![0x7b, 0x22, 0xff, 0xfe, 0x22, 0x7d]),
        ("empty", Vec::new()),
    ] {
        std::fs::write(&path, &bytes).expect("write");
        let said = match read_ledger(&path) {
            Loaded::Unreadable(why) => why,
            other => panic!("{which}: a damaged ledger was read as {other:?}"),
        };
        assert!(
            said.contains("not a ledger this window can read"),
            "{which}: {said}"
        );
        assert_eq!(
            std::fs::read(&path).expect("read back"),
            bytes,
            "{which}: reading a damaged ledger changed the file"
        );
    }

    // And a file that cannot be READ — a directory standing where the file
    // should be is the portable way to fail an open, which matters because
    // a mode bit means nothing on Windows.
    std::fs::remove_file(&path).expect("remove");
    std::fs::create_dir(&path).expect("directory");
    let said = match read_ledger(&path) {
        Loaded::Unreadable(why) => why,
        other => panic!("an unreadable ledger was read as {other:?}"),
    };
    assert!(said.contains("could not be read"), "{said}");
    assert!(
        path.is_dir(),
        "reading an unreadable ledger changed the file"
    );
}

#[test]
fn authority_diagnostics_explain_copy_validation_and_atomic_replacement() {
    let said = authority_is_unavailable("its runtime found a ledger content invariant violation");
    for part in [
        "ledger content invariant",
        "authority-before-",
        "backup",
        "WAL",
        "integrity_check",
        "Ledger::rebuild",
        "copy",
        "atomic",
        "closed",
    ] {
        assert!(said.contains(part), "missing {part}: {said}");
    }
}

/// The sentence a degraded window says is one sentence.
///
/// Raised by the Codex session against the first cut, where the message was
/// a backslash-continued literal whose continuation was eaten by the tool
/// that wrote it — the shipped string carried ten spaces of source
/// indentation into the middle of what a person reads. It is built with
/// `concat!` now, and this holds it to that: no run of spaces, and all
/// three things said.
#[test]
fn the_sentence_a_degraded_window_says_is_one_sentence() {
    let said = ledger_is_unavailable("it could not be read (permission denied)");
    assert!(
        !said.contains("  "),
        "the message carries its own indentation: [{said}]"
    );
    for part in [
        // What is wrong.
        "orchestration is unavailable",
        "it could not be read (permission denied)",
        // That the file is untouched — nobody should go looking for a
        // backup of something that was never overwritten.
        "left exactly as it was, byte for byte",
        // And what actually ends this state.
        "Repair or move it and restart the window.",
    ] {
        assert!(
            said.contains(part),
            "the message never said `{part}`: [{said}]"
        );
    }
}

/// A ledger that parses is not yet a ledger that can be acted on.
///
/// Raised by the Codex session against the first cut, which drew the line
/// at "does serde accept it". Every fixture below parses perfectly, and
/// each one costs the person something if the window opens it:
///
/// · A counter standing below an id the file already holds mints that id a
///   second time. `next_id: 0` beside a `run-1` gives two runs called
///   `run-1` on the next `run-create`, one of which is the person's, and
///   neither addressable apart from the other.
/// · Two things already sharing a name, which is the same wound already
///   inflicted.
/// · A reference to something that is not there — a binding, a dependency,
///   a message an inbox is holding — which sends a verb into a run that
///   does not exist or hands over mail nobody wrote.
/// · A field a NEWER window wrote. Read and dropped, it is deleted the
///   moment this window saves, so the shapes are `deny_unknown_fields` and
///   an unknown field is a file this window will not open.
///
/// None of these is repairable from here. Guessing a counter or dropping a
/// dangling reference is this window deciding what somebody's ledger ought
/// to have said. So each is refused with the bytes untouched — asserted
/// every time, because that is the whole difference between a degraded boot
/// and a deletion.
#[test]
fn a_ledger_that_parses_and_contradicts_itself_is_not_opened() {
    let root = tempfile::tempdir().expect("temp directory");
    let path = root.path().join(LEDGER_FILE);

    // One real ledger, built by the road, to bend in each of the ways
    // below. Its own, not the window's — see the note in
    // `a_ledger_that_is_missing_is_not_a_ledger_that_is_damaged`.
    let good = {
        let mut held = Ledger::new();
        // A second run, so a reference can be aimed at the WRONG one — the
        // shape a single-run fixture cannot express, and the shape a
        // whole-ledger "is anything called that" check waves through.
        let other = held.create_run("the other one", 999);
        held.create_task(
            &other,
            "elsewhere".into(),
            String::new(),
            Vec::new(),
            None,
            999,
        )
        .expect("a task");
        let run = held.create_run("real", 1_000);
        let task = held
            .create_task(
                &run,
                "migrate".into(),
                String::new(),
                Vec::new(),
                None,
                1_001,
            )
            .expect("a task");
        held.start_worker(&run, "claude", ("team-1", "%2"), Some(&task), 1_002)
            .expect("a worker");
        held.post(
            &run,
            zerocode_core::orchestration::Draft {
                from: "team-1/%2".to_string(),
                to: format!("run:{run}"),
                kind: zerocode_core::orchestration::MessageKind::Status,
                body: "one".into(),
                subject: Default::default(),
                priority: Default::default(),
                payload: Default::default(),
                thread: None,
                task: None,
                dispatch: None,
            },
            1_003,
        )
        .expect("a message");
        serde_json::to_value(&held).expect("value")
    };

    // The control: what the road wrote, read straight back.
    std::fs::write(&path, serde_json::to_vec(&good).expect("bytes")).expect("write");
    assert!(
        matches!(read_ledger(&path), Loaded::Read(_, _)),
        "a ledger this window wrote could not be read back"
    );

    /// One way to bend a good ledger, and what to call the wound.
    type Bend = (&'static str, fn(&mut serde_json::Value));

    let bends: Vec<Bend> = vec![
        ("a counter below what it already holds", |bent| {
            bent["next_id"] = serde_json::json!(0);
        }),
        // The other end of the same collision: `mint` adds one before it
        // formats, so a counter at the ceiling either panics the window or
        // wraps and starts the names over.
        ("a counter with no room left", |bent| {
            bent["next_id"] = serde_json::json!(u64::MAX);
        }),
        ("two runs of one name", |bent| {
            let twin = bent["runs"][1].clone();
            bent["runs"].as_array_mut().expect("runs").push(twin);
        }),
        ("two tasks of one name", |bent| {
            let twin = bent["runs"][1]["tasks"][0].clone();
            bent["runs"][1]["tasks"]
                .as_array_mut()
                .expect("tasks")
                .push(twin);
        }),
        ("two workers of one name", |bent| {
            let twin = bent["runs"][1]["workers"][0].clone();
            bent["runs"][1]["workers"]
                .as_array_mut()
                .expect("workers")
                .push(twin);
        }),
        ("two messages of one name", |bent| {
            let twin = bent["runs"][1]["messages"][0].clone();
            bent["runs"][1]["messages"]
                .as_array_mut()
                .expect("messages")
                .push(twin);
        }),
        ("a pane bound to a run that is not there", |bent| {
            bent["bound"] = serde_json::json!([["team-1/%1", "run-nowhere"]]);
        }),
        ("a dispatch pointing at no task", |bent| {
            bent["runs"][1]["dispatches"][0]["task"] = serde_json::json!("t-nowhere");
        }),
        ("a worker holding a dispatch that is not there", |bent| {
            bent["runs"][1]["workers"][0]["dispatch"] = serde_json::json!("dp-nowhere");
        }),
        ("an inbox holding mail nobody wrote", |bent| {
            let home = bent["runs"][1]["inboxes"][0][0].clone();
            bent["runs"][1]["inboxes"] = serde_json::json!([[home, { "pending": ["m-nowhere"], "open": null, "acked": null }]]);
        }),
        ("a field only a newer window knows", |bent| {
            bent["future_authority"] = serde_json::json!("something this build drops");
        }),
        ("a field only a newer window knows, nested", |bent| {
            bent["runs"][1]["tasks"][0]["future_verdict"] = serde_json::json!(true);
        }),
        // Cross-run, and cross-TYPE. Everything named below is really in
        // the file, which is exactly why a global "is anything called that"
        // waves all four through.
        ("a dispatch pointing at another run's task", |bent| {
            let theirs = bent["runs"][0]["tasks"][0]["id"].clone();
            bent["runs"][1]["dispatches"][0]["task"] = theirs;
        }),
        ("an inbox holding another run's mail", |bent| {
            let theirs = bent["runs"][1]["messages"][0]["id"].clone();
            let home = bent["runs"][1]["inboxes"][0][0].clone();
            bent["runs"][0]["inboxes"] = serde_json::json!([
                [home, { "pending": [theirs], "open": null, "acked": null }]
            ]);
        }),
        ("a dispatch whose task is really a worker", |bent| {
            let worker = bent["runs"][1]["workers"][0]["id"].clone();
            bent["runs"][1]["dispatches"][0]["task"] = worker;
        }),
        // And the keys. A duplicate is not a duplicate — it is a second row
        // that can never be read, holding something the file believes in.
        ("a pane bound twice", |bent| {
            let run = bent["runs"][1]["id"].clone();
            let other = bent["runs"][0]["id"].clone();
            bent["bound"] = serde_json::json!([["team-1/%1", run], ["team-1/%1", other],]);
        }),
        ("two inboxes for one address", |bent| {
            let home = bent["runs"][1]["inboxes"][0][0].clone();
            let mail = bent["runs"][1]["messages"][0]["id"].clone();
            bent["runs"][1]["inboxes"] = serde_json::json!([
                [home.clone(), { "pending": [mail], "open": null, "acked": null }],
                [home, { "pending": [], "open": null, "acked": null }],
            ]);
        }),
        // A batch already acknowledged has spent its id, whether or not
        // the thing it named is still here. A counter below one lets the
        // next delivery be minted with the same name — and the stale ack
        // then answers "said twice, meant once" for a batch nobody has
        // seen, leaving it open forever.
        ("an acknowledged id the counter has not passed", |bent| {
            let home = bent["runs"][1]["inboxes"][0][0].clone();
            bent["runs"][1]["inboxes"] = serde_json::json!([
                [home, { "pending": [], "open": null,
                         "acked": "d-999999", "acked_history": [] }]
            ]);
        }),
        ("an acknowledged id something else also has", |bent| {
            let run = bent["runs"][1]["id"].clone();
            let home = bent["runs"][1]["inboxes"][0][0].clone();
            bent["runs"][1]["inboxes"] = serde_json::json!([
                [home, { "pending": [], "open": null,
                         "acked": run, "acked_history": [] }]
            ]);
        }),
        /* The two halves of one fact, disagreeing. Everything named here
         * exists; what is wrong is that the file says two things.
         */
        (
            "a worker and its dispatch naming different partners",
            |bent| {
                // A second worker that really exists, so what fails is the
                // DISAGREEMENT and not a name pointing at nothing.
                let mut other = bent["runs"][1]["workers"][0].clone();
                other["id"] = serde_json::json!("w-somebody-else");
                other["pane"] = serde_json::json!("%9");
                other["dispatch"] = serde_json::json!(null);
                let elsewhere = other["id"].clone();
                bent["runs"][1]["workers"]
                    .as_array_mut()
                    .expect("workers")
                    .push(other);
                bent["runs"][1]["dispatches"][0]["worker"] = elsewhere;
            },
        ),
        ("a worker still carrying an attempt that ended", |bent| {
            // The task is settled too, so what fails is the WORKER still
            // holding it rather than the status disagreeing.
            bent["runs"][1]["dispatches"][0]["ended_ms"] = serde_json::json!(2_000);
            bent["runs"][1]["dispatches"][0]["succeeded"] = serde_json::json!(true);
            bent["runs"][1]["tasks"][0]["status"] = serde_json::json!("completed");
        }),
        ("an open attempt nobody is carrying", |bent| {
            bent["runs"][1]["workers"][0]["dispatch"] = serde_json::json!(null);
        }),
        /* A worker on its way out cannot still be carrying work.
         *
         * Nothing sweeps these states — `window_restarted` takes the LIVE
         * ones — so an attempt held by one of them would never end, and it
         * would hold a standing order's ceiling for the life of the file.
         * `Active` and `Retained` are the two that stay, and `Retained` is
         * in on purpose: `retain_worker` keeps a worker past the task it is
         * carrying.
         */
        ("a released worker still carrying work", |bent| {
            bent["runs"][1]["workers"][0]["state"] = serde_json::json!("released");
        }),
        ("a reclaimable worker still carrying work", |bent| {
            bent["runs"][1]["workers"][0]["state"] = serde_json::json!("reclaimable");
        }),
        ("a worker being released still carrying work", |bent| {
            bent["runs"][1]["workers"][0]["state"] = serde_json::json!("release_pending");
        }),
        (
            "a worker nobody heard back about still carrying work",
            |bent| {
                bent["runs"][1]["workers"][0]["state"] = serde_json::json!("release_unknown");
            },
        ),
        // Status and dispatches, both ways round.
        (
            "a task ready to be taken that somebody already has",
            |bent| {
                bent["runs"][1]["tasks"][0]["status"] = serde_json::json!("ready");
            },
        ),
        ("a task somebody has that nobody is carrying", |bent| {
            bent["runs"][0]["tasks"][0]["status"] = serde_json::json!("dispatched");
        }),
        ("a task two attempts are carrying at once", |bent| {
            // Ended by hand, which is the status that ALLOWS an open
            // attempt — so what fails is the second one and not the status.
            bent["runs"][1]["tasks"][0]["status"] = serde_json::json!("completed");
            let mut twin = bent["runs"][1]["dispatches"][0].clone();
            twin["id"] = serde_json::json!("dp-a-second-attempt");
            let second = twin["id"].clone();
            bent["runs"][1]["dispatches"]
                .as_array_mut()
                .expect("dispatches")
                .push(twin);
            // Somebody real carries it, so what fails is the SECOND open
            // attempt on one task and not a dispatch nobody holds.
            let mut other = bent["runs"][1]["workers"][0].clone();
            other["id"] = serde_json::json!("w-the-other-agent");
            other["pane"] = serde_json::json!("%9");
            other["dispatch"] = second.clone();
            bent["runs"][1]["dispatches"][1]["worker"] = other["id"].clone();
            bent["runs"][1]["workers"]
                .as_array_mut()
                .expect("workers")
                .push(other);
        }),
        // And the seat, across runs — a pane belongs to a window, not to a
        // run, so two runs cannot each have somebody unsettled in it.
        ("two runs with an unsettled worker in one pane", |bent| {
            let mut theirs = bent["runs"][1]["workers"][0].clone();
            theirs["id"] = serde_json::json!("w-in-the-other-run");
            theirs["dispatch"] = serde_json::json!(null);
            bent["runs"][0]["workers"] = serde_json::json!([theirs]);
        }),
        ("two receipts under one name", |bent| {
            bent["served"] = serde_json::json!([
                { "caller": "a", "request": "r-1", "answer": "{}",
                  "fingerprint": "f", "rebind": null },
                { "caller": "a", "request": "r-1", "answer": "{\"different\":true}",
                  "fingerprint": "f", "rebind": null },
            ]);
        }),
    ];

    /* And the ledgers that are FINE.
     *
     * The validator's failure mode is not only letting a bad file through;
     * it is refusing a good one, and a person whose window says "repair it
     * or restart" about a ledger with nothing wrong has been given a
     * problem instead of a warning. A forward dependency is the exact
     * shape: `task-create --deps t-99` before `t-99` exists is allowed, and
     * the task simply waits.
     */
    for (which, bend) in [
        (
            "a task waiting on one that is not written yet",
            (|bent| {
                bent["runs"][1]["tasks"][0]["deps"] = serde_json::json!(["t-99"]);
            }) as fn(&mut serde_json::Value),
        ),
        ("a note about a task that is no longer there", |bent| {
            bent["runs"][1]["messages"][0]["task"] = serde_json::json!("t-99");
        }),
        ("a counter well ahead of everything in it", |bent| {
            bent["next_id"] = serde_json::json!(9_000);
        }),
        // A coordinator ending a carried task by hand — `update_task`
        // allows it, and the attempt stays open until the worker reports
        // or its terminal dies.
        ("a carried task somebody ended by hand", |bent| {
            bent["runs"][1]["tasks"][0]["status"] = serde_json::json!("completed");
        }),
        ("a carried task somebody failed by hand", |bent| {
            bent["runs"][1]["tasks"][0]["status"] = serde_json::json!("failed");
        }),
        // And a worker kept past the task it is carrying.
        ("a retained worker carrying work", |bent| {
            bent["runs"][1]["workers"][0]["state"] = serde_json::json!("retained");
        }),
    ] {
        let mut bent = good.clone();
        bend(&mut bent);
        std::fs::write(&path, serde_json::to_vec(&bent).expect("bytes")).expect("write");
        assert!(
            matches!(read_ledger(&path), Loaded::Read(_, _)),
            "{which}: a healthy ledger was refused, which hands a person a \
                 problem where there was none"
        );
    }

    for (which, bend) in bends {
        let mut bent = good.clone();
        bend(&mut bent);
        let bytes = serde_json::to_vec(&bent).expect("bytes");
        std::fs::write(&path, &bytes).expect("write");
        match read_ledger(&path) {
            Loaded::Unreadable(_) => {}
            other => panic!("{which}: a ledger that contradicts itself was read as {other:?}"),
        }
        assert_eq!(
            std::fs::read(&path).expect("read back"),
            bytes,
            "{which}: refusing a ledger changed the file"
        );
    }
}

/// A window that could not read its ledger does nothing, loudly.
///
/// The road half. Every door is tried, and each is checked WHERE IT IS
/// rather than all of them at the end, because a single assertion after all
/// five would be answered by whichever of them still worked: the verb road
/// refuses, the beat never begins, neither lifecycle event settles
/// anything, and the file on disk is never written over. The last is the
/// one that was actually costing data.
///
/// The window is given a real worker with a real turn to end and a real
/// terminal to lose, and a second task the beat would dispatch. A degraded
/// window doing nothing to a window with nothing to do is not a
/// measurement — the first cut of this test made exactly that mistake, and
/// two of the five guards could be deleted with it still green.
///
/// "The ledger did not move" is asked as "MY run did not move". The ledger
/// is shared with every other test in this binary and a whole-ledger
/// snapshot would be green or red by whoever else happened to be running,
/// which is how the first cut passed alone and failed in the suite.
#[test]
fn a_window_that_could_not_read_its_ledger_refuses_rather_than_starting_over() {
    const LEADER_TERM: u32 = 10_250;
    const WORKER_TERM: u32 = 10_251;
    // Distinctive enough to be looked for in the rows themselves, which
    // is the one way to ask "did this land" that no other test can disturb.
    const AFTER: &str = "after-the-damage-10250";
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-degraded-{LEADER_TERM}");
    let seat = zerocode_core::agent_teams::LEADER_PANE;

    // Real work first, driven through the road: this is the state a
    // degraded window inherits from the boot before it, not something it
    // is asked to create. A second task is left ready, so the beat below
    // has something it would have dispatched.
    let (run_id, _worker, _pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKER_TERM);
    let second = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("task-create --spec read-the-file"),
        clock(),
    );
    assert_eq!(second.exit_code, 0, "{}", second.stderr);

    // My own run, read out of the shared rows.
    let mine = || {
        let rows = the_rows();
        serde_json::json!({
            "workers": rows
                .workers
                .iter()
                .filter(|one| one.run == run_id)
                .map(|one| format!("{}:{:?}", one.id, one.state))
                .collect::<Vec<_>>(),
            "dispatches": rows
                .dispatches
                .iter()
                .filter(|one| one.run == run_id)
                .map(|one| format!("{}:{:?}", one.id, one.ended_ms))
                .collect::<Vec<_>>(),
            "messages": rows
                .messages
                .iter()
                .filter(|one| one.run == run_id)
                .count(),
        })
    };

    // A file with something in it, standing in for the person's ledger.
    let root = tempfile::tempdir().expect("temp directory");
    let path = root.path().join(LEDGER_FILE);
    let theirs = b"{\"runs\":[{\"id\":\"r-theirs\"".to_vec();
    std::fs::write(&path, &theirs).expect("write");

    let counted = Counting::default();
    let beats = BEATS_BEGUN.load(std::sync::atomic::Ordering::SeqCst);
    let standing = mine();
    let _degraded = Degraded::begin();

    let refused = run(
        &counted,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words(&format!("run-create --name {AFTER}")),
        clock(),
    );
    assert_eq!(refused.exit_code, 1, "{refused:?}");
    for said in [
        "orchestration is unavailable",
        "left exactly as it was",
        "Repair or move it and restart the window",
    ] {
        assert!(
            refused.stderr.contains(said),
            "the refusal never said `{said}`: {}",
            refused.stderr
        );
    }

    // Reads are refused too, and deliberately: `run-list` out of an empty
    // ledger is not a truthful answer to "what am I running", it is a
    // confident wrong one. The disk-fault road keeps reads alive because
    // the ledger there is the person's; here it is not.
    let looking = run(
        &counted,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-list"),
        clock(),
    );
    assert_eq!(looking.exit_code, 1, "{looking:?}");
    assert_eq!(mine(), standing, "a refused verb moved the run anyway");
    assert!(
        !the_rows().runs.iter().any(|one| one.name.as_str() == AFTER),
        "a refused verb landed in the ledger anyway"
    );
    assert_eq!(counted.calls(), 0, "a refused verb reached the host");

    assert_eq!(tick(&counted, &[], clock()), 0, "a degraded window beat");
    assert_eq!(
        BEATS_BEGUN.load(std::sync::atomic::Ordering::SeqCst),
        beats,
        "a degraded window began a beat over a ledger it could not read"
    );
    assert_eq!(
        counted.calls(),
        0,
        "a degraded beat asked the host to cut a pane"
    );

    let turn_started = clock();
    pane_turn_ended(WORKER_TERM, turn_started, false, clock());
    assert_eq!(
        mine(),
        standing,
        "a degraded window settled a turn into a ledger it could not read"
    );

    terminal_gone(WORKER_TERM, clock());
    assert_eq!(
        mine(),
        standing,
        "a degraded window settled a dead terminal into a ledger it could \
             not read"
    );
    assert_eq!(
        counted.calls(),
        0,
        "a degraded lifecycle event asked the host to do something"
    );

    assert_eq!(
        std::fs::read(&path).expect("read back"),
        theirs,
        "a degraded window wrote over the file it could not read — this is \
             the assertion the whole slice exists for"
    );
}

/// A leader that exits leaves no child holding a dispatch nobody can reach.
///
/// The window ends a team when its LEADER's shell dies
/// (`agent_teams::forget_term`), and this road ran a line before that with
/// only the leader's own seat in hand. Every child was left `Active` with
/// its dispatch open — counting against a standing order's ceiling,
/// addressed by a pane record the very next line threw away. No verb could
/// reach it, no restart cleared it, and nothing on screen said why the run
/// had stopped dispatching.
///
/// Both halves are asserted in the order the window does them: the ledger
/// settles while the team is still there to be read, and only then is the
/// team forgotten. A teammate dying is the control — it takes one row out
/// and leaves everybody else alone.
#[test]
fn a_leader_that_exits_leaves_no_child_holding_a_dispatch_nobody_can_reach() {
    /* Its own band, and this is not housekeeping.
     *
     * The team table is a `static`, so a term is process-wide and two tests
     * sharing one are two tests dissolving each other's teams. This test
     * was first written on 10_300/10_400 — the band
     * `a_release_in_one_team_leaves_another_teams_pane_of_the_same_name_alone`
     * already stands in — and turned that test red without a line of it
     * changing, intermittently, depending on which ran first.
     */
    const LEADER_TERM: u32 = 10_500;
    const WORKING_TERM: u32 = 10_501;
    const IDLE_TERM: u32 = 10_502;
    const BYSTANDER_TERM: u32 = 10_550;
    const BYSTANDING_WORKER_TERM: u32 = 10_551;
    let _window = the_window();
    let team = format!("team-leaderdies-{LEADER_TERM}");
    let bystander = format!("team-bystander-{LEADER_TERM}");
    seat_a_team(&bystander, BYSTANDER_TERM);
    let seat = zerocode_core::agent_teams::LEADER_PANE;

    // The whole road seeds this: one run, a worker carrying a task, an
    // idle teammate beside it — and a BYSTANDER'S worker summoned into
    // the same run from another team by naming it, which one run can
    // hold (`--run` is a global name; the split lands in the caller's
    // own team and the worker row says so).
    let (run_id, watched, _pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKING_TERM);
    let summoned = |host: &dyn Host, from_team: &str, line: &str| {
        let said = run(
            host,
            Vec::new(),
            from_team,
            seat,
            TEST_CAPABILITY,
            &words(line),
            clock(),
        );
        assert_eq!(said.exit_code, 0, "`{line}`: {}", said.stderr);
        let said: serde_json::Value = serde_json::from_str(&said.stdout).expect("a worker");
        said["workerId"].as_str().expect("a worker id").to_string()
    };
    let idle = summoned(
        &Splitting::onto(IDLE_TERM),
        &team,
        "worker-start --agent claude",
    );
    // Somebody else's, in a team whose leader is alive and well.
    let bystanding = summoned(
        &Splitting::onto(BYSTANDING_WORKER_TERM),
        &bystander,
        &format!("worker-start --agent claude --run {run_id}"),
    );

    // By id, not by seat: half of what this test asks about is a worker
    // that has just been settled — which is exactly nobody's seat.
    let state = |id: &str| {
        the_rows()
            .workers
            .iter()
            .find(|one| one.id == id)
            .expect("the worker")
            .state
    };

    // A teammate dying first, as the control: one seat settles and nothing
    // else moves.
    terminal_gone(IDLE_TERM, clock());
    assert_eq!(
        state(&idle),
        zerocode_core::orchestration::WorkerState::Released,
        "a teammate whose terminal we watched exit was not settled"
    );
    assert_eq!(
        state(&watched),
        zerocode_core::orchestration::WorkerState::Active,
        "a teammate dying settled somebody else"
    );

    // And now the leader. Its child is KEPT (t-2512): the leader's exit
    // says nothing about the child's pane, so the attempt stays open for
    // the coordinator that sits next — or for the child's own report.
    terminal_gone(LEADER_TERM, clock());
    assert_eq!(
        state(&watched),
        zerocode_core::orchestration::WorkerState::Orphaned,
        "the leader died and its child lost the attempt it was still carrying"
    );
    assert!(
        the_rows()
            .workers
            .iter()
            .find(|one| one.id == watched)
            .expect("the worker")
            .dispatch
            .is_some(),
        "the dispatch closed under a child that may still be working: {watched}"
    );
    assert_eq!(
        state(&bystanding),
        zerocode_core::orchestration::WorkerState::Active,
        "another team's worker was settled by this team's leader dying"
    );

    // The window's own next line. The LEADER's pane goes; the team stays,
    // leaderless, for as long as a child still works in it — that is what
    // keeps the orphan's token answering (t-2512) — and goes with its last
    // child.
    crate::agent_teams::forget_term(LEADER_TERM);
    {
        let tables = crate::agent_teams::teams();
        let kept = tables
            .get(&team)
            .expect("the leader's team stands leaderless while its child works");
        assert!(
            kept.pane(zerocode_core::agent_teams::LEADER_PANE).is_none(),
            "the dead leader's pane record was kept"
        );
        assert!(
            kept.panes().any(|pane| pane.term == WORKING_TERM),
            "the working child's pane record went with its leader"
        );
    }
    // The idle teammate's pane record is still in the table — the test
    // settled its ledger seat above without the window's own `forget_term`
    // — so it is forgotten the way a dying pane is, and the last child's
    // exit then takes the table with it.
    crate::agent_teams::forget_term(IDLE_TERM);
    crate::agent_teams::forget_term(WORKING_TERM);
    assert!(
        !crate::agent_teams::teams().contains_key(&team),
        "a leaderless team outlived its last child"
    );
    crate::agent_teams::forget_term(BYSTANDER_TERM);
}

/* `a_turn_that_ends_is_settled_against_the_seat_it_was_read_from` fell
 * here. What it measured — the seat still held from the read through the
 * settlement — is structural in the actor: `turn_ended` resolves and
 * settles inside one `with_seat_of_term` borrow, and the actor's own
 * battery drives that door. A respawn between the REQUEST and its
 * processing re-points the seat, and the old term then resolves to
 * nothing — the wrong incarnation cannot be settled, only possibly
 * none. Terminal numbers are never re-let inside a window, so "the same
 * term, a different seat" cannot be spelled at all. */

/// The tmux roads — `kill-pane` and `respawn-pane` — end a terminal that
/// the LEDGER has a worker sitting in, and they did it without telling it.
/// `remove_pane` takes the seat out of the team and `respawn_pane` points
/// it at a different shell, and either way the ledger's worker is left
/// `Active` with its dispatch open, addressed by a seat that no longer
/// means what it meant. No verb could reach it, so a standing order's
/// ceiling was held for the rest of the session by an attempt that ended
/// the moment the pane did.
///
/// Both roads are walked, and each is asked the two questions that make the
/// settlement exact rather than merely present: does it settle THIS team's
/// worker and not another team's pane of the same name, and does a respawn
/// settle the OLD incarnation while leaving the new seat alone.
#[test]
fn a_pane_the_leader_closes_or_restarts_takes_its_dispatch_with_it() {
    const MINE_LEADER: u32 = 10_700;
    const MINE_WORKER: u32 = 10_701;
    const THEIRS_LEADER: u32 = 10_710;
    const THEIRS_WORKER: u32 = 10_711;
    const REPLACEMENT: u32 = 10_720;
    // The SAME pane id in both teams, which is what a real window has.
    const SHARED: &str = "%2";

    let _window = the_window();
    for road in ["kill-pane", "respawn-pane"] {
        let mine = format!("team-{road}-mine-{MINE_LEADER}");
        let theirs = format!("team-{road}-theirs-{THEIRS_LEADER}");
        let (run_id, worker, shared) = a_worker_carrying_work(&mine, MINE_LEADER, MINE_WORKER);
        let (_theirs_run, theirs_worker, theirs_pane) =
            a_worker_carrying_work(&theirs, THEIRS_LEADER, THEIRS_WORKER);
        // The SAME pane id in both teams, which is what a real window
        // has: each fresh team numbers its first teammate alike.
        assert_eq!(shared, theirs_pane, "the two teams no longer collide");
        assert_eq!(shared, SHARED);

        let host = Splitting::onto(REPLACEMENT);
        let line = match road {
            "kill-pane" => format!("kill-pane -t {SHARED}"),
            _ => format!("respawn-pane -k -t {SHARED} claude"),
        };
        let said = crate::agent_teams::run(
            &host,
            &mine,
            zerocode_core::agent_teams::LEADER_PANE,
            TEST_CAPABILITY,
            &words(&line),
        );
        assert_eq!(said.exit_code, 0, "{road}: {}", said.stderr);

        let rows = the_rows();
        // By id: the point of the settlement is that nobody is in that
        // pane any more, so the seat no longer answers for it.
        let settled = rows
            .workers
            .iter()
            .find(|one| one.id == worker && one.run == run_id)
            .expect("the worker");
        assert_eq!(
            settled.state,
            zerocode_core::orchestration::WorkerState::Released,
            "{road}: the worker in the pane that ended is still live, holding \
                 a dispatch nothing can reach: {worker}"
        );
        assert!(
            settled.dispatch.is_none(),
            "{road}: the dispatch stayed open on a pane that is gone"
        );

        // The other team's `%2` is a different pane in a different team and
        // was not touched.
        assert_eq!(
            // By SEAT on purpose: the other team's `%2` still has somebody
            // in it, and that it still answers is half the assertion.
            rows.workers
                .iter()
                .find(|one| one.team == theirs
                    && one.pane == shared
                    && one.state != zerocode_core::orchestration::WorkerState::Released)
                .expect("the worker")
                .state,
            zerocode_core::orchestration::WorkerState::Active,
            "{road}: another team's pane of the same name was settled with \
                 it: {theirs_worker}"
        );

        // And a respawn leaves the NEW incarnation where it put it: the
        // pane id is the same on purpose, and behind it is the new shell.
        if road == "respawn-pane" {
            assert_eq!(
                crate::agent_teams::teams()
                    .get(&mine)
                    .and_then(|team| team.term_of(SHARED)),
                Some(REPLACEMENT),
                "the seat was not re-pointed at the shell that replaced it"
            );
            assert_eq!(
                host.closed(),
                vec![MINE_WORKER],
                "the old shell was not the one ended"
            );
        }
        crate::agent_teams::forget_term(MINE_LEADER);
        crate::agent_teams::forget_term(THEIRS_LEADER);
    }
}

/// A host that closes by walking back into the tables never deadlocks.
///
/// The window's own close does exactly this — `forget_term_state` →
/// `forget_term` takes the team table — and it is why `close` has always
/// run outside the guard (live report 2026-08-17, a window frozen whole).
/// The settlement added a second lock to that road, so this asks the same
/// question of both: a host that takes the team table AND the ledger from
/// inside `close` must come back.
///
/// A test that hangs proves nothing, so the answer is fetched from another
/// thread with a deadline. What is measured is that it arrives at all.
#[test]
fn a_close_that_walks_back_into_the_tables_still_comes_back() {
    const LEADER_TERM: u32 = 10_730;
    const WORKER_TERM: u32 = 10_731;
    const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);
    let _window = the_window();
    let team = format!("team-reentrant-close-{LEADER_TERM}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKER_TERM);

    struct WalksBackIn;

    impl Host for WalksBackIn {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, _term: u32, _text: &str) -> bool {
            false
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, term: u32) {
            // The table AND the runtime, from inside the effect: this is
            // the re-entry the close arms must survive — `forget_term`
            // takes the team table, and a read walks the actor's own
            // mailbox round trip.
            crate::agent_teams::forget_term(term);
            let _ = the_rows().runs.len();
        }
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }

    let (came_back, answered) = std::sync::mpsc::channel();
    let asking = {
        let team = team.clone();
        let pane = pane.clone();
        std::thread::spawn(move || {
            let said = crate::agent_teams::run(
                &WalksBackIn,
                &team,
                zerocode_core::agent_teams::LEADER_PANE,
                TEST_CAPABILITY,
                &words(&format!("kill-pane -t {pane}")),
            );
            let _ = came_back.send(said);
        })
    };
    let said = answered.recv_timeout(PATIENCE).expect(
        "the close never came back — a road that takes the team table or the \
             ledger while the host is closing is a window frozen whole",
    );
    asking.join().expect("the closer");
    assert_eq!(said.exit_code, 0, "{}", said.stderr);
    assert_eq!(
        the_rows()
            .workers
            .iter()
            .find(|one| one.id == worker && one.run == run_id)
            .expect("the worker")
            .state,
        zerocode_core::orchestration::WorkerState::Released,
        "the settlement was skipped on the road that has to be careful"
    );
}

/// `kill-pane` will not take the leader, which is why the arm above settles
/// one seat and never a whole team.
///
/// Pinned rather than assumed: a leader's death dissolves its team
/// (`terminal_gone`), and if this refusal ever softened, `Effect::Close`
/// would be the wrong shape for what it was being asked to do — silently.
#[test]
fn killing_a_pane_will_not_take_the_leader() {
    let _window = the_window();
    const LEADER_TERM: u32 = 10_740;
    let team = format!("team-noregicide-{LEADER_TERM}");
    seat_a_team(&team, LEADER_TERM);
    let refused = crate::agent_teams::run(
        &Nowhere,
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(&format!(
            "kill-pane -t {}",
            zerocode_core::agent_teams::LEADER_PANE
        )),
    );
    assert_eq!(refused.exit_code, 1, "{refused:?}");
    assert!(
        refused.stderr.contains("refusing to kill leader pane"),
        "{}",
        refused.stderr
    );
    crate::agent_teams::forget_term(LEADER_TERM);
}

/// A beat names its own request, and names it the same way twice.
///
/// The road now refuses anything that changes the ledger without a
/// `--retry-request`, and the beat is the caller with nobody to type one
/// for it. That makes the NAME the contract: the same string for the same
/// summoning, so a beat whose save failed and which comes back on the very
/// next tick replays instead of cutting a second pane for work already
/// being done — and a different string for the next attempt, or the task's
/// second try would be answered from the first try's receipt and never
/// happen at all.
///
/// Both halves are asserted because each alone is satisfiable by a mistake:
/// a constant name is stable and never lets a retry through, and a name
/// with a clock in it is unique and never replays.
#[test]
fn a_beat_names_its_own_request_and_names_it_the_same_way_twice() {
    let summoning = |task: &str, attempt: u32| {
        beat_request_name(&zerocode_core::orchestration::Dispatchable {
            task: task.to_string(),
            spec: "read the file".into(),
            agent: "claude".to_string(),
            team: "team-1".to_string(),
            pane: zerocode_core::agent_teams::LEADER_PANE.to_string(),
            attempt,
        })
    };

    assert_eq!(
        summoning("t-1", 0),
        summoning("t-1", 0),
        "the same summoning named itself differently the second time — its \
             retry would cut a second pane for work already being done"
    );
    assert_ne!(
        summoning("t-1", 0),
        summoning("t-1", 1),
        "the next attempt reused the last one's name — it would be answered \
             from that receipt and never happen"
    );
    assert_ne!(
        summoning("t-1", 0),
        summoning("t-2", 0),
        "two tasks shared one name"
    );

    // And the spec is deliberately not in it: a coordinator that corrects a
    // task's wording has not made it a different attempt.
    let reworded = beat_request_name(&zerocode_core::orchestration::Dispatchable {
        task: "t-1".to_string(),
        spec: "read the file, carefully".into(),
        agent: "claude".to_string(),
        team: "team-1".to_string(),
        pane: zerocode_core::agent_teams::LEADER_PANE.to_string(),
        attempt: 0,
    });
    assert_eq!(summoning("t-1", 0), reworded);
}

/// A window that answers a capture, and can make the world move while it
/// does.
///
/// The respawn is the whole point of the first test below: it happens
/// INSIDE `capture`, which is exactly where the release road has let go of
/// both guards, so the interleaving is produced rather than waited for.
struct Releasing {
    captured: Mutex<Vec<u32>>,
    respawn: Option<(String, String, u32)>,
    closed: Mutex<Vec<u32>>,
}

/// A screen read that can replace either half of a pane incarnation while
/// the orchestration road has deliberately let go of its locks.
///
/// `capture` itself performs the move. There is no scheduler or timeout in
/// these tests: if the read returns, the change happened inside the exact
/// gap under test.
struct ReadSeatMove {
    team: String,
    pane: String,
    term: Option<u32>,
    generation: Option<String>,
}

struct Reading {
    screen: &'static str,
    move_during_capture: Mutex<Option<ReadSeatMove>>,
    captures: std::sync::atomic::AtomicUsize,
}

impl Reading {
    fn quiet(screen: &'static str) -> Self {
        Self {
            screen,
            move_during_capture: Mutex::new(None),
            captures: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn moving(
        screen: &'static str,
        team: &str,
        pane: &str,
        term: Option<u32>,
        generation: Option<&str>,
    ) -> Self {
        Self {
            screen,
            move_during_capture: Mutex::new(Some(ReadSeatMove {
                team: team.to_string(),
                pane: pane.to_string(),
                term,
                generation: generation.map(str::to_string),
            })),
            captures: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn captures(&self) -> usize {
        self.captures.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Host for Reading {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        _token: &str,
    ) -> Option<u32> {
        None
    }

    fn send(&self, _term: u32, _text: &str) -> bool {
        false
    }

    fn capture(&self, _term: u32) -> Option<String> {
        self.captures
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let moving = self
            .move_during_capture
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .take();
        if let Some(moving) = moving {
            // Production takes these in this order too: team table, then
            // its redacted token registry. Keeping the order here makes a
            // reentrant test a deadlock test as well as a stale-read test.
            let mut tables = crate::agent_teams::teams();
            if let Some(term) = moving.term
                && let Some(held) = tables.get_mut(&moving.team)
            {
                held.respawn_pane(&moving.pane, term);
            }
            if let Some(generation) = moving.generation {
                let _ =
                    crate::agent_teams::remember_pane_token(&moving.team, &moving.pane, generation);
            }
        }
        Some(self.screen.to_string())
    }

    fn focus(&self, _term: u32) -> bool {
        false
    }

    fn close(&self, _term: u32) {}

    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
}

impl Releasing {
    fn quiet() -> Self {
        Self {
            captured: Mutex::new(Vec::new()),
            respawn: None,
            closed: Mutex::new(Vec::new()),
        }
    }
    fn respawning(team: &str, pane: &str, into: u32) -> Self {
        Self {
            captured: Mutex::new(Vec::new()),
            respawn: Some((team.to_string(), pane.to_string(), into)),
            closed: Mutex::new(Vec::new()),
        }
    }
    fn closed(&self) -> Vec<u32> {
        self.closed
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    }
}

impl Host for Releasing {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        _token: &str,
    ) -> Option<u32> {
        None
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        false
    }
    fn capture(&self, term: u32) -> Option<String> {
        self.captured.lock().unwrap().push(term);
        if let Some((team, pane, into)) = &self.respawn
            && let Some(held) = crate::agent_teams::teams().get_mut(team)
        {
            held.respawn_pane(pane, *into);
        }
        Some("what the worker printed".to_string())
    }
    fn focus(&self, _term: u32) -> bool {
        false
    }
    fn close(&self, term: u32) {
        self.closed
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push(term);
    }
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
}

/// Like [`a_worker_in_a_pane`], with an open dispatch — which is the
/// thing a closed pane used to leave behind, so it is the thing the
/// settlement has to take. Answers the run, the worker, and the pane the
/// plan minted for it.
fn a_worker_carrying_work(team: &str, leader_term: u32, term: u32) -> (String, String, String) {
    an_agent_carrying_work("claude", team, leader_term, term)
}

/// The same, for a named agent's worker.
fn an_agent_carrying_work(
    agent: &str,
    team: &str,
    leader_term: u32,
    term: u32,
) -> (String, String, String) {
    seat_a_team(team, leader_term);
    let host = Splitting::onto(term);
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &host,
        Vec::new(),
        team,
        seat,
        TEST_CAPABILITY,
        &words("run-create --name carrying"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let opened: serde_json::Value = serde_json::from_str(&opened.stdout).expect("a run");
    let run_id = opened["runId"].as_str().expect("a run id").to_string();
    let carried = run(
        &host,
        Vec::new(),
        team,
        seat,
        TEST_CAPABILITY,
        &words("task-create --spec migrate"),
        clock(),
    );
    assert_eq!(carried.exit_code, 0, "{}", carried.stderr);
    let carried: serde_json::Value = serde_json::from_str(&carried.stdout).expect("a task");
    let task = carried["taskId"].as_str().expect("a task id");
    let started = run(
        &host,
        Vec::new(),
        team,
        seat,
        TEST_CAPABILITY,
        &words(&format!("worker-start --agent {agent} --task {task}")),
        clock(),
    );
    assert_eq!(started.exit_code, 0, "{}", started.stderr);
    let started: serde_json::Value = serde_json::from_str(&started.stdout).expect("a worker");
    (
        run_id,
        started["workerId"]
            .as_str()
            .expect("a worker id")
            .to_string(),
        started["pane"].as_str().expect("a pane").to_string(),
    )
}

/// A `dispatch --inject` through the whole road: the attach is durable,
/// the preamble lands on the WORKER's own terminal as one paste, and the
/// receipt exists only because it did — a typing that never happened
/// retries into the attach guard's own sentence, not into a second
/// attempt.
#[test]
fn an_inject_types_the_preamble_at_the_workers_own_terminal() {
    const LEADER: u32 = 10_900;
    const WORKER: u32 = 10_901;
    let _window = the_window();
    let team = format!("team-inject-{LEADER}");
    let (_run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;

    /// Remembers what it was asked to deliver, keystrokes and pastes on
    /// separate ledgers, and whether to pretend the terminal is alive.
    struct Typing {
        alive: bool,
        sent: Mutex<Vec<(u32, String)>>,
        pasted: Mutex<Vec<(u32, String)>>,
    }
    impl Host for Typing {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.sent
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            self.alive
        }
        fn paste(&self, term: u32, text: &str) -> bool {
            self.pasted
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            self.alive
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = Typing {
        alive: true,
        sent: Mutex::new(Vec::new()),
        pasted: Mutex::new(Vec::new()),
    };

    // The worker reports; its pane is idle and reusable. It presents its
    // OWN capability — the one the split minted for it — because a seat
    // is proven by its token, not by being mentioned.
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request done-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);

    // The spec quotes the very escape that would close a paste envelope
    // early — the exact prose a repo about terminals writes down — so the
    // delivery below has something real to disarm.
    let carried = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("task-create --spec follow-up:\u{1b}[201~then-typing"),
        clock(),
    );
    assert_eq!(carried.exit_code, 0, "{}", carried.stderr);
    let carried: serde_json::Value = serde_json::from_str(&carried.stdout).expect("a task");
    let task = carried["taskId"].as_str().expect("a task id").to_string();

    let line = format!("dispatch --task {task} --to {pane} --inject --retry-request re-{task}");
    let handed = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&line),
        clock(),
    );
    assert_eq!(handed.exit_code, 0, "{}", handed.stderr);
    {
        let pasted = host.pasted.lock().unwrap_or_else(|held| held.into_inner());
        assert_eq!(pasted.len(), 1, "{pasted:?}");
        let (term, ref text) = pasted[0];
        assert_eq!(term, WORKER, "the preamble went to somebody else's shell");
        // The paste road, not the key road — and the spec's own escape
        // arrives as its printable name, so it cannot close the window's
        // envelope around itself and have the rest read as typing.
        assert!(
            !text.contains('\u{1b}'),
            "an escape survived into the paste: {text:?}"
        );
        assert!(text.contains("follow-up:␛[201~then-typing"), "{text}");
        let sent = host.sent.lock().unwrap_or_else(|held| held.into_inner());
        assert!(
            sent.is_empty(),
            "a briefing is a document, and it rode the key road: {sent:?}"
        );
    }
    let rows = the_rows();
    let reused = rows
        .workers
        .iter()
        .find(|one| one.id == worker)
        .expect("the worker");
    assert_eq!(
        reused.state,
        zerocode_core::orchestration::WorkerState::Active,
        "the pane was handed work and is not carrying it"
    );

    // A retry of the SAME name replays the answer and types nothing new.
    let again = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&line),
        clock(),
    );
    assert_eq!(again.exit_code, 0, "{}", again.stderr);
    assert_eq!(again.stdout, handed.stdout, "a retry answered differently");
    assert_eq!(
        host.pasted
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .len(),
        1,
        "a replay pasted the preamble a second time"
    );
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// The torn middle: the attach was made durable and the typing failed.
/// The dispatch stands, the caller is told exactly that, and the SAME
/// retry name lands in the attach guard's own sentence rather than a
/// hunt for a rival.
#[test]
fn an_inject_whose_typing_failed_leaves_the_dispatch_standing_and_says_so() {
    const LEADER: u32 = 10_910;
    const WORKER: u32 = 10_911;
    let _window = the_window();
    let team = format!("team-inject-dead-{LEADER}");
    let (_run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let host = Splitting::onto(WORKER);

    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request done-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);
    let carried = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("task-create --spec doomed"),
        clock(),
    );
    let carried: serde_json::Value = serde_json::from_str(&carried.stdout).expect("a task");
    let task = carried["taskId"].as_str().expect("a task id").to_string();

    let line = format!("dispatch --task {task} --to {pane} --inject --retry-request re-{task}");
    let refused = run(
        &Nowhere,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&line),
        clock(),
    );
    assert_eq!(refused.exit_code, 1);
    assert!(
        refused.stderr.contains("the dispatch stands"),
        "{}",
        refused.stderr
    );
    let rows = the_rows();
    assert!(
        rows.dispatches
            .iter()
            .any(|one| one.task == task && one.ended_ms.is_none()),
        "the refusal took the attach with it"
    );
    let again = run(
        &Nowhere,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&line),
        clock(),
    );
    assert_eq!(again.exit_code, 1);
    assert!(
        again.stderr.contains("on this very pane"),
        "{}",
        again.stderr
    );
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// A zo pane whose exact launch was refused holds a program that will not
/// run the briefing (t-2773). The window says so instead of typing at it:
/// nothing is pasted, the dispatch stands, and the refusal carries the
/// contract's own reason rather than "the terminal is gone".
#[test]
fn an_inject_into_a_pane_whose_launch_contract_failed_is_withheld_by_name() {
    const LEADER: u32 = 10_920;
    const WORKER: u32 = 10_921;
    let _window = the_window();
    let team = format!("team-inject-contract-{LEADER}");
    let (_run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;

    /// Records pastes and refuses delivery at the worker's pane the way
    /// the window does for a refused contract.
    struct Withholding {
        pasted: Mutex<Vec<(u32, String)>>,
    }
    impl Host for Withholding {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, _term: u32, _text: &str) -> bool {
            true
        }
        fn paste(&self, term: u32, text: &str) -> bool {
            self.pasted
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            true
        }
        fn delivery_refusal(&self, term: u32) -> Option<String> {
            (term == WORKER).then(|| {
                    "zo refused its exact launch contract (unsupported-effort); the briefing was withheld"
                        .to_string()
                })
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = Withholding {
        pasted: Mutex::new(Vec::new()),
    };
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request done-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);
    let carried = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("task-create --spec exact-only"),
        clock(),
    );
    let carried: serde_json::Value = serde_json::from_str(&carried.stdout).expect("a task");
    let task = carried["taskId"].as_str().expect("a task id").to_string();
    let line = format!("dispatch --task {task} --to {pane} --inject --retry-request re-{task}");
    let refused = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&line),
        clock(),
    );
    assert_eq!(refused.exit_code, 1, "{}", refused.stdout);
    assert!(
        refused.stderr.contains("unsupported-effort")
            && refused.stderr.contains("the dispatch stands"),
        "{}",
        refused.stderr
    );
    assert!(
        host.pasted
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .is_empty(),
        "the briefing was typed at a pane whose launch was refused"
    );
    let rows = the_rows();
    assert!(
        rows.dispatches
            .iter()
            .any(|one| one.task == task && one.ended_ms.is_none()),
        "the refusal took the attach with it"
    );
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// The `--worktree` ask travels on the child's own capability, is
/// readable exactly WHILE the split runs — the production window takes
/// it there — and cannot outlive the call that placed it.
#[test]
fn a_worktree_ask_rides_the_childs_capability_once() {
    const LEADER: u32 = 11_020;
    const WORKER: u32 = 11_021;
    let _window = the_window();
    let team = format!("team-worktree-{LEADER}");
    seat_a_team(&team, LEADER);

    /// A splitting host that does what the window's does: spends the
    /// worktree ask its capability carries, and remembers both halves.
    struct Keeping {
        onto: u32,
        tokens: Mutex<Vec<(String, Option<crate::agent_teams::WorkerHostSpec>)>>,
    }
    impl Host for Keeping {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            token: &str,
        ) -> Option<u32> {
            let asked = crate::agent_teams::take_worker_host_ask(token);
            self.tokens
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((token.to_string(), asked));
            Some(self.onto)
        }
        fn send(&self, _term: u32, _text: &str) -> bool {
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            Some(String::new())
        }
        fn focus(&self, _term: u32) -> bool {
            true
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = Keeping {
        onto: WORKER,
        tokens: Mutex::new(Vec::new()),
    };
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("run-create --name isolated"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let carried = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("task-create --spec slice"),
        clock(),
    );
    let carried: serde_json::Value = serde_json::from_str(&carried.stdout).expect("a task");
    let task = carried["taskId"].as_str().expect("a task id").to_string();

    let started = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&format!(
            "worker-start --agent claude --task {task} --worktree"
        )),
        clock(),
    );
    assert_eq!(started.exit_code, 0, "{}", started.stderr);
    let (token, asked) = host
        .tokens
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .last()
        .expect("a split carried a capability")
        .clone();
    // The ask stood while the split ran, named after the task id AND its
    // title; a second spend — and any read after the call — finds it gone,
    // which is the guard doing its one job. The old value was only
    // `t-NNN`, leaving every worker checkout opaque in the sidebar and in
    // `git branch`.
    assert_eq!(
        asked.as_ref().map(|ask| &ask.placement),
        Some(&crate::agent_teams::WorkerHostPlacement::New(format!(
            "{task} slice"
        )))
    );
    assert_eq!(crate::agent_teams::take_worker_host_ask(&token), None);

    // A split WITHOUT the flag places nothing.
    let plain = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("worker-start --agent claude"),
        clock(),
    );
    assert_eq!(plain.exit_code, 0, "{}", plain.stderr);
    let (_, asked) = host
        .tokens
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .last()
        .expect("a split carried a capability")
        .clone();
    assert!(
        asked.is_some_and(|ask| matches!(
            ask.placement,
            crate::agent_teams::WorkerHostPlacement::Inherited
        )),
        "a bare start did not inherit the leader checkout"
    );
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// One sleeper per inbox, first come: a second `check --wait` on the same
/// address is told to look without waiting — and when it spent an `--ack`
/// on the way in, the refusal says the acknowledgement stood rather than
/// letting "refused" read as "nothing happened".
#[test]
fn a_second_sleeper_on_one_inbox_is_refused_with_its_ack_intact() {
    const LEADER: u32 = 11_060;
    const WORKER: u32 = 11_061;
    let _window = the_window();
    let team = format!("team-waiters-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let host = Splitting::onto(WORKER + 1);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let address = format!("run:{run_id}");
    let _first_sleeper = WaiterCard::hold(&run_id, &address, &format!("{team}/{leader}"));

    let second = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("check --wait"),
        clock(),
    );
    assert_eq!(second.exit_code, 1);
    assert!(
        second.stderr.contains("already has a sleeper"),
        "{}",
        second.stderr
    );

    // The acked spelling: the ack lands, the wait is declined, and the
    // refusal says which — then a plain check proves the batch is gone.
    let held =
        crate::agent_teams::current_pane_capability(&team, &pane).expect("the worker capability");
    let sent = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type status --body news --retry-request news-{worker}"
        )),
        clock(),
    );
    assert_eq!(sent.exit_code, 0, "{}", sent.stderr);
    let batch = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("check"),
        clock(),
    );
    let delivery =
        serde_json::from_str::<serde_json::Value>(&batch.stdout).expect("a batch")["deliveryId"]
            .as_str()
            .expect("a delivery")
            .to_string();
    let declined = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&format!(
            "check --ack {delivery} --wait --retry-request ack-{delivery}"
        )),
        clock(),
    );
    assert_eq!(declined.exit_code, 1);
    assert!(
        declined.stderr.contains("acknowledgement stood"),
        "{}",
        declined.stderr
    );
    let after = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("check"),
        clock(),
    );
    let count =
        serde_json::from_str::<serde_json::Value>(&after.stdout).expect("a look")["count"].clone();
    assert_eq!(count, 0, "the declined wait lost the acknowledgement");
    crate::agent_teams::forget_term(LEADER);
}

/// A blocking `ask` is a sleeper on its own thread: it holds no waiter
/// seat — the inbox card stays free for a real `check --wait` — and it
/// wakes when the ANSWER lands, not at its deadline.
#[test]
fn a_blocking_ask_wakes_on_its_answer_not_at_the_deadline() {
    const LEADER: u32 = 11_070;
    const WORKER: u32 = 11_071;
    let _window = the_window();
    let _turn = one_wait_at_a_time();
    let team = format!("team-asking-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");

    let before = WAITS_BEGUN.load(std::sync::atomic::Ordering::SeqCst);
    let asking = {
        let (team, pane, held) = (team.clone(), pane.clone(), held.clone());
        std::thread::spawn(move || {
            run(
                &Nowhere,
                Vec::new(),
                &team,
                &pane,
                &held,
                &words("ask --body which-way? --timeout-ms 8000"),
                clock(),
            )
        })
    };
    until_a_wait_has_begun(before);
    assert!(
        !WaiterCard::occupied(
            &run_id,
            &format!("worker:{worker}"),
            &format!("{team}/{pane}")
        ),
        "an ask wait took the inbox sleeper's seat"
    );
    let landed = std::time::Instant::now();
    let mail = run(
        &Nowhere,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("check --types question"),
        clock(),
    );
    assert_eq!(mail.exit_code, 0, "{}", mail.stderr);
    let batch = serde_json::from_str::<serde_json::Value>(&mail.stdout).expect("a question batch");
    let question = batch["messages"][0]["messageId"]
        .as_str()
        .unwrap_or_else(|| panic!("no question in the batch: {}", mail.stdout))
        .to_string();
    let posted = run(
        &Nowhere,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&format!("reply --to-message {question} --body go")),
        clock(),
    );
    assert_eq!(posted.exit_code, 0, "{}", posted.stderr);

    let answer = asking.join().expect("the asker panicked");
    assert_eq!(answer.exit_code, 0, "{}", answer.stderr);
    let said: serde_json::Value =
        serde_json::from_str(&answer.stdout).expect("a woken ask answers JSON");
    assert_eq!(said["answered"], true);
    assert_eq!(said["answer"]["body"], "go");
    let woke_after = landed.elapsed();
    assert!(
        woke_after < std::time::Duration::from_millis(4_000),
        "the asker surfaced {woke_after:?} after its answer landed — it \
             is polling the clock, not hearing the bell"
    );
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// A key from the person's own road reaches the ledger as a takeover —
/// once per terminal incarnation: the gate absorbs every later report,
/// and forgetting the terminal reopens it for the number's next life.
#[test]
fn a_persons_key_takes_the_workers_pane_over_durably() {
    const LEADER: u32 = 11_080;
    const WORKER: u32 = 11_081;
    let _window = the_window();
    let team = format!("team-taken-{LEADER}");
    let (run_id, worker, _pane) = a_worker_carrying_work(&team, LEADER, WORKER);

    super::pane_taken_over(WORKER, clock());
    let taken = |projection: &zerocode_core::orchestration::LedgerProjectionV1| {
        projection
            .workers
            .iter()
            .find(|row| row.run == run_id && row.id == worker)
            .expect("the worker row")
            .taken_over
    };
    assert!(taken(&the_rows()), "the key never reached the ledger");
    // The gate holds: a second report is a probe, not a round trip, and
    // the row stays exactly as taken as it was.
    super::pane_taken_over(WORKER, clock());
    assert!(taken(&the_rows()));
    // The number's next life starts unclaimed — the ledger row keeps its
    // fact, but a fresh incarnation may be reported again.
    crate::agent_teams::forget_term(WORKER);
    assert!(
        !super::taken_terms()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .contains(&WORKER),
        "the gate outlived the terminal"
    );
    crate::agent_teams::forget_term(LEADER);
}

/// The window's seat answer rides the split's own capability back and
/// lands on the worker row, durably — and a host that answers nothing
/// leaves the row honestly UNREPORTED, which the fourth group's refusal
/// counts out loud rather than reading as an empty checkout.
#[test]
fn a_seat_report_lands_the_checkout_on_the_worker_row() {
    const LEADER: u32 = 11_100;
    const WORKER: u32 = 11_101;
    let _window = the_window();
    let team = format!("team-seat-{LEADER}");
    seat_a_team(&team, LEADER);
    let host = Seating {
        onto: WORKER,
        checkout: "/wt/host-answer",
    };
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    let say = |host: &dyn Host, at: &str, line: &str| {
        let answered = run(
            host,
            Vec::new(),
            &team,
            at,
            TEST_CAPABILITY,
            &words(line),
            clock(),
        );
        assert_eq!(answered.exit_code, 0, "{line}: {}", answered.stderr);
        serde_json::from_str::<serde_json::Value>(&answered.stdout).expect("JSON")
    };
    say(&host, seat, "run-create --name seated");
    let started = say(&host, seat, "worker-start --agent claude");
    let worker = started["workerId"]
        .as_str()
        .expect("a worker id")
        .to_string();
    let pane = started["pane"].as_str().expect("a pane").to_string();

    // The fact rides out where a coordinator reads workers, and it is
    // durable — the projection row holds it, not a window-side map.
    let shown = say(&host, seat, &format!("worker-show --worker {worker}"));
    assert_eq!(shown["checkout"], "/wt/host-answer");
    assert_eq!(
        the_rows()
            .workers
            .iter()
            .find(|row| row.id == worker)
            .expect("the worker row")
            .checkout
            .as_deref(),
        Some("/wt/host-answer"),
        "the seat report never reached the store"
    );

    // And the fourth group resolves against it: mail addressed to the
    // checkout lands in that pane's inbox — read with the pane's OWN
    // capability, the one the split minted it.
    say(
        &host,
        seat,
        "send --to @worktree:/wt/host-answer --type status --body over-there",
    );
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let mail = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words("check"),
        clock(),
    );
    assert_eq!(mail.exit_code, 0, "{}", mail.stderr);
    let mail: serde_json::Value = serde_json::from_str(&mail.stdout).expect("mail");
    assert_eq!(mail["count"], 1, "{mail}");
    assert_eq!(mail["messages"][0]["body"], "over-there");

    // A host with no answer leaves the next row unreported — absence of
    // the fact, visible as `null`, never a guessed location.
    let silent = Splitting::onto(WORKER + 1);
    let second = say(&silent, seat, "worker-start --agent codex");
    let quiet = second["workerId"]
        .as_str()
        .expect("a worker id")
        .to_string();
    let shown = say(&host, seat, &format!("worker-show --worker {quiet}"));
    assert!(
        shown["checkout"].is_null(),
        "an unreported seat wore a location: {shown}"
    );

    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(WORKER + 1);
}

/// Dedupe compares the complete provider session. Providers commonly
/// publish the id first and attach the transcript path on a later event;
/// treating the id alone as the key would lose that durable path.
#[test]
fn a_late_transcript_path_is_not_deduped_away() {
    const LEADER: u32 = 11_110;
    const WORKER: u32 = 11_111;
    let _window = the_window();
    let team = format!("team-session-path-{LEADER}");
    let (_run, worker, _pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let first = zerocode_core::ProviderSession {
        key: zerocode_core::provider_session::SessionKey::SessionId,
        id: "same-provider-id".to_string(),
        transcript_path: None,
    };
    super::pane_session_reported(WORKER, &first, clock());

    let completed = zerocode_core::ProviderSession {
        transcript_path: Some("/transcripts/late.jsonl".to_string()),
        ..first
    };
    super::pane_session_reported(WORKER, &completed, clock());
    let carried = the_rows()
        .workers
        .into_iter()
        .find(|row| row.id == worker)
        .and_then(|row| row.session)
        .expect("the worker has a durable session");
    assert_eq!(carried, completed);

    crate::agent_teams::forget_term(WORKER);
    assert!(
        !super::reported_sessions()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .contains_key(&WORKER),
        "the session gate outlived the terminal incarnation"
    );
    crate::agent_teams::forget_term(LEADER);
}

/// A store refusal must not fill the dedupe map. Once the authority store
/// recovers, the next identical hook report walks the actor again and the
/// session reaches disk.
#[test]
fn a_failed_session_write_is_retried_on_the_next_hook() {
    const LEADER: u32 = 11_112;
    const WORKER: u32 = 11_113;
    let (window, store) = PrivateWindow::boot();
    let team = format!("team-session-retry-{LEADER}");
    let (_run, worker, _pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let session = zerocode_core::ProviderSession {
        key: zerocode_core::provider_session::SessionKey::SessionId,
        id: "retry-this-session".to_string(),
        transcript_path: None,
    };
    let connection = store
        .fault_connection_for_tests()
        .expect("fault connection");
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_session_write
                    AFTER UPDATE OF revision ON orchestration_ledger_heads
                    BEGIN SELECT RAISE(ABORT, 'injected session refusal'); END;",
        )
        .expect("the disk refuses the session");

    super::pane_session_reported(WORKER, &session, clock());
    assert!(
        !super::reported_sessions()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .contains_key(&WORKER),
        "a session the disk refused entered the dedupe map"
    );
    assert!(
        the_rows()
            .workers
            .iter()
            .find(|row| row.id == worker)
            .is_some_and(|row| row.session.is_none()),
        "the refused session remained in the actor image"
    );

    connection
        .execute_batch("DROP TRIGGER refuse_session_write;")
        .expect("the disk comes back");
    super::pane_session_reported(WORKER, &session, clock());
    assert!(
        the_rows()
            .workers
            .iter()
            .find(|row| row.id == worker)
            .and_then(|row| row.session.as_ref())
            .is_some_and(|held| held == &session),
        "the identical next hook was deduped after the refusal"
    );

    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);
    drop(window);
}

/// A restored agent can report before its terminal has been rebound to the
/// sleeping worker. `moved: false` is not a dedupe hit: replaying the same
/// report after the seat appears must persist it.
#[test]
fn a_session_seen_before_reseat_is_replayed_after_reseat() {
    const LEADER: u32 = 11_114;
    const WORKER: u32 = 11_115;
    let (window, _store) = PrivateWindow::boot();
    let session = zerocode_core::ProviderSession {
        key: zerocode_core::provider_session::SessionKey::SessionId,
        id: "raced-the-reseat".to_string(),
        transcript_path: None,
    };

    super::pane_session_reported(WORKER, &session, clock());
    assert!(
        !super::reported_sessions()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .contains_key(&WORKER),
        "an unseated report was cached"
    );

    let team = format!("team-session-reseat-{LEADER}");
    let (_run, worker, _pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    super::pane_session_reported(WORKER, &session, clock());
    assert!(
        the_rows()
            .workers
            .iter()
            .find(|row| row.id == worker)
            .and_then(|row| row.session.as_ref())
            .is_some_and(|held| held == &session),
        "the report was not replayed after the seat appeared"
    );

    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);
    drop(window);
}

#[test]
fn federation_join_accepts_type_checked_loopback_addresses() {
    for addr in [
        "127.0.0.1:1",
        "127.0.0.2:1",
        "[::1]:1",
        "[::ffff:127.0.0.1]:1",
        "localhost:1",
    ] {
        assert!(super::loopback_only(addr).is_ok(), "{addr} was refused");
    }
}

#[test]
fn federation_join_rejects_non_loopback_addresses_for_plaintext_transport() {
    for addr in ["10.0.0.9:1", "[2001:db8::9]:1", "not-an-address"] {
        let refused = super::loopback_only(addr).expect_err("a public address was accepted");
        assert!(
            refused.contains("plaintext"),
            "the refusal did not explain the transport boundary: {refused}"
        );
        if addr != "not-an-address" {
            assert!(refused.contains("ssh tunnel"), "{refused}");
        }
    }
}

#[test]
fn server_entry_debug_redacts_transport_token() {
    let token = "transport-token-that-must-not-appear";
    let entry = super::ServerEntry {
        addr: "127.0.0.1:1".to_string(),
        token: token.to_string(),
    };
    let debug = format!("{entry:?}");
    assert!(
        !debug.contains(token),
        "the transport token leaked: {debug}"
    );
    assert!(
        debug.contains("token_bytes"),
        "the token length was not retained: {debug}"
    );
}

#[test]
fn legacy_federation_tokens_get_an_actionable_migration_warning() {
    let warning = super::legacy_federation_token_warning("worker-1", "old-bridge-token")
        .expect("a bridge-wide token was treated as scoped");
    for action in [
        "federation-invite",
        "federation-forget worker-1",
        "federation-join worker-1",
    ] {
        assert!(
            warning.contains(action),
            "the warning omitted {action}: {warning}"
        );
    }

    let scoped = zerocode_hookd::federation_token("bridge-token");
    assert!(
        super::legacy_federation_token_warning("worker-1", scoped.as_str()).is_none(),
        "a scoped token was called legacy"
    );
}

#[test]
fn a_saved_public_server_is_reported_as_a_migration_not_sent() {
    let refused = super::canonical_server_address("wide", "10.0.0.9:1")
        .expect_err("a saved public address was accepted");
    assert!(refused.contains("plaintext"), "{refused}");
    assert!(
        refused.contains("federation-forget wide"),
        "the migration path was not explained: {refused}"
    );
}

/// V-1, the check-then-use question. The address book is a plain file:
/// an entry can be repointed between the join that validated it and the
/// relay call that spends this window's transport token on it. So the
/// relay half has to ask again — and it has to ask BEFORE the socket,
/// because a refusal that reads "could not reach" is a refusal that
/// already carried the token to the far address.
#[test]
fn a_book_repointed_after_the_join_is_refused_before_the_socket_opens() {
    let window = the_window();
    let root = super::federation_root().expect("the window has a data root");
    std::fs::create_dir_all(&root).expect("federation dir");
    let book = root.join("servers.json");
    let held = std::fs::read_to_string(&book).ok();

    // What a hand-edit — or a pre-validation entry written by an older
    // build — can leave standing behind a name the relay already trusts.
    std::fs::write(
        &book,
        serde_json::json!({
            "repointed": { "addr": "203.0.113.9:7791", "token": "must-not-travel" },
        })
        .to_string(),
    )
    .expect("a repointed book");

    let refused = super::call_server(
        "repointed",
        &serde_json::json!({ "verb": "pull" }),
        std::time::Duration::from_millis(200),
    )
    .expect_err("the relay dialled a public address out of the book");

    match held {
        Some(held) => std::fs::write(&book, held).expect("the book restored"),
        None => std::fs::remove_file(&book).expect("the book removed"),
    }
    drop(window);

    assert!(
        !refused.contains("could not reach"),
        "the address was checked after the socket, so the token already left: {refused}"
    );
    assert!(
        refused.contains("federation-forget repointed"),
        "the relay did not name the migration: {refused}"
    );
}

/// V-3, the several-answers question. A name can resolve to more than
/// one address, and the two readings of that are not equally safe: "one
/// answer is loopback, so allow it" hands the transport a destination
/// nobody chose, while "one answer is not loopback, so refuse" keeps the
/// wire's promise. Asserted against a hand-built resolution, because a
/// test cannot make the resolver answer twice on demand.
#[test]
fn a_name_that_also_resolves_publicly_is_refused_whole() {
    let loopback = "127.0.0.1:7791".parse().expect("a loopback endpoint");
    let public = "203.0.113.9:7791".parse().expect("a public endpoint");

    for resolution in [vec![loopback, public], vec![public, loopback]] {
        let refused = super::chosen_loopback_endpoint(&resolution, "two-faced.example")
            .expect_err("a name that also resolves publicly was joined");
        assert!(
            refused.contains("plaintext"),
            "the refusal did not explain the transport boundary: {refused}"
        );
    }
    assert_eq!(
        super::chosen_loopback_endpoint(&[loopback], "one-faced.example"),
        Ok(loopback),
        "a name that resolves only to loopback must stay joinable"
    );
    let refused = super::chosen_loopback_endpoint(&[], "silent.example")
        .expect_err("a name that resolved to nothing was joined");
    assert!(refused.contains("did not resolve"), "{refused}");
}

/// The endpoint a join freezes into the book has to be one the far
/// bridge is listening on. `zerocode_hookd` binds IPv4 loopback and
/// nothing else, while `getaddrinfo` answers `localhost` with `::1`
/// first on most hosts — so an endpoint taken off the head of the
/// resolution names a socket no federation server ever answers, and a
/// join the book reported as a success dies on every relay call after
/// it. Reachability is the property here; acceptance is not.
#[test]
fn a_named_loopback_join_stores_an_endpoint_the_bridge_answers() {
    use std::net::ToSocketAddrs as _;

    // The listener zerocode_hookd opens, in the family it opens it in.
    let bridge = std::net::TcpListener::bind(std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        0,
    ))
    .expect("a bridge-shaped listener");
    let named = format!(
        "localhost:{}",
        bridge.local_addr().expect("the bridge's port").port()
    );
    let order: Vec<std::net::SocketAddr> = named
        .to_socket_addrs()
        .expect("localhost resolves")
        .collect();

    let stored = super::loopback_only(&named).expect("localhost is a loopback name");
    if let Err(why) =
        std::net::TcpStream::connect_timeout(&stored, std::time::Duration::from_secs(5))
    {
        panic!(
            "the join stored {stored} for {named}, which the bridge does not answer \
                 ({why}) — {named} resolves to {order:?} and the bridge binds IPv4 loopback"
        );
    }
}

/// The join VERB, not just its helpers. Two things have to be true of
/// what it leaves behind: the endpoint it wrote is one the far bridge
/// answers, and a token that could forge a header never reached the book
/// at all — a guard that lives only in a helper is a guard nobody proved
/// was wired in.
#[test]
fn the_join_verb_writes_a_reachable_endpoint_and_refuses_a_forged_token() {
    let window = the_window();
    let root = super::federation_root().expect("the window has a data root");
    std::fs::create_dir_all(&root).expect("federation dir");
    let book = root.join("servers.json");
    let held = std::fs::read_to_string(&book).ok();
    let _ = std::fs::remove_file(&book);

    // The listener zerocode_hookd opens, in the family it opens it in.
    let bridge = std::net::TcpListener::bind(std::net::SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        0,
    ))
    .expect("a bridge-shaped listener");
    let named = format!(
        "localhost:{}",
        bridge.local_addr().expect("the bridge's port").port()
    );
    let join = |name: &str, token: &str| {
        super::federation_book_verbs(&[
            "federation-join".to_string(),
            name.to_string(),
            "--addr".to_string(),
            named.clone(),
            "--token".to_string(),
            token.to_string(),
        ])
        .expect("federation-join is a book verb")
    };

    let forged = join("forged", "good\r\nx-zerocode-hook-token: stolen");
    let after_forgery = super::server_book().contains_key("forged");
    let joined = join("reachable", "0f3c9d1ab27e4f5089cc");
    let stored = super::server_book()
        .get("reachable")
        .map(|entry| entry.addr.clone());
    let listed = super::federation_book_verbs(&["federation-servers".to_string()])
        .expect("federation-servers is a book verb");

    match held {
        Some(held) => std::fs::write(&book, held).expect("the book restored"),
        None => std::fs::remove_file(&book).expect("the book removed"),
    }
    drop(window);

    assert_ne!(forged.exit_code, 0, "a forged token was joined: {forged:?}");
    assert!(
        !after_forgery,
        "a token that is not header material reached the book"
    );
    assert_eq!(
        joined.exit_code, 0,
        "an ordinary join was refused: {joined:?}"
    );
    assert!(
        joined.stdout.contains("legacy bridge-wide token"),
        "the join hid its migration warning: {joined:?}"
    );
    assert!(
        listed.stdout.contains("legacy bridge-wide token"),
        "the saved entry hid its migration warning: {listed:?}"
    );
    let stored = stored.expect("the join wrote the book");
    let endpoint: std::net::SocketAddr = stored
        .parse()
        .expect("the join stored a canonical endpoint");
    if let Err(why) =
        std::net::TcpStream::connect_timeout(&endpoint, std::time::Duration::from_secs(5))
    {
        panic!("the book stored {stored} for {named}, which the bridge does not answer: {why}");
    }
}

/// `::127.0.0.1` is not `::ffff:127.0.0.1`. The first is the deprecated
/// IPv4-COMPATIBLE form (RFC 4291 §2.5.5.1) — an ordinary v6 address
/// inside `::/96` that no kernel routes to 127.0.0.1 — and
/// `Ipv6Addr::to_ipv4` answers `Some(127.0.0.1)` for it exactly as it
/// does for the mapped form. A predicate built on that call therefore
/// says "loopback" about an address which is not one, and loopback is
/// the whole of what this wire promises.
#[test]
fn the_deprecated_ipv4_compatible_form_is_not_a_loopback_address() {
    for addr in ["[::127.0.0.1]:1", "[::7f00:1]:1"] {
        let refused = super::loopback_only(addr)
            .expect_err("an IPv4-compatible v6 address was called loopback");
        assert!(
            refused.contains("plaintext"),
            "the refusal did not explain the transport boundary: {refused}"
        );
    }
    assert!(
        super::loopback_only("[::ffff:127.0.0.1]:1").is_ok(),
        "the IPv4-MAPPED form is a loopback address and must stay joinable"
    );
}

/// A transport token is written into a request header verbatim, so a
/// token carrying a line break does not travel as one header — it
/// forges a second one, on the socket that holds the far window's whole
/// bridge. The book takes its tokens from whoever ran
/// `federation-invite` on the far side, so the refusal belongs at JOIN,
/// where a person is reading it, and not at relay time, where nobody is.
#[test]
fn a_transport_token_that_would_forge_a_header_is_refused_at_join() {
    for token in [
        "good\r\nx-zerocode-hook-token: stolen",
        "good\nx-zerocode-hook-token: stolen",
        "good\rstill-not-one-header",
        "good\u{0}byte",
        "good\u{2028}byte",
    ] {
        let refused = super::usable_transport_token(token)
            .expect_err("a token that is not header material was accepted");
        assert!(
            refused.contains("header material"),
            "the refusal did not name the problem: {refused}"
        );
    }
    assert!(
        super::usable_transport_token("0f3c9d1ab27e4f5089cc").is_ok(),
        "an ordinary minted token must stay joinable"
    );
}

/// The whole federation wire, walked without the wire: a home's attach
/// summons through the NAMED team's own worker-start — durable retry
/// name and all, so a replayed attach answers the standing seat — the
/// worker's ordinary verbs ride the relay out, home control mail lands
/// in its inbox, the wrong home reads as a missing dispatch, and the
/// stop ends the pane through worker-release, archive kept. Every step
/// IS the local road; that is the entire design.
#[test]
fn a_federated_attach_summons_relays_and_stops_through_local_law() {
    const LEADER: u32 = 11_120;
    const WORKER: u32 = 11_121;
    let _window = the_window();
    let team = format!("team-fed-{LEADER}");
    seat_a_team(&team, LEADER);
    // Name the host team: the table is process-global and shared with
    // every parallel test, and guessing is exactly what naming forbids.
    let root = super::federation_root().expect("the window has a data root");
    std::fs::create_dir_all(&root).expect("federation dir");
    std::fs::write(root.join("host-team"), &team).expect("host-team named");

    let host = Seating {
        onto: WORKER,
        checkout: "/wt/fed",
    };
    let serve = |home: &str, body: serde_json::Value| -> serde_json::Value {
        serde_json::from_str(&super::federation_serve(&host, home, &body))
            .expect("federation answers JSON")
    };
    let attach = serde_json::json!({
        "verb": "attach", "dispatch": "dp-home-1",
        "agent": "claude", "prompt": "cross-build carefully",
    });
    let said = serve("home-fp-xyz", attach.clone());
    assert_eq!(said["state"], "ready", "{said}");
    let worker = said["workerId"].as_str().expect("a worker").to_string();
    let pane = said["pane"].as_str().expect("a pane").to_string();
    // A replayed attach is the standing seat, not a second pane — the
    // summons rode a durable retry name derived from (home, dispatch).
    let again = serve("home-fp-xyz", attach);
    assert_eq!(
        again["workerId"],
        worker.as_str(),
        "a replay summoned a second pane: {again}"
    );

    // The worker speaks ordinary verbs through its own capability, and
    // the news rides the relay out.
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted a capability");
    let spoke = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words("send --type status --body halfway"),
        clock(),
    );
    assert_eq!(spoke.exit_code, 0, "{}", spoke.stderr);
    let pulled = serve(
        "home-fp-xyz",
        serde_json::json!({"verb":"pull","dispatch":"dp-home-1","afterSeq":0}),
    );
    assert_eq!(pulled["items"][0]["body"], "halfway", "{pulled}");
    // The wrong home reads as a missing dispatch, word for word.
    let stranger = serve(
        "somebody-else",
        serde_json::json!({"verb":"pull","dispatch":"dp-home-1","afterSeq":0}),
    );
    assert!(
        stranger["refused"]
            .as_str()
            .expect("a refusal")
            .contains("was not found"),
        "{stranger}"
    );

    // Home control mail lands in the worker's own inbox, exactly once.
    let brought = serve(
        "home-fp-xyz",
        serde_json::json!({"verb":"import","dispatch":"dp-home-1","items":[
            {"seq":1,"kind":"status","body":"keep-going","message":"m-h-1"}
        ]}),
    );
    assert_eq!(brought["acknowledgedThrough"], 1, "{brought}");
    let mail = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words("check"),
        clock(),
    );
    assert_eq!(mail.exit_code, 0, "{}", mail.stderr);
    let mail: serde_json::Value = serde_json::from_str(&mail.stdout).expect("mail");
    assert_eq!(mail["messages"][0]["body"], "keep-going", "{mail}");

    let acked = serve(
        "home-fp-xyz",
        serde_json::json!({"verb":"ack","dispatch":"dp-home-1","throughSeq":1}),
    );
    assert_eq!(acked["acknowledgedThrough"], 1, "{acked}");

    // The stop ends the pane through worker-release — archive kept —
    // and a second stop answers the settled state instead of a second
    // ending.
    let stopped = serve(
        "home-fp-xyz",
        serde_json::json!({"verb":"stop","dispatch":"dp-home-1"}),
    );
    assert_eq!(stopped["state"], "stopped", "{stopped}");
    let already = serve(
        "home-fp-xyz",
        serde_json::json!({"verb":"stop","dispatch":"dp-home-1"}),
    );
    assert_eq!(already["alreadySettled"], true, "{already}");

    let _ = std::fs::remove_file(root.join("host-team"));
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// A federated stop releases through the team that OWNS the worker.
///
/// Two tables stand. One is LEADERLESS — the shape `forget_term` leaves
/// when a leader exits over its children: the leader pane record gone,
/// no capability behind it, the table kept for the orphans — and the
/// borrowed worker belongs to the other. Which is which is decided by
/// the table itself: whichever of the two it yields first is made the
/// leaderless one, so a stop that picks "some standing team" is handed
/// a seat with no capability, the runtime refuses the empty request as
/// invalid, and the home is told `stop_unknown` for a pane that is still
/// there. A stop that reads the worker's own row answers `stopped`,
/// whatever else is standing.
#[test]
fn a_federated_stop_releases_through_the_team_that_owns_the_worker() {
    const FIRST: u32 = 11_140;
    const SECOND: u32 = 11_141;
    const WORKER: u32 = 11_142;
    let _window = the_window();
    let first = format!("team-fed-first-{FIRST}");
    let second = format!("team-fed-second-{SECOND}");
    let owner = {
        let mut tables = crate::agent_teams::teams();
        for (id, term) in [(&first, FIRST), (&second, SECOND)] {
            tables.insert(
                id.clone(),
                zerocode_core::agent_teams::Team::new(id.clone(), TEST_CAPABILITY, term),
            );
        }
        let yielded = tables
            .keys()
            .find(|id| **id == first || **id == second)
            .cloned()
            .expect("both teams stand");
        let leaderless = tables.get_mut(&yielded).expect("the yielded table");
        leaderless.remove_pane(zerocode_core::agent_teams::LEADER_PANE);
        if yielded == first {
            second.clone()
        } else {
            first.clone()
        }
    };
    let _ = crate::agent_teams::remember_pane_token(
        &owner,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY.to_string(),
    );
    let root = super::federation_root().expect("the window has a data root");
    std::fs::create_dir_all(&root).expect("federation dir");
    std::fs::write(root.join("host-team"), &owner).expect("host-team named");

    let host = Seating {
        onto: WORKER,
        checkout: "/wt/fed-owner",
    };
    let serve = |home: &str, body: serde_json::Value| -> serde_json::Value {
        serde_json::from_str(&super::federation_serve(&host, home, &body))
            .expect("federation answers JSON")
    };
    let said = serve(
        "home-fp-owner",
        serde_json::json!({
            "verb": "attach", "dispatch": "dp-owner-1",
            "agent": "claude", "prompt": "cross-build carefully",
        }),
    );
    assert_eq!(said["state"], "ready", "{said}");
    let worker = said["workerId"].as_str().expect("a worker").to_string();
    let pane = said["pane"].as_str().expect("a pane").to_string();
    assert!(
        crate::agent_teams::current_pane_capability(&owner, &pane).is_some(),
        "the pane was not cut from the named team"
    );

    let stopped = serve(
        "home-fp-owner",
        serde_json::json!({"verb":"stop","dispatch":"dp-owner-1"}),
    );
    assert_eq!(stopped["state"], "stopped", "{stopped}");
    assert_eq!(stopped["workerId"], worker.as_str(), "{stopped}");

    let _ = std::fs::remove_file(root.join("host-team"));
    crate::agent_teams::forget_term(FIRST);
    crate::agent_teams::forget_term(SECOND);
    crate::agent_teams::forget_term(WORKER);
}

/// The federation files are process-global (one machine, one book), so
/// the tests that write them take this in turn.
fn the_federation_files() -> std::sync::MutexGuard<'static, ()> {
    static TURN: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    TURN.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|held| held.into_inner())
}

/// The wire itself, end to end over real TCP: the book verbs register a
/// server (loopback only — a public address is refused at join time),
/// `worker-start --on` walks an HTTP attach through a live hookd into
/// this same window's serving half, the borrowed worker's `worker_done`
/// rides the relay pass home — pull, absorb, settlement-carrying ack —
/// and the HOME dispatch settles: task completed, coordinator told.
/// One machine playing both ends is the loopback topology the transport
/// is built for; only the GUI is absent.
#[test]
#[ignore = "a live TCP server and its runtime are real load on a process running a \
                thousand parallel worlds — run alone: cargo test … the_wire_itself -- --ignored"]
fn the_wire_itself_carries_a_federation_round_trip() {
    const LEADER: u32 = 11_130;
    const WORKER: u32 = 11_131;
    let _window = the_window();
    let _turn = the_federation_files();
    // A live TCP server, a pump thread and a runtime are real load on a
    // process running a thousand parallel worlds — the timing-sensitive
    // families take their guards in turn, and so does this.
    let _beat = one_beat_at_a_time();
    let _wait = one_wait_at_a_time();
    let team = format!("team-fedwire-{LEADER}");
    seat_a_team(&team, LEADER);
    let root = super::federation_root().expect("a data root");
    std::fs::create_dir_all(&root).expect("federation dir");
    std::fs::write(root.join("host-team"), &team).expect("host-team");

    // A live hookd, and a pump standing in for the window's async half.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime");
    const WIRE_TOKEN: &str = "fedwire-transport-token";
    let (state, _events, _teams, _browser, _computer, mut calls) =
        zerocode_hookd::BridgeState::new_with_computer(WIRE_TOKEN, "b", "c");
    let (addr, _serving) = runtime
        .block_on(zerocode_hookd::serve(state, 0))
        .expect("hookd binds");
    let pump = std::thread::spawn(move || {
        while let Some(call) = calls.blocking_recv() {
            let host = Seating {
                onto: WORKER,
                checkout: "/wt/fedwire",
            };
            let said = super::federation_serve(&host, &call.home, &call.body);
            let _ = call.answer.send(said);
        }
    });

    let host = Seating {
        onto: WORKER,
        checkout: "/wt/fedwire",
    };
    let say = |line: &str| {
        let answered = run(
            &host,
            Vec::new(),
            &team,
            zerocode_core::agent_teams::LEADER_PANE,
            TEST_CAPABILITY,
            &words(line),
            clock(),
        );
        assert_eq!(answered.exit_code, 0, "{line}: {}", answered.stderr);
        serde_json::from_str::<serde_json::Value>(&answered.stdout).expect("JSON")
    };

    // The book: a public address is refused where a person can fix it,
    // and the loopback one lands — visible, token shortened.
    let public = run(
        &host,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(&format!(
            "federation-join wide --addr 10.0.0.9:{} --token {WIRE_TOKEN}",
            addr.port()
        )),
        clock(),
    );
    assert_eq!(public.exit_code, 1);
    assert!(public.stderr.contains("ssh tunnel"), "{}", public.stderr);
    say(&format!(
        "federation-join loop-1 --addr 127.0.0.1:{} --token {WIRE_TOKEN}",
        addr.port()
    ));
    let listed = say("federation-servers");
    assert_eq!(listed["servers"][0]["name"], "loop-1", "{listed}");
    assert!(
        !listed["servers"][0]["token"]
            .as_str()
            .expect("a token")
            .contains(WIRE_TOKEN),
        "the book printed a whole token: {listed}"
    );

    // The home half: claim here, seat over the wire.
    say(&format!(
        "run-create --name fedwire --retry-request fw-open-{LEADER}"
    ));
    let task = say(&format!(
        "task-create --spec cross-build --retry-request fw-task-{LEADER}"
    ));
    let task_id = task["taskId"].as_str().expect("an id").to_string();
    let started = say(&format!(
        "worker-start --agent claude --task {task_id} --on loop-1 \
             --retry-request fw-start-{LEADER}"
    ));
    assert_eq!(started["stage"], "ready", "{started}");
    let far_pane = started["remote"]["pane"]
        .as_str()
        .expect("a pane")
        .to_string();

    // The borrowed worker finishes, in its own pane, with its own verbs.
    let held = crate::agent_teams::current_pane_capability(&team, &far_pane)
        .expect("the summons minted a capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &far_pane,
        &held,
        &words(&format!(
            "send --type worker_done --retry-request fw-done-{LEADER} \
                 --body {{\"ok\":true,\"summary\":\"built\"}}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);

    // One relay pass carries it home and settles the dispatch.
    super::federation_relay_pass();
    let tasks = say("task-list");
    assert_eq!(tasks["tasks"][0]["status"], "completed", "{tasks}");
    let mail = say("check");
    let news = mail["messages"]
        .as_array()
        .expect("mail")
        .iter()
        .find(|one| one["type"] == "worker_done")
        .unwrap_or_else(|| panic!("no worker_done rode home: {mail}"));
    assert!(
        news["from"]
            .as_str()
            .expect("a sender")
            .starts_with("remote:"),
        "{news}"
    );

    /* The pointer machinery reads every run's unread mail, and a test
     * that leaves a batch standing steals the next test's pointer. Both
     * coordinators' inboxes are drained: the home run's, and the
     * federation run's (the worker_done was rerouted there too). */
    let fed_run = started["remote"]["run"]
        .as_str()
        .expect("a run")
        .to_string();
    for (scope, tag) in [
        (String::new(), "home"),
        (format!(" --run {fed_run}"), "fed"),
    ] {
        let batch = say(&format!("check{scope}"));
        if let Some(delivery) = batch["deliveryId"].as_str() {
            say(&format!(
                "check{scope} --ack {delivery} --retry-request fw-sweep-{LEADER}-{tag}"
            ));
        }
    }
    say("federation-forget loop-1");
    let _ = std::fs::remove_file(root.join("host-team"));
    drop(runtime);
    drop(pump);
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// `agentWait` is three-valued and absence is never "not waiting": a
/// pane no hook ever reported stays ABSENT from the answer, an evaluated
/// quiet pane lays an explicit `null`, and a waiting composer carries
/// the hook evidence with when the wait began. The ledger's own rows
/// travel untouched underneath.
#[test]
fn a_worker_observation_carries_the_windows_wait_verdict() {
    const LEADER: u32 = 11_090;
    const WORKER: u32 = 11_091;
    let _window = the_window();
    let team = format!("team-attn-{LEADER}");
    let (_run_id, worker, _pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let show = || {
        let answered = run(
            &Nowhere,
            Vec::new(),
            &team,
            leader,
            TEST_CAPABILITY,
            &words(&format!("worker-show --worker {worker}")),
            clock(),
        );
        assert_eq!(answered.exit_code, 0, "{}", answered.stderr);
        serde_json::from_str::<serde_json::Value>(&answered.stdout).expect("a worker")
    };

    // Never evaluated: the field is not there at all.
    assert!(
        show().get("agentWait").is_none(),
        "an unevaluated pane printed a verdict"
    );
    // Evaluated and quiet: an explicit null, which must not print the
    // same as never-looked.
    super::pane_attention_noted(WORKER, None, 4_000);
    let seen = show();
    assert!(seen.get("agentWait").is_some());
    assert!(seen["agentWait"].is_null());
    // Waiting on the person: the evidence and its clock.
    super::pane_attention_noted(WORKER, Some(4_242), 4_300);
    let seen = show();
    assert_eq!(seen["agentWait"]["source"], "hook");
    assert_eq!(seen["agentWait"]["since"], 4_242);
    // The list wears the same verdict on the same worker's row.
    let listed = run(
        &Nowhere,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("worker-list"),
        clock(),
    );
    let listed: serde_json::Value = serde_json::from_str(&listed.stdout).expect("a roster");
    let row = listed["workers"]
        .as_array()
        .expect("workers")
        .iter()
        .find(|row| row["workerId"] == worker.as_str())
        .expect("the worker's row");
    assert_eq!(row["agentWait"]["since"], 4_242);
    // The terminal's next life starts unevaluated again.
    super::forget_pane_attention(WORKER);
    assert!(show().get("agentWait").is_none());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// Every road OUT of the split arm sweeps the worktree ask — here the
/// road where the host refuses the pane and never spends it. The guard's
/// drop is the mechanism; without this red, deleting the guard leaves an
/// orphan ask standing for some later token to collide with.
#[test]
fn a_refused_split_sweeps_its_worktree_ask() {
    const LEADER: u32 = 11_040;
    let _window = the_window();
    let team = format!("team-worktree-refused-{LEADER}");
    seat_a_team(&team, LEADER);

    /// Refuses every pane, but remembers the capability it was offered —
    /// WITHOUT spending the ask, the way a host that fails before the
    /// take would.
    struct Refusing {
        tokens: Mutex<Vec<String>>,
    }
    impl Host for Refusing {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            token: &str,
        ) -> Option<u32> {
            self.tokens
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push(token.to_string());
            None
        }
        fn send(&self, _term: u32, _text: &str) -> bool {
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            Some(String::new())
        }
        fn focus(&self, _term: u32) -> bool {
            true
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = Refusing {
        tokens: Mutex::new(Vec::new()),
    };
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("run-create --name swept"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let carried = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("task-create --spec slice"),
        clock(),
    );
    let carried: serde_json::Value = serde_json::from_str(&carried.stdout).expect("a task");
    let task = carried["taskId"].as_str().expect("a task id").to_string();

    let refused = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&format!(
            "worker-start --agent claude --task {task} --worktree"
        )),
        clock(),
    );
    assert_eq!(refused.exit_code, 1, "{}", refused.stdout);
    let token = host
        .tokens
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .last()
        .expect("the refusal was offered a capability")
        .clone();
    assert_eq!(
        crate::agent_teams::take_worker_host_ask(&token),
        None,
        "a refused split left its worktree ask standing"
    );
    crate::agent_teams::forget_term(LEADER);
}

#[test]
fn a_zo_workers_wait_names_the_event_channel_as_its_source() {
    const LEADER: u32 = 11_092;
    const WORKER: u32 = 11_093;
    let _window = the_window();
    let team = format!("team-zo-attn-{LEADER}");
    seat_a_team(&team, LEADER);
    let host = Splitting::onto(WORKER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("run-create --name zo-attention"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let started = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("worker-start --agent zo"),
        clock(),
    );
    assert_eq!(started.exit_code, 0, "{}", started.stderr);
    let started: serde_json::Value = serde_json::from_str(&started.stdout).expect("a worker");
    let worker = started["workerId"].as_str().expect("a worker id");

    super::pane_attention_noted(WORKER, Some(4_243), 4_300);
    let shown = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&format!("worker-show --worker {worker}")),
        clock(),
    );
    let shown: serde_json::Value = serde_json::from_str(&shown.stdout).expect("a worker row");
    assert_eq!(shown["agentWait"]["source"], "channel");

    super::forget_pane_attention(WORKER);
    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);
}

/// The mail pointer, end to end on the beat: advice typed at the idle
/// coordinator, Enter on the next beat, silence for mail already leased,
/// and a fresh pointer when fresh mail supersedes the watermark.
#[test]
fn unsubmitted_pointer_never_acquires_a_durable_delivery_watermark() {
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let run = "test-coordinator-unsubmitted-pointer";
    let address = "run:test-coordinator-unsubmitted-pointer";
    let newest = "test-mail-unsubmitted";
    let (send, receive) = std::sync::mpsc::channel();
    super::pointer_receipts()
        .lock()
        .unwrap()
        .insert((run.to_string(), address.to_string()), receive);
    send.send(zerocode_pty::DeliveryOutcome::Unsubmitted(
        zerocode_pty::ready::Refusal::HandReached,
    ))
    .unwrap();
    let _ = super::advice_standing(run, address, 11_009, newest, false);
    assert!(
        !super::delivered_before(run, address, newest),
        "an Enter that was withheld permanently silenced unread mail"
    );
}

#[test]
fn the_beat_points_an_idle_coordinator_at_its_mail_and_enters_once() {
    const LEADER: u32 = 11_000;
    const WORKER: u32 = 11_001;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-pointer-{LEADER}");
    let (_run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);

    /// Sends always land; every keystroke is remembered.
    struct Pointing {
        sent: Mutex<Vec<(u32, String)>>,
    }
    impl Host for Pointing {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.sent
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = Pointing {
        sent: Mutex::new(Vec::new()),
    };
    let typed = |host: &Pointing| -> Vec<(u32, String)> {
        host.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    };

    // The worker reports; the coordinator has one message waiting.
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request done-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);

    // Not yet measured idle: a beat points at nothing.
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host),
        Vec::new(),
        "advice was typed at a pane nobody measured idle"
    );

    // The leader's turn ends; the next beat hands the advice line and
    // its Enter to the guarded door as ONE delivery — then nothing,
    // however many beats.
    super::pane_turn_ended(LEADER, 50, false, clock());
    super::tick(&host, &[], clock());
    let advice = zerocode_core::orchestration::pointer_text(1);
    assert_eq!(
        typed(&host),
        vec![(LEADER, advice.clone()), (LEADER, "\r".to_string())]
    );
    super::tick(&host, &[], clock());
    super::tick(&host, &[], clock());
    assert_eq!(typed(&host).len(), 2, "a settled pointer kept typing");

    // Fresh mail supersedes the watermark: advice again, for the larger
    // count, without waiting for the first to be read.
    let status = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type status --body more --retry-request more-{worker}"
        )),
        clock(),
    );
    assert_eq!(status.exit_code, 0, "{}", status.stderr);
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host).split_off(2),
        vec![
            (LEADER, zerocode_core::orchestration::pointer_text(2)),
            (LEADER, "\r".to_string()),
        ],
        "fresh mail was not pointed at as one delivery"
    );
    super::tick(&host, &[], clock());
    assert_eq!(typed(&host).len(), 4, "a settled pointer kept typing");

    // The coordinator takes the lease: the queue is empty behind it, so
    // advice stops even though the mail is not yet acknowledged — `check`
    // replays a lease on its own, and there is nothing else to name. What
    // a lease does NOT silence is mail that arrives after it; that is
    // `a_report_that_lands_behind_an_unacknowledged_lease_is_still_pointed_at`.
    let leased = run(
        &host,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words("check"),
        clock(),
    );
    assert_eq!(leased.exit_code, 0, "{}", leased.stderr);
    let before = typed(&host).len();
    super::tick(&host, &[], clock());
    assert_eq!(typed(&host).len(), before, "a leased batch was pointed at");

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// A pointer that reached Enter is written down under the window's data
/// root, so the window that comes back after a restart — its in-memory
/// marks gone, the mail still unacknowledged — does not type the same
/// line about the same message into the same composer again. Fresh mail
/// is still news and still gets its line.
#[test]
fn a_pointer_that_reached_enter_is_not_typed_again_after_a_restart() {
    const LEADER: u32 = 11_200;
    const WORKER: u32 = 11_201;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-pointer-{LEADER}");
    let (_run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);

    struct Pointing {
        sent: Mutex<Vec<(u32, String)>>,
    }
    impl Host for Pointing {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.sent
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = Pointing {
        sent: Mutex::new(Vec::new()),
    };
    let typed = |host: &Pointing| -> Vec<(u32, String)> {
        host.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    };

    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request done-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);

    // Advice, then Enter: the pointer is delivered.
    super::pane_turn_ended(LEADER, 50, false, clock());
    super::tick(&host, &[], clock());
    super::tick(&host, &[], clock());
    let advice = zerocode_core::orchestration::pointer_text(1);
    assert_eq!(
        typed(&host),
        vec![(LEADER, advice.clone()), (LEADER, "\r".to_string())]
    );

    // The window restarts: every mark is forgotten, the mail is still
    // waiting unacknowledged, the leader is measured idle again.
    super::restart_pointer_memory_for_tests();
    super::pane_turn_ended(LEADER, 50, false, clock());
    super::tick(&host, &[], clock());
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host).len(),
        2,
        "a restart typed the same pointer again: {:?}",
        typed(&host)
    );

    // Fresh mail after the restart is news, and the count names both.
    let status = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type status --body more --retry-request more-{worker}"
        )),
        clock(),
    );
    assert_eq!(status.exit_code, 0, "{}", status.stderr);
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host).split_off(2),
        vec![
            (LEADER, zerocode_core::orchestration::pointer_text(2)),
            (LEADER, "\r".to_string()),
        ],
        "fresh mail after the restart was not pointed at"
    );

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// Mail older than the news window is history, not news: the beat leaves
/// it to `check` and types nothing. The same mail, seen by a beat within
/// the window, is pointed at as usual — the age is the only difference.
#[test]
fn mail_older_than_the_news_window_is_left_to_check() {
    const LEADER: u32 = 11_300;
    const WORKER: u32 = 11_301;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-pointer-{LEADER}");
    let (_run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);

    struct Pointing {
        sent: Mutex<Vec<(u32, String)>>,
    }
    impl Host for Pointing {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.sent
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = Pointing {
        sent: Mutex::new(Vec::new()),
    };
    let typed = |host: &Pointing| -> Vec<(u32, String)> {
        host.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    };

    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request done-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);
    // The worker is done and released, so a beat a day later has no
    // silence to report about it — the only mail is the old report.
    let released = run(
        &host,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(&format!(
            "worker-release --worker {worker} --retry-request rel-{worker}"
        )),
        clock(),
    );
    assert_eq!(released.exit_code, 0, "{}", released.stderr);

    // A beat a day and a minute later finds the message old: nothing typed.
    let later = clock() + super::POINTER_NEWS_WINDOW_MS + 60_000;
    super::pane_turn_ended(LEADER, 50, false, later);
    super::tick(&host, &[], later);
    super::tick(&host, &[], later);
    assert_eq!(
        typed(&host),
        Vec::new(),
        "history was typed into the composer"
    );

    // The same mail seen in time is pointed at — the age was the reason.
    super::restart_pointer_memory_for_tests();
    super::pane_turn_ended(LEADER, 50, false, clock());
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host),
        vec![
            (LEADER, zerocode_core::orchestration::pointer_text(1)),
            (LEADER, "\r".to_string()),
        ]
    );

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// The three doors that keep the pointer polite: an interrupted turn is
/// the person's pane, a sleeper is the better pointer, and a `cursor`
/// composer submits by itself so the Enter never follows.
#[test]
fn the_pointer_yields_to_people_sleepers_and_self_submitting_composers() {
    const LEADER: u32 = 11_010;
    const WORKER: u32 = 11_011;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-polite-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);

    /// Landing sends, and an agent name for the cursor exception.
    struct CursorHost {
        sent: Mutex<Vec<(u32, String)>>,
    }
    impl Host for CursorHost {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.sent
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
        fn agent_of(&self, _term: u32) -> Option<String> {
            Some("cursor".to_string())
        }
    }
    let host = CursorHost {
        sent: Mutex::new(Vec::new()),
    };
    let typed = |host: &CursorHost| -> usize {
        host.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .len()
    };

    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request done-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);

    // An interrupted end is the person typing: no advice.
    super::pane_turn_ended(LEADER, 60, true, clock());
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host),
        0,
        "advice was typed over a person's interruption"
    );

    // A sleeper on the address is the better pointer: no advice.
    super::pane_turn_ended(LEADER, 61, false, clock());
    let card = super::WaiterCard::hold(
        &run_id,
        &format!("run:{run_id}"),
        &format!("{team}/{}", zerocode_core::agent_teams::LEADER_PANE),
    );
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host),
        0,
        "advice was typed over a sleeping check --wait"
    );
    drop(card);

    // A cursor composer gets the advice and never the Enter.
    super::tick(&host, &[], clock());
    assert_eq!(typed(&host), 1);
    super::tick(&host, &[], clock());
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host),
        1,
        "a self-submitting composer was handed an Enter"
    );

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// t-3720: a coordinator a person started by hand in a shell ends its turn at
/// rest, is quit, and then its mail lands. Nothing about the pane changed that
/// a hook would report — the last word is still `Done` — but the terminal is
/// the shell's again, and the advice line carries `zerocode-orc check` in
/// backticks, which zsh runs as a command substitution. So the pointer asks
/// the kernel who is in front before it types, says why it held back once,
/// and types again once an agent holds the terminal and ends a turn.
#[test]
fn the_pointer_is_never_typed_into_a_shell_its_agent_has_left() {
    const LEADER: u32 = 11_150;
    const WORKER: u32 = 11_151;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-quit-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);

    /// Records what is typed; the leader's shell is in front until told not.
    struct ShellHost {
        shell_in_front: std::sync::atomic::AtomicBool,
        sent: Mutex<Vec<(u32, String)>>,
    }
    impl Host for ShellHost {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.sent
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
        fn shell_in_front(&self, term: u32) -> bool {
            term == LEADER
                && self
                    .shell_in_front
                    .load(std::sync::atomic::Ordering::SeqCst)
        }
    }
    let host = ShellHost {
        shell_in_front: std::sync::atomic::AtomicBool::new(true),
        sent: Mutex::new(Vec::new()),
    };
    let typed = |host: &ShellHost| -> Vec<(u32, String)> {
        host.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    };
    let blackbox = super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    let needle = format!(
        "run:{run_id} in {run_id} could not be pointed at terminal {LEADER} {}",
        super::NO_AGENT_IN_FRONT
    );
    let said = || -> usize {
        std::fs::read_to_string(&blackbox)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(&needle))
            .count()
    };

    // The coordinator's last turn ended at rest; then it was quit.
    super::pane_turn_ended(LEADER, 150, false, clock());
    let held =
        crate::agent_teams::current_pane_capability(&team, &pane).expect("the worker's capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request quit-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);

    super::tick(&host, &[], clock());
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host),
        Vec::new(),
        "the advice line was typed into a shell whose agent had left"
    );
    assert_eq!(
        said(),
        1,
        "held back without saying why, or said it every beat"
    );

    // An agent holds the terminal again and ends a turn: pointed, once.
    host.shell_in_front
        .store(false, std::sync::atomic::Ordering::SeqCst);
    super::pane_turn_began(LEADER, clock());
    super::pane_turn_ended(LEADER, 151, false, clock());
    super::tick(&host, &[], clock());
    let advice = zerocode_core::orchestration::pointer_text(1);
    assert_eq!(
        typed(&host),
        vec![(LEADER, advice), (LEADER, "\r".to_string())],
        "the returned agent was not told about its mail"
    );

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// Mail that lands BEHIND an unacknowledged lease is still the holder's
/// news, and the pointer is the only thing that can say so.
///
/// The reproduction of the field failure this test was written for: a
/// coordinator took a batch and never acked it — its window died between
/// the `check` and the `--ack`, and the session that inherited the pane
/// had never heard of that delivery. Every report from every worker after
/// that moment went into `pending` and stayed there, unannounced, because
/// the lease said "this holder already knows". It knew about the BATCH.
/// It could not know about the eleven `worker_done`s queued behind it —
/// `deliver` replays the open batch and hands over nothing else, so no
/// amount of checking would have shown them either.
#[test]
fn a_report_that_lands_behind_an_unacknowledged_lease_is_still_pointed_at() {
    const LEADER: u32 = 11_030;
    const FIRST: u32 = 11_031;
    const SECOND: u32 = 11_032;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-stranded-{LEADER}");
    let (_run_id, _first, first_pane) = a_worker_carrying_work(&team, LEADER, FIRST);

    /// Lands the advice, and remembers every line.
    struct Pointing {
        sent: Mutex<Vec<(u32, String)>>,
    }
    impl Host for Pointing {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            Some(SECOND)
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.sent
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            Some(String::new())
        }
        fn focus(&self, _term: u32) -> bool {
            true
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = Pointing {
        sent: Mutex::new(Vec::new()),
    };
    let typed = |host: &Pointing| -> Vec<(u32, String)> {
        host.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    };

    // The first worker reports, and the idle coordinator is pointed at it.
    let first_held = crate::agent_teams::current_pane_capability(&team, &first_pane)
        .expect("the first worker's capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &first_pane,
        &first_held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request stranded-first-{LEADER}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);
    super::pane_turn_ended(LEADER, 80, false, clock());
    super::tick(&host, &[], clock());
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host),
        vec![
            (LEADER, zerocode_core::orchestration::pointer_text(1)),
            (LEADER, "\r".to_string()),
        ],
        "the first report was not pointed at"
    );

    // The coordinator takes the batch and never acknowledges it — the
    // window it was in is gone, and nothing that comes next knows the
    // delivery's name.
    let leased = run(
        &host,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words("check"),
        clock(),
    );
    assert_eq!(leased.exit_code, 0, "{}", leased.stderr);
    let leased: serde_json::Value = serde_json::from_str(&leased.stdout).expect("a batch");
    assert!(leased["deliveryId"].is_string(), "no lease was taken");
    let quiet = typed(&host).len();

    // A SECOND worker finishes. Nothing about that report is known to the
    // holder, and the lease it never acked cannot say it.
    let second = run(
        &host,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words("task-create --spec second-slice"),
        clock(),
    );
    assert_eq!(second.exit_code, 0, "{}", second.stderr);
    let second: serde_json::Value = serde_json::from_str(&second.stdout).expect("a task");
    let second_task = second["taskId"].as_str().expect("a task id").to_string();
    let started = run(
        &host,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(&format!("worker-start --agent claude --task {second_task}")),
        clock(),
    );
    assert_eq!(started.exit_code, 0, "{}", started.stderr);
    let started: serde_json::Value = serde_json::from_str(&started.stdout).expect("a worker");
    let second_pane = started["pane"].as_str().expect("a pane").to_string();
    let second_held = crate::agent_teams::current_pane_capability(&team, &second_pane)
        .expect("the second worker's capability");
    let reported = run(
        &host,
        Vec::new(),
        &team,
        &second_pane,
        &second_held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request stranded-second-{LEADER}"
        )),
        clock(),
    );
    assert_eq!(reported.exit_code, 0, "{}", reported.stderr);

    super::pane_turn_ended(LEADER, 81, false, clock());
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host).get(quiet),
        Some(&(LEADER, zerocode_core::orchestration::pointer_text(1))),
        "a worker finished and the coordinator was told nothing: the mail \
             is queued behind a lease the holder cannot ack and `check` will \
             not hand over"
    );

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(FIRST);
    crate::agent_teams::forget_term(SECOND);
}

/// A pointer that no road will carry is retried, and said once.
///
/// The native fast path is a hint that falls back and the fallback is a
/// write at a pty; when both decline, the holder has mail nothing told it
/// about. That silence used to be complete — the beat dropped the line and
/// wrote nothing anywhere — which is the same shape as the defect this
/// whole pass exists to answer. The beat still retries, so a pane that
/// comes back is pointed at once; the black box hears about it once, not
/// once a second.
#[test]
fn a_pointer_no_road_will_carry_is_retried_and_written_down_once() {
    const LEADER: u32 = 11_040;
    const WORKER: u32 = 11_041;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-mute-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);

    /// A pane that refuses every write until it is told to land them.
    struct Mute {
        landing: std::sync::atomic::AtomicBool,
        sent: Mutex<Vec<(u32, String)>>,
    }
    impl Host for Mute {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.sent
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            self.landing.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = Mute {
        landing: std::sync::atomic::AtomicBool::new(false),
        sent: Mutex::new(Vec::new()),
    };
    let tried = |host: &Mute| -> Vec<(u32, String)> {
        host.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    };

    let blackbox = super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    let needle = format!("run:{run_id} in {run_id} could not be pointed");
    let complaints = || -> usize {
        std::fs::read_to_string(&blackbox)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(&needle))
            .count()
    };
    assert_eq!(complaints(), 0, "the black box spoke before the beat did");

    let held =
        crate::agent_teams::current_pane_capability(&team, &pane).expect("the worker's capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request mute-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);
    super::pane_turn_ended(LEADER, 90, false, clock());

    // Neither road carries it: the fact is written down.
    let advice = zerocode_core::orchestration::pointer_text(1);
    super::tick(&host, &[], clock());
    assert_eq!(tried(&host), vec![(LEADER, advice.clone())]);
    assert_eq!(
        complaints(),
        1,
        "a pointer no road carried was dropped in silence"
    );

    // And the next beat tries the SAME line again without saying it twice.
    super::tick(&host, &[], clock());
    assert_eq!(
        tried(&host),
        vec![(LEADER, advice.clone()), (LEADER, advice.clone())],
        "a mute pane was given up on, or handed an Enter it never earned"
    );
    assert_eq!(complaints(), 1, "the black box was told once per beat");

    // The pane comes back: the advice lands, the Enter follows, and the
    // complaint is not repeated.
    host.landing
        .store(true, std::sync::atomic::Ordering::SeqCst);
    super::tick(&host, &[], clock());
    super::tick(&host, &[], clock());
    assert_eq!(
        tried(&host).split_off(2),
        vec![(LEADER, advice), (LEADER, "\r".to_string())]
    );
    super::tick(&host, &[], clock());
    assert_eq!(tried(&host).len(), 4, "a settled pointer kept typing");
    assert_eq!(complaints(), 1);

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// A coordinator whose SESSION changed still gets pointed at its mail.
///
/// A run binding is filed under its coordinator's actor, and an actor is
/// that pane's provider session — which is not stable. A `/clear`, a
/// resume, a respawn-pane, and the same agent at the same terminal comes
/// back under a new name; `bound_run` then finds nothing, the leader never
/// gets a seat, and its run's mail goes quiet for exactly as long as the
/// coordinator does not happen to type a bound verb. A coordinator waiting
/// on a worker's report has no reason to type one, so "for exactly as
/// long" means forever.
///
/// The run itself answers the question the binding could not: a worker
/// sits in a pane its coordinator cut, so its `team` names the
/// coordinator's team whatever the coordinator is calling itself today.
#[test]
fn a_coordinator_that_came_back_under_a_new_session_is_still_seated() {
    const LEADER: u32 = 11_070;
    const WORKER: u32 = 11_071;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-resumed-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);

    /// The same pane, under a name the ledger has never seen.
    struct Resumed {
        sent: Mutex<Vec<(u32, String)>>,
    }
    impl Host for Resumed {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.sent
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(format!("{}-after-the-resume", test_actor(term)))
        }
    }
    let host = Resumed {
        sent: Mutex::new(Vec::new()),
    };

    let held =
        crate::agent_teams::current_pane_capability(&team, &pane).expect("the worker's capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request resumed-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);
    super::pane_turn_ended(LEADER, 70, false, clock());
    super::tick(&host, &[], clock());

    assert_eq!(
        host.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone(),
        vec![
            (LEADER, zerocode_core::orchestration::pointer_text(1)),
            (LEADER, "\r".to_string()),
        ],
        "a report went unannounced because the coordinator's session id moved"
    );
    assert!(!run_id.is_empty());

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// Mail waiting on a run this window holds no seat for is REPORTED.
///
/// Every other refusal in the pointer is about a door it can see and will
/// not open. This is the case where there is no door at all: the pane
/// table is process memory, so a window that restarts comes back holding
/// no seats, the pointer's loop has nothing to iterate, and a run whose
/// mail is piling up says nothing anywhere — not typed, not logged, not on
/// a surface. `check` still owns delivery; what was missing was anybody
/// ever mentioning that delivery had stopped happening by itself.
#[test]
fn mail_for_a_run_with_no_seat_at_all_is_written_down_once() {
    const LEADER: u32 = 11_080;
    const WORKER: u32 = 11_081;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-seatless-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let host = Splitting::onto(WORKER);

    let blackbox = super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    let needle = format!("run:{run_id} in {run_id} has no seat in");
    let complaints = || -> usize {
        std::fs::read_to_string(&blackbox)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(&needle))
            .count()
    };

    let held =
        crate::agent_teams::current_pane_capability(&team, &pane).expect("the worker's capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request seatless-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);
    assert_eq!(complaints(), 0, "the black box spoke before the seat went");

    // The window comes back with an empty pane table. The mail did not go
    // anywhere; the only thing that could have pointed at it did.
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
    super::tick(&host, &[], clock());
    assert_eq!(
        complaints(),
        1,
        "a run left with no seat kept its waiting mail to itself"
    );

    // And a beat is a second long: the same silence costs one line, not
    // one line per second.
    super::tick(&host, &[], clock());
    super::tick(&host, &[], clock());
    assert_eq!(complaints(), 1, "the black box was told once per beat");
}

/// Not knowing a pane is not the same fact as the pane being busy.
///
/// The pointer's first door asks for a MEASURED rest, and it is right to:
/// advice typed into a running turn lands under somebody's hands. But a
/// pane the window has never heard from at all fails that door for a
/// different reason, and it can fail it forever — a vendor whose hooks are
/// not installed never reports, and a window that has just restarted has
/// heard nothing from anybody yet. Mail piling up behind that silence was
/// itself silent: nothing typed, and nothing said anywhere either.
///
/// Three states, one pass: never heard (named once per watermark, never
/// typed at), heard at work (left in peace AND out of the black box —
/// a working pane is not an alarm, because its turn ends by itself), and
/// heard at rest (pointed at, exactly as before).
#[test]
fn a_pane_the_window_never_heard_is_told_apart_from_one_at_work() {
    const LEADER: u32 = 11_050;
    const WORKER: u32 = 11_051;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-unheard-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);

    /// Sends always land; every keystroke is remembered.
    struct Pointing {
        sent: Mutex<Vec<(u32, String)>>,
    }
    impl Host for Pointing {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.sent
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = Pointing {
        sent: Mutex::new(Vec::new()),
    };
    let typed = |host: &Pointing| -> Vec<(u32, String)> {
        host.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    };

    let blackbox = super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    let needle =
        format!("run:{run_id} in {run_id} could not be pointed at terminal {LEADER} — the");
    let named = || -> usize {
        std::fs::read_to_string(&blackbox)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(&needle))
            .count()
    };
    assert_eq!(named(), 0, "the black box spoke before there was any mail");

    // The worker reports. The leader's pane has never said a word — no
    // hook has ever named this terminal — so nothing may be typed there.
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request unheard-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);

    // However many beats: still nothing typed, and the fact is said once.
    for _ in 0..5 {
        super::tick(&host, &[], clock());
    }
    assert_eq!(
        typed(&host),
        Vec::new(),
        "advice was typed at a pane the window has never heard from"
    );
    assert_eq!(
        named(),
        1,
        "mail waiting at a pane nothing can ever point at was dropped in silence"
    );

    // Fresh mail is fresh news: a new watermark is named again, and the
    // beats in between still cost one line and not one line per second.
    let status = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type status --body more --retry-request unheard-more-{worker}"
        )),
        clock(),
    );
    assert_eq!(status.exit_code, 0, "{}", status.stderr);
    for _ in 0..5 {
        super::tick(&host, &[], clock());
    }
    assert_eq!(
        typed(&host),
        Vec::new(),
        "a pane still unheard was typed at"
    );
    assert_eq!(named(), 2, "the black box was told once per beat");

    // Now the window HEARS the pane, at work. That is a different fact:
    // this turn will end and be measured, so the pointer waits — and the
    // black box is not told, because a working pane is not an alarm.
    super::pane_turn_began(LEADER, clock());
    for _ in 0..5 {
        super::tick(&host, &[], clock());
    }
    assert_eq!(
        typed(&host),
        Vec::new(),
        "advice landed inside a running turn"
    );
    assert_eq!(named(), 2, "a pane at work was reported as unreachable");

    // And the turn ends: the pointer does exactly what it always did —
    // the line and its Enter, as one guarded delivery.
    super::pane_turn_ended(LEADER, 90, false, clock());
    super::tick(&host, &[], clock());
    let advice = zerocode_core::orchestration::pointer_text(2);
    assert_eq!(
        typed(&host),
        vec![(LEADER, advice), (LEADER, "\r".to_string())]
    );
    super::tick(&host, &[], clock());
    assert_eq!(typed(&host).len(), 2, "a settled pointer kept typing");
    assert_eq!(
        named(),
        2,
        "a pane that was pointed at was complained about"
    );

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// A WORKING pane of a measured provider is reached without a keystroke.
///
/// This is the whole of the default-coordination answer. Mail that arrives
/// while an agent is mid-turn used to be skipped outright — wait, the turn
/// will end, and then type the advice into whatever composer is on screen
/// and hope the Enter follows. It is also the one moment the agent is most
/// reachable, because its own turn-end hook is about to knock; so the
/// pointer is left on the shelf for that knock instead, and no composer is
/// touched at all.
///
/// Three properties, and all three matter:
/// · nothing is typed while the pointer is parked;
/// · the black box says so once per watermark, not once per beat;
/// · a pointer nobody collects is taken back, and the composer road speaks
///   exactly as it always did — a native road that quietly does not work
///   on some machine must never become mail nobody is told about.
///
/// The shelf's clock stands still here (t-8938). Its grace is counted from
/// the park, and this test looks at the shelf five beats and two log reads
/// later: inside a loaded parallel suite that took longer than the three
/// seconds, the look itself called the pointer abandoned and threw it away,
/// and the hook had nothing to collect. What is asserted is what the shelf
/// HOLDS; the grace keeps its own test, aged by hand.
#[test]
fn a_working_claude_pane_is_pointed_at_through_its_own_hook_and_never_its_composer() {
    const LEADER: u32 = 11_070;
    const WORKER: u32 = 11_071;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let _stood = crate::standing_clock::stand_still();
    let team = format!("team-parked-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);

    /// Sends land; the pane holds the measured provider.
    struct Pointing {
        sent: Mutex<Vec<(u32, String)>>,
    }
    impl Host for Pointing {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.sent
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
        fn agent_of(&self, _term: u32) -> Option<String> {
            Some(zerocode_core::AgentKind::Claude.slug().to_string())
        }
    }
    let host = Pointing {
        sent: Mutex::new(Vec::new()),
    };
    let typed = |host: &Pointing| -> Vec<(u32, String)> {
        host.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    };
    let blackbox = super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    let counted = |needle: String| -> usize {
        std::fs::read_to_string(&blackbox)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(&needle))
            .count()
    };
    let parked = || {
        counted(format!(
            "mail waiting for run:{run_id} in {run_id} is parked for terminal {LEADER}"
        ))
    };
    let uncollected = || {
        counted(format!(
            "terminal {LEADER}'s turn-end hook never collected the pointer for \
                 run:{run_id} in {run_id}"
        ))
    };

    // The pane is at work when the worker's report lands.
    super::pane_turn_began(LEADER, clock());
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request parked-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);
    let reported: serde_json::Value =
        serde_json::from_str(&done.stdout).expect("worker_done answers JSON");
    let newest = reported["messageId"]
        .as_str()
        .expect("a message id")
        .to_string();

    for _ in 0..5 {
        super::tick(&host, &[], clock());
    }
    assert_eq!(
        typed(&host),
        Vec::new(),
        "a composer was used while the agent's own hook was the road"
    );
    assert_eq!(
        parked(),
        1,
        "the parked pointer was written down once per beat, or not at all"
    );
    assert!(
        crate::orchestration_pointer_mailbox::collect_stale(
            LEADER,
            &run_id,
            &format!("run:{run_id}"),
            &newest,
            crate::orchestration_pointer_mailbox::HOOK_COLLECTION_GRACE,
        ) == crate::orchestration_pointer_mailbox::Parked::Fresh,
        "nothing was left for the hook to collect"
    );

    /* The hook knocks: the bridge hands the pointer over as the turn's
     * continuation. That is delivery, and it needs no composer, no Enter,
     * and nobody watching. */
    use zerocode_hookd::pointer_mailbox::PointerMoment;
    let collected = crate::orchestration_pointer_mailbox::mailbox().take(
        &crate::hooks::pane_key_of(LEADER),
        // The test host records no launch, and a park with none refuses
        // nobody — the identity gate has its own cases.
        "",
        PointerMoment::TurnEnding,
    );
    assert_eq!(
        collected.as_ref().map(|notice| notice.text().to_string()),
        Some(zerocode_core::orchestration::pointer_text(1)),
        "the agent's own hook was handed something other than the fixed pointer"
    );

    /* The same `Stop` the bridge just continued must not read as the end
     * of a turn. Reported as done, it would light a finished card, ring a
     * completion, and hand this very pass a pane it thinks is idle. */
    assert!(
        !crate::orchestration_pointer_mailbox::turn_was_continued(
            WORKER,
            crate::orchestration_pointer_mailbox::CONTINUATION_RECOGNITION,
        ),
        "another pane's turn was called a continuation"
    );

    // The turn ends. The shelf is empty and the pointer has already been
    // written down as delivered, so NOTHING is typed: two roads must never
    // both carry the same sentence.
    super::pane_turn_ended(LEADER, 90, false, clock());
    for _ in 0..3 {
        super::tick(&host, &[], clock());
    }
    assert_eq!(uncollected(), 0, "a collected pointer was called abandoned");
    assert_eq!(
        typed(&host),
        Vec::new(),
        "a pointer the agent had already been handed was typed at it again"
    );

    /* And the end of the road the user asked for: the agent reads its
     * mail and acknowledges it, with nobody having pressed Enter anywhere
     * in this test. The pointer was advice; THIS is the delivery. */
    let leader_seat = zerocode_core::agent_teams::LEADER_PANE;
    let read = run(
        &host,
        Vec::new(),
        &team,
        leader_seat,
        TEST_CAPABILITY,
        &words("check"),
        clock(),
    );
    assert_eq!(read.exit_code, 0, "{}", read.stderr);
    let read: serde_json::Value = serde_json::from_str(&read.stdout).expect("a delivery");
    assert_eq!(
        read["count"], 1,
        "the mail the pointer named was not handed over"
    );
    let delivery = read["deliveryId"]
        .as_str()
        .expect("a delivery id")
        .to_string();
    let acked = run(
        &host,
        Vec::new(),
        &team,
        leader_seat,
        TEST_CAPABILITY,
        &words(&format!("check --ack {delivery}")),
        clock(),
    );
    assert_eq!(acked.exit_code, 0, "{}", acked.stderr);
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host),
        Vec::new(),
        "acknowledged mail was still being pointed at"
    );

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
    crate::orchestration_pointer_mailbox::forget_term(LEADER);
}

/// A coordinator's pane as a CLI at its wall keeps it (t-6560): every line
/// typed there opens a turn, and the CLI answers that turn itself, at once,
/// with the wall — no model is asked.
///
/// The order is the real window's, and the loop lives in it: the prompt's
/// own report (`pane_turn_began`) arrives before any beat has read the
/// pump's receipt, and it strikes the pointer's marks out; the turn ends
/// (`StopFailure`, measured as an ordinary end); and the next beat at rest
/// finds nothing written down about the line it typed a moment ago.
struct AtItsWall {
    term: u32,
    /// Every line typed, by terminal: a beat walks every run in the
    /// process's ledger, and another scenario's pane is not this one's.
    typed: Mutex<Vec<(u32, String)>>,
    /// The receipt of the line in flight, answered when the pane hears it.
    in_flight: Mutex<Option<std::sync::mpsc::SyncSender<zerocode_pty::DeliveryOutcome>>>,
    /// Whether the pane's last answer is a wall — and the next one will be.
    walled: std::sync::atomic::AtomicBool,
    cause: crate::quota_wall::StallCause,
}

impl AtItsWall {
    /// How long each look at the wall says it still stands. Short, so a
    /// hold's own deadline passes during the test and is looked at again.
    const STANDS_FOR_MS: i64 = 1_500;

    fn new(term: u32, cause: crate::quota_wall::StallCause, walled: bool) -> Self {
        Self {
            term,
            typed: Mutex::new(Vec::new()),
            in_flight: Mutex::new(None),
            walled: std::sync::atomic::AtomicBool::new(walled),
            cause,
        }
    }

    fn typed(&self) -> usize {
        self.typed
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .iter()
            .filter(|(term, _)| *term == self.term)
            .count()
    }

    fn wall_lifts(&self) {
        self.walled
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// What the pane does with the line in flight, if any: the prompt's
    /// report, the pump's receipt, and the turn's end — at the wall, when a
    /// wall answers every prompt here.
    fn answers(&self, meets_the_wall: bool) {
        let Some(settle) = self
            .in_flight
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .take()
        else {
            return;
        };
        super::pane_turn_began(self.term, clock());
        let _ = settle.send(zerocode_pty::DeliveryOutcome::Delivered);
        super::pane_turn_ended(self.term, clock(), false, clock());
        if meets_the_wall {
            self.walled.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

impl Host for AtItsWall {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        _token: &str,
    ) -> Option<u32> {
        None
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        true
    }
    fn point(
        &self,
        term: u32,
        line: &str,
        _submit: bool,
    ) -> Option<std::sync::mpsc::Receiver<zerocode_pty::DeliveryOutcome>> {
        self.typed
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push((term, line.to_string()));
        let (settle, receipt) = std::sync::mpsc::sync_channel(1);
        if term == self.term {
            *self
                .in_flight
                .lock()
                .unwrap_or_else(|held| held.into_inner()) = Some(settle);
        }
        Some(receipt)
    }
    fn capture(&self, _term: u32) -> Option<String> {
        None
    }
    fn focus(&self, _term: u32) -> bool {
        false
    }
    fn close(&self, _term: u32) {}
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
    fn agent_of(&self, _term: u32) -> Option<String> {
        Some(zerocode_core::AgentKind::Claude.slug().to_string())
    }
    fn pane_wall(&self, term: u32, agent: &str) -> Option<crate::quota_wall::PaneWall> {
        assert_eq!(agent, zerocode_core::AgentKind::Claude.slug());
        // The CLI's own sentences, as the coordinator's transcript has them.
        let said = match self.cause {
            crate::quota_wall::StallCause::LoginWall => "Login expired · Please run /login",
            _ => "You've hit your session limit · resets 6:30pm (Asia/Seoul)",
        };
        (term == self.term && self.walled.load(std::sync::atomic::Ordering::SeqCst)).then(|| {
            crate::quota_wall::PaneWall {
                cause: self.cause,
                line: zerocode_core::orchestration::Text::from(said.to_string()),
                stands_until_ms: clock() + Self::STANDS_FOR_MS,
            }
        })
    }
}

/// Twenty letters to a pane standing at its wall: the pointer is typed at
/// most once while the wall stands — the line that found it — and once when
/// it stops standing (t-6560).
///
/// Before, every beat at rest typed the same line again: the coordinator's
/// pane took it 1,391 times in four days, every two to five seconds, and
/// each one that met a quota wall was a request the provider refused. The
/// wall is read off the pane's own last answer and held for as long as the
/// wall's own window (`QUOTA_WAIT_POLICY`) says; a hold whose window runs
/// out looks again, and a wall that no longer answers is the lift.
#[test]
fn a_pane_at_its_wall_is_told_once_while_it_stands_and_once_when_it_lifts() {
    const LEADER: u32 = 11_560;
    const WORKER: u32 = 11_561;
    const LETTERS: usize = 20;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-walled-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    // The worker sending the letters is a working agent: its pane's hook says
    // a turn began, as a summoned worker's does, and the first beat retires
    // its readiness window. Unheard, that window ran on the wall clock, and a
    // body the machine's load stretched past its minute found the ledger's
    // `never_spoke` news among the letters — 21 handed over where 20 were
    // sent — in parallel runs only.
    super::pane_turn_began(WORKER, clock());
    let host = AtItsWall::new(LEADER, crate::quota_wall::StallCause::QuotaWall, false);
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let post = |n: usize| {
        let posted = run(
            &host,
            Vec::new(),
            &team,
            &pane,
            &held,
            &words(&format!(
                "send --type status --body letter-{n} --retry-request walled-{worker}-{n}"
            )),
            clock(),
        );
        assert_eq!(posted.exit_code, 0, "{}", posted.stderr);
    };
    // Three beats a letter, the pane answering at its wall between them.
    let beats = |host: &AtItsWall| {
        for _ in 0..3 {
            super::tick(host, &[], clock());
            host.answers(true);
        }
    };
    let blackbox = super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    let counted = |needle: &str| -> usize {
        std::fs::read_to_string(&blackbox)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(needle))
            .count()
    };
    let held_line =
        format!("mail waiting for run:{run_id} in {run_id} is held at terminal {LEADER}");
    let lifted_line = format!("terminal {LEADER}'s quota wall stopped standing");

    // The coordinator finished a turn in the ordinary way; nothing walls it.
    super::pane_turn_began(LEADER, clock());
    super::pane_turn_ended(LEADER, clock(), false, clock());

    // The first letter finds a pane nobody knows is walled — and the line
    // typed about it is the turn that meets the wall.
    post(1);
    beats(&host);
    assert_eq!(
        host.typed(),
        1,
        "the first letter, before any wall, was not pointed at exactly once"
    );
    for n in 2..=LETTERS {
        post(n);
        beats(&host);
    }
    let while_it_stood = host.typed();
    assert!(
        while_it_stood <= 1,
        "the pointer was typed {while_it_stood} times at a pane standing at its wall, \
         for {LETTERS} letters"
    );
    assert!(
        counted(&held_line) >= 1,
        "the black box never said why nothing was typed"
    );

    // The wall stops standing: its window ran out and the pane's last answer,
    // looked at again, is no wall. One line — and only one.
    host.wall_lifts();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while host.typed() == while_it_stood && std::time::Instant::now() < deadline {
        super::tick(&host, &[], clock());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(
        host.typed(),
        while_it_stood + 1,
        "the pane was not told about its mail once its wall had lifted"
    );
    assert_eq!(
        counted(&lifted_line),
        1,
        "the lift was not written down once"
    );

    /* The agent reads what the line pointed at: its turn is a real one now.
     * The mail is acknowledged, and nothing is typed after it. */
    host.answers(false);
    let leader_seat = zerocode_core::agent_teams::LEADER_PANE;
    let read = run(
        &host,
        Vec::new(),
        &team,
        leader_seat,
        TEST_CAPABILITY,
        &words("check"),
        clock(),
    );
    assert_eq!(read.exit_code, 0, "{}", read.stderr);
    let read: serde_json::Value = serde_json::from_str(&read.stdout).expect("a delivery");
    assert_eq!(
        read["count"], LETTERS,
        "the letters were not all handed over"
    );
    let delivery = read["deliveryId"].as_str().expect("a delivery id");
    let acked = run(
        &host,
        Vec::new(),
        &team,
        leader_seat,
        TEST_CAPABILITY,
        &words(&format!("check --ack {delivery}")),
        clock(),
    );
    assert_eq!(acked.exit_code, 0, "{}", acked.stderr);
    for _ in 0..3 {
        super::tick(&host, &[], clock());
    }
    assert_eq!(
        host.typed(),
        while_it_stood + 1,
        "read mail was pointed at again"
    );

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
    crate::orchestration_pointer_mailbox::forget_term(LEADER);
}

/// A pane whose last answer was already its login wall is not typed at
/// when mail comes — not even once — and is told the moment it answers
/// again (t-6560).
///
/// The login that expired on 2026-09-20 12:10 took nine lines in twenty
/// seconds, each answered by the CLI itself with `Login expired · Please
/// run /login`. The person signs in, their own turn goes through, and that
/// turn's end is where the pointer speaks — once.
#[test]
fn mail_for_a_pane_already_at_its_login_wall_waits_for_its_next_answer() {
    const LEADER: u32 = 11_562;
    const WORKER: u32 = 11_563;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-login-wall-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let host = AtItsWall::new(LEADER, crate::quota_wall::StallCause::LoginWall, true);
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    super::pane_turn_began(LEADER, clock());
    super::pane_turn_ended(LEADER, clock(), false, clock());

    for n in 1..=5 {
        let posted = run(
            &host,
            Vec::new(),
            &team,
            &pane,
            &held,
            &words(&format!(
                "send --type status --body login-{n} --retry-request login-wall-{worker}-{n}"
            )),
            clock(),
        );
        assert_eq!(posted.exit_code, 0, "{}", posted.stderr);
        for _ in 0..3 {
            super::tick(&host, &[], clock());
            host.answers(true);
        }
    }
    assert_eq!(
        host.typed(),
        0,
        "the pointer was typed at a pane whose own last answer was its login wall"
    );
    let blackbox = super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    let log = std::fs::read_to_string(&blackbox).unwrap_or_default();
    assert!(
        log.lines().any(|line| line.contains(&format!(
            "mail waiting for run:{run_id} in {run_id} is held at terminal {LEADER}"
        )) && line.contains("login wall")),
        "the black box never named the login wall"
    );

    // The person signs in and their own turn goes through: the turn begins,
    // ends, and its last answer is no wall. The next beat at rest speaks.
    host.wall_lifts();
    super::pane_turn_began(LEADER, clock());
    super::pane_turn_ended(LEADER, clock(), false, clock());
    for _ in 0..3 {
        super::tick(&host, &[], clock());
    }
    assert_eq!(
        host.typed(),
        1,
        "the pane was not told once its login answered again"
    );

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
    crate::orchestration_pointer_mailbox::forget_term(LEADER);
}

/// A pane a person interrupted is theirs, and stays theirs until they
/// finish a turn in it — so its door, like an unheard pane's, is one no
/// beat opens on its own. The pointer must not type there either, and the
/// window must not go quiet about mail it cannot deliver advice for.
///
/// The line between this and a running turn is the whole judgment: a turn
/// ends by itself, and a person may never come back.
#[test]
fn mail_behind_an_interrupted_turn_is_named_and_still_never_typed_at() {
    const LEADER: u32 = 11_060;
    const WORKER: u32 = 11_061;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-interrupted-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);

    struct Pointing {
        sent: Mutex<Vec<(u32, String)>>,
    }
    impl Host for Pointing {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.sent
                .lock()
                .unwrap_or_else(|held| held.into_inner())
                .push((term, text.to_string()));
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = Pointing {
        sent: Mutex::new(Vec::new()),
    };
    let typed = |host: &Pointing| -> Vec<(u32, String)> {
        host.sent
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    };
    let blackbox = super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    let needle = format!(
        "run:{run_id} in {run_id} could not be pointed at terminal {LEADER} \
             — a person interrupted"
    );
    let named = || -> usize {
        std::fs::read_to_string(&blackbox)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(&needle))
            .count()
    };

    // The person stopped the last turn in this pane.
    super::pane_turn_ended(LEADER, 40, true, clock());
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let done = run(
        &host,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request held-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);

    for _ in 0..5 {
        super::tick(&host, &[], clock());
    }
    assert_eq!(
        typed(&host),
        Vec::new(),
        "advice was typed into a pane a person had just taken back"
    );
    assert_eq!(
        named(),
        1,
        "mail waiting behind a person's pane went unsaid"
    );

    // They come back and finish a turn: the pane is the pointer's again.
    super::pane_turn_ended(LEADER, 80, false, clock());
    super::tick(&host, &[], clock());
    assert_eq!(
        typed(&host),
        vec![
            (LEADER, zerocode_core::orchestration::pointer_text(1)),
            (LEADER, "\r".to_string()),
        ],
        "a pane handed back was not pointed at"
    );
    assert_eq!(
        named(),
        1,
        "the black box was told twice about one watermark"
    );

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

#[test]
fn the_native_pointer_fast_path_carries_no_mail_or_provider_policy() {
    let source = include_str!("../orchestration.rs");
    let pointer = source
        .split_once("fn point_at_waiting_mail(host: &dyn Host, now_ms: i64) {")
        .expect("the pointer pass")
        .1
        .split_once("\n}\n\n/// What the beat calls its own request.")
        .expect("the end of the pointer pass")
        .0;

    /* Twice, and the two are the pass's two mutually exclusive branches:
     * the pane is mid-turn and the pointer is parked for its own hook, or
     * the turn is over and the pointer is composed for a road that types.
     * One beat takes one of them, so the ledger is still asked once per
     * address — and both ask with the SEAT, which is what keeps the
     * pointer counting exactly the mail that seat's own `check` would be
     * handed. */
    assert_eq!(
        pointer
            .matches("run.pointer_wanted(&address, Some(&seat))")
            .count(),
        2
    );
    assert_eq!(pointer.matches("PointerNotice::new(").count(), 2);
    assert_eq!(
        pointer
            .matches("crate::orchestration_notify::offer(")
            .count(),
        1
    );
    /* One guarded delivery through the window's typed-prompt door, and
     * never a raw write: `Host::send` consults nobody about the composer,
     * and the review of 2026-09-05 found the draft it appended to. */
    assert!(pointer.contains("host.point(one.term, &one.text, !submits_itself)"));
    assert!(!pointer.contains("host.send("));
    for forbidden in ["MessageRow", ".body", "worker_done", "claude", "codex"] {
        assert!(
            !pointer.contains(forbidden),
            "provider policy or mail content entered the common pointer pass: {forbidden}"
        );
    }
}

#[test]
fn confirmed_native_pointer_suppresses_pty_and_unknown_falls_back_without_retry() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use zerocode_hookd::session_notify::{NotificationOutcome, PointerNotice, SessionNotifier};

    struct Native {
        outcome: NotificationOutcome,
        calls: AtomicUsize,
    }
    impl SessionNotifier for Native {
        fn notify(&self, _notice: &PointerNotice) -> NotificationOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.outcome
        }
    }

    const LEADER: u32 = 11_020;
    const WORKER: u32 = 11_021;
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    let team = format!("team-native-pointer-{LEADER}");
    let (_run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let held =
        crate::agent_teams::current_pane_capability(&team, &pane).expect("the worker capability");
    let done = run(
        &Nowhere,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request native-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);
    pane_turn_ended(LEADER, 70, false, clock());

    let confirmed = Arc::new(Native {
        outcome: NotificationOutcome::Confirmed,
        calls: AtomicUsize::new(0),
    });
    let confirmed_lease = crate::orchestration_notify::register_route(LEADER, confirmed.clone());
    let host = Counting::default();
    for _ in 0..20 {
        tick(&host, &[], clock());
        if confirmed.calls.load(Ordering::SeqCst) == 1 {
            break;
        }
        std::thread::yield_now();
    }
    tick(&host, &[], clock());
    assert_eq!(confirmed.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        host.calls(),
        0,
        "confirmed native advice also touched the PTY"
    );
    drop(confirmed_lease);

    let unknown = Arc::new(Native {
        outcome: NotificationOutcome::Unknown,
        calls: AtomicUsize::new(0),
    });
    let unknown_lease = crate::orchestration_notify::register_route(LEADER, unknown.clone());
    for _ in 0..50 {
        tick(&host, &[], clock());
        if host.calls() > 0 {
            break;
        }
        std::thread::yield_now();
    }
    assert_eq!(unknown.calls.load(Ordering::SeqCst), 1);
    assert!(
        host.calls() > 0,
        "an unknown native outcome suppressed PTY forever"
    );

    drop(unknown_lease);
    pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// How a pointer reaches a pane in the scenarios below: the pane's native
/// queue route, or its composer when it holds none.
#[derive(Clone, Copy, Debug)]
enum Road {
    Native,
    Composer,
}

/// A native route that keeps what it was handed and says each time it was.
struct Knocked {
    carried: Mutex<Vec<String>>,
    said: Mutex<std::sync::mpsc::Sender<()>>,
}

impl Knocked {
    fn new() -> (Arc<Self>, std::sync::mpsc::Receiver<()>) {
        let (said, heard) = std::sync::mpsc::channel();
        let knocked = Arc::new(Self {
            carried: Mutex::new(Vec::new()),
            said: Mutex::new(said),
        });
        (knocked, heard)
    }

    fn carried(&self) -> Vec<String> {
        self.carried
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    }
}

impl zerocode_hookd::session_notify::SessionNotifier for Knocked {
    fn notify(
        &self,
        notice: &zerocode_hookd::session_notify::PointerNotice,
    ) -> zerocode_hookd::session_notify::NotificationOutcome {
        self.carried
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push(notice.text().to_string());
        let _ = self
            .said
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .send(());
        zerocode_hookd::session_notify::NotificationOutcome::Confirmed
    }
}

/// Panes holding the agent a scenario names, and every keystroke typed at
/// them — the composer road lands here, one line and its Enter.
struct Holding {
    agent: zerocode_core::AgentKind,
    typed: Mutex<Vec<(u32, String)>>,
}

impl Holding {
    fn new(agent: zerocode_core::AgentKind) -> Self {
        Self {
            agent,
            typed: Mutex::new(Vec::new()),
        }
    }

    fn typed_at(&self, term: u32) -> Vec<String> {
        self.typed
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .iter()
            .filter(|(at, _)| *at == term)
            .map(|(_, text)| text.clone())
            .collect()
    }
}

impl Host for Holding {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        _token: &str,
    ) -> Option<u32> {
        None
    }
    fn send(&self, term: u32, text: &str) -> bool {
        self.typed
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push((term, text.to_string()));
        true
    }
    fn capture(&self, _term: u32) -> Option<String> {
        None
    }
    fn focus(&self, _term: u32) -> bool {
        false
    }
    fn close(&self, _term: u32) {}
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
    fn agent_of(&self, _term: u32) -> Option<String> {
        Some(self.agent.slug().to_string())
    }
}

/// How long a native route's one knock may take to be heard. A hang
/// guard, not a bound any passing run comes near: the knock is an in-process
/// call on the hub's own worker thread.
const A_KNOCK_IS_HEARD: std::time::Duration = std::time::Duration::from_secs(10);

/// One verb from a pane, JSON back — panicking with the refusal.
fn said_at(host: &dyn Host, team: &str, pane: &str, held: &str, line: &str) -> serde_json::Value {
    let answer = run(host, Vec::new(), team, pane, held, &words(line), clock());
    assert_eq!(answer.exit_code, 0, "`{line}`: {}", answer.stderr);
    serde_json::from_str(&answer.stdout)
        .unwrap_or_else(|_| panic!("`{line}` did not answer JSON: {}", answer.stdout))
}

/// A pane reads everything waiting for it and acknowledges it, as `check`
/// then `check --ack` — the delivery the pointer only ever advises.
fn reads_its_mail(host: &dyn Host, team: &str, pane: &str, held: &str) -> usize {
    let batch = said_at(host, team, pane, held, "check");
    let count = batch["count"].as_u64().expect("a count") as usize;
    if let Some(delivery) = batch["deliveryId"].as_str() {
        said_at(host, team, pane, held, &format!("check --ack {delivery}"));
    }
    count
}

/// The beats that follow a pane coming to rest with `pending` messages it
/// has not read, and what reached it: exactly one pointer by `road`, none
/// through the other, and none again after it reads (t-8938).
fn one_pointer_reaches_the_resting_pane(
    road: Road,
    host: &Holding,
    term: u32,
    pending: usize,
    reads: impl FnOnce() -> usize,
) {
    let (knocked, heard) = Knocked::new();
    let lease = matches!(road, Road::Native)
        .then(|| crate::orchestration_notify::register_route(term, knocked.clone()));
    super::tick(host, &[], clock());
    if matches!(road, Road::Native) {
        heard.recv_timeout(A_KNOCK_IS_HEARD).unwrap_or_else(|_| {
            panic!(
                "no road carried a pointer to the resting pane (typed: {:?})",
                host.typed_at(term)
            )
        });
    }
    for _ in 0..3 {
        super::tick(host, &[], clock());
    }
    let advice = zerocode_core::orchestration::pointer_text(pending);
    let (native, composer) = match road {
        Road::Native => (vec![advice], Vec::new()),
        Road::Composer => (Vec::new(), vec![advice, "\r".to_string()]),
    };
    assert_eq!(
        (knocked.carried(), host.typed_at(term)),
        (native.clone(), composer.clone()),
        "the resting pane was not pointed at its mail exactly once by the {road:?} road"
    );
    assert_eq!(
        reads(),
        pending,
        "the pointer named mail the pane was not handed"
    );
    for _ in 0..3 {
        super::tick(host, &[], clock());
    }
    assert_eq!(
        (knocked.carried(), host.typed_at(term)),
        (native, composer),
        "read mail was pointed at again"
    );
    drop(lease);
}

/// How many messages wait unhanded at an address — what its `check` hands
/// over next, and so what a pointer names.
fn waiting_for(run_id: &str, address: &str) -> usize {
    the_rows()
        .inboxes
        .iter()
        .find(|row| row.run == run_id && row.address == address)
        .map_or(0, |row| row.pending.len())
}

/// The questions a holder asked that open a thread of their own.
fn questions_from(run_id: &str, address: &str) -> usize {
    the_rows()
        .messages
        .iter()
        .filter(|row| {
            row.run == run_id
                && row.from == address
                && row.kind == zerocode_core::orchestration::MessageKind::Question
                && row.thread.is_none()
        })
        .count()
}

/// How long the reviewer's two asks below may wait. Not a bound the test
/// asserts — the answer ends both waits on the new road — it is how long the
/// old road's second copy of the question waited for an answer nobody gave,
/// and so how long a red run of this test takes.
const ASKED_TWICE_PATIENCE_MS: u32 = 5_000;

/// B's order on 2026-09-25, through the window (t-8938). A Codex reviewer
/// asked the coordinator a question, and asked again under the same retry
/// name while the first wait was still out; the coordinator answered the
/// first copy; the reviewer read the answer and ended its turn, and three
/// messages came for it. The second copy was never answered, the ledger
/// read "asked and unanswered" as "waiting on purpose", and no road carried
/// a pointer to the resting pane for two hours. Now the retry joins the
/// question it repeats and a question is no door: the three messages earn
/// one pointer, and none once they are read.
fn a_resting_pane_that_asked_twice_is_pointed_at_its_mail_once(road: Road, leader_term: u32) {
    let worker_term = leader_term + 1;
    let _window = the_window();
    let _beat = one_beat_at_a_time();
    let _waits = one_wait_at_a_time();
    let team = format!("team-asked-twice-{leader_term}");
    let (run_id, worker, pane) = an_agent_carrying_work("codex", &team, leader_term, worker_term);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let host = Holding::new(zerocode_core::AgentKind::Codex);

    super::pane_turn_began(worker_term, clock());
    let asked = format!(
        "ask --body which-queue-next? --timeout-ms {ASKED_TWICE_PATIENCE_MS} \
         --retry-request asked-twice-{worker_term}"
    );
    let asking = || {
        let (team, pane, held, line) = (team.clone(), pane.clone(), held.clone(), asked.clone());
        std::thread::spawn(move || {
            run(
                &Nowhere,
                Vec::new(),
                &team,
                &pane,
                &held,
                &words(&line),
                clock(),
            )
        })
    };
    let before = WAITS_BEGUN.load(std::sync::atomic::Ordering::SeqCst);
    let first = asking();
    until_a_wait_has_begun(before);
    let before = WAITS_BEGUN.load(std::sync::atomic::Ordering::SeqCst);
    let again = asking();
    until_a_wait_has_begun(before);

    let questions = said_at(
        &host,
        &team,
        leader,
        TEST_CAPABILITY,
        "check --types question",
    );
    let asked_first = questions["messages"][0]["messageId"]
        .as_str()
        .unwrap_or_else(|| panic!("no question reached the coordinator: {questions}"))
        .to_string();
    if let Some(delivery) = questions["deliveryId"].as_str() {
        said_at(
            &host,
            &team,
            leader,
            TEST_CAPABILITY,
            &format!("check --ack {delivery}"),
        );
    }
    said_at(
        &host,
        &team,
        leader,
        TEST_CAPABILITY,
        &format!("reply --to-message {asked_first} --body hold-the-seat"),
    );
    let first = first.join().expect("the first ask");
    let again = again.join().expect("the retry");
    assert_eq!(first.exit_code, 0, "{}", first.stderr);
    assert_eq!(again.exit_code, 0, "{}", again.stderr);

    reads_its_mail(&host, &team, &pane, &held);
    for body in ["t-7936-first", "t-6877-next", "main-landed"] {
        said_at(
            &host,
            &team,
            leader,
            TEST_CAPABILITY,
            &format!("send --to worker:{worker} --type status --body {body}"),
        );
    }
    super::pane_turn_ended(worker_term, clock(), false, clock());
    one_pointer_reaches_the_resting_pane(road, &host, worker_term, 3, || {
        reads_its_mail(&host, &team, &pane, &held)
    });

    let (first, again): (serde_json::Value, serde_json::Value) = (
        serde_json::from_str(&first.stdout).expect("the first ask answers JSON"),
        serde_json::from_str(&again.stdout).expect("the retry answers JSON"),
    );
    assert_eq!(
        again["questionId"], first["questionId"],
        "the retry posted a second question instead of joining the first"
    );
    assert_eq!(
        questions_from(&run_id, &format!("worker:{worker}")),
        1,
        "one named ask left two questions in the ledger"
    );

    super::pane_turn_began(worker_term, clock());
    crate::agent_teams::forget_term(leader_term);
    crate::agent_teams::forget_term(worker_term);
}

#[test]
fn a_resting_codex_pane_that_asked_twice_is_pointed_at_its_mail_once_by_its_queue() {
    a_resting_pane_that_asked_twice_is_pointed_at_its_mail_once(Road::Native, 12_390);
}

#[test]
fn a_resting_codex_pane_that_asked_twice_is_pointed_at_its_mail_once_in_its_composer() {
    a_resting_pane_that_asked_twice_is_pointed_at_its_mail_once(Road::Composer, 12_392);
}

/// A question nobody answered is no door (t-8938). The ledger cannot see a
/// wait: an `ask` goes home at its deadline and leaves its question
/// standing, and the rule that read that row as "waiting on purpose" kept
/// every later message from the pointer. Here the ask times out, the turn
/// ends, and the mail after it is pointed at once — by either road.
fn a_question_left_standing_silences_no_mail(road: Road, leader_term: u32) {
    let worker_term = leader_term + 1;
    let _window = the_window();
    let _beat = one_beat_at_a_time();
    let _waits = one_wait_at_a_time();
    let team = format!("team-left-standing-{leader_term}");
    let (run_id, worker, pane) = an_agent_carrying_work("codex", &team, leader_term, worker_term);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let host = Holding::new(zerocode_core::AgentKind::Codex);

    super::pane_turn_began(worker_term, clock());
    let gave_up = said_at(
        &host,
        &team,
        &pane,
        &held,
        &format!(
            "ask --body which-way? --timeout-ms {}",
            zerocode_core::orchestration::WAIT_BUDGET_MIN_MS
        ),
    );
    assert_eq!(gave_up["answered"], false, "{gave_up}");
    assert_eq!(questions_from(&run_id, &format!("worker:{worker}")), 1);
    super::pane_turn_ended(worker_term, clock(), false, clock());
    said_at(
        &host,
        &team,
        leader,
        TEST_CAPABILITY,
        &format!("send --to worker:{worker} --type status --body next-in-the-queue"),
    );
    one_pointer_reaches_the_resting_pane(road, &host, worker_term, 1, || {
        reads_its_mail(&host, &team, &pane, &held)
    });

    super::pane_turn_began(worker_term, clock());
    crate::agent_teams::forget_term(leader_term);
    crate::agent_teams::forget_term(worker_term);
}

#[test]
fn a_question_left_standing_silences_no_mail_to_the_queue() {
    a_question_left_standing_silences_no_mail(Road::Native, 12_394);
}

#[test]
fn a_question_left_standing_silences_no_mail_to_the_composer() {
    a_question_left_standing_silences_no_mail(Road::Composer, 12_396);
}

/// The coordinator's own seat (t-8938): a question it put to another pane
/// that was never answered — the shape of a coordinator asking the next
/// run's pane at 01:36 and hearing nothing — silenced its own address for
/// the rest of the day. Its mail is pointed at like anybody's: the report,
/// and the ledger's word beside it about the receiver of its question.
#[test]
fn a_coordinators_unanswered_question_leaves_its_own_mail_pointed_at() {
    const LEADER: u32 = 12_398;
    const WORKER: u32 = 12_399;
    let _window = the_window();
    let _beat = one_beat_at_a_time();
    let _waits = one_wait_at_a_time();
    let team = format!("team-seat-asked-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let host = Holding::new(zerocode_core::AgentKind::Claude);

    let gave_up = said_at(
        &host,
        &team,
        leader,
        TEST_CAPABILITY,
        &format!(
            "ask --to worker:{worker} --body is-that-yours? --timeout-ms {}",
            zerocode_core::orchestration::WAIT_BUDGET_MIN_MS
        ),
    );
    assert_eq!(gave_up["answered"], false, "{gave_up}");
    assert_eq!(questions_from(&run_id, &format!("run:{run_id}")), 1);
    super::pane_turn_ended(LEADER, clock(), false, clock());
    said_at(
        &host,
        &team,
        &pane,
        &held,
        "send --type worker_done --body {\"ok\":true}",
    );
    let waiting = waiting_for(&run_id, &format!("run:{run_id}"));
    assert!(
        waiting >= 1,
        "the worker's report never reached its coordinator"
    );
    one_pointer_reaches_the_resting_pane(Road::Composer, &host, LEADER, waiting, || {
        reads_its_mail(&host, &team, leader, TEST_CAPABILITY)
    });

    super::pane_turn_began(LEADER, clock());
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// A pane held mid-turn inside its own `ask` is not parked at (t-8938): the
/// window runs that wait, and advice about other mail handed over at the
/// ask's end would ride in on the answer. Held and said once while the wait
/// is out — judged on the pane, not the question row — and parked for the
/// pane's own hook the beat the wait is over, with the hold's end said once.
#[test]
fn a_pane_waiting_in_its_own_question_is_held_and_then_parked() {
    const LEADER: u32 = 12_380;
    const WORKER: u32 = 12_381;
    let _window = the_window();
    let _beat = one_beat_at_a_time();
    let _waits = one_wait_at_a_time();
    let _stood = crate::standing_clock::stand_still();
    let team = format!("team-held-asking-{LEADER}");
    let (run_id, worker, pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let held = crate::agent_teams::current_pane_capability(&team, &pane)
        .expect("the split minted the worker a capability");
    let host = Holding::new(zerocode_core::AgentKind::Claude);
    let address = format!("worker:{worker}");
    let blackbox = super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    let said = |needle: String| {
        std::fs::read_to_string(&blackbox)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(&needle))
            .count()
    };
    let holding = || {
        said(format!(
            "{address} in {run_id} is held at terminal {WORKER}"
        ))
    };
    let let_go = || {
        said(format!(
            "terminal {WORKER} is no longer held in its own question"
        ))
    };
    let parked = || {
        said(format!(
            "{address} in {run_id} is parked for terminal {WORKER}"
        ))
    };
    let shelved = || {
        crate::orchestration_pointer_mailbox::collect_stale(
            WORKER,
            &run_id,
            &address,
            &the_rows()
                .messages
                .iter()
                .rev()
                .find(|row| row.run == run_id && row.to == address)
                .map(|row| row.id.clone())
                .unwrap_or_default(),
            crate::orchestration_pointer_mailbox::HOOK_COLLECTION_GRACE,
        )
    };

    super::pane_turn_began(WORKER, clock());
    let before = WAITS_BEGUN.load(std::sync::atomic::Ordering::SeqCst);
    let asking = {
        let (team, pane, held) = (team.clone(), pane.clone(), held.clone());
        std::thread::spawn(move || {
            run(
                &Nowhere,
                Vec::new(),
                &team,
                &pane,
                &held,
                &words(&format!(
                    "ask --body which-way? --timeout-ms {ASKED_TWICE_PATIENCE_MS}"
                )),
                clock(),
            )
        })
    };
    until_a_wait_has_begun(before);
    said_at(
        &host,
        &team,
        leader,
        TEST_CAPABILITY,
        &format!("send --to {address} --type status --body not-the-answer"),
    );
    for _ in 0..3 {
        super::tick(&host, &[], clock());
    }
    assert_eq!(
        shelved(),
        crate::orchestration_pointer_mailbox::Parked::Empty,
        "a pane waiting in its own question was parked at"
    );
    assert_eq!(
        holding(),
        1,
        "the hold was said once per beat, or not at all"
    );
    assert!(
        host.typed_at(WORKER).is_empty(),
        "a turn in progress was typed at"
    );

    let questions = said_at(
        &host,
        &team,
        leader,
        TEST_CAPABILITY,
        "check --types question",
    );
    let asked = questions["messages"][0]["messageId"]
        .as_str()
        .unwrap_or_else(|| panic!("no question reached the coordinator: {questions}"))
        .to_string();
    if let Some(delivery) = questions["deliveryId"].as_str() {
        said_at(
            &host,
            &team,
            leader,
            TEST_CAPABILITY,
            &format!("check --ack {delivery}"),
        );
    }
    said_at(
        &host,
        &team,
        leader,
        TEST_CAPABILITY,
        &format!("reply --to-message {asked} --body left"),
    );
    let answered = asking.join().expect("the asker");
    assert_eq!(answered.exit_code, 0, "{}", answered.stderr);

    for _ in 0..3 {
        super::tick(&host, &[], clock());
    }
    assert_eq!(
        shelved(),
        crate::orchestration_pointer_mailbox::Parked::Fresh,
        "the pane's own hook was left nothing once its wait was over"
    );
    assert_eq!((holding(), let_go(), parked()), (1, 1, 1));
    assert!(
        host.typed_at(WORKER).is_empty(),
        "a turn in progress was typed at"
    );

    super::pane_turn_began(WORKER, clock());
    crate::orchestration_pointer_mailbox::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// How many rounds the measurement below takes.
const MEASURED_ROUNDS: u32 = 20;

/// The window's side of B's road, measured (t-8938) — run by hand:
/// `cargo test -p zerocode-shell --bins -- --ignored --exact
/// orchestration::tests::the_pointer_to_a_resting_codex_pane_is_measured
/// --nocapture`. Each round is B's shape on the new road: a Codex worker
/// with a question nobody answered, its turn over, one message landing, one
/// beat. Timed end to end: the message landed → the pane's queue route was
/// handed the pointer → the pane's `check` came back with the message.
/// What lies outside it is said beside it: the beat's own phase
/// (`AUTO_BEAT_EVERY`, up to a second in the window) and the agent's turn,
/// which only the field can time.
#[test]
#[ignore = "measurement, run by hand"]
fn the_pointer_to_a_resting_codex_pane_is_measured() {
    let _window = the_window();
    let _beat = one_beat_at_a_time();
    let _waits = one_wait_at_a_time();
    let host = Holding::new(zerocode_core::AgentKind::Codex);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let mut handed = Vec::new();
    let mut read = Vec::new();
    for round in 0..MEASURED_ROUNDS {
        let leader_term = 12_400 + 2 * round;
        let worker_term = leader_term + 1;
        let team = format!("team-measured-{leader_term}");
        let (_run_id, worker, pane) =
            an_agent_carrying_work("codex", &team, leader_term, worker_term);
        let held = crate::agent_teams::current_pane_capability(&team, &pane)
            .expect("the split minted the worker a capability");
        said_at(
            &host,
            &team,
            &pane,
            &held,
            "send --type question --body which-way?",
        );
        super::pane_turn_ended(worker_term, clock(), false, clock());
        let (knocked, heard) = Knocked::new();
        let lease = crate::orchestration_notify::register_route(worker_term, knocked);
        said_at(
            &host,
            &team,
            leader,
            TEST_CAPABILITY,
            &format!("send --to worker:{worker} --type status --body next-in-the-queue"),
        );
        let landed = std::time::Instant::now();
        super::tick(&host, &[], clock());
        heard
            .recv_timeout(A_KNOCK_IS_HEARD)
            .expect("the pointer reached the pane's route");
        let knocked_at = std::time::Instant::now();
        assert_eq!(reads_its_mail(&host, &team, &pane, &held), 1);
        let read_at = std::time::Instant::now();
        handed.push(knocked_at - landed);
        read.push(read_at - knocked_at);
        drop(lease);
        super::pane_turn_began(worker_term, clock());
        crate::agent_teams::forget_term(leader_term);
        crate::agent_teams::forget_term(worker_term);
    }
    let spread = |mut held: Vec<std::time::Duration>| {
        held.sort();
        let at = |share: usize| held[(held.len() - 1) * share / 100];
        format!("p50 {:?} p95 {:?} max {:?}", at(50), at(95), at(100))
    };
    eprintln!(
        "t-8938 over {MEASURED_ROUNDS} rounds — message landed → pointer on the queue route: {}; \
         pointer → the pane's check read the message: {}",
        spread(handed),
        spread(read)
    );
}

/// A host that, like the window's own, answers where the pane it
/// spawned actually sits — the seat answer on the split's capability.
struct Seating {
    onto: u32,
    checkout: &'static str,
}

impl Host for Seating {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        token: &str,
    ) -> Option<u32> {
        crate::agent_teams::place_seat_checkout(token, self.checkout.to_string());
        Some(self.onto)
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        true
    }
    fn capture(&self, _term: u32) -> Option<String> {
        Some(String::new())
    }
    fn focus(&self, _term: u32) -> bool {
        true
    }
    fn close(&self, _term: u32) {}
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
}

#[test]
fn a_sleeping_worker_waits_for_its_bound_coordinator_without_spending_an_attempt() {
    const OLD_LEADER: u32 = 91_320;
    const WORKER: u32 = 91_321;
    const NEW_LEADER: u32 = 91_322;
    let (_window, _store) = PrivateWindow::boot();
    let old_team = format!("team-wait-old-{OLD_LEADER}");
    seat_a_team(&old_team, OLD_LEADER);
    let host = Seating {
        onto: WORKER,
        checkout: "/tmp",
    };
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &host,
        Vec::new(),
        &old_team,
        leader,
        TEST_CAPABILITY,
        &words("run-create --name waits-for-coordinator"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let task = run(
        &host,
        Vec::new(),
        &old_team,
        leader,
        TEST_CAPABILITY,
        &words("task-create --spec continue"),
        clock(),
    );
    let task: serde_json::Value = serde_json::from_str(&task.stdout).expect("a task");
    let task = task["taskId"].as_str().expect("a task id");
    let started = run(
        &host,
        Vec::new(),
        &old_team,
        leader,
        TEST_CAPABILITY,
        &words(&format!("worker-start --agent codex --task {task}")),
        clock(),
    );
    assert_eq!(started.exit_code, 0, "{}", started.stderr);
    let worker: serde_json::Value = serde_json::from_str(&started.stdout).expect("a worker");
    let worker = worker["workerId"].as_str().expect("a worker id");
    let held = super::runtime().expect("the private runtime");
    assert_eq!(
        held.actor
            .window_restarted(clock())
            .expect("sleep the worker")
            .0
            .sleeping,
        1
    );
    crate::agent_teams::forget_term(OLD_LEADER);
    let new_team = format!("team-wait-new-{NEW_LEADER}");
    seat_a_team(&new_team, NEW_LEADER);
    let before = the_rows();

    assert_eq!(
        super::reseat_sleeping(&host, Vec::new(), NEW_LEADER, Some(&test_actor(NEW_LEADER)),),
        0,
        "an unbound coordinator restored somebody else's worker"
    );
    let after = the_rows();
    let before_worker = before
        .workers
        .iter()
        .find(|held| held.id == worker)
        .expect("the sleeping worker");
    let after_worker = after
        .workers
        .iter()
        .find(|held| held.id == worker)
        .expect("the waiting worker");
    assert_eq!(after_worker.state, WorkerState::Sleeping);
    assert_eq!(after_worker.dispatch, before_worker.dispatch);
    let after_task = after
        .tasks
        .iter()
        .find(|held| held.id == task)
        .expect("the waiting task");
    assert_eq!(
        after_task.status,
        zerocode_core::orchestration::TaskStatus::Dispatched
    );
    assert_eq!(after_task.failures, 0);
    assert_eq!(after, before, "waiting spent or rewrote the attempt");
    crate::agent_teams::forget_term(NEW_LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// One private window, one run, one worker in a checkout with a
/// reported session — the shape every t-3058 scenario below starts from.
/// Answers the run's team, the task id and the worker id.
fn a_seated_worker_with_a_session(
    host: &dyn Host,
    leader_term: u32,
    worker_term: u32,
    session_id: &str,
) -> (String, String, String) {
    let team = format!("team-t3058-{leader_term}");
    seat_a_team(&team, leader_term);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let say = |argv: &str| {
        let answered = run(
            host,
            Vec::new(),
            &team,
            leader,
            TEST_CAPABILITY,
            &words(argv),
            clock(),
        );
        assert_eq!(answered.exit_code, 0, "{argv}: {}", answered.stderr);
        answered.stdout
    };
    say(&format!("run-create --name t3058-{leader_term}"));
    let task: serde_json::Value =
        serde_json::from_str(&say("task-create --spec keep-going")).expect("a task");
    let task = task["taskId"].as_str().expect("a task id").to_string();
    let started: serde_json::Value =
        serde_json::from_str(&say(&format!("worker-start --agent codex --task {task}")))
            .expect("a worker");
    let worker = started["workerId"]
        .as_str()
        .expect("a worker id")
        .to_string();
    let held = super::runtime().expect("the private runtime");
    let session = zerocode_core::ProviderSession {
        key: zerocode_core::SessionKey::SessionId,
        id: session_id.to_string(),
        transcript_path: None,
    };
    assert!(
        held.actor
            .worker_session_reported(worker_term, session, clock())
            .expect("the session lands")
            .0,
        "the worker's session was not written"
    );
    (team, task, worker)
}

fn worker_died_bodies(
    rows: &zerocode_core::orchestration::LedgerProjectionV1,
) -> Vec<serde_json::Value> {
    rows.messages
        .iter()
        .filter(|message| message.kind == zerocode_core::orchestration::MessageKind::WorkerDied)
        .map(|message| serde_json::from_str(message.body.as_str()).expect("a JSON death notice"))
        .collect()
}

/// t-3058 (1): the window's goodbye comes first. A seated worker sleeps
/// with its dispatch open, and the pane's own exit on the way out —
/// the settlement that used to win the race — settles nothing: no
/// `worker_died`, no gate, the task still dispatched.
#[test]
fn a_window_exiting_puts_the_seat_to_sleep_before_its_pane_exits() {
    const LEADER: u32 = 93_050;
    const WORKER: u32 = 93_051;
    let (_window, _store) = PrivateWindow::boot();
    let host = Seating {
        onto: WORKER,
        checkout: "/tmp",
    };
    let (_team, task, worker) =
        a_seated_worker_with_a_session(&host, LEADER, WORKER, "session-exiting");
    let before = the_rows();
    let dispatch = before
        .workers
        .iter()
        .find(|held| held.id == worker)
        .and_then(|held| held.dispatch.clone())
        .expect("the dispatch");

    super::window_exiting(
        clock(),
        crate::exit_runtime::ExitRoad::App,
        &super::restart_census::RestartCensus::default,
    );
    // The pane reaper reaches the seat afterwards, as it always could.
    super::terminal_gone(WORKER, clock());

    let after = the_rows();
    let slept = after
        .workers
        .iter()
        .find(|held| held.id == worker)
        .expect("the worker stands");
    assert_eq!(slept.state, WorkerState::Sleeping);
    assert_eq!(slept.dispatch.as_deref(), Some(dispatch.as_str()));
    let held = after
        .tasks
        .iter()
        .find(|held| held.id == task)
        .expect("the task stands");
    assert_eq!(
        held.status,
        zerocode_core::orchestration::TaskStatus::Dispatched
    );
    assert_eq!(held.failures, 0, "the exit spent the attempt");
    assert!(
        after.gates.is_empty(),
        "the exit put the task behind a gate"
    );
    assert!(
        worker_died_bodies(&after).is_empty(),
        "the exit was announced as a death"
    );
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// t-6428 ①: the goodbye names what it cuts. Before any seat sleeps the
/// census reads the worker's turn and the commands under its pane — here a
/// codex worker mid-turn, with a gate its native binary started under its
/// Node launcher — and the window's log says the road once and the worker
/// once. The census only reads: the seat still sleeps.
#[test]
fn the_goodbye_names_its_road_and_what_it_cuts_under_each_worker() {
    const LEADER: u32 = 93_070;
    const WORKER: u32 = 93_071;
    const ROOT: u32 = 64_280;
    let (_window, _store) = PrivateWindow::boot();
    let host = Seating {
        onto: WORKER,
        checkout: "/tmp",
    };
    let (_team, _task, worker) =
        a_seated_worker_with_a_session(&host, LEADER, WORKER, "session-goodbye");
    super::pane_turn_began(WORKER, clock());
    let listing = format!(
        "501 {ROOT} 1 {ROOT} 0 1 Thu Sep 24 01:00:00 2026 node /opt/homebrew/bin/codex resume s\n\
         501 64281 {ROOT} {ROOT} 0 1 Thu Sep 24 01:00:00 2026 /opt/codex/vendor/bin/codex resume s\n\
         501 64282 64281 64282 0 1 Thu Sep 24 01:00:00 2026 /bin/zsh -lc just shell-test\n"
    );
    let census = || {
        super::restart_census::take(&|term| (term == WORKER).then_some(ROOT), &|| {
            Ok(crate::resource_usage::ProcessSample::from_ps_listing(
                &listing,
            ))
        })
    };

    super::window_exiting(clock(), crate::exit_runtime::ExitRoad::Close, &census);

    let blackbox = super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    let log = std::fs::read_to_string(&blackbox).unwrap_or_default();
    let named = format!(
        "exit: worker {worker} on terminal {WORKER} (codex) · turn running · 1 command(s): \
         just shell-test"
    );
    assert_eq!(
        log.lines().filter(|line| line.ends_with(&named)).count(),
        1,
        "the goodbye did not name the worker once:\n{log}"
    );
    assert!(
        log.lines()
            .any(|line| line.contains("exit by close · workers 1 · mid-turn 1 · background 0")),
        "the goodbye did not say its road and numbers:\n{log}"
    );
    let slept = the_rows()
        .workers
        .iter()
        .find(|held| held.id == worker)
        .map(|held| held.state);
    assert_eq!(slept, Some(WorkerState::Sleeping));
    // A second goodbye on the same way out finds nobody live to name.
    super::window_exiting(clock(), crate::exit_runtime::ExitRoad::App, &census);
    let again = std::fs::read_to_string(&blackbox).unwrap_or_default();
    assert_eq!(
        again.lines().filter(|line| line.ends_with(&named)).count(),
        1,
        "the worker was named twice"
    );
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
}

/// t-3058 (1), the witness road through the production doors. After the
/// exit the next window resumes the same conversation into the same
/// checkout — the person's own restored tab, a leader of its own team —
/// and `pane_resumed` seats the sleeper there as the same worker; its
/// `worker_done` then lands through the live verb from that pane. Not
/// before the seat exists, and only once.
#[test]
fn a_resumed_pane_is_seated_as_the_sleeper_it_is_and_reports_done_from_there() {
    const LEADER: u32 = 93_060;
    const WORKER: u32 = 93_061;
    const RESUMED: u32 = 93_062;
    let (_window, _store) = PrivateWindow::boot();
    let host = Seating {
        onto: WORKER,
        checkout: "/tmp",
    };
    let (_team, task, worker) =
        a_seated_worker_with_a_session(&host, LEADER, WORKER, "session-witness");
    super::window_exiting(
        clock(),
        crate::exit_runtime::ExitRoad::App,
        &super::restart_census::RestartCensus::default,
    );
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);

    // The read the nudge is worded from: this IS a sleeper's conversation.
    assert_eq!(
        super::sleeper_awaiting("/tmp", "codex", "session-witness").as_deref(),
        Some(worker.as_str())
    );
    assert_eq!(
        super::sleeper_awaiting("/tmp", "codex", "session-other"),
        None
    );
    // No seat yet: nothing is written.
    assert_eq!(
        super::pane_resumed(RESUMED, "/tmp", "codex", "session-witness", clock()),
        Ok(None)
    );
    let resumed_team = format!("team-t3058-resumed-{RESUMED}");
    seat_a_team(&resumed_team, RESUMED);
    assert_eq!(
        super::pane_resumed(RESUMED, "/tmp/", "codex", "session-witness", clock()),
        Ok(Some(worker.clone()))
    );
    assert_eq!(
        super::pane_resumed(RESUMED, "/tmp", "codex", "session-witness", clock()),
        Ok(None),
        "the same witness seated the worker twice"
    );
    let rows = the_rows();
    let back = rows
        .workers
        .iter()
        .find(|held| held.id == worker)
        .expect("the worker");
    assert_eq!(back.state, WorkerState::Active);
    assert_eq!(
        (back.team.as_str(), back.pane.as_str()),
        (
            resumed_team.as_str(),
            zerocode_core::agent_teams::LEADER_PANE
        )
    );
    assert!(back.dispatch.is_some());

    let done = run(
        &host,
        Vec::new(),
        &resumed_team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words("send --type worker_done --body {\"ok\":true,\"summary\":\"landed\"}"),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);
    let rows = the_rows();
    assert_eq!(
        rows.tasks
            .iter()
            .find(|held| held.id == task)
            .expect("the task")
            .status,
        zerocode_core::orchestration::TaskStatus::Completed
    );
    crate::agent_teams::forget_term(RESUMED);
}

/// t-3058 (1), the grace. A sleeper nothing resumed is left alone by
/// every beat under a window younger than `RESEAT_GRACE_MS`, and ended
/// by the first beat past it — announced with the dispatch id a
/// `--retry-of` needs and the checkout it left, the way a pane's death
/// is announced.
#[test]
fn a_sleeper_nobody_resumed_dies_on_the_beat_after_the_grace() {
    const LEADER: u32 = 93_070;
    const WORKER: u32 = 93_071;
    // The window first, then the beat — the order every beating test
    // keeps. Taken the other way round this deadlocked against a test
    // holding the shared window while waiting for its beat turn.
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    let host = Seating {
        onto: WORKER,
        checkout: "/tmp",
    };
    let (_team, task, worker) =
        a_seated_worker_with_a_session(&host, LEADER, WORKER, "session-overdue");
    let dispatch = the_rows()
        .workers
        .iter()
        .find(|held| held.id == worker)
        .and_then(|held| held.dispatch.clone())
        .expect("the dispatch");
    super::window_exiting(
        clock(),
        crate::exit_runtime::ExitRoad::App,
        &super::restart_census::RestartCensus::default,
    );
    crate::agent_teams::forget_term(LEADER);
    crate::agent_teams::forget_term(WORKER);
    // The grace is the next window's (t-9091): its boot, then its beats.
    super::runtime()
        .expect("the private runtime")
        .actor
        .window_restarted(clock())
        .expect("the next boot's sweep");

    {
        let _young = BootedHere::at(clock());
        super::tick(&host, &[], clock());
    }
    assert_eq!(
        the_rows()
            .workers
            .iter()
            .find(|held| held.id == worker)
            .expect("the worker")
            .state,
        WorkerState::Sleeping,
        "a beat under a young window ended a sleeper"
    );

    {
        let _old = BootedHere::at(clock() - RESEAT_GRACE_MS - 1);
        super::tick(&host, &[], clock());
    }
    let rows = the_rows();
    let ended = rows
        .workers
        .iter()
        .find(|held| held.id == worker)
        .expect("the worker");
    assert_eq!(ended.state, WorkerState::Released);
    let held = rows
        .tasks
        .iter()
        .find(|held| held.id == task)
        .expect("the task");
    assert_eq!(held.failures, 1);
    let told = worker_died_bodies(&rows);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["workerId"], worker, "{}", told[0]);
    assert_eq!(told[0]["dispatchId"], dispatch, "{}", told[0]);
    assert_eq!(told[0]["checkout"], "/tmp", "{}", told[0]);
    assert_eq!(
        told[0]["reason"],
        zerocode_core::orchestration::NOT_RESUMED,
        "{}",
        told[0]
    );
}

/// Records only fake host effects; every authority write still uses the
/// private window's real actor and temporary SQLite store.
struct RestorationHost {
    checkout: String,
    returned_actor: Option<(u32, String)>,
    next: std::sync::atomic::AtomicU32,
    commands: Mutex<Vec<String>>,
}

impl Host for RestorationHost {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        command: &str,
        token: &str,
    ) -> Option<u32> {
        let _ = crate::agent_teams::take_worker_host_ask(token);
        crate::agent_teams::place_seat_checkout(token, self.checkout.clone());
        self.commands.lock().unwrap().push(command.to_string());
        Some(self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        true
    }
    fn capture(&self, _term: u32) -> Option<String> {
        Some(String::new())
    }
    fn focus(&self, _term: u32) -> bool {
        true
    }
    fn close(&self, _term: u32) {}
    fn actor_for(&self, term: u32) -> Option<String> {
        self.returned_actor
            .as_ref()
            .filter(|(returned, _)| *returned == term)
            .map(|(_, actor)| actor.clone())
            .or_else(|| Some(test_actor(term)))
    }
}

#[test]
fn the_grace_reseats_a_worker_under_its_returned_coordinator_without_a_seat_binding() {
    const OLD_LEADER: u32 = 193_500;
    const OLD_WORKER: u32 = 193_501;
    const NEW_WORKER: u32 = 193_502;
    const NEW_LEADER: u32 = 193_510;
    for through_beat in [false, true] {
        let (_window, _store) = PrivateWindow::boot();
        let _beat = one_beat_at_a_time();
        let checkout = tempfile::tempdir().expect("the worker's checkout");
        let host = RestorationHost {
            checkout: checkout.path().to_string_lossy().into_owned(),
            returned_actor: Some((NEW_LEADER, test_actor(OLD_LEADER))),
            next: std::sync::atomic::AtomicU32::new(OLD_WORKER),
            commands: Mutex::new(Vec::new()),
        };
        let (old_team, task, worker) =
            a_seated_worker_with_a_session(&host, OLD_LEADER, OLD_WORKER, "session-grace-reseat");
        let run_id = super::bound_run(&old_team, "%1", Some(&test_actor(OLD_LEADER)))
            .expect("the actor's run");
        assert_eq!(
            super::runtime()
                .unwrap()
                .actor
                .window_restarted(clock())
                .expect("restart the private window")
                .0
                .sleeping,
            1
        );
        crate::agent_teams::forget_term(OLD_WORKER);
        crate::agent_teams::forget_term(OLD_LEADER);
        let new_team = format!("team-grace-returned-{NEW_LEADER}");
        seat_a_team(&new_team, NEW_LEADER);
        let held = super::runtime().expect("the private runtime");
        assert!(
            held.actor
                .coordinator_returned(
                    &run_id,
                    &new_team,
                    "%1",
                    Some(test_actor(OLD_LEADER)),
                    clock(),
                )
                .expect("native coordinator return")
                .0
        );
        let before = the_rows();
        let sleeping = before.workers.iter().find(|row| row.id == worker).unwrap();
        assert_eq!(sleeping.state, WorkerState::Sleeping);
        assert!(
            before
                .bound
                .iter()
                .any(|row| row.caller.as_str() == test_actor(OLD_LEADER) && row.run == run_id)
        );
        assert!(
            !before
                .bound
                .iter()
                .any(|row| row.caller.as_str() == format!("{new_team}/%1"))
        );
        host.commands.lock().unwrap().clear();

        if through_beat {
            let _old = BootedHere::at(clock() - RESEAT_GRACE_MS - 1);
            super::tick(&host, &[], clock());
            super::tick(&host, &[], clock());
        } else {
            assert_eq!(
                super::reseat_sleeping(
                    &host,
                    Vec::new(),
                    NEW_LEADER,
                    Some(&test_actor(OLD_LEADER))
                ),
                1
            );
        }
        let after = the_rows();
        let restored = after.workers.iter().find(|row| row.id == worker).unwrap();
        assert_eq!(
            restored.state,
            WorkerState::Active,
            "through beat: {through_beat}"
        );
        assert_eq!(restored.dispatch, sleeping.dispatch);
        assert_eq!(restored.session, sleeping.session);
        assert_eq!(restored.team, new_team);
        assert_eq!(
            restored.adopted_by,
            before.runs[0]
                .coordinator
                .as_ref()
                .map(|seat| seat.generation)
        );
        assert_eq!(after.dispatches, before.dispatches);
        assert_eq!(after.tasks, before.tasks);
        assert_eq!(after.runs[0].coordinator, before.runs[0].coordinator);
        assert_eq!(
            after
                .tasks
                .iter()
                .find(|row| row.id == task)
                .unwrap()
                .failures,
            0
        );
        assert!(worker_died_bodies(&after).is_empty());
        let commands = host.commands.lock().unwrap();
        assert_eq!(commands.len(), 1, "duplicate or missing restoration");
        assert!(
            commands[0].contains("session-grace-reseat"),
            "{}",
            commands[0]
        );
        drop(commands);
        assert_eq!(
            super::reseat_sleeping(&host, Vec::new(), NEW_LEADER, Some(&test_actor(OLD_LEADER))),
            0
        );
        assert_eq!(host.commands.lock().unwrap().len(), 1);
        crate::agent_teams::forget_term(NEW_WORKER);
        crate::agent_teams::forget_term(NEW_LEADER);
    }
}

fn assert_other_coordinators_sleeper_is_untouched(checkout_exists: bool) {
    const OLD_LEADER: u32 = 193_520;
    const OLD_WORKER: u32 = 193_521;
    const NEW_LEADER: u32 = 193_530;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = RestorationHost {
        checkout: checkout.path().to_string_lossy().into_owned(),
        returned_actor: None,
        next: std::sync::atomic::AtomicU32::new(OLD_WORKER),
        commands: Mutex::new(Vec::new()),
    };
    let (old_team, _task, _worker) =
        a_seated_worker_with_a_session(&host, OLD_LEADER, OLD_WORKER, "session-stranger-reseat");
    let run_id = super::bound_run(&old_team, "%1", Some(&test_actor(OLD_LEADER))).unwrap();
    assert_eq!(
        super::runtime()
            .unwrap()
            .actor
            .window_restarted(clock())
            .expect("restart the private window")
            .0
            .sleeping,
        1
    );
    crate::agent_teams::forget_term(OLD_WORKER);
    let held = super::runtime().unwrap();
    assert!(
        held.actor
            .coordinator_returned(
                &run_id,
                &old_team,
                "%1",
                Some(test_actor(OLD_LEADER)),
                clock(),
            )
            .unwrap()
            .0
    );
    let new_team = format!("team-reseat-stranger-{NEW_LEADER}");
    seat_a_team(&new_team, NEW_LEADER);
    let bound = run(
        &host,
        Vec::new(),
        &new_team,
        "%1",
        TEST_CAPABILITY,
        &words(&format!("run-use {run_id}")),
        clock(),
    );
    assert_eq!(bound.exit_code, 0, "{}", bound.stderr);
    let bound: serde_json::Value = serde_json::from_str(&bound.stdout).unwrap();
    assert_eq!(bound["seated"], false);
    let before = the_rows();
    host.commands.lock().unwrap().clear();
    if !checkout_exists {
        checkout.close().expect("remove only the private checkout");
    }

    assert_eq!(
        super::reseat_sleeping(&host, Vec::new(), NEW_LEADER, Some(&test_actor(NEW_LEADER))),
        0
    );
    assert!(
        host.commands.lock().unwrap().is_empty(),
        "a stranger cut a pane"
    );
    assert_eq!(
        the_rows(),
        before,
        "a refused coordinator changed the attempt or seat"
    );
    crate::agent_teams::forget_term(NEW_LEADER);
    crate::agent_teams::forget_term(OLD_LEADER);
}

#[test]
fn a_bound_native_leader_cannot_restore_another_coordinators_sleepers() {
    assert_other_coordinators_sleeper_is_untouched(true);
}

#[test]
fn a_bound_native_leader_cannot_end_another_coordinators_sleeper() {
    assert_other_coordinators_sleeper_is_untouched(false);
}

/// The core bench proves that a report works after somebody has already
/// moved the row. The production contract is stricter: no task word may
/// reach the replacement process before that durable move, because an
/// agent can finish a small task in the host/actor gap and run the exact
/// `worker_done` its briefing names. Drive that report from the host's
/// delivery callback so this crosses the real verb, actor, capability and
/// SQLite roads instead of arranging the winning order by hand.
#[test]
fn a_restored_worker_reports_done_through_the_live_verb_after_durable_reseat() {
    const OLD_LEADER: u32 = 91_330;
    const OLD_WORKER: u32 = 91_331;
    const NEW_LEADER: u32 = 91_332;
    const NEW_WORKER: u32 = 91_333;

    struct ReportingRestore {
        checkout: String,
        next: std::sync::atomic::AtomicU32,
        seat: Mutex<Option<(String, String, String)>>,
        report: Mutex<Option<zerocode_hookd::TeamAnswer>>,
    }

    impl Host for ReportingRestore {
        fn split(
            &self,
            team: &str,
            _leader_term: u32,
            _from_term: u32,
            pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            token: &str,
        ) -> Option<u32> {
            // Consume the typed host ask exactly as TeamWindow does. The
            // live road must later deliver its prose through `paste`,
            // after the authority row has moved.
            let _ = crate::agent_teams::take_worker_host_ask(token);
            crate::agent_teams::place_seat_checkout(token, self.checkout.clone());
            *self.seat.lock().expect("the restored seat") =
                Some((team.to_string(), pane.to_string(), token.to_string()));
            Some(self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
        }

        fn send(&self, _term: u32, _text: &str) -> bool {
            true
        }

        fn paste(&self, _term: u32, text: &str) -> bool {
            let said = text.to_ascii_lowercase();
            assert!(
                said.contains("continue") || said.contains("interrupted"),
                "the restored worker received no continuation: {text}"
            );
            let (team, pane, token) = self
                .seat
                .lock()
                .expect("the restored seat")
                .clone()
                .expect("the replacement pane was seated");
            let answer = run(
                self,
                Vec::new(),
                &team,
                &pane,
                &token,
                &words(
                    "send --type worker_done --body {\"ok\":true} \
                         --retry-request done-restored-live",
                ),
                clock(),
            );
            let accepted = answer.exit_code == 0;
            *self.report.lock().expect("the live report") = Some(answer);
            accepted
        }

        fn capture(&self, _term: u32) -> Option<String> {
            Some(String::new())
        }

        fn focus(&self, _term: u32) -> bool {
            true
        }

        fn close(&self, _term: u32) {}

        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }

    let (private, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the durable checkout");
    let host = ReportingRestore {
        checkout: checkout.path().to_string_lossy().into_owned(),
        next: std::sync::atomic::AtomicU32::new(OLD_WORKER),
        seat: Mutex::new(None),
        report: Mutex::new(None),
    };
    let old_team = format!("team-live-report-old-{OLD_LEADER}");
    seat_a_team(&old_team, OLD_LEADER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &host,
        Vec::new(),
        &old_team,
        leader,
        TEST_CAPABILITY,
        &words("run-create --name restored-live-report"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let task = run(
        &host,
        Vec::new(),
        &old_team,
        leader,
        TEST_CAPABILITY,
        &words("task-create --spec continue"),
        clock(),
    );
    let task: serde_json::Value = serde_json::from_str(&task.stdout).expect("a task");
    let task = task["taskId"].as_str().expect("a task id").to_string();
    let started = run(
        &host,
        Vec::new(),
        &old_team,
        leader,
        TEST_CAPABILITY,
        &words(&format!("worker-start --agent codex --task {task}")),
        clock(),
    );
    assert_eq!(started.exit_code, 0, "{}", started.stderr);
    let started: serde_json::Value = serde_json::from_str(&started.stdout).expect("a worker");
    let worker = started["workerId"]
        .as_str()
        .expect("a worker id")
        .to_string();
    let held = super::runtime().expect("the private runtime");
    held.actor
        .worker_session_reported(
            OLD_WORKER,
            zerocode_core::ProviderSession {
                key: zerocode_core::provider_session::SessionKey::SessionId,
                id: "0199-live-reseat-report".to_string(),
                transcript_path: None,
            },
            clock(),
        )
        .expect("store the resumable session");
    // The window goes with this worker's turn under way, so its reseat
    // carries a continuation (t-7812 E) — the words this worker answers with
    // its report. A worker the goodbye found at rest is typed nothing.
    super::pane_turn_began(OLD_WORKER, clock());
    restore::the_window_goes(&restore::census_without_commands, &[OLD_WORKER]);
    assert_eq!(restore::row(&worker).state, WorkerState::Sleeping);
    held.actor
        .window_restarted(clock())
        .expect("the next boot's sweep");
    crate::agent_teams::forget_term(OLD_LEADER);

    let new_team = format!("team-live-report-new-{NEW_LEADER}");
    seat_a_team(&new_team, NEW_LEADER);
    assert_eq!(
        super::reseat_sleeping(&host, Vec::new(), NEW_LEADER, Some(&test_actor(OLD_LEADER)),),
        1,
        "the sleeping worker was not restored"
    );
    let report = host
        .report
        .lock()
        .expect("the live report")
        .take()
        .expect("the host never delivered the restored task");
    assert_eq!(report.exit_code, 0, "{}", report.stderr);
    let rows = the_rows();
    assert_eq!(
        rows.tasks
            .iter()
            .find(|held| held.id == task)
            .expect("the carried task")
            .status,
        zerocode_core::orchestration::TaskStatus::Completed
    );
    assert_eq!(
        rows.workers
            .iter()
            .find(|held| held.id == worker)
            .expect("the restored worker")
            .state,
        WorkerState::Reclaimable
    );
    crate::agent_teams::forget_term(NEW_WORKER);
    crate::agent_teams::forget_term(NEW_LEADER);
    drop(private);
}

/// A checkout is reserved when ANY interrupted worker in it is Sleeping,
/// even if a newer historical row in that checkout was already released.
/// Picking only the newest row lets the renderer launch that released
/// agent generically; the older worker is then reseated into the same tree
/// and two processes own one task directory.
#[test]
fn a_newer_released_row_cannot_hide_a_sleeping_checkout_reservation() {
    const LEADER: u32 = 91_340;
    const FIRST_WORKER: u32 = 91_341;
    const SECOND_WORKER: u32 = 91_342;

    struct SharedCheckout {
        checkout: String,
        next: std::sync::atomic::AtomicU32,
    }

    impl Host for SharedCheckout {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            token: &str,
        ) -> Option<u32> {
            let _ = crate::agent_teams::take_worker_host_ask(token);
            crate::agent_teams::place_seat_checkout(token, self.checkout.clone());
            Some(self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
        }

        fn send(&self, _term: u32, _text: &str) -> bool {
            true
        }

        fn capture(&self, _term: u32) -> Option<String> {
            Some(String::new())
        }

        fn focus(&self, _term: u32) -> bool {
            true
        }

        fn close(&self, _term: u32) {}

        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }

    let (private, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the shared checkout");
    let checkout = checkout.path().to_string_lossy().into_owned();
    let host = SharedCheckout {
        checkout: checkout.clone(),
        next: std::sync::atomic::AtomicU32::new(FIRST_WORKER),
    };
    let team = format!("team-hidden-sleeper-{LEADER}");
    seat_a_team(&team, LEADER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("run-create --name hidden-sleeper"),
        10,
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let task = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("task-create --spec interrupted"),
        20,
    );
    let task: serde_json::Value = serde_json::from_str(&task.stdout).expect("a task");
    let task = task["taskId"].as_str().expect("a task id");
    let older = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&format!("worker-start --agent codex --task {task}")),
        30,
    );
    assert_eq!(older.exit_code, 0, "{}", older.stderr);
    let newer = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("worker-start --agent claude"),
        40,
    );
    assert_eq!(newer.exit_code, 0, "{}", newer.stderr);
    let newer: serde_json::Value = serde_json::from_str(&newer.stdout).expect("a worker");
    let newer = newer["workerId"].as_str().expect("a worker id");
    let released = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&format!("worker-release --worker {newer}")),
        50,
    );
    assert_eq!(released.exit_code, 0, "{}", released.stderr);
    let held = super::runtime().expect("the private runtime");
    assert_eq!(
        held.actor
            .window_restarted(60)
            .expect("sleep the interrupted worker")
            .0
            .sleeping,
        1
    );

    let reserved =
        super::last_agent_in_checkout(&checkout).expect("the ledger remembers the checkout");
    assert!(
        reserved.sleeping,
        "the newer released row hid the interrupted worker"
    );
    assert_eq!(reserved.agent, "codex");
    crate::agent_teams::forget_term(FIRST_WORKER);
    crate::agent_teams::forget_term(SECOND_WORKER);
    crate::agent_teams::forget_term(LEADER);
    drop(private);
}

/// The two adoption roads, side by side (t-2512 §1.2). An orphan whose
/// pane is still standing is adopted WHERE IT IS the moment the next
/// coordinator sits — no second pane, which is the twin-session accident.
/// An orphan whose pane the window has confirmed gone (the reconciler's
/// streak, written on the row) is seated again in a fresh pane on the
/// attempt it never lost.
#[test]
fn an_orphan_is_adopted_in_place_or_reseated_once_its_pane_is_proven_gone() {
    const OLD_LEADER: u32 = 91_360;
    const STANDING: u32 = 91_361;
    const GONE: u32 = 91_363;
    const NEW_LEADER: u32 = 91_362;
    const FRESH: u32 = 91_364;
    let (private, _store) = PrivateWindow::boot();
    let old_team = format!("team-orphan-adopt-old-{OLD_LEADER}");
    seat_a_team(&old_team, OLD_LEADER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let say = |host: &dyn Host, line: &str| {
        let said = run(
            host,
            Vec::new(),
            &old_team,
            leader,
            TEST_CAPABILITY,
            &words(line),
            clock(),
        );
        assert_eq!(said.exit_code, 0, "{line}: {}", said.stderr);
        serde_json::from_str::<serde_json::Value>(&said.stdout).expect("JSON")
    };
    let standing_host = Seating {
        onto: STANDING,
        checkout: "/tmp",
    };
    let gone_host = Seating {
        onto: GONE,
        checkout: "/tmp",
    };
    say(&standing_host, "run-create --name orphan-adopt");
    let task = say(&standing_host, "task-create --spec continue")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let standing = say(
        &standing_host,
        &format!("worker-start --agent codex --task {task}"),
    )["workerId"]
        .as_str()
        .expect("a worker id")
        .to_string();
    let other = say(&gone_host, "task-create --spec also-continue")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let gone = say(
        &gone_host,
        &format!("worker-start --agent codex --task {other}"),
    )["workerId"]
        .as_str()
        .expect("a worker id")
        .to_string();

    let held = super::runtime().expect("the private runtime");
    assert!(
        held.actor
            .team_dissolved(&old_team, clock())
            .expect("orphan the workers")
            .0
    );
    crate::agent_teams::forget_term(OLD_LEADER);
    let row = |id: &str| {
        the_rows()
            .workers
            .iter()
            .find(|held| held.id == id)
            .cloned()
            .expect("the row")
    };
    assert_eq!(row(&standing).state, WorkerState::Orphaned);
    assert_eq!(row(&gone).state, WorkerState::Orphaned);
    let standing_before = row(&standing);
    let gone_before = row(&gone);

    // The window confirms ONE pane gone — the reconciler's streak,
    // written on the row — before the next coordinator sits.
    crate::agent_teams::forget_term(GONE);
    assert!(
        held.actor
            .panes_missing(vec![(gone.clone(), clock())], clock())
            .expect("the stamp")
            .0
    );

    // The next coordinator's restore pass: it sits, adopts the standing
    // orphan where it is, and cuts ONE fresh pane — for the proven-gone
    // one.
    let new_team = format!("team-orphan-adopt-new-{NEW_LEADER}");
    seat_a_team(&new_team, NEW_LEADER);
    let fresh_host = Seating {
        onto: FRESH,
        checkout: "/tmp",
    };
    assert_eq!(
        super::reseat_sleeping(
            &fresh_host,
            Vec::new(),
            NEW_LEADER,
            Some(&test_actor(OLD_LEADER))
        ),
        1,
        "the pass cut a pane for a worker that did not need one, or none for one that did"
    );
    let standing_after = row(&standing);
    assert_eq!(
        standing_after.state,
        WorkerState::Active,
        "the standing orphan was not adopted"
    );
    assert_eq!(standing_after.adopted_by, Some(2));
    assert_eq!(
        (standing_after.team.as_str(), standing_after.pane.as_str()),
        (standing_before.team.as_str(), standing_before.pane.as_str()),
        "an orphan whose pane still stands was moved to a second pane"
    );
    assert_eq!(standing_after.dispatch, standing_before.dispatch);
    let gone_after = row(&gone);
    assert_eq!(
        gone_after.state,
        WorkerState::Active,
        "the proven-gone orphan was not reseated"
    );
    assert_eq!(gone_after.adopted_by, Some(2));
    assert_eq!(
        gone_after.team, new_team,
        "the reseat did not land in the new leader's team"
    );
    assert_eq!(gone_after.dispatch, gone_before.dispatch);
    assert!(gone_after.pane_missing_since_ms.is_none());
    crate::agent_teams::forget_term(STANDING);
    crate::agent_teams::forget_term(FRESH);
    crate::agent_teams::forget_term(NEW_LEADER);
    drop(private);
}

/* ---- the coordinator seat through the window's doors (t-2512 §1.1) --- */

/// A restored coordinator sits again through the restore pass itself:
/// the window says which leader pane came back for which run, and the
/// vacated chair takes it at the next generation. A second copy of the
/// same conversation, restored beside it, does not.
#[test]
fn a_restored_coordinator_sits_again_through_the_restore_pass() {
    const OLD_LEADER: u32 = 91_380;
    const WORKER: u32 = 91_381;
    const NEW_LEADER: u32 = 91_382;
    const TWIN_LEADER: u32 = 91_383;
    let (private, _store) = PrivateWindow::boot();
    let old_team = format!("team-seat-restore-old-{OLD_LEADER}");
    seat_a_team(&old_team, OLD_LEADER);
    let host = Seating {
        onto: WORKER,
        checkout: "/tmp",
    };
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &host,
        Vec::new(),
        &old_team,
        leader,
        TEST_CAPABILITY,
        &words("run-create --name seat-restore"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let opened: serde_json::Value = serde_json::from_str(&opened.stdout).expect("a run");
    let run_id = opened["runId"].as_str().expect("a run id").to_string();
    assert_eq!(
        opened["generation"], 1,
        "the author was not seated: {opened}"
    );

    // The window restarts: every pane is gone, so every seat is empty.
    let held = super::runtime().expect("the private runtime");
    held.actor.window_restarted(clock()).expect("the sweep");
    let seat = the_rows()
        .runs
        .iter()
        .find(|run| run.id == run_id)
        .and_then(|run| run.coordinator.clone())
        .expect("the seat record");
    assert!(seat.vacated_ms.is_some(), "a restart left the seat held");

    // The same conversation comes back in a new leader pane and mounts.
    let new_team = format!("team-seat-restore-new-{NEW_LEADER}");
    seat_a_team(&new_team, NEW_LEADER);
    super::reseat_sleeping(&host, Vec::new(), NEW_LEADER, Some(&test_actor(OLD_LEADER)));
    let seat = the_rows()
        .runs
        .iter()
        .find(|run| run.id == run_id)
        .and_then(|run| run.coordinator.clone())
        .expect("the seat record");
    assert_eq!(
        (seat.seat.as_str(), seat.generation, seat.vacated_ms),
        (format!("{new_team}/{leader}").as_str(), 2, None),
        "the restored coordinator did not sit again"
    );

    // A twin — the same session restored a second time — finds the
    // chair held and leaves it.
    let twin_team = format!("team-seat-restore-twin-{TWIN_LEADER}");
    seat_a_team(&twin_team, TWIN_LEADER);
    super::reseat_sleeping(
        &host,
        Vec::new(),
        TWIN_LEADER,
        Some(&test_actor(OLD_LEADER)),
    );
    let seat = the_rows()
        .runs
        .iter()
        .find(|run| run.id == run_id)
        .and_then(|run| run.coordinator.clone())
        .expect("the seat record");
    assert_eq!(
        (seat.seat.as_str(), seat.generation),
        (format!("{new_team}/{leader}").as_str(), 2),
        "a twin took a held seat"
    );
    crate::agent_teams::forget_term(NEW_LEADER);
    crate::agent_teams::forget_term(TWIN_LEADER);
    drop(private);
}

/// The whole day, hermetically: a leader exits, its child keeps working
/// in a pane the window still holds, the next coordinator sits and adopts
/// it, the child's own `worker_done` lands with that seat — and a later
/// coordinator takes the seat over by name while the first is still
/// standing (t-2512 §2, the e2e bullet).
#[test]
fn a_leaders_exit_a_takeover_and_the_orphans_report_land_with_the_seat() {
    const OLD_LEADER: u32 = 91_390;
    const WORKER: u32 = 91_391;
    const NEW_LEADER: u32 = 91_392;
    const THIRD_LEADER: u32 = 91_393;
    let (private, _store) = PrivateWindow::boot();
    let old_team = format!("team-e2e-old-{OLD_LEADER}");
    seat_a_team(&old_team, OLD_LEADER);
    let host = Seating {
        onto: WORKER,
        checkout: "/tmp",
    };
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let say = |team: &str, pane: &str, token: &str, line: &str| {
        run(&host, Vec::new(), team, pane, token, &words(line), clock())
    };
    let opened = say(&old_team, leader, TEST_CAPABILITY, "run-create --name e2e");
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let opened: serde_json::Value = serde_json::from_str(&opened.stdout).expect("a run");
    let run_id = opened["runId"].as_str().expect("a run id").to_string();
    let task = say(
        &old_team,
        leader,
        TEST_CAPABILITY,
        "task-create --spec finish",
    );
    let task: serde_json::Value = serde_json::from_str(&task.stdout).expect("a task");
    let task = task["taskId"].as_str().expect("a task id").to_string();
    let started = say(
        &old_team,
        leader,
        TEST_CAPABILITY,
        &format!("worker-start --agent codex --task {task}"),
    );
    assert_eq!(started.exit_code, 0, "{}", started.stderr);
    let started: serde_json::Value = serde_json::from_str(&started.stdout).expect("a worker");
    let worker = started["workerId"]
        .as_str()
        .expect("a worker id")
        .to_string();
    let pane = started["pane"].as_str().expect("a pane").to_string();
    let worker_token = crate::agent_teams::current_pane_capability(&old_team, &pane)
        .expect("the worker's capability");

    // The leader's terminal exits: the production road settles its seat,
    // dissolves its team in the ledger, and forgets the term.
    let held = super::runtime().expect("the private runtime");
    assert!(
        held.actor
            .team_dissolved(&old_team, clock())
            .expect("dissolve")
            .0
    );
    crate::agent_teams::forget_term(OLD_LEADER);

    // Row one: the child is kept, and its pane is still REACHABLE — its
    // team table stands leaderless, its token still answers.
    let rows = the_rows();
    let row = rows
        .workers
        .iter()
        .find(|held| held.id == worker)
        .expect("the row");
    assert_eq!(row.state, WorkerState::Orphaned);
    assert!(
        crate::agent_teams::current_pane_capability(&old_team, &pane).is_some(),
        "the leader's exit took the child's capability with it"
    );
    let alive = say(
        &old_team,
        &pane,
        &worker_token,
        "send --type status --body still-here",
    );
    assert_eq!(
        alive.exit_code, 0,
        "an orphan could not reach the ledger from its own pane: {}",
        alive.stderr
    );

    // The next coordinator sits in the vacated chair and adopts the child
    // where it is.
    let new_team = format!("team-e2e-new-{NEW_LEADER}");
    seat_a_team(&new_team, NEW_LEADER);
    let bound = say(
        &new_team,
        leader,
        TEST_CAPABILITY,
        &format!("run-use {run_id}"),
    );
    assert_eq!(bound.exit_code, 0, "{}", bound.stderr);
    let bound: serde_json::Value = serde_json::from_str(&bound.stdout).expect("JSON");
    assert_eq!(bound["seated"], serde_json::Value::Bool(true), "{bound}");
    // Row three: the roster the new seat reads keeps the worker active,
    // with its task, in another team, and says its seat is live.
    let listed = say(&new_team, leader, TEST_CAPABILITY, "worker-list");
    let listed: serde_json::Value = serde_json::from_str(&listed.stdout).expect("JSON");
    let listed_row = listed["workers"]
        .as_array()
        .expect("rows")
        .iter()
        .find(|held| held["workerId"] == worker.as_str())
        .cloned()
        .unwrap_or_else(|| panic!("the adopted worker fell off the roster: {listed}"));
    assert_eq!(listed_row["state"], "active", "{listed_row}");
    assert_eq!(listed_row["adoptedBy"], 2, "{listed_row}");
    assert_eq!(listed_row["taskId"], task.as_str(), "{listed_row}");
    assert_eq!(listed_row["team"], old_team.as_str(), "{listed_row}");
    assert_eq!(listed_row["seat"], "live", "{listed_row}");
    assert_eq!(listed_row["term"], WORKER, "{listed_row}");
    // Row four: the new seat reads the other leader's pane.
    let read = say(
        &new_team,
        leader,
        TEST_CAPABILITY,
        &format!("worker-read --worker {worker}"),
    );
    assert_eq!(
        read.exit_code, 0,
        "the other leader's pane could not be read: {}",
        read.stderr
    );

    // A third coordinator takes the seat over by name while the second
    // still stands; the second is told in its own pane inbox.
    let third_team = format!("team-e2e-third-{THIRD_LEADER}");
    seat_a_team(&third_team, THIRD_LEADER);
    let refused = say(
        &third_team,
        leader,
        TEST_CAPABILITY,
        &format!("run-use {run_id}"),
    );
    let refused: serde_json::Value = serde_json::from_str(&refused.stdout).expect("JSON");
    assert_eq!(
        refused["seated"],
        serde_json::Value::Bool(false),
        "{refused}"
    );
    let taken = say(
        &third_team,
        leader,
        TEST_CAPABILITY,
        &format!("run-takeover --run {run_id} --from {new_team}/{leader} --reason quota"),
    );
    assert_eq!(taken.exit_code, 0, "{}", taken.stderr);

    // The orphan reports from the pane it always had. The report lands,
    // the task completes, and the CURRENT seat reads it — not the seat
    // that summoned it, not the seat that adopted it.
    let done = say(
        &old_team,
        &pane,
        &worker_token,
        "send --type worker_done --body {\"ok\":true,\"summary\":\"done\"}",
    );
    assert_eq!(
        done.exit_code, 0,
        "the orphan's report was refused: {}",
        done.stderr
    );
    let rows = the_rows();
    assert_eq!(
        rows.tasks
            .iter()
            .find(|held| held.id == task)
            .expect("the task")
            .status,
        zerocode_core::orchestration::TaskStatus::Completed
    );
    let third_mail = say(
        &third_team,
        leader,
        TEST_CAPABILITY,
        "check --types worker_done",
    );
    let third_mail: serde_json::Value = serde_json::from_str(&third_mail.stdout).expect("JSON");
    assert_eq!(
        third_mail["count"], 1,
        "the seat did not read the report: {third_mail}"
    );
    let second_mail = say(&new_team, leader, TEST_CAPABILITY, "check");
    let second_mail: serde_json::Value = serde_json::from_str(&second_mail.stdout).expect("JSON");
    let kinds: Vec<&str> = second_mail["messages"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(|one| one["type"].as_str())
        .collect();
    assert_eq!(
        kinds,
        vec!["handoff"],
        "the unseated coordinator read: {second_mail}"
    );

    // The child's own exit settles it through the ordinary road, and the
    // leaderless table goes with its last pane.
    crate::agent_teams::forget_term(WORKER);
    assert!(
        crate::agent_teams::teams().get(&old_team).is_none(),
        "a leaderless team outlived its last child"
    );
    crate::agent_teams::forget_term(NEW_LEADER);
    crate::agent_teams::forget_term(THIRD_LEADER);
    drop(private);
}

/// `check --wait` is per seat: the unseated coordinator's card on the run
/// address does not refuse the new seat's wait — the first thing a new
/// coordinator does after a takeover is wait for its workers, and the
/// old sleeper's card is still standing until the bell reaches it.
#[test]
fn a_new_seat_may_wait_on_the_run_address_while_the_old_seats_card_stands() {
    const OLD_LEADER: u32 = 11_070;
    const WORKER: u32 = 11_071;
    const NEW_LEADER: u32 = 11_072;
    let _window = the_window();
    let old_team = format!("team-seat-wait-old-{OLD_LEADER}");
    let (run_id, _worker, _pane) = a_worker_carrying_work(&old_team, OLD_LEADER, WORKER);
    let host = Splitting::onto(WORKER + 1);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let address = format!("run:{run_id}");
    // The old seat is asleep on the run's address.
    let _old_sleeper = WaiterCard::hold(&run_id, &address, &format!("{old_team}/{leader}"));

    let new_team = format!("team-seat-wait-new-{NEW_LEADER}");
    seat_a_team(&new_team, NEW_LEADER);
    let taken = run(
        &host,
        Vec::new(),
        &new_team,
        leader,
        TEST_CAPABILITY,
        &words(&format!(
            "run-takeover --run {run_id} --from {old_team}/{leader} --reason handover"
        )),
        clock(),
    );
    assert_eq!(taken.exit_code, 0, "{}", taken.stderr);
    let waited = run(
        &host,
        Vec::new(),
        &new_team,
        leader,
        TEST_CAPABILITY,
        &words("check --wait --timeout-ms 1000"),
        clock(),
    );
    assert_eq!(
        waited.exit_code, 0,
        "the new seat's wait was refused: {}",
        waited.stderr
    );
    let waited: serde_json::Value = serde_json::from_str(&waited.stdout).expect("JSON");
    assert_eq!(waited["count"], 0, "{waited}");
    crate::agent_teams::forget_term(NEW_LEADER);
}

#[test]
fn an_orphaned_checkout_is_reserved_without_spawning_a_replacement() {
    const LEADER: u32 = 91_350;
    const WORKER: u32 = 91_351;
    const CHECKOUT: &str = "/tmp/orphaned-checkout-reservation";
    let (private, _store) = PrivateWindow::boot();
    let team = format!("team-orphan-reservation-{LEADER}");
    seat_a_team(&team, LEADER);
    let host = Seating {
        onto: WORKER,
        checkout: CHECKOUT,
    };
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("run-create --name orphan-reservation"),
        10,
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let task = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("task-create --spec continue"),
        20,
    );
    let task: serde_json::Value = serde_json::from_str(&task.stdout).expect("a task");
    let task = task["taskId"].as_str().expect("a task id");
    let started = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&format!("worker-start --agent codex --task {task}")),
        30,
    );
    assert_eq!(started.exit_code, 0, "{}", started.stderr);
    let worker: serde_json::Value = serde_json::from_str(&started.stdout).expect("a worker");
    let worker = worker["workerId"].as_str().expect("a worker id");

    let held = super::runtime().expect("the private runtime");
    assert!(
        held.actor
            .team_dissolved(&team, 40)
            .expect("orphan the worker")
            .0
    );
    let rows = the_rows();
    let orphan = rows
        .workers
        .iter()
        .find(|held| held.id == worker)
        .expect("the orphan");
    assert_eq!(orphan.state, WorkerState::Orphaned);
    assert_eq!(orphan.checkout.as_deref(), Some(CHECKOUT));
    let image = held.actor.view().expect("the orphan image");
    let rebuilt = super::cached_ledger(&held, &image).expect("the orphan ledger rebuilds");
    assert!(
        rebuilt
            .runs()
            .iter()
            .flat_map(|run| run.workers.iter())
            .any(|worker| worker.id == orphan.id && worker.checkout.as_deref() == Some(CHECKOUT))
    );
    let reserved =
        super::last_agent_in_checkout(CHECKOUT).expect("the orphan's checkout is remembered");
    assert!(
        reserved.sleeping,
        "the renderer was allowed to launch a generic second owner"
    );
    assert_eq!(reserved.agent, "codex");

    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);
    drop(private);
}

/// A released worker's checkout is one nobody is coming back to, and a
/// SLEEPING worker's is not — however many released rows sit beside it.
///
/// The sleeping half is the safety property. A restart leaves interrupted
/// workers `Sleeping` with their checkout reserved, `reseat_sleeping`
/// walks them straight back into those directories, and a reclaim that
/// read the released row beside a sleeper would delete the tree a worker
/// is about to be seated in.
#[test]
fn a_settled_checkout_is_one_no_worker_is_coming_back_to() {
    const LEADER: u32 = 91_360;
    const FIRST_WORKER: u32 = 91_361;
    const SECOND_WORKER: u32 = 91_362;

    struct EachInItsOwn {
        checkouts: Vec<String>,
        next: std::sync::atomic::AtomicU32,
        cut: Mutex<usize>,
    }

    impl Host for EachInItsOwn {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            token: &str,
        ) -> Option<u32> {
            let _ = crate::agent_teams::take_worker_host_ask(token);
            let mut cut = self.cut.lock().unwrap_or_else(|held| held.into_inner());
            crate::agent_teams::place_seat_checkout(token, self.checkouts[*cut].clone());
            *cut += 1;
            Some(self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst))
        }

        fn send(&self, _term: u32, _text: &str) -> bool {
            true
        }

        fn capture(&self, _term: u32) -> Option<String> {
            Some(String::new())
        }

        fn focus(&self, _term: u32) -> bool {
            true
        }

        fn close(&self, _term: u32) {}

        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }

    let (private, _store) = PrivateWindow::boot();
    let finished = tempfile::tempdir().expect("the finished worker's checkout");
    let finished = finished.path().to_string_lossy().into_owned();
    let interrupted = tempfile::tempdir().expect("the interrupted worker's checkout");
    let interrupted = interrupted.path().to_string_lossy().into_owned();
    let host = EachInItsOwn {
        checkouts: vec![interrupted.clone(), finished.clone()],
        next: std::sync::atomic::AtomicU32::new(FIRST_WORKER),
        cut: Mutex::new(0),
    };
    let team = format!("team-settled-{LEADER}");
    seat_a_team(&team, LEADER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let say = |argv: &str, now_ms: i64| {
        let answered = run(
            &host,
            Vec::new(),
            &team,
            leader,
            TEST_CAPABILITY,
            &words(argv),
            now_ms,
        );
        assert_eq!(answered.exit_code, 0, "{argv}: {}", answered.stderr);
        answered.stdout
    };
    say("run-create --name settled", 10);
    let task = say("task-create --spec carried", 11);
    let task: serde_json::Value = serde_json::from_str(&task).expect("a task");
    let task = task["taskId"].as_str().expect("a task id").to_string();
    // The first cut lands in `interrupted` and keeps its dispatch, which
    // is what makes it a sleeper across the restart below; the second
    // lands in `finished` and is released.
    say(&format!("worker-start --agent codex --task {task}"), 12);
    say("worker-start --agent claude", 13);
    // The second worker was let go of. Its checkout is the one a reclaim
    // may look at.
    let listed = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("worker-list"),
        30,
    );
    let listed: serde_json::Value = serde_json::from_str(&listed.stdout).expect("workers");
    let released = listed["workers"]
        .as_array()
        .expect("a worker array")
        .iter()
        .find(|worker| worker["checkout"].as_str() == Some(finished.as_str()))
        .and_then(|worker| worker["workerId"].as_str())
        .expect("the worker seated in the finished checkout")
        .to_string();
    let answered = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&format!("worker-release --worker {released}")),
        40,
    );
    assert_eq!(answered.exit_code, 0, "{}", answered.stderr);
    // And the window died under the first one, which is what leaves a
    // sleeper reserved against its tree.
    let held = super::runtime().expect("the private runtime");
    assert_eq!(
        held.actor
            .window_restarted(50)
            .expect("sleep the interrupted worker")
            .0
            .sleeping,
        1
    );

    let settled = super::settled_checkouts();
    assert!(
        settled.iter().any(|one| one.path == finished),
        "the finished worker's checkout is not offered for reclaim: {settled:?}"
    );
    assert!(
        !settled.iter().any(|one| one.path == interrupted),
        "a sleeping worker's reserved checkout was offered for reclaim: {settled:?}"
    );
    assert!(
        super::checkout_is_settled(&finished) && !super::checkout_is_settled(&interrupted),
        "the single-checkout re-check disagrees with the listing"
    );
    // The name travels with it, because a reclaim reports whose checkout
    // it took.
    let reported = settled
        .iter()
        .find(|one| one.path == finished)
        .expect("the finished checkout");
    assert_eq!(reported.worker, released);
    assert_eq!(reported.agent, "claude");

    crate::agent_teams::forget_term(FIRST_WORKER);
    crate::agent_teams::forget_term(SECOND_WORKER);
    crate::agent_teams::forget_term(LEADER);
    drop(private);
}

/// A host that answers one fixed shell to a split, and remembers what it
/// was asked to close.
struct Splitting {
    spawned: std::sync::atomic::AtomicUsize,
    onto: u32,
    closed: Mutex<Vec<u32>>,
}

impl Splitting {
    fn onto(term: u32) -> Self {
        Self {
            spawned: std::sync::atomic::AtomicUsize::new(0),
            onto: term,
            closed: Mutex::new(Vec::new()),
        }
    }

    fn closed(&self) -> Vec<u32> {
        self.closed
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    }
}

impl Host for Splitting {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        _token: &str,
    ) -> Option<u32> {
        self.spawned
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Some(self.onto)
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        true
    }
    fn capture(&self, _term: u32) -> Option<String> {
        Some(String::new())
    }
    fn focus(&self, _term: u32) -> bool {
        true
    }
    fn close(&self, term: u32) {
        self.closed
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push(term);
    }
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
}

/// Readiness is a post-fence phase, and its typed screen reaches the
/// coordinator that asked for the worker.
#[test]
fn worker_readiness_runs_without_the_team_lock_and_reports_its_screen() {
    let _window = the_window();
    const LEADER_TERM: u32 = 91_350;
    const WORKER_TERM: u32 = 91_351;
    let team = format!("team-ready-outside-fence-{LEADER_TERM}");
    seat_a_team(&team, LEADER_TERM);

    struct NotReady {
        saw_locked_table: std::sync::atomic::AtomicBool,
    }
    impl Host for NotReady {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            Some(WORKER_TERM)
        }
        fn await_worker_ready(
            &self,
            _term: u32,
        ) -> Result<(), crate::agent_teams::HostStartFailure> {
            self.saw_locked_table.store(
                crate::agent_teams::team_table_is_locked(),
                std::sync::atomic::Ordering::SeqCst,
            );
            Err(crate::agent_teams::HostStartFailure::new(
                "composer never became ready",
                Some("Login required\nPress Enter".to_string()),
            ))
        }
        fn send(&self, _term: u32, _text: &str) -> bool {
            false
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = NotReady {
        saw_locked_table: std::sync::atomic::AtomicBool::new(false),
    };
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &host,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-create --name readiness"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let started = run(
        &host,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("worker-start --agent claude --retry-request ready-refusal"),
        clock(),
    );
    assert_eq!(started.exit_code, 1, "readiness refusal became success");
    assert!(
        !host
            .saw_locked_table
            .load(std::sync::atomic::Ordering::SeqCst),
        "readiness ran inside the team-table fence"
    );
    assert!(
        started.stderr.contains("composer never became ready")
            && started.stderr.contains("Login required")
            && started.stderr.contains("Press Enter"),
        "the typed readiness evidence was discarded: {}",
        started.stderr
    );
    assert!(
        crate::agent_teams::teams()
            .get(&team)
            .is_some_and(|held| held.term_of("%2").is_none()),
        "a refused readiness left a ghost pane in the team table"
    );
    crate::agent_teams::forget_term(LEADER_TERM);
}

/// A road that reads a seat has not let go of the table before it acts
/// on what it read.
///
/// The succession table pointed this property at the actor battery — but
/// that battery drives a PRIVATE fixture table, so the one
/// implementation that actually takes this lock (`ShellPaneTable`)
/// could drop it and every test would stay green. The witness the
/// migration lost, restored: the callback runs while the real team
/// table is held.
#[test]
fn the_seat_resolution_holds_the_table_it_reads() {
    let _window = the_window();
    const LEADER_TERM: u32 = 91_400;
    let team = format!("team-holding-{LEADER_TERM}");
    seat_a_team(&team, LEADER_TERM);
    let mut witnessed = None;
    ShellPaneTable.with_seat_of_term(LEADER_TERM, &mut |seat| {
        witnessed = Some((
            crate::agent_teams::table_is_held_for_tests(),
            seat.map(|(team, pane)| (team.to_string(), pane.to_string())),
        ));
    });
    let (held, seat) = witnessed.expect("the callback ran");
    assert!(
        held,
        "the seat was read and acted on without the table lock"
    );
    assert_eq!(
        seat,
        Some((
            team.clone(),
            zerocode_core::agent_teams::LEADER_PANE.to_string()
        )),
        "the resolved seat is not the one seated"
    );
}

/// A spawn that failed once is not a request that must never run.
///
/// The beat asks again under a DETERMINISTIC name — `beat-{task}-{n}` —
/// and nothing else can rename it. Until this landed the lane settled a
/// refused spawn `retryable: false`, which pinned that name to `Refused`
/// forever: the task stayed Ready, the beat kept choosing the same task
/// under the same name, and the run behind it never moved again — with
/// no line on any screen saying why. The journal re-proves incarnation,
/// authority and emptiness on the next attempt, so the honest word for
/// one failed spawn is retryable.
#[test]
fn a_spawn_that_failed_once_may_be_asked_again_by_the_same_name() {
    let _window = the_window();
    const LEADER_TERM: u32 = 91_300;
    let team = format!("team-asked-again-{LEADER_TERM}");
    seat_a_team(&team, LEADER_TERM);
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    struct Reluctant {
        onto: u32,
        refuse_one: std::sync::atomic::AtomicBool,
    }
    impl Host for Reluctant {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            if self
                .refuse_one
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                None
            } else {
                Some(self.onto)
            }
        }
        fn send(&self, _term: u32, _text: &str) -> bool {
            true
        }
        fn capture(&self, _term: u32) -> Option<String> {
            Some(String::new())
        }
        fn focus(&self, _term: u32) -> bool {
            true
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }
    let host = Reluctant {
        onto: 91_301,
        refuse_one: std::sync::atomic::AtomicBool::new(true),
    };
    let made = run(
        &host,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-create --name asked-again"),
        clock(),
    );
    assert_eq!(made.exit_code, 0, "{}", made.stderr);
    let named = "worker-start --agent claude --retry-request beat-asked-again-1";
    let first_at = clock();
    let first = run(
        &host,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words(named),
        first_at,
    );
    assert_eq!(
        first.exit_code, 1,
        "a refused spawn was answered as a success: {first:?}"
    );
    // Strictly past the retry floor the settlement wrote (first_at + 1).
    let mut second_at = clock();
    while second_at <= first_at.saturating_add(1) {
        std::hint::spin_loop();
        second_at = clock();
    }
    let second = run(
        &host,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words(named),
        second_at,
    );
    assert_eq!(
        second.exit_code, 0,
        "the SAME name was refused for good after one failed spawn: {}",
        second.stderr
    );
    let started: serde_json::Value = serde_json::from_str(&second.stdout).expect("a worker");
    assert!(
        started["workerId"].as_str().is_some(),
        "the second ask did not seat a worker: {}",
        second.stdout
    );
}

#[test]
fn a_worker_preflight_refusal_reaches_the_coordinator() {
    let _window = the_window();
    const LEADER_TERM: u32 = 91_360;
    let team = format!("team-preflight-reason-{LEADER_TERM}");
    seat_a_team(&team, LEADER_TERM);

    struct RefusesWithReason;
    impl Host for RefusesWithReason {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            token: &str,
        ) -> Option<u32> {
            crate::agent_teams::place_worker_host_failure(
                token,
                crate::agent_teams::HostStartFailure::new("selected account needs login", None),
            );
            None
        }
        fn send(&self, _term: u32, _text: &str) -> bool {
            false
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }

    let opened = run(
        &RefusesWithReason,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words("run-create --name preflight-reason"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let refused = run(
        &RefusesWithReason,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words("worker-start --agent claude --retry-request preflight-reason"),
        clock(),
    );
    assert_eq!(refused.exit_code, 1);
    assert!(
        refused.stderr.contains("selected account needs login")
            && !refused.stderr.contains("could not open a pane"),
        "the host's concrete refusal was discarded: {}",
        refused.stderr
    );
    crate::agent_teams::forget_term(LEADER_TERM);
}

/// A worker standing in a pane of its own, summoned through the whole
/// production road — plan, journal, host, table — against a host that
/// answers the split with `term`. Answers the run, the worker, and the
/// pane the plan minted for it.
///
/// Started bare on purpose: `worker-release` is cleaning up AFTER a
/// worker, and it refuses one that still holds an open dispatch. What
/// the callers measure is the retirement, not the refusal that guards
/// it.
fn a_worker_in_a_pane(team: &str, leader_term: u32, term: u32) -> (String, String, String) {
    seat_a_team(team, leader_term);
    let host = Splitting::onto(term);
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &host,
        Vec::new(),
        team,
        seat,
        TEST_CAPABILITY,
        &words("run-create --name releasing"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let opened: serde_json::Value = serde_json::from_str(&opened.stdout).expect("a run");
    let run_id = opened["runId"].as_str().expect("a run id").to_string();
    let started = run(
        &host,
        Vec::new(),
        team,
        seat,
        TEST_CAPABILITY,
        &words("worker-start --agent claude"),
        clock(),
    );
    assert_eq!(started.exit_code, 0, "{}", started.stderr);
    let started: serde_json::Value = serde_json::from_str(&started.stdout).expect("a worker");
    (
        run_id,
        started["workerId"]
            .as_str()
            .expect("a worker id")
            .to_string(),
        started["pane"].as_str().expect("a pane").to_string(),
    )
}

/// A release interrupted by a respawn retires nothing.
///
/// The defect this closes, found by the Codex session and independently by
/// a read-only audit: the release road drops both guards to read a screen —
/// it must, a capture reaches the pty — and then acted on what it had read
/// as if nothing could have moved. A `respawn-pane` in that window KEEPS
/// the pane id and replaces the terminal behind it, so the release marked
/// the ledger released, deleted the REPLACEMENT pane, and closed only the
/// old term: a worker started a moment ago, retired by somebody else's
/// verb, with its seat taken away so nothing could ever settle it.
///
/// The respawn happens INSIDE `capture`, so this is the interleaving
/// produced rather than a race waited for.
#[test]
fn a_release_interrupted_by_a_respawn_retires_nothing() {
    const LEADER_TERM: u32 = 10_200;
    const WORKER_TERM: u32 = 10_201;
    const REPLACEMENT_TERM: u32 = 10_202;
    let _window = the_window();
    let team = format!("team-respawned-{LEADER_TERM}");
    // Not named `run`: that is this file's own verb road, and shadowing it
    // here would make the call below a String.
    let (run_id, worker, pane) = a_worker_in_a_pane(&team, LEADER_TERM, WORKER_TERM);

    let host = Releasing::respawning(&team, &pane, REPLACEMENT_TERM);
    let said = run(
        &host,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        // Named rather than bound: this test builds its run through the
        // ledger, and binding is what `run-create` does for a real caller.
        &words(&format!("worker-release --run {run_id} --worker {worker}")),
        clock(),
    );
    assert_eq!(said.exit_code, 0, "{}", said.stderr);
    let answered: serde_json::Value = serde_json::from_str(&said.stdout).expect("json");
    assert_eq!(
        answered["archived"], false,
        "a release that could not be completed claimed it archived: {answered}"
    );
    assert_eq!(
        answered["state"], "release_unknown",
        "the ledger claimed a retirement that did not happen: {answered}"
    );

    // The replacement is untouched: still seated, still on its own term.
    assert_eq!(
        crate::agent_teams::teams()
            .get(&team)
            .and_then(|held| held.term_of(&pane)),
        Some(REPLACEMENT_TERM),
        "the release deleted the pane a respawn had just re-let"
    );
    assert!(
        host.closed().is_empty(),
        "the release closed a terminal on a stale reading: {:?}",
        host.closed()
    );
}

/// A `worker-read` answer belongs to the exact pane incarnation captured.
///
/// The old road dropped both guards, captured the old term, then filed the
/// screen and retry receipt without asking whether the seat had moved. A
/// respawn in that gap therefore made the replacement replay the old
/// shell's bytes forever. Term and capability generation are checked as
/// separate cases: accepting either mutation would still cross an
/// incarnation boundary.
#[test]
fn a_worker_read_interrupted_by_a_seat_change_files_nothing() {
    for (case, move_caller, replacement_term, replacement_generation, retry_capability) in [
        ("term", false, Some(90_102), None, TEST_CAPABILITY),
        (
            "generation",
            false,
            None,
            Some("replacement-worker-capability"),
            TEST_CAPABILITY,
        ),
        (
            "caller-generation",
            true,
            None,
            Some("replacement-caller-capability"),
            "replacement-caller-capability",
        ),
    ] {
        let leader_term = match case {
            "term" => 90_100,
            "generation" => 90_110,
            _ => 90_120,
        };
        let worker_term = leader_term + 1;
        let _window = the_window();
        let team = format!("team-read-stale-{case}-{leader_term}");
        let (run_id, worker, pane) = a_worker_in_a_pane(&team, leader_term, worker_term);
        /* No retry name: a `worker-read` is a `Doing::HostRead` and files
         * nothing, so one cannot be given. What this still measures is the
         * incarnation guard, which is the half that was ever load-bearing —
         * the receipt half of this bug cannot happen any more because
         * there is no receipt. */
        let argv = words(&format!("worker-read --run {run_id} --worker {worker}"));
        let run_before = a_runs_shadow(&run_id);

        let stale = Reading::moving(
            "screen from the old incarnation",
            &team,
            match move_caller {
                true => zerocode_core::agent_teams::LEADER_PANE,
                false => &pane,
            },
            replacement_term,
            replacement_generation,
        );
        let said = run(
            &stale,
            Vec::new(),
            &team,
            zerocode_core::agent_teams::LEADER_PANE,
            TEST_CAPABILITY,
            &argv,
            clock(),
        );
        assert_eq!(said.exit_code, 1, "{case}: {said:?}");
        assert!(said.stdout.is_empty(), "{case}: stale bytes escaped");
        assert_eq!(
            stale.captures(),
            1,
            "{case}: capture was not attempted once"
        );
        assert_eq!(
            a_runs_shadow(&run_id),
            run_before,
            "{case}: refusing a stale read changed its run"
        );

        // Asking again must reach the replacement rather than an old
        // answer. It cannot do otherwise now — nothing is filed — and the
        // capture count below is what says so out loud.
        let replacement = Reading::quiet("screen from the current incarnation");
        let retried = run(
            &replacement,
            Vec::new(),
            &team,
            zerocode_core::agent_teams::LEADER_PANE,
            retry_capability,
            &argv,
            clock(),
        );
        assert_eq!(retried.exit_code, 0, "{case}: {}", retried.stderr);
        assert_eq!(retried.stdout, "screen from the current incarnation\n");
        assert_eq!(
            replacement.captures(),
            1,
            "{case}: something other than the host answered the retry"
        );
    }
}

/// `worker-transcript` answers the worker ROW's own transcript as
/// structured turns (t-6742): the file its pane reported, read by the
/// conversation view's reader and shaped by core — the screen is never
/// captured, the path never printed, a credential in the file never
/// answered, and the read moves nothing: not the rows, not the file, and
/// no receipt. A file the window cannot open is said as unavailable.
#[test]
fn a_worker_transcript_reads_the_rows_own_file_and_never_the_screen() {
    const LEADER_TERM: u32 = 90_140;
    const WORKER_TERM: u32 = 90_141;
    let _window = the_window();
    let team = format!("team-transcript-{LEADER_TERM}");
    let (run_id, worker, _pane) = a_worker_in_a_pane(&team, LEADER_TERM, WORKER_TERM);
    let temp = tempfile::tempdir().expect("a transcript folder");
    let path = temp.path().join("w.jsonl");
    std::fs::write(
        &path,
        [
            r#"{"type":"user","timestamp":"2026-09-24T12:00:00.000Z","message":{"role":"user","content":"run the tests with --password hunter2"}}"#,
            r#"{"type":"assistant","timestamp":"2026-09-24T12:00:01.000Z","message":{"role":"assistant","content":[{"type":"text","text":"Running them."},{"type":"tool_use","id":"call-1","name":"Bash","input":{"command":"cargo test -p x"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-09-24T12:00:09.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"call-1","content":"ok · 12 passed"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-09-24T12:00:10.000Z","message":{"role":"assistant","content":[{"type":"text","text":"All green."}]}}"#,
            "",
        ]
        .join("\n"),
    )
    .expect("the transcript");
    let written = std::fs::metadata(&path).expect("stat");
    super::pane_session_reported(
        WORKER_TERM,
        &zerocode_core::ProviderSession {
            key: zerocode_core::provider_session::SessionKey::SessionId,
            id: "session-of-the-row".to_string(),
            transcript_path: Some(path.to_string_lossy().into_owned()),
        },
        clock(),
    );
    let host = Reading::quiet("THE SCREEN NOBODY ASKED FOR");
    let shadow = a_runs_shadow(&run_id);
    let receipts = the_rows().served.len();

    let answered = run(
        &host,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(&format!(
            "worker-transcript --run {run_id} --worker {worker} --json"
        )),
        clock(),
    );
    assert_eq!(answered.exit_code, 0, "{}", answered.stderr);
    let json: serde_json::Value = serde_json::from_str(&answered.stdout).expect("a record");
    assert_eq!(json["worker"], worker);
    assert_eq!(json["agent"], "claude");
    assert_eq!(
        json["found"], 3,
        "the prompt, the call's step, the words after"
    );
    assert_eq!(json["turns"][0]["role"], "user");
    assert_eq!(json["turns"][1]["role"], "assistant");
    assert_eq!(json["turns"][2]["text"]["text"], "All green.");
    assert_eq!(json["turns"][1]["tools"][0]["name"], "Bash");
    assert_eq!(json["turns"][1]["tools"][0]["callId"], "call-1");
    assert_eq!(
        json["turns"][1]["tools"][0]["result"]["text"],
        "ok · 12 passed"
    );
    assert_eq!(json["turns"][1]["atMs"], 1_790_251_201_000_i64);
    assert_eq!(json["scan"]["fileBytes"], written.len());
    assert!(
        !answered.stdout.contains("hunter2"),
        "a credential was answered"
    );
    assert!(answered.stdout.contains("[redacted]"));
    assert!(
        !answered
            .stdout
            .contains(temp.path().to_str().expect("utf8")),
        "the path escaped"
    );
    assert!(!answered.stdout.contains("NOBODY ASKED FOR"));
    assert_eq!(host.captures(), 0, "the screen was captured");

    let text = run(
        &host,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(&format!(
            "worker-transcript --run {run_id} --worker {worker}"
        )),
        clock(),
    );
    assert_eq!(text.exit_code, 0, "{}", text.stderr);
    assert!(
        text.stdout
            .contains("↳ Bash · cargo test -p x → ok · 12 passed"),
        "{}",
        text.stdout
    );
    assert!(!text.stdout.contains("hunter2"));

    // A read: the rows did not move, no receipt was filed, the file was
    // not touched.
    assert_eq!(a_runs_shadow(&run_id), shadow, "a read moved the run");
    assert_eq!(the_rows().served.len(), receipts, "a read filed a receipt");
    let after = std::fs::metadata(&path).expect("stat");
    assert_eq!(after.len(), written.len());
    assert_eq!(after.modified().ok(), written.modified().ok());

    // The file gone: unavailable, said without the path, never the screen.
    std::fs::remove_file(&path).expect("gone");
    let gone = run(
        &host,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(&format!(
            "worker-transcript --run {run_id} --worker {worker}"
        )),
        clock(),
    );
    assert_eq!(gone.exit_code, 1, "{gone:?}");
    assert!(gone.stdout.is_empty());
    assert!(
        gone.stderr.contains(TRANSCRIPT_UNAVAILABLE),
        "{}",
        gone.stderr
    );
    assert!(!gone.stderr.contains(temp.path().to_str().expect("utf8")));
    assert_eq!(host.captures(), 0);
}

/// The word a `worker-transcript` refusal for a transcript it cannot read
/// opens with (`zerocode_core::worker_transcript::UNAVAILABLE`), spelled as a
/// caller branching on it sees it — so the tests below name no module of the
/// verb's, compile against a window that has no such verb, and fail there on
/// their assertions rather than on the build (t-6742 R4).
const TRANSCRIPT_UNAVAILABLE: &str = "transcript unavailable";

/// `worker-transcript` of a worker whose row reported `transcript` as its
/// file, asked twice — `--json`, and the text rendering of the same record —
/// down the verb's own road (t-6742): the plan names the row's file, the
/// window reads it with the conversation view's reader, core shapes what was
/// read. The screen is never captured and the file's path never answered.
/// Each caller names terms of its own.
fn transcript_answers(
    leader_term: u32,
    worker_term: u32,
    transcript: &str,
) -> (serde_json::Value, String) {
    let _window = the_window();
    let team = format!("team-transcript-{leader_term}");
    let (run_id, worker, _pane) = a_worker_in_a_pane(&team, leader_term, worker_term);
    let temp = tempfile::tempdir().expect("a transcript folder");
    let path = temp.path().join("w.jsonl");
    std::fs::write(&path, transcript).expect("the transcript");
    super::pane_session_reported(
        worker_term,
        &zerocode_core::ProviderSession {
            key: zerocode_core::provider_session::SessionKey::SessionId,
            id: format!("session-{worker_term}"),
            transcript_path: Some(path.to_string_lossy().into_owned()),
        },
        clock(),
    );
    let host = Reading::quiet("THE SCREEN NOBODY ASKED FOR");
    let asked = |rendering: &str| {
        run(
            &host,
            Vec::new(),
            &team,
            zerocode_core::agent_teams::LEADER_PANE,
            TEST_CAPABILITY,
            &words(&format!(
                "worker-transcript --run {run_id} --worker {worker} {rendering}"
            )),
            clock(),
        )
    };
    let json = asked("--json");
    assert_eq!(json.exit_code, 0, "{}", json.stderr);
    let text = asked("");
    assert_eq!(text.exit_code, 0, "{}", text.stderr);
    assert_eq!(host.captures(), 0, "the screen was captured");
    let folder = temp.path().to_string_lossy().into_owned();
    for said in [&json.stdout, &text.stdout] {
        assert!(!said.contains("NOBODY ASKED FOR") && !said.contains(&folder));
    }
    (
        serde_json::from_str(&json.stdout).expect("a record"),
        text.stdout,
    )
}

/// One transcript line, from its JSON.
fn a_transcript_line(row: &serde_json::Value) -> String {
    format!("{row}\n")
}

/// The brief's first red (t-6742), end to end: a transcript's texts come
/// out of the verb masked in both renderings — the person's prompt, the
/// assistant's words, a call's input, its result — and a result past the
/// digest's cap is cut, says it was, and says how long it was.
#[test]
fn a_transcript_result_is_cut_at_the_digest_cap_and_masked() {
    let result = format!(
        "{} DB_PASSWORD=sekretvalue1234567890 {}",
        "x".repeat(390),
        "y".repeat(600)
    );
    let transcript = [
        serde_json::json!({"type":"user","timestamp":"2026-09-24T12:00:00.000Z","message":{"role":"user","content":"deploy with --password hunter2 please"}}),
        serde_json::json!({"type":"assistant","timestamp":"2026-09-24T12:00:01.000Z","message":{"role":"assistant","content":[
            {"type":"text","text":"Using Bearer abc.def now"},
            {"type":"tool_use","id":"c","name":"Bash","input":{"command":"curl -H 'Authorization: Bearer abc.def' https://user:tok3n@host.test/x"}}
        ]}}),
        serde_json::json!({"type":"user","timestamp":"2026-09-24T12:00:02.000Z","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"c","content": result}
        ]}}),
    ]
    .iter()
    .map(a_transcript_line)
    .collect::<String>();
    let (json, text) = transcript_answers(90_150, 90_151, &transcript);
    for said in [json.to_string(), text.clone()] {
        for secret in ["hunter2", "abc.def", "tok3n", "sekretvalue"] {
            assert!(!said.contains(secret), "{secret} escaped: {said}");
        }
        assert!(
            said.contains(zerocode_core::credential::MASK),
            "nothing was masked: {said}"
        );
    }
    let cut = &json["turns"][1]["tools"][0]["result"];
    let kept = cut["text"].as_str().expect("a result").chars().count();
    let chars = cut["chars"].as_u64().expect("its length");
    assert_eq!(cut["truncated"], true, "{cut}");
    assert_eq!(
        chars,
        result.chars().count() as u64,
        "the length said is the text's own"
    );
    assert!((kept as u64) < chars, "{kept} of {chars}");
    assert_eq!(json["truncated"], true);
    assert!(text.contains(&format!("…(of {chars} chars)")), "{text}");
}

/// A call's name and id are texts too (t-6742 R1): masked by the same table
/// in both renderings, while a result still finds its own call — the join is
/// on the ids as written, so two ids the mask folds into one `[redacted]`
/// never trade results.
#[test]
fn transcript_metadata_is_masked_in_both_renderings() {
    let mask = zerocode_core::credential::MASK;
    let first = format!("ghp_{}", "a1".repeat(18));
    let second = format!("ghp_{}", "b2".repeat(18));
    let named = format!("deploy_sk-{}", "c3".repeat(12));
    let transcript = [
        serde_json::json!({"type":"user","message":{"role":"user","content":"go"}}),
        serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[
            {"type":"tool_use","id": first,"name": named,"input":{"command":"one"}},
            {"type":"tool_use","id": second,"name":"Bash","input":{"command":"two"}}
        ]}}),
        serde_json::json!({"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id": second,"content":"result of two"},
            {"type":"tool_result","tool_use_id": first,"content":"result of one"}
        ]}}),
    ]
    .iter()
    .map(a_transcript_line)
    .collect::<String>();
    let (json, text) = transcript_answers(90_152, 90_153, &transcript);
    for said in [json.to_string(), text.clone()] {
        for secret in [first.as_str(), second.as_str(), named.as_str()] {
            assert!(!said.contains(secret), "{secret} escaped: {said}");
        }
    }
    let calls = &json["turns"][1]["tools"];
    assert_eq!(calls[0]["callId"], mask, "{calls}");
    assert_eq!(calls[1]["callId"], mask, "{calls}");
    assert_eq!(calls[0]["name"], mask, "{calls}");
    assert_eq!(
        calls[1]["name"], "Bash",
        "a name that is no credential stays"
    );
    assert_eq!(
        calls[0]["result"]["text"], "result of one",
        "a result joined on the masked id: {calls}"
    );
    assert_eq!(calls[1]["result"]["text"], "result of two", "{calls}");
    assert!(
        text.contains(&format!("↳ {mask} · one → result of one"))
            && text.contains("↳ Bash · two → result of two"),
        "{text}"
    );
}

/// The conversation view's 16 KiB cell never stands in front of a digest
/// (t-6742 R1): the length a digest says is its text's whole length, not the
/// view's cut of it, and a header whose value runs past that cut keeps the
/// lines after it — masked whole first, then cut.
#[test]
fn a_digest_keeps_the_shared_readers_prior_cut_visible() {
    let rows: String = (0..1_000)
        .map(|at| format!("row {at:04} of the listing\n"))
        .collect();
    let header = format!(
        "Authorization: Bearer {}\nthen: cargo test -p x",
        "q".repeat(17_000)
    );
    let transcript = [
        serde_json::json!({"type":"user","message":{"role":"user","content":"go"}}),
        serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"call-1","name":"Bash","input":{"command":"ls -R"}},
            {"type":"tool_use","id":"call-2","name":"Bash","input":{"command": header}}
        ]}}),
        serde_json::json!({"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"call-1","content": rows},
            {"type":"tool_result","tool_use_id":"call-2","content":"ok"}
        ]}}),
    ]
    .iter()
    .map(a_transcript_line)
    .collect::<String>();
    assert!(rows.chars().count() > 16 * 1024 && header.chars().count() > 16 * 1024);
    let (json, text) = transcript_answers(90_154, 90_155, &transcript);
    let listing = &json["turns"][1]["tools"][0]["result"];
    assert_eq!(
        listing["chars"],
        rows.chars().count(),
        "the length said is a cut's, not the text's: {listing}"
    );
    assert_eq!(listing["truncated"], true);
    assert!(
        text.contains(&format!("…(of {} chars)", rows.chars().count())),
        "{text}"
    );
    let input = &json["turns"][1]["tools"][1]["input"];
    assert_eq!(input["chars"], header.chars().count(), "{input}");
    assert!(
        input["text"]
            .as_str()
            .is_some_and(|kept| kept.contains("then: cargo test -p x")),
        "the line after the header was lost: {input}"
    );
    assert!(!json.to_string().contains("qqqqqqqq") && !text.contains("qqqqqqqq"));
}

/// A URL whose `@` stands past the conversation view's cut still loses its
/// userinfo (t-6742 R1): masked while the `@` is there to mark it, then cut —
/// in a call's input and in its result, in both renderings.
#[test]
fn a_url_cut_before_its_at_sign_still_hides_its_userinfo() {
    let url = format!("https://demo:{}@host.test/x", "z".repeat(20_000));
    let transcript = [
        serde_json::json!({"type":"user","message":{"role":"user","content":"go"}}),
        serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[
            {"type":"tool_use","id":"call-1","name":"Bash","input":{"command": format!("curl {url}")}}
        ]}}),
        serde_json::json!({"type":"user","message":{"role":"user","content":[
            {"type":"tool_result","tool_use_id":"call-1","content": format!("fetched {url}")}
        ]}}),
    ]
    .iter()
    .map(a_transcript_line)
    .collect::<String>();
    let (json, text) = transcript_answers(90_156, 90_157, &transcript);
    for said in [json.to_string(), text] {
        assert!(
            !said.contains("demo:") && !said.contains("zzzzzzzz"),
            "the userinfo escaped: {}",
            &said[..said.len().min(600)]
        );
        assert!(said.contains("https://***@host.test/x"), "{said}");
    }
}

/// A line longer than both of the verb's reads is said, not read whole
/// (t-6742 R2): the answer holds no turn out of it, says a line was skipped,
/// and says it read exactly its budget — a chunk and the wider read, each
/// with the byte before it. A line inside the wider read is read whole, for
/// the same budget.
#[test]
fn a_worker_transcript_says_a_line_past_its_read_budget_instead_of_reading_it() {
    let chunk = crate::shell_runtime::SUBAGENT_LOG_CHUNK;
    let widest = zerocode_core::worker_transcript::SCAN_CHUNKS * chunk;
    let budget = chunk + 1 + widest + 1;
    let prompts: String = (0..20_000)
        .map(|at| {
            a_transcript_line(
                &serde_json::json!({"type":"user","message":{"role":"user","content":format!("prompt {at:05}")}}),
            )
        })
        .collect();
    let with_a_result = |len: u64| {
        prompts.clone()
            + &a_transcript_line(
                &serde_json::json!({"type":"user","message":{"role":"user","content":[
                    {"type":"tool_result","tool_use_id":"call-1","content": "s".repeat(usize::try_from(len).expect("a length"))}
                ]}}),
            )
    };

    let (json, text) = transcript_answers(90_158, 90_159, &with_a_result(2 * widest));
    assert_eq!(json["found"], 0, "a line past the budget was answered");
    assert_eq!(json["scan"]["skipped"], true, "{}", json["scan"]);
    assert_eq!(json["scan"]["readBytes"], budget, "{}", json["scan"]);
    assert!(text.contains("a line skipped"), "{text}");

    let (json, _) = transcript_answers(90_160, 90_161, &with_a_result(chunk + 40_000));
    assert_eq!(json["scan"]["skipped"], false, "{}", json["scan"]);
    assert_eq!(json["scan"]["readBytes"], budget, "{}", json["scan"]);
    let last = json["turns"]
        .as_array()
        .and_then(|turns| turns.last())
        .expect("a turn");
    assert_eq!(
        last["tools"][0]["result"]["chars"],
        chunk + 40_000,
        "the long line, whole"
    );
}

/// A `worker-read` that arrives with a retry name is refused, and the
/// screen is never touched.
///
/// This test used to say the opposite — that a named read filed a receipt
/// and replayed it. That contract is gone: a read that writes nothing down
/// has no answer to file, and a name on it taught the caller it was
/// protected when it was not. What is left is the part that still costs
/// something: the refusal has to arrive BEFORE the capture, or the window
/// would have spent a look at somebody's pane to refuse to record it.
#[test]
fn a_named_worker_read_is_refused_before_the_screen_is_touched() {
    const LEADER_TERM: u32 = 90_130;
    const WORKER_TERM: u32 = 90_131;
    let _window = the_window();
    let team = format!("team-read-stable-{LEADER_TERM}");
    let (run_id, worker, _pane) = a_worker_in_a_pane(&team, LEADER_TERM, WORKER_TERM);
    let host = Reading::quiet("one stable screen");

    let named = words(&format!(
        "worker-read --run {run_id} --worker {worker} \
             --retry-request read-stable-{LEADER_TERM}"
    ));
    let refused = run(
        &host,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &named,
        clock(),
    );
    assert_eq!(refused.exit_code, 1, "{refused:?}");
    assert!(refused.stdout.is_empty(), "bytes escaped a refusal");
    assert!(
        refused.stderr.contains("worker-read") && refused.stderr.contains("screen"),
        "the refusal did not say what it was: {}",
        refused.stderr
    );
    assert_eq!(host.captures(), 0, "the refusal still asked for the screen");

    // And without the name it is answered, once, from the host.
    let plain = words(&format!("worker-read --run {run_id} --worker {worker}"));
    let said = run(
        &host,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &plain,
        clock(),
    );
    assert_eq!(said.exit_code, 0, "{}", said.stderr);
    assert_eq!(said.stdout, "one stable screen\n");
    assert_eq!(host.captures(), 1);
}

/// A release in one team leaves another team's pane of the same name alone.
///
/// Pane ids are unique only inside a team: every team numbers its first
/// teammate the same way, so two coordinators open at once both have a
/// `%2`. The release road removed the pane id from EVERY team, which
/// silently unseated a stranger's live worker — and a worker with no seat
/// can never be settled by `terminal_gone` again, so its dispatch stays
/// open and its standing order's ceiling stays spent for the rest of the
/// session.
#[test]
fn a_release_in_one_team_leaves_another_teams_pane_of_the_same_name_alone() {
    const MINE_LEADER: u32 = 10_300;
    const MINE_WORKER: u32 = 10_301;
    const THEIRS_LEADER: u32 = 10_400;
    const THEIRS_WORKER: u32 = 10_401;
    let _window = the_window();
    let mine = format!("team-mine-{MINE_LEADER}");
    let theirs = format!("team-theirs-{THEIRS_LEADER}");
    let (run_id, worker, shared) = a_worker_in_a_pane(&mine, MINE_LEADER, MINE_WORKER);
    let (_, _, theirs_pane) = a_worker_in_a_pane(&theirs, THEIRS_LEADER, THEIRS_WORKER);
    // The SAME pane id in both teams, which is what a real window has:
    // each fresh team numbers its first teammate alike.
    assert_eq!(shared, theirs_pane, "the two teams no longer collide");

    let host = Releasing::quiet();
    let said = run(
        &host,
        Vec::new(),
        &mine,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(&format!("worker-release --run {run_id} --worker {worker}")),
        clock(),
    );
    assert_eq!(said.exit_code, 0, "{}", said.stderr);
    let answered: serde_json::Value = serde_json::from_str(&said.stdout).expect("json");
    assert_eq!(answered["archived"], true, "{answered}");

    let tables = crate::agent_teams::teams();
    assert!(
        tables
            .get(&mine)
            .and_then(|held| held.pane(&shared))
            .is_none(),
        "the released pane is still seated in its own team"
    );
    assert_eq!(
        tables.get(&theirs).and_then(|held| held.term_of(&shared)),
        Some(THEIRS_WORKER),
        "releasing one team's pane unseated another team's live worker of \
             the same name"
    );
    drop(tables);
    assert_eq!(host.closed(), vec![MINE_WORKER]);
}

/* Two more fence tests fell here, and one door test replaces their
 * ground:
 *
 * · `a_stale_term_cannot_settle_the_replacement_in_the_same_seat` — the
 *   caller-held triple (`PaneBinding`) it drove is gone; the actor pins
 *   a settlement by TERM under its own table borrow, so a stale term
 *   resolves to no seat and settles nothing, by construction.
 *
 * · `a_settlement_does_not_let_go_of_the_seat_it_read` — the lock-hold
 *   probe (`team_table_is_locked` from inside the gap) has no gap left
 *   to probe; the same property is the actor's `with_seat_of_term`
 *   contract, exercised by its `seat_gone`/`turn_ended` battery.
 */

/// A standing order cuts a pane without anybody typing.
///
/// The pure half is decided in core and tested there; what only this file
/// can get wrong is the carrying-out — that the beat walks the SAME verb a
/// coordinator would have typed, at the seat the order named, and that the
/// task's spec arrives whole rather than cut at its first space.
#[test]
fn a_standing_order_summons_a_worker_with_nobody_typing() {
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    const LEADER_TERM: u32 = 9_500;
    let team = format!("team-auto-{LEADER_TERM}");
    seat_a_team(&team, LEADER_TERM);
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-create --name beating"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);

    // A spec with spaces in it, because that is what a task is.
    let spec = "read the file and report what changed";
    let wrote = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        // Built as argv rather than as a line, because the spec has spaces
        // in it — and named by hand for the same reason, since `words` is
        // the helper that would otherwise have named it.
        &[
            "task-create".to_string(),
            "--spec".to_string(),
            spec.to_string(),
            "--retry-request".to_string(),
            "test-beating-task".to_string(),
        ],
        clock(),
    );
    assert_eq!(wrote.exit_code, 0, "{}", wrote.stderr);

    // Nothing beats until the order is written down.
    let watching = Watching::new();
    assert_eq!(tick(&watching, &[], clock()), 0, "a run with no order beat");

    let armed = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-auto --agent claude --max 1"),
        clock(),
    );
    assert_eq!(armed.exit_code, 0, "{}", armed.stderr);

    assert_eq!(tick(&watching, &[], clock()), 1, "the order did not beat");
    let asked = watching.cut.lock().expect("the log").clone();
    assert_eq!(asked.len(), 1, "one beat cut {} panes", asked.len());
    let (from_team, from_term, command, worker) = asked.into_iter().next().expect("a cut");
    assert_eq!(from_team, team, "the pane was cut in another team");
    assert_eq!(from_term, LEADER_TERM, "the pane was not cut from the seat");
    assert!(
        worker
            .as_ref()
            .is_some_and(|worker| worker.prompt.contains(spec)),
        "the task's spec was cut on its way to the worker: {worker:?}; command={command}"
    );

    // The ceiling holds on the very next beat, with nothing said between.
    assert_eq!(tick(&watching, &[], clock()), 0, "the ceiling did not hold");

    // And a seat that is gone stops the order without ending it: the team
    // table answers nothing, so the beat does nothing — rather than reading
    // an absence as a decision.
    run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-auto --agent claude --max 4"),
        clock(),
    );
    crate::agent_teams::teams().remove(&team);
    assert_eq!(
        tick(&watching, &[], clock()),
        0,
        "the beat summoned a worker into a team that is gone"
    );
    assert!(
        the_rows().runs.iter().any(|held| held.auto.is_some()),
        "a missing seat stood the order down, which is a decision read out \
             of an absence"
    );
}

/// A beat that is still cutting a pane does not start a second beat.
///
/// Forking and exec-ing outlasts the gap between two beats, and the ledger
/// is only written when the cut comes back — so a second beat entering
/// meanwhile would plan against a run that still looks idle and dispatch
/// the same task twice. The second worker would arrive as a surprise on
/// work already being done, on a run whose ceiling said one.
///
/// Driven by re-entering rather than by racing two threads: the door either
/// holds against a beat that is demonstrably in flight, or it does not, and
/// a test that had to win a race to say so would be a test that sometimes
/// says nothing.
#[test]
fn a_beat_still_cutting_a_pane_does_not_start_another() {
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    const LEADER_TERM: u32 = 9_600;
    let team = format!("team-reentrant-{LEADER_TERM}");
    seat_a_team(&team, LEADER_TERM);
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    for line in ["run-create --name reentrant", "task-create --spec one"] {
        let said = run(
            &Nowhere,
            Vec::new(),
            &team,
            seat,
            TEST_CAPABILITY,
            &words(line),
            clock(),
        );
        assert_eq!(said.exit_code, 0, "{}", said.stderr);
    }
    let armed = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-auto --agent claude --max 4"),
        clock(),
    );
    assert_eq!(armed.exit_code, 0, "{}", armed.stderr);

    // A host whose `split` calls the tick again — the shape of a spawn that
    // has not returned when the next beat is due.
    struct Reentrant {
        inner: Watching,
        again: Mutex<Vec<usize>>,
    }
    impl Host for Reentrant {
        fn split(
            &self,
            team: &str,
            leader_term: u32,
            from_term: u32,
            pane: &str,
            direction: zerocode_core::agent_teams::Direction,
            command: &str,
            _token: &str,
        ) -> Option<u32> {
            // Inside the beat, before the ledger has been written. Asked
            // only ONCE however many panes are cut: without the door this
            // re-enters forever, and a mutation check that hangs proves
            // less than one that goes red — so the depth is bounded here
            // and the assertion below reads the answers.
            let mut asked = self.again.lock().expect("the log");
            if asked.is_empty() {
                asked.push(tick(&Nowhere, &[], clock()));
            }
            drop(asked);
            self.inner.split(
                team,
                leader_term,
                from_term,
                pane,
                direction,
                command,
                _token,
            )
        }
        fn send(&self, term: u32, text: &str) -> bool {
            self.inner.send(term, text)
        }
        fn capture(&self, term: u32) -> Option<String> {
            self.inner.capture(term)
        }
        fn focus(&self, term: u32) -> bool {
            self.inner.focus(term)
        }
        fn close(&self, term: u32) {
            self.inner.close(term);
        }
        fn actor_for(&self, term: u32) -> Option<String> {
            self.inner.actor_for(term)
        }
    }

    let host = Reentrant {
        inner: Watching::new(),
        again: Mutex::new(Vec::new()),
    };
    assert_eq!(
        tick(&host, &[], clock()),
        1,
        "the outer beat did not summon"
    );
    assert_eq!(
        host.again.lock().expect("the log").as_slice(),
        &[0],
        "a beat started while another was still cutting a pane"
    );
    assert_eq!(
        host.inner.cut.lock().expect("the log").len(),
        1,
        "one beat cut a pane twice"
    );
    // And the door is given back: the next beat works.
    let before = BEATS_BEGUN.load(std::sync::atomic::Ordering::SeqCst);
    let _ = tick(&host, &[], clock());
    assert_eq!(
        BEATS_BEGUN.load(std::sync::atomic::Ordering::SeqCst),
        before + 1,
        "the door was left open after the outer beat returned"
    );
}

/// A host panic cannot leave every later standing-order beat disabled.
#[test]
fn a_panicking_host_gives_the_beat_door_back() {
    let _window = the_window();
    let _turn = one_beat_at_a_time();
    const LEADER_TERM: u32 = 90_200;
    let team = format!("team-panicking-beat-{LEADER_TERM}");
    seat_a_team(&team, LEADER_TERM);
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    for line in ["run-create --name panicking-beat", "task-create --spec one"] {
        let said = run(
            &Nowhere,
            Vec::new(),
            &team,
            seat,
            TEST_CAPABILITY,
            &words(line),
            clock(),
        );
        assert_eq!(said.exit_code, 0, "{}", said.stderr);
    }
    let armed = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-auto --agent claude --max 1"),
        clock(),
    );
    assert_eq!(armed.exit_code, 0, "{}", armed.stderr);

    struct Panicking;
    impl Host for Panicking {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            panic!("injected host split panic")
        }
        fn send(&self, _term: u32, _text: &str) -> bool {
            false
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn actor_for(&self, term: u32) -> Option<String> {
            Some(test_actor(term))
        }
    }

    let before = BEATS_BEGUN.load(std::sync::atomic::Ordering::SeqCst);
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tick(&Panicking, &[], clock())
    }));
    assert!(panicked.is_err(), "the injected host panic did not happen");
    let after_panic = BEATS_BEGUN.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(after_panic, before + 1, "the panicking beat never began");

    // The pane effect crossed an unwind boundary, so its outcome is
    // unknown here. This regression deliberately neither repairs nor
    // asserts on ledger/effect state; it proves only that the process-local
    // single-flight gate admits the next beat.
    let _ = tick(&Nowhere, &[], clock());
    assert_eq!(
        BEATS_BEGUN.load(std::sync::atomic::Ordering::SeqCst),
        after_panic + 1,
        "the host panic permanently disabled standing-order beats"
    );

    /* Stand the order down. The task this test leaves is Ready forever —
     * its host panics by design, and a failed spawn is retryable now, as
     * it must be — so an order left armed here would ride into every
     * later beat on this shared window and inflate counts that are not
     * its own. */
    let put_down = run(
        &Nowhere,
        Vec::new(),
        &team,
        seat,
        TEST_CAPABILITY,
        &words("run-auto --off"),
        clock(),
    );
    assert_eq!(put_down.exit_code, 0, "{}", put_down.stderr);
}

/// A host that opens panes and remembers being asked.
type WatchedCut = (
    String,
    u32,
    String,
    Option<crate::agent_teams::WorkerHostSpec>,
);

struct Watching {
    cut: Mutex<Vec<WatchedCut>>,
    next: Mutex<u32>,
}

impl Watching {
    fn new() -> Self {
        Self {
            cut: Mutex::new(Vec::new()),
            next: Mutex::new(50_000),
        }
    }
}

impl Host for Watching {
    fn split(
        &self,
        team: &str,
        leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        command: &str,
        token: &str,
    ) -> Option<u32> {
        self.cut.lock().expect("the log").push((
            team.to_string(),
            leader_term,
            command.to_string(),
            crate::agent_teams::take_worker_host_ask(token),
        ));
        let mut next = self.next.lock().expect("the counter");
        *next += 1;
        Some(*next)
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        true
    }
    fn capture(&self, _term: u32) -> Option<String> {
        Some(String::new())
    }
    fn focus(&self, _term: u32) -> bool {
        true
    }
    fn close(&self, _term: u32) {}
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
}

/// A waiting `check` is woken by the message, not by the clock.
///
/// The flag was advertised in the verb table from the day the table was
/// written and never read, so every coordinator that followed the skill's
/// instructions was busy-polling an inbox. What it has to buy is LATENCY:
/// mail posted a moment after the look must come back at that moment, not
/// at the ceiling. So this test measures the clock — a wait that answered
/// correctly by sleeping the whole budget would be the same bug wearing a
/// green tick.
///
/// Threads and not a mock, because what is being tested is precisely the
/// handoff between two of them: the sleeper drops the ledger to wait, and
/// the sender needs the ledger to write. A single-threaded test of this
/// would be a test of a deadlock that could not happen.
#[test]
fn a_waiting_check_hears_the_message_when_it_lands_and_not_at_the_deadline() {
    let _window = the_window();
    const LEADER_TERM: u32 = 9_300;
    const WORKER_TERM: u32 = 9_301;
    let _turn = one_wait_at_a_time();
    let team = format!("team-wait-{LEADER_TERM}");
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    // Through the verb, not the field: `run-create` is what binds a caller
    // to a run, and a test that bound it by hand would be testing a seat
    // no shim can produce.
    let (_run_id, worker, pane) = a_worker_carrying_work(&team, LEADER_TERM, WORKER_TERM);

    let before = WAITS_BEGUN.load(std::sync::atomic::Ordering::SeqCst);
    let asking = {
        let team = team.clone();
        std::thread::spawn(move || {
            let began = std::time::Instant::now();
            let answer = run(
                &Nowhere,
                Vec::new(),
                &team,
                seat,
                TEST_CAPABILITY,
                &words("check --wait"),
                clock(),
            );
            (answer, began.elapsed())
        })
    };

    /* Proven asleep, not slept-for.
     *
     * This was 120ms of hoping, and the Codex session was right that a
     * machine under load turns that into a message posted BEFORE the wait
     * begins — which the first look then finds, and the assertion below
     * passes without anything ever having been woken. The counter is
     * raised by the road itself at the moment a caller is certainly
     * asleep, so this waits for the fact instead of for a duration.
     */
    until_a_wait_has_begun(before);
    let landed = std::time::Instant::now();
    let held =
        crate::agent_teams::current_pane_capability(&team, &pane).expect("the worker capability");
    let posted = run(
        &Nowhere,
        Vec::new(),
        &team,
        &pane,
        &held,
        &words(&format!(
            "send --type status --body hello --retry-request wake-{worker}"
        )),
        clock(),
    );
    assert_eq!(posted.exit_code, 0, "{}", posted.stderr);

    let (answer, took) = asking.join().expect("the sleeper panicked");
    assert_eq!(answer.exit_code, 0, "{}", answer.stderr);
    assert!(
        answer.stdout.contains("hello"),
        "the sleeper woke without the message it was waiting for: {}",
        answer.stdout
    );
    /* The ceiling is seconds; the message landed the moment the sleeper
     * was asleep. A wait that answers correctly only when the budget runs
     * out has not been woken by anything — it has outlasted the question.
     *
     * Measured from when the mail actually LANDED and ONLY from there.
     * The thread's whole life includes the plan, and the plan queues on
     * the one actor the full battery is hammering — a slow `took` under
     * load is lock traffic, not a deaf bell. Waking WITH the message
     * (asserted above) already rules out the deadline's empty answer, so
     * the landing-to-wake gap is the entire timing claim.
     */
    let woke_after = landed.elapsed();
    assert!(
        woke_after < std::time::Duration::from_secs(u64::from(WAIT_SECONDS)) / 2,
        "the sleeper surfaced {woke_after:?} after a message posted the \
             instant it was asleep (alive {took:?} in all) — it is polling \
             the clock, not hearing the bell"
    );
}

/// And an inbox that stays empty answers the empty answer, not a refusal.
///
/// The ninth invariant, on the one road that can now sit still long enough
/// to break it. A `--wait` that timed out into `orchestration: …` would have
/// coordinators reading a checkpoint as a dead worker.
#[test]
fn a_wait_that_hears_nothing_says_nothing_rather_than_refusing() {
    const LEADER_TERM: u32 = 9_400;
    let seat = zerocode_core::agent_teams::LEADER_PANE;
    // The deadline is the constant; this test would take that long to run,
    // so it asks the one thing it can ask cheaply — that the FIRST look's
    // answer, which is what a timed-out wait hands back, is the empty
    // answer and not a refusal. That is the planner's own decision, so it
    // is asked of the planner directly, over a ledger and a team of this
    // test's own — no window, no actor, no shared anything.
    let mut held = Ledger::new();
    let mut table =
        zerocode_core::agent_teams::Team::new("team-quiet-wait", TEST_CAPABILITY, LEADER_TERM);
    let opened = zerocode_core::orchestration::plan(
        &mut held,
        &mut table,
        &Catalog::new(Vec::new()),
        &words("run-create --name silent --retry-request r-quiet"),
        seat,
        1_000,
        Some(&test_actor(LEADER_TERM)),
    );
    assert_eq!(opened.reply.exit_code, 0, "{}", opened.reply.stderr);
    let decided = zerocode_core::orchestration::plan(
        &mut held,
        &mut table,
        &Catalog::new(Vec::new()),
        &words("check --wait"),
        seat,
        2_000,
        Some(&test_actor(LEADER_TERM)),
    );
    assert_eq!(decided.reply.exit_code, 0, "{}", decided.reply.stderr);
    assert!(
        decided.reply.stdout.contains(r#""count":0"#),
        "a quiet inbox stopped answering quietly: {}",
        decided.reply.stdout
    );
    assert!(
        decided.waiting.is_some(),
        "the flag is advertised and still not read"
    );
    // And without the flag, nothing waits — the default road is untouched.
    let plain = zerocode_core::orchestration::plan(
        &mut held,
        &mut table,
        &Catalog::new(Vec::new()),
        &words("check"),
        seat,
        2_001,
        Some(&test_actor(LEADER_TERM)),
    );
    assert!(
        plain.waiting.is_none(),
        "a plain check now sleeps, which is every coordinator's loop \
             suddenly holding a thread"
    );
}

/// The disk is measured with one syscall, and an unmeasurable path is
/// unmeasured rather than empty.
#[cfg(unix)]
#[test]
fn free_bytes_are_measured_at_a_real_path_and_unknown_at_a_missing_one() {
    let here = super::free_bytes_at(Path::new("/")).expect("the root volume answers");
    assert!(here > 0, "a mounted root reported no room at all");
    assert_eq!(
        super::free_bytes_at(Path::new("/definitely/not/a/path/on/this/machine")),
        None,
        "a path that does not exist was answered as a number"
    );
}
/* ---- the crash that becomes a task (t-3014 §2.4) ---------------------- */

/// The incident the sheet showed goes down the `task-create` door from
/// the seated leader — once. Before a leader sits there is nothing to
/// present and nothing is stamped; after the task is filed the stamp
/// keeps a second beat, and a second boot, from filing it again.
#[test]
fn the_crash_task_goes_down_the_task_create_door_from_the_seated_leader_once() {
    const LEADER: u32 = 91_640;
    let (_window, _store) = PrivateWindow::boot();
    let host = Seating {
        onto: LEADER + 1,
        checkout: "/tmp",
    };
    let root = tempfile::tempdir().expect("a crash root");
    crate::crash::record(
        root.path(),
        crate::crash::Kind::Panic,
        "pane update failed",
        crate::crash::Origin {
            site: Some(("crates/zerocode-shell/src/pane.rs", 41)),
            thread: Some("main"),
            backtrace: None,
        },
    )
    .expect("the panic report");
    assert!(crate::crash::consume(root.path()).is_some());
    // No seated leader in this window yet: nothing presented, nothing stamped.
    let waiting = super::file_crash_task(&host, &[], root.path(), clock());
    assert!(matches!(waiting, super::Triage::Waiting(_)), "{waiting:?}");
    assert!(!root.path().join("crash/triaged.json").exists());

    let team = format!("team-crash-task-{LEADER}");
    seat_a_team(&team, LEADER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let opened = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("run-create --name crash-task"),
        clock(),
    );
    assert_eq!(opened.exit_code, 0, "{}", opened.stderr);
    let filed = super::file_crash_task(&host, &[], root.path(), clock());
    let super::Triage::Filed { task, .. } = filed else {
        panic!("{filed:?}");
    };
    let rows = the_rows();
    let row = rows
        .tasks
        .iter()
        .find(|one| one.id == task)
        .expect("the crash task row");
    assert!(
        row.spec.as_str().starts_with("panic: pane update failed\n"),
        "{}",
        row.spec.as_str()
    );
    assert!(
        row.spec
            .as_str()
            .contains("@ crates/zerocode-shell/src/pane.rs:41 [thread main]")
    );
    let stamp: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.path().join("crash/triaged.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(stamp["task"], task);
    let again = super::file_crash_task(&host, &[], root.path(), clock());
    assert!(matches!(again, super::Triage::Nothing), "{again:?}");
    assert_eq!(
        the_rows()
            .tasks
            .iter()
            .filter(|one| one.spec.as_str().starts_with("panic: pane update failed"))
            .count(),
        1
    );
}

/// A window whose ledger is degraded files nothing and leaves no stamp,
/// so the next boot asks again — and it says so rather than waiting.
#[test]
fn a_degraded_window_files_no_crash_task_and_leaves_no_stamp() {
    let root = tempfile::tempdir().expect("a crash root");
    crate::crash::record(
        root.path(),
        crate::crash::Kind::Hang,
        "main thread unresponsive for 6000ms; focus_lane",
        crate::crash::Origin::NONE,
    )
    .expect("the hang report");
    assert!(crate::crash::consume(root.path()).is_some());
    let host = Nowhere;
    let _degraded = Degraded::begin();
    let said = super::file_crash_task(&host, &[], root.path(), clock());
    assert!(matches!(said, super::Triage::Unavailable(_)), "{said:?}");
    assert!(!root.path().join("crash/triaged.json").exists());
}

struct CoordinatorHost(AtTheWall);
impl Host for CoordinatorHost {
    fn split(
        &self,
        _: &str,
        _: u32,
        _: u32,
        _: &str,
        _: zerocode_core::agent_teams::Direction,
        _: &str,
        _: &str,
    ) -> Option<u32> {
        panic!("seat transfer must not spawn")
    }
    fn send(&self, _: u32, _: &str) -> bool {
        true
    }
    fn capture(&self, _: u32) -> Option<String> {
        Some(String::new())
    }
    fn focus(&self, _: u32) -> bool {
        true
    }
    fn close(&self, _: u32) {
        panic!("seat transfer must not close panes")
    }
    fn pane_exists(&self, _: u32) -> bool {
        true
    }
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
    fn agent_of(&self, _: u32) -> Option<String> {
        Some("codex".into())
    }
    fn quota_wall_marker(
        &self,
        term: u32,
        agent: &str,
    ) -> Option<zerocode_core::orchestration::QuotaWallMarker> {
        self.0.quota_wall_marker(term, agent)
    }
}

#[test]
fn coordinator_handover_native_order_walks_once_and_persists_in_the_store() {
    use super::coordinator_handover as handover;
    let now = 5_000_000;
    let (window, _store) = PrivateWindow::boot_with_usage(vec![(
        "codex",
        usage_snapshot("codex", Some((98, Some(now + 600_000))), None, now - 1000),
    )]);
    let _beat = one_beat_at_a_time();
    seat_a_team("seat-source", 77001);
    seat_a_team("seat-target", 77002);
    let host = CoordinatorHost(AtTheWall {
        closed: Mutex::new(Vec::new()),
        busy: Mutex::new(false),
        onto: Mutex::new(0),
        checkout: "/unused",
        markers: Mutex::new(std::collections::HashMap::new()),
        asked: Mutex::new(Vec::new()),
        declines: Mutex::new(std::collections::HashMap::new()),
        pty_quiet_ms: Mutex::new(None),
        screens: Mutex::new(std::collections::HashMap::new()),
        fence_reading: Mutex::new(None),
    });
    let answer = run(
        &host,
        Vec::new(),
        "seat-source",
        "%1",
        TEST_CAPABILITY,
        &words("run-create --name seats"),
        now,
    );
    assert_eq!(answer.exit_code, 0, "{}", answer.stderr);
    let id = serde_json::from_str::<serde_json::Value>(&answer.stdout).unwrap()["runId"]
        .as_str()
        .unwrap()
        .to_owned();
    let status = handover::status(&host, &id).unwrap();
    assert!(
        status["eligiblePanes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|pane| pane["term"] == 77002)
    );
    let armed = handover::set_policy(&host, &id, 1, Some(77002), "human-order", now + 1).unwrap();
    assert_eq!(armed["policy"]["status"], "armed");
    assert!(
        !handover::walk(&host, now + 2),
        "numeric quota and quiet alone cannot transfer"
    );
    host.0
        .screen_says(77001, "screen", "You've hit your usage limit");
    let connection =
        rusqlite::Connection::open(window._root.path().join("authority/authority.sqlite")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_seat_transfer BEFORE UPDATE OF revision ON orchestration_ledger_heads BEGIN SELECT RAISE(FAIL, 'seat transfer test'); END;").unwrap();
    assert!(
        !handover::walk(&host, now + 3),
        "a failed store commit cannot transfer authority"
    );
    connection
        .execute_batch("DROP TRIGGER fail_seat_transfer;")
        .unwrap();
    assert_eq!(
        handover::status(&host, &id).unwrap()["coordinator"]["generation"],
        1
    );
    assert!(handover::walk(&host, now + 3));
    let transferred = handover::status(&host, &id).unwrap();
    assert_eq!(transferred["coordinator"]["seat"], "seat-target/%1");
    assert_eq!(transferred["coordinator"]["generation"], 2);
    assert_eq!(transferred["policy"]["status"], "completed");
    assert!(!handover::walk(&host, now + 4));
    assert_eq!(
        handover::set_policy(&host, &id, 1, Some(77002), "human-order", now + 5).unwrap()["coordinator"]
            ["generation"],
        2
    );
    assert!(handover::set_policy(&host, &id, 1, Some(77001), "stale", now + 6).is_err());
    let leaked = transferred.to_string();
    assert!(
        !leaked.contains(&test_actor(77002)),
        "public status exposed the target identity"
    );
    // SQLite round-trip through the existing coordinator JSON column.
    let image = runtime().unwrap().actor.view().unwrap();
    let saved = zerocode_orchestrator::ledger_store::read(
        &connection,
        "main-ledger",
        zerocode_core::orchestration::PROJECTION_SCHEMA,
    )
    .unwrap()
    .unwrap();
    let rebuilt = Ledger::rebuild(saved.projection).unwrap();
    assert_eq!(
        rebuilt
            .run(&id)
            .unwrap()
            .coordinator
            .as_ref()
            .unwrap()
            .generation,
        2
    );
    assert!(image.projection().runs.iter().any(|run| {
        run.coordinator
            .as_ref()
            .is_some_and(|seat| seat.generation == 2)
    }));
    drop(window);
}

#[test]
fn coordinator_handover_cli_cannot_declare_a_human_order() {
    let (_window, _store) = PrivateWindow::boot();
    let host = CoordinatorHost(AtTheWall {
        closed: Mutex::new(Vec::new()),
        busy: Mutex::new(false),
        onto: Mutex::new(0),
        checkout: "/unused",
        markers: Mutex::new(std::collections::HashMap::new()),
        asked: Mutex::new(Vec::new()),
        declines: Mutex::new(std::collections::HashMap::new()),
        pty_quiet_ms: Mutex::new(None),
        screens: Mutex::new(std::collections::HashMap::new()),
        fence_reading: Mutex::new(None),
    });
    let answer = run(
        &host,
        Vec::new(),
        "none",
        "%1",
        "unused",
        &words("coordinator-handover-policy --off"),
        1000,
    );
    assert_ne!(answer.exit_code, 0);
    assert!(answer.stderr.contains("human declaration"));
}
#[test]
fn coordinator_manual_native_picker_and_claim_are_durable_and_retryable() {
    use super::coordinator_handover as seats;
    let (window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    seat_a_team("manual-source", 78001);
    seat_a_team("manual-target", 78002);
    let host = CoordinatorHost(AtTheWall {
        closed: Mutex::new(Vec::new()),
        busy: Mutex::new(false),
        onto: Mutex::new(0),
        checkout: "/unused",
        markers: Mutex::new(std::collections::HashMap::new()),
        asked: Mutex::new(Vec::new()),
        declines: Mutex::new(std::collections::HashMap::new()),
        pty_quiet_ms: Mutex::new(None),
        screens: Mutex::new(std::collections::HashMap::new()),
        fence_reading: Mutex::new(None),
    });
    let made = run(
        &host,
        Vec::new(),
        "manual-source",
        "%1",
        TEST_CAPABILITY,
        &words("run-create --name manual-picker"),
        1000,
    );
    assert_eq!(made.exit_code, 0, "{}", made.stderr);
    let id = serde_json::from_str::<serde_json::Value>(&made.stdout).unwrap()["runId"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = runtime().unwrap().actor.view().unwrap();
    let list = seats::recent_runs(Some(1)).unwrap();
    assert_eq!(list["runs"][0]["runId"], id);
    assert_eq!(list["runs"][0]["generation"], 1);
    assert_eq!(list["truncated"], false);
    assert_eq!(
        runtime().unwrap().actor.view().unwrap().projection(),
        before.projection()
    );
    let connection =
        rusqlite::Connection::open(window._root.path().join("authority/authority.sqlite")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_manual_claim BEFORE UPDATE OF revision ON orchestration_ledger_heads BEGIN SELECT RAISE(FAIL, 'manual claim test'); END;").unwrap();
    assert!(seats::claim_seat(&host, &id, 78002, 1, "manual-click", 1001).is_err());
    connection
        .execute_batch("DROP TRIGGER reject_manual_claim;")
        .unwrap();
    assert_eq!(
        seats::status(&host, &id).unwrap()["coordinator"]["generation"],
        1
    );
    let claimed = seats::claim_seat(&host, &id, 78002, 1, "manual-click", 1002).unwrap();
    assert_eq!(claimed["generation"], 2);
    assert_eq!(claimed["moved"], true);
    assert_eq!(claimed["coordinator"]["seat"], "manual-target/%1");
    assert_eq!(
        seats::claim_seat(&host, &id, 78002, 1, "manual-click", 1003).unwrap(),
        claimed
    );
    let noop = seats::claim_seat(&host, &id, 78002, 2, "same-seat", 1004).unwrap();
    assert_eq!(noop["moved"], false);
    assert!(seats::claim_seat(&host, &id, 78002, 1, "stale-click", 1005).is_err());
    assert!(seats::claim_seat(&host, &id, 78002, 2, "", 1005).is_err());
    assert!(seats::claim_seat(&host, &id, 79999, 2, "unknown-pane", 1005).is_err());
    let saved = zerocode_orchestrator::ledger_store::read(
        &connection,
        "main-ledger",
        zerocode_core::orchestration::PROJECTION_SCHEMA,
    )
    .unwrap()
    .unwrap();
    let rebuilt = Ledger::rebuild(saved.projection).unwrap();
    assert_eq!(
        rebuilt
            .run(&id)
            .unwrap()
            .coordinator_live()
            .unwrap()
            .generation,
        2
    );
    assert_eq!(rebuilt.bound_run(&test_actor(78002)), Some(id.as_str()));
}

#[test]
fn coordinator_manual_native_claim_is_available_when_the_source_pane_is_gone() {
    use super::coordinator_handover as seats;
    let (_window, _store) = PrivateWindow::boot();
    seat_a_team("manual-gone-source", 78011);
    seat_a_team("manual-gone-target", 78012);
    let host = CoordinatorHost(AtTheWall {
        closed: Mutex::new(Vec::new()),
        busy: Mutex::new(false),
        onto: Mutex::new(0),
        checkout: "/unused",
        markers: Mutex::new(std::collections::HashMap::new()),
        asked: Mutex::new(Vec::new()),
        declines: Mutex::new(std::collections::HashMap::new()),
        pty_quiet_ms: Mutex::new(None),
        screens: Mutex::new(std::collections::HashMap::new()),
        fence_reading: Mutex::new(None),
    });
    let made = run(
        &host,
        Vec::new(),
        "manual-gone-source",
        "%1",
        TEST_CAPABILITY,
        &words("run-create --name manual-empty"),
        2000,
    );
    let id = serde_json::from_str::<serde_json::Value>(&made.stdout).unwrap()["runId"]
        .as_str()
        .unwrap()
        .to_owned();
    runtime()
        .unwrap()
        .actor
        .team_dissolved("manual-gone-source", 2001)
        .unwrap();
    let answer = seats::claim_seat(&host, &id, 78012, 1, "empty-seat", 2002).unwrap();
    assert_eq!(answer["generation"], 2);
    assert_eq!(answer["coordinator"]["seat"], "manual-gone-target/%1");
    let denied = run(
        &host,
        Vec::new(),
        "manual-gone-target",
        "%1",
        TEST_CAPABILITY,
        &words("coordinator-seat-claim --generation 2"),
        2003,
    );
    assert_ne!(denied.exit_code, 0);
    assert!(denied.stderr.contains("human declaration"));
}

#[test]
fn coordinator_handover_can_disable_after_source_capability_disappears() {
    use super::coordinator_handover as seats;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    seat_a_team("off-gone-source", 79011);
    seat_a_team("off-live-target", 79012);
    let host = CoordinatorHost(AtTheWall {
        closed: Mutex::new(Vec::new()),
        busy: Mutex::new(false),
        onto: Mutex::new(0),
        checkout: "/unused",
        markers: Mutex::new(std::collections::HashMap::new()),
        asked: Mutex::new(Vec::new()),
        declines: Mutex::new(std::collections::HashMap::new()),
        pty_quiet_ms: Mutex::new(None),
        screens: Mutex::new(std::collections::HashMap::new()),
        fence_reading: Mutex::new(None),
    });
    let made = run(
        &host,
        Vec::new(),
        "off-gone-source",
        "%1",
        TEST_CAPABILITY,
        &words("run-create --name off-gone"),
        2000,
    );
    let id = serde_json::from_str::<serde_json::Value>(&made.stdout).unwrap()["runId"]
        .as_str()
        .unwrap()
        .to_owned();
    seats::set_policy(&host, &id, 1, Some(79012), "arm-before-gone", 2001).unwrap();
    crate::agent_teams::teams()
        .get_mut("off-gone-source")
        .unwrap()
        .remove_pane("%1");
    let disabled = seats::set_policy(&host, &id, 1, None, "off-after-gone", 2002).unwrap();
    assert!(disabled["policy"].is_null());
    assert_eq!(disabled["coordinator"]["generation"], 1);
}

#[test]
fn an_open_task_is_found_by_the_words_its_title_carries_and_a_final_one_is_not() {
    let mut ledger = super::Ledger::new();
    let run = ledger.create_run("scoreboard", 1);
    let make = |ledger: &mut super::Ledger, title: &str, at: i64| {
        ledger
            .create_task(&run, "spec".into(), title.into(), Vec::new(), None, at)
            .expect("task")
    };
    let done = make(&mut ledger, "점수표 [scoreboard:disk] 9 G free", 2);
    ledger
        .update_task(
            &run,
            &done,
            Some(zerocode_core::orchestration::TaskStatus::Completed),
            None,
            zerocode_core::orchestration::ResultAuthor::Ledger,
        )
        .expect("completed");
    assert_eq!(
        super::open_task_titled_in(&ledger, "[scoreboard:disk]"),
        None,
        "a completed task carries nothing"
    );
    let open = make(&mut ledger, "점수표 [scoreboard:disk] 8 G free", 3);
    make(&mut ledger, "점수표 [scoreboard:disk-io] 1 G", 4);
    assert_eq!(
        super::open_task_titled_in(&ledger, "[scoreboard:disk]"),
        Some(open),
        "the closing bracket keeps disk from matching disk-io"
    );
}

#[test]
fn adopted_worker_stop_closes_its_own_team_with_or_without_a_caller_pane_collision() {
    for collision in [true, false] {
        let (_window, _store) = PrivateWindow::boot();
        let (run_id, worker, pane) = a_worker_carrying_work("stop-origin", 94_000, 94_001);
        let other_worker = if collision {
            let (_, other, other_pane) = a_worker_carrying_work("stop-caller", 94_010, 94_011);
            assert_eq!(other_pane, pane);
            Some(other)
        } else {
            seat_a_team("stop-caller", 94_010);
            None
        };
        let host = Splitting::onto(94_020);
        let takeover = run(
            &host,
            Vec::new(),
            "stop-caller",
            "%1",
            TEST_CAPABILITY,
            &words(&format!(
                "run-takeover --run {run_id} --from stop-origin/%1 --reason test"
            )),
            clock(),
        );
        assert_eq!(takeover.exit_code, 0, "{}", takeover.stderr);
        let argv = words(&format!(
            "worker-stop --worker {worker} --retry-request stop-adopted"
        ));
        let stopped = run(
            &host,
            Vec::new(),
            "stop-caller",
            "%1",
            TEST_CAPABILITY,
            &argv,
            clock(),
        );
        assert_eq!(stopped.exit_code, 0, "{}", stopped.stderr);
        assert_eq!(host.closed(), vec![94_001]);
        let rows = the_rows();
        assert_eq!(
            rows.workers
                .iter()
                .find(|row| row.id == worker)
                .unwrap()
                .state,
            WorkerState::Released
        );
        assert!(
            rows.dispatches
                .iter()
                .filter(|row| row.worker == worker)
                .all(|row| row.ended_ms.is_some())
        );
        if let Some(other) = other_worker {
            assert_eq!(
                rows.workers
                    .iter()
                    .find(|row| row.id == other)
                    .unwrap()
                    .state,
                WorkerState::Active
            );
            assert_eq!(
                crate::agent_teams::teams()
                    .get("stop-caller")
                    .unwrap()
                    .term_of(&pane),
                Some(94_011)
            );
        }
        let replay = run(
            &host,
            Vec::new(),
            "stop-caller",
            "%1",
            TEST_CAPABILITY,
            &argv,
            clock(),
        );
        assert_eq!(replay.stdout, stopped.stdout);
        assert_eq!(
            host.closed(),
            vec![94_001],
            "retry closed the predecessor twice"
        );
    }
}

#[test]
fn adopted_worker_release_archives_the_foreign_terminal_and_preserves_the_collision() {
    let (_window, _store) = PrivateWindow::boot();
    let (run_id, worker, pane) = a_worker_in_a_pane("release-origin", 94_100, 94_101);
    let (_, other, other_pane) = a_worker_carrying_work("release-caller", 94_110, 94_111);
    assert_eq!(other_pane, pane);
    let host = Releasing::quiet();
    let taken = run(
        &host,
        Vec::new(),
        "release-caller",
        "%1",
        TEST_CAPABILITY,
        &words(&format!(
            "run-takeover --run {run_id} --from release-origin/%1 --reason test"
        )),
        clock(),
    );
    assert_eq!(taken.exit_code, 0, "{}", taken.stderr);
    let released = run(
        &host,
        Vec::new(),
        "release-caller",
        "%1",
        TEST_CAPABILITY,
        &words(&format!("worker-release --worker {worker}")),
        clock(),
    );
    assert_eq!(released.exit_code, 0, "{}", released.stderr);
    let answer: serde_json::Value = serde_json::from_str(&released.stdout).unwrap();
    assert_eq!(answer["archived"], true);
    assert_eq!(answer["state"], "released");
    assert_eq!(*host.captured.lock().unwrap(), vec![94_101]);
    assert_eq!(host.closed(), vec![94_101]);
    let rows = the_rows();
    let released = rows.workers.iter().find(|row| row.id == worker).unwrap();
    assert_eq!(released.archive.as_deref(), Some("what the worker printed"));
    assert_eq!(
        rows.workers
            .iter()
            .find(|row| row.id == other)
            .unwrap()
            .state,
        WorkerState::Active
    );
    assert_eq!(
        crate::agent_teams::teams()
            .get("release-caller")
            .unwrap()
            .term_of(&pane),
        Some(94_111)
    );
}

#[test]
fn stop_rejects_respawn_and_human_takeover_between_plan_and_effect_without_settling() {
    for change in ["term", "capability", "taken"] {
        let (_window, _store) = PrivateWindow::boot();
        let (run_id, worker, pane) = a_worker_carrying_work("stop-fenced", 94_200, 94_201);
        let held = super::runtime().unwrap();
        let actor = &held.actor;
        let command = PlanCommand::checked(
            words(&format!(
                "worker-stop --run {run_id} --worker {worker} --retry-request stop-fence"
            )),
            "stop-fenced",
            "%1",
            TEST_CAPABILITY,
            Some(test_actor(94_200)),
            clock(),
        )
        .unwrap();
        let (decided, _) = actor.plan(command).unwrap();
        assert_eq!(decided.reply.exit_code, 0);
        match change {
            "term" => crate::agent_teams::teams()
                .get_mut("stop-fenced")
                .unwrap()
                .respawn_pane(&pane, 94_202),
            "capability" => {
                let _ = crate::agent_teams::remember_pane_token(
                    "stop-fenced",
                    &pane,
                    "replacement-token".into(),
                );
            }
            "taken" => {
                actor.pane_taken_over(94_201, clock()).unwrap();
            }
            _ => unreachable!(),
        }
        let host = Splitting::onto(94_203);
        let refused = carried(
            &host,
            actor,
            *decided,
            "stop-fenced",
            "%1",
            TEST_CAPABILITY,
            clock(),
        );
        assert_ne!(refused.exit_code, 0, "{change}");
        assert!(host.closed().is_empty(), "{change}");
        let rows = the_rows();
        assert_eq!(
            rows.workers
                .iter()
                .find(|row| row.id == worker)
                .unwrap()
                .state,
            WorkerState::Active,
            "{change}"
        );
        assert!(
            rows.dispatches
                .iter()
                .filter(|row| row.worker == worker)
                .all(|row| row.ended_ms.is_none()),
            "{change}"
        );
    }
}

#[test]
fn retained_quota_mail_does_not_replace_a_recovered_worker_when_policy_is_enabled_later() {
    for recovery in ["activity", "marker", "headroom", "reset", "stale"] {
        let stood = Walled::stand(94_300, "/tmp", "");
        stood.wall_it();
        let historical =
            stood.json("check --all --types quota_walled", stood.began + 11_000)["messages"]
                .clone();
        match recovery {
            "activity" => {
                *stood.host.busy.lock().unwrap() = true;
            }
            "marker" => stood.host.markers.lock().unwrap().clear(),
            "headroom" => stood.window.set_usage(vec![(
                "codex",
                usage_snapshot("codex", Some((20, None)), None, stood.began + 12_000),
            )]),
            "reset" => stood.window.set_usage(vec![(
                "codex",
                usage_snapshot(
                    "codex",
                    Some((98, Some(stood.began + 12_000))),
                    None,
                    stood.began + 12_000,
                ),
            )]),
            "stale" => stood.window.set_usage(vec![(
                "codex",
                usage_snapshot(
                    "codex",
                    Some((98, None)),
                    None,
                    stood.began
                        - zerocode_core::orchestration::QUOTA_POLICY.snapshot_max_age_ms
                        - 1,
                ),
            )]),
            _ => unreachable!(),
        }
        stood.json(
            "handover-policy --on-quota-wall claude --wip-commit",
            stood.began + 13_000,
        );
        let plan = stood.plan();
        let door = Recording::through(&stood.host, Ok(Some("must-not-commit".into())));
        let held = super::runtime().unwrap();
        assert_eq!(
            walk_handover(
                &held.actor,
                &door,
                &plan,
                TEST_CAPABILITY,
                stood.began + 20_000
            ),
            Walked::Nothing,
            "{recovery}"
        );
        assert!(door.committed.lock().unwrap().is_empty(), "{recovery}");
        assert!(door.argv().is_empty(), "{recovery}");
        assert_eq!(stood.worker_state(&stood.worker), "Active");
        assert_eq!(
            stood.json("check --all --types quota_walled", stood.began + 20_001)["messages"],
            historical
        );
        assert!(stood.receipts(stood.began + 20_001).is_empty());
    }
}

#[test]
fn a_current_wall_after_recovery_can_use_the_order_without_rewriting_historical_mail() {
    let stood = Walled::stand(94_400, "/tmp", "--on-quota-wall claude");
    stood.wall_it();
    let history =
        stood.json("check --all --types quota_walled", stood.began + 11_000)["messages"].clone();
    stood.host.markers.lock().unwrap().clear();
    let plan = stood.plan();
    let held = super::runtime().unwrap();
    let door = Recording::through(&stood.host, Ok(None));
    assert_eq!(
        walk_handover(
            &held.actor,
            &door,
            &plan,
            TEST_CAPABILITY,
            stood.began + 12_000
        ),
        Walked::Nothing
    );
    stood
        .host
        .screen_says(94_401, "rollout", "usage_limit_exceeded");
    assert!(matches!(
        walk_handover(
            &held.actor,
            &door,
            &plan,
            TEST_CAPABILITY,
            stood.began + 13_000
        ),
        Walked::Done { .. }
    ));
    assert_eq!(*stood.host.closed.lock().unwrap(), vec![94_401]);
    assert_eq!(
        stood.json("check --all --types quota_walled", stood.began + 14_000)["messages"],
        history
    );
}

#[test]
fn a_superseded_policy_cannot_commit_or_stop_from_an_old_reservation() {
    let stood = Walled::stand(
        94_500,
        "/tmp",
        "--on-quota-wall claude:fable-5-1 --wip-commit",
    );
    stood.wall_it();
    let plan = stood.plan();
    let door = Recording::through(&stood.host, Ok(Some("must-not-commit".into()))).change_on_wall(
        2,
        || {
            stood.json(
                "handover-policy --on-quota-wall claude",
                stood.began + 20_000,
            );
        },
    );
    let held = super::runtime().unwrap();
    assert!(
        matches!(walk_handover(&held.actor, &door, &plan, TEST_CAPABILITY, stood.began + 20_000), Walked::Failed { at, .. } if at == "wip-commit")
    );
    assert!(door.committed.lock().unwrap().is_empty());
    assert!(door.argv().is_empty());
    assert_eq!(stood.worker_state(&stood.worker), "Active");
    assert_eq!(stood.receipts(stood.began + 20_001)[0]["status"], "revoked");
    let fresh = stood.plan();
    assert!(!fresh.wip_commit);
    assert_eq!(fresh.to.model, None);
    let current = Recording::through(&stood.host, Ok(None));
    assert!(matches!(
        walk_handover(
            &held.actor,
            &current,
            &fresh,
            TEST_CAPABILITY,
            stood.began + 21_000
        ),
        Walked::Done { .. }
    ));
}

#[test]
fn takeover_or_recovery_before_stop_revokes_the_walk_and_keeps_the_attempt_open() {
    for change in ["human", "recovery", "policy", "coordinator"] {
        let stood = Walled::stand(94_600, "/tmp", "--on-quota-wall claude --wip-commit");
        stood.wall_it();
        let plan = stood.plan();
        let held = super::runtime().unwrap();
        let door = Recording::through(&stood.host, Ok(None)).change_on_wall(3, || match change {
            "human" => {
                held.actor
                    .pane_taken_over(94_601, stood.began + 20_000)
                    .unwrap();
            }
            "recovery" => {
                *stood.host.busy.lock().unwrap() = true;
            }
            "policy" => {
                stood.json("handover-policy --off", stood.began + 20_000);
            }
            "coordinator" => {
                seat_a_team("handover-next-seat", 94_610);
                let moved = run(
                    &stood.host,
                    Vec::new(),
                    "handover-next-seat",
                    "%1",
                    TEST_CAPABILITY,
                    &words(&format!(
                        "run-takeover --run {} --from {}/%1 --reason test",
                        plan.run, stood.team
                    )),
                    stood.began + 20_000,
                );
                assert_eq!(moved.exit_code, 0, "{}", moved.stderr);
            }
            _ => unreachable!(),
        });
        assert!(
            matches!(walk_handover(&held.actor, &door, &plan, TEST_CAPABILITY, stood.began + 20_000), Walked::Failed { at, .. } if at == "worker-stop"),
            "{change}"
        );
        assert_eq!(door.committed.lock().unwrap().len(), 1);
        assert!(door.argv().is_empty(), "{change}");
        let rows = the_rows();
        assert!(
            rows.dispatches
                .iter()
                .find(|row| row.id == stood.dispatch)
                .unwrap()
                .ended_ms
                .is_none(),
            "{change}"
        );
        assert_eq!(stood.worker_state(&stood.worker), "Active");
        assert!(stood.host.closed.lock().unwrap().is_empty(), "{change}");
    }
}

#[test]
fn actor_rejects_a_superseded_handover_after_the_shell_checked_it() {
    let stood = Walled::stand(94_700, "/tmp", "--on-quota-wall claude --wip-commit");
    stood.wall_it();
    let plan = stood.plan();
    let wall = current_handover_wall(&stood.host, &plan, stood.began + 20_000).unwrap();
    let command = PlanCommand::checked(
        words(&format!(
            "worker-stop --worker {} --retry-request delayed-handover-stop",
            stood.worker
        )),
        &stood.team,
        "%1",
        TEST_CAPABILITY,
        Some(test_actor(94_700)),
        stood.began + 20_000,
    )
    .unwrap()
    .for_handover(
        plan.clone(),
        zerocode_core::agent_teams::PaneIncarnation {
            term: wall.term,
            capability: wall.capability,
        },
    )
    .unwrap();
    stood.json("handover-policy --off", stood.began + 20_001);
    let held = super::runtime().unwrap();
    assert!(matches!(
        held.actor.plan(command),
        Err(RuntimeError::AuthorityRejected)
    ));
    assert_eq!(stood.worker_state(&stood.worker), "Active");
}

#[test]
fn a_refused_handover_reservation_write_performs_no_external_step() {
    let stood = Walled::stand(94_800, "/tmp", "--on-quota-wall claude --wip-commit");
    stood.wall_it();
    let plan = stood.plan();
    {
        let connection = stood._store.fault_connection_for_tests().unwrap();
        connection.execute_batch("CREATE TRIGGER refuse_handover BEFORE UPDATE OF revision ON orchestration_ledger_heads BEGIN SELECT RAISE(FAIL, 'handover reservation test'); END;").unwrap();
    }
    let door = Recording::through(&stood.host, Ok(Some("must-not-commit".into())));
    let held = super::runtime().unwrap();
    assert_eq!(
        walk_handover(
            &held.actor,
            &door,
            &plan,
            TEST_CAPABILITY,
            stood.began + 20_000
        ),
        Walked::Nothing
    );
    assert!(door.committed.lock().unwrap().is_empty());
    assert!(door.argv().is_empty());
    assert_eq!(stood.worker_state(&stood.worker), "Active");
}

#[test]
fn a_handover_split_rechecks_the_order_after_planning_and_rolls_back_its_reservation() {
    for change in ["off", "alternative", "coordinator", "current"] {
        let stood = Walled::stand(94_900, "/tmp", "--on-quota-wall claude");
        stood.wall_it();
        let plan = stood.plan();
        let wall = current_handover_wall(&stood.host, &plan, clock()).unwrap();
        stood.json(&format!("worker-stop --worker {}", stood.worker), clock());
        let held = super::runtime().unwrap();
        let command = PlanCommand::checked(
            words(&format!("worker-start --agent claude --task {} --retry-of {} --inherit-checkout --retry-request delayed-start",
                stood.task, stood.dispatch)),
            &stood.team, "%1", TEST_CAPABILITY, Some(test_actor(94_900)), clock(),
        ).unwrap().for_handover(plan.clone(), zerocode_core::agent_teams::PaneIncarnation {
            term: wall.term, capability: wall.capability,
        }).unwrap();
        let (decided, _) = held.actor.plan(command).unwrap();
        assert_eq!(decided.reply.exit_code, 0, "{}", decided.reply.stderr);
        let prepared = decided.prepared_worker_start.clone().unwrap();
        assert_eq!(prepared.handover.as_deref(), Some(&plan));
        match change {
            "off" => {
                stood.json("handover-policy --off", clock());
            }
            "alternative" => {
                stood.json("handover-policy --on-quota-wall claude:fable-5-1", clock());
            }
            "coordinator" => {
                seat_a_team("delayed-start-coordinator", 94_910);
                let moved = run(
                    &stood.host,
                    Vec::new(),
                    "delayed-start-coordinator",
                    "%1",
                    TEST_CAPABILITY,
                    &words(&format!(
                        "run-takeover --run {} --from {}/%1 --reason test",
                        plan.run, stood.team
                    )),
                    clock(),
                );
                assert_eq!(moved.exit_code, 0, "{}", moved.stderr);
            }
            "current" => {}
            _ => unreachable!(),
        }
        let host = Splitting::onto(94_902);
        let answer = carried(
            &host,
            &held.actor,
            *decided,
            &stood.team,
            "%1",
            TEST_CAPABILITY,
            clock(),
        );
        let rows = the_rows();
        if change == "current" {
            assert_eq!(answer.exit_code, 0, "{}", answer.stderr);
            assert_eq!(host.spawned.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert!(rows.workers.iter().any(|row| row.id == prepared.worker));
        } else {
            assert_ne!(answer.exit_code, 0, "{change}");
            assert_eq!(
                host.spawned.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "{change}"
            );
            assert!(
                !rows.workers.iter().any(|row| row.id == prepared.worker),
                "{change}"
            );
            assert!(
                !rows
                    .dispatches
                    .iter()
                    .any(|row| Some(&row.id) == prepared.dispatch.as_ref()),
                "{change}"
            );
            assert_eq!(
                rows.tasks
                    .iter()
                    .find(|row| row.id == stood.task)
                    .unwrap()
                    .status,
                zerocode_core::orchestration::TaskStatus::Ready,
                "{change}"
            );
        }
        // A later split can reserve immediately: no cancelled operation remains open.
        let request = EffectRequest::new(
            "main-ledger",
            "a1".repeat(32),
            "a2".repeat(32),
            "a3".repeat(32),
            super::host_epoch().unwrap(),
            HostEffectKind::Split,
        )
        .unwrap();
        let (begun, _) = held.actor.prepare_effect(request, clock()).unwrap();
        let BeginEffect::Execute(permit) = begun else {
            panic!("journal stayed fenced: {begun:?}");
        };
        super::settle_permit_unstarted(&held.actor, permit, HostEffectFailure::Refused, clock());
    }
}

#[test]
fn an_early_release_refusal_settles_unknown_and_replays_without_reading_or_closing() {
    for change in ["caller", "missing-incarnation"] {
        let (_window, _store) = PrivateWindow::boot();
        let (run_id, worker, pane) = a_worker_in_a_pane("early-release-origin", 95_000, 95_001);
        seat_a_team("early-release-caller", 95_010);
        let host = Releasing::quiet();
        let taken = run(
            &host,
            Vec::new(),
            "early-release-caller",
            "%1",
            TEST_CAPABILITY,
            &words(&format!(
                "run-takeover --run {run_id} --from early-release-origin/%1 --reason test"
            )),
            clock(),
        );
        assert_eq!(taken.exit_code, 0, "{}", taken.stderr);
        if change == "missing-incarnation" {
            crate::agent_teams::teams()
                .get_mut("early-release-origin")
                .unwrap()
                .remove_pane(&pane);
        }
        let held = super::runtime().unwrap();
        let argv = words(&format!(
            "worker-release --worker {worker} --retry-request early-release"
        ));
        let command = |token: &str| {
            PlanCommand::checked(
                argv.clone(),
                "early-release-caller",
                "%1",
                token,
                Some(test_actor(95_010)),
                clock(),
            )
            .unwrap()
        };
        let (decided, _) = held.actor.plan(command(TEST_CAPABILITY)).unwrap();
        assert_eq!(decided.reply.exit_code, 0, "{}", decided.reply.stderr);
        let current_token = if change == "caller" {
            let _ = crate::agent_teams::remember_pane_token(
                "early-release-caller",
                "%1",
                "new-caller".into(),
            );
            "new-caller"
        } else {
            TEST_CAPABILITY
        };
        let answer = carried(
            &host,
            &held.actor,
            *decided,
            "early-release-caller",
            "%1",
            TEST_CAPABILITY,
            clock(),
        );
        assert_eq!(answer.exit_code, 0, "{}", answer.stderr);
        let json: serde_json::Value = serde_json::from_str(&answer.stdout).unwrap();
        assert_eq!(json["state"], "release_unknown");
        assert_eq!(json["archived"], false);
        assert!(host.captured.lock().unwrap().is_empty());
        assert!(host.closed().is_empty());
        let (replayed, _) = held.actor.plan(command(current_token)).unwrap();
        assert_eq!(replayed.reply.stdout, answer.stdout);
        assert!(matches!(replayed.effect, Effect::None));
        assert_eq!(
            the_rows()
                .workers
                .iter()
                .find(|row| row.id == worker)
                .unwrap()
                .state,
            WorkerState::ReleaseUnknown
        );
        if change == "caller" {
            assert_eq!(
                crate::agent_teams::teams()
                    .get("early-release-origin")
                    .unwrap()
                    .term_of(&pane),
                Some(95_001)
            );
        }
    }
}

fn relation_test_message(
    from: &str,
    to: &str,
    kind: zerocode_core::orchestration::MessageKind,
    body: &str,
    at: i64,
) -> zerocode_core::orchestration::Message {
    use zerocode_core::orchestration::{Message, Priority, Text};
    Message {
        id: format!("m-test-{at}"),
        from: from.to_owned(),
        to: to.to_owned(),
        kind,
        body: body.into(),
        subject: Text::default(),
        priority: Priority::Normal,
        payload: Text::default(),
        thread: None,
        task: None,
        dispatch: None,
        author_seat: None,
        created_ms: at,
    }
}

#[test]
fn relation_mail_maps_worker_run_and_pane_addresses_with_real_evidence() {
    use zerocode_core::orchestration::{Ledger, MessageKind, worker_address};
    let mut ledger = Ledger::new();
    let run = ledger.create_run("relations", 1);
    ledger.seat_coordinator(&run, "team/%1", None, 2).unwrap();
    let worker = ledger
        .start_worker(&run, "codex", ("team", "%2"), None, 3)
        .unwrap()
        .worker;
    let from = worker_address(&worker);
    let to = format!("run:{run}");
    let id = ledger
        .send(
            &run,
            relation_test_message(&from, &to, MessageKind::Status, "actual evidence", 10),
        )
        .unwrap();
    ledger
        .send(
            &run,
            relation_test_message("pane:team/%1", &from, MessageKind::Status, "reply", 11),
        )
        .unwrap();
    let seats = std::collections::HashMap::from([(
        "team".to_string(),
        std::collections::HashMap::from([("%1".to_string(), 101), ("%2".to_string(), 102)]),
    )]);
    let graph = super::graph_overlay_snapshot_for_seats(&ledger, &seats);
    let received = graph
        .mail
        .iter()
        .find(|edge| edge.to == "term:101")
        .unwrap();
    assert_eq!(received.from, "term:102");
    assert_eq!((received.count, received.unread), (1, 1));
    let evidence = received.last_message.as_ref().unwrap();
    assert_eq!(
        (&evidence.id, &evidence.run, &evidence.from, &evidence.to),
        (&id, &run, &from, &to)
    );
    assert_eq!(evidence.created_ms, 10);
    assert!(
        graph
            .mail
            .iter()
            .any(|edge| edge.from == "term:101" && edge.to == "term:102")
    );
}

#[test]
fn relation_dependencies_keep_completed_task_facts_after_its_latest_worker_is_released() {
    use zerocode_core::orchestration::{Ledger, MessageKind, TaskStatus, worker_address};
    let mut ledger = Ledger::new();
    let run = ledger.create_run("dependency evidence", 1);
    let upstream = ledger
        .create_task(&run, "build".into(), "build".into(), vec![], None, 2)
        .unwrap();
    let first = ledger
        .start_worker(&run, "codex", ("team", "%2"), Some(&upstream), 3)
        .unwrap()
        .worker;
    let address = format!("run:{run}");
    ledger
        .send(
            &run,
            zerocode_core::orchestration::Message {
                task: Some(upstream.clone()),
                dispatch: ledger
                    .run(&run)
                    .unwrap()
                    .worker(&first)
                    .unwrap()
                    .dispatch
                    .clone(),
                ..relation_test_message(
                    &worker_address(&first),
                    &address,
                    MessageKind::WorkerDone,
                    "{\"ok\":true}",
                    4,
                )
            },
        )
        .unwrap();
    ledger
        .update_task(
            &run,
            &upstream,
            Some(TaskStatus::Ready),
            None,
            zerocode_core::orchestration::ResultAuthor::Ledger,
        )
        .unwrap();
    let latest = ledger
        .start_worker(&run, "claude", ("team", "%3"), Some(&upstream), 5)
        .unwrap()
        .worker;
    ledger
        .send(
            &run,
            zerocode_core::orchestration::Message {
                task: Some(upstream.clone()),
                dispatch: ledger
                    .run(&run)
                    .unwrap()
                    .worker(&latest)
                    .unwrap()
                    .dispatch
                    .clone(),
                ..relation_test_message(
                    &worker_address(&latest),
                    &address,
                    MessageKind::WorkerDone,
                    "{\"ok\":true}",
                    6,
                )
            },
        )
        .unwrap();
    let downstream = ledger
        .create_task(
            &run,
            "verify".into(),
            "verify".into(),
            vec![upstream.clone()],
            None,
            7,
        )
        .unwrap();
    let verifier = ledger
        .start_worker(&run, "codex", ("team", "%4"), Some(&downstream), 8)
        .unwrap()
        .worker;
    ledger.begin_release(&latest).unwrap();
    ledger.finish_release(&latest, None);
    let seats = super::TeamSeatIndex::new();
    let graph = super::graph_overlay_snapshot_for_seats(&ledger, &seats);
    assert!(
        graph.dependencies.is_empty(),
        "the old retained attempt must not impersonate the released latest attempt"
    );
    assert_eq!(graph.task_dependencies.len(), 1);
    let fact = &graph.task_dependencies[0];
    assert_eq!(fact.dependency, upstream);
    assert_eq!(fact.task, downstream);
    assert_eq!(fact.dependency_state, Some("completed"));
    assert_eq!(fact.task_state, "dispatched");
    assert_eq!(
        fact.task_created_ms, 7,
        "creation time is not completion time"
    );
    assert_eq!(fact.from, None);
    assert_eq!(fact.to, format!("worker:{verifier}"));
    let row = super::ledger_agents_for_seats(&ledger, &seats)
        .into_iter()
        .find(|row| row.worker == verifier)
        .unwrap();
    assert!(!row.dispatch_id.is_empty());
    assert_eq!(
        (row.task_id.as_str(), row.dispatch_started_ms, row.reported),
        (downstream.as_str(), 8, false)
    );
}

/* ---- t-4048 Fable 적대 검증 재현 ------------------------------------------
 *
 * 워커가 `worker_done` 을 보내면 `close_dispatch` 가 `worker.dispatch = None`
 * 으로 고리를 푼다(core orchestration.rs `close_dispatch`). 그 뒤
 * `ledger_agents_for_seats` 는 `worker.dispatch` 만 읽으므로 아직 소환 중인
 * (Reclaimable) 워커의 행에서 task_id·dispatch_id·retry_of 가 비고
 * `reported` 는 false 로 떨어진다 — 보드는 끝난 시도를 제 과업 아래 묶지도,
 * 이전 시도와 현재 시도를 가르지도 못한다. `ledger_states_for_seats` 는 같은
 * 이유로 run.dispatches 에서 워커의 최신 디스패치를 다시 찾는다. */
#[test]
fn relation_rows_keep_a_closed_attempts_identity_while_its_worker_is_still_summoned() {
    use zerocode_core::orchestration::{Ledger, MessageKind, worker_address};
    let mut ledger = Ledger::new();
    let run = ledger.create_run("closed attempt", 1);
    let task = ledger
        .create_task(&run, "build".into(), "build".into(), vec![], None, 2)
        .unwrap();
    let worker = ledger
        .start_worker(&run, "codex", ("team", "%2"), Some(&task), 3)
        .unwrap()
        .worker;
    let seats = super::TeamSeatIndex::new();
    let before = super::ledger_agents_for_seats(&ledger, &seats)
        .into_iter()
        .find(|row| row.worker == worker)
        .unwrap();
    assert_eq!(
        (before.task_id.as_str(), before.reported),
        (task.as_str(), false)
    );
    let dispatch_id = before.dispatch_id.clone();
    assert!(!dispatch_id.is_empty());
    // `relation_test_message` stamps no dispatch; `close_dispatch` only runs
    // for a `worker_done` that names one, as the bridge does for a worker.
    ledger
        .send(
            &run,
            zerocode_core::orchestration::Message {
                dispatch: Some(dispatch_id.clone()),
                task: Some(task.clone()),
                ..relation_test_message(
                    &worker_address(&worker),
                    &format!("run:{run}"),
                    MessageKind::WorkerDone,
                    "{\"ok\":true}",
                    4,
                )
            },
        )
        .unwrap();
    assert!(
        ledger
            .run(&run)
            .unwrap()
            .worker(&worker)
            .unwrap()
            .dispatch
            .is_none(),
        "the fixture must actually close the attempt"
    );
    let after = super::ledger_agents_for_seats(&ledger, &seats)
        .into_iter()
        .find(|row| row.worker == worker)
        .expect("a reclaimable worker is still summoned and still drawn");
    assert_eq!(
        (
            after.task_id.as_str(),
            after.dispatch_id.as_str(),
            after.reported
        ),
        (task.as_str(), dispatch_id.as_str(), true),
        "an ended attempt must keep naming its task and dispatch: without them the \
         board cannot group the finished worker under its task, nor tell a previous \
         attempt from the current one"
    );
}

/* ---- worktree-evidence, through the door an agent uses ----------------- */

/// A host that knows where it put one pane and does nothing else.
///
/// The evidence verb asks the window one question — which checkout is this
/// terminal sitting in — and the answer is the window's placement record, so
/// this is the whole of what a test host has to stand in for.
struct PlacedIn {
    term: u32,
    checkout: std::path::PathBuf,
}

impl Host for PlacedIn {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        _token: &str,
    ) -> Option<u32> {
        None
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        false
    }
    fn capture(&self, _term: u32) -> Option<String> {
        None
    }
    fn focus(&self, _term: u32) -> bool {
        false
    }
    fn close(&self, _term: u32) {}
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
    fn worktree_of(&self, term: u32) -> Option<std::path::PathBuf> {
        (term == self.term).then(|| self.checkout.clone())
    }
}

fn evidence_checkout() -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("a checkout");
    let git = |args: &[&str]| {
        let output = crate::proc::quiet_command(zerocode_orchestrator::GIT_EXECUTABLE)
            .arg("-C")
            .arg(root.path())
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}");
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "ZeroCode Test"]);
    git(&["config", "user.email", "test@zerocode.local"]);
    std::fs::write(root.path().join("tracked.txt"), "one\n").expect("seed");
    git(&["add", "tracked.txt"]);
    git(&["commit", "-m", "seed"]);
    std::fs::write(root.path().join("tracked.txt"), "two\n").expect("an edit");
    root
}

/// The bare verb reads the tree the asking pane sits in, through the same
/// capability check and actor plan every verb takes, and answers the
/// versioned schema without a local path in it.
#[test]
fn worktree_evidence_through_the_door_reads_the_callers_own_checkout() {
    const TEAM: &str = "team-evidence";
    const TERM: u32 = 81_001;
    let _window = the_window();
    seat_a_team(TEAM, TERM);
    let checkout = evidence_checkout();
    let host = PlacedIn {
        term: TERM,
        checkout: checkout.path().to_path_buf(),
    };
    let answered = run(
        &host,
        Vec::new(),
        TEAM,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words("worktree-evidence"),
        clock(),
    );
    assert_eq!(answered.exit_code, 0, "{}", answered.stderr);
    let evidence: serde_json::Value =
        serde_json::from_str(&answered.stdout).expect("one JSON answer");
    assert_eq!(evidence["schemaVersion"], 1);
    assert_eq!(evidence["snapshot"]["state"], "ok");
    assert_eq!(evidence["snapshot"]["data"]["dirty"], true);
    assert_eq!(evidence["summary"]["changedPaths"], 1);
    assert_eq!(evidence["ci"]["state"], "unsupported");
    assert!(
        evidence["repositoryId"]
            .as_str()
            .is_some_and(|id| id.starts_with("repo-")),
        "{evidence}"
    );
    let private = checkout.path().to_string_lossy();
    assert!(
        !answered.stdout.contains(private.as_ref()),
        "the answer carried the checkout's path: {}",
        answered.stdout
    );
}

/// Every refusal of the verb is `{code, message, retryable}` — the ones the
/// window writes, the ones the actor writes, and the ones the plan writes.
#[test]
fn worktree_evidence_refusals_are_structured_on_every_road() {
    const TEAM: &str = "team-evidence-refusals";
    const TERM: u32 = 81_101;
    let _window = the_window();
    seat_a_team(TEAM, TERM);
    let structured = |answered: &zerocode_hookd::TeamAnswer| {
        assert_ne!(answered.exit_code, 0, "{}", answered.stdout);
        assert!(answered.stdout.is_empty(), "{}", answered.stdout);
        let value: serde_json::Value = serde_json::from_str(answered.stderr.trim_end())
            .unwrap_or_else(|_| panic!("not structured: {}", answered.stderr));
        let keys: Vec<&str> = value
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["code", "message", "retryable"], "{value}");
        value
    };

    // The window has no placement record for this pane.
    let unplaced = run(
        &Nowhere,
        Vec::new(),
        TEAM,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words("worktree-evidence"),
        clock(),
    );
    assert_eq!(structured(&unplaced)["code"], "checkout_unresolved");

    // The capability is wrong: the actor turns the seat away before any plan.
    let host = PlacedIn {
        term: TERM,
        checkout: std::env::temp_dir(),
    };
    let stranger = run(
        &host,
        Vec::new(),
        TEAM,
        zerocode_core::agent_teams::LEADER_PANE,
        "not-this-panes-capability",
        &words("worktree-evidence"),
        clock(),
    );
    let refused = structured(&stranger);
    assert_eq!(refused["code"], "refused");
    assert_eq!(refused["message"], "stale or unauthorized agent pane");

    // A retry name on a read is refused by the plan, in the same shape.
    let named = run(
        &host,
        Vec::new(),
        TEAM,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &[
            "worktree-evidence".to_string(),
            "--retry-request".to_string(),
            "r-1".to_string(),
        ],
        clock(),
    );
    assert_eq!(structured(&named)["code"], "refused");

    // A worker named in a run this pane is not bound to: the verb's own code.
    let unbound = run(
        &host,
        Vec::new(),
        TEAM,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words("worktree-evidence --worker w-404"),
        clock(),
    );
    assert_eq!(structured(&unbound)["code"], "run_unbound");

    // And every other verb keeps the sentence it has always printed.
    let other = run(
        &host,
        Vec::new(),
        TEAM,
        zerocode_core::agent_teams::LEADER_PANE,
        "not-this-panes-capability",
        &words("run-list"),
        clock(),
    );
    assert!(
        other.stderr.starts_with("orchestration: "),
        "{}",
        other.stderr
    );
}

/* ---- worktree-evidence: the window half's own rules ------------------- */

/// A pane with no placement record gets a refusal, never another tree.
#[test]
fn a_seat_the_window_never_placed_is_refused() {
    let refused = crate::worktree_evidence_runtime::for_seat(None, None, 1)
        .expect_err("no checkout, no answer");
    assert_eq!(
        refused,
        crate::worktree_evidence_runtime::EvidenceRefusal::UNRESOLVED_SEAT
    );
    assert!(!refused.retryable);
}

/// A checkout on another host is named and left alone. Git is not asked
/// about it, and the answer says `unsupported` rather than `empty`.
#[test]
fn a_remote_checkout_is_unsupported_and_not_looked_for_on_this_disk() {
    let read = crate::worktree_evidence_runtime::for_remote("edge-ssh:/srv/app", 7);
    assert_eq!(
        read.snapshot.state,
        zerocode_orchestrator::worktree_evidence::SourceState::Unsupported
    );
    assert_eq!(
        read.snapshot.freshness,
        zerocode_orchestrator::worktree_evidence::Freshness::Unobserved
    );
    assert!(read.snapshot.data.is_none());
    assert_eq!(
        read.ci.state,
        zerocode_orchestrator::worktree_evidence::SourceState::Unsupported
    );
    assert_eq!(read.observed_at_ms, 7);
    assert!(read.repository_id.is_none());
    // The opaque id is stable for the same key and says nothing about a
    // path on this machine.
    assert_eq!(
        read.workspace_id,
        crate::worktree_evidence_runtime::for_remote("edge-ssh:/srv/app", 9).workspace_id
    );
    assert!(!read.workspace_id.contains('/'));
    // The ledger's rows are keyed by local paths, so none of them can be
    // said to belong to a tree on another host — unsupported, not empty.
    assert_eq!(
        read.executions.state,
        zerocode_orchestrator::worktree_evidence::SourceState::Unsupported
    );
    assert_eq!(
        read.reports.state,
        zerocode_orchestrator::worktree_evidence::SourceState::Unsupported
    );
}

fn evidence_refusal_answer(stderr: &str) -> zerocode_hookd::TeamAnswer {
    zerocode_hookd::TeamAnswer {
        stdout: String::new(),
        stderr: stderr.to_string(),
        exit_code: 1,
    }
}

/// A refusal the verb already wrote in its own shape is handed out bare.
#[test]
fn a_typed_refusal_leaves_without_its_voice() {
    let typed =
        crate::worktree_evidence_runtime::structured_refusal(evidence_refusal_answer(&format!(
            "orchestration: {}\n",
            zerocode_core::orchestration::evidence_refusal(
                "unknown_worker",
                "unknown worker: w-9",
                false
            )
        )));
    let value: serde_json::Value =
        serde_json::from_str(typed.stderr.trim_end()).expect("one JSON object");
    assert_eq!(value["code"], "unknown_worker");
    assert_eq!(value["message"], "unknown worker: w-9");
    assert_eq!(value["retryable"], false);
    assert_eq!(value.as_object().map(serde_json::Map::len), Some(3));
    assert_eq!(typed.exit_code, 1);
}

/// A sentence written for every verb is carried whole under `refused`.
#[test]
fn an_untyped_refusal_is_carried_whole_and_never_parsed_for_a_code() {
    let wrapped = crate::worktree_evidence_runtime::structured_refusal(evidence_refusal_answer(
        "orchestration: stale or unauthorized agent pane\n",
    ));
    let value: serde_json::Value =
        serde_json::from_str(wrapped.stderr.trim_end()).expect("one JSON object");
    assert_eq!(value["code"], "refused");
    assert_eq!(value["message"], "stale or unauthorized agent pane");
    assert_eq!(value["retryable"], false);

    // A success passes through untouched.
    let fine = crate::worktree_evidence_runtime::structured_refusal(zerocode_hookd::TeamAnswer {
        stdout: "{}\n".to_string(),
        stderr: String::new(),
        exit_code: 0,
    });
    assert_eq!(
        (fine.stdout.as_str(), fine.stderr.as_str(), fine.exit_code),
        ("{}\n", "", 0)
    );
}

/// The grace seats before it kills.
///
/// `expire_sleepers` announced every sleeper dead once the grace ran out, and
/// nothing on that road had ever called `reseat_sleeping` — only a coordinator
/// tab mounting in the renderer did. So a window could boot with a live
/// coordinator team and sleepers on its run and end all of them without once
/// asking that team to seat them. It happened on 2026-09-18: three workers,
/// all of them finished, all announced dead, and a person had to notice.
///
/// Supplements the private-window grace fixture above with the shared-road
/// and launch-override contracts: the attempt precedes expiry and carries the
/// stored overrides unchanged.
#[test]
fn the_grace_tries_to_seat_a_sleeper_before_it_ends_it() {
    let shipped = include_str!("../orchestration.rs");
    let start = shipped.find("fn expire_sleepers").expect("the grace sweep");
    let body = shipped[start..]
        .split_once("\nfn ")
        .map_or(&shipped[start..], |(body, _)| body);

    let seats = body
        .find("seat_what_the_grace_would_kill(")
        .expect("the grace must try to seat the sleepers it is about to end");
    let ends = body
        .find("sleeper_expired(")
        .expect("the grace must still end a sleeper nothing could seat");
    assert!(
        seats < ends,
        "the seating attempt has to come before the ending, or it seats nothing"
    );

    /* And the seating is the same road a returned coordinator tab takes —
     * not a second copy of it, which would drift from the one that fills the
     * vacated chair and adopts under the right generation. */
    let helper = shipped
        .find("fn seat_what_the_grace_would_kill")
        .expect("the helper");
    let helper_body = shipped[helper..]
        .split_once("\nfn ")
        .map_or(&shipped[helper..], |(body, _)| body);
    assert!(
        helper_body.contains("reseat_sleeping("),
        "the grace must reuse reseat_sleeping, not a second seating road"
    );
    /* The overrides cell is handed back as it stands. `reseat_sleeping` writes
     * whatever it is given into that cell, so a sweep passing an empty list
     * would erase the launcher's stored overrides every beat. */
    assert!(
        helper_body.contains(".overrides") && helper_body.contains(".clone()"),
        "the grace must hand the stored overrides back rather than clearing them"
    );
}

/* ---- the step-effort seat (t-5637) ------------------------------------ */

/// The endpoint's answer to the step-effort question: `chosen` with the rest
/// of the moves spread evenly.
fn a_move_answer(chosen: &str) -> String {
    use zerocode_core::step_effort::Move;
    let rest = 0.2 / 2.0;
    let probabilities: serde_json::Map<String, serde_json::Value> = Move::ALL
        .iter()
        .map(|mv| {
            let share = if mv.word() == chosen { 0.8 } else { rest };
            (mv.word().to_string(), serde_json::json!(share))
        })
        .collect();
    serde_json::json!({
        "model": "jev-1.13.0",
        "answers": {
            "move": {
                "type": "choice",
                "choice": chosen,
                "probabilities": probabilities,
                "confidence": 0.7,
            }
        },
        "usage": { "input_tokens": 900, "output_tokens": 0 },
    })
    .to_string()
}

/// Claude Code's records for one turn at `effort`: a prompt, then `calls`
/// as `(tool, command, failed)`, each with its result.
fn a_claude_turn(prompt: &str, effort: &str, calls: &[(&str, &str, bool)]) -> Vec<String> {
    let mut rows = vec![
        serde_json::json!({"type": "user", "message": {"role": "user", "content": prompt}})
            .to_string(),
    ];
    for (at, (name, command, failed)) in calls.iter().enumerate() {
        rows.push(
            serde_json::json!({"type": "assistant", "effort": effort, "perTurnEffort": effort,
            "message": {"role": "assistant", "model": "claude-fable-5-1", "content": [
                {"type": "tool_use", "id": format!("toolu_{prompt}_{at}"), "name": name,
                 "input": {"command": command}}
            ]}})
            .to_string(),
        );
        rows.push(
            serde_json::json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": format!("toolu_{prompt}_{at}"),
                 "is_error": failed, "content": if *failed { "error" } else { "ok" }}
            ]}})
            .to_string(),
        );
    }
    rows.push(
        serde_json::json!({"type": "assistant", "effort": effort, "perTurnEffort": effort,
            "message": {"role": "assistant", "model": "claude-fable-5-1",
                        "content": [{"type": "text", "text": "done with that"}]}})
        .to_string(),
    );
    rows
}

/// A turn that made the same call three times and failed each time.
fn a_stuck_turn(prompt: &str, effort: &str) -> Vec<String> {
    a_claude_turn(
        prompt,
        effort,
        &[
            ("Bash", "cargo test -p x", true),
            ("Bash", "cargo test -p x", true),
            ("Bash", "cargo test -p x", true),
        ],
    )
}

/// A turn that read one file and went through.
fn a_progressed_turn(prompt: &str, effort: &str) -> Vec<String> {
    a_claude_turn(prompt, effort, &[("Read", "src/lib.rs", false)])
}

/// The worker's turn is under way on `rows`, then ends at `at`.
fn a_turn_runs_then_ends(stood: &StoppedWorker, rows: &[String], began_at: i64, at: i64) {
    std::fs::write(&stood.host.transcript, format!("{}\n", rows.join("\n")))
        .expect("the transcript");
    *stood.host.busy.lock().unwrap() = true;
    super::pane_turn_began(stood.host.worker_term, began_at);
    tick(&stood.host, &[], began_at);
    let owned: Vec<&str> = rows.iter().map(String::as_str).collect();
    stood.stops_on(&owned, at);
}

/// A stuck turn under a person's `on`: the seat asks once, the answer is a
/// row, the picker's command goes through the guarded door, its keys go
/// only once the screen shows the legend — one rung up, for this session
/// only — and the next turn's progress is the row's label. Then the rule
/// brings the effort back, through the same door, and the label says so.
#[test]
fn a_stuck_turn_raises_the_effort_one_rung_for_this_session_and_progress_brings_it_back() {
    use zerocode_core::capabilities::CLAUDE_EFFORT_PICKER;
    use zerocode_core::jev::{JevMode, STEP_EFFORT};
    let stood = StoppedWorker::stand_with(
        97_700,
        "--agent claude --model claude-fable-5-1 --effort medium",
        "",
        Vec::new(),
    );
    let raise =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_move_answer("raise"), 0);
    let home = stood.asks_jev_for(&STEP_EFFORT, &raise, JevMode::On, "/wt");
    let began = stood.began + 10_000;

    // The first turn: seen running, then ended stuck.
    let stuck = a_stuck_turn("fix the build", "medium");
    a_turn_runs_then_ends(&stood, &stuck, began, began + 1_000);
    tick(&stood.host, &[], began + 2_000);
    let rows = StoppedWorker::rows_of(&home, &STEP_EFFORT);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let asked = &rows[0];
    assert_eq!(asked["worker"], stood.worker.as_str());
    assert_eq!(asked["agent"], "claude");
    assert_eq!(asked["mode"], "on");
    assert_eq!(asked["door"], "picker");
    assert_eq!(asked["from"], "medium");
    assert_eq!(asked["to"], "high");
    assert_eq!(asked["floor"], "medium");
    assert_eq!(asked["ruled"], "raise");
    assert_eq!(asked["chosen"], "raise");
    assert_eq!(asked["outcome"], "answered");
    assert_eq!(asked["agreedWithRule"], true);
    assert_eq!(asked["signals"]["repeats"], 3);
    assert_eq!(asked["signals"]["toolFailures"], 3);
    assert_eq!(asked["requests"], 1);
    assert_eq!(
        asked[zerocode_core::jev::summary::MODEL.canonical],
        "jev-1.13.0",
        "the version that answered"
    );
    let heard = raise.asked();
    assert_eq!(heard.len(), 1, "asked {} times", heard.len());
    let sent = heard_body(&heard[0]);
    assert_eq!(sent["state"]["repeated"], "Bash · cargo test -p x");
    assert_eq!(sent["state"]["effort"], "medium");
    assert!(
        stood.sent_at_worker().is_empty(),
        "nothing typed on the asking beat"
    );

    // The next beat types the picker's command through the door; the one
    // after reads the receipt and waits for the legend; nothing else goes in
    // while the screen does not show it.
    tick(&stood.host, &[], began + 3_000);
    assert_eq!(stood.sent_at_worker(), [CLAUDE_EFFORT_PICKER.open, "\r"]);
    tick(&stood.host, &[], began + 4_000);
    tick(&stood.host, &[], began + 4_500);
    assert_eq!(stood.sent_at_worker().len(), 2, "no key before the legend");
    *stood.host.screen.lock().unwrap() = format!(
        "   Effort\n   low  ▲medium   high   xhigh   max   ultracode\n   ←/→ to adjust · Enter to confirm · {} · Esc to cancel\n",
        CLAUDE_EFFORT_PICKER.legend
    );
    tick(&stood.host, &[], began + 5_000);
    assert_eq!(
        stood.sent_at_worker(),
        [
            CLAUDE_EFFORT_PICKER.open,
            "\r",
            CLAUDE_EFFORT_PICKER.raise,
            CLAUDE_EFFORT_PICKER.session_only
        ]
    );
    *stood.host.screen.lock().unwrap() = String::new();
    assert_eq!(
        StoppedWorker::rows_of(&home, &STEP_EFFORT).len(),
        1,
        "no label before the next turn"
    );

    // The next turn goes through at the raised effort: the label says so,
    // and the rule now says bring it back.
    let mut progressed = stuck.clone();
    progressed.extend(a_progressed_turn("now the tests", "high"));
    let lower =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_move_answer("lower"), 0);
    stood.answers_from(&lower);
    a_turn_runs_then_ends(&stood, &progressed, began + 6_000, began + 7_000);
    tick(&stood.host, &[], began + 8_000);
    let rows = StoppedWorker::rows_of(&home, &STEP_EFFORT);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let label = &rows[1];
    assert_eq!(label["label"], asked["move"]);
    assert_eq!(label["applied"], true);
    assert_eq!(label["doorOutcome"], "keyed");
    assert_eq!(label["followed"], "progressed");
    assert_eq!(label["landed"], "high");
    // The seat raised where the rule raised: the next turn going through
    // grades the rule, not the answer (t-6342).
    assert!(label.get("agreed").is_none(), "{label}");
    assert_eq!(
        label[zerocode_core::jev::summary::NOT_COMPARED.canonical],
        zerocode_core::step_effort::SAME_AS_RULE
    );
    tick(&stood.host, &[], began + 9_000);
    let rows = StoppedWorker::rows_of(&home, &STEP_EFFORT);
    assert_eq!(rows.len(), 3, "{rows:?}");
    let restoring = &rows[2];
    assert_eq!(restoring["ruled"], "lower");
    assert_eq!(restoring["chosen"], "lower");
    assert_eq!(restoring["raised"], true);
    assert_eq!(restoring["from"], "high");
    assert_eq!(restoring["to"], "medium");
    tick(&stood.host, &[], began + 10_000);
    tick(&stood.host, &[], began + 11_000);
    *stood.host.screen.lock().unwrap() = CLAUDE_EFFORT_PICKER.legend.to_string();
    tick(&stood.host, &[], began + 12_000);
    let sent = stood.sent_at_worker();
    assert_eq!(
        &sent[4..],
        [
            CLAUDE_EFFORT_PICKER.open,
            "\r",
            CLAUDE_EFFORT_PICKER.lower,
            CLAUDE_EFFORT_PICKER.session_only
        ]
    );
}

/// Under `shadow` the seat asks and writes the same rows, and the composer
/// is never touched — the label grades what the turn did at the effort it
/// kept.
#[test]
fn a_recording_step_effort_seat_writes_its_rows_and_types_nothing() {
    use zerocode_core::jev::{JevMode, STEP_EFFORT};
    let stood = StoppedWorker::stand_with(
        97_800,
        "--agent claude --model claude-fable-5-1 --effort medium",
        "",
        Vec::new(),
    );
    let endpoint =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_move_answer("raise"), 0);
    let home = stood.asks_jev_for(&STEP_EFFORT, &endpoint, JevMode::Shadow, "/wt");
    let began = stood.began + 10_000;
    let stuck = a_stuck_turn("fix the build", "medium");
    a_turn_runs_then_ends(&stood, &stuck, began, began + 1_000);
    for beat in [2_000, 3_000, 4_000, 5_000] {
        tick(&stood.host, &[], began + beat);
    }
    let rows = StoppedWorker::rows_of(&home, &STEP_EFFORT);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["mode"], "shadow");
    assert_eq!(rows[0]["chosen"], "raise");
    assert!(
        stood.sent_at_worker().is_empty(),
        "a recording seat types nothing"
    );
    let mut next = stuck.clone();
    next.extend(a_stuck_turn("try again", "medium"));
    a_turn_runs_then_ends(&stood, &next, began + 6_000, began + 7_000);
    tick(&stood.host, &[], began + 8_000);
    let rows = StoppedWorker::rows_of(&home, &STEP_EFFORT);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1]["applied"], false);
    assert_eq!(rows[1]["doorOutcome"], "recorded");
    assert_eq!(rows[1]["followed"], "stuck");
    // Nothing was carried: the turn after says nothing about the answer.
    assert!(rows[1].get("agreed").is_none(), "{:?}", rows[1]);
    assert_eq!(
        rows[1][zerocode_core::jev::summary::NOT_COMPARED.canonical],
        zerocode_core::step_effort::NOT_CARRIED
    );
    assert!(stood.sent_at_worker().is_empty());
}

/// A move's label waits two hours for the next turn to end, as every
/// label of the seat's version 1 did (t-9087, astra R-EFFORT-1): the stall
/// label's move to four hours measured what follows a silence, not how long
/// a worker's next turn takes after an effort move, and a wait moved under
/// an unchanged rubric would grade version 1 rows two ways. A move whose
/// next turn ends three hours on is closed at two as `none`, with no mark,
/// and the turn that ends later grades nothing.
#[test]
fn a_move_whose_next_turn_ends_after_two_hours_is_closed_at_two_as_none() {
    use zerocode_core::jev::{JevMode, STEP_EFFORT};
    const HOUR_MS: i64 = 60 * 60 * 1_000;
    let stood = StoppedWorker::stand_with(
        97_850,
        "--agent claude --model claude-fable-5-1 --effort medium",
        "",
        Vec::new(),
    );
    let endpoint =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_move_answer("raise"), 0);
    let home = stood.asks_jev_for(&STEP_EFFORT, &endpoint, JevMode::Shadow, "/wt");
    let began = stood.began + 10_000;
    let stuck = a_stuck_turn("fix the build", "medium");
    a_turn_runs_then_ends(&stood, &stuck, began, began + 1_000);
    for beat in [2_000, 3_000, 4_000, 5_000] {
        tick(&stood.host, &[], began + beat);
    }
    let rows = StoppedWorker::rows_of(&home, &STEP_EFFORT);
    assert_eq!(rows.len(), 1, "{rows:?}");
    let asked = &rows[0];

    // Two hours and a second on, no turn has ended: the label closes.
    tick(&stood.host, &[], began + 5_000 + 2 * HOUR_MS + 1_000);
    let rows = StoppedWorker::rows_of(&home, &STEP_EFFORT);
    assert_eq!(rows.len(), 2, "the move is closed at two hours: {rows:?}");
    let label = &rows[1];
    assert_eq!(label["label"], asked["move"]);
    assert_eq!(label["followed"], "none");
    assert!(label.get("agreed").is_none(), "{label}");
    assert_eq!(
        label[zerocode_core::jev::summary::NOT_COMPARED.canonical],
        zerocode_core::step_effort::Followed::Nothing.word()
    );

    // The next turn ends three hours on: the move it follows is closed, and
    // nothing grades it again.
    let mut next = stuck.clone();
    next.extend(a_progressed_turn("now the tests", "medium"));
    a_turn_runs_then_ends(
        &stood,
        &next,
        began + 3 * HOUR_MS,
        began + 3 * HOUR_MS + 1_000,
    );
    tick(&stood.host, &[], began + 3 * HOUR_MS + 2_000);
    let labels = StoppedWorker::rows_of(&home, &STEP_EFFORT)
        .into_iter()
        .filter(|row| row["label"] == asked["move"])
        .count();
    assert_eq!(labels, 1, "one label for the move");
}

/// A composer with a turn under way gets nothing, and a move whose next
/// turn started before the composer rested is written down as such; a pane
/// a person took over is never read at all.
#[test]
fn a_busy_composer_and_a_taken_over_pane_get_no_effort_keys() {
    use zerocode_core::jev::{JevMode, STEP_EFFORT};
    let stood = StoppedWorker::stand_with(
        97_900,
        "--agent claude --model claude-fable-5-1 --effort medium",
        "",
        Vec::new(),
    );
    let endpoint =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_move_answer("raise"), 0);
    let home = stood.asks_jev_for(&STEP_EFFORT, &endpoint, JevMode::On, "/wt");
    let began = stood.began + 10_000;
    let stuck = a_stuck_turn("fix the build", "medium");
    a_turn_runs_then_ends(&stood, &stuck, began, began + 1_000);
    tick(&stood.host, &[], began + 2_000);
    assert_eq!(StoppedWorker::rows_of(&home, &STEP_EFFORT).len(), 1);
    // The next turn starts before the beat could type: the composer is busy.
    let mut next = stuck.clone();
    next.extend(a_stuck_turn("again", "medium"));
    std::fs::write(&stood.host.transcript, format!("{}\n", next.join("\n")))
        .expect("the transcript");
    *stood.host.busy.lock().unwrap() = true;
    super::pane_turn_began(stood.host.worker_term, began + 3_000);
    tick(&stood.host, &[], began + 3_000);
    tick(&stood.host, &[], began + 4_000);
    assert!(
        stood.sent_at_worker().is_empty(),
        "a running turn is not typed at"
    );
    let owned: Vec<&str> = next.iter().map(String::as_str).collect();
    stood.stops_on(&owned, began + 5_000);
    tick(&stood.host, &[], began + 6_000);
    let rows = StoppedWorker::rows_of(&home, &STEP_EFFORT);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1]["doorOutcome"], "turn_started");
    assert_eq!(rows[1]["applied"], false);
    assert!(stood.sent_at_worker().is_empty());

    // A person takes the pane: the seat reads nothing more of it.
    super::pane_taken_over(stood.host.worker_term, began + 6_500);
    let mut third = next.clone();
    third.extend(a_stuck_turn("and again", "medium"));
    a_turn_runs_then_ends(&stood, &third, began + 7_000, began + 8_000);
    for beat in [9_000, 10_000] {
        tick(&stood.host, &[], began + beat);
    }
    assert_eq!(
        StoppedWorker::rows_of(&home, &STEP_EFFORT).len(),
        2,
        "no row for a person's pane"
    );
    assert!(stood.sent_at_worker().is_empty());
}

/// A Codex worker has no door the beat can type through: its row records
/// the move under `relaunch`, the composer is never touched, and the label
/// still grades the next turn.
#[test]
fn a_codex_workers_move_is_recorded_under_relaunch_and_nothing_is_typed() {
    use zerocode_core::jev::{JevMode, STEP_EFFORT};
    let stood = StoppedWorker::stand_with(
        98_000,
        "--agent codex --model gpt-6-astra --effort medium",
        "",
        Vec::new(),
    );
    let endpoint =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_move_answer("raise"), 0);
    let home = stood.asks_jev_for(&STEP_EFFORT, &endpoint, JevMode::On, "/wt");
    let began = stood.began + 10_000;
    let context = |effort: &str| {
        serde_json::json!({"type": "turn_context", "payload": {"turn_id": "t", "model": "gpt-6-astra", "effort": effort}}).to_string()
    };
    let prompt = |text: &str| {
        serde_json::json!({"type": "response_item", "payload": {"type": "message", "role": "user",
            "content": [{"type": "input_text", "text": text}]}})
        .to_string()
    };
    let call = |id: &str| {
        serde_json::json!({"type": "response_item", "payload": {"type": "function_call", "name": "shell",
            "call_id": id, "arguments": "{\"command\":[\"cargo\",\"test\"]}"}}).to_string()
    };
    let output = |id: &str| {
        serde_json::json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": id, "output": "exit 101"}}).to_string()
    };
    let stuck = vec![
        context("medium"),
        prompt("run the tests"),
        call("c1"),
        output("c1"),
        call("c2"),
        output("c2"),
        call("c3"),
        output("c3"),
    ];
    a_turn_runs_then_ends(&stood, &stuck, began, began + 1_000);
    for beat in [2_000, 3_000, 4_000] {
        tick(&stood.host, &[], began + beat);
    }
    let rows = StoppedWorker::rows_of(&home, &STEP_EFFORT);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["agent"], "codex");
    assert_eq!(rows[0]["door"], "relaunch");
    assert_eq!(rows[0]["from"], "medium");
    assert_eq!(rows[0]["to"], "high");
    assert_eq!(rows[0]["chosen"], "raise");
    assert!(
        stood.sent_at_worker().is_empty(),
        "nothing to type at Codex"
    );
    let mut next = stuck.clone();
    next.extend([
        context("medium"),
        prompt("and now"),
        call("c4"),
        output("c4"),
    ]);
    a_turn_runs_then_ends(&stood, &next, began + 5_000, began + 6_000);
    tick(&stood.host, &[], began + 7_000);
    let rows = StoppedWorker::rows_of(&home, &STEP_EFFORT);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[1]["doorOutcome"], "relaunch");
    assert_eq!(rows[1]["applied"], false);
    assert_eq!(rows[1]["followed"], "progressed");
    assert_eq!(rows[1]["landed"], "medium");
    assert!(stood.sent_at_worker().is_empty());
}

/// The roster row the board draws reads a worker's `merged`/`verified` as
/// its claim, and only a coordinator's correction as the fact (t-6815) —
/// through the real road: the worker's `worker_done` posted, the ledger
/// exported and rebuilt as a restart would, the row built from that.
#[test]
fn a_workers_claimed_merge_reaches_the_roster_row_as_a_claim() {
    use zerocode_core::orchestration::{
        Draft, Ledger, MessageKind, Priority, ResultAuthor, Text, worker_address,
    };
    let mut ledger = Ledger::new();
    let run = ledger.create_run("claims", 1);
    let task = ledger
        .create_task(&run, "ship".into(), "ship it".into(), vec![], None, 2)
        .expect("a task");
    let started = ledger
        .start_worker(&run, "codex", ("team-claims", "%2"), Some(&task), 3)
        .expect("a worker");
    ledger
        .post(
            &run,
            Draft {
                from: worker_address(&started.worker),
                to: ledger.run(&run).expect("the run").address(),
                kind: MessageKind::WorkerDone,
                body: Text::from(
                    r#"{"ok":true,"merged":true,"verified":true,"deployed":true}"#.to_string(),
                ),
                subject: Text::default(),
                priority: Priority::Normal,
                payload: Text::default(),
                thread: None,
                task: Some(task.clone()),
                dispatch: started.dispatch.clone(),
            },
            4,
        )
        .expect("the worker's report");
    let row = |ledger: &Ledger| {
        super::ledger_agents_for_seats(ledger, &super::TeamSeatIndex::new())
            .into_iter()
            .find(|one| one.worker == started.worker)
            .expect("the worker's row")
    };
    let rebuilt = Ledger::rebuild(ledger.export()).expect("a readable ledger");
    for (which, held) in [("live", &ledger), ("rebuilt", &rebuilt)] {
        let claimed = row(held);
        assert!(claimed.reported, "{which}: the report closed the attempt");
        assert!(
            !claimed.review.verified && !claimed.review.merged && !claimed.review.deployed,
            "{which}: a worker's keys reached the row as the coordinator's facts: {:?}",
            claimed.review
        );
        assert!(
            claimed.review.claimed_merged && claimed.review.claimed_verified,
            "{which}: the claim was dropped rather than kept apart: {:?}",
            claimed.review
        );
    }

    // The coordinator's correction, against the attempt it reviewed, is the fact.
    let source = ledger
        .run(&run)
        .and_then(|held| {
            held.dispatches
                .iter()
                .find(|one| Some(one.id.as_str()) == started.dispatch.as_deref())
        })
        .and_then(|one| one.source.clone());
    ledger
        .update_task(
            &run,
            &task,
            None,
            Some(r#"{"merged":true,"mergeHead":"3a0a289bb5f3"}"#.to_string()),
            ResultAuthor::Coordinator {
                seat: "team-claims/%1".to_string(),
                generation: Some(1),
                attempt: started.dispatch.clone(),
                source,
            },
        )
        .expect("the coordinator corrects");
    let rebuilt = Ledger::rebuild(ledger.export()).expect("a readable ledger");
    for (which, held) in [("live", &ledger), ("rebuilt", &rebuilt)] {
        let accepted = row(held);
        assert!(
            accepted.review.merged && !accepted.review.claimed_merged,
            "{which}: {:?}",
            accepted.review
        );
        assert_eq!(
            accepted.review.merge_head.as_deref(),
            Some("3a0a289bb5f3"),
            "{which}"
        );
    }
}

/// What the window SAW is not what the ledger was TOLD (astra R1a,
/// t-6740 r2). A wait on the person that the disk refused to write down is
/// told on the next sighting of the same wait — the window's map of what it
/// saw no longer stands between that sighting and the actor — and it is
/// told ONCE: the ledger's own record of the fact, not the window's memory,
/// answers every sighting after that.
#[test]
fn a_refused_attention_notice_is_told_on_the_next_sighting() {
    const LEADER: u32 = 11_116;
    const WORKER: u32 = 11_117;
    let (window, store) = PrivateWindow::boot();
    let team = format!("team-attention-retry-{LEADER}");
    let (_run, worker, _pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    // The coordinator asks the worker; the shortest budget runs out and the
    // question stands, unanswered.
    let asked = run(
        &Nowhere,
        Vec::new(),
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(&format!(
            "ask --to worker:{worker} --body which-branch --timeout-ms {}",
            zerocode_core::orchestration::WAIT_BUDGET_MIN_MS
        )),
        clock(),
    );
    assert_eq!(asked.exit_code, 0, "{}", asked.stderr);
    let asked: serde_json::Value = serde_json::from_str(&asked.stdout).expect("an answer");
    let question = asked["questionId"]
        .as_str()
        .expect("a question")
        .to_string();
    let told = || -> Vec<serde_json::Value> {
        the_rows()
            .messages
            .iter()
            .filter(|row| {
                row.from == zerocode_core::orchestration::LEDGER_ITSELF
                    && row.thread.as_deref() == Some(question.as_str())
            })
            .map(|row| serde_json::from_str(row.body.as_str()).expect("a notice is JSON"))
            .collect()
    };

    let connection = store
        .fault_connection_for_tests()
        .expect("fault connection");
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_attention_write
                    AFTER UPDATE OF revision ON orchestration_ledger_heads
                    BEGIN SELECT RAISE(ABORT, 'injected attention refusal'); END;",
        )
        .expect("the disk refuses the notice");
    let since = clock();
    super::pane_attention_noted(WORKER, Some(since), since + 1);
    assert!(
        told().is_empty(),
        "a notice the disk refused stood in the rows"
    );

    connection
        .execute_batch("DROP TRIGGER refuse_attention_write;")
        .expect("the disk comes back");
    super::pane_attention_noted(WORKER, Some(since), since + 2);
    let lines = told();
    assert_eq!(
        lines.len(),
        1,
        "a wait the disk refused was never told: {lines:?}"
    );
    assert_eq!(
        lines[0]["reason"],
        zerocode_core::orchestration::ReceiverNews::AwaitingInput.word()
    );
    assert_eq!(lines[0]["factMs"], since);
    // Seen again: the ledger's record answers, and nothing is added.
    super::pane_attention_noted(WORKER, Some(since), since + 3);
    assert_eq!(told().len(), 1, "one wait was told twice");

    // The terminal changes hands: its worker is stopped and the terminal is
    // gone, and another worker is seated in a pane that reuses the number.
    // The last owner's wait, sighted again late, is not the new owner's —
    // and the new owner's own wait is its asker's news.
    let host = Splitting::onto(WORKER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let stopped = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&format!("worker-stop --worker {worker} --reason moved")),
        clock(),
    );
    assert_eq!(stopped.exit_code, 0, "{}", stopped.stderr);
    crate::agent_teams::forget_term(WORKER);
    let started = run(
        &host,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words("worker-start --agent claude"),
        clock(),
    );
    assert_eq!(started.exit_code, 0, "{}", started.stderr);
    let started: serde_json::Value = serde_json::from_str(&started.stdout).expect("a worker");
    let next = started["workerId"].as_str().expect("a worker id");
    let asked = run(
        &Nowhere,
        Vec::new(),
        &team,
        leader,
        TEST_CAPABILITY,
        &words(&format!(
            "ask --to worker:{next} --body which-tag --timeout-ms {}",
            zerocode_core::orchestration::WAIT_BUDGET_MIN_MS
        )),
        clock(),
    );
    assert_eq!(asked.exit_code, 0, "{}", asked.stderr);
    let asked: serde_json::Value = serde_json::from_str(&asked.stdout).expect("an answer");
    let next_question = asked["questionId"]
        .as_str()
        .expect("a question")
        .to_string();
    let told_next = || {
        the_rows()
            .messages
            .iter()
            .filter(|row| {
                row.from == zerocode_core::orchestration::LEDGER_ITSELF
                    && row.thread.as_deref() == Some(next_question.as_str())
            })
            .count()
    };
    super::pane_attention_noted(WORKER, Some(since), clock());
    assert_eq!(
        told_next(),
        0,
        "the new owner was told the last owner's wait"
    );
    let own = clock();
    super::pane_attention_noted(WORKER, Some(own), own + 1);
    assert_eq!(told_next(), 1, "the new owner's own wait was not told");

    super::forget_pane_attention(WORKER);
    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);
    drop(window);
}

/// The deadline's answer and the ask's receipt are ONE result (astra R2,
/// t-6740 r2): the caller went home with the timed-out answer, so the same
/// request asked again replays those bytes — never the first empty look the
/// wait began from, which said nothing about a deadline.
#[test]
fn a_timed_out_ask_replays_the_answer_it_went_home_with() {
    const LEADER: u32 = 11_118;
    const WORKER: u32 = 11_119;
    let (window, _store) = PrivateWindow::boot();
    let team = format!("team-ask-deadline-{LEADER}");
    let (_run, worker, _pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let line = format!(
        "ask --to worker:{worker} --body which-branch --timeout-ms {} \
         --retry-request ask-deadline-{LEADER}",
        zerocode_core::orchestration::WAIT_BUDGET_MIN_MS
    );
    let ask = || {
        run(
            &Nowhere,
            Vec::new(),
            &team,
            zerocode_core::agent_teams::LEADER_PANE,
            TEST_CAPABILITY,
            &words(&line),
            clock(),
        )
    };
    let first = ask();
    assert_eq!(first.exit_code, 0, "{}", first.stderr);
    let said: serde_json::Value = serde_json::from_str(&first.stdout).expect("an answer");
    assert_eq!(said["timedOut"], true, "{said}");
    let again = ask();
    assert_eq!(again.exit_code, 0, "{}", again.stderr);
    assert_eq!(
        again.stdout, first.stdout,
        "the same request replayed another answer than the one its caller went home with"
    );

    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);
    drop(window);
}

/// A late turn end is never the next occupant's sound (astra R3, t-6740
/// r3) — on either road the window hears it by: the turn-end report
/// itself, and the beat's readiness sweep, which drains the same sound.
/// The last worker in a terminal comes to rest; it is stopped, and a new
/// worker on work of its own is seated in a pane that reuses the number;
/// only then is the rest reported. The new worker's readiness window stays
/// open through the report and the beat after it, no `went_quiet` names its
/// attempt, and its asker hears nothing — until its own turn ends.
#[test]
fn a_late_turn_end_is_never_the_next_occupants_sound() {
    const LEADER: u32 = 11_160;
    const WORKER: u32 = 11_161;
    let (window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    let team = format!("team-late-sound-{LEADER}");
    let (_run, last, _pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    let host = Splitting::onto(WORKER);
    // Every step on its own millisecond, so "after" is never "the same".
    let base = clock() + 1_000;
    let verb = |line: &str, at: i64| -> serde_json::Value {
        let said = run(
            &host,
            Vec::new(),
            &team,
            leader,
            TEST_CAPABILITY,
            &words(line),
            at,
        );
        assert_eq!(said.exit_code, 0, "`{line}`: {}", said.stderr);
        serde_json::from_str(&said.stdout).expect("JSON")
    };

    // The last occupant comes to rest at `rested`; the report will be late.
    let rested = base + 1;
    verb(
        &format!("worker-stop --worker {last} --reason moved"),
        base + 2,
    );
    crate::agent_teams::forget_term(WORKER);
    let task = verb("task-create --spec next", base + 3)["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let started = verb(
        &format!("worker-start --agent claude --task {task}"),
        base + 4,
    );
    let next = started["workerId"]
        .as_str()
        .expect("a worker id")
        .to_string();
    let asked = verb(
        &format!(
            "ask --to worker:{next} --body which-tag --timeout-ms {}",
            zerocode_core::orchestration::WAIT_BUDGET_MIN_MS
        ),
        base + 5,
    );
    let question = asked["questionId"]
        .as_str()
        .expect("a question")
        .to_string();
    let standing = || {
        let rows = the_rows();
        let held = rows
            .workers
            .iter()
            .find(|row| row.id == next)
            .expect("the next occupant's row")
            .clone();
        let attempt = held.dispatch.clone().expect("the next occupant's attempt");
        let quiet = rows
            .messages
            .iter()
            .filter(|row| {
                row.kind == zerocode_core::orchestration::MessageKind::WentQuiet
                    && row.dispatch.as_deref() == Some(attempt.as_str())
            })
            .count();
        let told = rows
            .messages
            .iter()
            .filter(|row| {
                row.from == zerocode_core::orchestration::LEDGER_ITSELF
                    && row.thread.as_deref() == Some(question.as_str())
                    && row.body.as_str().contains("\"turn_ended\"")
            })
            .count();
        (held.ready_by_ms, held.quiet_at, quiet, told)
    };
    let before = standing();
    assert!(
        before.0.is_some(),
        "the next occupant has no readiness window"
    );
    assert_eq!(before, (before.0, None, 0, 0));

    super::pane_turn_ended(WORKER, rested, false, base + 6);
    let after_report = standing();
    tick(&Nowhere, &[], base + 7);
    let after_beat = standing();
    assert_eq!(
        (after_report, after_beat),
        (before, before),
        "the last occupant's late rest moved the next one"
    );

    // Its own turn end is its sound, its silence and its asker's news.
    let own = base + 8;
    super::pane_turn_ended(WORKER, own, false, base + 9);
    assert_eq!(standing(), (None, Some(own), 1, 1));

    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);
    drop(window);
}

/// t-7812: the window restart restore roads, driven through the production
/// doors of a private window (`tests/restore.rs`).
mod restore;
/// t-7812 r2: the window's own resume road through a fake launcher, beside
/// the ledger's reseat (`tests/restore_door.rs`).
mod restore_door;
/// t-9091: a closing window's beat leaves its sleepers to the next window
/// (`tests/restore_goodbye.rs`).
mod restore_goodbye;
/// t-7812: the host seams those roads added (`tests/restore_seams.rs`).
mod restore_seams;

/* ---- account switch: the same seat, another login (t-7538) --------------- */

/// The conversation a switch test's walled pane is in.
const SWITCH_SESSION: &str = "5a1e0000-0000-4000-8000-000000007538";

/// A window's panes for the switch road: splits open where told and run as
/// whichever account the window selected at that moment; every close, every
/// split command and every briefing handed to a new pane is kept; each pane
/// reports the conversation a test gives it.
struct Switching {
    closed: Mutex<Vec<u32>>,
    onto: Mutex<u32>,
    checkout: &'static str,
    /// The screens' wall words and the busy panes — shared, so a test can
    /// move them from inside the switch (`SwitchWorld::after_select`).
    markers: std::sync::Arc<
        Mutex<std::collections::HashMap<u32, zerocode_core::orchestration::QuotaWallMarker>>,
    >,
    busy: std::sync::Arc<Mutex<std::collections::HashSet<u32>>>,
    sessions: Mutex<std::collections::HashMap<u32, zerocode_core::ProviderSession>>,
    accounts: Mutex<std::collections::HashMap<u32, crate::agent_teams::PaneLogin>>,
    /// The account a pane opened NOW runs as — the window's selection.
    selected: std::sync::Arc<Mutex<Option<String>>>,
    /// Which login each account id names now, when a test says it names
    /// another than `login-of-{id}` — shared with the world's store.
    logins: std::sync::Arc<Mutex<std::collections::HashMap<String, String>>>,
    splits: Mutex<Vec<(u32, String)>>,
    asks: Mutex<
        Vec<(
            u32,
            String,
            Option<zerocode_core::orchestration::WorkerResume>,
        )>,
    >,
    /// A closed pane whose program outlives the wait.
    lingering: Mutex<bool>,
    /// Every continuation pasted into a pane, by term (t-7538 r4).
    pasted: Mutex<Vec<(u32, String)>>,
    /// How often a road asked whether a live pane holds a conversation —
    /// what taking a conversation's claim asks first (t-7812).
    standing_asked: Mutex<usize>,
    /// Run once inside the next wall observation at the rest's fence,
    /// before its commit — the world moving at the moment of use.
    during_observation: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Run once as the next pane closes — after the rest was written down,
    /// before the pane the move opens exists (astra R4-1).
    at_the_close: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Switching {
    fn new(checkout: &'static str, first_term: u32) -> Self {
        Self {
            closed: Mutex::new(Vec::new()),
            onto: Mutex::new(first_term),
            checkout,
            markers: std::sync::Arc::new(Mutex::new(std::collections::HashMap::new())),
            busy: std::sync::Arc::new(Mutex::new(std::collections::HashSet::new())),
            sessions: Mutex::new(std::collections::HashMap::new()),
            accounts: Mutex::new(std::collections::HashMap::new()),
            selected: std::sync::Arc::new(Mutex::new(Some("a-fixture".to_string()))),
            logins: std::sync::Arc::new(Mutex::new(std::collections::HashMap::new())),
            splits: Mutex::new(Vec::new()),
            asks: Mutex::new(Vec::new()),
            lingering: Mutex::new(false),
            pasted: Mutex::new(Vec::new()),
            standing_asked: Mutex::new(0),
            during_observation: Mutex::new(None),
            at_the_close: Mutex::new(None),
        }
    }

    fn seating_onto(&self, term: u32) {
        *self.onto.lock().unwrap_or_else(|held| held.into_inner()) = term;
    }

    fn screen_says(&self, term: u32, line: &str) {
        self.markers.lock().unwrap().insert(
            term,
            zerocode_core::orchestration::QuotaWallMarker {
                source: "screen".to_string(),
                line: zerocode_core::orchestration::Text::from(line),
            },
        );
    }

    fn working(&self, term: u32) {
        self.busy.lock().unwrap().insert(term);
    }

    fn in_conversation(&self, term: u32, id: &str, transcript: &Path) {
        self.sessions.lock().unwrap().insert(
            term,
            zerocode_core::ProviderSession {
                key: zerocode_core::provider_session::SessionKey::SessionId,
                id: id.to_string(),
                transcript_path: Some(transcript.to_string_lossy().into_owned()),
            },
        );
    }

    fn closed(&self) -> Vec<u32> {
        self.closed.lock().unwrap().clone()
    }

    /// The login `account` names now.
    fn login_of(&self, account: &str) -> String {
        self.logins
            .lock()
            .unwrap()
            .get(account)
            .cloned()
            .unwrap_or_else(|| format!("login-of-{account}"))
    }

    /// The pane in `term` runs as `account`, as a launch of it would have
    /// recorded.
    fn runs_as(&self, term: u32, account: &str) {
        let login = self.login_of(account);
        self.accounts.lock().unwrap().insert(
            term,
            crate::agent_teams::PaneLogin {
                account: account.to_string(),
                login,
            },
        );
    }
}

impl Host for Switching {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        command: &str,
        token: &str,
    ) -> Option<u32> {
        // The next split opens one term further on, so one switch that
        // reseats several panes gives each its own.
        let term = {
            let mut onto = self.onto.lock().unwrap_or_else(|held| held.into_inner());
            let term = *onto;
            *onto += 1;
            term
        };
        crate::agent_teams::place_seat_checkout(token, self.checkout.to_string());
        // The host is the ask's one reader, as the window's own is.
        if let Some(ask) = crate::agent_teams::take_worker_host_ask(token) {
            self.asks
                .lock()
                .unwrap()
                .push((term, ask.prompt.clone(), ask.resumed));
        }
        self.splits
            .lock()
            .unwrap()
            .push((term, command.to_string()));
        if let Some(account) = self.selected.lock().unwrap().clone() {
            self.runs_as(term, &account);
        }
        Some(term)
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        true
    }
    fn paste(&self, term: u32, text: &str) -> bool {
        self.pasted.lock().unwrap().push((term, text.to_string()));
        true
    }
    fn conversation_standing(
        &self,
        _agent: &str,
        _session: &zerocode_core::ProviderSession,
    ) -> Option<u32> {
        *self.standing_asked.lock().unwrap() += 1;
        None
    }
    fn pane_exists(&self, term: u32) -> bool {
        !self.closed.lock().unwrap().contains(&term)
    }
    fn quiet_since(&self, term: u32, _worker_started_ms: i64, now_ms: i64) -> Option<i64> {
        (!self.busy.lock().unwrap().contains(&term))
            .then(|| now_ms - zerocode_core::orchestration::QUIET_GRACE_MS)
    }
    fn quota_wall_marker(
        &self,
        term: u32,
        _agent: &str,
    ) -> Option<zerocode_core::orchestration::QuotaWallMarker> {
        self.markers.lock().unwrap().get(&term).cloned()
    }
    fn with_quota_wall_observation(
        &self,
        term: u32,
        _worker_started_ms: i64,
        _agent: &str,
        commit: &mut dyn FnMut(zerocode_core::orchestration::QuotaWallMarker),
    ) {
        let moving = self.during_observation.lock().unwrap().take();
        if let Some(moving) = moving {
            moving();
        }
        let busy = self.busy.lock().unwrap().contains(&term);
        let marker = self.markers.lock().unwrap().get(&term).cloned();
        if !busy && let Some(marker) = marker {
            commit(marker);
        }
    }
    fn capture(&self, _term: u32) -> Option<String> {
        Some(String::new())
    }
    fn focus(&self, _term: u32) -> bool {
        true
    }
    /// A close reaches the ledger the way the window's own does
    /// (`retire_terminal` → the terminal's settlement → `terminal_gone`), so
    /// what the ledger makes of a closed pane is observed, not assumed.
    fn close(&self, term: u32) {
        self.closed.lock().unwrap().push(term);
        terminal_gone(term, clock());
    }
    fn close_gone(&self, term: u32) -> crate::agent_teams::PaneExit {
        self.close(term);
        let closing = self.at_the_close.lock().unwrap().take();
        if let Some(closing) = closing {
            closing();
        }
        if *self.lingering.lock().unwrap() {
            crate::agent_teams::PaneExit::Lingering(crate::agent_teams::ExitWitness {
                group: term,
                started: Some(format!("fixture-start-{term}")),
            })
        } else {
            crate::agent_teams::PaneExit::Gone
        }
    }
    /// Every pane runs a program the test can name, read before a close.
    fn exit_witness(&self, term: u32) -> Option<crate::agent_teams::ExitWitness> {
        Some(crate::agent_teams::ExitWitness {
            group: term,
            started: Some(format!("fixture-start-{term}")),
        })
    }
    /// A lingering program leaves when the test says so.
    fn exit_seen(&self, _witness: &crate::agent_teams::ExitWitness) -> bool {
        !*self.lingering.lock().unwrap()
    }
    fn provider_session(&self, term: u32) -> Option<zerocode_core::ProviderSession> {
        self.sessions.lock().unwrap().get(&term).cloned()
    }
    fn pane_login(&self, term: u32) -> Option<crate::agent_teams::PaneLogin> {
        self.accounts.lock().unwrap().get(&term).cloned()
    }
    fn actor_for(&self, term: u32) -> Option<String> {
        Some(test_actor(term))
    }
}

/// The world a switch walks through besides the ledger and the panes: the
/// setting, the store's selection, the gauges, the clocks and a journal
/// directory — the test's own, so no two tests share a cooldown.
struct SwitchWorld {
    /// How many times the selection's followers ran — zo's login reload
    /// among them (`announce_account_switch` in the window's own doors).
    followed: Mutex<usize>,
    /// The setting — shared, so a test can move it from inside the host's
    /// own observation at the rest's fence.
    mode: std::sync::Arc<Mutex<zerocode_core::account_autoswitch::AutoSwitchMode>>,
    gauges: Mutex<Vec<zerocode_core::account_autoswitch::AccountGauge>>,
    selected: std::sync::Arc<Mutex<Option<String>>>,
    selects: Mutex<Vec<Option<String>>>,
    refuse: Mutex<Option<String>>,
    last_switch: Mutex<Option<i64>>,
    failures: Mutex<Vec<(String, i64)>>,
    journal: tempfile::TempDir,
    lines: Mutex<Vec<String>>,
    /// The clock the switch commits by — the last moment the situation was
    /// read, unless a test moves it.
    now: Mutex<i64>,
    /// Which login each account id names, when a test says it names
    /// another than the gauges' own (astra R6) — the host's own table, so a
    /// pane opened after a change runs as the login named then.
    logins: std::sync::Arc<Mutex<std::collections::HashMap<String, String>>>,
    /// Run once, right after a select lands — the world moving under the
    /// switch between its selection and its panes (astra R2).
    after_select: Mutex<Option<WorldMove>>,
    /// Run once, right before the first receipt is written — the ledger's
    /// disk refusing writes from there on (astra R4).
    before_receipt: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

/// One change a test makes to the switch's world from inside the switch.
type WorldMove = Box<dyn FnOnce(&SwitchWorld) + Send>;

impl SwitchWorld {
    fn new(host: &Switching, mode: zerocode_core::account_autoswitch::AutoSwitchMode) -> Self {
        Self {
            followed: Mutex::new(0),
            mode: std::sync::Arc::new(Mutex::new(mode)),
            gauges: Mutex::new(Vec::new()),
            selected: std::sync::Arc::clone(&host.selected),
            selects: Mutex::new(Vec::new()),
            refuse: Mutex::new(None),
            last_switch: Mutex::new(None),
            failures: Mutex::new(Vec::new()),
            journal: tempfile::tempdir().expect("a journal directory"),
            lines: Mutex::new(Vec::new()),
            now: Mutex::new(0),
            logins: std::sync::Arc::clone(&host.logins),
            after_select: Mutex::new(None),
            before_receipt: Mutex::new(None),
        }
    }

    /// Two accounts: `a-fixture` (Max) at `a_used` on its session, and
    /// `b-fixture` (Team) at `b_used`, both read a minute ago.
    fn reads(&self, a_used: u8, b_used: u8, now_ms: i64) {
        let gauge =
            |id: &str, org: &str, used: u8| zerocode_core::account_autoswitch::AccountGauge {
                id: id.to_string(),
                org_type: Some(org.to_string()),
                identity: Some(format!("login-of-{id}")),
                windows: vec![
                    zerocode_core::account_autoswitch::GaugeWindow {
                        kind: "session".to_string(),
                        used_percent: used,
                        resets_at_ms: Some(now_ms + 3 * 60 * 60_000),
                    },
                    zerocode_core::account_autoswitch::GaugeWindow {
                        kind: "weekly".to_string(),
                        used_percent: 40,
                        resets_at_ms: Some(now_ms + 5 * 24 * 60 * 60_000),
                    },
                ],
                observed_at_ms: now_ms - 60_000,
                status: "ok".to_string(),
            };
        *self.gauges.lock().unwrap() = vec![
            gauge("a-fixture", "claude_max", a_used),
            gauge("b-fixture", "claude_team", b_used),
        ];
    }

    fn selects(&self) -> Vec<Option<String>> {
        self.selects.lock().unwrap().clone()
    }
}

impl crate::account_switch::SwitchDoors for SwitchWorld {
    fn situation(&self, now_ms: i64) -> Result<crate::account_switch::Situation, String> {
        *self.now.lock().unwrap() = now_ms;
        let gauges = self.gauges.lock().unwrap().clone();
        let named = self.logins.lock().unwrap().clone();
        let logins = gauges
            .iter()
            .map(|gauge| {
                let login = named
                    .get(&gauge.id)
                    .cloned()
                    .or_else(|| gauge.identity.clone())
                    .unwrap_or_default();
                (gauge.id.clone(), login)
            })
            .collect();
        Ok(crate::account_switch::Situation {
            mode: *self.mode.lock().unwrap(),
            active: self.selected.lock().unwrap().clone(),
            gauges,
            last_switch_ms: *self.last_switch.lock().unwrap(),
            failures: self.failures.lock().unwrap().clone(),
            logins,
            now_ms,
        })
    }
    fn select(&self, to: Option<&str>) -> Result<(), String> {
        if let Some(why) = self.refuse.lock().unwrap().clone() {
            return Err(why);
        }
        self.selects.lock().unwrap().push(to.map(str::to_string));
        *self.selected.lock().unwrap() = to.map(str::to_string);
        let moving = self.after_select.lock().unwrap().take();
        if let Some(moving) = moving {
            moving(self);
        }
        Ok(())
    }
    fn now_ms(&self) -> i64 {
        *self.now.lock().unwrap()
    }
    fn record(
        &self,
        receipt: zerocode_core::orchestration::AccountSwitchReceipt,
        now_ms: i64,
    ) -> Result<bool, String> {
        let refusing = self.before_receipt.lock().unwrap().take();
        if let Some(refusing) = refusing {
            refusing();
        }
        super::record_account_switch(receipt, now_ms)
    }
    fn selected(&self, _to: Option<&str>, at_ms: i64) {
        *self.followed.lock().unwrap() += 1;
        *self.last_switch.lock().unwrap() = Some(at_ms);
    }
    fn select_refused(&self, to: &str, at_ms: i64) {
        self.failures.lock().unwrap().push((to.to_string(), at_ms));
    }
    fn overrides(&self) -> Vec<(String, LaunchOverride)> {
        Vec::new()
    }
    fn journal_dir(&self) -> PathBuf {
        self.journal.path().to_path_buf()
    }
    fn log(&self, line: &str) {
        self.lines.lock().unwrap().push(line.to_string());
    }
}

/// A Claude transcript as 2.1.28x writes one: the permission mode, a person
/// who moved the pane to Opus at xhigh (the summons said Fable at max), and
/// the turn that met the wall.
fn a_switch_transcript(dir: &Path) -> PathBuf {
    let path = dir.join(format!("{SWITCH_SESSION}.jsonl"));
    let rows = [
        serde_json::json!({"type": "permission-mode", "permissionMode": "bypassPermissions", "sessionId": SWITCH_SESSION}),
        serde_json::json!({"type": "user", "permissionMode": "bypassPermissions", "uuid": "u-1",
            "message": {"role": "user", "content": "carry on with the task"}}),
        serde_json::json!({"type": "assistant", "effort": "max", "perTurnEffort": "max", "uuid": "a-1",
            "message": {"role": "assistant", "model": "claude-fable-5-1", "id": "msg-1",
                "content": [{"type": "tool_use", "id": "toolu-1", "name": "Bash", "input": {"command": "cargo test"}}]}}),
        serde_json::json!({"type": "user", "permissionMode": "bypassPermissions", "uuid": "u-2",
            "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu-1", "content": "ok"}]}}),
        serde_json::json!({"type": "assistant", "effort": "xhigh", "perTurnEffort": "ultracode", "uuid": "a-2",
            "message": {"role": "assistant", "model": "claude-opus-5-5", "id": "msg-2",
                "content": [{"type": "text", "text": "You've hit your limit · resets 3am"}]}}),
    ];
    let text: String = rows.iter().map(|row| format!("{row}\n")).collect();
    std::fs::write(&path, text).expect("a transcript");
    path
}

fn digest_of(path: &Path) -> String {
    use std::hash::{Hash, Hasher};
    let bytes = std::fs::read(path).expect("the transcript");
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{}:{:016x}", bytes.len(), hasher.finish())
}

/// A private window with one CLAUDE worker at its wall in `checkout`, in a
/// conversation it reported, summoned as Fable at max — the two witnesses
/// written by the stall sweep against the gauge of the account its pane
/// runs as — and, when asked, `working` more Claude workers busy in panes
/// of their own. What every switch test starts from.
struct WalledClaude {
    _window: PrivateWindow,
    store: zerocode_orchestrator::workflow_store::WorkflowStore,
    _beat: std::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    host: Switching,
    world: SwitchWorld,
    leader: u32,
    team: String,
    task: String,
    worker: String,
    dispatch: String,
    transcript: PathBuf,
    working: Vec<(String, u32)>,
    /// A zo worker on a Claude model, when asked for: its pane and id.
    zo: Option<(String, u32)>,
    /// More walled Claude workers beside the first, each in its own
    /// conversation: `(worker, term)`.
    walled_more: Vec<(String, u32)>,
    began: i64,
}

impl WalledClaude {
    /// The worker in `term` reports a conversation of its own, as every
    /// worker does: a sleeper that never did is told once and not reseated
    /// (t-7812).
    fn in_its_own_conversation(&self, term: u32, session: &str) {
        let path = self.transcript.with_file_name(format!("{session}.jsonl"));
        std::fs::copy(&self.transcript, &path).expect("a transcript of its own");
        self.host.in_conversation(term, session, &path);
        super::pane_session_reported(
            term,
            &self.host.provider_session(term).expect("a session"),
            self.began + 19_999,
        );
    }

    fn stand(leader_term: u32, working: usize) -> Self {
        Self::stand_with(leader_term, working, false)
    }

    fn stand_with(leader_term: u32, working: usize, zo: bool) -> Self {
        Self::stand_full(leader_term, working, zo, 0)
    }

    fn stand_full(leader_term: u32, working: usize, zo: bool, more_walled: usize) -> Self {
        let began = clock();
        let dir = tempfile::tempdir().expect("a checkout");
        let checkout: &'static str =
            Box::leak(dir.path().to_string_lossy().into_owned().into_boxed_str());
        let gauges = |claude_used: u8| {
            let mut snapshot = usage_snapshot(
                "claude",
                Some((claude_used, Some(began + 3 * 60 * 60_000))),
                Some((40, None)),
                began - 60_000,
            );
            snapshot.account = Some("a-fixture".to_string());
            vec![("claude", snapshot)]
        };
        let (window, store) = PrivateWindow::boot_with_usage(gauges(40));
        let beat = one_beat_at_a_time();
        let team = format!("team-switch-{leader_term}");
        seat_a_team(&team, leader_term);
        let host = Switching::new(checkout, leader_term + 1);
        let world = SwitchWorld::new(
            &host,
            zerocode_core::account_autoswitch::AutoSwitchMode::Auto,
        );
        let leader = zerocode_core::agent_teams::LEADER_PANE;
        let verb = |line: &str, at: i64| {
            let said = run(
                &host,
                Vec::new(),
                &team,
                leader,
                TEST_CAPABILITY,
                &words(line),
                at,
            );
            assert_eq!(said.exit_code, 0, "`{line}`: {}", said.stderr);
            serde_json::from_str::<serde_json::Value>(&said.stdout).expect("json")
        };
        verb("run-create --name switching", began);
        let task = verb("task-create --spec keep-going", began + 1)["taskId"]
            .as_str()
            .expect("a task")
            .to_string();
        let started = verb(
            &format!(
                "worker-start --agent claude --task {task} --model claude-fable-5-1 --effort max"
            ),
            began + 2,
        );
        let worker = started["workerId"].as_str().expect("a worker").to_string();
        let dispatch = started["dispatchId"]
            .as_str()
            .expect("a dispatch")
            .to_string();
        let transcript = a_switch_transcript(dir.path());
        host.in_conversation(leader_term + 1, SWITCH_SESSION, &transcript);
        super::pane_session_reported(
            leader_term + 1,
            &host.provider_session(leader_term + 1).expect("a session"),
            began + 3,
        );
        // Summoned while the gauge still had room: at 98% the summons gate
        // itself refuses a new claude worker.
        let mut others = Vec::new();
        for at in 0..working {
            let term = leader_term + 5 + u32::try_from(at).expect("a small count");
            host.seating_onto(term);
            host.working(term);
            let other = verb(
                &format!("task-create --spec keep-working-{at}"),
                began + 4 + i64::try_from(at).expect("a small count"),
            )["taskId"]
                .as_str()
                .expect("a task")
                .to_string();
            let id = verb(
                &format!("worker-start --agent claude --task {other}"),
                began + 40 + i64::try_from(at).expect("a small count"),
            )["workerId"]
                .as_str()
                .expect("a worker")
                .to_string();
            others.push((id, term));
        }
        let zo = zo.then(|| {
            let term = leader_term + 4;
            host.seating_onto(term);
            let task = verb("task-create --spec zo-keeps-going", began + 60)["taskId"]
                .as_str()
                .expect("a task")
                .to_string();
            let id = verb(
                &format!("worker-start --agent zo --task {task} --model claude-opus-5-5"),
                began + 61,
            )["workerId"]
                .as_str()
                .expect("a zo worker")
                .to_string();
            // At the same wall as the Claude pane: zo's quota is its model's.
            host.screen_says(term, "You've hit your limit · resets 3am");
            (id, term)
        });
        let mut walled_more = Vec::new();
        for at in 0..more_walled {
            let step = u32::try_from(at).expect("a small count");
            let term = leader_term + 20 + step;
            host.seating_onto(term);
            let other = verb(
                &format!("task-create --spec walled-too-{at}"),
                began + 70 + i64::from(step),
            )["taskId"]
                .as_str()
                .expect("a task")
                .to_string();
            let id = verb(
                &format!("worker-start --agent claude --task {other}"),
                began + 80 + i64::from(step),
            )["workerId"]
                .as_str()
                .expect("a worker")
                .to_string();
            let session = format!("5a1e0000-0000-4000-8000-00000000{:04}", 7600 + step);
            let path = dir.path().join(format!("{session}.jsonl"));
            std::fs::copy(&transcript, &path).expect("a transcript of its own");
            host.in_conversation(term, &session, &path);
            super::pane_session_reported(
                term,
                &host.provider_session(term).expect("a session"),
                began + 90 + i64::from(step),
            );
            host.screen_says(term, "You've hit your limit · resets 3am");
            walled_more.push((id, term));
        }
        host.screen_says(leader_term + 1, "You've hit your limit · resets 3am");
        window.set_usage(gauges(98));
        notify_stalled_workers(&host, began + 10_000);
        world.reads(98, 10, began + 10_000);
        // The summonses above were the fixture's own splits; what a test
        // counts is what the switch does from here on.
        host.splits.lock().unwrap().clear();
        host.asks.lock().unwrap().clear();
        Self {
            _window: window,
            store,
            _beat: beat,
            _dir: dir,
            host,
            world,
            leader: leader_term,
            team,
            task,
            worker,
            dispatch,
            transcript,
            working: others,
            zo,
            walled_more,
            began,
        }
    }

    fn json(&self, line: &str, at: i64) -> serde_json::Value {
        let said = run(
            &self.host,
            Vec::new(),
            &self.team,
            zerocode_core::agent_teams::LEADER_PANE,
            TEST_CAPABILITY,
            &words(line),
            at,
        );
        assert_eq!(said.exit_code, 0, "`{line}`: {}", said.stderr);
        serde_json::from_str(&said.stdout).expect("json")
    }

    fn row(&self, id: &str) -> zerocode_core::orchestration::WorkerRow {
        the_rows()
            .workers
            .into_iter()
            .find(|one| one.id == id)
            .expect("the worker")
    }

    fn died(&self, at: i64) -> u64 {
        self.json("check --peek --types worker_died", at)["count"]
            .as_u64()
            .expect("a count")
    }

    fn plan(&self, at: i64) -> crate::account_switch::SwitchPlan {
        use crate::account_switch::SwitchDoors as _;
        crate::account_switch::plan_with(&self.host, self.world.situation(at).expect("a situation"))
    }

    /// The ledger's disk refuses every write from now — the trigger
    /// `a_disk_that_refuses_a_write_still_answers_a_look_from_the_disks_word`
    /// puts on the revision head — until [`Self::disk_takes_writes`].
    fn disk_refuses_writes(&self) {
        self.store
            .fault_connection_for_tests()
            .expect("a fault connection")
            .execute_batch(REFUSE_LEDGER_WRITES)
            .expect("a disk that refuses");
    }

    fn disk_takes_writes(&self) {
        self.store
            .fault_connection_for_tests()
            .expect("a fault connection")
            .execute_batch("DROP TRIGGER refuse_writes;")
            .expect("the disk comes back");
    }

    /// The disk starts refusing at the switch's first receipt: every effect
    /// before it — the selection, the rest, the close, the new pane — has
    /// landed, and the receipts meet a ledger that will not take them.
    fn disk_refuses_from_the_first_receipt(&self) {
        let fault = self
            .store
            .fault_connection_for_tests()
            .expect("a fault connection");
        *self.world.before_receipt.lock().unwrap() = Some(Box::new(move || {
            fault
                .execute_batch(REFUSE_LEDGER_WRITES)
                .expect("a disk that refuses");
        }));
    }

    fn journal_text(&self) -> String {
        std::fs::read_to_string(
            self.world
                .journal
                .path()
                .join(crate::app_paths::artifact_file::CLAUDE_ACCOUNT_SWITCH),
        )
        .unwrap_or_default()
    }

    /// The disk and the switch's journal both start refusing at the
    /// switch's first receipt (astra R4-1): what the journal took before it
    /// stays on disk as it was, and the replace that writes the book after
    /// the receipts meets a journal that takes nothing, until
    /// [`Self::journal_takes_writes`].
    fn disk_and_journal_refuse_from_the_first_receipt(&self) {
        let fault = self
            .store
            .fault_connection_for_tests()
            .expect("a fault connection");
        let journal = self.world.journal.path().to_path_buf();
        *self.world.before_receipt.lock().unwrap() = Some(Box::new(move || {
            fault
                .execute_batch(REFUSE_LEDGER_WRITES)
                .expect("a disk that refuses");
            journal_refuses_writes_in(&journal);
        }));
    }

    /// The journal takes nothing from the old pane's close on (astra R4-1):
    /// the rest is on disk, and whatever the move adds lives only in this
    /// window's memory until [`Self::journal_takes_writes`].
    fn journal_refuses_from_the_close(&self) {
        let journal = self.world.journal.path().to_path_buf();
        *self.host.at_the_close.lock().unwrap() =
            Some(Box::new(move || journal_refuses_writes_in(&journal)));
    }

    /// The journal takes writes again, holding what it held when it stopped.
    fn journal_takes_writes(&self) {
        journal_takes_writes_in(self.world.journal.path());
    }

    fn receipts(&self, at: i64) -> Vec<serde_json::Value> {
        self.json("check --peek --types account_switched", at)["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|told| serde_json::from_str(told["body"].as_str().expect("a body")).expect("json"))
            .collect()
    }
}

/// The trigger a test puts on the ledger's revision head so every write
/// is refused, as a full or read-only disk refuses one.
const REFUSE_LEDGER_WRITES: &str = "CREATE TRIGGER refuse_writes
    AFTER UPDATE OF revision ON orchestration_ledger_heads
    BEGIN SELECT RAISE(ABORT, 'injected disk refusal'); END;";

impl Drop for WalledClaude {
    fn drop(&mut self) {
        // A move that stopped halfway leaves its mark and its hold for the
        // restore road; the next test's worker of the same id is not that
        // worker.
        super::forget_switch_mark(&self.worker);
        for (worker, _) in &self.walled_more {
            super::forget_switch_mark(worker);
        }
        for term in [
            self.leader,
            self.leader + 1,
            self.leader + 2,
            self.leader + 3,
            self.leader + 4,
        ]
        .into_iter()
        .chain(self.working.iter().map(|(_, term)| *term))
        .chain(self.walled_more.iter().map(|(_, term)| *term))
        {
            crate::agent_teams::forget_term(term);
        }
    }
}

/// The RED this closes, at the window's own boundary: the old road — the
/// account handoff's `close_term` of every Claude pane whose turn ended —
/// reached the ledger as a death for each (2026-09-24 21:2x, five of them,
/// 함정 410). Replayed here with today's five working panes whose turns end
/// after a person's switch: five closes, five `worker_died`, five spent
/// attempts.
#[test]
fn the_old_handoff_road_killed_every_pane_it_closed() {
    let stood = WalledClaude::stand(87_000, 5);
    let at = stood.began + 20_000;
    for (at_step, (_, term)) in (0_i64..).zip(&stood.working) {
        terminal_gone(*term, at + at_step);
    }
    assert_eq!(stood.died(at + 10), 5, "the old road's five deaths");
    for (id, _) in &stood.working {
        assert_ne!(stood.row(id).state, WorkerState::Active);
    }
}

/// Today's incident, on the road as it stands: five Claude workers busy in
/// their panes (one of them later at rest), a walled one beside them, and
/// the PERSON switches the account. Every pane keeps its login, nobody is
/// closed or launched, nobody dies, and the switch leaves one receipt in
/// the ledger's voice, `by: person`, zero panes moved. The beats after it
/// — the stall sweep, the grace — move nothing either.
#[test]
fn a_manual_switch_keeps_working_panes_too() {
    let stood = WalledClaude::stand(87_100, 5);
    let at = stood.began + 20_000;
    let before: Vec<(String, String)> = stood
        .working
        .iter()
        .map(|(id, _)| (id.clone(), stood.row(id).pane))
        .collect();
    let applied =
        crate::account_switch::switch_by_person(&stood.host, &stood.world, Some("b-fixture"), at)
            .expect("a person's switch");
    assert!(applied.switched_default);
    assert!(applied.panes.is_empty(), "a person's pick moves no pane");
    assert_eq!(stood.world.selects(), vec![Some("b-fixture".to_string())]);
    // One of the working turns ends — the old road's drain point — and the
    // beat runs over all of it.
    stood.host.busy.lock().unwrap().remove(&stood.working[0].1);
    notify_stalled_workers(&stood.host, at + 1_000);
    assert!(stood.host.closed().is_empty(), "{:?}", stood.host.closed());
    assert!(stood.host.splits.lock().unwrap().is_empty());
    assert_eq!(stood.died(at + 2_000), 0);
    for (id, pane) in &before {
        let row = stood.row(id);
        assert_eq!(row.state, WorkerState::Active, "{id}");
        assert_eq!(&row.pane, pane, "{id} moved");
    }
    // The walled worker is not the person's pick's to move either.
    assert_eq!(stood.row(&stood.worker).state, WorkerState::Active);
    let mail = stood.json("check --peek --types account_switched", at + 3_000);
    assert_eq!(mail["count"], 1, "{mail}");
    let body: serde_json::Value =
        serde_json::from_str(mail["messages"][0]["body"].as_str().expect("a body")).expect("json");
    assert_eq!(body["by"], "person");
    assert_eq!(body["reason"], "picked");
    assert_eq!(body["moved"], "default");
    assert_eq!(body["fromAccount"], "a-fixture");
    assert_eq!(body["toAccount"], "b-fixture");
    assert_eq!(body["panesMoved"], 0);
    assert!(!mail.to_string().contains('@'));
    // The journal was written and taken away.
    assert!(
        std::fs::read_dir(stood.world.journal.path())
            .expect("the journal dir")
            .next()
            .is_none()
    );
}

/// GREEN of the red above, for the one pane that SHOULD move: under
/// `auto`, the walled worker is rested, its pane closed, and the SAME
/// worker id seated again — same dispatch, same task, no attempt spent —
/// in a pane that runs as the new account; the working panes beside it are
/// not touched, and the receipts are one per effect.
#[test]
fn an_auto_switch_relaunches_no_working_pane_and_reseats_only_the_walled_one() {
    let stood = WalledClaude::stand(87_200, 2);
    let at = stood.began + 20_000;
    let plan = stood.plan(at);
    assert!(
        matches!(
            plan.decision,
            zerocode_core::account_autoswitch::Decision::Switch { ref to, .. } if to == "b-fixture"
        ),
        "{plan:?}"
    );
    assert_eq!(plan.walled.len(), 1, "{:?}", plan.walled);
    assert_eq!(plan.walled[0].worker, stood.worker);
    stood.host.seating_onto(stood.leader + 2);
    let began = std::time::Instant::now();
    let applied =
        crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
            .expect("applied");
    let ms = began.elapsed().as_millis();
    // First the ledger's own word on the closed pane: the close reached it
    // (the host's close walks `terminal_gone`, as the window's does) and it
    // settled nothing.
    assert_eq!(
        stood.died(at + 10),
        0,
        "the switch road's close was a death: {:?}",
        applied.panes
    );
    assert!(applied.switched_default);
    assert_eq!(applied.panes.len(), 1, "{:?}", applied.panes);
    let pane = &applied.panes[0];
    assert!(pane.ok, "{pane:?}");
    assert_eq!(pane.to_term, Some(stood.leader + 2));
    assert_eq!(pane.to_account.as_deref(), Some("b-fixture"));
    assert_eq!(stood.host.closed(), vec![stood.leader + 1]);
    let row = stood.row(&stood.worker);
    assert_eq!(row.state, WorkerState::Active);
    assert_eq!(row.dispatch.as_deref(), Some(stood.dispatch.as_str()));
    let rows = the_rows();
    assert_eq!(
        rows.workers
            .iter()
            .filter(|one| one.id == stood.worker)
            .count(),
        1,
        "the switch made a second row"
    );
    let task = rows
        .tasks
        .iter()
        .find(|one| one.id == stood.task)
        .expect("the task");
    assert_eq!(
        task.status,
        zerocode_core::orchestration::TaskStatus::Dispatched
    );
    assert_eq!(task.failures, 0);
    for (id, _) in &stood.working {
        assert_eq!(stood.row(id).state, WorkerState::Active, "{id}");
    }
    // One receipt per effect: the default and the pane, in the ledger's
    // voice, ids only.
    let mail = stood.json("check --peek --types account_switched", at + 20);
    assert_eq!(mail["count"], 2, "{mail}");
    let bodies: Vec<serde_json::Value> = mail["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|told| serde_json::from_str(told["body"].as_str().expect("a body")).expect("json"))
        .collect();
    let default = bodies
        .iter()
        .find(|body| body["moved"] == "default")
        .expect("the default's receipt");
    assert_eq!(default["by"], "auto");
    assert_eq!(default["reason"], "walled");
    assert_eq!(default["panesMoved"], 1);
    let moved = bodies
        .iter()
        .find(|body| body["moved"] == "pane")
        .expect("the pane's receipt");
    assert_eq!(moved["workerId"], stood.worker);
    assert_eq!(moved["fromAccount"], "a-fixture");
    assert_eq!(moved["toAccount"], "b-fixture");
    assert!(!mail.to_string().contains('@'));
    // The same token again is a plan that no longer stands.
    assert!(
        crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at + 30)
            .is_err()
    );
    assert_eq!(
        stood.host.closed().len(),
        1,
        "a second apply closed something"
    );
    eprintln!(
        "ACCOUNT_SWITCH_RESEAT plan→rest→close→reseat→receipts={ms}ms (default {}ms)",
        applied.default_ms
    );
}

/// Condition 6 (22:1x): the pane's conversation and its settings survive
/// the move. The replacement resumes the SAME session, on the model and
/// effort the pane really ran — the person moved it to Opus at xhigh; the
/// summons said Fable at max — in the same checkout, told once what
/// happened; the transcript the switch read is not written by it.
#[test]
fn a_switch_keeps_the_panes_conversation_and_its_effort() {
    let stood = WalledClaude::stand(87_300, 0);
    let at = stood.began + 20_000;
    let before = digest_of(&stood.transcript);
    let plan = stood.plan(at);
    stood.host.seating_onto(stood.leader + 2);
    let applied =
        crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
            .expect("applied");
    assert!(applied.panes.iter().all(|pane| pane.ok), "{applied:?}");
    let splits = stood.host.splits.lock().unwrap().clone();
    assert_eq!(splits.len(), 1, "{splits:?}");
    let argv = split_command_line(&splits[0].1).expect("an argv");
    let after = |flag: &str| {
        argv.iter()
            .position(|word| word == flag)
            .and_then(|at| argv.get(at + 1))
            .cloned()
    };
    assert_eq!(
        after("--resume").as_deref(),
        Some(SWITCH_SESSION),
        "{argv:?}"
    );
    assert_eq!(
        after("--model").as_deref(),
        Some("claude-opus-5-5"),
        "{argv:?}"
    );
    assert_eq!(after("--effort").as_deref(), Some("xhigh"), "{argv:?}");
    assert!(
        argv.iter()
            .any(|word| word == "--dangerously-skip-permissions"),
        "the pane ran bypassPermissions: {argv:?}"
    );
    let asks = stood.host.asks.lock().unwrap().clone();
    assert_eq!(asks.len(), 1, "told more than once: {asks:?}");
    assert!(asks[0].1.contains("b-fixture"), "{}", asks[0].1);
    assert_eq!(
        asks[0].2,
        Some(zerocode_core::orchestration::WorkerResume::Session)
    );
    let row = stood.row(&stood.worker);
    assert_eq!(row.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(row.effort.as_deref(), Some("xhigh"));
    assert_eq!(
        row.session.as_ref().map(|held| held.id.as_str()),
        Some(SWITCH_SESSION)
    );
    assert_eq!(
        digest_of(&stood.transcript),
        before,
        "the switch wrote the transcript"
    );
    eprintln!("ACCOUNT_SWITCH_TRANSCRIPT prefix={before} unchanged");
}

/// zo on a Claude model follows the window's login where it stands (t-5777:
/// `auth.reload` over its channel, the selection's follower) — so a switch
/// closes no zo pane and relaunches none, even one at the same wall: its
/// conversation, model and seat are the ones it had, and only the Claude
/// pane beside it is moved.
#[test]
fn a_zo_pane_follows_the_switch_where_it_stands() {
    let stood = WalledClaude::stand_with(88_000, 0, true);
    let (zo, zo_term) = stood.zo.clone().expect("the zo pane");
    let before = stood.row(&zo);
    let at = stood.began + 20_000;
    let plan = stood.plan(at);
    assert!(
        plan.walled.iter().all(|row| row.worker != zo),
        "a zo pane was put on the moving list: {:?}",
        plan.walled
    );
    stood.host.seating_onto(stood.leader + 2);
    let applied =
        crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
            .expect("applied");
    assert!(applied.switched_default);
    assert_eq!(*stood.world.followed.lock().unwrap(), 1, "zo's reload door");
    assert!(!stood.host.closed().contains(&zo_term));
    let after = stood.row(&zo);
    assert_eq!(after.state, WorkerState::Active);
    assert_eq!(after.pane, before.pane);
    assert_eq!(after.model, before.model);
    assert_eq!(after.session, before.session);
    assert_eq!(after.dispatch, before.dispatch);
    assert_eq!(stood.died(at + 10), 0);
}

/// Asking is free: planning over a walled pane, a pending proposal and a
/// journal-less window writes nothing — no ledger revision, no journal, no
/// pane touched — however often the bar asks (the quiet poll's mutation
/// count stays 0).
#[test]
fn a_plan_is_a_read_and_moves_nothing() {
    let stood = WalledClaude::stand(88_100, 2);
    let at = stood.began + 20_000;
    let revision = || {
        super::runtime()
            .expect("a runtime")
            .actor
            .view()
            .expect("the rows")
            .revision()
    };
    let before = revision();
    let mut tokens = std::collections::HashSet::new();
    for step in 0..50 {
        crate::account_switch::reconcile(&stood.host, &stood.world, at + step);
        tokens.insert(stood.plan(at).token);
    }
    assert_eq!(revision(), before, "a plan wrote the ledger");
    assert_eq!(tokens.len(), 1, "one situation, one token");
    assert!(stood.host.closed().is_empty());
    assert!(stood.host.splits.lock().unwrap().is_empty());
    assert!(stood.world.selects().is_empty());
    assert!(
        std::fs::read_dir(stood.world.journal.path())
            .expect("the journal dir")
            .next()
            .is_none()
    );
    // And once the wall no longer stands (its reset and the grace after it
    // have passed), nothing is on the moving list at all.
    let lifted = stood.plan(at + 4 * 60 * 60_000);
    assert!(lifted.walled.is_empty(), "{:?}", lifted.walled);
    assert_eq!(lifted.moves().count(), 0);
    eprintln!("ACCOUNT_SWITCH_QUIET_POLL plans=50 ledger_writes=0 journal_writes=0");
}

/// N walled panes in one switch: every one continues on the new account in
/// a pane of its own, one receipt each and one for the default; the total
/// time is the report's "재세움 총 시간" for N.
#[test]
fn every_walled_pane_of_one_switch_continues_with_its_own_receipt() {
    const MORE: usize = 2;
    let stood = WalledClaude::stand_full(88_200, 0, false, MORE);
    let at = stood.began + 20_000;
    let plan = stood.plan(at);
    assert_eq!(plan.moves().count(), 1 + MORE, "{:?}", plan.walled);
    stood.host.seating_onto(stood.leader + 100);
    let began = std::time::Instant::now();
    let applied =
        crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
            .expect("applied");
    let ms = began.elapsed().as_millis();
    assert_eq!(applied.panes.len(), 1 + MORE);
    assert!(applied.panes.iter().all(|pane| pane.ok), "{applied:?}");
    assert_eq!(stood.host.closed().len(), 1 + MORE);
    assert_eq!(stood.died(at + 10), 0);
    for (worker, _) in
        std::iter::once(&(stood.worker.clone(), stood.leader + 1)).chain(stood.walled_more.iter())
    {
        assert_eq!(stood.row(worker).state, WorkerState::Active, "{worker}");
    }
    assert_eq!(
        stood.json("check --peek --types account_switched", at + 20)["count"],
        u64::try_from(2 + MORE).expect("a small count")
    );
    let per_pane: Vec<u128> = applied.panes.iter().map(|pane| pane.ms).collect();
    eprintln!(
        "ACCOUNT_SWITCH_N_PANES n={} total={ms}ms per_pane={per_pane:?} default={}ms",
        1 + MORE,
        applied.default_ms
    );
    for (worker, _) in &stood.walled_more {
        super::forget_switch_mark(worker);
    }
}

/// A pane the switch cannot move faithfully is left exactly where it is:
/// a conversation the window cannot read (a restore would start it
/// empty), a pane the person put in another permission mode, a program
/// that outlives the close. Nothing is closed in the first two; the third
/// closed and says so, and the worker sleeps for the restore road with its
/// attempt open — never a `worker_died`.
#[test]
fn a_pane_that_cannot_be_moved_faithfully_stays() {
    // No conversation the window can read.
    {
        let stood = WalledClaude::stand(87_400, 0);
        stood.host.sessions.lock().unwrap().clear();
        let at = stood.began + 20_000;
        let plan = stood.plan(at);
        let applied =
            crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
                .expect("the default still moves");
        assert!(!applied.panes[0].ok);
        assert!(
            applied.panes[0]
                .why
                .as_deref()
                .unwrap_or_default()
                .contains("empty"),
            "{applied:?}"
        );
        assert!(stood.host.closed().is_empty());
        assert_eq!(stood.row(&stood.worker).state, WorkerState::Active);
        assert_eq!(stood.died(at + 10), 0);
    }
    // The person put the pane in plan mode; a relaunch would start it in
    // bypassPermissions.
    {
        let stood = WalledClaude::stand(87_500, 0);
        let mut text = std::fs::read_to_string(&stood.transcript).expect("the transcript");
        text.push_str(
            &serde_json::json!({"type": "permission-mode", "permissionMode": "plan"}).to_string(),
        );
        text.push('\n');
        std::fs::write(&stood.transcript, text).expect("rewritten");
        let at = stood.began + 20_000;
        let plan = stood.plan(at);
        let applied =
            crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
                .expect("applied");
        assert!(!applied.panes[0].ok);
        assert!(
            applied.panes[0]
                .why
                .as_deref()
                .unwrap_or_default()
                .contains("plan"),
            "{applied:?}"
        );
        assert!(stood.host.closed().is_empty());
        assert_eq!(stood.row(&stood.worker).state, WorkerState::Active);
    }
    // The old program outlives the wait: closed, not relaunched, asleep.
    {
        let stood = WalledClaude::stand(87_600, 0);
        *stood.host.lingering.lock().unwrap() = true;
        let at = stood.began + 20_000;
        let plan = stood.plan(at);
        let applied =
            crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
                .expect("applied");
        assert!(!applied.panes[0].ok);
        assert!(
            stood.host.splits.lock().unwrap().is_empty(),
            "a second writer"
        );
        assert_eq!(stood.row(&stood.worker).state, WorkerState::Sleeping);
        assert_eq!(stood.died(at + 10), 0);
        // The journal keeps the sleeper for the restore road, and says so.
        assert!(
            std::fs::read_to_string(
                stood
                    .world
                    .journal
                    .path()
                    .join(crate::app_paths::artifact_file::CLAUDE_ACCOUNT_SWITCH)
            )
            .expect("the journal stays")
            .contains(&stood.worker)
        );
    }
}

/// `off` does nothing and proposes nothing; `ask` does nothing until the
/// person's yes, and a yes to a plan that changed is refused; `auto` from
/// a stale page under `ask` is refused; after a switch the default rests
/// for the cooldown whatever the numbers say; a refused select is left out
/// of the next plans.
#[test]
fn off_does_nothing_ask_waits_for_a_yes_and_a_cooldown_holds() {
    use zerocode_core::account_autoswitch::{AutoSwitchMode, Decision};
    let stood = WalledClaude::stand(87_700, 0);
    let at = stood.began + 20_000;
    *stood.world.mode.lock().unwrap() = AutoSwitchMode::Off;
    let off = stood.plan(at);
    assert_eq!(off.decision, Decision::Stay { why: "off" });
    assert!(!off.acts());
    assert!(
        crate::account_switch::apply_with(&stood.host, &stood.world, &off.token, "ask", at)
            .is_err()
    );
    *stood.world.mode.lock().unwrap() = AutoSwitchMode::Ask;
    let asked = stood.plan(at);
    assert!(asked.acts());
    assert!(
        crate::account_switch::apply_with(&stood.host, &stood.world, &asked.token, "auto", at)
            .is_err(),
        "the beat applied an `ask` plan by itself"
    );
    // The situation moves before the yes — B fills past the blocked line,
    // so the table has nowhere to go: the yes was for another plan.
    stood.world.reads(98, 98, at);
    assert!(
        crate::account_switch::apply_with(&stood.host, &stood.world, &asked.token, "ask", at)
            .is_err()
    );
    assert!(stood.world.selects().is_empty());
    assert!(stood.host.closed().is_empty());
    stood.world.reads(98, 10, at);
    // A refused select: nothing moves, and B is left out for one cooldown.
    *stood.world.refuse.lock().unwrap() =
        Some("선택한 Claude 계정(b-fixture)의 로그인이 만료되었습니다".into());
    let fresh = stood.plan(at);
    assert!(
        crate::account_switch::apply_with(&stood.host, &stood.world, &fresh.token, "ask", at)
            .is_err()
    );
    assert!(stood.host.closed().is_empty());
    assert_eq!(stood.row(&stood.worker).state, WorkerState::Active);
    let after_refusal = stood.plan(at + 1);
    assert_eq!(after_refusal.failed_recently, vec!["b-fixture".to_string()]);
    assert!(!matches!(after_refusal.decision, Decision::Switch { .. }));
    // The person's yes to the plan as it stands NOW: the default moves and
    // the walled pane continues on it.
    *stood.world.refuse.lock().unwrap() = None;
    stood.world.failures.lock().unwrap().clear();
    stood.world.reads(98, 10, at);
    let yes = stood.plan(at + 2);
    stood.host.seating_onto(stood.leader + 2);
    let applied =
        crate::account_switch::apply_with(&stood.host, &stood.world, &yes.token, "ask", at + 2)
            .expect("the person's yes");
    assert!(applied.switched_default);
    assert!(applied.panes.iter().all(|pane| pane.ok), "{applied:?}");
    assert_eq!(stood.world.selects(), vec![Some("b-fixture".to_string())]);
    let mail = stood.json("check --peek --types account_switched", at + 3);
    assert!(
        mail["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .all(|told| told["body"]
                .as_str()
                .is_some_and(|body| body.contains("\"by\":\"ask\""))),
        "{mail}"
    );
    // The cooldown: the switch a moment ago holds the default still,
    // whatever the numbers say.
    stood.world.reads(10, 95, at + 60_000);
    *stood.world.selected.lock().unwrap() = Some("b-fixture".to_string());
    assert_eq!(
        stood.plan(at + 60_000).decision,
        Decision::Stay { why: "cooldown" }
    );
}

/// A window that dies between an effect and its receipt finishes the
/// receipt on its next look — once — and never the effect: a default that
/// landed gets its row; a select that never landed owes nothing; the
/// first round's one-switch file is read as a book of one; the journal
/// goes once nothing is owed (astra B2).
#[test]
fn a_switch_that_died_halfway_finishes_its_receipts_once() {
    let stood = WalledClaude::stand(87_800, 0);
    let at = stood.began + 20_000;
    let journal_path = stood
        .world
        .journal
        .path()
        .join(crate::app_paths::artifact_file::CLAUDE_ACCOUNT_SWITCH);
    // As if the window died right after the select: the store names B, the
    // journal says a default was owed, no receipt was written. Written in
    // the first round's shape — one switch, not a book.
    let journal = crate::account_switch::Journal {
        key: "switch-halfway".to_string(),
        by: "auto".to_string(),
        reason: "near_limit".to_string(),
        from: Some("a-fixture".to_string()),
        to: Some("b-fixture".to_string()),
        default: true,
        landed: false,
        observed_percent: Some(95),
        observed_window: Some("session".to_string()),
        generation: None,
        panes: Vec::new(),
        moved: 0,
        began_ms: at,
    };
    std::fs::write(
        &journal_path,
        serde_json::to_string(&journal).expect("json"),
    )
    .expect("the journal");
    *stood.host.selected.lock().unwrap() = Some("b-fixture".to_string());
    assert_eq!(
        crate::account_switch::reconcile(&stood.host, &stood.world, at + 1),
        1
    );
    assert_eq!(
        crate::account_switch::reconcile(&stood.host, &stood.world, at + 2),
        0,
        "the journal was read twice"
    );
    assert!(!journal_path.exists(), "a settled journal stayed");
    let mail = stood.json("check --peek --types account_switched", at + 3);
    assert_eq!(mail["count"], 1, "{mail}");
    // A select that never landed owes nothing.
    std::fs::write(
        &journal_path,
        serde_json::to_string(&crate::account_switch::JournalBook {
            switches: vec![crate::account_switch::Journal {
                key: "switch-never-landed".to_string(),
                to: Some("c-fixture".to_string()),
                ..journal
            }],
        })
        .expect("json"),
    )
    .expect("the journal");
    assert_eq!(
        crate::account_switch::reconcile(&stood.host, &stood.world, at + 4),
        0
    );
    assert_eq!(
        stood.json("check --peek --types account_switched", at + 5)["count"],
        1
    );
    assert!(!journal_path.exists());
}

/// Two applies of one plan at once — two windows, or a press and a beat —
/// make one default move and one move per pane; the second is told the
/// plan changed (astra B1).
#[test]
fn two_applies_of_one_plan_move_once() {
    let stood = WalledClaude::stand(87_900, 0);
    let at = stood.began + 20_000;
    let plan = stood.plan(at);
    stood.host.seating_onto(stood.leader + 2);
    let results: Vec<Result<crate::account_switch::Applied, String>> =
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                crate::account_switch::apply_with(
                    &stood.host,
                    &stood.world,
                    &plan.token,
                    "auto",
                    at,
                )
            });
            let second = scope.spawn(|| {
                crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "ask", at)
            });
            vec![
                first.join().expect("the first apply"),
                second.join().expect("the second apply"),
            ]
        });
    assert_eq!(
        results.iter().filter(|result| result.is_ok()).count(),
        1,
        "{results:?}"
    );
    assert_eq!(stood.world.selects().len(), 1);
    assert_eq!(stood.host.closed().len(), 1);
    assert_eq!(stood.host.splits.lock().unwrap().len(), 1);
    assert_eq!(
        stood.json("check --peek --types account_switched", at + 10)["count"],
        2
    );
}

/// A `quota_walled` row says the two witnesses met ONCE (astra R2). A
/// worker that went back to work after its wall — its pane busy again, or
/// its own words past the wall — keeps that row until the wall's time runs
/// out, and the old plan listed it and moved it on the row alone. Now the
/// plan asks the pane now: busy with the words still on screen, quiet with
/// the words gone, or both, the worker is not on the moving list; the
/// default still moves on its own numbers, and nothing is rested, closed
/// or seated. And a pane that goes back to work WHILE the default is being
/// selected is not moved either: its last door reads the plan again. The
/// same wall still standing moves — the green road above.
#[test]
fn a_pane_back_at_work_after_its_wall_is_not_moved_by_the_wall_it_left() {
    let unmoved = |stood: &WalledClaude, seat: &str, case: &str, at: i64| {
        assert!(
            stood.host.closed().is_empty(),
            "{case}: {:?}",
            stood.host.closed()
        );
        assert!(stood.host.splits.lock().unwrap().is_empty(), "{case}");
        let row = stood.row(&stood.worker);
        assert_eq!(row.state, WorkerState::Active, "{case}");
        assert_eq!(row.pane, seat, "{case}");
        assert_eq!(row.model.as_deref(), Some("claude-fable-5-1"), "{case}");
        assert_eq!(stood.died(at + 10), 0, "{case}");
    };
    for (leader, busy, words_gone) in [
        (89_000, true, true),
        (89_100, true, false),
        (89_200, false, true),
    ] {
        let stood = WalledClaude::stand(leader, 0);
        let at = stood.began + 20_000;
        let walled_term = stood.leader + 1;
        let seat = stood.row(&stood.worker).pane;
        if busy {
            stood.host.working(walled_term);
        }
        if words_gone {
            stood.host.markers.lock().unwrap().remove(&walled_term);
        }
        let case = format!("busy={busy} words_gone={words_gone}");
        let plan = stood.plan(at);
        assert!(plan.walled.is_empty(), "{case}: {:?}", plan.walled);
        stood.host.seating_onto(stood.leader + 2);
        let applied =
            crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
                .expect("the default moves on its own numbers");
        assert!(applied.switched_default, "{case}: {applied:?}");
        assert!(applied.panes.is_empty(), "{case}: {applied:?}");
        unmoved(&stood, &seat, &case, at);
    }
    // Back to work while the default is being selected.
    let stood = WalledClaude::stand(89_250, 0);
    let at = stood.began + 20_000;
    let walled_term = stood.leader + 1;
    let seat = stood.row(&stood.worker).pane;
    let plan = stood.plan(at);
    assert_eq!(plan.moves().count(), 1, "{:?}", plan.walled);
    let busy = std::sync::Arc::clone(&stood.host.busy);
    *stood.world.after_select.lock().unwrap() = Some(Box::new(move |_: &SwitchWorld| {
        busy.lock().unwrap().insert(walled_term);
    }));
    stood.host.seating_onto(stood.leader + 2);
    let applied =
        crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
            .expect("the default moved");
    assert!(!applied.panes[0].ok, "{applied:?}");
    assert!(
        applied.panes[0]
            .why
            .as_deref()
            .unwrap_or_default()
            .contains("no longer sends"),
        "{applied:?}"
    );
    unmoved(&stood, &seat, "back to work during the selection", at);
}

/// The rest itself — at the actor and the host, the last door of all (astra
/// R2). The ledger rests a walled worker only at the fence, on the window's
/// CURRENT observation: a pane busy again, words that left the screen, or
/// its account's number that fell back under the wall is refused there, and
/// so is an approval for another attempt or another coordinator generation
/// — each time with the worker left exactly where it is. The approval as it
/// stands, on a wall that still stands, rests it with the pane's tuning.
#[test]
fn a_rest_is_written_only_while_the_window_sees_the_wall_now() {
    let stood = WalledClaude::stand(89_260, 0);
    let at = stood.began + 20_000;
    let walled_term = stood.leader + 1;
    let row = stood.row(&stood.worker);
    let approved = zerocode_core::orchestration::SwitchRest {
        worker: stood.worker.clone(),
        dispatch: stood.dispatch.clone(),
        generation: super::worker_now(&stood.worker).and_then(|now| now.generation),
        session: SWITCH_SESSION.to_string(),
        model: "claude-opus-5-5".to_string(),
        effort: "xhigh".to_string(),
        exit: None,
    };
    let as_a = stood
        .host
        .pane_login(walled_term)
        .expect("the pane's login");
    let rest = |approval: &zerocode_core::orchestration::SwitchRest| {
        super::rest_worker_for_switch(&stood.host, approval, &as_a, &|| Ok(()), &|| at, at)
    };
    let untouched = |why: &str| {
        let now = stood.row(&stood.worker);
        assert_eq!(now.state, WorkerState::Active, "{why}");
        assert_eq!(now.model, row.model, "{why}");
        assert_eq!(now.effort, row.effort, "{why}");
    };
    // Busy again.
    stood.host.working(walled_term);
    let refused = rest(&approved).unwrap_err();
    assert!(refused.contains("not at its wall now"), "{refused}");
    untouched(&refused);
    stood.host.busy.lock().unwrap().clear();
    // Quiet, but its words left the screen.
    let words = stood
        .host
        .markers
        .lock()
        .unwrap()
        .remove(&walled_term)
        .expect("the wall words");
    let refused = rest(&approved).unwrap_err();
    assert!(refused.contains("not at its wall now"), "{refused}");
    untouched(&refused);
    stood
        .host
        .markers
        .lock()
        .unwrap()
        .insert(walled_term, words);
    // Words on screen, but its account's number fell back under the wall.
    let gauges = |used: u8| {
        let mut snapshot = usage_snapshot(
            "claude",
            Some((used, Some(stood.began + 3 * 60 * 60_000))),
            Some((40, None)),
            stood.began - 60_000,
        );
        snapshot.account = Some("a-fixture".to_string());
        vec![("claude", snapshot)]
    };
    stood._window.set_usage(gauges(40));
    let refused = rest(&approved).unwrap_err();
    assert!(refused.contains("not at its wall now"), "{refused}");
    untouched(&refused);
    stood._window.set_usage(gauges(98));
    // Another attempt, another coordinator generation.
    for (other, said) in [
        (
            zerocode_core::orchestration::SwitchRest {
                dispatch: "dp-another-attempt".to_string(),
                ..approved.clone()
            },
            "approved for",
        ),
        (
            zerocode_core::orchestration::SwitchRest {
                generation: Some(approved.generation.map_or(1, |at| at + 1)),
                ..approved.clone()
            },
            "coordinator",
        ),
    ] {
        let refused = rest(&other).unwrap_err();
        assert!(refused.contains(said), "{refused}");
        untouched(&refused);
    }
    // The approval as it stands, on a wall that still stands.
    rest(&approved).expect("rested");
    let rested = stood.row(&stood.worker);
    assert_eq!(rested.state, WorkerState::Sleeping);
    assert_eq!(rested.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(rested.effort.as_deref(), Some("xhigh"));
    assert!(stood.host.closed().is_empty(), "the rest closes nothing");
    assert_eq!(stood.died(at + 10), 0);
}

/// The selection and the checks before a pane moves take time, and the
/// world moves under them (astra R2): the person turns the switch off, or
/// the landing fills past the blocked line, between the plan and the pane.
/// The pane is moved only by the policy as it stands at its last door —
/// read again there, the plan no longer sends it anywhere — so nothing is
/// rested, closed or seated. The default had already moved; its receipt
/// says so, and says no pane moved with it.
#[test]
fn a_pane_moves_only_by_the_policy_as_it_stands_at_its_last_door() {
    use zerocode_core::account_autoswitch::AutoSwitchMode;
    let cases: [(u32, &str, WorldMove); 2] = [
        (
            89_300,
            "turned off",
            Box::new(|world: &SwitchWorld| {
                *world.mode.lock().unwrap() = AutoSwitchMode::Off;
            }),
        ),
        (
            89_400,
            "no longer sends",
            Box::new(|world: &SwitchWorld| {
                let now = *world.now.lock().unwrap();
                world.reads(98, 98, now);
            }),
        ),
    ];
    for (leader, said, moving) in cases {
        let stood = WalledClaude::stand(leader, 0);
        let at = stood.began + 20_000;
        let seat = stood.row(&stood.worker).pane;
        let plan = stood.plan(at);
        assert_eq!(plan.moves().count(), 1, "{:?}", plan.walled);
        *stood.world.after_select.lock().unwrap() = Some(moving);
        stood.host.seating_onto(stood.leader + 2);
        let applied =
            crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
                .expect("the default moved");
        assert!(applied.switched_default, "{applied:?}");
        assert!(!applied.panes[0].ok, "{applied:?}");
        assert!(
            applied.panes[0]
                .why
                .as_deref()
                .unwrap_or_default()
                .contains(said),
            "{applied:?}"
        );
        assert!(stood.host.closed().is_empty(), "{:?}", stood.host.closed());
        assert!(stood.host.splits.lock().unwrap().is_empty());
        let row = stood.row(&stood.worker);
        assert_eq!(row.state, WorkerState::Active);
        assert_eq!(row.pane, seat);
        let bodies = stood.receipts(at + 10);
        assert_eq!(bodies.len(), 1, "{bodies:?}");
        assert_eq!(bodies[0]["moved"], "default");
        assert_eq!(bodies[0]["panesMoved"], 0);
        assert_eq!(bodies[0]["panesPending"], 0);
        assert_eq!(stood.died(at + 20), 0);
    }
}

/// A pane whose program outlives its close (astra R3): the switch rested
/// the worker and closed the pane, and the process group is still there
/// after the wait. No restore opens the conversation beside it — not the
/// coordinator's own restore, not the grace's, not a window that boots
/// again — and the grace does not end the worker either: its attempt stays
/// open. The program is held on the ledger's own row, written by the rest
/// before the close, so the boot holds it with no journal to read. Once a
/// look sees the program gone, the next restore seats the SAME worker on
/// the SAME attempt, once, and the switch's pane receipt follows once.
#[test]
fn a_program_that_outlived_its_close_holds_every_restore_until_it_is_gone() {
    let stood = WalledClaude::stand(89_500, 0);
    let at = stood.began + 20_000;
    *stood.host.lingering.lock().unwrap() = true;
    let plan = stood.plan(at);
    stood.host.seating_onto(stood.leader + 2);
    let applied =
        crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
            .expect("applied");
    assert!(!applied.panes[0].ok, "{applied:?}");
    assert_eq!(stood.host.closed(), vec![stood.leader + 1]);
    let splits = || stood.host.splits.lock().unwrap().len();
    assert_eq!(splits(), 0);
    assert_eq!(stood.row(&stood.worker).state, WorkerState::Sleeping);
    assert_eq!(
        stood.row(&stood.worker).exit_unconfirmed,
        Some(crate::agent_teams::ExitWitness {
            group: stood.leader + 1,
            started: Some(format!("fixture-start-{}", stood.leader + 1)),
        }),
        "the program is not held on the ledger's row for a window that restarts"
    );
    let actor = test_actor(stood.leader);
    let restore = || super::reseat_sleeping(&stood.host, Vec::new(), stood.leader, Some(&actor));
    // The coordinator's own restore opens nothing beside the program.
    assert_eq!(restore(), 0);
    assert_eq!(
        splits(),
        0,
        "a restore opened the conversation beside its lingering program"
    );
    // The grace, past its time: seats nothing, and ends nothing.
    {
        let _old = BootedHere::at(clock() - RESEAT_GRACE_MS - 1);
        super::expire_sleepers(&stood.host, clock());
    }
    assert_eq!(splits(), 0);
    assert_eq!(stood.row(&stood.worker).state, WorkerState::Sleeping);
    assert_eq!(stood.died(at + 10), 0);
    // A window that boots again reads the same hold off the ledger.
    assert!(stood.row(&stood.worker).exit_unconfirmed.is_some());
    assert_eq!(restore(), 0);
    assert_eq!(splits(), 0);
    // The program leaves: the next restore seats the same worker on the
    // same attempt, once.
    *stood.host.lingering.lock().unwrap() = false;
    assert_eq!(restore(), 1);
    assert_eq!(splits(), 1);
    let row = stood.row(&stood.worker);
    assert_eq!(row.state, WorkerState::Active);
    assert_eq!(row.dispatch.as_deref(), Some(stood.dispatch.as_str()));
    assert_eq!(restore(), 0);
    assert_eq!(splits(), 1, "seated twice");
    assert_eq!(stood.died(at + 20), 0);
    // The switch's own receipt for the pane, on the next look, once.
    assert_eq!(
        crate::account_switch::reconcile(&stood.host, &stood.world, at + 30),
        1
    );
    assert_eq!(
        crate::account_switch::reconcile(&stood.host, &stood.world, at + 31),
        0
    );
    let panes: Vec<serde_json::Value> = stood
        .receipts(at + 40)
        .into_iter()
        .filter(|body| body["moved"] == "pane")
        .collect();
    assert_eq!(panes.len(), 1, "{panes:?}");
    assert_eq!(panes[0]["workerId"], stood.worker);
    assert!(stood.journal_text().is_empty(), "{}", stood.journal_text());
}

/// Every receipt a switch owes stays owed until the ledger takes it
/// (astra R4). The ledger's disk refuses writes from the switch's first
/// receipt on — every effect has already landed — for the beat's own
/// switch, for the look that settles a journal, and for a person's pick
/// made on top of the unfinished switch: nothing leaves the journal, each
/// answer says the switch happened unrecorded, and the pick is written
/// BESIDE the unfinished switch, never over it. Once the disk takes writes
/// again, one look writes each owed receipt exactly once; the next look —
/// and a window reading the journal fresh — writes none.
#[test]
fn a_refused_receipt_stays_owed_until_the_ledger_takes_it_once() {
    let stood = WalledClaude::stand(89_600, 0);
    let at = stood.began + 20_000;
    let plan = stood.plan(at);
    stood.host.seating_onto(stood.leader + 2);
    stood.disk_refuses_from_the_first_receipt();
    let applied =
        crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
            .expect("every effect landed");
    assert!(applied.switched_default, "{applied:?}");
    assert!(applied.panes[0].ok, "{applied:?}");
    assert_eq!(applied.receipts, 0, "{applied:?}");
    assert!(applied.receipt_error.is_some(), "{applied:?}");
    let owed = stood.journal_text();
    assert!(
        owed.contains(&stood.worker) && owed.contains("\"default\": true"),
        "{owed}"
    );
    // The look that settles it meets the same disk: still owed.
    assert_eq!(
        crate::account_switch::reconcile(&stood.host, &stood.world, at + 1),
        0
    );
    assert_eq!(
        stood.journal_text(),
        owed,
        "a refused look changed the journal"
    );
    // A person's pick on top of the unfinished switch.
    let picked = crate::account_switch::switch_by_person(
        &stood.host,
        &stood.world,
        Some("a-fixture"),
        at + 2,
    )
    .expect("the pick landed");
    assert!(picked.receipt_error.is_some(), "{picked:?}");
    let book: crate::account_switch::JournalBook =
        serde_json::from_str(&stood.journal_text()).expect("a book");
    assert_eq!(book.switches.len(), 2, "{book:?}");
    stood.disk_takes_writes();
    assert_eq!(
        crate::account_switch::reconcile(&stood.host, &stood.world, at + 3),
        3
    );
    assert_eq!(
        crate::account_switch::reconcile(&stood.host, &stood.world, at + 4),
        0
    );
    assert!(stood.journal_text().is_empty(), "{}", stood.journal_text());
    let bodies = stood.receipts(at + 5);
    assert_eq!(bodies.len(), 3, "{bodies:?}");
    let pane: Vec<&serde_json::Value> = bodies
        .iter()
        .filter(|body| body["moved"] == "pane")
        .collect();
    assert_eq!(pane.len(), 1, "{bodies:?}");
    assert_eq!(pane[0]["workerId"], stood.worker);
    assert_eq!(pane[0]["toAccount"], "b-fixture");
    let auto = bodies
        .iter()
        .find(|body| body["moved"] == "default" && body["by"] == "auto")
        .expect("the beat's default");
    assert_eq!(auto["panesMoved"], 1);
    assert_eq!(auto["panesPending"], 0);
    assert!(
        bodies
            .iter()
            .any(|body| body["moved"] == "default" && body["by"] == "person"),
        "{bodies:?}"
    );
    assert_eq!(stood.died(at + 6), 0);
}

/// Rewrite a switch test's transcript row by row.
fn rewrite_transcript(path: &Path, change: impl Fn(&mut serde_json::Value)) {
    let text = std::fs::read_to_string(path).expect("the transcript");
    let rows: String = text
        .lines()
        .map(|line| {
            let mut row: serde_json::Value = serde_json::from_str(line).expect("a row");
            change(&mut row);
            format!("{row}\n")
        })
        .collect();
    std::fs::write(path, rows).expect("rewritten");
}

/// What a pane runs is read off its own transcript, or the pane is not
/// moved (astra R5): a transcript whose newest words are behind a tool
/// output longer than the tail, and a CLI that never wrote its effort,
/// leave the pane where it is — the row keeps the summons' words and no
/// relaunch is made on them. And the pane's tuning rides in the rest
/// itself: a rest the ledger's disk refuses writes neither, and nothing is
/// closed.
#[test]
fn a_pane_whose_tuning_is_not_read_or_not_written_stays_where_it_is() {
    let refused = |stood: &WalledClaude, needle: &str| {
        let at = stood.began + 20_000;
        let plan = stood.plan(at);
        stood.host.seating_onto(stood.leader + 2);
        let applied =
            crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
                .expect("the default still moves");
        assert!(!applied.panes[0].ok, "{applied:?}");
        assert!(
            applied.panes[0]
                .why
                .as_deref()
                .unwrap_or_default()
                .contains(needle),
            "{applied:?}"
        );
        assert!(stood.host.closed().is_empty(), "{:?}", stood.host.closed());
        assert!(stood.host.splits.lock().unwrap().is_empty());
        let row = stood.row(&stood.worker);
        assert_eq!(row.state, WorkerState::Active);
        assert_eq!(row.model.as_deref(), Some("claude-fable-5-1"));
        assert_eq!(row.effort.as_deref(), Some("max"));
        assert_eq!(stood.died(at + 10), 0);
    };
    // The newest model, effort and mode are behind a tool output longer
    // than the tail.
    {
        let stood = WalledClaude::stand(89_700, 0);
        let mut text = std::fs::read_to_string(&stood.transcript).expect("the transcript");
        let long = "x".repeat(
            usize::try_from(zerocode_core::transcript::MAX_TAIL_BYTES).expect("a size") + 4_096,
        );
        text.push_str(
            &serde_json::json!({"type": "user", "uuid": "u-3",
                "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu-2", "content": long}]}})
            .to_string(),
        );
        text.push('\n');
        std::fs::write(&stood.transcript, text).expect("rewritten");
        refused(
            &stood,
            "does not say its current model, effort, permission mode",
        );
    }
    // A CLI that never wrote its effort.
    {
        let stood = WalledClaude::stand(89_800, 0);
        rewrite_transcript(&stood.transcript, |row| {
            if let Some(held) = row.as_object_mut() {
                held.remove("effort");
                held.remove("perTurnEffort");
            }
        });
        refused(&stood, "does not say its current effort");
    }
    // A rest the disk refuses: the row's tuning and its sleep are one
    // transition, and neither landed.
    {
        let stood = WalledClaude::stand(89_900, 0);
        stood.disk_refuses_writes();
        let at = stood.began + 20_000;
        let plan = stood.plan(at);
        stood.host.seating_onto(stood.leader + 2);
        let applied =
            crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
                .expect("the default still moves");
        assert!(!applied.panes[0].ok, "{applied:?}");
        assert!(stood.host.closed().is_empty(), "{:?}", stood.host.closed());
        assert!(stood.host.splits.lock().unwrap().is_empty());
        stood.disk_takes_writes();
        let row = stood.row(&stood.worker);
        assert_eq!(row.state, WorkerState::Active);
        assert_eq!(
            row.model.as_deref(),
            Some("claude-fable-5-1"),
            "the row names a tuning the refused rest never wrote"
        );
        assert_eq!(stood.died(at + 10), 0);
    }
}

/// The wall witness reads the gauge of the account the PANE runs as, not
/// the selected account's (astra A4): a pane launched as A after the
/// default moved to B is judged by A's number, and a pane the window
/// cannot attribute is judged by the provider gauge as before.
#[test]
fn a_panes_wall_is_judged_by_its_own_accounts_gauge() {
    let began = clock();
    let mut b = usage_snapshot(
        "claude",
        Some((98, Some(began + 60 * 60_000))),
        None,
        began - 60_000,
    );
    b.account = Some("b-fixture".to_string());
    let mut a = usage_snapshot(
        "claude",
        Some((40, Some(began + 60 * 60_000))),
        None,
        began - 60_000,
    );
    a.account = Some("a-fixture".to_string());
    let usage = super::UsageSource::fixed(vec![("claude", b), ("claude", a)]);
    let pane = |account: &str| crate::agent_teams::PaneLogin {
        account: account.to_string(),
        login: format!("login-of-{account}"),
    };
    // The selected account (B) is at the wall; the pane runs as A.
    let as_a = super::usage_headroom_of_account(&usage, "claude", None, Some(&pane("a-fixture")))
        .expect("A's number");
    assert_eq!(as_a.used_percent, 40);
    let as_b = super::usage_headroom_of_account(&usage, "claude", None, Some(&pane("b-fixture")))
        .expect("B's number");
    assert_eq!(as_b.used_percent, 98);
    // An account nobody read is unknown — never a neighbour's number.
    assert!(
        super::usage_headroom_of_account(&usage, "claude", None, Some(&pane("c-fixture")))
            .is_none()
    );
    // No attribution: the provider gauge, first row, as it always was.
    assert_eq!(
        super::usage_headroom_of_account(&usage, "claude", None, None)
            .expect("the provider gauge")
            .used_percent,
        98
    );
}

/* ---- t-7538 r4: the switch on t-7812's restore roads --------------------- */

/// Every continuation pasted into `term` once its seat was durable, in order.
fn pasted_into(host: &Switching, term: u32) -> Vec<String> {
    host.pasted
        .lock()
        .unwrap()
        .iter()
        .filter(|(at, _)| *at == term)
        .map(|(_, words)| words.clone())
        .collect()
}

/// A new process of the window, as far as a switch's memory goes: the
/// goodbye's book is read off the disk again, no switch mark is left, and
/// no journal the window could not write (astra R4-1) — only what was
/// written down survives it.
fn a_new_process_forgets(stood: &WalledClaude) {
    restart_census::a_new_process(super::BLACKBOX.get().expect("the window's data root"));
    super::forget_switch_mark(&stood.worker);
    crate::account_switch::a_new_process(stood.world.journal.path());
}

/// The account-switch receipts the ledger holds, read off its rows — the
/// coordinator's seat may have moved to another team by then.
fn switch_receipts() -> Vec<serde_json::Value> {
    the_rows()
        .messages
        .into_iter()
        .filter(|message| {
            message.kind == zerocode_core::orchestration::MessageKind::AccountSwitched
        })
        .map(|message| serde_json::from_str(message.body.as_str()).expect("json"))
        .collect()
}

/// A window of a switch test goes, the way the ⌘Q road goes
/// (`restore::the_window_goes`) — as its OWN window's first goodbye: the
/// flag that makes a window say its goodbye once is the process's, and one
/// test process plays many windows.
fn the_window_goes_once(panes: &[u32]) {
    super::GOODBYE_SAID.store(false, std::sync::atomic::Ordering::SeqCst);
    restore::the_window_goes(&restore::census_without_commands, panes);
}

/// The next window of a switch test: its boot sweep, then the run's
/// coordinator back in `new_leader` — the same conversation the run was
/// created from in `first_leader`, however many windows ago — asking its
/// run's sleepers back. Answers how many it seated.
fn the_coordinator_returns_to(
    host: &dyn Host,
    name: &str,
    old_leader: u32,
    new_leader: u32,
    first_leader: u32,
) -> usize {
    crate::agent_teams::forget_term(old_leader);
    super::runtime()
        .expect("the private runtime")
        .actor
        .window_restarted(clock())
        .expect("the next boot's sweep");
    seat_a_team(&format!("team-t7538-{name}-back-{new_leader}"), new_leader);
    super::reseat_sleeping(
        host,
        Vec::new(),
        new_leader,
        Some(&test_actor(first_leader)),
    )
}

/// Where the ledger seats `worker` now.
fn term_now(worker: &str) -> u32 {
    super::worker_now(worker)
        .and_then(|now| now.term)
        .expect("a seated worker")
}

/// t-7538 r4 (brief ①): a worker an account switch rested, and that no road
/// of that window seated again (its old program outlived the close), comes
/// back after the window restarts through the ledger's own reseat, told the
/// SWITCH's words — once — and not the restart's: the switch is what
/// happened to it, and a worker told the window restarted looks for a cut
/// turn that never was. A plain sleeper beside it, at rest when the window
/// went, is told nothing (t-7812 E). The words ride the goodbye's note that
/// both roads read and are spent once they reach a pane, so after a second
/// restart neither of them is told anything by the other road either — the
/// doors their restored tabs open. (A test process has one host epoch, so
/// the ledger's own reseat cuts a worker's pane once per process — 함정
/// 429 — and the second wake here is the door's.) The pane the reseat cut
/// for it is written down as the login it runs as (brief ③).
#[test]
fn a_worker_the_switch_rested_hears_the_switchs_words_once_after_a_restart_and_a_plain_sleeper_none()
 {
    let stood = WalledClaude::stand(89_700, 1);
    let at = stood.began + 20_000;
    let (plain, plain_term) = stood.working[0].clone();
    let plain_session = "5a1e0000-0000-4000-8000-000000007539";
    stood.in_its_own_conversation(plain_term, plain_session);
    *stood.host.lingering.lock().unwrap() = true;
    let plan = stood.plan(at);
    stood.host.seating_onto(stood.leader + 2);
    let applied =
        crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
            .expect("applied");
    assert!(!applied.panes[0].ok, "{applied:?}");
    assert_eq!(stood.row(&stood.worker).state, WorkerState::Sleeping);
    assert!(stood.host.splits.lock().unwrap().is_empty());
    // The program leaves after all, and the person restarts the window
    // before anything seated the worker again. The plain worker is at rest.
    *stood.host.lingering.lock().unwrap() = false;
    stood.host.busy.lock().unwrap().clear();
    the_window_goes_once(&[stood.leader, plain_term]);
    a_new_process_forgets(&stood);
    let back = stood.leader + 40;
    stood.host.seating_onto(stood.leader + 30);
    assert_eq!(
        the_coordinator_returns_to(
            &stood.host,
            "switch-words",
            stood.leader,
            back,
            stood.leader
        ),
        2
    );
    let (moved, rested) = (term_now(&stood.worker), term_now(&plain));
    let told = pasted_into(&stood.host, moved);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(
        told[0].contains("another of the person's accounts (b-fixture)"),
        "{told:?}"
    );
    assert!(
        !told[0].contains("The window restarted"),
        "the switch's worker was told a restart cut its turn: {told:?}"
    );
    assert!(
        pasted_into(&stood.host, rested).is_empty(),
        "a sleeper the goodbye cut nothing of was typed at"
    );
    assert_eq!(
        restore_door::owed(&stood.worker),
        restart_census::Cut::default()
    );
    // Brief ③: the pane the reseat road cut runs as a login the window wrote
    // down, the one selected when it was cut.
    assert_eq!(
        stood.host.pane_login(moved),
        Some(crate::agent_teams::PaneLogin {
            account: "b-fixture".to_string(),
            login: "login-of-b-fixture".to_string(),
        })
    );
    // Once: after the next restart neither of them is told anything, by
    // the doors their restored tabs open.
    the_window_goes_once(&[back, moved, rested]);
    a_new_process_forgets(&stood);
    let door = restore_door::Door::new(Path::new(stood.host.checkout));
    let (tab, plain_tab) = (stood.leader + 80, stood.leader + 81);
    door.wake(tab, "claude", SWITCH_SESSION)
        .expect("the worker's tab");
    door.wake(plain_tab, "claude", plain_session)
        .expect("the plain worker's tab");
    let started = door.started();
    assert_eq!(started.len(), 2, "{started:?}");
    for (term, command) in &started {
        assert!(
            !command.contains("another of the person's accounts")
                && !command.contains("The window restarted"),
            "t{term} was told something again: {command}"
        );
        assert_eq!(door.typed(*term).0, 0, "t{term} was typed at");
    }
    assert_eq!(
        door.seated_at_start(tab).as_deref(),
        Some(stood.worker.as_str())
    );
    assert_eq!(
        door.seated_at_start(plain_tab).as_deref(),
        Some(plain.as_str())
    );
    assert!(restore::deaths_of(&stood.worker).is_empty());
    assert!(restore::deaths_of(&plain).is_empty());
    for term in [back, moved, rested, tab, plain_tab] {
        crate::agent_teams::forget_term(term);
    }
}

/// t-7538 r4 (brief ② as astra m-8816 set it; astra R3a, R3b): a pane whose
/// program the switch's close did not see leave holds EVERY road to its
/// conversation. The ledger's reseat opens nothing and takes no claim on
/// the conversation while held; a door opens nothing either — the worker's
/// own restored tab and a person's tab in another checkout alike start no
/// process; the grace ends nothing. The hold is the ledger's own row,
/// written by the rest before the close, so a journal that took nothing
/// after the close and a window that boots again with the journal
/// unreadable hold it the same. Once a look sees the program gone the
/// conversation comes back ONCE, as the same worker on the same attempt,
/// with the switch's words — here through a door, after which the ledger's
/// reseat finds nothing to seat.
#[test]
fn a_program_nobody_saw_leave_holds_every_road_even_a_door_and_a_boot_without_its_journal() {
    let stood = WalledClaude::stand(89_800, 0);
    let at = stood.began + 20_000;
    *stood.host.lingering.lock().unwrap() = true;
    // The journal takes nothing from the first receipt on — its directory is
    // gone, a file stands in its place — while the rest, the program on the
    // row and the close have all landed by then.
    let journal = stood.world.journal.path().to_path_buf();
    let sealed = journal.clone();
    *stood.world.before_receipt.lock().unwrap() = Some(Box::new(move || {
        std::fs::remove_dir_all(&sealed).expect("the journal's directory goes");
        std::fs::write(&sealed, b"not a directory").expect("a journal that takes nothing");
    }));
    let plan = stood.plan(at);
    stood.host.seating_onto(stood.leader + 2);
    let applied =
        crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
            .expect("applied");
    assert!(!applied.panes[0].ok, "{applied:?}");
    assert!(applied.receipt_error.is_some(), "{applied:?}");
    assert_eq!(stood.host.closed(), vec![stood.leader + 1]);
    let splits = || stood.host.splits.lock().unwrap().len();
    assert_eq!(splits(), 0);
    let row = stood.row(&stood.worker);
    assert_eq!(row.state, WorkerState::Sleeping);
    assert!(row.exit_unconfirmed.is_some(), "{row:?}");
    // The coordinator's own restore: nothing opened, no claim taken.
    let actor = test_actor(stood.leader);
    let restore = || super::reseat_sleeping(&stood.host, Vec::new(), stood.leader, Some(&actor));
    let asked = *stood.host.standing_asked.lock().unwrap();
    assert_eq!(restore(), 0);
    assert_eq!(splits(), 0);
    assert_eq!(
        *stood.host.standing_asked.lock().unwrap(),
        asked,
        "a held worker's reseat took a claim on its conversation"
    );
    // The doors: the worker's own tab, and a person's tab in another
    // checkout that would not even seat the worker.
    let door = restore_door::Door::new(Path::new(stood.host.checkout));
    *door.lingering.lock().unwrap() = true;
    let elsewhere = tempfile::tempdir().expect("another checkout");
    let other = restore_door::Door::new(elsewhere.path());
    *other.lingering.lock().unwrap() = true;
    let (own_tab, persons_tab) = (stood.leader + 70, stood.leader + 71);
    let refused = other.wake(persons_tab, "claude", SWITCH_SESSION);
    assert!(
        other.started().is_empty(),
        "a person's tab started a second process on the conversation: {:?}",
        other.started()
    );
    assert!(refused.is_err(), "{refused:?}");
    let refused = door
        .wake(own_tab, "claude", SWITCH_SESSION)
        .expect_err("the worker's own tab");
    assert!(refused.contains("떠나지 않아"), "{refused}");
    assert!(door.started().is_empty(), "{:?}", door.started());
    assert!(
        !door
            .did
            .lock()
            .unwrap()
            .iter()
            .any(|did| matches!(did, restore_door::Did::Prepared(_))),
        "a held door built a launch"
    );
    assert_eq!(door.noted("has not been seen to leave").len(), 1);
    assert_eq!(stood.row(&stood.worker).state, WorkerState::Sleeping);
    // astra R3-1: a ledger this window cannot read is not a ledger that
    // holds nothing. The runtime's own image does not answer — its store
    // refuses the connection — and then a boot of this window stands down
    // beside the durable store and leaves no runtime at all: neither
    // door builds a launch, starts a process or says a word, and the row
    // and its hold stay as they were. The ledger reads again with the
    // program still there: held, at both doors.
    held_doors_open_nothing_while_the_ledger_cannot_be_read(
        &stood,
        &[(&other, persons_tab), (&door, own_tab)],
    );
    assert_eq!(door.noted("has not been seen to leave").len(), 2);
    let row = stood.row(&stood.worker);
    assert_eq!(row.state, WorkerState::Sleeping);
    assert!(row.exit_unconfirmed.is_some(), "{row:?}");
    // The grace, past its time: ends nothing.
    {
        let _old = BootedHere::at(clock() - RESEAT_GRACE_MS - 1);
        super::expire_sleepers(&stood.host, clock());
    }
    assert_eq!(stood.row(&stood.worker).state, WorkerState::Sleeping);
    assert!(restore::deaths_of(&stood.worker).is_empty());
    // The window restarts with the program still there — its journal
    // unreadable, its memory gone — and the coordinator comes back.
    std::fs::remove_file(&journal).expect("the stand-in file goes");
    std::fs::create_dir_all(&journal).expect("the directory back");
    std::fs::write(
        journal.join(crate::app_paths::artifact_file::CLAUDE_ACCOUNT_SWITCH),
        "{ not a journal",
    )
    .expect("an unreadable journal");
    the_window_goes_once(&[stood.leader]);
    a_new_process_forgets(&stood);
    let back = stood.leader + 40;
    stood.host.seating_onto(stood.leader + 30);
    assert_eq!(
        the_coordinator_returns_to(&stood.host, "held-boot", stood.leader, back, stood.leader),
        0
    );
    assert_eq!(splits(), 0);
    assert!(stood.row(&stood.worker).exit_unconfirmed.is_some());
    // The program leaves: one road brings the conversation back, once.
    *stood.host.lingering.lock().unwrap() = false;
    *door.lingering.lock().unwrap() = false;
    door.wake(own_tab, "claude", SWITCH_SESSION)
        .expect("the door opens it now");
    let started = door.started();
    assert_eq!(started.len(), 1, "{started:?}");
    assert!(
        started[0]
            .1
            .contains("another of the person's accounts (b-fixture)"),
        "{started:?}"
    );
    assert_eq!(
        door.seated_at_start(own_tab).as_deref(),
        Some(stood.worker.as_str())
    );
    let row = stood.row(&stood.worker);
    assert_eq!(row.state, WorkerState::Active);
    assert_eq!(row.dispatch.as_deref(), Some(stood.dispatch.as_str()));
    assert!(row.exit_unconfirmed.is_none(), "{row:?}");
    assert_eq!(
        super::reseat_sleeping(&stood.host, Vec::new(), back, Some(&actor)),
        0
    );
    assert_eq!(splits(), 0, "a second process on the conversation");
    assert!(restore::deaths_of(&stood.worker).is_empty());
    for term in [own_tab, persons_tab, back] {
        crate::agent_teams::forget_term(term);
    }
}

/// astra R3-1: every door in `doors` — `(door, the tab it opens)` — asked
/// for the switch's conversation while this window cannot read its ledger:
/// first the runtime's image does not answer (a directory stands where its
/// store's journal goes, so the store refuses the connection every request
/// opens — a refusal the actor lives through, unlike an authority moved
/// under it, which ends it), then a boot of this window over the same data
/// root stands down beside the durable store and leaves no runtime at all.
/// Nothing is built, started or typed, and each door says why once per
/// look. Then the ledger reads again with the program still there: each
/// door is held.
fn held_doors_open_nothing_while_the_ledger_cannot_be_read(
    stood: &WalledClaude,
    doors: &[(&restore_door::Door, u32)],
) {
    let prepared = |door: &restore_door::Door| {
        door.did
            .lock()
            .unwrap()
            .iter()
            .filter(|did| matches!(did, restore_door::Did::Prepared(_)))
            .count()
    };
    let asked = |scene: &str, said: &str| {
        for (door, tab) in doors {
            let refused = door.wake(*tab, "claude", SWITCH_SESSION).expect_err(scene);
            assert!(refused.contains(said), "{scene}: {refused}");
            assert!(
                door.started().is_empty(),
                "{scene}: a door started a process: {:?}",
                door.started()
            );
            assert_eq!(prepared(door), 0, "{scene}: a door built a launch");
        }
    };
    let root = stood._window._root.path().to_path_buf();
    let mut journal = root
        .join(super::AUTHORITY_DIR)
        .join(super::AUTHORITY_STORE_FILE)
        .into_os_string();
    journal.push("-wal");
    let journal = PathBuf::from(journal);
    let unread = "원장을 읽지 못해";
    std::fs::create_dir(&journal).expect("a directory where the store's journal goes");
    asked("the runtime's image does not answer", unread);
    std::fs::remove_dir(&journal).expect("the store's journal can be made again");
    std::fs::write(root.join(super::LEDGER_FILE), b"{ not a ledger")
        .expect("a ledger file the boot cannot read");
    let standing = super::runtime_cell()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .take();
    {
        let _stood_down = Degraded::booted_over(&root);
        asked("a boot that stood down beside the store", unread);
    }
    *super::runtime_cell()
        .lock()
        .unwrap_or_else(|held| held.into_inner()) = standing;
    std::fs::remove_file(root.join(super::LEDGER_FILE)).expect("the file goes");
    for (door, _) in doors {
        assert_eq!(door.noted("could not be read").len(), 2);
    }
    asked(
        "the ledger reads, the program is still there",
        "떠나지 않아",
    );
}

/// astra R3-1: a person's tab in another checkout — the door no seat
/// guards, since it would never seat the worker — opens nothing while the
/// ledger that holds the conversation cannot be read, nothing while it
/// reads and the program is still there, and the conversation once that
/// program is seen gone: one process, at the same door. The worker stays
/// asleep with its attempt open; only its hold is lifted.
#[test]
fn a_persons_door_elsewhere_opens_a_held_conversation_only_once_the_hold_is_read_gone() {
    let stood = WalledClaude::stand(92_000, 0);
    let at = stood.began + 20_000;
    *stood.host.lingering.lock().unwrap() = true;
    let plan = stood.plan(at);
    stood.host.seating_onto(stood.leader + 2);
    let applied =
        crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
            .expect("applied");
    assert!(!applied.panes[0].ok, "{applied:?}");
    assert!(stood.row(&stood.worker).exit_unconfirmed.is_some());
    let elsewhere = tempfile::tempdir().expect("another checkout");
    let door = restore_door::Door::new(elsewhere.path());
    *door.lingering.lock().unwrap() = true;
    let tab = stood.leader + 71;
    held_doors_open_nothing_while_the_ledger_cannot_be_read(&stood, &[(&door, tab)]);
    *door.lingering.lock().unwrap() = false;
    door.wake(tab, "claude", SWITCH_SESSION)
        .expect("the door opens it once its program is gone");
    let started = door.started();
    assert_eq!(started.len(), 1, "{started:?}");
    assert!(started[0].1.contains(SWITCH_SESSION), "{started:?}");
    let row = stood.row(&stood.worker);
    assert_eq!(row.state, WorkerState::Sleeping);
    assert_eq!(row.dispatch.as_deref(), Some(stood.dispatch.as_str()));
    assert!(row.exit_unconfirmed.is_none(), "{row:?}");
    crate::agent_teams::forget_term(tab);
}

/// astra R2 (r4): a yes is a yes to the logins it was shown, under the
/// setting it was given in, until the rest itself. After the default's
/// selection landed, account B comes to name ANOTHER login — a person
/// re-logged it — and that login's gauge arrives with the same room: the
/// pane is not moved, nothing is rested, closed or seated, and the answer
/// says the login changed and a new plan needs a new yes. The same change
/// at the very last moment — inside the rest's fence, after every check
/// before it passed — refuses the rest there, and so does the switch being
/// turned off at that moment.
#[test]
fn a_login_that_changed_under_a_yes_moves_no_pane() {
    use zerocode_core::account_autoswitch::AutoSwitchMode;
    let relogged = |logins: &Mutex<std::collections::HashMap<String, String>>| {
        logins
            .lock()
            .unwrap()
            .insert("b-fixture".to_string(), "login-b2".to_string());
    };
    for (leader, scene) in [
        (89_900, "after the selection"),
        (89_950, "at the fence"),
        (89_975, "turned off at the fence"),
    ] {
        let stood = WalledClaude::stand(leader, 0);
        let at = stood.began + 20_000;
        let seat = stood.row(&stood.worker).pane;
        let summoned = stood.row(&stood.worker).model;
        let plan = stood.plan(at);
        assert_eq!(plan.moves().count(), 1, "{:?}", plan.walled);
        let said = if scene == "turned off at the fence" {
            let mode = std::sync::Arc::clone(&stood.world.mode);
            *stood.host.during_observation.lock().unwrap() = Some(Box::new(move || {
                *mode.lock().unwrap() = AutoSwitchMode::Off;
            }));
            "turned off"
        } else if scene == "at the fence" {
            let logins = std::sync::Arc::clone(&stood.host.logins);
            *stood.host.during_observation.lock().unwrap() =
                Some(Box::new(move || relogged(&logins)));
            "the login account b-fixture names changed"
        } else {
            *stood.world.after_select.lock().unwrap() =
                Some(Box::new(move |world: &SwitchWorld| {
                    relogged(&world.logins);
                    for gauge in world
                        .gauges
                        .lock()
                        .unwrap()
                        .iter_mut()
                        .filter(|gauge| gauge.id == "b-fixture")
                    {
                        gauge.identity = Some("login-b2".to_string());
                    }
                }));
            "the login account b-fixture names changed"
        };
        stood.host.seating_onto(stood.leader + 2);
        let applied =
            crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
                .expect("the default moved");
        assert!(applied.switched_default, "{applied:?}");
        assert!(!applied.panes[0].ok, "{applied:?}");
        assert!(
            applied.panes[0]
                .why
                .as_deref()
                .unwrap_or_default()
                .contains(said),
            "{scene}: {applied:?}"
        );
        assert!(stood.host.closed().is_empty(), "{:?}", stood.host.closed());
        assert!(stood.host.splits.lock().unwrap().is_empty());
        let row = stood.row(&stood.worker);
        assert_eq!(row.state, WorkerState::Active);
        assert_eq!(row.pane, seat);
        assert_eq!(row.model, summoned, "the rest's tuning was written");
        assert!(row.exit_unconfirmed.is_none());
    }
}

/// Where a journal's file waits, byte for byte, while its directory
/// refuses writes (astra R4-1).
fn journal_aside(dir: &Path) -> PathBuf {
    dir.join("the-journal-as-it-stood")
}

/// A directory stands where the journal's file was, so every replace of it
/// fails, and the file waits aside. A read-only directory would not do: the
/// durable writer makes a parent private again before it writes.
fn journal_refuses_writes_in(dir: &Path) {
    let file = dir.join(crate::app_paths::artifact_file::CLAUDE_ACCOUNT_SWITCH);
    std::fs::rename(&file, journal_aside(dir)).expect("the journal's file steps aside");
    std::fs::create_dir(&file).expect("a directory where the file was");
    std::fs::write(file.join("in-the-way"), b"").expect("a directory that is not empty");
}

fn journal_takes_writes_in(dir: &Path) {
    let file = dir.join(crate::app_paths::artifact_file::CLAUDE_ACCOUNT_SWITCH);
    std::fs::remove_dir_all(&file).expect("the directory goes");
    std::fs::rename(journal_aside(dir), &file).expect("the journal's file comes back");
}

/// How the switch's journal fares from some moment of a move on
/// (astra R4-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalFares {
    /// Every write lands.
    Takes,
    /// The replace that writes the book after the receipts meets a journal
    /// that takes nothing; what it held before stays as it was.
    RefusesItsLastWrite,
    /// Nothing is written from the old pane's close on: the rest's book
    /// stays on disk, and the move lives only in this window's memory.
    RefusesFromTheClose,
    /// As [`Self::RefusesFromTheClose`] — then the journal takes writes
    /// again, one look runs while the ledger still refuses the receipts, and
    /// the window ends: what it held must be on disk by then.
    RefusesFromTheCloseAndTheWindowEnds,
}

/// astra R4 (r4): a move this switch saw complete keeps its receipt as it
/// saw it until the ledger takes it. The disk refuses the receipts, so the
/// journal holds the pane's completed move — A's pane to B's. Then, before
/// any look settles it, (1) the worker finishes its task and says
/// `worker_done`: the move still happened, and its receipt is written once,
/// as it was seen; (2) in another world the window restarts and the worker
/// comes back in yet another pane as another account: the receipt still
/// names the pane and the account the switch moved it to, not where the
/// worker went after. A second Claude worker keeps working beside it, so
/// the run is one the default's receipt concerns in both.
///
/// astra R4-1: the same, when the journal also refuses the replace that
/// writes the book after the receipts — the move seen complete is not
/// lost with it, across a restart too — and when it refuses everything
/// from the old pane's close on, so the move lives only in this window's
/// memory until a write lands: the next look writes it down even when it
/// has nothing else to change, and a window that ends after that look
/// leaves the move on disk.
#[test]
fn a_moved_panes_receipt_is_the_move_it_saw_whatever_the_worker_does_next() {
    use JournalFares::{
        RefusesFromTheClose, RefusesFromTheCloseAndTheWindowEnds, RefusesItsLastWrite, Takes,
    };
    for (leader, moves_on, journal) in [
        (90_000, false, Takes),
        (90_100, true, Takes),
        (92_200, false, RefusesItsLastWrite),
        (92_300, true, RefusesItsLastWrite),
        (92_400, false, RefusesFromTheClose),
        (92_500, false, RefusesFromTheCloseAndTheWindowEnds),
    ] {
        let scene = format!("moves_on={moves_on} journal={journal:?}");
        let stood = WalledClaude::stand(leader, 1);
        let (_, beside) = stood.working[0].clone();
        stood.in_its_own_conversation(beside, "5a1e0000-0000-4000-8000-000000007540");
        let at = stood.began + 20_000;
        let from_pane = stood.row(&stood.worker).pane;
        let plan = stood.plan(at);
        stood.host.seating_onto(stood.leader + 2);
        match journal {
            Takes => stood.disk_refuses_from_the_first_receipt(),
            RefusesItsLastWrite => stood.disk_and_journal_refuse_from_the_first_receipt(),
            RefusesFromTheClose | RefusesFromTheCloseAndTheWindowEnds => {
                stood.journal_refuses_from_the_close();
                stood.disk_refuses_from_the_first_receipt();
            }
        }
        let applied =
            crate::account_switch::apply_with(&stood.host, &stood.world, &plan.token, "auto", at)
                .expect("every effect landed");
        assert!(applied.panes[0].ok, "{scene}: {applied:?}");
        assert_eq!(applied.receipts, 0, "{scene}: {applied:?}");
        let moved = stood.row(&stood.worker);
        if journal == Takes {
            let book: crate::account_switch::JournalBook =
                serde_json::from_str(&stood.journal_text()).expect("a book");
            let owed = &book.switches[0].panes[0];
            assert_eq!(owed.to_pane.as_deref(), Some(moved.pane.as_str()));
            assert_eq!(owed.to_account.as_deref(), Some("b-fixture"));
        } else {
            assert!(applied.receipt_error.is_some(), "{scene}: {applied:?}");
            stood.journal_takes_writes();
            if journal == RefusesFromTheCloseAndTheWindowEnds {
                assert_eq!(
                    crate::account_switch::reconcile(&stood.host, &stood.world, at),
                    0,
                    "{scene}"
                );
                crate::account_switch::a_new_process(stood.world.journal.path());
            }
        }
        stood.disk_takes_writes();
        if moves_on {
            *stood.host.selected.lock().unwrap() = Some("a-fixture".to_string());
            let moved_term = term_now(&stood.worker);
            the_window_goes_once(&[stood.leader, moved_term, beside]);
            a_new_process_forgets(&stood);
            let back = stood.leader + 40;
            stood.host.seating_onto(stood.leader + 30);
            assert_eq!(
                the_coordinator_returns_to(
                    &stood.host,
                    "moved-on",
                    stood.leader,
                    back,
                    stood.leader
                ),
                2
            );
            let now = stood.row(&stood.worker);
            assert_ne!(now.pane, moved.pane);
            assert_eq!(now.dispatch, moved.dispatch);
            let (moved_on, beside_again) = (term_now(&stood.worker), term_now(&stood.working[0].0));
            assert_eq!(
                stood.host.pane_account(moved_on).as_deref(),
                Some("a-fixture")
            );
            for term in [back, moved_on, beside_again] {
                crate::agent_teams::forget_term(term);
            }
        } else {
            let capability = crate::agent_teams::current_pane_capability(&moved.team, &moved.pane)
                .expect("the moved pane's capability");
            assert_eq!(
                restore_door::done_from(&stood.host, &moved.team, &moved.pane, &capability),
                0
            );
            assert_eq!(
                stood.row(&stood.worker).dispatch,
                None,
                "the attempt did not end"
            );
        }
        assert_eq!(
            crate::account_switch::reconcile(&stood.host, &stood.world, at + 1),
            2,
            "{scene}"
        );
        assert_eq!(
            crate::account_switch::reconcile(&stood.host, &stood.world, at + 2),
            0,
            "{scene}"
        );
        let receipts = switch_receipts();
        let panes: Vec<&serde_json::Value> = receipts
            .iter()
            .filter(|body| body["moved"] == "pane")
            .collect();
        assert_eq!(panes.len(), 1, "{scene}: {receipts:?}");
        assert_eq!(panes[0]["workerId"], stood.worker, "{scene}");
        assert_eq!(panes[0]["fromPane"], from_pane, "{scene}");
        assert_eq!(panes[0]["toPane"], moved.pane, "{scene}");
        assert_eq!(panes[0]["fromAccount"], "a-fixture", "{scene}");
        assert_eq!(panes[0]["toAccount"], "b-fixture", "{scene}");
        assert!(
            stood.journal_text().is_empty(),
            "{scene}: {}",
            stood.journal_text()
        );
    }
}

/// Managed accounts `ids`, each in a store directory of its own under
/// `root`, with `selected` the selected one — the store the switch table
/// and the usage roads read (t-7538 r4).
fn a_claude_store(root: &Path, ids: &[&str], selected: &str) -> Vec<zerocode_core::ClaudeAccount> {
    let accounts: Vec<zerocode_core::ClaudeAccount> = ids
        .iter()
        .map(|id| {
            let dir = root.join("stores").join(id);
            std::fs::create_dir_all(&dir).expect("a store directory");
            zerocode_core::ClaudeAccount {
                id: (*id).to_string(),
                email: format!("{id}@example.test"),
                account_uuid: Some(format!("{id}-person")),
                organization_uuid: Some(format!("{id}-org")),
                config_dir: dir.to_string_lossy().into_owned(),
                added_at: 1,
                ..zerocode_core::ClaudeAccount::default()
            }
        })
        .collect();
    let store = crate::accounts::AccountStore {
        accounts: accounts.clone(),
        selection: zerocode_core::account::AccountSelection {
            active: Some(selected.to_string()),
            system_default: false,
        },
    };
    std::fs::write(
        root.join(crate::accounts::ACCOUNT_STORE_FILE),
        serde_json::to_string(&store).expect("json"),
    )
    .expect("the store");
    accounts
}

/// One OAuth answer with a session window at `used`.
fn an_oauth_reading(used: f32) -> crate::usage_oauth::OauthUsage {
    crate::usage_oauth::OauthUsage {
        session: Some(crate::usage_oauth::OauthWindow {
            used_percent: used,
            window_minutes: 300,
            resets_at: Some(clock() + 3_600_000),
        }),
        ..crate::usage_oauth::OauthUsage::default()
    }
}

/// The switch table's row for `id`, read the way a plan reads it.
fn table_row(
    config: &Path,
    data: &Path,
    id: &str,
) -> zerocode_core::account_autoswitch::AccountGauge {
    crate::usage_runtime::claude_account_gauges_of(&crate::accounts::read_store(config), data)
        .into_iter()
        .find(|gauge| gauge.id == id)
        .expect("the account's row")
}

/// astra R6a (r4): the SELECTED account's own gauge answers to the range
/// rule every other account's does. Its OAuth answer at -5 or 130 used to be
/// clamped to 0 or 100 and stand as `ok` — the table took the account for
/// room, a wall read it as a number. Now that read is `invalid`, lands with
/// no window, the table calls the account unknown and passes it over, and a
/// pane launched as its login has no number to be walled by.
#[test]
fn a_selected_accounts_read_outside_the_range_is_unknown_to_the_table_and_the_wall() {
    use zerocode_core::account_autoswitch::{Decision, Question, decide};
    let config = tempfile::tempdir().expect("a config root");
    let data = tempfile::tempdir().expect("a data root");
    let accounts = a_claude_store(
        config.path(),
        &["t7538r4-range-a", "t7538r4-range-b"],
        "t7538r4-range-b",
    );
    let b = &accounts[1];
    let pane_of_b = crate::agent_teams::PaneLogin {
        account: b.id.clone(),
        login: crate::usage_runtime::claude_login_key(b),
    };
    let usage = super::UsageSource::Cached {
        local_data_root: data.path().to_path_buf(),
    };
    let now = clock();
    let walled_a = zerocode_core::account_autoswitch::AccountGauge {
        id: accounts[0].id.clone(),
        org_type: Some("claude_max".to_string()),
        identity: Some("the-walled-login".to_string()),
        windows: vec![zerocode_core::account_autoswitch::GaugeWindow {
            kind: "session".to_string(),
            used_percent: 99,
            resets_at_ms: Some(now + 3_600_000),
        }],
        observed_at_ms: now - 60_000,
        status: "ok".to_string(),
    };
    for said in [-5.0_f32, 130.0] {
        let read =
            crate::usage_runtime::claude_oauth_reading(an_oauth_reading(said), Some(b.id.clone()));
        assert_eq!(
            read.status,
            crate::usage_runtime::ACCOUNT_READING_INVALID,
            "{said}"
        );
        assert!(read.session.is_none(), "{said}");
        assert!(
            crate::usage_runtime::land_claude_usage_as(config.path(), data.path(), Some(b), &read),
            "{said}"
        );
        let row = table_row(config.path(), data.path(), &b.id);
        assert!(row.windows.is_empty(), "{said}: {row:?}");
        assert_eq!(
            decide(&Question {
                source: &walled_a.id,
                gauges: &[walled_a.clone(), row],
                model: None,
                last_switch_ms: None,
                walled: true,
                now_ms: now,
            }),
            Decision::Stay {
                why: "no_candidate"
            },
            "{said}"
        );
        assert!(
            super::usage_headroom_of_account(&usage, "claude", None, Some(&pane_of_b)).is_none(),
            "{said}: a pane of that login was given a number"
        );
    }
}

/// astra R6b (r4): the selected account's read that lands after its row came
/// to name another login is that other login's number no longer. B is
/// selected as login one; its read goes out; B is re-logged as login two
/// and its readings forgotten, as every login change does; login one's late
/// answer arrives, fresh and `ok`, and the status bar's snapshot shows it
/// under B's id as it always has. But it lands nowhere the switch or the wall
/// reads: the table has no number for B until login two is read, and a pane
/// launched as login two has none to be walled by — while a pane still
/// running as login one is judged by login one's own reading as long as it
/// is the one on file.
#[test]
fn a_selected_read_that_lands_after_a_relogin_is_nobodys_number() {
    let config = tempfile::tempdir().expect("a config root");
    let data = tempfile::tempdir().expect("a data root");
    let accounts = a_claude_store(
        config.path(),
        &["t7538r4-late-a", "t7538r4-late-b"],
        "t7538r4-late-b",
    );
    let b = accounts[1].clone();
    let usage = super::UsageSource::Cached {
        local_data_root: data.path().to_path_buf(),
    };
    let as_one = crate::agent_teams::PaneLogin {
        account: b.id.clone(),
        login: crate::usage_runtime::claude_login_key(&b),
    };
    // Login one's own read, landed while B still names it.
    let first =
        crate::usage_runtime::claude_oauth_reading(an_oauth_reading(20.0), Some(b.id.clone()));
    assert!(crate::usage_runtime::land_claude_usage_as(
        config.path(),
        data.path(),
        Some(&b),
        &first
    ));
    assert!(
        !table_row(config.path(), data.path(), &b.id)
            .windows
            .is_empty()
    );
    assert_eq!(
        super::usage_headroom_of_account(&usage, "claude", None, Some(&as_one))
            .expect("login one's number")
            .used_percent,
        20
    );
    // B is re-logged as login two, and forgets what it read.
    let mut store = crate::accounts::read_store(config.path());
    for account in store.accounts.iter_mut().filter(|held| held.id == b.id) {
        account.account_uuid = Some("t7538r4-late-b-second-person".to_string());
    }
    std::fs::write(
        config.path().join(crate::accounts::ACCOUNT_STORE_FILE),
        serde_json::to_string(&store).expect("json"),
    )
    .expect("the store");
    crate::usage_runtime::forget_claude_account_usage(data.path(), &b.id);
    let relogged = crate::accounts::read_store(config.path())
        .accounts
        .into_iter()
        .find(|held| held.id == b.id)
        .expect("B's row");
    let as_two = crate::agent_teams::PaneLogin {
        account: b.id.clone(),
        login: crate::usage_runtime::claude_login_key(&relogged),
    };
    // Login one's late answer: the status bar shows it under B's id…
    let late =
        crate::usage_runtime::claude_oauth_reading(an_oauth_reading(10.0), Some(b.id.clone()));
    let status_bar = crate::usage_runtime::claude_usage_cache(data.path());
    let shown_before = status_bar.lock().unwrap().clone();
    *status_bar.lock().unwrap() = Some(late.clone());
    // …and lands nowhere an effect reads.
    let landed =
        crate::usage_runtime::land_claude_usage_as(config.path(), data.path(), Some(&b), &late);
    let row = table_row(config.path(), data.path(), &b.id);
    let walled_as_two = super::usage_headroom_of_account(&usage, "claude", None, Some(&as_two));
    *status_bar.lock().unwrap() = shown_before;
    assert!(!landed, "login one's late read landed under login two");
    assert!(row.windows.is_empty(), "{row:?}");
    assert_eq!(row.status, "unknown");
    assert!(
        walled_as_two.is_none(),
        "a pane of login two was judged by login one's number: {walled_as_two:?}"
    );
}

/// `ids` as [`a_claude_store`] makes them, each signed in: its store holds a
/// login of its own — a fixture token that names the account — beside the
/// identity its row names (t-7538 r5).
fn signed_in_claude_store(
    root: &Path,
    ids: &[&str],
    selected: &str,
) -> Vec<zerocode_core::ClaudeAccount> {
    let accounts = a_claude_store(root, ids, selected);
    for account in &accounts {
        let login = serde_json::json!({
            "claudeAiOauth": {"accessToken": format!("fixture-{}", account.id)},
            "oauthAccount": {
                "emailAddress": account.email,
                "accountUuid": account.account_uuid,
                "organizationUuid": account.organization_uuid,
            },
        });
        std::fs::write(
            Path::new(&account.config_dir).join(crate::accounts::CREDENTIALS_FILE),
            login.to_string(),
        )
        .expect("a login in the account's store");
    }
    accounts
}

/// The store selects `id` now, as the selection road writes it.
fn select_in_store(root: &Path, id: &str) {
    let mut store = crate::accounts::read_store(root);
    store.selection.active = Some(id.to_string());
    std::fs::write(
        root.join(crate::accounts::ACCOUNT_STORE_FILE),
        serde_json::to_string(&store).expect("json"),
    )
    .expect("the store");
}

/// One change a test makes from inside the selected read's doors.
type ReadMove = Box<dyn FnOnce() + Send>;

/// The selected account's doors with no keychain and no server (astra
/// R6-1). A login is found where the read's environment points: the
/// account store it names, standing in for that store's own keychain item —
/// or, when `file` says, the runtime home's credentials file, the half of
/// the walk that reads the one home. The endpoint answers each account's
/// fixture token with that account's number. A test may move the world once
/// inside the preparation — after the look, before the login — and once
/// inside the ask — after the login, before the answer.
struct SelectedWorld {
    used: Vec<(String, f32)>,
    file: bool,
    at_prepare: Mutex<Option<ReadMove>>,
    at_ask: Mutex<Option<ReadMove>>,
}

impl SelectedWorld {
    fn answering(used: &[(&zerocode_core::ClaudeAccount, f32)], file: bool) -> Self {
        Self {
            used: used
                .iter()
                .map(|(account, used)| (format!("fixture-{}", account.id), *used))
                .collect(),
            file,
            at_prepare: Mutex::new(None),
            at_ask: Mutex::new(None),
        }
    }
}

impl crate::usage_runtime::SelectedDoors for SelectedWorld {
    fn prepare(
        &self,
        _config_root: &Path,
        _account: &zerocode_core::ClaudeAccount,
    ) -> Result<(), String> {
        let moving = self.at_prepare.lock().unwrap().take();
        if let Some(moving) = moving {
            moving();
        }
        Ok(())
    }

    fn login(&self, env: &[(String, String)]) -> Option<(String, crate::accounts::LoginFrom)> {
        let named = |var: &str| {
            env.iter()
                .rev()
                .find(|(key, _)| key == var)
                .map(|(_, dir)| PathBuf::from(dir))
        };
        let (dir, from) = if self.file {
            (
                named(zerocode_core::account::CONFIG_DIR_VAR)?,
                crate::accounts::LoginFrom::File,
            )
        } else {
            (
                named(zerocode_core::account::SECURE_STORAGE_CONFIG_DIR_VAR)?,
                crate::accounts::LoginFrom::Keychain,
            )
        };
        std::fs::read_to_string(dir.join(crate::accounts::CREDENTIALS_FILE))
            .ok()
            .map(|login| (login, from))
    }

    fn ask(
        &self,
        login: Option<&str>,
        _now_ms: i64,
    ) -> Result<crate::usage_oauth::OauthUsage, crate::usage_http::Failure> {
        let moving = self.at_ask.lock().unwrap().take();
        if let Some(moving) = moving {
            moving();
        }
        let login = login.ok_or_else(crate::usage_http::Failure::no_credentials)?;
        let used = self
            .used
            .iter()
            .find(|(token, _)| login.contains(token.as_str()))
            .map(|(_, used)| *used)
            .expect("a login this test made");
        Ok(an_oauth_reading(used))
    }
}

/// The session figure an account's row in the switch table holds, if any.
fn session_in_table(config: &Path, data: &Path, id: &str) -> Option<u8> {
    table_row(config, data, id)
        .windows
        .iter()
        .find(|window| window.kind == "session")
        .map(|window| window.used_percent)
}

/// astra R6-1: the selected account's read takes the row it files its
/// answer under and the environment it asks with in ONE look, out of one
/// read of the store. A (100%) and B (10%) each read as themselves, filed
/// under themselves. Then, with A selected, the selection moves to B after
/// the read took its look and before it read its login: the read asks with
/// the look's login — A's — and A's own number lands under A; B's number
/// never does. And a login read before the selection moved keeps its late
/// answer under the row it was read for.
#[test]
fn a_selected_read_files_its_answer_under_the_row_whose_login_it_asked_with() {
    let config = tempfile::tempdir().expect("a config root");
    let data = tempfile::tempdir().expect("a data root");
    let accounts = signed_in_claude_store(
        config.path(),
        &["t7538r5-look-a", "t7538r5-look-b"],
        "t7538r5-look-a",
    );
    let (a, b) = (accounts[0].clone(), accounts[1].clone());
    let doors = SelectedWorld::answering(&[(&a, 100.0), (&b, 10.0)], false);
    let read = || {
        crate::usage_runtime::read_selected_claude_usage(config.path(), data.path(), &doors).usage
    };
    let session = |usage: &crate::usage::ProviderUsage| {
        usage.session.as_ref().map(|window| window.used_percent)
    };
    // Nothing moves: each account reads as itself.
    let as_a = read();
    assert_eq!(
        (as_a.account.as_deref(), session(&as_a)),
        (Some(a.id.as_str()), Some(100))
    );
    select_in_store(config.path(), &b.id);
    let as_b = read();
    assert_eq!(
        (as_b.account.as_deref(), session(&as_b)),
        (Some(b.id.as_str()), Some(10))
    );
    assert_eq!(
        session_in_table(config.path(), data.path(), &a.id),
        Some(100)
    );
    assert_eq!(
        session_in_table(config.path(), data.path(), &b.id),
        Some(10)
    );
    // The selection moves under the read, between its look and its login.
    select_in_store(config.path(), &a.id);
    let root = config.path().to_path_buf();
    let to_b = b.id.clone();
    *doors.at_prepare.lock().unwrap() = Some(Box::new(move || select_in_store(&root, &to_b)));
    let raced = read();
    assert_eq!(
        (raced.account.as_deref(), session(&raced)),
        (Some(a.id.as_str()), Some(100)),
        "a read filed under A asked with B's login: {raced:?}"
    );
    assert_eq!(
        session_in_table(config.path(), data.path(), &a.id),
        Some(100),
        "B's number landed under A"
    );
    assert_eq!(
        session_in_table(config.path(), data.path(), &b.id),
        Some(10)
    );
    // A login read before the selection moved: its late answer is still A's.
    select_in_store(config.path(), &a.id);
    let root = config.path().to_path_buf();
    let to_b = b.id.clone();
    *doors.at_ask.lock().unwrap() = Some(Box::new(move || select_in_store(&root, &to_b)));
    let late = read();
    assert_eq!(
        (late.account.as_deref(), session(&late)),
        (Some(a.id.as_str()), Some(100))
    );
    assert_eq!(
        session_in_table(config.path(), data.path(), &a.id),
        Some(100)
    );
}

/// astra R6-1, the runtime home's half: a login read out of the one home is
/// the selected row's only while the home's record names the row and no
/// login was put into the home since the read took its look. With A's
/// login in the home, a read of A through the home is A's. When the switch
/// to B puts B's login into the home between the look and the read — or has
/// put it there already while the store still selects A, or is in the
/// middle of putting it there, B's login in the home and the record still
/// naming A — the read cannot say whose login it used: nothing lands under
/// A, and the bar says the number is nobody's.
#[test]
fn a_selected_read_through_the_runtime_home_is_the_rows_only_while_the_home_holds_its_login() {
    let config = tempfile::tempdir().expect("a config root");
    let data = tempfile::tempdir().expect("a data root");
    let accounts = signed_in_claude_store(
        config.path(),
        &["t7538r5-home-a", "t7538r5-home-b"],
        "t7538r5-home-a",
    );
    let (a, b) = (accounts[0].clone(), accounts[1].clone());
    let doors = SelectedWorld::answering(&[(&a, 100.0), (&b, 10.0)], true);
    let read = || {
        crate::usage_runtime::read_selected_claude_usage(config.path(), data.path(), &doors).usage
    };
    crate::accounts::materialize(config.path(), &a).expect("A's login in the one home");
    let as_a = read();
    assert_eq!(as_a.status, "ok", "{as_a:?}");
    assert_eq!(as_a.account.as_deref(), Some(a.id.as_str()));
    assert_eq!(
        session_in_table(config.path(), data.path(), &a.id),
        Some(100)
    );
    // The switch puts B's login into the home between the look and the read.
    let (root, to_b) = (config.path().to_path_buf(), b.clone());
    *doors.at_prepare.lock().unwrap() = Some(Box::new(move || {
        crate::accounts::materialize(&root, &to_b).expect("B's login in the one home");
    }));
    let raced = read();
    assert_eq!(raced.account.as_deref(), Some(a.id.as_str()));
    assert_eq!(raced.status, "error", "{raced:?}");
    assert!(raced.session.is_none(), "{raced:?}");
    assert_eq!(
        session_in_table(config.path(), data.path(), &a.id),
        Some(100),
        "B's login's number landed under A"
    );
    // B's login stands in the home; the store still selects A — the switch
    // is between its put and its selection.
    let waiting = read();
    assert_eq!(waiting.status, "error", "{waiting:?}");
    assert_eq!(
        session_in_table(config.path(), data.path(), &a.id),
        Some(100)
    );
    // A put under way: B's login is in the home, the record still names A.
    crate::accounts::materialize(config.path(), &a).expect("A's login back in the one home");
    let (in_the_middle, middle) = std::sync::mpsc::channel::<()>();
    let (go_on, gone_on) = std::sync::mpsc::channel::<()>();
    let putting = std::sync::Arc::new(Mutex::new(None));
    let (root, to_b, slot) = (
        config.path().to_path_buf(),
        b.clone(),
        std::sync::Arc::clone(&putting),
    );
    *doors.at_prepare.lock().unwrap() = Some(Box::new(move || {
        let home = crate::accounts::runtime_home(&root);
        let put = std::thread::spawn(move || {
            crate::accounts::before_the_record_of(
                &home,
                Box::new(move || {
                    in_the_middle.send(()).expect("the read waits for the put");
                    gone_on.recv().expect("the read is done");
                }),
            );
            crate::accounts::materialize(&root, &to_b).expect("B's login in the one home");
        });
        middle.recv().expect("the put reached its record");
        *slot.lock().unwrap() = Some(put);
    }));
    let beside = read();
    go_on.send(()).expect("the put goes on");
    putting
        .lock()
        .unwrap()
        .take()
        .expect("the put")
        .join()
        .expect("the put ends");
    assert_eq!(beside.status, "error", "{beside:?}");
    assert_eq!(
        session_in_table(config.path(), data.path(), &a.id),
        Some(100),
        "the number of a login put into the home under the read landed under A"
    );
    assert!(
        table_row(config.path(), data.path(), &b.id)
            .windows
            .is_empty(),
        "a read of A landed under B"
    );
}

/// astra R6-2: a put that ended is not a put whose record landed. A's login
/// stands in the home and reads as A's. The switch to B puts B's login into
/// the home, and the disk refuses the record's last replace — the record
/// stays readable, naming A. The put is over; the store still selects A,
/// the switch between its put and its selection. A read that starts now
/// finds B's login in the home and nothing under way: it cannot say whose
/// login that is, so nothing lands under A and nothing under B. Once a put
/// of B is recorded, B's read is B's; once A's is again, A's is A's.
#[test]
fn a_put_whose_record_the_disk_refused_leaves_the_homes_login_nobodys_until_a_put_is_recorded() {
    let config = tempfile::tempdir().expect("a config root");
    let data = tempfile::tempdir().expect("a data root");
    let accounts = signed_in_claude_store(
        config.path(),
        &["t7538r6-put-a", "t7538r6-put-b"],
        "t7538r6-put-a",
    );
    let (a, b) = (accounts[0].clone(), accounts[1].clone());
    let doors = SelectedWorld::answering(&[(&a, 100.0), (&b, 10.0)], true);
    let read = || {
        crate::usage_runtime::read_selected_claude_usage(config.path(), data.path(), &doors).usage
    };
    let session = |usage: &crate::usage::ProviderUsage| {
        usage.session.as_ref().map(|window| window.used_percent)
    };
    crate::accounts::materialize(config.path(), &a).expect("A's login in the one home");
    let as_a = read();
    assert_eq!(as_a.status, "ok", "{as_a:?}");
    assert_eq!(
        session_in_table(config.path(), data.path(), &a.id),
        Some(100)
    );
    // B's login goes into the home; the record's last replace is refused.
    let root = config.path().to_path_buf();
    crate::accounts::before_the_record_of(
        &crate::accounts::runtime_home(config.path()),
        Box::new(move || crate::accounts::refuse_the_next_record_of(&root)),
    );
    let put = crate::accounts::materialize(config.path(), &b);
    let after = read();
    assert_eq!(
        session_in_table(config.path(), data.path(), &a.id),
        Some(100),
        "B's login's number landed under A after a put whose record was refused: {after:?}"
    );
    assert_eq!(after.status, "error", "{after:?}");
    assert!(after.session.is_none(), "{after:?}");
    assert!(
        table_row(config.path(), data.path(), &b.id)
            .windows
            .is_empty(),
        "a read of A landed under B"
    );
    assert!(
        put.is_err(),
        "a put whose record was refused answered as a put that happened"
    );
    // The put again, recorded: B's login is B's.
    crate::accounts::materialize(config.path(), &b).expect("B's put, recorded");
    select_in_store(config.path(), &b.id);
    let as_b = read();
    assert_eq!(as_b.status, "ok", "{as_b:?}");
    assert_eq!(
        (as_b.account.as_deref(), session(&as_b)),
        (Some(b.id.as_str()), Some(10))
    );
    assert_eq!(
        session_in_table(config.path(), data.path(), &b.id),
        Some(10)
    );
    // And A's put, recorded: A's login is A's again.
    select_in_store(config.path(), &a.id);
    crate::accounts::materialize(config.path(), &a).expect("A's put, recorded");
    let back = read();
    assert_eq!(back.status, "ok", "{back:?}");
    assert_eq!(
        (back.account.as_deref(), session(&back)),
        (Some(a.id.as_str()), Some(100))
    );
    assert_eq!(
        session_in_table(config.path(), data.path(), &a.id),
        Some(100)
    );
}
