//! The walk that feeds [`zerocode_core::usage_stats`], and its cache.
//!
//! The core module decides what a turn is worth; this one finds the files and
//! keeps the answer. The split is deliberate — every rule a person could
//! disagree with (which rows count, what a day is, what a token costs) lives
//! in the pure module with a test beside it, and what is left here is I/O.
//!
//! ## Where the transcripts are
//!
//! Claude Code writes one JSONL per session under `<config>/projects/`, where
//! `<config>` is `CLAUDE_CONFIG_DIR` when set and `~/.claude` otherwise. This
//! window launches its agents under a per-account config directory, so the
//! machine has SEVERAL of those roots and a scan that reads only `~/.claude`
//! reports a fraction of the work and calls it the total. Every root is read
//! and the results merge, which is what "all local Claude usage" has to mean
//! on a machine that signs in more than once.
//!
//! ## Why the scan is never in the way
//!
//! A first scan reads every transcript on the disk, which on a working
//! machine is thousands of files. It runs on its own thread, the pane draws
//! the previous answer while it runs, and a second ask does not start a
//! second scan. That is the same contract the plan-limit readers already
//! hold, for the same reason: a settings pane that hangs while it counts is
//! worse than one that says "reading…".

use crate::scan_cell::ScanCell;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use zerocode_core::usage_stats::{self, Ledger, WorktreeRef};
use zerocode_core::usage_stats_codex as codex_stats;

/// The deepest a project directory is walked.
///
/// Claude Code's own layout is `projects/<slug>/<session>.jsonl` — one level.
/// The slack is for a vendor that adds one, and the bound is required rather
/// than tidy: these roots are outside this program's control and a symlink
/// inside one would otherwise walk the disk.
const MAX_DEPTH: usize = 4;

/// The most transcripts one scan will read, over all roots.
///
/// A bound rather than a promise of completeness, and the pane says when it
/// bites — a silent cap reads as "this is your total" when it is not.
const MAX_FILES: usize = 20_000;

/// What one scan learned, with enough about the scan itself for the pane to
/// say how fresh it is.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scan {
    pub ledger: Ledger,
    /// Epoch milliseconds of the scan that produced this.
    pub scanned_at: i64,
    /// Transcripts read.
    pub files: usize,
    /// True when [`MAX_FILES`] stopped the walk before it ran out of files.
    pub capped: bool,
    pub error: Option<String>,
}

/// The roots that can hold transcripts: the default one and every managed
/// account's own.
///
/// Deduped by canonical path — an account whose config directory IS `~/.claude`
/// must not have its sessions counted twice.
#[must_use]
pub fn transcript_roots(config_root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut consider = |dir: PathBuf| {
        let projects = dir.join("projects");
        let key = projects.canonicalize().unwrap_or_else(|_| projects.clone());
        if projects.is_dir() && seen.insert(key) {
            roots.push(projects);
        }
    };
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        consider(home.join(".claude"));
    }
    if let Ok(set) = std::env::var(zerocode_core::account::CONFIG_DIR_VAR) {
        consider(PathBuf::from(set));
    }
    for account in crate::accounts::read_store(config_root).accounts {
        consider(PathBuf::from(account.config_dir));
    }
    roots
}

/// Every `*.jsonl` under a root, bounded in depth and in count — and, when
/// `compressed` is asked for, every `*.jsonl.zst` its CLI compressed where it
/// lay ([`zerocode_core::record_file`]): Codex does that to a rollout idle for
/// a week (0.164). Claude Code does not, and its scan does not ask: its
/// turns are deduplicated per file, so a record found twice would count twice.
fn transcripts(root: &Path, budget: &mut usize, compressed: bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH || *budget == 0 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if *budget == 0 {
                break;
            }
            let path = entry.path();
            // `file_type` rather than `metadata`: a symlink out of the root is
            // not followed, which is what keeps the depth bound meaningful.
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                stack.push((path, depth + 1));
            } else if kind.is_file()
                && if compressed {
                    path.file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .is_some_and(|name| {
                            zerocode_core::record_file::names_record(name, ".jsonl")
                        })
                } else {
                    path.extension().is_some_and(|ext| ext == "jsonl")
                }
            {
                *budget -= 1;
                found.push(path);
            }
        }
    }
    found
}

