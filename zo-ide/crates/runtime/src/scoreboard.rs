//! The scoreboard beat's judgement: evidence ledgers against a baseline.
//!
//! The evidence ledgers — request timings, prompt-cache breaks, the release
//! lane's phase lines, the flake list — were written and read by nobody.
//! Every regression they held (a first byte that doubled, a gate that took
//! six hours under load) was found by a person who happened to look
//! (docs/design/scoreboard-beat-20260911.md). This module is the reader's
//! judgement, pure: a [`Window`] of recent evidence against a [`Baseline`]
//! yields [`Finding`]s, one per axis that crossed its threshold and is not
//! already an open task. The CLI (`zo scoreboard`) gathers the window and
//! files the findings; a cron fires it daily.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::prompt_cache_breaks::PromptCacheBreakRecord;
use crate::request_timings::RequestTimingRecord;

/// Nearest-rank percentile of an unsorted sample; `None` for an empty one.
#[must_use]
pub fn percentile(sample: &[u64], percent: u32) -> Option<u64> {
    if sample.is_empty() {
        return None;
    }
    let mut sorted = sample.to_vec();
    sorted.sort_unstable();
    let rank = (usize::try_from(percent).unwrap_or(100) * sorted.len()).div_ceil(100);
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// One model's first-byte waits over a window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirstByteAxis {
    pub p50_ms: u64,
    pub p90_ms: u64,
    pub n: usize,
}

/// What "normal" looked like when a person last wrote it down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Baseline {
    /// Unix seconds when the baseline was written.
    pub written_at: u64,
    /// First-byte waits by wire model.
    #[serde(default)]
    pub first_byte: BTreeMap<String, FirstByteAxis>,
    /// Unexpected cache breaks per ten thousand requests (an integer so the
    /// comparison is exact).
    #[serde(default)]
    pub breaks_per_10k: u64,
    /// Green, quiet gate phases: seconds by phase name.
    #[serde(default)]
    pub gate_secs: BTreeMap<String, u64>,
    /// Lines in the flake list.
    #[serde(default)]
    pub flake_names: usize,
}

/// The recent evidence, aggregated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Window {
    pub since_secs: u64,
    pub requests: usize,
    pub breaks: usize,
    #[serde(default)]
    pub first_byte: BTreeMap<String, FirstByteAxis>,
    /// Quiet green gate phases — the lane found calm before the gate started
    /// and the load at its end was at or under the calm threshold — every
    /// observed duration, by phase name, from each lane log's last run.
    #[serde(default)]
    pub gate_secs: BTreeMap<String, Vec<u64>>,
    /// Lines in the flake list; `None` when the list could not be read —
    /// never a zero, which would hide its growth and poison a baseline.
    #[serde(default)]
    pub flake_names: Option<usize>,
    pub disk_free_gb: Option<u64>,
}

impl Window {
    /// Unexpected cache breaks per ten thousand requests.
    #[must_use]
    pub fn breaks_per_10k(&self) -> u64 {
        if self.requests == 0 {
            0
        } else {
            (self.breaks as u64).saturating_mul(10_000) / self.requests as u64
        }
    }
}

/// `1234` per ten thousand → `12.34%`.
#[must_use]
pub fn percent_of_10k(per_10k: u64) -> String {
    format!("{}.{:02}%", per_10k / 100, per_10k % 100)
}

/// The lines a finding crosses, as percentages of the baseline (125 = a
/// quarter over) so every comparison is integer arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thresholds {
    pub p50_percent: u64,
    pub p90_percent: u64,
    pub min_requests: usize,
    pub break_percent: u64,
    pub min_break_requests: usize,
    pub gate_percent: u64,
    pub disk_floor_gb: u64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            p50_percent: 125,
            p90_percent: 150,
            min_requests: 20,
            break_percent: 200,
            min_break_requests: 50,
            gate_percent: 150,
            disk_floor_gb: 25,
        }
    }
}

