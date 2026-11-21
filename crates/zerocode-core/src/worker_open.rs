//! Guard for macOS `open` calls from ledger-summoned worker panes.
//!
//! When an unauthenticated MCP server (such as `mcp-remote`) runs inside a
//! worker pane and invokes `open <url>`, macOS activates the default browser
//! and steals focus from whatever the person is using. This module owns:
//!
//! - The POSIX `open` shim installed into the window's existing shim directory
//!   on macOS. Human panes and leader panes (`%1`) `exec` the real `open`
//!   immediately in the shell script without touching `zerocode-mirror`.
//! - Parsing macOS `open` arguments so only plain `http(s)` web-address opens
//!   receive `-g` (background) and window-wide deduplication. Files, `-a app`
//!   calls, and other invocations pass through unchanged.
//! - Window-wide deduplication stored in the window's `ZEROCODE_MIRROR_DIR`
//!   under an advisory file lock, keyed by `scheme://host[:port]path` with
//!   query parameters, fragments, and userinfo stripped.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::ops::Range;
use std::path::Path;
use std::time::Duration;

use crate::agent_teams::{
    LEADER_PANE, TEAM_ID_VAR, TEAM_PANE_VAR, TEAM_TOKEN_FILE_VAR, TEAM_TOKEN_VAR,
};
use crate::atomic_file::write_atomically;
use crate::held_file_lock::HeldFileLock;

/// Environment marker the `open` shim sets only when handing off a worker pane's
/// `open` invocation to `zerocode-mirror`. Removed before `exec`ing real `open`.
pub const MARKER: &str = "ZEROCODE_OPEN_GUARD";

/// File name of the shim laid into the window's existing shim directory on macOS.
pub const SHIM_NAME: &str = "open";

/// Production macOS system `open` path; tests inject a fake executable instead.
pub const SYSTEM_OPEN_PATH: &str = "/usr/bin/open";

/// How long a worker-opened web address is suppressed from opening again across
/// all worker panes in this window.
///
/// Rationale: a single unauthenticated worker start measured 6 OAuth `open`
/// calls over 16–20 s (`FINDING.md`), and coordinated worker batches or retries
/// typically span 1–3 minutes. Five minutes (300 s, matching
/// `mcp_first_call::START_LIMIT`) keeps duplicate background tabs from piling up
/// while allowing a fresh background open later if the person still needs to
/// sign in.
pub const RECENT_URL_WINDOW: Duration = Duration::from_secs(300);

const LOCK_FILE_NAME: &str = "worker-open.lock";
const STATE_FILE_NAME: &str = "worker-open-urls.tsv";
const MAX_RECENT_URLS: usize = 256;

/// Outcome of inspecting a worker pane's `open` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDecision {
    /// Arguments to pass to the real `open` binary, or `None` when every URL in
    /// the call was skipped as a recent duplicate.
    pub exec_args: Option<Vec<OsString>>,
    /// One-line notices to print to stderr for each skipped duplicate URL.
    pub notices: Vec<String>,
}

/// Build the POSIX `open` shim script installed on macOS.
///
/// Worker-pane detection lives here once, interpolating the shared team
/// environment variable constants. Non-worker panes (plain shells, leader panes
/// `%1`, and shells without team capability variables) `exec` `real_open`
/// immediately without starting `zerocode-mirror`.
#[must_use]
pub fn open_shim_script(mirror: &Path, real_open: &Path) -> String {
    let pane = TEAM_PANE_VAR;
    let leader = LEADER_PANE;
    let team = TEAM_ID_VAR;
    let token_file = TEAM_TOKEN_FILE_VAR;
    let token = TEAM_TOKEN_VAR;
    let marker = MARKER;
    let mirror = sh_single_quote(&mirror.to_string_lossy());
    let real = sh_single_quote(&real_open.to_string_lossy());
    format!(
        r#"#!/bin/sh
case "${{{pane}:-}}" in
  ''|'{leader}') exec {real} "$@" ;;
esac
if [ -z "${{{team}:-}}" ] || {{ [ -z "${{{token_file}:-}}" ] && [ -z "${{{token}:-}}" ]; }}; then
  exec {real} "$@"
fi
if [ -x {mirror} ]; then
  {marker}=1 exec {mirror} {real} "$@"
fi
exec {real} "$@"
"#
    )
}

