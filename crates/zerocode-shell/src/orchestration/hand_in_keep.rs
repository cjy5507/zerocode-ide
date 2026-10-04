//! Keeping what a worker's hand-in names (t-32798) — the window's half.
//!
//! A `worker_done` may name a report and evidence files
//! ([`zerocode_core::hand_in`]). They live in the worker's checkout or in its
//! agent's scratch folder and go when the checkout is cleaned, which is why a
//! coordinator copied them out by hand, again and again. This file reads them
//! while they are there, puts every text file through the mask
//! ([`zerocode_core::private_data`]), and keeps the result in the artifact store
//! ([`Store::register_kept`]) with one manifest per hand-in that says what was
//! kept, what was left out and why ([`Store::note_hand_in`]).
//!
//! Three roads end here, and all three are the same keeping:
//!
//! - **the hand-in** — a `send` that names files ([`after_send`]) starts it on a
//!   thread of its own at the lowest priority the system has, so the agent that
//!   reported never waits and the person's window never feels it;
//! - **a cleanup** — before any road takes a checkout ([`before_cleanup`]) the
//!   ledger is asked which hand-ins that checkout holds, and the cleanup waits
//!   while one has not been kept or its keeping failed. The beat's sweep asks
//!   without reading a file (a stat survey, then a thread); the person's removal
//!   keeps on the spot, because they are waiting for the answer;
//! - **a release** ([`note_release`]) — the worker's seat is let go, and what it
//!   handed in is kept before anything else can take its checkout.
//!
//! The road is the payload, so it is the same for every agent CLI: Claude Code,
//! Codex, zo, Antigravity, Kimi and Grok all report through `zerocode-orc send`.
//! No agent has an adapter here, because none needs one.
//!
//! Nothing here runs on the paint path or on a clock of its own: a keeping is a
//! thread that lives as long as the files take to copy, one at a time.

use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::UNIX_EPOCH;

use zerocode_core::artifact::Origin;
use zerocode_core::hand_in::{
    self, Candidate, DEPTH_MAX, Entry, Expect, FILE_BYTES_MAX, FILES_MAX, Facts, Fault, HandIn,
    Kind, Manifest, Outcome, RETRY_FAILED_AFTER_MS, ReportKind, Role, Standing, Verdict,
    WALK_ENTRIES_MAX, Why,
};
use zerocode_core::orchestration::{Ledger, RETENTION_DEFAULT_DAYS};
use zerocode_core::private_data::{self, Found};
use zerocode_orchestrator::same_worktree_path;

use super::desk::DeskSnapshot;
use super::{BoardLedgerSnapshot, LedgerAgent, with_ledger_seats};
use crate::artifact_runtime::{KeptFile, Store, origin_of_worker, store};

/// How much of a file is read to tell what it is: enough to see a picture's
/// signature or a NUL in text, one read per file. A file this size or smaller is
/// seen whole.
const SNIFF_BYTES: u64 = 8192;

/// Where the keeping may read from, and nowhere else: the checkout the worker
/// sat in and the temporary folders its agent keeps scratch files in. A path a
/// payload names is an agent's word, and an agent that names `~/.ssh/id_rsa` — by
/// mistake or because it was told to — has named a file nobody asked the window
/// to copy into a place the board can open.
#[derive(Debug, Clone, Default)]
pub(crate) struct Roots {
    checkout: Option<PathBuf>,
    temps: Vec<PathBuf>,
}

/// The unix temporary folders by name: `/tmp`, and where macOS keeps it, which
/// is where an agent's scratch folder is (`$TMPDIR` is a different one).
#[cfg(unix)]
const UNIX_TEMPS: [&str; 2] = ["/tmp", "/private/tmp"];

impl Roots {
    /// The roots for a worker that sat in `checkout`. Both are resolved, so a
    /// path is judged by where it really is and not by how it is spelled.
    pub(crate) fn of(checkout: Option<&str>) -> Self {
        Self {
            checkout: checkout.and_then(|at| Path::new(at).canonicalize().ok()),
            temps: temp_roots(),
        }
    }

    fn holds(&self, path: &Path) -> bool {
        self.checkout
            .iter()
            .chain(self.temps.iter())
            .any(|root| path.starts_with(root))
    }
}

/// The temporary folders this machine has, resolved and without repeats.
fn temp_roots() -> Vec<PathBuf> {
    #[cfg(unix)]
    let named = UNIX_TEMPS.iter().map(|root| PathBuf::from(*root));
    #[cfg(not(unix))]
    let named = std::iter::empty::<PathBuf>();
    let mut real: Vec<PathBuf> = std::iter::once(std::env::temp_dir())
        .chain(named)
        .filter_map(|root| root.canonicalize().ok())
        .collect();
    real.sort();
    real.dedup();
    real
}

