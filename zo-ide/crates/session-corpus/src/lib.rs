//! Synthetic session corpus for measuring and pinning session search (t-2947).
//!
//! One producer writes every transcript — [`Session`]'s own snapshot writer —
//! so a generated file is what a real `zo` session leaves behind, byte for
//! byte in shape. The size mix mirrors the real store's newest rows (measured
//! 2026-09-07 over 288 sessions of one project: the newest twenty held three
//! transcripts of 160–500 KB, nine or ten of ~55 KB, the rest under 45 KB) inside a
//! head window; every older session is small, so the 2,000-session corpus
//! stays near 30 MB and a test can afford to write it.
//!
//! Everything a test may want to assert against comes back in the manifest:
//! the id, the tier, the stamped times, the first prompt, whether the session
//! carries [`NEEDLE`].

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use core_types::session::{ContentBlock, ConversationMessage, MessageRole, Session, SessionFork};

/// The token a search test looks for; only the sessions the spec marks carry it.
pub const NEEDLE: &str = "haystack-needle";

/// Every knob of a corpus, in one table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorpusSpec {
    /// How many transcripts to write. Index 0 is the newest.
    pub sessions: usize,
    /// Seed of the word picker; the same seed writes the same bytes.
    pub seed: u64,
    /// Indices below this follow the measured newest-N size mix; from here on
    /// every session is small.
    pub head_window: usize,
    /// Inside the head window, every `large_every`-th index is a large session.
    pub large_every: usize,
    /// Inside the head window, every `medium_every`-th index (that is not
    /// large) is a medium session.
    pub medium_every: usize,
    /// Every `empty_every`-th index is a session nobody spoke into (meta only).
    pub empty_every: usize,
    /// Every `named_every`-th session carries a `name` in its header.
    pub named_every: usize,
    /// Every `forked_every`-th session carries fork provenance in its header.
    pub forked_every: usize,
    /// Every `needle_every`-th non-empty session carries [`NEEDLE`].
    pub needle_every: usize,
    /// Turns (user → assistant+tool call → tool result → assistant) per tier.
    pub small_turns: usize,
    pub medium_turns: usize,
    pub large_turns: usize,
    /// Distance between neighbouring sessions' modification times.
    pub mtime_step_ms: u64,
}

impl CorpusSpec {
    /// The 2,000-session corpus the t-2947 bounds are pinned on.
    pub const REAL_MIX: Self = Self {
        sessions: 2_000,
        seed: 7,
        head_window: 100,
        large_every: 7,
        medium_every: 2,
        empty_every: 25,
        named_every: 40,
        forked_every: 70,
        needle_every: 10,
        small_turns: 4,
        medium_turns: 22,
        large_turns: 160,
        mtime_step_ms: 60_000,
    };

    /// Twenty transcripts of ~2 MB at the head, everything behind them small:
    /// the corpus that shows whether opening the picker depends on how long
    /// the newest sessions are. Today's picker parses those twenty whole.
    pub const HEAVY_HEAD: Self = Self {
        head_window: 20,
        large_every: 1,
        large_turns: 700,
        ..Self::REAL_MIX
    };

    #[must_use]
    pub const fn with_sessions(mut self, sessions: usize) -> Self {
        self.sessions = sessions;
        self
    }

    #[must_use]
    pub const fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    /// Which tier an index lands in. Empty wins over size so the picker's
    /// "nothing to resume" filter has rows to drop at every depth.
    #[must_use]
    pub const fn tier(&self, index: usize) -> Tier {
        if self.empty_every > 1 && index % self.empty_every == self.empty_every - 1 {
            return Tier::Empty;
        }
        if index < self.head_window {
            if self.large_every > 0 && index.is_multiple_of(self.large_every) {
                return Tier::Large;
            }
            if self.medium_every > 0 && index % self.medium_every == self.medium_every - 1 {
                return Tier::Medium;
            }
        }
        Tier::Small
    }

