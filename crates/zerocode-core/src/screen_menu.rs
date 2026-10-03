//! A numbered menu standing on a pane's screen: what it asks, what it offers,
//! and whether a card made earlier is still about it.
//!
//! Two roads need the same reading and used to have none:
//!
//! - **The late answer.** A card is drawn from what an agent SAID it asked
//!   (a hook's payload) and answered some moments later by typing into the
//!   pane. In between the question may have been answered at the keyboard,
//!   resolved by the agent itself, or replaced by the next one — and a key
//!   typed then lands in whatever the screen shows now: a digit in somebody
//!   else's menu, an Escape that stops a running turn. [`ScreenMenu::shows`]
//!   is the question "is the thing this card asks still the thing on the
//!   screen", asked just before the key is typed.
//! - **The menu nobody described.** A CLI with no structured question road
//!   still stops on a numbered menu and signals only that it waits. The
//!   screen is the only place that menu exists, so [`read_menu`] reads it
//!   there and [`ScreenMenu::card`] draws the same card an agent-described
//!   question gets.
//!
//! The reading is deliberately generic — a block of `N. label` / `N) label` /
//! `❯ N. label` rows at the bottom of the screen with the selection on one of
//! them — and the places where one CLI differs (how it takes a choice) are the
//! caller's: [`crate::ask::walk_to_row`] is the generic way, an agent's own
//! grammar is [`crate::ask::keys_for`].
//!
//! **What "the same question" means.** Not the whole screen: a clock, a
//! spinner or a token counter elsewhere on it is not the question changing,
//! and a check that read them would refuse a good answer every second. The
//! check compares the words the CARD carries — the question's tail and each
//! declared row — with the menu standing on the screen now, normalised the
//! way a person reads them (case, punctuation, padding and trailing hints do
//! not count; a counter that ticks inside the question does not count; a
//! number that is part of what is asked does).
//!
//! Pure on purpose: rows in, words out. The terminal, its lock and its clock
//! are the shell's.

use crate::ask::{AskOption, AskPrompt, AskQuestion};

/// A menu offers a choice, and a choice is between two rows at least. One
/// numbered row is a line of ordinary output.
const MENU_ROWS_MIN: usize = 2;

/// How many rows of text a menu may have under its last option: its hint
/// line, a status line. A live menu owns the bottom of the screen; one with
/// more than this below it has been answered and scrolled away from.
const FOOTER_ROWS_MAX: usize = 4;

/// How many rows that are not options may sit between two options before
/// they are two lists: a description of a few lines, a rule.
const OPTION_GAP_ROWS_MAX: usize = 5;

/// How many rows above the options are read as what is being asked.
const QUESTION_ROWS_MAX: usize = 8;

/// How many of those rows — the ones nearest the options — a card carries as
/// its question. A person reads the command and the ask, not the dialog's
/// whole body, and the card is small; what the card carries is also what the
/// door checks before it types, so it is the part that stands against the
/// options.
const CARD_QUESTION_ROWS: usize = 3;

/// How far above the options the question is looked for, blank rows
/// included, so a screen that is mostly empty is not walked to its top.
const QUESTION_SCAN_ROWS_MAX: usize = 24;

/// How much of a question's end is compared: the part that stands against the
/// options, which is the part that stays when something above it scrolls.
const QUESTION_TAIL_CHARS: usize = 80;

/// How short a label can be and still be taken as the front of a longer one
/// the pane cut off with an ellipsis. Below this a fragment matches too many
/// labels to say anything.
const TRUNCATED_LABEL_MIN_CHARS: usize = 8;

/// How many horizontal bars make a row a rule. Fewer is a dash in a sentence.
const RULE_MIN_CHARS: usize = 4;

/// A menu's numbers are small. A longer run of digits is a year, a count, a
/// port — not a row number.
const NUMBER_DIGITS_MAX: usize = 3;

/// What a TUI puts in front of the row its selection is on.
const SELECTION_GLYPHS: [char; 8] = ['❯', '›', '>', '→', '➜', '▶', '▸', '➤'];

/// Checkbox spellings in front of a label.
const CHECKBOX_WORDS: [&str; 5] = ["[ ]", "[x]", "[X]", "[✓]", "[✔]"];
const CHECKBOX_GLYPHS: [char; 5] = ['☐', '☑', '☒', '✓', '✔'];

/// Units that make a number a thing that moves on its own — an elapsed time,
/// a size, a token count — so it is not read as part of what is asked.
/// "3 files" is a count of the question; "3m 10s" is a clock.
const VOLATILE_UNITS: [&str; 18] = [
    "ms", "s", "sec", "secs", "m", "min", "mins", "h", "hr", "hrs", "d", "k", "K", "M", "G", "%",
    "token", "tokens",
];

/// One row a menu offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuOption {
    /// The number the row wears — the screen's own, which the reading
    /// guarantees runs from 1 without a gap.
    pub number: u32,
    /// The row's words, with the selection glyph, the number and a checkbox
    /// taken off.
    pub label: String,
    /// What the rows indented under this one say — a description, or the
    /// tail of a label the pane wrapped. Empty when there is none.
    pub detail: String,
    /// Whether the row carries a checkbox, which makes the menu a
    /// multi-select.
    pub checkbox: bool,
}

