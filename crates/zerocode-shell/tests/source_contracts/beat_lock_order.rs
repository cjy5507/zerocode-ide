//! Every test that takes the beat takes a window road first (t-19979).
//!
//! The window and the beat are two locks. A deadlock needs both halves of a
//! cycle: one test holds the beat and waits for a window, while another holds a
//! window and waits for the beat. `WalledClaude::stand_full` is the second half,
//! so a test that took the beat and then booted a window would wait on the
//! window while it held the beat. This contract reads every function in the
//! shell sources that calls `one_beat_at_a_time()`, and refuses one whose first
//! window road comes after its first beat call, or that takes the beat with no
//! window road at all.

use std::collections::BTreeSet;

use super::quiet_children::{code_only, shell_sources};
use super::support::strip_rust_comments;

/// The roads that take a window: a private one, a walled Claude, the shared one.
const WINDOW_ROADS: [&str; 3] = ["PrivateWindow::boot", "WalledClaude::stand", "the_window()"];

/// A function with a body: its name, and the bytes of its `{` and its `}`.
struct Body<'a> {
    name: &'a str,
    open: usize,
    close: usize,
}

/// What the order check reads from one source.
struct Order {
    /// Each function that takes the beat before its first window road, or takes it with none.
    refused: Vec<String>,
    /// The beat calls the walk reads; the definition is not one.
    calls: usize,
    /// How many of those stand inside a function body.
    placed: usize,
}

/// Whether the text before `at` ends in a letter, a digit or `_`. Then `at` is
/// the middle of a longer name, and `xthe_window()` is not `the_window()`.
fn follows_a_name(code: &str, at: usize) -> bool {
    code[..at]
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
}

/// Every place `needle` stands in `code` as a whole name.
fn stands(code: &str, needle: &str) -> Vec<usize> {
    code.match_indices(needle)
        .map(|(at, _)| at)
        .filter(|at| !follows_a_name(code, *at))
        .collect()
}

/// Every call of the beat in `code`. The definition, `fn one_beat_at_a_time(`, is not a call.
fn beat_calls(code: &str) -> Vec<usize> {
    stands(code, "one_beat_at_a_time(")
        .into_iter()
        .filter(|at| !code[..*at].ends_with("fn "))
        .collect()
}

/// Where the char literal that opens at `at` ends, or `None` when the quote opens
/// a lifetime. A brace inside `'{'` is a character, not a block.
fn char_literal_end(code: &str, at: usize) -> Option<usize> {
    let rest = &code[at + 1..];
    let inside = if rest.starts_with('\\') {
        // `'\n'`, `'\''`, `'\u{7b}'`: the glyph after the backslash belongs to the
        // escape, and the closing quote is the next one.
        rest.get(2..)?.find('\'').filter(|to| *to <= 10)? + 3
    } else {
        // `'{'`: one glyph and its closing quote. A lifetime has no quote there.
        let width = rest.chars().next()?.len_utf8();
        rest[width..].starts_with('\'').then_some(width + 1)?
    };
    Some(at + 1 + inside)
}

/// Where the body of a function whose name ends at `from` opens: its first `{`
/// outside the brackets of the signature, or `None` for a declaration ending in `;`.
fn body_opens(code: &str, from: usize) -> Option<usize> {
    let mut brackets = 0usize;
    for (step, byte) in code.as_bytes()[from..].iter().copied().enumerate() {
        match byte {
            b'(' | b'[' => brackets += 1,
            b')' | b']' => brackets = brackets.saturating_sub(1),
            b'{' if brackets == 0 => return Some(from + step),
            b';' if brackets == 0 => return None,
            _ => {}
        }
    }
    None
}

