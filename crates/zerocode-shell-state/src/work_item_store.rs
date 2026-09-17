//! Durable worktree-to-work-item links and the Jira synchronization outbox.
//!
//! The Jira credential store owns secrets and site lifecycle. This module owns
//! only non-secret worktree metadata. It uses the settings repository so links
//! inherit the same revision, backup, quarantine, atomic-replace and
//! cross-process locking guarantees as the rest of the typed configuration.

use serde::{Deserialize, Serialize};
use zerocode_core::workitem::{
    JiraSyncActionState, JiraSyncEvent, JiraSyncTrigger, LinkedWorkItem, jira_sync_event_id,
};

use crate::settings::SettingsRepository;
use crate::settings_file;

const SCHEMA_VERSION: u16 = 1;
const LINK_LIMIT: usize = 2_048;
const EVENT_LIMIT: usize = 1_024;

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub revision: u64,
    /// Whether outbound Jira writes are held. While held, pending events keep
    /// their place and nothing is sent — a person resumes explicitly.
    pub paused: bool,
    pub links: Vec<LinkedWorkItem>,
    pub sync_events: Vec<JiraSyncEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Document {
    schema_version: u16,
    /// Held by default so a freshly installed outbox — or one an older build
    /// wrote without this field — never posts to Jira until a person resumes.
    #[serde(default = "held")]
    paused: bool,
    #[serde(default)]
    links: Vec<LinkedWorkItem>,
    #[serde(default)]
    sync_events: Vec<JiraSyncEvent>,
}

const fn held() -> bool {
    true
}

impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            paused: held(),
            links: Vec::new(),
            sync_events: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum SyncAction {
    Transition,
    Comment,
}

pub fn report(settings: &SettingsRepository) -> Result<Report, String> {
    let read = settings
        .read_json::<Document>(settings_file::WORK_ITEM_LINKS)
        .map_err(|error| error.to_string())?;
    let document = read.value.unwrap_or_default();
    validate_document(&document)?;
    Ok(Report {
        revision: read.revision,
        paused: document.paused,
        links: document.links,
        sync_events: document.sync_events,
    })
}

/// Hold or resume outbound Jira writes. Returns the value now in force.
///
/// Resuming does not itself send anything; the caller drains what is pending.
pub fn set_paused(settings: &SettingsRepository, paused: bool) -> Result<bool, String> {
    mutate(settings, |document| {
        document.paused = paused;
        Ok(paused)
    })
}

/// Every event with an action still awaiting its first attempt. Draining these
/// on boot recovers a crash between enqueue and the network without any risk of
/// duplication — a `Pending` action was, by definition, never sent.
pub fn pending_event_ids(settings: &SettingsRepository) -> Result<Vec<String>, String> {
    Ok(report(settings)?
        .sync_events
        .into_iter()
        .filter(JiraSyncEvent::is_pending)
        .map(|event| event.event_id)
        .collect())
}

/// Re-arm the failed actions of one event so a person can retry them. Only
/// `Refused`/`Unknown` actions return to `Pending`; `Applied` stays applied and
/// `NotRequested` stays unasked. Returns the event when anything was re-armed.
pub fn requeue_failed(
    settings: &SettingsRepository,
    event_id: &str,
    now_ms: i64,
) -> Result<Option<JiraSyncEvent>, String> {
    mutate(settings, |document| {
        let Some(event) = document
            .sync_events
            .iter_mut()
            .find(|event| event.event_id == event_id)
        else {
            return Ok(None);
        };
        let mut rearmed = false;
        if event.transition.is_retryable() {
            event.transition = JiraSyncActionState::Pending;
            rearmed = true;
        }
        if event.comment.is_retryable() {
            event.comment = JiraSyncActionState::Pending;
            rearmed = true;
        }
        if !rearmed {
            return Ok(None);
        }
        event.updated_at_ms = event.updated_at_ms.max(now_ms);
        event.validate()?;
        Ok(Some(event.clone()))
    })
}

pub fn link_for_worktree(
    settings: &SettingsRepository,
    worktree_id: &str,
) -> Result<Option<LinkedWorkItem>, String> {
    Ok(report(settings)?
        .links
        .into_iter()
        .find(|link| link.worktree_id == worktree_id))
}

