use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

#[cfg(test)]
mod original_observation_regressions {
    use super::*;

    #[test]
    fn an_agent_read_captures_its_revision_without_a_prior_revision_input() {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        for verb in ["read", "marks", "fields", "screenshot"] {
            let result = workspaces.begin_at("browser-1", Some("pane-a"), verb, None);
            assert!(
                result.is_ok(),
                "{verb} must capture the read's own revision"
            );
            if let Ok(Some(operation)) = result {
                operation.finish(true);
            }
        }
    }

    #[test]
    fn a_managed_write_requires_an_explicit_original_observation_revision() {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        workspaces
            .begin_at("browser-1", Some("pane-a"), "read", Some(1))
            .unwrap()
            .unwrap()
            .finish(true);
        for verb in ["click", "type", "fill", "goto", "close", "scroll", "find"] {
            let result = workspaces.begin_at("browser-1", Some("pane-a"), verb, None);
            assert!(
                matches!(result, Err(ref error) if error.starts_with("workspace_revision_required:")),
                "{verb}"
            );
        }
    }

    async fn switch(workspaces: &Workspaces, mode: Mode) {
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces.transition("browser-1", mode, revision).unwrap();
        transition.wait().await.unwrap();
        transition.confirm_fence().unwrap();
        transition.finish().unwrap();
    }

    #[tokio::test]
    async fn a_request_delayed_before_send_cannot_borrow_a_resumed_agents_new_read() {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        let original = workspaces
            .begin_at("browser-1", Some("pane-a"), "marks", None)
            .unwrap()
            .unwrap()
            .finish_observation()
            .unwrap();
        switch(&workspaces, Mode::Human).await;
        switch(&workspaces, Mode::Agent).await;
        let current = workspaces
            .begin_at("browser-1", Some("pane-a"), "marks", None)
            .unwrap()
            .unwrap()
            .finish_observation()
            .unwrap();
        assert_ne!(original, current);
        let delayed = workspaces.begin_at("browser-1", Some("pane-a"), "click", Some(original));
        assert!(matches!(delayed, Err(error) if error.starts_with("workspace_stale:")));
        workspaces
            .begin_at("browser-1", Some("pane-a"), "click", Some(current))
            .unwrap()
            .unwrap()
            .finish(true);
    }

    #[tokio::test]
    async fn a_read_interrupted_by_handoff_cannot_issue_a_completion_stamp() {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        let reading = workspaces
            .begin_at("browser-1", Some("pane-a"), "read", None)
            .unwrap()
            .unwrap();
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        assert!(reading.finish_observation().is_err());
        transition.wait().await.unwrap();
        transition.confirm_fence().unwrap();
        transition.finish().unwrap();
        assert!(
            workspaces
                .snapshot("browser-1")
                .unwrap()
                .unwrap()
                .needs_observation
        );
    }

    #[test]
    fn observation_completion_checks_owner_even_when_the_revision_is_unchanged() {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        let reading = workspaces
            .begin_at("browser-1", Some("pane-a"), "read", None)
            .unwrap()
            .unwrap();
        workspaces
            .get("browser-1")
            .unwrap()
            .unwrap()
            .lock()
            .unwrap()
            .owner = Some("pane-b".into());
        assert!(reading.finish_observation().is_err());
    }

    #[test]
    fn an_original_revision_does_not_authorize_another_actor_or_managed_eval() {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        let revision = workspaces
            .begin_at("browser-1", Some("pane-a"), "fields", None)
            .unwrap()
            .unwrap()
            .finish_observation()
            .unwrap();
        let other = workspaces.begin_at("browser-1", Some("pane-b"), "click", Some(revision));
        assert!(matches!(other, Err(error) if error.starts_with("workspace_owner_mismatch:")));
        let eval = workspaces.begin_at("browser-1", Some("pane-a"), "eval", Some(revision));
        assert!(matches!(eval, Err(error) if error.starts_with("workspace_eval_refused:")));
        assert!(
            workspaces
                .begin_at("ordinary", Some("pane-a"), "click", None)
                .unwrap()
                .is_none()
        );
    }
}

const WORKSPACE_CAP: usize = 128;
const FRAME_INTERVAL: Duration = Duration::from_secs(1);
type Notice = Box<dyn Fn(Snapshot) + Send + Sync>;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Mode {
    Agent,
    Pausing,
    Paused,
    Human,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub(crate) label: String,
    pub(crate) mode: Mode,
    pub(crate) revision: u64,
    pub(crate) in_flight: usize,
    pub(crate) needs_observation: bool,
    pub(crate) owner_term: Option<String>,
    pub(crate) snapshot_supported: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Dispatch {
    Completed,
    NotSent,
    Uncertain,
}

impl Dispatch {
    fn of(succeeded: bool) -> Self {
        if succeeded {
            Self::Completed
        } else {
            Self::Uncertain
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StepControl {
    label: String,
    owner: String,
    revision: u64,
}

#[derive(Clone, Default)]
pub(crate) struct EvidenceCheck(Option<(Arc<Workspace>, StepControl)>);

impl EvidenceCheck {
    pub(crate) fn holds(&self) -> bool {
        self.0.as_ref().is_none_or(|(workspace, control)| {
            workspace
                .lock()
                .is_ok_and(|state| agent_holds(&state, control))
        })
    }

    pub(crate) fn start<T>(&self, send: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        let Some((workspace, control)) = &self.0 else {
            return send();
        };
        let _sending = {
            let mut state = workspace.lock()?;
            if !agent_holds(&state, control) {
                return Err(changed());
            }
            state.capturing += 1;
            Sending(workspace)
        };
        send()
    }
}

struct Sending<'a>(&'a Workspace);

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.lock() {
            state.capturing = state.capturing.saturating_sub(1);
        }
        self.0.drained.notify_waiters();
    }
}

struct State {
    mode: Mode,
    revision: u64,
    in_flight: usize,
    capturing: usize,
    needs_observation: bool,
    owner: Option<String>,
    popup_pending: bool,
    closed: bool,
    person_controlled: bool,
}

struct Workspace {
    label: String,
    state: Mutex<State>,
    drained: Notify,
}

impl Workspace {
    fn lock(&self) -> Result<MutexGuard<'_, State>, String> {
        self.state.lock().map_err(|_| {
            "workspace_unavailable: 작업 브라우저 상태를 확인할 수 없습니다".to_string()
        })
    }

    fn snapshot(&self, state: &State) -> Snapshot {
        Snapshot {
            label: self.label.clone(),
            mode: state.mode,
            revision: state.revision,
            in_flight: state.in_flight,
            needs_observation: state.needs_observation,
            owner_term: state
                .owner
                .as_deref()
                .and_then(crate::hooks::term_of_pane_key)
                .map(|term| term.to_string()),
            snapshot_supported: cfg!(target_os = "macos"),
        }
    }
}

#[derive(Default)]
struct FrameState {
    in_flight: bool,
    started: Option<Instant>,
}

#[derive(Default)]
struct ActorState {
    in_flight: usize,
    uncertain: BTreeSet<String>,
}

#[derive(Default)]
struct ActorActivity {
    state: Mutex<ActorState>,
    drained: Notify,
}

#[derive(Default)]
pub(crate) struct Workspaces {
    held: Mutex<BTreeMap<String, Arc<Workspace>>>,
    actors: Mutex<BTreeMap<String, Arc<ActorActivity>>>,
    frames: Mutex<BTreeMap<String, Arc<Mutex<FrameState>>>>,
    closed: Mutex<BTreeMap<String, u64>>,
}

fn changed() -> String {
    "workspace_stale: 작업 브라우저 상태가 바뀌었습니다 — 새 상태를 확인하세요".to_string()
}

fn paused() -> String {
    "workspace_paused: 작업 브라우저가 일시정지 또는 직접 조작 중입니다 — 자동으로 재시도하지 마세요".to_string()
}

fn busy() -> String {
    "workspace_busy: 작업 브라우저의 이전 요청이 끝나지 않았습니다".to_string()
}

/// What an agent is told at the work browsers' door while the person's stop
/// stands (t-62872): the person stopped it, and only they start it again.
pub(crate) const PERSONS_STOPPED: &str = "workspace_person_stopped: 사람이 정지했습니다 — 사람이 띠에서 다시 시작할 때까지 작업 브라우저를 열거나 쓰지 마세요. 어디까지 했는지 알리고 기다리세요";

fn next_revision(state: &mut State) -> Result<(), String> {
    state.revision = state
        .revision
        .checked_add(1)
        .ok_or("workspace_unavailable: 제어 상태 번호가 소진됐습니다")?;
    Ok(())
}

fn owner_ready(held: &BTreeMap<String, Arc<Workspace>>, actor: &str) -> Result<(), String> {
    for workspace in held.values() {
        let state = workspace.lock()?;
        if state.owner.as_deref() == Some(actor) && state.mode != Mode::Agent {
            return Err(paused());
        }
    }
    Ok(())
}

fn agent_holds(state: &State, control: &StepControl) -> bool {
    !state.closed
        && state.mode == Mode::Agent
        && state.revision == control.revision
        && state.owner.as_deref() == Some(control.owner.as_str())
}

