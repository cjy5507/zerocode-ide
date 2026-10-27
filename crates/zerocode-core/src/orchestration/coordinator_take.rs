//! A coordinator's own attempt on a task (t-78075): `task-take`, `hand-in` and `task-backfill`.
//!
//! A review names the attempt and the source it looked at, and an attempt has a source only when
//! a report ended it ([`Dispatch::source`]). So work a coordinator took over from a worker that
//! handed nothing in — or did by hand from the start — could take no review at all: `task-update`
//! refused it ("the attempt has not handed in a source yet"), and the only road left was closing
//! the task `outdated` with the facts in its result, where the board and the review count never
//! read them.
//!
//! This file fills the steps BEFORE the review and nothing else. The coordinator takes the task
//! as an attempt of its own seat ([`TAKE_VERB`]), hands a commit in on it ([`HAND_IN_VERB`]) and
//! then reviews it with the same `task-update --attempt --source --result` every other review
//! goes through. The ledger writes only what it heard: the commit is one git answered for, read
//! by the window before the plan ([`CommitsSeen`]) because the ledger runs no git and no process
//! ([`super::land_check`] keeps the same split). [`BACKFILL_VERB`] walks the three steps for old
//! finished work whose result already names a landed commit, writing only what git proves.

use std::collections::BTreeMap;

use super::*;

/// The verb that opens a coordinator's own attempt on a task.
pub const TAKE_VERB: &str = "task-take";

/// The verb that hands a commit in on a coordinator's own attempt.
pub const HAND_IN_VERB: &str = "hand-in";

/// The verb that writes the three steps for finished work whose result names a landed commit.
pub const BACKFILL_VERB: &str = "task-backfill";

/// What a coordinator's own attempt names in [`Dispatch::worker`], before its seat's generation:
/// `coordinator@g3`. Not a worker id — no worker row carries it — and read back by
/// [`carrier_generation`] alone.
pub const COORDINATOR_CARRIER: &str = "coordinator@g";

/// The ref a backfill measures landed commits against when `--base` names none.
pub const DEFAULT_BASE: &str = "main";

/// The tags a backfill reads as releases when `--tags` names none: a glob, as `git tag --list`
/// takes it.
pub const RELEASE_TAGS: &str = "v*";

/// The carrier a coordinator seat of this generation writes on its own attempt.
#[must_use]
pub fn coordinator_carrier(generation: u32) -> String {
    format!("{COORDINATOR_CARRIER}{generation}")
}

/// The generation of the coordinator seat carrying an attempt, when a coordinator carries it.
#[must_use]
pub fn carrier_generation(worker: &str) -> Option<u32> {
    worker.strip_prefix(COORDINATOR_CARRIER)?.parse().ok()
}

/// What git said about the commits a hand-in or a backfill names.
///
/// Read by the window before the ledger plans, in the checkout the verb names, and judged here:
/// the ledger runs no git, so a commit it was not told about is a commit it does not know. Keyed
/// by the commit as it was named, in lower case.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitsSeen {
    /// The checkout git was asked in.
    pub repo: String,
    /// The ref the commits were measured against, as named, when the verb named one.
    pub base: Option<BaseSeen>,
    /// The glob of release tags each commit was looked for in, when the verb asked.
    pub tags: Option<String>,
    pub commits: BTreeMap<String, CommitSeen>,
}

/// The ref a verb measured against, and the commit git resolved it to — `None` when git does not
/// know the name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseSeen {
    pub name: String,
    pub head: Option<String>,
}

/// One commit, as git answered for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitSeen {
    /// The full hash git resolved the name to; `None` when git knows no such commit.
    pub full: Option<String>,
    /// Whether the commit is the base or one of its ancestors; `None` when nothing was asked or
    /// git could not say.
    pub in_base: Option<bool>,
    /// The first release tag (in version order) that holds the commit, when one does.
    pub tag: Option<String>,
}

impl CommitsSeen {
    /// What git said about `named`, whatever case it was written in.
    #[must_use]
    pub fn commit(&self, named: &str) -> Option<&CommitSeen> {
        self.commits.get(&named.trim().to_ascii_lowercase())
    }

    /// Write down what git said about `named`.
    pub fn insert(&mut self, named: &str, seen: CommitSeen) {
        self.commits
            .insert(named.trim().to_ascii_lowercase(), seen);
    }
}
