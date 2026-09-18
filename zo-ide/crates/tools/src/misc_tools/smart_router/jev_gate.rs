//! zo's side of the Jev door (`zerocode_core::jev::door`): what the door is
//! told — a key, the person's own settings, the workspace the words come
//! from, the day's count — and the one place a System One request leaves zo.
//!
//! The door itself is shared with the window, which asks System One about a
//! stopped browser walk down a wire of its own. This file reads what the door
//! needs from zo's side of the machine and sends what it clears: the routing
//! judgment in both modes, the recall judgment and a key check all pass
//! [`JevDoor::pass`] or [`JevDoor::pass_key_check`] and leave through
//! [`send`], and a source contract holds that nothing else in the workspace
//! calls the wire.
//!
//! Consent is read from zo's config home — the settings file the window's
//! settings pane writes — and never from a project's `.zo/settings.json`,
//! which arrives with a clone. One migration runs here: a workspace whose
//! ledgers hold rows written before the door, and none since, is one where a
//! person had a judgment turned on, so it is consented once, in that file,
//! under zo's settings lock. A row the door writes carries its count of
//! withheld lines, so a consent the person later takes back is not written
//! again.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::{SystemOneCall, SystemOneClient, SystemOneRequest};
use serde::Serialize;
use serde_json::{Map, Value};
use zerocode_core::jev::door::{self, Cleared, JevSettings, Refused};
use zerocode_core::jev::{count, JevUse, RECALL, ROUTING, SMART_SETTINGS_KEY};

use super::shadow_ledger::{last_shadow_line, shadow_ledger_path};

/// The person's settings file under zo's config home.
const SETTINGS_FILE: &str = "settings.json";

/// The uses whose ledgers live under a zo project's state: the ones whose rows
/// say a person had a judgment turned on in a workspace.
const ZO_USES: [JevUse; 2] = [ROUTING, RECALL];

/// The door as it stands for one batch of requests.
#[derive(Debug, Clone)]
pub struct JevDoor {
    settings: JevSettings,
    workspace: Option<String>,
    requests: PathBuf,
}

impl JevDoor {
    /// The door for words from `cwd`: the person's own settings, `cwd` spelled
    /// as the filesystem spells it, and today's count under zo's config home.
    /// A workspace whose ledgers predate the door is consented here, once.
    #[must_use]
    pub fn open(cwd: &Path) -> Self {
        let home = runtime::default_config_home();
        let settings_path = home.join(SETTINGS_FILE);
        let mut settings = read_settings(&settings_path);
        let workspace = door::resolved_path(cwd);
        if settings.enabled
            && !settings.consents(&workspace)
            && ledgers_predate_the_door(cwd)
            && consent(&settings_path, &workspace).is_ok()
        {
            settings.workspaces.push(workspace.clone());
        }
        Self { settings, workspace: Some(workspace), requests: todays_requests(&home) }
    }

    /// The door for a key check, which carries no workspace's words.
    #[must_use]
    pub fn for_key_check() -> Self {
        let home = runtime::default_config_home();
        Self { settings: read_settings(&home.join(SETTINGS_FILE)), workspace: None, requests: todays_requests(&home) }
    }

    /// A door told exactly these facts, for a test that must not read the
    /// machine's settings.
    #[cfg(test)]
    pub(super) fn at(settings: JevSettings, workspace: &Path, config_home: &Path) -> Self {
        Self {
            settings,
            workspace: Some(door::resolved_path(workspace)),
            requests: count::requests_path(config_home, "2026-09-17"),
        }
    }

    /// Ask the door about one request of `row`'s whose body is `body`; cleared,
    /// the request is counted in the day.
    ///
    /// # Errors
    /// The door's refusal.
    pub fn pass(&self, row: &JevUse, key: bool, body: Value) -> Result<Cleared, Refused> {
        door::pass(
            |asking| door::may_send(row, asking, body),
            key,
            &self.settings,
            self.workspace.as_deref(),
            &self.requests,
        )
    }

    /// Ask the door about a key check; cleared, it is counted in the day.
    ///
    /// # Errors
    /// The door's refusal.
    pub fn pass_key_check(&self, key: bool, body: &Value) -> Result<Cleared, Refused> {
        door::pass(|asking| door::may_check_key(asking, body), key, &self.settings, None, &self.requests)
    }
}

/// A typed request as the body the door reads; `None` for one that does not
/// serialize, which is sent nowhere.
#[must_use]
pub fn body_of<S: Serialize + ?Sized>(request: &SystemOneRequest<'_, S>) -> Option<Value> {
    serde_json::to_value(request).ok()
}

/// Send what the door cleared — the one place a System One request leaves zo.
pub async fn send(client: &SystemOneClient, cleared: Cleared, deadline: Duration) -> SystemOneCall {
    client.decide_body(cleared.into_bytes(), deadline).await
}

/// The door's settings in the person's settings file, each root spelled as the
/// filesystem spells it. A file that cannot be read or parsed consents to
/// nothing.
fn read_settings(path: &Path) -> JevSettings {
    let root = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .unwrap_or(Value::Null);
    JevSettings::from_root(&root).resolved()
}

/// Today's count file under `home`, the day read on this machine's clock.
fn todays_requests(home: &Path) -> PathBuf {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
    let offset_minutes = i32::try_from(core_types::date::local_utc_offset_secs() / 60).unwrap_or(0);
    count::requests_path(home, &count::day_of(now_ms, offset_minutes))
}

