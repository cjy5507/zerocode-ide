//! Which blocks of a page's text a browser read should hand over, and which
//! it should fold away (t-6041, [`crate::jev::BROWSER_READ`]).
//!
//! `zerocode-browser read` hands an agent the visible text of a page, whole,
//! and most of a page is not what the agent came for: the site's navigation,
//! its masthead and footer, the cookie banner, the sponsored rail, the
//! related-links column. The page script cuts the body into blocks at the
//! landmarks and sections the page's author named
//! ([`crate::jev::BROWSER_READ_BLOCK_ROOTS`]), and this module asks one closed
//! choice per block — the page, or its furniture — over one state: the page's
//! title and, for each block, its structural path and the head of its text.
//! A block's body never leaves the machine; it is what the fold keeps or
//! drops, on the page this machine already holds.
//!
//! Nothing here calls anything. It builds the requests (cut into shards the
//! read asks side by side), reads a reply against the questions it asked,
//! decides what a verdict lets the fold drop, and writes the one line the
//! folded text carries in the dropped blocks' place. The wire, the door, the
//! ledger and the setting are the window's (`crates/zerocode-shell/src/browser_read.rs`).
//!
//! **Nothing gets worse for asking.** A block nobody asked about — past the
//! table's cap, in a shard that timed out, on a page of one block — is
//! content, and a read whose judgment did not come back whole is the read
//! that existed before this seat: the page, entire.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use crate::jev::choice::{self, Choice, ChoiceRefusal};
use crate::jev::door::cut;
use crate::jev::{
    BROWSER_READ_BLOCK_CAP, BROWSER_READ_CHROME, BROWSER_READ_CONTENT, BROWSER_READ_HEAD_CHAR_CAP,
    BROWSER_READ_OPTIONS, BROWSER_READ_PATH_CHAR_CAP, BROWSER_READ_SHARD_TARGET,
    BROWSER_READ_TITLE_CHAR_CAP, Cap, JevUse, shard,
};

/// Bumped whenever the instructions, a criterion or the state's shape
/// changes. A row carries it, so a reading taken under other words is never
/// read as evidence about these ones; `the_version_is_pinned_to_the_words`
/// holds it to [`crate::jev::rubric_fingerprint`].
pub const BROWSER_READ_RUBRIC_VERSION: u32 = 1;

/// The letter a question id opens with, so what comes back is named rather
/// than numbered. Spelled once: [`ask`] writes ids with it and
/// [`ReadAsk::read`] asks for each id it wrote.
const QUESTION_ID_PREFIX: &str = "b";

/// The words of every block's question. They are ours: a page's own text
/// reaches the model as state, never as an instruction. `{at}` is the
/// block's place in THIS request's state.
const INSTRUCTIONS: &str = "An agent asked for the visible text of the page titled `title`. `blocks[{at}]` is one block of that text: `path` is where the block sits in the document, from the body down through the landmarks and sections the page's author named, and `head` is the beginning of its text. Decide whether this block is what a reader came to the page for, or the furniture around it.";

/// What the `content` option means.
const CONTENT_MEANS: &str = "The block is the page's own substance — the article and its references or footnotes, the product, the listing or results, the thread and its replies, the document's body, or the form the reader is here to use — or it cannot be told apart from that substance.";

/// What the `chrome` option means.
const CHROME_MEANS: &str = "The block is furniture the reader did not come for: site navigation, a masthead or footer, a cookie or consent banner, a sign-in or subscribe prompt, an advertisement or sponsored unit, a related-links, trending or share rail, a newsletter or app-install pitch.";

/// The state's keys, in the order the fingerprint reads them.
const STATE_KEYS: [&str; 4] = ["title", "blocks", "path", "head"];

/// The fewest blocks that make a question. One block is the page, and a
/// request about it could only buy a row that folded the page into nothing.
pub const FEWEST_BLOCKS: usize = 2;

/// What separates two kept blocks in the folded text — the paragraph break
/// `innerText` puts between block-level elements, so a folded page reads
/// like the page.
const BLOCK_SEPARATOR: &str = "\n\n";

/// What separates the kinds the fold line names.
const KIND_SEPARATOR: &str = "·";

/// The path segment the page script gives the run of non-structural children
/// under a structural element — the block that is "everything in `main`
/// that is not itself a landmark".
pub const RUN_SEGMENT: &str = "*";

/// What separates the segments of a block's path.
pub const PATH_SEPARATOR: char = '>';

/// One block of a page's visible text, as the page script cut it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadBlock {
    /// Where it sits: `body>main>article>aside[role=complementary]`, or
    /// `body>main>*` for the run of non-structural children under `main`.
    /// Cut at the table's cap by [`ReadBlock::new`].
    pub path: String,
    /// The block's whole visible text. Never sent.
    pub text: String,
}

