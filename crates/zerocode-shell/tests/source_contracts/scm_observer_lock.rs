//! A tick of the SCM observer never holds the observer while `gh` runs
//! (t-47457).
//!
//! The Checks pane's health read and a person's retry take the same lock a
//! tick takes. A tick used to take it for its whole run while it asked GitHub
//! up to `gh_calls_max` (32) times of `gh_call_ms` (10 s) each, so the read
//! could wait 320 s. Every `gh` call a tick makes spends its `Budget`, so this
//! contract reads `src/scm_observer.rs` for four things: no method of
//! `Observer` is handed the budget; a function that spends the budget holds
//! the observer only as `hold(…)`'s temporary, for one statement that spends
//! nothing and keeps no reference into it; a function that spends the budget
//! and never calls `hold` does not reach the observer at all; and the
//! observer is locked inside `hold` alone.

use super::beat_lock_order::{block_closes, fn_bodies, owner, stands};
use super::quiet_children::code_only;

const OBSERVER: &str = include_str!("../../src/scm_observer.rs");

/// The shipped part of a source: what comes before its tests.
fn shipped(source: &str) -> &str {
    source
        .split_once("#[cfg(test)]")
        .map_or(source, |(before, _)| before)
}

/// Whether `word` stands in `text` as a whole name.
fn names(text: &str, word: &str) -> bool {
    stands(text, word).into_iter().any(|at| {
        !text[at + word.len()..].starts_with(|ch: char| ch.is_alphanumeric() || ch == '_')
    })
}

/// Whether `text` spends a tick's `gh` budget.
fn spends(text: &str) -> bool {
    names(text, "budget") || names(text, "Budget")
}

/// The statement the byte at `at` stands in: from the `;`, `{` or `}` before
/// it to the `;` that ends it at its own depth, or to the `}` that closes its
/// block when it is the block's last expression.
fn statement_at(text: &str, at: usize) -> &str {
    let start = text[..at].rfind([';', '{', '}']).map_or(0, |to| to + 1);
    let mut depth = 0usize;
    for (step, byte) in text.as_bytes()[start..].iter().copied().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth == 0 => return &text[start..start + step],
            b')' | b']' | b'}' => depth -= 1,
            b';' if depth == 0 => return &text[start..start + step],
            _ => {}
        }
    }
    &text[start..]
}

/// Whether the `hold(` at `at` is used where it stands: its call is followed
/// by `.`, so the guard is a temporary that goes with its statement.
fn held_in_place(text: &str, at: usize) -> bool {
    let call = at + "hold".len();
    let mut depth = 0usize;
    for (step, byte) in text.as_bytes()[call..].iter().copied().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' if depth == 1 => {
                return text[call + step + 1..].trim_start().starts_with('.');
            }
            b')' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// What the contract refuses in one source, a line each.
fn refusals(source: &str) -> Vec<String> {
    let code = code_only(shipped(source));
    let bodies = fn_bodies(&code);
    let mut refused = Vec::new();
    for at in stands(&code, "impl Observer {") {
        let open = at + "impl Observer ".len();
        let close = block_closes(&code, open).unwrap_or(code.len());
        for method in bodies
            .iter()
            .filter(|body| open < body.at && body.close < close)
        {
            if spends(&code[method.at..=method.close]) {
                refused.push(format!("Observer::{} is handed the gh budget", method.name));
            }
        }
    }
    for body in &bodies {
        let text = &code[body.at..=body.close];
        if !spends(text) {
            continue;
        }
        let holds = stands(text, "hold(");
        if holds.is_empty() {
            if names(text, "Observer") || text.contains("remembered(") || text.contains(".lock(") {
                refused.push(format!(
                    "{} spends the gh budget and reaches the observer without hold",
                    body.name
                ));
            }
            continue;
        }
        for at in holds {
            let statement = statement_at(text, at);
            if !held_in_place(text, at) {
                refused.push(format!(
                    "{} keeps the observer held past its statement: `{}`",
                    body.name,
                    statement.trim()
                ));
            } else if spends(statement) {
                refused.push(format!(
                    "{} spends the gh budget while it holds the observer: `{}`",
                    body.name,
                    statement.trim()
                ));
            } else if statement.trim_start().starts_with("let ")
                && (statement.contains("&hold(") || statement.contains("&mut hold("))
            {
                refused.push(format!(
                    "{} keeps a reference into the held observer: `{}`",
                    body.name,
                    statement.trim()
                ));
            }
        }
    }
    for (at, _) in code.match_indices(".lock()") {
        let holder = owner(&bodies, at).map(|index| bodies[index].name);
        if holder != Some("hold") {
            refused.push(format!(
                "{} locks the observer outside hold",
                holder.unwrap_or("code outside every function")
            ));
        }
    }
    refused
}

