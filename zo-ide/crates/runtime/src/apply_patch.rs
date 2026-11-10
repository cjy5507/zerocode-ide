//! Codex's `apply_patch` envelope: parsing it, and computing what each file
//! becomes. Nothing here touches the filesystem; the tool layer
//! (`tools::file_tools::run_apply_patch`) resolves paths, asks permission and
//! writes.
//!
//! The grammar is codex-cli's, so a model trained on it writes patches this
//! accepts unchanged:
//!
//! ```text
//! *** Begin Patch
//! *** Add File: <path>          followed by `+` lines
//! *** Delete File: <path>
//! *** Update File: <path>
//! *** Move to: <path>           optional, right after Update File
//! @@ <anchor line>              optional on the first hunk
//!  context / -removed / +added  hunk lines
//! *** End of File               optional: the hunk ends the file
//! *** End Patch
//! ```
//!
//! Matching tolerates what a model gets wrong without changing what it meant:
//! trailing whitespace, then surrounding whitespace, then typographic
//! punctuation (`–` for `-`, `“` for `"`), in that order, first exact match
//! wins. A hunk that still matches nothing fails the whole patch.

const BEGIN_PATCH: &str = "*** Begin Patch";
const END_PATCH: &str = "*** End Patch";
const ADD_FILE: &str = "*** Add File: ";
const DELETE_FILE: &str = "*** Delete File: ";
const UPDATE_FILE: &str = "*** Update File: ";
const MOVE_TO: &str = "*** Move to: ";
const END_OF_FILE: &str = "*** End of File";
const HUNK_HEADER: &str = "@@";
/// Every operation header starts with this; a hunk ends where one begins.
const HEADER_PREFIX: &str = "*** ";

/// One file operation of a patch. Paths and lines borrow the patch text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchOperation<'a> {
    Add {
        path: &'a str,
        lines: Vec<&'a str>,
    },
    Delete {
        path: &'a str,
    },
    Update {
        path: &'a str,
        move_to: Option<&'a str>,
        hunks: Vec<PatchHunk<'a>>,
    },
}

impl<'a> PatchOperation<'a> {
    /// The path the operation names first (an update's source).
    #[must_use]
    pub const fn path(&self) -> &'a str {
        match self {
            Self::Add { path, .. } | Self::Delete { path } | Self::Update { path, .. } => path,
        }
    }
}

/// One hunk of an update: the lines it expects and the lines it leaves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchHunk<'a> {
    /// The `@@ <line>` anchor: the hunk is looked for after this line.
    pub anchor: Option<&'a str>,
    pub old_lines: Vec<&'a str>,
    pub new_lines: Vec<&'a str>,
    /// `*** End of File`: the hunk must sit at the end of the file.
    pub at_end_of_file: bool,
}

/// Why a patch was refused. Either way nothing has been written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatchError {
    /// The text does not follow the envelope grammar.
    #[error("invalid patch (line {line}): {reason}")]
    Malformed { line: usize, reason: String },
    /// An update's anchor or expected lines are not in the file.
    #[error("{path}: these lines are not in the file:\n{wanted}")]
    NotFound { path: String, wanted: String },
}

