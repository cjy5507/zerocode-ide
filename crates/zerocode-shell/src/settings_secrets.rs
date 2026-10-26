//! Secrets written as plain text in zo's settings file, moved into the
//! keychain by a person's press (t-76698; zo's half is t-76674).
//!
//! The window only stopped new plain-text secrets from being saved (t-75740,
//! t-75848): a key a person typed into `mcpServers.<server>.env.<NAME>` before
//! then stayed there. zo answers three doors, none of which ever prints a
//! value:
//!
//! - `zo settings secrets --json` names every such cell (file, path, name);
//! - `zo settings secrets check --name <NAME> --json` says whether `${NAME}`
//!   resolves (zo's environment, then the window's keychain item);
//! - `zo settings secrets move --file … --path … --name … --json` writes the
//!   cell as `${NAME}`, after it has found the reference resolves to the very
//!   bytes the cell holds, and keeps a backup.
//!
//! The window's half is between the first door and the other two: it reads
//! the one cell the listing named into memory, puts the value in the keychain
//! item zo reads (`dev.zerocode.key.<NAME>`, the item the TypeSafe key is kept
//! in), and drops it. The value goes to no log, no error, no answer to the
//! page and no argument of zo's; a reason zo gives that carries it is not
//! said either.
//!
//! A zo without the doors (an older one) answers the listing with an error,
//! and the settings say nothing.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use zerocode_harness::SERVICE_KEYCHAIN_SERVICE_PREFIX;

use crate::api_routers::RouterKeys;

/// How long one door may take. zo reads one file and asks one keychain item;
/// a door that outlives this is killed and counts as no answer.
const ZO_WAIT: Duration = Duration::from_secs(10);

/// The longest reason the page is given from zo, in characters.
const WHY_CHARS: usize = 300;

/// The zo the window asks, and the environment it asks with: a zo pane's
/// account variables (read, never written), without the window's own
/// `ZEROCODE*` names — so it reads the settings file and the keychain a pane's
/// zo reads.
pub(crate) struct ZoDoor {
    bin: PathBuf,
    env: Vec<(String, String)>,
    wait: Duration,
}

impl ZoDoor {
    /// The installed zo, with a zo pane's account variables.
    pub(crate) fn of_this_window(config_root: &Path) -> Option<Self> {
        let home = dirs::home_dir()?;
        Some(Self::at(
            crate::zo_companion::zo_path_under(&home),
            crate::accounts::reading_env_for(config_root, "zo"),
        ))
    }

    pub(crate) fn at(bin: PathBuf, env: Vec<(String, String)>) -> Self {
        Self {
            bin,
            env,
            wait: ZO_WAIT,
        }
    }