impl Workspaces {
    fn lock(&self) -> Result<MutexGuard<'_, BTreeMap<String, Arc<Workspace>>>, String> {
        self.held.lock().map_err(|_| {
            "workspace_unavailable: 작업 브라우저 목록을 확인할 수 없습니다".to_string()
        })
    }

    pub(crate) fn register(
        &self,
        label: &str,
        owner: Option<&str>,
        restored: bool,
    ) -> Result<Snapshot, String> {
        let mut held = self.lock()?;
        if held.contains_key(label) {
            return Err("workspace_exists: 이미 등록된 작업 브라우저입니다".to_string());
        }
        if held.len() >= WORKSPACE_CAP {
            return Err("workspace_limit: 작업 브라우저 수가 한도에 도달했습니다".to_string());
        }
        if let Some(owner) = owner {
            owner_ready(&held, owner)?;
        }
        let workspace = Arc::new(Workspace {
            label: label.to_string(),
            state: Mutex::new(State {
                mode: if restored || owner.is_none() {
                    Mode::Paused
                } else {
                    Mode::Agent
                },
                revision: 1,
                in_flight: 0,
                capturing: 0,
                needs_observation: true,
                owner: owner.map(str::to_string),
                popup_pending: false,
                closed: false,
                person_controlled: false,
            }),
            drained: Notify::new(),
        });
        let snapshot = workspace.snapshot(&*workspace.lock()?);
        self.closed
            .lock()
            .map_err(|_| "workspace_unavailable: 닫기 확인 상태를 읽을 수 없습니다")?
            .remove(label);
        held.insert(label.to_string(), workspace);
        Ok(snapshot)
    }

    fn get(&self, label: &str) -> Result<Option<Arc<Workspace>>, String> {
        Ok(self.lock()?.get(label).cloned())
    }

    pub(crate) fn snapshot(&self, label: &str) -> Result<Option<Snapshot>, String> {
        self.get(label)?
            .map(|workspace| Ok(workspace.snapshot(&*workspace.lock()?)))
            .transpose()
    }

    pub(crate) fn snapshots(&self) -> Result<Vec<Snapshot>, String> {
        self.lock()?
            .values()
            .map(|workspace| Ok(workspace.snapshot(&*workspace.lock()?)))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn allows_person(&self, label: &str) -> Result<bool, String> {
        Ok(self
            .snapshot(label)?
            .is_none_or(|snapshot| snapshot.mode == Mode::Human && snapshot.in_flight == 0))
    }

    #[cfg(test)]
    pub(crate) fn allows_agent_metadata(
        &self,
        label: &str,
        actor: Option<&str>,
    ) -> Result<bool, String> {
        let Some(workspace) = self.get(label)? else {
            return Ok(actor.is_some());
        };
        let state = workspace.lock()?;
        Ok(!state.closed
            && state.mode == Mode::Agent
            && state.owner.as_deref() == actor
            && actor.is_some())
    }

    pub(crate) fn check_actor(&self, actor: Option<&str>, verb: &str) -> Result<(), String> {
        let actor =
            actor.ok_or("workspace_actor_required: 현재 창이 보유한 에이전트 판이 필요합니다")?;
        let row = zerocode_core::agent_browser::browser_verb(verb)
            .ok_or("workspace_verb_unknown: 지원하지 않는 브라우저 명령입니다")?;
        let held = self.lock()?;
        if row.changes_page {
            owner_ready(&held, actor)?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn begin_actor(
        &self,
        actor: Option<&str>,
        verb: &str,
    ) -> Result<Option<ActorOperation>, String> {
        self.begin_actor_target(actor, verb, None)
    }

    pub(crate) fn begin_actor_target(
        &self,
        actor: Option<&str>,
        verb: &str,
        label: Option<&str>,
    ) -> Result<Option<ActorOperation>, String> {
        let actor =
            actor.ok_or("workspace_actor_required: 현재 창이 보유한 에이전트 판이 필요합니다")?;
        let row = zerocode_core::agent_browser::browser_verb(verb)
            .ok_or("workspace_verb_unknown: 지원하지 않는 브라우저 명령입니다")?;
        if !row.changes_page {
            return Ok(None);
        }
        let held = self.lock()?;
        owner_ready(&held, actor)?;
        let mut actors = self
            .actors
            .lock()
            .map_err(|_| "workspace_unavailable: 에이전트 요청 상태를 확인할 수 없습니다")?;
        let mut owners = Vec::with_capacity(held.len());
        let mut workspaces = Vec::new();
        for workspace in held.values() {
            let state = workspace.lock()?;
            if let Some(owner) = &state.owner {
                owners.push(owner.clone());
                if owner == actor {
                    workspaces.push(workspace.clone());
                }
            }
        }
        actors.retain(|actor, activity| {
            owners.contains(actor)
                || activity.state.lock().map_or(true, |state| {
                    state.in_flight != 0 || !state.uncertain.is_empty()
                })
        });
        if actors.len() >= WORKSPACE_CAP && !actors.contains_key(actor) {
            return Err("workspace_limit: 에이전트 요청 수가 한도에 도달했습니다".to_string());
        }
        let activity = actors.entry(actor.to_string()).or_default().clone();
        let mut state = activity
            .state
            .lock()
            .map_err(|_| "workspace_unavailable: 에이전트 요청 상태를 확인할 수 없습니다")?;
        if state.in_flight >= WORKSPACE_CAP
            || (state.uncertain.len() >= WORKSPACE_CAP
                && label.is_some_and(|label| !state.uncertain.contains(label)))
        {
            return Err("workspace_limit: 에이전트 요청 수가 한도에 도달했습니다".to_string());
        }
        state.in_flight += 1;
        drop(state);
        Ok(Some(ActorOperation {
            activity,
            label: label.map(str::to_string),
            workspaces,
            completed: false,
            notice: None,
        }))
    }

    #[cfg(test)]
    pub(crate) fn begin(
        &self,
        label: &str,
        actor: Option<&str>,
        verb: &str,
    ) -> Result<Option<Operation>, String> {
        let revision = self.snapshot(label)?.map(|snapshot| snapshot.revision);
        self.begin_at(label, actor, verb, revision)
    }

    pub(crate) fn begin_at(
        &self,
        label: &str,
        actor: Option<&str>,
        verb: &str,
        expected_revision: Option<u64>,
    ) -> Result<Option<Operation>, String> {
        let actor =
            actor.ok_or("workspace_actor_required: 현재 창이 보유한 에이전트 판이 필요합니다")?;
        let row = zerocode_core::agent_browser::browser_verb(verb)
            .ok_or("workspace_verb_unknown: 지원하지 않는 브라우저 명령입니다")?;
        let held = self.lock()?;
        if row.changes_page {
            owner_ready(&held, actor)?;
        }
        let Some(workspace) = held.get(label).cloned() else {
            if expected_revision.is_some() {
                return Err(changed());
            }
            return Ok(None);
        };
        let mut state = workspace.lock()?;
        if state.closed || expected_revision.is_some_and(|revision| revision != state.revision) {
            return Err(changed());
        }
        if state.owner.as_deref() != Some(actor) {
            return Err("workspace_owner_mismatch: 다른 에이전트의 작업 브라우저입니다 — 자신의 작업 탭을 여세요".to_string());
        }
        if state.mode != Mode::Agent {
            return Err(paused());
        }
        if verb == "eval" {
            return Err("workspace_eval_refused: 전용 작업 브라우저는 임의 eval을 받지 않습니다 — read, fields, fill을 사용하세요".to_string());
        }
        if state.person_controlled && zerocode_core::agent_browser::reads_history(verb) {
            return Err("workspace_history_unsupported: 사람이 조작한 작업 탭의 콘솔·요청 기록은 에이전트에게 주지 않습니다 — 새 작업 탭을 여세요".to_string());
        }
        if row.changes_page && expected_revision.is_none() {
            return Err("workspace_revision_required: 작업 브라우저 변경에는 원관측의 workspaceRevision이 필요합니다".to_string());
        }
        if state.in_flight != 0 {
            return Err(busy());
        }
        if row.changes_page && state.needs_observation {
            return Err("workspace_observation_required: 작업 브라우저의 화면을 다시 확인하세요 — 먼저 read, marks 또는 fields를 실행하세요".to_string());
        }
        state.in_flight = 1;
        let revision = state.revision;
        drop(state);
        drop(held);
        Ok(Some(Operation {
            workspace,
            revision,
            owner: Some(actor.to_string()),
            changes_page: row.changes_page,
            observes: zerocode_core::agent_browser::observes(verb),
            completed: false,
            notice: None,
        }))
    }

    /// An agent's `open`, at the door (t-62872): refused while the person's
    /// stop stands, and registered under that stop's own lock, so no work
    /// browser comes into an agent's hands after the person's stop.
    pub(crate) fn register_agents(
        &self,
        label: &str,
        owner: Option<&str>,
    ) -> Result<Snapshot, String> {
        crate::computer_use::guard::unless_persons_stop(|| self.register(label, owner, false))
            .unwrap_or_else(|| Err(PERSONS_STOPPED.to_string()))
    }

    /// An agent's verb on a browser, at the door (t-62872): on a work browser
    /// it is refused while the person's stop stands — the window's own stop,
    /// the helper never asked — and admitted under that stop's own lock, so no
    /// read or input an agent sends after the person's stop gets in while the
    /// window is still on its way to pause the browser. A browser that is no
    /// work browser is not the stop's.
    pub(crate) fn begin_agents(
        &self,
        label: &str,
        actor: Option<&str>,
        verb: &str,
        expected_revision: Option<u64>,
    ) -> Result<Option<Operation>, String> {
        if self.get(label)?.is_none() {
            return self.begin_at(label, actor, verb, expected_revision);
        }
        crate::computer_use::guard::unless_persons_stop(|| {
            self.begin_at(label, actor, verb, expected_revision)
        })
        .unwrap_or_else(|| Err(PERSONS_STOPPED.to_string()))
    }

    pub(crate) fn begin_person(
        &self,
        label: &str,
        expected_revision: Option<u64>,
    ) -> Result<Option<Operation>, String> {
        let Some(workspace) = self.get(label)? else {
            return if expected_revision.is_some() {
                Err(changed())
            } else {
                Ok(None)
            };
        };
        let mut state = workspace.lock()?;
        if state.closed || expected_revision != Some(state.revision) {
            return Err(changed());
        }
        if state.mode != Mode::Human {
            return Err(paused());
        }
        if state.in_flight != 0 {
            return Err(busy());
        }
        state.in_flight = 1;
        let revision = state.revision;
        drop(state);
        Ok(Some(Operation {
            workspace,
            revision,
            owner: None,
            changes_page: true,
            observes: false,
            completed: false,
            notice: None,
        }))
    }

    pub(crate) fn with_placement<T>(
        &self,
        label: &str,
        expected_revision: Option<u64>,
        operation: impl FnOnce(bool) -> Result<T, String>,
    ) -> Result<T, String> {
        let Some(workspace) = self.get(label)? else {
            return operation(expected_revision.is_none());
        };
        let state = workspace.lock()?;
        operation(
            !state.closed && expected_revision == Some(state.revision) && state.mode == Mode::Human,
        )
    }

    #[cfg(test)]
    pub(crate) fn transition(
        &self,
        label: &str,
        target: Mode,
        expected_revision: u64,
    ) -> Result<Transition, String> {
        self.transition_owned(label, target, expected_revision, None)
    }

    pub(crate) fn transition_owned(
        &self,
        label: &str,
        target: Mode,
        expected_revision: u64,
        owner: Option<&str>,
    ) -> Result<Transition, String> {
        if target == Mode::Pausing {
            return Err(
                "workspace_mode_invalid: 전환 중 상태를 직접 지정할 수 없습니다".to_string(),
            );
        }
        let held = self.lock()?;
        let workspace = held
            .get(label)
            .cloned()
            .ok_or("workspace_missing: 이 탭은 전용 작업 브라우저가 아닙니다")?;
        let mut state = workspace.lock()?;
        if state.closed || state.revision != expected_revision {
            return Err(changed());
        }
        if target == Mode::Agent && owner.or(state.owner.as_deref()).is_none() {
            return Err("workspace_owner_required: 맡길 에이전트 판을 선택하세요".to_string());
        }
        let activity = match state.owner.as_deref() {
            Some(owner) => self
                .actors
                .lock()
                .map_err(|_| "workspace_unavailable: 에이전트 요청 상태를 확인할 수 없습니다")?
                .get(owner)
                .cloned(),
            None => None,
        };
        next_revision(&mut state)?;
        state.mode = Mode::Pausing;
        state.needs_observation = true;
        let revision = state.revision;
        drop(state);
        drop(held);
        Ok(Transition {
            workspace,
            activity,
            owner: owner.map(str::to_string),
            revision,
            target,
            fenced: target == Mode::Paused,
            completed: false,
            notice: None,
        })
    }

    pub(crate) fn reserve_popup(
        &self,
        label: &str,
        expected_revision: u64,
    ) -> Result<bool, String> {
        let Some(workspace) = self.get(label)? else {
            return Ok(false);
        };
        let mut state = workspace.lock()?;
        if state.closed
            || state.mode != Mode::Agent
            || state.revision != expected_revision
            || state.popup_pending
        {
            return Ok(false);
        }
        state.popup_pending = true;
        Ok(true)
    }

    pub(crate) fn release_popup(&self, label: &str) -> Result<(), String> {
        if let Some(workspace) = self.get(label)? {
            workspace.lock()?.popup_pending = false;
        }
        Ok(())
    }

    pub(crate) fn with_agent_popup(
        &self,
        label: &str,
        expected_revision: u64,
        navigate: impl FnOnce() -> Result<(), String>,
    ) -> Result<bool, String> {
        let held = self.lock()?;
        let Some(workspace) = held.get(label) else {
            return Ok(false);
        };
        workspace.lock()?.popup_pending = false;
        let owner = workspace.lock()?.owner.clone();
        let Some(owner) = owner else {
            return Ok(false);
        };
        if owner_ready(&held, &owner).is_err() {
            return Ok(false);
        }
        let mut state = workspace.lock()?;
        if state.closed || state.mode != Mode::Agent || state.revision != expected_revision {
            return Ok(false);
        }
        state.needs_observation = true;
        next_revision(&mut state)?;
        if let Err(error) = navigate() {
            state.mode = Mode::Paused;
            return Err(error);
        }
        Ok(true)
    }

    #[cfg(test)]
    pub(crate) fn begin_snapshot(&self, label: &str) -> Result<Frame, String> {
        self.begin_snapshot_for(label, true)
    }

    pub(crate) fn begin_snapshot_for(&self, label: &str, spectator: bool) -> Result<Frame, String> {
        let mut frames = self
            .frames
            .lock()
            .map_err(|_| "workspace_unavailable: 관전 상태를 확인할 수 없습니다")?;
        if frames.len() >= WORKSPACE_CAP && !frames.contains_key(label) {
            return Err("workspace_limit: 관전 화면 수가 한도에 도달했습니다".to_string());
        }
        let frame = frames.entry(label.to_string()).or_default().clone();
        let mut state = frame
            .lock()
            .map_err(|_| "workspace_unavailable: 관전 상태를 확인할 수 없습니다")?;
        let now = Instant::now();
        if state.in_flight
            || (spectator
                && state
                    .started
                    .is_some_and(|started| now.duration_since(started) < FRAME_INTERVAL))
        {
            return Err(
                "workspace_snapshot_busy: 관전 화면은 초당 한 번만 요청할 수 있습니다".to_string(),
            );
        }
        state.in_flight = true;
        if spectator {
            state.started = Some(now);
        }
        drop(state);
        Ok(Frame(frame))
    }

    pub(crate) fn navigation_check(
        &self,
        label: &str,
        actor: Option<&str>,
    ) -> Result<EvidenceCheck, String> {
        let Some(workspace) = self.get(label)? else {
            return Ok(EvidenceCheck(None));
        };
        let control = {
            let state = workspace.lock()?;
            let control = StepControl {
                label: label.to_string(),
                owner: actor.ok_or_else(changed)?.to_string(),
                revision: state.revision,
            };
            if !agent_holds(&state, &control) {
                return Err(changed());
            }
            control
        };
        Ok(EvidenceCheck(Some((workspace, control))))
    }

    pub(crate) fn admit_evidence(
        &self,
        label: &str,
        control: Option<&StepControl>,
    ) -> Result<Option<(Frame, EvidenceCheck)>, String> {
        match (self.get(label)?, control) {
            (None, None) => Ok(Some((
                self.begin_snapshot_for(label, false)?,
                EvidenceCheck(None),
            ))),
            (Some(workspace), Some(control)) if control.label == label => {
                let state = workspace.lock()?;
                if !agent_holds(&state, control) {
                    return Ok(None);
                }
                let frame = self.begin_snapshot_for(label, false)?;
                drop(state);
                Ok(Some((
                    frame,
                    EvidenceCheck(Some((workspace, control.clone()))),
                )))
            }
            _ => Ok(None),
        }
    }

    pub(crate) fn publish(
        &self,
        label: &str,
        expected_revision: u64,
        publish: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let held = self.lock()?;
        let workspace = held.get(label).ok_or_else(changed)?;
        let state = workspace.lock()?;
        if state.closed || state.revision != expected_revision {
            return Err(changed());
        }
        publish()
    }

    pub(crate) fn cancel_unpublished(
        &self,
        label: &str,
        expected_revision: u64,
        published: impl FnOnce() -> bool,
    ) -> Result<bool, String> {
        let mut held = self.lock()?;
        if published() {
            return Ok(false);
        }
        let Some(workspace) = held.get(label).cloned() else {
            return Ok(true);
        };
        let mut state = workspace.lock()?;
        if state.revision != expected_revision {
            return Err(changed());
        }
        state.closed = true;
        state.mode = Mode::Paused;
        state.needs_observation = true;
        drop(state);
        held.remove(label);
        workspace.drained.notify_waiters();
        Ok(true)
    }

    pub(crate) fn begin_close(
        &self,
        label: &str,
        expected_revision: Option<u64>,
    ) -> Result<Option<Closing>, String> {
        let Some(workspace) = self.get(label)? else {
            return Ok(None);
        };
        let mut state = workspace.lock()?;
        if state.closed || expected_revision.is_some_and(|expected| expected != state.revision) {
            return Err(changed());
        }
        let revision = state.revision;
        next_revision(&mut state)?;
        state.closed = true;
        state.mode = Mode::Paused;
        state.needs_observation = true;
        drop(state);
        workspace.drained.notify_waiters();
        Ok(Some(Closing {
            workspace,
            revision,
            completed: false,
            notice: None,
        }))
    }

    pub(crate) fn closed_at(&self, label: &str, revision: u64) -> Result<bool, String> {
        Ok(self
            .closed
            .lock()
            .map_err(|_| "workspace_unavailable: 닫기 확인 상태를 읽을 수 없습니다")?
            .get(label)
            == Some(&revision))
    }

    pub(crate) fn confirm_closed(&self, label: &str, revision: u64) -> Result<(), String> {
        let mut closed = self
            .closed
            .lock()
            .map_err(|_| "workspace_unavailable: 닫기 확인 상태를 읽을 수 없습니다")?;
        if closed.len() >= WORKSPACE_CAP && !closed.contains_key(label) {
            closed.pop_first();
        }
        closed.insert(label.to_string(), revision);
        Ok(())
    }

    pub(crate) fn remove(&self, label: &str) -> Result<(), String> {
        let workspace = self.lock()?.remove(label);
        if let Some(workspace) = workspace {
            workspace.lock()?.closed = true;
            workspace.drained.notify_waiters();
        }
        self.frames
            .lock()
            .map_err(|_| "workspace_unavailable: 관전 상태를 확인할 수 없습니다")?
            .remove(label);
        for activity in self
            .actors
            .lock()
            .map_err(|_| "workspace_unavailable: 에이전트 요청 상태를 확인할 수 없습니다")?
            .values()
        {
            activity
                .state
                .lock()
                .map_err(|_| "workspace_unavailable: 에이전트 요청 상태를 확인할 수 없습니다")?
                .uncertain
                .remove(label);
        }
        Ok(())
    }
}

pub(crate) struct Closing {
    workspace: Arc<Workspace>,
    revision: u64,
    completed: bool,
    notice: Option<Notice>,
}

impl Closing {
    pub(crate) fn notifying(mut self, notice: impl Fn(Snapshot) + Send + Sync + 'static) -> Self {
        self.notice = Some(Box::new(notice));
        self
    }

    pub(crate) fn finish(mut self) {
        self.completed = true;
    }
}

impl Drop for Closing {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let snapshot = if let Ok(mut state) = self.workspace.lock() {
            if state.revision <= self.revision {
                return;
            }
            state.closed = false;
            state.mode = Mode::Paused;
            state.needs_observation = true;
            state.revision = state.revision.saturating_add(1);
            Some(self.workspace.snapshot(&state))
        } else {
            None
        };
        self.workspace.drained.notify_waiters();
        if let (Some(notice), Some(snapshot)) = (&self.notice, snapshot) {
            notice(snapshot);
        }
    }
}

pub(crate) struct ActorOperation {
    activity: Arc<ActorActivity>,
    label: Option<String>,
    workspaces: Vec<Arc<Workspace>>,
    completed: bool,
    notice: Option<Notice>,
}

impl ActorOperation {
    pub(crate) fn notifying(mut self, notice: impl Fn(Snapshot) + Send + Sync + 'static) -> Self {
        self.notice = Some(Box::new(notice));
        self
    }

    pub(crate) fn settle(mut self, dispatch: Dispatch) {
        self.completed = dispatch != Dispatch::Uncertain;
    }
}

impl Drop for ActorOperation {
    fn drop(&mut self) {
        if let Ok(mut state) = self.activity.state.lock() {
            state.in_flight = state.in_flight.saturating_sub(1);
            if !self.completed
                && let Some(label) = &self.label
            {
                state.uncertain.insert(label.clone());
            }
        }
        if !self.completed {
            for workspace in &self.workspaces {
                if let Ok(mut state) = workspace.lock()
                    && !state.closed
                {
                    state.mode = Mode::Paused;
                    state.needs_observation = true;
                    state.revision = state.revision.saturating_add(1);
                    let snapshot = workspace.snapshot(&state);
                    drop(state);
                    workspace.drained.notify_waiters();
                    if let Some(notice) = &self.notice {
                        notice(snapshot);
                    }
                }
            }
        }
        self.activity.drained.notify_waiters();
    }
}

pub(crate) struct Frame(Arc<Mutex<FrameState>>);
impl Drop for Frame {
    fn drop(&mut self) {
        if let Ok(mut state) = self.0.lock() {
            state.in_flight = false;
        }
    }
}

pub(crate) struct Operation {
    workspace: Arc<Workspace>,
    revision: u64,
    owner: Option<String>,
    changes_page: bool,
    observes: bool,
    completed: bool,
    notice: Option<Notice>,
}

impl Operation {
    pub(crate) fn notifying(mut self, notice: impl Fn(Snapshot) + Send + Sync + 'static) -> Self {
        self.notice = Some(Box::new(notice));
        self
    }

    pub(crate) fn step_control(&self) -> Option<StepControl> {
        Some(StepControl {
            label: self.workspace.label.clone(),
            owner: self.owner.clone()?,
            revision: self.revision,
        })
    }

    /// Issue the read's captured revision only while the same actor still
    /// owns the same Agent control interval. A transition during the await
    /// makes the whole observation unusable, including an otherwise good PNG.
    pub(crate) fn finish_observation(mut self) -> Result<u64, String> {
        let mut state = self.workspace.lock()?;
        if !self.observes
            || state.closed
            || state.revision != self.revision
            || state.mode != Mode::Agent
            || state.owner != self.owner
            || self.owner.is_none()
        {
            return Err(changed());
        }
        state.needs_observation = false;
        self.completed = true;
        Ok(self.revision)
    }

    pub(crate) fn finish(self, succeeded: bool) {
        self.settle(Dispatch::of(succeeded));
    }

    pub(crate) fn settle(mut self, dispatch: Dispatch) {
        if dispatch != Dispatch::NotSent
            && let Ok(mut state) = self.workspace.lock()
            && !state.closed
            && state.revision == self.revision
            && state.mode == Mode::Agent
            && state.owner == self.owner
            && self.observes
        {
            state.needs_observation = dispatch == Dispatch::Uncertain;
        }
        self.completed = dispatch != Dispatch::Uncertain || !self.changes_page;
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        let snapshot = if let Ok(mut state) = self.workspace.lock() {
            state.in_flight = state.in_flight.saturating_sub(1);
            if !state.closed
                && !self.completed
                && self.observes
                && state.revision == self.revision
                && state.mode == Mode::Agent
            {
                state.needs_observation = true;
            }
            if !state.closed && !self.completed && self.changes_page {
                state.mode = Mode::Paused;
                state.needs_observation = true;
                state.revision = state.revision.saturating_add(1);
            }
            Some(self.workspace.snapshot(&state))
        } else {
            None
        };
        self.workspace.drained.notify_waiters();
        if let (Some(notice), Some(snapshot)) = (&self.notice, snapshot) {
            notice(snapshot);
        }
    }
}

pub(crate) struct Transition {
    workspace: Arc<Workspace>,
    activity: Option<Arc<ActorActivity>>,
    owner: Option<String>,
    revision: u64,
    target: Mode,
    fenced: bool,
    completed: bool,
    notice: Option<Notice>,
}

impl Transition {
    pub(crate) fn notifying(mut self, notice: impl Fn(Snapshot) + Send + Sync + 'static) -> Self {
        self.notice = Some(Box::new(notice));
        self
    }

    pub(crate) fn snapshot(&self) -> Result<Snapshot, String> {
        Ok(self.workspace.snapshot(&*self.workspace.lock()?))
    }

    pub(crate) async fn wait(&self) -> Result<(), String> {
        loop {
            let notified = self.workspace.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let activity_notified = self
                .activity
                .as_ref()
                .map(|activity| activity.drained.notified());
            tokio::pin!(activity_notified);
            if let Some(notified) = activity_notified.as_mut().as_pin_mut() {
                notified.enable();
            }
            let actor_drained = self.actor_drained()?;
            {
                let state = self.workspace.lock()?;
                if state.closed || state.revision != self.revision || state.mode != Mode::Pausing {
                    return Err(changed());
                }
                if state.in_flight == 0 && state.capturing == 0 && actor_drained {
                    return Ok(());
                }
            }
            tokio::select! {
                _ = notified => {},
                _ = async {
                    if let Some(notified) = activity_notified.as_mut().as_pin_mut() { notified.await; }
                    else { std::future::pending::<()>().await; }
                } => {}
            }
        }
    }

    fn actor_drained(&self) -> Result<bool, String> {
        self.activity.as_ref().map_or(Ok(true), |activity| {
            activity
                .state
                .lock()
                .map(|state| state.in_flight == 0)
                .map_err(|_| {
                    "workspace_unavailable: 에이전트 요청 상태를 확인할 수 없습니다".to_string()
                })
        })
    }

    pub(crate) fn fence_labels(&self) -> Result<Vec<String>, String> {
        if !self.actor_drained()? {
            return Err(changed());
        }
        let mut labels = BTreeSet::from([self.workspace.label.clone()]);
        if let Some(activity) = &self.activity {
            labels.extend(
                activity
                    .state
                    .lock()
                    .map_err(|_| "workspace_unavailable: 에이전트 요청 상태를 확인할 수 없습니다")?
                    .uncertain
                    .iter()
                    .cloned(),
            );
        }
        Ok(labels.into_iter().collect())
    }

    pub(crate) fn confirm_fence(&mut self) -> Result<(), String> {
        if !self.actor_drained()? {
            return Err(changed());
        }
        let state = self.workspace.lock()?;
        if state.closed
            || state.revision != self.revision
            || state.mode != Mode::Pausing
            || state.in_flight != 0
        {
            return Err(changed());
        }
        if let Some(activity) = &self.activity {
            activity
                .state
                .lock()
                .map_err(|_| "workspace_unavailable: 에이전트 요청 상태를 확인할 수 없습니다")?
                .uncertain
                .clear();
        }
        self.fenced = true;
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<Snapshot, String> {
        if !self.actor_drained()? {
            return Err(changed());
        }
        let snapshot = {
            let mut state = self.workspace.lock()?;
            if state.closed
                || state.revision != self.revision
                || state.mode != Mode::Pausing
                || state.in_flight != 0
                || state.capturing != 0
                || !self.fenced
            {
                return Err(changed());
            }
            next_revision(&mut state)?;
            state.mode = self.target;
            state.person_controlled |= self.target == Mode::Human;
            if let Some(owner) = self.owner.take() {
                state.owner = Some(owner);
            }
            self.workspace.snapshot(&state)
        };
        self.completed = true;
        if let Some(notice) = &self.notice {
            notice(snapshot.clone());
        }
        Ok(snapshot)
    }
}

impl Drop for Transition {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let snapshot = if let Ok(mut state) = self.workspace.lock() {
            if state.closed || state.revision != self.revision || state.mode != Mode::Pausing {
                return;
            }
            state.mode = Mode::Paused;
            state.needs_observation = true;
            state.revision = state.revision.saturating_add(1);
            Some(self.workspace.snapshot(&state))
        } else {
            None
        };
        self.workspace.drained.notify_waiters();
        if let (Some(notice), Some(snapshot)) = (&self.notice, snapshot) {
            notice(snapshot);
        }
    }
}

#[cfg(test)]
mod review_regressions {
    use super::*;

    fn owned() -> Workspaces {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("term-1"), false)
            .unwrap();
        workspaces
    }

    #[test]
    fn a_failed_mutation_pauses_before_a_late_page_reply() {
        let workspaces = owned();
        workspaces
            .begin("browser-1", Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
        workspaces
            .begin("browser-1", Some("term-1"), "click")
            .unwrap()
            .unwrap()
            .finish(false);
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
    }

    #[test]
    fn an_unbound_workspace_is_paused_without_a_restore_flag() {
        let workspaces = Workspaces::default();
        assert_eq!(
            workspaces.register("browser-1", None, false).unwrap().mode,
            Mode::Paused
        );
    }

    #[test]
    fn an_unbound_workspace_cannot_be_resumed_by_the_first_agent() {
        let workspaces = Workspaces::default();
        workspaces.register("browser-1", None, false).unwrap();
        assert!(workspaces.transition("browser-1", Mode::Agent, 1).is_err());
    }

    #[test]
    fn arbitrary_eval_cannot_schedule_a_write_in_a_managed_workspace() {
        let workspaces = owned();
        workspaces
            .begin("browser-1", Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "eval")
                .is_err()
        );
    }

    #[test]
    fn an_unknown_verb_is_refused_before_an_operation_is_created() {
        let workspaces = owned();
        workspaces
            .begin("browser-1", Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "unknown")
                .is_err()
        );
    }

    #[test]
    fn dropping_a_transition_leaves_actionable_paused_state() {
        let workspaces = owned();
        drop(workspaces.transition("browser-1", Mode::Human, 1).unwrap());
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
    }

    #[test]
    fn one_paused_tab_blocks_its_owners_other_tab_mutations() {
        let workspaces = owned();
        workspaces
            .register("browser-2", Some("term-1"), false)
            .unwrap();
        workspaces
            .begin("browser-2", Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
        workspaces
            .transition("browser-1", Mode::Paused, 1)
            .unwrap()
            .finish()
            .unwrap();
        assert!(
            workspaces
                .begin("browser-2", Some("term-1"), "click")
                .is_err()
        );
        assert!(
            workspaces
                .begin("browser-person", Some("term-1"), "goto")
                .is_err()
        );
    }

    #[test]
    fn nil_actors_cannot_use_unmanaged_tabs() {
        assert!(
            Workspaces::default()
                .begin("browser-person", None, "read")
                .is_err()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened() -> Workspaces {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("pane-a"), false)
            .unwrap();
        workspaces
    }

    fn observe(workspaces: &Workspaces) {
        workspaces
            .begin("browser-1", Some("pane-a"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    async fn change(workspaces: &Workspaces, target: Mode) -> Snapshot {
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces
            .transition("browser-1", target, revision)
            .unwrap();
        transition.wait().await.unwrap();
        if target != Mode::Paused {
            transition.confirm_fence().unwrap();
        }
        transition.finish().unwrap()
    }

    #[test]
    fn a_new_workspace_hides_native_input_and_requires_a_fresh_observation() {
        let workspaces = opened();
        assert!(!workspaces.allows_person("browser-1").unwrap());
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "click")
                .is_err()
        );
        observe(&workspaces);
        workspaces
            .begin("browser-1", Some("pane-a"), "click")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    #[test]
    fn an_unmanaged_person_tab_is_not_silently_registered() {
        let workspaces = opened();
        assert!(
            workspaces
                .begin("browser-2", Some("pane-a"), "read")
                .unwrap()
                .is_none()
        );
        assert!(workspaces.allows_person("browser-2").unwrap());
        assert_eq!(workspaces.snapshots().unwrap().len(), 1);
    }

    #[test]
    fn a_restored_workspace_starts_paused_without_an_old_actor() {
        let workspaces = Workspaces::default();
        let snapshot = workspaces.register("browser-1", None, true).unwrap();
        assert_eq!(snapshot.mode, Mode::Paused);
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "goto")
                .is_err()
        );
        assert!(!workspaces.allows_person("browser-1").unwrap());
    }

    #[tokio::test]
    async fn observing_a_paused_workspace_does_not_claim_its_next_run() {
        let workspaces = Workspaces::default();
        workspaces.register("browser-1", None, true).unwrap();
        assert!(
            workspaces
                .begin("browser-1", Some("pane-b"), "read")
                .is_err()
        );
        assert!(workspaces.transition("browser-1", Mode::Agent, 1).is_err());
        let mut transition = workspaces
            .transition_owned("browser-1", Mode::Agent, 1, Some("pane-a"))
            .unwrap();
        transition.wait().await.unwrap();
        transition.confirm_fence().unwrap();
        transition.finish().unwrap();
        observe(&workspaces);
        workspaces
            .begin("browser-1", Some("pane-a"), "click")
            .unwrap()
            .unwrap()
            .finish(true);
        assert!(
            workspaces
                .begin("browser-1", Some("pane-b"), "read")
                .is_err()
        );
    }

    #[test]
    fn another_actor_cannot_read_or_write_an_owned_workspace() {
        let workspaces = opened();
        for verb in ["read", "marks", "eval", "click", "close"] {
            assert!(workspaces.begin("browser-1", Some("pane-b"), verb).is_err());
            assert!(workspaces.begin("browser-1", None, verb).is_err());
        }
    }

    #[tokio::test]
    async fn takeover_waits_for_the_in_flight_operation_and_blocks_new_input() {
        let workspaces = opened();
        observe(&workspaces);
        let operation = workspaces
            .begin("browser-1", Some("pane-a"), "click")
            .unwrap()
            .unwrap();
        let mut transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        assert_eq!(transition.snapshot().unwrap().mode, Mode::Pausing);
        assert!(!workspaces.allows_person("browser-1").unwrap());
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "eval")
                .is_err()
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(5), transition.wait())
                .await
                .is_err()
        );
        operation.finish(true);
        transition.wait().await.unwrap();
        transition.confirm_fence().unwrap();
        assert_eq!(transition.finish().unwrap().mode, Mode::Human);
        assert!(workspaces.allows_person("browser-1").unwrap());
    }

    #[tokio::test]
    async fn every_page_mutation_including_eval_is_blocked_during_human_control() {
        let workspaces = opened();
        change(&workspaces, Mode::Human).await;
        for verb in [
            "eval", "click", "type", "fill", "goto", "close", "viewport", "scroll", "find",
        ] {
            assert!(
                workspaces.begin("browser-1", Some("pane-a"), verb).is_err(),
                "{verb}"
            );
        }
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "read")
                .is_err()
        );
        assert!(
            workspaces
                .snapshot("browser-1")
                .unwrap()
                .unwrap()
                .needs_observation
        );
    }

    #[tokio::test]
    async fn a_resume_requires_observation_after_the_control_transition() {
        let workspaces = opened();
        observe(&workspaces);
        change(&workspaces, Mode::Human).await;
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "read")
                .is_err()
        );
        change(&workspaces, Mode::Agent).await;
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "fill")
                .is_err()
        );
        observe(&workspaces);
        workspaces
            .begin("browser-1", Some("pane-a"), "fill")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    #[test]
    fn a_stale_control_response_cannot_replace_the_newer_transition() {
        let workspaces = opened();
        let old = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        assert!(workspaces.transition("browser-1", Mode::Agent, 1).is_err());
        let newer = workspaces.transition("browser-1", Mode::Paused, 2).unwrap();
        assert!(old.finish().is_err());
        assert_eq!(newer.finish().unwrap().mode, Mode::Paused);
    }

    #[tokio::test]
    async fn a_cancelled_input_cannot_satisfy_a_pending_takeover() {
        let workspaces = opened();
        observe(&workspaces);
        let operation = workspaces
            .begin("browser-1", Some("pane-a"), "type")
            .unwrap()
            .unwrap();
        let transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        drop(operation);
        assert!(transition.wait().await.is_err());
        assert!(!workspaces.allows_person("browser-1").unwrap());
    }

    #[tokio::test]
    async fn closing_a_workspace_invalidates_held_operations_and_transitions() {
        let workspaces = opened();
        let transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        workspaces.remove("browser-1").unwrap();
        assert!(transition.wait().await.is_err());
        assert!(workspaces.snapshots().unwrap().is_empty());
    }

    /// The person's one stop pauses a work browser while its agent keeps
    /// sending input from another thread (t-62872): an input is admitted
    /// before the pause began — and the pause waits for it — or refused; none
    /// is admitted once the pause began, and the pause leaves nothing in
    /// flight.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_pause_racing_an_agents_input_admits_none_once_it_began() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        const ROUNDS: usize = 200;
        for _ in 0..ROUNDS {
            let workspaces = Arc::new(opened());
            observe(&workspaces);
            let before = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
            let paused = Arc::new(AtomicBool::new(false));
            let landed = Arc::new(AtomicUsize::new(0));
            let agent = {
                let workspaces = workspaces.clone();
                let paused = paused.clone();
                let landed = landed.clone();
                std::thread::spawn(move || {
                    let mut admitted = Vec::new();
                    while !paused.load(Ordering::Acquire) {
                        let seen = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
                        if let Ok(Some(operation)) =
                            workspaces.begin_at("browser-1", Some("pane-a"), "click", Some(seen))
                        {
                            admitted.push(operation.revision);
                            landed.fetch_add(1, Ordering::AcqRel);
                            operation.finish(true);
                        }
                    }
                    admitted
                })
            };
            // The pause comes while the agent's input is already landing.
            while landed.load(Ordering::Acquire) == 0 {
                std::thread::yield_now();
            }
            let transition = workspaces
                .transition("browser-1", Mode::Paused, before)
                .unwrap();
            transition.wait().await.unwrap();
            let left = transition.finish().unwrap();
            let again = Some(left.revision);
            let after = workspaces.begin_at("browser-1", Some("pane-a"), "click", again);
            paused.store(true, Ordering::Release);
            let admitted = agent.join().unwrap();
            assert!(!admitted.is_empty(), "the agent's input raced the pause");
            assert!(
                admitted.iter().all(|revision| *revision == before),
                "an input was admitted after the pause began at {before}: {admitted:?}"
            );
            assert_eq!(left.mode, Mode::Paused);
            assert_eq!(left.in_flight, 0, "the pause leaves nothing in flight");
            assert!(after.is_err(), "nothing is admitted once paused");
        }
    }

    /// The person's stop at the work browsers' door (t-62872): an agent that
    /// keeps sending input from another thread gets none in once the person's
    /// stop is made, though the window has not paused its browser yet — its
    /// walk is still on the way — and the pause that follows finds nothing in
    /// flight. The inputs admitted between the person's stop and the window's
    /// pause, over every round, are what the stop holds at nought.
    #[test]
    fn no_agent_input_gets_in_between_the_persons_stop_and_the_windows_pause() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use zerocode_core::computer_use::STOP_REASON_WINDOW;
        const ROUNDS: usize = 50;
        const TRIES_AFTER: usize = 64;
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (mut between, mut tried) = (0, 0);
        for _ in 0..ROUNDS {
            crate::computer_use::guard::lift();
            let workspaces = Arc::new(opened());
            observe(&workspaces);
            let stopped = Arc::new(AtomicBool::new(false));
            let done = Arc::new(AtomicBool::new(false));
            let landed = Arc::new(AtomicUsize::new(0));
            let tries_after = Arc::new(AtomicUsize::new(0));
            let agent = {
                let (workspaces, stopped, done) =
                    (workspaces.clone(), stopped.clone(), done.clone());
                let (landed, tries_after) = (landed.clone(), tries_after.clone());
                std::thread::spawn(move || {
                    let mut after = 0;
                    while !done.load(Ordering::Acquire) {
                        let late = stopped.load(Ordering::Acquire);
                        let seen = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
                        if let Ok(Some(operation)) = workspaces.begin_agents(
                            "browser-1",
                            Some("pane-a"),
                            "click",
                            Some(seen),
                        ) {
                            landed.fetch_add(1, Ordering::AcqRel);
                            after += usize::from(late);
                            operation.finish(true);
                        }
                        if late {
                            tries_after.fetch_add(1, Ordering::AcqRel);
                        }
                    }
                    after
                })
            };
            while landed.load(Ordering::Acquire) == 0 {
                std::thread::yield_now();
            }
            crate::computer_use::guard::stop(STOP_REASON_WINDOW);
            stopped.store(true, Ordering::Release);
            // The window's walk is still on the way: the agent keeps trying.
            while tries_after.load(Ordering::Acquire) < TRIES_AFTER {
                std::thread::yield_now();
            }
            let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
            let transition = workspaces
                .transition("browser-1", Mode::Paused, revision)
                .unwrap();
            runtime.block_on(transition.wait()).unwrap();
            let left = transition.finish().unwrap();
            done.store(true, Ordering::Release);
            between += agent.join().unwrap();
            tried += tries_after.load(Ordering::Acquire);
            crate::computer_use::guard::lift();
            assert_eq!(left.mode, Mode::Paused);
            assert_eq!(left.in_flight, 0, "the pause leaves nothing in flight");
        }
        assert!(
            tried >= ROUNDS * TRIES_AFTER,
            "the agent kept trying after every stop: {tried}"
        );
        assert_eq!(
            between, 0,
            "agent inputs admitted after the person's stop and before the window's pause, over {ROUNDS} rounds: {between}"
        );
    }

    /// While the person's stop stands (t-62872) an agent's `open` registers no
    /// work browser, and every verb an agent sends to a work browser — a read
    /// as much as an input — is refused in words that say the person stopped
    /// it. A browser that is no work browser is not the stop's, nor is an
    /// agent's own stop the person's; once the person's stop is lifted, the
    /// door opens again for a browser in the agent's hands.
    #[test]
    fn the_persons_stop_refuses_an_agents_open_and_every_verb_on_a_work_browser() {
        use zerocode_core::computer_use::{STOP_REASON_HOTKEY, STOP_REASON_REQUEST};
        fn person<T>(result: &Result<T, String>) -> bool {
            result
                .as_ref()
                .is_err_and(|error| error.starts_with("workspace_person_stopped:"))
        }
        let _hand = crate::tests::computer_desktop_wait::ONE_HAND
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let admitted = |result: Result<Option<Operation>, String>| {
            result.map(|operation| operation.map(|operation| operation.finish(true)).is_some())
        };
        crate::computer_use::guard::lift();
        let workspaces = opened();
        observe(&workspaces);
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        crate::computer_use::guard::adopt(STOP_REASON_HOTKEY);
        let open = workspaces.register_agents("browser-2", Some("pane-a"));
        let registered = workspaces.snapshot("browser-2").unwrap().is_some();
        let read = workspaces.begin_agents("browser-1", Some("pane-a"), "read", None);
        let read_refused = person(&read);
        let read = admitted(read);
        let click = workspaces.begin_agents("browser-1", Some("pane-a"), "click", Some(revision));
        let click_refused = person(&click);
        let click = admitted(click);
        let ordinary = admitted(workspaces.begin_agents("ordinary", Some("pane-a"), "read", None));
        crate::computer_use::guard::lift();
        crate::computer_use::guard::stop(STOP_REASON_REQUEST);
        let under_agents_stop =
            admitted(workspaces.begin_agents("browser-1", Some("pane-a"), "read", None));
        crate::computer_use::guard::lift();
        let lifted = admitted(workspaces.begin_agents("browser-1", Some("pane-a"), "read", None));
        let open_lifted = workspaces
            .register_agents("browser-3", Some("pane-a"))
            .map(|snapshot| snapshot.mode);
        assert!(
            person(&open) && !registered,
            "an agent's open while the person's stop stands registers no work browser: {:?}",
            open.map(|snapshot| snapshot.mode)
        );
        assert!(
            read_refused && click_refused,
            "a read and an input on a work browser are refused in the person's words: {read:?} {click:?}"
        );
        assert_eq!(
            ordinary,
            Ok(false),
            "a browser that is no work browser is not the stop's"
        );
        assert_eq!(
            under_agents_stop,
            Ok(true),
            "an agent's own stop is not the person's"
        );
        assert_eq!(lifted, Ok(true), "the lifted stop opens the door again");
        assert_eq!(open_lifted, Ok(Mode::Agent), "and an agent may open again");
    }

    #[test]
    fn simultaneous_commands_do_not_overlap_on_one_workspace() {
        let workspaces = opened();
        let operation = workspaces
            .begin("browser-1", Some("pane-a"), "read")
            .unwrap()
            .unwrap();
        assert!(
            workspaces
                .begin("browser-1", Some("pane-a"), "marks")
                .is_err()
        );
        operation.finish(true);
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().in_flight,
            0
        );
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn owned() -> Workspaces {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("term-1"), false)
            .unwrap();
        workspaces
    }

    fn observe(workspaces: &Workspaces) {
        workspaces
            .begin("browser-1", Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    fn change(workspaces: &Workspaces, mode: Mode) -> Snapshot {
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces.transition("browser-1", mode, revision).unwrap();
        if mode != Mode::Paused {
            transition.confirm_fence().unwrap();
        }
        transition.finish().unwrap()
    }

    #[test]
    fn human_control_refuses_every_registered_agent_verb_without_changing_visibility() {
        let workspaces = owned();
        change(&workspaces, Mode::Human);
        for row in zerocode_core::agent_browser::BROWSER_VERBS {
            assert!(
                workspaces
                    .begin("browser-1", Some("term-1"), row.word)
                    .is_err(),
                "{}",
                row.word
            );
            assert!(
                workspaces.allows_person("browser-1").unwrap(),
                "{}",
                row.word
            );
            assert_eq!(
                workspaces.snapshot("browser-1").unwrap().unwrap().in_flight,
                0
            );
        }
        assert!(
            !workspaces
                .allows_agent_metadata("browser-1", Some("term-1"))
                .unwrap()
        );
    }

    #[test]
    fn an_uncertain_mutation_requires_a_page_fence_before_human_control() {
        let workspaces = owned();
        observe(&workspaces);
        workspaces
            .begin("browser-1", Some("term-1"), "click")
            .unwrap()
            .unwrap()
            .finish(false);
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        assert!(transition.finish().is_err());
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
        assert!(!workspaces.allows_person("browser-1").unwrap());
        assert_eq!(change(&workspaces, Mode::Human).mode, Mode::Human);
    }

    #[test]
    fn a_fence_cannot_confirm_while_the_old_script_is_in_flight() {
        let workspaces = owned();
        observe(&workspaces);
        let operation = workspaces
            .begin("browser-1", Some("term-1"), "click")
            .unwrap()
            .unwrap();
        let mut transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        assert!(transition.confirm_fence().is_err());
        operation.finish(true);
        transition.confirm_fence().unwrap();
        assert_eq!(transition.finish().unwrap().mode, Mode::Human);
    }

    #[tokio::test]
    async fn cancellation_of_the_handoff_wait_is_paused_and_can_be_retried() {
        let workspaces = owned();
        let operation = workspaces
            .begin("browser-1", Some("term-1"), "read")
            .unwrap()
            .unwrap();
        let transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(1), transition.wait())
                .await
                .is_err()
        );
        drop(transition);
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
        operation.finish(true);
        assert_eq!(change(&workspaces, Mode::Human).mode, Mode::Human);
    }

    #[test]
    fn pause_blocks_open_and_unmanaged_writes_for_the_same_owner_only() {
        let workspaces = owned();
        change(&workspaces, Mode::Paused);
        for verb in ["open", "goto", "type", "fill", "eval", "viewport", "close"] {
            assert!(
                workspaces.check_actor(Some("term-1"), verb).is_err(),
                "{verb}"
            );
            assert!(
                workspaces.begin_actor(Some("term-1"), verb).is_err(),
                "{verb}"
            );
        }
        assert!(
            workspaces
                .register("browser-2", Some("term-1"), false)
                .is_err()
        );
        assert!(workspaces.check_actor(Some("term-2"), "open").is_ok());
        assert!(
            workspaces
                .begin("browser-person", Some("term-2"), "goto")
                .unwrap()
                .is_none()
        );
        assert_eq!(workspaces.snapshots().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn handoff_drains_a_previously_accepted_unmanaged_write_by_the_owner() {
        let workspaces = owned();
        let activity = workspaces
            .begin_actor(Some("term-1"), "type")
            .unwrap()
            .unwrap();
        let mut transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(1), transition.wait())
                .await
                .is_err()
        );
        assert!(transition.confirm_fence().is_err());
        activity.settle(Dispatch::Completed);
        tokio::time::timeout(Duration::from_millis(100), transition.wait())
            .await
            .unwrap()
            .unwrap();
        transition.confirm_fence().unwrap();
        assert_eq!(transition.finish().unwrap().mode, Mode::Human);
    }

    #[test]
    fn changing_the_owner_cleans_the_old_owners_pause_only_when_the_transition_finishes() {
        let workspaces = owned();
        let mut transition = workspaces
            .transition_owned("browser-1", Mode::Human, 1, Some("term-2"))
            .unwrap();
        assert!(workspaces.check_actor(Some("term-1"), "open").is_err());
        transition.confirm_fence().unwrap();
        let snapshot = transition.finish().unwrap();
        assert_eq!(snapshot.owner_term.as_deref(), Some("2"));
        assert!(workspaces.check_actor(Some("term-1"), "open").is_ok());
        assert!(workspaces.check_actor(Some("term-2"), "open").is_err());
    }

    #[test]
    fn goto_after_human_control_requires_a_new_agent_observation() {
        let workspaces = owned();
        observe(&workspaces);
        change(&workspaces, Mode::Human);
        change(&workspaces, Mode::Agent);
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "goto")
                .is_err()
        );
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "close")
                .is_err()
        );
        observe(&workspaces);
        workspaces
            .begin("browser-1", Some("term-1"), "goto")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    #[test]
    fn a_request_queued_before_handoff_cannot_borrow_a_new_observation() {
        let workspaces = owned();
        let old = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        change(&workspaces, Mode::Human);
        change(&workspaces, Mode::Agent);
        observe(&workspaces);
        assert!(
            workspaces
                .begin_at("browser-1", Some("term-1"), "click", Some(old))
                .is_err()
        );
        let current = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        workspaces
            .begin_at("browser-1", Some("term-1"), "click", Some(current))
            .unwrap()
            .unwrap()
            .finish(true);
    }

    #[test]
    fn human_mutations_require_the_current_revision_and_keep_normal_tabs_compatible() {
        let workspaces = owned();
        let human = change(&workspaces, Mode::Human);
        assert!(workspaces.begin_person("browser-1", None).is_err());
        assert!(
            workspaces
                .begin_person("browser-1", Some(human.revision - 1))
                .is_err()
        );
        workspaces
            .begin_person("browser-1", Some(human.revision))
            .unwrap()
            .unwrap()
            .finish(true);
        assert!(
            workspaces
                .begin_person("browser-person", None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_late_human_popup_does_not_enter_a_later_human_session() {
        let workspaces = owned();
        let old = change(&workspaces, Mode::Human);
        change(&workspaces, Mode::Agent);
        let new = change(&workspaces, Mode::Human);
        assert!(
            workspaces
                .begin_person("browser-1", Some(old.revision))
                .is_err()
        );
        workspaces
            .begin_person("browser-1", Some(new.revision))
            .unwrap()
            .unwrap()
            .finish(true);
    }

    #[test]
    fn placement_rechecks_the_revision_at_the_native_operation() {
        let workspaces = owned();
        let old = change(&workspaces, Mode::Human);
        change(&workspaces, Mode::Agent);
        let current = change(&workspaces, Mode::Human);
        workspaces
            .with_placement("browser-1", Some(old.revision), |allowed| {
                assert!(!allowed);
                Ok(())
            })
            .unwrap();
        workspaces
            .with_placement("browser-1", Some(current.revision), |allowed| {
                assert!(allowed);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn agent_popup_navigation_invalidates_observation_without_an_extra_agent_request() {
        let workspaces = owned();
        observe(&workspaces);
        let navigated = AtomicUsize::new(0);
        assert!(
            workspaces
                .with_agent_popup("browser-1", 1, || {
                    navigated.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
                .unwrap()
        );
        assert_eq!(navigated.load(Ordering::Relaxed), 1);
        assert!(
            workspaces
                .snapshot("browser-1")
                .unwrap()
                .unwrap()
                .needs_observation
        );
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "click")
                .is_err()
        );
    }

    #[test]
    fn paused_stale_and_closed_popups_never_navigate() {
        let workspaces = owned();
        let navigated = AtomicUsize::new(0);
        let paused = change(&workspaces, Mode::Paused);
        assert!(
            !workspaces
                .with_agent_popup("browser-1", paused.revision, || {
                    navigated.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
                .unwrap()
        );
        change(&workspaces, Mode::Agent);
        assert!(
            !workspaces
                .with_agent_popup("browser-1", 1, || {
                    navigated.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
                .unwrap()
        );
        workspaces.remove("browser-1").unwrap();
        assert!(
            !workspaces
                .with_agent_popup("browser-1", 1, || {
                    navigated.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                })
                .unwrap()
        );
        assert_eq!(navigated.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_failed_native_popup_is_paused() {
        let workspaces = owned();
        assert!(
            workspaces
                .with_agent_popup("browser-1", 1, || Err("native unavailable".to_string()))
                .is_err()
        );
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
    }

    #[test]
    fn every_browser_verb_uses_the_core_mutation_classification() {
        for row in zerocode_core::agent_browser::BROWSER_VERBS {
            assert!(!row.acts || row.changes_page, "{}", row.word);
        }
        let find = zerocode_core::agent_browser::browser_verb("find").unwrap();
        assert!(find.check && find.changes_page);
        for verb in ["open", "close", "viewport"] {
            assert!(
                zerocode_core::agent_browser::browser_verb(verb)
                    .unwrap()
                    .changes_page
            );
        }
    }

    #[test]
    fn cancellation_notifies_main_with_paused_state_after_releasing_the_operation() {
        let workspaces = owned();
        observe(&workspaces);
        let notices = Arc::new(Mutex::new(Vec::new()));
        let received = notices.clone();
        let operation = workspaces
            .begin("browser-1", Some("term-1"), "type")
            .unwrap()
            .unwrap()
            .notifying(move |snapshot| received.lock().unwrap().push(snapshot));
        drop(operation);
        let notices = notices.lock().unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].mode, Mode::Paused);
        assert_eq!(notices[0].in_flight, 0);
    }

    #[test]
    fn observer_snapshots_do_not_satisfy_the_agents_observation_requirement() {
        let workspaces = owned();
        let frame = workspaces.begin_snapshot("browser-1").unwrap();
        assert!(workspaces.begin_snapshot("browser-1").is_err());
        drop(frame);
        assert!(workspaces.begin_snapshot("browser-1").is_err());
        assert!(
            workspaces
                .snapshot("browser-1")
                .unwrap()
                .unwrap()
                .needs_observation
        );
    }

    #[test]
    fn a_capture_lease_stays_exclusive_until_the_native_callback_releases_it() {
        let workspaces = owned();
        let frame = workspaces.begin_snapshot("browser-1").unwrap();
        frame.0.lock().unwrap().started = Some(Instant::now() - FRAME_INTERVAL);
        assert!(workspaces.begin_snapshot("browser-1").is_err());
        drop(frame);
        assert!(workspaces.begin_snapshot("browser-1").is_ok());
    }

    #[test]
    fn registry_and_frame_records_are_bounded_and_removed_on_close() {
        let workspaces = Workspaces::default();
        for index in 0..WORKSPACE_CAP {
            let label = format!("browser-{index}");
            workspaces.register(&label, Some("term-1"), false).unwrap();
            drop(workspaces.begin_snapshot(&label).unwrap());
        }
        assert!(
            workspaces
                .register("browser-overflow", None, false)
                .is_err()
        );
        assert!(workspaces.begin_snapshot("browser-overflow").is_err());
        workspaces.remove("browser-0").unwrap();
        assert!(workspaces.register("browser-overflow", None, false).is_ok());
        assert!(workspaces.begin_snapshot("browser-overflow").is_ok());
    }

    #[test]
    fn snapshots_keep_null_owner_and_report_the_actual_platform_capability() {
        let workspaces = Workspaces::default();
        let snapshot = workspaces.register("browser-1", None, false).unwrap();
        let value = serde_json::to_value(snapshot).unwrap();
        assert_eq!(value["mode"], "paused");
        assert!(value["ownerTerm"].is_null());
        assert_eq!(value["snapshotSupported"], cfg!(target_os = "macos"));
        assert_eq!(value["inFlight"], 0);
        assert_eq!(value["needsObservation"], true);
    }

    #[test]
    fn unavailable_state_never_grants_native_input() {
        let workspaces = Arc::new(owned());
        let broken = workspaces.clone();
        assert!(
            std::thread::spawn(move || {
                let _state = broken.held.lock().unwrap();
                panic!("poison the registry");
            })
            .join()
            .is_err()
        );
        assert!(workspaces.allows_person("browser-1").is_err());
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "read")
                .is_err()
        );
        assert!(
            workspaces
                .with_placement("browser-1", Some(1), |_| -> Result<(), String> {
                    panic!("unavailable state must not show a pane")
                })
                .is_err()
        );
    }

    #[test]
    fn managed_mutations_require_an_epoch_even_when_the_owner_is_correct() {
        let workspaces = owned();
        observe(&workspaces);
        for entry in zerocode_core::agent_browser::BROWSER_VERBS {
            if !entry.changes_page {
                continue;
            }
            assert!(
                workspaces
                    .begin_at("browser-1", Some("term-1"), entry.word, None)
                    .is_err(),
                "{}",
                entry.word
            );
            assert_eq!(
                workspaces.snapshot("browser-1").unwrap().unwrap().in_flight,
                0,
                "{}",
                entry.word
            );
        }
    }

    #[test]
    fn a_first_observation_acquires_its_revision_only_when_it_completes() {
        let workspaces = owned();
        let initial = workspaces.snapshot("browser-1").unwrap().unwrap();
        let reading = workspaces
            .begin_at("browser-1", Some("term-1"), "read", None)
            .unwrap()
            .unwrap();
        let pending = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert_eq!(pending.in_flight, 1);
        assert!(pending.needs_observation);
        assert_eq!(reading.finish_observation().unwrap(), initial.revision);
        let completed = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert_eq!(completed.in_flight, 0);
        assert!(!completed.needs_observation);
    }

    #[test]
    fn a_stale_read_cannot_borrow_a_resumed_agents_revision() {
        let workspaces = owned();
        let original = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        change(&workspaces, Mode::Human);
        let current = change(&workspaces, Mode::Agent);
        let delayed = workspaces.begin_at("browser-1", Some("term-1"), "read", Some(original));
        assert!(matches!(delayed, Err(error) if error.starts_with("workspace_stale:")));
        let state = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert_eq!(state.revision, current.revision);
        assert_eq!(state.in_flight, 0);
        assert!(state.needs_observation);
    }

    #[test]
    fn a_removed_workspace_cannot_be_reinterpreted_as_a_normal_visible_tab() {
        let workspaces = owned();
        let human = change(&workspaces, Mode::Human);
        workspaces.remove("browser-1").unwrap();
        workspaces
            .with_placement("browser-1", Some(human.revision), |allowed| {
                assert!(!allowed);
                Ok(())
            })
            .unwrap();
        assert!(
            workspaces
                .begin_person("browser-1", Some(human.revision))
                .is_err()
        );
        workspaces
            .with_placement("browser-person", None, |allowed| {
                assert!(allowed);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn an_uncertain_unmanaged_write_is_fenced_without_adopting_the_persons_tab() {
        let workspaces = owned();
        let operation = workspaces
            .begin_actor_target(Some("term-1"), "type", Some("browser-person"))
            .unwrap()
            .unwrap();
        operation.settle(Dispatch::Uncertain);
        assert_eq!(workspaces.snapshots().unwrap().len(), 1);
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        assert_eq!(
            transition.fence_labels().unwrap(),
            ["browser-1", "browser-person"]
        );
        transition.confirm_fence().unwrap();
        transition.finish().unwrap();
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let transition = workspaces
            .transition("browser-1", Mode::Agent, revision)
            .unwrap();
        assert_eq!(transition.fence_labels().unwrap(), ["browser-1"]);
    }

    #[test]
    fn at_most_one_managed_popup_is_deferred_and_a_failed_initial_open_releases_it() {
        let workspaces = owned();
        assert!(workspaces.reserve_popup("browser-1", 1).unwrap());
        assert!(!workspaces.reserve_popup("browser-1", 1).unwrap());
        workspaces.release_popup("browser-1").unwrap();
        assert!(workspaces.reserve_popup("browser-1", 1).unwrap());
        change(&workspaces, Mode::Paused);
        assert!(
            !workspaces
                .with_agent_popup("browser-1", 1, || panic!("stale popup must not navigate"))
                .unwrap()
        );
        assert!(!workspaces.reserve_popup("browser-1", 1).unwrap());
    }

    #[test]
    fn a_canceled_human_mutation_also_requires_recovery_before_control_is_granted() {
        let workspaces = owned();
        let human = change(&workspaces, Mode::Human);
        drop(
            workspaces
                .begin_person("browser-1", Some(human.revision))
                .unwrap()
                .unwrap(),
        );
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
        assert!(
            workspaces
                .begin_person("browser-1", Some(human.revision))
                .is_err()
        );
    }

    #[test]
    fn the_watch_rate_limit_does_not_starve_the_agents_fresh_screenshot() {
        let workspaces = owned();
        let watch = workspaces.begin_snapshot_for("browser-1", true).unwrap();
        assert!(workspaces.begin_snapshot_for("browser-1", false).is_err());
        drop(watch);
        drop(workspaces.begin_snapshot_for("browser-1", false).unwrap());
        assert!(workspaces.begin_snapshot_for("browser-1", true).is_err());
    }

    #[test]
    fn stale_agent_close_preserves_human_control_but_user_close_is_still_allowed() {
        let workspaces = owned();
        let human = change(&workspaces, Mode::Human);
        assert!(workspaces.begin_close("browser-1", Some(1)).is_err());
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().revision,
            human.revision
        );
        assert!(workspaces.allows_person("browser-1").unwrap());
        drop(workspaces.begin_close("browser-1", None).unwrap().unwrap());
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
    }

    #[test]
    fn native_close_failure_rolls_back_to_paused_without_a_closed_receipt() {
        let workspaces = owned();
        let closing = workspaces
            .begin_close("browser-1", Some(1))
            .unwrap()
            .unwrap();
        assert!(workspaces.check_actor(Some("term-1"), "open").is_err());
        assert!(workspaces.transition("browser-1", Mode::Human, 1).is_err());
        assert!(!workspaces.closed_at("browser-1", 1).unwrap());
        drop(closing);
        assert_eq!(
            workspaces.snapshot("browser-1").unwrap().unwrap().mode,
            Mode::Paused
        );
        assert!(!workspaces.closed_at("browser-1", 1).unwrap());
        assert_eq!(change(&workspaces, Mode::Human).mode, Mode::Human);
    }

    #[test]
    fn an_unpublished_cancelled_open_cannot_publish_a_late_native_view() {
        let workspaces = owned();
        assert!(
            workspaces
                .cancel_unpublished("browser-1", 1, || false)
                .unwrap()
        );
        assert!(
            workspaces
                .publish("browser-1", 1, || panic!(
                    "canceled birth must not become public"
                ))
                .is_err()
        );
        assert!(workspaces.snapshots().unwrap().is_empty());
        assert!(!workspaces.closed_at("browser-1", 1).unwrap());
    }

    #[test]
    fn cancellation_cannot_claim_an_already_published_tab_with_a_new_human_revision() {
        let workspaces = owned();
        workspaces.publish("browser-1", 1, || Ok(())).unwrap();
        change(&workspaces, Mode::Human);
        assert!(
            !workspaces
                .cancel_unpublished("browser-1", 1, || true)
                .unwrap()
        );
        assert!(workspaces.begin_close("browser-1", Some(1)).is_err());
        assert!(workspaces.allows_person("browser-1").unwrap());
    }

    #[test]
    fn closed_acknowledgements_are_exact_bounded_and_cleared_on_a_new_registration() {
        let workspaces = Workspaces::default();
        assert!(!workspaces.closed_at("browser-absent", 1).unwrap());
        for index in 0..=WORKSPACE_CAP {
            workspaces
                .confirm_closed(&format!("browser-{index}"), 1)
                .unwrap();
        }
        assert_eq!(workspaces.closed.lock().unwrap().len(), WORKSPACE_CAP);
        assert!(
            workspaces
                .closed_at(&format!("browser-{WORKSPACE_CAP}"), 1)
                .unwrap()
        );
        assert!(
            !workspaces
                .closed_at(&format!("browser-{WORKSPACE_CAP}"), 2)
                .unwrap()
        );
        let label = format!("browser-{WORKSPACE_CAP}");
        workspaces.register(&label, None, false).unwrap();
        assert!(!workspaces.closed_at(&label, 1).unwrap());
    }

    #[test]
    fn held_actor_identity_does_not_depend_on_provider_or_model_names() {
        for term in [7, 44, 2048] {
            let actor = crate::hooks::pane_key_of(term);
            let workspaces = Workspaces::default();
            let snapshot = workspaces
                .register("browser-1", Some(&actor), false)
                .unwrap();
            assert_eq!(snapshot.owner_term, Some(term.to_string()));
            workspaces
                .begin("browser-1", Some(&actor), "read")
                .unwrap()
                .unwrap()
                .finish(true);
            workspaces
                .begin("browser-1", Some(&actor), "click")
                .unwrap()
                .unwrap()
                .finish(true);
            let mut transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
            transition.confirm_fence().unwrap();
            transition.finish().unwrap();
            for row in zerocode_core::agent_browser::BROWSER_VERBS {
                assert!(
                    workspaces
                        .begin("browser-1", Some(&actor), row.word)
                        .is_err(),
                    "{term} {}",
                    row.word
                );
            }
            assert!(workspaces.allows_person("browser-1").unwrap());
        }
    }
}

#[cfg(test)]
mod handoff_followup_regressions {
    use super::*;

    const HISTORY: [&str; 3] = ["console", "network", "diagnose"];

    fn owned() -> Workspaces {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("term-1"), false)
            .unwrap();
        workspaces
    }

    fn observe(workspaces: &Workspaces) {
        workspaces
            .begin("browser-1", Some("term-1"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
    }

    fn change(workspaces: &Workspaces, mode: Mode) -> Snapshot {
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces.transition("browser-1", mode, revision).unwrap();
        if mode != Mode::Paused {
            transition.confirm_fence().unwrap();
        }
        transition.finish().unwrap()
    }

    fn stepped(workspaces: &Workspaces, verb: &str) -> StepControl {
        let operation = workspaces
            .begin("browser-1", Some("term-1"), verb)
            .unwrap()
            .unwrap();
        let control = operation.step_control().unwrap();
        operation.finish(true);
        control
    }

    #[test]
    fn a_step_frame_queued_before_a_person_takes_the_tab_is_skipped_without_a_capture() {
        let workspaces = owned();
        observe(&workspaces);
        let control = stepped(&workspaces, "click");
        change(&workspaces, Mode::Human);
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .unwrap()
                .is_none()
        );
        drop(workspaces.begin_snapshot_for("browser-1", false).unwrap());
        change(&workspaces, Mode::Agent);
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_step_frame_admitted_just_before_a_transfer_is_never_kept_and_never_delays_it() {
        let workspaces = owned();
        observe(&workspaces);
        let control = stepped(&workspaces, "click");
        let (frame, check) = workspaces
            .admit_evidence("browser-1", Some(&control))
            .unwrap()
            .unwrap();
        assert!(check.holds());
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        assert!(!check.holds());
        tokio::time::timeout(Duration::from_millis(100), transition.wait())
            .await
            .unwrap()
            .unwrap();
        transition.confirm_fence().unwrap();
        assert_eq!(transition.finish().unwrap().mode, Mode::Human);
        assert!(!check.holds());
        drop(frame);
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn admitting_and_keeping_a_step_frame_issues_no_observation() {
        let workspaces = owned();
        let control = stepped(&workspaces, "wait");
        let before = workspaces.snapshot("browser-1").unwrap().unwrap();
        let (frame, check) = workspaces
            .admit_evidence("browser-1", Some(&control))
            .unwrap()
            .unwrap();
        assert!(workspaces.begin_snapshot_for("browser-1", true).is_err());
        assert!(check.holds());
        drop(frame);
        let after = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert!(before.needs_observation && after.needs_observation);
        assert_eq!(before.revision, after.revision);
        assert!(matches!(
            workspaces.begin_at("browser-1", Some("term-1"), "click", Some(after.revision)),
            Err(error) if error.starts_with("workspace_observation_required:")
        ));
        let watch = workspaces.begin_snapshot_for("browser-1", true).unwrap();
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .is_err()
        );
        drop(watch);
    }

    #[test]
    fn a_step_frame_needs_its_own_tab_owner_and_managed_state() {
        let workspaces = owned();
        workspaces
            .register("browser-2", Some("term-1"), false)
            .unwrap();
        observe(&workspaces);
        let control = stepped(&workspaces, "click");
        assert!(
            workspaces
                .admit_evidence("browser-2", Some(&control))
                .unwrap()
                .is_none()
        );
        assert!(
            workspaces
                .admit_evidence("browser-1", None)
                .unwrap()
                .is_none()
        );
        assert!(
            workspaces
                .admit_evidence("browser-person", Some(&control))
                .unwrap()
                .is_none()
        );
        drop(
            workspaces
                .admit_evidence("browser-person", None)
                .unwrap()
                .unwrap(),
        );
        workspaces
            .get("browser-1")
            .unwrap()
            .unwrap()
            .lock()
            .unwrap()
            .owner = Some("term-2".into());
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .unwrap()
                .is_none()
        );
        workspaces.remove("browser-1").unwrap();
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_refusal_before_dispatch_keeps_control_of_the_tab_and_the_owners_other_tabs() {
        let workspaces = owned();
        workspaces
            .register("browser-2", Some("term-1"), false)
            .unwrap();
        observe(&workspaces);
        let before = workspaces.snapshot("browser-1").unwrap().unwrap();
        workspaces
            .begin("browser-1", Some("term-1"), "click")
            .unwrap()
            .unwrap()
            .settle(Dispatch::NotSent);
        for verb in ["goto", "type", "fill"] {
            workspaces
                .begin_actor_target(Some("term-1"), verb, Some("browser-person"))
                .unwrap()
                .unwrap()
                .settle(Dispatch::NotSent);
        }
        workspaces
            .begin_actor_target(Some("term-1"), "open", None)
            .unwrap()
            .unwrap()
            .settle(Dispatch::NotSent);
        let after = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert_eq!(
            (after.mode, after.revision, after.needs_observation),
            (Mode::Agent, before.revision, false)
        );
        assert_eq!(
            workspaces.snapshot("browser-2").unwrap().unwrap().mode,
            Mode::Agent
        );
        workspaces
            .begin("browser-1", Some("term-1"), "click")
            .unwrap()
            .unwrap()
            .settle(Dispatch::Completed);
        let transition = workspaces
            .transition("browser-1", Mode::Human, before.revision)
            .unwrap();
        assert_eq!(transition.fence_labels().unwrap(), ["browser-1"]);
    }

    #[test]
    fn an_uncertain_dispatch_pauses_the_tab_and_every_target_of_its_owner() {
        let workspaces = owned();
        observe(&workspaces);
        workspaces
            .begin("browser-1", Some("term-1"), "fill")
            .unwrap()
            .unwrap()
            .settle(Dispatch::Uncertain);
        let paused = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert!(paused.mode == Mode::Paused && paused.needs_observation);
        let workspaces = owned();
        workspaces
            .register("browser-2", Some("term-1"), false)
            .unwrap();
        workspaces
            .begin_actor_target(Some("term-1"), "eval", Some("browser-person"))
            .unwrap()
            .unwrap()
            .settle(Dispatch::Uncertain);
        for label in ["browser-1", "browser-2"] {
            assert_eq!(
                workspaces.snapshot(label).unwrap().unwrap().mode,
                Mode::Paused,
                "{label}"
            );
        }
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        assert_eq!(
            transition.fence_labels().unwrap(),
            ["browser-1", "browser-person"]
        );
    }

    #[test]
    fn a_persons_own_request_keeps_the_human_view_placed_and_a_second_one_is_busy() {
        let workspaces = owned();
        let human = change(&workspaces, Mode::Human);
        let operation = workspaces
            .begin_person("browser-1", Some(human.revision))
            .unwrap()
            .unwrap();
        workspaces
            .with_placement("browser-1", Some(human.revision), |allowed| {
                assert!(allowed);
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            workspaces.begin_person("browser-1", Some(human.revision)),
            Err(error) if error.starts_with("workspace_busy:")
        ));
        assert!(
            workspaces
                .begin("browser-1", Some("term-1"), "read")
                .is_err()
        );
        operation.finish(true);
        workspaces
            .with_placement("browser-1", Some(human.revision), |allowed| {
                assert!(allowed);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn page_history_is_refused_to_agents_once_a_person_has_controlled_the_tab() {
        let workspaces = owned();
        for verb in HISTORY {
            workspaces
                .begin("browser-1", Some("term-1"), verb)
                .unwrap()
                .unwrap()
                .finish(true);
        }
        change(&workspaces, Mode::Human);
        change(&workspaces, Mode::Agent);
        observe(&workspaces);
        for verb in HISTORY {
            assert!(
                matches!(
                    workspaces.begin("browser-1", Some("term-1"), verb),
                    Err(error) if error.starts_with("workspace_history_unsupported:")
                ),
                "{verb}"
            );
        }
        change(&workspaces, Mode::Paused);
        change(&workspaces, Mode::Agent);
        observe(&workspaces);
        for verb in HISTORY {
            assert!(
                workspaces.begin("browser-1", Some("term-1"), verb).is_err(),
                "{verb}"
            );
        }
        workspaces
            .register("browser-2", Some("term-1"), false)
            .unwrap();
        for verb in HISTORY {
            workspaces
                .begin("browser-2", Some("term-1"), verb)
                .unwrap()
                .unwrap()
                .finish(true);
        }
        for row in zerocode_core::agent_browser::BROWSER_VERBS {
            assert_eq!(
                zerocode_core::agent_browser::reads_history(row.word),
                HISTORY.contains(&row.word),
                "{}",
                row.word
            );
        }
    }
}

#[cfg(test)]
mod capture_start_regressions {
    use super::*;
    use std::cell::Cell;

    fn stepped(observed: bool, verb: &str) -> (Workspaces, StepControl) {
        let workspaces = Workspaces::default();
        workspaces
            .register("browser-1", Some("term-1"), false)
            .unwrap();
        if observed {
            workspaces
                .begin("browser-1", Some("term-1"), "read")
                .unwrap()
                .unwrap()
                .finish(true);
        }
        let operation = workspaces
            .begin("browser-1", Some("term-1"), verb)
            .unwrap()
            .unwrap();
        let control = operation.step_control().unwrap();
        operation.finish(true);
        (workspaces, control)
    }

    #[test]
    fn a_capture_queued_before_a_transfer_starts_nothing_once_the_person_holds_the_tab() {
        let (workspaces, control) = stepped(true, "click");
        let (frame, check) = workspaces
            .admit_evidence("browser-1", Some(&control))
            .unwrap()
            .unwrap();
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        transition.confirm_fence().unwrap();
        assert_eq!(transition.finish().unwrap().mode, Mode::Human);
        let sends = Cell::new(0);
        let started = check.start(|| {
            sends.set(sends.get() + 1);
            Ok(())
        });
        assert!(started.is_err());
        assert_eq!(
            sends.get(),
            0,
            "a native capture started while the person held the tab"
        );
        assert!(!check.holds());
        drop(frame);
    }

    #[tokio::test]
    async fn a_transfer_waits_for_a_capture_send_in_progress_but_not_for_its_picture() {
        let (workspaces, control) = stepped(true, "click");
        let (frame, check) = workspaces
            .admit_evidence("browser-1", Some(&control))
            .unwrap()
            .unwrap();
        let (entered, entering) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel::<()>();
        let sender = check.clone();
        let native = std::thread::spawn(move || {
            sender.start(|| {
                entered.send(()).unwrap();
                released.recv().unwrap();
                Ok(())
            })
        });
        entering.recv().unwrap();
        let revision = workspaces.snapshot("browser-1").unwrap().unwrap().revision;
        let mut transition = workspaces
            .transition("browser-1", Mode::Human, revision)
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), transition.wait())
                .await
                .is_err(),
            "the person was handed the tab while a capture was still being sent"
        );
        release.send(()).unwrap();
        native.join().unwrap().unwrap();
        tokio::time::timeout(Duration::from_millis(100), transition.wait())
            .await
            .unwrap()
            .unwrap();
        transition.confirm_fence().unwrap();
        assert_eq!(transition.finish().unwrap().mode, Mode::Human);
        assert!(
            !check.holds(),
            "a picture that arrives after the transfer is kept"
        );
        drop(frame);
        assert!(
            workspaces
                .admit_evidence("browser-1", Some(&control))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn a_capture_sent_under_agent_control_keeps_its_lease_and_issues_no_observation() {
        let (workspaces, control) = stepped(false, "wait");
        let before = workspaces.snapshot("browser-1").unwrap().unwrap();
        let (frame, check) = workspaces
            .admit_evidence("browser-1", Some(&control))
            .unwrap()
            .unwrap();
        let sends = Cell::new(0);
        check
            .start(|| {
                sends.set(sends.get() + 1);
                Ok(())
            })
            .unwrap();
        assert_eq!(sends.get(), 1);
        assert!(workspaces.begin_snapshot_for("browser-1", false).is_err());
        assert!(check.holds());
        drop(frame);
        let after = workspaces.snapshot("browser-1").unwrap().unwrap();
        assert_eq!(
            (after.revision, after.needs_observation, after.mode),
            (before.revision, true, Mode::Agent)
        );
        assert!(
            EvidenceCheck::default()
                .start(|| Ok::<_, String>(sends.get()))
                .is_ok_and(|seen| seen == 1)
        );
        drop(workspaces.begin_snapshot_for("browser-1", false).unwrap());
    }
}