/// Whether `observed` is over `baseline × percent / 100`.
fn over(observed: u64, baseline: u64, percent: u64) -> bool {
    observed.saturating_mul(100) > baseline.saturating_mul(percent)
}

/// The raw evidence a window is aggregated from.
#[derive(Debug, Clone, Copy)]
pub struct Evidence<'a> {
    pub timings: &'a [RequestTimingRecord],
    pub breaks: &'a [PromptCacheBreakRecord],
    pub lane_lines: &'a [&'a str],
    /// The lane's `CALM_LOAD`: a gate run above it measured the machine.
    pub calm_load: f64,
    /// Lines in the flake list, or `None` when it could not be read.
    pub flake_names: Option<usize>,
    pub disk_free_gb: Option<u64>,
}

/// One regression, with the numbers a task can carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Stable per axis (and model): `first-byte:claude-opus-5`,
    /// `cache-break-rate`, `gate:gate-zo`, `flakes`, `disk`.
    pub key: String,
    pub observed: String,
    pub baseline: String,
    /// One sentence a person or a worker can act on.
    pub words: String,
}

/// A project slug a scratch bench or a temporary workspace left behind — its
/// rows are fixtures, not sessions. The slug is the path's last 80
/// characters, so a temporary path is matched anywhere in it (`e-var-folders-…`
/// as well as `private-var-folders-…`); one that lost even that is caught by
/// the path its session recorded ([`is_temporary_workspace`]). A crate
/// directory is not a fixture: it is where a person runs zo in a Rust
/// workspace, and the crates' own tests write no evidence at all — the
/// ledgers write only for a host that armed them (`durable_traces_armed`).
#[must_use]
pub fn is_test_slug(slug: &str) -> bool {
    slug.contains("scratchpad") || slug.contains("var-folders-")
}

/// A workspace under one of the machine's temporary roots — a bench round's
/// `mkdtemp`, a test's `tempfile` — whose rows are fixtures, not sessions.
/// Judged on the path a session recorded, because the slug keeps only the last
/// 80 characters of it and a long temporary path loses the prefix that said
/// so. `temp_roots` is the caller's (the system temporary directory, `/tmp`,
/// and their canonical forms), so this stays a pure judgement.
#[must_use]
pub fn is_temporary_workspace(cwd: &std::path::Path, temp_roots: &[std::path::PathBuf]) -> bool {
    temp_roots.iter().any(|root| cwd.starts_with(root))
}

/// The words after the clock that open one run of the release lane
/// (`tools/release/lane.sh` `run_sha`: `log "== lane $SHA8 start …"`).
pub const LANE_RUN_MARK: &str = "== lane ";
/// What `wait_for_calm` logs after `<phase>: ` when a gate starts without the
/// calm it waits for — the budget ran out on a loud machine, or the load
/// could not be read. A gate that started so measured the machine, not itself.
pub const LANE_CALM_MISSED: [&str; 2] = ["loud — ", "load unreadable"];

/// The words of a lane log line after its `HH:MM:SS` clock.
fn after_clock(line: &str) -> &str {
    line.trim_start()
        .split_once(char::is_whitespace)
        .map_or("", |(_, rest)| rest.trim_start())
}

/// The last run of one lane log. lane.sh appends a re-queued sha's run to the
/// same `lane-<sha8>.log` and its lines carry only a clock, so an earlier run
/// cannot be dated; the file's mtime dates the last one.
#[must_use]
pub fn last_lane_run(text: &str) -> &str {
    let mut start = 0;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if after_clock(line).starts_with(LANE_RUN_MARK) {
            start = at;
        }
        at += line.len();
    }
    &text[start..]
}

/// The phase a `<phase>: loud — …` / `<phase>: load unreadable …` line names.
fn lane_calm_missed(line: &str) -> Option<&str> {
    let (phase, rest) = after_clock(line).split_once(": ")?;
    LANE_CALM_MISSED
        .iter()
        .any(|missed| rest.starts_with(missed))
        .then_some(phase)
}

