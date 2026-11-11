//! What the session vault's walk costs on a machine's own stores, and what
//! the list it gives looks like — in counts, sizes and times, never a line of
//! anybody's conversation.
//!
//! The measure behind the vault's index ([`zerocode_core::vault_index`],
//! t-103659). It reads the stores the way the window does — the person's home
//! for every agent but the one the window runs under its own data, and that
//! data folder for that one — three ways:
//!
//! 1. a first scan, through an empty index: every session file is opened;
//! 2. the scan again, through the index the first one left: a file that has
//!    not moved is not opened;
//! 3. the list from the index alone, written down and read back: what the
//!    panel opens on after the window starts.
//!
//! Then what a query costs over the kept list, and how the cards read: how
//! many are named, placed and spoken in.
//!
//! ```text
//! cargo run --release -p zerocode-core --example vault_scan_cost -- \
//!   ~ "<the window's config root>" [rounds]
//! ```
//!
//! Read-only: every store is opened for reading and nothing is written but
//! the report this prints.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use zerocode_core::vault::{
    self, Hidden, HiddenCount, Home, Limits, Scan, VaultQuery, VaultSession,
};
use zerocode_core::vault_index::Index;

/// The agent the window keeps under its own data (`vault_walk` in the shell).
const APP_STORE_AGENT: &str = "claude";

/// How a worker's briefing opens: a card named by it is named by nothing.
const BRIEFING_HEAD: &str = "You are a worker in this window";

fn ms(took: Duration) -> f64 {
    (took.as_secs_f64() * 10_000.0).round() / 10.0
}

fn timed<T>(run: impl FnOnce() -> T) -> (T, f64) {
    let at = Instant::now();
    let out = run();
    (out, ms(at.elapsed()))
}

/// One scan of both homes, as the window's walk asks for it.
fn scan(home: &Path, app_home: &Path, index: &mut Index) -> Scan {
    vault::scan_homes(
        &[
            Home {
                dir: home,
                reads: |source| source.slug != APP_STORE_AGENT,
            },
            Home {
                dir: app_home,
                reads: |source| source.slug == APP_STORE_AGENT,
            },
        ],
        Limits::DEFAULT.absolute_max,
        &vault::env_home,
        index,
    )
}

fn count_of(found: &Scan) -> serde_json::Value {
    serde_json::json!({
        "folders_opened": found.count.dirs,
        "session_files": found.count.files,
        "files_opened": found.count.read,
        "files_from_the_index": found.count.kept,
        "sessions": found.sessions.len(),
    })
}

/// The tag a line opens with (`<name`), when it opens with one.
fn leading_tag(text: &str) -> Option<String> {
    let rest = text.trim_start().strip_prefix('<')?;
    let name: String = rest
        .chars()
        .take_while(|one| one.is_ascii_alphanumeric() || matches!(one, '-' | '_'))
        .collect();
    (!name.is_empty()).then_some(name)
}