/// The shape before t-47457: the tick locks the observer once and runs whole
/// under it, `gh` calls and all.
const HELD_FOR_THE_WHOLE_TICK: &str = r#"
fn poll(observer: &Mutex<Observer>, limits: &Limits) -> Result<(), String> {
    let mut budget = Budget::new(limits);
    let mut held = hold(observer);
    held.observe(&mut budget)
}
impl Observer {
    fn observe(&mut self, budget: &mut Budget) -> Result<(), String> {
        let review = self.client.discover(&self.root, budget);
        self.book.apply(review)
    }
}
fn hold(observer: &Mutex<Observer>) -> MutexGuard<'_, Observer> {
    observer.lock().unwrap_or_else(PoisonError::into_inner)
}
"#;

/// The shape since: the observer is held a statement at a time, and `gh`
/// runs between those statements.
const HELD_A_STATEMENT_AT_A_TIME: &str = r#"
fn observe(observer: &Mutex<Observer>, budget: &mut Budget) -> Result<(), String> {
    let Some(mut plan) = hold(observer).plan() else {
        return Ok(());
    };
    let heard = plan.discover(budget);
    publish(&hold(observer).book);
    hold(observer).close(heard)
}
impl Plan {
    fn discover(&mut self, budget: &mut Budget) -> Heard {
        self.client.discover(&self.root, budget)
    }
}
fn hold(observer: &Mutex<Observer>) -> MutexGuard<'_, Observer> {
    observer.lock().unwrap_or_else(PoisonError::into_inner)
}
"#;

#[test]
fn a_tick_held_whole_is_refused() {
    assert_eq!(
        refusals(HELD_FOR_THE_WHOLE_TICK),
        [
            "Observer::observe is handed the gh budget",
            "poll keeps the observer held past its statement: `let mut held = hold(observer)`",
        ]
    );
}

#[test]
fn a_tick_held_a_statement_at_a_time_is_accepted() {
    assert_eq!(refusals(HELD_A_STATEMENT_AT_A_TIME), Vec::<String>::new());
}

#[test]
fn a_statement_that_holds_the_observer_and_spends_the_budget_is_refused() {
    let source = r#"
fn observe(observer: &Mutex<Observer>, budget: &mut Budget) {
    hold(observer).write(plan.discover(budget));
}
"#;
    assert_eq!(
        refusals(source),
        ["observe spends the gh budget while it holds the observer: \
          `hold(observer).write(plan.discover(budget))`"]
    );
}

#[test]
fn a_reference_kept_into_the_held_observer_is_refused() {
    let source = r#"
fn observe(observer: &Mutex<Observer>, budget: &mut Budget) {
    let book = &hold(observer).book;
    let heard = plan.discover(budget);
}
"#;
    assert_eq!(
        refusals(source),
        ["observe keeps a reference into the held observer: `let book = &hold(observer).book`"]
    );
}

#[test]
fn a_lock_outside_hold_and_a_spender_that_reaches_the_observer_are_refused() {
    let source = r#"
fn note(observer: &Mutex<Observer>) {
    let held = observer.lock();
}
fn refresh(held: &mut Observer, budget: &mut Budget) {
    held.client.discover(&held.root, budget);
}
"#;
    assert_eq!(
        refusals(source),
        [
            "refresh spends the gh budget and reaches the observer without hold",
            "note locks the observer outside hold",
        ]
    );
}

/// The window's own observer: no `gh` call runs while it is held.
#[test]
fn the_scm_observer_is_never_held_while_gh_runs() {
    let refused = refusals(OBSERVER);
    assert!(
        refused.is_empty(),
        "the SCM observer is held while a tick asks GitHub, so the Checks pane's health \
         read waits for the tick's gh calls (t-47457):\n  {}",
        refused.join("\n  ")
    );
    // The walk is not blind: it reads the tick, which spends the budget and
    // holds the observer.
    let code = code_only(shipped(OBSERVER));
    assert!(
        fn_bodies(&code).iter().any(|body| {
            let text = &code[body.at..=body.close];
            body.name == "observe" && spends(text) && !stands(text, "hold(").is_empty()
        }),
        "the walk found no tick in src/scm_observer.rs that spends the gh budget and holds \
         the observer"
    );
}
