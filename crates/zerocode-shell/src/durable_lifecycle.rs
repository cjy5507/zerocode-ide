//! Durable lanes for the lifecycle effects — `Close`, `Release`, `Respawn`.
//!
//! The journal has known these three kinds since it was laid down, but only
//! `Split` ever asks it for a permit: the shell performs the others bare, and
//! a crash between "the ledger says released" and "the pane is actually gone"
//! leaves a window the ledger no longer describes, with no record of how far
//! the host got. These lanes give each lifecycle effect the same discipline a
//! split has — prepare, then stride through the work one durable checkpoint
//! at a time, then settle — so the next window can read what already happened
//! instead of guessing.
//!
//! The lane owns the VOCABULARY and the ORDER of its strides; the journal
//! checks only their shape and their place in line. That is the same division
//! that keeps enum spellings out of the schema: one place holds the words.
//!
//! Nothing here is wired to a live road yet. The split lane went live with
//! the actor cutover; these lanes go live at the LIFECYCLE cutover slice,
//! when Close/Release/Respawn walk the journal the same way.
#![allow(dead_code)] // Intentionally unwired until the lifecycle cutover slice.

use zerocode_orchestrator::effect_journal::{
    AuthorityLease, EffectJournal, EffectJournalError, EffectPermit, HostEffectKind,
    HostEffectState,
};

use crate::durable_split::{MAX_PANE_BYTES, PaneIncarnation, digest};

/// Which lifecycle effect a lane is walking through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LifecycleKind {
    Close,
    Release,
    Respawn,
}

impl LifecycleKind {
    pub(crate) const fn host_kind(self) -> HostEffectKind {
        match self {
            Self::Close => HostEffectKind::Close,
            Self::Release => HostEffectKind::Release,
            Self::Respawn => HostEffectKind::Respawn,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::Release => "release",
            Self::Respawn => "respawn",
        }
    }

