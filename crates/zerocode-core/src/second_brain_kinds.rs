//! The vault's page kinds (t-43992 unit 2, design §4's third unit): Karpathy's
//! LLM-wiki ontology as one `type:` frontmatter key. Six kinds, read off a page
//! by the graph scan, counted by the lint where a page names none, and given
//! to pages that name none by a one-shot backfill (`zerocode vault-types`)
//! that only ever adds the key — a `type:` someone wrote stands as written,
//! and nothing under `raw/` is opened.

use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// What a page is, as its `type:` key says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageType {
    /// A summary of one raw source.
    Source,
    /// A person, an organisation, a tool or a product.
    Entity,
    /// An idea, a method or a pattern.
    Concept,
    /// Two or more things set side by side.
    Comparison,
    /// The vault's one-page map, `wiki/overview.md`.
    Overview,
    /// An answer drawn from two or more pages.
    Synthesis,
}

impl PageType {
    /// Every kind, in the order the schema lists them.
    pub const ALL: [Self; 6] = [
        Self::Source,
        Self::Entity,
        Self::Concept,
        Self::Comparison,
        Self::Overview,
        Self::Synthesis,
    ];

    /// The word the key carries.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Entity => "entity",
            Self::Concept => "concept",
            Self::Comparison => "comparison",
            Self::Overview => "overview",
            Self::Synthesis => "synthesis",
        }
    }

    /// The kind a written `type:` value names — trimmed, unquoted, any case —
    /// or nothing for a value outside the six.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let _ = value;
        None
    }
}

/// The kind the backfill gives a page that names none, from its id, its
/// title and its tags.
#[must_use]
pub fn guess(id: &str, title: &str, tags: &[String]) -> PageType {
    let _ = (id, title, tags);
    PageType::Concept
}

/// What putting a `type:` key into one page's text comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typing {
    /// The text with `type: <kind>` as the first line of its frontmatter.
    Added(String),
    /// The frontmatter already says `type:` — this value, kept as written.
    Declared(String),
    /// No frontmatter to put the key in.
    NoFrontmatter,
}

/// `text` with `kind` declared, unless it declares a type already or has no
/// frontmatter to hold one.
#[must_use]
pub fn with_type(text: &str, kind: PageType) -> Typing {
    let _ = (text, kind);
    Typing::NoFrontmatter
}

/// What one backfill over a vault found, and did when asked to write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Backfill {
    /// Pages that name no type, each with the kind it gets, sorted by id.
    pub planned: Vec<(String, PageType)>,
    /// Pages whose `type:` names something outside the six, with that value —
    /// left as written.
    pub kept: Vec<(String, String)>,
    /// Pages with no frontmatter to hold the key — left as they are.
    pub no_frontmatter: Vec<String>,
    /// Pages written; zero unless asked to write.
    pub written: usize,
    /// The line appended to `wiki/log.md`, when anything was written.
    pub log_line: Option<String>,
}

