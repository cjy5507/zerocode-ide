//! A light lint for the words an agent writes to a person.
//!
//! The person reads Korean and has said what hurts: codebase metaphors carried
//! into prose (a feature called a "seat", a rule list called a "table", a
//! repeat interval called a "beat") and sentences that read as if they were
//! translated from English. The `plain-report` skill tells every agent how to
//! write instead. This module counts how far a text is from that rule, so the
//! window can show the counts beside the text. It counts and never refuses:
//! nothing here stops a report from being filed or read.
//!
//! The lint is a pure function of the text. It makes no model call, opens no
//! file and keeps no state, so it costs the same wherever it runs: on the
//! summary of a `worker_done` when the board shows it, and on a report when the
//! window previews it. Where no text is shown, it costs nothing.
//!
//! For a Korean or an English text it counts:
//!
//! - the mean and the longest sentence, and how many sentences pass the limit
//!   named for the language;
//! - the hits on the rule table (`plain_text/rules.json`): words to replace
//!   (the codebase metaphors, the plain-word rows of ASD-STE100) and
//!   constructions to rewrite (translationese in Korean, stilted phrasing in
//!   English).
//!
//! It does not count the voice of an English sentence, a number without a unit
//! or the order of a Korean sentence. A rule that a few words cannot show is
//! for the skill to teach, not for the lint to guess.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// The rule table, one object per rule. It is data and not code: a metaphor
/// the person corrects next is a new row here, and a test checks the skill's
/// lists against these rows, so the two cannot drift apart.
const RULES_JSON: &str = include_str!("plain_text/rules.json");

/// The longest Korean sentence, in characters with the spaces, before it
/// counts as long. ASD-STE100 caps a descriptive sentence at 25 words, about
/// 150 Latin characters. A Korean text is roughly 40 to 60 % as long as the
/// same English one, so the same limit comes to 60 to 90 characters. The limit
/// takes the end that the person's own range for a sentence, 40 to 60
/// characters, shares with it.
pub const KO_SENTENCE_CHARS_MAX: usize = 60;

/// The longest English sentence, in words, before it counts as long: the
/// ASD-STE100 limit for a descriptive sentence. A step in a procedure gets 20
/// words, which the skill asks for and a lint cannot tell apart from the rest.
pub const EN_SENTENCE_WORDS_MAX: usize = 25;

/// A text is Korean when at least this percent of its letters are Hangul. A
/// Korean report names identifiers and commands in Latin letters, so "mostly
/// Hangul" would call it English; thirty percent keeps it Korean, while an
/// English text with one Korean word stays English.
pub const KOREAN_LETTER_SHARE_MIN_PERCENT: usize = 30;

/// A text that is not Korean is English when at least this percent of its
/// letters are Latin. Below it the text is Japanese, Chinese or mixed, and no
/// rule of the table applies to it.
pub const LATIN_LETTER_SHARE_MIN_PERCENT: usize = 70;

/// The most bytes of one text the lint reads. The cost grows with the text, and
/// a report that fills the preview is still read for its first pages; the
/// lint then says it was cut. A 20 KB report is about a third of the cap, so
/// the worst case costs about three times the measured one.
pub const LINT_TEXT_BYTES_MAX: usize = 64 * 1024;

/// The most rules a result names. The badge has room for a few; the counts
/// still cover every rule.
pub const MAX_REPORTED_RULES: usize = 5;

/// What stands for an inline code span while sentences are measured. One
/// character: an identifier is not prose and must not make a sentence long.
const CODE_STAND_IN: &str = "\u{2022}";

/// The line that opens and closes the metadata block of a Markdown file.
const FRONTMATTER_FENCE: &str = "---";

/// The language a text was read in. The rule table has rows for Korean and
/// English; a text in any other script is `Other` and gets no counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    #[serde(rename = "ko")]
    Korean,
    #[serde(rename = "en")]
    English,
    #[default]
    #[serde(rename = "other")]
    Other,
}

impl Language {
    /// The longest sentence the language allows, in its own unit: characters
    /// for Korean, words for English. `None` for a language with no rules.
    #[must_use]
    pub const fn sentence_limit(self) -> Option<usize> {
        match self {
            Self::Korean => Some(KO_SENTENCE_CHARS_MAX),
            Self::English => Some(EN_SENTENCE_WORDS_MAX),
            Self::Other => None,
        }
    }
}

/// What a rule asks the writer to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    /// A word to replace with a plainer one.
    Word,
    /// A construction to rewrite.
    Pattern,
}

/// How a spelling of a rule is found in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Match {
    /// The spelling starts at the start of a word. A Korean word takes its
    /// particle on the right, so only the left edge is checked there; an
    /// English word is checked on both edges.
    Word,
    /// The spelling is found anywhere in the text.
    Phrase,
}