/// One hand-in to keep: what the ledger says about it, the origin its rows wear,
/// and where it may read from.
#[derive(Debug, Clone)]
pub(crate) struct Job {
    pub(crate) hand_in: HandIn,
    pub(crate) origin: Origin,
    pub(crate) roots: Roots,
}

/// One named file, found on disk.
#[derive(Debug, Clone)]
struct Seen {
    /// The name it is listed under: the file's name, or its path inside the
    /// named folder.
    name: String,
    /// The file's own name — what it is kept under, extension and all, in a
    /// folder of its own, so a log summary reads `x.rc` and `x.log` as they were.
    file_name: String,
    role: Role,
    /// The path as the payload spelled it — what the kept row's id is made from,
    /// so a report registered by the old road and kept again is one row.
    identity: PathBuf,
    /// The resolved path it is read from.
    read_from: PathBuf,
    bytes: u64,
    modified_ms: i64,
    kind: Kind,
    /// Whether the file opened when its first bytes were read. One that did not
    /// stays in the list — so what the files look like does not depend on how
    /// they were looked at — and is left out as unreadable.
    readable: bool,
    /// What the worker said of this file, when it said.
    said: Said,
}

/// What a worker said of one named file beyond its path: the note, the closed
/// word beside it, and — for the report — what kind of report it is. Carried
/// from the payload to the manifest as it was said (t-32798, t-36910).
#[derive(Debug, Clone, Default)]
struct Said {
    expected: Option<String>,
    expect: Option<Expect>,
    report_kind: Option<ReportKind>,
}

/// One named path, as the payload named it.
struct Wanted<'a> {
    role: Role,
    path: &'a Path,
    said: Said,
}

/// One named file as the walk meets it, before it has been looked at.
struct Spot {
    /// The path as the payload spelled it, or inside the folder it names.
    identity: PathBuf,
    /// The resolved path it is read from.
    real: PathBuf,
    /// What it is listed under.
    name: String,
    /// What it is kept under.
    file_name: String,
    said: Said,
}

/// One named thing that could not be kept before the plan was asked.
#[derive(Debug, Clone)]
struct Missed {
    name: String,
    role: Role,
    why: Why,
}

/// What a walk over the named paths shares: where it may go, whether to open
/// files, which it has had, and how many entries it has looked at.
struct Walk<'a> {
    roots: &'a Roots,
    sniff: bool,
    taken: HashSet<PathBuf>,
    entries: usize,
}

/// The named files as they are on disk now.
#[derive(Debug, Default)]
struct Survey {
    seen: Vec<Seen>,
    missed: Vec<Missed>,
    beyond: u32,
}

impl Survey {
    /// Look at everything the hand-in names. With `sniff` off it only stats —
    /// what a cleanup's quick question needs; with it on, each file is opened
    /// once to tell what it is.
    fn of(job: &Job, sniff: bool) -> Self {
        let mut survey = Self::default();
        let mut walk = Walk {
            roots: &job.roots,
            sniff,
            taken: HashSet::new(),
            entries: 0,
        };
        let named = &job.hand_in.named;
        let report = named.report.iter().map(|path| Wanted {
            role: Role::Report,
            path,
            said: Said {
                report_kind: named.report_kind,
                ..Said::default()
            },
        });
        let evidence = named.evidence.iter().map(|item| Wanted {
            role: Role::Evidence,
            path: &item.path,
            said: Said {
                expected: item.expected.clone(),
                expect: item.expect,
                report_kind: None,
            },
        });
        for wanted in report.chain(evidence) {
            survey.take(&mut walk, wanted);
        }
        survey.beyond = named.beyond;
        survey
    }

    fn miss(&mut self, name: String, role: Role, why: Why) {
        self.missed.push(Missed { name, role, why });
    }

    /// One named path: a file, a folder, or something that is neither.
    fn take(&mut self, walk: &mut Walk<'_>, wanted: Wanted<'_>) {
        let Wanted { role, path, said } = wanted;
        let name = display_name(path);
        let real = match path.canonicalize() {
            Ok(real) => real,
            Err(error) => {
                let why = if error.kind() == std::io::ErrorKind::NotFound {
                    Why::Gone
                } else {
                    Why::Unreadable
                };
                self.miss(name, role, why);
                return;
            }
        };
        if !walk.roots.holds(&real) {
            self.miss(name, role, Why::OutsideRoots);
            return;
        }
        match std::fs::metadata(&real) {
            Ok(meta) if meta.is_file() => {
                let spot = Spot {
                    identity: path.to_path_buf(),
                    real,
                    file_name: name.clone(),
                    name,
                    said,
                };
                self.file(walk, role, spot, &meta);
            }
            Ok(meta) if meta.is_dir() => self.folder(walk, role, path, &real, &name),
            Ok(_) => self.miss(name, role, Why::NotKept),
            Err(_) => self.miss(name, role, Why::Unreadable),
        }
    }

