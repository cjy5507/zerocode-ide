//! The vault's page kinds (t-43992 unit 2, design §4's third unit): Karpathy's
//! LLM-wiki ontology as one `type:` frontmatter key. Six kinds, read off a page
//! by the graph scan, counted by the lint where a page names none, and given
//! to pages that name none by a one-shot backfill (`zerocode vault-types`)
//! that only ever adds the key — a `type:` someone wrote stands as written,
//! and nothing under `raw/` is opened.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::civil;
use crate::second_brain::{
    MAX_STATUS_ENTRIES, WIKI_DIR, WIKI_INDEX_FILE, WIKI_LOG_FILE, WIKI_OVERVIEW_FILE,
};
use crate::second_brain_graph::{
    MARKDOWN_SUFFIX, parse_frontmatter, split_frontmatter, split_scalar_list, unquote,
};
use crate::second_brain_live::write_atomically;

/// The frontmatter key a page's kind is written under.
pub const TYPE_KEY: &str = "type";

/// Words that set two things side by side in a title or a file name. Not
/// 「비교」: a Korean title uses it as an everyday noun.
const COMPARISON_MARKERS: [&str; 2] = ["vs", "versus"];

/// Tag words that say what a page is beyond the kinds' own names, checked in
/// this order; the first kind with a matching word wins. A word a vault also
/// files its subjects under — `product`, `qa`, `answer`, `video` — is not one.
const TAG_WORDS: [(PageType, &[&str]); 3] = [
    (
        PageType::Entity,
        &[
            "person",
            "people",
            "org",
            "organization",
            "company",
            "tool",
            "project",
            "library",
            "인물",
            "회사",
            "도구",
        ],
    ),
    (
        PageType::Source,
        &[
            "paper", "article", "book", "talk", "podcast", "논문", "기사", "책",
        ],
    ),
    (PageType::Comparison, &COMPARISON_MARKERS),
];

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
        let word = unquote(value.trim()).trim();
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str().eq_ignore_ascii_case(word))
    }
}

/// The kind the backfill gives a page that names none, from its id, its
/// title and its tags: the overview by its path alone, a kind a tag names
/// outright, a comparison its title or file name sets side by side, then the
/// tag words above, and a concept when nothing says otherwise.
#[must_use]
pub fn guess(id: &str, title: &str, tags: &[String]) -> PageType {
    if id == WIKI_OVERVIEW_FILE {
        return PageType::Overview;
    }
    let words: Vec<String> = tags
        .iter()
        .map(|tag| tag.trim().trim_start_matches('#').to_lowercase())
        .collect();
    if let Some(kind) = words
        .iter()
        .filter_map(|word| PageType::parse(word))
        .find(|kind| *kind != PageType::Overview)
    {
        return kind;
    }
    if [id, title].into_iter().any(sets_side_by_side) {
        return PageType::Comparison;
    }
    TAG_WORDS
        .iter()
        .find(|(_, vocabulary)| words.iter().any(|word| vocabulary.contains(&word.as_str())))
        .map_or(PageType::Concept, |(kind, _)| *kind)
}