/// Reads one transcript into a ledger of its own.
fn read_one(path: &Path, worktrees: &[WorktreeRef], offset_minutes: i32) -> Option<Ledger> {
    let text = std::fs::read_to_string(path).ok()?;
    let stem = path.file_stem().and_then(std::ffi::OsStr::to_str);
    let turns: Vec<_> = text
        .lines()
        .filter_map(|line| usage_stats::parse_record(line, stem))
        .collect();
    if turns.is_empty() {
        return None;
    }
    Some(usage_stats::aggregate(usage_stats::attribute(
        usage_stats::dedupe(turns),
        worktrees,
        offset_minutes,
    )))
}

/// Reads every transcript under every root and merges the answer.
///
/// The dedupe is per file, which is where the stream's repeats are: a message
/// repeated across two different files is a fork, and the merge counts each
/// file's turns because they are turns that really were spent.
#[must_use]
pub fn scan(
    config_root: &Path,
    worktrees: &[WorktreeRef],
    offset_minutes: i32,
    now_ms: i64,
) -> Scan {
    let mut budget = MAX_FILES;
    let mut ledger = Ledger::default();
    let mut files = 0usize;
    for root in transcript_roots(config_root) {
        for path in transcripts(&root, &mut budget, false) {
            files += 1;
            if let Some(one) = read_one(&path, worktrees, offset_minutes) {
                usage_stats::merge(&mut ledger, one);
            }
        }
    }
    usage_stats::finalize(&mut ledger);
    Scan {
        ledger,
        scanned_at: now_ms,
        files,
        capped: budget == 0,
        error: None,
    }
}

/// The last scan, kept so a settings pane opens on a number rather than a
/// spinner.
static CLAUDE: ScanCell<Scan> = ScanCell::new();

/// The held scan, if there is one.
#[must_use]
pub fn held() -> Option<Scan> {
    CLAUDE.held()
}

/// The held scan, read where it lies ([`ScanCell::with_held`]).
pub fn with_held<R>(read: impl FnOnce(&Scan) -> R) -> Option<R> {
    CLAUDE.with_held(read)
}

/// Whether a scan is running right now.
#[must_use]
pub fn scanning() -> bool {
    CLAUDE.running()
}

/// Starts a scan unless one is already running.
///
/// Returns whether this call started one, which is what lets the pane say
/// "reading…" honestly rather than guessing.
pub fn start(
    config_root: PathBuf,
    worktrees: Vec<WorktreeRef>,
    offset_minutes: i32,
    now_ms: i64,
) -> bool {
    CLAUDE.start(move || scan(&config_root, &worktrees, offset_minutes, now_ms))
}

/// The roots that can hold Codex rollouts: the person's own home and every
/// managed account's.
///
/// The same union rule as the Claude side, for the same reason — this window
/// signs into Codex more than once, and a scan that reads only `~/.codex`
/// reports a fraction and calls it the total.
#[must_use]
pub fn codex_roots(config_root: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut consider = |home: PathBuf| {
        let sessions = home.join("sessions");
        let key = sessions.canonicalize().unwrap_or_else(|_| sessions.clone());
        if sessions.is_dir() && seen.insert(key) {
            roots.push(sessions);
        }
    };
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        consider(home.join(".codex"));
    }
    if let Ok(set) = std::env::var("CODEX_HOME") {
        consider(PathBuf::from(set));
    }
    for account in crate::codex_accounts::read_store(config_root).accounts {
        consider(PathBuf::from(account.home_dir));
    }
    roots
}

/// What one Codex scan learned.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CodexScan {
    pub ledger: codex_stats::Ledger,
    pub scanned_at: i64,
    pub files: usize,
    pub capped: bool,
}

/// [`codex_scan_through`] with no digests kept: every rollout read whole —
/// the answer the tests and the measuring runs hold the kept road to.
#[cfg(test)]
#[must_use]
pub fn codex_scan(
    config_root: &Path,
    worktrees: &[WorktreeRef],
    offset_minutes: i32,
    now_ms: i64,
) -> CodexScan {
    codex_scan_through(config_root, worktrees, offset_minutes, now_ms, None)
}