/// Plan — and with `write`, apply — a kind for every page under `wiki/` that
/// names none. `now_ms` stamps the log line.
pub fn backfill(root: &Path, write: bool, now_ms: i64) -> io::Result<Backfill> {
    let _ = (root, write, now_ms);
    Ok(Backfill::default())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::civil;
    use crate::second_brain::{RAW_DIR, WIKI_LOG_FILE};

    fn tags(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_string()).collect()
    }

    #[test]
    fn a_written_type_names_one_of_six_kinds_or_none() {
        assert_eq!(PageType::parse("concept"), Some(PageType::Concept));
        assert_eq!(PageType::parse(" \"Entity\" "), Some(PageType::Entity));
        assert_eq!(PageType::parse("'synthesis'"), Some(PageType::Synthesis));
        assert_eq!(PageType::parse("gotcha"), None);
        assert_eq!(PageType::parse(""), None);
        for kind in PageType::ALL {
            assert_eq!(PageType::parse(kind.as_str()), Some(kind));
        }
    }

    /// The spec's three fixture pages, and the three kinds a tag or a path
    /// names outright.
    #[test]
    fn the_backfill_reads_a_kind_off_the_id_the_title_and_the_tags() {
        assert_eq!(
            guess("wiki/obsidian.md", "Obsidian", &tags(&["tool", "editor"])),
            PageType::Entity
        );
        assert_eq!(
            guess(
                "wiki/karpathy-vs-graphify.md",
                "Karpathy vs Graphify",
                &tags(&["llm"])
            ),
            PageType::Comparison
        );
        assert_eq!(
            guess("wiki/atomic-notes.md", "Atomic notes", &tags(&["writing"])),
            PageType::Concept
        );
        assert_eq!(guess("wiki/overview.md", "개요", &[]), PageType::Overview);
        assert_eq!(
            guess(
                "wiki/answer.md",
                "What the vault says",
                &tags(&["#synthesis"])
            ),
            PageType::Synthesis
        );
        assert_eq!(
            guess("wiki/paper.md", "A paper", &tags(&["Paper"])),
            PageType::Source
        );
    }

    #[test]
    fn a_type_goes_first_in_the_frontmatter_and_a_written_one_stands() {
        assert_eq!(
            with_type("---\ntitle: A\n---\nbody\n", PageType::Entity),
            Typing::Added("---\ntype: entity\ntitle: A\n---\nbody\n".to_string())
        );
        assert_eq!(
            with_type("---\r\ntitle: A\r\n---\r\nbody\r\n", PageType::Concept),
            Typing::Added("---\r\ntype: concept\r\ntitle: A\r\n---\r\nbody\r\n".to_string()),
            "a page written with CRLF keeps CRLF"
        );
        assert_eq!(
            with_type("---\ntype: gotcha\n---\nbody\n", PageType::Concept),
            Typing::Declared("gotcha".to_string())
        );
        assert_eq!(
            with_type("# no frontmatter\n", PageType::Concept),
            Typing::NoFrontmatter
        );
    }

    fn vault(files: &[(&str, &str)]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        crate::second_brain::setup(root.path()).unwrap();
        for (relative, body) in files {
            let path = root.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        root
    }

    const TOOL: &str = "---\ntags: [tool]\n---\nObsidian is an editor.\n";
    const VERSUS: &str = "---\ntitle: Karpathy vs Graphify\n---\nTwo wikis.\n";
    const PLAIN: &str = "---\ntags: [writing]\n---\nOne idea per page.\n";
    const GOTCHA: &str = "---\ntype: gotcha\n---\nA trap.\n";
    const BARE: &str = "No frontmatter here.\n";
    const RAW: &str = "---\ntags: [tool]\n---\nraw text\n";

    fn backfill_vault() -> tempfile::TempDir {
        vault(&[
            ("wiki/obsidian.md", TOOL),
            ("wiki/karpathy-vs-graphify.md", VERSUS),
            ("wiki/atomic-notes.md", PLAIN),
            ("wiki/trap.md", GOTCHA),
            ("wiki/bare.md", BARE),
            ("raw/clip.md", RAW),
        ])
    }

    fn now() -> i64 {
        civil::days_from_civil(2026, 10, 12) * civil::MS_PER_DAY + 3_723_000
    }

    #[test]
    fn a_dry_run_plans_every_untyped_page_and_writes_nothing() {
        let root = backfill_vault();
        let log_before = fs::read_to_string(root.path().join(WIKI_LOG_FILE)).unwrap();
        let plan = backfill(root.path(), false, now()).unwrap();
        // The overview the setup wrote says `type: overview` already, and the
        // index and the log are the vault's ledgers, not pages to type.
        assert_eq!(
            plan.planned,
            [
                ("wiki/atomic-notes.md".to_string(), PageType::Concept),
                (
                    "wiki/karpathy-vs-graphify.md".to_string(),
                    PageType::Comparison
                ),
                ("wiki/obsidian.md".to_string(), PageType::Entity),
            ]
        );
        assert_eq!(
            plan.kept,
            [("wiki/trap.md".to_string(), "gotcha".to_string())]
        );
        assert_eq!(plan.no_frontmatter, ["wiki/bare.md"]);
        assert_eq!((plan.written, plan.log_line), (0, None));
        assert_eq!(
            fs::read_to_string(root.path().join("wiki/obsidian.md")).unwrap(),
            TOOL
        );
        assert_eq!(
            fs::read_to_string(root.path().join(WIKI_LOG_FILE)).unwrap(),
            log_before
        );
    }

    #[test]
    fn a_written_backfill_adds_the_key_logs_one_line_and_never_opens_raw() {
        let root = backfill_vault();
        let log_before = fs::read_to_string(root.path().join(WIKI_LOG_FILE)).unwrap();
        let done = backfill(root.path(), true, now()).unwrap();
        assert_eq!((done.planned.len(), done.written), (3, 3), "{done:?}");
        let read = |relative: &str| fs::read_to_string(root.path().join(relative)).unwrap();
        assert_eq!(
            read("wiki/obsidian.md"),
            format!("---\ntype: entity\n{}", &TOOL[4..])
        );
        assert_eq!(
            read("wiki/karpathy-vs-graphify.md"),
            format!("---\ntype: comparison\n{}", &VERSUS[4..])
        );
        assert_eq!(
            read("wiki/atomic-notes.md"),
            format!("---\ntype: concept\n{}", &PLAIN[4..])
        );
        assert_eq!(read("wiki/trap.md"), GOTCHA, "a written type stands");
        assert_eq!(read("wiki/bare.md"), BARE);
        assert_eq!(
            read(&format!("{RAW_DIR}/clip.md")),
            RAW,
            "raw/ is never opened"
        );

        let log = read(WIKI_LOG_FILE);
        let added = log
            .strip_prefix(log_before.as_str())
            .expect("the log is only appended to");
        assert_eq!(added.lines().count(), 1, "{added}");
        let line = done.log_line.expect("a write is logged");
        assert_eq!(added, format!("{line}\n"));
        assert!(line.starts_with("- 2026-10-12 01:02 UTC — "), "{line}");
        assert!(line.contains("zerocode vault-types"), "{line}");

        let again = backfill(root.path(), true, now()).unwrap();
        assert!(again.planned.is_empty(), "{again:?}");
        assert_eq!((again.written, again.log_line), (0, None));
        assert_eq!(read(WIKI_LOG_FILE), log, "nothing written, nothing logged");
    }

    /// The schema's words live in the two texts an agent reads before it
    /// writes a page; each names the key, every kind, and the overview page.
    #[test]
    fn the_guide_and_the_skill_name_the_key_and_every_kind() {
        let skill = include_str!("../../../skills/second-brain/SKILL.md");
        for (name, text) in [
            ("AGENTS_GUIDE", crate::second_brain::AGENTS_GUIDE),
            ("SKILL.md", skill),
        ] {
            assert!(text.contains("`type`"), "{name} does not name the key");
            for kind in PageType::ALL {
                assert!(
                    text.contains(&format!("`{}`", kind.as_str())),
                    "{name} does not name `{}`",
                    kind.as_str()
                );
            }
            assert!(
                text.contains(crate::second_brain::WIKI_OVERVIEW_FILE),
                "{name} does not name the overview page"
            );
        }
        assert!(
            skill.contains("zerocode vault-types"),
            "the skill does not name the backfill"
        );
    }
}
