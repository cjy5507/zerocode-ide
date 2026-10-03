//! Bounded, single-use acknowledgment slots for context actually received by
//! the hook client. A timed-out producer never consumes an advisory.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::{AgentKind, TurnBriefAsk};

const LIMIT: usize = 128;
const TTL: Duration = Duration::from_secs(60);

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Key {
    pub agent: AgentKind,
    pub pane: String,
    pub launch: String,
    pub id: String,
}

impl Key {
    pub fn valid(&self) -> bool {
        !self.pane.is_empty()
            && self.pane.len() <= 256
            && self.launch.len() <= 512
            && !self.id.is_empty()
            && self.id.len() <= 96
            && self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    }
}

#[derive(Default)]
pub(crate) struct Receipts(VecDeque<Slot>);

struct Slot {
    key: Key,
    created: Instant,
    phase: Phase,
}

enum Phase {
    Reserved,
    Ready(TurnBriefAsk, String),
    InFlight,
    Done,
    Ambiguous,
}

impl Receipts {
    pub fn unused(&mut self, key: &Key) {
        self.0
            .retain(|slot| &slot.key != key || !matches!(slot.phase, Phase::Reserved));
    }

    fn expire(&mut self, now: Instant) {
        self.0
            .retain(|slot| now.saturating_duration_since(slot.created) < TTL);
    }

    pub fn reserve(&mut self, key: Key, now: Instant) -> bool {
        self.expire(now);
        if !key.valid() {
            return false;
        }
        if let Some(slot) = self.0.iter_mut().find(|slot| slot.key == key) {
            slot.phase = Phase::Ambiguous;
            return false;
        }
        if self.0.len() == LIMIT {
            return false;
        }
        self.0.push_back(Slot {
            key,
            created: now,
            phase: Phase::Reserved,
        });
        true
    }

    pub fn ready(&mut self, key: &Key, ask: TurnBriefAsk, text: String) -> bool {
        // Bound every retained caller field, not only the final brief.
        if text.chars().count() > crate::TURN_BRIEF_CHAR_CAP
            || ask.agent != key.agent
            || ask.pane_key != key.pane
            || ask.launch_token != key.launch
            || ask.prompt.len() > 16 * 1024
            || ask.worktree.len() > 16 * 1024
            || ask.session_id.as_ref().is_some_and(|id| id.len() > 2_048)
        {
            return false;
        }
        let Some(slot) = self.0.iter_mut().find(|slot| &slot.key == key) else {
            return false;
        };
        if !matches!(slot.phase, Phase::Reserved) {
            return false;
        }
        slot.phase = Phase::Ready(ask, text);
        true
    }

    pub fn take(&mut self, key: &Key, now: Instant) -> Option<(TurnBriefAsk, String)> {
        self.expire(now);
        let slot = self.0.iter_mut().find(|slot| &slot.key == key)?;
        if !matches!(slot.phase, Phase::Ready(..)) {
            return None;
        }
        let Phase::Ready(ask, text) = std::mem::replace(&mut slot.phase, Phase::InFlight) else {
            unreachable!()
        };
        Some((ask, text))
    }

    pub fn finish(&mut self, key: &Key, retry: Option<(TurnBriefAsk, String)>) {
        if let Some(slot) = self.0.iter_mut().find(|slot| &slot.key == key)
            && matches!(slot.phase, Phase::InFlight)
        {
            slot.phase = retry.map_or(Phase::Done, |(ask, text)| Phase::Ready(ask, text));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> Key {
        Key {
            agent: AgentKind::Claude,
            pane: "term-1".into(),
            launch: "launch".into(),
            id: id.into(),
        }
    }

    fn ask() -> TurnBriefAsk {
        TurnBriefAsk {
            agent: AgentKind::Claude,
            pane_key: "term-1".into(),
            launch_token: "launch".into(),
            session_id: Some("session".into()),
            supports_receipts: true,
            worktree: "/work/project".into(),
            prompt: "Continue".into(),
            wall: Duration::from_millis(100),
        }
    }

    #[test]
    fn only_a_ready_exact_client_receipt_can_commit_once() {
        let mut held = Receipts::default();
        let now = Instant::now();
        let ticket = key("one");
        assert!(held.reserve(ticket.clone(), now));
        assert!(held.take(&ticket, now).is_none());
        assert!(held.ready(&ticket, ask(), "context".into()));
        let wrong = Key {
            launch: "another".into(),
            ..ticket.clone()
        };
        assert!(held.take(&wrong, now).is_none());
        let delivered = held.take(&ticket, now).unwrap();
        assert!(held.take(&ticket, now).is_none());
        held.finish(&ticket, Some(delivered));
        assert!(
            held.take(&ticket, now).is_some(),
            "failed storage may retry"
        );
        held.finish(&ticket, None);
        assert!(held.take(&ticket, now).is_none());
        assert!(!held.reserve(ticket, now), "a reused id is ambiguous");
    }

    #[test]
    fn collisions_expiry_and_capacity_do_not_acknowledge_other_replies() {
        let mut held = Receipts::default();
        let now = Instant::now();
        assert!(held.reserve(key("one"), now));
        assert!(!held.reserve(key("one"), now));
        assert!(!held.ready(&key("one"), ask(), "old".into()));
        assert!(held.take(&key("one"), now).is_none());
        for id in 0..LIMIT - 1 {
            assert!(held.reserve(key(&id.to_string()), now));
        }
        assert!(!held.reserve(key("full"), now));
        assert!(held.reserve(key("later"), now + TTL));
        assert_eq!(held.0.len(), 1);
    }
}
