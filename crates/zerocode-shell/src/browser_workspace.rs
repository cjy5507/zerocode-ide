use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

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
}

struct State {
    mode: Mode,
    revision: u64,
    in_flight: usize,
    needs_observation: bool,
    owner: Option<String>,
    closed: bool,
}

struct Workspace {
    label: String,
    state: Mutex<State>,
    drained: Notify,
}

impl Workspace {
    fn lock(&self) -> Result<MutexGuard<'_, State>, String> {
        self.state
            .lock()
            .map_err(|_| "작업 브라우저의 제어 상태를 확인할 수 없습니다".to_string())
    }

    fn snapshot(&self, state: &State) -> Snapshot {
        Snapshot {
            label: self.label.clone(),
            mode: state.mode,
            revision: state.revision,
            in_flight: state.in_flight,
            needs_observation: state.needs_observation,
        }
    }
}

#[derive(Default)]
pub(crate) struct Workspaces {
    held: Mutex<BTreeMap<String, Arc<Workspace>>>,
}

impl Workspaces {
    pub(crate) fn register(
        &self,
        label: &str,
        owner: Option<&str>,
        restored: bool,
    ) -> Result<Snapshot, String> {
        let mut held = self
            .held
            .lock()
            .map_err(|_| "작업 브라우저 목록을 확인할 수 없습니다".to_string())?;
        if held.contains_key(label) {
            return Err("이미 등록된 작업 브라우저입니다".to_string());
        }
        let workspace = Arc::new(Workspace {
            label: label.to_string(),
            state: Mutex::new(State {
                mode: if restored { Mode::Paused } else { Mode::Agent },
                revision: 1,
                in_flight: 0,
                needs_observation: true,
                owner: owner.map(str::to_string),
                closed: false,
            }),
            drained: Notify::new(),
        });
        let snapshot = workspace.snapshot(&workspace.lock()?);
        held.insert(label.to_string(), workspace);
        Ok(snapshot)
    }

    fn get(&self, label: &str) -> Result<Option<Arc<Workspace>>, String> {
        self.held
            .lock()
            .map(|held| held.get(label).cloned())
            .map_err(|_| "작업 브라우저 목록을 확인할 수 없습니다".to_string())
    }

    pub(crate) fn snapshot(&self, label: &str) -> Result<Option<Snapshot>, String> {
        self.get(label)?
            .map(|workspace| Ok(workspace.snapshot(&workspace.lock()?)))
            .transpose()
    }

    pub(crate) fn snapshots(&self) -> Result<Vec<Snapshot>, String> {
        self.held
            .lock()
            .map_err(|_| "작업 브라우저 목록을 확인할 수 없습니다".to_string())?
            .values()
            .map(|workspace| Ok(workspace.snapshot(&workspace.lock()?)))
            .collect()
    }

    pub(crate) fn allows_person(&self, label: &str) -> Result<bool, String> {
        Ok(self
            .snapshot(label)?
            .is_none_or(|snapshot| snapshot.mode == Mode::Human && snapshot.in_flight == 0))
    }

    pub(crate) fn begin(
        &self,
        label: &str,
        actor: Option<&str>,
        verb: &str,
    ) -> Result<Option<Operation>, String> {
        let Some(workspace) = self.get(label)? else {
            return Ok(None);
        };
        let mut state = workspace.lock()?;
        if state.closed {
            return Err("닫힌 작업 브라우저입니다".to_string());
        }
        if state
            .owner
            .as_deref()
            .is_some_and(|owner| Some(owner) != actor)
        {
            return Err(
                "다른 에이전트의 작업 브라우저입니다 — 자신의 작업 탭을 여세요".to_string(),
            );
        }
        let changes_page = !matches!(
            verb,
            "read"
                | "marks"
                | "fields"
                | "screenshot"
                | "console"
                | "network"
                | "diagnose"
                | "wait"
        );
        if changes_page && state.mode != Mode::Agent {
            return Err(
                "작업 브라우저가 일시정지 또는 직접 조작 중입니다 — 자동으로 재시도하지 마세요"
                    .to_string(),
            );
        }
        if state.in_flight != 0 || state.mode == Mode::Pausing {
            return Err("작업 브라우저의 이전 요청이 끝나지 않았습니다".to_string());
        }
        if changes_page && state.needs_observation && !matches!(verb, "goto" | "close" | "viewport")
        {
            return Err("작업 브라우저의 화면을 다시 확인하세요 — 먼저 read, marks 또는 fields를 실행하세요".to_string());
        }
        if state.owner.is_none() && state.mode == Mode::Agent {
            state.owner = actor.map(str::to_string);
        }
        state.in_flight = 1;
        let revision = state.revision;
        drop(state);
        Ok(Some(Operation {
            workspace,
            revision,
            changes_page,
            observes: matches!(verb, "read" | "marks" | "fields" | "screenshot"),
            completed: false,
        }))
    }