/// Parse a whole patch envelope.
///
/// # Errors
/// [`PatchError::Malformed`] naming the first line that breaks the grammar.
pub fn parse_patch(text: &str) -> Result<Vec<PatchOperation<'_>>, PatchError> {
    let lines = envelope_body(text)?;
    let mut operations = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim_end();
        // Line numbers count from `*** Begin Patch` as line 1.
        let line_number = index + 2;
        if line.is_empty() {
            index += 1;
        } else if let Some(path) = line.strip_prefix(ADD_FILE) {
            index += 1;
            let start = index;
            while index < lines.len() && lines[index].starts_with('+') {
                index += 1;
            }
            operations.push(PatchOperation::Add {
                path: header_path(path, line_number)?,
                lines: lines[start..index].iter().map(|added| &added[1..]).collect(),
            });
        } else if let Some(path) = line.strip_prefix(DELETE_FILE) {
            index += 1;
            operations.push(PatchOperation::Delete {
                path: header_path(path, line_number)?,
            });
        } else if let Some(path) = line.strip_prefix(UPDATE_FILE) {
            let path = header_path(path, line_number)?;
            index += 1;
            let move_to = match lines.get(index).and_then(|next| next.trim_end().strip_prefix(MOVE_TO)) {
                Some(destination) => {
                    index += 1;
                    Some(header_path(destination, index + 1)?)
                }
                None => None,
            };
            let hunks = parse_hunks(&lines, &mut index, path)?;
            operations.push(PatchOperation::Update { path, move_to, hunks });
        } else {
            return Err(malformed(
                line_number,
                format!("expected `{ADD_FILE}`, `{DELETE_FILE}` or `{UPDATE_FILE}`, found `{line}`"),
            ));
        }
    }
    if operations.is_empty() {
        return Err(malformed(1, "the patch names no file".to_string()));
    }
    Ok(operations)
}

/// The lines between `*** Begin Patch` and `*** End Patch`. A shell heredoc
/// around the envelope (`<<'EOF'` … `EOF`) is tolerated: models trained to
/// call the tool through a shell sometimes keep it.
fn envelope_body(text: &str) -> Result<Vec<&str>, PatchError> {
    let mut lines: Vec<&str> = text.trim().lines().collect();
    if lines.first().is_some_and(|first| first.trim_start().starts_with("<<"))
        && lines.last().is_some_and(|last| last.trim() == "EOF")
    {
        lines.remove(0);
        lines.pop();
    }
    if lines.first().map(|first| first.trim()) != Some(BEGIN_PATCH) {
        return Err(malformed(1, format!("the patch must start with `{BEGIN_PATCH}`")));
    }
    if lines.len() < 2 || lines.last().map(|last| last.trim()) != Some(END_PATCH) {
        return Err(malformed(lines.len(), format!("the patch must end with `{END_PATCH}`")));
    }
    lines.pop();
    lines.remove(0);
    Ok(lines)
}

/// The hunks of one `*** Update File`, advancing `index` past them.
fn parse_hunks<'a>(
    lines: &[&'a str],
    index: &mut usize,
    path: &str,
) -> Result<Vec<PatchHunk<'a>>, PatchError> {
    let mut hunks = Vec::new();
    while *index < lines.len() {
        let opening = lines[*index];
        if opening.starts_with(HEADER_PREFIX) && opening.trim_end() != END_OF_FILE {
            break;
        }
        let mut hunk = PatchHunk::default();
        if let Some(anchor) = opening.strip_prefix(HUNK_HEADER) {
            let anchor = anchor.strip_prefix(' ').unwrap_or(anchor);
            hunk.anchor = (!anchor.trim().is_empty()).then_some(anchor);
            *index += 1;
        } else if !hunks.is_empty() {
            return Err(malformed(
                *index + 2,
                format!("a hunk after the first in `{path}` must open with `{HUNK_HEADER}`"),
            ));
        }
        while *index < lines.len() {
            let line = lines[*index];
            if line.trim_end() == END_OF_FILE {
                hunk.at_end_of_file = true;
                *index += 1;
                break;
            }
            if line.starts_with(HUNK_HEADER) || line.starts_with(HEADER_PREFIX) {
                break;
            }
            match line.as_bytes().first() {
                Some(b' ') => {
                    hunk.old_lines.push(&line[1..]);
                    hunk.new_lines.push(&line[1..]);
                }
                Some(b'-') => hunk.old_lines.push(&line[1..]),
                Some(b'+') => hunk.new_lines.push(&line[1..]),
                // A context line whose single space an editor stripped.
                None => {
                    hunk.old_lines.push("");
                    hunk.new_lines.push("");
                }
                Some(_) => {
                    return Err(malformed(
                        *index + 2,
                        format!("a hunk line must start with ' ', '-' or '+', found `{line}`"),
                    ))
                }
            }
            *index += 1;
        }
        if hunk.old_lines.is_empty() && hunk.new_lines.is_empty() {
            return Err(malformed(*index + 1, format!("an empty hunk in `{path}`")));
        }
        hunks.push(hunk);
    }
    if hunks.is_empty() {
        return Err(malformed(*index + 1, format!("`{UPDATE_FILE}{path}` has no hunks")));
    }
    Ok(hunks)
}

