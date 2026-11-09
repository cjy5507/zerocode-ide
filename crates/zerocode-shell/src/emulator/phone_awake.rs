//! Keeping a real Android phone awake (T7, 2026-11-09): a session the person
//! starts in the window — never an agent, no verb names it — for one of a few
//! set lengths, which ends by itself.
//!
//! While it stands the window pokes the phone every
//! [`AWAKE_POKE_EVERY_SECS`] with the one key no app is sent
//! ([`zerocode_core::agent_emulator::real_android::AWAKE_POKE`]), through the
//! road every verb takes (`phone::awake_poke`): the person's answer, the
//! phone's own gate, its lock. A locked or sleeping phone is not poked — the
//! session waits, and says so — so it is never woken or unlocked, and no
//! setting of the phone is written. A phone the person stopped allowing ends
//! its session; one that stops answering ends it after a few tries.
//!
//! The window hears every change on `phone:awake` (the status bar's chip and
//! the settings card's rows) with the sessions that stand and, when one ended,
//! why.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::Emitter;
use zerocode_core::agent_emulator::real_android::{AWAKE_MINUTES, AWAKE_POKE_EVERY_SECS};
use zerocode_core::agent_emulator::real_phone::{PHONE_LOCKED, PHONE_NOT_ALLOWED};
use zerocode_core::computer_use_protocol::ProviderError;

use super::android::phone;

/// How many pokes in a row may find the phone gone before its session ends.
const UNREACHABLE_TRIES: u32 = 3;
/// How often a waiting session looks whether it was stopped or replaced.
const LOOK_EVERY: Duration = Duration::from_secs(1);

/// One phone kept awake, as the window draws it.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct AwakeSession {
    pub(crate) device: String,
    pub(crate) name: String,
    pub(crate) started_ms: i64,
    pub(crate) until_ms: i64,
    /// When the last poke went, `None` before the first.
    pub(crate) poked_ms: Option<i64>,
    /// Why the session waits instead of poking: `locked` or `unreachable`.
    pub(crate) waiting: Option<&'static str>,
    #[serde(skip)]
    generation: u64,
}

/// A session that ended for a reason the person should hear (not its time).
#[derive(Clone, Debug, Serialize)]
pub(crate) struct AwakeEnded {
    pub(crate) device: String,
    pub(crate) name: String,
    /// `time`, `stopped`, `not_allowed` or `unreachable`.
    pub(crate) why: &'static str,
}

#[derive(Clone, Serialize)]
struct AwakeNews<'a> {
    sessions: &'a [AwakeSession],
    #[serde(skip_serializing_if = "Option::is_none")]
    ended: Option<&'a AwakeEnded>,
}

/// One phone the person may keep awake (allowed, listed as up).
#[derive(Clone, Debug, Serialize)]
pub(crate) struct AwakePhone {
    pub(crate) device: String,
    pub(crate) name: String,
}

static SESSIONS: Mutex<BTreeMap<String, AwakeSession>> = Mutex::new(BTreeMap::new());
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        })
}

/// The sessions that stand. A poisoned lock still holds whole sessions: each
/// change below is one assignment.
fn held() -> MutexGuard<'static, BTreeMap<String, AwakeSession>> {
    SESSIONS.lock().unwrap_or_else(PoisonError::into_inner)
}

fn sessions() -> Vec<AwakeSession> {
    held().values().cloned().collect()
}

fn announce(ended: Option<&AwakeEnded>) {
    if let Some(app) = super::WINDOW.get() {
        let sessions = sessions();
        let _ = app.emit_to(
            "main",
            "phone:awake",
            AwakeNews {
                sessions: &sessions,
                ended,
            },
        );
    }
}

/// When the session `generation` started for `device` ends, while it stands.
fn standing_until(device: &str, generation: u64) -> Option<i64> {
    held()
        .get(device)
        .filter(|session| session.generation == generation)
        .map(|session| session.until_ms)
}

/// Change the standing session for `device` (the one `generation` started);
/// `change` says whether it changed what the window draws.
fn note(device: &str, generation: u64, change: impl FnOnce(&mut AwakeSession) -> bool) -> bool {
    held()
        .get_mut(device)
        .filter(|session| session.generation == generation)
        .is_some_and(change)
}

/// End the session `generation` started, saying why.
fn end(device: &str, generation: u64, why: &'static str) {
    let removed = {
        let mut standing = held();
        match standing.get(device) {
            Some(session) if session.generation == generation => standing.remove(device),
            _ => None,
        }
    };
    if let Some(session) = removed {
        announce(Some(&AwakeEnded {
            device: session.device,
            name: session.name,
            why,
        }));
    }
}

