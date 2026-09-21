//! `zo scoreboard` — the scoreboard beat's hands.
//!
//! Gathers the recent evidence (request timings and prompt-cache breaks under
//! every real project of the zo home, the release lane's phase lines, the
//! flake list, the disk), judges it against a baseline file with
//! [`runtime::scoreboard::judge`], prints the findings as JSON and — with
//! `--file-tasks` — writes each new one into the orchestration ledger through
//! `zerocode-orc task-create`, once per key per day — or, with `--defer <file>`,
//! appends them to a pending file the window's beat files from its own seat
//! (`scoreboard_inbox` in the shell), which is how a launchd clock with no
//! pane identity reaches the ledger. `--write-baseline` declares the window
//! normal and writes the baseline; a person does that after a green release,
//! never the beat (docs/design/scoreboard-beat-20260911.md).

use std::path::{Path, PathBuf};
use std::process::Command;

use runtime::prompt_cache_breaks::read_prompt_cache_breaks_at_path;
use runtime::request_timings::read_request_timings_at_path;
use runtime::scoreboard::{
    Baseline, Evidence, Finding, Thresholds, Window, baseline_from, is_temporary_workspace, is_test_slug, judge,
    last_lane_run, window_from,
};
use serde_json::{Value, json};

const DEFAULT_SINCE_SECS: u64 = 24 * 3600;
const DEFAULT_BASELINE: &str = "zo-ide/bench/scoreboard-baseline.json";
const DEFAULT_LANE_LOGS: &str = ".local/share/zerocode/release/out";
const DEFAULT_FLAKES: &str = "tools/release/flakes.txt";
const DEFAULT_CALM_LOAD: f64 = 12.0;
/// The title marker a filed task carries, so a rerun can see it is open.
const TASK_KEY_OPEN: &str = "[scoreboard:";
const TASK_KEY_CLOSE: char = ']';

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub since_secs: u64,
    pub baseline: PathBuf,
    pub write_baseline: bool,
    pub file_tasks: bool,
    /// Append findings here instead of filing them; the window's beat files
    /// them from its seat.
    pub defer: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub lane_logs: Option<PathBuf>,
    pub flakes: PathBuf,
    pub calm_load: f64,
}

/// The units `--since` takes, and the seconds in one of each.
const SINCE_UNITS: [(char, u64); 3] = [('m', 60), ('h', 3600), ('d', 86_400)];

/// `Nm` / `Nh` / `Nd` → seconds. The unit is the last character, whatever
/// its width, and a span past the clock is refused rather than wrapped.
fn parse_since(word: &str) -> Result<u64, String> {
    let refused = || format!("--since wants <N>m|<N>h|<N>d, not {word}");
    let mut chars = word.chars();
    let unit = chars.next_back().ok_or_else(refused)?;
    let (_, secs) = SINCE_UNITS
        .iter()
        .find(|(named, _)| *named == unit)
        .ok_or_else(refused)?;
    let n: u64 = chars.as_str().parse().map_err(|_| refused())?;
    n.checked_mul(*secs).ok_or_else(refused)
}