fn sh_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Normalize an `http://` or `https://` URL to `scheme://host[:port]path`,
/// omitting default ports (`80`/`443`), `?query`, `#fragment`, and userinfo.
///
/// Returns `None` for non-HTTP(S) schemes, hostless URLs, and file paths.
#[must_use]
pub fn normalize_web_url(raw: &str) -> Option<String> {
    let parsed = url::Url::parse(raw).ok()?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let host = parsed.host_str()?;
    match parsed.port() {
        Some(port) => Some(format!("{scheme}://{host}:{port}{}", parsed.path())),
        None => Some(format!("{scheme}://{host}{}", parsed.path())),
    }
}

/// Decide how a worker pane's `open` call should proceed using
/// [`RECENT_URL_WINDOW`].
#[must_use]
pub fn decide(args: &[OsString], state_dir: Option<&Path>, now_ms: u64) -> OpenDecision {
    decide_with_window(args, state_dir, now_ms, RECENT_URL_WINDOW)
}

/// Same as [`decide`], with an explicit deduplication window for deterministic
/// unit tests.
#[must_use]
pub fn decide_with_window(
    args: &[OsString],
    state_dir: Option<&Path>,
    now_ms: u64,
    window: Duration,
) -> OpenDecision {
    let ParsedOpen::WebUrls {
        has_background,
        targets,
    } = parse_open_args(args)
    else {
        return OpenDecision {
            exec_args: Some(args.to_vec()),
            notices: Vec::new(),
        };
    };

    let (skipped, notices) = dedupe_targets(&targets, state_dir, now_ms, window);
    if skipped.iter().all(|was_skipped| *was_skipped) {
        return OpenDecision {
            exec_args: None,
            notices,
        };
    }

    let mut out_args = Vec::with_capacity(args.len() + usize::from(!has_background));
    if !has_background {
        out_args.push(OsString::from("-g"));
    }
    for (index, arg) in args.iter().enumerate() {
        let drop_arg = targets
            .iter()
            .zip(&skipped)
            .any(|(target, was_skipped)| *was_skipped && target.span.contains(&index));
        if !drop_arg {
            out_args.push(arg.clone());
        }
    }

    OpenDecision {
        exec_args: Some(out_args),
        notices,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct UrlTarget {
    span: Range<usize>,
    key: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ParsedOpen {
    Passthrough,
    WebUrls {
        has_background: bool,
        targets: Vec<UrlTarget>,
    },
}

fn parse_open_args(args: &[OsString]) -> ParsedOpen {
    let mut has_background = false;
    let mut targets = Vec::new();
    let mut end_of_options = false;
    let mut index = 0;

    while index < args.len() {
        let Some(arg) = args[index].to_str() else {
            return ParsedOpen::Passthrough;
        };

        if !end_of_options {
            if arg == "--" {
                end_of_options = true;
                index += 1;
                continue;
            }
            if arg == "--args" {
                break;
            }
            if arg == "-u" || arg == "--url" {
                let Some(next) = args.get(index + 1).and_then(|value| value.to_str()) else {
                    return ParsedOpen::Passthrough;
                };
                let Some(key) = normalize_web_url(next) else {
                    return ParsedOpen::Passthrough;
                };
                targets.push(UrlTarget {
                    span: index..index + 2,
                    key,
                });
                index += 2;
                continue;
            }
            if let Some(raw_url) = arg.strip_prefix("--url=") {
                let Some(key) = normalize_web_url(raw_url) else {
                    return ParsedOpen::Passthrough;
                };
                targets.push(UrlTarget {
                    span: index..index + 1,
                    key,
                });
                index += 1;
                continue;
            }
            if matches!(
                arg,
                "--background" | "--fresh" | "--wait-apps" | "--new" | "--hide"
            ) {
                if arg == "--background" {
                    has_background = true;
                }
                index += 1;
                continue;
            }
            if matches!(
                arg,
                "--arch" | "--env" | "--stdin" | "--stdout" | "--stderr"
            ) {
                if index + 1 >= args.len() {
                    return ParsedOpen::Passthrough;
                }
                index += 2;
                continue;
            }
            if arg.starts_with("--arch=")
                || arg.starts_with("--env=")
                || arg.starts_with("--stdin=")
                || arg.starts_with("--stdout=")
                || arg.starts_with("--stderr=")
            {
                index += 1;
                continue;
            }
            if let Some(flags) = arg.strip_prefix('-')
                && !flags.is_empty()
                && !flags.starts_with('-')
            {
                if !flags
                    .bytes()
                    .all(|byte| matches!(byte, b'g' | b'F' | b'W' | b'n' | b'j'))
                {
                    return ParsedOpen::Passthrough;
                }
                if flags.bytes().any(|byte| byte == b'g') {
                    has_background = true;
                }
                index += 1;
                continue;
            }
            if arg.starts_with('-') {
                return ParsedOpen::Passthrough;
            }
        }

        let Some(key) = normalize_web_url(arg) else {
            return ParsedOpen::Passthrough;
        };
        targets.push(UrlTarget {
            span: index..index + 1,
            key,
        });
        index += 1;
    }

    if targets.is_empty() {
        return ParsedOpen::Passthrough;
    }

    ParsedOpen::WebUrls {
        has_background,
        targets,
    }
}

fn dedupe_targets(
    targets: &[UrlTarget],
    state_dir: Option<&Path>,
    now_ms: u64,
    window: Duration,
) -> (Vec<bool>, Vec<String>) {
    let window_ms = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);
    let Some(dir) = state_dir.filter(|path| path.is_dir()) else {
        return dedupe_within_call_only(targets);
    };

    let Ok(lock_file) = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK_FILE_NAME))
    else {
        return dedupe_within_call_only(targets);
    };
    if lock_file.lock().is_err() {
        return dedupe_within_call_only(targets);
    }
    let _guard = HeldFileLock::locked(lock_file);

    let state_path = dir.join(STATE_FILE_NAME);
    let mut entries = read_recent_entries(&state_path, now_ms, window_ms);
    let mut skipped = Vec::with_capacity(targets.len());
    let mut notices = Vec::new();
    let mut dirty = false;

    for target in targets {
        if let Some(&recorded_ms) = entries.get(&target.key) {
            let age_secs = now_ms.saturating_sub(recorded_ms) / 1000;
            skipped.push(true);
            notices.push(format!(
                "zerocode: skipped repeated open of {} (opened {age_secs}s ago)",
                target.key
            ));
        } else {
            skipped.push(false);
            entries.insert(target.key.clone(), now_ms);
            dirty = true;
        }
    }

    if dirty {
        let _ = write_recent_entries(&state_path, &entries);
    }

    (skipped, notices)
}

