//! A range of a skill's text, asked in the `Skill` call's own `skill` string
//! (t-116290): `computer-use#L476-L1070` is lines 476 to 1,070 of the skill,
//! `computer-use#L7C46001-L7` line 7 from its 46,001st char.
//!
//! The `Skill` wire schema takes `skill` and `args` and nothing else, and it
//! stays that way: declaring `offset` and `limit` beside them took the tool
//! schemas every turn carries from 2,135 tokens to 2,189 against the r49
//! budget of 2,136. A provider that holds a model to the schema refuses a
//! call carrying either, so the call for the lines a shortened skill left
//! out names them in the name.
//!
//! The range follows a `#`, the mark of a part of a page in a URL and the
//! form a code host links lines with. A skill's name is its folder's name,
//! matched whole and ignoring case (`SkillCatalog::resolve`); `/` and `$` in
//! front of it are trimmed as the prefixes of a slash command and of Codex;
//! other harnesses name a plugin's skill `plugin:skill` and a folder-scoped
//! one `apps/web:deploy`; and a skill's name is lowercase letters, digits and
//! hyphens by the agent-skills rule and by zo's own slug rule — every one of
//! the 77 skill folders on the machine this was written on. A range is read
//! off the end only when it is exactly this form, and the loader looks the
//! whole name up first, so a folder whose name happened to end that way
//! would still load by it.
//!
//! [`SkillRange::call`] writes the call — on a shortened skill's first line
//! and on an answer that ran out of room — and [`SkillRange::split`] reads
//! it back: one form, written and read in one place.

use crate::SKILL_TOOL;

/// Where a range of a skill starts — a line, and a char of that line, each
/// counted from 1 — and the line it ends with, whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkillRange {
    /// The first line, counted from 1.
    pub line: usize,
    /// The char of `line` the range starts at, counted from 1: the line's
    /// first is 1, and its end of line the one after its last.
    pub column: usize,
    /// The last line, counted from 1.
    pub last_line: usize,
}

impl SkillRange {
    /// Lines `first` through `last`, whole.
    #[must_use]
    pub fn lines(first: usize, last: usize) -> Self {
        Self { line: first, column: 1, last_line: last }
    }

    /// `skill` split into the name in front of its range and the range,
    /// when it ends in one: `<name>#L<line>[C<char>][-L<line>]`, where a
    /// range with no end is its first line alone and 0 counts as 1, the way
    /// `offset` counts it. `None` when `skill` ends in no range or nothing
    /// stands in front of it.
    #[must_use]
    pub fn split(skill: &str) -> Option<(&str, Self)> {
        let (name, range) = skill.rsplit_once('#')?;
        let (start, last) = match range.split_once('-') {
            Some((start, last)) => (start, Some(last.strip_prefix('L').unwrap_or(last))),
            None => (range, None),
        };
        let start = start.strip_prefix('L')?;
        let (line, column) = match start.split_once('C') {
            Some((line, column)) => (count(line)?, count(column)?),
            None => (count(start)?, 1),
        };
        let last_line = match last {
            Some(last) => count(last)?,
            None => line,
        };
        (!name.trim().is_empty()).then_some((name, Self { line, column, last_line }))
    }

    /// The `Skill` call that hands this range of `name` back, as a note or
    /// an answer names it: `` `Skill` with skill=<name>#L476-L1070 ``.
    #[must_use]
    pub fn call(self, name: &str) -> String {
        let column = if self.column > 1 { format!("C{}", self.column) } else { String::new() };
        format!("`{SKILL_TOOL}` with skill={name}#L{}{column}-L{}", self.line, self.last_line)
    }
}

/// A count written in digits, 0 read as 1.
fn count(digits: &str) -> Option<usize> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse::<usize>().ok().map(|number| number.max(1))
}

#[cfg(test)]
mod tests {
    use super::SkillRange;

    #[test]
    fn a_range_is_read_off_the_end_of_a_name_in_the_one_form_the_call_writes() {
        let cases = [
            ("computer-use#L476-L1070", "computer-use", SkillRange::lines(476, 1_070)),
            ("zqgiant#L7C46001-L7", "zqgiant", SkillRange { line: 7, column: 46_001, last_line: 7 }),
            ("zqgiant#L7", "zqgiant", SkillRange::lines(7, 7)),
            ("computer-use#L476-1070", "computer-use", SkillRange::lines(476, 1_070)),
            ("$computer-use#L0-L3", "$computer-use", SkillRange::lines(1, 3)),
            ("plugin:skill#L5-L9", "plugin:skill", SkillRange::lines(5, 9)),
            ("apps/web:deploy#L5-L9", "apps/web:deploy", SkillRange::lines(5, 9)),
            ("odd#name#L2-L3", "odd#name", SkillRange::lines(2, 3)),
        ];
        for (asked, name, range) in cases {
            assert_eq!(SkillRange::split(asked), Some((name, range)), "{asked}");
        }
        for range in [SkillRange::lines(476, 1_070), SkillRange { line: 7, column: 46_001, last_line: 9 }] {
            let call = range.call("computer-use");
            let asked = call.strip_prefix("`Skill` with skill=").unwrap_or_else(|| panic!("{call}"));
            assert_eq!(SkillRange::split(asked), Some(("computer-use", range)), "{call}");
        }
        assert_eq!(SkillRange::lines(476, 1_070).call("x"), "`Skill` with skill=x#L476-L1070");
        assert_eq!(SkillRange { line: 7, column: 46_001, last_line: 7 }.call("x"), "`Skill` with skill=x#L7C46001-L7");
    }

    #[test]
    fn a_name_that_ends_in_no_range_is_a_name() {
        for asked in [
            "computer-use",
            "computer-use#",
            "computer-use#476-1070",
            "computer-use#L",
            "computer-use#L476-",
            "computer-use#L476-L",
            "computer-use#L4x6",
            "computer-use#l476",
            "computer-use#L7C",
            "computer-use#L7-L9C3",
            "computer-use#L476 ",
            "computer-use#L99999999999999999999999",
            "#L5-L9",
            "  #L5",
        ] {
            assert_eq!(SkillRange::split(asked), None, "{asked}");
        }
    }
}
