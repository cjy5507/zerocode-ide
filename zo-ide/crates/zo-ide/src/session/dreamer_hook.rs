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

use runtime::memory::{AutoDream, AutoDreamTrigger, DreamError, DreamReport};

use crate::autonomy::dreaming::DreamAttempt;

const SESSION_END_BUDGET: Duration = Duration::from_secs(2);
const DREAM_ENV: &str = "ZO_DREAM";

/// `zo auto-dream --on idle|compaction --cwd <dir> [--session <id>] [--model
/// <name>]`: one automatic pass in this process, on the interval of the path
/// that asks, answered as the one JSON line the session reads back. An idle
/// pass also mirrors into the second-brain vault, so it needs its session.
///
/// # Errors
/// Says which word was missing or not understood.
pub fn answer_auto_dream(args: &[String]) -> Result<String, String> {
    let (mut on, mut cwd, mut session, mut model) = (None, None, None, None);
    let mut words = args.iter();
    while let Some(word) = words.next() {
        let slot = match word.as_str() {
            "--on" => &mut on,
            "--cwd" => &mut cwd,
            "--session" => &mut session,
            "--model" => &mut model,
            other => return Err(format!("unknown word {other:?}")),
        };
        *slot = Some(words.next().ok_or_else(|| format!("{word} needs a value"))?);
    }
    let (Some(on), Some(cwd)) = (on, cwd) else {
        return Err("needs --on and --cwd".to_string());
    };
    let reply = match AutoDreamTrigger::from_word(on) {
        Some(AutoDreamTrigger::Idle) => {
            let (Some(session), Some(model)) = (session, model) else {
                return Err("an idle pass needs --session and --model".to_string());
            };
            curate_here(Path::new(cwd), session, model)
        }
        Some(AutoDreamTrigger::Compaction) => PassReply::of(&runtime::memory::try_auto_dream(
            Path::new(cwd),
            runtime::memory::DEFAULT_COMPACTION_DREAM_INTERVAL,
            model.map(String::as_str),
        )),
        None => return Err(format!("--on takes idle or compaction, not {on:?}")),
    };
    serde_json::to_string(&reply).map_err(|error| error.to_string())
}

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
            if let Ok(Some(report)) = &result {
                crate::dream::record(report);
                promote_to_second_brain(&cwd, &worker_session_id, report);
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
/// Start an idle pass without borrowing the session or blocking its renderer.
pub(crate) fn spawn_on_idle(
    cwd: &Path, session_id: &str, model: &str,
    attempt: DreamAttempt,
) {
    let cwd = cwd.to_path_buf();
    let session_id = session_id.to_string();
    let model = model.to_string();
    let _ = thread::Builder::new().name("dreamer-idle".to_string()).spawn(move || {
        let reply = idle_pass(runtime::memory::auto_dream_program(), &cwd, &session_id, &model);
        settle(&attempt, &reply, crate::autonomy::scheduler::now_unix_ms());
        match reply {
            PassReply::Ran { summary, promoted } => {
                log_status(&session_id, &format!("idle {summary}"));
                crate::dream::record_pass(summary, promoted);
            }
            PassReply::Failed => log_status(&session_id, "idle_failed"),
            PassReply::NotDue { .. } | PassReply::NoEvidence | PassReply::Disabled | PassReply::Locked => {}
        }
    });
}

/// What an automatic pass found, in the terms the idle clock and the status
/// card read.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", rename_all_fields = "camelCase")]
pub(crate) enum PassReply {
    Ran { summary: String, promoted: Vec<String> },
    NotDue { until_ms: u64 },
    NoEvidence,
    Disabled,
    Locked,
    Failed,
}

impl PassReply {
    fn of(result: &Result<AutoDream, DreamError>) -> Self {
        match result {
            Ok(AutoDream::Ran(report)) => Self::Ran {
                summary: report.summary_line(),
                promoted: crate::dream::promoted_slugs(report),
            },
            Ok(AutoDream::NotDueUntil(until)) => Self::NotDue { until_ms: unix_ms(*until) },
            Ok(AutoDream::NoEvidence) => Self::NoEvidence,
            Ok(AutoDream::Disabled) => Self::Disabled,
            Ok(AutoDream::Locked) => Self::Locked,
            Err(_) => Self::Failed,
        }
    }
}

fn unix_ms(at: std::time::SystemTime) -> u64 {
    at.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| u64::try_from(since.as_millis()).unwrap_or(u64::MAX))
}

/// The cheap gate first, in this process; a pass it lets through curates in
/// `program` when there is one, else here.
fn idle_pass(program: Option<&Path>, cwd: &Path, session_id: &str, model: &str) -> PassReply {
    if let Some(held) = runtime::memory::auto_dream_held(cwd, runtime::memory::DEFAULT_AUTO_DREAM_INTERVAL) {
        return PassReply::of(&Ok(held));
    }
    match program {
        Some(zo) => run_idle_pass(zo, cwd, session_id, model),
        None => curate_here(cwd, session_id, model),
    }
}

fn curate_here(cwd: &Path, session_id: &str, model: &str) -> PassReply {
    let result = runtime::memory::try_auto_dream(
        cwd, runtime::memory::DEFAULT_AUTO_DREAM_INTERVAL, Some(model),
    );
    if let Ok(AutoDream::Ran(report)) = &result {
        promote_to_second_brain(cwd, session_id, report);
    }
    PassReply::of(&result)
}

