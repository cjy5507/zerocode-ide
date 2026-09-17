//! Session-end bridge for the between-sessions Dreamer pass.
//!
//! The runtime owns the Dreamer brain and filesystem hands. This module only
//! supplies the lifecycle boundary: it honors the IDE opt-out, gives a normal
//! pass a small synchronous budget, and lets a slow pass finish on its detached
//! worker without holding up process shutdown.
//!
//! Every finished pass is also handed to [`crate::dream`], which is what the
//! exit summary and the `/status` card read. That happens on the WORKER, before
//! the result is sent back, so a pass that outran the budget — the caller
//! already gave up on it — still leaves its evidence behind. `/resume` swaps the
//! session inside a live process, so a late report has a reader.
//!
//! The same worker mirrors the pass into the second-brain vault when one is
//! configured ([`runtime::second_brain::promote`]). It runs there rather than in
//! the runtime's own dream pass because a vault is a host concern — the vault
//! belongs to the person, not to the project's memory store — and because the
//! worker is already the place a slow session-end job is allowed to outlive its
//! budget.

use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use runtime::memory::DreamReport;

const SESSION_END_BUDGET: Duration = Duration::from_secs(2);
const DREAM_ENV: &str = "ZO_DREAM";

/// Run the automatic Dreamer pass when a session ends.
///
/// A report is returned only when the pass completes within the shutdown
/// budget. Errors, the explicit `ZO_DREAM=0` opt-out, and throttled passes are
/// deliberately represented as `None`: session cleanup must never fail because
/// memory curation did. A pass that outlives the budget continues on the
/// detached worker and is not waited on by the caller.
#[must_use]
pub(crate) fn run_on_session_end(
    cwd: &Path,
    session_id: &str,
    active_model: &str,
) -> Option<DreamReport> {
    if dream_disabled() {
        log_status(session_id, "disabled");
        return None;
    }

    let cwd = cwd.to_path_buf();
    // The pass runs on a detached worker, so it takes the model and the session
    // id by value: the session they belong to may be gone before the promotion
    // is written.
    let active_model = active_model.to_string();
    let worker_session_id = session_id.to_string();
    let (result_tx, result_rx) = mpsc::sync_channel(1);
    let spawned = thread::Builder::new()
        .name("dreamer-session-end".to_string())
        .spawn(move || {
            // Match the forge automatic path: the runtime helper supplies the
            // composite `.zo/dream`/turn/user-pattern/automation source,
            // FsMemoryStore, and PromotionPolicy::default(). Its interval is
            // the production six-hour automatic-dream window.
            let result = runtime::maybe_auto_dream(
                &cwd,
                runtime::memory::DEFAULT_AUTO_DREAM_INTERVAL,
                Some(&active_model),
            );
            match &result {
                Ok(Some(report)) => {
                    crate::dream::record(report);
                    promote_to_second_brain(&cwd, &worker_session_id, report);
                }
                Err(error) => {
                    let _ = runtime::record_auto_dream_failure(&cwd, error);
                }
                Ok(None) => {}
            }
            let _ = result_tx.send(result);
        });

    if spawned.is_err() {
        log_status(session_id, "thread_spawn_failed");
        return None;
    }

    match result_rx.recv_timeout(SESSION_END_BUDGET) {
        Ok(Ok(Some(report))) => {
            // The counts, not just the fact: "completed" alone left a log that
            // could not tell a pass that promoted three lessons from one that
            // promoted none.
            log_status(session_id, &format!("completed {}", report.summary_line()));
            Some(report)
        }
        Ok(Ok(None)) => {
            log_status(session_id, "throttled");
            None
        }
        Err(RecvTimeoutError::Timeout) => {
            log_status(session_id, "deferred");
            None
        }
        Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => {
            log_status(session_id, "failed");
            None
        }
    }
}

/// Mirror a finished pass into the configured second-brain vault.
///
/// Silent when no vault is configured or `secondBrain.writeBack` is off — the
/// same rule as every other seam of the feature — and loud only when it
/// actually wrote something, so the session-end log can say what left the
/// process.
fn promote_to_second_brain(cwd: &Path, session_id: &str, report: &runtime::memory::DreamReport) {
    let Some(promotion) =
        runtime::second_brain::promote::promote_session_lessons(cwd, session_id, report)
    else {
        return;
    };
    if !promotion.is_noop() {
        log_status(session_id, &promotion.summary_line());
    }
}

