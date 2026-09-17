//! One process-scoped, allowlisted observation shared by settings and the board.
use super::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const MAX_CAPABILITY_BYTES: usize = 32 * 1024;
const MAX_TRANSITIONS: usize = 16;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const ADOPTION_RETRY_DELAY: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Reason {
    Discovering,
    Verified,
    MethodNotFound,
    DiscoveryMissing,
    DiscoveryInvalid,
    DiscoveryStale,
    ConnectRefused,
    AuthRejected,
    InfoInvalid,
    CapabilitiesInvalid,
    CapabilitiesTimeout,
    ProtocolUnsupported,
    OwnerConflict,
    SessionMismatch,
    ProcessReplaced,
    Disconnected,
    /// The receipt is this process's own and says its exact launch contract
    /// refused the selection (t-2773): connected, current, and not a pane
    /// the window may deliver a briefing to.
    LaunchRefused,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    device: u64,
}

fn file_identity(path: &str) -> Option<FileIdentity> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(FileIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        inode: {
            use std::os::unix::fs::MetadataExt;
            metadata.ino()
        },
        #[cfg(unix)]
        device: {
            use std::os::unix::fs::MetadataExt;
            metadata.dev()
        },
    })
}

#[derive(Clone)]
pub(crate) struct LaunchObservation {
    path: String,
    file: Option<FileIdentity>,
    deadline: Instant,
    configured_mode: Value,
}

