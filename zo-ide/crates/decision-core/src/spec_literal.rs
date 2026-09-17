//! Spec-literal verification — detect prompt-specified literals that the code
//! reproduced with the *wrong case*.
//!
//! When a task spells out an exact output literal in backticks (e.g. a help
//! marker `` `(DEPRECATED)` ``) but the codebase already uses a different casing
//! (`(Deprecated)`), an agent sometimes follows the existing convention instead
//! of the spec — passing the feature's own logic yet failing the gold tests that
//! assert the exact marker. (Measured: on the `click` deprecated-feature task
//! this single miscasing failed 4 parametrized tests and dropped a run from
//! 197/197 to 193/197.)
//!
//! This is a high-signal, low-false-positive class, so the detector is
//! deliberately narrow: it only flags a literal the code reproduced
//! *case-insensitively but not exactly*. A literal the code never wrote (a file
//! path, a code example, an unimplemented marker) is a different failure class
//! and is intentionally NOT reported here — absence ≠ miscasing.

use std::collections::BTreeSet;

/// Minimum literal length to consider. Short tokens (`x`, `id`) collide with
/// incidental case variants too easily to be a reliable casing signal.
const MIN_LITERAL_LEN: usize = 4;

/// A prompt-specified literal the code reproduced with the wrong case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseMismatch {
    /// The exact literal the prompt spelled out (e.g. `(DEPRECATED)`).
    pub spec: String,
    /// The case-variant the code actually wrote (e.g. `(Deprecated)`).
    pub found: String,
}

/// Extract the contents of single-backtick spans from `prompt`, in order. An
/// unterminated trailing backtick contributes nothing. Triple-backtick code
/// fences split into empty spans and are skipped by the emptiness filter.
fn extract_backtick_literals(prompt: &str) -> Vec<String> {
    prompt
        .split('`')
        .enumerate()
        .filter(|(i, span)| i % 2 == 1 && !span.is_empty())
        .map(|(_, span)| span.to_string())
        .collect()
}

/// First substring of `haystack` that matches `needle` case-insensitively but
/// NOT exactly — a wrong-case copy. `None` if every case-insensitive match is
/// exact, or there is no match at all. Reporting a wrong copy even when a correct
/// copy also exists elsewhere is deliberate: a feature task writes the same
/// marker in several places (option, argument, command), so one correct spot
/// must not mask a miscased one. ASCII-only folding keeps the slice byte-aligned
/// with `haystack`, so the index slice is always on a UTF-8 boundary.
fn find_wrong_case_variant(haystack: &str, needle: &str) -> Option<String> {
    let (h, n) = (haystack.as_bytes(), needle.as_bytes());
    if n.is_empty() || n.len() > h.len() {
        return None;
    }
    (0..=h.len() - n.len())
        .filter(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
        .map(|i| &haystack[i..i + n.len()])
        .find(|slice| *slice != needle)
        .map(ToString::to_string)
}

/// A literal is safe to auto-patch only if it is a *marker* — it contains at
/// least one punctuation char (`(`, `)`, `:`, `-`, …), so it appears verbatim
/// in output and stands apart from surrounding code. A bare word or identifier
/// (`click`, `deprecated`, `ValueError`) is NOT a marker: case-folding it would
/// substring-match an unrelated symbol (`Click` inside `ClickException`) and the
/// global replace would corrupt the source — exactly the `clickException`
/// `ImportError` seen on the bench. Identifier casing is the compiler's job, not
/// this gate's.
fn is_marker_literal(s: &str) -> bool {
    s.chars()
        .any(|c| !c.is_ascii_alphanumeric() && c != '_' && c != ' ')
        && !is_code_reference_literal(s)
}

/// Backticks are also Markdown's inline-code delimiter. Method calls, paths,
/// and qualified symbols therefore satisfy the old "contains punctuation"
/// marker test even though changing their case is a semantic source edit, not
/// an output-literal correction. Keep the autopatcher on display markers and
/// CLI tokens, and leave code references to the compiler/tests.
fn is_code_reference_literal(s: &str) -> bool {
    let value = s.trim();
    if value.contains(['/', '\\']) || value.contains("::") || value.contains("->") {
        return true;
    }

    let starts_like_identifier = value
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    if !starts_like_identifier {
        return false;
    }

    if [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')]
        .iter()
        .any(|(open, close)| value.contains(*open) && value.ends_with(*close))
    {
        return true;
    }

    let dotted = value.split('.').collect::<Vec<_>>();
    dotted.len() >= 2
        && dotted.iter().all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        })
}

