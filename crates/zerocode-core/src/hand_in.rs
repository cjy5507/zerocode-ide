//! A worker's hand-in, and what the ledger keeps of it (t-32798).
//!
//! A `worker_done` may name files:
//! `{"reportPath":"/abs/report.md","evidencePaths":["/abs/shot.png","/abs/dir"],
//! "lifetime":"ephemeral"}`. They usually live in the worker's checkout or in its
//! agent's scratch folder, and both go when the checkout is cleaned — which is
//! why a coordinator copied them out by hand, again and again. This module is
//! the pure half of keeping them for every agent CLI alike: what a payload names
//! ([`named`]), which hand-ins the ledger holds for a checkout ([`hand_ins_at`]),
//! how much of the files fits ([`plan`]) and what the keeping says about
//! itself ([`Manifest`], [`Facts`], [`Standing`]). The window does the reading,
//! masking and writing ([`crate::private_data`] is the mask) and puts a manifest
//! to each hand-in; a cleanup asks [`Standing`] before it takes a checkout.
//!
//! The payload is the CLI-neutral hand-in: Claude Code, Codex, zo, Antigravity,
//! Kimi and Grok all report through `zerocode-orc send`, so nothing here knows
//! which agent wrote it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::orchestration::{Message, MessageKind, Run, WORKER_ADDRESS_PREFIX};
use crate::private_data::Found;

/// The payload key a report is named under — the one the worker briefing spells.
pub const REPORT_KEY: &str = "reportPath";
/// The payload key a list of evidence files and folders is named under.
pub const EVIDENCE_KEY: &str = "evidencePaths";
/// The payload key that says what kind of report `reportPath` is (t-36910).
pub const REPORT_KIND_KEY: &str = "reportKind";
/// The key of an evidence entry that says what the file is expected to show.
pub const EXPECT_KEY: &str = "expect";

/// The most evidence paths one payload may name. A longer list is a folder that
/// should have been named once; the rest are counted, not kept.
pub const NAMED_MAX: usize = 32;
/// The largest file kept whole. The artifact table's own page cap
/// (`Limits::page_bytes_max`): a file past it is a dump, not a report, and a
/// 5 MB report keeps whole.
pub const FILE_BYTES_MAX: u64 = 8 * 1024 * 1024;
/// The most one hand-in keeps in all: three full-size files. It stays well under
/// the 64 MiB one catalog scan reads (`Limits::scan_bytes_max`), so keeping a
/// hand-in never starves the store's own bounded pass.
pub const HAND_IN_BYTES_MAX: u64 = 24 * 1024 * 1024;
/// The most files one hand-in keeps. A folder of screenshots rarely holds more
/// than a few dozen; the number also bounds the catalog rows one hand-in adds.
pub const FILES_MAX: usize = 48;
/// How deep a named folder is walked: evidence is flat or one level (`dark/`,
/// `light/`), and the bound keeps a folder named by mistake a short look.
pub const DEPTH_MAX: usize = 3;
/// How many directory entries one hand-in looks at. `target/` named by mistake
/// costs one bounded look, not a walk of three hundred thousand files.
pub const WALK_ENTRIES_MAX: usize = 2048;
/// The shortest head of an over-cap text file worth keeping: shorter says
/// nothing a reader can use, and the row says the file was left out instead.
pub const TRIM_MIN_BYTES: u64 = 16 * 1024;
/// How many entries a manifest lists by name; the rest are counted in `more`.
/// Twenty-four is the rows a desk stage carries (`STAGE_ROWS`).
pub const ENTRIES_LISTED: usize = 24;
/// How many distinct reasons a row's facts name; the rest are in the manifest.
pub const REASONS_SHOWN: usize = 3;
/// The longest name a kept file is stored under, extension kept.
pub const KEPT_NAME_MAX: usize = 120;
/// How long a failed keeping waits before the sweep tries again. A failure is
/// the window's own write (a full disk, a closed store), which a minute does not
/// mend and a person who asks for a cleanup retries at once.
pub const RETRY_FAILED_AFTER_MS: i64 = 5 * 60 * 1000;
/// The longest note a worker may put beside an evidence file (what it expects
/// the file to show), in characters. A line, not a second report.
pub const EXPECTED_CHARS_MAX: usize = 240;
/// The manifest layout this build writes.
pub const MANIFEST_VERSION: u32 = 1;

/// What kind of report a payload's `reportKind` says it is — a closed list, so
/// a row never wears a kind nobody defined: a word outside it is dropped, as if
/// the payload had not said (t-36910).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportKind {
    Report,
    Review,
    Brief,
    Handover,
    Proposal,
}

impl ReportKind {
    pub const ALL: [Self; 5] = [
        Self::Report,
        Self::Review,
        Self::Brief,
        Self::Handover,
        Self::Proposal,
    ];

    /// The word on the wire.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Report => "report",
            Self::Review => "review",
            Self::Brief => "brief",
            Self::Handover => "handover",
            Self::Proposal => "proposal",
        }
    }

    /// The kind a word names, if the list has it.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.word() == word.trim())
    }
}

/// What the worker says a piece of evidence will show — a closed pair, so an
/// intended failure (a red test's log, named on purpose) is said, not guessed
/// from a file name (t-36910).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    Fail,
    Pass,
}

impl Expect {
    /// The word on the wire.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Fail => "fail",
            Self::Pass => "pass",
        }
    }

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        [Self::Fail, Self::Pass]
            .into_iter()
            .find(|expect| expect.word() == word.trim())
    }
}

/// One evidence path a payload names, and what the worker said to expect of it:
/// a failing test's log, named on purpose, says it should fail — and says where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence {
    pub path: PathBuf,
    /// The worker's own words, cut to [`EXPECTED_CHARS_MAX`] and otherwise as
    /// written. Rides on a named file; a named folder carries none.
    pub expected: Option<String>,
    /// The closed word beside them: `fail` or `pass`, or nothing.
    pub expect: Option<Expect>,
}

impl Evidence {
    /// An evidence path with no note.
    #[must_use]
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            expected: None,
            expect: None,
        }
    }
}

/// What one payload (and body) names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Named {
    pub report: Option<PathBuf>,
    /// What kind of report it is, when the payload said and the list has the word.
    pub report_kind: Option<ReportKind>,
    pub evidence: Vec<Evidence>,
    /// Evidence paths named beyond [`NAMED_MAX`].
    pub beyond: u32,
}

impl Named {
    /// Nothing named at all — most `worker_done`s name nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.report.is_none() && self.evidence.is_empty() && self.beyond == 0
    }
}

