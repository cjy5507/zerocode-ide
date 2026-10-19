//! The window sends nothing to Google's Code Assist quota API.
//!
//! Until 2026-10-17 the status bar carried an Antigravity gauge that read
//! this machine's Google login — the subscription sign-in zo and the window
//! share — and asked `daily-cloudcode-pa.googleapis.com/v1internal` for the
//! plan's quota (`loadCodeAssist`, then `retrieveUserQuota`) on every poll.
//! A subscription login is the person's to spend in the vendor's own clients
//! alone (「구독 OAuth로 제품 호출 금지」, decided 2026-10-17 14:04), so the
//! road was removed rather than switched off (t-62187). This file keeps it
//! removed: no source file under the crate names the API, the backend
//! registers no `antigravity_usage`, the window asks none, and the bar has
//! no segment or settings row for it. The Google card in 설정 stays — it
//! shows and makes the login, and reads nothing but who is signed in.
//!
//! Antigravity the AGENT — `agy` in a pane, its catalog row, trust preset and
//! hook family — is not a gauge and is not what this file is about.

use std::path::{Path, PathBuf};

/// Every `.rs` file under the crate's `src/`, tests included — a test that
/// still spoke to the API would be a fixture waiting to be promoted.
fn every_shell_source() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut pending = vec![root];
    let mut found = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("a shell source directory") {
            let path = entry.expect("a shell source entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|why| panic!("{}: {why}", path.display()));
                found.push((path, text));
            }
        }
    }
    found.sort();
    found
}

/// The four spellings the gauge spoke the API by — the host, the API
/// version segment and its two methods. Written in full here so this test is
/// the one place under the crate they may appear.
const QUOTA_API_WORDS: [&str; 4] = [
    "v1internal",
    "daily-cloudcode-pa.googleapis.com",
    "loadCodeAssist",
    "retrieveUserQuota",
];

#[test]
fn no_shell_source_names_googles_code_assist_quota_api() {
    let sources = every_shell_source();
    assert!(
        sources.len() > 50,
        "the crate's source walk found {} files — this gate would then pass by reading nothing",
        sources.len()
    );
    let mut hits = Vec::new();
    for (path, text) in &sources {
        for (number, line) in text.lines().enumerate() {
            for word in QUOTA_API_WORDS {
                if line.contains(word) {
                    hits.push(format!("{}:{}: {word}", path.display(), number + 1));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "the window still names Google's quota API ({} lines):\n{}",
        hits.len(),
        hits.join("\n")
    );
}

/// The gauge is gone from both halves, not hidden: no command to ask, no
/// segment to paint, no settings row to switch, no words to translate.
#[test]
fn the_antigravity_gauge_has_no_command_segment_or_setting() {
    let backend = super::support::shipped_backend();
    assert!(
        !backend.contains("antigravity_usage"),
        "the backend still registers or defines an `antigravity_usage` command"
    );

    let window = super::support::window_source();
    for spelled in [
        "antigravity_usage",
        "sb-antigravity",
        "usage.antigravityTip",
        "settings.statusBar.antigravity",
        "usageProvider(\"antigravity\")",
        "id: \"antigravity\"",
    ] {
        assert!(
            !window.contains(spelled),
            "the window still carries the Antigravity gauge: {spelled}"
        );
    }

    let markup = include_str!("../../../../ui/index.html");
    for spelled in [
        "id=\"sb-antigravity\"",
        "data-status-bar-item=\"antigravity\"",
        "settings.statusBar.antigravity",
    ] {
        assert!(
            !markup.contains(spelled),
            "the markup still carries the Antigravity gauge: {spelled}"
        );
    }
}

/// The text from `opening` up to the first `closing` after it.
fn between<'a>(text: &'a str, opening: &str, closing: &str) -> &'a str {
    let start = text
        .find(opening)
        .unwrap_or_else(|| panic!("no `{opening}` in the source"));
    let rest = &text[start..];
    let end = rest
        .find(closing)
        .unwrap_or_else(|| panic!("`{opening}` never reaches `{closing}`"));
    &rest[..end]
}

/// The orchestration beat asks no Antigravity quota: the quota table the
/// coordinator reads, the table the window asks through, and the cache the
/// beat reads all leave the gauge out, so an agy worker at a quota wall reads
/// "unknown", as it does on main.
#[test]
fn the_orchestration_beat_and_its_ask_table_name_no_antigravity_gauge() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let core = std::fs::read_to_string(root.join("../zerocode-core/src/orchestration.rs"))
        .expect("the core orchestration source");
    let quota = between(&core, "const QUOTA_GAUGE: &[(&str, &str)] = &[", "];");
    assert!(
        !quota.contains("antigravity"),
        "the coordinator's quota table reads the Antigravity gauge:\n{quota}"
    );

    let usage = std::fs::read_to_string(root.join("src/cmd/usage.rs")).expect("the usage commands");
    let asks = between(&usage, "pub(crate) const USAGE_ASKS", "];");
    assert!(
        !asks.contains("antigravity"),
        "the beat's ask table asks the Antigravity gauge:\n{asks}"
    );

    let orchestration = std::fs::read_to_string(root.join("src/orchestration.rs"))
        .expect("the shell orchestration source");
    let cache = between(&orchestration, "fn cached_usage(", "_ => return None");
    assert!(
        !cache.contains("antigravity"),
        "the beat reads the Antigravity cache:\n{cache}"
    );
}