    const fn turns(&self, tier: Tier) -> usize {
        match tier {
            Tier::Empty => 0,
            Tier::Small => self.small_turns,
            Tier::Medium => self.medium_turns,
            Tier::Large => self.large_turns,
        }
    }

    const fn every(index: usize, every: usize) -> bool {
        every > 0 && index.is_multiple_of(every)
    }
}

/// Size class of one generated session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Header only — a `zo` that was opened and closed.
    Empty,
    Small,
    Medium,
    Large,
}

/// What was written for one index — the facts a test can assert against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedSession {
    pub index: usize,
    pub id: String,
    pub path: PathBuf,
    pub tier: Tier,
    /// The stamped file modification time, newest first by index.
    pub modified_epoch_millis: u128,
    /// The header's `created_at_ms`.
    pub created_at_ms: u64,
    pub name: Option<String>,
    pub fork: Option<SessionFork>,
    /// The first user prompt, whole; `None` for an empty session.
    pub first_prompt: Option<String>,
    pub carries_needle: bool,
}

/// Write the corpus into `dir` (created if missing) and return its manifest,
/// index order. `dir` is the sessions directory itself — point
/// `ZO_SESSION_ROOT` at its parent.
pub fn generate(dir: &Path, spec: &CorpusSpec) -> io::Result<Vec<GeneratedSession>> {
    fs::create_dir_all(dir)?;
    let now_ms = epoch_millis_now()?;
    let mut rng = Lcg::new(spec.seed);
    let mut manifest = Vec::with_capacity(spec.sessions);
    for index in 0..spec.sessions {
        let tier = spec.tier(index);
        let step = spec.mtime_step_ms.saturating_mul(u64::try_from(index).unwrap_or(u64::MAX));
        let modified_ms = now_ms.saturating_sub(step);
        let created_at_ms = modified_ms.saturating_sub(spec.mtime_step_ms);
        let id = format!("synth-{}-{index:05}", spec.seed);
        let carries_needle = tier != Tier::Empty && CorpusSpec::every(index, spec.needle_every);
        let name = CorpusSpec::every(index, spec.named_every).then(|| format!("named session {index}"));
        let fork = CorpusSpec::every(index, spec.forked_every).then(|| SessionFork {
            parent_session_id: format!("synth-{}-parent-{index:05}", spec.seed),
            branch_name: (index % 2 == 0).then(|| format!("branch-{index}")),
        });

        let mut messages = Vec::new();
        for turn in 0..spec.turns(tier) {
            messages.extend(turn_messages(&mut rng, index, turn, carries_needle && turn == 0));
        }
        let first_prompt = messages.iter().find_map(|message| match message.blocks.first() {
            Some(ContentBlock::Text { text }) if message.role == MessageRole::User => {
                Some(text.clone())
            }
            _ => None,
        });

        let mut session = Session::new();
        session.session_id.clone_from(&id);
        session.name.clone_from(&name);
        session.created_at_ms = created_at_ms;
        session.updated_at_ms = modified_ms;
        session.fork.clone_from(&fork);
        session.messages = Arc::new(messages);
        let path = dir.join(format!("{id}.jsonl"));
        // The writer's own layout, but a plain write: `save_to_path` fsyncs
        // every file (F_FULLFSYNC, ~8 ms on this disk) and a 2,000-session
        // corpus took sixteen seconds to land — far too slow for a test.
        let snapshot = session
            .render_jsonl_snapshot()
            .map_err(|error| io::Error::other(error.to_string()))?;
        fs::write(&path, snapshot)?;
        fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(UNIX_EPOCH + Duration::from_millis(modified_ms))?;

        manifest.push(GeneratedSession {
            index,
            id,
            path,
            tier,
            modified_epoch_millis: u128::from(modified_ms),
            created_at_ms,
            name,
            fork,
            first_prompt,
            carries_needle,
        });
    }
    Ok(manifest)
}

