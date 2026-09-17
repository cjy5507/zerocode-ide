//! One declared relation, written into a wiki page's frontmatter (t-4140 S5).
//!
//! The vault protocol (`second_brain::AGENTS_GUIDE`) declares relations as
//! frontmatter keys — `related`, `implements`, `depends_on`, `supersedes`,
//! `contradicts` — whose items are `[[wikilinks]]`. This is the one road that
//! writes such an item, and it keeps to what the scanner reads
//! (`second_brain_graph::parse_page`): the link is the page's path below
//! `wiki/` without `.md`, which is the scanner's first lookup and the form the
//! vault's own pages use; an inline list stays inline (every link quoted, the
//! only YAML-valid spelling of a link in a flow list), a block list stays a
//! block list, a missing key is appended to the frontmatter, and a page with
//! no frontmatter gains one. Everything else in the file is byte for byte —
//! the line ending too. The file is replaced whole (temp beside, then rename)
//! so a reader never sees half a page.
//!
//! `raw/` is never written: the road takes ids under `wiki/` only. Removing is
//! the same road with `remove`, so an undo is an ordinary call.

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::second_brain::WIKI_DIR;
use crate::second_brain_graph::{
    EdgeKind, MARKDOWN_SUFFIX, MAX_FRONTMATTER_LINES, page_path, relation_target,
    split_relation_list,
};
use crate::second_brain_live::write_atomically;

/// What the road did: which page, which link text, which key, whether it was
/// a removal, whether the file changed at all, and the key's line afterwards
/// (empty when the key was dropped) — the window shows it as the receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelationWrite {
    pub page: String,
    pub target: String,
    pub kind: EdgeKind,
    pub removed: bool,
    pub changed: bool,
    pub line: String,
}

/// The link text a target id is written as: a page's path below `wiki/`
/// without `.md`; a ghost (`ghost:<target>`) its own target; anything else as
/// given. Empty text and a closing bracket are refused — both would write a
/// link the scanner cannot read back.
fn link_text(to: &str) -> Result<String, String> {
    let below = format!("{WIKI_DIR}/");
    let text = if let Some(rest) = to.strip_prefix("ghost:") {
        rest.trim().to_string()
    } else if let Some(rest) = to.strip_prefix(&below) {
        rest.strip_suffix(MARKDOWN_SUFFIX)
            .unwrap_or(rest)
            .to_string()
    } else {
        to.trim().to_string()
    };
    if text.is_empty() || text.contains("]]") || text.contains('\n') {
        return Err("연결할 페이지 이름이 비어 있거나 읽을 수 없습니다".to_string());
    }
    Ok(text)
}

