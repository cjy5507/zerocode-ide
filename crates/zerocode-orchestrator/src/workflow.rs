//! Durable workflow identities and state visible to callers.
//!
//! Paths and mutable Git handles deliberately do not appear here. A workflow
//! binds immutable object ids and ref names; the publisher is responsible for
//! resolving those names immediately before its compare-and-swap.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::handoff::{HandoffManifestV1, PublishPolicy, TestRequirement};

/// Publication refs owned by ZeroCode rather than by a checked-out branch.
pub const INTEGRATION_REF_PREFIX: &str = "refs/zerocode/integration/";

/// A Git ref and the exact object it must name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefBinding {
    pub name: String,
    pub oid: String,
}

/// Immutable facts accepted when a task is first assigned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAssignment {
    pub workflow_id: String,
    pub assignee_id: String,
    /// Opaque repository identity minted by the local snapshotter.
    pub repository_id: String,
    /// Stable worktree identity allocated to this assignment.
    pub worktree_id: String,
    /// Host/account/remote boundary the trusted publisher must echo.
    pub publisher_scope: String,
    /// The target object the worker started from.
    pub base_oid: String,
    /// The only source branch this assignment may submit.
    pub source_ref: String,
    /// The explicit ZeroCode integration ref that may move from `base_oid`.
    /// Branch promotion is a separate, worktree-aware host operation.
    pub target_ref: String,
    /// Coordinator-owned policy, fixed before an assignee submits evidence.
    pub policy: PublishPolicy,
}

/// The durable state machine. `Publishing` and `PublishUnknown` both require
/// observation before another permit can be issued after a process restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    Assigned,
    Submitted,
    Approved,
    ChangesRequested,
    Publishing,
    PublishUnknown,
    Published,
    PublishFailed,
}

impl WorkflowState {
    pub(crate) const fn as_db(self) -> &'static str {
        match self {
            Self::Assigned => "assigned",
            Self::Submitted => "submitted",
            Self::Approved => "approved",
            Self::ChangesRequested => "changes_requested",
            Self::Publishing => "publishing",
            Self::PublishUnknown => "publish_unknown",
            Self::Published => "published",
            Self::PublishFailed => "publish_failed",
        }
    }

    pub(crate) fn from_db(value: &str) -> Option<Self> {
        match value {
            "assigned" => Some(Self::Assigned),
            "submitted" => Some(Self::Submitted),
            "approved" => Some(Self::Approved),
            "changes_requested" => Some(Self::ChangesRequested),
            "publishing" => Some(Self::Publishing),
            "publish_unknown" => Some(Self::PublishUnknown),
            "published" => Some(Self::Published),
            "publish_failed" => Some(Self::PublishFailed),
            _ => None,
        }
    }
}

/// A path-free, serializable view of one workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRecord {
    pub workflow_id: String,
    /// Allocated by the store, never supplied by an agent.
    pub generation: u64,
    pub parent_generation: Option<u64>,
    pub assignee_id: String,
    pub repository_id: String,
    pub worktree_id: String,
    pub publisher_scope: String,
    pub base_oid: String,
    pub source: Option<RefBinding>,
    pub target: RefBinding,
    pub manifest_id: Option<String>,
    pub policy_digest: Option<String>,
    pub reviewer_id: Option<String>,
    pub state: WorkflowState,
    pub published_oid: Option<String>,
}

/// The review result written by an identity other than the assignee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Approve,
    RequestChanges,
}

impl ReviewDecision {
    pub(crate) const fn as_db(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::RequestChanges => "request_changes",
        }
    }

    pub(crate) fn from_db(value: &str) -> Option<Self> {
        match value {
            "approve" => Some(Self::Approve),
            "request_changes" => Some(Self::RequestChanges),
            _ => None,
        }
    }
}

/// Every fact a publisher must compare before changing the target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishExpectation {
    pub workflow_id: String,
    pub generation: u64,
    pub manifest_id: String,
    pub repository_id: String,
    pub worktree_id: String,
    pub publisher_scope: String,
    pub source: RefBinding,
    pub target: RefBinding,
    pub policy_digest: String,
}

