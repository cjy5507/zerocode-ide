//! A `:has()` costs what it reaches (t-84136).
//!
//! The engine asks a `:has()` again whenever anything its arguments name may
//! have changed — and `[hidden]`, named in many, changes all the time. A
//! `:has()` hung on an element a conversation stands in, and reaching past
//! that element's children or siblings, is then matched over every row of
//! every conversation in the window: turning a long pane back from its
//! terminal to its conversation (84 rows) took 46 ms at a 4x CPU, 26 ms of it
//! styling, until `body:has(…)` read only the body's children. And what a
//! rule with a `:has()` styles joins one set the engine walks for every
//! `:has()` that may have changed: a bare `span` there restyled every span in
//! the window — a long conversation's hundreds — at each `hidden` that changed.
//!
//! So: on the elements a conversation stands in, a `:has()` reaches only its
//! children (`>`) or its siblings (`+`, `~`), and a rule with a `:has()`
//! styles a class, an id or an attribute, never a bare tag.

use crate::support::strip_comments;

/// The elements a conversation's rows stand in, from the document down to
/// its list — each a compound's first selector.
const AROUND_CONVERSATIONS: &[&str] = &[
    "body",
    "html",
    ":root",
    "#workbench",
    ".panes",
    ".pane",
    ".pane-split",
    ".pane-slot",
    ".worker-view",
    ".pane-chat",
    ".helper-turns",
];

/// Every selector in the stylesheet that names a `:has()`, one complex
/// selector at a time, its whitespace folded.
fn has_selectors() -> Vec<String> {
    let css = strip_comments(include_str!("../../../../ui/shell.css"));
    let mut selectors = Vec::new();
    let mut buf = String::new();
    for glyph in css.chars() {
        match glyph {
            '{' => {
                let opened = buf.trim().to_string();
                buf.clear();
                if !opened.starts_with('@') && opened.contains(":has(") {
                    selectors.extend(
                        split_top(&opened, ',')
                            .into_iter()
                            .map(|one| one.split_whitespace().collect::<Vec<_>>().join(" "))
                            .filter(|one| one.contains(":has(")),
                    );
                }
            }
            '}' | ';' => buf.clear(),
            _ => buf.push(glyph),
        }
    }
    selectors
}

/// `text` cut at every `at` outside parentheses.
fn split_top(text: &str, at: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut depth = 0usize;
    for glyph in text.chars() {
        match glyph {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if glyph == at && depth == 0 {
            parts.push(String::new());
        } else if let Some(last) = parts.last_mut() {
            last.push(glyph);
        }
    }
    parts
}

/// A complex selector's compounds, cut at its combinators outside
/// parentheses.
fn compounds(selector: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for glyph in selector.chars() {
        match glyph {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth == 0 && (glyph.is_whitespace() || matches!(glyph, '>' | '+' | '~')) {
            if !current.trim().is_empty() {
                found.push(current.trim().to_string());
            }
            current.clear();
        } else {
            current.push(glyph);
        }
    }
    if !current.trim().is_empty() {
        found.push(current.trim().to_string());
    }
    found
}

/// The arguments of the first `:has(…)` in `compound`.
fn has_arguments(compound: &str) -> Vec<String> {
    let Some(at) = compound.find(":has(") else {
        return Vec::new();
    };
    let body = &compound[at + ":has(".len()..];
    let mut depth = 1usize;
    let mut end = body.len();
    for (index, glyph) in body.char_indices() {
        match glyph {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = index;
                    break;
                }
            }
            _ => {}
        }
    }
    split_top(&body[..end], ',')
        .into_iter()
        .map(|one| one.trim().to_string())
        .collect()
}

/// Whether `compound` begins with `name` as a whole selector.
fn begins_with(compound: &str, name: &str) -> bool {
    compound.strip_prefix(name).is_some_and(|rest| {
        !rest
            .chars()
            .next()
            .is_some_and(|next| next.is_alphanumeric() || next == '-' || next == '_')
    })
}

#[test]
fn a_has_rule_around_a_conversation_reaches_only_its_children_or_siblings() {
    let selectors = has_selectors();
    assert!(
        selectors.len() >= 10,
        "the stylesheet's `:has()` rules stopped parsing: {} found",
        selectors.len()
    );
    let mut wrong = Vec::new();
    for selector in &selectors {
        for compound in compounds(selector) {
            if !compound.contains(":has(")
                || !AROUND_CONVERSATIONS
                    .iter()
                    .any(|name| begins_with(&compound, name))
            {
                continue;
            }
            if has_arguments(&compound)
                .iter()
                .any(|argument| !argument.starts_with(['>', '+', '~']))
            {
                wrong.push(selector.clone());
            }
        }
    }
    wrong.dedup();
    assert!(
        wrong.is_empty(),
        "a `:has()` on an element a conversation stands in reads past its children or its \
         siblings, and is matched again over every row in the window whenever a `hidden` \
         changes anywhere — say it from the children (`>`) or the siblings (`+`, `~`), or let \
         the code that changes it say so:\n  {}",
        wrong.join("\n  ")
    );
}

#[test]
fn a_has_rule_styles_no_bare_tag() {
    let mut wrong = Vec::new();
    for selector in has_selectors() {
        let Some(last) = compounds(&selector).pop() else {
            continue;
        };
        let last = last
            .trim_end_matches("::before")
            .trim_end_matches("::after")
            .to_string();
        let (mut outside, mut inside) = (String::new(), String::new());
        let mut depth = 0usize;
        for glyph in last.chars() {
            match glyph {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ if depth == 0 => outside.push(glyph),
                _ => inside.push(glyph),
            }
        }
        let named = |text: &str| text.contains(['.', '#', '[']);
        if !named(&outside) && !named(&inside) {
            wrong.push(selector);
        }
    }
    assert!(
        wrong.is_empty(),
        "a rule with a `:has()` styles a bare tag — the engine then restyles every element of \
         that tag in the window whenever any `:has()` may have changed; name it by a class:\n  {}",
        wrong.join("\n  ")
    );
}
