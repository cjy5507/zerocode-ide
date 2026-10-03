//! Bounded local review evidence, shared by both Jev transports.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::{Case, MAX_CASE_BYTES, Outcome, Reason};
use crate::jev::{JevUse, door, promote};

/// A full queue remains inspectable and exportable; it never grows without a bound.
pub const MAX_CASES: usize = 512;
const DIRECTORY: &str = "jev/review";
const AUDIT_ONE_IN: u64 = 20;
const UNCERTAIN_BELOW: f64 = 0.8;

mod archive;
pub use archive::ArchiveReport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    home: PathBuf,
    seat: String,
    rubric_version: u32,
    workspace: String,
    origin_group: Option<String>,
}

impl Target {
    #[must_use]
    pub(crate) fn new(home: &Path, seat: &JevUse, workspace: &str) -> Self {
        Self {
            home: home.to_path_buf(),
            seat: seat.id.to_string(),
            rubric_version: seat.rubric_version,
            workspace: workspace.to_string(),
            origin_group: None,
        }
    }

    pub(crate) fn bind_origin(&mut self, namespace: &str, origin: &str) {
        self.origin_group = super::origin_group(namespace, origin).ok();
    }
}

/// Prepared only when the existing label-draft preference opted into retention.
pub struct Pending {
    target: Target,
    request: Value,
}

impl Pending {
    #[must_use]
    pub(crate) fn prepare(target: &Target, bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_CASE_BYTES {
            return None;
        }
        Some(Self {
            target: target.clone(),
            request: serde_json::from_slice(bytes).ok()?,
        })
    }

    /// Recheck the existing settings before retaining a completed request.
    ///
    /// # Errors
    /// Invalid evidence or failure to persist it. This is observation only and
    /// must never change the answer the caller received.
    pub fn finish(self, response: &Value, at: i64) -> io::Result<Option<Recorded>> {
        // The person's configuration is not a case file. It may be managed
        // through a symlink and contain unrelated settings larger than a case.
        // Deserialize only the branch that the shared Jev readers need.
        #[derive(serde::Deserialize)]
        struct Settings {
            #[serde(default)]
            smart: Value,
        }
        let settings: Settings = serde_json::from_reader(BufReader::new(File::open(
            self.target.home.join("settings.json"),
        )?))
        .map_err(io::Error::other)?;
        let root = serde_json::json!({"smart": settings.smart});
        let Some(seat) = crate::jev::jev_use(&self.target.seat) else {
            return Ok(None);
        };
        let settings = door::JevSettings::from_root(&root).resolved();
        if !settings.enabled
            || !settings.consents(&self.target.workspace)
            || !promote::label_drafts_wanted(&root)
            || !seat.mode_in(&root).asks()
        {
            return Ok(None);
        }
        let mut size = super::SizeBound {
            remaining: MAX_CASE_BYTES,
        };
        serde_json::to_writer(&mut size, response).map_err(io::Error::other)?;
        let case = Case::from_request(
            &self.target.seat,
            self.target.rubric_version,
            self.request,
            response,
            at,
            Some(self.target.workspace),
        )
        .and_then(|case| case.with_origin_group(self.target.origin_group))
        .map_err(io::Error::other)?;
        let certainty = case.certainty_except(&std::collections::HashSet::new());
        let audit = super::random_rank(&case.id, 0).is_multiple_of(AUDIT_ONE_IN);
        if !audit && !certainty.is_some_and(|value| value < UNCERTAIN_BELOW) {
            return Ok(None);
        }
        Store::at(&self.target.home).record(&case).map(Some)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Recorded {
    Saved,
    Existing,
    Full,
}

#[derive(Debug, Default)]
pub struct Snapshot {
    pub cases: Vec<Case>,
    pub outcomes: Vec<Outcome>,
    pub invalid: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewedSample {
    pub case: Case,
    pub reason: Reason,
    pub remaining: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Review {
    pub cases: usize,
    /// Occupied slots, including malformed case files that still consume capacity.
    pub total: usize,
    pub invalid: usize,
    pub samples: Vec<ReviewedSample>,
}

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    #[must_use]
    pub fn at(home: &Path) -> Self {
        Self {
            root: home.join(DIRECTORY),
        }
    }

    fn lock(&self) -> io::Result<File> {
        fs::create_dir_all(&self.root)?;
        let metadata = fs::symlink_metadata(&self.root)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(io::Error::other("review store is not a plain directory"));
        }
        let lock = private_options()
            .create(true)
            .truncate(false)
            .read(true)
            .open(self.root.join(".lock"))?;
        lock.try_lock().map_err(io::Error::other)?;
        Ok(lock)
    }

    /// Write one immutable case. Identical observations cost no new storage.
    ///
    /// # Errors
    /// Invalid evidence, a busy store, or an I/O error.
    pub fn record(&self, case: &Case) -> io::Result<Recorded> {
        case.validate().map_err(io::Error::other)?;
        let _lock = self.lock()?;
        let path = self.root.join(format!("{}.json", case.id));
        if path.exists() {
            let existing: Case = read_json(&path, MAX_CASE_BYTES)?;
            existing.validate().map_err(io::Error::other)?;
            if existing.id != case.id {
                return Err(io::Error::other("review case identity collision"));
            }
            return Ok(Recorded::Existing);
        }
        let count = fs::read_dir(&self.root)?
            .filter_map(Result::ok)
            .filter(|entry| case_name(&entry.file_name().to_string_lossy()))
            .take(MAX_CASES)
            .count();
        if count >= MAX_CASES {
            return Ok(Recorded::Full);
        }
        atomic_json(&path, case)?;
        Ok(Recorded::Saved)
    }

    fn paths(&self) -> io::Result<Vec<PathBuf>> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let entry = entry?;
            if case_name(&entry.file_name().to_string_lossy()) {
                paths.push(entry.path());
            }
            if paths.len() > MAX_CASES {
                return Err(io::Error::other("review store exceeds its case limit"));
            }
        }
        paths.sort();
        Ok(paths)
    }