/// Add (or, with `remove`, take away) the relation `from --kind--> to`.
pub fn relate(
    root: &Path,
    from_id: &str,
    to: &str,
    kind: EdgeKind,
    remove: bool,
) -> Result<RelationWrite, String> {
    if kind == EdgeKind::Mentions {
        return Err("본문 링크(mentions)는 frontmatter 관계가 아닙니다".to_string());
    }
    let key = kind.as_str();
    if from_id.split('/').next() != Some(WIKI_DIR) {
        return Err("관계는 wiki/ 페이지에만 씁니다".to_string());
    }
    let path = page_path(root, from_id).ok_or("볼트 안의 페이지가 아닙니다")?;
    if !path.is_file() {
        return Err("아직 없는 페이지입니다".to_string());
    }
    let target = link_text(to)?;
    let text = fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let edited = edit_frontmatter(&text, key, &target, remove);
    let changed = edited.text != text;
    if changed {
        write_atomically(&path, edited.text.as_bytes())
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(RelationWrite {
        page: from_id.to_string(),
        target,
        kind,
        removed: remove,
        changed,
        line: edited.line,
    })
}

struct Edited {
    text: String,
    line: String,
}

/// The page's text with the key's items changed, and the key's line(s) after.
fn edit_frontmatter(text: &str, key: &str, target: &str, remove: bool) -> Edited {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let (bom, whole) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let trailing = whole.ends_with('\n');
    let mut lines: Vec<String> = whole.lines().map(str::to_string).collect();
    // Where the frontmatter is: `open` the index of its first line, `close` the
    // index of the closing fence. A page without one gets a fence pair at the top.
    let fenced = lines.first().is_some_and(|line| line.trim_end() == "---");
    let close = if fenced {
        lines
            .iter()
            .enumerate()
            .skip(1)
            .take(MAX_FRONTMATTER_LINES)
            .find(|(_, line)| matches!(line.trim_end(), "---" | "..."))
            .map(|(at, _)| at)
    } else {
        None
    };
    let (open, close) = match close {
        Some(close) => (1, close),
        None => {
            lines.insert(0, "---".to_string());
            lines.insert(1, "---".to_string());
            (1, 1)
        }
    };
    let quoted = format!("\"[[{target}]]\"");
    let key_at = (open..close).find(|&at| {
        let line = lines[at].trim_end();
        !line.starts_with(' ')
            && !line.starts_with('\t')
            && line
                .split_once(':')
                .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case(key))
    });
    let line_after: String;
    match key_at {
        None => {
            if remove {
                line_after = String::new();
            } else {
                lines.insert(close, format!("{key}: [{quoted}]"));
                line_after = lines[close].clone();
            }
        }
        Some(at) => {
            let value = lines[at]
                .split_once(':')
                .map(|(_, value)| value.trim())
                .unwrap_or("");
            if value.is_empty() {
                // A block list: the indented `- item` lines that follow.
                let mut end = at + 1;
                while end < close
                    && lines[end].trim_start().starts_with("- ")
                    && (lines[end].starts_with(' ') || lines[end].starts_with('\t'))
                {
                    end += 1;
                }
                let held: Vec<usize> = (at + 1..end)
                    .filter(|&item| {
                        relation_target(lines[item].trim_start().trim_start_matches("- "))
                            .is_some_and(|held| held == target)
                    })
                    .collect();
                if remove {
                    for &item in held.iter().rev() {
                        lines.remove(item);
                        end -= 1;
                    }
                    if end == at + 1 {
                        lines.remove(at);
                    }
                } else if held.is_empty() {
                    let indent = lines
                        .get(at + 1)
                        .filter(|_| end > at + 1)
                        .map(|line| line[..line.len() - line.trim_start().len()].to_string())
                        .unwrap_or_else(|| "  ".to_string());
                    lines.insert(end, format!("{indent}- {quoted}"));
                    end += 1;
                }
                line_after = if lines.get(at).is_some_and(|line| {
                    line.split_once(':')
                        .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case(key))
                }) {
                    lines[at..end].join(newline)
                } else {
                    String::new()
                };
            } else {
                // An inline list (or one bare link): rebuilt as a flow list of
                // quoted links — the one spelling both YAML and the scanner read.
                let mut items: Vec<String> = split_relation_list(value)
                    .into_iter()
                    .filter_map(|item| relation_target(&item))
                    .collect();
                if remove {
                    items.retain(|item| item != target);
                } else if !items.iter().any(|item| item == target) {
                    items.push(target.to_string());
                }
                if items.is_empty() {
                    lines.remove(at);
                    line_after = String::new();
                } else {
                    let list = items
                        .iter()
                        .map(|item| format!("\"[[{item}]]\""))
                        .collect::<Vec<_>>()
                        .join(", ");
                    lines[at] = format!("{key}: [{list}]");
                    line_after = lines[at].clone();
                }
            }
        }
    }
    let mut out = String::with_capacity(text.len() + 64);
    out.push_str(bom);
    out.push_str(&lines.join(newline));
    if trailing || lines.len() > 1 {
        out.push_str(newline);
    }
    Edited {
        text: out,
        line: line_after,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(pages: &[(&str, &str)]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        crate::second_brain::setup(root.path()).unwrap();
        for (relative, body) in pages {
            let path = root.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        root
    }

    fn read(root: &Path, relative: &str) -> String {
        fs::read_to_string(root.join(relative)).unwrap()
    }

    #[test]
    fn appends_to_an_inline_list_and_removes_from_it_leaving_the_rest_byte_for_byte() {
        let root = vault(&[(
            "wiki/a.md",
            "---\ntitle: \"A\"\nrelated: [\"[[b]]\", \"[[c|see]]\"]\ntags: [x]\n---\n\n# A\n\nbody [[b]]\n",
        )]);
        let wrote = relate(
            root.path(),
            "wiki/a.md",
            "wiki/d.md",
            EdgeKind::Related,
            false,
        )
        .unwrap();
        assert!(wrote.changed);
        assert_eq!(wrote.target, "d");
        assert_eq!(wrote.line, "related: [\"[[b]]\", \"[[c]]\", \"[[d]]\"]");
        assert_eq!(
            read(root.path(), "wiki/a.md"),
            "---\ntitle: \"A\"\nrelated: [\"[[b]]\", \"[[c]]\", \"[[d]]\"]\ntags: [x]\n---\n\n# A\n\nbody [[b]]\n"
        );
        // Idempotent: the same link again changes nothing.
        let again = relate(
            root.path(),
            "wiki/a.md",
            "wiki/d.md",
            EdgeKind::Related,
            false,
        )
        .unwrap();
        assert!(!again.changed);
        // Removing by the alias-carrying item's target works too.
        let took = relate(
            root.path(),
            "wiki/a.md",
            "wiki/c.md",
            EdgeKind::Related,
            true,
        )
        .unwrap();
        assert!(took.changed && took.removed);
        assert_eq!(took.line, "related: [\"[[b]]\", \"[[d]]\"]");
        // Removing the last item drops the key line.
        relate(
            root.path(),
            "wiki/a.md",
            "wiki/b.md",
            EdgeKind::Related,
            true,
        )
        .unwrap();
        let last = relate(
            root.path(),
            "wiki/a.md",
            "wiki/d.md",
            EdgeKind::Related,
            true,
        )
        .unwrap();
        assert_eq!(last.line, "");
        assert_eq!(
            read(root.path(), "wiki/a.md"),
            "---\ntitle: \"A\"\ntags: [x]\n---\n\n# A\n\nbody [[b]]\n"
        );
        // No temp file is left beside the page.
        assert!(!root.path().join("wiki/a.md.tmp").exists());
    }

    #[test]
    fn keeps_a_block_list_a_block_list_and_appends_a_missing_key() {
        let root = vault(&[(
            "wiki/a.md",
            "---\r\ntitle: A\r\ndepends_on:\r\n  - [[b]]\r\n  - \"[[c]]\"\r\n---\r\nbody\r\n",
        )]);
        let wrote = relate(
            root.path(),
            "wiki/a.md",
            "wiki/sub/d.md",
            EdgeKind::DependsOn,
            false,
        )
        .unwrap();
        assert_eq!(wrote.target, "sub/d");
        assert_eq!(
            wrote.line,
            "depends_on:\r\n  - [[b]]\r\n  - \"[[c]]\"\r\n  - \"[[sub/d]]\""
        );
        assert_eq!(
            read(root.path(), "wiki/a.md"),
            "---\r\ntitle: A\r\ndepends_on:\r\n  - [[b]]\r\n  - \"[[c]]\"\r\n  - \"[[sub/d]]\"\r\n---\r\nbody\r\n"
        );
        let other = relate(
            root.path(),
            "wiki/a.md",
            "ghost:missing",
            EdgeKind::Contradicts,
            false,
        )
        .unwrap();
        assert_eq!(other.line, "contradicts: [\"[[missing]]\"]");
        assert!(
            read(root.path(), "wiki/a.md")
                .contains("\r\ncontradicts: [\"[[missing]]\"]\r\n---\r\n")
        );
        let gone = relate(
            root.path(),
            "wiki/a.md",
            "wiki/b.md",
            EdgeKind::DependsOn,
            true,
        )
        .unwrap();
        assert_eq!(
            gone.line,
            "depends_on:\r\n  - \"[[c]]\"\r\n  - \"[[sub/d]]\""
        );
    }

    #[test]
    fn a_page_without_frontmatter_gains_one_and_a_bare_link_becomes_a_list() {
        let root = vault(&[
            ("wiki/plain.md", "# Plain\n\ntext\n"),
            ("wiki/bare.md", "---\nimplements: [[adr/003]]\n---\n"),
        ]);
        let wrote = relate(
            root.path(),
            "wiki/plain.md",
            "wiki/x.md",
            EdgeKind::Supersedes,
            false,
        )
        .unwrap();
        assert_eq!(wrote.line, "supersedes: [\"[[x]]\"]");
        assert_eq!(
            read(root.path(), "wiki/plain.md"),
            "---\nsupersedes: [\"[[x]]\"]\n---\n# Plain\n\ntext\n"
        );
        let listed = relate(
            root.path(),
            "wiki/bare.md",
            "wiki/y.md",
            EdgeKind::Implements,
            false,
        )
        .unwrap();
        assert_eq!(listed.line, "implements: [\"[[adr/003]]\", \"[[y]]\"]");
        // Removing a relation that is not there changes nothing and says so.
        let none = relate(
            root.path(),
            "wiki/plain.md",
            "wiki/zz.md",
            EdgeKind::Related,
            true,
        )
        .unwrap();
        assert!(!none.changed && none.line.is_empty());
    }

    #[test]
    fn refuses_raw_outside_and_missing_pages_and_body_links() {
        let root = vault(&[("wiki/a.md", "---\n---\n"), ("raw/src.md", "raw\n")]);
        assert!(
            relate(
                root.path(),
                "raw/src.md",
                "wiki/a.md",
                EdgeKind::Related,
                false
            )
            .is_err()
        );
        assert!(
            relate(
                root.path(),
                "../wiki/a.md",
                "wiki/b.md",
                EdgeKind::Related,
                false
            )
            .is_err()
        );
        assert!(
            relate(
                root.path(),
                "wiki/none.md",
                "wiki/a.md",
                EdgeKind::Related,
                false
            )
            .is_err()
        );
        assert!(
            relate(
                root.path(),
                "wiki/a.md",
                "wiki/b.md",
                EdgeKind::Mentions,
                false
            )
            .is_err()
        );
        assert!(relate(root.path(), "wiki/a.md", "", EdgeKind::Related, false).is_err());
        assert!(
            relate(
                root.path(),
                "wiki/a.md",
                "wiki/b]].md",
                EdgeKind::Related,
                false
            )
            .is_err()
        );
        assert_eq!(read(root.path(), "raw/src.md"), "raw\n");
    }
}