/// A numbered menu with the selection on one of its rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenMenu {
    /// The rows above the options that say what is being asked, top first,
    /// each with its glyphs and its padding taken off.
    pub question: Vec<String>,
    pub options: Vec<MenuOption>,
    /// Which option the selection stands on, as an index into `options`.
    pub selected: usize,
}

/// One screen row, read once.
struct Row {
    /// Columns of padding before the words, counted inside a box's border.
    indent: usize,
    /// The words, border and trailing padding off.
    text: String,
    kind: Kind,
}

enum Kind {
    Blank,
    /// A horizontal rule or the edge of a box.
    Rule,
    Option(Numbered),
    Text,
}

struct Numbered {
    number: u32,
    label: String,
    marked: bool,
    checkbox: bool,
    /// The column the number starts at — what a description has to be
    /// indented past to belong to this row.
    number_col: usize,
}

/// Read the menu a pane is stopped on, if the screen shows one.
///
/// `rows` are the visible rows top to bottom, each trailing-trimmed and with
/// its leading padding kept (the padding is how a description is told from
/// the next question); `cursor_row` is where the terminal's own cursor sits,
/// which some menus use as their only selection mark.
#[must_use]
pub fn read_menu(rows: &[String], cursor_row: Option<usize>) -> Option<ScreenMenu> {
    let classed: Vec<Row> = rows.iter().map(|row| classify(row)).collect();
    let last = last_option(&classed)?;
    let group = option_group(&classed, last)?;
    if group.len() < MENU_ROWS_MIN {
        return None;
    }
    let selected = selection(&classed, &group, cursor_row)?;
    let listed: Vec<(usize, &Numbered)> = group
        .iter()
        .filter_map(|&at| match &classed[at].kind {
            Kind::Option(numbered) => Some((at, numbered)),
            _ => None,
        })
        .collect();
    let options = listed
        .iter()
        .enumerate()
        .map(|(k, &(at, numbered))| MenuOption {
            number: numbered.number,
            label: numbered.label.clone(),
            detail: detail_below(
                &classed,
                at,
                listed.get(k + 1).map(|&(next, _)| next),
                numbered.number_col,
            ),
            checkbox: numbered.checkbox,
        })
        .collect();
    Some(ScreenMenu {
        question: question_above(&classed, group[0]),
        options,
        selected,
    })
}

/// Whether the words `text` are on the screen's last rows — the check for a
/// question that offers no rows to compare.
#[must_use]
pub fn words_are_up(rows: &[String], text: &str) -> bool {
    let wanted = comparable(text);
    if wanted.is_empty() {
        return false;
    }
    let last: Vec<&String> = rows
        .iter()
        .filter(|row| !row.trim().is_empty())
        .rev()
        .take(QUESTION_SCAN_ROWS_MAX)
        .collect();
    let shown: String = last.iter().rev().map(|row| comparable(row)).collect();
    shown.contains(tail_chars(&wanted, QUESTION_TAIL_CHARS))
}

impl ScreenMenu {
    /// Whether any row has a checkbox — a menu this reading can show but not
    /// answer with one choice.
    #[must_use]
    pub fn multi_select(&self) -> bool {
        self.options.iter().any(|option| option.checkbox)
    }

    /// The card this menu is drawn as: one single-select question whose rows
    /// are the menu's, and whose words are the rows nearest them
    /// (`CARD_QUESTION_ROWS` of them). `None` for a multi-select, which a card of one
    /// pick would answer wrongly.
    #[must_use]
    pub fn card(&self) -> Option<AskPrompt> {
        if self.multi_select() {
            return None;
        }
        let options = self
            .options
            .iter()
            .map(|option| AskOption {
                label: option.label.clone(),
                description: (!option.detail.is_empty()).then(|| option.detail.clone()),
            })
            .collect();
        let nearest = &self.question[self.question.len().saturating_sub(CARD_QUESTION_ROWS)..];
        Some(AskPrompt {
            questions: vec![AskQuestion {
                question: nearest.join("\n"),
                header: None,
                multi_select: false,
                options,
            }],
        })
    }

    /// Whether this menu is the one `question` asked about: the question's
    /// words are the ones standing above the options, and each option it
    /// declared is the row of the same number.
    ///
    /// The menu may offer MORE rows than the question declared — an agent's
    /// own "type something" and "chat about this" rows — but never fewer, and
    /// never the same rows in another order.
    #[must_use]
    pub fn shows(&self, question: &AskQuestion) -> bool {
        options_agree(&self.options, &question.options)
            && words_agree(&self.question.join("\n"), &question.question)
    }
}

// ---- rows ------------------------------------------------------------

/// Box-drawing and block characters: the border a TUI draws a dialog in.
fn is_frame(c: char) -> bool {
    ('\u{2500}'..='\u{259F}').contains(&c)
}

/// Whether a row is a horizontal rule or the top or bottom of a box: nothing
/// but border, bars and padding, with enough bars to be a line.
fn is_rule(row: &str) -> bool {
    let bars = row
        .chars()
        .filter(|&c| (is_frame(c) && !"│┃║▏▕".contains(c)) || matches!(c, '-' | '=' | '_'))
        .count();
    bars >= RULE_MIN_CHARS
        && row
            .chars()
            .all(|c| is_frame(c) || matches!(c, '-' | '=' | '_' | ' '))
}