/// One row of the table, ready to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub language: Language,
    pub kind: RuleKind,
    pub mode: Match,
    /// The spellings that count, lower-cased once here so that no scan
    /// lower-cases them again.
    pub finds: Vec<String>,
    /// How the rule is named to a reader.
    pub shown: String,
    /// What to write instead.
    pub plain: String,
    /// Why the rule is there, for whoever edits the table.
    pub note: String,
}

/// A row as the table file spells it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    lang: Language,
    kind: RuleKind,
    mode: Match,
    find: Vec<String>,
    #[serde(default)]
    show: Option<String>,
    plain: String,
    note: String,
}

impl RawRule {
    /// The ready row, or `None` for a row that names no spelling or an empty one
    /// (an empty spelling would match at every place of every text).
    fn into_rule(self) -> Option<Rule> {
        if self.find.iter().any(|find| find.trim().is_empty()) {
            return None;
        }
        let first = self.find.first()?.clone();
        Some(Rule {
            language: self.lang,
            kind: self.kind,
            mode: self.mode,
            finds: self.find.iter().map(|find| find.to_lowercase()).collect(),
            shown: self.show.unwrap_or(first),
            plain: self.plain,
            note: self.note,
        })
    }
}

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    serde_json::from_str::<Vec<RawRule>>(RULES_JSON)
        .unwrap_or_default()
        .into_iter()
        .filter_map(RawRule::into_rule)
        .collect()
});

/// Every rule of the table, in table order. Read once, on the first lint.
#[must_use]
pub fn rules() -> &'static [Rule] {
    RULES.as_slice()
}

/// One rule that a text hit, and how many times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuleHit {
    pub kind: RuleKind,
    /// How the rule is named to a reader.
    pub find: String,
    /// What to write instead.
    pub plain: String,
    pub count: u32,
}

/// What the lint counted in one text. Counts only: nothing here is a verdict.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TextLint {
    pub lang: Language,
    pub sentences: u32,
    /// The mean sentence length, rounded: characters for Korean, words for
    /// English.
    pub avg_len: u32,
    pub longest: u32,
    /// The limit the sentences were held to, in the same unit.
    pub limit: u32,
    /// How many sentences passed the limit.
    pub long_sentences: u32,
    /// Hits on the rows that ask for a plainer word.
    pub words: u32,
    /// Hits on the rows that ask for a rewritten construction.
    pub patterns: u32,
    /// The rules hit most often, most first, at most [`MAX_REPORTED_RULES`].
    pub hits: Vec<RuleHit>,
    /// The text was longer than [`LINT_TEXT_BYTES_MAX`] and only its start was
    /// read.
    pub cut: bool,
}

/// Count how far a text is from the plain-writing rule. Pure: the same text
/// gives the same counts, and nothing outside the text is read.
#[must_use]
pub fn lint(text: &str) -> TextLint {
    let (text, cut) = bounded(text);
    let units = units_of(text);
    let prose = units.join("\n");
    let lang = language_of(&prose);
    let Some(limit) = lang.sentence_limit() else {
        return TextLint {
            lang,
            cut,
            ..TextLint::default()
        };
    };
    let lengths: Vec<usize> = units
        .iter()
        .flat_map(|unit| split_sentences(unit))
        .map(|sentence| measure(lang, sentence))
        .collect();
    let (hits, words, patterns) = scan(&prose.to_lowercase(), lang);
    TextLint {
        lang,
        sentences: counted(lengths.len()),
        avg_len: counted(mean(&lengths)),
        longest: counted(lengths.iter().copied().max().unwrap_or(0)),
        limit: counted(limit),
        long_sentences: counted(lengths.iter().filter(|length| **length > limit).count()),
        words,
        patterns,
        hits,
        cut,
    }
}

/// A lint remembered for as long as the text it read stays the same.
///
/// The board asks about the same few finished tasks on every beat. The answer
/// to each is worked out once and kept; a beat that moved nothing works out
/// nothing, and a task nobody asked about in a beat is let go.
#[derive(Debug, Default)]
pub struct LintMemo {
    held: HashMap<(String, String), Memo>,
    beat: u64,
    worked: usize,
}

#[derive(Debug)]
struct Memo {
    fingerprint: u64,
    lint: Option<TextLint>,
    /// The beat that last asked for it.
    beat: u64,
}

impl LintMemo {
    /// Open a beat.
    pub fn begin(&mut self) {
        self.beat += 1;
    }

