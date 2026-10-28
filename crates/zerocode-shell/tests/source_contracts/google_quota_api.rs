//! The Antigravity gauge reads Google's Code Assist quota API, and nothing else does.
//!
//! The status bar's Antigravity gauge reads this machine's Google login — the
//! subscription sign-in zo and the window share — with two read-only calls on
//! `daily-cloudcode-pa.googleapis.com/v1internal`: `loadCodeAssist`, then
//! `retrieveUserQuota`. The person restored that one gauge on 2026-10-19
//! (「이것만 예외로 되살리기」), first with ZeroCode's own headers (「정직한 신원으로
//! 먼저」); Google refused those, and on 2026-10-28 the person moved the two reads
//! to the identity zo sends for the same login (「zo와 같은 신원이면 될거같은데」).
//! The rest of the subscription login stays the vendor's own clients' (「구독 OAuth로
//! 제품 호출 금지」, decided 2026-10-17 14:04). This file keeps that scope fixed: the
//! host, the version segment and the two methods are named only in
//! `usage_antigravity.rs`, the bearer road has one caller, the scan has one entry
//! point, and the orchestration beat and its ask table name no Antigravity gauge.
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

/// The four spellings the gauge speaks the API by — the host, the API
/// version segment and its two methods. Written in full here so this test is
/// the one place under the crate they may appear.
const QUOTA_API_WORDS: [&str; 4] = [
    "v1internal",
    "daily-cloudcode-pa.googleapis.com",
    "loadCodeAssist",
    "retrieveUserQuota",
];

/// The file names of the sources that contain `spelling`.
fn files_containing(sources: &[(PathBuf, String)], spelling: &str) -> Vec<String> {
    sources
        .iter()
        .filter(|(_, text)| text.contains(spelling))
        .filter_map(|(path, _)| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
        })
        .collect()
}

/// The gauge's own module is the one place under the crate that names the API.
#[test]
fn only_the_gauge_names_googles_code_assist_quota_api() {
    let sources = every_shell_source();
    assert!(
        sources.len() > 50,
        "the crate's source walk found {} files — this gate would then pass by reading nothing",
        sources.len()
    );
    let mut hits = Vec::new();
    for (path, text) in &sources {
        if path.ends_with("usage_antigravity.rs") {
            continue;
        }
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
        "a source other than the gauge names Google's quota API ({} lines):\n{}",
        hits.len(),
        hits.join("\n")
    );
}

/// The gauge makes exactly two reads, and each goes through the one bearer road
/// from the one scan that the status bar's refresh reaches.
#[test]
fn the_gauge_makes_exactly_two_reads_on_the_one_bearer_road() {
    let sources = every_shell_source();
    let gauge = sources
        .iter()
        .find(|(path, _)| path.ends_with("usage_antigravity.rs"))
        .map(|(_, text)| text.as_str())
        .expect("the gauge module");
    assert_eq!(
        gauge.matches("v1internal:").count(),
        2,
        "the gauge names a Code Assist method other than its two reads"
    );
    assert!(
        gauge.contains("v1internal:loadCodeAssist")
            && gauge.contains("v1internal:retrieveUserQuota"),
        "the gauge's two reads are not loadCodeAssist and retrieveUserQuota"
    );
    assert_eq!(
        files_containing(&sources, "usage_http::post_json_bearer("),
        ["usage_antigravity.rs"],
        "the bearer road has a caller other than the gauge"
    );
    assert_eq!(
        files_containing(&sources, "usage_antigravity::scan("),
        ["usage_runtime.rs"],
        "the gauge's scan is reached from somewhere other than the status bar's refresh"
    );
}

/// When Google's answer names no project, the gauge reads the one zo resolved
/// for the same login (t-72144) — out of zo's credentials file, by zo's own
/// spelling: the record's key and fields, the grant's sixteen hex digits of
/// the refresh token's SHA-256, and the two variables the configured project
/// is read from, in zo's order. A rename on either side is a record nobody
/// reads, so both sides are pinned here.
#[test]
fn the_gauge_reads_zos_project_record_by_zos_own_spelling() {
    let zo_store = include_str!("../../../../zo-ide/crates/api/src/oauth_store.rs");
    for spelled in [
        "const GOOGLE_CODE_ASSIST_PROJECT_KEY: &str = \"google_code_assist_project\";",
        "const CONFIGURED_PROJECT_FIELD: &str = \"configured_project\";",
        "const GRANT_FINGERPRINT_LEN: usize = 16;",
        "format!(\"{:x}\", Sha256::digest(refresh_token.as_bytes()))",
        "\"grant\": grant,",
        "\"project\": project,",
    ] {
        assert!(
            zo_store.contains(spelled),
            "zo's project record no longer reads `{spelled}`"
        );
    }
    let zo_client =
        include_str!("../../../../zo-ide/crates/api/src/providers/gemini_code_assist.rs");
    let configured = between(zo_client, "fn google_cloud_project(", "Ok(project)");
    let first = configured
        .find("std::env::var(\"GOOGLE_CLOUD_PROJECT\")")
        .expect("zo reads GOOGLE_CLOUD_PROJECT");
    let second = configured
        .find("std::env::var(\"GOOGLE_CLOUD_PROJECT_ID\")")
        .expect("zo reads GOOGLE_CLOUD_PROJECT_ID");
    assert!(
        first < second,
        "zo reads its two variables in another order"
    );

    let window = include_str!("../../src/google_login.rs");
    for spelled in [
        "const PROJECT_KEY: &str = \"google_code_assist_project\";",
        "const CONFIGURED_PROJECT_FIELD: &str = \"configured_project\";",
        "const GRANT_FINGERPRINT_LEN: usize = 16;",
        "[\"GOOGLE_CLOUD_PROJECT\", \"GOOGLE_CLOUD_PROJECT_ID\"]",
        ".get(\"grant\")",
        ".get(\"project\")",
    ] {
        assert!(
            window.contains(spelled),
            "the window does not read zo's project record as `{spelled}`"
        );
    }
}

/// The gauge is on the bar and in the settings again: the backend registers its
/// command, the window draws its segment and lists its row, and the markup
/// carries both.
#[test]
fn the_antigravity_gauge_has_its_command_segment_and_setting() {
    let backend = super::support::shipped_backend();
    assert!(
        backend.contains("\n            antigravity_usage,\n"),
        "the backend does not register the `antigravity_usage` command"
    );

    let window = super::support::window_source();
    for spelled in [
        "command: \"antigravity_usage\",",
        "{ id: \"antigravity\" },",
    ] {
        assert!(
            window.contains(spelled),
            "the window lost the Antigravity gauge: {spelled}"
        );
    }

    let markup = include_str!("../../../../ui/index.html");
    for spelled in [
        "id=\"sb-antigravity\"",
        "data-status-bar-item=\"antigravity\"",
        "settings.statusBar.antigravity",
    ] {
        assert!(
            markup.contains(spelled),
            "the markup lost the Antigravity gauge: {spelled}"
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
