//! Which pages of the vault name which artifacts (2026-11-10).
//!
//! An agent that writes up what it learned cites the page it published: the
//! window's artifact id — `p-` and sixteen hex digits — in the wiki page's
//! `source:` or its body. Measured on one vault: 101 of 2,413 pages name 76
//! artifacts, and nothing joined the two. The gallery could not say which
//! knowledge a page had become, a search for a page did not say where it was
//! written up, and an agent shown a wiki page was not shown the page it cites.
//!
//! This is the one join. [`Citations::scan`] walks `wiki/`, reads each page
//! once per change of its file (a stat pass for a vault seen before), and
//! answers the artifacts each page names; the artifact store knows nothing of
//! the vault and the vault's graph knows nothing of the store. A citation is
//! what a person wrote, never inferred: no title is matched, no folder guessed.
//!
//! Every bound is in [`LIMITS`].

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

use crate::second_brain::WIKI_DIR;

/// The bounds of one scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Pages one scan looks at; a vault past this is read in part, and the
    /// answer says nothing of the rest.
    pub pages_max: usize,
    /// How much of one page is read for the ids it names.
    pub page_bytes_max: u64,
    /// How deep `wiki/` is walked.
    pub depth_max: usize,
    /// Pages named for one artifact, newest first.
    pub citing_per_artifact_max: usize,
    /// Artifacts one page may name; a page that lists every artifact ever
    /// made (an index, a log) is not a page about any of them.
    pub artifacts_per_page_max: usize,
}

pub const LIMITS: Limits = Limits {
    pages_max: 20_000,
    page_bytes_max: 256 * 1024,
    depth_max: 4,
    citing_per_artifact_max: 8,
    artifacts_per_page_max: 12,
};

/// The mark a window artifact's id opens with, and how many hex digits follow.
const ID_MARK: &str = "p-";
const ID_HEX_DIGITS: usize = 16;

/// One page that names an artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Citing {
    /// Vault-relative path with `/` separators — `wiki/<slug>.md`.
    pub page: String,
    /// The page's `title:`, else its file's stem.
    pub title: String,
    /// The page's file, for a window that opens it.
    pub path: PathBuf,
    pub modified_ms: i64,
}

/// What one page's file said the last time it was read.
struct Held {
    len: u64,
    modified: Option<SystemTime>,
    title: String,
    ids: Vec<String>,
}

/// The vault's citations, with the per-page memory that makes a second scan a
/// stat pass.
#[derive(Default)]
pub struct Citations {
    pages: HashMap<PathBuf, Held>,
}

impl Citations {
    /// Every artifact a page of `root`'s `wiki/` names, with the pages that
    /// name it — newest first, bounded by [`LIMITS`]. A vault with no `wiki/`
    /// answers nothing.
    pub fn scan(&mut self, root: &Path) -> BTreeMap<String, Vec<Citing>> {
        let wiki = root.join(WIKI_DIR);
        let mut files = Vec::new();
        walk(&wiki, LIMITS.depth_max, &mut files);
        files.truncate(LIMITS.pages_max);
        let mut seen: HashMap<PathBuf, Held> = HashMap::with_capacity(files.len());
        let mut cited: BTreeMap<String, Vec<Citing>> = BTreeMap::new();
        for path in files {
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let (len, modified) = (meta.len(), meta.modified().ok());
            let held = match self.pages.remove(&path) {
                Some(held) if held.len == len && held.modified == modified => held,
                _ => {
                    let text = read_front(&path, LIMITS.page_bytes_max);
                    Held {
                        len,
                        modified,
                        title: title_of(&text, &path),
                        ids: cited_ids(&text),
                    }
                }
            };
            if !held.ids.is_empty() && held.ids.len() <= LIMITS.artifacts_per_page_max {
                let page = path.strip_prefix(root).map_or_else(
                    |_| path.to_string_lossy().into_owned(),
                    |inside| {
                        inside
                            .components()
                            .map(|part| part.as_os_str().to_string_lossy())
                            .collect::<Vec<_>>()
                            .join("/")
                    },
                );
                let modified_ms = modified
                    .and_then(|at| at.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |since| {
                        i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
                    });
                for id in &held.ids {
                    cited.entry(id.clone()).or_default().push(Citing {
                        page: page.clone(),
                        title: held.title.clone(),
                        path: path.clone(),
                        modified_ms,
                    });
                }
            }
            seen.insert(path, held);
        }
        self.pages = seen;
        for pages in cited.values_mut() {
            pages.sort_by(|a, b| {
                b.modified_ms
                    .cmp(&a.modified_ms)
                    .then_with(|| a.page.cmp(&b.page))
            });
            pages.truncate(LIMITS.citing_per_artifact_max);
        }
        cited
    }
}

/// Markdown files under `dir`, in path order so two scans of one vault answer
/// the same; dot-folders (`.obsidian`, `.zerocode`) are not pages.
fn walk(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = read.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        if files.len() >= LIMITS.pages_max {
            return;
        }
        let dotted = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if dotted {
            continue;
        }
        if path.is_dir() {
            if depth > 0 {
                walk(&path, depth - 1, files);
            }
        } else if path.extension().is_some_and(|ext| ext == "md") {
            files.push(path);
        }
    }
}

fn read_front(path: &Path, max: u64) -> String {
    use std::io::Read as _;
    let Ok(file) = std::fs::File::open(path) else {
        return String::new();
    };
    let mut held = Vec::new();
    let _ = file.take(max).read_to_end(&mut held);
    String::from_utf8_lossy(&held).into_owned()
}