pub fn put(settings: &SettingsRepository, link: LinkedWorkItem) -> Result<LinkedWorkItem, String> {
    link.validate()?;
    mutate(settings, |document| {
        if let Some(existing) = document
            .links
            .iter_mut()
            .find(|held| held.worktree_id == link.worktree_id)
        {
            *existing = link.clone();
        } else {
            if document.links.len() >= LINK_LIMIT {
                return Err("work-item link limit reached".to_string());
            }
            document.links.push(link.clone());
        }
        Ok(link)
    })
}

pub fn remove(settings: &SettingsRepository, worktree_id: &str) -> Result<bool, String> {
    mutate(settings, |document| {
        let before = document.links.len();
        let removed_ids: Vec<String> = document
            .links
            .iter()
            .filter(|link| link.worktree_id == worktree_id)
            .map(|link| link.link_id.clone())
            .collect();
        document
            .links
            .retain(|link| link.worktree_id != worktree_id);
        document
            .sync_events
            .retain(|event| !removed_ids.iter().any(|id| id == &event.link_id));
        Ok(document.links.len() != before)
    })
}

pub fn observe_orchestration(
    settings: &SettingsRepository,
    worktree_id: &str,
    run: &str,
    task: Option<&str>,
    dispatch: Option<&str>,
    now_ms: i64,
) -> Result<Option<LinkedWorkItem>, String> {
    mutate(settings, |document| {
        let Some(link) = document
            .links
            .iter_mut()
            .find(|link| link.worktree_id == worktree_id)
        else {
            return Ok(None);
        };
        link.orchestration.observe(run, task, dispatch);
        link.updated_at_ms = link.updated_at_ms.max(now_ms);
        link.validate()?;
        Ok(Some(link.clone()))
    })
}

pub fn enqueue_started(
    settings: &SettingsRepository,
    worktree_id: &str,
    now_ms: i64,
) -> Result<Option<String>, String> {
    mutate(settings, |document| {
        let Some(link) = document
            .links
            .iter()
            .find(|link| link.worktree_id == worktree_id)
            .cloned()
        else {
            return Ok(None);
        };
        let Some(transition_id) = link.sync.start_transition_id.clone() else {
            return Ok(None);
        };
        let source_id = format!("worktree-start-{}", link.link_id);
        let event_id = jira_sync_event_id(&link.link_id, &source_id);
        if document
            .sync_events
            .iter()
            .any(|event| event.event_id == event_id)
        {
            return Ok(Some(event_id));
        }
        push_event(
            document,
            JiraSyncEvent {
                event_id: event_id.clone(),
                link_id: link.link_id,
                trigger: JiraSyncTrigger::WorkStarted,
                source_id,
                succeeded: None,
                summary: String::new(),
                transition_id: Some(transition_id),
                transition: JiraSyncActionState::Pending,
                comment: JiraSyncActionState::NotRequested,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            },
        )?;
        Ok(Some(event_id))
    })
}

