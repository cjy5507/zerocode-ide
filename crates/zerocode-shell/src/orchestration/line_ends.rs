//! A build line job's end the line could not tell, told by the ledger
//! (t-131644, I-line-notify-1).
//!
//! `zo line` tells a job's end through `zerocode-orc send`, signed with the
//! seat of the pane that put the job in: the job's waiter keeps that pane's
//! environment. A window restart mints every seat anew, so the end of a job
//! put in before a restart and ending after it is told under a seat no pane of
//! the new window holds, and the window refuses it — on 2026-11-25 and 26
//! three ends were told to nobody, and the workers waiting on them waited. The
//! line writes such a failure in the job's record (`notify_failed`), with
//! whether the window the teller's seat came from had ended by then
//! (`window_gone`).
//!
//! For those ends the beat has the ledger tell, in the ledger's own voice: one
//! `status` from the ledger itself at the address the job named, the road a
//! window observation for one seat takes (`seat_observation`, t-67064) — a
//! worker of the run the job was put in from, or a run's chair.
//!
//! - **The wall.** Only when the job's checkout is in the project the run is
//!   written down as, the run's recorded project the release sweep answers
//!   for too (t-94842). A run of another project, or a run or a checkout whose
//!   project is not known, is not told; the record keeps the failure, and the
//!   window log says why.
//! - **Facts only.** The job, how and when it ended, its code, its log, and
//!   why the ledger tells it — never the words whoever put the job in wrote of
//!   it: the ledger's voice is not theirs to borrow.
//! - **Once.** Under the end's name (`line::told_as`), which the line's own
//!   letter carries as its subject too: not when a letter by that name stands
//!   in the run, whoever sent it, and the ledger files one of its own a name.
//!
//! A teller whose window stood when its tell failed is not the ledger's to
//! speak for — that send may still land — so its failure stays in the record
//! and the log.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use zerocode_core::line::{self, Job, Line};
use zerocode_core::orchestration::project_scope::label;
use zerocode_core::orchestration::{RUN_ADDRESS_PREFIX, WORKER_ADDRESS_PREFIX};
use zerocode_orchestrator::runtime_actor::RuntimeError;

use crate::agent_teams::Host;

use super::{LiveRuntime, note_ledger_unwritten, project_scope, rang, runtime, with_ledger_seats};

/// What the beat holds of the line it reads: its stamp when last read — the
/// records are read again only once it moves — the ends this window settled,
/// told or not the ledger's to tell, and the ends the ledger could not be
/// asked about yet, asked again each beat without reading the line.
#[derive(Default)]
struct Seen {
    stamp: Option<(PathBuf, u64)>,
    settled: HashSet<String>,
    again: Vec<Job>,
}

fn memory() -> &'static Mutex<Seen> {
    static SEEN: OnceLock<Mutex<Seen>> = OnceLock::new();
    SEEN.get_or_init(|| Mutex::new(Seen::default()))
}

/// How asking about one end went.
enum Asked {
    /// Told, or not the ledger's to tell: not asked again.
    Settled,
    /// The ledger could not be read or written: asked again on a later beat.
    Again,
}

/// The beat's pass over the host's build line, when it has one.
pub(super) fn sweep(host: &dyn Host, now_ms: i64) {
    if let Some(line) = host.build_line() {
        sweep_line(&line, now_ms);
    }
}

/// One pass over `line`: each ended job whose end its line could not tell, for
/// a seat whose window was gone, is told by the ledger or settled as not its to
/// tell. The records are read only when the line moved; an end the ledger
/// could not be asked about is asked again on the next pass.
pub(super) fn sweep_line(line: &Line, now_ms: i64) {
    let Some(held) = runtime() else {
        return;
    };
    let stamp = (line.root().to_path_buf(), line.stamp());
    let mut seen = memory().lock().unwrap_or_else(|held| held.into_inner());
    if seen
        .stamp
        .as_ref()
        .is_some_and(|(root, _)| *root != stamp.0)
    {
        *seen = Seen::default();
    }
    let mut asking = std::mem::take(&mut seen.again);
    if seen.stamp.as_ref() != Some(&stamp) {
        let jobs = line.jobs();
        seen.settled
            .retain(|id| jobs.iter().any(|job| job.id == *id));
        for job in jobs {
            if untold(&job)
                && !seen.settled.contains(&job.id)
                && !asking.iter().any(|waiting| waiting.id == job.id)
            {
                asking.push(job);
            }
        }
        seen.stamp = Some(stamp);
    }
    for end in asking {
        match ask(&held, line, &end, now_ms) {
            Asked::Settled => {
                seen.settled.insert(end.id);
            }
            Asked::Again => seen.again.push(end),
        }
    }
}