/// The files a message names: the payload's `reportPath` and `evidencePaths`
/// (absolute paths only — a relative one names nothing a window could find), and
/// for the report alone, the first absolute Markdown path the body mentions when
/// the payload names none.
#[must_use]
pub fn named(payload: &str, body: &str) -> Named {
    let mut named = Named::default();
    if !payload.trim().is_empty()
        && let Ok(serde_json::Value::Object(map)) =
            serde_json::from_str::<serde_json::Value>(payload)
    {
        named.report = map
            .get(REPORT_KEY)
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        named.report_kind = map
            .get(REPORT_KIND_KEY)
            .and_then(serde_json::Value::as_str)
            .and_then(ReportKind::parse);
        if let Some(list) = map.get(EVIDENCE_KEY).and_then(serde_json::Value::as_array) {
            let mut seen = BTreeSet::new();
            for item in list.iter().filter_map(evidence_of) {
                if !seen.insert(item.path.clone()) {
                    continue;
                }
                if named.evidence.len() < NAMED_MAX {
                    named.evidence.push(item);
                } else {
                    named.beyond += 1;
                }
            }
        }
    }
    if named.report.is_none() {
        named.report = report_in_body(body);
    }
    named
}

/// One entry of `evidencePaths`: an absolute path as a string, or an object
/// `{"path": "/abs", "expected": "what the file should show", "expect": "fail"}`.
/// Anything else, and a path that is not absolute, names nothing; an `expect`
/// outside `fail` and `pass` is dropped.
fn evidence_of(item: &serde_json::Value) -> Option<Evidence> {
    let (path, expected, expect) = match item {
        serde_json::Value::String(path) => (path.as_str(), None, None),
        serde_json::Value::Object(map) => (
            map.get("path").and_then(serde_json::Value::as_str)?,
            map.get("expected").and_then(serde_json::Value::as_str),
            map.get(EXPECT_KEY)
                .and_then(serde_json::Value::as_str)
                .and_then(Expect::parse),
        ),
        _ => return None,
    };
    let path = PathBuf::from(path);
    path.is_absolute().then(|| Evidence {
        path,
        expected: expected
            .map(str::trim)
            .filter(|note| !note.is_empty())
            .map(|note| note.chars().take(EXPECTED_CHARS_MAX).collect()),
        expect,
    })
}

/// The first absolute Markdown path a body mentions — a worker that wrote
/// "report at `/abs/r.md`" in prose rather than in the payload.
fn report_in_body(body: &str) -> Option<PathBuf> {
    body.split(|ch: char| ch.is_whitespace() || matches!(ch, '`' | '"' | '\'' | '(' | ')' | ','))
        .find(|word| {
            word.ends_with(".md") && (word.starts_with('/') || Path::new(word).is_absolute())
        })
        .map(PathBuf::from)
}

/// One message that names files, with the worker and the work it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandIn {
    pub run: String,
    /// The ledger's own id for the message: the key a manifest is kept under.
    pub message: String,
    pub task: Option<String>,
    pub dispatch: Option<String>,
    pub worker: String,
    pub at_ms: i64,
    /// The commit the report handed in, when it named one: what the ledger wrote
    /// on the attempt it ended (`Dispatch::source`) and is not the message's own id.
    pub commit: Option<String>,
    pub named: Named,
}

/// A message as a hand-in: `None` when it names nothing, when its dispatch lives
/// in another window (its paths are on another machine), or when no worker can
/// be told from it.
fn hand_in_of_message(run: &Run, message: &Message) -> Option<HandIn> {
    let named = named(message.payload.as_str(), message.body.as_str());
    if named.is_empty() {
        return None;
    }
    let dispatch = message
        .dispatch
        .as_deref()
        .and_then(|dispatch| run.dispatch(dispatch));
    if dispatch.is_some_and(|dispatch| dispatch.remote.is_some()) {
        return None;
    }
    let worker = match dispatch {
        Some(dispatch) => dispatch.worker.clone(),
        None => message
            .from
            .strip_prefix(WORKER_ADDRESS_PREFIX)?
            .to_string(),
    };
    Some(HandIn {
        run: run.id.clone(),
        message: message.id.clone(),
        task: message.task.clone(),
        dispatch: message.dispatch.clone(),
        worker,
        at_ms: message.created_ms,
        commit: dispatch
            .and_then(|dispatch| dispatch.source.as_deref())
            .and_then(crate::orchestration::commit_named),
        named,
    })
}

/// The hand-in a message id is, whatever kind of mail it is — a status report
/// may name a file as well as a `worker_done`.
#[must_use]
pub fn hand_in_of(runs: &[Run], message: &str) -> Option<HandIn> {
    runs.iter().find_map(|run| {
        run.message(message)
            .and_then(|held| hand_in_of_message(run, held))
    })
}

/// The `worker_done`s in `run` that name files and were sent by one of `workers`.
fn worker_dones(run: &Run, workers: &[&str], found: &mut Vec<HandIn>) {
    for message in run.messages() {
        if message.kind != MessageKind::WorkerDone {
            continue;
        }
        if let Some(hand_in) = hand_in_of_message(run, message)
            && workers.contains(&hand_in.worker.as_str())
        {
            found.push(hand_in);
        }
    }
}

/// Every `worker_done` that names files, sent by a worker whose checkout `sat_in`
/// says is the one asked about — what a cleanup of that checkout would take with
/// it. The caller says what "the same checkout" means: a path is spelled the way
/// the pane reported it in one place and the way git resolves it in another, and
/// a window that knows its filesystem compares them as the filesystem does. A
/// checkout handed from one worker to the next answers for both.
#[must_use]
pub fn hand_ins_where(runs: &[Run], sat_in: impl Fn(&str) -> bool) -> Vec<HandIn> {
    let mut found = Vec::new();
    for run in runs {
        let workers: Vec<&str> = run
            .workers
            .iter()
            .filter(|worker| worker.checkout.as_deref().is_some_and(&sat_in))
            .map(|worker| worker.id.as_str())
            .collect();
        if !workers.is_empty() {
            worker_dones(run, &workers, &mut found);
        }
    }
    found
}

/// [`hand_ins_where`] for a checkout named by the same spelling the ledger holds
/// it under (a trailing separator aside).
#[must_use]
pub fn hand_ins_at(runs: &[Run], checkout: &str) -> Vec<HandIn> {
    let here = checkout.trim_end_matches('/');
    hand_ins_where(runs, |at| at.trim_end_matches('/') == here)
}

/// Every `worker_done` that names files and one worker sent — what is owed when
/// its seat is released.
#[must_use]
pub fn hand_ins_of_worker(runs: &[Run], worker: &str) -> Vec<HandIn> {
    let mut found = Vec::new();
    for run in runs.iter().filter(|run| run.worker(worker).is_some()) {
        worker_dones(run, &[worker], &mut found);
    }
    found
}

/// What a file turned out to be, read from its first bytes — never from its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Valid UTF-8 with no NUL: it can be read and masked.
    Text,
    /// A raster picture: pixels hold no text a mask could read.
    Image,
    /// Anything else — it cannot be checked, so it is not kept.
    Other,
}

/// One file to plan: its name, its size and its [`Kind`].
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub name: &'a str,
    pub bytes: u64,
    pub kind: Kind,
}

/// What the plan says of one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Read this many bytes; `trimmed` when that is the head of a longer file.
    Keep {
        read: u64,
        trimmed: bool,
    },
    Leave(Why),
}