fn dream_disabled() -> bool {
    std::env::var(DREAM_ENV).ok().as_deref() == Some("0")
}

fn log_status(session_id: &str, status: &str) {
    eprintln!(
        "[zo] dreamer session-end session={} status={status}",
        safe_session_id(session_id)
    );
}

fn safe_session_id(session_id: &str) -> String {
    let id: String = session_id
        .chars()
        .filter(|character| !character.is_control())
        .take(128)
        .collect();
    if id.is_empty() {
        "unknown".to_string()
    } else {
        id
    }
}

#[cfg(test)]
mod tests {
    use super::{run_on_session_end, DREAM_ENV};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let count = COUNTER.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "zo-ide-dreamer-hook-{label}-{}-{timestamp}-{count}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("test root should be created");
        root
    }

    fn with_dream_env<T>(config_home: &Path, value: &str, f: impl FnOnce() -> T) -> T {
        let _lock = crate::test_env_lock();
        let previous_config_home =
            std::env::var_os(core_types::paths::ZO_CONFIG_HOME_ENV);
        let previous_dream = std::env::var_os(DREAM_ENV);
        std::env::set_var(core_types::paths::ZO_CONFIG_HOME_ENV, config_home);
        std::env::set_var(DREAM_ENV, value);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        match previous_config_home {
            Some(value) => std::env::set_var(core_types::paths::ZO_CONFIG_HOME_ENV, value),
            None => std::env::remove_var(core_types::paths::ZO_CONFIG_HOME_ENV),
        }
        match previous_dream {
            Some(value) => std::env::set_var(DREAM_ENV, value),
            None => std::env::remove_var(DREAM_ENV),
        }
        match result {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    fn seed_verified_observations(cwd: &Path) {
        for session_id in ["dreamer-test-session-1", "dreamer-test-session-2"] {
            let observation = runtime::memory::verified_check_observation(
                session_id,
                Some("cargo test -p zo-ide"),
            )
            .expect("a check command should produce an observation");
            runtime::memory::record_observation(cwd, &observation)
                .expect("observation JSONL should be written");
        }
    }

    #[test]
    fn session_end_promotes_seeded_observation_to_local_memory() {
        let root = temp_root("promotes");
        let cwd = root.join("workspace");
        let config_home = root.join("config");
        fs::create_dir_all(&cwd).expect("workspace should be created");
        fs::create_dir_all(&config_home).expect("config home should be created");
        seed_verified_observations(&cwd);

        with_dream_env(&config_home, "1", || {
            let report = run_on_session_end(&cwd, "dreamer-test-session-2", "claude-opus-5")
                .expect("a fast Dreamer pass should return its report");
            assert_eq!(report.applied.len(), 1);
            let slug = &report.applied[0].slug;
            let memory_dir = runtime::memory::paths::memory_write_dir(&cwd, true);
            assert!(
                memory_dir.join(format!("{slug}.md")).is_file(),
                "Dreamer should create the promoted memory slug"
            );
        });

        fs::remove_dir_all(root).expect("test root should be removed");
    }

    #[test]
    fn session_end_opt_out_writes_no_memory_or_throttle_marker() {
        let root = temp_root("disabled");
        let cwd = root.join("workspace");
        let config_home = root.join("config");
        fs::create_dir_all(&cwd).expect("workspace should be created");
        fs::create_dir_all(&config_home).expect("config home should be created");
        seed_verified_observations(&cwd);

        with_dream_env(&config_home, "0", || {
            assert!(run_on_session_end(&cwd, "dreamer-test-session-2", "claude-opus-5").is_none());
            let memory_dir = runtime::memory::paths::memory_write_dir(&cwd, false);
            assert!(
                !memory_dir.exists(),
                "ZO_DREAM=0 must not create global memory files"
            );
            assert!(
                !cwd.join(".zo/dream/.last_auto_dream").exists(),
                "ZO_DREAM=0 must not create an automatic-dream marker"
            );
        });

        fs::remove_dir_all(root).expect("test root should be removed");
    }
}