/// Whether a title or a file name sets two things side by side.
fn sets_side_by_side(text: &str) -> bool {
    text.split(|held: char| !held.is_alphanumeric())
        .any(|word| {
            COMPARISON_MARKERS
                .iter()
                .any(|marker| word.eq_ignore_ascii_case(marker))
        })
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
/// frontmatter to hold one. The key goes first, in the text's own line ending.
#[must_use]
pub fn with_type(text: &str, kind: PageType) -> Typing {
    let (mark, body) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    let newline = if body.starts_with("---\r\n") {
        "\r\n"
    } else if body.starts_with("---\n") {
        "\n"
    } else {
        return Typing::NoFrontmatter;
    };
    let (front, after) = split_frontmatter(body);
    // A fence that never closes hands the whole text back: no frontmatter.
    if after.len() == body.len() {
        return Typing::NoFrontmatter;
    }
    if let Some(value) = parse_frontmatter(front).get(TYPE_KEY) {
        return Typing::Declared(unquote(value.trim()).trim().to_string());
    }
    let opened = "---".len() + newline.len();
    Typing::Added(format!(
        "{mark}---{newline}{TYPE_KEY}: {}{newline}{}",
        kind.as_str(),
        &body[opened..]
    ))
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
    /// Pages that are not UTF-8 text — left as they are.
    pub unreadable: Vec<String>,
    /// Pages written; zero unless asked to write.
    pub written: usize,
    /// The line appended to `wiki/log.md`, when anything was written.
    pub log_line: Option<String>,
}

/// Plan — and with `write`, apply — a kind for every page under `wiki/` that
/// names none. The index and the log are ledgers, not pages to type. Each
/// write replaces one page whole (temp beside, then rename); a write that
/// fails stops the run, and what was written before it is still logged.
/// `now_ms` stamps the log line.
pub fn backfill(root: &Path, write: bool, now_ms: i64) -> io::Result<Backfill> {
    let mut plan = Backfill::default();
    let mut typed: Vec<(PathBuf, String)> = Vec::new();
    for (path, id) in wiki_pages(root)? {
        let Ok(text) = String::from_utf8(fs::read(&path)?) else {
            plan.unreadable.push(id);
            continue;
        };
        let stem = Path::new(&id)
            .file_stem()
            .and_then(|held| held.to_str())
            .unwrap_or_default();
        let (front, _) = split_frontmatter(&text);
        let fields = parse_frontmatter(front);
        let title = fields
            .get("title")
            .map(|held| unquote(held.trim()).trim())
            .filter(|held| !held.is_empty())
            .unwrap_or(stem);
        let tags = fields
            .get("tags")
            .map(|held| split_scalar_list(held))
            .unwrap_or_default();
        let kind = guess(&id, title, &tags);
        match with_type(&text, kind) {
            Typing::Added(retyped) => {
                plan.planned.push((id, kind));
                typed.push((path, retyped));
            }
            Typing::Declared(value) => {
                if PageType::parse(&value).is_none() {
                    plan.kept.push((id, value));
                }
            }
            Typing::NoFrontmatter => plan.no_frontmatter.push(id),
        }
    }
    if !write {
        return Ok(plan);
    }
    let mut failed = None;
    for (path, retyped) in &typed {
        if let Err(error) = write_atomically(path, retyped.as_bytes()) {
            failed = Some(error);
            break;
        }
        plan.written += 1;
    }
    if plan.written > 0 {
        let line = log_line(&plan.planned[..plan.written], now_ms);
        append_line(&root.join(WIKI_LOG_FILE), &line)?;
        plan.log_line = Some(line);
    }
    failed.map_or(Ok(plan), Err)
}

/// Every Markdown page under `wiki/` but the two ledgers, as (path, id),
/// sorted by id. Links are not followed, and the walk reads at most
/// [`MAX_STATUS_ENTRIES`] entries.
fn wiki_pages(root: &Path) -> io::Result<Vec<(PathBuf, String)>> {
    let mut pages = Vec::new();
    let mut folders = vec![root.join(WIKI_DIR)];
    let mut read = 0usize;
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            read += 1;
            if read > MAX_STATUS_ENTRIES {
                return Err(io::Error::other(format!(
                    "{WIKI_DIR}/ holds more than {MAX_STATUS_ENTRIES} entries"
                )));
            }
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                folders.push(path);
                continue;
            }
            let id = path
                .strip_prefix(root)
                .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if kind.is_file()
                && id.ends_with(MARKDOWN_SUFFIX)
                && id != WIKI_INDEX_FILE
                && id != WIKI_LOG_FILE
            {
                pages.push((path, id));
            }
        }
    }
    pages.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(pages)
}

