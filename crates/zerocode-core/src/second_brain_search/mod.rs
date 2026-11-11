//! The vault's search: a few words in, the passages that answer them out.
//!
//! One index of passages answers. It covers the vault's pages (`wiki/`), its
//! originals (`raw/`) and any folder a person added, each read where it lies
//! and each its own layer: the curated pages first, the evidence after. A
//! search walks the folders with `stat` alone, reads only the files that
//! changed, and answers with the files the words point at — each with the
//! passage that holds the most of them and the line it starts on.
//!
//! No model, no embedding and no database: words, counted
//! (`second_brain_related`'s arithmetic), and a file format of this crate's
//! own (`segment`).
//!
//! ```text
//! settings  what is read: the folders, and one table of bounds
//! passages  where a file's passages begin and end
//! segment   one index file: passages by the words they hold
//! index     a folder's index files, kept in step with the folder
//! rank      which files the words point at
//! ```
//!
//! The index lives in `<vault>/.zerocode/search-index/` and is a copy of
//! nothing: removed, cut short or unwritable, the search reads the files
//! whole and answers the same.

mod index;
mod passages;
mod rank;
mod segment;
mod settings;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub use index::Freshness;
pub use rank::{NAME_WEIGHT, NEIGHBOR_RULE, NeighborRule, PASSAGE_WEIGHT};
pub use settings::{
    ADDED_FOLDER_NOTICE, INDEX_DIR, Kind, PROSE_EXTENSIONS, RECORD_EXTENSIONS, SETTINGS_FILE,
    Settings, Source, Table, add_source, read as read_settings, remove_source,
};

use index::Layer;
use rank::{Arrival, Corpus, Ranked};
use segment::Damaged;

use crate::second_brain::WIKI_DIR;
use crate::second_brain_graph::{EdgeKind, MARKDOWN_SUFFIX};
use crate::second_brain_related::{
    MAX_RELATED_LINE_BYTES, MAX_RELATED_RELATIONS_PER_LINE, Replacement, asked_tokens, clipped,
    standing_page, tokenize,
};

/// Answers of a layer that carry their passage whole; the rest carry the one
/// line of it that holds the most of what was asked.
pub const PASSAGE_HITS: usize = 3;
/// How much of a passage an answer shows, in bytes.
pub const PASSAGE_SHOW_BYTES: usize = 600;
/// Answers a layer gets when it is shown after another: the evidence under
/// the pages is a few pointers, not a second list.
pub const LATER_LAYER_HITS: usize = 3;
/// Answers the ranking is asked for, for each one shown. A page that was
/// replaced gives its place to the page that replaced it, and two that end at
/// one page are one answer, so the list is shorter once that is done.
const RANKED_PER_SHOWN: usize = 2;
/// How many places a text longer than its room may be shown from, per room:
/// the stretch shown starts on one of them.
const SHOWN_STARTS: usize = 4;

/// What is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request<'a> {
    pub words: &'a str,
    /// Answers the first layer shown may give.
    pub limit: usize,
    /// One layer by name, or every layer.
    pub layer: Option<&'a str>,
}

/// How a file came to be an answer without holding the words: the relation
/// it was brought in by and the answer at the other end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Via {
    pub kind: EdgeKind,
    pub seed: String,
    /// Whether the answer declared the relation, or this file did.
    pub from_seed: bool,
}

impl Via {
    /// The words zo's recall says the same thing with.
    #[must_use]
    pub fn note(&self) -> String {
        if self.from_seed {
            format!("← {} of [[{}]]", self.kind.as_str(), self.seed)
        } else {
            format!("→ {} [[{}]]", self.kind.as_str(), self.seed)
        }
    }
}

/// One file of the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub layer: String,
    /// What a `[[link]]` would call it: `wiki/a/b` for a page, the layer and
    /// the path below its folder for anything else.
    pub slug: String,
    pub path: PathBuf,
    pub title: String,
    pub tags: Vec<String>,
    /// The relations the file declares, as it wrote them.
    pub relations: Vec<(EdgeKind, String)>,
    pub score: f64,
    pub via: Option<Via>,
    /// The line of its best passage that holds the most of what was asked.
    pub line: Option<String>,
    /// The line of the file its best passage starts on.
    pub line_number: Option<u32>,
    /// The passage itself, for the first [`PASSAGE_HITS`] answers of a layer.
    pub passage: Option<String>,
}

impl Hit {
    /// The file on one line: its title, its tags, the relations it declares
    /// and how it arrived.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut text = self.title.clone();
        if !self.tags.is_empty() {
            let _ = write!(text, " · tags: {}", self.tags.join(", "));
        }
        for (kind, target) in self.relations.iter().take(MAX_RELATED_RELATIONS_PER_LINE) {
            let _ = write!(text, " · {}: [[{target}]]", kind.as_str());
        }
        if let Some(via) = &self.via {
            let _ = write!(text, " · {}", via.note());
        }
        text
    }
}

/// What one layer's index held and how it got there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerReport {
    pub layer: String,
    pub files: usize,
    pub passages: usize,
    pub freshness: Freshness,
    /// Files read to bring the index in step.
    pub read: usize,
    /// Whether the folder holds more files than the table lets one source add.
    pub capped: bool,
    pub found: usize,
    pub took_ms: u64,
}

/// The answer: the files, layer by layer in the order the layers are read,
/// and what each layer's index did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Answer {
    pub hits: Vec<Hit>,
    pub layers: Vec<LayerReport>,
    pub notices: Vec<String>,
}

