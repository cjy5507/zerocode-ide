//! The window's half of a finished task's cost (t-9470): the usage scans it
//! holds and when each was read, the stamping seats' Jev ledgers as far as
//! they have been read, and the costs already worked out — so a ledger beat
//! that moved nothing a finished task's cost is made of works nothing out.
//!
//! The rules are the core's ([`task_cost::task_cost`]); this is where the
//! facts are held. No transcript is opened here: the conversations are the
//! ones the usage scans already hold in memory, read into the book once per
//! scan ([`ScansAt`]). A Jev ledger is read on from its last whole row only
//! while the rows already read are still the ones it holds ([`JevLedger`]) —
//! an offset says how far a read got, never that it is still the same file,
//! and a ledger rewritten under the same name is read again from the top.
//! They are the seats' own files; this only reads them.

use std::collections::HashMap;
use std::hash::Hasher;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::SystemTime;

use zerocode_core::orchestration::task_cost::{
    self, JevBook, JevTally, SessionAttribution, SessionBook, TASK_STAMPED, TaskCost,
};
use zerocode_core::orchestration::{Ledger, Run, Task};

use super::LedgerAgent;

mod receipts;
mod zo_usage;

/// When each held scan was read — Claude's, Codex's, OpenCode's — `None`
/// where none is held. The book reads the conversations again only when this
/// moves.
pub(crate) type ScansAt = [Option<i64>; 3];

/// What a finished task's cost was worked out from — every fact
/// [`task_cost::task_cost`] reads that can still move once the task is
/// finished. Equal keys, equal costs.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MemoKey {
    ended_ms: Vec<Option<i64>>,
    completed_ms: Option<i64>,
    generation: Option<u64>,
    scans: ScansAt,
    jev: JevTally,
    zo_scanned_at: Option<i64>,
}

/// What says a stamping ledger is the file last read: its length, when it
/// was last written, and when it was made — taken off the handle the ledger
/// is then read through, so the two describe one file. An append moves the
/// first two, a rewrite one of the three; only a rewrite to the same length
/// inside one tick of the file's clock passes for the file it replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Witness {
    length: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

/// One stamping ledger as far as the book has read it: the file it was, the
/// bytes of whole rows read, their fingerprint, and what they came to.
#[derive(Default)]
struct JevLedger {
    path: PathBuf,
    witness: Option<Witness>,
    read: usize,
    fingerprint: u64,
    book: JevBook,
}

impl JevLedger {
    fn at(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            ..Self::default()
        }
    }

    /// Brings the book up to the file as it stands, answering whether it
    /// read anything. A witness that has not moved reads nothing; a file
    /// whose bytes already read still carry their fingerprint is read on from
    /// its last whole row; anything else is read again from the top.
    fn catch_up(&mut self) -> bool {
        let Some((file, witness)) = opened(&self.path) else {
            let held = self.witness.is_some();
            *self = Self::at(&self.path);
            return held;
        };
        if self.witness == Some(witness) {
            return false;
        }
        // As far as the witness says and no further: rows appended since
        // are the next beat's, read with the witness that counts them.
        let mut bytes = Vec::new();
        if file.take(witness.length).read_to_end(&mut bytes).is_err() {
            return false;
        }
        // Whole rows only: a row still being written waits for its end.
        let whole = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |at| at + 1);
        let unchanged = self.read <= whole && fingerprint(&bytes[..self.read]) == self.fingerprint;
        if !unchanged {
            self.book = JevBook::default();
            self.read = 0;
        }
        for line in String::from_utf8_lossy(&bytes[self.read..whole]).lines() {
            if let Ok(row) = serde_json::from_str::<serde_json::Value>(line) {
                self.book.read(&row);
            }
        }
        self.read = whole;
        self.fingerprint = fingerprint(&bytes[..whole]);
        self.witness = Some(witness);
        true
    }
}

/// The fingerprint of rows read — what tells a ledger appended to from one
/// written over.
fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

struct Memo {
    key: MemoKey,
    cost: TaskCost,
    /// The beat that last asked for it — a memo no beat asked for is let go.
    beat: u64,
}

/// The book: what the costs are read from, and what they came to.
#[derive(Default)]
pub(crate) struct CostBook {
    receipt_root: Option<PathBuf>,
    zo_scanned_at: Option<i64>,
    scans: Option<ScansAt>,
    sessions: SessionBook,
    attribution: SessionAttribution,
    jev: Vec<JevLedger>,
    /// Ledger reads that read something, since the book was made.
    jev_reads: usize,
    memo: HashMap<(String, String), Memo>,
    beat: u64,
    /// Costs worked out rather than remembered, since the book was made.
    worked: usize,
}

static BOOK: LazyLock<Mutex<CostBook>> = LazyLock::new(Mutex::default);