    /// One door's exit code and the JSON it printed, whatever the code; `None`
    /// when zo is missing, is killed for outliving the wait, or prints no JSON.
    fn ask(&self, args: &[&str]) -> Option<(i32, Value)> {
        let mut command = crate::proc::quiet_command_with_env(&self.bin, &self.env);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy();
            if key.starts_with("ZEROCODE") {
                command.env_remove(&*key);
            }
        }
        let mut child = command.spawn().ok()?;
        let mut stdout = child.stdout.take()?;
        let started = Instant::now();
        let printed = std::thread::scope(|scope| {
            let reader = scope.spawn(move || {
                let mut text = String::new();
                stdout.read_to_string(&mut text).map(|_| text).ok()
            });
            while !reader.is_finished() {
                if started.elapsed() > self.wait {
                    let _ = child.kill();
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            reader.join().ok().flatten()
        });
        let status = child.wait().ok()?;
        if started.elapsed() > self.wait {
            return None;
        }
        Some((status.code()?, serde_json::from_str(printed?.trim()).ok()?))
    }
}

/// One plain-text cell zo's listing named: the file as zo spelled it (and is
/// handed back), the keys down to the cell, and the name it is to refer to.
struct Literal {
    file: String,
    path: Vec<String>,
    name: String,
}

impl Literal {
    fn of(row: &Value) -> Option<Self> {
        let file = row.get("file")?.as_str()?;
        let path = row
            .get("path")?
            .as_array()?
            .iter()
            .map(|key| key.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()?;
        let name = row.get("name")?.as_str()?;
        (Path::new(file).is_absolute() && !path.is_empty() && is_name(name)).then(|| Self {
            file: file.to_string(),
            path,
            name: name.to_string(),
        })
    }
}

/// A variable's name as `${NAME}` spells one: a letter or `_`, then letters,
/// digits and `_`. Anything else would name a keychain item zo never reads.
fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

/// Whether zo's `file` is the person's settings file the window knows. A
/// cell elsewhere is not the window's to read.
fn is_settings_file(file: &str, settings: &Path) -> bool {
    match (std::fs::canonicalize(file), std::fs::canonicalize(settings)) {
        (Ok(file), Ok(settings)) => file == settings,
        _ => Path::new(file) == settings,
    }
}

/// What the settings show: the names of the secrets written as plain text,
/// each once — never a value, a piece of one or its length.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretNotice {
    pub names: Vec<String>,
}

/// One cell moved: the name it now refers to and zo's backup of the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Moved {
    pub name: String,
    pub backup: String,
}

/// Where a move stopped and why. `reason` is the window's word for the page's
/// sentence; `why` is zo's own sentence when zo gave one; `keychain_kept` says
/// the keychain item holding the value was left in place for the next try.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stopped {
    pub name: String,
    pub reason: &'static str,
    pub why: Option<String>,
    pub keychain_kept: bool,
}

/// A press of 「키체인으로 옮기기」: the cells moved in order, where it
/// stopped, and the names not tried because it stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MoveOutcome {
    pub moved: Vec<Moved>,
    pub stopped: Option<Stopped>,
    pub left: Vec<String>,
}

/// The keychain item zo reads `${name}` from.
#[must_use]
pub fn keychain_service(name: &str) -> String {
    format!("{SERVICE_KEYCHAIN_SERVICE_PREFIX}{name}")
}

/// The plain-text cells of the person's settings file zo lists; `None` when
/// zo does not answer the door.
fn literals(zo: &ZoDoor, settings: &Path) -> Option<Vec<Literal>> {
    let (code, said) = zo.ask(&["settings", "secrets", "--json"])?;
    if code != 0 {
        return None;
    }
    Some(
        said.get("literals")?
            .as_array()?
            .iter()
            .filter_map(Literal::of)
            .filter(|literal| is_settings_file(&literal.file, settings))
            .collect(),
    )
}

/// The names the settings line shows, or `None` when there is nothing to show
/// — none written as plain text, or a zo that does not know the door.
pub(crate) fn notice(zo: &ZoDoor, settings: &Path) -> Option<SecretNotice> {
    let mut names: Vec<String> = Vec::new();
    for literal in literals(zo, settings)? {
        if !names.contains(&literal.name) {
            names.push(literal.name);
        }
    }
    (!names.is_empty()).then_some(SecretNotice { names })
}

/// Every plain-text cell of the person's settings file, moved in the order zo
/// listed them, stopping at the first that cannot be.
pub(crate) fn move_all(zo: &ZoDoor, settings: &Path, keys: &dyn RouterKeys) -> MoveOutcome {
    let Some(literals) = literals(zo, settings) else {
        return MoveOutcome {
            stopped: Some(Stopped {
                name: String::new(),
                reason: "zo_silent",
                why: None,
                keychain_kept: false,
            }),
            ..MoveOutcome::default()
        };
    };
    let mut outcome = MoveOutcome::default();
    let mut rest = literals.into_iter();
    while let Some(literal) = rest.next() {
        match move_one(zo, settings, &literal, keys) {
            Ok(backup) => outcome.moved.push(Moved {
                name: literal.name,
                backup,
            }),
            Err(stopped) => {
                outcome.stopped = Some(stopped);
                outcome.left = rest.map(|literal| literal.name).collect();
                break;
            }
        }
    }
    outcome
}