/// The files of `vault` and of the folders added to it that `request` points
/// at.
///
/// # Errors
///
/// The request names a layer the vault's settings do not have.
pub fn search(vault: &Path, request: &Request<'_>) -> Result<Answer, String> {
    let settings = settings::read(vault);
    let sources: Vec<&Source> = settings
        .sources
        .iter()
        .filter(|held| request.layer.is_none_or(|layer| held.layer == layer))
        .collect();
    if sources.is_empty() {
        let known: BTreeSet<&str> = settings
            .sources
            .iter()
            .map(|held| held.layer.as_str())
            .collect();
        return Err(format!(
            "no layer named `{}` — one of {}",
            request.layer.unwrap_or_default(),
            known.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    let asked = asked_tokens(request.words);
    let index_dir = settings::index_dir(vault);
    let (table, asked, index_dir) = (&settings.table, &asked, &index_dir);
    // No layer needs another: each is brought in step and asked on its own.
    let answered: Vec<Option<(LayerReport, Vec<Hit>)>> = std::thread::scope(|scope| {
        let asking: Vec<_> = (0_usize..)
            .zip(&sources)
            .map(|(at, source)| {
                let limit = if at == 0 {
                    request.limit
                } else {
                    request.limit.min(LATER_LAYER_HITS)
                };
                scope.spawn(move || {
                    ask_layer(&index_dir.join(&source.key), source, table, asked, limit)
                })
            })
            .collect();
        asking
            .into_iter()
            .map(|held| held.join().ok().flatten())
            .collect()
    });
    let mut answer = Answer {
        notices: settings.notice.into_iter().collect(),
        ..Answer::default()
    };
    for (source, held) in sources.iter().zip(answered) {
        if !source.root.is_dir() {
            answer.notices.push(format!(
                "{} is not there — layer {} is empty",
                source.root.display(),
                source.layer
            ));
        }
        match held {
            Some((report, hits)) => {
                if report.capped {
                    answer.notices.push(format!(
                        "{} holds more than {} files — the rest are not read",
                        source.root.display(),
                        table.max_files
                    ));
                }
                answer.layers.push(report);
                answer.hits.extend(hits);
            }
            None => answer
                .notices
                .push(format!("layer {} could not be read", source.layer)),
        }
    }
    Ok(answer)
}

/// One layer's answer. An index file that reads wrong in the middle of a
/// question is thrown away and the question asked again of a new one.
fn ask_layer(
    dir: &Path,
    source: &Source,
    table: &Table,
    asked: &BTreeSet<String>,
    limit: usize,
) -> Option<(LayerReport, Vec<Hit>)> {
    let began = Instant::now();
    let asking =
        |layer: &mut Layer| rank::rank(layer, asked, limit.saturating_mul(RANKED_PER_SHOWN));
    let mut layer = index::open(dir, source, table, segment::word_hash).ok()?;
    let ranked = match asking(&mut layer) {
        Ok(ranked) => ranked,
        Err(Damaged) => {
            index::discard(dir);
            layer = index::open(dir, source, table, segment::word_hash).ok()?;
            asking(&mut layer).ok()?
        }
    };
    let ranked = standing(&layer, source, ranked, limit);
    let hits = hits_of(&layer, source, table, asked, &ranked);
    let report = LayerReport {
        layer: source.layer.clone(),
        files: layer.standing() as usize,
        passages: layer.standing_passages(),
        freshness: layer.freshness,
        read: layer.read,
        capped: layer.capped,
        found: hits.len(),
        took_ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
    };
    Some((report, hits))
}

/// The answer with every replaced page given up for the page that stands in
/// its place (`second_brain_related::standing_page`), each page once.
///
/// The page that stands takes the place the replaced one earned and says
/// whose it was. When its own words found it too, that finding keeps its
/// passage and moves up to the better of the two places.
fn standing(layer: &Layer, source: &Source, ranked: Vec<Ranked>, limit: usize) -> Vec<Ranked> {
    let replaced_by = |doc: &u32| -> Vec<Replacement<u32>> {
        layer
            .edges()
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Supersedes && edge.to == *doc)
            .map(|edge| Replacement {
                page: edge.from,
                written_ms: layer.record(edge.from).stamp.modified_ms,
                slug: slug_of(source, &layer.record(edge.from).path),
            })
            .collect()
    };
    let mut kept: Vec<Ranked> = Vec::with_capacity(ranked.len());
    for one in &ranked {
        let stands = standing_page(one.doc, replaced_by);
        if kept.iter().any(|held| held.doc == stands) {
            continue;
        }
        kept.push(if stands == one.doc {
            one.clone()
        } else {
            // Its own finding, when the words found it further down.
            ranked
                .iter()
                .find(|other| other.doc == stands)
                .cloned()
                .unwrap_or(Ranked {
                    doc: stands,
                    score: one.score,
                    boost: one.boost,
                    passage: None,
                    arrival: Some(Arrival {
                        kind: EdgeKind::Supersedes,
                        seed: one.doc,
                        from_seed: false,
                    }),
                })
        });
    }
    kept.truncate(limit);
    kept
}

fn hits_of(
    layer: &Layer,
    source: &Source,
    table: &Table,
    asked: &BTreeSet<String>,
    ranked: &[Ranked],
) -> Vec<Hit> {
    ranked
        .iter()
        .enumerate()
        .map(|(at, one)| {
            let record = layer.record(one.doc);
            let path = file_path(&source.root, &record.path);
            // The passage is read from the file where it lies; a file that
            // moved since the index read it shows no passage rather than the
            // wrong bytes.
            let passage = one.passage.and_then(|passage| {
                let place = layer.place(passage);
                let text = index::read_text(&path, table.max_file_bytes)?;
                let start = place.start as usize;
                let held = text.get(start..start + place.len as usize)?.to_string();
                Some((place.line, held))
            });
            Hit {
                layer: source.layer.clone(),
                slug: slug_of(source, &record.path),
                title: record.title.clone(),
                tags: record.tags.clone(),
                relations: record
                    .links
                    .iter()
                    .filter(|(kind, _)| *kind != EdgeKind::Mentions)
                    .cloned()
                    .collect(),
                score: one.score,
                via: one.arrival.map(|arrival| Via {
                    kind: arrival.kind,
                    seed: slug_of(source, &layer.record(arrival.seed).path),
                    from_seed: arrival.from_seed,
                }),
                line: passage
                    .as_ref()
                    .and_then(|(_, text)| pointer_line(text, asked)),
                line_number: passage.as_ref().map(|(line, _)| *line),
                passage: passage
                    .filter(|_| at < PASSAGE_HITS)
                    .map(|(_, text)| shown(text.trim(), PASSAGE_SHOW_BYTES, asked)),
                path,
            }
        })
        .collect()
}

/// A file below a folder, joined one component at a time: the index spells
/// paths with `/` on every platform, and the host does not.
fn file_path(root: &Path, below: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(below.split('/'));
    path
}

/// What a `[[link]]` would call a file: a page of the vault without its
/// extension, anything else by its layer and its path.
fn slug_of(source: &Source, below: &str) -> String {
    if source.kind == Kind::Wiki {
        let stem = below
            .len()
            .checked_sub(MARKDOWN_SUFFIX.len())
            .filter(|at| {
                below.is_char_boundary(*at) && below[*at..].eq_ignore_ascii_case(MARKDOWN_SUFFIX)
            })
            .map_or(below, |at| &below[..at]);
        format!("{WIKI_DIR}/{stem}")
    } else {
        format!("{}/{below}", source.layer)
    }
}

/// `text` within `budget` bytes. A text that fits is shown whole. A longer
/// one is shown as the stretch of it that holds the most of the words asked
/// — the first such stretch — with an ellipsis wherever it was cut: a long
/// passage's answer is as often at its end as at its start, and a reader
/// shown only the start would have to open the file after all.
fn shown(text: &str, budget: usize, asked: &BTreeSet<String>) -> String {
    const ELLIPSIS: &str = "…";
    if text.len() <= budget {
        return text.to_string();
    }
    // A stretch that does not start the text gives up an ellipsis' room.
    let room = budget.saturating_sub(ELLIPSIS.len());
    let step = (budget / SHOWN_STARTS).max(1);
    let mut best = (0, 0);
    let mut start = 0;
    while start < text.len() {
        while !text.is_char_boundary(start) {
            start += 1;
        }
        let stretch = stretch_of(&text[start..], room);
        let held = tokenize(stretch).intersection(asked).count();
        if held > best.1 {
            best = (start, held);
        }
        if start + stretch.len() >= text.len() {
            break;
        }
        start += step;
    }
    match best.0 {
        0 => clipped(text, budget),
        start if text.len() - start <= room => format!("{ELLIPSIS}{}", &text[start..]),
        start => format!("{ELLIPSIS}{}", clipped(&text[start..], room)),
    }
}

/// The start of `text`, `room` bytes at most, ending between characters.
fn stretch_of(text: &str, room: usize) -> &str {
    let mut end = room.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The line of a passage that holds most of the words asked, without its
/// Markdown marks and cut to a pointer's length. The `# ` heading is skipped,
/// because it repeats the title the summary already shows, and a line of
/// text beats a section heading holding as many words, because it says more.
fn pointer_line(passage: &str, asked: &BTreeSet<String>) -> Option<String> {
    let mut best: Option<((usize, bool), &str)> = None;
    for line in passage.lines() {
        let line = line.trim();
        if line.starts_with("# ") {
            continue;
        }
        let said = !line.starts_with('#');
        let line = line.trim_start_matches(['#', '-', '>', '*', ' ']).trim();
        if line.is_empty() {
            continue;
        }
        let held = tokenize(line).intersection(asked).count();
        let rank = (held, said);
        if held > 0 && best.is_none_or(|(most, _)| rank > most) {
            best = Some((rank, line));
        }
    }
    best.map(|(_, line)| shown(line, MAX_RELATED_LINE_BYTES, asked))
}

#[cfg(test)]
mod tests {
    use super::rank::tests::Whole;
    use super::*;
    use std::fs;

    const LIMIT: usize = 8;

    struct Vault {
        dir: tempfile::TempDir,
    }

    impl Vault {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("vault");
            fs::create_dir_all(dir.path().join(WIKI_DIR)).unwrap();
            fs::create_dir_all(dir.path().join(crate::second_brain::RAW_DIR)).unwrap();
            Self { dir }
        }

        fn root(&self) -> &Path {
            self.dir.path()
        }

        fn write(&self, below: &str, text: &str) {
            let path = file_path(self.root(), below);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn remove(&self, below: &str) {
            fs::remove_file(file_path(self.root(), below)).unwrap();
        }

        fn ask(&self, words: &str) -> Answer {
            self.ask_in(words, None)
        }

        fn ask_in(&self, words: &str, layer: Option<&str>) -> Answer {
            search(
                self.root(),
                &Request {
                    words,
                    limit: LIMIT,
                    layer,
                },
            )
            .unwrap()
        }

        fn index_files(&self, layer: &str) -> Vec<PathBuf> {
            let mut files: Vec<PathBuf> =
                fs::read_dir(settings::index_dir(self.root()).join(layer))
                    .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
                    .unwrap_or_default();
            files.sort();
            files
        }
    }

    fn slugs(answer: &Answer) -> Vec<&str> {
        answer.hits.iter().map(|hit| hit.slug.as_str()).collect()
    }

    fn freshness(answer: &Answer, layer: &str) -> Freshness {
        answer
            .layers
            .iter()
            .find(|held| held.layer == layer)
            .map(|held| held.freshness)
            .expect("the layer answered")
    }

    /// The page the words name comes first with the passage that holds them,
    /// the line of the file it starts on and its one best line; the pages
    /// come before the originals, and each says which layer it is.
    #[test]
    fn an_answer_names_the_file_its_passage_and_the_line_and_pages_come_before_originals() {
        let vault = Vault::new();
        vault.write(
            "wiki/harbor-crane.md",
            "---\ntitle: \"Harbor Crane\"\ntags: [port]\ndepends_on: [[tide-table]]\n---\n\n# Harbor Crane\n\n## Night shift\n\nThe crane lifts cargo at night when the tide is high.\n\nA second paragraph about ropes.\n",
        );
        vault.write(
            "wiki/tide-table.md",
            "# Tide table\n\nHigh water times by the door.\n",
        );
        vault.write("wiki/cooking.md", "# Cooking\n\nUnrelated soup.\n");
        vault.write(
            "raw/shift-log.txt",
            "22:10 the crane lifts cargo from the blue ship\n\n23:40 rain\n",
        );
        let answer = vault.ask("crane cargo");
        assert_eq!(
            slugs(&answer),
            ["wiki/harbor-crane", "wiki/tide-table", "raw/shift-log.txt"]
        );
        let first = &answer.hits[0];
        assert_eq!(
            (first.layer.as_str(), first.title.as_str()),
            ("wiki", "Harbor Crane")
        );
        assert_eq!(
            first.path,
            vault.root().join("wiki").join("harbor-crane.md")
        );
        assert_eq!(
            first.passage.as_deref(),
            Some(
                "# Harbor Crane\n\n## Night shift\n\nThe crane lifts cargo at night when the tide is high."
            )
        );
        assert_eq!(first.line_number, Some(7));
        assert_eq!(
            first.line.as_deref(),
            Some("The crane lifts cargo at night when the tide is high.")
        );
        assert_eq!(
            first.summary(),
            "Harbor Crane · tags: port · depends_on: [[tide-table]]"
        );
        // The page the answer depends on holds none of the words and says how it came.
        let second = &answer.hits[1];
        assert_eq!(
            second.summary(),
            "tide-table · ← depends_on of [[wiki/harbor-crane]]"
        );
        assert_eq!(
            (second.passage.as_deref(), second.line.as_deref()),
            (None, None)
        );
        let original = &answer.hits[2];
        assert_eq!(original.layer, "raw");
        assert_eq!(original.line_number, Some(1));
        assert_eq!(
            original.passage.as_deref(),
            Some("22:10 the crane lifts cargo from the blue ship\n\n23:40 rain")
        );
        assert_eq!(
            answer
                .layers
                .iter()
                .map(|held| (held.layer.as_str(), held.files, held.found))
                .collect::<Vec<_>>(),
            [("wiki", 3, 2), ("raw", 1, 1)]
        );
    }

    /// A page another page says it replaced is not an answer: the page that
    /// replaced it is, once, in the place the old one earned, saying whose.
    #[test]
    fn a_replaced_page_gives_its_place_to_the_page_that_replaced_it() {
        let vault = Vault::new();
        vault.write(
            "wiki/lifting-2024.md",
            "The crane lifts forty tons of cargo.\n",
        );
        vault.write(
            "wiki/lifting-2026.md",
            "---\nsupersedes: [[lifting-2024]]\n---\n\nThe new hoist carries sixty tons.\n",
        );
        vault.write("wiki/cooking.md", "Unrelated soup.\n");
        let answer = vault.ask("crane cargo");
        assert_eq!(slugs(&answer), ["wiki/lifting-2026"]);
        assert_eq!(
            answer.hits[0].summary(),
            "lifting-2026 · supersedes: [[lifting-2024]] · → supersedes [[wiki/lifting-2024]]"
        );
        assert_eq!(answer.hits[0].passage, None);

        // Found by its own words too: one answer, with its own passage.
        vault.write(
            "wiki/lifting-2026.md",
            "---\nsupersedes: [[lifting-2024]]\n---\n\nThe new crane carries sixty tons of cargo.\n",
        );
        let answer = vault.ask("crane cargo");
        assert_eq!(slugs(&answer), ["wiki/lifting-2026"]);
        assert_eq!(
            answer.hits[0].line.as_deref(),
            Some("The new crane carries sixty tons of cargo.")
        );
        assert_eq!(answer.hits[0].via, None);
    }

    /// The first search builds the index, the second finds it in step and
    /// reads no file, and one layer can be asked alone.
    #[test]
    fn the_index_is_built_once_and_one_layer_can_be_asked_alone() {
        let vault = Vault::new();
        vault.write("wiki/crane.md", "The crane lifts cargo.\n");
        vault.write("raw/log.txt", "the crane was oiled\n");
        let first = vault.ask("crane");
        assert_eq!(freshness(&first, "wiki"), Freshness::Built);
        let again = vault.ask("crane");
        assert_eq!(freshness(&again, "wiki"), Freshness::Fresh);
        assert_eq!(again.layers.iter().map(|held| held.read).sum::<usize>(), 0);
        assert_eq!(again.hits, first.hits);
        assert_eq!(slugs(&vault.ask_in("crane", Some("raw"))), ["raw/log.txt"]);
        assert_eq!(slugs(&vault.ask_in("crane", Some("wiki"))), ["wiki/crane"]);
        let refused = search(
            vault.root(),
            &Request {
                words: "crane",
                limit: LIMIT,
                layer: Some("nope"),
            },
        );
        assert_eq!(
            refused.unwrap_err(),
            "no layer named `nope` — one of raw, wiki"
        );
    }

    /// An index another release left — one whose rules for cutting a text
    /// into words and passages may not be this one's — is not read as this
    /// one's: it is built again, once.
    #[test]
    fn an_index_built_under_another_releases_rules_is_built_again() {
        let vault = Vault::new();
        vault.write("wiki/crane.md", "The crane lifts cargo.\n");
        assert_eq!(freshness(&vault.ask("crane"), WIKI_DIR), Freshness::Built);
        assert_eq!(freshness(&vault.ask("crane"), WIKI_DIR), Freshness::Fresh);

        // The same files, as a release with other rules would have named them.
        let manifest = vault
            .index_files(WIKI_DIR)
            .into_iter()
            .find(|path| path.file_name().unwrap() == "manifest.json")
            .expect("the manifest");
        let written = fs::read_to_string(&manifest).unwrap();
        let other = written.replace(&format!("|{}|", settings::RULES), "|0.0.0-other|");
        fs::write(&manifest, other).unwrap();

        let answer = vault.ask("crane");
        assert_eq!(freshness(&answer, WIKI_DIR), Freshness::Built);
        assert_eq!(slugs(&answer), ["wiki/crane"]);
        assert_eq!(freshness(&vault.ask("crane"), WIKI_DIR), Freshness::Fresh);
    }

    /// A changed file answers with what it says now, a removed file no longer
    /// answers, a new one does — and each time only what moved is read.
    #[test]
    fn a_changed_file_answers_with_its_new_words_and_a_removed_one_is_gone() {
        let vault = Vault::new();
        for at in 0..40 {
            vault.write(
                &format!("wiki/page-{at:02}.md"),
                &format!("Page {at} is about ballast and keel number {at}.\n"),
            );
        }
        vault.write("wiki/dock-notes.md", "The crane lifts cargo.\n");
        assert_eq!(slugs(&vault.ask("crane")), ["wiki/dock-notes"]);
        assert!(vault.ask("lighthouse").hits.is_empty());

        vault.write(
            "wiki/dock-notes.md",
            "The lighthouse keeper keeps the lamp and nothing else at all.\n",
        );
        let changed = vault.ask("lighthouse");
        assert_eq!(slugs(&changed), ["wiki/dock-notes"]);
        assert_eq!(
            (freshness(&changed, "wiki"), changed.layers[0].read),
            (Freshness::Updated, 1)
        );
        assert!(
            vault.ask("crane").hits.is_empty(),
            "the old words left with the old text"
        );

        vault.write("wiki/new-page.md", "A winch pulls the mooring line.\n");
        let added = vault.ask("winch");
        assert_eq!(slugs(&added), ["wiki/new-page"]);
        assert_eq!(freshness(&added, "wiki"), Freshness::Updated);

        vault.remove("wiki/dock-notes.md");
        let removed = vault.ask("lighthouse");
        assert!(removed.hits.is_empty());
        assert_eq!(removed.layers[0].files, 41);
        assert_eq!(freshness(&vault.ask("winch"), "wiki"), Freshness::Fresh);
    }

    /// Past a share of the base the folder is written whole again and the
    /// overlay goes; a change taken back leaves the base alone.
    #[test]
    fn many_changes_build_the_base_again_and_a_change_taken_back_drops_the_overlay() {
        let vault = Vault::new();
        for at in 0..40 {
            vault.write(
                &format!("wiki/page-{at:02}.md"),
                &format!("Page {at} is about ballast.\n"),
            );
        }
        vault.ask("ballast");
        assert_eq!(
            vault.index_files("wiki").len(),
            3,
            "a manifest, a lock and a base"
        );
        vault.write("wiki/extra.md", "A winch pulls the mooring line.\n");
        assert_eq!(freshness(&vault.ask("winch"), "wiki"), Freshness::Updated);
        assert_eq!(vault.index_files("wiki").len(), 4, "and an overlay");
        vault.remove("wiki/extra.md");
        let back = vault.ask("winch");
        assert!(back.hits.is_empty());
        assert_eq!(
            vault.index_files("wiki").len(),
            3,
            "the overlay went with the change"
        );

        let table = Table::default();
        for at in 0..=table.rebuild_floor {
            vault.write(
                &format!("wiki/more-{at:02}.md"),
                &format!("More {at} about a capstan.\n"),
            );
        }
        // One word half the pages hold is what chance gives; with the number
        // only one of them holds, it names that page.
        let rebuilt = vault.ask("capstan more 05");
        assert_eq!(freshness(&rebuilt, "wiki"), Freshness::Built);
        assert_eq!(slugs(&rebuilt), ["wiki/more-05"]);
        assert_eq!(vault.index_files("wiki").len(), 3);
    }

    /// Made-up words: every pairing of a start and an end, so a vault of them
    /// names no thing and no person. The Korean ones share syllables, so their
    /// two-character pieces overlap the way real words' do.
    fn vocabulary() -> Vec<String> {
        const STARTS: [&str; 12] = [
            "ka", "lo", "mi", "ne", "su", "ta", "ri", "vo", "ze", "hu", "ba", "de",
        ];
        const ENDS: [&str; 12] = [
            "rn", "lt", "sk", "mp", "nd", "rt", "st", "lm", "rk", "ns", "ft", "pt",
        ];
        const KOREAN_STARTS: [&str; 6] = ["항구", "등대", "화물", "기중", "선박", "부두"];
        const KOREAN_ENDS: [&str; 4] = ["지기", "선장", "기사", "터"];
        let latin = STARTS
            .iter()
            .flat_map(|start| ENDS.iter().map(move |end| format!("{start}{end}")));
        let korean = KOREAN_STARTS
            .iter()
            .flat_map(|start| KOREAN_ENDS.iter().map(move |end| format!("{start}{end}")));
        latin.chain(korean).collect()
    }

    /// A made-up vault large enough for every rule to bite: files of a few
    /// passages, some with a name and tags, some declaring a relation, some
    /// linking in their text. Early words of the vocabulary are in most
    /// files and late ones in few, so some matches are chance and some are not.
    fn made_up(vault: &Vault, files: usize, seed: u64) {
        let words = vocabulary();
        let mut state = seed;
        let mut next = move |below: usize| {
            // A linear congruential step: the same files every run.
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) as usize % below
        };
        for at in 0..files {
            let mut text = String::new();
            // The product of two draws leans toward the vocabulary's start.
            let word = |next: &mut dyn FnMut(usize) -> usize| {
                words[next(words.len()) * next(words.len()) / words.len()].as_str()
            };
            let named = at % 3 == 0;
            if named {
                let (first, second, tag) = (word(&mut next), word(&mut next), word(&mut next));
                let _ = writeln!(text, "---\ntitle: {first} {second}\ntags: [{tag}]");
                if at > 0 && at % 2 == 0 {
                    // Not `supersedes`: a replaced page gives up its place
                    // after the ranking these tests compare, by a rule with
                    // a test of its own.
                    let kind = ["depends_on", "related", "implements", "contradicts"][at / 6 % 4];
                    let _ = writeln!(text, "{kind}: [[file-{:03}]]", next(at));
                }
                text.push_str("---\n\n");
            }
            for passage in 0..1 + at % 4 {
                for _ in 0..4 + (at + passage) % 11 {
                    text.push_str(word(&mut next));
                    text.push(' ');
                }
                text.push_str("\n\n");
            }
            if at > 0 && at % 4 == 1 {
                let _ = writeln!(text, "[[file-{:03}]]", next(at));
            }
            vault.write(&format!("wiki/file-{at:03}.md"), &text);
        }
    }

    /// Questions about a made-up vault: whole passages of files in it, a few
    /// words, rare words, Korean words, and a word no file holds.
    fn questions(vault: &Vault) -> Vec<String> {
        let words = vocabulary();
        let first_passage = |at: usize| {
            let text =
                fs::read_to_string(vault.root().join("wiki").join(format!("file-{at:03}.md")))
                    .unwrap();
            let body = text.rsplit("---\n\n").next().unwrap().to_string();
            body.split("\n\n").next().unwrap().to_string()
        };
        let mut asked: Vec<String> = [5, 9, 12, 17].into_iter().map(first_passage).collect();
        asked.push(words[words.len() - 30].clone());
        asked.push(format!("{} {} {}", words[3], words[70], words[120]));
        asked.push(words[words.len() - 1].clone());
        asked.push(format!(
            "{} {}",
            words[words.len() - 2],
            words[words.len() - 9]
        ));
        asked.push(format!("{} {} {}", words[0], words[1], words[2]));
        asked.push("absent".to_string());
        asked
    }

    /// What the index must answer: the same files read whole into memory and
    /// asked with the same arithmetic, no index file and no hash between.
    fn read_whole(vault: &Vault, question: &str) -> Vec<(String, u64, Option<u32>)> {
        let settings = settings::read(vault.root());
        let source = &settings.sources[0];
        let (walked, _) = index::walk(source, &settings.table);
        let docs: Vec<_> = walked
            .iter()
            .map(|file| index::read_doc(source, &settings.table, file))
            .collect();
        let records: Vec<_> = docs.iter().map(|doc| doc.record.clone()).collect();
        let mut whole = Whole {
            files: docs
                .iter()
                .map(|doc| {
                    (
                        doc.record.path.clone(),
                        doc.name.clone(),
                        doc.passages
                            .iter()
                            .map(|(_, words)| words.clone())
                            .collect(),
                    )
                })
                .collect(),
            edges: index::edges_of(source, &records, &vec![true; records.len()]),
        };
        let ranked = rank::rank(&mut whole, &asked_tokens(question), LIMIT).unwrap();
        ranked
            .iter()
            .map(|one| {
                let line = one.passage.map(|passage| {
                    let at = (passage - whole.body_of(one.doc).start) as usize;
                    docs[one.doc as usize].passages[at].0.line
                });
                (whole.path(one.doc).to_string(), one.score.to_bits(), line)
            })
            .collect()
    }

    fn read_index(vault: &Vault, question: &str) -> Vec<(String, u64, Option<u32>)> {
        vault
            .ask_in(question, Some(WIKI_DIR))
            .hits
            .iter()
            .map(|hit| {
                let below = hit.path.strip_prefix(vault.root().join(WIKI_DIR)).unwrap();
                (
                    below.to_string_lossy().replace('\\', "/"),
                    hit.score.to_bits(),
                    hit.line_number,
                )
            })
            .collect()
    }

    /// Every question answered both ways and found alike; how many of them
    /// found a file at all.
    fn the_index_answers_as_the_files_do(vault: &Vault, asked: &[String], when: &str) -> usize {
        let mut answered = 0;
        for question in asked {
            let found = read_index(vault, question);
            assert_eq!(found, read_whole(vault, question), "{when}: {question}");
            answered += usize::from(!found.is_empty());
        }
        answered
    }

    /// The index and the files read whole give the same files, the same
    /// scores to the last bit and the same passages — when it is first built,
    /// after files change, after files go, and after the base is built again.
    #[test]
    fn the_index_answers_exactly_as_the_files_read_whole_do_through_every_change() {
        let vault = Vault::new();
        made_up(&vault, 120, 7);
        let asked = questions(&vault);
        let answered = the_index_answers_as_the_files_do(&vault, &asked, "first built");
        assert!(
            answered >= 2,
            "the questions find files: {answered} of {}",
            asked.len()
        );

        vault.write(
            "wiki/file-003.md",
            &format!(
                "---\ntitle: karn lolt\nrelated: [[file-090]]\n---\n\n{} 항구지기 등대선장\n",
                asked[0]
            ),
        );
        vault.write(
            "wiki/file-120.md",
            &format!("{} [[file-003]]\n\n{}\n", asked[1], asked[4]),
        );
        the_index_answers_as_the_files_do(&vault, &asked, "two files laid over");
        assert_eq!(
            freshness(&vault.ask_in("crane", Some(WIKI_DIR)), WIKI_DIR),
            Freshness::Fresh
        );

        vault.remove("wiki/file-090.md");
        vault.remove("wiki/file-011.md");
        the_index_answers_as_the_files_do(&vault, &asked, "two files gone, one of them linked to");

        vault.write("wiki/file-003.md", "mirn only now\n");
        the_index_answers_as_the_files_do(&vault, &asked, "a laid-over file changed again");

        for at in 20..60 {
            vault.write(
                &format!("wiki/file-{at:03}.md"),
                &format!("{} number {at}\n\n{}\n", asked[2], asked[5]),
            );
        }
        let rebuilt = vault.ask_in(&asked[2], Some(WIKI_DIR));
        assert_eq!(freshness(&rebuilt, WIKI_DIR), Freshness::Built);
        let answered = the_index_answers_as_the_files_do(&vault, &asked, "built again");
        assert!(answered >= 2, "the questions still find files: {answered}");
    }

    /// With every word's hash the same, the index still answers as the files
    /// do: a word is found by its own spelling, never by its slot.
    #[test]
    fn an_index_whose_hashes_all_collide_answers_as_the_files_do() {
        fn same(_: &[u8]) -> u64 {
            42
        }
        let vault = Vault::new();
        made_up(&vault, 30, 11);
        let settings = settings::read(vault.root());
        let source = &settings.sources[0];
        let dir = settings::index_dir(vault.root()).join("collide");
        for question in &questions(&vault) {
            let mut layer = index::open(&dir, source, &settings.table, same).unwrap();
            let found: Vec<(String, u64)> = rank::rank(&mut layer, &asked_tokens(question), LIMIT)
                .unwrap()
                .iter()
                .map(|one| (layer.path(one.doc).to_string(), one.score.to_bits()))
                .collect();
            let whole: Vec<(String, u64)> = read_whole(&vault, question)
                .into_iter()
                .map(|(path, score, _)| (path, score))
                .collect();
            assert_eq!(found, whole, "{question}");
        }
    }

    /// The index removed, cut in half, overwritten with noise or named by a
    /// manifest that is not one: the search answers the same and leaves a
    /// whole index behind.
    #[test]
    fn a_missing_or_damaged_index_is_built_again_and_the_answer_is_the_same() {
        let vault = Vault::new();
        made_up(&vault, 80, 3);
        let question = &questions(&vault)[0];
        let expected = read_whole(&vault, question);
        assert!(!expected.is_empty());
        assert_eq!(read_index(&vault, question), expected);
        let named = |vault: &Vault, start: &str| {
            vault
                .index_files(WIKI_DIR)
                .into_iter()
                .find(|path| {
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(start)
                })
                .expect("the index file")
        };

        // Cut in half.
        let base = named(&vault, "base-");
        let bytes = fs::read(&base).unwrap();
        fs::write(&base, &bytes[..bytes.len() / 2]).unwrap();
        assert_eq!(
            read_index(&vault, question),
            expected,
            "the base cut in half"
        );

        // The words' passages overwritten: the header still reads true.
        let base = named(&vault, "base-");
        let mut bytes = fs::read(&base).unwrap();
        let len = bytes.len();
        bytes[len / 2..len - 8].fill(0xff);
        fs::write(&base, bytes).unwrap();
        assert_eq!(
            read_index(&vault, question),
            expected,
            "the base's second half overwritten"
        );

        // Gone.
        fs::remove_file(named(&vault, "base-")).unwrap();
        assert_eq!(read_index(&vault, question), expected, "the base removed");
        fs::write(named(&vault, "manifest"), b"{ not json").unwrap();
        assert_eq!(
            read_index(&vault, question),
            expected,
            "the manifest unreadable"
        );
        fs::remove_dir_all(settings::index_dir(vault.root())).unwrap();
        assert_eq!(
            read_index(&vault, question),
            expected,
            "the whole index removed"
        );

        // An overlay cut short costs the base nothing it cannot rebuild.
        vault.write("wiki/file-001.md", &format!("{question} and more\n"));
        let expected = read_whole(&vault, question);
        assert_eq!(read_index(&vault, question), expected);
        let overlay = named(&vault, "overlay-");
        let bytes = fs::read(&overlay).unwrap();
        fs::write(&overlay, &bytes[..bytes.len() - 5]).unwrap();
        assert_eq!(
            read_index(&vault, question),
            expected,
            "the overlay cut short"
        );
        assert_eq!(
            freshness(&vault.ask_in(question, Some(WIKI_DIR)), WIKI_DIR),
            Freshness::Fresh
        );
    }

    /// A vault whose index cannot be written is read whole on every search
    /// and answers the same.
    #[test]
    fn a_vault_that_cannot_hold_an_index_is_read_whole() {
        let vault = Vault::new();
        made_up(&vault, 40, 5);
        let question = &questions(&vault)[0];
        let expected = read_whole(&vault, question);
        assert!(!expected.is_empty());
        // A file where the index folder would be: nothing can be made under it.
        fs::create_dir_all(vault.root().join(".zerocode")).unwrap();
        fs::write(settings::index_dir(vault.root()), b"in the way").unwrap();
        let answer = vault.ask_in(question, Some(WIKI_DIR));
        assert_eq!(freshness(&answer, WIKI_DIR), Freshness::Scanned);
        assert_eq!(read_index(&vault, question), expected);
    }

    /// Six searches at once on a vault with no index, and six more while
    /// files change under them: every answer is one a whole reading gives,
    /// and one generation of the index is left.
    #[test]
    fn six_searches_at_once_build_one_index_and_all_answer_alike() {
        let vault = Vault::new();
        made_up(&vault, 100, 13);
        let question = &questions(&vault)[0];
        let expected = read_whole(&vault, question);
        assert!(!expected.is_empty());
        let together = |vault: &Vault| -> Vec<Vec<(String, u64, Option<u32>)>> {
            std::thread::scope(|scope| {
                let asking: Vec<_> = (0..6)
                    .map(|_| scope.spawn(|| read_index(vault, question)))
                    .collect();
                asking
                    .into_iter()
                    .map(|held| held.join().unwrap())
                    .collect()
            })
        };
        for found in together(&vault) {
            assert_eq!(found, expected);
        }
        assert_eq!(
            vault.index_files(WIKI_DIR).len(),
            4,
            "a manifest, a lock, a base and its relations"
        );

        vault.write(
            "wiki/file-002.md",
            &format!("{question} said twice {question}\n"),
        );
        vault.remove("wiki/file-050.md");
        let expected = read_whole(&vault, question);
        for found in together(&vault) {
            assert_eq!(found, expected);
        }
        assert_eq!(vault.index_files(WIKI_DIR).len(), 5, "and one overlay");
    }

    /// A folder outside the vault is read where it lies once it is added,
    /// under its own layer, and no longer once it is removed.
    #[test]
    fn a_folder_a_person_adds_is_searched_in_place_under_its_layer() {
        let vault = Vault::new();
        vault.write("wiki/crane.md", "The crane lifts cargo.\n");
        let outside = tempfile::tempdir().expect("reports");
        let reports = outside.path().join("Reports");
        fs::create_dir_all(reports.join("week-1")).unwrap();
        fs::create_dir_all(reports.join("node_modules")).unwrap();
        fs::write(
            reports.join("week-1").join("crane-check.md"),
            "# Crane check\n\nThe crane passed its load test.\n",
        )
        .unwrap();
        fs::write(
            reports.join("node_modules").join("readme.md"),
            "crane crane crane\n",
        )
        .unwrap();
        fs::write(reports.join("data.json"), "{\"crane\": true}\n").unwrap();

        let source = add_source(vault.root(), &reports, None).unwrap();
        assert_eq!(
            (source.layer.as_str(), source.kind),
            ("reports", Kind::Added)
        );
        assert!(
            add_source(vault.root(), &reports, Some("again"))
                .unwrap_err()
                .contains("read already")
        );
        assert!(
            add_source(vault.root(), &vault.root().join("wiki"), None)
                .unwrap_err()
                .contains("inside the vault")
        );
        assert!(
            add_source(vault.root(), outside.path(), Some("Wiki!"))
                .unwrap_err()
                .contains("cannot name a layer")
        );
        assert!(
            add_source(vault.root(), &reports.join("missing"), None)
                .unwrap_err()
                .ends_with("is not a folder")
        );

        let answer = vault.ask("crane");
        assert_eq!(
            slugs(&answer),
            ["wiki/crane", "reports/week-1/crane-check.md"]
        );
        assert_eq!(
            answer.hits[1].path,
            reports
                .canonicalize()
                .unwrap()
                .join("week-1")
                .join("crane-check.md")
        );
        assert_eq!(answer.hits[1].line_number, Some(1));
        assert_eq!(
            slugs(&vault.ask_in("crane", Some("reports"))),
            ["reports/week-1/crane-check.md"]
        );
        assert_eq!(
            read_settings(vault.root())
                .sources
                .iter()
                .map(|held| held.layer.as_str())
                .collect::<Vec<_>>(),
            ["wiki", "raw", "reports"]
        );
        // Nothing was written beside the files that were read.
        assert_eq!(fs::read_dir(&reports).unwrap().count(), 3);

        let gone = remove_source(vault.root(), "reports").unwrap();
        assert_eq!(gone.len(), 1);
        assert_eq!(slugs(&vault.ask("crane")), ["wiki/crane"]);
        assert!(!settings::index_dir(vault.root()).join(&source.key).exists());
        assert!(
            remove_source(vault.root(), "reports")
                .unwrap_err()
                .starts_with("no added folder")
        );
    }

    /// Taking a folder out waits for the search that is writing its index:
    /// the files do not go from under a writer, and go once it is done.
    #[test]
    fn removing_a_folder_waits_for_the_search_writing_its_index() {
        /// How long the writer holds on before it lets go. Nothing can answer
        /// in that time, so the wait makes this test slower and never wrong.
        const WRITER_HOLDS: std::time::Duration = std::time::Duration::from_millis(300);

        let vault = Vault::new();
        let outside = tempfile::tempdir().expect("reports");
        fs::write(outside.path().join("crane.md"), "The crane lifts cargo.\n").unwrap();
        let source = add_source(vault.root(), outside.path(), Some("reports")).unwrap();
        assert_eq!(slugs(&vault.ask("crane")), ["reports/crane.md"]);
        let dir = settings::index_dir(vault.root()).join(&source.key);

        // A search writing the layer's index holds its lock.
        let writer = fs::File::options()
            .write(true)
            .open(dir.join(index::LOCK_FILE))
            .unwrap();
        writer.try_lock().unwrap();
        let (done, removed) = std::sync::mpsc::channel();
        let root = vault.root();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let gone = remove_source(root, "reports").map(|gone| gone.len());
                let _ = done.send(gone);
            });
            assert!(
                removed.recv_timeout(WRITER_HOLDS).is_err(),
                "the index went while a search was writing it"
            );
            assert!(dir.join(index::MANIFEST_FILE).is_file());
            writer.unlock().unwrap();
            assert_eq!(removed.recv().unwrap(), Ok(1));
        });
        assert!(!dir.exists());
        assert!(vault.ask("crane").hits.is_empty());
    }

    /// The table's defaults read prose alone; the settings file turns the
    /// record formats on, changes a bound, and an index built under another
    /// table is built again.
    #[test]
    fn the_settings_file_overrides_the_table_and_the_index_follows() {
        let vault = Vault::new();
        vault.write("raw/notes.md", "The crane lifts cargo.\n");
        vault.write("raw/dump.json", "{\"crane\": \"cargo\"}\n");
        vault.write(
            "raw/long.txt",
            &"crane cargo and a long tail of words\n".repeat(40),
        );
        let found = |answer: &Answer| {
            let mut found: Vec<String> = slugs(answer).into_iter().map(str::to_string).collect();
            found.sort();
            found
        };
        assert_eq!(
            found(&vault.ask_in("crane", Some("raw"))),
            ["raw/long.txt", "raw/notes.md"]
        );

        vault.write(
            ".zerocode/search.json",
            "{\"records\": true, \"maxFileBytes\": 1000, \"passageBytes\": 300}\n",
        );
        let answer = vault.ask_in("crane", Some("raw"));
        assert_eq!(found(&answer), ["raw/dump.json", "raw/notes.md"]);
        assert_eq!(freshness(&answer, "raw"), Freshness::Built);
        let table = read_settings(vault.root()).table;
        assert!(table.extensions.contains("json") && table.extensions.contains("md"));
        assert_eq!(
            (
                table.max_file_bytes,
                table.passage.most,
                table.passage.least
            ),
            (1000, 300, 20)
        );

        vault.write(".zerocode/search.json", "[1, 2]\n");
        let answer = vault.ask_in("crane", Some("raw"));
        assert_eq!(found(&answer), ["raw/long.txt", "raw/notes.md"]);
        assert!(
            answer.notices[0].ends_with("is not a JSON object — the defaults are used"),
            "{:?}",
            answer.notices
        );
    }

    /// A number in the settings file past any file's length — the largest a
    /// JSON number holds — is cut to the longest file read, and the search
    /// answers.
    #[test]
    fn a_passage_bound_past_any_file_is_cut_to_the_longest_file_read() {
        let vault = Vault::new();
        vault.write("wiki/crane.md", "The crane lifts cargo.\n");
        vault.write(
            ".zerocode/search.json",
            &format!("{{\"passageBytes\": {}}}\n", u64::MAX),
        );
        let whole = Table::default();
        let table = read_settings(vault.root()).table;
        assert_eq!(
            u64::try_from(table.passage.most).unwrap(),
            whole.max_file_bytes
        );
        assert_eq!(
            table.passage.least,
            table.passage.most * whole.passage.least / whole.passage.most
        );
        assert_eq!(slugs(&vault.ask("crane")), ["wiki/crane"]);

        vault.write(
            ".zerocode/search.json",
            &format!(
                "{{\"maxFileBytes\": 1000, \"passageBytes\": {}}}\n",
                u64::MAX
            ),
        );
        assert_eq!(read_settings(vault.root()).table.passage.most, 1000);
        assert_eq!(slugs(&vault.ask("crane")), ["wiki/crane"]);
    }

    /// The vault's own folders are read as its knowledge graph reads them: a
    /// page in a folder named like a build's output, in a folder whose name
    /// starts with a dot, or thirteen folders down is a page — in `wiki/` and
    /// in `raw/` alike. Only a folder a person added leaves such folders out.
    #[test]
    fn a_page_in_a_folder_named_like_a_builds_output_is_read_as_the_graph_reads_it() {
        use crate::second_brain_graph::{GraphCache, NodeKind};

        let vault = Vault::new();
        let deep = "a/".repeat(Table::default().max_depth + 1);
        vault.write("wiki/target/crane.md", "The crane lifts cargo.\n");
        vault.write(
            "wiki/node_modules/winch.md",
            "A winch pulls the mooring line.\n",
        );
        vault.write("wiki/.drafts/capstan.md", "The capstan turns at dawn.\n");
        vault.write(
            &format!("wiki/{deep}ballast.md"),
            "Ballast keeps the keel down.\n",
        );
        vault.write("wiki/cooking.md", "Unrelated soup.\n");
        vault.write("raw/target/shift-log.txt", "the crane was oiled\n");

        let asked = |words: &str, layer: &str| -> Vec<String> {
            slugs(&vault.ask_in(words, Some(layer)))
                .into_iter()
                .map(str::to_string)
                .collect()
        };
        assert_eq!(asked("crane", WIKI_DIR), ["wiki/target/crane"]);
        assert_eq!(asked("winch", WIKI_DIR), ["wiki/node_modules/winch"]);
        assert_eq!(asked("capstan", WIKI_DIR), ["wiki/.drafts/capstan"]);
        assert_eq!(asked("ballast", WIKI_DIR), [format!("wiki/{deep}ballast")]);
        assert_eq!(asked("crane", "raw"), ["raw/target/shift-log.txt"]);

        // File for file, the pages the graph holds.
        let settings = settings::read(vault.root());
        let (walked, _) = index::walk(&settings.sources[0], &settings.table);
        let searched: Vec<String> = walked
            .iter()
            .map(|file| format!("{WIKI_DIR}/{}", file.rel))
            .collect();
        let graph = GraphCache::new().scan(vault.root(), false);
        let mut pages: Vec<&str> = graph
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Page)
            .map(|node| node.id.as_str())
            .collect();
        pages.sort_unstable();
        assert_eq!(searched, pages);
        assert_eq!(searched.len(), 5);
    }

    /// The home and the journal link every page and are about none: they are
    /// not read, and neither is anything under the folder an application
    /// keeps for itself.
    #[test]
    fn the_home_the_journal_and_an_applications_own_folder_are_not_read() {
        let vault = Vault::new();
        vault.write("wiki/index.md", "crane crane crane\n");
        vault.write("wiki/log.md", "crane crane crane\n");
        vault.write("wiki/.obsidian/crane.md", "crane crane crane\n");
        vault.write(
            "wiki/sub/index.md",
            "A crane in a folder's own index page.\n",
        );
        let answer = vault.ask("crane");
        assert_eq!(slugs(&answer), ["wiki/sub/index"]);
        assert_eq!(answer.layers[0].files, 1);
    }

    /// A passage longer than what is shown is shown where the words are: the
    /// start when they are there, and the stretch that holds them, marked as
    /// cut on both sides, when they come later.
    #[test]
    fn a_long_passage_is_shown_where_the_words_asked_are() {
        let asked = asked_tokens("crane cargo");
        let filler = "ballast and keel and rope and ".repeat(30);
        let late = format!("{filler}then the crane lifts the cargo at last. {filler}");
        let cut = shown(&late, 200, &asked);
        assert!(cut.len() <= 200, "{} bytes", cut.len());
        assert!(cut.starts_with('…') && cut.ends_with('…'), "{cut}");
        assert!(cut.contains("crane") && cut.contains("cargo"), "{cut}");

        let early = format!("The crane lifts the cargo first. {filler}");
        let cut = shown(&early, 200, &asked);
        assert!(
            cut.starts_with("The crane lifts the cargo first.") && cut.ends_with('…'),
            "{cut}"
        );
        assert!(cut.len() <= 200, "{} bytes", cut.len());

        // The words at the very end: nothing is cut after them.
        let last = format!("{filler}the crane lifts the cargo");
        let cut = shown(&last, 200, &asked);
        assert!(
            cut.starts_with('…') && cut.ends_with("the crane lifts the cargo"),
            "{cut}"
        );
        assert!(cut.len() <= 200, "{} bytes", cut.len());

        // Words nowhere in it: the start, as before. A text that fits: whole.
        assert!(shown(&filler, 200, &asked).starts_with("ballast and keel"));
        assert_eq!(shown("short", 200, &asked), "short");
        // Characters wider than a byte are never cut through.
        let wide = format!("{}기중기가 화물을 든다", "항구 ".repeat(100));
        let cut = shown(&wide, 100, &asked_tokens("기중기 화물"));
        assert!(cut.len() <= 100 && cut.contains("기중기"), "{cut}");
    }

    /// A file with bytes that are not UTF-8 still answers with the right
    /// passage: its text is read the same way to index it and to show it, so
    /// a passage's place names the same bytes both times.
    #[test]
    fn a_file_with_bytes_that_are_not_utf8_answers_with_the_right_passage() {
        let vault = Vault::new();
        let mut bytes = b"A first line with a stray byte \xff\xfe in it, long enough to stand alone as a passage.\n\n".to_vec();
        bytes.extend_from_slice("The crane lifts cargo after the stray bytes.\n".as_bytes());
        fs::write(vault.root().join("raw").join("mixed.txt"), bytes).unwrap();
        let answer = vault.ask_in("crane cargo", Some("raw"));
        assert_eq!(slugs(&answer), ["raw/mixed.txt"]);
        assert_eq!(
            answer.hits[0].passage.as_deref(),
            Some("The crane lifts cargo after the stray bytes.")
        );
        assert_eq!(answer.hits[0].line_number, Some(3));
    }

    /// What a link would call a file: a page loses its extension however it
    /// is cased, and anything else keeps its layer and its whole path.
    #[test]
    fn a_slug_is_what_a_link_would_call_the_file() {
        let settings = settings::read(Path::new("vault"));
        let wiki = &settings.sources[0];
        assert_eq!(slug_of(wiki, "sub/Page.md"), "wiki/sub/Page");
        assert_eq!(slug_of(wiki, "NOTES.MD"), "wiki/NOTES");
        assert_eq!(slug_of(wiki, "한글.md"), "wiki/한글");
        assert_eq!(slug_of(wiki, "md"), "wiki/md");
        let added = Source {
            layer: "reports".to_string(),
            kind: Kind::Added,
            ..wiki.clone()
        };
        assert_eq!(slug_of(&added, "week/one.md"), "reports/week/one.md");
        assert_eq!(
            file_path(Path::new("root"), "a/b/c.md"),
            Path::new("root").join("a").join("b").join("c.md")
        );
    }

    /// A passage past the shown length is cut with an ellipsis, and only the
    /// first answers of a layer carry a passage; every answer carries its line.
    #[test]
    fn only_the_first_answers_carry_a_passage_and_a_long_one_is_cut() {
        let vault = Vault::new();
        for at in 0..5 {
            vault.write(
                &format!("wiki/crane-{at}.md"),
                &format!(
                    "The crane number {at} lifts cargo {}.\n",
                    "and more ".repeat(100)
                ),
            );
        }
        let answer = vault.ask("crane cargo");
        assert_eq!(answer.hits.len(), 5);
        for (at, hit) in answer.hits.iter().enumerate() {
            assert_eq!(hit.passage.is_some(), at < PASSAGE_HITS, "{at}");
            assert!(hit.line.as_deref().is_some_and(|line| line.len() <= MAX_RELATED_LINE_BYTES && line.ends_with('…')));
        }
        let passage = answer.hits[0].passage.as_deref().unwrap();
        assert!(
            passage.len() <= PASSAGE_SHOW_BYTES && passage.ends_with('…'),
            "{passage}"
        );
    }
}