/// Cheap preflight used by the runtime before it probes git. It intentionally
/// shares the detector's exact candidate predicate so inline code references do
/// not trigger either an unsafe patch or unnecessary worktree scans.
#[must_use]
pub fn has_candidate_spec_literals(prompt: &str) -> bool {
    extract_backtick_literals(prompt).into_iter().any(|literal| {
        let literal = literal.trim();
        literal.len() >= MIN_LITERAL_LEN && is_marker_literal(literal)
    })
}

/// Detect prompt literals the code reproduced with the wrong case.
///
/// For each distinct backtick literal in `prompt` at least `MIN_LITERAL_LEN`
/// long: if `code` contains a copy that matches case-insensitively but not
/// exactly (a wrong-case copy), that is a casing violation — reported even when
/// a correct-case copy also exists elsewhere, since the same marker is written in
/// several places and one correct spot must not mask a miscased one. A literal
/// the code never wrote, or wrote only in the exact case, is not reported.
#[must_use]
pub fn detect_case_mismatched_literals(prompt: &str, code: &str) -> Vec<CaseMismatch> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for literal in extract_backtick_literals(prompt) {
        let literal = literal.trim();
        if literal.len() < MIN_LITERAL_LEN
            || !is_marker_literal(literal)
            || !seen.insert(literal.to_string())
        {
            continue;
        }
        if let Some(found) = find_wrong_case_variant(code, literal) {
            out.push(CaseMismatch {
                spec: literal.to_string(),
                found,
            });
        }
    }
    out
}

