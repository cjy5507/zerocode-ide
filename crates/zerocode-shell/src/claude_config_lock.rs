//! The window's own Claude config (`.claude.json` in the Claude home the
//! window launches Claude Code with), changed the way Claude Code changes it:
//! under its lock, read again under it, and put in place with one rename.
//!
//! Claude Code 2.1.326 writes that file through `proper-lockfile`: the lock is
//! a folder named after the file plus `.lock`, made with one `mkdir`; its
//! holder freshens the folder's time while it holds it, and a folder older
//! than ten seconds is any writer's to take. Under it Claude Code reads the
//! file again, changes it and renames a new copy over it. So a writer that
//! takes the same folder for its own read, change and rename loses no write
//! of Claude Code's and has none of its own lost.
//!
//! Measured on 2026-11-17 (t-112281) with the real CLI in a throwaway home:
//! `claude mcp add-json` waited out a folder held 8 s and wrote once it was
//! let go; against one held (and freshened) 60 s it gave up after 16 to 18 s
//! and wrote nothing; a folder left behind unfreshened it took after 13.4 s,
//! one left 60 s before at once; a change the window wrote under the folder
//! while it waited was still in the file after its own write, and one written
//! while a session ran was still there after the session.
//!
//! `claude mcp add-json` does not replace a name the list holds (「already
//! exists in user config」, exit 1), so a server written again in place goes
//! through here — not through `remove` and `add-json`, between which a pane
//! that starts has no such server for its whole session.
//!
//! The window never takes another's folder, however old: Claude Code's next
//! write takes a forsaken one, and a write that could not take the folder
//! writes nothing, so the list keeps what it held.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long a write waits for Claude Code to let its lock go. Claude Code
/// holds it for the few milliseconds of one read, change and rename; a folder
/// still there after this is a forsaken one (or a stuck process), and the
/// write is tried again on a later look.
pub(crate) const LOCK_WAIT: Duration = Duration::from_secs(5);

/// The error a write answers when the lock stayed taken for the whole wait.
pub(crate) const BUSY: &str = "library.claudeListBusy";

/// The lock Claude Code takes for `file`: the file's own path plus `.lock`,
/// beside the path as written rather than where a link leads, as Claude Code
/// names it.
fn lock_path(file: &Path) -> PathBuf {
    let mut lock = file.as_os_str().to_owned();
    lock.push(".lock");
    PathBuf::from(lock)
}

/// The lock, held: let go when dropped.
struct Held(PathBuf);

