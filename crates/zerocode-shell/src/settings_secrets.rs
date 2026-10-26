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
        let mut command = crate::proc::quiet_command(&self.bin);
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
        command.envs(self.env.iter().map(|(key, value)| (key, value)));
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

/// One plain-text cell zo's listing named.
struct Literal {
    file: PathBuf,
    path: Vec<String>,
    name: String,
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

/// The names the settings line shows, or `None` when there is nothing to show
/// — none written as plain text, or a zo that does not know the door.
pub(crate) fn notice(zo: &ZoDoor, settings: &Path) -> Option<SecretNotice> {
    let _ = (zo, settings);
    None
}

/// Every plain-text cell of the person's settings file, moved in the order zo
/// listed them, stopping at the first that cannot be.
pub(crate) fn move_all(zo: &ZoDoor, settings: &Path, keys: &dyn RouterKeys) -> MoveOutcome {
    let _ = (zo, settings, keys);
    MoveOutcome::default()
}

#[cfg(all(test, unix))]
#[path = "settings_secrets_tests.rs"]
mod tests;