fn header_path(raw: &str, line: usize) -> Result<&str, PatchError> {
    let path = raw.trim();
    if path.is_empty() {
        return Err(malformed(line, "a file header names no path".to_string()));
    }
    Ok(path)
}

fn malformed(line: usize, reason: String) -> PatchError {
    PatchError::Malformed { line, reason }
}

/// The text a new file gets from an `*** Add File` block: each line ends with
/// a newline, as the envelope wrote it.
#[must_use]
pub fn added_file_contents(lines: &[&str]) -> String {
    let mut contents = String::with_capacity(lines.iter().map(|line| line.len() + 1).sum());
    for line in lines {
        contents.push_str(line);
        contents.push('\n');
    }
    contents
}

/// What `original` becomes after `hunks`, keeping its line ending (`\r\n` or
/// `\n`) and ending with one. `path` only labels a failure.
///
/// # Errors
/// [`PatchError::NotFound`] for the first anchor or hunk whose lines are not
/// in the file after the previous hunk.
pub fn apply_hunks(original: &str, hunks: &[PatchHunk<'_>], path: &str) -> Result<String, PatchError> {
    let ending = if original.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<&str> = original
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    // `split` leaves an empty last piece after the final newline; `diff`
    // counts lines without it, and so do the hunks.
    if lines.last() == Some(&"") {
        lines.pop();
    }
    let replacements = plan_replacements(&lines, hunks, path)?;

    let replaced: usize = replacements.iter().map(|change| change.old_len).sum();
    let inserted: usize = replacements.iter().map(|change| change.new_lines.len()).sum();
    let kept_lines = lines.len() - replaced + inserted;
    let mut updated = String::with_capacity(original.len() + inserted * 32 + kept_lines * ending.len());
    let mut next = 0;
    for change in &replacements {
        for line in &lines[next..change.start] {
            push_line(&mut updated, line, ending);
        }
        for line in change.new_lines {
            push_line(&mut updated, line, ending);
        }
        next = change.start + change.old_len;
    }
    for line in &lines[next..] {
        push_line(&mut updated, line, ending);
    }
    Ok(updated)
}

fn push_line(buffer: &mut String, line: &str, ending: &str) {
    buffer.push_str(line);
    buffer.push_str(ending);
}

/// Lines of the file a refusal shows, at most.
const CLOSEST_SHOWN_LINES: usize = 16;
/// How many lines later than its own place a line of the hunk may stand and
/// still count with it — lines the file holds between the hunk's.
const CLOSEST_SLACK_LINES: usize = 2;
/// The longest file searched, and the most (hunk line, file line) matches
/// weighed: a file of one line repeated is not worth the search, and the
/// refusal stays as it was.
const CLOSEST_FILE_LINES: usize = 100_000;
const CLOSEST_WORK_LIMIT: usize = 200_000;

/// Where in `original` the lines of `wanted` come closest to standing: one
/// sentence naming the place, then the lines the file holds there — for a
/// refusal to end with.
///
/// Lines that are not in the file are usually nearly in it: one of them
/// changed since the model last saw the file, or was misremembered. Told only
/// that its lines "are not in the file", a model reads the file and writes
/// the change again — two requests where the refusal could have been the
/// read. Seven days on one machine to 2026-11-10: 17 of 29 failed
/// `apply_patch` calls were this, and 4 of 41 failed `edit_file` calls.
///
/// Every line of the hunk that stands somewhere in the file, under the
/// loosest tolerance, says where the hunk would have to begin for that to be
/// so; the place most lines agree on is the closest. It counts when at least
/// half of the lines (blank ones aside) are there — a place that shares one
/// brace with the hunk says nothing and is not shown. One pass over the file
/// and one over the hunk; `None` also for a file or a repetition too large to
/// be worth it.
#[must_use]
pub fn closest_place(original: &str, wanted: &str) -> Option<String> {
    use std::collections::{BTreeMap, HashMap};
    use std::fmt::Write as _;

    let lines: Vec<&str> = original.lines().collect();
    let wanted: Vec<&str> = wanted.lines().collect();
    let asked = wanted.iter().filter(|line| !line.trim().is_empty()).count();
    if asked == 0 || lines.len() > CLOSEST_FILE_LINES {
        return None;
    }
    let mut stands: HashMap<String, Vec<usize>> = HashMap::new();
    for (at, line) in lines.iter().enumerate() {
        if !line.trim().is_empty() {
            stands.entry(loosely(line)).or_default().push(at);
        }
    }
    // `begins` is where the hunk would begin, kept `wanted.len()` high so a
    // hunk that would begin above the file's first line still has a number;
    // each holds the lines of the hunk that say so.
    let mut begins: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut weighed = 0;
    for (index, line) in wanted.iter().enumerate() {
        let Some(found_at) = stands.get(&loosely(line)) else {
            continue;
        };
        weighed += found_at.len();
        if weighed > CLOSEST_WORK_LIMIT {
            return None;
        }
        for at in found_at {
            begins.entry(at + wanted.len() - index).or_default().push(index);
        }
    }
    let (begin, found, reach) = begins
        .keys()
        .map(|&begin| {
            let mut there: Vec<usize> = Vec::new();
            let mut reach = 0;
            for later in 0..=CLOSEST_SLACK_LINES {
                if let Some(indexes) = begins.get(&(begin + later)) {
                    there.extend(indexes);
                    reach = later;
                }
            }
            there.sort_unstable();
            there.dedup();
            (begin, there.len(), reach)
        })
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))?;
    if found * 2 < asked {
        return None;
    }
    let first = begin.saturating_sub(wanted.len());
    let last = (begin + reach).saturating_sub(1).min(lines.len() - 1);
    let shown_last = last.min(first + CLOSEST_SHOWN_LINES - 1);
    let mut place = if first == last {
        format!("The closest place is line {} of the file", first + 1)
    } else {
        format!("The closest place is lines {}-{} of the file", first + 1, last + 1)
    };
    let _ = write!(place, " ({found} of {asked} lines are there). The file has:");
    for line in &lines[first..=shown_last] {
        place.push('\n');
        place.push_str(line);
    }
    if shown_last < last {
        let _ = write!(place, "\n… ({} more lines)", last - shown_last);
    }
    Some(place)
}