    pub(crate) fn transition(
        &self,
        label: &str,
        target: Mode,
        expected_revision: u64,
    ) -> Result<Transition, String> {
        if target == Mode::Pausing {
            return Err("전환 중 상태를 직접 지정할 수 없습니다".to_string());
        }
        let workspace = self
            .get(label)?
            .ok_or("이 탭은 전용 작업 브라우저가 아닙니다")?;
        let mut state = workspace.lock()?;
        if state.closed || state.revision != expected_revision {
            return Err("작업 브라우저 상태가 바뀌었습니다 — 새 상태를 확인하세요".to_string());
        }
        state.revision = state
            .revision
            .checked_add(1)
            .ok_or("제어 상태 번호가 소진됐습니다")?;
        state.mode = Mode::Pausing;
        state.needs_observation = true;
        let revision = state.revision;
        drop(state);
        Ok(Transition {
            workspace,
            revision,
            target,
        })
    }

    pub(crate) fn remove(&self, label: &str) -> Result<(), String> {
        let workspace = self
            .held
            .lock()
            .map_err(|_| "작업 브라우저 목록을 확인할 수 없습니다".to_string())?
            .remove(label);
        if let Some(workspace) = workspace {
            workspace.lock()?.closed = true;
            workspace.drained.notify_waiters();
        }
        Ok(())
    }
}

pub(crate) struct Operation {
    workspace: Arc<Workspace>,
    revision: u64,
    changes_page: bool,
    observes: bool,
    completed: bool,
}

impl Operation {
    pub(crate) fn finish(mut self, succeeded: bool) {
        if let Ok(mut state) = self.workspace.lock() {
            if !state.closed && state.revision == self.revision && state.mode == Mode::Agent {
                if succeeded && self.observes {
                    state.needs_observation = false;
                } else if !succeeded && self.changes_page {
                    state.needs_observation = true;
                }
            }
        }
        self.completed = true;
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        if let Ok(mut state) = self.workspace.lock() {
            state.in_flight = state.in_flight.saturating_sub(1);
            if !self.completed && self.changes_page {
                state.mode = Mode::Paused;
                state.needs_observation = true;
                state.revision = state.revision.saturating_add(1);
            }
        }
        self.workspace.drained.notify_waiters();
    }
}

pub(crate) struct Transition {
    workspace: Arc<Workspace>,
    revision: u64,
    target: Mode,
}

impl Transition {
    pub(crate) fn snapshot(&self) -> Result<Snapshot, String> {
        Ok(self.workspace.snapshot(&self.workspace.lock()?))
    }

    pub(crate) async fn wait(&self) -> Result<(), String> {
        loop {
            let notified = self.workspace.drained.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let state = self.workspace.lock()?;
                if state.closed || state.revision != self.revision || state.mode != Mode::Pausing {
                    return Err("작업 브라우저의 제어권 전환이 취소됐습니다".to_string());
                }
                if state.in_flight == 0 {
                    return Ok(());
                }
            }
            notified.await;
        }
    }

    pub(crate) fn finish(self) -> Result<Snapshot, String> {
        let mut state = self.workspace.lock()?;
        if state.closed
            || state.revision != self.revision
            || state.mode != Mode::Pausing
            || state.in_flight != 0
        {
            return Err("작업 브라우저의 제어권을 아직 넘길 수 없습니다".to_string());
        }
        state.mode = self.target;
        state.revision = state
            .revision
            .checked_add(1)
            .ok_or("제어 상태 번호가 소진됐습니다")?;
        Ok(self.workspace.snapshot(&state))
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
        let transition = workspaces
            .transition("browser-1", target, revision)
            .unwrap();
        transition.wait().await.unwrap();
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
        workspaces
            .begin("browser-1", Some("pane-b"), "read")
            .unwrap()
            .unwrap()
            .finish(true);
        change(&workspaces, Mode::Agent).await;
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
        let transition = workspaces.transition("browser-1", Mode::Human, 1).unwrap();
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
        observe(&workspaces);
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
        observe(&workspaces);
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