/// Apply each detected mismatch to `code`, returning the corrected text. For
/// every wrong-case copy, the marker prefix (everything up to the trailing
/// bracket) is rewritten to the spec casing — so the boolean form `(Deprecated)`
/// AND the string form `(Deprecated: msg)` are both fixed, not only the exact
/// literal. Deterministic: no model involved.
#[must_use]
pub fn apply_case_fixes(code: &str, mismatches: &[CaseMismatch]) -> String {
    let mut seen = BTreeSet::new();
    let mut out = code.to_string();
    for m in mismatches {
        let trim = |s: &str| s.trim_end_matches([')', ']', '}']).to_string();
        let (found, spec) = (trim(&m.found), trim(&m.spec));
        if found.is_empty() || found == spec || !seen.insert(found.clone()) {
            continue;
        }
        out = out.replace(&found, &spec);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        apply_case_fixes, detect_case_mismatched_literals, has_candidate_spec_literals,
        CaseMismatch,
    };

    #[test]
    fn detects_casing_mismatch() {
        // The exact bug from the bench: spec demands `(DEPRECATED)`, code wrote
        // the library's existing `(Deprecated)` casing.
        let prompt = "Its `--help` entry must include a `(DEPRECATED)` marker.";
        let code = r#"            text = "(Deprecated) {text}".format(text=text)"#;
        let hits = detect_case_mismatched_literals(prompt, code);
        assert_eq!(
            hits,
            vec![CaseMismatch {
                spec: "(DEPRECATED)".to_string(),
                found: "(Deprecated)".to_string(),
            }]
        );
    }

    #[test]
    fn exact_case_passes_clean() {
        let prompt = "include a `(DEPRECATED)` marker";
        let code = r#"        else "(DEPRECATED)""#;
        assert!(detect_case_mismatched_literals(prompt, code).is_empty());
    }

    #[test]
    fn absent_literal_is_not_a_casing_bug() {
        // Paths/examples/unimplemented markers the code never wrote are a
        // different failure class — not flagged here (no false positive).
        let prompt = "edit files under `src/click/` and raise `ValueError`";
        let code = "def foo():\n    pass\n";
        assert!(detect_case_mismatched_literals(prompt, code).is_empty());
    }

    #[test]
    fn correctly_cased_identifier_passes() {
        let prompt = "raise `ValueError` for a deprecated required option";
        let code = "raise ValueError(\"deprecated and still required\")";
        assert!(detect_case_mismatched_literals(prompt, code).is_empty());
    }

    #[test]
    fn short_literals_are_ignored() {
        // `id` (2 chars) is below MIN_LITERAL_LEN even though `ID` is a variant.
        let prompt = "store the `id` value";
        let code = "let ID = compute();";
        assert!(detect_case_mismatched_literals(prompt, code).is_empty());
    }

    #[test]
    fn custom_message_marker_casing() {
        // String form: `(Deprecated: ...)` vs spec `(DEPRECATED: ...)`.
        let prompt = "show `(DEPRECATED: USE OTHER COMMAND)` in help";
        let code = "help_text = \"cmd (Deprecated: USE OTHER COMMAND)\"";
        let hits = detect_case_mismatched_literals(prompt, code);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].spec, "(DEPRECATED: USE OTHER COMMAND)");
        assert_eq!(hits[0].found, "(Deprecated: USE OTHER COMMAND)");
    }

    #[test]
    fn duplicate_spec_literal_reported_once() {
        let prompt = "marker `(DEPRECATED)` and again `(DEPRECATED)`";
        let code = "a = \"(Deprecated)\"; b = \"(Deprecated)\"";
        assert_eq!(detect_case_mismatched_literals(prompt, code).len(), 1);
    }

    #[test]
    fn wrong_case_flagged_even_when_correct_copy_exists() {
        // The gate2 bug: the agent wrote the marker correctly in one place
        // (the option) but miscased it in others (command/argument). A single
        // correct copy must NOT mask the wrong ones — the gold tests check each.
        let prompt = "every deprecated entry shows `(DEPRECATED)`";
        let code = "opt = \"(DEPRECATED)\"\ncmd = \"(Deprecated)\"\narg = \"(Deprecated)\"";
        let hits = detect_case_mismatched_literals(prompt, code);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].spec, "(DEPRECATED)");
        assert_eq!(hits[0].found, "(Deprecated)");
    }

    #[test]
    fn apply_fixes_both_boolean_and_string_forms() {
        // Deterministic patch must fix `(Deprecated)` AND `(Deprecated: msg)`
        // via prefix rewrite, even though detect only matches the closed literal.
        let prompt = "every deprecated entry shows `(DEPRECATED)`";
        let code = "opt = \"(Deprecated)\"\ncmd = \"(Deprecated: USE OTHER)\"";
        let mismatches = detect_case_mismatched_literals(prompt, code);
        let fixed = apply_case_fixes(code, &mismatches);
        assert!(fixed.contains("(DEPRECATED)"), "boolean fixed: {fixed}");
        assert!(
            fixed.contains("(DEPRECATED: USE OTHER)"),
            "string fixed: {fixed}"
        );
        assert!(
            !fixed.contains("(Deprecated"),
            "no wrong-case left: {fixed}"
        );
    }

    #[test]
    fn apply_is_noop_without_mismatches() {
        let code = "already = \"(DEPRECATED)\"";
        assert_eq!(apply_case_fixes(code, &[]), code);
    }

    #[test]
    fn library_name_word_is_not_autopatched() {
        // Bench regression: backtick `click` (the library name in the task
        // intro) must NOT match `Click` inside `ClickException`. The old global
        // replace produced `from .exceptions import clickException` -> ImportError.
        let prompt = "You work in the `click` Python library. Help shows `(DEPRECATED)`.";
        let code = "from .exceptions import ClickException\nhelp = \"(Deprecated)\"";
        let hits = detect_case_mismatched_literals(prompt, code);
        assert_eq!(
            hits.len(),
            1,
            "only the marker, never the bare word: {hits:?}"
        );
        assert_eq!(hits[0].spec, "(DEPRECATED)");
        let fixed = apply_case_fixes(code, &hits);
        assert!(
            fixed.contains("ClickException"),
            "class name intact: {fixed}"
        );
        assert!(fixed.contains("(DEPRECATED)"), "marker fixed: {fixed}");
    }

    #[test]
    fn bare_identifier_literals_are_skipped() {
        // `deprecated`, `ValueError`, `prompt` are words, not output markers --
        // skipped so their case variants in code are never globally rewritten.
        let prompt = "add `deprecated`, raise `ValueError`, reject `prompt`";
        let code = "x = Deprecated\nraise ValueError\np = Prompt";
        assert!(detect_case_mismatched_literals(prompt, code).is_empty());
    }

    #[test]
    fn code_reference_literals_are_never_autopatched() {
        let prompt = "Call `Cart.subtotal()` and `Cart.requirements()` after validation.";
        let code = "subtotal = cart.subtotal()\nrequirements = cart.requirements()\n";

        let hits = detect_case_mismatched_literals(prompt, code);
        assert!(hits.is_empty(), "code references are not output markers: {hits:?}");
        assert!(!has_candidate_spec_literals(prompt));
        assert_eq!(apply_case_fixes(code, &hits), code);
    }
}