/// `HH:MM:SS <phase> rc=<rc> <secs>s free=<n>G load=<1m>/<15m>` → (phase, rc, secs, load1).
/// Lines without `load=` (before 2026-09-11) answer `None` for the load.
#[must_use]
pub fn parse_lane_phase_line(line: &str) -> Option<(String, i32, u64, Option<f64>)> {
    let mut words = line.split_whitespace();
    let _clock = words.next()?;
    let phase = words.next()?;
    let rc = words.next()?.strip_prefix("rc=")?.parse().ok()?;
    let secs = words.next()?.strip_suffix('s')?.parse().ok()?;
    let load = words
        .find_map(|word| word.strip_prefix("load="))
        .and_then(|pair| pair.split('/').next())
        .and_then(|one| one.parse().ok());
    Some((phase.to_string(), rc, secs, load))
}

/// Aggregate the evidence newer than `now - since_secs`.
#[must_use]
pub fn window_from(now: u64, since_secs: u64, evidence: &Evidence<'_>) -> Window {
    let Evidence {
        timings,
        breaks,
        lane_lines,
        calm_load,
        flake_names,
        disk_free_gb,
    } = *evidence;
    let floor = now.saturating_sub(since_secs);
    let mut by_model: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    let mut requests = 0;
    for record in timings {
        if record.recorded_at < floor || record.outcome != "completed" {
            continue;
        }
        requests += 1;
        if let (Some(model), Some(ttfb)) = (record.model.as_deref(), record.ttfb_ms) {
            by_model.entry(model.to_string()).or_default().push(ttfb);
        }
    }
    let first_byte = by_model
        .into_iter()
        .filter_map(|(model, sample)| {
            Some((
                model,
                FirstByteAxis {
                    p50_ms: percentile(&sample, 50)?,
                    p90_ms: percentile(&sample, 90)?,
                    n: sample.len(),
                },
            ))
        })
        .collect();
    let breaks = breaks.iter().filter(|b| b.recorded_at >= floor).count();
    let mut gate_secs: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    // Phases of the current run that started without calm; a run's start
    // forgets the last run's.
    let mut started_loud: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for line in lane_lines {
        if after_clock(line).starts_with(LANE_RUN_MARK) {
            started_loud.clear();
            continue;
        }
        if let Some(phase) = lane_calm_missed(line) {
            started_loud.insert(phase);
            continue;
        }
        let Some((phase, rc, secs, load)) = parse_lane_phase_line(line) else {
            continue;
        };
        let loud_start = started_loud.remove(phase.as_str());
        if rc != 0 || !phase.starts_with("gate-") {
            continue;
        }
        // Only a quiet run measures the gate; a loud one measures the machine.
        // Quiet is judged at both ends: the lane found calm before it started
        // (no calm-missed line), and the load at its end is under the line.
        if loud_start || load.is_none_or(|load1| load1 > calm_load) {
            continue;
        }
        gate_secs.entry(phase).or_default().push(secs);
    }
    Window {
        since_secs,
        requests,
        breaks,
        first_byte,
        gate_secs,
        flake_names,
        disk_free_gb,
    }
}

/// A baseline that says the window is normal — refused while the flake list
/// is unread, because a baseline that recorded no count would judge the list
/// against nothing until a person wrote it again.
pub fn baseline_from(now: u64, window: &Window) -> Result<Baseline, String> {
    let flake_names = window.flake_names.ok_or_else(|| {
        "the flake list could not be read, so this baseline would record no flake count — point --flakes at the list"
            .to_string()
    })?;
    Ok(Baseline {
        written_at: now,
        first_byte: window.first_byte.clone(),
        breaks_per_10k: window.breaks_per_10k(),
        gate_secs: window
            .gate_secs
            .iter()
            .filter_map(|(phase, secs)| Some((phase.clone(), percentile(secs, 50)?)))
            .collect(),
        flake_names,
    })
}

