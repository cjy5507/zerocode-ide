//! What one pane looks like on the phone, and when the phone is told.
//!
//! The projection keeps four facts — which agent, which checkout (by its
//! folder name, as the window's own notices call it), what it waits on, since
//! when — and drops everything a pane holds besides: the full path, the
//! command line, its environment, the agent's words and the screen.

use std::collections::HashMap;

use serde::Serialize;
use zerocode_core::board::Bucket;

use super::server::PaneView;

/// Everything the window hands over about one pane; most of it stays here.
#[derive(Debug, Clone, Default)]
pub(crate) struct RawPane<'a> {
    pub(crate) term: u32,
    pub(crate) agent: &'a str,
    pub(crate) state: &'a str,
    pub(crate) at: i64,
    pub(crate) state_started_at: i64,
    pub(crate) worktree: Option<&'a str>,
    pub(crate) task_id: Option<&'a str>,
    pub(crate) approval: bool,
}

/// The last segment of a checkout path — the word a window notice already
/// uses for it.
pub(crate) fn place_of(worktree: &str) -> String {
    worktree
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A task id the ledger minted (`t-123`), or nothing: free text from a
/// briefing never reaches the phone.
fn task_word(task_id: Option<&str>) -> Option<String> {
    let id = task_id?.trim();
    let digits = id.strip_prefix("t-")?;
    (!digits.is_empty() && digits.len() <= 12 && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| id.to_string())
}

pub(crate) fn project(raw: &RawPane<'_>) -> Option<PaneView> {
    let bucket = Bucket::of_word(raw.state);
    let group = match bucket {
        Bucket::Attention => "attention",
        Bucket::Working => "working",
        Bucket::Done => "done",
        Bucket::Idle => return None,
    };
    let name = raw.worktree.map(place_of).unwrap_or_default();
    let name = if name.is_empty() {
        raw.agent.to_string()
    } else {
        name
    };
    Some(PaneView {
        id: format!("term-{}", raw.term),
        agent: raw.agent.to_string(),
        name,
        task: task_word(raw.task_id),
        group,
        waiting: (bucket == Bucket::Attention).then_some(if raw.approval {
            "permission"
        } else {
            "question"
        }),
        since_ms: if raw.state_started_at > 0 {
            raw.state_started_at
        } else {
            raw.at
        },
    })
}

/// Rings from one pane inside this long fold into the next notice.
pub(crate) const FOLD_MS: i64 = 2 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Send now; `more` rings from this pane were folded since the last one.
    Send { more: u32 },
    /// A notice for this pane went out moments ago; this one is counted.
    Fold,
    /// The person is at the window: the phone stays quiet.
    Present,
}

/// When a phone is told: never while the person is at the window, and once
/// per pane per [`FOLD_MS`] — the push service's `Topic` replaces an
/// undelivered notice of the same pane, and this counts the ones in between.
#[derive(Debug, Default)]
pub(crate) struct Gate {
    panes: HashMap<u32, (i64, u32)>,
}

impl Gate {
    pub(crate) fn judge(&mut self, term: u32, present: bool, now_ms: i64) -> Verdict {
        if present {
            return Verdict::Present;
        }
        match self.panes.get_mut(&term) {
            Some((sent, folded)) if now_ms.saturating_sub(*sent) < FOLD_MS => {
                *folded += 1;
                Verdict::Fold
            }
            Some((sent, folded)) => {
                let more = *folded;
                *sent = now_ms;
                *folded = 0;
                Verdict::Send { more }
            }
            None => {
                self.panes.insert(term, (now_ms, 0));
                Verdict::Send { more: 0 }
            }
        }
    }
}

/// The notice's payload. The phone's service worker words it in the
/// phone's own language; no sentence is composed here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Notice {
    pub(crate) v: u8,
    pub(crate) kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pane: Option<String>,
    pub(crate) more: u32,
}

impl Notice {
    pub(crate) fn for_pane(view: &PaneView, more: u32) -> Self {
        Self {
            v: 1,
            kind: view.waiting.unwrap_or("question"),
            agent: Some(view.agent.clone()),
            name: Some(view.name.clone()),
            task: view.task.clone(),
            pane: Some(view.id.clone()),
            more,
        }
    }