/// How much of the files fits, in the order they are named (the report first):
/// each is kept whole, kept as its head (text only), or left out with a reason.
/// A file that does not fit does not stop the ones after it.
#[must_use]
pub fn plan(candidates: &[Candidate<'_>]) -> Vec<Verdict> {
    let mut used = 0u64;
    candidates
        .iter()
        .enumerate()
        .map(|(at, candidate)| {
            if at >= FILES_MAX {
                return Verdict::Leave(Why::TooManyFiles);
            }
            if secret_name(candidate.name) {
                return Verdict::Leave(Why::SecretName);
            }
            if candidate.kind == Kind::Other {
                return Verdict::Leave(Why::NotKept);
            }
            let room = HAND_IN_BYTES_MAX.saturating_sub(used);
            let limit = FILE_BYTES_MAX.min(room);
            if candidate.bytes <= limit {
                used = used.saturating_add(candidate.bytes);
                Verdict::Keep {
                    read: candidate.bytes,
                    trimmed: false,
                }
            } else if candidate.kind == Kind::Text && limit >= TRIM_MIN_BYTES {
                used = used.saturating_add(limit);
                Verdict::Keep {
                    read: limit,
                    trimmed: true,
                }
            } else if candidate.bytes > FILE_BYTES_MAX {
                Verdict::Leave(Why::OverFileCap)
            } else {
                Verdict::Leave(Why::OverTotalCap)
            }
        })
        .collect()
}

/// File names a credential store wears. A file named like one is not evidence of
/// anything a board shows, so it is refused by name — before it is read, whatever
/// it holds.
const SECRET_FILE_NAMES: &[&str] = &[
    ".env",
    ".netrc",
    ".npmrc",
    ".pgpass",
    ".git-credentials",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "credentials",
    "credentials.json",
    "secrets.json",
];
const SECRET_FILE_PREFIXES: &[&str] = &[".env."];
const SECRET_FILE_SUFFIXES: &[&str] =
    &[".pem", ".key", ".p12", ".pfx", ".jks", ".keystore", ".kdbx"];

/// Whether a file is named like a credential store (case ignored; only the last
/// path component counts).
#[must_use]
pub fn secret_name(name: &str) -> bool {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    SECRET_FILE_NAMES.contains(&base.as_str())
        || SECRET_FILE_PREFIXES
            .iter()
            .any(|prefix| base.starts_with(prefix))
        || SECRET_FILE_SUFFIXES
            .iter()
            .any(|suffix| base.ends_with(suffix))
}

/// The one-component name a kept file is stored under: separators flattened, no
/// control characters, no leading dot, at most [`KEPT_NAME_MAX`] characters with
/// the extension kept.
#[must_use]
pub fn kept_name(relative: &str) -> String {
    let flat: String = relative
        .chars()
        .filter(|ch| !ch.is_control())
        .map(|ch| {
            if matches!(ch, '/' | '\\' | ':') {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let flat = flat.trim_start_matches('.');
    if flat.is_empty() {
        return "file".to_string();
    }
    if flat.chars().count() <= KEPT_NAME_MAX {
        return flat.to_string();
    }
    let (stem, extension) = match flat.rfind('.') {
        Some(at) if flat.len() - at <= 16 => (&flat[..at], &flat[at..]),
        _ => (flat, ""),
    };
    let room = KEPT_NAME_MAX.saturating_sub(extension.chars().count());
    format!("{}{extension}", stem.chars().take(room).collect::<String>())
}

/// What the files named looked like when they were read — a name, a size and a
/// modification time each — as one short word. A keeping whose files still look
/// the same needs no second read.
#[must_use]
pub fn fingerprint(files: &[(String, u64, i64)]) -> String {
    let mut lines: Vec<String> = files
        .iter()
        .map(|(name, bytes, modified)| format!("{name}\0{bytes}\0{modified}"))
        .collect();
    lines.sort();
    let mut hasher = Sha256::new();
    for line in &lines {
        hasher.update(line.as_bytes());
        hasher.update(b"\n");
    }
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Why a named file is not (all) in the keeping. The word is what the board
/// translates, one sentence per word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// A file over [`FILE_BYTES_MAX`] that could not be kept as a head.
    OverFileCap,
    /// What the hand-in had already kept left no room for this file.
    OverTotalCap,
    /// Past [`FILES_MAX`] files, or [`NAMED_MAX`] named paths.
    TooManyFiles,
    /// Neither text nor a picture: it cannot be checked, so it is not kept.
    NotKept,
    /// Named like a credential store ([`secret_name`]).
    SecretName,
    /// Outside the worker's checkout and the temporary folders.
    OutsideRoots,
    /// A link, which is never followed out of a folder.
    Link,
    /// A copy that still held a private value after the mask.
    Unsafe,
    /// No longer there to keep — the checkout was already gone.
    Gone,
    /// There, and could not be read.
    Unreadable,
    /// On another machine.
    Remote,
}

impl Why {
    /// The word on the wire and on the board.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::OverFileCap => "over_file_cap",
            Self::OverTotalCap => "over_total_cap",
            Self::TooManyFiles => "too_many_files",
            Self::NotKept => "not_kept",
            Self::SecretName => "secret_name",
            Self::OutsideRoots => "outside_roots",
            Self::Link => "link",
            Self::Unsafe => "unsafe",
            Self::Gone => "gone",
            Self::Unreadable => "unreadable",
            Self::Remote => "remote",
        }
    }

    /// Kept nothing of it on purpose — a refusal, not a cap and not an absence.
    #[must_use]
    pub const fn is_refusal(self) -> bool {
        matches!(
            self,
            Self::NotKept | Self::SecretName | Self::OutsideRoots | Self::Link | Self::Unsafe
        )
    }
}

/// Whether a file is the report or one of the evidence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    #[default]
    Report,
    Evidence,
}

/// What became of one file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// All of it is kept.
    #[default]
    Kept,
    /// The head of it is kept ([`Entry::why`] says the cap).
    Trimmed,
    /// None of it is kept ([`Entry::why`] says why).
    Left,
}

/// One file's line in a manifest: what it is called, what became of it, how much
/// of it is kept, and what the mask took out of it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub name: String,
    pub role: Role,
    pub outcome: Outcome,
    pub why: Option<Why>,
    pub source_bytes: u64,
    pub kept_bytes: u64,
    /// The catalog id of the kept copy — what the board opens.
    pub artifact: Option<String>,
    pub masked: Found,
    /// What the worker said to expect of this file, masked like the file.
    pub expected: Option<String>,
    /// The closed word beside it (`fail` or `pass`), as the worker said it.
    pub expect: Option<Expect>,
    /// What kind of report this entry is, when it is the report and the payload
    /// said (t-36910) — kept here until the catalog row has a field of its own.
    pub report_kind: Option<ReportKind>,
}

/// The window's own failure to write a keeping — what a retry can mend, unlike
/// a file that is not there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    /// A write into the store failed (a full disk, a folder that cannot be made).
    CopyFailed,
    /// The store this keeping goes into is not open.
    StoreUnavailable,
}

impl Fault {
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::CopyFailed => "copy_failed",
            Self::StoreUnavailable => "store_unavailable",
        }
    }
}

