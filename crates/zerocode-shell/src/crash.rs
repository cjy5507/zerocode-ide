//! Crash evidence, boot consumption and allowlisted local diagnostics bundles.

use std::io::{Read as _, Seek as _};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{crumbs, durable_file};

/// One numbers table; only the watchdog switch and thresholds are overlaid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Limits {
    pub(crate) watchdog: bool,
    pub(crate) first_ms: u64,
    pub(crate) second_ms: u64,
    pub(crate) ping_ms: u64,
    pub(crate) warmup_ms: u64,
    pub(crate) threshold_max_ms: u64,
}

impl Limits {
    #[cfg(target_os = "macos")]
    pub(crate) const SAMPLE_SECONDS: u64 = 1;
    #[cfg(target_os = "macos")]
    pub(crate) const SAMPLE_INTERVAL_MS: u64 = 10;
    #[cfg(target_os = "macos")]
    pub(crate) const SAMPLE_TIMEOUT_MS: u64 = 10000;
    #[cfg(target_os = "macos")]
    pub(crate) const SAMPLE_BYTES: u64 = 2 * 1024 * 1024;
    pub(crate) const SAMPLE_FRAMES: usize = 64;
    pub(crate) const RING_SIZE: usize = 256;
    pub(crate) const LAST_CRUMBS: usize = 96;
    pub(crate) const TEXT_BYTES: usize = 384;
    pub(crate) const LOG_LINES: usize = 200;
    pub(crate) const LOG_BYTES: u64 = 262144;
    pub(crate) const REPORT_BYTES: u64 = 262144;
    /// The crash task's own bounds (t-3014 §2.4): how many symbol frames
    /// and how many of the LAST crumbs ride in the task body, how far back
    /// the same `file:line` is counted as a repeat, and how many history
    /// rows a boot loop may leave behind.
    pub(crate) const TASK_FRAMES: usize = 12;
    pub(crate) const TASK_CRUMBS: usize = 24;
    pub(crate) const REPEAT_WINDOW_MS: u64 = 7 * 24 * 60 * 60 * 1000;
    pub(crate) const HISTORY_ROWS: usize = 128;
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const WRITE_BUDGET_NS: u128 = 20000;
    pub(crate) const DEFAULT: Self = Self {
        watchdog: true,
        first_ms: 2000,
        second_ms: 6000,
        ping_ms: 250,
        warmup_ms: 15000,
        threshold_max_ms: 120000,
    };

    /// How late the watchdog's own beat may come before it cannot testify
    /// (t-6388): one whole beat. It judges the main thread by an uptime
    /// clock that keeps running while macOS gives this process no CPU — a
    /// lid-closed DarkWake, a sleep being entered or left — and nearly every
    /// hang report of 2026-09-15..24 was that stopped time, not the main
    /// thread. Measured on the observer's own road on 2026-09-24: under load
    /// average 88–117 on 12 cores a 250 ms nap overshot p50 6.4 ms, p99
    /// 12.4 ms, max 21.3 ms (n=469), so a whole missed beat is never load.
    pub(crate) const fn late_ms(self) -> u64 {
        self.ping_ms
    }