    /// The strides this effect takes, in the only order it takes them.
    ///
    /// These mirror the live arms in `agent_teams.rs`: a close settles the
    /// seat before the pane leaves the table, and a respawn cuts the new
    /// pane, settles the old dispatch, and re-points the seat before the old
    /// shell is ended. The final host action (the kill, the close) is not a
    /// stride — it is the settlement itself, recorded by `settle`.
    pub(crate) const fn strides(self) -> &'static [&'static str] {
        match self {
            Self::Close => &["seat_settled", "pane_removed"],
            Self::Release => &["seat_settled"],
            Self::Respawn => &["pane_cut", "seat_settled", "seat_repointed"],
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LifecycleLaneError {
    InvalidInput { field: &'static str },
    MissingIncarnation,
    PermitMismatch,
    StrideOutOfTurn,
    RecordDisagrees,
    Journal(EffectJournalError),
}

const fn invalid(field: &'static str) -> LifecycleLaneError {
    LifecycleLaneError::InvalidInput { field }
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Captured before the journal mints an operation id: which pane, in which
/// incarnation, is about to be walked through which lifecycle.
pub(crate) struct LifecycleDraft {
    kind: LifecycleKind,
    incarnation: PaneIncarnation,
    binding_digest: String,
    host_epoch: String,
}

/// The permit that comes back for this draft hides these same digests as
/// `<opaque-host-binding>` — a value cannot be a secret in one place and a
/// log line in another.
impl std::fmt::Debug for LifecycleDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LifecycleDraft")
            .field("kind", &self.kind)
            .field("incarnation", &self.incarnation)
            .field("binding", &"<opaque-host-binding>")
            .field("host_epoch", &"<opaque>")
            .finish()
    }
}

impl LifecycleDraft {
    /// Unlike a split — whose destination must NOT exist — a lifecycle names
    /// a pane that must be there right now, in the incarnation the caller is
    /// looking at.
    pub(crate) fn capture(
        kind: LifecycleKind,
        team: &str,
        pane: &str,
        host_epoch: String,
    ) -> Result<Self, LifecycleLaneError> {
        if team.is_empty() || team.len() > MAX_PANE_BYTES {
            return Err(invalid("team"));
        }
        if pane.is_empty() || pane.len() > MAX_PANE_BYTES {
            return Err(invalid("pane"));
        }
        if !valid_digest(&host_epoch) {
            return Err(invalid("host_epoch"));
        }
        let incarnation =
            PaneIncarnation::capture(team, pane).ok_or(LifecycleLaneError::MissingIncarnation)?;
        let binding_digest = lifecycle_binding_digest(kind, &incarnation, &host_epoch);
        Ok(Self {
            kind,
            incarnation,
            binding_digest,
            host_epoch,
        })
    }

    /// Digest passed to `EffectRequest` before `prepare` returns a permit.
    pub(crate) fn binding_digest(&self) -> &str {
        &self.binding_digest
    }

    pub(crate) fn host_epoch(&self) -> &str {
        &self.host_epoch
    }

    /// Bind the permit the journal minted for THIS draft, and nothing else.
    ///
    /// On refusal the permit comes BACK beside the error, the same shape
    /// the split lane's `finish` uses at the other end of this lane. It matters
    /// more HERE than anywhere in the actor: the shell cannot ask the journal
    /// to mint a row again, so a permit dropped on this side is a fence
    /// nothing in this window will ever lift.
    pub(crate) fn finalize(
        self,
        permit: EffectPermit,
    ) -> Result<LifecycleLease, Box<(EffectPermit, LifecycleLaneError)>> {
        if permit.kind() != self.kind.host_kind()
            || permit.state() != HostEffectState::Prepared
            || permit.binding_digest() != self.binding_digest
            || permit.host_epoch() != self.host_epoch
            || !valid_digest(permit.operation_id())
        {
            return Err(Box::new((permit, LifecycleLaneError::PermitMismatch)));
        }
        Ok(LifecycleLease {
            kind: self.kind,
            incarnation: Some(self.incarnation),
            permit,
            taken: 0,
        })
    }
}

/// One lifecycle effect being walked, stride by durable stride.
#[derive(Debug)]
pub(crate) struct LifecycleLease {
    kind: LifecycleKind,
    /// `Some` when the lane was drafted against a live pane this window
    /// captured; `None` after recovery, where the pane may LEGITIMATELY be
    /// gone — a close that crashed after `pane_removed` has nothing left to
    /// capture, and that absence is the record being true.
    incarnation: Option<PaneIncarnation>,
    permit: EffectPermit,
    taken: usize,
}

impl LifecycleLease {
    /// This window cannot finish this walk. Spelled, not dropped — see
    /// [`EffectPermit::abandon`].
    pub(crate) fn abandon(self, why: &'static str) {
        self.permit.abandon(why);
    }

    /// Rebuild the lane out of what the journal remembers, after a crash.
    ///
    /// The lane reads the record ITSELF, through the permit it was handed —
    /// there is no parameter for a caller to pair this permit with some
    /// other operation's strides, so the mispairing that would finish the
    /// wrong walk cannot be spelled.
    ///
    /// The strides must be a PREFIX of the kind's table — the record and the
    /// table disagreeing is not "some progress", it is two accounts of the
    /// same operation that cannot both be true, and a lane must not walk on
    /// from a record it cannot read.
    ///
    /// On refusal the permit comes BACK beside the error. This one is a
    /// RECOVERY door, which makes eating the permit worse than anywhere
    /// else in the lane: the window it refuses is the window that was
    /// already trying to finish somebody's interrupted walk, and the shell
    /// cannot ask the journal to mint the row again.
    pub(crate) fn recover(
        kind: LifecycleKind,
        permit: EffectPermit,
        journal: &EffectJournal,
        authority: &AuthorityLease,
    ) -> Result<Self, Box<(EffectPermit, LifecycleLaneError)>> {
        let refuse = |permit: EffectPermit, why: LifecycleLaneError| Box::new((permit, why));
        if permit.kind() != kind.host_kind()
            || !matches!(
                permit.state(),
                HostEffectState::Prepared | HostEffectState::Unknown
            )
            || !valid_digest(permit.operation_id())
        {
            return Err(refuse(permit, LifecycleLaneError::PermitMismatch));
        }
        let strides = match journal.strides_taken(authority, permit.operation_id()) {
            Ok(strides) => strides,
            Err(why) => return Err(refuse(permit, LifecycleLaneError::Journal(why))),
        };
        let table = kind.strides();
        if strides.len() > table.len() {
            return Err(refuse(permit, LifecycleLaneError::RecordDisagrees));
        }
        for (at, taken) in strides.iter().enumerate() {
            if taken.ordinal as usize != at || taken.stride != table[at] {
                return Err(refuse(permit, LifecycleLaneError::RecordDisagrees));
            }
        }
        Ok(Self {
            kind,
            incarnation: None,
            permit,
            taken: strides.len(),
        })
    }

    /// Record that the NEXT stride of the table has durably happened.
    ///
    /// The caller performs the work first and strides second — a checkpoint
    /// names what already happened, never what is merely intended. The name
    /// must be the table's next word: passing it spells out, at the call
    /// site, which stride the caller believes it just took, and the lane
    /// refuses a belief that disagrees with the table.
    pub(crate) fn stride(
        &mut self,
        journal: &EffectJournal,
        authority: &AuthorityLease,
        name: &str,
        now_ms: i64,
    ) -> Result<(), LifecycleLaneError> {
        let table = self.kind.strides();
        if self.taken >= table.len() || name != table[self.taken] {
            return Err(LifecycleLaneError::StrideOutOfTurn);
        }
        let ordinal = u32::try_from(self.taken).map_err(|_| LifecycleLaneError::StrideOutOfTurn)?;
        journal
            .step(authority, &self.permit, ordinal, name, now_ms)
            .map_err(LifecycleLaneError::Journal)?;
        self.taken += 1;
        Ok(())
    }

    /// The strides not yet taken, in the order they must be.
    pub(crate) fn remaining(&self) -> &'static [&'static str] {
        &self.kind.strides()[self.taken..]
    }

    /// The incarnation this lane was drafted against, when this window
    /// captured one. The driver runs pane mutations through
    /// [`PaneIncarnation::with_current`], not through the lane.
    pub(crate) fn incarnation(&self) -> Option<&PaneIncarnation> {
        self.incarnation.as_ref()
    }

    /// Hand the permit back for settlement once the walk is done — or once
    /// recovery decides the operation cannot be finished.
    pub(crate) fn into_permit(self) -> EffectPermit {
        self.permit
    }
}

fn lifecycle_binding_digest(
    kind: LifecycleKind,
    incarnation: &PaneIncarnation,
    host_epoch: &str,
) -> String {
    digest(
        b"zerocode.orchestration.lifecycle-binding.v1",
        &[
            kind.as_str().as_bytes(),
            host_epoch.as_bytes(),
            incarnation.team().as_bytes(),
            incarnation.pane().as_bytes(),
            &incarnation.term().to_le_bytes(),
            incarnation.generation().as_bytes(),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_teams::{remember_pane_token, teams};
    use crate::durable_split::lane_fixtures::{snapshot, test_digest};
    use zerocode_core::agent_teams::Team;
    use zerocode_orchestrator::effect_journal::{BeginEffect, EffectRequest, EffectSettlement};
    use zerocode_orchestrator::workflow_store::WorkflowStore;

    fn open_team(base: u32) -> String {
        let id = format!("durable-lifecycle-team-{base}");
        teams().insert(id.clone(), Team::new(id.clone(), "unused", base));
        let _ = remember_pane_token(&id, "%1", format!("leader-token-{base}"));
        id
    }

    struct PermitFactory {
        _root: tempfile::TempDir,
        journal: EffectJournal,
        authority: AuthorityLease,
    }

    impl PermitFactory {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("authority root");
            let private = root.path().join("authority");
            std::fs::create_dir(&private).expect("authority directory");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))
                    .expect("private authority directory");
            }
            let store = WorkflowStore::open(private.join("authority.sqlite")).expect("store");
            let journal = EffectJournal::new(&store);
            let authority = journal
                .claim_authority("main-ledger", &test_digest(0xe1), 0)
                .expect("claim authority");
            journal
                .import_legacy(&authority, &snapshot(0, "legacy"), &test_digest(0xaa), 1)
                .expect("legacy import");
            Self {
                _root: root,
                journal,
                authority,
            }
        }

        fn permit(&self, draft: &LifecycleDraft, kind: LifecycleKind, slot: u8) -> EffectPermit {
            let request = EffectRequest::new(
                "main-ledger",
                test_digest(slot),
                test_digest(0x71),
                draft.binding_digest(),
                draft.host_epoch(),
                kind.host_kind(),
            )
            .expect("effect request");
            match self
                .journal
                .prepare(&self.authority, 0, &snapshot(1, "prepared"), &request, 20)
                .expect("prepare")
            {
                BeginEffect::Execute(permit) => permit,
                other => panic!("expected a fresh permit, got {other:?}"),
            }
        }
    }

    /// The vocabulary is pinned: changing a word or its place is seen HERE,
    /// not discovered by a recovery that no longer reads old records.
    #[test]
    fn the_vocabulary_of_each_lane_is_pinned() {
        assert_eq!(
            LifecycleKind::Close.strides(),
            &["seat_settled", "pane_removed"]
        );
        assert_eq!(LifecycleKind::Release.strides(), &["seat_settled"]);
        assert_eq!(
            LifecycleKind::Respawn.strides(),
            &["pane_cut", "seat_settled", "seat_repointed"]
        );
    }

    /// A lifecycle names a pane that must be there right now.
    #[test]
    fn a_draft_needs_the_pane_it_names() {
        assert_eq!(
            LifecycleDraft::capture(
                LifecycleKind::Close,
                "no-such-team",
                "%9",
                test_digest(0xaa),
            )
            .expect_err("a missing pane must refuse the draft"),
            LifecycleLaneError::MissingIncarnation,
        );
    }

    /// A permit minted for another effect, or another binding, does not bind.
    #[test]
    fn a_permit_for_another_effect_does_not_finalize() {
        let team = open_team(0xc1);
        let factory = PermitFactory::new();
        let close = LifecycleDraft::capture(LifecycleKind::Close, &team, "%1", test_digest(0xaa))
            .expect("close draft");
        let respawn_permit = factory.permit(&close, LifecycleKind::Respawn, 0x51);
        let refused = *close
            .finalize(respawn_permit)
            .expect_err("a respawn permit must not bind a close lane");
        assert_eq!(refused.1, LifecycleLaneError::PermitMismatch);
        refused
            .0
            .abandon("fixture: this test wanted the refusal, not the row");
    }

    /// The walk takes the table's words in the table's order, or not at all.
    #[test]
    fn strides_walk_the_table_in_order_or_not_at_all() {
        let team = open_team(0xc2);
        let factory = PermitFactory::new();
        let draft = LifecycleDraft::capture(LifecycleKind::Respawn, &team, "%1", test_digest(0xab))
            .expect("respawn draft");
        let permit = factory.permit(&draft, LifecycleKind::Respawn, 0x52);
        let mut lane = draft.finalize(permit).expect("lane");

        assert_eq!(
            lane.stride(&factory.journal, &factory.authority, "seat_settled", 21)
                .expect_err("the first stride is pane_cut"),
            LifecycleLaneError::StrideOutOfTurn,
        );
        lane.stride(&factory.journal, &factory.authority, "pane_cut", 21)
            .expect("first stride");
        lane.stride(&factory.journal, &factory.authority, "seat_settled", 22)
            .expect("second stride");
        assert_eq!(lane.remaining(), &["seat_repointed"]);
        lane.stride(&factory.journal, &factory.authority, "seat_repointed", 23)
            .expect("third stride");
        assert_eq!(lane.remaining(), &[] as &[&str]);
        assert_eq!(
            lane.stride(&factory.journal, &factory.authority, "seat_repointed", 24)
                .expect_err("a finished walk takes no more strides"),
            LifecycleLaneError::StrideOutOfTurn,
        );

        factory
            .journal
            .settle(
                &factory.authority,
                lane.into_permit(),
                1,
                &snapshot(2, "settled"),
                EffectSettlement::Applied {
                    result_digest: test_digest(0x91),
                    settled_at_ms: 25,
                },
            )
            .expect("settle the walked effect");
    }

    /// What recovery is FOR: the next window reads the record and knows what
    /// remains — and refuses a record that is not a prefix of the table.
    #[test]
    fn a_recovered_lane_knows_what_remains_and_refuses_a_lying_record() {
        let team = open_team(0xc3);
        let factory = PermitFactory::new();
        let draft = LifecycleDraft::capture(LifecycleKind::Respawn, &team, "%1", test_digest(0xac))
            .expect("respawn draft");
        let draft_binding = draft.binding_digest().to_string();
        let draft_epoch = draft.host_epoch().to_string();
        let request = EffectRequest::new(
            "main-ledger",
            test_digest(0x53),
            test_digest(0x71),
            draft.binding_digest(),
            draft.host_epoch(),
            HostEffectKind::Respawn,
        )
        .expect("effect request");
        let BeginEffect::Execute(permit) = factory
            .journal
            .prepare(
                &factory.authority,
                0,
                &snapshot(1, "prepared"),
                &request,
                20,
            )
            .expect("prepare")
        else {
            panic!("expected a fresh permit");
        };
        let mut lane = draft.finalize(permit).expect("lane");
        lane.stride(&factory.journal, &factory.authority, "pane_cut", 21)
            .expect("first stride");
        /* The crash shape: a window that walked one stride and stopped. It is
         * SAID rather than dropped, because letting the permit go IS what
         * this fixture is testing. */
        lane.abandon("fixture: the crash shape — this window stops mid-walk");

        // The next window: the same operation offered back, and the record.
        let BeginEffect::Reconcile(offered) = factory
            .journal
            .prepare(
                &factory.authority,
                2,
                &snapshot(3, "reconcile"),
                &request,
                30,
            )
            .expect("reconcile")
        else {
            panic!("an in-flight operation reconciles");
        };
        let recovered = LifecycleLease::recover(
            LifecycleKind::Respawn,
            offered,
            &factory.journal,
            &factory.authority,
        )
        .expect("a prefix record recovers");
        assert_eq!(recovered.remaining(), &["seat_settled", "seat_repointed"]);
        assert!(
            recovered.incarnation().is_none(),
            "recovery holds no incarnation — the pane may legitimately be gone"
        );

        // A record that is not a prefix of the table refuses to walk on.
        // The lie lives in the JOURNAL itself: the journal checks only shape
        // and place in line, so a lane gone wrong can durably write a word
        // this lane's table does not have at that position — and recovery,
        // reading through its own permit, must still refuse it.
        let BeginEffect::Reconcile(offered_again) = factory
            .journal
            .prepare(
                &factory.authority,
                2,
                &snapshot(3, "reconcile"),
                &request,
                31,
            )
            .expect("reconcile again")
        else {
            panic!("an in-flight operation reconciles");
        };
        factory
            .journal
            .step(&factory.authority, &offered_again, 1, "pane_removed", 31)
            .expect("a shape-valid word the journal cannot know is wrong");
        let refused = *LifecycleLease::recover(
            LifecycleKind::Respawn,
            offered_again,
            &factory.journal,
            &factory.authority,
        )
        .expect_err("a lying record must not recover");
        assert_eq!(refused.1, LifecycleLaneError::RecordDisagrees);
        refused
            .0
            .abandon("fixture: this test wanted the refusal, not the row");

        // And a record LONGER than the table is the same disagreement from
        // the other side — the walk it describes never existed.
        //
        // On its OWN operation, whose record is the table's true prefix plus
        // one more word. That shape matters: every word that has a table
        // position matches it, so the word comparison can never refuse this
        // record — only the length gate can, and without that gate the walk
        // would index past the table's end. Reusing the lied-to operation
        // above would hand the refusal to the word comparison at position
        // one, and the length gate would stand unwitnessed.
        let settle_permit = match factory
            .journal
            .prepare(
                &factory.authority,
                2,
                &snapshot(3, "reconcile"),
                &request,
                32,
            )
            .expect("reconcile to settle the lied-to operation")
        {
            BeginEffect::Reconcile(permit) => permit,
            other => panic!("an in-flight operation reconciles, got {other:?}"),
        };
        factory
            .journal
            .settle(
                &factory.authority,
                settle_permit,
                1,
                &snapshot(2, "settled"),
                EffectSettlement::Applied {
                    result_digest: test_digest(0x93),
                    settled_at_ms: 33,
                },
            )
            .expect("settle the lied-to operation out of the way");
        let fresh_request = EffectRequest::new(
            "main-ledger",
            test_digest(0x54),
            test_digest(0x72),
            draft_binding.clone(),
            draft_epoch.clone(),
            HostEffectKind::Respawn,
        )
        .expect("a fresh effect request");
        let BeginEffect::Execute(overlong_permit) = factory
            .journal
            .prepare(
                &factory.authority,
                2,
                &snapshot(3, "fresh-walk"),
                &fresh_request,
                40,
            )
            .expect("prepare the overlong walk")
        else {
            panic!("a settled ledger takes a fresh effect");
        };
        for (at, word) in ["pane_cut", "seat_settled", "seat_repointed", "extra"]
            .iter()
            .enumerate()
        {
            factory
                .journal
                .step(
                    &factory.authority,
                    &overlong_permit,
                    u32::try_from(at).expect("small ordinal"),
                    word,
                    41 + at as i64,
                )
                .expect("a durable word of the overlong walk");
        }
        let refused = *LifecycleLease::recover(
            LifecycleKind::Respawn,
            overlong_permit,
            &factory.journal,
            &factory.authority,
        )
        .expect_err("a record longer than the table must not recover");
        assert_eq!(refused.1, LifecycleLaneError::RecordDisagrees);
        refused
            .0
            .abandon("fixture: this test wanted the refusal, not the row");
        recovered.abandon("fixture: this test wanted the row read, not the row moved");
    }

    /// The draft holds the same two digests its permit will come back
    /// hiding — so the draft's Debug must hide them too, and the lease it
    /// becomes must stay hidden.
    #[test]
    fn lifecycle_debug_discloses_no_digest() {
        let team = open_team(0xc4);
        let factory = PermitFactory::new();
        let draft = LifecycleDraft::capture(LifecycleKind::Close, &team, "%1", test_digest(0xad))
            .expect("close draft");
        let binding_digest = draft.binding_digest().to_string();
        let host_epoch = draft.host_epoch().to_string();
        let draft_debug = format!("{draft:?}");
        let permit = factory.permit(&draft, LifecycleKind::Close, 0x54);
        let operation = permit.operation_id().to_string();
        let lane = draft.finalize(permit).expect("lane");
        let debug = format!("{draft_debug} {lane:?}");
        for private in [
            binding_digest.as_str(),
            host_epoch.as_str(),
            operation.as_str(),
        ] {
            assert!(!debug.contains(private), "Debug disclosed `{private}`");
        }
        lane.abandon("fixture: this test wanted the row read, not the row moved");
    }
}