/// Whether `cwd`'s ledgers hold rows and every last one was written before the
/// door: a workspace where a person had a judgment on, and the door has not
/// spoken since.
fn ledgers_predate_the_door(cwd: &Path) -> bool {
    let last: Vec<String> =
        ZO_USES.iter().filter_map(|row| last_shadow_line(&shadow_ledger_path(cwd, row.ledger))).collect();
    !last.is_empty() && last.iter().all(|row| door::predates_the_door(row))
}

/// Add `workspace` to `smart.jev.workspaces` in the settings file at `path`,
/// under zo's settings lock, every other key as it stood. A file whose `smart`,
/// `jev` or `workspaces` is of another shape is refused rather than rewritten.
fn consent(path: &Path, workspace: &str) -> io::Result<()> {
    let _lock = runtime::SettingsFileLock::acquire(path)?;
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let mut root: Map<String, Value> =
        if text.trim().is_empty() { Map::new() } else { serde_json::from_str(&text).map_err(io::Error::other)? };
    let shape = || io::Error::new(io::ErrorKind::InvalidData, "the settings file holds another shape here");
    let roots = root
        .entry(SMART_SETTINGS_KEY)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(shape)?
        .entry(door::JEV_SETTINGS_KEY)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(shape)?
        .entry(door::WORKSPACES_SETTING)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(shape)?;
    if !roots.iter().any(|root| root.as_str() == Some(workspace)) {
        roots.push(Value::String(workspace.to_string()));
    }
    let mut rendered = serde_json::to_string_pretty(&root).map_err(io::Error::other)?;
    rendered.push('\n');
    runtime::replace_file_atomic(path, rendered.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No file in the workspace but this one calls the wire, and the wire
    /// takes nothing but bytes: every request zo makes of System One has been
    /// through the door.
    #[test]
    fn the_door_is_the_only_road_to_the_wire() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut sources = Vec::new();
        let mut pending = vec![crates];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).expect("a source directory").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|name| name != "target") {
                        pending.push(path);
                    }
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    sources.push(path);
                }
            }
        }
        let gate = Path::new(file!()).file_name().expect("this file");
        let wire = Path::new("api").join("src").join("systemone.rs");
        let callers: Vec<String> = sources
            .iter()
            .filter(|path| !path.ends_with(&wire))
            .filter(|path| {
                std::fs::read_to_string(path).is_ok_and(|text| text.contains(".decide_body("))
            })
            .map(|path| path.display().to_string())
            .collect();
        assert_eq!(callers.len(), 1, "the wire is called from {callers:?}");
        assert!(Path::new(&callers[0]).ends_with(Path::new("smart_router").join(gate)), "{callers:?}");
        assert!(sources.len() > 100, "the scan found the workspace: {} files", sources.len());
    }

    /// Both of zo's ledgers keep the door's counts under the door's own keys —
    /// the keys a migration reads a row's age by — and zo's wire names a
    /// missing key with the door's word.
    #[test]
    fn zos_ledgers_and_wire_speak_the_doors_words() {
        let routing = serde_json::to_value(super::super::decision_shadow::DecisionShadowRow {
            at: 1,
            attempt: None,
            task: "0000000000000001".to_string(),
            rubric_version: 1,
            model: None,
            outcome: Refused::Budget.token().to_string(),
            elapsed_ms: 0,
            retries: 0,
            cached: false,
            input_tokens: None,
            route_use: super::super::decision_shadow::DecisionRouteUse::RecordOnly,
            requests: Some(0),
            redacted_lines: Some(0),
            probe: super::super::decision_shadow::ProbeCell::Failed("timeout".to_string()),
            jev: None,
        })
        .expect("a routing row");
        let recall: Value = serde_json::from_str(
            &serde_json::to_string(&super::super::rerank_shadow::RerankShadowRow {
                at: 1,
                query: 1,
                notes: 1,
                rubric_version: 1,
                outcome: Refused::Budget.token().to_string(),
                candidates: 1,
                cached: false,
                elapsed_ms: 0,
                retries: 0,
                model: None,
                input_tokens: None,
                requests: Some(0),
                redacted_lines: Some(0),
                judged: None,
                applied: false,
            })
            .expect("a recall row"),
        )
        .expect("json");
        for row in [&routing, &recall] {
            assert_eq!(row[door::REQUESTS_KEY], 0, "{row}");
            assert_eq!(row[door::REDACTED_LINES_KEY], 0, "{row}");
            assert!(!door::predates_the_door(&row.to_string()));
        }
        assert_eq!(api::SystemOneFailure::NoKey.token(), Refused::NoKey.token());
    }

    #[test]
    fn a_consent_is_written_once_and_nothing_else_of_the_file_moves() {
        let home = tempfile::tempdir().expect("a config home");
        let path = home.path().join(SETTINGS_FILE);
        std::fs::write(&path, r#"{"model":"fable","smart":{"decisionShadow":"shadow","jev":{"dailyRequests":40}}}"#)
            .expect("settings");

        consent(&path, "/work/app").expect("consented");
        consent(&path, "/work/app").expect("consented again");

        let root: Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(root["smart"]["jev"]["workspaces"], serde_json::json!(["/work/app"]));
        assert_eq!(root["smart"]["jev"]["dailyRequests"], 40);
        assert_eq!(root["smart"]["decisionShadow"], "shadow");
        assert_eq!(root["model"], "fable");

        std::fs::write(&path, r#"{"smart":{"jev":{"workspaces":"/work/app"}}}"#).expect("a stranger's shape");
        assert!(consent(&path, "/work/app").is_err());
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            r#"{"smart":{"jev":{"workspaces":"/work/app"}}}"#,
            "a refused consent writes nothing"
        );
    }
}
