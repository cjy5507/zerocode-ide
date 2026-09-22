//! Cron records as autonomous turns.
//!
//! The registry the `CronCreate` / `CronList` / `CronRunDue` tools write is
//! [`runtime::team_cron_registry::CronRegistry`], shared through the tool
//! context and persisted per working directory. This module is the other
//! half the tools used to say did not exist: the session's idle-time driver
//! asks [`next_wakeup`] when to wake and [`dispatch_due`] for the turn to run.
//! Nothing here keeps its own clock — the registry's local-minute matching
//! decides "due", and the one autonomy driver decides "now".
//!
//! A due run is recorded through the registry's cross-process transaction
//! before the turn opens, so two sessions in the same directory fire one
//! minute once, and `CronRunDue` — the by-hand path — sees it as already run.

use runtime::team_cron_registry::CronRegistry;

/// The word the frontends print for a cron turn where a loop turn says `loop`.
pub const TURN_LABEL: &str = "cron";

/// One cron firing: the registry entry, the minute it fired for, and the
/// prompt the turn opens with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronTurn {
    pub id: String,
    pub prompt: String,
    /// The local minute the run was recorded for, unix seconds.
    pub due_at_secs: u64,
}

/// The earliest minute any enabled cron is next due, in unix milliseconds —
/// `None` when nothing is registered.
#[must_use]
pub fn next_wakeup(registry: &CronRegistry, now_unix_ms: u64) -> Option<u64> {
    let now = now_unix_ms / 1_000;
    registry
        .list(true)
        .iter()
        .filter_map(|entry| registry.next_due_at(&entry.cron_id, now).ok().flatten())
        .min()
        .map(|secs| secs.saturating_mul(1_000))
}

/// The first due cron with its run recorded — the one turn to open now.
/// A cron another process recorded first is skipped; missed minutes coalesce
/// to the latest one, as the by-hand `CronRunDue` already promised.
#[must_use]
pub fn dispatch_due(registry: &CronRegistry, now_unix_ms: u64) -> Option<CronTurn> {
    let now = now_unix_ms / 1_000;
    for due in registry.due_at(now) {
        let id = due.entry.cron_id.clone();
        let recorded = match registry.record_due_run_at(&id, due.due_at) {
            Ok(recorded) => recorded,
            // The registry keeps the record in memory when the disk refuses
            // it (and has already warned once); the turn still owes its run.
            Err(_) => registry.get(&id).is_some_and(|entry| {
                entry
                    .last_run_at
                    .is_some_and(|last| last / 60 >= due.due_at / 60)
            }),
        };
        if recorded {
            return Some(CronTurn {
                prompt: prompt_for(&id, &due.entry.prompt, due.entry.description.as_deref()),
                id,
                due_at_secs: due.due_at,
            });
        }
    }
    None
}

/// The first due cron's id when the session cannot run it — so the refusal
/// can name what it refused.
#[must_use]
pub fn first_due(registry: &CronRegistry, now_unix_ms: u64) -> Option<String> {
    registry
        .due_at(now_unix_ms / 1_000)
        .first()
        .map(|due| due.entry.cron_id.clone())
}

fn prompt_for(id: &str, prompt: &str, description: Option<&str>) -> String {
    let description = description
        .map(|description| format!(" ({description})"))
        .unwrap_or_default();
    format!(
        "[zo:cron id={id}]\n{prompt}\n\nThis turn was opened by the cron `{id}`{description} at its scheduled minute. Do not ask the absent user a question."
    )
}

#[cfg(test)]
mod tests {
    use super::{dispatch_due, first_due, next_wakeup};
    use runtime::team_cron_registry::CronRegistry;

    const MINUTE_MS: u64 = 60_000;
    /// 2023-11-14T22:13:50Z — fifty seconds into a minute.
    const CREATED_MID_MINUTE_SECS: u64 = 1_700_000_030;

    #[test]
    fn a_cron_created_on_the_minute_is_due_that_minute() {
        let registry = CronRegistry::new_in_memory();
        let on_the_minute = CREATED_MID_MINUTE_SECS / 60 * 60;
        let entry = registry
            .create_at("* * * * *", "say hello", None, on_the_minute)
            .expect("create");
        assert_eq!(
            next_wakeup(&registry, on_the_minute * 1_000),
            Some(on_the_minute * 1_000),
            "created on the minute, the entry is due that minute, not the next"
        );
        assert!(dispatch_due(&registry, on_the_minute * 1_000).is_some());
        assert_eq!(entry.created_at, on_the_minute);
    }

    #[test]
    fn an_empty_registry_never_wakes_the_session() {
        let registry = CronRegistry::new_in_memory();
        assert_eq!(next_wakeup(&registry, 1_700_000_000_000), None);
        assert_eq!(dispatch_due(&registry, 1_700_000_000_000), None);
    }

    #[test]
    fn a_cron_wakes_the_session_at_its_next_minute_and_fires_once_for_it() {
        let registry = CronRegistry::new_in_memory();
        // A known second, fifty seconds into its minute: the entry's first
        // minute is the next one, whatever the wall clock says. (Created ON a
        // minute it would be due that minute — the race the wall clock used to
        // lose once in sixty runs; `a_cron_created_on_the_minute_is_due_that_minute`.)
        let entry = registry
            .create_at("* * * * *", "say hello", Some("every minute"), CREATED_MID_MINUTE_SECS)
            .expect("create");
        let created_ms = entry.created_at * 1_000;
        // The first matching minute strictly after creation.
        let next_minute_ms = (created_ms / MINUTE_MS + 1) * MINUTE_MS;

        assert_eq!(next_wakeup(&registry, created_ms), Some(next_minute_ms));
        assert_eq!(dispatch_due(&registry, next_minute_ms - 1), None, "not before its minute");

        let turn = dispatch_due(&registry, next_minute_ms).expect("due at the minute");
        assert_eq!(turn.id, entry.cron_id);
        assert_eq!(turn.due_at_secs, next_minute_ms / 1_000);
        assert!(turn.prompt.contains(&format!("[zo:cron id={}]", entry.cron_id)));
        assert!(turn.prompt.contains("say hello"));
        assert!(turn.prompt.contains("(every minute)"));

        assert_eq!(
            dispatch_due(&registry, next_minute_ms + 30_000),
            None,
            "the same minute does not fire twice"
        );
        assert_eq!(
            next_wakeup(&registry, next_minute_ms + 30_000),
            Some(next_minute_ms + MINUTE_MS),
            "the clock moves to the following minute"
        );
        assert_eq!(first_due(&registry, next_minute_ms + 30_000), None);
    }

    #[test]
    fn a_missed_minute_is_coalesced_into_one_turn() {
        let registry = CronRegistry::new_in_memory();
        let entry = registry.create("* * * * *", "catch up", None).expect("create");
        let created_ms = entry.created_at * 1_000;
        let three_minutes_on = (created_ms / MINUTE_MS + 3) * MINUTE_MS + 5_000;

        assert_eq!(first_due(&registry, three_minutes_on), Some(entry.cron_id.clone()));
        let turn = dispatch_due(&registry, three_minutes_on).expect("due");
        assert_eq!(turn.due_at_secs, (three_minutes_on / MINUTE_MS) * 60);
        assert_eq!(dispatch_due(&registry, three_minutes_on), None);
    }
}