fn dedupe_within_call_only(targets: &[UrlTarget]) -> (Vec<bool>, Vec<String>) {
    let mut seen = HashSet::with_capacity(targets.len());
    let mut skipped = Vec::with_capacity(targets.len());
    let mut notices = Vec::new();
    for target in targets {
        if seen.insert(target.key.as_str()) {
            skipped.push(false);
        } else {
            skipped.push(true);
            notices.push(format!(
                "zerocode: skipped repeated open of {} (opened 0s ago)",
                target.key
            ));
        }
    }
    (skipped, notices)
}

fn read_recent_entries(path: &Path, now_ms: u64, window_ms: u64) -> HashMap<String, u64> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    let mut entries = HashMap::new();
    for line in text.lines() {
        let Some((stamp, url)) = line.split_once('\t') else {
            continue;
        };
        let Ok(recorded_ms) = stamp.parse::<u64>() else {
            continue;
        };
        if url.is_empty() || recorded_ms > now_ms {
            continue;
        }
        if now_ms.saturating_sub(recorded_ms) < window_ms {
            entries
                .entry(url.to_string())
                .and_modify(|existing: &mut u64| *existing = (*existing).max(recorded_ms))
                .or_insert(recorded_ms);
        }
    }
    entries
}

fn write_recent_entries(path: &Path, entries: &HashMap<String, u64>) -> std::io::Result<()> {
    let mut rows: Vec<(&String, u64)> = entries.iter().map(|(url, &ts)| (url, ts)).collect();
    rows.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    rows.truncate(MAX_RECENT_URLS);

    let mut body = String::new();
    for (url, ts) in rows {
        body.push_str(&ts.to_string());
        body.push('\t');
        body.push_str(url);
        body.push('\n');
    }
    write_atomically(path, body.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn normalize_web_url_strips_query_fragment_userinfo_and_default_ports() {
        assert_eq!(
            normalize_web_url("https://auth.example.com/authorize?state=123&scope=read#top")
                .as_deref(),
            Some("https://auth.example.com/authorize")
        );
        assert_eq!(
            normalize_web_url("HTTPS://AUTH.EXAMPLE.COM:443/authorize?state=456").as_deref(),
            Some("https://auth.example.com/authorize")
        );
        assert_eq!(
            normalize_web_url("http://example.com:80/login?next=/a").as_deref(),
            Some("http://example.com/login")
        );
        assert_eq!(
            normalize_web_url("http://127.0.0.1:8080/callback?code=xyz").as_deref(),
            Some("http://127.0.0.1:8080/callback")
        );
        assert_eq!(
            normalize_web_url("https://user:secret@auth.example.com/authorize?token=1").as_deref(),
            Some("https://auth.example.com/authorize")
        );
        assert_eq!(
            normalize_web_url("https://example.com"),
            normalize_web_url("https://example.com/")
        );
    }

    #[test]
    fn normalize_web_url_rejects_non_web_schemes_and_paths() {
        for raw in [
            "/Users/dev/notes.txt",
            "./notes.txt",
            "file:///Users/dev/index.html",
            "mailto:dev@example.com",
            "custom-scheme://auth/callback",
            "https://",
            "",
        ] {
            assert_eq!(normalize_web_url(raw), None, "unexpected match for {raw:?}");
        }
    }

    #[test]
    fn decide_passes_through_non_url_explicit_app_and_args_tail() {
        let dir = tempfile::tempdir().expect("tempdir");
        for args in [
            &["/Users/dev/notes.txt"][..],
            &["."],
            &["file:///Users/dev/index.html"],
            &["-a", "Calculator"],
            &["-a", "Safari", "https://auth.example.com/authorize"],
            &["-b", "com.example.browser", "https://auth.example.com/a"],
            &["-e", "https://auth.example.com/a"],
            &["-R", "/Users/dev/notes.txt"],
            &["--args", "https://auth.example.com/authorize"],
            &["https://auth.example.com/a", "/Users/dev/notes.txt"],
            &[],
            &["-g"],
        ] {
            let input = os_args(args);
            let decided = decide(&input, Some(dir.path()), 10_000);
            assert_eq!(
                decided,
                OpenDecision {
                    exec_args: Some(input),
                    notices: Vec::new(),
                },
                "expected passthrough for {args:?}"
            );
        }
        assert!(
            !dir.path().join(STATE_FILE_NAME).exists(),
            "passthrough calls must not touch dedupe state"
        );
    }

    #[test]
    fn decide_adds_background_for_web_urls_including_dash_u_and_double_dash() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bare = decide(
            &os_args(&["https://auth.example.com/a?x=1"]),
            Some(dir.path()),
            1_000,
        );
        assert_eq!(
            bare.exec_args,
            Some(os_args(&["-g", "https://auth.example.com/a?x=1"]))
        );

        let via_u = decide(
            &os_args(&["-u", "https://auth.example.com/b?x=1"]),
            Some(dir.path()),
            1_000,
        );
        assert_eq!(
            via_u.exec_args,
            Some(os_args(&["-g", "-u", "https://auth.example.com/b?x=1"]))
        );

        let after_double_dash = decide(
            &os_args(&["--", "https://auth.example.com/c?x=1"]),
            Some(dir.path()),
            1_000,
        );
        assert_eq!(
            after_double_dash.exec_args,
            Some(os_args(&["-g", "--", "https://auth.example.com/c?x=1"]))
        );

        let wait_apps = decide(
            &os_args(&["--wait-apps", "https://auth.example.com/d?x=1"]),
            Some(dir.path()),
            1_000,
        );
        assert_eq!(
            wait_apps.exec_args,
            Some(os_args(&[
                "-g",
                "--wait-apps",
                "https://auth.example.com/d?x=1"
            ]))
        );
    }

    #[test]
    fn decide_preserves_existing_background_flag_including_bundled_short_flags() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bundled = decide(
            &os_args(&["-ng", "https://auth.example.com/bundled?1"]),
            Some(dir.path()),
            1_000,
        );
        assert_eq!(
            bundled.exec_args,
            Some(os_args(&["-ng", "https://auth.example.com/bundled?1"]))
        );

        let long_flag = decide(
            &os_args(&["--background", "https://auth.example.com/long?1"]),
            Some(dir.path()),
            1_000,
        );
        assert_eq!(
            long_flag.exec_args,
            Some(os_args(&[
                "--background",
                "https://auth.example.com/long?1"
            ]))
        );
    }

    #[test]
    fn decide_dedupes_within_window_and_allows_again_after_expiry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let window = Duration::from_secs(300);

        let first = decide_with_window(
            &os_args(&["https://auth.example.com/oauth?state=1"]),
            Some(dir.path()),
            10_000,
            window,
        );
        assert_eq!(
            first.exec_args,
            Some(os_args(&["-g", "https://auth.example.com/oauth?state=1"]))
        );
        assert!(first.notices.is_empty());

        let within = decide_with_window(
            &os_args(&["https://auth.example.com/oauth?state=2#frag"]),
            Some(dir.path()),
            15_000,
            window,
        );
        assert_eq!(within.exec_args, None);
        assert_eq!(
            within.notices,
            vec![
                "zerocode: skipped repeated open of https://auth.example.com/oauth (opened 5s ago)"
            ]
        );

        let after_expiry = decide_with_window(
            &os_args(&["https://auth.example.com/oauth?state=3"]),
            Some(dir.path()),
            10_000 + 300_000,
            window,
        );
        assert_eq!(
            after_expiry.exec_args,
            Some(os_args(&["-g", "https://auth.example.com/oauth?state=3"]))
        );
        assert!(after_expiry.notices.is_empty());
    }

    #[test]
    fn decide_recovers_from_backwards_clock_and_corrupt_state_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = dir.path().join(STATE_FILE_NAME);
        std::fs::write(
            &state,
            "not-a-number\thttps://auth.example.com/a\n999999999\thttps://auth.example.com/future\ngarbage-line\n",
        )
        .expect("write corrupt state");

        let decided = decide(
            &os_args(&["https://auth.example.com/future?retry=1"]),
            Some(dir.path()),
            5_000,
        );
        assert_eq!(
            decided.exec_args,
            Some(os_args(&["-g", "https://auth.example.com/future?retry=1"]))
        );
    }

    #[test]
    fn decide_caps_stored_entries_at_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        for idx in 0..(MAX_RECENT_URLS + 10) {
            let url = format!("https://auth.example.com/p/{idx}");
            let _ = decide(
                &os_args(&[&url]),
                Some(dir.path()),
                10_000 + u64::try_from(idx).unwrap_or(0),
            );
        }
        let stored = std::fs::read_to_string(dir.path().join(STATE_FILE_NAME)).expect("read state");
        assert_eq!(stored.lines().count(), MAX_RECENT_URLS);
    }

    #[test]
    fn decide_handles_multiple_urls_in_one_call_skipping_only_duplicates() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _ = decide(
            &os_args(&["https://auth.example.com/one?a=1"]),
            Some(dir.path()),
            1_000,
        );
        let mixed = decide(
            &os_args(&[
                "https://auth.example.com/one?a=2",
                "https://auth.example.com/two?b=1",
                "https://auth.example.com/two?b=2",
            ]),
            Some(dir.path()),
            4_000,
        );
        assert_eq!(
            mixed.exec_args,
            Some(os_args(&["-g", "https://auth.example.com/two?b=1"]))
        );
        assert_eq!(mixed.notices.len(), 2);
    }

    #[test]
    fn decide_still_adds_background_when_state_dir_is_missing() {
        let decided = decide(&os_args(&["https://auth.example.com/a?x=1"]), None, 1_000);
        assert_eq!(
            decided.exec_args,
            Some(os_args(&["-g", "https://auth.example.com/a?x=1"]))
        );
        assert!(decided.notices.is_empty());
    }

    #[test]
    fn open_shim_script_interpolates_single_source_constants_and_quotes_paths() {
        let script = open_shim_script(
            Path::new("/Users/dev/My Apps/zerocode-mirror"),
            Path::new("/Users/dev/fake's/open"),
        );
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("'/Users/dev/My Apps/zerocode-mirror'"));
        assert!(script.contains("'/Users/dev/fake'\\''s/open'"));
        for constant in [
            TEAM_PANE_VAR,
            LEADER_PANE,
            TEAM_ID_VAR,
            TEAM_TOKEN_FILE_VAR,
            TEAM_TOKEN_VAR,
            MARKER,
        ] {
            assert!(script.contains(constant), "missing {constant}:\n{script}");
        }
    }
}