/// A failed keeping's fault and the system's own words for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub fault: Fault,
    pub detail: String,
}

/// What the ledger keeps of one hand-in, and what became of every file it named.
/// A manifest holds no path: the ledger's own message already names them, and a
/// copy of a home directory in a kept record would be the leak it exists to stop.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub version: u32,
    pub message: String,
    pub run: String,
    pub task: Option<String>,
    pub dispatch: Option<String>,
    pub worker: String,
    pub at_ms: i64,
    /// What the named files looked like when this was written
    /// ([`fingerprint`]); a keeping whose files still look so is not redone.
    pub fingerprint: String,
    /// The first [`ENTRIES_LISTED`] files, in the order named.
    pub entries: Vec<Entry>,
    /// Files beyond the list — counted in the totals, not named.
    pub more: u32,
    pub kept: u32,
    pub trimmed: u32,
    pub kept_bytes: u64,
    pub left_out: u32,
    pub left_out_bytes: u64,
    pub refused: u32,
    pub masked: Found,
    pub failed: Option<Failure>,
}

/// How a keeping stands, in one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Everything named is kept.
    Kept,
    /// Some is kept and some is not (a cap, a refusal, a file that was gone) or
    /// a file is kept as its head.
    Partial,
    /// Nothing named is kept.
    Withheld,
    /// The window could not write what it meant to keep.
    Failed,
}

impl State {
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Kept => "kept",
            Self::Partial => "partial",
            Self::Withheld => "withheld",
            Self::Failed => "failed",
        }
    }
}

impl Manifest {
    /// A manifest for `hand_in`, with nothing in it yet.
    #[must_use]
    pub fn begin(hand_in: &HandIn, fingerprint: String, at_ms: i64) -> Self {
        Self {
            version: MANIFEST_VERSION,
            message: hand_in.message.clone(),
            run: hand_in.run.clone(),
            task: hand_in.task.clone(),
            dispatch: hand_in.dispatch.clone(),
            worker: hand_in.worker.clone(),
            at_ms,
            fingerprint,
            ..Self::default()
        }
    }

    /// Add one file's line, and its share of every total.
    pub fn push(&mut self, entry: Entry) {
        match entry.outcome {
            Outcome::Kept => {
                self.kept += 1;
                self.kept_bytes = self.kept_bytes.saturating_add(entry.kept_bytes);
            }
            Outcome::Trimmed => {
                self.kept += 1;
                self.trimmed += 1;
                self.kept_bytes = self.kept_bytes.saturating_add(entry.kept_bytes);
                self.left_out_bytes = self
                    .left_out_bytes
                    .saturating_add(entry.source_bytes.saturating_sub(entry.kept_bytes));
            }
            Outcome::Left => {
                self.left_out += 1;
                self.left_out_bytes = self.left_out_bytes.saturating_add(entry.source_bytes);
                if entry.why.is_some_and(Why::is_refusal) {
                    self.refused += 1;
                }
            }
        }
        self.masked.add(entry.masked);
        if self.entries.len() < ENTRIES_LISTED {
            self.entries.push(entry);
        } else {
            self.more += 1;
        }
    }

    /// Count `paths` that were named and cannot be listed: past [`NAMED_MAX`].
    pub fn leave_unlisted(&mut self, paths: u32) {
        self.left_out += paths;
        self.more += paths;
    }

    /// Record that the window could not write the keeping.
    pub fn fail(&mut self, fault: Fault, detail: &str) {
        self.failed = Some(Failure {
            fault,
            detail: detail.to_string(),
        });
    }

    #[must_use]
    pub fn state(&self) -> State {
        if self.failed.is_some() {
            State::Failed
        } else if self.kept == 0 {
            State::Withheld
        } else if self.left_out > 0 || self.trimmed > 0 {
            State::Partial
        } else {
            State::Kept
        }
    }

    /// Where a cleanup stands against this keeping.
    #[must_use]
    pub fn standing(&self) -> Standing {
        match &self.failed {
            Some(failure) => Standing::Failed {
                at_ms: self.at_ms,
                fault: failure.fault,
                detail: failure.detail.clone(),
            },
            None => Standing::Settled,
        }
    }

    /// What the board shows of this keeping.
    #[must_use]
    pub fn facts(&self) -> Facts {
        let mut reasons: Vec<&'static str> = Vec::new();
        if let Some(failure) = &self.failed {
            reasons.push(failure.fault.word());
        }
        for word in self
            .entries
            .iter()
            .filter_map(|entry| entry.why)
            .map(Why::word)
        {
            if reasons.len() < REASONS_SHOWN && !reasons.contains(&word) {
                reasons.push(word);
            }
        }
        Facts {
            state: self.state().word(),
            kept: self.kept,
            bytes: self.kept_bytes,
            left_out: self.left_out,
            left_out_bytes: self.left_out_bytes,
            refused: self.refused,
            masked: self.masked.total(),
            report: self
                .entries
                .iter()
                .find(|entry| entry.role == Role::Report)
                .and_then(|entry| entry.artifact.clone()),
            reasons,
            cap_bytes: HAND_IN_BYTES_MAX,
            file_cap_bytes: FILE_BYTES_MAX,
            detail: self.failed.as_ref().map(|failure| failure.detail.clone()),
            at_ms: self.at_ms,
        }
    }
}

/// What the board shows of one keeping — counted once when the manifest is
/// written, so a board that asks every second does no counting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Facts {
    /// `keeping` (still being written), or a [`State`] word.
    pub state: &'static str,
    pub kept: u32,
    pub bytes: u64,
    pub left_out: u32,
    pub left_out_bytes: u64,
    /// Of the files left out, how many were refused on purpose.
    pub refused: u32,
    /// Private values the mask took out, all kinds.
    pub masked: u32,
    /// The catalog id of the kept report, when there is one — what opens it.
    pub report: Option<String>,
    /// The first few reasons ([`Why::word`], [`Fault::word`]).
    pub reasons: Vec<&'static str>,
    /// The caps the reasons speak of, so a sentence can name the limit.
    pub cap_bytes: u64,
    pub file_cap_bytes: u64,
    /// The system's words for a failure, when it failed.
    pub detail: Option<String>,
    pub at_ms: i64,
}

impl Facts {
    /// The facts of a keeping that is being written right now.
    #[must_use]
    pub fn keeping(at_ms: i64) -> Self {
        Self {
            state: "keeping",
            kept: 0,
            bytes: 0,
            left_out: 0,
            left_out_bytes: 0,
            refused: 0,
            masked: 0,
            report: None,
            reasons: Vec::new(),
            cap_bytes: HAND_IN_BYTES_MAX,
            file_cap_bytes: FILE_BYTES_MAX,
            detail: None,
            at_ms,
        }
    }
}

/// Where a cleanup stands against a hand-in's keeping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// The keeping finished: whatever it kept is kept, whatever it left out says
    /// why. A cleanup may go on.
    Settled,
    /// The window could not write it. A cleanup waits, and the keeping is tried
    /// again.
    Failed {
        at_ms: i64,
        fault: Fault,
        detail: String,
    },
    /// Named, and no keeping has finished.
    Owed,
}