    fn file(&mut self, walk: &mut Walk<'_>, role: Role, spot: Spot, meta: &std::fs::Metadata) {
        if !walk.taken.insert(spot.real.clone()) {
            return;
        }
        if self.seen.len() >= FILES_MAX {
            self.miss(spot.name, role, Why::TooManyFiles);
            return;
        }
        let (kind, readable) = if walk.sniff {
            sniff_file(&spot.real, meta.len()).map_or((Kind::Other, false), |kind| (kind, true))
        } else {
            // A stat survey is never asked what a file is.
            (Kind::Text, true)
        };
        let modified_ms = meta
            .modified()
            .ok()
            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| {
                i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
            });
        self.seen.push(Seen {
            name: spot.name,
            file_name: spot.file_name,
            role,
            identity: spot.identity,
            read_from: spot.real,
            bytes: meta.len(),
            modified_ms,
            kind,
            readable,
            said: spot.said,
        });
    }

    /// A named folder, to [`DEPTH_MAX`] levels and [`WALK_ENTRIES_MAX`] entries,
    /// links never followed.
    fn folder(&mut self, walk: &mut Walk<'_>, role: Role, named: &Path, real: &Path, name: &str) {
        let mut pending = vec![(
            real.to_path_buf(),
            named.to_path_buf(),
            name.to_string(),
            1usize,
        )];
        while let Some((dir, identity, prefix, depth)) = pending.pop() {
            let Ok(read) = std::fs::read_dir(&dir) else {
                self.miss(prefix, role, Why::Unreadable);
                continue;
            };
            let mut entries: Vec<(String, std::fs::FileType)> = Vec::new();
            for entry in read.flatten() {
                walk.entries += 1;
                if walk.entries > WALK_ENTRIES_MAX {
                    break;
                }
                if let Ok(file_type) = entry.file_type() {
                    entries.push((entry.file_name().to_string_lossy().into_owned(), file_type));
                }
            }
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            for (entry_name, file_type) in entries {
                let relative = format!("{prefix}/{}", shown(&entry_name));
                if file_type.is_symlink() {
                    self.miss(relative, role, Why::Link);
                } else if file_type.is_dir() {
                    if depth < DEPTH_MAX {
                        pending.push((
                            dir.join(&entry_name),
                            identity.join(&entry_name),
                            relative,
                            depth + 1,
                        ));
                    } else {
                        self.miss(relative, role, Why::TooManyFiles);
                    }
                } else if file_type.is_file() {
                    let path = dir.join(&entry_name);
                    match std::fs::metadata(&path) {
                        Ok(meta) => {
                            let spot = Spot {
                                identity: identity.join(&entry_name),
                                real: path,
                                name: relative,
                                file_name: shown(&entry_name),
                                said: Said::default(),
                            };
                            self.file(walk, role, spot, &meta);
                        }
                        Err(_) => self.miss(relative, role, Why::Unreadable),
                    }
                }
            }
            if walk.entries > WALK_ENTRIES_MAX {
                self.miss(name.to_string(), role, Why::TooManyFiles);
                break;
            }
        }
    }

    /// What the files looked like — the fingerprint a keeping is judged
    /// unchanged by.
    fn print(&self) -> String {
        let files: Vec<(String, u64, i64)> = self
            .seen
            .iter()
            .map(|seen| (seen.name.clone(), seen.bytes, seen.modified_ms))
            .collect();
        hand_in::fingerprint(&files)
    }

    fn found_nothing(&self) -> bool {
        self.seen.is_empty()
    }
}

/// A file's own name as a row and a manifest may carry it: with whatever private
/// value it holds taken out, because a name is kept as surely as a file is.
fn shown(name: &str) -> String {
    private_data::mask(name).text
}

/// A named path's last component, for the name a row lists it under.
fn display_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| "file".to_string(), |name| shown(&name.to_string_lossy()))
}

/// What a file is, from its first bytes.
fn sniff(head: &[u8], whole: bool) -> Kind {
    if is_picture(head) {
        return Kind::Image;
    }
    if head.contains(&0) {
        return Kind::Other;
    }
    match std::str::from_utf8(head) {
        Ok(_) => Kind::Text,
        // The window cut a character in two; the file is still text.
        Err(error) if !whole && error.error_len().is_none() => Kind::Text,
        Err(_) => Kind::Other,
    }
}

