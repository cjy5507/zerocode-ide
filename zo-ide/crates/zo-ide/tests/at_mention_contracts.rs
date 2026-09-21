//! The source contracts of the `@` popup and its search engine: the row cap
//! and the engine's options are named constants that carry Codex 0.155.1's
//! names and values, and every place that needs them reads the name — a
//! literal `8`, `20` or `2` in those seams is the drift this gate catches.

use std::path::Path;

fn source(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

/// Source lines that are code — doc comments and comments say what they like.
fn code_lines(source: &str) -> impl Iterator<Item = &str> {
    source
        .lines()
        .map(str::trim_start)
        .filter(|line| !line.starts_with("//"))
}

#[test]
fn the_popup_row_cap_is_one_named_constant_read_by_the_mention_popup() {
    let view = source("src/tui/view.rs");
    assert!(
        view.contains("pub const MAX_POPUP_ROWS: usize = 8;"),
        "view.rs names codex's MAX_POPUP_ROWS with its value"
    );
    let mention = source("src/tui/mention.rs");
    assert!(mention.contains("use super::view::MAX_POPUP_ROWS;"));
    for line in code_lines(&mention) {
        assert!(
            !line.contains("take(8)") && !line.contains("clamp(1, 8)") && !line.contains("min(8)"),
            "the mention popup spells its cap by name, not as 8: {line}"
        );
    }
    assert!(mention.contains("rows.len().clamp(1, MAX_POPUP_ROWS)"), "codex calculate_required_height");
}

#[test]
fn the_engine_options_are_named_constants_with_codex_values() {
    let engine = source("../runtime/src/file_search.rs");
    for needle in [
        "pub const DEFAULT_LIMIT: NonZero<usize> = NonZero::new(20).unwrap();",
        "pub const DEFAULT_THREADS: NonZero<usize> = NonZero::new(2).unwrap();",
        "pub const TICK_TIMEOUT_MS: u64 = 10;",
        "pub const CHECK_INTERVAL: usize = 1024;",
        "pub const RERANK_WINDOW: usize = 8;",
        "pub const TOUCHED_BOOST: u32 = 24;",
    ] {
        assert!(engine.contains(needle), "{needle} is the engine's named constant");
    }
    let default_impl = engine
        .split("impl Default for FileSearchOptions {")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("the Default impl");
    assert!(default_impl.contains("limit: DEFAULT_LIMIT"));
    assert!(default_impl.contains("threads: DEFAULT_THREADS"));
    assert!(!default_impl.contains("NonZero::new("), "the defaults read the constants: {default_impl}");
    for needle in ["require_git(true)", ".hidden(false)", ".follow_links(true)", "CaseMatching::Ignore", "Normalization::Smart", "Config::DEFAULT.match_paths()"] {
        assert!(engine.contains(needle), "codex's walker and matcher settings: {needle}");
    }
}

#[test]
fn the_popup_words_are_constants_and_the_manager_asks_for_highlight_and_the_boost() {
    let mention = source("src/tui/mention.rs");
    for constant in ["LOADING", "NO_MATCHES", "MODE_RESULTS", "MODE_FILES", "MODE_TOOLS", "TAG_FILE", "TAG_DIRECTORY", "TAG_SKILL", "TAG_PAGE", "TAG_WIDTH"] {
        assert!(mention.contains(&format!("pub const {constant}:")), "{constant} is named");
    }
    let literal_uses: Vec<&str> = code_lines(&mention)
        .filter(|line| !line.starts_with("pub const"))
        .filter(|line| line.contains("\"loading...\"") || line.contains("\"no matches\"") || line.contains("\"All Results\"") || line.contains("\"Plugin\""))
        .filter(|line| !line.contains("assert"))
        .collect();
    assert!(literal_uses.is_empty(), "popup words are read by name: {literal_uses:?}");
    let manager = source("src/session/file_search.rs");
    assert!(manager.contains("compute_indices: true"), "codex asks for indices for highlighting");
    assert!(manager.contains("boost: Some("), "zo's rerank rides every session");
    assert!(manager.contains("SecondBrain::from_env()"), "the vault comes from the environment, never a literal path");
    assert!(!manager.contains("/Users/"), "no vault path is spelled in the manager");
}