/// Enqueue the completion sync for one orchestration run, at most once per run.
///
/// A run may report `worker_done` many times — once per worker — but the Jira
/// issue owns a single lifecycle, so the event id is scoped to the run, not the
/// message. The first completion to arrive wins and settles the issue; later
/// reports in the same run find that event and change nothing. In the common
/// case a person cares about — one worktree taken from an issue — there is
/// exactly one completion, so first-wins is also last-wins.
pub fn enqueue_worker_done(
    settings: &SettingsRepository,
    worktree_id: Option<&str>,
    dispatch_id: Option<&str>,
    run_id: &str,
    succeeded: bool,
    summary: &str,
    now_ms: i64,
) -> Result<Option<String>, String> {
    mutate(settings, |document| {
        let link = document
            .links
            .iter()
            .find(|link| {
                dispatch_id.is_some_and(|dispatch| {
                    link.orchestration
                        .dispatch_ids
                        .iter()
                        .any(|held| held == dispatch)
                })
            })
            .or_else(|| {
                worktree_id.and_then(|worktree| {
                    document
                        .links
                        .iter()
                        .find(|link| link.worktree_id == worktree)
                })
            })
            .cloned();
        let Some(link) = link else { return Ok(None) };
        let transition_id = match succeeded {
            true => link.sync.success_transition_id.clone(),
            false => link.sync.failure_transition_id.clone(),
        };
        let comment_requested = match succeeded {
            true => link.sync.comment_on_success,
            false => link.sync.comment_on_failure,
        };
        if transition_id.is_none() && !comment_requested {
            return Ok(None);
        }
        let source_id = format!("run-complete-{run_id}");
        let event_id = jira_sync_event_id(&link.link_id, &source_id);
        if document
            .sync_events
            .iter()
            .any(|event| event.event_id == event_id)
        {
            return Ok(Some(event_id));
        }
        push_event(
            document,
            JiraSyncEvent {
                event_id: event_id.clone(),
                link_id: link.link_id,
                trigger: JiraSyncTrigger::WorkerDone,
                source_id,
                succeeded: Some(succeeded),
                summary: bounded_summary(summary),
                transition_id: transition_id.clone(),
                transition: if transition_id.is_some() {
                    JiraSyncActionState::Pending
                } else {
                    JiraSyncActionState::NotRequested
                },
                comment: if comment_requested {
                    JiraSyncActionState::Pending
                } else {
                    JiraSyncActionState::NotRequested
                },
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            },
        )?;
        Ok(Some(event_id))
    })
}

pub fn pending_event(
    settings: &SettingsRepository,
    event_id: &str,
) -> Result<Option<(LinkedWorkItem, JiraSyncEvent)>, String> {
    let report = report(settings)?;
    let Some(event) = report
        .sync_events
        .into_iter()
        .find(|event| event.event_id == event_id)
    else {
        return Ok(None);
    };
    if !event.is_pending() {
        return Ok(None);
    }
    let link = report
        .links
        .into_iter()
        .find(|link| link.link_id == event.link_id)
        .ok_or_else(|| "Jira sync event has no linked work item".to_string())?;
    Ok(Some((link, event)))
}

pub fn settle_action(
    settings: &SettingsRepository,
    event_id: &str,
    action: SyncAction,
    state: JiraSyncActionState,
    now_ms: i64,
) -> Result<JiraSyncEvent, String> {
    mutate(settings, |document| {
        let event = document
            .sync_events
            .iter_mut()
            .find(|event| event.event_id == event_id)
            .ok_or_else(|| format!("unknown Jira sync event: {event_id}"))?;
        match action {
            SyncAction::Transition => event.transition = state,
            SyncAction::Comment => event.comment = state,
        }
        event.updated_at_ms = event.updated_at_ms.max(now_ms);
        event.validate()?;
        Ok(event.clone())
    })
}

fn mutate<R>(
    settings: &SettingsRepository,
    change: impl FnOnce(&mut Document) -> Result<R, String>,
) -> Result<R, String> {
    let committed = settings
        .try_mutate_json(
            settings_file::WORK_ITEM_LINKS,
            Document::default,
            |document| {
                validate_document(document)?;
                let result = change(document)?;
                validate_document(document)?;
                Ok::<R, String>(result)
            },
        )
        .map_err(|error| error.to_string())??;
    Ok(committed.result)
}

fn push_event(document: &mut Document, event: JiraSyncEvent) -> Result<(), String> {
    event.validate()?;
    while document.sync_events.len() >= EVENT_LIMIT {
        // Forget a fully-settled event first, then an unretried Unknown/Refused,
        // and only as an absolute last resort — an outbox wholly of never-sent
        // Pending, the trajectory of a long-held sync — the oldest of any kind.
        // The outbox is a bounded ring: it sheds the stalest fact so the newest
        // is never lost to a hard error.
        let at = document
            .sync_events
            .iter()
            .position(prunable_event)
            .or_else(|| oldest_settled(&document.sync_events))
            .or_else(|| oldest_created(&document.sync_events));
        let Some(at) = at else { break };
        document.sync_events.remove(at);
    }
    document.sync_events.push(event);
    Ok(())
}

/// Fully finished and nothing a person might retry — safe to forget first.
fn prunable_event(event: &JiraSyncEvent) -> bool {
    !event.is_pending() && !event.has_retryable()
}