/// The raster pictures a screenshot is: PNG, JPEG, GIF, WebP — by signature,
/// never by extension, so a text file renamed `.png` is read as text.
fn is_picture(head: &[u8]) -> bool {
    head.starts_with(b"\x89PNG\r\n\x1a\n")
        || head.starts_with(&[0xFF, 0xD8, 0xFF])
        || head.starts_with(b"GIF87a")
        || head.starts_with(b"GIF89a")
        || (head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP")
}

fn sniff_file(path: &Path, len: u64) -> Option<Kind> {
    let file = crate::durable_file::open_plain_file(path).ok()?;
    let mut head = Vec::new();
    file.take(SNIFF_BYTES).read_to_end(&mut head).ok()?;
    Some(sniff(&head, len <= SNIFF_BYTES))
}

/// The first `read` bytes of a file, opened the way an application-owned file
/// is: never through a link.
fn read_head(path: &Path, read: u64) -> std::io::Result<Vec<u8>> {
    let file = crate::durable_file::open_plain_file(path)?;
    let mut bytes = Vec::with_capacity(usize::try_from(read).unwrap_or(0));
    file.take(read).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// A line to say, inside a kept head, that it is a head and how much of the file
/// it is. The row says which cap it was; the file says only what a reader holding
/// the file alone needs.
fn trailer(source_bytes: u64, kept: u64) -> String {
    format!(
        "\n\n[trimmed: the first {kept} of {source_bytes} bytes are kept, the rest is over the keeping cap]\n"
    )
}

/// Text through the mask: the bytes to keep and what was taken out, or why the
/// file cannot be kept. A head cut in the middle of a character is still text; a
/// file that is not UTF-8, or holds a NUL, is not. A head ends where a line does:
/// the cap may fall inside a value, and a mask cannot know the half of a key it
/// was never shown — so what the cap cut is cut back to the last whole line (or,
/// in a file of one long line, the last whitespace), and a head with neither is
/// left out.
fn mask_text(bytes: Vec<u8>, trimmed: bool, source_bytes: u64) -> Result<(Vec<u8>, Found), Why> {
    let mut text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            let whole = error.utf8_error().valid_up_to();
            if !(trimmed && error.utf8_error().error_len().is_none()) {
                return Err(Why::NotKept);
            }
            let mut bytes = error.into_bytes();
            bytes.truncate(whole);
            String::from_utf8(bytes).map_err(|_| Why::NotKept)?
        }
    };
    if text.contains('\0') {
        return Err(Why::NotKept);
    }
    if trimmed {
        let end = text
            .rfind('\n')
            .map(|at| at + 1)
            .or_else(|| text.rfind(char::is_whitespace));
        match end {
            Some(end) if end > 0 => text.truncate(end),
            _ => return Err(trim_reason(source_bytes)),
        }
    }
    let masked = private_data::mask(&text);
    // The copy is checked by the same table before anything is written: a mask
    // that missed a value fails closed, and nothing private is kept.
    if !private_data::is_clean(&masked.text) {
        return Err(Why::Unsafe);
    }
    let mut kept = masked.text;
    if trimmed {
        kept.push_str(&trailer(source_bytes, text.len() as u64));
    }
    Ok((kept.into_bytes(), masked.found))
}

fn left(name: String, role: Role, why: Why, bytes: u64) -> Entry {
    Entry {
        name,
        role,
        outcome: Outcome::Left,
        why: Some(why),
        source_bytes: bytes,
        ..Entry::default()
    }
}

/// Why a head was kept instead of the whole: the file's own cap, or what the
/// hand-in had left.
fn trim_reason(source_bytes: u64) -> Why {
    if source_bytes > FILE_BYTES_MAX {
        Why::OverFileCap
    } else {
        Why::OverTotalCap
    }
}

/// One file kept. `Ok` is what became of it — kept, or left out for a reason of
/// its own; `Err` is a write the store refused, which fails the whole keeping.
fn keep_one(
    store: &Store,
    job: &Job,
    seen: &Seen,
    read: u64,
    trimmed: bool,
    now_ms: i64,
) -> Result<Entry, String> {
    let Ok(bytes) = read_head(&seen.read_from, read) else {
        return Ok(left(
            seen.name.clone(),
            seen.role,
            Why::Unreadable,
            seen.bytes,
        ));
    };
    let (bytes, masked) = match seen.kind {
        Kind::Text => match mask_text(bytes, trimmed, seen.bytes) {
            Ok(done) => done,
            Err(why) => return Ok(left(seen.name.clone(), seen.role, why, seen.bytes)),
        },
        Kind::Image | Kind::Other => (bytes, Found::default()),
    };
    // The worker's note is its own words, so it goes through the same mask.
    let expected = seen
        .said
        .expected
        .as_deref()
        .map(|note| private_data::mask(note).text);
    let kept = store.register_kept(
        &KeptFile {
            name: &hand_in::kept_name(&seen.file_name),
            role: seen.role,
            bytes: &bytes,
            identity: &seen.identity,
            origin: &job.origin,
            note: expected.as_deref(),
        },
        now_ms,
    )?;
    Ok(Entry {
        name: seen.name.clone(),
        role: seen.role,
        outcome: if trimmed {
            Outcome::Trimmed
        } else {
            Outcome::Kept
        },
        why: trimmed.then(|| trim_reason(seen.bytes)),
        source_bytes: seen.bytes,
        kept_bytes: bytes.len() as u64,
        artifact: Some(kept.id),
        masked,
        expected,
        expect: seen.said.expect,
        report_kind: seen.said.report_kind,
    })
}

