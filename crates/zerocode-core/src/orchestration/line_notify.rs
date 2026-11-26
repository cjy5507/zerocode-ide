//! `line-notify` (t-131644, F1 of the review t-131831): `zo line add` binds a
//! build line job's end to the pane that puts the job in, before the job runs.
//!
//! The line tells a job's end through `zerocode-orc send` from the job's
//! waiter, which keeps the environment of the pane that put the job in. A
//! window restart mints every pane's address anew and refuses a verb from an
//! address no pane of it holds, so an end that outlives a restart is told by
//! the ledger itself (`line_ends` in the shell). What the line's record says
//! cannot stand for who asked: any process of the person's can write a record,
//! and its folder is whatever folder `zo line add` ran in. So the pane binds
//! the end while it is still the window's own. The window holds this verb to
//! a `send`'s boundary ([`super::project_scope::reach`]); the ledger answers
//! only for a job id, an address the run reads at — a worker of the run, or
//! its chair — and a pane and a run of one known project; and the answer is
//! filed under the job's own name ([`crate::line::bound_as`]) as every named
//! answer is. Nothing else is written: no letter, no row.
//!
//! The ledger tells an end only when such an answer stands for that job, at
//! the address and in the run the line's record names, in the project the run
//! is written down as then, filed no later than the job was put in
//! ([`Ledger::line_end_bound`]). That last is what binds a job to the pane
//! that put it in (review t-131848): `zo line add` binds before it puts the
//! job in and queues it from the answer, and another pane can read the job's
//! id only off the line, once the job is in it — the format of an id says
//! nothing of whose it is. A window too old to know the verb, a `zo` too old
//! to ask it, an answer the retention sweep has taken, a binding filed after
//! the job went in: the end is not told, and its record keeps that it was not.

use super::{
    Ledger, RETRY_REQUEST, RUN_ADDRESS_PREFIX, ServedAnswer, Team, WORKER_ADDRESS_PREFIX, Words,
    bound, sender,
};

/// The verb, named once: the table, the plan, the window's boundary and `zo`.
pub const VERB: &str = "line-notify";

/// A job's end bound to the pane that put the job in: what `line-notify`
/// answers, and so what its receipt keeps.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bound {
    /// The build line job.
    pub job: String,
    /// Where its end is told: `worker:<id>` or `run:<id>`.
    pub to: String,
    /// The run it is told in.
    pub run: String,
    /// The project the pane worked in: the window's record of the pane's
    /// folder, read for its repository, the same as the run's when bound.
    pub project: String,
}

/// `line-notify --job <id> --to <worker:…|run:…> [--run <id>]` from the pane
/// `team/pane`, working in `project` — the window's record of its folder:
/// `Some("")` for a pane in none it knows, `None` only for the window's own
/// verbs. Answers the binding, or why the ledger could not tell that end.
pub(super) fn bind(
    ledger: &Ledger,
    team: &Team,
    pane: &str,
    words: &Words,
    (caller, seat): (&str, &str),
    project: Option<&str>,
) -> Result<serde_json::Value, String> {
    let job = words
        .value("--job")
        .ok_or("line-notify needs --job <id>: the build line job whose end is bound")?;
    if !crate::line::is_job_id(job) {
        return Err(format!("`{job}` is no build line job id"));
    }
    let name = crate::line::bound_as(job);
    if words.value(RETRY_REQUEST) != Some(name.as_str()) {
        return Err(format!(
            "line-notify binds {job} under {RETRY_REQUEST} {name} — the name the ledger \
             finds the binding by"
        ));
    }
    let to = words
        .value("--to")
        .ok_or("line-notify needs --to <worker:…|run:…>: where the job's end is told")?;
    let run_id = bound(ledger, words, caller, seat)?;
    match to.strip_prefix(RUN_ADDRESS_PREFIX) {
        Some(there) if there != run_id => {
            return Err(format!(
                "--to {to} names another run than {run_id} — bind a run's chair in that run \
                 (--run {there})"
            ));
        }
        Some(_) => {}
        None if to.starts_with(WORKER_ADDRESS_PREFIX) => {}
        None => {
            return Err(format!(
                "the ledger tells a build line job's end only to a worker or a run's chair, \
                 not to {to}"
            ));
        }
    }
    let from = sender(ledger, &team.leader_pane, &run_id, (&team.id, pane));
    ledger.resolve_address(&run_id, to, &from)?;
    let here = project.filter(|root| !root.is_empty()).ok_or(
        "the project this pane works in is not known, so the ledger cannot tell a build line \
         job's end it puts in",
    )?;
    let run = ledger
        .run(&run_id)
        .ok_or_else(|| super::unknown_run(&run_id))?;
    match run.project.as_ref() {
        Some(theirs) if theirs.root == here => {}
        Some(_) => {
            return Err(format!(
                "{run_id} is another project's than this pane's — the ledger tells no build \
                 line job's end across projects"
            ));
        }
        None => {
            return Err(format!(
                "{run_id}'s project is not written down, so the ledger cannot tell a build \
                 line job's end in it"
            ));
        }
    }
    serde_json::to_value(Bound {
        job: job.to_string(),
        to: to.to_string(),
        run: run_id,
        project: here.to_string(),
    })
    .map_err(|error| error.to_string())
}

impl Ledger {
    /// Whether build line job `job`'s end, the job put in at `put_ms` as the
    /// line's record says, is bound to be told at `to` in `run`: an answer of
    /// `line-notify` stands under the job's own name for that job, that
    /// address and that run, by a pane of the project `run` is written down as
    /// now, filed no later than `put_ms` (review t-131848). The pane that put
    /// the job in bound its end before it put the job in; a pane that read the
    /// id off the line bound it after, and its answer is never told by, though
    /// it stands. An answer with no filing time cannot say which it was. The
    /// ledger's clock and the line's are the one machine's. A run this ledger
    /// does not hold, or holds with no project, has none.
    #[must_use]
    pub fn line_end_bound(&self, job: &str, to: &str, run: &str, put_ms: i64) -> bool {
        let Some(project) = self.run(run).and_then(|held| held.project.as_ref()) else {
            return false;
        };
        let name = crate::line::bound_as(job);
        self.served
            .iter()
            .filter(|held| {
                held.request == name
                    && held.verb.as_deref() == Some(VERB)
                    && !held.is_tombstone()
                    && held.filed_ms.is_some_and(|filed| filed <= put_ms)
            })
            .filter_map(|held| match &held.answer {
                ServedAnswer::Inline(said) => serde_json::from_str::<Bound>(said).ok(),
                ServedAnswer::Check(_) => None,
            })
            .any(|binding| {
                binding.job == job
                    && binding.to == to
                    && binding.run == run
                    && binding.project == project.root
            })
    }
}
