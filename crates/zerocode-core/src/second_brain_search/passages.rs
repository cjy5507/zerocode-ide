//! Where one file's passages begin and end.
//!
//! A passage is what a search answers with: enough of a file to say something
//! and little enough to read at a glance. The rule is the same for a page, a
//! note and a log, because it only looks at lines:
//!
//! - the frontmatter is not a passage — a file's title and tags are its name;
//! - an empty line ends a passage, unless it stands inside a code fence;
//! - a heading waits for the lines under it, and a passage shorter than
//!   [`Bounds::least`] waits for the next one, so neither is an answer alone;
//! - a passage that would pass [`Bounds::most`] ends before the line that
//!   would take it there, and a single line longer than that is cut on
//!   character boundaries.

use super::segment::Place;
use crate::second_brain_graph::{fence_mark, split_frontmatter};

/// How long a passage may be and how short one may stand alone, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Bounds {
    pub most: usize,
    pub least: usize,
}

/// A passage being gathered: the bytes it covers so far, the line it began
/// on, and whether every line of it is a heading.
struct Open {
    start: usize,
    end: usize,
    line: u32,
    headings_only: bool,
}

/// The passages of `text`, in order. Offsets are bytes of `text` itself.
pub(super) fn split(text: &str, bounds: &Bounds) -> Vec<Place> {
    let (_, body) = split_frontmatter(text);
    let body_at = text.len() - body.len();
    let mut line_number = 1 + count_lines(&text[..body_at]);
    let mut places = Vec::new();
    let mut open: Option<Open> = None;
    let mut fence: Option<(char, usize)> = None;
    let mut at = body_at;
    for line in body.split_inclusive('\n') {
        let (start, this_line) = (at, line_number);
        at += line.len();
        line_number += 1;
        let said = line.trim();
        let in_fence = fence.is_some();
        if let Some(mark) = fence_mark(said) {
            match fence {
                // A fence closes on its own character at its own width or more.
                Some((opened, width)) if opened == mark.0 && mark.1 >= width => fence = None,
                Some(_) => {}
                None => fence = Some(mark),
            }
        }
        if said.is_empty() && !in_fence {
            if open
                .as_ref()
                .is_some_and(|held| !held.headings_only && held.end - held.start >= bounds.least)
            {
                close(text, &mut open, &mut places);
            }
            continue;
        }
        if open
            .as_ref()
            .is_some_and(|held| at - held.start > bounds.most)
        {
            close(text, &mut open, &mut places);
        }
        if open.is_none() && line.len() > bounds.most {
            cut_long_line(line, start, this_line, bounds.most, &mut places);
            continue;
        }
        let heading = said.starts_with('#') && !in_fence;
        match &mut open {
            Some(held) => {
                held.end = at;
                held.headings_only &= heading;
            }
            None => {
                open = Some(Open {
                    start,
                    end: at,
                    line: this_line,
                    headings_only: heading,
                });
            }
        }
    }
    close(text, &mut open, &mut places);
    places
}

/// Ends the passage being gathered, without the blank space at its end.
fn close(text: &str, open: &mut Option<Open>, places: &mut Vec<Place>) {
    let Some(held) = open.take() else {
        return;
    };
    let len = text[held.start..held.end].trim_end().len();
    if len > 0 {
        places.push(place(held.start, len, held.line));
    }
}

/// One line longer than a passage may be, as passages of that length at most.
fn cut_long_line(line: &str, start: usize, line_number: u32, most: usize, places: &mut Vec<Place>) {
    let line = line.trim_end();
    let mut from = 0;
    while from < line.len() {
        let mut to = (from + most.max(1)).min(line.len());
        while !line.is_char_boundary(to) {
            to -= 1;
        }
        if to == from {
            // A bound smaller than one character: take the character whole.
            to = from + line[from..].chars().next().map_or(1, char::len_utf8);
        }
        places.push(place(start + from, to - from, line_number));
        from = to;
    }
}