/// The pass criteria of t-2947, in one table. A bound is checked on the
/// minimum of `min_of` runs: machine load can only push a sample up, so the
/// minimum is the number the code is responsible for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchBounds {
    /// `/resume` picker: open plus the first keystroke's filter, over
    /// [`CorpusSpec::HEAVY_HEAD`].
    pub picker_open_and_first_keystroke: Duration,
    /// A search-mode `session_recall` over [`CorpusSpec::REAL_MIX`].
    pub session_recall_search: Duration,
    pub min_of: usize,
}

impl SearchBounds {
    pub const T2947: Self = Self {
        picker_open_and_first_keystroke: Duration::from_millis(50),
        session_recall_search: Duration::from_millis(300),
        min_of: 5,
    };

    /// The fastest of `min_of` runs of `run`.
    pub fn min_of_runs(&self, mut run: impl FnMut()) -> Duration {
        (0..self.min_of.max(1))
            .map(|_| {
                let started = Instant::now();
                run();
                started.elapsed()
            })
            .min()
            .unwrap_or_default()
    }
}

/// Bytes on disk across the manifest — the number a measurement reports.
pub fn corpus_bytes(manifest: &[GeneratedSession]) -> io::Result<u64> {
    manifest
        .iter()
        .try_fold(0u64, |total, session| Ok(total + fs::metadata(&session.path)?.len()))
}

fn epoch_millis_now() -> io::Result<u64> {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| io::Error::other(error.to_string()))?;
    u64::try_from(elapsed.as_millis()).map_err(|error| io::Error::other(error.to_string()))
}

/// One turn: the user asks, the assistant answers and calls a tool, the tool
/// returns a page of a file, the assistant closes. Roughly 2.9 KB on disk —
/// the tool result carries most of it, as in real transcripts.
fn turn_messages(
    rng: &mut Lcg,
    index: usize,
    turn: usize,
    with_needle: bool,
) -> [ConversationMessage; 4] {
    let tool_use_id = format!("toolu_{index}_{turn}");
    let prompt = format!("prompt {index}/{turn}: {}", rng.words(PROMPT_WORDS));
    let mut answer = rng.words(ANSWER_WORDS);
    if with_needle {
        answer.push(' ');
        answer.push_str(NEEDLE);
    }
    let path = format!("src/{}/{}.rs", rng.word(), rng.word());
    [
        ConversationMessage::user_text(prompt),
        ConversationMessage::assistant(vec![
            ContentBlock::Text { text: answer },
            ContentBlock::ToolUse {
                id: tool_use_id.clone(),
                name: "read_file".to_string(),
                input: format!("{{\"path\":\"{path}\"}}"),
            },
        ]),
        ConversationMessage::tool_result(tool_use_id, "read_file", file_page(rng), false),
        ConversationMessage::assistant(vec![ContentBlock::Text {
            text: rng.words(CLOSING_WORDS),
        }]),
    ]
}

const PROMPT_WORDS: usize = 8;
const ANSWER_WORDS: usize = 40;
const CLOSING_WORDS: usize = 24;
const FILE_PAGE_LINES: usize = 48;

fn file_page(rng: &mut Lcg) -> String {
    use std::fmt::Write as _;

    let mut page = String::new();
    for line in 0..FILE_PAGE_LINES {
        let _ = writeln!(
            page,
            "    fn {}_{line}() -> u32 {{ {} }} // {}",
            rng.word(),
            rng.next() % 1_000,
            rng.words(3)
        );
    }
    page
}

/// The vocabulary: English and Korean so lowercase paths see multi-byte text.
const WORDS: &[&str] = &[
    "session", "search", "picker", "resume", "index", "mtime", "transcript", "vault", "compaction",
    "anchor", "worker", "ledger", "gate", "clippy", "cargo", "runtime", "tools", "painter", "ring",
    "wrap", "reflow", "cursor", "stream", "phase", "budget", "receipt", "seat", "lane", "pane",
    "세션", "검색", "최적화", "측정", "픽커", "머리", "읽기", "색인", "원장", "게이트", "워커",
    "판", "지연", "결과", "동일", "실측", "합성", "코퍼스", "바운드",
];