/// Reads every rollout under every Codex root and merges the answer — each
/// rollout at rest through its kept digest ([`crate::token_scan::Digests`])
/// when `digests` are given: the same ledger, without decoding every rollout
/// again for it.
///
/// The claim set is shared across the whole scan rather than per file: a fork
/// copies earlier `token_count` records byte for byte into its new rollout,
/// and a per-file set would bill that work once for the original and again
/// for every fork of it.
#[must_use]
pub(crate) fn codex_scan_through(
    config_root: &Path,
    worktrees: &[WorktreeRef],
    offset_minutes: i32,
    now_ms: i64,
    digests: Option<&crate::token_scan::Digests>,
) -> CodexScan {
    codex_scan_of(
        &codex_roots(config_root),
        worktrees,
        offset_minutes,
        now_ms,
        digests,
    )
}

/// [`codex_scan_through`] over the roots it was handed, so the walk can be
/// read apart from where a machine keeps its rollouts.
fn codex_scan_of(
    roots: &[PathBuf],
    worktrees: &[WorktreeRef],
    offset_minutes: i32,
    now_ms: i64,
    digests: Option<&crate::token_scan::Digests>,
) -> CodexScan {
    let mut budget = MAX_FILES;
    let mut claimed: HashSet<String> = HashSet::new();
    let mut ledger = codex_stats::Ledger::default();
    let mut files = 0usize;
    for root in roots {
        // Oldest first, so the ORIGINAL of a copied record is the one that
        // claims it and the fork is the copy — the other order would file a
        // session's early turns under whichever fork happened to sort first.
        let mut found = transcripts(root, &mut budget, true);
        found.sort();
        for path in found {
            files += 1;
            let stem = zerocode_core::record_file::record_stem(&path).unwrap_or_default();
            // A line at a time, not the file whole: a rollout compressed in
            // place decodes to hundreds of megabytes, and the shared reader
            // skips a line past its cap the way the token scan does.
            let mut reader = codex_stats::RolloutReader::new(stem, &mut claimed);
            crate::token_scan::for_each_rollout_line(&path, digests, |line| {
                reader.line(line.trim_end_matches(['\n', '\r']));
            });
            let events = reader.finish();
            if events.is_empty() {
                continue;
            }
            codex_stats::merge(
                &mut ledger,
                codex_stats::aggregate(events, worktrees, offset_minutes),
            );
        }
    }
    if let Some(digests) = digests {
        digests.sweep();
    }
    codex_stats::finalize(&mut ledger);
    CodexScan {
        ledger,
        scanned_at: now_ms,
        files,
        capped: budget == 0,
    }
}

static CODEX: ScanCell<CodexScan> = ScanCell::new();

/// The held Codex scan, if there is one.
#[must_use]
pub fn codex_held() -> Option<CodexScan> {
    CODEX.held()
}

/// The held Codex scan, read where it lies ([`ScanCell::with_held`]).
pub fn with_codex_held<R>(read: impl FnOnce(&CodexScan) -> R) -> Option<R> {
    CODEX.with_held(read)
}

/// Whether a Codex scan is running right now.
#[must_use]
pub fn codex_scanning() -> bool {
    CODEX.running()
}