/// The one log line a written backfill leaves, in the log's own shape. The
/// time is UTC and says so: this command does not know the person's zone.
fn log_line(typed: &[(String, PageType)], now_ms: i64) -> String {
    let day = civil::iso_date_of(now_ms, 0);
    let since_midnight = now_ms.rem_euclid(civil::MS_PER_DAY);
    let ms_per_minute = civil::MS_PER_HOUR / 60;
    let hour = since_midnight / civil::MS_PER_HOUR;
    let minute = since_midnight / ms_per_minute % 60;
    let counts: Vec<String> = PageType::ALL
        .into_iter()
        .filter_map(|kind| {
            let count = typed.iter().filter(|(_, held)| *held == kind).count();
            (count > 0).then(|| format!("{} {count}", kind.as_str()))
        })
        .collect();
    format!(
        "- {day} {hour:02}:{minute:02} UTC — zerocode vault-types → 페이지 {}개에 {TYPE_KEY} 추가 ({})",
        typed.len(),
        counts.join(" · ")
    )
}

/// Append `line` to the log, opening a new line first when the log's last
/// line was left open. Only the log's last byte is read.
fn append_line(path: &Path, line: &str) -> io::Result<()> {
    let open_ended = match File::open(path) {
        Ok(mut file) => {
            if file.metadata()?.len() == 0 {
                false
            } else {
                file.seek(SeekFrom::End(-1))?;
                let mut last = [0u8; 1];
                file.read_exact(&mut last)?;
                last[0] != b'\n'
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let lead = if open_ended { "\n" } else { "" };
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(format!("{lead}{line}\n").as_bytes())
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

    /// Tag words a vault also files its subjects under — `qa` for testing,
    /// `answer` for a reply, `product` for the app's direction, `video` for a
    /// capture feature — say nothing about a page's kind, and neither does
    /// 「비교」, an everyday noun in a Korean title. The overview is one path:
    /// no tag or word makes a second one.
    #[test]
    fn a_word_with_a_second_meaning_leaves_a_page_a_concept() {
        let pages: [(&str, &str, &[&str]); 7] = [
            (
                "wiki/release-checks.md",
                "Release checks",
                &["qa", "testing"],
            ),
            (
                "wiki/reply-door.md",
                "A reply is typed once",
                &["answer", "terminal"],
            ),
            (
                "wiki/direction.md",
                "Where the app goes next",
                &["product", "direction"],
            ),
            (
                "wiki/screen-capture.md",
                "Screen capture keeps a clip",
                &["video", "capture"],
            ),
            (
                "wiki/sorted-tables.md",
                "두 표는 비교 전에 정렬한다",
                &["testing"],
            ),
            ("wiki/site-map.md", "Site map", &["map"]),
            ("wiki/summary.md", "Summary", &["overview"]),
        ];
        let misread: Vec<(&str, PageType)> = pages
            .into_iter()
            .map(|(id, title, words)| (id, guess(id, title, &tags(words))))
            .filter(|(_, kind)| *kind != PageType::Concept)
            .collect();
        assert!(misread.is_empty(), "{misread:?}");
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
        assert!(
            line.contains(&format!("페이지 {}개에", done.written)),
            "pages are counted, not numbered: {line}"
        );
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

    /// An answer drawn from two or more pages is how the vault compounds:
    /// both texts an agent reads before it answers say to file it back,
    /// naming the kind, the key for the pages it rests on, and the two files
    /// every write touches.
    #[test]
    fn the_guide_and_the_skill_file_an_answer_from_two_or_more_pages_back() {
        let skill = include_str!("../../../skills/second-brain/SKILL.md");
        let kind = format!("`{TYPE_KEY}: {}`", PageType::Synthesis.as_str());
        let rests_on = format!(
            "`{}`",
            crate::second_brain_graph::EdgeKind::DependsOn.as_str()
        );
        for (name, text, heading) in [
            (
                "AGENTS_GUIDE",
                crate::second_brain::AGENTS_GUIDE,
                "## 질문 답변",
            ),
            ("SKILL.md", skill, "## Answer from the vault"),
        ] {
            let (_, after) = text
                .split_once(heading)
                .unwrap_or_else(|| panic!("{name} has no {heading} section"));
            let section = after.split("\n## ").next().unwrap_or(after);
            for words in [
                kind.as_str(),
                rests_on.as_str(),
                crate::second_brain::WIKI_INDEX_FILE,
                WIKI_LOG_FILE,
            ] {
                assert!(
                    section.contains(words),
                    "{name}'s {heading} section does not name {words}"
                );
            }
        }
    }
}
