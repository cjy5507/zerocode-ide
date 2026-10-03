use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::Ledger;
use crate::provider_session::ProviderSession;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionHistory {
    pub complete: bool,
    pub sessions: Vec<ProviderSession>,
}

impl SessionHistory {
    #[must_use]
    pub fn from_start() -> Self {
        Self {
            complete: true,
            sessions: Vec::new(),
        }
    }

    #[must_use]
    pub fn continuing_from(session: Option<&ProviderSession>) -> Self {
        Self {
            complete: false,
            sessions: session.into_iter().cloned().collect(),
        }
    }

    #[must_use]
    pub fn contains(&self, session: &ProviderSession) -> bool {
        self.sessions
            .iter()
            .any(|held| held.key == session.key && held.id == session.id)
    }

    pub fn observe(&mut self, session: &ProviderSession) -> bool {
        if let Some(held) = self
            .sessions
            .iter_mut()
            .find(|held| held.key == session.key && held.id == session.id)
        {
            let carried = session.carrying_forward(Some(held));
            if *held == carried {
                return false;
            }
            *held = carried;
        } else {
            self.sessions.push(session.clone());
        }
        true
    }

    #[must_use]
    pub fn valid(&self) -> bool {
        let mut seen = HashSet::new();
        self.sessions.iter().all(|session| {
            crate::provider_session::is_usable_session_id(&session.id)
                && seen.insert((session.key, session.id.as_str()))
        })
    }

    #[must_use]
    pub fn bytes(&self) -> usize {
        self.sessions
            .iter()
            .map(|session| {
                session.id.len() + session.transcript_path.as_ref().map_or(0, String::len)
            })
            .sum()
    }
}

impl Ledger {
    pub fn worker_session_reported(
        &mut self,
        seat: (&str, &str),
        session: ProviderSession,
    ) -> bool {
        if !crate::provider_session::is_usable_session_id(&session.id) {
            return false;
        }
        let (team, pane) = seat;
        for run in &mut self.runs {
            let Some(worker) = run.workers.iter_mut().find(|worker| {
                worker.team == team && worker.pane == pane && worker.state.may_occupy_pane()
            }) else {
                continue;
            };
            let carried = session.carrying_forward(worker.session.as_ref());
            let mut moved = worker.session.as_ref() != Some(&carried);
            for dispatch in run
                .dispatches
                .iter_mut()
                .filter(|dispatch| dispatch.worker == worker.id)
            {
                if dispatch.session_history.is_none() {
                    dispatch.session_history =
                        Some(SessionHistory::continuing_from(worker.session.as_ref()));
                    moved = true;
                }
                if worker.dispatch.as_deref() == Some(dispatch.id.as_str()) && dispatch.is_open() {
                    if let Some(history) = dispatch.session_history.as_mut() {
                        moved |= history.observe(&carried);
                    }
                } else if let Some(history) = dispatch.session_history.as_mut()
                    && history.contains(&carried)
                {
                    moved |= history.observe(&carried);
                }
            }
            if moved {
                worker.session = Some(carried);
            }
            return moved;
        }
        false
    }
}