/// The window's one book, for the board's beat.
pub(crate) fn book() -> MutexGuard<'static, CostBook> {
    BOOK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl CostBook {
    /// Opens a beat on the facts held now: the scans the usage panes read,
    /// and the stamping seats' ledgers under zo's config home.
    pub(crate) fn begin(&mut self, ledger: &Ledger) {
        self.receipt_root = crate::api_routers::zo_settings_path().and_then(|settings| {
            settings
                .parent()
                .map(|home| home.join("state/task-cost-receipts"))
        });
        self.begin_with(ledger, held_scans(), read_held_scans, &stamping_ledgers());
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok());
        if let Some(now) = now
            && self
                .zo_scanned_at
                .is_none_or(|previous| now.saturating_sub(previous) >= 30_000)
        {
            zo_usage::read(ledger, &mut self.sessions, now);
            self.zo_scanned_at = Some(now);
        }
    }

    /// Opens a beat on the facts handed in: when each scan was read, how to
    /// read their conversations — asked only when those times moved — and
    /// the ledgers to read on in.
    fn begin_with(
        &mut self,
        ledger: &Ledger,
        scans: ScansAt,
        read: impl FnOnce(&mut SessionBook),
        ledgers: &[PathBuf],
    ) {
        self.beat += 1;
        self.attribution = SessionAttribution::new(ledger.runs());
        if self.scans != Some(scans) {
            let mut sessions = SessionBook::default();
            read(&mut sessions);
            self.sessions = sessions;
            self.zo_scanned_at = None;
            self.scans = Some(scans);
        }
        self.read_jev(ledgers);
    }

    /// Lets go of every cost this beat did not ask for.
    pub(crate) fn end(&mut self) {
        let beat = self.beat;
        self.memo.retain(|_, memo| memo.beat == beat);
    }

    /// What a finished task cost: remembered while nothing it is made of
    /// moved ([`MemoKey`]), worked out by the core's one rule otherwise.
    pub(crate) fn cost(&mut self, run: &Run, task: &Task) -> TaskCost {
        let key = self.key_of(run, task);
        let beat = self.beat;
        let slot = (run.id.clone(), task.id.clone());
        if let Some(memo) = self.memo.get_mut(&slot)
            && memo.key == key
        {
            memo.beat = beat;
            return memo.cost.clone();
        }
        let mut cost =
            task_cost::task_cost(run, &task.id, &self.sessions, key.jev, &self.attribution);
        if cost.wall_ms.is_some()
            && let Some(root) = self.receipt_root.as_ref()
        {
            receipts::preserve(
                root,
                &run.id,
                &task.id,
                key.generation,
                &mut cost.generation,
            );
        }
        self.worked += 1;
        self.memo.insert(
            slot,
            Memo {
                key,
                cost: cost.clone(),
                beat,
            },
        );
        cost
    }

    pub(super) fn attempt_generation(
        &self,
        run: &Run,
        dispatch: &zerocode_core::orchestration::Dispatch,
    ) -> task_cost::GenerationCost {
        task_cost::attempt_generation(run, dispatch, &self.sessions, &self.attribution)
    }

    /// The worker rows the board draws, each carrying its task's cost where
    /// the task is finished ([`super::desk::finished`]).
    pub(crate) fn dress(
        &mut self,
        ledger: &Ledger,
        mut agents: Vec<LedgerAgent>,
    ) -> Vec<LedgerAgent> {
        for row in &mut agents {
            let Some(run) = ledger.run(&row.run) else {
                continue;
            };
            let Some(task) = run.task(&row.task_id) else {
                continue;
            };
            if super::desk::finished(run, task) {
                row.cost = Some(self.cost(run, task));
            }
        }
        agents
    }

    fn key_of(&self, run: &Run, task: &Task) -> MemoKey {
        let attempts = task_cost::attempts(run, &task.id);
        MemoKey {
            ended_ms: attempts.iter().map(|one| one.ended_ms).collect(),
            completed_ms: run.completion_ms(task),
            generation: self.attribution.generation_fingerprint(&run.id),
            scans: self.scans.unwrap_or_default(),
            jev: self.jev.iter().fold(JevTally::default(), |sum, ledger| {
                sum + ledger.book.tally_in(&run.id, &task.id)
            }),
            zo_scanned_at: self.zo_scanned_at,
        }
    }

    /// Brings every stamping ledger's book up to its file.
    fn read_jev(&mut self, ledgers: &[PathBuf]) {
        let same = self.jev.len() == ledgers.len()
            && self
                .jev
                .iter()
                .zip(ledgers)
                .all(|(held, path)| held.path == *path);
        if !same {
            self.jev = ledgers.iter().map(|path| JevLedger::at(path)).collect();
        }
        for ledger in &mut self.jev {
            self.jev_reads += usize::from(ledger.catch_up());
        }
    }

    /// What `task_id`'s stamped rows came to, over every stamping ledger.
    #[cfg(test)]
    fn jev_tally(&self, task_id: &str) -> JevTally {
        self.jev.iter().fold(JevTally::default(), |sum, ledger| {
            sum + ledger.book.tally(task_id)
        })
    }
}

/// A stamping ledger opened, with its witness off the same handle; `None`
/// where it is not there or will not open.
fn opened(path: &Path) -> Option<(std::fs::File, Witness)> {
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    let witness = Witness {
        length: meta.len(),
        modified: meta.modified().ok(),
        created: meta.created().ok(),
    };
    Some((file, witness))
}

/// When each scan the usage panes hold was read.
fn held_scans() -> ScansAt {
    [
        crate::usage_stats_scan::with_held(|scan| scan.scanned_at),
        crate::usage_stats_scan::with_codex_held(|scan| scan.scanned_at),
        crate::usage_stats_opencode_scan::with_held(|scan| scan.scanned_at),
    ]
}

/// The held scans' conversations, read where they lie — no copy of a ledger
/// thousands of conversations long.
fn read_held_scans(sessions: &mut SessionBook) {
    crate::usage_stats_scan::with_held(|scan| sessions.read_claude(&scan.ledger, scan.scanned_at));
    crate::usage_stats_scan::with_codex_held(|scan| {
        sessions.read_codex(&scan.ledger, scan.scanned_at);
    });
    crate::usage_stats_opencode_scan::with_held(|scan| {
        sessions.read_opencode(&scan.ledger, scan.scanned_at);
    });
}

/// The stamping seats' ledgers under zo's config home.
fn stamping_ledgers() -> Vec<PathBuf> {
    let wire = crate::systemone::Wire::of_this_machine();
    TASK_STAMPED
        .iter()
        .filter_map(|seat| crate::systemone::ledger_of(&wire, seat))
        .collect()
}

#[cfg(test)]
mod tests;