impl ReadBlock {
    /// A block whose path is cut to the table's cap, so the path a row
    /// records and the path a question carries are the same path.
    #[must_use]
    pub fn new(path: &str, text: &str) -> Self {
        Self {
            path: cut(path, Cap::Chars(BROWSER_READ_PATH_CHAR_CAP)),
            text: text.to_string(),
        }
    }

    /// The head of its text — what a question carries.
    #[must_use]
    pub fn head(&self) -> String {
        cut(&self.text, Cap::Chars(BROWSER_READ_HEAD_CHAR_CAP))
    }

    /// The kind of block its path says it is: the tag word of its last
    /// segment — `nav`, `footer`, `aside`, `div` — without the id, class,
    /// role or index the segment carries, and `*` for a run.
    #[must_use]
    pub fn kind(&self) -> &str {
        kind_of(&self.path)
    }
}

/// The tag word of a path's last segment.
#[must_use]
pub fn kind_of(path: &str) -> &str {
    let segment = path.rsplit(PATH_SEPARATOR).next().unwrap_or(path);
    let end = segment.find(['#', '.', '[']).unwrap_or(segment.len());
    &segment[..end]
}

/// Whether the element whose structural chain is `chain` lies in the block
/// at `block_path` — the containment rule a label is written by.
///
/// A block is either a whole structural element (its path IS the chain of
/// every element inside it that is not under a deeper landmark) or the run
/// of non-structural children under one (`chain>*`). A press whose chain is
/// longer than the block's path landed under a deeper landmark, which is a
/// block of its own.
#[must_use]
pub fn inside(block_path: &str, chain: &str) -> bool {
    block_path == chain
        || block_path
            .strip_suffix(RUN_SEGMENT)
            .and_then(|head| head.strip_suffix(PATH_SEPARATOR))
            .is_some_and(|head| head == chain)
}

/// One request: its state, its questions, and which blocks they are about,
/// so an answer can be read back into the page's own numbering.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadAsk {
    /// The request's `state`.
    pub state: Value,
    /// The request's `questions`.
    pub questions: Value,
    /// The indexes, into the page's blocks, this request asks about — in
    /// the order its state lists them.
    asked: Vec<usize>,
}

/// The words that define the question, as one string. The version is pinned
/// to this, not to a date or to a reviewer's memory.
#[must_use]
pub fn rubric_words() -> String {
    [
        INSTRUCTIONS,
        CONTENT_MEANS,
        CHROME_MEANS,
        &BROWSER_READ_OPTIONS.join(","),
        &STATE_KEYS.join(","),
    ]
    .join("\n")
}

/// The requests a page's blocks are judged in: none when the page has fewer
/// than [`FEWEST_BLOCKS`], else the first [`BROWSER_READ_BLOCK_CAP`] blocks
/// in even shards of [`BROWSER_READ_SHARD_TARGET`], so the shard that
/// decides the answer is the one the read waits for and not the longest one.
#[must_use]
pub fn ask(title: &str, blocks: &[ReadBlock]) -> Vec<ReadAsk> {
    let judged = blocks.len().min(BROWSER_READ_BLOCK_CAP);
    if judged < FEWEST_BLOCKS {
        return Vec::new();
    }
    let title = cut(title, Cap::Chars(BROWSER_READ_TITLE_CHAR_CAP));
    shard::even_shards(judged, BROWSER_READ_SHARD_TARGET)
        .into_iter()
        .map(|range| {
            let asked: Vec<usize> = range.collect();
            let state = json!({
                "title": title,
                "blocks": asked
                    .iter()
                    .map(|&index| json!({
                        "path": blocks[index].path,
                        "head": blocks[index].head(),
                    }))
                    .collect::<Vec<_>>(),
            });
            let mut questions = Map::new();
            for (at, &index) in asked.iter().enumerate() {
                let criteria = Map::from_iter([
                    (BROWSER_READ_CONTENT.to_string(), Value::from(CONTENT_MEANS)),
                    (BROWSER_READ_CHROME.to_string(), Value::from(CHROME_MEANS)),
                ]);
                let asked_as = choice::asked(
                    &format!("{QUESTION_ID_PREFIX}{index}"),
                    &INSTRUCTIONS.replace("{at}", &at.to_string()),
                    criteria,
                );
                if let Value::Object(one) = asked_as {
                    questions.extend(one);
                }
            }
            ReadAsk {
                state,
                questions: Value::Object(questions),
                asked,
            }
        })
        .collect()
}

