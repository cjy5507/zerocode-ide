//! The window's half of one checkout's evidence read.
//!
//! `zerocode_orchestrator::worktree_evidence` assembles and labels. This file
//! answers the one question that module refuses to answer for itself: WHICH
//! checkout, and by whose authority.
//!
//! Two callers, one resolution each, and neither of them trusts a path:
//!
//! - the CLI's `worktree-evidence` reaches [`for_seat`], where the checkout is
//!   the ledger's own word for a named worker's seat or the window's own
//!   record of where it put the asking pane;
//! - the window's own view reaches [`for_workspace`], which its command only
//!   calls with a checkout the catalog of projects the person actually added
//!   handed back; a path outside it is refused before this file is reached.
//!
//! Both then hand the same [`EvidenceTarget`] to the same `read`, so the JSON
//! a person sees in the window and the JSON an agent reads on stdout are the
//! same schema with the same bounds.

use std::path::{Path, PathBuf};

use zerocode_orchestrator::Orchestrator;
use zerocode_orchestrator::worktree_evidence::{
    EvidenceTarget, GitSnapshotSource, LedgerSource, WorkflowSource, WorktreeEvidenceV1,
};

use crate::orchestration;

/// Why a read could not even begin. The same `{code, message, retryable}`
/// shape the sections inside an answer use, so a caller has one refusal
/// vocabulary rather than two.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvidenceRefusal {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
    pub(crate) retryable: bool,
}

impl EvidenceRefusal {
    pub(crate) const UNRESOLVED_SEAT: Self = Self {
        code: "checkout_unresolved",
        message: "this pane is not sitting in a checkout this window placed it in",
        retryable: false,
    };

    /// The window asked about a path its own catalog does not name.
    pub(crate) const fn not_catalogued() -> Self {
        Self {
            code: "not_catalogued",
            message: "that workspace is not one of this window's projects",
            retryable: false,
        }
    }
}

/// Every refusal of the verb, in `{code, message, retryable}` and nothing else.
///
/// The refusals this verb writes itself already arrive in that shape, behind
/// the `orchestration: ` voice every ledger answer on stderr wears; they are
/// handed out bare. Anything else — a capability the actor turned away, a
/// retry name on a read, a window whose runtime never started — is a sentence
/// written for every verb, and it is carried whole under `refused`, never
/// taken apart by its words: a code guessed from prose would be a second
/// authority for what went wrong.
pub(crate) fn structured_refusal(
    mut answered: zerocode_hookd::TeamAnswer,
) -> zerocode_hookd::TeamAnswer {
    if answered.exit_code == 0 {
        return answered;
    }
    let said = answered.stderr.trim_end();
    let said = said.strip_prefix(REFUSAL_VOICE).unwrap_or(said);
    let structured = serde_json::from_str::<serde_json::Value>(said)
        .ok()
        .filter(|value| value.get("code").is_some_and(serde_json::Value::is_string))
        .map_or_else(
            || zerocode_core::orchestration::evidence_refusal("refused", said, false),
            |value| value.to_string(),
        );
    answered.stderr = format!("{structured}\n");
    answered.stdout.clear();
    answered
}

/// The voice a ledger refusal is printed in on its way to the shim.
const REFUSAL_VOICE: &str = "orchestration: ";

/// The checkout a CLI caller means, and nothing it merely asked for.
///
/// `checkout` is the ledger's own word for a `--worker`'s seat; `None` is the
/// bare form, which is about the pane that asked. A pane the window has no
/// placement record for gets a refusal rather than somebody else's tree.
pub(crate) fn for_seat(
    checkout: Option<String>,
    own: Option<PathBuf>,
    now_ms: i64,
) -> Result<WorktreeEvidenceV1, EvidenceRefusal> {
    let Some(path) = checkout.map(PathBuf::from).or(own) else {
        return Err(EvidenceRefusal::UNRESOLVED_SEAT);
    };
    Ok(read_local(&path, now_ms))
}

/// The checkout a CLICK means, once the catalog has vouched for it.
///
/// The caller does the vouching, with the same two roads `set_active_worktree`
/// uses — the catalog owners this window built, then git's own worktree list
/// for the stored projects — so a path from a webview is a request and never
/// an authority, and this function is only ever given a checkout that came
/// back out of one of those.
pub(crate) fn for_workspace(chosen: &Path, now_ms: i64) -> WorktreeEvidenceV1 {
    read_local(chosen, now_ms)
}

/// A remote checkout is named and declared unsupported rather than looked for
/// on this disk: `/workspaces/thing` on another host is a real directory HERE
/// on some machines, and reading it would be evidence about the wrong tree.
pub(crate) fn for_remote(key: &str, now_ms: i64) -> WorktreeEvidenceV1 {
    let target = EvidenceTarget::remote(key);
    let ledger = orchestration::ledger_image();
    let store = orchestration::authority_store_path();
    zerocode_orchestrator::worktree_evidence::read(
        &target,
        &GitSnapshotSource::new(None),
        &LedgerSource::new(ledger.as_deref()),
        &WorkflowSource::new(store.as_deref()),
        now_ms,
    )
}

/// Read one checkout on this disk with whatever this window actually has.
///
/// Every source is optional and its absence is an answer: a directory outside
/// any repository has no `Orchestrator`, a window whose runtime never started
/// has no ledger, and a window that has opened no authority store has no
/// receipts. None of those three is an error, and none of them is silence —
/// the assembled answer says which.
fn read_local(path: &Path, now_ms: i64) -> WorktreeEvidenceV1 {
    let target = EvidenceTarget::local(path);
    let orchestrator = Orchestrator::open(path).ok();
    /* The ledger image is an `Arc` clone taken before Git is asked. The
     * window's ledger lock is therefore held for the clone and not for the
     * seconds a `git status` over a large checkout can take. */
    let ledger = orchestration::ledger_image();
    let store = orchestration::authority_store_path();
    zerocode_orchestrator::worktree_evidence::read(
        &target,
        &GitSnapshotSource::new(orchestrator.as_ref()),
        &LedgerSource::new(ledger.as_deref()),
        &WorkflowSource::new(store.as_deref()),
        now_ms,
    )
}
