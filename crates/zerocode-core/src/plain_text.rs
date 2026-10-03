//! A light lint for the words an agent writes to a person.
//!
//! The person reads Korean and has said what hurts: codebase metaphors carried
//! into prose and sentences that read as if they were translated from English.
//! The `plain-report` skill tells every agent how to write instead. This module
//! counts how far a text is from that rule, so the window can show the counts
//! beside the text. It counts and never refuses.
//!
//! This is the red skeleton of t-32786: the types, the limits and the entry
//! points the tests name, with no counting behind them yet.

use serde::Serialize;

/// The rule table. The skeleton carries none; the table arrives with the lint.
const RULES_JSON: &str = "";

/// The longest Korean sentence, in characters with the spaces, before it
/// counts as long.
pub const KO_SENTENCE_CHARS_MAX: usize = 60;

/// The longest English sentence, in words, before it counts as long.
pub const EN_SENTENCE_WORDS_MAX: usize = 25;

/// The most bytes of one text the lint reads.
pub const LINT_TEXT_BYTES_MAX: usize = 64 * 1024;

/// The most rules a result names.
pub const MAX_REPORTED_RULES: usize = 5;

/// The language a text was read in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum Language {
    #[serde(rename = "ko")]
    Korean,
    #[serde(rename = "en")]
    English,
    #[default]
    #[serde(rename = "other")]
    Other,
}

/// What a rule asks the writer to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    Word,
    Pattern,
}

/// How a spelling of a rule is found in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Match {
    Word,
    Phrase,
}

/// One row of the table, ready to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub language: Language,
    pub kind: RuleKind,
    pub mode: Match,
    pub finds: Vec<String>,
    pub shown: String,
    pub plain: String,
    pub note: String,
}

/// Every rule of the table.
#[must_use]
pub fn rules() -> &'static [Rule] {
    &[]
}

/// One rule that a text hit, and how many times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleHit {
    pub kind: RuleKind,
    pub find: String,
    pub plain: String,
    pub count: u32,
}

/// What the lint counted in one text. Counts only: nothing here is a verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TextLint {
    pub lang: Language,
    pub sentences: u32,
    pub avg_len: u32,
    pub longest: u32,
    pub limit: u32,
    pub long_sentences: u32,
    pub words: u32,
    pub patterns: u32,
    pub hits: Vec<RuleHit>,
    pub cut: bool,
}

/// Count how far a text is from the plain-writing rule.
#[must_use]
pub fn lint(_text: &str) -> TextLint {
    TextLint::default()
}

/// A lint remembered for as long as the text it read stays the same.
#[derive(Debug, Default)]
pub struct LintMemo;

impl LintMemo {
    /// Open a beat.
    pub fn begin(&mut self) {}

    /// The lint of `text`, which belongs to `id` in `scope`.
    pub fn lint_of(&mut self, _scope: &str, _id: &str, text: &str) -> TextLint {
        lint(text)
    }

    /// Close the beat.
    pub fn end(&mut self) {}

    /// The lints worked out rather than remembered, since the memo was made.
    #[must_use]
    pub fn worked(&self) -> usize {
        0
    }

    /// The lints held now.
    #[must_use]
    pub fn len(&self) -> usize {
        0
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