pub fn parse(args: &[String], cwd: &Path) -> Result<Request, String> {
    let mut request = Request {
        since_secs: DEFAULT_SINCE_SECS,
        baseline: cwd.join(DEFAULT_BASELINE),
        write_baseline: false,
        file_tasks: false,
        defer: None,
        home: None,
        lane_logs: None,
        flakes: cwd.join(DEFAULT_FLAKES),
        calm_load: DEFAULT_CALM_LOAD,
    };
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let mut value = |flag: &str| words.next().cloned().ok_or_else(|| format!("{flag} wants a value"));
        match word.as_str() {
            "--since" => request.since_secs = parse_since(&value("--since")?)?,
            "--baseline" => request.baseline = cwd.join(value("--baseline")?),
            "--write-baseline" => request.write_baseline = true,
            "--file-tasks" => request.file_tasks = true,
            "--defer" => request.defer = Some(cwd.join(value("--defer")?)),
            "--home" => request.home = Some(cwd.join(value("--home")?)),
            "--lane-logs" => request.lane_logs = Some(cwd.join(value("--lane-logs")?)),
            "--flakes" => request.flakes = cwd.join(value("--flakes")?),
            "--calm-load" => {
                request.calm_load = value("--calm-load")?
                    .parse()
                    .map_err(|_| "--calm-load wants a number".to_string())?;
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(request)
}

/// Non-empty, non-comment lines.
#[must_use]
pub fn count_flake_names(text: &str) -> usize {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .count()
}

/// The scoreboard keys of tasks already open, read off a `task-list` answer.
#[must_use]
pub fn open_keys_from_task_list(answer: &Value) -> Vec<String> {
    answer["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|task| task["title"].as_str())
        .filter_map(|title| {
            let start = title.find(TASK_KEY_OPEN)? + TASK_KEY_OPEN.len();
            let end = title[start..].find(TASK_KEY_CLOSE)? + start;
            Some(title[start..end].to_string())
        })
        .collect()
}

/// Free gigabytes on the root volume, as `df -k /` says; `None` off-platform.
fn disk_free_gb() -> Option<u64> {
    let output = Command::new("df").args(["-k", "/"]).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().nth(1)?;
    let avail_kb: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(avail_kb / (1024 * 1024))
}

/// The machine's temporary roots, as written and as resolved (`/tmp` is
/// `/private/tmp` on macOS; `$TMPDIR` is `/var/folders/…` and resolves under
/// `/private`) — a session records whichever its process was handed.
fn temp_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for root in [std::env::temp_dir(), PathBuf::from("/tmp")] {
        if let Ok(resolved) = root.canonicalize() {
            roots.push(resolved);
        }
        roots.push(root);
    }
    roots.sort();
    roots.dedup();
    roots
}

/// The workspace a project's sessions recorded (the `.cwd` sidecar beside a
/// transcript, [`crate::resume::session_cwd_path`]), if any did.
fn recorded_workspace(project: &Path) -> Option<PathBuf> {
    std::fs::read_dir(project.join("sessions"))
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .find_map(|transcript| crate::resume::load_session_cwd(&transcript))
}

/// The recent evidence under `home`, `lane_logs` and `flakes`.
#[must_use]
pub fn gather(request: &Request, home: &Path, lane_logs: &Path, now: u64, disk: Option<u64>) -> Window {
    let floor = now.saturating_sub(request.since_secs);
    let mut timings = Vec::new();
    let mut breaks = Vec::new();
    let temp_roots = temp_roots();
    if let Ok(projects) = std::fs::read_dir(home.join("projects")) {
        for project in projects.flatten() {
            let slug = project.file_name().to_string_lossy().into_owned();
            if is_test_slug(&slug) {
                continue;
            }
            let state = project.path().join("state");
            let project_timings =
                read_request_timings_at_path(&state.join("request-timings").join("timings.jsonl")).unwrap_or_default();
            let project_breaks =
                read_prompt_cache_breaks_at_path(&state.join("prompt-cache").join("breaks.jsonl")).unwrap_or_default();
            // Only a project with evidence is asked where it ran — a handful
            // among thousands of project directories.
            if project_timings.is_empty() && project_breaks.is_empty() {
                continue;
            }
            if recorded_workspace(&project.path()).is_some_and(|cwd| is_temporary_workspace(&cwd, &temp_roots)) {
                continue;
            }
            timings.extend(project_timings);
            breaks.extend(project_breaks);
        }
    }
    let mut lane_text = String::new();
    if let Ok(logs) = std::fs::read_dir(lane_logs) {
        for log in logs.flatten() {
            let name = log.file_name().to_string_lossy().into_owned();
            let is_lane_log = name.starts_with("lane-")
                && Path::new(&name)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("log"));
            if !is_lane_log {
                continue;
            }
            let recent = log
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .is_some_and(|age| age.as_secs() >= floor);
            if recent {
                if let Ok(text) = std::fs::read_to_string(log.path()) {
                    // The mtime dates only the log's last run.
                    lane_text.push_str(last_lane_run(&text));
                    lane_text.push('\n');
                }
            }
        }
    }
    let lane_lines: Vec<&str> = lane_text.lines().collect();
    // An unreadable list is unread, not empty: a zero would hide its growth.
    let flake_names = std::fs::read_to_string(&request.flakes)
        .ok()
        .map(|text| count_flake_names(&text));
    window_from(
        now,
        request.since_secs,
        &Evidence {
            timings: &timings,
            breaks: &breaks,
            lane_lines: &lane_lines,
            calm_load: request.calm_load,
            flake_names,
            disk_free_gb: disk,
        },
    )
}

/// `YYYYMMDD` of a unix time (civil-from-days, Hinnant).
#[must_use]
pub fn day_stamp(now: u64) -> String {
    let z = i64::try_from(now / 86_400).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}{m:02}{d:02}")
}