impl Held {
    fn take(lock: PathBuf, wait: Duration) -> Result<Self, String> {
        let started = Instant::now();
        let mut pause = Duration::from_millis(10);
        loop {
            match std::fs::create_dir(&lock) {
                Ok(()) => return Ok(Self(lock)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if started.elapsed() >= wait {
                        return Err(BUSY.to_string());
                    }
                    std::thread::sleep(pause.min(wait.saturating_sub(started.elapsed())));
                    pause = (pause * 2).min(Duration::from_millis(200));
                }
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// Change `file` with `edit` under Claude Code's lock, waiting at most
/// `wait` for it: the file is read under the lock, `edit` answers whether it
/// changed the document, and a changed document is written as Claude Code
/// writes it (two-space JSON) through one rename that keeps the file's
/// permissions. Answers whether the file was written: `false` for a file that
/// is not there or that `edit` left as it was. A file that is not a JSON
/// object is not written.
pub(crate) fn rewrite(
    file: &Path,
    wait: Duration,
    edit: impl FnOnce(&mut Value) -> bool,
) -> Result<bool, String> {
    let _held = Held::take(lock_path(file), wait)?;
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    let mut doc: Value =
        serde_json::from_str(&text).map_err(|_| "library.listUnreadable".to_string())?;
    if !doc.is_object() {
        return Err("library.listUnreadable".to_string());
    }
    if !edit(&mut doc) {
        return Ok(false);
    }
    let text = serde_json::to_string_pretty(&doc).map_err(|error| error.to_string())?;
    crate::library_runtime::replace_file(file, text.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list_at(dir: &Path, doc: &Value) -> PathBuf {
        let file = dir.join(".claude.json");
        std::fs::write(&file, serde_json::to_string_pretty(doc).unwrap()).unwrap();
        file
    }

    /// The lock is Claude Code's: a folder beside the file, taken for the
    /// write and let go after it; the document keeps every other key in its
    /// own order and the file its permissions.
    #[cfg(unix)]
    #[test]
    fn a_write_takes_claudes_lock_and_keeps_the_rest_of_the_file() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let file = list_at(
            dir.path(),
            &serde_json::json!({
                "numStartups": 4,
                "oauthAccount": { "accountUuid": "fixture-account" },
                "mcpServers": { "a": { "type": "stdio", "command": "a-mcp" } },
                "projects": { "/p": { "allowedTools": [] } },
            }),
        );
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut saw_lock = false;
        let written = rewrite(&file, LOCK_WAIT, |doc| {
            saw_lock = dir.path().join(".claude.json.lock").is_dir();
            doc["mcpServers"]["a"]["command"] = "b-mcp".into();
            true
        })
        .unwrap();
        assert!(written);
        assert!(saw_lock, "the change is made under Claude Code's lock");
        assert!(
            !dir.path().join(".claude.json.lock").exists(),
            "and the lock is let go"
        );
        let text = std::fs::read_to_string(&file).unwrap();
        let doc: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["mcpServers"]["a"]["command"], "b-mcp");
        assert_eq!(doc["oauthAccount"]["accountUuid"], "fixture-account");
        let keys: Vec<&str> = doc
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["numStartups", "oauthAccount", "mcpServers", "projects"]
        );
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            !rewrite(&file, LOCK_WAIT, |_| false).unwrap(),
            "a document left as it was is not written"
        );
        assert!(
            !rewrite(&dir.path().join("none.json"), LOCK_WAIT, |_| true).unwrap(),
            "nor is a file that is not there"
        );
    }

    /// A lock Claude Code holds is waited for and never taken: held past the
    /// wait, nothing is written and the folder stays; let go within it, the
    /// write lands after.
    #[test]
    fn a_held_lock_is_waited_for_and_never_taken() {
        let dir = tempfile::tempdir().unwrap();
        let original = serde_json::json!({ "mcpServers": { "a": { "command": "a-mcp" } } });
        let file = list_at(dir.path(), &original);
        let before = std::fs::read(&file).unwrap();
        let lock = dir.path().join(".claude.json.lock");
        std::fs::create_dir(&lock).unwrap();

        let refused = rewrite(&file, Duration::from_millis(200), |doc| {
            doc["mcpServers"]["a"]["command"] = "b-mcp".into();
            true
        });
        assert_eq!(refused, Err(BUSY.to_string()));
        assert_eq!(std::fs::read(&file).unwrap(), before, "nothing is written");
        assert!(lock.is_dir(), "another's lock is not taken");

        let letting_go = {
            let lock = lock.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                std::fs::remove_dir(&lock).unwrap();
            })
        };
        let started = Instant::now();
        let written = rewrite(&file, Duration::from_secs(10), |doc| {
            doc["mcpServers"]["a"]["command"] = "b-mcp".into();
            true
        });
        letting_go.join().unwrap();
        assert_eq!(written, Ok(true));
        assert!(started.elapsed() >= Duration::from_millis(300));
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(doc["mcpServers"]["a"]["command"], "b-mcp");
    }

    /// A file that does not read as a JSON object is not written, and the
    /// lock is let go all the same.
    #[test]
    fn a_file_that_is_not_a_list_is_not_written() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".claude.json");
        for text in ["not json", "[1]"] {
            std::fs::write(&file, text).unwrap();
            let refused = rewrite(&file, LOCK_WAIT, |_| true);
            assert_eq!(refused, Err("library.listUnreadable".to_string()));
            assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
            assert!(!dir.path().join(".claude.json.lock").exists());
        }
    }
}