/// Next resort when the outbox is full of unretried `Unknown`/`Refused` rows:
/// forget the oldest settled one so a fresh lifecycle fact is never dropped for
/// want of room. Pending events — never yet attempted — are kept over these.
fn oldest_settled(events: &[JiraSyncEvent]) -> Option<usize> {
    events
        .iter()
        .enumerate()
        .filter(|(_, event)| !event.is_pending())
        .min_by_key(|(_, event)| event.created_at_ms)
        .map(|(index, _)| index)
}

/// Absolute last resort: the oldest event of any kind, `Pending` included. Used
/// only when the outbox is entirely never-sent `Pending` and a new fact must
/// fit — the ring sheds its stalest unsent intent rather than the newest one.
fn oldest_created(events: &[JiraSyncEvent]) -> Option<usize> {
    events
        .iter()
        .enumerate()
        .min_by_key(|(_, event)| event.created_at_ms)
        .map(|(index, _)| index)
}

fn validate_document(document: &Document) -> Result<(), String> {
    if document.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "unsupported work-item link schema: {}",
            document.schema_version
        ));
    }
    if document.links.len() > LINK_LIMIT || document.sync_events.len() > EVENT_LIMIT {
        return Err("work-item link document exceeds its bounds".to_string());
    }
    let mut link_ids = std::collections::HashSet::new();
    let mut worktree_ids = std::collections::HashSet::new();
    for link in &document.links {
        link.validate()?;
        if !link_ids.insert(link.link_id.as_str())
            || !worktree_ids.insert(link.worktree_id.as_str())
        {
            return Err("work-item link document contains duplicate identities".to_string());
        }
    }
    let mut event_ids = std::collections::HashSet::new();
    for event in &document.sync_events {
        event.validate()?;
        if !event_ids.insert(event.event_id.as_str()) {
            return Err("work-item link document contains duplicate sync events".to_string());
        }
        if !link_ids.contains(event.link_id.as_str()) {
            return Err("Jira sync event references a missing link".to_string());
        }
    }
    Ok(())
}