/// A file past four gibibytes is not indexed (`Table::max_file_bytes`), so
/// these fit; a saturated place reads nothing rather than the wrong bytes.
fn place(start: usize, len: usize, line: u32) -> Place {
    Place {
        start: u32::try_from(start).unwrap_or(u32::MAX),
        len: u32::try_from(len).unwrap_or(0),
        line,
    }
}

fn count_lines(text: &str) -> u32 {
    u32::try_from(text.bytes().filter(|byte| *byte == b'\n').count()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: Bounds = Bounds {
        most: 60,
        least: 12,
    };

    fn passages(text: &str) -> Vec<(u32, &str)> {
        split(text, &BOUNDS)
            .into_iter()
            .map(|held| {
                let start = held.start as usize;
                (held.line, &text[start..start + held.len as usize])
            })
            .collect()
    }

    /// The frontmatter is skipped and still counted: a passage's line is its
    /// line in the file.
    #[test]
    fn an_empty_line_ends_a_passage_and_lines_are_counted_from_the_files_top() {
        let text = "---\ntitle: Harbor\n---\n\nThe crane lifts cargo.\nIt works at night.\n\nThe tide table hangs by the door.\n";
        assert_eq!(
            passages(text),
            [
                (5, "The crane lifts cargo.\nIt works at night."),
                (8, "The tide table hangs by the door."),
            ]
        );
    }

    /// A heading alone says nothing, and neither does a line shorter than the
    /// least: each joins what follows it.
    #[test]
    fn a_heading_and_a_short_line_wait_for_what_follows() {
        let text = "# Cranes\n\n## Night shift\n\nThe crane lifts cargo.\n\nShort.\n\nThe tide table hangs here.\n";
        assert_eq!(
            passages(text),
            [
                (1, "# Cranes\n\n## Night shift\n\nThe crane lifts cargo."),
                (7, "Short.\n\nThe tide table hangs here."),
            ]
        );
    }

    /// An empty line inside a fence is part of the code, and a `#` there is a
    /// comment, not a heading.
    #[test]
    fn a_code_fence_stays_whole_across_its_empty_lines() {
        let text = "Run it:\n\n```sh\n# build\n\nmake all\n```\n\nThen read the log file.\n";
        assert_eq!(
            passages(text),
            [
                (1, "Run it:\n\n```sh\n# build\n\nmake all\n```"),
                (9, "Then read the log file."),
            ]
        );
    }

    /// Lines with no empty line between them — a log, a data file — end at
    /// the bound, on a line's edge.
    #[test]
    fn a_run_of_lines_ends_before_the_line_that_would_pass_the_bound() {
        let line = "2026-01-01 worker started ok\n";
        let text = line.repeat(5);
        let held = passages(&text);
        assert_eq!(held.len(), 3, "{held:?}");
        assert_eq!(
            held[0],
            (
                1,
                "2026-01-01 worker started ok\n2026-01-01 worker started ok"
            )
        );
        assert_eq!(held[1].0, 3);
        assert_eq!(held[2], (5, "2026-01-01 worker started ok"));
        assert!(held.iter().all(|(_, text)| text.len() <= BOUNDS.most));
    }

    /// One line longer than the bound is cut on character boundaries, every
    /// piece on that line, and nothing of it is lost.
    #[test]
    fn a_line_longer_than_the_bound_is_cut_between_characters() {
        let text = format!("{}\n", "항구".repeat(30));
        let held = passages(&text);
        assert!(held.len() > 1);
        assert!(
            held.iter()
                .all(|(line, text)| *line == 1 && text.len() <= BOUNDS.most)
        );
        assert_eq!(
            held.iter().map(|(_, text)| *text).collect::<String>(),
            "항구".repeat(30)
        );
    }

    /// Windows line ends and a file with no line end at all read the same.
    #[test]
    fn crlf_and_a_missing_last_line_end_change_nothing() {
        let text = "The crane lifts cargo.\r\n\r\nThe tide table hangs here.";
        assert_eq!(
            passages(text),
            [
                (1, "The crane lifts cargo."),
                (3, "The tide table hangs here.")
            ]
        );
        assert!(passages("").is_empty());
        assert!(passages("---\ntitle: only\n---\n").is_empty());
    }
}
