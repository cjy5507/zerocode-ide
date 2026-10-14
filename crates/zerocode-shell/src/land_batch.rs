//! 통합 큐 (`land-batch`, t-45315) — 창의 반쪽. 아직 만들지 않았다: 시험이 먼저 빨갛게 선다.

use super::*;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::land_check::GIT_BUDGET;
use serde_json::json;
use zerocode_core::host::{Host, lock_free_git};
use zerocode_core::orchestration::land_batch::{
    Action, BatchState, DropWhy, Dropped, LandBatchAsk, LandBatchReceipt, Landed, Member,
    PressedBy, PromoteStop, Promoting, Record, StackStop, Verdict, integration_queue_supported,
    is_batch_id, operation_id,
};
use zerocode_core::orchestration::land_check::{LandCheckAsk, LandCheckReceipt};
use zerocode_orchestrator::git_publisher::{APPLIED_REF_PREFIX, CANCELLED_REF_PREFIX};

pub(crate) const PROMOTE_GIT_BUDGET: Duration = Duration::from_secs(120);

const INTEGRATION_KEEP_MS: i64 = 7 * 24 * 60 * 60 * 1000;

const POLL: Duration = Duration::from_millis(50);

const NOT_BUILT: &str = "the integration queue is not carried by this window yet";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Policy {
    pub(crate) enabled: bool,
    pub(crate) pinned_base: Option<String>,
    pub(crate) git_budget: Duration,
    pub(crate) promote_budget: Duration,
    pub(crate) git: PathBuf,
}

impl Policy {
    fn off() -> Policy {
        Policy {
            enabled: false,
            pinned_base: None,
            git_budget: GIT_BUDGET,
            promote_budget: PROMOTE_GIT_BUDGET,
            git: PathBuf::from("git"),
        }
    }
}

pub(crate) fn policy_in(_settings: &settings::SettingsRepository, _repo: &Path) -> Policy {
    Policy::off()
}

pub(crate) type Records = Arc<dyn Fn(LandBatchReceipt) -> Result<(), String> + Send + Sync>;

pub(crate) type Checks = Arc<dyn Fn(&LandCheckAsk, i64) -> Result<String, String> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct Roads {
    pub(crate) ledger: Records,
    pub(crate) check: Checks,
    pub(crate) quiet: Checks,
    pub(crate) folder: PathBuf,
}

pub(crate) fn run(
    _policy_for: &dyn Fn(&Path) -> Policy,
    _ask: &LandBatchAsk,
    _held: &[BatchState],
    _now_ms: i64,
    _roads: &Roads,
) -> Result<String, String> {
    Err(NOT_BUILT.to_string())
}

pub(crate) fn settle(
    _policy_for: &dyn Fn(&Path) -> Policy,
    _held: &[BatchState],
    _run: &str,
    _pressed: Option<&str>,
    _now_ms: i64,
    _roads: &Roads,
) {
}

fn live(_run: &str, _batch: &str) -> bool {
    false
}

pub(crate) fn stack_ref(_batch: &str, _generation: u32) -> String {
    String::new()
}

pub(crate) fn integration_ref(_batch: &str, _generation: u32) -> String {
    String::new()
}

struct StackJob {
    run: String,
    batch: String,
    generation: u32,
    members: Vec<Member>,
    messages: Vec<String>,
    base_branch: String,
    base: String,
    repo: PathBuf,
    by: PressedBy,
    policy: Policy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Built {
    steps: Vec<String>,
    tree: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Halt {
    step: u32,
    task: String,
    why: StackStop,
    files: Vec<String>,
    total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Halted {
    Stop(Halt),
    Unsettled(String),
}

fn build(_host: &Host, _job: &StackJob) -> Result<Built, Halted> {
    Err(Halted::Unsettled(NOT_BUILT.to_string()))
}

fn publish(_job: &StackJob, _built: &Built) -> Result<(), Halted> {
    Err(Halted::Unsettled(NOT_BUILT.to_string()))
}

#[cfg(test)]
mod tests;