    pub(crate) fn overlay(value: &serde_json::Value) -> Self {
        let mut limits = Self::DEFAULT;
        limits.watchdog = value["watchdog"].as_bool().unwrap_or(limits.watchdog);
        limits.first_ms = value["first_ms"]
            .as_u64()
            .unwrap_or(limits.first_ms)
            .clamp(limits.ping_ms, limits.threshold_max_ms - limits.ping_ms);
        limits.second_ms = value["second_ms"]
            .as_u64()
            .unwrap_or(limits.second_ms)
            .clamp(limits.first_ms + limits.ping_ms, limits.threshold_max_ms);
        limits
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Kind {
    Panic,
    Hang,
    Killed,
}

/// The same public build vocabulary as session.capabilities.process.build.
/// These stamps describe this shell; a zo receipt remains zo's own identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BuildIdentity {
    version: String,
    id: String,
    git_sha: String,
    dirty: bool,
    ui_digest: String,
}

/// The commit under a build stamp, with the `-dirty` mark taken off.
///
/// One rule, read by the crash report here and by the update notice
/// (`update_runtime`): a stamp that says `<sha>-dirty` names the same
/// snapshot as `<sha>`, so a comparison against an installed sha strips it
/// the way this report does. `-unverified` is left on deliberately — that
/// stamp means the tree was never checked, and a sha nobody verified should
/// not pass as equal to one the lane installed.
pub(crate) fn plain_sha(commit: &str) -> &str {
    commit.strip_suffix("-dirty").unwrap_or(commit)
}

pub(crate) fn build_identity() -> BuildIdentity {
    let commit = env!("ZEROCODE_COMMIT");
    BuildIdentity {
        version: env!("CARGO_PKG_VERSION").into(),
        id: commit.into(),
        git_sha: plain_sha(commit).into(),
        dirty: commit.ends_with("-dirty"),
        ui_digest: env!("ZEROCODE_UI_DIGEST").into(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ZoBuild {
    version: Option<String>,
    id: Option<String>,
    git_sha: Option<String>,
    dirty: Option<bool>,
}

fn public_build_string(value: &str) -> String {
    // Both producers publish SHA-based ids: shell uses SHA-dirty, while
    // session.capabilities uses SHA-build-time. They are public identities,
    // not the opaque credential-shaped strings the generic crumb mask drops.
    if value.len() <= Limits::TEXT_BYTES
        && value.split_once('-').is_some_and(|(sha, suffix)| {
            !sha.is_empty()
                && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
                && (matches!(suffix, "dirty" | "unverified")
                    || (!suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())))
        })
    {
        return value.to_string();
    }
    crumbs::masked(value).as_str().to_string()
}

impl ZoBuild {
    fn mask(&mut self) {
        for value in [&mut self.version, &mut self.id, &mut self.git_sha]
            .into_iter()
            .flatten()
        {
            *value = public_build_string(value);
        }
    }
}

static ZO_BUILD: std::sync::Mutex<Option<ZoBuild>> = std::sync::Mutex::new(None);

/// Keep exactly the public build leaves from a verified session.capabilities
/// receipt. The report names shell and zo separately rather than attributing
/// this executable's git stamp to the independently built session process.
pub(crate) fn note_zo_build(value: &serde_json::Value) {
    if let Ok(mut build) = serde_json::from_value::<ZoBuild>(value.clone()) {
        build.mask();
        if let Ok(mut held) = ZO_BUILD.try_lock() {
            *held = Some(build);
        }
    }
}

fn zo_build() -> Option<ZoBuild> {
    ZO_BUILD.try_lock().ok().and_then(|held| held.clone())
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LedgerSummary {
    pub(crate) revision: u64,
    pub(crate) workers: u64,
    pub(crate) dispatches: u64,
    pub(crate) at_ms: u64,
}

static LEDGER_STAMP: AtomicU64 = AtomicU64::new(0);
static LEDGER_REVISION: AtomicU64 = AtomicU64::new(0);
static LEDGER_WORKERS: AtomicU64 = AtomicU64::new(0);
static LEDGER_DISPATCHES: AtomicU64 = AtomicU64::new(0);
static LEDGER_AT: AtomicU64 = AtomicU64::new(0);

/// Called while a normal ledger view is already in hand. A panic never asks
/// the actor or a pane lock for a snapshot; the timestamp says how fresh it is.
pub(crate) fn note_ledger(revision: u64, workers: u64, dispatches: u64) {
    let stamp = LEDGER_STAMP.load(SeqCst);
    if !stamp.is_multiple_of(2)
        || LEDGER_STAMP
            .compare_exchange(stamp, stamp + 1, SeqCst, SeqCst)
            .is_err()
    {
        return;
    }
    LEDGER_REVISION.store(revision, SeqCst);
    LEDGER_WORKERS.store(workers, SeqCst);
    LEDGER_DISPATCHES.store(dispatches, SeqCst);
    LEDGER_AT.store(now(), SeqCst);
    LEDGER_STAMP.store(stamp + 2, SeqCst);
}

fn ledger_summary() -> LedgerSummary {
    let stamp = LEDGER_STAMP.load(SeqCst);
    if !stamp.is_multiple_of(2) {
        return LedgerSummary::default();
    }
    let snapshot = LedgerSummary {
        revision: LEDGER_REVISION.load(SeqCst),
        workers: LEDGER_WORKERS.load(SeqCst),
        dispatches: LEDGER_DISPATCHES.load(SeqCst),
        at_ms: LEDGER_AT.load(SeqCst),
    };
    if LEDGER_STAMP.load(SeqCst) == stamp {
        snapshot
    } else {
        LedgerSummary::default()
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct ProcessRecord {
    id: String,
    pid: u32,
    at_ms: u64,
    build: BuildIdentity,
}

static PROCESS: OnceLock<ProcessRecord> = OnceLock::new();

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Report {
    id: String,
    process: ProcessRecord,
    at_ms: u64,
    kind: Kind,
    summary: String,
    crumbs: Vec<crumbs::Crumb>,
    build: BuildIdentity,
    ledger: LedgerSummary,
    #[serde(default)]
    zo_build: Option<ZoBuild>,
    /// Where a panic was raised — `file:line`, said by `public_site` without
    /// the machine it was built on — and on which thread. The same two facts
    /// the panic hook writes to the log's headline, kept here as NAMED
    /// fields so the crash task (t-3014 §2.4) never reads a log to learn
    /// them. `None` for a hang or a kill, and for every report written
    /// before these existed.
    #[serde(default)]
    site: Option<String>,
    #[serde(default)]
    thread: Option<String>,
    /// The top symbol frames of the forced backtrace, kept by the same rule
    /// the diagnostics bundle applies to the log (`symbol_frame`): a frame
    /// index and a `::` symbol, never a source path or a payload. Bounded
    /// at `Limits::TASK_FRAMES` when written.
    #[serde(default)]
    frames: Vec<String>,
}

/// What a panic knows about itself beyond its payload, handed to [`record`]
/// by the hook that has it in hand. The hang watchdog supplies sampled native
/// symbols and the main thread name, leaving the panic site absent.
#[derive(Clone, Copy)]
pub(crate) struct Origin<'a> {
    pub(crate) site: Option<(&'a str, u32)>,
    pub(crate) thread: Option<&'a str>,
    pub(crate) backtrace: Option<&'a str>,
}

impl Origin<'static> {
    pub(crate) const NONE: Self = Self {
        site: None,
        thread: None,
        backtrace: None,
    };
}

/// One line of `crash/history.jsonl`: enough to count how often the same
/// place has crashed lately, and nothing a report does not already say.
#[derive(Clone, Serialize, Deserialize)]
struct HistoryRow {
    id: String,
    at_ms: u64,
    kind: Kind,
    site: Option<String>,
}

/// The stamp beside `consumed.json`: which report was filed as which task.
#[derive(Clone, Serialize, Deserialize)]
struct Triaged {
    report: String,
    task: String,
}

/// The incident the sheet showed, ready to go down the ledger's door.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Triage {
    pub(crate) report: String,
    pub(crate) argv: Vec<String>,
}

#[derive(Clone, Serialize)]
pub(crate) struct LastCrash {
    at_ms: u64,
    kind: Kind,
    summary: String,
    crumbs_path: String,
    bundle_ready: bool,
    crumbs: Vec<crumbs::Crumb>,
    ledger: LedgerSummary,
}

fn now() -> u64 {
    crate::now_epoch_ms().max(0) as u64
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    // The existing primitive uses a private same-directory tmp + atomic rename
    // (MoveFileExW on Windows), syncs contents and never follows a symlink.
    durable_file::replace_bytes(path, &bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let mut file = durable_file::open_plain_file(path).ok()?;
    if file.metadata().ok()?.len() > Limits::REPORT_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn process_record() -> ProcessRecord {
    ProcessRecord {
        id: uuid::Uuid::new_v4().to_string(),
        pid: std::process::id(),
        at_ms: now(),
        build: build_identity(),
    }
}

/// Before the state and ledger recover: their normal boot cannot overwrite
/// the evidence of the process that disappeared. A clean stamp names one
/// process incarnation, so PID reuse and an older stamp cannot hide a kill.
pub(crate) fn begin(root: &Path) -> Result<(), String> {
    let process = process_record();
    begin_process(root, &process)?;
    let _ = PROCESS.set(process);
    crumbs::record("boot", format_args!("process"));
    Ok(())
}

fn begin_process(root: &Path, process: &ProcessRecord) -> Result<(), String> {
    let dir = root.join("crash");
    if let Some(previous) = read_json::<ProcessRecord>(&dir.join("process.json")) {
        let clean = read_json::<String>(&dir.join("clean-exit.json"));
        let evidence = read_json::<Report>(&dir.join("last.json"));
        if clean.as_deref() != Some(previous.id.as_str())
            && evidence
                .as_ref()
                .is_none_or(|report| report.process.id != previous.id)
        {
            let report = Report {
                id: uuid::Uuid::new_v4().to_string(),
                build: previous.build.clone(),
                process: previous,
                at_ms: now(),
                kind: Kind::Killed,
                summary: "previous process ended without a clean-exit stamp".into(),
                // SIGKILL cannot flush a memory ring. Do not invent old crumbs
                // from this boot or a report belonging to another process.
                crumbs: Vec::new(),
                ledger: LedgerSummary::default(),
                zo_build: None,
                site: None,
                thread: None,
                frames: Vec::new(),
            };
            persist(root, &report)?;
        }
    }
    write_json(&dir.join("process.json"), process)
}

pub(crate) fn clean_exit(root: &Path) -> Result<(), String> {
    match PROCESS.get() {
        Some(process) => write_json(&root.join("crash/clean-exit.json"), &process.id),
        None => Ok(()),
    }
}

pub(crate) fn record(
    root: &Path,
    kind: Kind,
    summary: &str,
    origin: Origin<'_>,
) -> Result<(), String> {
    let process = PROCESS.get().cloned().unwrap_or_else(process_record);
    let mut rows = crumbs::snapshot();
    if rows.len() > Limits::LAST_CRUMBS {
        rows.drain(..rows.len() - Limits::LAST_CRUMBS);
    }
    let masked = |value: &str| crumbs::masked(value).as_str().to_string();
    persist(
        root,
        &Report {
            id: uuid::Uuid::new_v4().to_string(),
            at_ms: now(),
            kind,
            summary: masked(summary),
            crumbs: rows,
            build: process.build.clone(),
            process,
            ledger: ledger_summary(),
            zo_build: zo_build(),
            site: origin.site.map(|(file, line)| public_site(file, line)),
            thread: origin.thread.map(masked),
            frames: origin
                .backtrace
                .map(|text| {
                    text.lines()
                        .filter_map(symbol_frame)
                        .take(Limits::TASK_FRAMES)
                        .collect()
                })
                .unwrap_or_default(),
        },
    )
}

/// A compiler symbol frame of a backtrace, or nothing: `N: a::b::c`, the
/// index digits and a `::` symbol over a closed alphabet. The absolute
/// source-path line under it and any payload text fail the rule. One rule,
/// read by the diagnostics bundle's log tail and by the crash task's frames.
///
/// The alphabet IS the mask: no `=`, quote, `-` or `/` can pass, so no
/// credential value can ride a frame. The free-text word mask is not laid
/// over it — that mask reads `claude_tokens::refresh` as a credential key
/// and a long symbol with a digit as a credential value, and the frame a
/// crash task most needs would be the one it hid.
fn symbol_frame(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let (index, symbol) = trimmed.split_once(": ")?;
    (!index.is_empty()
        && index.bytes().all(|byte| byte.is_ascii_digit())
        && symbol.contains("::")
        && symbol
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_:<>{}.".contains(&byte)))
    .then(|| trimmed.to_string())
}

/// A panic's `file:line`, said without the machine it was built on.
///
/// A workspace-relative path and a `/rustc/` path are public as they are; a
/// registry crate is named from its own directory (`tauri-2.8.0/src/…`);
/// anything else absolute — a home directory, a build machine — keeps its
/// file and parent only, because the person's directory name is the secret
/// and the file is the evidence. The free-text word mask is not the rule
/// here: it reads any path over 24 characters with a digit as a credential
/// and would hide every site in this repository. Any other alphabet is
/// `[redacted]`.
fn public_site(file: &str, line: u32) -> String {
    let unix = file.replace('\\', "/");
    let absolute = unix.starts_with('/') || unix.as_bytes().get(1) == Some(&b':');
    let tail: String = if let Some((_, rest)) = unix.split_once("/registry/src/") {
        rest.split_once('/')
            .map_or(rest, |(_, crate_path)| crate_path)
            .to_string()
    } else if unix.starts_with("/rustc/") || !absolute {
        unix.clone()
    } else {
        let mut parts: Vec<&str> = unix.rsplit('/').take(2).collect();
        parts.reverse();
        parts.join("/")
    };
    if tail.is_empty()
        || !tail
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_./+-".contains(&byte))
    {
        return "[redacted]".to_string();
    }
    format!("{tail}:{line}")
}

fn persist(root: &Path, report: &Report) -> Result<(), String> {
    // last.json owns the snapshot; crumbs.jsonl is a convenience export and
    // is recreated from that snapshot at consumption, never trusted as input.
    write_json(&root.join("crash/last.json"), report)?;
    // One appended line so the next boot can count how often this place has
    // crashed lately (t-3014 §2.4). A convenience beside the snapshot: its
    // failure never costs the report, and it is bounded at its reader.
    let _ = append_history(root, report);
    Ok(())
}

fn append_history(root: &Path, report: &Report) -> Result<(), String> {
    use std::io::Write as _;
    let row = HistoryRow {
        id: report.id.clone(),
        at_ms: report.at_ms,
        kind: report.kind,
        site: report.site.clone(),
    };
    let mut bytes = serde_json::to_vec(&row).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("crash/history.jsonl"))
        .map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())
}

/// The history rows, newest last, pruned to the table on the way in: a boot
/// loop that appended past `HISTORY_ROWS` is cut back to the last rows and
/// the file rewritten, so the file never grows beyond one read.
fn history(root: &Path) -> Vec<HistoryRow> {
    let path = root.join("crash/history.jsonl");
    let Ok(mut file) = durable_file::open_plain_file(&path) else {
        return Vec::new();
    };
    if file
        .metadata()
        .ok()
        .is_none_or(|meta| meta.len() > Limits::REPORT_BYTES)
    {
        let _ = durable_file::replace_bytes(&path, b"");
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let mut rows: Vec<HistoryRow> = String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    if rows.len() > Limits::HISTORY_ROWS {
        rows.drain(..rows.len() - Limits::HISTORY_ROWS);
        let mut kept = Vec::new();
        for row in &rows {
            if serde_json::to_writer(&mut kept, row).is_ok() {
                kept.push(b'\n');
            }
        }
        let _ = durable_file::replace_bytes(&path, &kept);
    }
    rows
}

/// How many reports inside the window — this one included — name the same
/// `file:line`. One for a first crash, and nothing for a report without a
/// site: a hang has no place to repeat at.
fn repeats_of(root: &Path, report: &Report) -> usize {
    let Some(site) = report.site.as_deref() else {
        return 0;
    };
    let since = report.at_ms.saturating_sub(Limits::REPEAT_WINDOW_MS);
    history(root)
        .iter()
        .filter(|row| {
            row.site.as_deref() == Some(site) && row.at_ms >= since && row.at_ms <= report.at_ms
        })
        .count()
        .max(1)
}

/// A bounded prefix on a char boundary, for the one line the table caps.
fn capped(text: &str, most: usize) -> &str {
    if text.len() <= most {
        return text;
    }
    let mut end = most;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The task's body, from the report's NAMED fields and nothing else — never
/// `env::vars`, never a log: the same allowlist principle the diagnostics
/// bundle stands on. Every line inherits the report's mask (its fields were
/// masked when written) and the table's bounds.
fn task_body(report: &Report, repeats: usize) -> String {
    let kind = match report.kind {
        Kind::Panic => "panic",
        Kind::Hang => "hang",
        Kind::Killed => "killed",
    };
    let mut body = format!("{kind}: {}\n", capped(&report.summary, Limits::TEXT_BYTES));
    if repeats >= 2 {
        body.push_str(&format!("같은 자리 {repeats}번째\n"));
    }
    if let Some(site) = &report.site {
        let thread = report.thread.as_deref().unwrap_or("<unnamed>");
        body.push_str(&format!(
            "{kind}: {} @ {site} [thread {thread}]\n",
            capped(&report.summary, Limits::TEXT_BYTES)
        ));
    }
    let frames: Vec<&str> = report
        .frames
        .iter()
        .take(Limits::TASK_FRAMES)
        .map(String::as_str)
        .collect();
    if !frames.is_empty() {
        body.push_str("frames:\n");
        for frame in frames {
            body.push_str("  ");
            body.push_str(frame);
            body.push('\n');
        }
    }
    let skip = report.crumbs.len().saturating_sub(Limits::TASK_CRUMBS);
    if skip < report.crumbs.len() {
        body.push_str("crumbs:\n");
        for row in &report.crumbs[skip..] {
            body.push_str(&format!(
                "  {} {}\n",
                row.at_ms,
                crumbs::masked(&row.line).as_str()
            ));
        }
    }
    body.push_str(&format!(
        "ledger: revision {} · workers {} · dispatches {} · at {}\n",
        report.ledger.revision,
        report.ledger.workers,
        report.ledger.dispatches,
        report.ledger.at_ms
    ));
    body.push_str(&format!(
        "build: shell {} {} ui {}",
        public_build_string(&report.build.version),
        public_build_string(&report.build.id),
        public_build_string(&report.build.ui_digest)
    ));
    if let Some(zo) = &report.zo_build {
        let mut zo = zo.clone();
        zo.mask();
        body.push_str(&format!(
            " · zo {} {}",
            zo.version.as_deref().unwrap_or("?"),
            zo.id.as_deref().unwrap_or("?")
        ));
    }
    body.push('\n');
    body
}

/// The `task-create` verb for one report, element by element — the same
/// shape the standing-order beat builds `worker-start` in, for the same
/// reason: a spec is a sentence, and a line split on whitespace would hand
/// the ledger the first word. The request name is the report id, so a
/// retry after a lost answer is the SAME request and never a second task.
/// A kill has no evidence and files nothing.
pub(crate) fn task_argv(report: &Report, repeats: usize) -> Option<Vec<String>> {
    if report.kind == Kind::Killed {
        return None;
    }
    Some(vec![
        "task-create".to_string(),
        "--retry-request".to_string(),
        format!("crash-{}", report.id),
        "--spec".to_string(),
        task_body(report, repeats),
    ])
}

/// The incident the sheet showed, if it still wants filing: consumed (so
/// `presented.json` names it), not a kill, not yet stamped in
/// `triaged.json`, and not older than the window — a crash from last month
/// is not news a task should carry. Asked on every beat until it is filed,
/// so it reads three small files and touches no ledger.
pub(crate) fn pending_triage(root: &Path, now_ms: u64) -> Option<Triage> {
    let dir = root.join("crash");
    let report = read_json::<Report>(&dir.join("presented.json"))?;
    if read_json::<String>(&dir.join("consumed.json")).as_deref() != Some(report.id.as_str()) {
        return None;
    }
    if read_json::<Triaged>(&dir.join("triaged.json"))
        .is_some_and(|stamp| stamp.report == report.id)
    {
        return None;
    }
    if now_ms.saturating_sub(report.at_ms) > Limits::REPEAT_WINDOW_MS {
        return None;
    }
    let repeats = repeats_of(root, &report);
    Some(Triage {
        report: report.id.clone(),
        argv: task_argv(&report, repeats)?,
    })
}

/// The once-only stamp beside `consumed.json`: this report became that
/// task. Written after the ledger answered, never before — a stamp without
/// a task would silence the next boot for nothing.
pub(crate) fn note_triaged(root: &Path, report: &str, task: &str) -> Result<(), String> {
    write_json(
        &root.join("crash/triaged.json"),
        &Triaged {
            report: report.to_string(),
            task: task.to_string(),
        },
    )
}

pub(crate) fn consume(root: &Path) -> Option<LastCrash> {
    // Two concurrent boot reports must not both consume the same incident.
    static CONSUMING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = CONSUMING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = root.join("crash");
    let report = read_json::<Report>(&dir.join("last.json"))?;
    if read_json::<String>(&dir.join("consumed.json")).as_deref() == Some(report.id.as_str()) {
        return None;
    }
    let path = dir.join("crumbs.jsonl");
    write_crumbs(&path, &report.crumbs).ok()?;
    // The open sheet owns this incident even if a new hang replaces last.json.
    write_json(&dir.join("presented.json"), &report).ok()?;
    write_json(&dir.join("consumed.json"), &report.id).ok()?;
    Some(LastCrash {
        at_ms: report.at_ms,
        kind: report.kind,
        summary: report.summary,
        crumbs_path: path.display().to_string(),
        bundle_ready: dir.join(report.at_ms.to_string()).is_dir(),
        crumbs: report.crumbs,
        ledger: report.ledger,
    })
}

fn write_crumbs(path: &Path, rows: &[crumbs::Crumb]) -> Result<(), String> {
    let mut bytes = Vec::new();
    for row in rows
        .iter()
        .rev()
        .take(Limits::LAST_CRUMBS)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        // Sanitize again at export: files from an older build may predate the mask.
        let clean = crumbs::Crumb {
            seq: row.seq,
            at_ms: row.at_ms,
            line: crumbs::masked(&row.line).as_str().to_string(),
        };
        serde_json::to_writer(&mut bytes, &clean).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
    }
    durable_file::replace_bytes(path, &bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// The environment allowlist is constructed, never selected from env::vars,
/// settings, request headers, credentials, account stores or a whole ledger.
fn environment() -> serde_json::Value {
    json!({"os": std::env::consts::OS, "arch": std::env::consts::ARCH})
}

fn log_tail(root: &Path) -> Result<String, String> {
    let path = root.join("window-errors.log");
    if !path.exists() {
        return Ok(String::new());
    }
    let mut file = durable_file::open_plain_file(&path).map_err(|error| error.to_string())?;
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    file.seek(std::io::SeekFrom::Start(
        length.saturating_sub(Limits::LOG_BYTES),
    ))
    .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(Limits::LOG_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let raw = String::from_utf8_lossy(&bytes);
    let lines: Vec<_> = raw.lines().rev().take(Limits::LOG_LINES).collect();
    let mut safe = String::new();
    for line in lines.into_iter().rev() {
        // Preserve compiler symbol frames, but never the absolute source path
        // line or panic payload. This keeps the stack useful without exporting
        // a user's directory or free-form exception data.
        if let Some(frame) = symbol_frame(line) {
            safe.push_str(&frame);
            safe.push('\n');
            continue;
        }
        // Free-form log messages are NOT an export schema. Keep only the
        // timestamp and recognized diagnostic category, never their payload.
        let (at, body) = line.split_once(' ').unwrap_or(("", line));
        let category = [
            "panic:",
            "hang:",
            "event loop exited",
            "exit requested:",
            "system slept",
            "window destroyed:",
            "term ",
        ]
        .into_iter()
        .find(|prefix| body.starts_with(prefix))
        .unwrap_or("event");
        if at.bytes().all(|byte| byte.is_ascii_digit()) {
            safe.push_str(at);
        }
        safe.push(' ');
        safe.push_str(category);
        safe.push_str(" [details omitted]\n");
    }
    Ok(safe)
}

pub(crate) fn bundle(root: &Path) -> Result<PathBuf, String> {
    let report: Report = read_json(&root.join("crash/presented.json"))
        .or_else(|| read_json(&root.join("crash/last.json")))
        .ok_or("crash evidence unavailable")?;
    let folder = root.join("crash").join(report.at_ms.to_string());
    durable_file::ensure_private_directory(&folder).map_err(|error| error.to_string())?;
    let allowed = [
        "crumbs.jsonl",
        "environment.json",
        "build.json",
        "ledger.json",
        "window-errors.log",
    ];
    for entry in std::fs::read_dir(&folder).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if !allowed.iter().any(|name| entry.file_name() == *name) {
            return Err("diagnostic folder contains an unexpected file".into());
        }
    }
    write_crumbs(&folder.join("crumbs.jsonl"), &report.crumbs)?;
    write_json(&folder.join("environment.json"), &environment())?;
    // A typed build record excludes arbitrary JSON keys. Its string values
    // are sanitized too in case an older producer stored an unexpected value.
    let mut build = report.build;
    for value in [
        &mut build.version,
        &mut build.id,
        &mut build.git_sha,
        &mut build.ui_digest,
    ] {
        *value = public_build_string(value);
    }
    write_json(
        &folder.join("build.json"),
        &json!({"shell":build,"zo":report.zo_build.map(|mut build| { build.mask(); build })}),
    )?;
    write_json(&folder.join("ledger.json"), &report.ledger)?;
    durable_file::replace_bytes(
        &folder.join("window-errors.log"),
        log_tail(root)?.as_bytes(),
    )
    .map_err(|error| error.to_string())?;
    Ok(folder)
}

pub(crate) fn open_local(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "linux")]
    let launcher = "xdg-open";
    #[cfg(target_os = "windows")]
    let launcher = "explorer";
    let mut command = crate::proc::quiet_command(launcher);
    command.arg(path);
    zerocode_core::reap::spawn_forgotten(command)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_persists_identity_crumbs_atomically_and_boot_consumes_once() {
        let root = tempfile::tempdir().unwrap();
        crumbs::record("boot", format_args!("ready"));
        crate::record_panic(
            root.path(),
            &"failure token=short",
            Some(("src/test.rs", 7)),
            Some("main"),
        );
        let path = root.path().join("crash/last.json");
        let report: Report = read_json(&path).expect("panic report");
        assert_eq!(report.kind, Kind::Panic);
        assert_eq!(report.build, build_identity());
        assert!(!report.crumbs.is_empty());
        assert!(report.crumbs.len() <= Limits::LAST_CRUMBS);
        assert!(!report.summary.contains("short"));
        assert!(consume(root.path()).is_some());
        assert!(consume(root.path()).is_none());
        assert!(
            std::fs::read_to_string(root.path().join("window-errors.log"))
                .unwrap()
                .contains("backtrace:")
        );
        assert!(
            std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".tmp"))
        );
    }

    #[test]
    fn killed_requires_a_previous_process_and_its_missing_clean_stamp() {
        let root = tempfile::tempdir().unwrap();
        let previous = process_record();
        begin_process(root.path(), &previous).unwrap();
        assert!(consume(root.path()).is_none());
        write_json(&root.path().join("crash/clean-exit.json"), &previous.id).unwrap();
        let next = process_record();
        begin_process(root.path(), &next).unwrap();
        assert!(consume(root.path()).is_none());
        // An older clean stamp and reused PID do not bless the new incarnation.
        let latest = process_record();
        begin_process(root.path(), &latest).unwrap();
        let said = consume(root.path()).unwrap();
        assert_eq!(said.kind, Kind::Killed);
        assert!(said.crumbs.is_empty());
        assert!(consume(root.path()).is_none());
    }

    #[test]
    fn a_panicked_process_is_not_reclassified_as_killed() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), Kind::Panic, "failure", Origin::NONE).unwrap();
        let report: Report = read_json(&root.path().join("crash/last.json")).unwrap();
        write_json(&root.path().join("crash/process.json"), &report.process).unwrap();
        begin_process(root.path(), &process_record()).unwrap();
        assert_eq!(consume(root.path()).unwrap().kind, Kind::Panic);
        assert!(consume(root.path()).is_none());
    }

    #[test]
    fn bundle_allowlist_never_copies_credentials_email_home_or_raw_payloads() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), Kind::Panic, "failure", Origin::NONE).unwrap();
        std::fs::write(root.path().join("window-errors.log"), "7 panic: token=short person@example.org /Users/person/private\n8 term 7 ended Cookie: private\n9 arbitrary confidential sentence\n   0: zerocode_shell::system_runtime::record_panic\n").unwrap();
        std::fs::write(root.path().join("accounts.json"), "top secret credential").unwrap();
        let folder = bundle(root.path()).unwrap();
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&folder).unwrap() {
            let entry = entry.unwrap();
            names.push(entry.file_name().to_string_lossy().into_owned());
            let text = std::fs::read_to_string(entry.path()).unwrap();
            for secret in [
                "short",
                "person@example.org",
                "/Users/person",
                "Cookie:",
                "private",
                "confidential",
                "credential",
            ] {
                assert!(!text.contains(secret), "bundle leaked {secret}");
            }
        }
        assert!(
            std::fs::read_to_string(folder.join("window-errors.log"))
                .unwrap()
                .contains("zerocode_shell::system_runtime::record_panic")
        );
        names.sort();
        assert_eq!(
            names,
            [
                "build.json",
                "crumbs.jsonl",
                "environment.json",
                "ledger.json",
                "window-errors.log"
            ]
        );
        assert_eq!(
            environment()
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["os", "arch"]
        );
    }

    #[test]
    fn bundle_keeps_the_incident_shown_in_the_sheet_and_refuses_extra_files() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), Kind::Panic, "first", Origin::NONE).unwrap();
        let shown = consume(root.path()).unwrap();
        record(root.path(), Kind::Hang, "second", Origin::NONE).unwrap();
        let mut latest: Report = read_json(&root.path().join("crash/last.json")).unwrap();
        latest.at_ms = shown.at_ms + 1;
        persist(root.path(), &latest).unwrap();
        let folder = bundle(root.path()).unwrap();
        assert_eq!(
            folder.file_name().unwrap(),
            shown.at_ms.to_string().as_str()
        );
        std::fs::write(folder.join("credentials.json"), "private").unwrap();
        assert!(bundle(root.path()).is_err());
    }

    #[test]
    fn verified_zo_build_uses_the_capabilities_values_and_never_waits_in_a_panic() {
        let id = format!("{}-1788633594973", "a1".repeat(20));
        let input = json!({"version":"1.2.3","id":id,"git_sha":"abcdef","dirty":false});
        let mut build: ZoBuild = serde_json::from_value(input.clone()).unwrap();
        build.mask();
        assert_eq!(serde_json::to_value(build).unwrap(), input);
        let shell_id = format!("{}-dirty", "a1".repeat(20));
        assert_eq!(public_build_string(&shell_id), shell_id);
        assert_eq!(
            public_build_string("sk-proj-example123456789"),
            "[redacted]"
        );
        let _held = ZO_BUILD.lock().unwrap();
        assert_eq!(zo_build(), None);
    }

    #[test]
    fn the_second_clean_boot_is_silent_and_leaves_the_report_for_a_bundle() {
        let root = tempfile::tempdir().unwrap();
        record(root.path(), Kind::Panic, "failure", Origin::NONE).unwrap();
        let report: Report = read_json(&root.path().join("crash/last.json")).unwrap();
        write_json(&root.path().join("crash/process.json"), &report.process).unwrap();
        let next = process_record();
        begin_process(root.path(), &next).unwrap();
        assert!(consume(root.path()).is_some());
        write_json(&root.path().join("crash/clean-exit.json"), &next.id).unwrap();
        begin_process(root.path(), &process_record()).unwrap();
        assert!(consume(root.path()).is_none());
        assert!(bundle(root.path()).unwrap().is_dir());
    }

    /// One loud panic for the task tests: every field carries something the
    /// mask or a bound has to take off — a credential word in the summary,
    /// a summary past the text cap, three tables' worth of crumbs and
    /// frames, and a home directory inside the frames' source lines.
    fn a_loud_panic(root: &Path) -> Report {
        // This fixture tests report truncation, not the process-global ring.
        // Other tests recording concurrently must not change its expected tail.
        let ring = crumbs::RingStorage::<{ Limits::LAST_CRUMBS }>::new();
        for index in 0..(Limits::TASK_CRUMBS * 3) {
            ring.write(index as u64, &format!("pane spawn term={index}"));
        }
        let backtrace: String = (0..(Limits::TASK_FRAMES * 3))
            .map(|index| {
                format!(
                    "  {index}: zerocode_shell::frame_{index}::run\n             at /Users/person/src/frame.rs:{index}:1\n"
                )
            })
            .collect();
        let summary = format!(
            "update failed token=short {}",
            "x".repeat(Limits::TEXT_BYTES * 2)
        );
        record(
            root,
            Kind::Panic,
            &summary,
            Origin {
                site: Some(("crates/zerocode-shell/src/pane.rs", 41)),
                thread: Some("main"),
                backtrace: Some(&backtrace),
            },
        )
        .unwrap();
        let mut report: Report =
            read_json(&root.join("crash/last.json")).expect("the panic report");
        report.crumbs = ring.snapshot();
        write_json(&root.join("crash/last.json"), &report).expect("the fixture snapshot");
        report
    }

    #[test]
    fn a_crash_task_inherits_the_reports_mask_and_bounds() {
        let root = tempfile::tempdir().unwrap();
        let report = a_loud_panic(root.path());
        let argv = task_argv(&report, 3).expect("a panic files a task");
        assert_eq!(argv[0], "task-create");
        assert_eq!(argv[1], "--retry-request");
        assert_eq!(argv[2], format!("crash-{}", report.id));
        assert_eq!(argv[3], "--spec");
        assert_eq!(argv.len(), 5, "{argv:?}");
        let body = &argv[4];
        let mut lines = body.lines();
        let head = lines.next().unwrap();
        assert_eq!(head, format!("panic: {}", report.summary));
        assert!(head.len() <= "panic: ".len() + Limits::TEXT_BYTES, "{head}");
        assert_eq!(lines.next().unwrap(), "같은 자리 3번째");
        assert!(
            body.contains("panic: update failed [redacted]")
                && body.contains(" @ crates/zerocode-shell/src/pane.rs:41 [thread main]"),
            "{body}"
        );
        let frames: Vec<&str> = body
            .lines()
            .filter(|line| line.contains("zerocode_shell::frame_"))
            .collect();
        assert_eq!(frames.len(), Limits::TASK_FRAMES, "{body}");
        assert!(frames[0].contains("frame_0::run"));
        let crumb_rows: Vec<&str> = body
            .lines()
            .filter(|line| line.contains("pane spawn term="))
            .collect();
        assert_eq!(crumb_rows.len(), Limits::TASK_CRUMBS, "{body}");
        assert!(
            crumb_rows
                .last()
                .unwrap()
                .ends_with(&format!("term={}", Limits::TASK_CRUMBS * 3 - 1)),
            "the LAST crumbs travel, not the first: {crumb_rows:?}"
        );
        assert!(body.contains("ledger: revision "), "{body}");
        assert!(
            body.contains(&format!("build: shell {} ", build_identity().version)),
            "{body}"
        );
        for secret in ["short", "/Users/person", "token="] {
            assert!(
                !body.contains(secret),
                "the task body leaked {secret}:\n{body}"
            );
        }
        assert!(body.len() < zerocode_core::orchestration::MAX_PROSE);
    }

    #[test]
    fn a_hang_files_a_task_without_a_site_line_and_a_kill_files_nothing() {
        let root = tempfile::tempdir().unwrap();
        record(
            root.path(),
            Kind::Hang,
            "main thread unresponsive for 6000ms; focus_lane",
            Origin::NONE,
        )
        .unwrap();
        let hang: Report = read_json(&root.path().join("crash/last.json")).unwrap();
        let argv = task_argv(&hang, 1).expect("a hang files a task");
        let body = &argv[4];
        assert!(body.starts_with("hang: main thread unresponsive for 6000ms; focus_lane\n"));
        assert!(!body.contains(" @ "), "{body}");
        assert!(
            !body.contains("같은 자리"),
            "a first crash is not a repeat: {body}"
        );
        // A kill has no evidence and files nothing — not through the argv
        // builder and not through the boot door.
        let killed = tempfile::tempdir().unwrap();
        let previous = process_record();
        begin_process(killed.path(), &previous).unwrap();
        begin_process(killed.path(), &process_record()).unwrap();
        let report: Report = read_json(&killed.path().join("crash/last.json")).unwrap();
        assert_eq!(report.kind, Kind::Killed);
        assert!(task_argv(&report, 1).is_none());
        assert!(consume(killed.path()).is_some());
        assert!(pending_triage(killed.path(), now()).is_none());
        assert!(!killed.path().join("crash/triaged.json").exists());
    }

    #[test]
    fn a_crash_is_filed_once_and_the_stamp_survives_a_boot_loop() {
        let root = tempfile::tempdir().unwrap();
        let report = a_loud_panic(root.path());
        // Not consumed yet: the sheet has not shown it, so nothing to file.
        assert!(pending_triage(root.path(), now()).is_none());
        assert!(consume(root.path()).is_some());
        let pending = pending_triage(root.path(), now()).expect("the shown incident");
        assert_eq!(pending.report, report.id);
        assert_eq!(pending.argv[2], format!("crash-{}", report.id));
        // The same incident asked twice is the same request name.
        assert_eq!(
            pending_triage(root.path(), now()).unwrap().argv,
            pending.argv
        );
        note_triaged(root.path(), &report.id, "t-7").unwrap();
        assert!(pending_triage(root.path(), now()).is_none());
        let stamp: serde_json::Value = read_json(&root.path().join("crash/triaged.json")).unwrap();
        assert_eq!(stamp["report"], report.id);
        assert_eq!(stamp["task"], "t-7");
        // A second boot with consumed.json present: the sheet stays shut and
        // the stamp keeps the task from being filed again.
        write_json(&root.path().join("crash/process.json"), &report.process).unwrap();
        begin_process(root.path(), &process_record()).unwrap();
        assert!(consume(root.path()).is_none());
        assert!(pending_triage(root.path(), now()).is_none());
        // A report older than the window is stale news, not a task.
        let later = tempfile::tempdir().unwrap();
        a_loud_panic(later.path());
        assert!(consume(later.path()).is_some());
        assert!(pending_triage(later.path(), now()).is_some());
        assert!(pending_triage(later.path(), now() + Limits::REPEAT_WINDOW_MS + 1).is_none());
    }

    #[test]
    fn repeats_at_the_same_site_are_counted_inside_the_window() {
        let root = tempfile::tempdir().unwrap();
        let at = |file: &'static str| Origin {
            site: Some((file, 41)),
            thread: Some("main"),
            backtrace: None,
        };
        record(root.path(), Kind::Panic, "first", at("src/pane.rs")).unwrap();
        record(root.path(), Kind::Panic, "second", at("src/pane.rs")).unwrap();
        record(root.path(), Kind::Panic, "elsewhere", at("src/other.rs")).unwrap();
        record(root.path(), Kind::Panic, "third", at("src/pane.rs")).unwrap();
        let third: Report = read_json(&root.path().join("crash/last.json")).unwrap();
        assert_eq!(repeats_of(root.path(), &third), 3);
        // A row outside the window does not count, and a row without a site
        // never matches anything.
        let history = root.path().join("crash/history.jsonl");
        let mut rows = std::fs::read_to_string(&history).unwrap();
        rows.push_str(&format!(
            "{}\n{}\n",
            serde_json::json!({"id":"old","at_ms":third.at_ms - Limits::REPEAT_WINDOW_MS - 1,"kind":"panic","site":"src/pane.rs:41"}),
            serde_json::json!({"id":"blank","at_ms":third.at_ms,"kind":"killed","site":null}),
        ));
        std::fs::write(&history, rows).unwrap();
        assert_eq!(repeats_of(root.path(), &third), 3);
        assert!(consume(root.path()).is_some());
        let pending = pending_triage(root.path(), now()).unwrap();
        assert!(
            pending.argv[4].contains("같은 자리 3번째"),
            "{}",
            pending.argv[4]
        );
        // The history is bounded: a boot loop cannot grow it forever.
        for _ in 0..(Limits::HISTORY_ROWS * 2) {
            record(root.path(), Kind::Panic, "loop", at("src/loop.rs")).unwrap();
        }
        let looped: Report = read_json(&root.path().join("crash/last.json")).unwrap();
        assert_eq!(repeats_of(root.path(), &looped), Limits::HISTORY_ROWS);
        assert!(std::fs::read_to_string(&history).unwrap().lines().count() <= Limits::HISTORY_ROWS);
        // A site under a home directory keeps its file and parent, never
        // the person's directory; and it counts as its own place.
        record(
            root.path(),
            Kind::Panic,
            "home",
            at("/Users/person/src/pane.rs"),
        )
        .unwrap();
        let masked: Report = read_json(&root.path().join("crash/last.json")).unwrap();
        assert_eq!(masked.site.as_deref(), Some("src/pane.rs:41"));
        assert_eq!(repeats_of(root.path(), &masked), 1);
    }

    #[test]
    fn a_public_site_names_the_file_without_the_machine_it_was_built_on() {
        assert_eq!(
            public_site("crates/zerocode-shell/src/main.rs", 2831),
            "crates/zerocode-shell/src/main.rs:2831"
        );
        assert_eq!(
            public_site("/rustc/abcdef0123/library/core/src/panicking.rs", 75),
            "/rustc/abcdef0123/library/core/src/panicking.rs:75"
        );
        assert_eq!(
            public_site(
                "/Users/person/.cargo/registry/src/index.crates.io-6f17d22bba15001f/tauri-2.8.0/src/lib.rs",
                12
            ),
            "tauri-2.8.0/src/lib.rs:12"
        );
        assert_eq!(
            public_site("/Users/person/private/src/pane.rs", 41),
            "src/pane.rs:41"
        );
        assert_eq!(
            public_site("C:\\Users\\person\\private\\src\\pane.rs", 41),
            "src/pane.rs:41"
        );
        assert_eq!(public_site("person@host:src/pane.rs", 1), "[redacted]");
        assert_eq!(public_site("", 1), "[redacted]");
        // A frame is its symbol over a closed alphabet — and nothing else.
        assert_eq!(
            symbol_frame("   3: zerocode_shell::claude_tokens::refresh_0").as_deref(),
            Some("3: zerocode_shell::claude_tokens::refresh_0")
        );
        assert_eq!(
            symbol_frame("             at /Users/person/src/frame.rs:3:1"),
            None
        );
        assert_eq!(symbol_frame("7 panic: token=short"), None);
    }

    #[test]
    fn write_diagnostics_evidence_when_requested() {
        let Ok(path) = std::env::var("ZEROCODE_CRASH_EVIDENCE_DIR") else {
            return;
        };
        let root = Path::new(&path);
        std::fs::create_dir_all(root).unwrap();
        crumbs::record("boot", format_args!("ready"));
        crumbs::record("pane", format_args!("spawn term=7"));
        crumbs::record("hook", format_args!("pane=7 state=Working"));
        crumbs::record("hang", format_args!("ms=6000 last_command=focus_lane"));
        crate::record_panic(
            root,
            &"diagnostics verification fixture",
            Some(("fixture.rs", 7)),
            Some("test-main"),
        );
        let folder = bundle(root).unwrap();
        eprintln!("diagnostics_evidence_folder={}", folder.display());
    }

    #[test]
    fn crash_overlay_clamps_order_and_round_trips_through_settings() {
        let root = tempfile::tempdir().unwrap();
        let repository = crate::settings::SettingsRepository::new(root.path());
        let saved = crate::mutate_settings(&repository, |document| {
            document.crash = json!({"watchdog":false,"first_ms":3000,"second_ms":7000});
            Ok(())
        })
        .unwrap();
        assert!(!saved.crash_limits.watchdog);
        let loaded = crate::load_settings(&repository).unwrap();
        assert_eq!(loaded.document.crash, saved.document.crash);
        let limits = Limits::overlay(&json!({"watchdog":false,"first_ms":3000,"second_ms":1000}));
        assert!(!limits.watchdog);
        assert_eq!(limits.first_ms, 3000);
        assert_eq!(limits.second_ms, 3000 + Limits::DEFAULT.ping_ms);
        assert_eq!(
            Limits::overlay(&json!({"first_ms":"invalid"})),
            Limits::DEFAULT
        );
        let fixture = include_str!("../../../ui/tests/settings.mjs")
            .split_once("const RUST_CRASH_LIMITS_JSON = String.raw`")
            .unwrap()
            .1
            .split_once('`')
            .unwrap()
            .0;
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(fixture).unwrap(),
            serde_json::to_value(Limits::DEFAULT).unwrap()
        );
    }
}
