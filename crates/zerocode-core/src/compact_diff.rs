//! Line diffs, and the rows a diff is drawn as.
//!
//! `compact_line_diff` is the LCS walk zo's tools have used for their edit
//! previews and hunk attribution; it moved here (2026-09-15) so the window's
//! conversation view could draw the same diff under an Edit row and neither
//! side kept its own copy. `DiffLine` is the one row shape both the review
//! surface (`parse_unified_diff`, from git) and the conversation's inline diff
//! (`hunk_lines`, from an edit's old and new text) hand the window.

use std::borrow::Cow;

const DEFAULT_CONTEXT_LINES: usize = 3;
const EXACT_DIFF_MAX_CELLS: usize = 1_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactDiffHunk {
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<CompactDiffLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactDiffLine {
    pub kind: CompactDiffLineKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactDiffLineKind {
    Context,
    Removed,
    Added,
}

#[derive(Clone, Copy, Debug)]
struct LineDiffOp<'a> {
    kind: CompactDiffLineKind,
    old_index: usize,
    new_index: usize,
    text: &'a str,
}

#[must_use]
pub fn compact_line_diff(original: &str, updated: &str) -> Vec<CompactDiffHunk> {
    if original == updated {
        return Vec::new();
    }

    let original_lines: Vec<&str> = original.lines().collect();
    let updated_lines: Vec<&str> = updated.lines().collect();
    if let Some(ops) = line_diff_ops(&original_lines, &updated_lines) {
        return hunks_from_ops(&ops);
    }

    vec![single_hunk_from_lines(&original_lines, &updated_lines)]
}

fn line_diff_ops<'a>(old_lines: &[&'a str], new_lines: &[&'a str]) -> Option<Vec<LineDiffOp<'a>>> {
    let cells = old_lines.len().saturating_mul(new_lines.len());
    if cells > EXACT_DIFF_MAX_CELLS {
        return None;
    }

    let width = new_lines.len() + 1;
    let index = |old_index: usize, new_index: usize| old_index * width + new_index;
    let mut lengths = vec![0usize; (old_lines.len() + 1) * width];
    for old_index in (0..old_lines.len()).rev() {
        for new_index in (0..new_lines.len()).rev() {
            lengths[index(old_index, new_index)] = if old_lines[old_index] == new_lines[new_index] {
                lengths[index(old_index + 1, new_index + 1)] + 1
            } else {
                lengths[index(old_index + 1, new_index)]
                    .max(lengths[index(old_index, new_index + 1)])
            };
        }
    }

    let mut ops = Vec::new();
    let mut old_index = 0usize;
    let mut new_index = 0usize;
    while old_index < old_lines.len() && new_index < new_lines.len() {
        if old_lines[old_index] == new_lines[new_index] {
            ops.push(LineDiffOp {
                kind: CompactDiffLineKind::Context,
                old_index,
                new_index,
                text: old_lines[old_index],
            });
            old_index += 1;
            new_index += 1;
        } else if lengths[index(old_index + 1, new_index)]
            >= lengths[index(old_index, new_index + 1)]
        {
            ops.push(LineDiffOp {
                kind: CompactDiffLineKind::Removed,
                old_index,
                new_index,
                text: old_lines[old_index],
            });
            old_index += 1;
        } else {
            ops.push(LineDiffOp {
                kind: CompactDiffLineKind::Added,
                old_index,
                new_index,
                text: new_lines[new_index],
            });
            new_index += 1;
        }
    }

    while old_index < old_lines.len() {
        ops.push(LineDiffOp {
            kind: CompactDiffLineKind::Removed,
            old_index,
            new_index,
            text: old_lines[old_index],
        });
        old_index += 1;
    }
    while new_index < new_lines.len() {
        ops.push(LineDiffOp {
            kind: CompactDiffLineKind::Added,
            old_index,
            new_index,
            text: new_lines[new_index],
        });
        new_index += 1;
    }

    Some(ops)
}

fn hunks_from_ops(ops: &[LineDiffOp<'_>]) -> Vec<CompactDiffHunk> {
    let mut hunks = Vec::new();
    let mut current = Vec::new();
    let mut pending_context = Vec::new();
    let mut in_hunk = false;

    for &op in ops {
        if op.kind == CompactDiffLineKind::Context {
            if in_hunk {
                pending_context.push(op);
            } else {
                pending_context.push(op);
                if pending_context.len() > DEFAULT_CONTEXT_LINES {
                    pending_context.remove(0);
                }
            }
            continue;
        }

        if !in_hunk {
            current.extend_from_slice(&pending_context);
            pending_context.clear();
            in_hunk = true;
        } else if pending_context.len() > DEFAULT_CONTEXT_LINES * 2 {
            current.extend_from_slice(&pending_context[..DEFAULT_CONTEXT_LINES]);
            if let Some(hunk) = hunk_from_ops(&current) {
                hunks.push(hunk);
            }
            current.clear();
            current.extend_from_slice(
                &pending_context[pending_context.len() - DEFAULT_CONTEXT_LINES..],
            );
            pending_context.clear();
        } else {
            current.extend_from_slice(&pending_context);
            pending_context.clear();
        }

        current.push(op);
    }

    if in_hunk {
        current.extend_from_slice(
            &pending_context[..pending_context.len().min(DEFAULT_CONTEXT_LINES)],
        );
        if let Some(hunk) = hunk_from_ops(&current) {
            hunks.push(hunk);
        }
    }

    hunks
}

fn hunk_from_ops(ops: &[LineDiffOp<'_>]) -> Option<CompactDiffHunk> {
    let first = ops.first()?;
    Some(CompactDiffHunk {
        old_start: first.old_index.saturating_add(1),
        old_lines: ops
            .iter()
            .filter(|op| op.kind != CompactDiffLineKind::Added)
            .count(),
        new_start: first.new_index.saturating_add(1),
        new_lines: ops
            .iter()
            .filter(|op| op.kind != CompactDiffLineKind::Removed)
            .count(),
        lines: ops
            .iter()
            .map(|op| CompactDiffLine {
                kind: op.kind,
                text: op.text.to_string(),
            })
            .collect(),
    })
}

fn single_hunk_from_lines(original_lines: &[&str], updated_lines: &[&str]) -> CompactDiffHunk {
    if original_lines.is_empty() {
        return CompactDiffHunk {
            old_start: 1,
            old_lines: 0,
            new_start: 1,
            new_lines: updated_lines.len(),
            lines: updated_lines
                .iter()
                .map(|line| CompactDiffLine {
                    kind: CompactDiffLineKind::Added,
                    text: (*line).to_string(),
                })
                .collect(),
        };
    }
    if updated_lines.is_empty() {
        return CompactDiffHunk {
            old_start: 1,
            old_lines: original_lines.len(),
            new_start: 1,
            new_lines: 0,
            lines: original_lines
                .iter()
                .map(|line| CompactDiffLine {
                    kind: CompactDiffLineKind::Removed,
                    text: (*line).to_string(),
                })
                .collect(),
        };
    }

    let mut prefix = 0usize;
    while prefix < original_lines.len()
        && prefix < updated_lines.len()
        && original_lines[prefix] == updated_lines[prefix]
    {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix < original_lines.len().saturating_sub(prefix)
        && suffix < updated_lines.len().saturating_sub(prefix)
        && original_lines[original_lines.len() - 1 - suffix]
            == updated_lines[updated_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let old_change_end = original_lines.len().saturating_sub(suffix);
    let new_change_end = updated_lines.len().saturating_sub(suffix);
    let context_start = prefix.saturating_sub(DEFAULT_CONTEXT_LINES);
    let old_context_end = old_change_end
        .saturating_add(DEFAULT_CONTEXT_LINES)
        .min(original_lines.len());
    let new_context_end = new_change_end
        .saturating_add(DEFAULT_CONTEXT_LINES)
        .min(updated_lines.len());

    let mut lines = Vec::new();
    for line in &original_lines[context_start..prefix] {
        lines.push(CompactDiffLine {
            kind: CompactDiffLineKind::Context,
            text: (*line).to_string(),
        });
    }
    for line in &original_lines[prefix..old_change_end] {
        lines.push(CompactDiffLine {
            kind: CompactDiffLineKind::Removed,
            text: (*line).to_string(),
        });
    }
    for line in &updated_lines[prefix..new_change_end] {
        lines.push(CompactDiffLine {
            kind: CompactDiffLineKind::Added,
            text: (*line).to_string(),
        });
    }
    for line in &updated_lines[new_change_end..new_context_end] {
        lines.push(CompactDiffLine {
            kind: CompactDiffLineKind::Context,
            text: (*line).to_string(),
        });
    }

    CompactDiffHunk {
        old_start: context_start.saturating_add(1),
        old_lines: old_context_end.saturating_sub(context_start),
        new_start: context_start.saturating_add(1),
        new_lines: new_context_end.saturating_sub(context_start),
        lines,
    }
}

/// One line of a diff, as the window draws it — the review surface and the
/// conversation's inline diff share this row.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// `meta` for a file header, `hunk` for an `@@` line, then `add`, `del`
    /// and `ctx` for the body. Borrowed for the literals the parsers write —
    /// a review diff runs to a hundred thousand rows — owned when read back.
    pub kind: Cow<'static, str>,
    /// The line without its diff marker — the marker is the `kind`, and
    /// leaving it in the text would put it inside anything copied out.
    pub text: String,
    /// Where the line sits on each side, absent where it does not exist —
    /// or where nobody knows (a snippet whose place in its file is unknown).
    pub old: Option<u32>,
    pub new: Option<u32>,
}

impl DiffLine {
    #[must_use]
    pub fn marker(kind: &'static str, text: &str) -> Self {
        Self {
            kind: Cow::Borrowed(kind),
            text: text.to_string(),
            old: None,
            new: None,
        }
    }
}

/// Hunks as the rows the window draws.
///
/// `numbered` is whether the hunks' starts are places in a file: true for a
/// whole file (`Write`, a patch's added file), false for a snippet (an Edit's
/// old and new strings), whose gutters stay blank rather than count from 1
/// as if that were the file's top. Between two hunks a `hunk` row stands —
/// the `@@` header where it is known, a bare mark where it is not.
#[must_use]
pub fn hunk_lines(hunks: &[CompactDiffHunk], numbered: bool) -> Vec<DiffLine> {
    let mut lines = Vec::new();
    for (index, hunk) in hunks.iter().enumerate() {
        if index > 0 || (numbered && hunks.len() > 1) {
            let header = if numbered {
                format!(
                    "@@ -{},{} +{},{} @@",
                    hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
                )
            } else {
                "@@".to_string()
            };
            lines.push(DiffLine::marker("hunk", &header));
        }
        let (mut old, mut new) = (hunk.old_start, hunk.new_start);
        let place = |at: usize| numbered.then(|| u32::try_from(at).unwrap_or(u32::MAX));
        for line in &hunk.lines {
            let row = match line.kind {
                CompactDiffLineKind::Added => {
                    let row = DiffLine {
                        kind: Cow::Borrowed("add"),
                        text: line.text.clone(),
                        old: None,
                        new: place(new),
                    };
                    new += 1;
                    row
                }
                CompactDiffLineKind::Removed => {
                    let row = DiffLine {
                        kind: Cow::Borrowed("del"),
                        text: line.text.clone(),
                        old: place(old),
                        new: None,
                    };
                    old += 1;
                    row
                }
                CompactDiffLineKind::Context => {
                    let row = DiffLine {
                        kind: Cow::Borrowed("ctx"),
                        text: line.text.clone(),
                        old: place(old),
                        new: place(new),
                    };
                    old += 1;
                    new += 1;
                    row
                }
            };
            lines.push(row);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snippet_diff_keeps_its_gutters_blank_and_a_file_diff_counts() {
        let hunks = compact_line_diff("a\nb\nc\n", "a\nB\nc\n");
        let snippet = hunk_lines(&hunks, false);
        assert_eq!(
            snippet
                .iter()
                .map(|line| (line.kind.as_ref(), line.text.as_str(), line.old, line.new))
                .collect::<Vec<_>>(),
            vec![
                ("ctx", "a", None, None),
                ("del", "b", None, None),
                ("add", "B", None, None),
                ("ctx", "c", None, None),
            ]
        );
        let file = hunk_lines(&hunks, true);
        assert_eq!(
            file.iter()
                .map(|line| (line.kind.as_ref(), line.old, line.new))
                .collect::<Vec<_>>(),
            vec![
                ("ctx", Some(1), Some(1)),
                ("del", Some(2), None),
                ("add", None, Some(2)),
                ("ctx", Some(3), Some(3)),
            ]
        );
    }

    #[test]
    fn two_hunks_are_parted_by_a_hunk_row() {
        let old: String = (1..=20).map(|n| format!("l{n}\n")).collect();
        let new = old.replace("l2\n", "L2\n").replace("l19\n", "L19\n");
        let hunks = compact_line_diff(&old, &new);
        assert_eq!(hunks.len(), 2);
        let rows = hunk_lines(&hunks, true);
        let headers: Vec<&str> = rows
            .iter()
            .filter(|row| row.kind == "hunk")
            .map(|row| row.text.as_str())
            .collect();
        assert_eq!(headers, vec!["@@ -1,5 +1,5 @@", "@@ -16,5 +16,5 @@"]);
        let bare = hunk_lines(&hunks, false);
        assert_eq!(bare.iter().filter(|row| row.kind == "hunk").count(), 1);
        assert_eq!(
            bare.iter()
                .find(|row| row.kind == "hunk")
                .map(|row| row.text.as_str()),
            Some("@@")
        );
    }
}
