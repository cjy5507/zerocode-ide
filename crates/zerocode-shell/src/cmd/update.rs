//! Versioned and automatic updates — the five commands of design
//! docs/design/versioned-auto-update.md §3 (U-B). Every judgement is
//! `update_runtime`'s and pure; this file is the edge where the plugin, the
//! network, the disk (`update_store`) and the settings store are touched.
//!
//! The roads, in order: `update_check` asks the feed when the table says a
//! knock is due and hands the window a verdict (`next`: announce or
//! download); `update_download` fetches and verifies the archive to disk;
//! `update_install` unpacks it *beside* the running bundle; the swap itself
//! happens in `relaunch_window`, the one restart road, right before the
//! restart. `update_history` is the release list, cached.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri_plugin_updater::UpdaterExt;

use crate::update_runtime::{
    self as feed, Announced, Clock, FailureWord, Found, HistoryCache, Knock, NextStep, Phase,
    Release, UpdateChannel,
};
use crate::*;

/// One feed round trip may take this long before it is a network word.
const CHECK_TIMEOUT_SECS: u64 = 20;
/// The release list likewise.
const HISTORY_TIMEOUT_SECS: u64 = 20;
/// Download progress reaches the window on this event: `{version,
/// received_bytes, total_bytes, percent}` — one per percent moved, or per
/// [`PROGRESS_STRIDE_BYTES`] when the server did not say how much.
pub(crate) const PROGRESS_EVENT: &str = "update:progress";
const PROGRESS_STRIDE_BYTES: u64 = 4 << 20;

/// Where the update stands for the life of this process. Managed beside
/// `AppState`; the phase is what every command answers with, and the three
/// held things are what the next road needs: the plugin's announced update
/// (for the download), the verified archive (for the staging), the staged
/// bundle (for the swap at restart).
pub(crate) struct UpdateState {
    booted: Instant,
    held: Mutex<Held>,
}

#[derive(Default)]
struct Held {
    phase: Phase,
    announced: Option<tauri_plugin_updater::Update>,
    archive: Option<(String, PathBuf)>,
    staged: Option<(String, PathBuf)>,
    /// What the boot did about `~/.local/bin/zo` (design §2.4), once known.
    zo: Option<zo_companion::Report>,
}