impl Standing {
    /// Where a hand-in stands, given the manifest its message has, if any.
    #[must_use]
    pub fn of(manifest: Option<&Manifest>) -> Self {
        manifest.map_or(Self::Owed, Manifest::standing)
    }

    /// Whether a cleanup may go on past it.
    #[must_use]
    pub const fn lets_a_cleanup_go_on(&self) -> bool {
        matches!(self, Self::Settled)
    }
}

/// The sentence a refused cleanup carries, for the window log and a refusal: the
/// hand-in, the work, and why the checkout stays.
#[must_use]
pub fn hold_sentence(message: &str, task: Option<&str>, standing: &Standing) -> String {
    let of = task.map_or_else(String::new, |task| format!(" of task {task}"));
    match standing {
        Standing::Settled => format!("the hand-in {message}{of} is kept"),
        Standing::Owed => format!(
            "the hand-in {message}{of} is still being kept — the checkout stays until it is"
        ),
        Standing::Failed { fault, detail, .. } => format!(
            "the hand-in {message}{of} was not kept ({}: {detail}) — the checkout stays until it is",
            fault.word()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::ALL_AGENTS;
    use crate::orchestration::{Draft, HANDED_IN_HEAD, Ledger, Priority, Text, worker_address};
    use crate::test_paths::absolute;

    fn payload(report: Option<&Path>, evidence: &[&Path]) -> String {
        let mut said = serde_json::Map::new();
        if let Some(report) = report {
            said.insert(REPORT_KEY.into(), report.to_string_lossy().into());
        }
        if !evidence.is_empty() {
            said.insert(
                EVIDENCE_KEY.into(),
                evidence
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .into(),
            );
        }
        said.insert("lifetime".into(), "ephemeral".into());
        serde_json::Value::Object(said).to_string()
    }

    #[test]
    fn a_payload_names_its_report_and_its_evidence() {
        let report = absolute("/tmp/t-9/report.md");
        let shot = absolute("/tmp/t-9/shot.png");
        let folder = absolute("/tmp/t-9/frames");
        let said = named(
            &payload(
                Some(report.as_path()),
                &[shot.as_path(), folder.as_path(), shot.as_path()],
            ),
            "done",
        );
        assert_eq!(said.report, Some(report));
        assert_eq!(
            said.evidence,
            vec![Evidence::at(shot), Evidence::at(folder)],
            "a path named twice is one"
        );
        assert_eq!(said.beyond, 0);
        assert!(!said.is_empty());
    }

    #[test]
    fn a_report_is_named_in_the_body_when_the_payload_names_none() {
        // The path the host spells: `C:\tmp\…` is where a Windows worker's
        // report is, in the payload (escaped) and in the body (as written).
        let report = absolute("/tmp/t-9-report.md");
        assert_eq!(
            named(&payload(Some(report.as_path()), &[]), "done").report,
            Some(report.clone())
        );
        assert_eq!(
            named(
                "",
                &format!("landed; report at `{}` (ephemeral)", report.display())
            )
            .report,
            Some(report)
        );
        assert!(named("", "done, nothing to read").is_empty());
        assert!(
            named(r#"{"reportPath":"relative.md"}"#, "done").is_empty(),
            "a relative path is not a report"
        );
    }

    #[test]
    fn paths_beyond_the_cap_are_counted_and_not_named() {
        let many: Vec<PathBuf> = (0..NAMED_MAX + 5)
            .map(|at| absolute(&format!("/tmp/t-9/e{at}.png")))
            .collect();
        let refs: Vec<&Path> = many.iter().map(PathBuf::as_path).collect();
        let said = named(&payload(None, &refs), "");
        assert_eq!(said.evidence.len(), NAMED_MAX);
        assert_eq!(said.beyond, 5);
    }

    #[test]
    fn an_evidence_entry_may_carry_what_the_worker_expects_of_it() {
        let red = absolute("/tmp/t-9/red.log");
        let plain = absolute("/tmp/t-9/plain.png");
        let long = "x".repeat(EXPECTED_CHARS_MAX + 50);
        let said = named(
            &serde_json::json!({
                EVIDENCE_KEY: [
                    {"path": red, "expected": "  fails at the assertion on main  "},
                    plain,
                    {"path": absolute("/tmp/t-9/long.log"), "expected": long},
                    {"path": absolute("/tmp/t-9/blank.log"), "expected": "   "},
                    {"expected": "no path is no evidence"},
                    {"path": "relative/red.log", "expected": "nor is a relative one"},
                    7,
                ]
            })
            .to_string(),
            "",
        );
        let notes: Vec<(&Path, Option<usize>, Option<&str>)> = said
            .evidence
            .iter()
            .map(|item| {
                (
                    item.path.as_path(),
                    item.expected.as_ref().map(|note| note.chars().count()),
                    item.expected.as_deref().filter(|note| note.len() < 40),
                )
            })
            .collect();
        assert_eq!(
            notes,
            [
                (
                    red.as_path(),
                    Some(30),
                    Some("fails at the assertion on main")
                ),
                (plain.as_path(), None, None),
                (
                    absolute("/tmp/t-9/long.log").as_path(),
                    Some(EXPECTED_CHARS_MAX),
                    None
                ),
                (absolute("/tmp/t-9/blank.log").as_path(), None, None),
            ]
        );
    }

    #[test]
    fn a_report_and_an_evidence_entry_say_a_closed_word_or_nothing() {
        let said = named(
            &serde_json::json!({
                REPORT_KIND_KEY: "review",
                EVIDENCE_KEY: [
                    {"path": absolute("/tmp/t-9/red.log"), "expect": "fail"},
                    {"path": absolute("/tmp/t-9/ok.log"), "expect": " pass "},
                    {"path": absolute("/tmp/t-9/maybe.log"), "expect": "maybe"},
                    {"path": absolute("/tmp/t-9/none.log")},
                    {"path": absolute("/tmp/t-9/shout.log"), "expect": "FAIL"},
                ],
            })
            .to_string(),
            "",
        );
        assert_eq!(said.report_kind, Some(ReportKind::Review));
        let expects: Vec<Option<Expect>> = said.evidence.iter().map(|item| item.expect).collect();
        assert_eq!(
            expects,
            [Some(Expect::Fail), Some(Expect::Pass), None, None, None],
            "a word outside the pair is dropped, never kept as one nobody defined"
        );
        for said in [
            serde_json::json!({ REPORT_KIND_KEY: "memo" }),
            serde_json::json!({ REPORT_KIND_KEY: 7 }),
            serde_json::json!({ REPORT_KIND_KEY: "Review" }),
            serde_json::json!({}),
        ] {
            assert_eq!(named(&said.to_string(), "").report_kind, None, "{said}");
        }
        let words: Vec<&str> = ReportKind::ALL.iter().map(|kind| kind.word()).collect();
        assert_eq!(words, ["report", "review", "brief", "handover", "proposal"]);
        for kind in ReportKind::ALL {
            assert_eq!(ReportKind::parse(kind.word()), Some(kind));
            assert_eq!(serde_json::to_value(kind).expect("serializes"), kind.word());
        }
        for expect in [Expect::Fail, Expect::Pass] {
            assert_eq!(Expect::parse(expect.word()), Some(expect));
            assert_eq!(
                serde_json::to_value(expect).expect("serializes"),
                expect.word()
            );
        }
    }

    /// A worker is taught the keys the window keeps — from the one place that
    /// spells them — and so is the coordinator's guide.
    #[test]
    fn the_worker_briefing_and_the_guide_teach_the_keys_the_window_keeps() {
        let briefing = crate::orchestration::worker_briefing("t-1", "keep", "");
        for needle in [
            format!("\"{EVIDENCE_KEY}\":["),
            format!("\"{EXPECT_KEY}\":\"fail\""),
            format!("\"{REPORT_KIND_KEY}\":\"review\""),
        ] {
            assert!(
                briefing.contains(&needle),
                "{needle} is not taught:\n{briefing}"
            );
        }
        for kind in ReportKind::ALL {
            assert!(
                briefing.contains(kind.word()),
                "{} is not offered",
                kind.word()
            );
        }
        let guide = include_str!("../../../skills/orchestration/SKILL.md");
        for key in [REPORT_KEY, EVIDENCE_KEY, REPORT_KIND_KEY, EXPECT_KEY] {
            assert!(guide.contains(key), "the guide does not teach `{key}`");
        }
    }

    fn text(name: &str, bytes: u64) -> Candidate<'_> {
        Candidate {
            name,
            bytes,
            kind: Kind::Text,
        }
    }

    #[test]
    fn a_hand_in_that_fits_is_kept_whole() {
        let five_mb = 5 * 1024 * 1024;
        let said = plan(&[
            text("report.md", five_mb),
            Candidate {
                name: "shot.png",
                bytes: 400_000,
                kind: Kind::Image,
            },
        ]);
        assert_eq!(
            said,
            vec![
                Verdict::Keep {
                    read: five_mb,
                    trimmed: false
                },
                Verdict::Keep {
                    read: 400_000,
                    trimmed: false
                },
            ]
        );
    }

    #[test]
    fn a_text_file_over_the_cap_keeps_its_head_and_a_picture_is_left_out() {
        let said = plan(&[
            text("big.log", FILE_BYTES_MAX + 1),
            Candidate {
                name: "big.png",
                bytes: FILE_BYTES_MAX + 1,
                kind: Kind::Image,
            },
        ]);
        assert_eq!(
            said,
            vec![
                Verdict::Keep {
                    read: FILE_BYTES_MAX,
                    trimmed: true
                },
                Verdict::Leave(Why::OverFileCap),
            ]
        );
    }

    fn picture(name: &str, bytes: u64) -> Candidate<'_> {
        Candidate {
            name,
            bytes,
            kind: Kind::Image,
        }
    }

    #[test]
    fn what_does_not_fit_does_not_stop_what_does() {
        let mib = 1024 * 1024;
        let said = plan(&[
            picture("a.png", 7 * mib),
            picture("b.png", 8 * mib),
            picture("c.png", 8 * mib),
            // One mebibyte of the 24 is left: this does not fit, the next does.
            picture("d.png", 8 * mib),
            picture("e.png", 1024),
        ]);
        assert_eq!(said.len(), 5, "every file is given a verdict");
        assert!(
            matches!(said[2], Verdict::Keep { trimmed: false, .. }),
            "{said:?}"
        );
        assert_eq!(said[3], Verdict::Leave(Why::OverTotalCap));
        assert_eq!(
            said[4],
            Verdict::Keep {
                read: 1024,
                trimmed: false
            }
        );
    }

    #[test]
    fn a_full_hand_in_leaves_a_head_too_short_to_read_out() {
        let eight = FILE_BYTES_MAX;
        let said = plan(&[
            text("a.md", eight),
            text("b.md", eight),
            text("c.md", eight),
            picture("d.png", 10),
            text("e.md", 10),
        ]);
        assert_eq!(said.len(), 5, "every file is given a verdict");
        assert_eq!(said[3], Verdict::Leave(Why::OverTotalCap));
        assert_eq!(said[4], Verdict::Leave(Why::OverTotalCap));
        // A text file over what is left keeps a head only when the head is
        // worth reading.
        let said = plan(&[
            text("a.md", eight),
            text("b.md", eight),
            text("c.md", eight - 1024),
            text("d.md", eight),
        ]);
        assert_eq!(said.len(), 4, "every file is given a verdict");
        assert_eq!(
            said[3],
            Verdict::Leave(Why::OverTotalCap),
            "1 KiB is no report"
        );
    }

    #[test]
    fn a_refused_kind_and_a_secret_name_are_left_out_before_size_is_asked() {
        let said = plan(&[
            Candidate {
                name: "blob.bin",
                bytes: 10,
                kind: Kind::Other,
            },
            text("id_rsa", 10),
            text(".env", 10),
            text("keys/server.PEM", 10),
            text("notes.md", 10),
        ]);
        assert_eq!(said.len(), 5, "every file is given a verdict");
        assert_eq!(said[0], Verdict::Leave(Why::NotKept));
        assert_eq!(said[1], Verdict::Leave(Why::SecretName));
        assert_eq!(said[2], Verdict::Leave(Why::SecretName));
        assert_eq!(said[3], Verdict::Leave(Why::SecretName));
        assert_eq!(
            said[4],
            Verdict::Keep {
                read: 10,
                trimmed: false
            }
        );
    }

    #[test]
    fn files_past_the_count_cap_are_left_out() {
        let names: Vec<String> = (0..FILES_MAX + 2).map(|at| format!("f{at}.md")).collect();
        let candidates: Vec<Candidate<'_>> = names.iter().map(|name| text(name, 1)).collect();
        let said = plan(&candidates);
        assert_eq!(said.len(), FILES_MAX + 2, "every file is given a verdict");
        assert!(matches!(said[FILES_MAX - 1], Verdict::Keep { .. }));
        assert_eq!(said[FILES_MAX], Verdict::Leave(Why::TooManyFiles));
        assert_eq!(said[FILES_MAX + 1], Verdict::Leave(Why::TooManyFiles));
    }

    #[test]
    fn a_secret_name_is_told_from_an_ordinary_one() {
        for secret in [
            ".env",
            ".ENV.local",
            "x/.netrc",
            "ID_RSA",
            "a\\b\\id_ed25519",
            "server.key",
            "cert.P12",
            "credentials.json",
        ] {
            assert!(secret_name(secret), "{secret}");
        }
        for ordinary in [
            "report.md",
            "monkey.png",
            "environment.md",
            "keyboard.png",
            "env",
        ] {
            assert!(!secret_name(ordinary), "{ordinary}");
        }
    }

    #[test]
    fn a_kept_name_is_one_short_component() {
        assert_eq!(kept_name("shots/dark.png"), "shots_dark.png");
        assert_eq!(kept_name("a\\b:c.md"), "a_b_c.md");
        assert_eq!(kept_name("..hidden"), "hidden");
        assert_eq!(kept_name("...."), "file");
        assert_eq!(kept_name("tab\tname.md"), "tabname.md");
        let long = format!("{}.png", "x".repeat(300));
        let kept = kept_name(&long);
        assert_eq!(kept.chars().count(), KEPT_NAME_MAX);
        assert!(kept.ends_with(".png"), "{kept}");
    }

    #[test]
    fn a_fingerprint_follows_what_the_files_look_like_and_not_their_order() {
        let one = vec![("a.md".to_string(), 10, 1), ("b.png".to_string(), 20, 2)];
        let swapped = vec![one[1].clone(), one[0].clone()];
        assert_eq!(fingerprint(&one), fingerprint(&swapped));
        let longer = vec![("a.md".to_string(), 11, 1), one[1].clone()];
        let later = vec![("a.md".to_string(), 10, 9), one[1].clone()];
        let renamed = vec![("c.md".to_string(), 10, 1), one[1].clone()];
        for other in [longer, later, renamed] {
            assert_ne!(fingerprint(&one), fingerprint(&other));
        }
        assert_eq!(fingerprint(&one).len(), 16);
    }

    #[test]
    fn every_reason_has_its_wire_word_and_the_refusals_are_the_five() {
        let all = [
            Why::OverFileCap,
            Why::OverTotalCap,
            Why::TooManyFiles,
            Why::NotKept,
            Why::SecretName,
            Why::OutsideRoots,
            Why::Link,
            Why::Unsafe,
            Why::Gone,
            Why::Unreadable,
            Why::Remote,
        ];
        for why in all {
            let said = serde_json::to_value(why).expect("serializes");
            assert_eq!(said, why.word(), "the serde name and the word drifted");
        }
        let refusals: Vec<&str> = all
            .iter()
            .filter(|why| why.is_refusal())
            .map(|why| why.word())
            .collect();
        assert_eq!(
            refusals,
            ["not_kept", "secret_name", "outside_roots", "link", "unsafe"]
        );
        for fault in [Fault::CopyFailed, Fault::StoreUnavailable] {
            assert_eq!(
                serde_json::to_value(fault).expect("serializes"),
                fault.word()
            );
        }
    }

    fn entry(name: &str, outcome: Outcome, why: Option<Why>, source: u64, kept: u64) -> Entry {
        Entry {
            name: name.to_string(),
            role: if name.ends_with(".md") {
                Role::Report
            } else {
                Role::Evidence
            },
            outcome,
            why,
            source_bytes: source,
            kept_bytes: kept,
            artifact: (outcome != Outcome::Left).then(|| format!("a-{name}")),
            masked: Found::default(),
            expected: None,
            expect: None,
            report_kind: None,
        }
    }

    fn hand_in() -> HandIn {
        HandIn {
            run: "run-1".into(),
            message: "m-7".into(),
            task: Some("t-1".into()),
            dispatch: Some("dp-1".into()),
            worker: "w-1".into(),
            at_ms: 5,
            commit: None,
            named: Named::default(),
        }
    }

    #[test]
    fn a_manifest_counts_what_was_kept_what_was_not_and_says_which() {
        let mut manifest = Manifest::begin(&hand_in(), "f".into(), 10);
        assert_eq!(manifest.state(), State::Withheld, "nothing is kept yet");
        manifest.push(Entry {
            masked: Found {
                home_path: 3,
                email: 1,
                ..Found::default()
            },
            ..entry("report.md", Outcome::Kept, None, 100, 98)
        });
        assert_eq!(manifest.state(), State::Kept);
        manifest.push(entry(
            "big.log",
            Outcome::Trimmed,
            Some(Why::OverFileCap),
            900,
            400,
        ));
        assert_eq!(manifest.state(), State::Partial, "a head is not all of it");
        manifest.push(entry("key.pem", Outcome::Left, Some(Why::SecretName), 5, 0));
        manifest.push(entry("gone.png", Outcome::Left, Some(Why::Gone), 0, 0));
        assert_eq!(
            (
                manifest.kept,
                manifest.trimmed,
                manifest.kept_bytes,
                manifest.left_out,
                manifest.left_out_bytes,
                manifest.refused,
                manifest.masked.total()
            ),
            (2, 1, 498, 2, 505, 1, 4)
        );
        let facts = manifest.facts();
        assert_eq!(facts.state, "partial");
        assert_eq!(facts.report.as_deref(), Some("a-report.md"));
        assert_eq!(facts.reasons, ["over_file_cap", "secret_name", "gone"]);
        assert_eq!((facts.masked, facts.refused), (4, 1));
        assert_eq!(facts.cap_bytes, HAND_IN_BYTES_MAX);
    }

    #[test]
    fn a_manifest_lists_so_many_entries_and_counts_the_rest() {
        let mut manifest = Manifest::begin(&hand_in(), "f".into(), 1);
        for at in 0..ENTRIES_LISTED + 3 {
            manifest.push(entry(&format!("f{at}.png"), Outcome::Kept, None, 1, 1));
        }
        manifest.leave_unlisted(2);
        assert_eq!(manifest.entries.len(), ENTRIES_LISTED);
        assert_eq!(manifest.more, 5);
        assert_eq!(manifest.kept as usize, ENTRIES_LISTED + 3);
        assert_eq!(manifest.left_out, 2);
    }

    #[test]
    fn a_failed_keeping_holds_a_cleanup_with_the_systems_own_words() {
        let mut manifest = Manifest::begin(&hand_in(), "f".into(), 42);
        manifest.push(entry("report.md", Outcome::Kept, None, 10, 10));
        manifest.fail(Fault::CopyFailed, "No space left on device (os error 28)");
        assert_eq!(manifest.state(), State::Failed);
        let standing = manifest.standing();
        assert!(!standing.lets_a_cleanup_go_on());
        assert_eq!(manifest.facts().reasons, ["copy_failed"]);
        assert_eq!(
            hold_sentence("m-7", Some("t-1"), &standing),
            "the hand-in m-7 of task t-1 was not kept (copy_failed: No space left on device \
             (os error 28)) — the checkout stays until it is"
        );
        // Owed: named, no keeping yet — also a hold; settled ones let it go on.
        assert!(!Standing::of(None).lets_a_cleanup_go_on());
        assert_eq!(
            hold_sentence("m-7", None, &Standing::Owed),
            "the hand-in m-7 is still being kept — the checkout stays until it is"
        );
        let mut done = Manifest::begin(&hand_in(), "f".into(), 1);
        done.push(entry("gone.md", Outcome::Left, Some(Why::Gone), 0, 0));
        assert!(
            Standing::of(Some(&done)).lets_a_cleanup_go_on(),
            "a file that is gone, a refusal and a cap are finished keepings, not failed ones"
        );
    }

    #[test]
    fn a_manifest_round_trips_and_reads_a_newer_one() {
        let mut manifest = Manifest::begin(&hand_in(), "f".into(), 1);
        manifest.push(entry("report.md", Outcome::Kept, None, 10, 10));
        let said = serde_json::to_string(&manifest).expect("serializes");
        assert_eq!(
            serde_json::from_str::<Manifest>(&said).expect("reads"),
            manifest
        );
        let mut newer: serde_json::Value = serde_json::from_str(&said).expect("json");
        newer["a_field_a_later_build_adds"] = serde_json::json!(1);
        let read: Manifest = serde_json::from_value(newer).expect("a newer manifest reads");
        assert_eq!(read, manifest);
        assert!(
            !said.contains("/Users") && !said.contains("source_path"),
            "a manifest holds no path: {said}"
        );
    }

    /// The word a worker's `worker_done` says, with no commit in it.
    const BODY: &str = r#"{"ok":true,"summary":"built"}"#;

    /// What one run, one task, one worker and one message are called.
    struct Made {
        run: String,
        task: String,
        worker: String,
        message: String,
    }

    /// A run with one worker that carries one task in `checkout`, and the
    /// message it filed with `payload`.
    fn hand_in_in(
        ledger: &mut Ledger,
        agent: &str,
        pane: &str,
        checkout: &str,
        payload: &str,
        kind: MessageKind,
        body: &str,
    ) -> Made {
        let run = ledger.create_run("keep", 1);
        let task = ledger
            .create_task(&run, "x".into(), "keep it".into(), vec![], None, 2)
            .expect("a task");
        let started = ledger
            .start_worker(&run, agent, ("team-keep", pane), Some(&task), 3)
            .expect("a worker");
        assert!(ledger.worker_seated(("team-keep", pane), checkout));
        let message = ledger
            .post(
                &run,
                Draft {
                    from: worker_address(&started.worker),
                    to: ledger.run(&run).expect("the run").address(),
                    kind,
                    body: Text::from(body.to_string()),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::from(payload.to_string()),
                    thread: None,
                    task: Some(task.clone()),
                    dispatch: started.dispatch.clone(),
                },
                9,
            )
            .expect("the report");
        Made {
            run,
            task,
            worker: started.worker,
            message,
        }
    }

    #[test]
    fn the_hand_ins_a_checkout_holds_are_the_worker_dones_of_the_workers_that_sat_in_it() {
        let mut ledger = Ledger::new();
        let report = absolute("/tmp/t-9/report.md");
        let said = payload(Some(report.as_path()), &[]);
        let made = hand_in_in(
            &mut ledger,
            "claude",
            "%2",
            "/work/t-9",
            &said,
            MessageKind::WorkerDone,
            BODY,
        );
        let found = hand_ins_at(ledger.runs(), "/work/t-9/");
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].run, made.run);
        assert_eq!(found[0].worker, made.worker);
        assert_eq!(found[0].message, made.message);
        assert_eq!(found[0].task.as_deref(), Some(made.task.as_str()));
        assert_eq!(found[0].named.report, Some(report));
        assert!(hand_ins_at(ledger.runs(), "/work/elsewhere").is_empty());
        let by_worker = hand_ins_of_worker(ledger.runs(), &made.worker);
        assert_eq!(
            by_worker, found,
            "a worker's hand-ins and its checkout's are one list"
        );
        assert!(hand_ins_of_worker(ledger.runs(), "w-nobody").is_empty());
        assert_eq!(
            hand_in_of(ledger.runs(), &made.message).map(|held| held.message),
            Some(made.message)
        );
        assert_eq!(hand_in_of(ledger.runs(), "m-nope"), None);
    }

    #[test]
    fn a_hand_in_carries_the_commit_its_report_named_and_only_that() {
        let report = absolute("/tmp/t-9/report.md");
        let said = payload(Some(report.as_path()), &[]);
        let commit = "0123abcd4567ef890123abcd4567ef890123abcd";
        let body = format!(r#"{{"ok":true,"{HANDED_IN_HEAD}":"{commit}"}}"#);
        let mut ledger = Ledger::new();
        let named_one = hand_in_in(
            &mut ledger,
            "claude",
            "%2",
            "/work/with",
            &said,
            MessageKind::WorkerDone,
            &body,
        );
        let none = hand_in_in(
            &mut ledger,
            "claude",
            "%3",
            "/work/without",
            &said,
            MessageKind::WorkerDone,
            BODY,
        );
        let commit_of =
            |made: &Made| hand_in_of(ledger.runs(), &made.message).and_then(|held| held.commit);
        assert_eq!(commit_of(&named_one).as_deref(), Some(commit));
        assert_eq!(
            commit_of(&none),
            None,
            "the report's own id is not a commit"
        );
    }

    #[test]
    fn a_report_that_names_nothing_is_no_hand_in_and_a_status_is_not_one_for_a_checkout() {
        let mut ledger = Ledger::new();
        let silent = hand_in_in(
            &mut ledger,
            "codex",
            "%2",
            "/work/quiet",
            "",
            MessageKind::WorkerDone,
            BODY,
        );
        assert!(hand_ins_at(ledger.runs(), "/work/quiet").is_empty());
        assert_eq!(hand_in_of(ledger.runs(), &silent.message), None);
        // A status naming a file is kept when it is sent, but a checkout's
        // cleanup waits only for what a `worker_done` handed in.
        let said = payload(Some(absolute("/tmp/t-9/r.md").as_path()), &[]);
        let status = hand_in_in(
            &mut ledger,
            "codex",
            "%3",
            "/work/status",
            &said,
            MessageKind::Status,
            BODY,
        );
        assert!(hand_ins_at(ledger.runs(), "/work/status").is_empty());
        assert!(hand_in_of(ledger.runs(), &status.message).is_some());
    }

    /// The same road for every agent the catalog knows: the payload is the
    /// hand-in, and nothing in it is a CLI's own.
    #[test]
    fn every_agent_cli_hands_in_through_the_same_payload() {
        let report = absolute("/tmp/t-9/report.md");
        let shot = absolute("/tmp/t-9/shot.png");
        for (at, agent) in ALL_AGENTS.iter().enumerate() {
            let mut ledger = Ledger::new();
            let checkout = format!("/work/{}", agent.slug());
            let made = hand_in_in(
                &mut ledger,
                agent.slug(),
                &format!("%{}", at + 2),
                &checkout,
                &payload(Some(report.as_path()), &[shot.as_path()]),
                MessageKind::WorkerDone,
                BODY,
            );
            let found = hand_ins_at(ledger.runs(), &checkout);
            assert_eq!(found.len(), 1, "{}: {found:?}", agent.slug());
            assert_eq!(found[0].message, made.message, "{}", agent.slug());
            assert_eq!(found[0].worker, made.worker, "{}", agent.slug());
            assert_eq!(
                found[0].named.report.as_ref(),
                Some(&report),
                "{}",
                agent.slug()
            );
            assert_eq!(
                found[0].named.evidence,
                vec![Evidence::at(shot.clone())],
                "{}",
                agent.slug()
            );
            let run = ledger.run(&made.run).expect("the run");
            let seated = run.worker(&made.worker).expect("the worker");
            assert_eq!(
                seated.agent,
                agent.slug(),
                "the row names the agent that wrote it"
            );
        }
    }
}