/// One idle pass in `<zo> auto-dream`; the session keeps only the reply line.
/// A child that cannot start, exits badly or answers something else counts as
/// a failed pass.
pub(crate) fn run_idle_pass(zo: &Path, cwd: &Path, session_id: &str, model: &str) -> PassReply {
    runtime::memory::run_auto_dream_pass(zo, AutoDreamTrigger::Idle, cwd, Some(session_id), Some(model))
        .and_then(|out| serde_json::from_slice(out.trim_ascii()).ok())
        .unwrap_or(PassReply::Failed)
}

/// Move the idle clock by what the pass found: a pass that ran waits the full
/// automatic interval, one the shared marker holds sleeps until it lets go,
/// one with nothing to curate waits for this session's next turn, and one that
/// failed or found another pass running looks again later each time.
fn settle(attempt: &DreamAttempt, reply: &PassReply, now: u64) {
    match reply {
        PassReply::Ran { .. } => attempt.completed(now),
        PassReply::NotDue { until_ms } => attempt.not_due_until(*until_ms, now),
        PassReply::NoEvidence | PassReply::Disabled => attempt.awaits_activity(),
        PassReply::Locked | PassReply::Failed => attempt.retry_later(now),
    }
}

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

    #[test]
    fn an_idle_pass_moves_the_clock_by_what_it_found() {
        use super::{settle, PassReply};
        use crate::autonomy::dreaming::IdleDreamer;
        let minute = 60_000;
        let pass = |clock: &IdleDreamer, now: u64, reply: &PassReply| {
            let attempt = clock.begin(now, true).expect("the idle clock is due");
            settle(&attempt, reply, now);
        };

        let held = IdleDreamer::new(true, 0, minute);
        pass(&held, minute, &PassReply::NotDue { until_ms: 300 * minute });
        assert_eq!(held.next_wakeup(true), Some(300 * minute));

        for reply in [PassReply::NoEvidence, PassReply::Disabled] {
            let mut quiet = IdleDreamer::new(true, 0, minute);
            pass(&quiet, minute, &reply);
            assert_eq!(quiet.next_wakeup(true), None, "{reply:?}");
            quiet.activity(9 * minute);
            assert_eq!(quiet.next_wakeup(true), Some(10 * minute), "{reply:?}");
        }

        for reply in [PassReply::Locked, PassReply::Failed] {
            let busy = IdleDreamer::new(true, 0, minute);
            pass(&busy, minute, &reply);
            pass(&busy, 2 * minute, &reply);
            assert_eq!(busy.next_wakeup(true), Some(4 * minute), "{reply:?}");
        }

        let ran = IdleDreamer::new(true, 0, minute);
        pass(&ran, minute, &PassReply::Ran { summary: String::new(), promoted: Vec::new() });
        assert_eq!(ran.next_wakeup(true), Some(minute + 6 * 60 * minute));
    }

    #[cfg(unix)]
    #[test]
    fn an_idle_pass_runs_in_a_process_of_its_own_and_the_session_keeps_only_its_reply() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = temp_root("child");
        let cwd = root.join("workspace");
        let config_home = root.join("config");
        fs::create_dir_all(&cwd).expect("workspace should be created");
        fs::create_dir_all(&config_home).expect("config home should be created");
        let seen = root.join("seen");
        let zo = root.join("zo");
        fs::write(
            &zo,
            format!(
                "#!/bin/sh\npwd > '{seen}'\nprintf '%s\\n' \"$@\" >> '{seen}'\necho '{{\"outcome\":\"locked\"}}'\n",
                seen = seen.display(),
            ),
        )
        .expect("a stand-in zo should be written");
        fs::set_permissions(&zo, fs::Permissions::from_mode(0o755))
            .expect("the stand-in zo should be executable");

        let reply = with_dream_env(&config_home, "0", || {
            super::run_idle_pass(&zo, &cwd, "session-7", "model-x")
        });
        assert_eq!(reply, super::PassReply::Locked);
        let seen = fs::read_to_string(&seen).expect("the pass ran in the program it was given");
        let mut lines = seen.lines();
        let ran_in = lines.next().expect("the child says where it ran");
        assert_eq!(
            fs::canonicalize(ran_in).expect("the child's folder exists"),
            fs::canonicalize(&cwd).expect("the workspace exists"),
        );
        let cwd_arg = cwd.to_str().expect("the test root is UTF-8");
        assert_eq!(
            lines.collect::<Vec<_>>(),
            ["auto-dream", "--on", "idle", "--cwd", cwd_arg, "--session", "session-7", "--model", "model-x"]
        );

        fs::remove_dir_all(root).expect("test root should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn a_pass_the_cheap_gate_holds_starts_no_process() {
        use std::os::unix::fs::PermissionsExt as _;
        let root = temp_root("held");
        let cwd = root.join("workspace");
        let config_home = root.join("config");
        fs::create_dir_all(&cwd).expect("workspace should be created");
        fs::create_dir_all(&config_home).expect("config home should be created");
        let seen = root.join("seen");
        let zo = root.join("zo");
        fs::write(
            &zo,
            format!("#!/bin/sh\ntouch '{}'\necho '{{\"outcome\":\"locked\"}}'\n", seen.display()),
        )
        .expect("a stand-in zo should be written");
        fs::set_permissions(&zo, fs::Permissions::from_mode(0o755))
            .expect("the stand-in zo should be executable");

        with_dream_env(&config_home, "1", || {
            assert_eq!(super::idle_pass(Some(&zo), &cwd, "s", "m"), super::PassReply::NoEvidence);
            assert!(!seen.exists(), "a pass with nothing to curate must not start a process");
            seed_verified_observations(&cwd);
            assert_eq!(super::idle_pass(Some(&zo), &cwd, "s", "m"), super::PassReply::Locked);
            assert!(seen.exists(), "a pass the gate lets through runs in the program");
        });

        fs::remove_dir_all(root).expect("test root should be removed");
    }
}