/// A file that was kept and has since gone from where it was is still kept: a
/// keeping adds and refreshes, it never forgets.
fn carried_or_left(old: Option<&Manifest>, missed: &Missed) -> Entry {
    if missed.why == Why::Gone
        && let Some(held) = old.and_then(|manifest| {
            manifest.entries.iter().find(|entry| {
                entry.name == missed.name
                    && entry.role == missed.role
                    && entry.outcome != Outcome::Left
            })
        })
    {
        return held.clone();
    }
    left(missed.name.clone(), missed.role, missed.why, 0)
}

/// The system's words for a failure, with anything private taken out of them.
fn masked_detail(error: &str) -> String {
    private_data::mask(error).text
}

/// Read, mask and keep everything a survey found, and write the manifest.
fn keep(
    store: &Store,
    job: &Job,
    survey: &Survey,
    old: Option<&Manifest>,
    now_ms: i64,
) -> Manifest {
    let mut manifest = Manifest::begin(&job.hand_in, survey.print(), now_ms);
    let candidates: Vec<Candidate<'_>> = survey
        .seen
        .iter()
        .map(|seen| Candidate {
            name: &seen.name,
            bytes: seen.bytes,
            kind: seen.kind,
        })
        .collect();
    for (seen, verdict) in survey.seen.iter().zip(hand_in::plan(&candidates)) {
        if !seen.readable {
            manifest.push(left(
                seen.name.clone(),
                seen.role,
                Why::Unreadable,
                seen.bytes,
            ));
            continue;
        }
        match verdict {
            Verdict::Leave(why) => {
                manifest.push(left(seen.name.clone(), seen.role, why, seen.bytes));
            }
            Verdict::Keep { read, trimmed } => {
                match keep_one(store, job, seen, read, trimmed, now_ms) {
                    Ok(entry) => manifest.push(entry),
                    Err(detail) => {
                        manifest.fail(Fault::CopyFailed, &masked_detail(&detail));
                        break;
                    }
                }
            }
        }
    }
    if manifest.failed.is_none() {
        for missed in &survey.missed {
            manifest.push(carried_or_left(old, missed));
        }
        manifest.leave_unlisted(survey.beyond);
    }
    if let Err(error) = store.note_hand_in(manifest.clone()) {
        crate::note_window_event(
            store.local_data_root(),
            &format!(
                "orchestration: the manifest of hand-in {} could not be written: {}",
                job.hand_in.message,
                masked_detail(&error)
            ),
        );
    }
    manifest
}

/// Keep a hand-in, unless it is already kept and what it names still looks the
/// way it did. A keeping whose files are all gone stands as it is.
pub(crate) fn ensure(store: &Store, job: &Job, now_ms: i64) -> Manifest {
    let held = store.hand_in(&job.hand_in.message);
    // A stat of every named file decides; only a keeping that is going to read
    // them opens them.
    let quick = Survey::of(job, false);
    if let Some(held) = &held
        && held.failed.is_none()
        && (quick.found_nothing() || quick.print() == held.fingerprint)
    {
        return held.clone();
    }
    keep(store, job, &Survey::of(job, true), held.as_ref(), now_ms)
}

/// What a keeping in flight is, for the board and for a second asker.
#[derive(Debug, Clone)]
struct Flying {
    run: String,
    task: Option<String>,
    worker: String,
    at_ms: i64,
}

/// The hand-ins being kept right now, by message.
static FLYING: LazyLock<Mutex<HashMap<String, Flying>>> = LazyLock::new(Mutex::default);

/// One keeping at a time: a hand-in is a few files and a burst of them is a
/// queue, not a crowd of threads reading the disk together.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn flying() -> std::sync::MutexGuard<'static, HashMap<String, Flying>> {
    FLYING.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A hand-in's place in [`FLYING`], held for as long as it is being kept —
/// and given back when the thread ends, however it ends.
struct Flight {
    message: String,
}