impl ReadAsk {
    /// The blocks this request asks about, by index into the page's blocks.
    #[must_use]
    pub fn asked(&self) -> &[usize] {
        &self.asked
    }

    /// What `answers` says about every block this request asked about.
    ///
    /// # Errors
    ///
    /// [`ChoiceRefusal`] names the first rule an answer broke. A block asked
    /// about and not answered refuses the whole request: a shard half-read
    /// would fold some of the page on the judgment and the rest on nothing.
    pub fn read(&self, answers: &Value) -> Result<Vec<(usize, Choice)>, ChoiceRefusal> {
        let offered: BTreeSet<String> = BROWSER_READ_OPTIONS
            .iter()
            .map(|word| (*word).to_string())
            .collect();
        self.asked
            .iter()
            .map(|&index| {
                choice::read(answers, &format!("{QUESTION_ID_PREFIX}{index}"), &offered)
                    .map(|choice| (index, choice))
            })
            .collect()
    }
}

/// What every shard's readings came to: for each block the judgment called
/// chrome, how sure it was.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Verdict {
    /// Blocks the judgment called chrome, by index, with the answer's
    /// confidence.
    pub chrome: BTreeMap<usize, f64>,
    /// Blocks the judgment answered, either way.
    pub answered: usize,
}

impl Verdict {
    /// Fold the readings of every shard into one verdict. A reading whose id
    /// names no block this module wrote is not one and is left out.
    #[must_use]
    pub fn of(readings: impl IntoIterator<Item = (usize, Choice)>) -> Self {
        let mut verdict = Self::default();
        for (index, choice) in readings {
            verdict.answered += 1;
            if choice.chosen == BROWSER_READ_CHROME {
                verdict.chrome.insert(index, choice.confidence);
            }
        }
        verdict
    }

    /// The blocks the fold may drop under `seat`'s own line
    /// ([`JevUse::permits_press`]): the chrome blocks whose answer was sure
    /// enough to act on alone.
    #[must_use]
    pub fn droppable(&self, seat: &JevUse) -> Vec<usize> {
        self.chrome
            .iter()
            .filter(|(_, confidence)| seat.permits_press(**confidence))
            .map(|(index, _)| *index)
            .collect()
    }
}

/// A page's text with its chrome blocks folded away, and which they were.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folded {
    /// The kept blocks' text, then the fold line in the dropped blocks' place.
    pub text: String,
    /// The blocks that were dropped, by index, in document order.
    pub dropped: Vec<usize>,
}

/// The text `blocks` fold to when `dropped` are left out: every kept block's
/// text in document order, and one line after them saying what was left
/// out and how to read it anyway — `hint` is the command that does.
///
/// Nothing dropped is nothing folded: the text is every block's, whole, with
/// no line, so an agent reading a page with no furniture never sees a fold.
#[must_use]
pub fn fold(blocks: &[ReadBlock], dropped: &[usize], hint: &str) -> Folded {
    let dropped: Vec<usize> = {
        let mut held: Vec<usize> = dropped
            .iter()
            .copied()
            .filter(|index| *index < blocks.len())
            .collect();
        held.sort_unstable();
        held.dedup();
        held
    };
    let kept: Vec<&str> = blocks
        .iter()
        .enumerate()
        .filter(|(index, block)| !dropped.contains(index) && !block.text.trim().is_empty())
        .map(|(_, block)| block.text.as_str())
        .collect();
    let mut text = kept.join(BLOCK_SEPARATOR);
    if !dropped.is_empty() {
        let kinds = kinds(blocks, &dropped);
        if !text.is_empty() {
            text.push_str(BLOCK_SEPARATOR);
        }
        text.push_str(&fold_line(dropped.len(), &kinds, hint));
    }
    Folded { text, dropped }
}

/// The kinds of the dropped blocks, each named once, in document order.
#[must_use]
pub fn kinds(blocks: &[ReadBlock], dropped: &[usize]) -> Vec<String> {
    let mut named: Vec<String> = Vec::new();
    for &index in dropped {
        let Some(block) = blocks.get(index) else {
            continue;
        };
        let kind = match block.kind() {
            RUN_SEGMENT | "" => "block".to_string(),
            kind => kind.to_string(),
        };
        if !named.contains(&kind) {
            named.push(kind);
        }
    }
    named
}

/// The one line the folded text carries in the dropped blocks' place.
#[must_use]
pub fn fold_line(dropped: usize, kinds: &[String], hint: &str) -> String {
    format!(
        "[{dropped}개 블록 생략: {} — 전체는 {hint}]",
        kinds.join(KIND_SEPARATOR)
    )
}

#[cfg(test)]
mod tests;