impl UpdateState {
    pub(crate) fn new() -> Self {
        Self {
            booted: Instant::now(),
            held: Mutex::new(Held::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Held> {
        self.held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn phase(&self) -> Phase {
        self.lock().phase.clone()
    }

    fn set_phase(&self, phase: Phase) {
        self.lock().phase = phase;
    }

    fn boot_secs(&self) -> u64 {
        self.booted.elapsed().as_secs()
    }

    /// The staged bundle, if `update_install` finished: what `relaunch_window`
    /// swaps in right before the restart.
    pub(crate) fn staged(&self) -> Option<PathBuf> {
        self.lock().staged.as_ref().map(|(_, path)| path.clone())
    }

    /// The boot thread's one report about zo, kept for the pane.
    pub(crate) fn note_zo(&self, report: zo_companion::Report) {
        self.lock().zo = Some(report);
    }
}

/// What every update command answers: the phase, what the window does next
/// (only a check says so), the running build, the row as stored, and what
/// the boot did about zo (`null` until the boot thread has said).
#[derive(Clone, Serialize)]
pub(crate) struct UpdateReport {
    pub(crate) phase: Phase,
    pub(crate) next: Option<NextStep>,
    pub(crate) running: feed::BuildStamp,
    pub(crate) prefs: UpdatePrefs,
    pub(crate) zo: Option<zo_companion::Report>,
}

fn report(state: &AppState, held: &UpdateState, next: Option<NextStep>) -> UpdateReport {
    UpdateReport {
        phase: held.phase(),
        next,
        running: feed::build_stamp(),
        prefs: current_prefs(state),
        zo: held.lock().zo.clone(),
    }
}

fn current_prefs(state: &AppState) -> UpdatePrefs {
    load_settings_resilient(state.settings()).document.update
}

/// Seconds since the stored clock word, or `None` when the feed was never
/// asked or the word does not parse — both read as 「never」, which is due.
pub(crate) fn since_last_check(prefs: &UpdatePrefs, now_ms: i64) -> Option<u64> {
    let last = zerocode_core::civil::epoch_ms_of_iso(prefs.last_checked.as_deref()?)?;
    u64::try_from((now_ms - last) / 1_000).ok()
}

/// Ask the feed when the table says so, and judge the answer (§2.3).
///
/// A dev build never asks (`DevBuild`); a download or a staged bundle in
/// flight is left alone (a fresh check would only confuse what the person
/// already has); otherwise the policy and the clock decide whether this
/// knock is a check. Every attempt that produced a verdict stamps
/// `last_checked` — a failure too, so the next knock is not a retry loop.
#[tauri::command]
pub(crate) async fn update_check(
    app: AppHandle,
    state: State<'_, AppState>,
    held: State<'_, UpdateState>,
    knock: Knock,
) -> Result<UpdateReport, String> {
    let _crumb = crate::crumbs::Command::enter("update_check");
    let running = feed::build_stamp();
    if let Some(reason) = feed::dev_build(&running, &feed::lane_installed()) {
        held.set_phase(Phase::DevBuild { reason });
        return Ok(report(&state, &held, None));
    }
    if matches!(
        held.phase(),
        Phase::Checking
            | Phase::Downloading { .. }
            | Phase::Downloaded { .. }
            | Phase::Ready { .. }
    ) {
        return Ok(report(&state, &held, None));
    }
    let prefs = current_prefs(&state);
    let now_ms = now_epoch_ms();
    let clock = Clock {
        boot_secs: held.boot_secs(),
        since_last_check_secs: since_last_check(&prefs, now_ms),
    };
    if !feed::check_is_due(prefs.policy, knock, clock) {
        return Ok(report(&state, &held, None));
    }
    held.set_phase(Phase::Checking);
    let (found, detail, announced) = ask_feed(&app, prefs.channel).await;
    let mut verdict = feed::judge_found(prefs.policy, found, prefs.skipped_version.as_deref());
    if let Phase::Failed { detail: held, .. } = &mut verdict.phase {
        *held = detail;
    }
    let stamp = zerocode_core::civil::iso_utc_of(now_ms);
    let clear_skip = verdict.clear_skip;
    let revision = mutate_settings(state.settings(), |document| {
        document.update.last_checked = Some(stamp);
        if clear_skip {
            document.update.skipped_version = None;
        }
        Ok(())
    })?
    .revision;
    emit_settings_changed(&app, "", revision, &[setting_key::UPDATE]);
    {
        let mut guard = held.lock();
        guard.announced = announced;
        guard.phase = verdict.phase;
    }
    Ok(report(&state, &held, verdict.next))
}

/// One feed round trip through the plugin: the endpoint is the table's for
/// the channel (never the configured list, which holds only stable), and
/// the plugin's answer is read into a [`Found`] — a platform asset that is
/// not ours is 「이 플랫폼의 자산이 아직 없음」 before any signature is looked at.
async fn ask_feed(
    app: &AppHandle,
    channel: UpdateChannel,
) -> (Found, String, Option<tauri_plugin_updater::Update>) {
    let endpoint = match feed::feed_endpoint(channel).parse::<url::Url>() {
        Ok(url) => url,
        Err(error) => {
            return (
                Found::Failed(FailureWord::Malformed),
                error.to_string(),
                None,
            );
        }
    };
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .and_then(|builder| {
            builder
                .timeout(Duration::from_secs(CHECK_TIMEOUT_SECS))
                .build()
        });
    let updater = match updater {
        Ok(updater) => updater,
        Err(error) => {
            return (
                Found::Failed(feed::failure_word(&error)),
                error.to_string(),
                None,
            );
        }
    };
    match updater.check().await {
        Ok(None) => (Found::UpToDate, String::new(), None),
        Ok(Some(update)) => {
            let url = update.download_url.to_string();
            if !feed::asset_is_ours(&url) {
                return (Found::NoAssetForPlatform, url, None);
            }
            let announced = Announced {
                version: update.version.clone(),
                notes: update.body.clone(),
                pub_date: update.raw_json["pub_date"].as_str().map(str::to_string),
                asset: feed::url_file_name(&url).unwrap_or_default().to_string(),
            };
            (Found::Newer(announced), String::new(), Some(update))
        }
        Err(error) => {
            let detail = error.to_string();
            match feed::failure_word(&error) {
                FailureWord::NothingPublished => (Found::NothingPublished, detail, None),
                FailureWord::NoAssetForPlatform => (Found::NoAssetForPlatform, detail, None),
                word => (Found::Failed(word), detail, None),
            }
        }
    }
}

/// The configured public key, or the table's `UNSET` word when the field is
/// missing — the plugin reads it only at verification, so the check above
/// works either way and only this road refuses.
fn configured_pubkey(app: &AppHandle) -> String {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|updater| updater.get("pubkey"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .unwrap_or(feed::PUBKEY_UNSET)
        .to_string()
}

#[derive(Clone, Serialize)]
struct Progress {
    version: String,
    received_bytes: u64,
    total_bytes: Option<u64>,
    percent: Option<u8>,
}

/// Download the announced archive and verify its signature (the plugin's
/// road), then keep it on disk under the update directory. Progress reaches
/// the window as [`PROGRESS_EVENT`]. Refuses without a key: a download
/// nobody can verify is not one this window makes.
#[tauri::command]
pub(crate) async fn update_download(
    app: AppHandle,
    state: State<'_, AppState>,
    held: State<'_, UpdateState>,
) -> Result<UpdateReport, String> {
    let _crumb = crate::crumbs::Command::enter("update_download");
    let update = held
        .lock()
        .announced
        .clone()
        .ok_or_else(|| "no announced version — check first".to_string())?;
    if matches!(held.phase(), Phase::Downloading { .. }) {
        return Ok(report(&state, &held, None));
    }
    if configured_pubkey(&app) == feed::PUBKEY_UNSET {
        held.set_phase(Phase::Failed {
            word: FailureWord::SigningKeyUnset,
            detail: String::new(),
        });
        return Ok(report(&state, &held, None));
    }
    let version = update.version.clone();
    held.set_phase(Phase::Downloading {
        version: version.clone(),
        received_bytes: 0,
        total_bytes: None,
        percent: None,
    });
    let mut received = 0u64;
    let mut last_percent: Option<u8> = None;
    let mut last_stride = 0u64;
    let emitter = app.clone();
    let holder: &UpdateState = &held;
    let downloaded = update
        .download(
            |chunk, total| {
                received = received.saturating_add(chunk as u64);
                let percent = feed::progress(received, total);
                let moved = match percent {
                    Some(percent) => last_percent != Some(percent),
                    None => received / PROGRESS_STRIDE_BYTES != last_stride,
                };
                if !moved {
                    return;
                }
                last_percent = percent;
                last_stride = received / PROGRESS_STRIDE_BYTES;
                holder.set_phase(Phase::Downloading {
                    version: version.clone(),
                    received_bytes: received,
                    total_bytes: total,
                    percent,
                });
                let _ = emitter.emit_to(
                    MAIN_WINDOW_LABEL,
                    PROGRESS_EVENT,
                    Progress {
                        version: version.clone(),
                        received_bytes: received,
                        total_bytes: total,
                        percent,
                    },
                );
            },
            || {},
        )
        .await;
    let version = update.version.clone();
    let phase = match downloaded {
        Ok(bytes) => match feed::update_dir() {
            Some(dir) => {
                match update_store::write_archive(&dir, &feed::archive_file_name(&version), &bytes)
                {
                    Ok(path) => {
                        held.lock().archive = Some((version.clone(), path));
                        Phase::Downloaded { version }
                    }
                    Err(error) => Phase::Failed {
                        word: FailureWord::Disk,
                        detail: error.to_string(),
                    },
                }
            }
            None => Phase::Failed {
                word: FailureWord::Disk,
                detail: "no home directory".into(),
            },
        },
        Err(error) => Phase::Failed {
            word: feed::failure_word(&error),
            detail: error.to_string(),
        },
    };
    held.set_phase(phase);
    Ok(report(&state, &held, None))
}

/// 「준비」: unpack the verified archive beside the running bundle. Nothing
/// is swapped here — the swap is `relaunch_window`'s, right before the
/// restart, by rename (design §2.3, deploy-overwrite-kills-running-binary).
#[tauri::command(async)]
pub(crate) fn update_install(
    state: State<'_, AppState>,
    held: State<'_, UpdateState>,
) -> Result<UpdateReport, String> {
    let _crumb = crate::crumbs::Command::enter("update_install");
    let (version, archive) = held
        .lock()
        .archive
        .clone()
        .ok_or_else(|| "nothing downloaded — download first".to_string())?;
    let bundle = std::env::current_exe()
        .ok()
        .and_then(|exe| feed::app_bundle_of(&exe));
    let phase = match bundle {
        None => Phase::Failed {
            word: FailureWord::NotABundle,
            detail: String::new(),
        },
        Some(bundle) => match update_store::stage_beside(&bundle, &archive) {
            Ok(staged) => {
                let _ = std::fs::remove_file(&archive);
                let mut guard = held.lock();
                guard.archive = None;
                guard.staged = Some((version.clone(), staged));
                Phase::Ready { version }
            }
            Err(error) => Phase::Failed {
                word: error.word(),
                detail: error.detail(),
            },
        },
    };
    held.set_phase(phase);
    Ok(report(&state, &held, None))
}

/// Change one field of `settings.update` against the latest document — the
/// policy, the channel, or the skipped version. The clock (`last_checked`)
/// has no patch: only a check that reached the feed writes it.
#[tauri::command(async)]
pub(crate) fn patch_update_prefs(
    app: AppHandle,
    webview: tauri::Webview,
    state: State<'_, AppState>,
    patch: UpdatePrefsPatch,
) -> Result<SettingsSnapshot, String> {
    commit_setting(
        &app,
        &webview,
        &state,
        &[setting_key::UPDATE],
        move |settings| {
            patch.apply(&mut settings.update);
            Ok(())
        },
    )
}

/// Where a history answer came from.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum HistorySource {
    Cache,
    Live,
    None,
}

/// The release list as the pane draws it: our rows (§2.2 공존 규칙), when
/// they were fetched, where they came from, and the failure word when the
/// API could not be reached — the cache stands in then, if there is one.
#[derive(Clone, Serialize)]
pub(crate) struct HistoryReport {
    pub(crate) releases: Vec<Release>,
    pub(crate) fetched_at: Option<String>,
    pub(crate) source: HistorySource,
    pub(crate) failure: Option<FailureWord>,
}

/// The version history: the cache while it is fresh (six hours), the API
/// otherwise or when forced; on a failed fetch the cache stands, stale, with
/// the failure named. Public repository, no token, one page.
#[tauri::command]
pub(crate) async fn update_history(force: bool) -> Result<HistoryReport, String> {
    let _crumb = crate::crumbs::Command::enter("update_history");
    let dir = feed::update_dir().ok_or_else(|| "no home directory".to_string())?;
    let now_ms = now_epoch_ms();
    let cached = update_store::read_history_cache(&dir);
    if !force
        && let Some(cache) = cached
            .as_ref()
            .filter(|cache| feed::cache_is_fresh(cache, now_ms))
    {
        return Ok(HistoryReport {
            releases: cache.releases.clone(),
            fetched_at: Some(cache.fetched_at.clone()),
            source: HistorySource::Cache,
            failure: None,
        });
    }
    Ok(history_after_fetch(
        cached,
        fetch_releases(feed::GITHUB_API).await,
        now_ms,
        |cache| {
            let _ = update_store::write_history_cache(&dir, cache);
        },
    ))
}

/// The answer over a fetch result and whatever the cache held: a live list
/// is filtered to ours and written back; a failure keeps the cache standing.
pub(crate) fn history_after_fetch(
    cached: Option<HistoryCache>,
    fetched: Result<serde_json::Value, FailureWord>,
    now_ms: i64,
    keep: impl FnOnce(&HistoryCache),
) -> HistoryReport {
    match fetched {
        Ok(list) => {
            let cache = HistoryCache {
                fetched_at: zerocode_core::civil::iso_utc_of(now_ms),
                releases: feed::read_releases(&list),
            };
            keep(&cache);
            HistoryReport {
                releases: cache.releases,
                fetched_at: Some(cache.fetched_at),
                source: HistorySource::Live,
                failure: None,
            }
        }
        Err(word) => match cached {
            Some(cache) => HistoryReport {
                releases: cache.releases,
                fetched_at: Some(cache.fetched_at),
                source: HistorySource::Cache,
                failure: Some(word),
            },
            None => HistoryReport {
                releases: Vec::new(),
                fetched_at: None,
                source: HistorySource::None,
                failure: Some(word),
            },
        },
    }
}

/// GET the release list from `api_base` (the table's GitHub API, or a test's
/// own server). GitHub requires a User-Agent; the status decides the word.
pub(crate) async fn fetch_releases(api_base: &str) -> Result<serde_json::Value, FailureWord> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(HISTORY_TIMEOUT_SECS))
        .user_agent(format!("zerocode-shell/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| FailureWord::Network)?;
    let response = client
        .get(feed::releases_api_url(api_base, feed::HISTORY_PER_PAGE))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| FailureWord::Network)?;
    let status = response.status();
    if !status.is_success() {
        return Err(feed::history_failure(Some(status.as_u16())));
    }
    response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| FailureWord::Malformed)
}