impl Flight {
    /// `None` when that hand-in is already being kept.
    fn enter(job: &Job, now_ms: i64) -> Option<Self> {
        let mut held = flying();
        if held.contains_key(&job.hand_in.message) {
            return None;
        }
        held.insert(
            job.hand_in.message.clone(),
            Flying {
                run: job.hand_in.run.clone(),
                task: job.hand_in.task.clone(),
                worker: job.hand_in.worker.clone(),
                at_ms: now_ms,
            },
        );
        Some(Self {
            message: job.hand_in.message.clone(),
        })
    }
}

impl Drop for Flight {
    fn drop(&mut self) {
        flying().remove(&self.message);
    }
}

/// Lower this thread to the lowest priority the system has, CPU and disk: a
/// keeping is a background chore and must not be felt on a slow machine.
fn lower_priority() {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: lowers only the calling thread's own priority.
        let _ = unsafe { libc::setpriority(libc::PRIO_DARWIN_THREAD, 0, libc::PRIO_DARWIN_BG) };
    }
}

/// Keep one hand-in on a thread of its own, at the lowest priority the system
/// has, one at a time. Cheap to ask twice: a hand-in already being kept is left
/// to the thread that has it.
pub(crate) fn schedule(job: Job) {
    let Some(flight) = Flight::enter(&job, crate::now_epoch_ms()) else {
        return;
    };
    let _ = std::thread::Builder::new()
        .name("hand-in-keep".into())
        .spawn(move || {
            lower_priority();
            let _flight = flight;
            let _one = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
            run_one(&job);
        });
}

/// The thread's work: keep the hand-in, say so in the window log when the
/// keeping failed, and tell the window the catalog moved.
fn run_one(job: &Job) {
    let Some(store) = store() else {
        return;
    };
    let manifest = ensure(&store, job, crate::now_epoch_ms());
    if manifest.failed.is_some() {
        crate::note_window_event(
            store.local_data_root(),
            &format!(
                "orchestration: {}",
                hand_in::hold_sentence(
                    &job.hand_in.message,
                    job.hand_in.task.as_deref(),
                    &manifest.standing(),
                )
            ),
        );
    }
    if let Some(app) = crate::artifact_runtime::window_handle() {
        use tauri::Emitter as _;
        let _ = app.emit(crate::artifact_runtime::CHANGED_EVENT, ());
    }
}

/// The job for one hand-in, with the origin its worker's row vouches for.
fn job_for(ledger: &Ledger, hand_in: HandIn) -> Option<Job> {
    let run = ledger.run(&hand_in.run)?;
    let worker = run.worker(&hand_in.worker)?;
    // The row's origin is what the ledger vouches for: the seat, the agent and
    // model it launched as, the task — and the commit the report named, which the
    // ledger wrote on the attempt it ended.
    let mut origin = origin_of_worker(run, worker);
    origin.commit.clone_from(&hand_in.commit);
    // A worker that carried several tasks one after another answers for its newest
    // in `origin_of_worker`; a hand-in is for the task it was filed on.
    if let Some(task) = hand_in.task.as_deref() {
        origin.task = Some(task.to_string());
        origin.work_summary = run.task(task).map(|held| held.display_name().to_string());
    }
    Some(Job {
        origin,
        roots: Roots::of(worker.checkout.as_deref()),
        hand_in,
    })
}

/// Whether the checkout a ledger row holds, spelled `at`, is the one asked about.
/// The ledger holds the path the pane reported, git hands back the resolved one
/// (`/var/…` and `/private/var/…`), and the removal's door gets the latter, so the
/// filesystem decides ([`same_worktree_path`]). Most rows are other checkouts: a
/// row is looked at only when its last component is this one's — a string
/// compare, not a syscall.
fn is_the_checkout(at: &str, checkout: &Path) -> bool {
    let at = Path::new(at);
    at.file_name() == checkout.file_name() && (at == checkout || same_worktree_path(at, checkout))
}

/// Every hand-in the workers that sat in `checkout` filed — what a cleanup of it
/// would take with it.
pub(crate) fn jobs_at(checkout: &Path) -> Vec<Job> {
    with_ledger_seats(|ledger, _| {
        hand_in::hand_ins_where(ledger.runs(), |at| is_the_checkout(at, checkout))
            .into_iter()
            .filter_map(|hand_in| job_for(ledger, hand_in))
            .collect()
    })
    .unwrap_or_default()
}

/// The hand-in a message is, as the ledger filed it.
fn job_of_message(message: &str) -> Option<Job> {
    with_ledger_seats(|ledger, _| {
        hand_in::hand_in_of(ledger.runs(), message).and_then(|hand_in| job_for(ledger, hand_in))
    })
    .flatten()
}