    /// Load bounded evidence and report malformed files instead of counting them as labels.
    ///
    /// # Errors
    /// Failure to enumerate the store. A missing store is empty.
    pub fn snapshot(&self) -> io::Result<Snapshot> {
        let mut out = Snapshot::default();
        for path in self.paths()? {
            let case = match read_case(&path) {
                Ok(case) => case,
                _ => {
                    out.invalid += 1;
                    continue;
                }
            };
            let (outcomes, invalid) = read_outcomes(&path, &case);
            out.outcomes.extend(outcomes);
            out.invalid += invalid;
            out.cases.push(case);
        }
        Ok(out)
    }

    /// Pick a review batch while holding only one full input at a time during
    /// acquisition. The second pass loads only selected bodies, so opening a
    /// five-case drawer does not retain all 512 cases in memory.
    ///
    /// # Errors
    /// Invalid limits or a directory read failure. Missing stores are empty.
    pub fn review(
        &self,
        seat: Option<&str>,
        workspace: Option<&str>,
        limit: usize,
        audit: usize,
        seed: u64,
    ) -> io::Result<Review> {
        if !(1..=super::MAX_BATCH).contains(&limit) || audit > limit {
            return Err(io::Error::other(super::Invalid::Batch));
        }
        let paths = self.paths()?;
        let mut out = Review {
            total: paths.len(),
            ..Review::default()
        };
        let mut candidates = Vec::new();
        for (index, path) in paths.iter().enumerate() {
            let case = match read_case(path) {
                Ok(case) => case,
                Err(_) => {
                    out.invalid += 1;
                    continue;
                }
            };
            if seat.is_some_and(|seat| case.seat != seat)
                || workspace.is_some_and(|workspace| case.workspace.as_deref() != Some(workspace))
            {
                continue;
            }
            out.cases += 1;
            let (outcomes, invalid) = read_outcomes(path, &case);
            out.invalid += invalid;
            let reviewed = outcomes
                .iter()
                .map(|one| (one.case_id.as_str(), one.question.as_str()))
                .collect::<HashSet<_>>();
            if let Some(certainty) = case.certainty_except(&reviewed) {
                candidates.push((certainty, super::random_rank(&case.id, seed), index));
            }
        }
        for (index, reason) in super::selected_indices(&candidates, limit, audit) {
            let path = &paths[index];
            let case = match read_case(path) {
                Ok(case) => case,
                Err(_) => {
                    out.invalid += 1;
                    continue;
                }
            };
            // Another reviewer may have finished while acquisition ran. Its
            // newer outcomes win; do not offer those questions again.
            let (outcomes, _) = read_outcomes(path, &case);
            let reviewed = outcomes
                .iter()
                .map(|one| one.question.as_str())
                .collect::<HashSet<_>>();
            let remaining = case
                .answers
                .as_object()
                .into_iter()
                .flat_map(|answers| answers.keys())
                .filter(|id| !reviewed.contains(id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if !remaining.is_empty() {
                out.samples.push(ReviewedSample {
                    case,
                    reason,
                    remaining,
                });
            }
        }
        Ok(out)
    }

    /// Record a reviewed answer; retries are idempotent and older reviews cannot win.
    ///
    /// # Errors
    /// Missing evidence, invalid or stale review, busy store, or an I/O error.
    pub fn outcome(&self, outcome: Outcome) -> io::Result<()> {
        if !identity(&outcome.case_id) {
            return Err(io::Error::other("invalid review case id"));
        }
        let _lock = self.lock()?;
        let path = self.root.join(format!("{}.json", outcome.case_id));
        let case: Case = read_json(&path, MAX_CASE_BYTES)?;
        outcome.validate(&case).map_err(io::Error::other)?;
        let path = path.with_extension("outcomes");
        let mut held: Vec<Outcome> = if path.exists() {
            read_json(&path, MAX_CASE_BYTES)?
        } else {
            Vec::new()
        };
        if held.len() > super::MAX_BATCH * super::Reviewer::ALL.len()
            || held.iter().any(|old| old.for_valid_case(&case).is_err())
        {
            return Err(io::Error::other("stored review is malformed"));
        }
        if let Some(old) = held
            .iter_mut()
            .find(|old| old.question == outcome.question && old.reviewer == outcome.reviewer)
        {
            if *old == outcome {
                return Ok(());
            }
            if old.at >= outcome.at {
                return Err(io::Error::other("review is older than the stored outcome"));
            }
            *old = outcome;
        } else {
            if held.len() >= super::MAX_BATCH * super::Reviewer::ALL.len() {
                return Err(io::Error::other("review outcome limit reached"));
            }
            held.push(outcome);
        }
        atomic_json(&path, &held)
    }
}

fn read_case(path: &Path) -> io::Result<Case> {
    let case: Case = read_json(path, MAX_CASE_BYTES)?;
    case.validate().map_err(io::Error::other)?;
    if path.file_stem().and_then(|stem| stem.to_str()) != Some(&case.id) {
        return Err(io::Error::other(
            "review case identity does not match its filename",
        ));
    }
    Ok(case)
}

fn read_outcomes(path: &Path, case: &Case) -> (Vec<Outcome>, usize) {
    let path = path.with_extension("outcomes");
    if !path.exists() {
        return (Vec::new(), 0);
    }
    match read_json::<Vec<Outcome>>(&path, MAX_CASE_BYTES) {
        Ok(outcomes) if outcomes.len() <= super::MAX_BATCH * super::Reviewer::ALL.len() => {
            let before = outcomes.len();
            let kept = outcomes
                .into_iter()
                .filter(|one| one.for_valid_case(case).is_ok())
                .collect::<Vec<_>>();
            let invalid = before - kept.len();
            (kept, invalid)
        }
        _ => (Vec::new(), 1),
    }
}

fn identity(word: &str) -> bool {
    word.len() == 64
        && word
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn case_name(name: &str) -> bool {
    name.strip_suffix(".json").is_some_and(identity)
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn read_json<T: DeserializeOwned>(path: &Path, cap: usize) -> io::Result<T> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > cap as u64 {
        return Err(io::Error::other("review file is not a bounded plain file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(cap as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > cap {
        return Err(io::Error::other("review file grew past its limit"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn atomic_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut size = super::SizeBound {
        remaining: MAX_CASE_BYTES - 1,
    };
    serde_json::to_writer(&mut size, value).map_err(io::Error::other)?;
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = private_options().create_new(true).open(&temp)?;
        serde_json::to_writer(&mut file, value).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