/// An end its line could not tell, for a seat whose window had ended.
fn untold(job: &Job) -> bool {
    job.state.ended()
        && job
            .notify_failed
            .as_ref()
            .is_some_and(|failed| failed.window_gone)
}

/// The run a job's end is told in, and the address told there: a run's chair,
/// in that run, or a worker, in the run the pane that put the job in stood in
/// (`notify_run`). A pane's address goes with its window: none.
fn addressed(job: &Job) -> Option<(String, String)> {
    let to = job.notify.as_deref()?;
    let run = match to.strip_prefix(RUN_ADDRESS_PREFIX) {
        Some(run) => run.to_string(),
        None if to.starts_with(WORKER_ADDRESS_PREFIX) => job.notify_run.clone()?,
        None => return None,
    };
    Some((run, to.to_string()))
}

/// Tell `end`, unless it is not the ledger's to tell.
fn ask(held: &LiveRuntime, line: &Line, end: &Job, now_ms: i64) -> Asked {
    let Some((run, to)) = addressed(end) else {
        return Asked::Settled;
    };
    let name = line::told_as(&end.id);
    let checkout = project_scope::folder_root(Path::new(&end.checkout));
    let Some(found) = with_ledger_seats(|ledger, _| {
        ledger.run(&run).map(|row| {
            let project = row.project.as_ref().map(|project| project.root.clone());
            let stands = row
                .messages()
                .iter()
                .any(|mail| mail.subject.as_str() == name);
            (project, stands)
        })
    }) else {
        return Asked::Again;
    };
    // A run this ledger does not hold, or a letter about the end already there.
    let Some((project, false)) = found else {
        return Asked::Settled;
    };
    if project.is_none() || project != checkout {
        project_scope::note(&format!(
            "the end of build line job {} was not told to {to} in {run}: the job's checkout {} \
             is in {}, and {run} is {} — the ledger tells no run the end of a job in another \
             or an unidentified project",
            end.id,
            end.checkout,
            checkout
                .as_deref()
                .map_or_else(|| "an unidentified project".to_string(), label),
            project
                .as_deref()
                .map_or_else(|| "an unidentified project".to_string(), label),
        ));
        return Asked::Settled;
    }
    match held
        .actor
        .seat_observation(run, to, letter(line, end), name, now_ms)
    {
        Ok((moved, _)) => {
            rang(moved);
            Asked::Settled
        }
        Err(error) => {
            if let Some(count) = project_scope::worth_a_line(format!("line end {}", end.id)) {
                note_ledger_unwritten(&format!(
                    "the end of build line job {} ({error}; met {count} time(s))",
                    end.id
                ));
            }
            // A letter the ledger will never take is not asked for again.
            if matches!(error, RuntimeError::InvalidInput) {
                Asked::Settled
            } else {
                Asked::Again
            }
        }
    }
}

/// The letter: the facts of the end, and why the ledger tells it.
fn letter(line: &Line, end: &Job) -> String {
    let code = end
        .rc
        .map_or_else(|| "no code".to_string(), |rc| format!("rc {rc}"));
    let at = end.ended_ms.map_or_else(
        || "a moment not written down".to_string(),
        zerocode_core::civil::iso_utc_of,
    );
    format!(
        "zo line {}: {} ({code}) at {at}; its log is {}. The line could not tell this end: the \
         pane that put the job in held a seat of a window that has restarted since, so the \
         ledger tells it.",
        end.id,
        end.state.word(),
        line.log_path(&end.id).display(),
    )
}