/// Where the block whose `{` is at `open` closes, stepping over char literals.
/// Strings and comments are already gone from `code`.
fn block_closes(code: &str, open: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    let mut depth = 0usize;
    let mut at = open;
    while at < bytes.len() {
        match bytes[at] {
            b'\'' => {
                if let Some(end) = char_literal_end(code, at) {
                    at = end;
                    continue;
                }
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// Every function in `code` that has a body. Comments and strings are gone, so a
/// `fn` in prose is not one, and no brace inside a string closes a body early.
fn fn_bodies(code: &str) -> Vec<Body<'_>> {
    code.match_indices("fn ")
        .filter(|(at, _)| !follows_a_name(code, *at))
        .filter_map(|(at, _)| {
            let name_at = at + "fn ".len();
            let name_len = code[name_at..]
                .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                .unwrap_or(code.len() - name_at);
            let open = body_opens(code, name_at + name_len)?;
            let close = block_closes(code, open)?;
            Some(Body {
                name: &code[name_at..name_at + name_len],
                open,
                close,
            })
        })
        .collect()
}

/// The innermost function whose body holds the byte at `at`, as its index.
fn owner(bodies: &[Body<'_>], at: usize) -> Option<usize> {
    bodies
        .iter()
        .enumerate()
        .filter(|(_, body)| body.open < at && at < body.close)
        .max_by_key(|(_, body)| body.open)
        .map(|(index, _)| index)
}

/// Reads one source in order: the beat calls and the window roads, each placed in
/// the function that holds it, and a function refused when its first beat call
/// has no window road before it in that same function.
fn order_of(source: &str) -> Order {
    let code = code_only(source);
    let bodies = fn_bodies(&code);
    let beats = beat_calls(&code);
    let mut events: Vec<(usize, bool)> = beats.iter().map(|at| (*at, true)).collect();
    for road in WINDOW_ROADS {
        events.extend(stands(&code, road).into_iter().map(|at| (at, false)));
    }
    events.sort_unstable();
    let mut window_taken = BTreeSet::new();
    let mut reported = BTreeSet::new();
    let mut refused = Vec::new();
    let mut placed = 0;
    for (at, is_beat) in events {
        let Some(index) = owner(&bodies, at) else {
            continue;
        };
        if !is_beat {
            window_taken.insert(index);
            continue;
        }
        placed += 1;
        if !window_taken.contains(&index) && reported.insert(index) {
            refused.push(bodies[index].name.to_string());
        }
    }
    Order {
        refused,
        calls: beats.len(),
        placed,
    }
}

/// The beat calls a source states outside its comment lines. A walk that reads
/// fewer has gone blind on some text, and then the order check cannot refuse
/// what it cannot see.
fn stated_calls(source: &str) -> usize {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .map(|line| {
            line.matches("one_beat_at_a_time(").count()
                - line.matches("fn one_beat_at_a_time(").count()
        })
        .sum()
}

/// A test that boots its window and then takes the beat: the order every test keeps.
const WINDOW_FIRST: &str = r#"
#[test]
fn an_idle_beat_walks_the_ledger() {
    let (_window, _store) = PrivateWindow::boot_seeded(a_ledger());
    let _beat = one_beat_at_a_time();
}
"#;

/// The order the v1.1.46 gate deadlocked on: the beat first, then the window.
const BEAT_FIRST: &str = r#"
#[test]
fn an_idle_beat_walks_the_ledger() {
    let _beat = one_beat_at_a_time();
    let (_window, _store) = PrivateWindow::boot_seeded(a_ledger());
}
"#;

/// The beat named in a comment and in a string is no call.
const PROSE: &str = r#"
// one_beat_at_a_time() comes before PrivateWindow::boot() here, in prose only.
fn a_note() -> &'static str {
    "the_window() and one_beat_at_a_time() are words in a string"
}
"#;

/// A raw string with a quote and a `/*` before the window: the walk must read on past it.
const RAW_QUOTE_FIRST: &str = r##"
#[test]
fn a_raw_string_with_a_quote_before_the_window() {
    let json = r#"{"a":"/*"}"#;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
}
"##;

#[test]
fn a_test_that_takes_the_beat_before_its_window_is_refused() {
    let order = order_of(BEAT_FIRST);
    assert_eq!(order.refused, ["an_idle_beat_walks_the_ledger"]);
    assert_eq!((order.calls, order.placed), (1, 1));
}

#[test]
fn a_test_that_takes_the_window_first_is_accepted() {
    let order = order_of(WINDOW_FIRST);
    assert!(order.refused.is_empty(), "{:?}", order.refused);
    assert_eq!((order.calls, order.placed), (1, 1));
}

#[test]
fn a_beat_named_in_a_comment_or_a_string_is_no_call() {
    let order = order_of(PROSE);
    assert!(order.refused.is_empty(), "{:?}", order.refused);
    assert_eq!(order.calls, 0);
}

#[test]
fn a_raw_string_with_a_quote_hides_nothing_from_the_order_check() {
    let order = order_of(RAW_QUOTE_FIRST);
    assert!(order.refused.is_empty(), "{:?}", order.refused);
    assert_eq!((order.calls, order.placed), (1, 1));
}

#[test]
fn the_walker_reads_a_raw_string_whole() {
    let source =
        r##"let json = r#"{"a":"/*"}"#; let bytes = br#"/* ""#; let beat = one_beat_at_a_time();"##;
    assert_eq!(strip_rust_comments(source), source);
}

/// Every test that takes the beat takes a window road before it, in every shell source.
#[test]
fn every_test_takes_the_window_before_the_beat() {
    let mut refused = Vec::new();
    let mut calls = 0;
    for (name, source) in shell_sources() {
        if !source.contains("one_beat_at_a_time") {
            continue;
        }
        let order = order_of(&source);
        let stated = stated_calls(&source);
        assert_eq!(
            order.calls, stated,
            "{name}: the walk reads {} beat calls, but the source states {stated} outside its comment lines",
            order.calls
        );
        assert_eq!(
            order.placed, order.calls,
            "{name}: a beat call stands outside every function body"
        );
        refused.extend(
            order
                .refused
                .iter()
                .map(|function| format!("{name}: {function}")),
        );
        calls += order.calls;
    }
    assert!(
        calls > 0,
        "the walk found no beat call in the shell sources"
    );
    assert!(
        refused.is_empty(),
        "these functions take the beat before any window road, the order that deadlocked the shell suite (t-19979):\n  {}",
        refused.join("\n  ")
    );
}