/// A review attestation resolved by a host-owned verifier. Its binding covers
/// every immutable fact that a later publish permit will carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewReceipt {
    pub attestation_id: String,
    pub reviewer_id: String,
    pub decision: ReviewDecision,
    pub binding: PublishExpectation,
}

/// Authentication boundary for an independent review system. Implementations
/// must resolve `attestation_id` and compare the complete receipt, not merely
/// trust the caller-supplied reviewer spelling.
pub trait TrustedReviewVerifier: Send + Sync {
    fn verifies(&self, receipt: &ReviewReceipt) -> bool;
}

/// Immutable evidence retained when a correction generation replaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedGeneration {
    pub workflow_id: String,
    pub generation: u64,
    pub parent_generation: Option<u64>,
    pub manifest: HandoffManifestV1,
    pub reviewer_id: String,
    pub decision: ReviewDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredPolicy {
    required_tests: Vec<StoredTestRequirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredTestRequirement {
    name: String,
    command_id: String,
    /// Absent when false, so a policy that never asked for a baseline keeps
    /// the exact bytes — and so the exact digest — it was stored with.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    must_fail_first: bool,
}

impl StoredPolicy {
    pub(crate) fn from_policy(policy: &PublishPolicy) -> Self {
        Self {
            required_tests: policy
                .required_tests
                .iter()
                .map(|requirement| StoredTestRequirement {
                    name: requirement.name.clone(),
                    command_id: requirement.command_id.clone(),
                    must_fail_first: requirement.must_fail_first,
                })
                .collect(),
        }
    }

    pub(crate) fn into_policy(self) -> PublishPolicy {
        PublishPolicy {
            required_tests: self
                .required_tests
                .into_iter()
                .map(|requirement| TestRequirement {
                    name: requirement.name,
                    command_id: requirement.command_id,
                    must_fail_first: requirement.must_fail_first,
                })
                .collect(),
        }
    }
}

pub(crate) fn encoded_policy(
    policy: &PublishPolicy,
) -> Result<(Vec<u8>, String), serde_json::Error> {
    let bytes = serde_json::to_vec(&StoredPolicy::from_policy(policy))?;
    let mut digest = Sha256::new();
    hash_frame(&mut digest, b"workflow-publish-policy-v1", &bytes);
    Ok((bytes, format!("{:x}", digest.finalize())))
}

pub(crate) fn decoded_policy(bytes: &[u8]) -> Result<PublishPolicy, serde_json::Error> {
    serde_json::from_slice::<StoredPolicy>(bytes).map(StoredPolicy::into_policy)
}

fn hash_frame(digest: &mut Sha256, label: &[u8], value: &[u8]) {
    digest.update((label.len() as u64).to_le_bytes());
    digest.update(label);
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}

pub(crate) fn valid_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

pub(crate) fn valid_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn valid_branch_ref(value: &str) -> bool {
    valid_ref_below(value, "refs/heads/")
}

pub(crate) fn valid_publish_target_ref(value: &str) -> bool {
    valid_ref_below(value, INTEGRATION_REF_PREFIX)
}

fn valid_ref_below(value: &str, prefix: &str) -> bool {
    let Some(branch) = value.strip_prefix(prefix) else {
        return false;
    };
    if branch.is_empty()
        || value.len() > 512
        || value.ends_with('/')
        || value.ends_with('.')
        || value.contains("//")
        || value.contains("..")
        || value.contains("@{")
        || value
            .bytes()
            .any(|byte| byte <= b' ' || byte == 0x7f || b"~^:?*[\\".contains(&byte))
    {
        return false;
    }
    branch.split('/').all(|component| {
        !component.is_empty()
            && component != "."
            && !component.starts_with('.')
            && !component.ends_with(".lock")
    })
}

pub(crate) fn source_ref(branch: &str) -> Option<String> {
    if !valid_identity(branch) {
        return None;
    }
    let full = if branch.starts_with("refs/heads/") {
        branch.to_string()
    } else {
        format!("refs/heads/{branch}")
    };
    if valid_branch_ref(&full) {
        Some(full)
    } else {
        None
    }
}