    /// The lint of the text `text_of` reads, which belongs to `id` in `scope`:
    /// the remembered one while `stamp` — everything the text is read from —
    /// stays the same, a fresh one when it changed. A beat that finds the stamp
    /// unchanged neither reads the text nor lints it, so the parse of a
    /// worker's report is paid once and not once a beat. A row with no text is
    /// remembered too, as `None`.
    pub fn lint_in<S: Hash + ?Sized>(
        &mut self,
        scope: &str,
        id: &str,
        stamp: &S,
        text_of: impl FnOnce() -> Option<String>,
    ) -> Option<TextLint> {
        let fingerprint = fingerprint_of(stamp);
        let key = (scope.to_string(), id.to_string());
        if let Some(memo) = self.held.get_mut(&key)
            && memo.fingerprint == fingerprint
        {
            memo.beat = self.beat;
            return memo.lint.clone();
        }
        let found = text_of().map(|text| lint(&text));
        self.worked += 1;
        self.held.insert(
            key,
            Memo {
                fingerprint,
                lint: found.clone(),
                beat: self.beat,
            },
        );
        found
    }

    /// The lint of `text`, which belongs to `id` in `scope`: the remembered one
    /// while the text is the same, a fresh one when it changed.
    pub fn lint_of(&mut self, scope: &str, id: &str, text: &str) -> TextLint {
        self.lint_in(scope, id, text, || Some(text.to_string()))
            .unwrap_or_default()
    }

    /// Close the beat: let go of every lint it did not ask for.
    pub fn end(&mut self) {
        let beat = self.beat;
        self.held.retain(|_, memo| memo.beat == beat);
    }

    /// The lints worked out rather than remembered, since the memo was made.
    #[must_use]
    pub fn worked(&self) -> usize {
        self.worked
    }

    /// The lints held now.
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

fn fingerprint_of<S: Hash + ?Sized>(stamp: &S) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    stamp.hash(&mut hasher);
    hasher.finish()
}

/// A count as the wire's number; a count too big for it is the biggest it holds.
fn counted(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// The text the lint reads, and whether the cap cut it. A cut falls on a whole
/// character.
fn bounded(text: &str) -> (&str, bool) {
    if text.len() <= LINT_TEXT_BYTES_MAX {
        return (text, false);
    }
    let mut end = LINT_TEXT_BYTES_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// The running text of a Markdown file, one unit per paragraph or list item.
///
/// A reader reads the lines of a paragraph as one flow, so they are joined;
/// a list item starts a unit of its own. A heading, a table row, a rule line, a
/// fenced block, a block of HTML and the metadata block at the top are not
/// prose and are left out. Inline code and link targets are taken out of
/// the units that stay.
fn units_of(text: &str) -> Vec<String> {
    let mut units: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut fenced = false;
    let mut front = has_frontmatter(text);
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if front {
            if index > 0 && line == FRONTMATTER_FENCE {
                front = false;
            }
            continue;
        }
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            flush(&mut current, &mut units);
            continue;
        }
        if fenced {
            continue;
        }
        if line.is_empty() || is_structure(line) {
            flush(&mut current, &mut units);
            continue;
        }
        let (body, opens) = strip_marker(line);
        if opens {
            flush(&mut current, &mut units);
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&cleaned(body));
    }
    flush(&mut current, &mut units);
    units
}

/// Whether the text opens with a metadata block that a later line closes.
fn has_frontmatter(text: &str) -> bool {
    let mut lines = text.lines();
    lines
        .next()
        .is_some_and(|first| first.trim() == FRONTMATTER_FENCE)
        && lines.any(|line| line.trim() == FRONTMATTER_FENCE)
}

fn flush(current: &mut String, units: &mut Vec<String>) {
    let said = current.trim();
    if !said.is_empty() {
        units.push(said.to_string());
    }
    current.clear();
}

/// A line that is not prose: a heading, a table row, a block of HTML or a
/// rule line.
fn is_structure(line: &str) -> bool {
    line.starts_with('#')
        || line.starts_with('|')
        || line.starts_with('<')
        || (line.len() >= 3
            && line
                .chars()
                .all(|mark| matches!(mark, '-' | '*' | '_' | '=' | ' ')))
}

/// The text of a line without its list or quote marker, and whether the marker
/// opens a new unit (a list item does; a quote continues its paragraph).
fn strip_marker(line: &str) -> (&str, bool) {
    for marker in ["- ", "* ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return (strip_checkbox(rest), true);
        }
    }
    let digits = line.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(after) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return (after, true);
        }
    }
    match line.strip_prefix("> ") {
        Some(rest) => (rest, false),
        None => (line, false),
    }
}

fn strip_checkbox(rest: &str) -> &str {
    for mark in ["[ ] ", "[x] ", "[X] "] {
        if let Some(after) = rest.strip_prefix(mark) {
            return after;
        }
    }
    rest
}

/// A line of prose without inline code (one character stands for each span),
/// link targets and emphasis marks.
fn cleaned(body: &str) -> String {
    let code_free = replace_spans(body, "`", "`", CODE_STAND_IN);
    let link_free = replace_spans(&code_free, "](", ")", "");
    link_free.replace(['*', '[', ']'], "")
}