fn orc(args: &[&str]) -> Result<Value, String> {
    let output = Command::new("zerocode-orc")
        .args(args)
        .output()
        .map_err(|error| format!("zerocode-orc did not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "zerocode-orc {} refused: {}",
            args.first().copied().unwrap_or(""),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| format!("zerocode-orc answered no JSON: {error}"))
}

/// The ledger door: one `zerocode-orc` verb, its JSON answer or its refusal.
pub type Orc<'a> = &'a dyn Fn(&[&str]) -> Result<Value, String>;

/// Keys of the scoreboard tasks the ledger has not finished — whatever their
/// status, as the ledger judges it (`task-list --open`), in the run this pane
/// is bound to. A ledger that cannot be read answers its refusal.
fn open_keys_with(orc: Orc<'_>) -> Result<Vec<String>, String> {
    orc(&["task-list", "--open"]).map(|answer| open_keys_from_task_list(&answer))
}

/// File each finding; a refusal is reported beside the filed ones and never
/// stops the findings behind it.
fn file_tasks_with(findings: &[Finding], now: u64, orc: Orc<'_>) -> (Vec<Value>, Vec<Value>) {
    let mut filed = Vec::new();
    let mut refused = Vec::new();
    for finding in findings {
        match file_task(finding, now, orc) {
            Ok(answer) => filed.push(answer),
            Err(why) => refused.push(json!({ "key": finding.key, "why": why })),
        }
    }
    (filed, refused)
}

fn file_task(finding: &Finding, now: u64, orc: Orc<'_>) -> Result<Value, String> {
    let key_slug: String = finding
        .key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let retry = format!("scoreboard-{key_slug}-{}", day_stamp(now));
    let title = format!("점수표 {TASK_KEY_OPEN}{}{TASK_KEY_CLOSE} {}", finding.key, finding.observed);
    let spec = format!(
        "{} — observed {}, baseline {}. Filed by `zo scoreboard` (docs/design/scoreboard-beat-20260911.md): find the cause with numbers, fix red-first, and say what moved in the commit.",
        finding.words, finding.observed, finding.baseline
    );
    orc(&["task-create", "--retry-request", &retry, "--title", &title, "--spec", &spec])
}

pub fn run(args: &[String], cwd: &Path, now: u64) -> Result<String, String> {
    run_with(args, cwd, now, disk_free_gb(), &orc)
}

/// One pending line per finding, appended (`O_APPEND`, one write each) with
/// the time it was found — the shape the window's `scoreboard_inbox` reads.
fn defer_findings(file: &Path, findings: &[Finding], now: u64) -> Result<usize, String> {
    use std::io::Write as _;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .map_err(|error| format!("could not open {}: {error}", file.display()))?;
    for finding in findings {
        let mut line = json!({
            "key": finding.key,
            "observed": finding.observed,
            "baseline": finding.baseline,
            "words": finding.words,
            "found_at": now,
        })
        .to_string();
        line.push('\n');
        out.write_all(line.as_bytes()).map_err(|error| error.to_string())?;
    }
    Ok(findings.len())
}

/// [`run`] with the disk reading and the ledger door handed in — the tests'
/// road, since a test must not read the developer's disk (the same rule the
/// knowledge graph tests keep for their headroom fixture) nor speak to a
/// real ledger.
pub fn run_with(args: &[String], cwd: &Path, now: u64, disk: Option<u64>, orc: Orc<'_>) -> Result<String, String> {
    let request = parse(args, cwd)?;
    let home = request.home.clone().unwrap_or_else(runtime::default_config_home);
    let lane_logs = request.lane_logs.clone().unwrap_or_else(|| {
        std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
            .join(DEFAULT_LANE_LOGS)
    });
    let window = gather(&request, &home, &lane_logs, now, disk);
    if request.write_baseline {
        let baseline = baseline_from(now, &window)?;
        if let Some(parent) = request.baseline.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(
            &request.baseline,
            serde_json::to_string_pretty(&baseline).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        return Ok(json!({"wrote_baseline": request.baseline, "window": window}).to_string());
    }
    let baseline: Baseline = std::fs::read_to_string(&request.baseline)
        .map_err(|_| format!("no baseline at {} — write one with --write-baseline after a green release", request.baseline.display()))
        .and_then(|text| serde_json::from_str(&text).map_err(|error| format!("baseline does not parse: {error}")))?;
    // Filing needs the open tasks first. A ledger that cannot be read files
    // nothing — filing blind would cut a second task for a regression already
    // being worked — but the findings are still judged, printed and deferred.
    let open = request.file_tasks.then(|| open_keys_with(orc));
    let open_keys = match &open {
        Some(Ok(keys)) => keys.as_slice(),
        _ => &[],
    };
    let findings = judge(&baseline, &window, open_keys, &Thresholds::default());
    let (filed, refused) = match &open {
        Some(Ok(_)) => file_tasks_with(&findings, now, orc),
        Some(Err(why)) => (
            Vec::new(),
            findings
                .iter()
                .map(|finding| json!({ "key": finding.key, "why": format!("the open tasks could not be read: {why}") }))
                .collect(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let deferred = match &request.defer {
        Some(file) if !findings.is_empty() => defer_findings(file, &findings, now)?,
        _ => 0,
    };
    Ok(json!({
        "window": window,
        "baseline_written_at": baseline.written_at,
        "findings": findings,
        "filed": filed,
        "refused": refused,
        "deferred": deferred,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ledger as `zerocode-orc` answers it: `task-list --status` knows the
    /// six statuses and refuses any other word, `task-list --open` is every
    /// task not completed or failed, and `task-create` refuses the named keys.
    fn ledger(refuse: &'static [&'static str]) -> impl Fn(&[&str]) -> Result<Value, String> {
        let tasks = [
            ("점수표 [scoreboard:disk] 9 G free", "dispatched"),
            ("점수표 [scoreboard:flakes] 38", "ready"),
            ("점수표 [scoreboard:gate:gate-zo] 900 s", "completed"),
            ("점수표 [scoreboard:first-byte:m] p50 2000 ms", "blocked"),
        ];
        move |args: &[&str]| {
            let listed = |keep: &dyn Fn(&str) -> bool| {
                let rows: Vec<Value> = tasks
                    .iter()
                    .filter(|(_, status)| keep(status))
                    .map(|(title, status)| json!({ "title": title, "status": status }))
                    .collect();
                Ok(json!({ "tasks": rows }))
            };
            match args {
                ["task-list", "--open"] => listed(&|status| !matches!(status, "completed" | "failed")),
                ["task-list", "--status", word] => {
                    if !matches!(*word, "pending" | "ready" | "dispatched" | "completed" | "failed" | "blocked") {
                        return Err(format!("zerocode-orc task-list refused: unknown status: {word}"));
                    }
                    listed(&|status| status == *word)
                }
                ["task-create", ..] if refuse.iter().any(|key| args.iter().any(|arg| arg.contains(&format!("[scoreboard:{key}]")))) => {
                    Err("zerocode-orc task-create refused: no run in use".to_string())
                }
                ["task-create", ..] => Ok(json!({ "taskId": "t-7" })),
                other => Err(format!("unexpected {other:?}")),
            }
        }
    }

    fn finding(key: &str) -> Finding {
        Finding {
            key: key.to_string(),
            observed: "2600 ms".to_string(),
            baseline: "2000 ms".to_string(),
            words: format!("{key} rose"),
        }
    }

    #[test]
    fn every_task_the_ledger_has_not_finished_holds_its_key() {
        let open = open_keys_with(&ledger(&[])).expect("the ledger answered");
        assert_eq!(open, ["disk", "flakes", "first-byte:m"], "dispatched, ready and blocked are all open");
    }

    #[test]
    fn a_refused_finding_is_reported_and_the_findings_behind_it_are_still_filed() {
        let findings = [finding("disk"), finding("flakes")];
        let (filed, refused) = file_tasks_with(&findings, 1_789_100_000, &ledger(&["disk"]));
        assert_eq!(filed, [json!({ "taskId": "t-7" })], "the second finding was filed");
        assert_eq!(refused.len(), 1, "{refused:?}");
        assert_eq!(refused[0]["key"], "disk");
        assert!(refused[0]["why"].as_str().is_some_and(|why| why.contains("no run in use")), "{refused:?}");
    }

    #[test]
    fn since_refuses_a_unit_it_does_not_know_and_a_span_past_the_clock_instead_of_panicking() {
        let cwd = Path::new("/w");
        let since = |word: &str| parse(&["--since".into(), word.into()], cwd).map(|r| r.since_secs);
        assert_eq!(since("90m"), Ok(5_400));
        assert_eq!(since("2h"), Ok(7_200));
        for word in ["7일", "5ｈ", "h", "", "213503982334602d"] {
            assert!(since(word).is_err(), "{word:?} is refused, not a panic");
        }
    }

    #[test]
    fn a_flake_list_that_cannot_be_read_is_unread_never_zero() {
        let root = tempfile::tempdir().expect("tempdir");
        let args = |extra: &[&str]| -> Vec<String> {
            let mut v: Vec<String> = vec![
                "--home".into(), root.path().join("home").to_string_lossy().into(),
                "--lane-logs".into(), root.path().join("lanes").to_string_lossy().into(),
                "--flakes".into(), root.path().join("moved.txt").to_string_lossy().into(),
                "--baseline".into(), "base.json".into(),
            ];
            v.extend(extra.iter().map(std::string::ToString::to_string));
            v
        };
        let request = parse(&args(&[]), root.path()).unwrap();
        let window = gather(&request, &root.path().join("home"), &root.path().join("lanes"), 10_000, None);
        assert!(serde_json::to_value(&window).unwrap()["flake_names"].is_null(), "{window:?}");
        let refused = run_with(&args(&["--write-baseline"]), root.path(), 10_000, Some(100), &no_ledger);
        assert!(
            refused.as_ref().is_err_and(|why| why.contains("flake list")),
            "a baseline that would record no flake count is refused: {refused:?}"
        );
        assert!(!root.path().join("base.json").exists());
    }

    #[test]
    fn arguments_have_defaults_and_units() {
        let cwd = Path::new("/w");
        let r = parse(&[], cwd).expect("defaults");
        assert_eq!(r.since_secs, 86_400);
        assert_eq!(r.baseline, Path::new("/w/zo-ide/bench/scoreboard-baseline.json"));
        let r = parse(&["--since".into(), "7d".into(), "--file-tasks".into()], cwd).expect("parse");
        assert_eq!(r.since_secs, 7 * 86_400);
        assert!(r.file_tasks);
        assert!(parse(&["--since".into(), "soon".into()], cwd).is_err());
        assert!(parse(&["--wat".into()], cwd).is_err());
    }

    #[test]
    fn open_keys_and_flake_names_and_day_stamps_read_right() {
        let answer = json!({"tasks": [
            {"title": "점수표 [scoreboard:first-byte:claude-opus-5] p50 1800 ms"},
            {"title": "something else"},
            {"title": "점수표 [scoreboard:disk] 9 G free"}
        ]});
        assert_eq!(open_keys_from_task_list(&answer), ["first-byte:claude-opus-5", "disk"]);
        assert_eq!(count_flake_names("# comment\nroot:a\n\nzo:b\n"), 2);
        assert_eq!(day_stamp(1_789_100_000), "20260911");
        assert_eq!(day_stamp(0), "19700101");
    }

    #[test]
    fn a_lane_log_a_rerun_appended_to_counts_only_its_last_run() {
        // lane.sh appends a re-queued sha's run to the same lane-<sha8>.log
        // (`tee -a`), and its lines carry only a clock: the fresh mtime would
        // pull the old run's gates into today's window.
        let root = tempfile::tempdir().expect("tempdir");
        let lanes = root.path().join("lanes");
        std::fs::create_dir_all(&lanes).unwrap();
        std::fs::write(
            lanes.join("lane-abcd1234.log"),
            "09:00:00 == lane abcd1234 start (dry=0)\n09:20:00 gate-zo rc=0 1200s free=20G load=3.0/4.0\n\
             19:00:00 == lane abcd1234 start (dry=0)\n19:13:00 gate-zo rc=0 780s free=20G load=3.0/4.0\n",
        )
        .unwrap();
        let flakes = root.path().join("flakes.txt");
        std::fs::write(&flakes, "").unwrap();
        let request = parse(&["--flakes".into(), flakes.to_string_lossy().into()], root.path()).unwrap();
        let window = gather(&request, &root.path().join("home"), &lanes, 10_000, None);
        assert_eq!(window.gate_secs.get("gate-zo"), Some(&vec![780]), "{:?}", window.gate_secs);
    }

    #[test]
    fn a_bench_workspace_whose_slug_lost_its_temporary_prefix_is_still_a_fixture() {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("home");
        // The slug's tail kept none of `/private/var/folders/…`; the session
        // sidecar still recorded the temporary workspace it ran in.
        let bench = home.join("projects/3c9krnj0h983cl1_j42h2y80000gn-T-zo-bench-1786356721-deep-ledger-zo-t1-69d8a29ec313aa02");
        std::fs::create_dir_all(bench.join("state/request-timings")).unwrap();
        std::fs::create_dir_all(bench.join("sessions")).unwrap();
        let transcript = bench.join("sessions/session-1-0.jsonl");
        std::fs::write(&transcript, "").unwrap();
        let workspace = std::env::temp_dir().join("zo-bench-1786356721-deep-ledger-zo-t1");
        std::fs::write(
            crate::resume::session_cwd_path(&transcript),
            json!({ "cwd": workspace }).to_string(),
        )
        .unwrap();
        std::fs::write(
            bench.join("state/request-timings/timings.jsonl"),
            r#"{"recorded_at":9500,"session_id":"s","model":"m","iteration":1,"request_messages":2,"attempts":1,"assemble_ms":0,"ttfb_ms":90000,"ttfv_ms":90000,"stream_ms":5,"outcome":"completed"}"#.to_string() + "\n",
        )
        .unwrap();
        let flakes = root.path().join("flakes.txt");
        std::fs::write(&flakes, "").unwrap();
        let request = parse(
            &["--flakes".into(), flakes.to_string_lossy().into(), "--since".into(), "1h".into()],
            root.path(),
        )
        .unwrap();
        let window = gather(&request, &home, &root.path().join("no-lanes"), 10_000, None);
        assert_eq!(window.requests, 0, "the bench round's row was skipped: {window:?}");
    }

    #[test]
    fn the_window_is_gathered_from_real_projects_only_and_a_baseline_round_trips_into_a_finding() {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("home");
        let real = home.join("projects/Users-me-work-abc/state");
        let fixture = home.join("projects/private-var-folders-xy-T-zo-bench-def/state");
        for state in [&real, &fixture] {
            std::fs::create_dir_all(state.join("request-timings")).unwrap();
            std::fs::create_dir_all(state.join("prompt-cache")).unwrap();
        }
        let row = |at: u64, ttfb: u64| format!(r#"{{"recorded_at":{at},"session_id":"s","model":"m","iteration":1,"request_messages":2,"attempts":1,"assemble_ms":0,"ttfb_ms":{ttfb},"ttfv_ms":{ttfb},"stream_ms":5,"outcome":"completed"}}"#);
        let now = 10_000;
        let real_rows: Vec<String> = (0..30).map(|i| row(9_000 + i, 1_000)).collect();
        std::fs::write(real.join("request-timings/timings.jsonl"), real_rows.join("\n") + "\n").unwrap();
        // The temporary workspace's slug carries a wild outlier the beat must
        // never see.
        std::fs::write(fixture.join("request-timings/timings.jsonl"), row(9_500, 90_000) + "\n").unwrap();
        std::fs::write(real.join("prompt-cache/breaks.jsonl"), "").unwrap();
        let lanes = root.path().join("lanes");
        std::fs::create_dir_all(&lanes).unwrap();
        std::fs::write(lanes.join("lane-abc.log"), "10:21:16 gate-root rc=0 795s free=20G load=3.5/7.9\n10:22:00 gate-zo rc=0 700s free=20G load=4.0/7.0\n").unwrap();
        let flakes = root.path().join("flakes.txt");
        std::fs::write(&flakes, "zo:a\nzo:b\n").unwrap();
        let cwd = root.path();
        let base_args = |extra: &[&str]| -> Vec<String> {
            let mut v: Vec<String> = vec![
                "--home".into(), home.to_string_lossy().into(),
                "--lane-logs".into(), lanes.to_string_lossy().into(),
                "--flakes".into(), flakes.to_string_lossy().into(),
                "--baseline".into(), "base.json".into(),
                "--since".into(), "1h".into(),
            ];
            v.extend(extra.iter().map(std::string::ToString::to_string));
            v
        };
        // Lane logs are chosen by mtime against `now - since`; a small `now`
        // keeps every freshly written log.
        let request = parse(&base_args(&[]), cwd).unwrap();
        let window = gather(&request, &home, &lanes, now, None);
        assert_eq!(window.requests, 30, "the fixture slug's row was skipped");
        assert_eq!(window.first_byte["m"].p50_ms, 1_000);
        assert_eq!(window.flake_names, Some(2));

        let wrote = run_with(&base_args(&["--write-baseline"]), cwd, now, Some(100), &no_ledger).expect("write");
        assert!(wrote.contains("wrote_baseline"));
        let quiet = run_with(&base_args(&[]), cwd, now, Some(100), &no_ledger).expect("judge");
        let quiet: Value = serde_json::from_str(&quiet).unwrap();
        assert_eq!(quiet["findings"].as_array().unwrap().len(), 0);

        // The next day the same model answers half as fast.
        let slow_rows: Vec<String> = (0..30).map(|i| row(19_000 + i, 2_000)).collect();
        std::fs::write(real.join("request-timings/timings.jsonl"), slow_rows.join("\n") + "\n").unwrap();
        let later = run_with(&base_args(&[]), cwd, 20_000, Some(100), &no_ledger).expect("judge");
        let later: Value = serde_json::from_str(&later).unwrap();
        let findings = later["findings"].as_array().unwrap();
        assert_eq!(findings.len(), 1, "{later}");
        assert_eq!(findings[0]["key"], "first-byte:m");
        assert!(findings[0]["words"].as_str().unwrap().contains("1000 → 2000 ms"));

        // Deferred: the finding lands in the pending file, one line, with its time.
        let pending = root.path().join("inbox/pending.jsonl");
        let deferred = run_with(
            &base_args(&["--defer", pending.to_string_lossy().as_ref()]),
            cwd,
            20_000,
            Some(100),
            &no_ledger,
        )
        .expect("defer");
        let deferred: Value = serde_json::from_str(&deferred).unwrap();
        assert_eq!(deferred["deferred"], 1);
        let rows: Vec<Value> = std::fs::read_to_string(&pending)
            .unwrap()
            .lines()
            .map(|row| serde_json::from_str(row).unwrap())
            .collect();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["key"], "first-byte:m");
        assert_eq!(rows[0]["found_at"], 20_000);

        // Filing through a ledger that cannot be read files nothing — never
        // blind — and loses nothing: the findings are still printed, each
        // refused with the ledger's reason, and still deferred.
        let unreadable =
            |args: &[&str]| -> Result<Value, String> { Err(format!("zerocode-orc {} refused: no run in use", args[0])) };
        let blind = run_with(
            &base_args(&["--file-tasks", "--defer", pending.to_string_lossy().as_ref()]),
            cwd,
            20_000,
            Some(100),
            &unreadable,
        )
        .expect("an unreadable ledger does not end the run");
        let blind: Value = serde_json::from_str(&blind).unwrap();
        assert_eq!(blind["findings"].as_array().map(Vec::len), Some(1), "{blind}");
        assert_eq!(blind["filed"], json!([]));
        assert_eq!(blind["refused"][0]["key"], "first-byte:m");
        assert!(blind["refused"][0]["why"].as_str().is_some_and(|why| why.contains("no run in use")), "{blind}");
        assert_eq!(blind["deferred"], 1, "the deferral waited on the ledger");
    }

    /// The door for a run that never files: any call is a test bug.
    fn no_ledger(args: &[&str]) -> Result<Value, String> {
        panic!("no ledger call without --file-tasks: {args:?}")
    }
}
