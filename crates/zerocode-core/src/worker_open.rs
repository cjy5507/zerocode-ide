//! Guard for macOS `open` calls from ledger-summoned worker panes.
//!
//! When an unauthenticated MCP server (such as `mcp-remote`) runs inside a
//! worker pane and invokes `open <url>`, macOS activates the default browser
//! and steals focus from whatever the person is using. This module owns:
//!
//! - The POSIX `open` shim installed into a worker-only subdirectory under the
//!   window's shim folder on macOS ([`worker_shim_dir`]). Only summoned worker
//!   panes put that subdirectory on `PATH`, so human panes and leader panes
//!   (`%1`) never hit a shim and keep invoking the system `open` directly.
//! - Parsing macOS `open` arguments so only plain `http(s)` web-address opens
//!   receive `-g` (background). Files, `-a app` calls, already-backgrounded
//!   calls, and other invocations pass through unchanged.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Environment marker the worker-only `open` shim sets when handing off an
/// `open` invocation to `zerocode-mirror`. Removed before `exec`ing real `open`.
pub const MARKER: &str = "ZEROCODE_OPEN_GUARD";

/// File name of the shim laid into [`worker_shim_dir`] on macOS.
pub const SHIM_NAME: &str = "open";

/// Subdirectory under the window's shared shim folder that holds worker-only
/// shims (`open` on macOS). Only summoned worker panes prepend this directory
/// to `PATH`.
pub const WORKER_SHIM_SUBDIR: &str = "worker-open";

/// Production macOS system `open` path; tests inject a fake executable instead.
pub const SYSTEM_OPEN_PATH: &str = "/usr/bin/open";

/// Path to the worker-only shim subdirectory inside the window's shared shim
/// directory.
#[must_use]
pub fn worker_shim_dir(shared_shims: &Path) -> PathBuf {
    shared_shims.join(WORKER_SHIM_SUBDIR)
}