/// The word a `send` answers with for the message it filed.
fn message_id_of(stdout: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(stdout.trim())
        .ok()?
        .get("messageId")?
        .as_str()
        .map(str::to_string)
}

/// A `send` the ledger accepted: when it names files, keep them. Read only after
/// the send succeeded, so a refused message keeps nothing, and cheap for the many
/// sends that name nothing — a look at the argv, no ledger read, no disk.
pub(crate) fn after_send(argv: &[String], reply: &zerocode_hookd::TeamAnswer) {
    if reply.exit_code != 0 || argv.first().map(String::as_str) != Some("send") {
        return;
    }
    let value = |flag: &str| {
        argv.iter()
            .position(|word| word == flag)
            .and_then(|at| argv.get(at + 1))
            .map(String::as_str)
    };
    let named = hand_in::named(
        value("--payload").unwrap_or_default(),
        value("--body").unwrap_or_default(),
    );
    if named.is_empty() {
        return;
    }
    // The message as the ledger filed it: its run, task, dispatch and worker are
    // the ledger's own, not what the argv claimed.
    if let Some(job) = message_id_of(&reply.stdout).and_then(|id| job_of_message(&id)) {
        schedule(job);
    }
}

/// A worker's seat is released: what it handed in is kept before anything else
/// can take its checkout. The same question a cleanup asks, so the same hand-ins
/// are owed — recent, not yet kept or changed or failed, a few at a time — and
/// the release never waits: nothing is read here, the keeping is a thread.
pub(crate) fn note_release(worker: &str) {
    let jobs: Vec<Job> = with_ledger_seats(|ledger, _| {
        hand_in::hand_ins_of_worker(ledger.runs(), worker)
            .into_iter()
            .filter_map(|hand_in| job_for(ledger, hand_in))
            .collect()
    })
    .unwrap_or_default();
    if !jobs.is_empty() {
        let _ = ask(&jobs, Mode::Wait);
    }
}

/// What a cleanup is told about the hand-ins a checkout holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Clearance {
    /// Nothing is owed: the cleanup goes on.
    Clear,
    /// A keeping is being written or has just been started. The cleanup waits and
    /// asks again; nothing is wrong.
    Keeping(String),
    /// A keeping failed. The cleanup waits, with the reason, until it is mended.
    Held(String),
}

impl Clearance {
    /// The worse of two answers: a failure over a wait over a go-ahead.
    fn worse(self, other: Self) -> Self {
        let rank = |clearance: &Self| match clearance {
            Self::Clear => 0,
            Self::Keeping(_) => 1,
            Self::Held(_) => 2,
        };
        if rank(&other) > rank(&self) {
            other
        } else {
            self
        }
    }
}

/// Who is asking, and so what a keeping may cost them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The beat's sweep: no file is read and nothing is written on its thread. A
    /// hand-in not yet kept is started on a thread of its own, and the cleanup
    /// waits for the next beat.
    Wait,
    /// A person asked for the removal and is waiting for the answer: whatever is
    /// owed is kept on the spot. A hand-in whose keeping is already under way on
    /// its own thread is not kept twice — the answer is that it is still being
    /// kept, and the person asks again.
    Now,
}

/// A day, in milliseconds.
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// How many keepings one ask starts. A checkout that answers for many hand-ins —
/// a worker that carried many tasks, or the repository's own checkout — starts a
/// few at a time, newest first; the rest wait for the next ask, and the cleanup
/// waits until every one is kept.
const STARTED_PER_ASK: usize = 8;

/// How old a hand-in may be and still be owed a keeping: the days the store keeps
/// what it keeps. An older one's copy would be swept the moment it was made, and a
/// checkout that has stood that long is not waiting on it.
fn owed_window_ms(store: Option<&Store>) -> i64 {
    let days = store.map_or(RETENTION_DEFAULT_DAYS, |store| {
        store
            .limits()
            .effective_retention_days(RETENTION_DEFAULT_DAYS)
    });
    i64::from(days) * DAY_MS
}

/// Whether a hand-in that is settled needs keeping again: what it names still
/// exists and no longer looks the way it did.
fn changed(job: &Job, held: &Manifest) -> bool {
    let survey = Survey::of(job, false);
    !survey.found_nothing() && survey.print() != held.fingerprint
}

