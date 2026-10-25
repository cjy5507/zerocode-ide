use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::{LEDGER_ITSELF, Ledger, Message, MessageKind, Priority, Text};
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
    /// [`Self::worker_session_reported_at`] with no clock — the spelling the
    /// ledger's own scenes write a seat's conversation with. A refusal's
    /// letter is dated at the epoch; the window's road always names its now.
    pub fn worker_session_reported(
        &mut self,
        seat: (&str, &str),
        session: ProviderSession,
    ) -> bool {
        self.worker_session_reported_at(seat, session, 0)
    }
}

/// The `reason` of the ledger's letter that a pane reported a conversation
/// another worker carries on (t-68069) — refused, and told.
pub const CONVERSATION_HELD_ELSEWHERE: &str = "conversation_held_by_another_worker";

impl Ledger {
    /// Tell the coordinator of the worker seated at `seat` that its pane
    /// reported `holder`'s conversation, unless it was told already. Whether
    /// a letter was written.
    fn conversation_held_elsewhere(
        &mut self,
        seat: (&str, &str),
        holder: &str,
        now_ms: i64,
    ) -> bool {
        let (team, pane) = seat;
        let Some((run_at, worker)) = self.runs.iter().enumerate().find_map(|(at, run)| {
            run.workers
                .iter()
                .find(|worker| {
                    worker.team == team && worker.pane == pane && worker.state.may_occupy_pane()
                })
                .map(|worker| (at, worker.clone()))
        }) else {
            return false;
        };
        let body = serde_json::json!({
            "reason": CONVERSATION_HELD_ELSEWHERE,
            "workerId": worker.id,
            "pane": worker.pane,
            "agent": worker.agent,
            "holderId": holder,
        });
        let told = self.runs[run_at].messages.iter().any(|one| {
            one.from == LEDGER_ITSELF
                && one.kind == MessageKind::Status
                && serde_json::from_str::<serde_json::Value>(one.body.as_str()).is_ok_and(|said| {
                    said["reason"] == CONVERSATION_HELD_ELSEWHERE
                        && said["workerId"] == body["workerId"]
                        && said["holderId"] == body["holderId"]
                })
        });
        if told {
            return false;
        }
        let id = self.mint("m-");
        let run = &mut self.runs[run_at];
        let address = run.address();
        let dispatch = worker.dispatch.clone();
        let task = dispatch
            .as_deref()
            .and_then(|id| run.dispatch(id))
            .map(|one| one.task.clone());
        run.messages.push(Message {
            id: id.clone(),
            from: LEDGER_ITSELF.to_string(),
            to: address.clone(),
            kind: MessageKind::Status,
            body: body.to_string().into(),
            subject: Text::from(format!(
                "{}'s pane reported the conversation {holder} carries; refused",
                worker.id
            )),
            priority: Priority::Normal,
            payload: Text::default(),
            thread: None,
            task,
            dispatch,
            author_seat: None,
            created_ms: now_ms,
        });
        run.inbox_mut(&address).pending.push_back(id);
        true
    }

    /// Writes the conversation the pane at `seat` reported onto the worker
    /// seated there, and onto its attempts' histories; whether anything moved.
    ///
    /// One conversation is one worker's (t-68069). A report naming the
    /// conversation ANOTHER row still carries on is refused, whatever seat it
    /// came by: on 2026-10-21 two workers shared one checkout, and a pane
    /// continued after its tab closed ran its neighbour's conversation — two
    /// processes writing one transcript. Taking that report would make the
    /// neighbour's conversation this worker's own, and every later resume of
    /// it would bring the neighbour's back. A pane whose agent was restarted
    /// on a conversation of its own still reports a new id, and is taken.
    ///
    /// A refusal is told to the seated worker's coordinator, once per pair of
    /// workers ([`CONVERSATION_HELD_ELSEWHERE`]): the pane may still be writing
    /// the neighbour's transcript, and that is somebody's to look at. The
    /// letter names the two workers and never the conversation's id.
    pub fn worker_session_reported_at(
        &mut self,
        seat: (&str, &str),
        session: ProviderSession,
        now_ms: i64,
    ) -> bool {
        if !crate::provider_session::is_usable_session_id(&session.id) {
            return false;
        }
        let (team, pane) = seat;
        let seated = |worker: &super::Worker| {
            worker.team == team && worker.pane == pane && worker.state.may_occupy_pane()
        };
        // A conversation is its agent's: two CLIs may spell an id alike.
        let rows = || self.runs.iter().flat_map(|run| run.workers.iter());
        let agent = rows()
            .find(|worker| seated(worker))
            .map(|worker| worker.agent.as_str());
        let holder = rows()
            .find(|worker| {
                !seated(worker)
                    && Some(worker.agent.as_str()) == agent
                    && super::carries_its_conversation_on(worker.state)
                    && worker
                        .session
                        .as_ref()
                        .is_some_and(|held| held.key == session.key && held.id == session.id)
            })
            .map(|worker| worker.id.clone());
        if let Some(holder) = holder {
            return self.conversation_held_elsewhere(seat, &holder, now_ms);
        }
        for run in &mut self.runs {
            let Some(worker) = run.workers.iter_mut().find(|worker| seated(worker)) else {
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