fn bounded_summary(summary: &str) -> String {
    let mut summary = summary.replace('\0', " ");
    if summary.len() <= 8_192 {
        return summary;
    }
    let mut boundary = 8_192;
    while boundary > 0 && !summary.is_char_boundary(boundary) {
        boundary -= 1;
    }
    summary.truncate(boundary);
    summary
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use zerocode_core::workitem::{
        JiraSyncPolicy, LinkedOrchestration, LinkedWorkItemSource, jira_link_id,
    };

    fn link(root: &Path, at: i64) -> LinkedWorkItem {
        let worktree_id = "worktree-1".to_string();
        let site_id = "site-1".to_string();
        let key = "ABC-9".to_string();
        LinkedWorkItem {
            link_id: jira_link_id(&worktree_id, &site_id, &key),
            repository_id: "repo-1".to_string(),
            worktree_id,
            worktree_path: root.to_string_lossy().into_owned(),
            source: LinkedWorkItemSource::Jira {
                site_id,
                key,
                url: "https://acme.atlassian.net/browse/ABC-9".to_string(),
                title: "Fix login".to_string(),
                status: "To Do".to_string(),
                updated: None,
            },
            sync: JiraSyncPolicy {
                start_transition_id: Some("11".to_string()),
                success_transition_id: Some("31".to_string()),
                failure_transition_id: None,
                comment_on_success: true,
                comment_on_failure: false,
            },
            orchestration: LinkedOrchestration::default(),
            linked_at_ms: at,
            updated_at_ms: at,
        }
    }

    #[test]
    fn one_worktree_has_one_link_and_unknown_sync_is_never_pruned() {
        let root = tempfile::tempdir().expect("tempdir");
        let settings = SettingsRepository::new(root.path());
        let first = put(&settings, link(root.path(), 1)).expect("first link");
        let replacement = put(&settings, link(root.path(), 2)).expect("replacement");
        assert_eq!(first.link_id, replacement.link_id);
        assert_eq!(report(&settings).expect("report").links.len(), 1);

        let event = enqueue_started(&settings, &replacement.worktree_id, 3)
            .expect("enqueue")
            .expect("event");
        settle_action(
            &settings,
            &event,
            SyncAction::Transition,
            JiraSyncActionState::Unknown("connection lost".to_string()),
            4,
        )
        .expect("settle");
        let saved = report(&settings).expect("saved");
        assert!(matches!(
            saved.sync_events[0].transition,
            JiraSyncActionState::Unknown(_)
        ));
    }

    #[test]
    fn worker_done_is_idempotent_and_finds_the_link_by_dispatch() {
        let root = tempfile::tempdir().expect("tempdir");
        let settings = SettingsRepository::new(root.path());
        let held = put(&settings, link(root.path(), 1)).expect("link");
        observe_orchestration(
            &settings,
            &held.worktree_id,
            "run-1",
            Some("task-1"),
            Some("dispatch-1"),
            2,
        )
        .expect("observe");
        let first = enqueue_worker_done(
            &settings,
            None,
            Some("dispatch-1"),
            "run-1",
            true,
            "done",
            3,
        )
        .expect("enqueue")
        .expect("event");
        let replay = enqueue_worker_done(
            &settings,
            None,
            Some("dispatch-1"),
            "run-1",
            true,
            "done",
            4,
        )
        .expect("replay")
        .expect("event");
        assert_eq!(first, replay);
        assert_eq!(report(&settings).expect("report").sync_events.len(), 1);
    }

    #[test]
    fn one_run_settles_the_issue_once_even_across_many_workers() {
        let root = tempfile::tempdir().expect("tempdir");
        let settings = SettingsRepository::new(root.path());
        let held = put(&settings, link(root.path(), 1)).expect("link");
        observe_orchestration(
            &settings,
            &held.worktree_id,
            "run-7",
            Some("task-1"),
            Some("dispatch-a"),
            2,
        )
        .expect("observe");
        observe_orchestration(
            &settings,
            &held.worktree_id,
            "run-7",
            Some("task-1"),
            Some("dispatch-b"),
            2,
        )
        .expect("observe");
        // Two different workers of the same run each report done. The issue
        // has one lifecycle, so only the first completion is enqueued.
        let first = enqueue_worker_done(
            &settings,
            None,
            Some("dispatch-a"),
            "run-7",
            true,
            "worker a done",
            3,
        )
        .expect("enqueue a")
        .expect("event");
        let second = enqueue_worker_done(
            &settings,
            None,
            Some("dispatch-b"),
            "run-7",
            true,
            "worker b done",
            4,
        )
        .expect("enqueue b")
        .expect("event");
        assert_eq!(first, second, "same run must collapse to one event");
        let saved = report(&settings).expect("report");
        assert_eq!(saved.sync_events.len(), 1);
        assert_eq!(saved.sync_events[0].summary, "worker a done");
    }

    #[test]
    fn the_outbox_is_held_until_a_person_resumes() {
        let root = tempfile::tempdir().expect("tempdir");
        let settings = SettingsRepository::new(root.path());
        // A fresh store holds every outbound write by default.
        assert!(report(&settings).expect("report").paused);
        put(&settings, link(root.path(), 1)).expect("link");
        let event = enqueue_started(&settings, "worktree-1", 3)
            .expect("enqueue")
            .expect("event");
        // Pending while held — boot drain can find it, but nothing is sent.
        assert_eq!(pending_event_ids(&settings).expect("pending"), vec![event]);
        assert!(!set_paused(&settings, false).expect("resume"));
        assert!(!report(&settings).expect("report").paused);
        assert!(set_paused(&settings, true).expect("hold"));
    }

    #[test]
    fn a_person_can_re_arm_a_failed_action_without_touching_the_settled_one() {
        let root = tempfile::tempdir().expect("tempdir");
        let settings = SettingsRepository::new(root.path());
        put(&settings, link(root.path(), 1)).expect("link");
        let event = enqueue_worker_done(
            &settings,
            Some("worktree-1"),
            None,
            "run-9",
            true,
            "done",
            3,
        )
        .expect("enqueue")
        .expect("event");
        // Transition lost to the network, comment already applied.
        settle_action(
            &settings,
            &event,
            SyncAction::Transition,
            JiraSyncActionState::Unknown("timeout".to_string()),
            4,
        )
        .expect("settle transition");
        settle_action(
            &settings,
            &event,
            SyncAction::Comment,
            JiraSyncActionState::Applied,
            4,
        )
        .expect("settle comment");
        let rearmed = requeue_failed(&settings, &event, 5)
            .expect("requeue")
            .expect("event");
        assert!(rearmed.transition.is_pending(), "unknown must re-arm");
        assert_eq!(
            rearmed.comment,
            JiraSyncActionState::Applied,
            "applied must not be disturbed"
        );
        // Nothing left to re-arm the second time.
        assert!(
            requeue_failed(&settings, &event, 6)
                .expect("requeue")
                .is_none()
        );
    }

    #[test]
    fn a_full_outbox_of_unknowns_still_admits_a_fresh_event() {
        let root = tempfile::tempdir().expect("tempdir");
        let settings = SettingsRepository::new(root.path());
        put(&settings, link(root.path(), 1)).expect("link");
        let held = link(root.path(), 1);
        mutate(&settings, |document| {
            // Fill the outbox with old, unretried Unknown transitions.
            for index in 0..EVENT_LIMIT {
                document.sync_events.push(JiraSyncEvent {
                    event_id: format!("event-{index}"),
                    link_id: held.link_id.clone(),
                    trigger: JiraSyncTrigger::WorkStarted,
                    source_id: format!("source-{index}"),
                    succeeded: None,
                    summary: String::new(),
                    transition_id: Some("11".to_string()),
                    transition: JiraSyncActionState::Unknown("lost".to_string()),
                    comment: JiraSyncActionState::NotRequested,
                    created_at_ms: index as i64,
                    updated_at_ms: index as i64,
                });
            }
            Ok(())
        })
        .expect("fill");
        // A new lifecycle fact must not be lost for want of room.
        let event = enqueue_started(&settings, "worktree-1", EVENT_LIMIT as i64 + 1)
            .expect("enqueue")
            .expect("event");
        let saved = report(&settings).expect("report");
        assert_eq!(saved.sync_events.len(), EVENT_LIMIT);
        assert!(
            saved.sync_events.iter().any(|held| held.event_id == event),
            "the fresh event displaced the oldest settled one"
        );
        assert!(
            !saved
                .sync_events
                .iter()
                .any(|held| held.event_id == "event-0"),
            "the oldest unknown yielded its place"
        );
    }

    #[test]
    fn a_full_outbox_of_pending_still_admits_the_newest_fact() {
        let root = tempfile::tempdir().expect("tempdir");
        let settings = SettingsRepository::new(root.path());
        let held = put(&settings, link(root.path(), 1)).expect("link");
        mutate(&settings, |document| {
            // A long-held sync: every event is unsent Pending and none drains.
            for index in 0..EVENT_LIMIT {
                document.sync_events.push(JiraSyncEvent {
                    event_id: format!("pending-{index}"),
                    link_id: held.link_id.clone(),
                    trigger: JiraSyncTrigger::WorkStarted,
                    source_id: format!("src-{index}"),
                    succeeded: None,
                    summary: String::new(),
                    transition_id: Some("11".to_string()),
                    transition: JiraSyncActionState::Pending,
                    comment: JiraSyncActionState::NotRequested,
                    created_at_ms: index as i64,
                    updated_at_ms: index as i64,
                });
            }
            Ok(())
        })
        .expect("fill");
        // The newest lifecycle fact must still fit; the stalest unsent intent
        // yields rather than the enqueue failing and losing the new one.
        let fresh = enqueue_worker_done(
            &settings,
            Some("worktree-1"),
            None,
            "run-fresh",
            true,
            "done",
            EVENT_LIMIT as i64 + 1,
        )
        .expect("enqueue")
        .expect("event");
        let saved = report(&settings).expect("report");
        assert_eq!(saved.sync_events.len(), EVENT_LIMIT);
        assert!(saved.sync_events.iter().any(|held| held.event_id == fresh));
        assert!(
            !saved
                .sync_events
                .iter()
                .any(|held| held.event_id == "pending-0"),
            "the stalest unsent intent yielded its place"
        );
    }
}