    pub(crate) fn test() -> Self {
        Self {
            v: 1,
            kind: "test",
            agent: None,
            name: None,
            task: None,
            pane: None,
            more: 0,
        }
    }

    /// RFC 8030 `Topic`: one per pane, so the push service keeps only the
    /// newest undelivered notice of it.
    pub(crate) fn topic(&self) -> String {
        self.pane.as_deref().map_or_else(
            || "zerocode-test".to_string(),
            |pane| format!("zerocode-{pane}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw<'a>(state: &'a str) -> RawPane<'a> {
        RawPane {
            term: 7,
            agent: "claude",
            state,
            at: 50,
            state_started_at: 40,
            worktree: Some("/srv/fixture/secret-client/checkout-a"),
            task_id: Some("t-123"),
            approval: false,
        }
    }

    #[test]
    fn a_pane_keeps_only_the_folder_name_and_never_the_path() {
        let view = project(&raw("waiting")).expect("attention");
        assert_eq!(view.name, "checkout-a");
        let text = serde_json::to_string(&view).unwrap();
        assert!(!text.contains("/srv"));
        assert!(!text.contains("secret-client"));
        assert!(!text.contains('/'));
        assert_eq!(view.group, "attention");
        assert_eq!(view.waiting, Some("question"));
        assert_eq!(view.since_ms, 40);
        assert_eq!(view.task.as_deref(), Some("t-123"));
    }

    #[test]
    fn a_windows_path_is_cut_the_same_way() {
        let mut pane = raw("working");
        pane.worktree = Some(r"C:\work\fixture\checkout-b\");
        assert_eq!(project(&pane).unwrap().name, "checkout-b");
    }

    #[test]
    fn a_permission_wait_is_named_as_one() {
        let mut pane = raw("awaiting-permission");
        pane.approval = true;
        assert_eq!(project(&pane).unwrap().waiting, Some("permission"));
    }

    #[test]
    fn idle_panes_are_left_off_and_finished_ones_kept() {
        assert!(project(&raw("idle")).is_none());
        assert_eq!(project(&raw("done")).unwrap().group, "done");
        assert_eq!(project(&raw("working")).unwrap().waiting, None);
    }

    #[test]
    fn free_text_never_passes_as_a_task() {
        let mut pane = raw("working");
        pane.task_id = Some("deploy /etc/passwd");
        assert_eq!(project(&pane).unwrap().task, None);
        pane.task_id = Some("t-12a");
        assert_eq!(project(&pane).unwrap().task, None);
    }

    #[test]
    fn rings_from_one_pane_fold_and_the_next_notice_counts_them() {
        let mut gate = Gate::default();
        assert_eq!(gate.judge(1, false, 0), Verdict::Send { more: 0 });
        assert_eq!(gate.judge(1, false, 1_000), Verdict::Fold);
        assert_eq!(gate.judge(1, false, 2_000), Verdict::Fold);
        assert_eq!(
            gate.judge(2, false, 2_500),
            Verdict::Send { more: 0 },
            "another pane is its own"
        );
        assert_eq!(gate.judge(1, false, FOLD_MS + 1), Verdict::Send { more: 2 });
        assert_eq!(gate.judge(1, false, FOLD_MS + 2), Verdict::Fold);
    }

    #[test]
    fn nothing_is_sent_while_the_person_is_at_the_window() {
        let mut gate = Gate::default();
        assert_eq!(gate.judge(1, true, 0), Verdict::Present);
        assert_eq!(
            gate.judge(1, false, 1),
            Verdict::Send { more: 0 },
            "a quiet ring is not counted as sent"
        );
    }

    #[test]
    fn the_notice_carries_names_only() {
        let view = project(&raw("waiting")).unwrap();
        let notice = Notice::for_pane(&view, 3);
        let text = serde_json::to_string(&notice).unwrap();
        assert!(!text.contains('/'));
        assert_eq!(notice.topic(), "zerocode-term-7");
        assert!(notice.topic().len() <= 32);
        assert_eq!(Notice::test().kind, "test");
    }
}