/// A tiny deterministic generator — no crate, no global state.
struct Lcg(u64);

impl Lcg {
    const fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1))
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn word(&mut self) -> &'static str {
        let pick = self.next() % u64::try_from(WORDS.len()).unwrap_or(1);
        WORDS[usize::try_from(pick).unwrap_or(0)]
    }

    fn words(&mut self, count: usize) -> String {
        (0..count).map(|_| self.word()).collect::<Vec<_>>().join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::{generate, CorpusSpec, Tier, NEEDLE};

    fn scratch(tag: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("session-corpus-{tag}-{unique}"))
    }

    #[test]
    fn the_same_seed_writes_the_same_bytes() {
        let spec = CorpusSpec::REAL_MIX.with_sessions(30);
        let left_dir = scratch("left");
        let right_dir = scratch("right");
        let left = generate(&left_dir, &spec).expect("left corpus");
        let right = generate(&right_dir, &spec).expect("right corpus");
        assert_eq!(left.len(), 30);
        for (l, r) in left.iter().zip(&right) {
            assert_eq!(l.id, r.id);
            let lb = std::fs::read(&l.path).expect("left bytes");
            let rb = std::fs::read(&r.path).expect("right bytes");
            // The header stamps the run's own clock; everything after it is the seed's.
            assert_eq!(
                lb.split(|b| *b == b'\n').skip(1).collect::<Vec<_>>(),
                rb.split(|b| *b == b'\n').skip(1).collect::<Vec<_>>(),
                "{} differs between runs",
                l.id
            );
        }
        let _ = std::fs::remove_dir_all(&left_dir);
        let _ = std::fs::remove_dir_all(&right_dir);
    }

    #[test]
    fn the_tier_table_puts_the_real_mix_at_the_head() {
        let spec = CorpusSpec::REAL_MIX;
        assert_eq!(spec.tier(0), Tier::Large);
        assert_eq!(spec.tier(1), Tier::Medium);
        assert_eq!(spec.tier(2), Tier::Small);
        assert_eq!(spec.tier(24), Tier::Empty);
        assert_eq!(spec.tier(700), Tier::Small, "past the head window everything is small");
        assert_eq!(spec.tier(724), Tier::Empty, "except the rows nobody spoke into");
        let newest_twenty: Vec<Tier> = (0..20).map(|index| spec.tier(index)).collect();
        assert_eq!(newest_twenty.iter().filter(|t| **t == Tier::Large).count(), 3);
        assert_eq!(newest_twenty.iter().filter(|t| **t == Tier::Medium).count(), 9);
    }

    #[test]
    fn the_manifest_says_what_was_written() {
        let spec = CorpusSpec::REAL_MIX.with_sessions(40);
        let dir = scratch("manifest");
        let manifest = generate(&dir, &spec).expect("corpus");
        let first = &manifest[0];
        assert!(first.carries_needle && first.name.is_some() && first.fork.is_some());
        let text = std::fs::read_to_string(&first.path).expect("first transcript");
        assert!(text.contains(NEEDLE));
        assert!(text.starts_with("{\"created_at_ms\":"), "the header is line one");
        assert_eq!(
            first.first_prompt.as_deref().map(|p| p.starts_with("prompt 0/0:")),
            Some(true)
        );
        let empty = &manifest[24];
        assert_eq!(empty.tier, Tier::Empty);
        assert_eq!(empty.first_prompt, None);
        assert_eq!(std::fs::read_to_string(&empty.path).expect("empty").lines().count(), 1);
        assert!(!std::fs::read_to_string(&manifest[1].path).expect("second").contains(NEEDLE));
        assert!(manifest[0].modified_epoch_millis > manifest[1].modified_epoch_millis);
        let stamped = std::fs::metadata(&manifest[5].path)
            .and_then(|m| m.modified())
            .expect("mtime")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("epoch")
            .as_millis();
        assert_eq!(stamped, manifest[5].modified_epoch_millis);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