/// A row's padding and words. A box's left border is taken off first, so the
/// padding is counted inside it and rows of one box agree with each other.
fn unframed(row: &str) -> (usize, &str) {
    let lead = row.chars().take_while(|c| *c == ' ').count();
    let after_lead = &row[lead..];
    let body = after_lead.trim_start_matches(is_frame);
    let (indent, body) = if body.len() == after_lead.len() {
        (lead, after_lead)
    } else {
        let padding = body.chars().take_while(|c| *c == ' ').count();
        (padding, &body[padding..])
    };
    (
        indent,
        body.trim_end_matches(|c: char| is_frame(c) || c == ' '),
    )
}

fn classify(raw: &str) -> Row {
    let trimmed = raw.trim_end();
    let blank = |kind| Row {
        indent: 0,
        text: String::new(),
        kind,
    };
    if trimmed.is_empty() {
        return blank(Kind::Blank);
    }
    if is_rule(trimmed) {
        return blank(Kind::Rule);
    }
    let (indent, inner) = unframed(trimmed);
    if inner.is_empty() {
        return blank(Kind::Blank);
    }
    let kind = numbered_row(indent, inner).map_or(Kind::Text, Kind::Option);
    Row {
        indent,
        text: inner.to_string(),
        kind,
    }
}

/// `❯ 2. label`, `2) label` and `  2. label` — a number, its separator, and
/// words; with or without a selection glyph in front.
fn numbered_row(indent: usize, inner: &str) -> Option<Numbered> {
    let mut rest = inner;
    let mut before_number = 0;
    let mut marked = false;
    if let Some(first) = rest.chars().next()
        && SELECTION_GLYPHS.contains(&first)
    {
        marked = true;
        rest = &rest[first.len_utf8()..];
        let padding = rest.chars().take_while(|c| *c == ' ').count();
        rest = &rest[padding..];
        before_number = 1 + padding;
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > NUMBER_DIGITS_MAX {
        return None;
    }
    let number: u32 = rest[..digits].parse().ok()?;
    let mut after = rest[digits..].chars();
    if !matches!(after.next()?, '.' | ')') {
        return None;
    }
    let tail = after.as_str();
    if !tail.starts_with(char::is_whitespace) {
        return None;
    }
    let (label, checkbox) = without_checkbox(tail.trim());
    let label = words_of_label(label);
    if label.is_empty() {
        return None;
    }
    Some(Numbered {
        number,
        label,
        marked,
        checkbox,
        number_col: indent + before_number,
    })
}

fn without_checkbox(label: &str) -> (&str, bool) {
    for word in CHECKBOX_WORDS {
        if let Some(rest) = label.strip_prefix(word) {
            return (rest.trim_start(), true);
        }
    }
    if let Some(first) = label.chars().next()
        && CHECKBOX_GLYPHS.contains(&first)
    {
        return (label[first.len_utf8()..].trim_start(), true);
    }
    (label, false)
}

fn words_of_label(label: &str) -> String {
    label.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A row of the question's words: padding squeezed, and the glyphs a TUI puts
/// in front of a line — a spinner, a bullet, a header chip's checkbox — off.
fn words_of_line(text: &str) -> String {
    words_of_label(text)
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .to_string()
}

// ---- finding the menu ----------------------------------------------------

/// The last option row on the screen, if no more than [`FOOTER_ROWS_MAX`]
/// rows of text stand under it.
fn last_option(rows: &[Row]) -> Option<usize> {
    let mut text_below = 0;
    for (at, row) in rows.iter().enumerate().rev() {
        match row.kind {
            Kind::Option(_) => return Some(at),
            Kind::Text => {
                text_below += 1;
                if text_below > FOOTER_ROWS_MAX {
                    return None;
                }
            }
            Kind::Blank | Kind::Rule => {}
        }
    }
    None
}

fn number_of(row: &Row) -> Option<u32> {
    match &row.kind {
        Kind::Option(numbered) => Some(numbered.number),
        _ => None,
    }
}

/// The option rows of the menu that ends at `last`, top first: the numbered
/// rows reached upward with no more than [`OPTION_GAP_ROWS_MAX`] other rows
/// between them, from the last one numbered 1 on, each one more than the one
/// before. Anything else is output that happens to carry numbers.
fn option_group(rows: &[Row], last: usize) -> Option<Vec<usize>> {
    let mut found = vec![last];
    let mut gap = 0;
    for (at, row) in rows[..last].iter().enumerate().rev() {
        if matches!(row.kind, Kind::Option(_)) {
            found.push(at);
            gap = 0;
        } else {
            gap += 1;
            if gap > OPTION_GAP_ROWS_MAX {
                break;
            }
        }
    }
    found.reverse();
    let start = found
        .iter()
        .rposition(|&at| number_of(&rows[at]) == Some(1))?;
    let group = found.split_off(start);
    let sequential = group
        .iter()
        .enumerate()
        .all(|(k, &at)| number_of(&rows[at]) == u32::try_from(k + 1).ok());
    sequential.then_some(group)
}

/// Which option the selection is on: the one wearing the glyph, or the one
/// the terminal's cursor stands on. A menu with neither is a list.
fn selection(rows: &[Row], group: &[usize], cursor_row: Option<usize>) -> Option<usize> {
    group
        .iter()
        .position(|&at| matches!(&rows[at].kind, Kind::Option(numbered) if numbered.marked))
        .or_else(|| cursor_row.and_then(|row| group.iter().position(|&at| at == row)))
}

/// The rows indented under an option, up to the next option: its description,
/// or the rest of a label the pane wrapped.
fn detail_below(rows: &[Row], at: usize, next: Option<usize>, number_col: usize) -> String {
    let end = next.unwrap_or(rows.len());
    let lines: Vec<&str> = rows[at + 1..end]
        .iter()
        .take_while(|row| matches!(row.kind, Kind::Text) && row.indent > number_col)
        .map(|row| row.text.as_str())
        .collect();
    words_of_label(&lines.join(" "))
}

/// What is being asked: the text rows above the first option, up to a rule or
/// another list, nearest the options last.
fn question_above(rows: &[Row], first: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for row in rows[..first].iter().rev().take(QUESTION_SCAN_ROWS_MAX) {
        match row.kind {
            Kind::Blank => {}
            Kind::Rule | Kind::Option(_) => break,
            Kind::Text => {
                lines.push(words_of_line(&row.text));
                if lines.len() == QUESTION_ROWS_MAX {
                    break;
                }
            }
        }
    }
    lines.reverse();
    lines.retain(|line| !line.is_empty());
    lines
}

// ---- is it still the question -------------------------------------------

/// Whether the menu's rows are the rows the question declared, one for one,
/// from the top. The menu may have more.
fn options_agree(shown: &[MenuOption], declared: &[AskOption]) -> bool {
    declared.len() <= shown.len()
        && declared
            .iter()
            .zip(shown)
            .all(|(want, row)| label_agrees(row, &want.label))
}

/// Whether a row is the label an agent said, once the way a pane spells it is
/// allowed for: a hint in brackets after it, a cut-off with an ellipsis, a
/// wrap onto the next row.
fn label_agrees(row: &MenuOption, said: &str) -> bool {
    let wanted = comparable(without_hint(said));
    if wanted.is_empty() {
        return true;
    }
    let shown = comparable(without_hint(&row.label));
    if shown == wanted {
        return true;
    }
    let truncated = row.label.ends_with('…') || row.label.ends_with("...");
    if truncated && shown.chars().count() >= TRUNCATED_LABEL_MIN_CHARS && wanted.starts_with(&shown)
    {
        return true;
    }
    let wrapped = format!("{shown}{}", comparable(&row.detail));
    wanted.starts_with(&shown) && wrapped.starts_with(&wanted)
}

/// The label without the bracketed hints a TUI hangs after it — `(esc)`,
/// `(y)`, `[recommended]`. A label that IS a bracket keeps it.
fn without_hint(label: &str) -> &str {
    let mut rest = label.trim_end();
    while let Some(open) = hint_start(rest) {
        rest = rest[..open].trim_end();
    }
    rest
}

fn hint_start(text: &str) -> Option<usize> {
    let open = match text.chars().next_back()? {
        ')' => '(',
        ']' => '[',
        _ => return None,
    };
    text.rfind(open).filter(|&at| at > 0)
}

/// Whether the end of what the card asked is among the words above the
/// options. An empty question has nothing to disagree with.
fn words_agree(region: &str, asked: &str) -> bool {
    let wanted = comparable(asked);
    wanted.is_empty() || comparable(region).contains(tail_chars(&wanted, QUESTION_TAIL_CHARS))
}

fn tail_chars(text: &str, count: usize) -> &str {
    let skip = text.chars().count().saturating_sub(count);
    text.char_indices()
        .nth(skip)
        .map_or("", |(at, _)| &text[at..])
}

/// The words of a line as a person compares them: counters that move by
/// themselves taken out, then letters and digits only, in lower case.
fn comparable(text: &str) -> String {
    masked(text)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// `text` with every clock and every number-with-a-unit made a space.
fn masked(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < chars.len() {
        let starts_number =
            chars[at].is_ascii_digit() && (at == 0 || !chars[at - 1].is_alphanumeric());
        if starts_number
            && let Some(end) = clock_end(&chars, at).or_else(|| quantity_end(&chars, at))
        {
            out.push(' ');
            at = end;
            continue;
        }
        out.push(chars[at]);
        at += 1;
    }
    out
}

fn digits_end(chars: &[char], from: usize) -> usize {
    from + chars[from.min(chars.len())..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count()
}

/// `12:07` and `1:02:03`, where the token starts at `at`.
fn clock_end(chars: &[char], at: usize) -> Option<usize> {
    let hours = digits_end(chars, at);
    if hours - at > 2 || chars.get(hours) != Some(&':') {
        return None;
    }
    let minutes = digits_end(chars, hours + 1);
    if minutes - (hours + 1) != 2 {
        return None;
    }
    if chars.get(minutes) == Some(&':') {
        let seconds = digits_end(chars, minutes + 1);
        if seconds - (minutes + 1) == 2 {
            return Some(seconds);
        }
    }
    Some(minutes)
}

/// `12s`, `3 m`, `1.2k`, `50%`, where the token starts at `at`: a number, at
/// most one space, a unit from [`VOLATILE_UNITS`] that is not the start of a
/// longer word.
fn quantity_end(chars: &[char], at: usize) -> Option<usize> {
    let mut end = digits_end(chars, at);
    if chars.get(end) == Some(&'.') {
        let fraction = digits_end(chars, end + 1);
        if fraction > end + 1 {
            end = fraction;
        }
    }
    let unit_at = end + usize::from(chars.get(end) == Some(&' '));
    let unit_end = unit_at
        + chars[unit_at.min(chars.len())..]
            .iter()
            .take_while(|c| c.is_alphabetic() || **c == '%')
            .count();
    let unit: String = chars[unit_at.min(chars.len())..unit_end].iter().collect();
    let bounded = chars.get(unit_end).is_none_or(|c| !c.is_alphanumeric());
    (VOLATILE_UNITS.contains(&unit.as_str()) && bounded).then_some(unit_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(screen: &str) -> Vec<String> {
        screen.lines().map(str::to_string).collect()
    }

    fn menu_of(screen: &str) -> Option<ScreenMenu> {
        read_menu(&rows(screen), None)
    }

    /// The menu on `screen`, asserted to be there — so a reading that finds
    /// nothing fails at this assertion, with the screen, and not at an unwrap.
    fn read(screen: &str) -> ScreenMenu {
        let menu = menu_of(screen);
        assert!(menu.is_some(), "no menu was read from:\n{screen}");
        menu.expect("asserted above")
    }

    /// The card a menu is drawn as, asserted to exist.
    fn card_of(menu: &ScreenMenu) -> AskPrompt {
        let card = menu.card();
        assert!(card.is_some(), "no card was drawn for {menu:?}");
        card.expect("asserted above")
    }

    fn labels(menu: &Option<ScreenMenu>) -> Vec<String> {
        menu.as_ref()
            .map(|menu| menu.options.iter().map(|o| o.label.clone()).collect())
            .unwrap_or_default()
    }

    fn selected(menu: &Option<ScreenMenu>) -> Option<usize> {
        menu.as_ref().map(|menu| menu.selected)
    }

    /// A permission prompt as a coding agent's TUI draws it: a body, the
    /// question, three rows with the selection glyph on the first.
    const PERMISSION: &str = "\
 Bash command

   make build
   Build the project

 Do you want to proceed?
 ❯ 1. Yes
   2. Yes, and don't ask again for similar commands
   3. No, and tell the agent what to do differently (esc)
";

    /// A form question with descriptions under its rows, an agent's own
    /// extra rows, a rule before the last of them and a hint line.
    const FORM: &str = "\
────────────────────────────────────────
 ☐ Library

Which library should we use for dates?

❯ 1. date-fns
     Modern and tree-shakable
  2. dayjs
     Small and familiar
  3. Type something.
────────────────────────────────────────
  4. Chat about this

Enter to select · ↑/↓ to navigate · Esc to cancel
";

    /// The same permission prompt inside a drawn box.
    const BOXED: &str = "\
╭──────────────────────────────────────────╮
│ Do you want to make this edit?           │
│ ❯ 1. Yes                                 │
│   2. No                                  │
╰──────────────────────────────────────────╯
";

    /// A third CLI's menu: `>` for the selection, `N)` for the number.
    const PLAIN: &str = "\
 Select an approach
 > 1) Rebase onto main
   2) Merge main into this branch
   3) Cancel
";

    fn question(text: &str, options: &[&str]) -> AskQuestion {
        AskQuestion {
            question: text.to_string(),
            header: None,
            multi_select: false,
            options: options
                .iter()
                .map(|label| AskOption {
                    label: (*label).to_string(),
                    description: None,
                })
                .collect(),
        }
    }

    // ---- reading -----------------------------------------------------

    /// The rows, their numbers, which one the selection is on and the words
    /// above them — for a prompt whose body is several paragraphs.
    #[test]
    fn a_numbered_menu_with_the_selection_on_a_row_reads_as_a_menu() {
        let menu = menu_of(PERMISSION);
        assert_eq!(
            labels(&menu),
            vec![
                "Yes",
                "Yes, and don't ask again for similar commands",
                "No, and tell the agent what to do differently (esc)",
            ]
        );
        assert_eq!(selected(&menu), Some(0));
        assert_eq!(
            menu.map(|menu| menu.question),
            Some(vec![
                "Bash command".to_string(),
                "make build".to_string(),
                "Build the project".to_string(),
                "Do you want to proceed?".to_string(),
            ])
        );
    }

    /// Both ways a number is spelled and every glyph a selection is drawn
    /// with: the generic reading is not one CLI's.
    #[test]
    fn every_number_separator_and_selection_glyph_reads() {
        assert_eq!(labels(&menu_of(PLAIN)).len(), 3);
        assert_eq!(selected(&menu_of(PLAIN)), Some(0));
        for glyph in ["❯", "›", ">", "→", "➜", "▶", "▸", "➤"] {
            let screen = format!("Pick\n  1. Alpha\n{glyph} 2. Beta\n  3. Gamma\n");
            assert_eq!(
                selected(&menu_of(&screen)),
                Some(1),
                "the glyph {glyph} did not mark a selection"
            );
        }
    }

    /// A menu drawn inside a box reads through the border, and the box's own
    /// top and bottom edges are neither rows nor words.
    #[test]
    fn a_menu_inside_a_box_reads_through_its_border() {
        let menu = menu_of(BOXED);
        assert_eq!(labels(&menu), vec!["Yes", "No"]);
        assert_eq!(selected(&menu), Some(0));
        assert_eq!(
            menu.map(|menu| menu.question),
            Some(vec!["Do you want to make this edit?".to_string()])
        );
    }

    /// A description is the rows indented under its option; a rule between
    /// two options does not split the menu; the hint line under it is no
    /// option's detail; and the header chip's checkbox glyph is not part of
    /// the question.
    #[test]
    fn descriptions_ride_with_their_option_and_a_rule_does_not_split_the_menu() {
        let menu = menu_of(FORM);
        assert_eq!(
            labels(&menu),
            vec!["date-fns", "dayjs", "Type something.", "Chat about this"]
        );
        let details: Vec<String> = menu
            .as_ref()
            .map(|menu| menu.options.iter().map(|o| o.detail.clone()).collect())
            .unwrap_or_default();
        assert_eq!(
            details,
            vec!["Modern and tree-shakable", "Small and familiar", "", ""]
        );
        assert_eq!(
            menu.map(|menu| menu.question),
            Some(vec![
                "Library".to_string(),
                "Which library should we use for dates?".to_string(),
            ])
        );
    }

    /// A menu with no glyph on any row still has a selection when the
    /// terminal's own cursor stands on one of its rows.
    #[test]
    fn a_cursor_on_a_numbered_row_stands_for_the_selection_glyph() {
        let screen = rows("Pick one\n  1. Alpha\n  2. Beta\n  3. Gamma\n");
        assert_eq!(selected(&read_menu(&screen, Some(2))), Some(1));
        assert_eq!(read_menu(&screen, Some(0)), None);
        assert_eq!(read_menu(&screen, None), None);
    }

    /// What a numbered list in ordinary output looks like next to what a
    /// menu looks like: no selection on a row, rows far above the bottom,
    /// numbers that do not start at one, a single row.
    #[test]
    fn a_numbered_list_in_ordinary_output_is_not_a_menu() {
        let plan =
            "Here is the plan:\n1. Read the file\n2. Change the code\n3. Run the tests\n❯ \n";
        assert_eq!(menu_of(plan), None, "a list with no selection is output");
        let buried = format!(
            "Pick\n❯ 1. Alpha\n  2. Beta\n{}",
            "some later output line\n".repeat(7)
        );
        assert_eq!(menu_of(&buried), None, "an answered menu buried in output");
        assert_eq!(
            menu_of("Pick\n❯ 2. Beta\n  3. Gamma\n"),
            None,
            "must start at 1"
        );
        assert_eq!(
            menu_of("Pick\n❯ 1. Alpha\n  3. Gamma\n"),
            None,
            "must not skip"
        );
        assert_eq!(menu_of("Pick\n❯ 1. Alpha\n"), None, "one row is no menu");
    }

    /// A list of numbers earlier in the same paragraph does not make the
    /// menu start at its first row.
    #[test]
    fn an_earlier_list_above_the_menu_is_not_part_of_it() {
        let screen = "\
1. First thing mentioned
Some words in between
Which one do you want?
❯ 1. Yes
  2. No
";
        let menu = menu_of(screen);
        assert_eq!(labels(&menu), vec!["Yes", "No"]);
    }

    /// Checkboxes make a multi-select; the label is clean of them and the
    /// card is not offered, since one pick cannot answer it.
    #[test]
    fn checkboxes_make_a_multi_select_with_clean_labels_and_no_card() {
        let screen = "Which?\n❯ 1. [ ] Alpha\n  2. [x] Beta\n  3. [ ] Gamma\n";
        let menu = menu_of(screen);
        assert_eq!(labels(&menu), vec!["Alpha", "Beta", "Gamma"]);
        assert_eq!(menu.as_ref().map(ScreenMenu::multi_select), Some(true));
        assert_eq!(menu.and_then(|menu| menu.card()), None);
    }

    /// The card a read menu is drawn as: one single-select question, the
    /// words above the options as its question, a row each with its detail
    /// as the description.
    #[test]
    fn a_read_menu_is_drawn_as_one_single_select_question() {
        let card = menu_of(FORM).and_then(|menu| menu.card());
        let questions = card.map(|card| card.questions).unwrap_or_default();
        assert_eq!(questions.len(), 1);
        let only = &questions[0];
        assert_eq!(
            only.question,
            "Library\nWhich library should we use for dates?"
        );
        assert!(!only.multi_select);
        assert_eq!(only.options.len(), 4);
        assert_eq!(only.options[0].label, "date-fns");
        assert_eq!(
            only.options[0].description.as_deref(),
            Some("Modern and tree-shakable")
        );
        assert_eq!(only.options[2].description, None);
    }

    /// A dialog with a long body is drawn as a card of the rows nearest its
    /// options — the command and the ask — and a menu with fewer rows keeps
    /// all of them. What the card carries is still checked against the screen.
    #[test]
    fn a_card_carries_the_rows_nearest_the_options_of_a_long_question() {
        let long = "\
Allow this edit?
Path: src/lib.rs
Change: rename the helper
Reason: the old name is misleading
Do you want to apply it?
❯ 1. Yes
  2. No
";
        let menu = read(long);
        assert_eq!(menu.question.len(), 5);
        let card = card_of(&menu);
        assert_eq!(
            card.questions[0].question,
            "Change: rename the helper\nReason: the old name is misleading\nDo you want to apply it?"
        );
        assert!(menu.shows(&card.questions[0]));
        let short = card_of(&read(FORM));
        assert_eq!(
            short.questions[0].question,
            "Library\nWhich library should we use for dates?"
        );
    }

    /// The one screen this repository holds that was recorded off a real agent
    /// instead of written for a test: Claude Code's pause dialog, as a
    /// coordinator read it off a worker's pane (2.1.281, 2026-09-21 — the
    /// shell's `quota_wall` carries the same capture). The reading was written
    /// from known layouts; this is a layout the product actually drew.
    const RECORDED_PAUSE_DIALOG: &str = "\
 Session paused
 Fable 5.1's safeguards flagged this message. Our intentionally broad safeguards allow us to deliver more capabilities faster.
   Details: `[cyber]`
 ❯ 1. Switch to Opus 4.8
   2. Edit prompt and retry
";

    /// The same pane after the error was printed instead: no menu stands on it.
    const RECORDED_PRINTED_DECLINE: &str = "\
⎿  API Error: Fable 5.1's safeguards flagged this message (https://www.anthropic.com/legal/aup).
   Double press esc to edit your last message, or try a different model with /model.
❯ ";

    #[test]
    fn a_screen_recorded_off_a_real_agent_reads_as_the_menu_it_is() {
        let menu = read(RECORDED_PAUSE_DIALOG);
        assert_eq!(
            labels(&Some(menu.clone())),
            vec!["Switch to Opus 4.8", "Edit prompt and retry"]
        );
        assert_eq!(menu.selected, 0);
        assert_eq!(menu.question.len(), 3);
        assert_eq!(menu.question[0], "Session paused");
        assert!(menu.shows(&card_of(&menu).questions[0]));
        assert_eq!(menu_of(RECORDED_PRINTED_DECLINE), None);
    }

    // ---- is it still the question ---------------------------------------

    /// The card a menu was drawn from is shown by that menu.
    #[test]
    fn a_card_drawn_from_a_menu_is_shown_by_it() {
        let menu = read(PERMISSION);
        let card = card_of(&menu);
        assert!(menu.shows(&card.questions[0]));
    }

    /// An agent's own description of a question names only the options it
    /// declared; the screen adds rows of its own. Those match.
    #[test]
    fn a_declared_question_is_shown_by_a_menu_with_extra_rows() {
        let menu = read(FORM);
        let asked = question(
            "Which library should we use for dates?",
            &["date-fns", "dayjs"],
        );
        assert!(menu.shows(&asked));
    }

    /// The ways a question can have changed under a card: other words, other
    /// rows, rows in another order, more rows than the screen has.
    #[test]
    fn a_changed_question_is_not_shown() {
        let menu = read(FORM);
        for (why, changed) in [
            (
                "other words",
                question("Which framework should we use?", &["date-fns", "dayjs"]),
            ),
            (
                "reordered rows",
                question(
                    "Which library should we use for dates?",
                    &["dayjs", "date-fns"],
                ),
            ),
            (
                "a row the screen lacks",
                question(
                    "Which library should we use for dates?",
                    &["date-fns", "dayjs", "moment", "luxon", "temporal"],
                ),
            ),
            (
                "another row",
                question(
                    "Which library should we use for dates?",
                    &["date-fns", "moment"],
                ),
            ),
        ] {
            assert!(!menu.shows(&changed), "{why} was shown");
        }
        // Same options, but the command above them is another command.
        let build = read(PERMISSION);
        let test = read(&PERMISSION.replace("make build", "make test"));
        let card = card_of(&build);
        assert!(build.shows(&card.questions[0]));
        assert!(
            !test.shows(&card.questions[0]),
            "another command, same rows"
        );
    }

    /// How a screen spells a row differs from how an agent said it: the
    /// pane's width truncates, wraps and adds hints; case and punctuation
    /// are not words.
    #[test]
    fn a_label_the_screen_wraps_truncates_or_hints_is_still_the_label() {
        let wrapped = read(
            "Which?\n❯ 1. A very long option label that wraps\n       around to the next row\n  2. Short\n",
        );
        assert!(wrapped.shows(&question(
            "Which?",
            &[
                "A very long option label that wraps around to the next row",
                "Short"
            ]
        )));
        let truncated = read("Which?\n❯ 1. Yes, and don't ask again for simi…\n  2. No\n");
        assert!(truncated.shows(&question(
            "Which?",
            &["Yes, and don't ask again for similar commands", "No"]
        )));
        let hinted = read(PERMISSION);
        assert!(hinted.shows(&question(
            "Do you want to proceed?",
            &[
                "Yes",
                "Yes, and don't ask again for similar commands",
                "No, and tell the agent what to do differently",
            ]
        )));
        let cased = read(FORM);
        assert!(cased.shows(&question(
            "which LIBRARY should we use for dates",
            &["Date-FNS!", "DayJS"]
        )));
    }

    /// A counter that ticks inside the question is not the question
    /// changing; a number that is part of what is asked is.
    #[test]
    fn a_ticking_counter_in_the_question_is_not_a_change_but_a_count_is() {
        let before = "Waiting for your answer (12s)\nDelete 3 files?\n❯ 1. Yes\n  2. No\n";
        let ticked = "Waiting for your answer (13s)\nDelete 3 files?\n❯ 1. Yes\n  2. No\n";
        let counted = "Waiting for your answer (13s)\nDelete 5 files?\n❯ 1. Yes\n  2. No\n";
        let card = card_of(&read(before));
        assert!(read(ticked).shows(&card.questions[0]));
        assert!(!read(counted).shows(&card.questions[0]));
    }

    /// The set the "no false refusal" claim rests on: the same question on a
    /// screen that differs everywhere a menu does not — clocks and counters
    /// above it, a spinner, the selection on every row, trailing blanks, a
    /// different hint, the box around it. Every variant must still show it.
    #[test]
    fn nothing_that_happens_elsewhere_on_the_screen_refuses_a_good_answer() {
        let asked = question(
            "Which library should we use for dates?",
            &["date-fns", "dayjs"],
        );
        let mut refused = Vec::new();
        let mut variants = 0;
        let spinners = ["⠋", "⠙", "⠹", "⠸", "✻", "✽", "·", "*"];
        let hints = [
            "Enter to select · ↑/↓ to navigate · Esc to cancel",
            "Press enter to confirm or esc to cancel",
            "",
        ];
        for tick in 0..12_u32 {
            for selection in 0..4_usize {
                for (h, hint) in hints.iter().enumerate() {
                    let mut screen = String::new();
                    // Rows far above the dialog: output, a status line that
                    // ticks, a spinner line.
                    screen.push_str("● Ran the formatter and the tests\n");
                    screen.push_str(&format!(
                        "✻ Worked for {}m {}s · {}.{}k tokens\n",
                        tick,
                        7 + tick,
                        tick,
                        h
                    ));
                    screen.push_str(&format!(
                        "{} Thinking… ({}s)   {:02}:{:02}\n\n",
                        spinners[(tick as usize + h) % spinners.len()],
                        tick * 3,
                        9 + tick,
                        tick * 5
                    ));
                    screen.push_str("────────────────────────────────────────\n");
                    screen.push_str(" ☐ Library\n\nWhich library should we use for dates?\n\n");
                    let options = [
                        ("date-fns", "Modern and tree-shakable"),
                        ("dayjs", "Small and familiar"),
                        ("Type something.", ""),
                        ("Chat about this", ""),
                    ];
                    for (i, (label, detail)) in options.iter().enumerate() {
                        let glyph = if i == selection { "❯" } else { " " };
                        screen.push_str(&format!("{glyph} {}. {label}   \n", i + 1));
                        if !detail.is_empty() {
                            screen.push_str(&format!("     {detail}\n"));
                        }
                        if i == 2 {
                            screen.push_str("────────────────────────────────────────\n");
                        }
                    }
                    screen.push_str(&format!("\n{hint}\n"));
                    variants += 1;
                    let shown = menu_of(&screen).is_some_and(|menu| menu.shows(&asked));
                    if !shown {
                        refused.push((tick, selection, h));
                    }
                }
            }
        }
        assert_eq!(variants, 144);
        assert_eq!(refused, Vec::<(u32, usize, usize)>::new(), "false refusals");
    }

    /// MEASUREMENT (ignored): what reading one screen costs, in the cases the
    /// board and the door meet — a menu at the bottom of a 40-row screen, the
    /// same screen with no menu (the common case for a pane that waits on
    /// something else), and the question check. Run on purpose, normally and
    /// under `taskpolicy -b`:
    /// `cargo test -p zerocode-core --lib screen_menu::tests::measure -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, run on purpose: -- --ignored --nocapture"]
    fn measure_the_reading_a_screen_costs() {
        use std::time::Instant;
        const ITERATIONS: u32 = 100_000;
        let output =
            "some earlier output line, as wide as an agent's answer usually runs on a real pane
"
            .repeat(34);
        let with_menu = rows(&format!("{output}{FORM}"));
        let without_menu = rows(&format!(
            "{output}





"
        ));
        let asked = question(
            "Which library should we use for dates?",
            &["date-fns", "dayjs"],
        );
        let menu = read_menu(&with_menu, None).expect("a menu");
        let per_call = |what: &str, work: &dyn Fn() -> bool| {
            let began = Instant::now();
            let mut hits = 0_u32;
            for _ in 0..ITERATIONS {
                hits += u32::from(work());
            }
            let nanos = began.elapsed().as_nanos() as f64 / f64::from(ITERATIONS);
            println!("MEASURE {what}: {nanos:.0} ns/call ({hits} hits of {ITERATIONS})");
        };
        per_call("read_menu, menu at the bottom of 40+ rows", &|| {
            read_menu(&with_menu, None).is_some()
        });
        per_call("read_menu, no menu on 40 rows", &|| {
            read_menu(&without_menu, None).is_some()
        });
        per_call("ScreenMenu::shows", &|| menu.shows(&asked));
    }

    /// The same question, with no row to compare: its words are on the last
    /// rows or they are not.
    #[test]
    fn a_question_with_no_rows_is_shown_by_its_words_on_the_last_rows() {
        let screen = rows("old output\nmore output\nWhat should the branch be called?\n> \n");
        assert!(words_are_up(&screen, "What should the branch be called?"));
        assert!(!words_are_up(&screen, "What should the tag be called?"));
        let buried = rows(&format!(
            "What should the branch be called?\n{}",
            "later output\n".repeat(30)
        ));
        assert!(!words_are_up(&buried, "What should the branch be called?"));
    }
}