/// What one poke's answer means for the session: poked, waiting, or over.
enum Turn {
    Poked,
    Waiting(&'static str),
    Over(&'static str),
}

fn turn_of(answer: Result<(), ProviderError>, misses: &mut u32) -> Turn {
    match answer {
        Ok(()) => {
            *misses = 0;
            Turn::Poked
        }
        Err(error) if error.code == PHONE_LOCKED => {
            *misses = 0;
            Turn::Waiting("locked")
        }
        Err(error) if error.code == PHONE_NOT_ALLOWED => Turn::Over("not_allowed"),
        Err(_) => {
            *misses += 1;
            if *misses >= UNREACHABLE_TRIES {
                Turn::Over("unreachable")
            } else {
                Turn::Waiting("unreachable")
            }
        }
    }
}

async fn run(device: String, generation: u64) {
    let mut misses = 0;
    loop {
        let Some(until) = standing_until(&device, generation) else {
            return;
        };
        if epoch_ms() >= until {
            end(&device, generation, "time");
            return;
        }
        match turn_of(phone::awake_poke(&device).await, &mut misses) {
            Turn::Poked => {
                let now = epoch_ms();
                if note(&device, generation, |session| {
                    session.poked_ms = Some(now);
                    session.waiting = None;
                    true
                }) {
                    announce(None);
                }
            }
            Turn::Waiting(why) => {
                if note(&device, generation, |session| {
                    session.waiting.replace(why) != Some(why)
                }) {
                    announce(None);
                }
            }
            Turn::Over(why) => {
                end(&device, generation, why);
                return;
            }
        }
        // Wait out the beat a second at a time: a stop or a new session for the
        // phone is heard within one, and the end comes on its minute.
        let beat = Duration::from_secs(AWAKE_POKE_EVERY_SECS);
        let mut waited = Duration::ZERO;
        while waited < beat {
            tokio::time::sleep(LOOK_EVERY).await;
            waited += LOOK_EVERY;
            if standing_until(&device, generation).is_none() {
                return;
            }
            if epoch_ms() >= until {
                break;
            }
        }
    }
}

/// The phones a person may keep awake now.
#[tauri::command(async)]
pub(crate) async fn phone_awake_phones(webview: tauri::Webview) -> Result<Vec<AwakePhone>, String> {
    crate::from_the_main_webview(&webview)?;
    Ok(phone::awake_phones()
        .await
        .map_err(|error| error.message)?
        .into_iter()
        .map(|(device, name)| AwakePhone { device, name })
        .collect())
}

/// Keep `device` awake for `minutes` (one of [`AWAKE_MINUTES`]): the person's,
/// from the window. A phone already kept awake starts again from now.
#[tauri::command(async)]
pub(crate) async fn phone_awake_start(
    webview: tauri::Webview,
    device: String,
    minutes: u32,
) -> Result<Vec<AwakeSession>, String> {
    crate::from_the_main_webview(&webview)?;
    if !AWAKE_MINUTES.contains(&minutes) {
        return Err(format!("keep awake for one of {AWAKE_MINUTES:?} minutes"));
    }
    let name = phone::awake_phones()
        .await
        .map_err(|error| error.message)?
        .into_iter()
        .find_map(|(handle, name)| (handle == device).then_some(name))
        .ok_or_else(|| {
            "that phone is not an allowed Android phone that is connected now".to_string()
        })?;
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let started_ms = epoch_ms();
    held().insert(
        device.clone(),
        AwakeSession {
            device: device.clone(),
            name,
            started_ms,
            until_ms: started_ms + i64::from(minutes) * 60_000,
            poked_ms: None,
            waiting: None,
            generation,
        },
    );
    announce(None);
    tauri::async_runtime::spawn(run(device, generation));
    Ok(sessions())
}

/// Stop keeping `device` awake.
#[tauri::command(async)]
pub(crate) fn phone_awake_stop(
    webview: tauri::Webview,
    device: String,
) -> Result<Vec<AwakeSession>, String> {
    crate::from_the_main_webview(&webview)?;
    let removed = held().remove(&device);
    if let Some(session) = removed {
        announce(Some(&AwakeEnded {
            device: session.device,
            name: session.name,
            why: "stopped",
        }));
    }
    Ok(sessions())
}

/// The sessions that stand, for a window that opens — or reloads — while one does.
#[tauri::command(async)]
pub(crate) fn phone_awake_sessions(webview: tauri::Webview) -> Result<Vec<AwakeSession>, String> {
    crate::from_the_main_webview(&webview)?;
    Ok(sessions())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locked_phone_waits_a_gone_one_ends_after_three_tries_and_a_withdrawn_one_at_once() {
        let mut misses = 0;
        assert!(matches!(turn_of(Ok(()), &mut misses), Turn::Poked));
        assert!(matches!(
            turn_of(Err(ProviderError::new(PHONE_LOCKED, "locked")), &mut misses),
            Turn::Waiting("locked")
        ));
        let gone = || ProviderError::new("phone_unreachable", "gone");
        assert!(matches!(
            turn_of(Err(gone()), &mut misses),
            Turn::Waiting("unreachable")
        ));
        assert!(matches!(
            turn_of(Err(gone()), &mut misses),
            Turn::Waiting("unreachable")
        ));
        assert!(matches!(
            turn_of(Err(gone()), &mut misses),
            Turn::Over("unreachable")
        ));
        let mut fresh = 0;
        assert!(matches!(
            turn_of(
                Err(ProviderError::new(PHONE_NOT_ALLOWED, "withdrawn")),
                &mut fresh
            ),
            Turn::Over("not_allowed")
        ));
        // A poke that lands clears the count: three misses must be in a row.
        let mut count = 0;
        let _ = turn_of(Err(gone()), &mut count);
        let _ = turn_of(Ok(()), &mut count);
        assert_eq!(count, 0);
    }

    #[test]
    fn a_session_is_one_of_the_set_lengths_and_names_no_generation_to_the_window() {
        let session = AwakeSession {
            device: "phone-android-x".into(),
            name: "Synthetic Pixel".into(),
            started_ms: 0,
            until_ms: 900_000,
            poked_ms: None,
            waiting: Some("locked"),
            generation: 7,
        };
        let said = serde_json::to_value(&session).expect("serialises");
        assert!(said.get("generation").is_none(), "{said}");
        assert_eq!(said["waiting"], "locked");
        assert_eq!(AWAKE_MINUTES, [15, 30, 60]);
    }
}