/// A line as the loosest of [`LINE_TOLERANCES`] reads it.
fn loosely(line: &str) -> String {
    line.trim().chars().map(plain_punctuation).collect()
}

/// Lines `start..start + old_len` of the file become `new_lines`.
struct Replacement<'h, 'a> {
    start: usize,
    old_len: usize,
    new_lines: &'h [&'a str],
}

/// Where each hunk lands, in file order. Each hunk is looked for after the
/// previous one, so two hunks never claim the same lines.
fn plan_replacements<'h, 'a>(
    lines: &[&str],
    hunks: &'h [PatchHunk<'a>],
    path: &str,
) -> Result<Vec<Replacement<'h, 'a>>, PatchError> {
    let not_found = |wanted: &[&str]| PatchError::NotFound {
        path: path.to_string(),
        wanted: wanted.join("\n"),
    };
    let mut replacements = Vec::with_capacity(hunks.len());
    let mut cursor = 0;
    for hunk in hunks {
        if let Some(anchor) = hunk.anchor {
            let found = find_lines(lines, &[anchor], cursor, false).ok_or_else(|| not_found(&[anchor]))?;
            cursor = found + 1;
        }
        let mut old: &[&str] = &hunk.old_lines;
        let mut new: &'h [&'a str] = &hunk.new_lines;
        if old.is_empty() {
            // Only `+` lines: insert after the anchor, or at the end of a file
            // the hunk did not anchor.
            let at = if hunk.anchor.is_some() { cursor } else { lines.len() };
            replacements.push(Replacement { start: at, old_len: 0, new_lines: new });
            continue;
        }
        let mut found = find_lines(lines, old, cursor, hunk.at_end_of_file);
        // A hunk copied with the file's final blank line still matches a file
        // that has none.
        if found.is_none() && old.last() == Some(&"") {
            old = &old[..old.len() - 1];
            if new.last() == Some(&"") {
                new = &new[..new.len() - 1];
            }
            found = find_lines(lines, old, cursor, hunk.at_end_of_file);
        }
        let start = found.ok_or_else(|| not_found(old))?;
        replacements.push(Replacement { start, old_len: old.len(), new_lines: new });
        cursor = start + old.len();
    }
    // Unanchored insertions sit at the end; everything else is already in
    // order. A stable sort keeps two changes at one index in patch order.
    replacements.sort_by_key(|change| change.start);
    Ok(replacements)
}