/// The flake list's line, if it has one: grown past the baseline, or unread —
/// a list nobody could read hides its growth, so that is said as well.
fn flakes_finding(baseline: &Baseline, window: &Window) -> Option<Finding> {
    let (key, observed, words) = match window.flake_names {
        Some(names) if names > baseline.flake_names => (
            "flakes",
            format!("{names} names"),
            format!(
                "the flake list grew {} → {names} names — each new name needs three solo runs to stay a flake, or a fix",
                baseline.flake_names
            ),
        ),
        Some(_) => return None,
        None => (
            "flakes:unread",
            "unread".to_string(),
            "the flake list could not be read, so its growth goes unseen — point --flakes at it".to_string(),
        ),
    };
    Some(Finding {
        key: key.to_string(),
        observed,
        baseline: format!("{} names", baseline.flake_names),
        words,
    })
}

/// The regressions in `window` against `baseline`, minus keys already open.
#[must_use]
pub fn judge(
    baseline: &Baseline,
    window: &Window,
    open_keys: &[String],
    thresholds: &Thresholds,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut file = |key: String, observed: String, base: String, words: String| {
        if !open_keys.iter().any(|open| open == &key) {
            findings.push(Finding {
                key,
                observed,
                baseline: base,
                words,
            });
        }
    };
    for (model, now) in &window.first_byte {
        let Some(then) = baseline.first_byte.get(model) else {
            continue;
        };
        // The floor holds on both sides: a baseline of two requests has a
        // p90 that is simply the slower of the two, and cannot judge a day.
        if now.n.min(then.n) < thresholds.min_requests {
            continue;
        }
        if over(now.p50_ms, then.p50_ms, thresholds.p50_percent)
            || over(now.p90_ms, then.p90_ms, thresholds.p90_percent)
        {
            file(
                format!("first-byte:{model}"),
                format!("p50 {} ms · p90 {} ms (n={})", now.p50_ms, now.p90_ms, now.n),
                format!("p50 {} ms · p90 {} ms", then.p50_ms, then.p90_ms),
                format!(
                    "{model} first byte rose: p50 {} → {} ms, p90 {} → {} ms over {} requests — find where the wait went (request-timings ledger: assemble/ttfb/ttfv)",
                    then.p50_ms, now.p50_ms, then.p90_ms, now.p90_ms, now.n
                ),
            );
        }
    }
    if window.requests >= thresholds.min_break_requests {
        let rate = window.breaks_per_10k();
        let base = baseline.breaks_per_10k;
        let rose = if base > 0 {
            over(rate, base, thresholds.break_percent)
        } else {
            rate > 0
        };
        if rose {
            file(
                "cache-break-rate".to_string(),
                format!("{} ({} of {})", percent_of_10k(rate), window.breaks, window.requests),
                percent_of_10k(base),
                format!(
                    "unexpected prompt-cache breaks rose: {} → {} of requests ({} of {}) — read prompt-cache/breaks.jsonl for the prefix that moved",
                    percent_of_10k(base),
                    percent_of_10k(rate),
                    window.breaks,
                    window.requests
                ),
            );
        }
    }
    for (phase, secs) in &window.gate_secs {
        let (Some(then), Some(now)) = (baseline.gate_secs.get(phase), percentile(secs, 50)) else {
            continue;
        };
        if over(now, *then, thresholds.gate_percent) {
            file(
                format!("gate:{phase}"),
                format!("{now} s (median of {} quiet runs)", secs.len()),
                format!("{then} s"),
                format!(
                    "{phase} takes {then} → {now} s on a quiet machine — a gate member grew or a target went cold"
                ),
            );
        }
    }
    if let Some(flakes) = flakes_finding(baseline, window) {
        file(flakes.key, flakes.observed, flakes.baseline, flakes.words);
    }
    if let Some(free) = window.disk_free_gb {
        if free < thresholds.disk_floor_gb {
            file(
                "disk".to_string(),
                format!("{free} G free"),
                format!("≥ {} G", thresholds.disk_floor_gb),
                format!(
                    "{free} G free on / — the release lane refuses under 10 G; reclaim targets and old release outputs"
                ),
            );
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unread_flake_list_is_a_finding_and_never_a_zero() {
        let mut w = window();
        w.flake_names = None;
        let found = judge(&baseline(), &w, &[], &Thresholds::default());
        assert_eq!(
            found.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(),
            ["flakes:unread"],
            "{found:?}"
        );
        assert_eq!(found[0].baseline, "36 names");
        assert!(
            baseline_from(2_000, &w).is_err_and(|why| why.contains("flake list")),
            "a baseline is not written without the flake count"
        );
    }

    #[test]
    fn a_real_project_under_a_crates_directory_is_not_a_fixture() {
        // A Rust workspace member a person runs zo in.
        assert!(!is_test_slug("Users-me-src-shop-crates-server-0123456789abcdef"));
        // A temporary path, anywhere in the slug's last 80 characters.
        assert!(is_test_slug("private-var-folders-3c-T-zo-bench-1-0123456789abcdef"));
        assert!(is_test_slug("e-var-folders-3c-T-zo-bench-1-0123456789abcdef"));
        assert!(is_test_slug("Users-me-tmp-claude-501-scratchpad-probe-0123456789abcdef"));
    }

    fn axis(p50: u64, p90: u64, n: usize) -> FirstByteAxis {
        FirstByteAxis { p50_ms: p50, p90_ms: p90, n }
    }

    fn baseline() -> Baseline {
        Baseline {
            written_at: 1_000,
            first_byte: [("gemini-3.8-flash".to_string(), axis(2_000, 3_500, 70))].into(),
            breaks_per_10k: 200,
            gate_secs: [("gate-zo".to_string(), 800)].into(),
            flake_names: 36,
        }
    }

    fn window() -> Window {
        Window {
            since_secs: 86_400,
            requests: 100,
            breaks: 2,
            first_byte: [("gemini-3.8-flash".to_string(), axis(2_100, 3_600, 60))].into(),
            gate_secs: [("gate-zo".to_string(), vec![790, 820])].into(),
            flake_names: Some(36),
            disk_free_gb: Some(40),
        }
    }

    #[test]
    fn a_normal_window_files_nothing() {
        assert!(judge(&baseline(), &window(), &[], &Thresholds::default()).is_empty());
    }

    #[test]
    fn a_first_byte_that_rose_a_quarter_is_a_finding_with_its_numbers() {
        let mut w = window();
        w.first_byte.insert("gemini-3.8-flash".to_string(), axis(2_600, 3_600, 60));
        let found = judge(&baseline(), &w, &[], &Thresholds::default());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].key, "first-byte:gemini-3.8-flash");
        assert!(found[0].words.contains("2000 → 2600 ms"), "{}", found[0].words);
        // The same key already open: nothing new.
        assert!(judge(&baseline(), &w, &[found[0].key.clone()], &Thresholds::default()).is_empty());
        // Too few requests to say: nothing.
        w.first_byte.insert("gemini-3.8-flash".to_string(), axis(2_600, 3_600, 5));
        assert!(judge(&baseline(), &w, &[], &Thresholds::default()).is_empty());
    }

    #[test]
    fn a_thin_baseline_judges_nothing_however_full_the_day() {
        // The committed baseline once held claude-fable-5-1 at n=2: its p90
        // was the larger of two requests. A real day of 60 must not be judged
        // against that.
        let mut b = baseline();
        b.first_byte.insert("gemini-3.8-flash".to_string(), axis(1_000, 1_200, 2));
        let w = window();
        assert!(
            judge(&b, &w, &[], &Thresholds::default()).is_empty(),
            "{:?}",
            judge(&b, &w, &[], &Thresholds::default())
        );
    }

    #[test]
    fn breaks_gates_flakes_and_disk_each_have_a_line() {
        let mut w = window();
        w.breaks = 6; // 6% vs 2% baseline
        w.gate_secs.insert("gate-zo".to_string(), vec![1_300, 1_250, 1_400]);
        w.flake_names = Some(38);
        w.disk_free_gb = Some(9);
        let keys: Vec<String> = judge(&baseline(), &w, &[], &Thresholds::default())
            .into_iter()
            .map(|f| f.key)
            .collect();
        assert_eq!(keys, ["cache-break-rate", "gate:gate-zo", "flakes", "disk"]);
    }

    #[test]
    fn the_window_keeps_only_completed_recent_rows_and_quiet_green_gates() {
        let timing = |at: u64, model: &str, ttfb: u64, outcome: &str| RequestTimingRecord {
            recorded_at: at,
            session_id: "s".into(),
            model: Some(model.into()),
            iteration: 1,
            request_messages: 2,
            attempts: 1,
            assemble_ms: Some(0),
            ttfb_ms: Some(ttfb),
            ttfv_ms: Some(ttfb),
            stream_ms: Some(10),
            outcome: outcome.into(),
            attempt: String::new(),
        };
        let timings = vec![
            timing(900, "m", 100, "completed"),   // too old
            timing(1_500, "m", 200, "completed"),
            timing(1_600, "m", 400, "completed"),
            timing(1_700, "m", 300, "aborted"),   // not completed
        ];
        let breaks = vec![PromptCacheBreakRecord {
            recorded_at: 1_500,
            session_id: "s".into(),
            model: None,
            iteration: 2,
            request_messages: 3,
            reason: "r".into(),
            previous_cache_read: 6_000,
            current_cache_read: 1_000,
            token_drop: 5_000,
            warning: None,
        }];
        let lines = [
            "10:21:16 gate-root rc=0 795s free=20G load=3.56/7.91",
            "08:33:27 gate-zo rc=101 22231s free=19G load=30.1/28.0", // red
            "02:22:56 gate-root rc=0 1697s free=24G load=31.2/29.0",  // loud
            "17:31:09 gate-root rc=0 749s free=31G",                  // no load word: unknown, skipped
            "09:06:29 == flakes",
        ];
        let w = window_from(
            2_000,
            1_000,
            &Evidence {
                timings: &timings,
                breaks: &breaks,
                lane_lines: &lines,
                calm_load: 12.0,
                flake_names: Some(36),
                disk_free_gb: Some(20),
            },
        );
        assert_eq!(w.requests, 2);
        assert_eq!(w.breaks, 1);
        assert_eq!(w.first_byte["m"], axis(200, 400, 2));
        assert_eq!(w.gate_secs["gate-root"], vec![795]);
        let b = baseline_from(2_000, &w).expect("the flake list was read");
        assert_eq!(b.gate_secs["gate-root"], 795);
        assert_eq!(b.breaks_per_10k, 5_000);
        assert_eq!(percent_of_10k(5_000), "50.00%");
        assert_eq!(percent_of_10k(123), "1.23%");
    }

    #[test]
    fn a_gate_that_started_loud_is_not_a_quiet_run_whatever_the_load_at_its_end() {
        let lines = [
            "19:00:06 == lane ab484234 start (dry=0)",
            "19:00:08 == gate-zo",
            "19:15:08 gate-zo: loud — load 31.2 > 12 after 900s, running anyway",
            // Hours under load; the last minute happened to be calm.
            "01:22:56 gate-zo rc=0 22000s free=24G load=3.1/9.0",
            "01:22:56 == gate-root",
            "01:22:57 gate-root: load unreadable — running without the calm check",
            "01:40:00 gate-root rc=0 1020s free=24G load=2.0/4.0",
            // The next run of the lane starts clean.
            "09:00:00 == lane cd123456 start (dry=0)",
            "09:13:00 gate-zo rc=0 780s free=30G load=4.0/5.0",
        ];
        let w = window_from(
            2_000,
            1_000,
            &Evidence {
                timings: &[],
                breaks: &[],
                lane_lines: &lines,
                calm_load: 12.0,
                flake_names: Some(0),
                disk_free_gb: None,
            },
        );
        assert_eq!(w.gate_secs.get("gate-zo"), Some(&vec![780]), "{:?}", w.gate_secs);
        assert_eq!(w.gate_secs.get("gate-root"), None, "{:?}", w.gate_secs);
    }

    #[test]
    fn the_lane_words_read_here_are_the_words_lane_sh_logs() {
        let lane = include_str!("../../../../tools/release/lane.sh");
        let run = format!("log \"{LANE_RUN_MARK}$SHA8 start");
        assert!(lane.contains(&run), "lane.sh opens a run with {run}");
        for missed in LANE_CALM_MISSED {
            let said = format!("log \"$name: {missed}");
            assert!(lane.contains(&said), "wait_for_calm logs {said}");
        }
        let text = "09:00:00 == lane a start\n09:01:00 x\n19:00:00 == lane a start\n19:01:00 y\n";
        assert_eq!(last_lane_run(text), "19:00:00 == lane a start\n19:01:00 y\n");
        assert_eq!(last_lane_run("09:01:00 x\n"), "09:01:00 x\n", "no run mark: the whole log");
    }

    #[test]
    fn lane_lines_parse_with_and_without_the_load_word() {
        assert_eq!(
            parse_lane_phase_line("10:21:16 gate-root rc=0 795s free=20G load=3.56/7.91"),
            Some(("gate-root".into(), 0, 795, Some(3.56)))
        );
        assert_eq!(
            parse_lane_phase_line("17:31:09 gate-zo rc=101 136s free=29G"),
            Some(("gate-zo".into(), 101, 136, None))
        );
        assert_eq!(parse_lane_phase_line("09:06:29 == flakes"), None);
    }

    #[test]
    fn percentiles_are_nearest_rank_and_test_slugs_are_named() {
        assert_eq!(percentile(&[5, 1, 3], 50), Some(3));
        assert_eq!(percentile(&[5, 1, 3], 90), Some(5));
        assert_eq!(percentile(&[], 50), None);
        assert!(is_test_slug("2026-zerocode-a1098da8-scratchpad-ttft-seed-3544fc7fb33e67bc"));
        assert!(!is_test_slug("Users-dev-2026-zerocode-0fef7579911bc688"));
    }

    #[test]
    fn a_temporary_workspace_is_named_by_its_slug_tail_or_by_the_path_it_recorded() {
        // `project_slug` keeps the LAST 80 characters, so a bench round's
        // `mkdtemp` under /private/var/folders loses its `private-` prefix…
        assert!(is_test_slug(
            "e-var-folders-yv-23c9krnj0h983cl1_j42h2y80000gn-T-r38-bench-r1-17-zo-nv-7w4txvwh-fc27b75ef2a15d5b"
        ));
        // …and a longer name loses `var-folders` altogether; only the path
        // the session recorded still says where it was.
        let temp = std::path::PathBuf::from("/private/var/folders/yv/x/T");
        let roots = [temp.clone(), std::path::PathBuf::from("/private/tmp")];
        assert!(is_temporary_workspace(&temp.join("zo-bench-1786356721-deep"), &roots));
        assert!(is_temporary_workspace(std::path::Path::new("/private/tmp/r50"), &roots));
        assert!(!is_temporary_workspace(std::path::Path::new("/Users/dev/work"), &roots));
        assert!(
            !is_temporary_workspace(std::path::Path::new("/private/tmpfoo/x"), &roots),
            "a prefix is a whole component"
        );
    }
}