/// One cell: its value read into memory, kept in the keychain item zo reads,
/// the reference checked, the cell rewritten by zo. The value lives in this
/// call alone and is dropped with it.
fn move_one(
    zo: &ZoDoor,
    settings: &Path,
    literal: &Literal,
    keys: &dyn RouterKeys,
) -> Result<String, Stopped> {
    let stop = |reason: &'static str, why: Option<String>, keychain_kept: bool| Stopped {
        name: literal.name.clone(),
        reason,
        why,
        keychain_kept,
    };
    let value = cell(settings, &literal.path).ok_or_else(|| stop("cell_changed", None, false))?;
    // A trimmed copy would be another value, and zo refuses a reference that
    // does not resolve to the cell's very bytes.
    if value.is_empty() || value.trim() != value {
        return Err(stop("padded", None, false));
    }
    let service = keychain_service(&literal.name);
    match keys.read(&service) {
        Ok(Some(held)) if held == value => {}
        Ok(Some(_)) => return Err(stop("keychain_differs", None, false)),
        Ok(None) => {
            crate::typesafe_settings::save_key_at(&service, &value, "비밀 값이 비어 있습니다", keys)
                .map_err(|refusal| {
                    stop("keychain", said_without(&refusal.message, &value), false)
                })?
        }
        Err(refusal) => {
            return Err(stop(
                "keychain",
                said_without(&refusal.message, &value),
                false,
            ));
        }
    }
    let name = literal.name.as_str();
    match zo.ask(&["settings", "secrets", "check", "--name", name, "--json"]) {
        Some((0, said)) if said.get("resolves") == Some(&Value::Bool(true)) => {}
        Some((_, said)) if said.get("resolves") == Some(&Value::Bool(false)) => {
            return Err(stop("not_resolved", why_of(&said, &value), true));
        }
        _ => return Err(stop("zo_silent", None, true)),
    }
    let path =
        serde_json::to_string(&literal.path).map_err(|_| stop("cell_changed", None, true))?;
    let args = [
        "settings",
        "secrets",
        "move",
        "--file",
        literal.file.as_str(),
        "--path",
        path.as_str(),
        "--name",
        name,
        "--json",
    ];
    match zo.ask(&args) {
        Some((0, said)) if said.get("moved") == Some(&Value::Bool(true)) => said
            .get("backup")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| stop("zo_silent", None, true)),
        Some((_, said)) if said.get("moved") == Some(&Value::Bool(false)) => {
            Err(stop("move_refused", why_of(&said, &value), true))
        }
        _ => Err(stop("zo_silent", None, true)),
    }
}

/// The string at `path` in the settings file, read under its lock; `None`
/// when the file, a key on the way or the string is gone.
fn cell(settings: &Path, path: &[String]) -> Option<String> {
    crate::api_routers::with_zo_settings_root(settings, |root| {
        let (first, rest) = path.split_first().ok_or_else(String::new)?;
        let mut at = root.get(first);
        for key in rest {
            at = at.and_then(|node| node.get(key));
        }
        Ok(at.and_then(Value::as_str).map(str::to_string))
    })
    .ok()
    .flatten()
}

/// zo's reason, for the page.
fn why_of(said: &Value, value: &str) -> Option<String> {
    said_without(said.get("why")?.as_str()?, value)
}

/// A sentence for the page, cut to [`WHY_CHARS`] — and not said at all when
/// it carries the value.
fn said_without(sentence: &str, value: &str) -> Option<String> {
    let sentence = sentence.trim();
    if sentence.is_empty() || sentence.contains(value) {
        return None;
    }
    Some(sentence.chars().take(WHY_CHARS).collect())
}

#[cfg(all(test, unix))]
#[path = "settings_secrets_tests.rs"]
mod tests;