/// How the cards read, by agent — counts only.
fn shapes(sessions: &[VaultSession], app_home: &Path, temporary: &[PathBuf]) -> serde_json::Value {
    let mut per_agent: BTreeMap<&str, BTreeMap<&'static str, usize>> = BTreeMap::new();
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();
    let mut temp_runs: BTreeMap<String, usize> = BTreeMap::new();
    for session in sessions {
        let row = per_agent.entry(session.agent.as_str()).or_default();
        let mut count = |what: &'static str, holds: bool| {
            *row.entry(what).or_default() += usize::from(holds);
        };
        count("sessions", true);
        count("no_folder", session.cwd.is_none());
        count("nobody_spoke", !session.resumable());
        count("no_name", session.title.is_empty());
        count("named_by_its_id", session.title == session.session_id);
        count(
            "named_by_the_briefing",
            session.title.starts_with(BRIEFING_HEAD),
        );
        count("with_helpers", !session.children.is_empty());
        count("only_the_window_spoke", session.window_made);
        // Read by the head alone, and that head held no turn or only the
        // window's: listed, where a file read whole would be hidden.
        count(
            "head_only_and_nobody_spoke",
            session.cut_short && !session.resumable(),
        );
        count(
            "head_only_and_only_the_window_spoke",
            session.cut_short && session.window_made,
        );
        for line in &session.preview {
            if let Some(tag) = leading_tag(&line.text) {
                *tags.entry(format!("{}:{tag}", line.role)).or_default() += 1;
            }
        }
        // Where a run in a temporary folder was stored, and whether the
        // window keeps that store: what a measurement or a test leaves in
        // the list.
        if session.hidden_as(temporary) == Some(Hidden::Measured) {
            let store = if Path::new(&session.file_path).starts_with(app_home) {
                "the window's own store"
            } else {
                "the person's home"
            };
            *temp_runs
                .entry(format!("{} in {store}", session.agent))
                .or_default() += 1;
        }
    }
    serde_json::json!({
        "per_agent": per_agent,
        "preview_line_opens_with_a_tag": tags,
        "ran_in_a_temporary_folder": temp_runs,
    })
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(home), Some(app_home)) = (args.next(), args.next()) else {
        eprintln!("usage: vault_scan_cost <home> <the window's config root> [rounds]");
        std::process::exit(2);
    };
    let (home, app_home) = (PathBuf::from(home), PathBuf::from(app_home));
    let rounds: usize = args.next().and_then(|said| said.parse().ok()).unwrap_or(3);
    let mut report = serde_json::Map::new();

    // 1. Every file opened.
    let mut index = Index::default();
    let (first, first_ms) = timed(|| scan(&home, &app_home, &mut index));
    report.insert(
        "first_scan".into(),
        serde_json::json!({ "ms": first_ms, "count": count_of(&first) }),
    );

    // 2. Nothing moved (but the conversation this run may be part of).
    let mut again = Vec::new();
    let mut second = None;
    for _ in 0..rounds {
        let (found, took) = timed(|| scan(&home, &app_home, &mut index));
        again.push(took);
        second = Some(found);
    }
    if let Some(found) = &second {
        report.insert(
            "scan_again".into(),
            serde_json::json!({ "ms": again, "count": count_of(found) }),
        );
    }

    // 3. The list from what is written down.
    let written = serde_json::to_vec(&index).unwrap_or_default();
    let mut open_ms = Vec::new();
    let mut opened = 0;
    for _ in 0..rounds {
        let (sessions, took) = timed(|| {
            let read: Index = serde_json::from_slice(&written).unwrap_or_default();
            vault::sessions_of(&read, Limits::DEFAULT.absolute_max).0
        });
        open_ms.push(took);
        opened = sessions.len();
    }
    report.insert(
        "open_from_the_written_index".into(),
        serde_json::json!({ "ms": open_ms, "bytes": written.len(), "files_kept": index.len(), "sessions": opened }),
    );

    // What a query costs over the kept list.
    let sessions = second.map_or(first.sessions, |found| found.sessions);
    let temporary = zerocode_core::zo_project::temporary_roots();
    let mut views = serde_json::Map::new();
    for (name, query, limit) in [
        ("default_200", "", Some(200)),
        ("all", "", Some(0)),
        ("one_letter_200", "a", Some(200)),
    ] {
        let asked = VaultQuery {
            query: query.to_string(),
            limit,
            hide_empty: true,
            ..VaultQuery::default()
        };
        let mut took_all = Vec::new();
        let mut wire = 0;
        let mut seen = (0, 0, HiddenCount::default());
        for _ in 0..rounds {
            let (view, took) =
                timed(|| vault::view_of(&sessions, &asked, Vec::new(), false, &[], &temporary));
            took_all.push(took);
            wire = serde_json::to_vec(&view).map_or(0, |bytes| bytes.len());
            seen = (view.shown, view.matched, view.hidden);
        }
        views.insert(
            name.into(),
            serde_json::json!({
                "ms": took_all, "wire_bytes": wire,
                "shown": seen.0, "matched": seen.1, "hidden": seen.2,
            }),
        );
    }
    report.insert("views".into(), views.into());
    report.insert("cards".into(), shapes(&sessions, &app_home, &temporary));

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::Value::Object(report)).unwrap_or_default()
    );
}
