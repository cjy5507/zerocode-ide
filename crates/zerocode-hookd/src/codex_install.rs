//! Writing a Codex hook, and being able to take it back.
//!
//! `~/.codex/hooks.json` belongs to the user. [`crate::codex_grant`] can ask
//! Codex to trust what we put there, and when that fails the entry **must not
//! stay** — an untrusted hook stalls the agent on a prompt. So writing and
//! undoing are one operation here, and the undo has to restore the bytes and the
//! mode, not approximately the same file.
//!
//! Measured from Orca **1.4.169** (`installRealHomeCodexHook`,
//! out/main/index.js:213440-213505; `writeHooksJson` / `readHooksJsonWithRaw` /
//! `removeManagedCommands` / `resolveHooksJsonWritePath`,
//! managed-agent-hook-controls-Hy3KYvcp.js:684-920).
//!
//! ## The five refusals
//!
//! Each one is a case where writing would damage something:
//!
//! 1. **Unreadable** — we cannot restore what we cannot read.
//! 2. **Not JSON**, or JSON that is not an object — rewriting it would discard
//!    whatever the user meant.
//! 3. **A top-level key other than `hooks`** — Orca's own refusal, and the right
//!    one: this file has a shape we have measured, and a key we do not know about
//!    means we are looking at a newer format we would silently drop.
//! 4. **The file moved while we prepared** — the generation check. Between reading
//!    and writing, Codex or the user may have rewritten it, and our write would
//!    be based on bytes that no longer exist.
//! 5. **Nothing to change** — an identical file is not written at all, so a
//!    reconcile that changes nothing does not touch the mtime.
//!
//! ## Two lanes, tried in this order
//!
//! [`install`] is the first lane and the better one: the hook goes in the user's
//! own `~/.codex/hooks.json` and Codex itself grants the trust, so it runs for
//! every `codex` on the machine, including the ones the user starts.
//!
//! When that grant cannot happen — a Codex too old for `app-server`, a refusal, a
//! machine where the binary will not answer — the rollback leaves no hook at all,
//! which is correct and permanent. [`land`] adds the second lane:
//! [`crate::codex_mirror`], a `CODEX_HOME` we own, where the trust is ours to
//! write because the home is ours. A launch on that lane carries `CODEX_HOME`;
//! the user's own `codex` is untouched.
//!
//! The order is not a preference, it is the consent boundary. See the mirror
//! module for why writing our own hash into the user's config would be forgery.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::codex_trust::{CODEX_EVENTS, TrustEntry};

/// The timeout our hooks carry. Orca's `buildManagedCommandHook` default, and it
/// is part of the trust hash — changing it un-trusts every hook we installed.
pub const MANAGED_TIMEOUT_SECONDS: u64 = 10;

/// Why we did not write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Could not read it, so could not restore it.
    Unreadable(String),
    /// Not JSON, or JSON that is not an object.
    NotAnObject,
    /// A top-level key we do not know. Names it, because that is the one piece of
    /// information that makes this actionable.
    UnknownKey(String),
    /// It changed between reading and writing.
    Moved,
    Failed(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Unreadable(why) => write!(f, "hooks.json could not be read: {why}"),
            Refusal::NotAnObject => write!(f, "hooks.json is not a JSON object"),
            Refusal::UnknownKey(key) => {
                write!(f, "hooks.json carries an unknown top-level key `{key}`")
            }
            Refusal::Moved => write!(f, "hooks.json changed while we prepared the write"),
            Refusal::Failed(why) => write!(f, "hooks.json could not be written: {why}"),
        }
    }
}

impl std::error::Error for Refusal {}

/// The file as we found it: the exact bytes, and the parsed object.
///
/// The raw bytes are kept because they are what a rollback restores. A rollback
/// that re-serialised the parsed form would reformat a file it was supposed to
/// put back.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// `None` when there was no file — which is different from an empty one, and
    /// decides whether a rollback restores or DELETES.
    pub raw: Option<String>,
    pub config: Map<String, Value>,
    /// The permission bits, to be put back.
    pub mode: Option<u32>,
}

/// Where a write actually lands.
///
/// A symlinked `hooks.json` is written THROUGH: replacing the link with a regular
/// file would quietly detach a config somebody deliberately shared between homes
/// (`resolveHooksJsonWritePath`, :684-694). A missing file writes where it was
/// asked for.
pub fn write_path(config_path: &Path) -> PathBuf {
    match std::fs::symlink_metadata(config_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf())
        }
        _ => config_path.to_path_buf(),
    }
}

fn mode_of(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .ok()
            .map(|meta| meta.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Read `hooks.json`, refusing anything we could not put back.
pub fn read(config_path: &Path) -> Result<Found, Refusal> {
    let target = write_path(config_path);
    if !target.exists() {
        return Ok(Found {
            raw: None,
            config: Map::new(),
            mode: None,
        });
    }
    let raw =
        std::fs::read_to_string(&target).map_err(|error| Refusal::Unreadable(error.to_string()))?;
    // An empty file is an object with nothing in it, which is how a config gets
    // created by `touch`. Refusing it would refuse a normal machine.
    let parsed: Value = if raw.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(&raw).map_err(|_| Refusal::NotAnObject)?
    };
    let Value::Object(config) = parsed else {
        return Err(Refusal::NotAnObject);
    };
    // Orca's refusal: this file's measured shape is `{hooks: {...}}` and nothing
    // else. A key we do not know means a format we would silently drop.
    if let Some(unknown) = config.keys().find(|key| key.as_str() != "hooks") {
        return Err(Refusal::UnknownKey(unknown.clone()));
    }
    Ok(Found {
        raw: Some(raw),
        mode: mode_of(&target),
        config,
    })
}

/// Does this command string belong to us?
///
/// Matched on the DIRECTORY-QUALIFIED script name — `.zerocode/agent-hooks/…` —
/// never on the bare file name, exactly as `install::managed_needle` does for
/// every other agent. The doc comment said this from the start; the code
/// matched any `agent-hooks/<script>` until a live install on a machine
/// running Orca beside us planned ORCA's codex hook away: its command is
/// `~/.orca/agent-hooks/codex-hook.sh` — same file name, different dotdir —
/// and a successful grant would have destroyed it (uninstall would have,
/// without needing the grant). The extension stays open the way Orca leaves
/// it: a `.cmd` written by a Windows install of this app is still ours.
pub fn managed_matcher(script_file_name: &str) -> impl Fn(&str) -> bool + '_ {
    let stem = script_file_name
        .trim_end_matches(".sh")
        .trim_end_matches(".cmd")
        .trim_end_matches(".ps1")
        .to_string();
    let needle = format!(".zerocode/agent-hooks/{stem}.");
    move |command: &str| command.contains(needle.as_str())
}

/// The handler object our hooks are (`buildManagedCommandHook`, :730-736).
fn managed_hook(command: &str) -> Value {
    serde_json::json!({
        "type": "command",
        "command": command,
        "timeout": MANAGED_TIMEOUT_SECONDS
    })
}

/// The keys a definition can carry a command under directly.
const DIRECT_KEYS: [&str; 3] = ["command", "bash", "powershell"];

fn is_managed_at(definition: &Value, key: &str, ours: &impl Fn(&str) -> bool) -> bool {
    definition
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(ours)
}

/// Strip every one of our handlers out of a set of definitions, leaving
/// everything else exactly as it was (`removeManagedCommands`, :831-850).
///
/// A definition that held only our handler disappears; one that held ours
/// alongside somebody else's keeps theirs. The point is that uninstalling us is
/// invisible to every other product that writes here.
pub fn remove_managed(definitions: &[Value], ours: &impl Fn(&str) -> bool) -> Vec<Value> {
    let mut kept = Vec::new();
    for definition in definitions {
        let direct: Vec<&str> = DIRECT_KEYS
            .iter()
            .copied()
            .filter(|key| is_managed_at(definition, key, ours))
            .collect();
        let nested_ours = definition
            .get("hooks")
            .and_then(Value::as_array)
            .is_some_and(|hooks| {
                hooks
                    .iter()
                    .any(|hook| is_managed_at(hook, "command", ours))
            });
        if direct.is_empty() && !nested_ours {
            kept.push(definition.clone());
            continue;
        }
        let Some(map) = definition.as_object() else {
            continue;
        };
        let mut next = map.clone();
        for key in direct {
            next.remove(key);
        }
        if nested_ours {
            let remaining: Vec<Value> = definition
                .get("hooks")
                .and_then(Value::as_array)
                .map(|hooks| {
                    hooks
                        .iter()
                        .filter(|hook| !is_managed_at(hook, "command", ours))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if remaining.is_empty() {
                next.remove("hooks");
            } else {
                next.insert("hooks".to_string(), Value::Array(remaining));
            }
        }
        // A definition with nothing left to run is dropped rather than left as an
        // empty shell.
        let still_runs = DIRECT_KEYS
            .iter()
            .any(|key| next.get(*key).and_then(Value::as_str).is_some())
            || next
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hooks| !hooks.is_empty());
        if still_runs {
            kept.push(Value::Object(next));
        }
    }
    kept
}

/// Where our handler ended up, which is what a trust key is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub group_index: usize,
    pub handler_index: usize,
}