/// The page's `title:` from its frontmatter, quotes off; else its file's stem.
fn title_of(text: &str, path: &Path) -> String {
    let stem = || {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let Some(front) = text.strip_prefix("---") else {
        return stem();
    };
    let front = front.split("\n---").next().unwrap_or_default();
    front
        .lines()
        .find_map(|line| line.strip_prefix("title:"))
        .map(|said| said.trim().trim_matches(['"', '\'']).trim().to_string())
        .filter(|said| !said.is_empty())
        .unwrap_or_else(stem)
}

/// The window artifact ids a text names, each once, in the order first named:
/// `p-` and exactly sixteen lowercase hex digits, standing as a word of its
/// own — `step-0123456789abcdef` names none, and neither does an id with a
/// seventeenth digit.
#[must_use]
pub fn cited_ids(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let hex = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
    let word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_';
    let mut ids: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(ID_MARK) {
        let at = from + found;
        from = at + ID_MARK.len();
        let digits = from..from + ID_HEX_DIGITS;
        if digits.end > bytes.len() || (at > 0 && word(bytes[at - 1])) {
            continue;
        }
        if !bytes[digits.clone()].iter().all(|byte| hex(*byte))
            || bytes.get(digits.end).is_some_and(|byte| word(*byte))
        {
            continue;
        }
        let id = &text[at..digits.end];
        if !ids.iter().any(|held| held == id) {
            ids.push(id.to_string());
        }
        from = digits.end;
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(root: &Path, name: &str, text: &str) -> PathBuf {
        let path = root.join(WIKI_DIR).join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    /// An id is `p-` and sixteen hex digits standing as a word of its own.
    #[test]
    fn an_id_is_cited_only_as_a_word_of_its_own() {
        let text = "시안은 p-0123456789abcdef(v2)와 `p-fedcba9876543210`, 다시 p-0123456789abcdef.\n\
            step-0123456789abcdef · p-0123456789abcdef0 · p-0123456789ABCDEF · p-012345 · xp-0123456789abcdef";
        assert_eq!(
            cited_ids(text),
            ["p-0123456789abcdef", "p-fedcba9876543210"]
        );
        assert!(cited_ids("p-").is_empty());
        assert!(cited_ids("").is_empty());
        assert_eq!(cited_ids("p-0123456789abcdef"), ["p-0123456789abcdef"]);
    }

    /// The scan answers, for each artifact, the pages that name it — in the
    /// frontmatter or the body, in a folder under `wiki/` too — newest first,
    /// under each page's own title. A page read once is not read again until
    /// its file changes, and a page that lists more artifacts than a page is
    /// about names none.
    #[test]
    fn the_scan_joins_each_artifact_to_the_pages_that_name_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let first = page(
            root,
            "gallery.md",
            "---\ntitle: \"갤러리의 세 이음매\"\nsource: \"시안 (Artifact p-aaaaaaaaaaaaaaaa)\"\n---\n\n# 본문\n",
        );
        page(
            root,
            "notes/board.md",
            "# 보드\n\n옛 시안은 p-aaaaaaaaaaaaaaaa, 새 시안은 p-bbbbbbbbbbbbbbbb.\n",
        );
        page(root, "plain.md", "# 아무것도 인용하지 않는다\n");
        let many: String = (0..=LIMITS.artifacts_per_page_max)
            .map(|n| format!("p-{n:016x}\n"))
            .collect();
        page(root, "log.md", &many);
        page(root, ".obsidian/hidden.md", "p-cccccccccccccccc");
        std::fs::write(
            root.join(WIKI_DIR).join("not-a-page.txt"),
            "p-dddddddddddddddd",
        )
        .unwrap();

        let mut citations = Citations::default();
        let cited = citations.scan(root);
        assert_eq!(
            cited.keys().collect::<Vec<_>>(),
            ["p-aaaaaaaaaaaaaaaa", "p-bbbbbbbbbbbbbbbb"]
        );
        let both = &cited["p-aaaaaaaaaaaaaaaa"];
        let mut pages: Vec<(&str, &str)> = both
            .iter()
            .map(|one| (one.page.as_str(), one.title.as_str()))
            .collect();
        pages.sort_unstable();
        assert_eq!(
            pages,
            [
                ("wiki/gallery.md", "갤러리의 세 이음매"),
                ("wiki/notes/board.md", "board")
            ]
        );
        assert_eq!(
            both.iter()
                .find(|one| one.page == "wiki/gallery.md")
                .unwrap()
                .path,
            first
        );
        assert_eq!(cited["p-bbbbbbbbbbbbbbbb"].len(), 1);

        // A second scan reads nothing it has read: a page whose bytes changed
        // under an unchanged stamp is still what it was, and a page whose
        // stamp moved is read again.
        let again = citations.scan(root);
        assert_eq!(again, cited);
        std::fs::write(
            &first,
            "---\ntitle: 고친 제목\n---\n\np-eeeeeeeeeeeeeeee 하나만 말한다, 조금 더 길게.\n",
        )
        .unwrap();
        let moved = citations.scan(root);
        assert_eq!(moved["p-eeeeeeeeeeeeeeee"][0].title, "고친 제목");
        assert_eq!(
            moved["p-aaaaaaaaaaaaaaaa"]
                .iter()
                .map(|one| one.page.as_str())
                .collect::<Vec<_>>(),
            ["wiki/notes/board.md"]
        );
        // A vault with no wiki answers nothing.
        assert!(Citations::default().scan(&root.join("nowhere")).is_empty());
    }
}