impl LaunchObservation {
    pub(crate) fn capture(path: &Path, timeout: Duration, env: &[(String, String)]) -> Self {
        let path = path.to_string_lossy().to_string();
        let value = |key: &str| {
            env.iter()
                .rev()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        let team_binding = [
            "TMUX",
            "ZEROCODE_AGENT_TEAM_PANE",
            "ZEROCODE_AGENT_TEAM_ID",
            "ZEROCODE_AGENT_TEAM_TOKEN",
        ]
        .iter()
        .all(|key| value(key).is_some_and(|v| !v.is_empty()));
        let configured_mode = json!({"requested":value("ZO_SUBAGENT_MODE").filter(|mode| ["auto","inline","panes"].contains(mode)),
            "team_binding": team_binding, "source":"window-launch"});
        Self {
            configured_mode,
            file: file_identity(&path),
            path,
            deadline: Instant::now() + timeout,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Receipt {
    pub protocol: Value,
    pub revision: u64,
    pub process: Value,
    pub identity: Value,
    pub launch: Value,
    pub support: Value,
    pub mode: Value,
    pub current: Value,
}

/// Only known public leaves cross into window state. Unknown fields, raw errors,
/// auth/account/activity objects and free-form provenance are never retained.
fn public_object(value: &Value, fields: &[&str]) -> Value {
    let mut out = serde_json::Map::new();
    for field in fields {
        let value = &value[*field];
        let safe = match value {
            Value::String(s) => {
                s.len() <= 4096 && !s.chars().any(char::is_control) && !s.contains("://")
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => true,
            _ => false,
        };
        out.insert(
            (*field).into(),
            if safe { value.clone() } else { Value::Null },
        );
    }
    Value::Object(out)
}

fn selection(value: &Value) -> Value {
    public_object(
        value,
        &[
            "agent",
            "provider",
            "model",
            "effort",
            "wire_model",
            "wire_effort",
            "speed_tier",
        ],
    )
}

/// The exact launch contract's receipt (t-2773), allowlisted: a verdict word,
/// a reason code, and the two selections. Anything else zo or a future zo puts
/// there stays behind.
fn contract(value: &Value) -> Value {
    if !value.is_object() {
        return Value::Null;
    }
    let mut out = public_object(value, &["version", "request_id"]);
    out["verdict"] = value["verdict"]
        .as_str()
        .filter(|verdict| ["accepted", "refused"].contains(verdict))
        .map_or(Value::Null, |verdict| json!(verdict));
    out["reason"] = value["reason"]
        .as_str()
        .filter(|reason| {
            !reason.is_empty()
                && reason.len() <= 64
                && reason.chars().all(|c| c.is_ascii_lowercase() || c == '-')
        })
        .map_or(Value::Null, |reason| json!(reason));
    out["requested"] = selection(&value["requested"]);
    out["effective"] = if value["effective"].is_object() {
        selection(&value["effective"])
    } else {
        Value::Null
    };
    out
}

/// The one sentence a refused contract reads as, from safe fields only.
pub(crate) fn refusal_sentence(receipt: &Receipt) -> Option<String> {
    let contract = &receipt.launch["contract"];
    if contract["verdict"] != "refused" {
        return None;
    }
    let reason = contract["reason"].as_str().unwrap_or("unknown-reason");
    let model = contract["requested"]["model"].as_str().unwrap_or("-");
    let effort = contract["requested"]["effort"].as_str().unwrap_or("-");
    Some(format!(
        "zo refused its exact launch contract ({reason}): requested model {model}, effort {effort}; the briefing was withheld"
    ))
}

fn verdict_reason(receipt: &Receipt) -> Reason {
    if receipt.launch["contract"]["verdict"] == "refused" {
        Reason::LaunchRefused
    } else {
        Reason::Verified
    }
}

fn catalog(value: &Value) -> Value {
    let mut out = public_object(value, &["revision"]);
    for (field, allowed) in [
        (
            "source",
            &["export", "overlay", "discovered", "shipped", "unknown"][..],
        ),
        ("freshness", &["fresh", "stale"][..]),
        (
            "facts_provenance",
            &["declared", "mixed", "inherited", "unknown"][..],
        ),
    ] {
        out[field] = value[field]
            .as_str()
            .filter(|s| allowed.contains(s))
            .map_or(Value::Null, |s| json!(s));
    }
    let mut model = public_object(&value["model"], &["id", "family", "display_name", "wire"]);
    for field in ["effort_levels", "speed_tiers", "capabilities"] {
        model[field] = json!(
            value["model"][field]
                .as_array()
                .into_iter()
                .flatten()
                .take(32)
                .filter_map(Value::as_str)
                .filter(|s| s.len() <= 128
                    && s.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "_-./".contains(c)))
                .collect::<Vec<_>>()
        );
    }
    out["model"] = model;
    out
}

fn parse_receipt(value: Value) -> Result<Receipt, Reason> {
    if value.to_string().len() > MAX_CAPABILITY_BYTES {
        return Err(Reason::CapabilitiesInvalid);
    }
    if value["protocol"]["name"] != "zo-events"
        || value["protocol"]["major"].as_u64().is_none()
        || value["protocol"]["minor"].as_u64().is_none()
    {
        return Err(Reason::CapabilitiesInvalid);
    }
    if value["protocol"]["major"] != 1 {
        return Err(Reason::ProtocolUnsupported);
    }
    let revision = value["revision"]
        .as_u64()
        .ok_or(Reason::CapabilitiesInvalid)?;
    for identifier in [
        &value["process"]["instance_id"],
        &value["identity"]["channel_session_id"],
        &value["identity"]["session_id"],
    ] {
        if identifier.as_str().is_none_or(|s| {
            s.is_empty() || s.len() > 256 || s.chars().any(char::is_control) || s.contains("://")
        }) {
            return Err(Reason::CapabilitiesInvalid);
        }
    }
    if value["process"]["pid"]
        .as_u64()
        .is_none_or(|n| n == 0 || n > u64::from(u32::MAX))
        || value["identity"]["channel_session_id"]
            .as_str()
            .is_none_or(str::is_empty)
        || value["identity"]["session_id"]
            .as_str()
            .is_none_or(str::is_empty)
    {
        return Err(Reason::CapabilitiesInvalid);
    }
    let mut process = public_object(&value["process"], &["instance_id", "pid", "executable"]);
    process["build"] = public_object(
        &value["process"]["build"],
        &["version", "id", "git_sha", "dirty"],
    );
    let mut identity = public_object(
        &value["identity"],
        &[
            "channel_session_id",
            "session_id",
            "parent_session_id",
            "agent_id",
        ],
    );
    identity["pane"] = public_object(&value["identity"]["pane"], &["namespace", "id"]);
    let mut launch = public_object(&value["launch"], &["request_id", "policy"]);
    for field in ["requested", "resolved", "effective"] {
        launch[field] = selection(&value["launch"][field]);
    }
    launch["origins"] = public_object(&value["launch"]["origins"], &["model", "effort"]);
    launch["validation"] = public_object(&value["launch"]["validation"], &["state", "reason"]);
    launch["catalog"] = catalog(&value["launch"]["catalog"]);
    launch["contract"] = contract(&value["launch"]["contract"]);
    let mut support = json!({});
    for field in [
        "roster",
        "steer",
        "resume",
        "subagent_panes",
        "subagent_steer",
        "subagent_resume",
        "exact_launch",
    ] {
        support[field] = public_object(
            &value["support"][field],
            &[
                "supported",
                "transport",
                "method",
                "scope",
                "selector",
                "continuation",
                "registry_ordering",
                "version",
            ],
        );
    }
    let current = if value["current"].is_object() {
        json!({"revision":value["current"]["revision"].as_u64(), "effective":selection(&value["current"]["effective"]), "catalog":catalog(&value["current"]["catalog"])})
    } else {
        Value::Null
    };
    Ok(Receipt {
        protocol: public_object(&value["protocol"], &["name", "major", "minor"]),
        revision,
        process,
        identity,
        launch,
        support,
        current,
        mode: public_object(
            &value["mode"],
            &["frontend", "requested", "effective", "available", "reason"],
        ),
    })
}

#[derive(Clone, Serialize)]
pub(crate) struct Transition {
    reason: Reason,
    at: i64,
}

#[derive(Clone, Serialize)]
pub(crate) struct Integration {
    pub pane: String,
    pub session: Option<String>,
    pub reason: Reason,
    pub stale: bool,
    pub updated_at: i64,
    pub receipt: Option<Receipt>,
    pub launch_path: Option<String>,
    pub configured_mode: Value,
    pub source_revision: Option<String>,
    pub source_differs: Option<bool>,
    pub installed_changed: Option<bool>,
    /// A briefing the window withheld from this pane because its contract was
    /// refused: `{refused, reason, at}` once it happened, null before.
    pub delivery: Value,
    pub transitions: VecDeque<Transition>,
    #[serde(skip)]
    epoch: u64,
    #[serde(skip)]
    pid: Option<u32>,
    #[serde(skip)]
    file: Option<FileIdentity>,
    #[serde(skip)]
    deadline: Option<Instant>,
    #[serde(skip)]
    retry_after: Option<Instant>,
}

impl Integration {
    fn new(pane: String, epoch: u64, pid: Option<u32>) -> Self {
        Self {
            pane,
            session: None,
            reason: Reason::Discovering,
            stale: false,
            updated_at: 0,
            receipt: None,
            launch_path: None,
            configured_mode: Value::Null,
            source_revision: None,
            source_differs: None,
            installed_changed: None,
            delivery: Value::Null,
            transitions: VecDeque::new(),
            epoch,
            pid,
            file: None,
            deadline: None,
            retry_after: None,
        }
    }
    fn transition(&mut self, reason: Reason, now: i64) -> bool {
        if self.reason == reason && !self.transitions.is_empty() {
            if self.retry_after.is_some() {
                self.retry_after = Some(Instant::now() + ADOPTION_RETRY_DELAY);
            }
            return false;
        }
        self.reason = reason;
        self.retry_after = if matches!(
            reason,
            Reason::Verified
                | Reason::LaunchRefused
                | Reason::Discovering
                | Reason::MethodNotFound
                | Reason::DiscoveryMissing
                | Reason::DiscoveryInvalid
                | Reason::DiscoveryStale
        ) {
            None
        } else {
            Some(Instant::now() + ADOPTION_RETRY_DELAY)
        };
        // A refused contract is this process's own current receipt, not an
        // observation that has gone stale.
        self.stale =
            self.receipt.is_some() && !matches!(reason, Reason::Verified | Reason::LaunchRefused);
        self.updated_at = now;
        if self.transitions.len() == MAX_TRANSITIONS {
            self.transitions.pop_front();
        }
        self.transitions.push_back(Transition { reason, at: now });
        true
    }
    fn accept(&mut self, epoch: u64, result: Result<Receipt, Reason>, now: i64) -> bool {
        if self.epoch != epoch {
            return false;
        }
        let receipt = match result {
            Ok(r) => r,
            Err(reason) => return self.transition(reason, now),
        };
        if self.session.as_deref() != receipt.identity["channel_session_id"].as_str() {
            return self.transition(Reason::SessionMismatch, now);
        }
        if receipt.identity["pane"]["namespace"] == "zerocode"
            && receipt.identity["pane"]["id"]
                .as_str()
                .is_some_and(|id| id != self.pane)
        {
            return self.transition(Reason::OwnerConflict, now);
        }
        if self
            .pid
            .is_some_and(|pid| Some(u64::from(pid)) != receipt.process["pid"].as_u64())
        {
            return self.transition(Reason::ProcessReplaced, now);
        }
        let verdict = verdict_reason(&receipt);
        if let Some(previous) = &self.receipt {
            if previous.process["instance_id"] != receipt.process["instance_id"] {
                return self.transition(Reason::ProcessReplaced, now);
            }
            if receipt.revision < previous.revision {
                return false;
            }
            if receipt.revision == previous.revision {
                return self.transition(verdict, now);
            }
        }
        self.pid = receipt.process["pid"].as_u64().map(|pid| pid as u32);
        crate::crash::note_zo_build(&receipt.process["build"]);
        self.receipt = Some(receipt);
        self.transition(verdict, now);
        self.updated_at = now;
        true
    }
    fn drift(&mut self, installed: Option<FileIdentity>) {
        self.installed_changed = self
            .file
            .as_ref()
            .zip(installed.as_ref())
            .map(|(old, new)| old != new);
        self.source_differs = self
            .source_revision
            .as_deref()
            .zip(
                self.receipt
                    .as_ref()
                    .and_then(|r| r.process["build"]["git_sha"].as_str()),
            )
            .map(|(source, build)| source != build);
    }
    fn exported(&self, names: &mut HashMap<String, String>) -> Value {
        let mut out = serde_json::to_value(self).unwrap_or(Value::Null);
        out["launch_path"] = Value::Null;
        if let Some(receipt) = out.get_mut("receipt").filter(|r| r.is_object()) {
            receipt["process"]["executable"] = Value::Null;
        }
        // Equality survives within one export; durable identifiers never leave.
        for (path, namespace) in [
            ("/pane", "pane"),
            ("/session", "session"),
            ("/receipt/process/instance_id", "process"),
            ("/receipt/identity/channel_session_id", "session"),
            ("/receipt/identity/session_id", "session"),
            ("/receipt/identity/parent_session_id", "session"),
            ("/receipt/identity/agent_id", "agent"),
            ("/receipt/identity/pane/id", "pane"),
            ("/receipt/launch/request_id", "request"),
        ] {
            if let Some(value) = out.pointer_mut(path)
                && let Some(id) = value.as_str()
            {
                let next = format!("id-{}", names.len() + 1);
                *value = json!(names.entry(format!("{namespace}:{id}")).or_insert(next));
            }
        }
        if let Some(receipt) = out.get_mut("receipt").filter(|r| r.is_object()) {
            receipt["process"]["pid"] = Value::Null;
        }
        out
    }
}

#[derive(Default)]
pub(crate) struct Integrations {
    next: u64,
    records: HashMap<String, Integration>,
}

fn owner_key(owner: ZoChannelOwner) -> String {
    match owner {
        ZoChannelOwner::Term(id) => format!("term-{id}"),
        ZoChannelOwner::Lane(id) => id.to_string(),
    }
}

fn records(state: &AppState) -> std::sync::MutexGuard<'_, Integrations> {
    state
        .shell_runtime()
        .zo_integrations
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The build of every zo pane whose handshake receipt is this process's
/// own (not stale) — `build.git_sha` with `build.version` beside it —
/// distinct by sha, in record order. What the update notice
/// (`update_runtime::update_notice`) compares against the sha the release
/// lane installed: a pane still running the binary the lane just replaced
/// is the one 「새 zo 판부터 새 버전」 is about, and its version is what
/// tells 「zo {{version}} 설치됨」 from 「새 zo 빌드({{sha}})」 (t-3237).
pub(crate) fn running_zo_builds(state: &AppState) -> Vec<update_runtime::RunningZo> {
    let held = records(state);
    let mut builds: Vec<update_runtime::RunningZo> = Vec::new();
    for record in held.records.values().filter(|record| !record.stale) {
        let Some(build) = record
            .receipt
            .as_ref()
            .map(|receipt| &receipt.process["build"])
        else {
            continue;
        };
        let Some(sha) = build["git_sha"].as_str().filter(|sha| !sha.is_empty()) else {
            continue;
        };
        if !builds.iter().any(|held| held.sha == sha) {
            builds.push(update_runtime::RunningZo {
                sha: sha.to_string(),
                version: build["version"]
                    .as_str()
                    .filter(|version| !version.is_empty())
                    .map(str::to_string),
            });
        }
    }
    builds
}

pub(crate) fn begin(
    state: &AppState,
    owner: ZoChannelOwner,
    session: &str,
    launch: Option<LaunchObservation>,
) -> u64 {
    let mut held = records(state);
    held.next += 1;
    let epoch = held.next;
    let key = owner_key(owner);
    let previous = held.records.remove(&key);
    let mut record = if launch.is_none() {
        previous
            .filter(|r| r.session.as_deref().is_none_or(|id| id == session))
            .unwrap_or_else(|| Integration::new(key.clone(), epoch, None))
    } else {
        Integration::new(key.clone(), epoch, None)
    };
    record.epoch = epoch;
    record.session = Some(session.into());
    if let Some(launch) = launch {
        record.configured_mode = launch.configured_mode;
        record.launch_path = Some(launch.path);
        record.file = launch.file;
        record.deadline = Some(launch.deadline);
    }
    record.transition(Reason::Discovering, now_epoch_ms());
    held.records.insert(key, record);
    epoch
}

fn announce(app: &AppHandle, record: &Integration) {
    let _ = app.emit("zo:integration", record);
}

pub(crate) fn discovery(app: &AppHandle, term: TermId, pid: u32, reason: Reason) {
    let state = app.state::<AppState>();
    let mut held = records(&state);
    let key = owner_key(ZoChannelOwner::Term(term));
    let record = held
        .records
        .entry(key.clone())
        .or_insert_with(|| Integration::new(key, 0, Some(pid)));
    if record.pid != Some(pid) {
        *record = Integration::new(record.pane.clone(), record.epoch + 1, Some(pid));
    }
    if reason != Reason::Discovering && record.transition(reason, now_epoch_ms()) {
        announce(app, record);
    }
}

pub(crate) fn retry_due(state: &AppState, term: TermId, pid: u32) -> bool {
    records(state)
        .records
        .get(&owner_key(ZoChannelOwner::Term(term)))
        .is_none_or(|r| {
            r.pid != Some(pid) || r.retry_after.is_none_or(|when| Instant::now() >= when)
        })
}

pub(crate) fn prune_owner(state: &AppState, owner: ZoChannelOwner) {
    records(state).records.remove(&owner_key(owner));
}

pub(crate) fn note(
    app: &AppHandle,
    owner: ZoChannelOwner,
    epoch: u64,
    result: Result<Receipt, Reason>,
) {
    let state = app.state::<AppState>();
    if let Some(record) = records(&state).records.get_mut(&owner_key(owner))
        && record.accept(epoch, result, now_epoch_ms())
    {
        if record.file.is_none() && record.launch_path.is_none() {
            // A manually started binary can differ from PATH at first attach.
            // Anchor installation changes to the installation observed now,
            // rather than calling that pre-existing difference a new install.
            record.file = zerocode_pty::ZoBinary::discover()
                .and_then(|binary| file_identity(&binary.path.to_string_lossy()));
        }
        announce(app, record);
    }
}

/// Whether the pane at `term` refuses a ledger briefing, and why (t-2773).
///
/// Read by both roads that type a briefing into a zo pane — the worker's held
/// launch briefing and a `dispatch --inject` — before they write. The first
/// time a pane refuses, the record says so (settings and board redraw from the
/// same event), the black box gets one line, and the sentence is fed to the
/// pane's own grid so the person reading it sees why nothing was typed. Later
/// asks answer the same sentence without another word anywhere.
pub(crate) fn delivery_refusal(app: &AppHandle, term: TermId) -> Option<String> {
    let state = app.state::<AppState>();
    let (sentence, first, record) = {
        let mut held = records(&state);
        let record = held
            .records
            .get_mut(&owner_key(ZoChannelOwner::Term(term)))?;
        let receipt = record.receipt.as_ref()?;
        let sentence = refusal_sentence(receipt)?;
        let first = record.delivery.is_null();
        if first {
            record.delivery = json!({
                "refused": true,
                "reason": receipt.launch["contract"]["reason"].clone(),
                "at": now_epoch_ms(),
            });
        }
        (sentence, first, first.then(|| record.clone()))
    };
    if let Some(record) = record.filter(|_| first) {
        announce(app, &record);
        note_window_event(state.local_data_root(), &format!("term {term}: {sentence}"));
        if let Some(held) = state.terminals().handle(term) {
            let mut pty = lock_pty(&held);
            pty.terminal_mut()
                .feed(format!("\r\n{sentence}\r\n").as_bytes());
        }
        state.cadence().wake();
    }
    Some(sentence)
}

pub(crate) fn frame(app: &AppHandle, owner: ZoChannelOwner, epoch: u64, frame: &Value) {
    if frame["type"] == "session_capabilities" {
        note(app, owner, epoch, parse_receipt(frame.clone()));
    }
}

pub(crate) fn is_current(state: &AppState, owner: ZoChannelOwner, epoch: u64) -> bool {
    records(state)
        .records
        .get(&owner_key(owner))
        .is_some_and(|r| r.epoch == epoch)
}

pub(crate) fn adoption_reason(app: &AppHandle, term: TermId, pid: u32, reason: Reason) {
    if app
        .state::<AppState>()
        .zo_adoptions()
        .get(&term)
        .is_some_and(|held| held.pid == pid)
    {
        discovery(app, term, pid, reason);
    }
}

pub(crate) fn disconnected(app: &AppHandle, owner: ZoChannelOwner, epoch: u64) {
    note(app, owner, epoch, Err(Reason::Disconnected));
}

fn rpc_reason(error: zerocode_harness::HarnessError) -> Reason {
    use zerocode_harness::{HarnessError, ServeErrorKind};
    match error {
        HarnessError::Rpc {
            kind: ServeErrorKind::MethodNotFound,
            ..
        } => Reason::MethodNotFound,
        HarnessError::Rpc {
            kind: ServeErrorKind::Unauthorized,
            ..
        } => Reason::AuthRejected,
        HarnessError::Io(_) | HarnessError::Closed => Reason::ConnectRefused,
        _ => Reason::CapabilitiesInvalid,
    }
}

fn handshake_budget(deadline: Option<Instant>, now: Instant) -> Duration {
    deadline.map_or(HANDSHAKE_TIMEOUT, |d| {
        d.saturating_duration_since(now).min(HANDSHAKE_TIMEOUT)
    })
}

pub(crate) async fn negotiate(
    app: &AppHandle,
    owner: ZoChannelOwner,
    epoch: u64,
    addr: &str,
    token: Option<String>,
) {
    let deadline = records(&app.state::<AppState>())
        .records
        .get(&owner_key(owner))
        .and_then(|r| r.deadline);
    let budget = handshake_budget(deadline, Instant::now());
    let result = tokio::time::timeout(budget, async {
        let mut client = Client::connect(addr, token).await.map_err(rpc_reason)?;
        let value = client
            .call("session.capabilities", json!({}))
            .await
            .map_err(rpc_reason)?;
        parse_receipt(value)
    })
    .await
    .unwrap_or(Err(Reason::CapabilitiesTimeout));
    note(app, owner, epoch, result);
}

/// Reading details is the only installed-file/source comparison trigger. No poller.
#[tauri::command]
pub(crate) fn zo_integration_details(
    state: State<'_, AppState>,
    pane: Option<String>,
    source_revision: Option<String>,
    export: Option<bool>,
) -> Result<Value, String> {
    let _crumb = crate::crumbs::Command::enter("zo_integration_details");
    let set_source = source_revision.is_some();
    let source = source_revision
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase());
    if source
        .as_ref()
        .is_some_and(|s| s.len() != 40 || !s.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return Err("source revision must be a full Git SHA".into());
    }
    let installed_path =
        zerocode_pty::ZoBinary::discover().map(|binary| binary.path.to_string_lossy().into_owned());
    let installed_file = installed_path.as_deref().and_then(file_identity);
    let mut held = records(&state);
    let mut result = Vec::new();
    let mut export_names = HashMap::new();
    for record in held
        .records
        .values_mut()
        .filter(|r| pane.as_ref().is_none_or(|p| *p == r.pane))
    {
        if set_source {
            record.source_revision.clone_from(&source);
        }
        record.drift(installed_file.clone());
        result.push(if export == Some(true) {
            record.exported(&mut export_names)
        } else {
            json!(record)
        });
    }
    result.sort_by(|a, b| a["pane"].as_str().cmp(&b["pane"].as_str()));
    Ok(
        json!({"records":result, "installed":installed_path.is_some(),
        "installed_path":if export == Some(true) { None } else { installed_path },
        "window":{"version":env!("CARGO_PKG_VERSION"),"git_sha":env!("ZEROCODE_COMMIT"),"ui_digest":env!("ZEROCODE_UI_DIGEST")}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        json!({"protocol":{"name":"zo-events","major":1,"minor":0},"revision":1,
            "process":{"instance_id":"process-a","pid":42,"executable":"/private/home/zo","build":{"version":"1","git_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","id":"build-a","dirty":false}},
            "identity":{"channel_session_id":"session-a","session_id":"session-a","pane":{"namespace":"zerocode","id":"term-1"}},
            "launch":{"policy":"legacy","requested":{"model":"fixture-model","effort":"high"},"effective":{"model":"fixture-model","effort":"high"}},
            "mode":{"requested":"panes","effective":"panes","available":false,"reason":"missing-team-binding"}})
    }
    fn record() -> Integration {
        let mut record = Integration::new("term-1".into(), 3, Some(42));
        record.session = Some("session-a".into());
        record
    }

    #[test]
    fn handshake_uses_only_the_remaining_readiness_budget() {
        let now = Instant::now();
        assert_eq!(handshake_budget(None, now), Duration::from_secs(2));
        assert_eq!(
            handshake_budget(Some(now + Duration::from_secs(9)), now),
            Duration::from_secs(2)
        );
        assert_eq!(
            handshake_budget(Some(now + Duration::from_millis(50)), now),
            Duration::from_millis(50)
        );
        assert_eq!(handshake_budget(Some(now), now), Duration::ZERO);
    }

    #[test]
    fn an_address_file_still_being_written_does_not_delay_startup() {
        let mut record = record();
        record.transition(Reason::DiscoveryMissing, 1);
        assert!(record.retry_after.is_none());
        record.transition(Reason::DiscoveryInvalid, 2);
        assert!(record.retry_after.is_none());
        record.transition(Reason::ConnectRefused, 3);
        assert!(record.retry_after.is_some());
    }

    #[test]
    fn host_mode_observation_copies_presence_without_copying_team_secrets() {
        let env = [
            "TMUX",
            "ZEROCODE_AGENT_TEAM_PANE",
            "ZEROCODE_AGENT_TEAM_ID",
            "ZEROCODE_AGENT_TEAM_TOKEN",
        ]
        .map(|key| (key.to_string(), "CANARY-SECRET".to_string()));
        let observation =
            LaunchObservation::capture(Path::new("/missing/zo"), Duration::ZERO, &env);
        assert_eq!(observation.configured_mode["team_binding"], true);
        assert!(observation.configured_mode["requested"].is_null());
        assert!(!observation.configured_mode.to_string().contains("CANARY"));
    }

    #[test]
    fn receipt_dedupes_reconnect_and_refuses_old_owner_or_process() {
        let mut record = record();
        let first = parse_receipt(fixture()).unwrap();
        assert!(record.accept(3, Ok(first.clone()), 10));
        assert!(!record.accept(3, Ok(first.clone()), 11));
        assert!(!record.accept(2, Err(Reason::Disconnected), 12));
        let mut replacement = fixture();
        replacement["process"]["instance_id"] = json!("replacement");
        record.accept(3, parse_receipt(replacement), 13);
        assert_eq!(record.reason, Reason::ProcessReplaced);
        assert_eq!(record.receipt, Some(first));
        assert!(record.stale);
    }

    #[test]
    fn protocol_unknown_fields_are_ignored_but_major_schema_and_size_are_distinct() {
        let mut next = fixture();
        next["protocol"]["minor"] = json!(99);
        next["future"] = json!({"secret":"CANARY"});
        assert!(
            !serde_json::to_string(&parse_receipt(next).unwrap())
                .unwrap()
                .contains("CANARY")
        );
        let mut major = fixture();
        major["protocol"]["major"] = json!(2);
        assert_eq!(parse_receipt(major), Err(Reason::ProtocolUnsupported));
        let mut invalid = fixture();
        invalid["process"]["instance_id"] = Value::Null;
        assert_eq!(parse_receipt(invalid), Err(Reason::CapabilitiesInvalid));
        let mut unsafe_id = fixture();
        unsafe_id["identity"]["session_id"] = json!("https://CANARY");
        assert_eq!(parse_receipt(unsafe_id), Err(Reason::CapabilitiesInvalid));
        let mut huge = fixture();
        huge["unknown"] = json!("x".repeat(MAX_CAPABILITY_BYTES));
        assert_eq!(parse_receipt(huge), Err(Reason::CapabilitiesInvalid));
    }

    #[test]
    fn method_not_found_is_the_only_legacy_classification_and_errors_are_not_copied() {
        use zerocode_harness::{HarnessError, ServeErrorKind};
        for (kind, expected) in [
            (ServeErrorKind::MethodNotFound, Reason::MethodNotFound),
            (ServeErrorKind::Unauthorized, Reason::AuthRejected),
            (ServeErrorKind::Internal, Reason::CapabilitiesInvalid),
        ] {
            assert_eq!(
                rpc_reason(HarnessError::Rpc {
                    method: "session.capabilities".into(),
                    code: 0,
                    kind,
                    message: "CANARY private prompt token".into()
                }),
                expected
            );
        }
    }

    #[test]
    fn export_drops_paths_private_channels_and_preserves_identifier_equality() {
        let mut value = fixture();
        for field in [
            "token", "prompt", "account", "activity", "auth", "env", "argv",
        ] {
            value[field] = json!("CANARY");
        }
        value["launch"]["catalog"] = json!({"source":"CANARY", "notes":"CANARY", "model":{"url":"https://CANARY", "token":"CANARY"}});
        let mut record = record();
        record.launch_path = Some("/private/home/zo".into());
        record.accept(3, parse_receipt(value), 10);
        let exported = record.exported(&mut HashMap::new());
        let text = exported.to_string();
        for private in [
            "CANARY",
            "/private/home",
            "session-a",
            "process-a",
            "term-1",
        ] {
            assert!(!text.contains(private), "{private} leaked");
        }
        assert_eq!(
            exported["session"],
            exported["receipt"]["identity"]["session_id"]
        );
        assert_eq!(exported["receipt"]["mode"]["available"], false);
    }

    #[test]
    fn one_export_preserves_relationships_between_distinct_panes() {
        let mut first = record();
        first.accept(3, parse_receipt(fixture()), 10);
        first.receipt.as_mut().unwrap().process["instance_id"] = json!("session-a");
        let mut second = first.clone();
        second.pane = "term-2".into();
        second.session = Some("session-b".into());
        let receipt = second.receipt.as_mut().unwrap();
        receipt.identity["session_id"] = json!("session-b");
        receipt.identity["parent_session_id"] = json!("session-a");
        let mut names = HashMap::new();
        let a = first.exported(&mut names);
        let b = second.exported(&mut names);
        assert_ne!(a["session"], a["receipt"]["process"]["instance_id"]);
        assert_ne!(a["pane"], b["pane"]);
        assert_ne!(a["session"], b["session"]);
        assert_eq!(a["session"], b["receipt"]["identity"]["parent_session_id"]);
    }

    #[test]
    fn transitions_are_bounded_and_identical_failures_do_not_add_entries() {
        let mut record = record();
        for n in 0..100 {
            record.transition(
                if n % 2 == 0 {
                    Reason::Disconnected
                } else {
                    Reason::Discovering
                },
                n,
            );
        }
        assert_eq!(record.transitions.len(), MAX_TRANSITIONS);
        assert!(!record.transition(Reason::Discovering, 101));
        assert_eq!(record.updated_at, 99);
    }

    #[test]
    fn drift_needs_explicit_source_and_compares_file_identity_not_path_spelling() {
        let mut record = record();
        record.accept(3, parse_receipt(fixture()), 10);
        let file = FileIdentity {
            len: 1,
            modified: None,
            #[cfg(unix)]
            inode: 1,
            #[cfg(unix)]
            device: 1,
        };
        record.file = Some(file.clone());
        record.drift(Some(file.clone()));
        assert_eq!(record.source_differs, None);
        assert_eq!(record.installed_changed, Some(false));
        record.source_revision = Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into());
        let mut replaced = file;
        replaced.len = 2;
        record.drift(Some(replaced));
        assert_eq!(record.source_differs, Some(true));
        assert_eq!(record.installed_changed, Some(true));
        record.receipt.as_mut().unwrap().process["build"]["git_sha"] = Value::Null;
        record.drift(None);
        assert_eq!(record.source_differs, None);
    }

    /// t-2773: the contract's receipt crosses as an allowlisted object, a
    /// refused verdict is its own current (not stale) state with a sentence
    /// built from safe fields only, and accepted or legacy launches say
    /// nothing.
    #[test]
    fn a_refused_contract_is_its_own_current_state_and_reads_as_one_safe_sentence() {
        let mut value = fixture();
        value["launch"]["policy"] = json!("exact");
        value["launch"]["validation"] = json!({"state":"refused","reason":"unsupported-effort"});
        value["launch"]["contract"] = json!({"version":1,"request_id":"req-1","verdict":"refused",
            "reason":"unsupported-effort","secret":"CANARY",
            "requested":{"agent":"zo","model":"fixture-model","effort":"ultra","token":"CANARY"},
            "effective":null});
        let receipt = parse_receipt(value).unwrap();
        assert_eq!(receipt.launch["contract"]["verdict"], "refused");
        assert_eq!(receipt.launch["contract"]["request_id"], "req-1");
        assert!(receipt.launch["contract"]["effective"].is_null());
        assert!(!serde_json::to_string(&receipt).unwrap().contains("CANARY"));
        let sentence = refusal_sentence(&receipt).unwrap();
        assert!(
            sentence.contains("unsupported-effort")
                && sentence.contains("fixture-model")
                && sentence.contains("ultra"),
            "{sentence}"
        );
        let mut record = record();
        assert!(record.accept(3, Ok(receipt.clone()), 10));
        assert_eq!(record.reason, Reason::LaunchRefused);
        assert!(!record.stale, "the verdict is this process's own receipt");
        assert!(record.retry_after.is_none());
        assert!(
            !record.accept(3, Ok(receipt), 11),
            "the same receipt is not news"
        );

        let mut shaky = fixture();
        shaky["launch"]["contract"] = json!({"verdict":"maybe","reason":"Bad Reason!"});
        let shaky = parse_receipt(shaky).unwrap();
        assert!(shaky.launch["contract"]["verdict"].is_null());
        assert!(shaky.launch["contract"]["reason"].is_null());
        assert!(refusal_sentence(&shaky).is_none());
        let mut accepted = fixture();
        accepted["launch"]["contract"] = json!({"version":1,"verdict":"accepted","reason":null,
            "requested":{"model":"fixture-model"},"effective":{"model":"fixture-model","effort":"high"}});
        let accepted = parse_receipt(accepted).unwrap();
        assert!(refusal_sentence(&accepted).is_none());
        assert_eq!(verdict_reason(&accepted), Reason::Verified);
        let legacy = parse_receipt(fixture()).unwrap();
        assert!(legacy.launch["contract"].is_null());
        assert!(refusal_sentence(&legacy).is_none());
    }

    #[test]
    fn session_mismatch_preserves_the_last_good_receipt() {
        let mut record = record();
        record.accept(3, parse_receipt(fixture()), 10);
        let original = record.receipt.clone();
        let mut bad = fixture();
        bad["identity"]["channel_session_id"] = json!("another-session");
        record.accept(3, parse_receipt(bad), 11);
        assert_eq!(record.reason, Reason::SessionMismatch);
        assert_eq!(record.receipt, original);
    }
}