/// The text with every span from `opener` to `closer` replaced by `stand_in`.
/// An opener that nothing closes stays as it stands.
fn replace_spans(text: &str, opener: &str, closer: &str, stand_in: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(opener) {
        let after = &rest[start + opener.len()..];
        let Some(end) = after.find(closer) else {
            break;
        };
        out.push_str(&rest[..start]);
        out.push_str(stand_in);
        rest = &after[end + closer.len()..];
    }
    out.push_str(rest);
    out
}

/// The sentences of one unit. A sentence ends at a full stop, a question mark
/// or an exclamation mark that is followed by a space or by the end, so the
/// point in `7.4` or in `1.1.50` does not end one.
fn split_sentences(unit: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut start = 0;
    let mut chars = unit.char_indices().peekable();
    while let Some((at, mark)) = chars.next() {
        if !matches!(mark, '.' | '!' | '?') {
            continue;
        }
        if chars.peek().is_none_or(|(_, next)| next.is_whitespace()) {
            let end = at + mark.len_utf8();
            found.push(&unit[start..end]);
            start = end;
        }
    }
    found.push(&unit[start..]);
    found
        .into_iter()
        .map(str::trim)
        .filter(|sentence| sentence.chars().any(char::is_alphanumeric))
        .collect()
}

/// A sentence's length in the language's unit: characters for Korean, words
/// for English.
fn measure(language: Language, sentence: &str) -> usize {
    match language {
        Language::Korean => sentence.chars().count(),
        _ => sentence.split_whitespace().count(),
    }
}

fn mean(lengths: &[usize]) -> usize {
    let count = lengths.len();
    if count == 0 {
        return 0;
    }
    (lengths.iter().sum::<usize>() + count / 2) / count
}

/// The language of a prose, by the share of its letters.
fn language_of(prose: &str) -> Language {
    let (mut hangul, mut latin, mut letters) = (0usize, 0usize, 0usize);
    for letter in prose.chars().filter(|one| one.is_alphabetic()) {
        letters += 1;
        if ('\u{ac00}'..='\u{d7a3}').contains(&letter) {
            hangul += 1;
        } else if letter.is_ascii_alphabetic() || ('\u{00c0}'..='\u{024f}').contains(&letter) {
            latin += 1;
        }
    }
    if letters == 0 {
        Language::Other
    } else if hangul * 100 >= letters * KOREAN_LETTER_SHARE_MIN_PERCENT {
        Language::Korean
    } else if latin * 100 >= letters * LATIN_LETTER_SHARE_MIN_PERCENT {
        Language::English
    } else {
        Language::Other
    }
}

/// The hits of the language's rows on a lower-cased prose, most frequent first,
/// and the two totals.
fn scan(lowered: &str, language: Language) -> (Vec<RuleHit>, u32, u32) {
    let mut hits = Vec::new();
    let (mut words, mut patterns) = (0usize, 0usize);
    for rule in rules().iter().filter(|rule| rule.language == language) {
        let count = count_matches(lowered, rule);
        if count == 0 {
            continue;
        }
        match rule.kind {
            RuleKind::Word => words += count,
            RuleKind::Pattern => patterns += count,
        }
        hits.push(RuleHit {
            kind: rule.kind,
            find: rule.shown.clone(),
            plain: rule.plain.clone(),
            count: counted(count),
        });
    }
    // A stable sort: rules with the same count stay in table order.
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.count));
    hits.truncate(MAX_REPORTED_RULES);
    (hits, counted(words), counted(patterns))
}

/// How many times the rule's spellings stand in the text.
fn count_matches(lowered: &str, rule: &Rule) -> usize {
    rule.finds
        .iter()
        .map(|find| {
            lowered
                .match_indices(find.as_str())
                .filter(|(at, _)| stands(lowered, *at, find.len(), rule))
                .count()
        })
        .sum()
}

/// Whether a spelling found at `at` counts, by the rule's way of matching.
fn stands(text: &str, at: usize, len: usize, rule: &Rule) -> bool {
    match rule.mode {
        Match::Phrase => true,
        Match::Word => {
            left_is_open(text, at)
                && (rule.language == Language::Korean || right_is_open(text, at + len))
        }
    }
}

fn is_word_char(one: char) -> bool {
    one.is_alphanumeric() || one == '_'
}

fn left_is_open(text: &str, at: usize) -> bool {
    text[..at]
        .chars()
        .next_back()
        .is_none_or(|before| !is_word_char(before))
}

fn right_is_open(text: &str, end: usize) -> bool {
    text[end..]
        .chars()
        .next()
        .is_none_or(|after| !is_word_char(after))
}

#[cfg(test)]
mod tests;