/// Put our handler into one event's definitions, in place when we can.
///
/// The in-place branch is what keeps trust from being revoked on every reconcile:
/// our handler's trust key contains its group and handler index, so moving it
/// changes the key and un-trusts it. So when the file already holds exactly one
/// of ours, in a group with no matcher and no direct command, it is REPLACED where
/// it sits (`reconcileManagedHookDefinition`, out/main/index.js:213403-213439).
/// Otherwise ours are removed and one is appended.
pub fn reconcile(
    current: &[Value],
    ours: &impl Fn(&str) -> bool,
    command: &str,
) -> (Vec<Value>, Placed) {
    let has_direct = current.iter().any(|definition| {
        DIRECT_KEYS
            .iter()
            .any(|key| is_managed_at(definition, key, ours))
    });
    let nested: Vec<Placed> = current
        .iter()
        .enumerate()
        .flat_map(|(group_index, definition)| {
            definition
                .get("hooks")
                .and_then(Value::as_array)
                .map(|hooks| {
                    hooks
                        .iter()
                        .enumerate()
                        .filter(|(_, hook)| is_managed_at(hook, "command", ours))
                        .map(move |(handler_index, _)| Placed {
                            group_index,
                            handler_index,
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect();

    if !has_direct && nested.len() == 1 {
        let at = nested[0];
        if let Some(definition) = current.get(at.group_index) {
            let definition_has_direct = DIRECT_KEYS
                .iter()
                .any(|key| definition.get(*key).and_then(Value::as_str).is_some());
            if definition.get("matcher").is_none() && !definition_has_direct {
                let mut definitions = current.to_vec();
                let mut hooks = definition
                    .get("hooks")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                if at.handler_index < hooks.len() {
                    hooks[at.handler_index] = managed_hook(command);
                    let mut map = definition.as_object().cloned().unwrap_or_default();
                    map.insert("hooks".to_string(), Value::Array(hooks));
                    definitions[at.group_index] = Value::Object(map);
                    return (definitions, at);
                }
            }
        }
    }

    let mut definitions = remove_managed(current, ours);
    let group_index = definitions.len();
    definitions.push(serde_json::json!({ "hooks": [managed_hook(command)] }));
    (
        definitions,
        Placed {
            group_index,
            handler_index: 0,
        },
    )
}

/// What a write will do, before it does it.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The whole file, after.
    pub config: Map<String, Value>,
    /// One per event, in `CODEX_EVENTS` order, ready to be turned into trust keys.
    pub entries: Vec<TrustEntry>,
    /// False when the file already says exactly this.
    pub changes: bool,
}

/// Work out the file our hook belongs in, and where each handler landed.
///
/// Events we no longer use are swept: a handler of ours left on an event that has
/// been dropped from [`CODEX_EVENTS`] would keep firing at a bridge that stopped
/// expecting it.
pub fn plan(found: &Found, config_path: &Path, script_file_name: &str, command: &str) -> Plan {
    let ours = managed_matcher(script_file_name);
    let existing = found
        .config
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut hooks = existing.clone();
    let mut entries = Vec::new();

    for (event_name, event_label) in CODEX_EVENTS {
        let current = hooks
            .get(event_name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let (definitions, at) = reconcile(&current, &ours, command);
        hooks.insert(event_name.to_string(), Value::Array(definitions));
        entries.push(TrustEntry {
            source_path: config_path.to_path_buf(),
            event_label: event_label.to_string(),
            group_index: at.group_index,
            handler_index: at.handler_index,
            command: command.to_string(),
            timeout_seconds: Some(MANAGED_TIMEOUT_SECONDS),
            async_hook: false,
            matcher: None,
            status_message: None,
        });
    }

    // Every other event: ours come out. An event left with nothing is removed
    // rather than kept as an empty array, so uninstalling leaves no trace.
    let managed: Vec<&str> = CODEX_EVENTS.iter().map(|(name, _)| *name).collect();
    let others: Vec<String> = hooks
        .keys()
        .filter(|key| !managed.contains(&key.as_str()))
        .cloned()
        .collect();
    for event in others {
        let Some(definitions) = hooks.get(&event).and_then(Value::as_array).cloned() else {
            continue;
        };
        let cleaned = remove_managed(&definitions, &ours);
        if cleaned.is_empty() {
            hooks.remove(&event);
        } else {
            hooks.insert(event, Value::Array(cleaned));
        }
    }

    let mut config = found.config.clone();
    config.insert("hooks".to_string(), Value::Object(hooks.clone()));
    Plan {
        changes: serialize(&config) != found.raw.clone().unwrap_or_default(),
        config,
        entries,
    }
}

/// The bytes a config becomes: two-space JSON with a trailing newline, which is
/// what Orca writes and therefore what an unchanged file compares equal to.
pub fn serialize(config: &Map<String, Value>) -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(config).unwrap_or_default()
    )
}

/// Keep the file as it was, once, before we have ever changed it.
///
/// Once — a second backup would overwrite the pre-install copy with our own
/// output, which is exactly the thing somebody reaching for a backup wants back.
pub fn backup_once(state_dir: &Path, previous_raw: Option<&str>) -> Result<(), Refusal> {
    let Some(raw) = previous_raw else {
        return Ok(());
    };
    let backup = state_dir.join("hooks.json.pre-zerocode");
    if backup.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(state_dir).map_err(|error| Refusal::Failed(error.to_string()))?;
    write_atomically(&backup, raw, Some(0o600)).map_err(|error| Refusal::Failed(error.to_string()))
}

pub(crate) fn write_atomically(path: &Path, body: &str, mode: Option<u32>) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    // The temp name carries our own prefix so a sweep can tell its leftovers from
    // another writer's.
    let temp = dir.join(format!(".zerocode-hooks-{}.tmp", std::process::id()));
    std::fs::write(&temp, body)?;
    // Unix gets the exact mode back (the undo restores "the bytes and the
    // mode"); elsewhere only an owner-only mode has a road.
    if let Some(mode) = mode {
        crate::private_file::apply_mode(&temp, mode)?;
    }
    let renamed = std::fs::rename(&temp, path);
    if renamed.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    renamed
}

/// Remove our groups from one hooks file, as it stands NOW.
///
/// Shared by `unland` (both lanes) and `retract` (a rollback that found a
/// neighbour's newer file where its own write used to be): in both, the file's
/// present content is the truth and only our handlers may leave it.
fn sweep_ours(path: &Path, ours: &impl Fn(&str) -> bool) -> Result<(), Refusal> {
    let found = read(path)?;
    let mut hooks = found
        .config
        .get("hooks")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for event in hooks.keys().cloned().collect::<Vec<_>>() {
        let Some(definitions) = hooks.get(&event).and_then(Value::as_array).cloned() else {
            continue;
        };
        let cleaned = remove_managed(&definitions, ours);
        if cleaned.is_empty() {
            hooks.remove(&event);
        } else {
            hooks.insert(event, Value::Array(cleaned));
        }
    }
    let mut config = found.config.clone();
    // An empty `hooks` the user already had is theirs and stays. Removing it
    // because it ended up empty would make uninstalling edit a key we never
    // wrote — tidier, and not ours to tidy.
    if found.config.contains_key("hooks") {
        config.insert("hooks".to_string(), Value::Object(hooks));
    } else {
        config.remove("hooks");
    }
    write(path, &found, &config)?;
    Ok(())
}

/// Take a landing back without erasing a neighbour's meanwhile.
///
/// A rollback may run long after the write it undoes — the grant in between
/// holds a whole app-server conversation — and the other tenant of this file
/// (the real Orca reconciles its own hooks on a timer) may have rewritten it.
/// Restoring the pre-install bytes over that would erase THEIR change: the very
/// damage `assert_unchanged` refuses on the way in. So the file is read again:
/// still byte-identical to what we wrote → the original bytes come back,
/// formatting and mode and all; anything else → the file as it stands is the
/// truth, and only our groups leave it.
fn retract(
    config_path: &Path,
    found: &Found,
    planned: &Map<String, Value>,
    script_file_name: &str,
) -> Result<(), Refusal> {
    let target = write_path(config_path);
    let now = if target.exists() {
        std::fs::read_to_string(&target).ok()
    } else {
        None
    };
    if now.as_deref() == Some(serialize(planned).as_str()) {
        return restore(config_path, found);
    }
    sweep_ours(config_path, &managed_matcher(script_file_name))
}

/// Refuse if the file is not the one we read.
///
/// Between the read and the write, Codex or the user may have rewritten it. Our
/// write would be based on bytes that no longer exist, and — worse — our rollback
/// would restore over their change.
pub fn assert_unchanged(config_path: &Path, expected: Option<&str>) -> Result<(), Refusal> {
    let target = write_path(config_path);
    let now = if target.exists() {
        std::fs::read_to_string(&target).ok()
    } else {
        None
    };
    if now.as_deref() == expected {
        Ok(())
    } else {
        Err(Refusal::Moved)
    }
}

/// Write the planned file, keeping the mode it had.
///
/// An identical file is not written at all — a reconcile that changed nothing
/// must not touch the mtime, because something else is watching this file.
pub fn write(
    config_path: &Path,
    found: &Found,
    config: &Map<String, Value>,
) -> Result<bool, Refusal> {
    let target = write_path(config_path);
    let body = serialize(config);
    if found.raw.as_deref() == Some(body.as_str()) {
        return Ok(false);
    }
    assert_unchanged(config_path, found.raw.as_deref())?;
    write_atomically(&target, &body, found.mode)
        .map(|()| true)
        .map_err(|error| Refusal::Failed(error.to_string()))
}

/// Put the file back exactly as it was.
///
/// `None` means there was no file, and then a rollback DELETES rather than
/// writing an empty one — leaving a `hooks.json` behind on a machine that never
/// had one is its own kind of damage.
pub fn restore(config_path: &Path, found: &Found) -> Result<(), Refusal> {
    let target = write_path(config_path);
    match &found.raw {
        None => {
            if target.exists() {
                std::fs::remove_file(&target)
                    .map_err(|error| Refusal::Failed(error.to_string()))?;
            }
            Ok(())
        }
        Some(raw) => write_atomically(&target, raw, found.mode)
            .map_err(|error| Refusal::Failed(error.to_string())),
    }
}

/// How an install ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Installed {
    /// Written and trusted. The hook will run.
    Trusted { wrote_trust: bool },
    /// Nothing needed doing — already written and already trusted.
    Unchanged,
    /// The grant did not happen, so the hook was **taken back out**. The file is
    /// as it was. Carries why, so the window can say whether to retry.
    RolledBack {
        why: String,
        /// False for an `Unsupported` grant: this Codex will never do it, and a
        /// retry timer on it is a timer that never stops.
        retryable: bool,
    },
}

/// Write the hook, ask Codex to trust it, and undo the write if it will not.
///
/// The order is forced by the mechanism: Codex can only be asked about a hook it
/// can see, so the file has to be written BEFORE the grant — which is why the
/// rollback is not optional. Every exit from this function leaves either a
/// trusted hook or the file we found.
///
/// `state_dir` is where the one pre-install backup goes. `grant` is passed in so
/// this is testable against a server that refuses.
pub fn install(
    config_path: &Path,
    state_dir: &Path,
    script_file_name: &str,
    command: &str,
    grant: impl Fn(&[TrustEntry]) -> Result<crate::codex_grant::Grant, crate::codex_grant::GrantError>,
) -> Result<Installed, Refusal> {
    let found = read(config_path)?;
    let planned = plan(&found, config_path, script_file_name, command);
    // The backup first, and only ever from the file as we found it.
    backup_once(state_dir, found.raw.as_deref())?;
    let wrote = write(config_path, &found, &planned.config)?;

    match grant(&planned.entries) {
        Ok(crate::codex_grant::Grant::Granted { wrote_trust, .. }) => {
            if wrote || wrote_trust {
                Ok(Installed::Trusted { wrote_trust })
            } else {
                Ok(Installed::Unchanged)
            }
        }
        Ok(crate::codex_grant::Grant::VerifyFailed { reason, class }) => {
            // Only undo what we did. A file we never changed is left alone —
            // restoring it anyway would be a write nobody asked for.
            if wrote {
                retract(config_path, &found, &planned.config, script_file_name)?;
            }
            Ok(Installed::RolledBack {
                why: format!("{class:?}: {reason}"),
                retryable: true,
            })
        }
        Err(error) => {
            if wrote {
                retract(config_path, &found, &planned.config, script_file_name)?;
            }
            let retryable = !matches!(error, crate::codex_grant::GrantError::Unsupported(_));
            Ok(Installed::RolledBack {
                why: error.to_string(),
                retryable,
            })
        }
    }
}

/// Where a Codex hook ended up, and what a launch has to do about it.
#[derive(Debug, Clone, PartialEq)]
pub enum Landed {
    /// In the user's own home, trusted by Codex. A launch needs nothing extra —
    /// and the hook also runs for `codex` started outside this window, which is
    /// why this lane is tried first.
    System { wrote_trust: bool },
    /// Already so. Nothing was written.
    Unchanged,
    /// The grant could not happen, so the user's file was put back and the hook
    /// went into a home we own. **A launch must set `CODEX_HOME` to `home`**, or
    /// the hook is written where nothing reads it.
    Mirror {
        home: crate::codex_mirror::Home,
        /// Why the first lane failed, for the window to say.
        why: String,
        /// Whether asking Codex again could ever work.
        retryable: bool,
    },
    /// Both lanes failed. No hook, and nothing of the user's was changed.
    Nowhere { why: String },
}

/// Write the hook wherever it can be trusted: the user's home first, our mirror
/// second.
///
/// `user_data` is the app's data directory — the mirror is built under it, never
/// from a caller-supplied path, which is what makes the mirror incapable of being
/// the user's own home.
///
/// The mirror lane **verifies by reading back** with the same reader that judges
/// the system home ([`crate::codex_trust::trust_of`]). Writing trust and trusting
/// that we wrote it are different claims: a quoting slip in a key, or two hashes
/// left under one key, produces a file that looks written and reads as untrusted,
/// and the symptom is an agent stalled on a review prompt. So the answer here is
/// what the reader says, not what the writer intended.
pub fn land(
    config_path: &Path,
    state_dir: &Path,
    user_data: &Path,
    system_home: &Path,
    script_file_name: &str,
    command: &str,
    grant: impl Fn(&[TrustEntry]) -> Result<crate::codex_grant::Grant, crate::codex_grant::GrantError>,
) -> Result<Landed, Refusal> {
    let mut attempt = install(config_path, state_dir, script_file_name, command, &grant)?;
    // One more try when the rollback is the retryable kind. The measured cause
    // on this machine is a race, not a verdict: the file's other tenant (the
    // real Orca) reconciles its own hooks on a timer, and a landing of ours
    // that straddles that rewrite fails its listing. A race passes on the
    // second attempt — and it is ONE retry, not a loop, because two collisions
    // in a row mean the timer is winning today and the mirror exists for
    // exactly that day.
    if matches!(
        attempt,
        Installed::RolledBack {
            retryable: true,
            ..
        }
    ) {
        attempt = install(config_path, state_dir, script_file_name, command, &grant)?;
    }
    let (why, retryable) = match attempt {
        Installed::Trusted { wrote_trust } => return Ok(Landed::System { wrote_trust }),
        Installed::Unchanged => return Ok(Landed::Unchanged),
        Installed::RolledBack { why, retryable } => (why, retryable),
    };

    let home = crate::codex_mirror::Home::under(user_data);
    let nowhere = |what: String| {
        Ok(Landed::Nowhere {
            why: format!("{why}; {what}"),
        })
    };

    // The settings first, so the agent in the mirror behaves like the user's own.
    if let Err(error) = crate::codex_mirror::sync(&home, system_home) {
        return nowhere(error.to_string());
    }
    let hooks_path = home.hooks_path();
    let found = match read(&hooks_path) {
        Ok(found) => found,
        Err(refusal) => return nowhere(refusal.to_string()),
    };
    let planned = plan(&found, &hooks_path, script_file_name, command);
    if let Err(refusal) = write(&hooks_path, &found, &planned.config) {
        return nowhere(refusal.to_string());
    }
    if let Err(error) = crate::codex_mirror::write_trust(&home, &planned.entries) {
        return nowhere(error.to_string());
    }

    let states = crate::codex_trust::read_trust_states(&home.config_path());
    if let Some(untrusted) = planned
        .entries
        .iter()
        .find(|entry| !crate::codex_trust::is_trusted(entry, &states))
    {
        let verdict = crate::codex_trust::trust_of(untrusted, &states);
        return nowhere(format!(
            "미러에 쓴 신뢰가 다시 읽히지 않습니다 ({}: {verdict:?})",
            untrusted.event_label
        ));
    }
    Ok(Landed::Mirror {
        home,
        why,
        retryable,
    })
}

/// Take the hook out of both lanes.
///
/// Both, unconditionally, because "off" has to mean off: a mirror entry left
/// behind would keep firing at a bridge for every launch that still carries
/// `CODEX_HOME`, and the user turned the feature off.
pub fn unland(
    config_path: &Path,
    user_data: &Path,
    script_file_name: &str,
    command: &str,
) -> Result<(), Refusal> {
    let ours = managed_matcher(script_file_name);
    for path in [
        config_path.to_path_buf(),
        crate::codex_mirror::Home::under(user_data).hooks_path(),
    ] {
        sweep_ours(&path, &ours)?;
    }

    // And the trust. The mirror's is ours wholesale; the system home's
    // config.toml can hold RESIDUE — a grant plants trust before the final
    // verification, and a rollback takes back only the hooks file (measured
    // on this machine: eight such blocks, at a group index that no longer
    // exists). The identity hash carries no position, so residue is known by
    // its content: only a block whose every hash is this command's own
    // identity is swept. A hash we cannot reproduce is somebody else's
    // consent, and it stays whatever key it sits under.
    let home = crate::codex_mirror::Home::under(user_data);
    let found = read(&home.hooks_path())?;
    let keys: Vec<String> = plan(&found, &home.hooks_path(), script_file_name, command)
        .entries
        .iter()
        .map(crate::codex_trust::trust_key)
        .collect();
    let _ = crate::codex_mirror::remove_trust(&home, &keys);

    let system_hooks = write_path(config_path);
    let system_config = system_hooks.with_file_name("config.toml");
    let our_hashes: std::collections::BTreeSet<String> = CODEX_EVENTS
        .iter()
        .map(|(_, label)| {
            crate::codex_trust::trusted_hash(&TrustEntry {
                source_path: system_hooks.clone(),
                event_label: (*label).to_string(),
                group_index: 0,
                handler_index: 0,
                command: command.to_string(),
                timeout_seconds: Some(MANAGED_TIMEOUT_SECONDS),
                async_hook: false,
                matcher: None,
                status_message: None,
            })
        })
        .collect();
    let prefix = format!("{}:", system_hooks.to_string_lossy());
    let residue: Vec<String> = crate::codex_trust::read_trust_states(&system_config)
        .into_iter()
        .filter(|(key, state)| {
            key.starts_with(&prefix)
                && !state.trusted_hashes.is_empty()
                && state
                    .trusted_hashes
                    .iter()
                    .all(|hash| our_hashes.contains(hash))
        })
        .map(|(key, _)| key)
        .collect();
    if !residue.is_empty() {
        let _ = crate::codex_mirror::remove_trust_at(&system_config, &residue);
    }
    Ok(())
}

// ------------------------------------------------------ the window's surface

/// Where the user's Codex home is (`getSystemCodexHomePath`,
/// managed-agent-hook-controls-Hy3KYvcp.js:443-445), honouring an explicit
/// `CODEX_HOME` — a user who moved their home meant it. App-owned homes are
/// excluded: a test or nested shell inherits those from its parent pane, and
/// they are precisely the mirrors/managed accounts this source must not treat
/// as the person's independently owned login.
pub fn system_home(home: &Path) -> PathBuf {
    let configured = std::env::var("CODEX_HOME").ok().map(PathBuf::from);
    system_home_from(home, configured.as_deref())
}

fn system_home_from(home: &Path, configured: Option<&Path>) -> PathBuf {
    configured
        .and_then(Path::to_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| !is_app_owned_codex_home(path))
        .unwrap_or_else(|| home.join(".codex"))
}

pub(crate) fn is_app_owned_codex_home(path: &Path) -> bool {
    let parts: Vec<_> = path
        .components()
        .filter_map(|part| match part {
            std::path::Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect();
    (parts.len() >= 2
        && parts[parts.len() - 2].eq_ignore_ascii_case(crate::codex_mirror::HOME_SEGMENTS[0])
        && parts[parts.len() - 1].eq_ignore_ascii_case(crate::codex_mirror::HOME_SEGMENTS[1]))
        || (parts.len() >= 3
            && parts[parts.len() - 3].eq_ignore_ascii_case("codex-accounts")
            && parts[parts.len() - 1].eq_ignore_ascii_case("home"))
}

/// Everything the two lanes need, gathered once.
#[derive(Debug, Clone)]
pub struct Lane {
    /// The user's `$HOME` — where the hook script lives.
    pub home: PathBuf,
    /// The app's data directory. The mirror is built under it.
    pub user_data: PathBuf,
    /// Where the one pre-install backup goes.
    pub state_dir: PathBuf,
}

impl Lane {
    fn script_file_name(&self) -> String {
        crate::install::script_file_name(zerocode_core::AgentKind::Codex)
    }

    fn command(&self) -> String {
        let host =
            crate::install::ScriptHost::current().for_vendor(zerocode_core::AgentKind::Codex);
        crate::install::wrapper(
            host,
            &crate::install::script_path(&self.home, zerocode_core::AgentKind::Codex),
            &[],
        )
    }

    fn config_path(&self) -> PathBuf {
        system_home(&self.home).join("hooks.json")
    }
}

/// What a launch has to carry for the hook to be heard.
///
/// Empty on the system lane, which is what "the user's own Codex also reports"
/// looks like from here. One variable on the mirror lane.
pub fn launch_env(lane: &Lane) -> Vec<(String, String)> {
    launch_env_with_lock(lane).0
}

/// Give a running mirror another chance to return rotated credentials.
///
/// Launch preparation uses the lock-carrying form below because its lease has
/// to survive through child creation. A terminal that is already ending has
/// no child to create, so the complete bidirectional sync is its one job.
pub fn sync_auth(lane: &Lane) -> std::io::Result<crate::codex_runtime_auth::AuthSync> {
    let mirror = crate::codex_mirror::Home::under(&lane.user_data);
    crate::codex_runtime_auth::sync(&mirror, &system_home(&lane.home))
}

/// Prepare a Codex launch and keep the mirror lease until its child exists.
///
/// The ordinary environment API intentionally remains a `Vec` for its many
/// existing callers. A worker summon needs the second half of the transaction,
/// though: returning the lease lets the shell hold it across trust setup and
/// `PtyLane::spawn`, so four workers cannot all present one refresh token to
/// Codex at once. A missing lease never refuses a launch; it only falls back to
/// the historical best-effort sync behavior.
pub fn launch_env_with_lock(
    lane: &Lane,
) -> (
    Vec<(String, String)>,
    Option<crate::codex_runtime_auth::LaunchLock>,
) {
    let home = crate::codex_mirror::Home::under(&lane.user_data);
    let lock = crate::codex_runtime_auth::acquire_launch_lock(&home).ok();
    // The mirror is only the active lane when it actually holds a trusted hook.
    // Asking the question this way — read the files, do not remember a decision —
    // means a mirror emptied by hand stops being used on the next launch rather
    // than sending every Codex to a home with no hook in it.
    let Ok(found) = read(&home.hooks_path()) else {
        return (Vec::new(), None);
    };
    let script = lane.script_file_name();
    let ours = managed_matcher(&script);
    let has_ours = found
        .config
        .get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|hooks| {
            hooks
                .values()
                .filter_map(Value::as_array)
                .flatten()
                .any(|definition| {
                    !remove_managed(std::slice::from_ref(definition), &ours).contains(definition)
                })
        });
    if !has_ours {
        return (Vec::new(), None);
    }
    let states = crate::codex_trust::read_trust_states(&home.config_path());
    let planned = plan(&found, &home.hooks_path(), &script, &lane.command());
    if !planned
        .entries
        .iter()
        .any(|entry| crate::codex_trust::is_trusted(entry, &states))
    {
        return (Vec::new(), None);
    }
    // The login travels with the home. A mirror without `auth.json` asks a
    // signed-in person to sign in again, and a mirror whose Codex rotated
    // tokens must hand them back — both directions live in one sync, and a
    // sync that fails must not cost the launch (Codex asking to log in is a
    // worse screen than this module has, but a better one than no pane).
    if let Some(held) = lock.as_ref() {
        let _ = crate::codex_runtime_auth::sync_locked(held, &home, &system_home(&lane.home));
    } else {
        // The lock is best effort for the same reason sync is: an auth failure
        // must not turn a usable worker into a missing pane.
        let _ = crate::codex_runtime_auth::sync(&home, &system_home(&lane.home));
    }
    (
        vec![(
            "CODEX_HOME".to_string(),
            home.path().to_string_lossy().into_owned(),
        )],
        lock,
    )
}

/// Install the Codex hook, trying the user's home first and the mirror second.
///
/// `grant` is passed in for the same reason [`install`] takes one: a test must be
/// able to run both lanes without a Codex on the machine.
pub fn install_agent(
    lane: &Lane,
    grant: impl Fn(&[TrustEntry]) -> Result<crate::codex_grant::Grant, crate::codex_grant::GrantError>,
) -> crate::install::HookStatus {
    let agent = zerocode_core::AgentKind::Codex;
    let config_path = lane.config_path();
    let say = |state, path: &Path, detail: Option<String>, note| crate::install::HookStatus {
        agent,
        state,
        config_path: path.to_string_lossy().into_owned(),
        detail,
        note,
    };
    if crate::install::write_script(&lane.home, agent).is_err() {
        return say(
            crate::install::HookInstallState::Error,
            &config_path,
            Some("훅 스크립트를 쓰지 못했습니다".into()),
            None,
        );
    }
    let script = lane.script_file_name();
    match land(
        &config_path,
        &lane.state_dir,
        &lane.user_data,
        &system_home(&lane.home),
        &script,
        &lane.command(),
        grant,
    ) {
        Ok(Landed::System { .. }) | Ok(Landed::Unchanged) => say(
            crate::install::HookInstallState::Installed,
            &config_path,
            None,
            None,
        ),
        // Installed, and the note is not an apology: it says which home the hook
        // is in, because that is the difference the user can observe. The WHY
        // rides in the detail — a mirror the user cannot explain is a mirror
        // they cannot fix, and this string is the only place the reason
        // surfaces (the fallback logs nowhere else).
        Ok(Landed::Mirror { home, why, .. }) => say(
            crate::install::HookInstallState::Installed,
            &home.hooks_path(),
            Some(why),
            Some(crate::install::HookNote::MirrorOnly),
        ),
        Ok(Landed::Nowhere { why }) => say(
            crate::install::HookInstallState::Error,
            &config_path,
            Some(why),
            None,
        ),
        Err(refusal) => say(
            crate::install::HookInstallState::Error,
            &config_path,
            Some(refusal.to_string()),
            None,
        ),
    }
}

pub fn remove_agent(lane: &Lane) -> crate::install::HookStatus {
    let _ = unland(
        &lane.config_path(),
        &lane.user_data,
        &lane.script_file_name(),
        &lane.command(),
    );
    status_of(lane)
}

/// What the settings screen shows, reading both lanes and writing nothing.
pub fn status_of(lane: &Lane) -> crate::install::HookStatus {
    let agent = zerocode_core::AgentKind::Codex;
    let script = lane.script_file_name();
    let command = lane.command();
    let mut looked_at = Vec::new();

    for (path, in_mirror) in [
        (lane.config_path(), false),
        (
            crate::codex_mirror::Home::under(&lane.user_data).hooks_path(),
            true,
        ),
    ] {
        let Ok(found) = read(&path) else {
            return crate::install::HookStatus {
                agent,
                state: crate::install::HookInstallState::Error,
                config_path: path.to_string_lossy().into_owned(),
                detail: Some("이 파일을 읽지 못했습니다".into()),
                note: None,
            };
        };
        let planned = plan(&found, &path, &script, &command);
        // `plan` never removes; if it would change nothing, every event already
        // carries our handler exactly as we would write it.
        if planned.changes {
            looked_at.push(path);
            continue;
        }
        let states = if in_mirror {
            crate::codex_trust::read_trust_states(
                &crate::codex_mirror::Home::under(&lane.user_data).config_path(),
            )
        } else {
            crate::codex_trust::read_trust_states(&system_home(&lane.home).join("config.toml"))
        };
        let untrusted: Vec<&TrustEntry> = planned
            .entries
            .iter()
            .filter(|entry| !crate::codex_trust::is_trusted(entry, &states))
            .collect();
        let state = if untrusted.is_empty() {
            crate::install::HookInstallState::Installed
        } else {
            // Written but not trusted is the shape that stalls the agent, so it is
            // reported as partial rather than installed — the whole reason this
            // agent has its own module.
            crate::install::HookInstallState::Partial
        };
        return crate::install::HookStatus {
            agent,
            state,
            config_path: path.to_string_lossy().into_owned(),
            // The verdict is free text because it names an event and a reason
            // this side computed; the two fixed sentences are tokens.
            detail: untrusted.first().map(|entry| {
                format!(
                    "{}: {:?}",
                    entry.event_label,
                    crate::codex_trust::trust_of(entry, &states)
                )
            }),
            note: match (in_mirror, untrusted.is_empty()) {
                (_, false) => Some(crate::install::HookNote::NeedsTrust),
                (true, true) => Some(crate::install::HookNote::MirrorOnly),
                (false, true) => None,
            },
        };
    }

    crate::install::HookStatus {
        agent,
        state: crate::install::HookInstallState::NotInstalled,
        config_path: looked_at
            .first()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
        detail: None,
        note: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OURS: &str = "/bin/sh '/h/.zerocode/agent-hooks/codex-hook.sh'";
    const SCRIPT: &str = "codex-hook.sh";

    fn ours() -> impl Fn(&str) -> bool {
        managed_matcher(SCRIPT)
    }

    /// A test process launched inside a ZeroCode Codex pane inherits the
    /// app-owned mirror as `CODEX_HOME`. That process must not mistake the
    /// fixture target for the person's independently owned Codex home.
    #[test]
    fn a_managed_codex_home_inherited_by_a_test_is_not_the_system_home() {
        let user_home = Path::new("/users/joe");
        for inherited in [
            Path::new("/app/data/codex-runtime-home/home"),
            Path::new("/APP/DATA/CODEX-RUNTIME-HOME/HOME"),
            Path::new("/app/data/codex-accounts/account-1/home"),
        ] {
            assert_eq!(
                system_home_from(user_home, Some(inherited)),
                user_home.join(".codex")
            );
        }
        assert_eq!(
            system_home_from(user_home, Some(Path::new("/external/codex"))),
            Path::new("/external/codex"),
            "a user's independently relocated Codex home was ignored"
        );
    }

    /// The four refusals that protect the file, and the one shape that is fine.
    #[test]
    fn a_file_we_could_not_put_back_is_never_read_as_writable() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("hooks.json");

        // No file at all is the normal machine.
        let missing = read(&path).expect("a missing file is fine");
        assert_eq!(missing.raw, None);
        assert!(missing.config.is_empty());

        // An empty file is an empty object — `touch` made it.
        std::fs::write(&path, "").expect("write");
        assert!(
            read(&path)
                .expect("an empty file is fine")
                .config
                .is_empty()
        );

        std::fs::write(&path, "{ not json").expect("write");
        assert_eq!(read(&path), Err(Refusal::NotAnObject));

        std::fs::write(&path, "[1, 2]").expect("write");
        assert_eq!(read(&path), Err(Refusal::NotAnObject));

        // A key we do not know is a format we would silently drop.
        std::fs::write(&path, r#"{"hooks":{},"version":2}"#).expect("write");
        assert_eq!(read(&path), Err(Refusal::UnknownKey("version".into())));

        std::fs::write(&path, r#"{"hooks":{}}"#).expect("write");
        assert!(read(&path).is_ok());
    }

    /// Our handler goes in, and everything else in the file comes out untouched.
    #[test]
    fn another_products_hooks_survive_ours_going_in_and_coming_out() {
        let theirs = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    { "matcher": "Bash", "hooks": [
                        { "type": "command", "command": "/other/product.sh" }
                    ]}
                ],
                "SomeEventWeDoNotUse": [
                    { "hooks": [{ "type": "command", "command": "/other/thing.sh" }] }
                ]
            }
        });
        let found = Found {
            raw: Some(theirs.to_string()),
            config: theirs.as_object().cloned().expect("object"),
            mode: None,
        };
        let planned = plan(&found, Path::new("/h/hooks.json"), SCRIPT, OURS);
        let hooks = planned.config["hooks"].as_object().expect("hooks");

        // Theirs is still there, with its matcher.
        let pre = hooks["PreToolUse"].as_array().expect("array");
        assert_eq!(pre[0]["matcher"], "Bash");
        assert_eq!(pre[0]["hooks"][0]["command"], "/other/product.sh");
        // Ours was appended rather than merged into their matcher group.
        assert_eq!(pre[1]["hooks"][0]["command"], OURS);
        assert_eq!(pre[1]["hooks"][0]["timeout"], MANAGED_TIMEOUT_SECONDS);
        // Their unrelated event is untouched.
        assert_eq!(
            hooks["SomeEventWeDoNotUse"][0]["hooks"][0]["command"],
            "/other/thing.sh"
        );

        // Every managed event got an entry, and the indices point at ours.
        assert_eq!(planned.entries.len(), CODEX_EVENTS.len());
        let pre_entry = planned
            .entries
            .iter()
            .find(|entry| entry.event_label == "pre_tool_use")
            .expect("an entry for pre_tool_use");
        assert_eq!(pre_entry.group_index, 1);
        assert_eq!(pre_entry.handler_index, 0);

        // And removing ours leaves their file as it started.
        let after = remove_managed(pre, &ours());
        assert_eq!(after.len(), 1);
        assert_eq!(after[0]["hooks"][0]["command"], "/other/product.sh");
        assert_eq!(after[0]["matcher"], "Bash");
    }

    /// Reinstalling keeps our handler WHERE IT IS. The index is part of the trust
    /// key, so moving it un-trusts the hook — a reconcile that shuffled would
    /// make every launch a fresh consent prompt.
    #[test]
    fn a_reinstall_replaces_our_handler_in_place() {
        let before = vec![serde_json::json!({
            "hooks": [{ "type": "command", "command": OURS, "timeout": 10 }]
        })];
        let (after, at) = reconcile(&before, &ours(), OURS);
        assert_eq!(
            at,
            Placed {
                group_index: 0,
                handler_index: 0
            }
        );
        assert_eq!(after.len(), 1, "{after:?}");
        assert_eq!(after[0]["hooks"][0]["command"], OURS);

        // With somebody else's group first, ours still keeps its own index.
        let mixed = vec![
            serde_json::json!({ "hooks": [{ "type": "command", "command": "/other.sh" }] }),
            serde_json::json!({ "hooks": [
                { "type": "command", "command": "/another.sh" },
                { "type": "command", "command": OURS }
            ]}),
        ];
        let (after, at) = reconcile(&mixed, &ours(), OURS);
        assert_eq!(
            at,
            Placed {
                group_index: 1,
                handler_index: 1
            }
        );
        assert_eq!(after.len(), 2);
        assert_eq!(after[1]["hooks"][0]["command"], "/another.sh");
    }

    /// Two of ours, or one in a group carrying a matcher, is not replaced in
    /// place: both are ambiguous, so ours are swept and one is appended.
    #[test]
    fn an_ambiguous_file_is_swept_rather_than_edited_in_place() {
        let twice = vec![
            serde_json::json!({ "hooks": [{ "type": "command", "command": OURS }] }),
            serde_json::json!({ "hooks": [{ "type": "command", "command": OURS }] }),
        ];
        let (after, at) = reconcile(&twice, &ours(), OURS);
        assert_eq!(
            at,
            Placed {
                group_index: 0,
                handler_index: 0
            }
        );
        assert_eq!(after.len(), 1, "a duplicate survived: {after:?}");

        // A matcher on the group means our handler is conditional there, and
        // replacing it in place would keep a condition we did not ask for.
        let conditional = vec![serde_json::json!({
            "matcher": "Bash",
            "hooks": [{ "type": "command", "command": OURS }]
        })];
        let (after, at) = reconcile(&conditional, &ours(), OURS);
        assert_eq!(at.group_index, 0);
        assert_eq!(after.len(), 1);
        assert!(
            after[0].get("matcher").is_none(),
            "the matcher came along: {after:?}"
        );
    }

    /// Ours goes away from an event we no longer use, and the event goes with it
    /// if nothing else was there.
    #[test]
    fn a_handler_on_a_dropped_event_is_swept() {
        let stale = serde_json::json!({
            "hooks": {
                "PreCompact": [{ "hooks": [{ "type": "command", "command": OURS }] }],
                "PostCompact": [
                    { "hooks": [
                        { "type": "command", "command": OURS },
                        { "type": "command", "command": "/theirs.sh" }
                    ]}
                ]
            }
        });
        let found = Found {
            raw: Some(stale.to_string()),
            config: stale.as_object().cloned().expect("object"),
            mode: None,
        };
        let planned = plan(&found, Path::new("/h/hooks.json"), SCRIPT, OURS);
        let hooks = planned.config["hooks"].as_object().expect("hooks");
        assert!(
            !hooks.contains_key("PreCompact"),
            "an emptied event stayed behind: {hooks:?}"
        );
        // Theirs kept its event.
        assert_eq!(hooks["PostCompact"][0]["hooks"][0]["command"], "/theirs.sh");
        assert_eq!(
            hooks["PostCompact"][0]["hooks"]
                .as_array()
                .expect("array")
                .len(),
            1
        );
    }

    /// Another product's `agent-hooks/` directory is not ours. The matcher is
    /// directory-qualified for exactly this.
    #[test]
    fn a_similarly_named_script_from_elsewhere_is_not_ours() {
        let ours = ours();
        assert!(ours("/bin/sh '/h/.zerocode/agent-hooks/codex-hook.sh'"));
        // Extension open: a `.cmd` a Windows install of this app wrote is ours.
        assert!(ours("/h/.zerocode/agent-hooks/codex-hook.cmd"));
        // Orca's own codex hook: the same file name under ITS dotdir. This
        // assertion used to point the other way — any `agent-hooks/codex-hook`
        // was claimed — and on a real machine running Orca beside us the plan
        // stripped Orca's entries; only the grant failing saved them.
        assert!(!ours("/bin/sh '/h/.orca/agent-hooks/codex-hook.sh'"));
        assert!(!ours("/h/other/agent-hooks/codex-hook.cmd"));
        // No qualified segment: a script that merely shares the file name.
        assert!(!ours("/h/scripts/codex-hook.sh"));
        assert!(!ours("/usr/local/bin/codex-hook"));
        assert!(!ours("/h/.other/agent-hooks/their-hook.sh"));
    }

    /// The whole plan, against the file this machine actually had: Orca's
    /// codex hook in every event's first group. Theirs stays where their
    /// trust keys point (`:0:0`), ours lands after (`:1:0`), and an
    /// uninstall takes only ours back out.
    #[test]
    fn a_neighbours_codex_hook_survives_ours_going_in_and_coming_out() {
        let orca = "if [ -f '/h/.orca/agent-hooks/codex-hook.sh' ]; then /bin/sh '/h/.orca/agent-hooks/codex-hook.sh'; fi";
        let theirs = serde_json::json!({
            "hooks": {
                "Stop": [
                    { "hooks": [{ "type": "command", "command": orca, "timeout": 10 }] }
                ]
            }
        });
        let found = Found {
            raw: Some(theirs.to_string()),
            config: theirs.as_object().cloned().expect("object"),
            mode: None,
        };
        let planned = plan(&found, Path::new("/h/hooks.json"), SCRIPT, OURS);
        let stop = planned.config["hooks"]["Stop"].as_array().expect("array");
        assert_eq!(stop.len(), 2, "{stop:?}");
        assert_eq!(stop[0]["hooks"][0]["command"], orca);
        assert_eq!(stop[1]["hooks"][0]["command"], OURS);
        let entry = planned
            .entries
            .iter()
            .find(|entry| entry.event_label == "stop")
            .expect("an entry for stop");
        assert_eq!((entry.group_index, entry.handler_index), (1, 0));
        // And coming out: only ours leaves.
        let after = remove_managed(stop, &ours());
        assert_eq!(after.len(), 1, "{after:?}");
        assert_eq!(after[0]["hooks"][0]["command"], orca);
    }

    /// A write, a rollback, and the file is byte-identical — including its mode.
    #[test]
    fn a_rollback_puts_the_bytes_and_the_mode_back() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("hooks.json");
        // Deliberately not our formatting: a rollback that re-serialised would
        // reformat the file it was supposed to put back.
        let original = "{\n\t\"hooks\": {\"Stop\": []}\n}\n";
        std::fs::write(&path, original).expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("chmod");
        }
        let found = read(&path).expect("read");
        let planned = plan(&found, &path, SCRIPT, OURS);
        assert!(planned.changes);
        assert!(write(&path, &found, &planned.config).expect("write"));
        assert!(std::fs::read_to_string(&path).expect("read").contains(OURS));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o640, "the mode was not preserved");
        }

        restore(&path, &found).expect("restore");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            original,
            "the rollback did not restore the exact bytes"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o640);
        }
    }

    /// An owner-only mode is still the file's own mode on unix: a 0400 or
    /// 0700 `hooks.json` comes back 0400 or 0700 from the install and from
    /// the rollback — not the 0600 of the private-file road, which is for a
    /// file we create, not one we put back.
    #[cfg(unix)]
    #[test]
    fn an_owner_only_mode_comes_back_exactly_not_as_0600() {
        use std::os::unix::fs::PermissionsExt;
        for kept in [0o400, 0o700, 0o500] {
            let dir = tempfile::tempdir().expect("temp");
            let path = dir.path().join("hooks.json");
            let original = "{\n\t\"hooks\": {\"Stop\": []}\n}\n";
            std::fs::write(&path, original).expect("write");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(kept)).expect("chmod");
            let found = read(&path).expect("read");
            let planned = plan(&found, &path, SCRIPT, OURS);
            assert!(write(&path, &found, &planned.config).expect("write"));
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(
                mode & 0o777,
                kept,
                "the install changed a {kept:o} file's mode"
            );
            restore(&path, &found).expect("restore");
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(
                mode & 0o777,
                kept,
                "the rollback changed a {kept:o} file's mode"
            );
        }
    }

    /// A machine that had no `hooks.json` gets none back. Leaving one behind is
    /// its own damage — the file is now something Codex reads.
    #[test]
    fn a_rollback_on_a_machine_with_no_file_deletes_rather_than_empties() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("hooks.json");
        let found = read(&path).expect("read");
        let planned = plan(&found, &path, SCRIPT, OURS);
        assert!(write(&path, &found, &planned.config).expect("write"));
        assert!(path.exists());
        restore(&path, &found).expect("restore");
        assert!(!path.exists(), "a file was left on a machine that had none");
    }

    /// A rollback that finds a neighbour's NEWER file where its own write used
    /// to be sweeps only itself out of it. Restoring the pre-install bytes here
    /// would erase what the neighbour wrote meanwhile — and the neighbour is
    /// not hypothetical: the real Orca on this machine reconciles its hooks on
    /// a timer, which is the measured cause of the rolled-back landings.
    #[test]
    fn a_rollback_after_a_neighbours_rewrite_keeps_their_file() {
        let orca_v1 = "if [ -f '/h/.orca/agent-hooks/codex-hook.sh' ]; then /bin/sh '/h/.orca/agent-hooks/codex-hook.sh'; fi";
        let orca_v2 = "if [ -x '/h/.orca/agent-hooks/codex-hook.sh' ]; then /bin/sh '/h/.orca/agent-hooks/codex-hook.sh'; else :; fi";
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("hooks.json");
        let v1 = serde_json::json!({ "hooks": { "Stop": [
            { "hooks": [{ "type": "command", "command": orca_v1, "timeout": 10 }] }
        ]}});
        std::fs::write(&path, serialize(v1.as_object().expect("object"))).expect("write");
        let found = read(&path).expect("read");
        let planned = plan(&found, &path, SCRIPT, OURS);
        assert!(write(&path, &found, &planned.config).expect("write"));

        // The neighbour reconciles while our grant is out: their v2 replaces
        // the file, and our group is gone with it.
        let v2 = serde_json::json!({ "hooks": { "Stop": [
            { "hooks": [{ "type": "command", "command": orca_v2, "timeout": 10 }] }
        ]}});
        std::fs::write(&path, serialize(v2.as_object().expect("object"))).expect("write");

        retract(&path, &found, &planned.config, SCRIPT).expect("retract");
        let after = std::fs::read_to_string(&path).expect("read");
        assert!(
            after.contains(orca_v2) && !after.contains(orca_v1),
            "the neighbour's newer write did not survive the rollback:\n{after}"
        );
        assert!(
            !after.contains(OURS),
            "our handler survived its own rollback:\n{after}"
        );

        // And the quiet half: when nothing moved, the original bytes come back.
        std::fs::write(&path, serialize(v1.as_object().expect("object"))).expect("write");
        let found = read(&path).expect("read");
        let planned = plan(&found, &path, SCRIPT, OURS);
        assert!(write(&path, &found, &planned.config).expect("write"));
        retract(&path, &found, &planned.config, SCRIPT).expect("retract");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            found.raw.clone().expect("raw"),
            "an undisturbed rollback stopped being byte-exact"
        );
    }

    /// A retryable rollback gets exactly one more attempt. The race that causes
    /// it passes on the second try; a second failure in a row goes to the
    /// mirror — this half only asserts the retry and the landing.
    #[test]
    fn a_retryable_rollback_is_retried_once_and_the_second_landing_stands() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("hooks.json");
        std::fs::write(&path, "{\n  \"hooks\": {}\n}\n").expect("write");
        let state = dir.path().join("state");
        let user_data = dir.path().join("data");
        let system = dir.path().join("system-home");
        let calls = std::cell::Cell::new(0u32);
        let landed = land(
            &path,
            &state,
            &user_data,
            &system,
            SCRIPT,
            OURS,
            |_entries| {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    Ok(crate::codex_grant::Grant::VerifyFailed {
                        reason: "hooks/list reported 0 entries".to_string(),
                        class: crate::codex_grant::VerifyClass::ListMismatch,
                    })
                } else {
                    Ok(crate::codex_grant::Grant::Granted {
                        wrote_trust: false,
                        entries: Vec::new(),
                    })
                }
            },
        )
        .expect("land");
        assert_eq!(calls.get(), 2, "the retry did not happen exactly once");
        assert_eq!(
            landed,
            Landed::System { wrote_trust: false },
            "the second attempt did not land in the user's home"
        );
        let after = std::fs::read_to_string(&path).expect("read");
        assert!(
            after.contains(OURS),
            "the landing left no handler behind:\n{after}"
        );
    }

    /// Uninstalling sweeps the system config's trust residue — the blocks a
    /// grant planted for a landing that later retreated — and ONLY those: a
    /// block is residue when every hash in it is this command's own identity,
    /// whatever stale group index its key sits under. The neighbour's consent
    /// under the same file, and our own hash under a foreign file, both stay.
    #[test]
    fn unland_sweeps_only_our_trust_residue_from_the_system_config() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("hooks.json");
        std::fs::write(&path, "{\n  \"hooks\": {}\n}\n").expect("write");
        let user_data = dir.path().join("data");
        let our_hash = crate::codex_trust::trusted_hash(&TrustEntry {
            source_path: path.clone(),
            event_label: "stop".to_string(),
            group_index: 9,
            handler_index: 9,
            command: OURS.to_string(),
            timeout_seconds: Some(MANAGED_TIMEOUT_SECONDS),
            async_hook: false,
            matcher: None,
            status_message: None,
        });
        let config = dir.path().join("config.toml");
        let body = format!(
            "[hooks.state.\"{p}:stop:1:0\"]\ntrusted_hash = \"{h}\"\n\n\
             [hooks.state.\"{p}:stop:0:0\"]\ntrusted_hash = \"sha256:a-neighbours-consent\"\n\n\
             [hooks.state.\"/elsewhere/hooks.json:stop:1:0\"]\ntrusted_hash = \"{h}\"\n",
            p = path.to_string_lossy(),
            h = our_hash
        );
        std::fs::write(&config, &body).expect("write");

        unland(&path, &user_data, SCRIPT, OURS).expect("unland");
        let after = std::fs::read_to_string(&config).expect("read");
        assert!(
            !after.contains(&format!("{}:stop:1:0\"", path.to_string_lossy())),
            "our residue block survived the uninstall:\n{after}"
        );
        assert!(
            after.contains("sha256:a-neighbours-consent"),
            "a neighbour's consent was swept:\n{after}"
        );
        assert!(
            after.contains("/elsewhere/hooks.json"),
            "a foreign file's block was swept:\n{after}"
        );
    }

    /// A file that changed under us is not written, and not restored over.
    #[test]
    fn a_file_that_moved_while_we_prepared_is_refused() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("hooks.json");
        std::fs::write(&path, "{\"hooks\":{}}\n").expect("write");
        let found = read(&path).expect("read");
        let planned = plan(&found, &path, SCRIPT, OURS);
        // Somebody else writes between our read and our write.
        std::fs::write(&path, "{\"hooks\":{\"Stop\":[]}}\n").expect("write");
        assert_eq!(
            write(&path, &found, &planned.config),
            Err(Refusal::Moved),
            "a stale write went through"
        );
        // And theirs is still theirs.
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "{\"hooks\":{\"Stop\":[]}}\n"
        );
    }

    /// Writing what is already there does nothing at all, so a reconcile that
    /// changes nothing does not touch the mtime something else is watching.
    #[test]
    fn an_identical_file_is_not_rewritten() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("hooks.json");
        let found = read(&path).expect("read");
        let planned = plan(&found, &path, SCRIPT, OURS);
        assert!(write(&path, &found, &planned.config).expect("write"));

        let again = read(&path).expect("read");
        let replanned = plan(&again, &path, SCRIPT, OURS);
        assert!(!replanned.changes, "a settled file reported changes");
        assert!(
            !write(&path, &again, &replanned.config).expect("write"),
            "an identical file was rewritten"
        );
    }

    /// The backup is the file as it was BEFORE us, and a second install does not
    /// overwrite it with our own output.
    #[test]
    fn the_backup_is_taken_once_and_holds_the_pre_install_file() {
        let dir = tempfile::tempdir().expect("temp");
        let state = dir.path().join("state");
        backup_once(&state, Some("original")).expect("backup");
        backup_once(&state, Some("ours, later")).expect("backup again");
        assert_eq!(
            std::fs::read_to_string(state.join("hooks.json.pre-zerocode")).expect("read"),
            "original"
        );
        // Nothing to back up when there was no file.
        let empty = dir.path().join("empty");
        backup_once(&empty, None).expect("backup");
        assert!(!empty.join("hooks.json.pre-zerocode").exists());
    }

    /// The whole install, and the property the rollback exists for: a grant that
    /// refuses leaves the file **byte-identical**.
    ///
    /// This is the test that makes it safe to write before asking. Codex cannot be
    /// asked about a hook it cannot see, so the write has to come first — and the
    /// only thing that makes that acceptable is that a refusal undoes it
    /// completely.
    #[test]
    fn a_refused_grant_leaves_the_file_exactly_as_it_was() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("hooks.json");
        let state = dir.path().join("state");
        let original = "{\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n          {\n            \"type\": \"command\",\n            \"command\": \"/theirs.sh\"\n          }\n        ]\n      }\n    ]\n  }\n}\n";
        std::fs::write(&path, original).expect("write");

        // Every refusal, and each one has to end the same way.
        // Named, because clippy is right that the inline form is unreadable —
        // and this is the list of every way a grant can decline.
        type Refuse = Box<
            dyn Fn(
                &[TrustEntry],
            )
                -> Result<crate::codex_grant::Grant, crate::codex_grant::GrantError>,
        >;
        let refusals: Vec<Refuse> = vec![
            Box::new(|_| {
                Ok(crate::codex_grant::Grant::VerifyFailed {
                    reason: "0 of 8".into(),
                    class: crate::codex_grant::VerifyClass::ListMismatch,
                })
            }),
            Box::new(|_| Err(crate::codex_grant::GrantError::Timeout("slow".into()))),
            Box::new(|_| Err(crate::codex_grant::GrantError::Unsupported("old".into()))),
            Box::new(|_| Err(crate::codex_grant::GrantError::Failed("broke".into()))),
        ];
        for (at, refuse) in refusals.into_iter().enumerate() {
            let outcome = install(&path, &state, SCRIPT, OURS, refuse).expect("install ran");
            let Installed::RolledBack { retryable, .. } = outcome else {
                panic!("a refused grant was not rolled back: {outcome:?}");
            };
            // The third one is `Unsupported`, which must not be retried.
            assert_eq!(retryable, at != 2, "case {at}");
            assert_eq!(
                std::fs::read_to_string(&path).expect("read"),
                original,
                "case {at} left the file changed"
            );
        }

        // And the backup holds what was there before any of that.
        assert_eq!(
            std::fs::read_to_string(state.join("hooks.json.pre-zerocode")).expect("read"),
            original
        );
    }

    /// The second lane. A refused grant is no longer the end: the user's file
    /// goes back exactly as it was, and the hook lands in a home we own with
    /// trust that reads back as trust.
    #[test]
    fn a_refused_grant_falls_to_a_home_we_own_and_leaves_theirs_alone() {
        let dir = tempfile::tempdir().expect("temp");
        let system_home = dir.path().join("dot-codex");
        std::fs::create_dir_all(&system_home).expect("mkdir");
        let their_hooks = system_home.join("hooks.json");
        let their_config = system_home.join("config.toml");
        let user_data = dir.path().join("app-data");
        let state = dir.path().join("state");

        let original_hooks = "{\n  \"hooks\": {}\n}\n";
        std::fs::write(&their_hooks, original_hooks).expect("write");
        let original_config = "model = \"gpt-5\"\nlog_dir = \"logs\"\n";
        std::fs::write(&their_config, original_config).expect("write");

        let landed = land(
            &their_hooks,
            &state,
            &user_data,
            &system_home,
            SCRIPT,
            OURS,
            |_| {
                Err(crate::codex_grant::GrantError::Unsupported(
                    "too old".into(),
                ))
            },
        )
        .expect("land ran");
        let Landed::Mirror {
            home,
            retryable,
            why,
        } = landed
        else {
            panic!("a refused grant did not reach the mirror: {landed:?}");
        };
        // `Unsupported` will never work, so the window must not put a retry timer
        // on it — but the hook still runs, which is the point of the lane.
        assert!(!retryable, "{why}");

        // Nothing of the user's moved. Both files, not just the one we wrote.
        assert_eq!(
            std::fs::read_to_string(&their_hooks).expect("read"),
            original_hooks
        );
        assert_eq!(
            std::fs::read_to_string(&their_config).expect("read"),
            original_config
        );

        // The mirror carries the hook on every event, with the settings mirrored
        // and the relative path pinned to where it came from.
        let mirrored_hooks = std::fs::read_to_string(home.hooks_path()).expect("read");
        for (event, _) in CODEX_EVENTS {
            assert!(
                mirrored_hooks.contains(event),
                "{event} missing: {mirrored_hooks}"
            );
        }
        let mirrored_config = std::fs::read_to_string(home.config_path()).expect("read");
        assert!(
            mirrored_config.contains("model = \"gpt-5\""),
            "{mirrored_config}"
        );
        assert!(
            mirrored_config.contains(&format!("'{}/logs'", system_home.to_string_lossy())),
            "{mirrored_config}"
        );

        // And the trust reads as trust for every event — which is the only claim
        // that matters, because Codex reads the file, not our intent.
        let found = read(&home.hooks_path()).expect("read back");
        let planned = plan(&found, &home.hooks_path(), SCRIPT, OURS);
        let states = crate::codex_trust::read_trust_states(&home.config_path());
        for entry in &planned.entries {
            assert_eq!(
                crate::codex_trust::trust_of(entry, &states),
                crate::codex_trust::CodexTrust::Trusted,
                "{} is not trusted in the mirror: {mirrored_config}",
                entry.event_label
            );
        }

        // Off means off in both lanes, trust included.
        unland(&their_hooks, &user_data, SCRIPT, OURS).expect("unland");
        let after = std::fs::read_to_string(home.hooks_path()).expect("read");
        assert!(!after.contains(SCRIPT), "the mirror kept our hook: {after}");
        let cleared = std::fs::read_to_string(home.config_path()).expect("read");
        assert!(
            !cleared.contains("hooks.state"),
            "trust outlived the hook it was for: {cleared}"
        );
        // The user's settings are still in the mirror — uninstalling a hook is not
        // a reason to throw the mirror away.
        assert!(cleared.contains("model = \"gpt-5\""), "{cleared}");
        // And their own files are still untouched.
        assert_eq!(
            std::fs::read_to_string(&their_hooks).expect("read"),
            original_hooks
        );
    }

    /// A grant that succeeds never touches the mirror — the better lane is the
    /// one that runs for the user's own `codex` too, so it must win outright.
    #[test]
    fn a_granted_hook_never_reaches_the_mirror() {
        let dir = tempfile::tempdir().expect("temp");
        let system_home = dir.path().join("dot-codex");
        std::fs::create_dir_all(&system_home).expect("mkdir");
        let user_data = dir.path().join("app-data");

        let landed = land(
            &system_home.join("hooks.json"),
            &dir.path().join("state"),
            &user_data,
            &system_home,
            SCRIPT,
            OURS,
            |_| {
                Ok(crate::codex_grant::Grant::Granted {
                    wrote_trust: true,
                    entries: Vec::new(),
                })
            },
        )
        .expect("land ran");
        assert_eq!(landed, Landed::System { wrote_trust: true });
        assert!(
            !crate::codex_mirror::Home::under(&user_data).path().exists(),
            "a granted install built a mirror it does not need"
        );
    }

    /// A grant that succeeds keeps the hook, and a second install of the same
    /// hook is `Unchanged` rather than another write.
    #[test]
    fn a_granted_hook_stays_and_settles() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("hooks.json");
        let state = dir.path().join("state");
        let granted = |wrote_trust: bool| {
            move |_: &[TrustEntry]| {
                Ok(crate::codex_grant::Grant::Granted {
                    wrote_trust,
                    entries: Vec::new(),
                })
            }
        };
        let first = install(&path, &state, SCRIPT, OURS, granted(true)).expect("install");
        assert_eq!(first, Installed::Trusted { wrote_trust: true });
        assert!(std::fs::read_to_string(&path).expect("read").contains(OURS));

        let second = install(&path, &state, SCRIPT, OURS, granted(false)).expect("install");
        assert_eq!(
            second,
            Installed::Unchanged,
            "a settled install reported work"
        );
    }

    /// The grant is handed the entries the write actually produced — the indices
    /// it is asked about have to be the ones in the file, or every trust key is
    /// for a handler that is somewhere else.
    #[test]
    fn the_grant_is_asked_about_where_the_handlers_really_landed() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("hooks.json");
        let state = dir.path().join("state");
        // Somebody else already occupies group 0 of PreToolUse.
        std::fs::write(
            &path,
            serde_json::json!({
                "hooks": { "PreToolUse": [
                    { "hooks": [{ "type": "command", "command": "/theirs.sh" }] }
                ]}
            })
            .to_string(),
        )
        .expect("write");

        let seen = std::cell::RefCell::new(Vec::new());
        install(&path, &state, SCRIPT, OURS, |entries| {
            seen.replace(entries.to_vec());
            Ok(crate::codex_grant::Grant::Granted {
                wrote_trust: true,
                entries: Vec::new(),
            })
        })
        .expect("install");

        let entries = seen.into_inner();
        assert_eq!(entries.len(), CODEX_EVENTS.len());
        let pre = entries
            .iter()
            .find(|entry| entry.event_label == "pre_tool_use")
            .expect("pre_tool_use");
        // Group 1, because group 0 was theirs — and this is exactly what the file
        // now says.
        assert_eq!(pre.group_index, 1);
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(
            written["hooks"]["PreToolUse"][pre.group_index]["hooks"][pre.handler_index]["command"],
            OURS
        );
        // Every entry carries our timeout, which the trust hash covers.
        assert!(
            entries
                .iter()
                .all(|entry| entry.timeout_seconds == Some(MANAGED_TIMEOUT_SECONDS)),
            "an entry disagreed with the timeout actually written"
        );
    }

    /// A symlinked `hooks.json` is written THROUGH, not replaced — the link is
    /// something the user set up on purpose.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_config_is_written_through_to_its_target() {
        let dir = tempfile::tempdir().expect("temp");
        let real = dir.path().join("real-hooks.json");
        let link = dir.path().join("hooks.json");
        std::fs::write(&real, "{\"hooks\":{}}\n").expect("write");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        assert_eq!(
            write_path(&link),
            std::fs::canonicalize(&real).expect("canonical")
        );
        let found = read(&link).expect("read");
        let planned = plan(&found, &link, SCRIPT, OURS);
        assert!(write(&link, &found, &planned.config).expect("write"));
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("stat")
                .file_type()
                .is_symlink(),
            "the link was replaced with a regular file"
        );
        assert!(std::fs::read_to_string(&real).expect("read").contains(OURS));
    }
}