/// Where each hand-in stands, and what is to be started. Pure over the store it
/// is handed, so a test asks it without a ledger and without a window.
pub(crate) fn clearance(
    store: Option<&Store>,
    jobs: &[Job],
    mode: Mode,
    now_ms: i64,
) -> (Clearance, Vec<Job>) {
    let mut verdict = Clearance::Clear;
    let mut to_start = Vec::new();
    let window_ms = owed_window_ms(store);
    for job in jobs {
        if now_ms.saturating_sub(job.hand_in.at_ms) > window_ms {
            continue;
        }
        let message = job.hand_in.message.as_str();
        let task = job.hand_in.task.as_deref();
        let owed = hand_in::hold_sentence(message, task, &Standing::Owed);
        let Some(store) = store else {
            verdict = verdict.worse(Clearance::Keeping(owed));
            continue;
        };
        if flying().contains_key(message) {
            verdict = verdict.worse(Clearance::Keeping(owed));
            continue;
        }
        let held = store.hand_in(message);
        let standing = Standing::of(held.as_ref());
        let needs = match (&standing, &held) {
            (Standing::Owed, _) => true,
            (Standing::Failed { at_ms, .. }, _) => {
                mode == Mode::Now || now_ms.saturating_sub(*at_ms) >= RETRY_FAILED_AFTER_MS
            }
            (Standing::Settled, Some(held)) => changed(job, held),
            (Standing::Settled, None) => false,
        };
        if needs {
            match mode {
                Mode::Now => {
                    let kept = ensure(store, job, now_ms);
                    let after = Standing::of(Some(&kept));
                    if !after.lets_a_cleanup_go_on() {
                        verdict = verdict.worse(Clearance::Held(hand_in::hold_sentence(
                            message, task, &after,
                        )));
                    }
                }
                Mode::Wait => {
                    to_start.push(job.clone());
                    verdict = verdict.worse(Clearance::Keeping(owed));
                }
            }
        } else if !standing.lets_a_cleanup_go_on() {
            verdict = verdict.worse(Clearance::Held(hand_in::hold_sentence(
                message, task, &standing,
            )));
        }
    }
    to_start.sort_by_key(|job: &Job| std::cmp::Reverse(job.hand_in.at_ms));
    to_start.truncate(STARTED_PER_ASK);
    (verdict, to_start)
}

/// Ask before a cleanup takes `checkout`: which hand-ins it holds, whether each
/// is kept, and — for the ones that are not — start the keeping. The one question
/// every road that takes a checkout asks, so the answer is the same on all of them.
pub(crate) fn before_cleanup(checkout: &Path, mode: Mode) -> Clearance {
    let jobs = jobs_at(checkout);
    if jobs.is_empty() {
        return Clearance::Clear;
    }
    ask(&jobs, mode)
}

/// Ask where `jobs` stand and start what is owed: the one place a keeping is
/// started from a question, whoever asks.
fn ask(jobs: &[Job], mode: Mode) -> Clearance {
    let open = store();
    let (verdict, to_start) = clearance(open.as_deref(), jobs, mode, crate::now_epoch_ms());
    for job in to_start {
        schedule(job);
    }
    verdict
}

/// The facts a task row or a worker row shows: a keeping in flight says so,
/// otherwise the newest manifest's own facts.
fn flying_at(
    held: &HashMap<String, Flying>,
    run: &str,
    task: Option<&str>,
    worker: Option<&str>,
) -> Option<Facts> {
    held.values()
        .find(|one| {
            one.run == run
                && (task.is_some_and(|task| one.task.as_deref() == Some(task))
                    || worker.is_some_and(|worker| one.worker == worker))
        })
        .map(|one| Facts::keeping(one.at_ms))
}

/// Lay the keepings over a desk's task rows. Only the rows the desk sends are
/// asked, from a book that is in memory.
pub(crate) fn dress_desk_with(store: &Store, desk: &mut DeskSnapshot) {
    let book = store.hand_ins();
    let flying = flying();
    if book.is_empty() && flying.is_empty() {
        return;
    }
    for row in &mut desk.tasks {
        row.kept = flying_at(&flying, &row.run, Some(&row.id), None)
            .or_else(|| book.facts_of_task(&row.run, &row.id).cloned());
    }
}

/// Lay the keepings over the roster of workers.
pub(crate) fn dress_agents_with(store: &Store, agents: &mut [LedgerAgent]) {
    let book = store.hand_ins();
    let flying = flying();
    if book.is_empty() && flying.is_empty() {
        return;
    }
    for row in agents {
        row.kept = flying_at(&flying, &row.run, None, Some(&row.worker))
            .or_else(|| book.facts_of_worker(&row.run, &row.worker).cloned());
    }
}

/// The beat's dressing of what it publishes: desk rows and worker rows both.
pub(crate) fn dress(next: &mut BoardLedgerSnapshot) {
    let Some(store) = store() else {
        return;
    };
    dress_desk_with(&store, std::sync::Arc::make_mut(&mut next.desk));
    let agents: &mut Vec<LedgerAgent> = std::sync::Arc::make_mut(&mut next.agents);
    dress_agents_with(&store, agents);
}

#[cfg(test)]
mod tests;