/// Build the POSIX `open` shim script installed in [`worker_shim_dir`] on macOS.
///
/// Worker-pane selection is done by `PATH` alone (only worker panes include
/// [`worker_shim_dir`]), so this script reads no environment variables — it
/// continues to work even when an agent (such as Codex) clears the environment
/// before launching an MCP server and forwards only a small allowlist that
/// includes `PATH`.
#[must_use]
pub fn open_shim_script(mirror: &Path, real_open: &Path) -> String {
    let marker = MARKER;
    let mirror = sh_single_quote(&mirror.to_string_lossy());
    let real = sh_single_quote(&real_open.to_string_lossy());
    format!(
        r#"#!/bin/sh
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

fn is_web_url(raw: &str) -> bool {
    url::Url::parse(raw).ok().is_some_and(|parsed| {
        matches!(parsed.scheme(), "http" | "https") && parsed.host_str().is_some()
    })
}

/// Decide the argument vector to pass to real `open` for a worker pane's
/// invocation.
///
/// Pure `http(s)` web-address opens without an existing background flag get
/// `-g` prepended. Files, explicit-app calls (`-a`/`-b`), already-backgrounded
/// calls (`-g`/`--background`), and unrecognized flags pass through unchanged.
#[must_use]
pub fn decide(args: &[OsString]) -> Vec<OsString> {
    match parse_open_args(args) {
        ParsedOpen::WebUrls {
            has_background: false,
        } => {
            let mut out = Vec::with_capacity(args.len() + 1);
            out.push(OsString::from("-g"));
            out.extend(args.iter().cloned());
            out
        }
        ParsedOpen::Passthrough
        | ParsedOpen::WebUrls {
            has_background: true,
        } => args.to_vec(),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ParsedOpen {
    Passthrough,
    WebUrls { has_background: bool },
}

fn parse_open_args(args: &[OsString]) -> ParsedOpen {
    let mut has_background = false;
    let mut has_url = false;
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
                if !is_web_url(next) {
                    return ParsedOpen::Passthrough;
                }
                has_url = true;
                index += 2;
                continue;
            }
            if let Some(raw_url) = arg.strip_prefix("--url=") {
                if !is_web_url(raw_url) {
                    return ParsedOpen::Passthrough;
                }
                has_url = true;
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

        if !is_web_url(arg) {
            return ParsedOpen::Passthrough;
        }
        has_url = true;
        index += 1;
    }

    if !has_url {
        return ParsedOpen::Passthrough;
    }

    ParsedOpen::WebUrls { has_background }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_teams::{
        LEADER_PANE, TEAM_ID_VAR, TEAM_PANE_VAR, TEAM_TOKEN_FILE_VAR, TEAM_TOKEN_VAR,
    };

    fn os_args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn worker_shim_dir_nests_under_shared_shims() {
        let shared = Path::new("/tmp/zerocode-shims-42");
        assert_eq!(worker_shim_dir(shared), shared.join(WORKER_SHIM_SUBDIR));
    }

    #[test]
    fn decide_passes_through_non_url_explicit_app_and_args_tail() {
        for args in [
            &["/Users/dev/notes.txt"][..],
            &["./notes.txt"],
            &["."],
            &["file:///Users/dev/index.html"],
            &["mailto:dev@example.com"],
            &["custom-scheme://auth/callback"],
            &["https://"],
            &[""],
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
            assert_eq!(decide(&input), input, "expected passthrough for {args:?}");
        }
    }

    #[test]
    fn decide_adds_background_for_web_urls_including_dash_u_and_double_dash() {
        assert_eq!(
            decide(&os_args(&["https://auth.example.com/a?x=1"])),
            os_args(&["-g", "https://auth.example.com/a?x=1"])
        );
        assert_eq!(
            decide(&os_args(&["http://127.0.0.1:8080/callback?code=xyz"])),
            os_args(&["-g", "http://127.0.0.1:8080/callback?code=xyz"])
        );
        assert_eq!(
            decide(&os_args(&["-u", "https://auth.example.com/b?x=1"])),
            os_args(&["-g", "-u", "https://auth.example.com/b?x=1"])
        );
        assert_eq!(
            decide(&os_args(&["--url=https://auth.example.com/b?x=2"])),
            os_args(&["-g", "--url=https://auth.example.com/b?x=2"])
        );
        assert_eq!(
            decide(&os_args(&["--", "https://auth.example.com/c?x=1"])),
            os_args(&["-g", "--", "https://auth.example.com/c?x=1"])
        );
        assert_eq!(
            decide(&os_args(&["--wait-apps", "https://auth.example.com/d?x=1"])),
            os_args(&["-g", "--wait-apps", "https://auth.example.com/d?x=1"])
        );
        assert_eq!(
            decide(&os_args(&[
                "https://auth.example.com/one?a=1",
                "https://auth.example.com/one?a=2",
            ])),
            os_args(&[
                "-g",
                "https://auth.example.com/one?a=1",
                "https://auth.example.com/one?a=2",
            ])
        );
    }

    #[test]
    fn decide_preserves_existing_background_flag_including_bundled_short_flags() {
        assert_eq!(
            decide(&os_args(&["-g", "https://auth.example.com/plain?1"])),
            os_args(&["-g", "https://auth.example.com/plain?1"])
        );
        assert_eq!(
            decide(&os_args(&["-ng", "https://auth.example.com/bundled?1"])),
            os_args(&["-ng", "https://auth.example.com/bundled?1"])
        );
        assert_eq!(
            decide(&os_args(&[
                "--background",
                "https://auth.example.com/long?1"
            ])),
            os_args(&["--background", "https://auth.example.com/long?1"])
        );
    }

    #[test]
    fn open_shim_script_quotes_paths_and_reads_no_team_env() {
        let script = open_shim_script(
            Path::new("/Users/dev/My Apps/zerocode-mirror"),
            Path::new("/Users/dev/fake's/open"),
        );
        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("'/Users/dev/My Apps/zerocode-mirror'"));
        assert!(script.contains("'/Users/dev/fake'\\''s/open'"));
        assert!(script.contains(&format!("{MARKER}=1")), "{script}");
        for forbidden in [
            TEAM_PANE_VAR,
            LEADER_PANE,
            TEAM_ID_VAR,
            TEAM_TOKEN_FILE_VAR,
            TEAM_TOKEN_VAR,
        ] {
            assert!(
                !script.contains(forbidden),
                "unexpected team env check `{forbidden}`:\n{script}"
            );
        }
    }
}