/// The first index at or after `from` where `pattern` matches `lines`,
/// trying each tolerance in turn. With `at_end`, only the end of the file.
fn find_lines(lines: &[&str], pattern: &[&str], from: usize, at_end: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(from);
    }
    let last_start = lines.len().checked_sub(pattern.len())?;
    let first_start = if at_end { last_start } else { from };
    if first_start > last_start {
        return None;
    }
    LINE_TOLERANCES.iter().find_map(|same| {
        (first_start..=last_start).find(|&start| {
            pattern
                .iter()
                .zip(&lines[start..])
                .all(|(wanted, line)| same(wanted, line))
        })
    })
}

/// Line comparisons from strictest to loosest; the first that finds the
/// hunk decides where it goes.
const LINE_TOLERANCES: [fn(&str, &str) -> bool; 4] = [
    |wanted, line| wanted == line,
    |wanted, line| wanted.trim_end() == line.trim_end(),
    |wanted, line| wanted.trim() == line.trim(),
    |wanted, line| {
        wanted
            .trim()
            .chars()
            .map(plain_punctuation)
            .eq(line.trim().chars().map(plain_punctuation))
    },
];

/// The ASCII character a typographic one stands for.
const fn plain_punctuation(ch: char) -> char {
    match ch {
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{2018}'..='\u{201B}' => '\'',
        '\u{201C}'..='\u{201F}' => '"',
        '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(path: &str, body: &str) -> String {
        format!("*** Begin Patch\n*** Update File: {path}\n{body}\n*** End Patch")
    }

    mod closest_place {
        use super::*;

        const FILE: &str = "fn main() {\n    let total = 1;\n    let other = 2;\n    println!(\"{total}\");\n}\n";

        /// One line of the hunk is not what the file holds: the place is
        /// named and the file's own lines are shown, the differing one among
        /// them.
        #[test]
        fn names_the_place_and_shows_what_the_file_holds_there() {
            let wanted = "fn main() {\n    let total = 2;\n    let other = 2;";

            let place = closest_place(FILE, wanted).expect("two of the three lines are there");

            assert!(
                place.starts_with("The closest place is lines 1-3 of the file (2 of 3 lines are there)."),
                "{place}"
            );
            assert!(place.ends_with("fn main() {\n    let total = 1;\n    let other = 2;"), "{place}");
        }

        /// The line that differs is the hunk's first: the place still starts
        /// with the file's line for it.
        #[test]
        fn reaches_over_a_first_line_that_differs() {
            let wanted = "fn start() {\n    let total = 1;\n    let other = 2;";

            let place = closest_place(FILE, wanted).expect("two of the three lines are there");

            assert!(place.contains("lines 1-3 of the file"), "{place}");
            assert!(place.contains("\nfn main() {\n"), "{place}");
        }

        /// The file holds a line between two the hunk wrote next to each
        /// other: both are found, and the line between is shown.
        #[test]
        fn finds_lines_the_file_holds_another_line_between() {
            let wanted = "    let total = 1;\n    println!(\"{total}\");";

            let place = closest_place(FILE, wanted).expect("both lines are there, one line apart");

            assert!(place.contains("lines 2-4 of the file (2 of 2 lines are there)"), "{place}");
            assert!(place.contains("    let other = 2;"), "{place}");
        }

        /// A place that shares one closing brace with the hunk says nothing.
        #[test]
        fn a_place_with_less_than_half_of_the_lines_is_not_shown() {
            assert_eq!(closest_place(FILE, "struct Other;\nimpl Other {\n}"), None);
        }

        #[test]
        fn nothing_to_look_for_and_nothing_to_look_in_find_nothing() {
            assert_eq!(closest_place(FILE, "\n  \n"), None);
            assert_eq!(closest_place("", "fn main() {"), None);
        }

        /// A long place is cut after sixteen lines and says how many are left.
        #[test]
        fn a_long_place_is_cut_and_says_how_much_is_left() {
            let rows = |count: usize| {
                let mut text = String::new();
                for n in 1..=count {
                    text.push_str("row ");
                    text.push_str(&n.to_string());
                    text.push_str(";\n");
                }
                text
            };
            let file = rows(40);
            let mut wanted = rows(30);
            wanted.push_str("row x;");

            let place = closest_place(&file, &wanted).expect("thirty of thirty-one lines are there");

            assert!(place.contains("lines 1-31 of the file (30 of 31 lines are there)"), "{place}");
            assert!(place.contains("\nrow 16;\n") && !place.contains("row 17;"), "{place}");
            assert!(place.ends_with("… (15 more lines)"), "{place}");
        }
    }

    mod parse_patch {
        use super::*;

        #[test]
        fn reads_add_delete_and_update_operations_in_order() {
            let patch = "*** Begin Patch\n*** Add File: new.txt\n+hello\n*** Delete File: old.txt\n*** Update File: a.rs\n@@\n-x\n+y\n*** End Patch";

            let operations = parse_patch(patch).expect("valid patch");

            assert_eq!(
                operations,
                vec![
                    PatchOperation::Add { path: "new.txt", lines: vec!["hello"] },
                    PatchOperation::Delete { path: "old.txt" },
                    PatchOperation::Update {
                        path: "a.rs",
                        move_to: None,
                        hunks: vec![PatchHunk {
                            anchor: None,
                            old_lines: vec!["x"],
                            new_lines: vec!["y"],
                            at_end_of_file: false,
                        }],
                    },
                ]
            );
        }

        #[test]
        fn reads_a_move_and_an_anchor() {
            let patch = "*** Begin Patch\n*** Update File: a.rs\n*** Move to: b.rs\n@@ fn main() {\n-x\n+y\n*** End Patch";

            let operations = parse_patch(patch).expect("valid patch");

            assert_eq!(
                operations,
                vec![PatchOperation::Update {
                    path: "a.rs",
                    move_to: Some("b.rs"),
                    hunks: vec![PatchHunk {
                        anchor: Some("fn main() {"),
                        old_lines: vec!["x"],
                        new_lines: vec!["y"],
                        at_end_of_file: false,
                    }],
                }]
            );
        }

        #[test]
        fn accepts_a_first_hunk_without_an_at_at_header() {
            let patch = update("a.rs", " keep\n-old\n+new");

            let operations = parse_patch(&patch).expect("valid patch");

            assert_eq!(operations.len(), 1, "{operations:?}");
        }

        #[test]
        fn tolerates_a_heredoc_around_the_envelope() {
            let patch = "<<'EOF'\n*** Begin Patch\n*** Delete File: a.rs\n*** End Patch\nEOF";

            let operations = parse_patch(patch).expect("heredoc-wrapped patch");

            assert_eq!(operations, vec![PatchOperation::Delete { path: "a.rs" }]);
        }

        #[test]
        fn refuses_text_without_the_begin_marker() {
            let error = parse_patch("*** Update File: a.rs\n-x\n+y\n*** End Patch").unwrap_err();

            assert!(error.to_string().contains("*** Begin Patch"), "{error}");
        }

        #[test]
        fn refuses_a_hunk_line_without_a_prefix() {
            let error = parse_patch(&update("a.rs", "-x\nplain")).unwrap_err();

            assert!(error.to_string().contains("found `plain`"), "{error}");
        }

        #[test]
        fn refuses_an_update_with_no_hunks() {
            let error = parse_patch("*** Begin Patch\n*** Update File: a.rs\n*** End Patch").unwrap_err();

            assert!(error.to_string().contains("has no hunks"), "{error}");
        }

        #[test]
        fn refuses_a_second_hunk_without_an_at_at_header() {
            let patch = "*** Begin Patch\n*** Update File: a.rs\n@@\n-x\n+y\n*** End of File\n-z\n+w\n*** End Patch";

            let error = parse_patch(patch).unwrap_err();

            assert!(error.to_string().contains("must open with `@@`"), "{error}");
        }
    }

    mod apply_hunks {
        use super::*;

        fn apply(original: &str, body: &str) -> Result<String, PatchError> {
            let patch = update("f.txt", body);
            let operations = parse_patch(&patch).expect("valid patch");
            let PatchOperation::Update { hunks, .. } = &operations[0] else {
                panic!("expected an update: {operations:?}");
            };
            apply_hunks(original, hunks, "f.txt")
        }

        #[test]
        fn replaces_the_matched_lines() {
            let updated = apply("a\nb\nc\n", " a\n-b\n+B\n c").expect("applies");

            assert_eq!(updated, "a\nB\nc\n");
        }

        #[test]
        fn applies_two_hunks_in_file_order() {
            let updated = apply("1\n2\n3\n4\n5\n", "@@\n-2\n+two\n@@\n-4\n+four").expect("applies");

            assert_eq!(updated, "1\ntwo\n3\nfour\n5\n");
        }

        #[test]
        fn looks_for_the_hunk_after_its_anchor() {
            let original = "fn a() {\n    x();\n}\nfn b() {\n    x();\n}\n";

            let updated = apply(original, "@@ fn b() {\n-    x();\n+    y();").expect("applies");

            assert_eq!(updated, "fn a() {\n    x();\n}\nfn b() {\n    y();\n}\n");
        }

        #[test]
        fn inserts_an_anchored_addition_right_after_the_anchor() {
            let updated = apply("head\ntail\n", "@@ head\n+middle").expect("applies");

            assert_eq!(updated, "head\nmiddle\ntail\n");
        }

        #[test]
        fn appends_an_unanchored_addition_at_the_end() {
            let updated = apply("one\n", "+two").expect("applies");

            assert_eq!(updated, "one\ntwo\n");
        }

        #[test]
        fn matches_lines_whose_trailing_whitespace_differs() {
            let updated = apply("value = 1   \n", "-value = 1\n+value = 2").expect("applies");

            assert_eq!(updated, "value = 2\n");
        }

        #[test]
        fn matches_typographic_punctuation_against_ascii() {
            let updated = apply("say(\"hi\") – done\n", "-say(“hi”) - done\n+say(\"bye\")").expect("applies");

            assert_eq!(updated, "say(\"bye\")\n");
        }

        #[test]
        fn keeps_a_crlf_file_crlf() {
            let updated = apply("a\r\nb\r\n", "-b\n+c").expect("applies");

            assert_eq!(updated, "a\r\nc\r\n");
        }

        #[test]
        fn ends_a_file_that_lacked_a_final_newline_with_one() {
            let updated = apply("a\nb", "-b\n+c").expect("applies");

            assert_eq!(updated, "a\nc\n");
        }

        #[test]
        fn pins_an_end_of_file_hunk_to_the_last_lines() {
            let updated = apply("x\ny\nx\n", "-x\n+z\n*** End of File").expect("applies");

            assert_eq!(updated, "x\ny\nz\n");
        }

        #[test]
        fn names_the_lines_it_could_not_find() {
            let error = apply("a\nb\n", "-missing\n+x").unwrap_err();

            assert_eq!(
                error,
                PatchError::NotFound { path: "f.txt".to_string(), wanted: "missing".to_string() }
            );
        }
    }

    #[test]
    fn added_file_contents_ends_every_line_with_a_newline() {
        assert_eq!(added_file_contents(&["a", "", "b"]), "a\n\nb\n");
    }
}
