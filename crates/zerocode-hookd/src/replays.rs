//! A hook post tried again is answered once (t-78634).
//!
//! A script that stopped waiting (curl 28, 52, 56) may have left its body
//! with the bridge, so it posts again under the same delivery id. The first
//! post's work runs to its end whether its client is still there or not; a
//! post again under that id waits for the first one's answer and is given
//! it, so the window is handed the event once and the agent the reply once.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use tokio::sync::watch;

use crate::brief_receipts::Key;

/// More deliveries than every pane of a window starts while an id is
/// remembered.
const LIMIT: usize = 256;

/// How long an id is remembered — well past [`crate::HOOK_DELIVERY_BUDGET`]
/// and every agent's own hook timeout. The budget bounds when the last try
/// starts, not when it lands: a busy machine spends more on starting curl and
/// sleep, and the first claim came a little after the script began, so an
/// id remembered for the budget alone was forgotten under its last try and
/// the window was handed the event twice. Kept as long as a brief receipt.
const REMEMBERED_FOR: Duration = Duration::from_secs(60);

/// What the bridge answered a hook post — kept whole so a post again can be
/// given the same.
#[derive(Debug, Clone)]
pub(crate) enum HookAnswer {
    Status(StatusCode),
    Said(StatusCode, &'static str),
    Json(StatusCode, serde_json::Value),
}

impl IntoResponse for HookAnswer {
    fn into_response(self) -> Response {
        match self {
            Self::Status(status) => status.into_response(),
            Self::Said(status, words) => (status, words).into_response(),
            Self::Json(status, body) => (status, axum::Json(body)).into_response(),
        }
    }
}

type Answer = watch::Receiver<Option<HookAnswer>>;

/// Whose post this is.
pub(crate) enum Claim {
    /// The first under its id: it does the work and leaves its answer here.
    First(watch::Sender<Option<HookAnswer>>),
    /// A post again: the first one's answer, when it has one.
    Again(Answer),
}

#[derive(Default)]
pub(crate) struct Replays(VecDeque<Entry>);

struct Entry {
    key: Key,
    created: Instant,
    answer: Answer,
}

impl Replays {
    /// Claim `key` — a valid one ([`Key::valid`]). An id older than
    /// [`REMEMBERED_FOR`] cannot be tried again and is forgotten; past the
    /// limit the oldest goes, which costs it only its replay.
    pub fn claim(&mut self, key: Key, now: Instant) -> Claim {
        self.0
            .retain(|entry| now.saturating_duration_since(entry.created) < REMEMBERED_FOR);
        if let Some(entry) = self.0.iter().find(|entry| entry.key == key) {
            return Claim::Again(entry.answer.clone());
        }
        if self.0.len() == LIMIT {
            self.0.pop_front();
        }
        let (answer, waiting) = watch::channel(None);
        self.0.push_back(Entry {
            key,
            created: now,
            answer: waiting,
        });
        Claim::First(answer)
    }
}

/// The first post's answer for a post again. A first post whose work died
/// left no answer, and nothing went to the window after it: 503, which the
/// script reads as the refusal it is.
pub(crate) async fn replay(mut answer: Answer) -> Response {
    let given = answer
        .wait_for(Option::is_some)
        .await
        .ok()
        .and_then(|answer| answer.clone());
    given.map_or_else(
        || StatusCode::SERVICE_UNAVAILABLE.into_response(),
        IntoResponse::into_response,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str) -> Key {
        Key {
            agent: zerocode_core::AgentKind::Claude,
            pane: "term-1".into(),
            launch: "launch".into(),
            id: id.into(),
        }
    }

    #[test]
    fn a_post_again_waits_on_the_first_and_an_old_id_is_forgotten() {
        let mut replays = Replays::default();
        let now = Instant::now();
        assert!(matches!(replays.claim(key("p-1"), now), Claim::First(_)));
        assert!(matches!(replays.claim(key("p-1"), now), Claim::Again(_)));
        assert!(matches!(replays.claim(key("p-2"), now), Claim::First(_)));
        let later = now + REMEMBERED_FOR;
        assert!(matches!(replays.claim(key("p-1"), later), Claim::First(_)));
    }

    /// The last try lands at the end of the budget or, on a busy machine,
    /// after it — still a replay, never a second event for the window.
    #[test]
    fn the_last_try_landing_at_or_past_the_budget_is_still_a_replay() {
        for agent in [
            zerocode_core::AgentKind::Claude,
            zerocode_core::AgentKind::Codex,
            zerocode_core::AgentKind::Copilot,
        ] {
            assert!(crate::install::hook_timeout(agent) < REMEMBERED_FOR);
        }
        let mut replays = Replays::default();
        let now = Instant::now();
        assert!(matches!(replays.claim(key("p-1"), now), Claim::First(_)));
        for late in [
            crate::HOOK_DELIVERY_BUDGET,
            crate::HOOK_DELIVERY_BUDGET + Duration::from_millis(500),
            crate::install::hook_timeout(zerocode_core::AgentKind::Claude),
        ] {
            assert!(
                matches!(replays.claim(key("p-1"), now + late), Claim::Again(_)),
                "{late:?}"
            );
        }
        let just_before = now + REMEMBERED_FOR - Duration::from_millis(1);
        assert!(matches!(
            replays.claim(key("p-1"), just_before),
            Claim::Again(_)
        ));
    }

    #[test]
    fn the_oldest_goes_past_the_limit() {
        let mut replays = Replays::default();
        let now = Instant::now();
        for number in 0..=LIMIT {
            let _ = replays.claim(key(&format!("p-{number}")), now);
        }
        assert_eq!(replays.0.len(), LIMIT);
        assert!(matches!(replays.claim(key("p-0"), now), Claim::First(_)));
    }
}