/// Starts a Codex scan unless one is already running. The rollouts at rest
/// are read through the digests kept under `local_data_root`.
pub fn codex_start(
    config_root: PathBuf,
    local_data_root: PathBuf,
    worktrees: Vec<WorktreeRef>,
    offset_minutes: i32,
    now_ms: i64,
) -> bool {
    CODEX.start(move || {
        let digests = crate::token_scan::Digests::under(&local_data_root);
        codex_scan_through(
            &config_root,
            &worktrees,
            offset_minutes,
            now_ms,
            Some(&digests),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ledger is the same read whole, read while the digests are being
    /// made, and read from the digests kept — a fork's copied records claimed
    /// once either way.
    #[test]
    fn the_codex_ledger_is_the_same_whole_and_through_digests() {
        let root = tempfile::tempdir().expect("tempdir");
        let day = root.path().join("2026").join("05").join("30");
        std::fs::create_dir_all(&day).expect("mkdir");
        let usage = |times: u64| {
            serde_json::json!({
                "input_tokens": 1200 * times, "cached_input_tokens": 1000 * times,
                "output_tokens": 80 * times, "reasoning_output_tokens": 30 * times,
                "total_tokens": 1280 * times,
            })
        };
        let spent = |at: &str, turns: u64| {
            serde_json::json!({
                "timestamp": at, "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": { "total_token_usage": usage(turns), "last_token_usage": usage(1) },
                    "rate_limits": { "primary": { "used_percent": 1.5 } },
                },
            })
            .to_string()
        };
        let header = |id: &str, cwd: &str| {
            serde_json::json!({
                "timestamp": "2026-05-30T01:00:00.000Z", "type": "session_meta",
                "payload": { "id": id, "cwd": cwd, "instructions": "y".repeat(300) },
            })
            .to_string()
        };
        let model = r#"{"timestamp":"2026-05-30T01:00:00.500Z","type":"turn_context","payload":{"model":"gpt-5.5"}}"#;
        let original = [
            header("s-1", "/work/app"),
            model.to_string(),
            spent("2026-05-30T01:00:01.000Z", 1),
            r#"{"type":"response_item","payload":{"type":"message","content":"…"}}"#.to_string(),
            spent("2026-05-30T01:00:02.000Z", 2),
        ];
        let fork = [
            header("s-2", "/work/app"),
            model.to_string(),
            spent("2026-05-30T01:00:01.000Z", 1),
            spent("2026-05-30T01:00:02.000Z", 2),
            spent("2026-05-31T09:00:00.000Z", 3),
        ];
        for (name, lines) in [
            ("rollout-2026-05-30-a.jsonl.zst", &original),
            ("rollout-2026-05-31-b.jsonl.zst", &fork),
        ] {
            let squeezed = ruzstd::encoding::compress_to_vec(
                lines.join("\n").as_bytes(),
                ruzstd::encoding::CompressionLevel::Fastest,
            );
            std::fs::write(day.join(name), squeezed).expect("write");
        }
        let roots = [root.path().to_path_buf()];
        let said = |scan: &CodexScan| serde_json::to_value(scan).expect("JSON");
        let whole = codex_scan_of(&roots, &[], 0, 7, None);
        assert_eq!(whole.files, 2);
        let printed = said(&whole).to_string();
        assert!(printed.contains("gpt-5.5"), "{printed}");

        let data = tempfile::tempdir().expect("tempdir");
        let through = || {
            let digests = crate::token_scan::Digests::under(data.path());
            codex_scan_of(&roots, &[], 0, 7, Some(&digests))
        };
        assert_eq!(
            said(&through()),
            said(&whole),
            "read while the digests were made"
        );
        assert_eq!(
            std::fs::read_dir(data.path().join("codex-rollout-digests"))
                .expect("the digests")
                .count(),
            2
        );
        assert_eq!(said(&through()), said(&whole), "read from the digests kept");
    }

    /// The Codex scan walks the rollouts Codex compressed where they lay
    /// (`….jsonl.zst`, 0.164) as well as the plain ones; the Claude scan,
    /// whose turns are deduplicated per file, keeps to `*.jsonl`.
    #[test]
    fn only_the_codex_walk_takes_a_compressed_rollout() {
        let root = tempfile::tempdir().expect("tempdir");
        let day = root.path().join("2026").join("05").join("30");
        std::fs::create_dir_all(&day).expect("mkdir");
        for name in [
            "rollout-a.jsonl",
            "rollout-b.jsonl.zst",
            "notes.txt.zst",
            "c.zst",
        ] {
            std::fs::write(day.join(name), b"").expect("write");
        }
        let names = |compressed: bool| {
            let mut budget = MAX_FILES;
            let mut found: Vec<String> = transcripts(root.path(), &mut budget, compressed)
                .iter()
                .filter_map(|path| path.file_name()?.to_str().map(str::to_string))
                .collect();
            found.sort();
            found
        };
        assert_eq!(names(true), ["rollout-a.jsonl", "rollout-b.jsonl.zst"]);
        assert_eq!(names(false), ["rollout-a.jsonl"]);
    }
}
