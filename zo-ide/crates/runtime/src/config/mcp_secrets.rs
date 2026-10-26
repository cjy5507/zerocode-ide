//! Credentials written out in the `mcpServers` section of a settings document,
//! found by name, and moved to a `${NAME}` reference on request.
//!
//! Finding reads the slot NAMES the credential table names
//! ([`zerocode_core::credential::names_a_credential`]) whose value is not a
//! whole reference ([`zerocode_core::credential::is_reference_value`]) — the
//! same two readings the memory and skill writers refuse a kept credential by
//! (t-74223, t-75740). Moving rewrites one slot to `${NAME}` only once the
//! name resolves to exactly the value the slot holds, so the server is handed
//! the same token before and after; the rest of the document is carried
//! through the loader's own parser as `zo mcp add` carries it
//! ([`super::mcp_edit`]), the old document is kept beside it, and both are
//! owner-only.
//!
//! **A value is never said.** What this module returns names files, keys and
//! reasons; a value is compared in memory and dropped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use core_types::paths::{write_private_file, ParentDirPolicy};

use crate::json::JsonValue;

use super::parsers::{mcp_keys, mcp_server_context, parse_json_object_contents, parse_mcp_server_config};

/// One `env` or `headers` slot of an MCP server that holds a credential as
/// written text rather than as a reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpLiteralSecret {
    pub server: String,
    /// `env` or `headers`.
    pub section: &'static str,
    /// The slot's key — the variable or header name.
    pub name: String,
}

impl McpLiteralSecret {
    /// The keys from the document's root to the slot.
    #[must_use]
    pub fn key_path(&self) -> [String; 4] {
        [
            mcp_keys::SERVERS.to_string(),
            self.server.clone(),
            self.section.to_string(),
            self.name.clone(),
        ]
    }
}

/// The sections of a server whose values may be credentials.
const SECRET_SECTIONS: [&str; 2] = [mcp_keys::ENV, mcp_keys::HEADERS];

/// Whether a slot named `name` holding `value` carries a credential written
/// out: the name names one and the value is not wholly a reference.
#[must_use]
pub fn slot_holds_literal_secret(name: &str, value: &str) -> bool {
    zerocode_core::credential::names_a_credential(name)
        && !value.trim().is_empty()
        && !zerocode_core::credential::is_reference_value(value)
}

/// The slots of the document at `path` that hold a credential written out. A
/// file that is not there holds none.
///
/// # Errors
///
/// One sentence: the file could not be read, or is not a JSON object.
pub fn mcp_literal_secrets(path: &Path) -> Result<Vec<McpLiteralSecret>, String> {
    let _ = path;
    Ok(Vec::new())
}

/// Rewrite the slot at `key_path` of the document at `path` to `${name}`, and
/// answer where the document as it was is kept.
///
/// `resolve` answers what `name` resolves to (or one sentence why it does not);
/// it is asked after the document is read and before it is written. The
/// rewrite happens only when that value is byte-equal to the slot's, the slot
/// is not already a reference, and the file still holds what was read. The new
/// document goes to a temporary file beside it, then over it by rename.
///
/// # Errors
///
/// One sentence naming why nothing was written. Never a value.
pub fn move_mcp_literal_to_reference(
    path: &Path,
    key_path: &[String],
    name: &str,
    resolve: &dyn Fn(&str) -> Result<String, String>,
) -> Result<PathBuf, String> {
    let _ = (path, key_path, name, resolve);
    Err("not implemented yet".to_string())
}

fn not_a_slot() -> String {
    "the path does not name an env or headers slot of an MCP server in this file.".to_string()
}

fn slot_value<'a>(
    root: &'a mut BTreeMap<String, JsonValue>,
    server: &str,
    section: &str,
    slot: &str,
) -> Option<&'a mut JsonValue> {
    let JsonValue::Object(servers) = root.get_mut(mcp_keys::SERVERS)? else {
        return None;
    };
    let JsonValue::Object(definition) = servers.get_mut(server)? else {
        return None;
    };
    let JsonValue::Object(slots) = definition.get_mut(section)? else {
        return None;
    };
    slots.get_mut(slot)
}

/// `<file>.bak-<UTC stamp>` beside `path`, owner-only, holding `contents`. A
/// second move in the same second takes the next free `-<n>`.
fn write_backup(path: &Path, contents: &[u8]) -> Result<PathBuf, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "the settings file has no name.".to_string())?;
    let stem = format!("{file_name}.bak-{}", utc_stamp());
    let backup = (1..100)
        .map(|n| {
            path.with_file_name(if n == 1 {
                stem.clone()
            } else {
                format!("{stem}-{n}")
            })
        })
        .find(|candidate| std::fs::symlink_metadata(candidate).is_err())
        .ok_or_else(|| "no free backup name beside the settings file; nothing was written.".to_string())?;
    write_private_file(&backup, contents, &ParentDirPolicy::LeaveParent)
        .map_err(|_| "the backup could not be written; nothing was written.".to_string())?;
    Ok(backup)
}

/// Write `contents` to a fresh owner-only file beside `path`, then rename it
/// over `path`: a reader sees the old document or the new one, never half.
fn replace_owner_only(path: &Path, contents: &[u8]) -> Result<(), String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "the settings file has no name.".to_string())?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let temp = path.with_file_name(format!(".{file_name}.zo-move-{}-{nanos}", std::process::id()));
    let written = write_private_file(&temp, contents, &ParentDirPolicy::LeaveParent)
        .and_then(|()| std::fs::rename(&temp, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
        return Err("the settings file could not be replaced; it is as it was.".to_string());
    }
    Ok(())
}

/// `YYYYMMDDTHHMMSSZ`, now.
fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let (year, month, day) =
        core_types::date::civil_from_unix_days(i64::try_from(secs / 86_400).unwrap_or(0));
    let of_day = secs % 86_400;
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::{mcp_literal_secrets, move_mcp_literal_to_reference, McpLiteralSecret};

    /// A value no real key has, so a leak is a string a test can look for.
    const LITERAL: &str = "fixture-literal-token-40cc0c0ffee0ddba11fee1dead00";

    const DOCUMENT: &str = r#"{
  "model": "opus",
  "hooks": {"PreToolUse": []},
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "server-github"],
      "env": {
        "GITHUB_PERSONAL_ACCESS_TOKEN": "fixture-literal-token-40cc0c0ffee0ddba11fee1dead00",
        "GITHUB_HOST": "github.example",
        "OTHER_TOKEN": "${OTHER_TOKEN}"
      }
    },
    "remote": {
      "url": "https://vendor.example/mcp",
      "headers": {
        "Authorization": "Bearer fixture-literal-token-40cc0c0ffee0ddba11fee1dead00",
        "X-Trace": "on",
        "Cookie": "{{session}}"
      }
    }
  }
}"#;

    fn settings(dir: &TempDir) -> PathBuf {
        let path = dir.path().join("settings.json");
        std::fs::write(&path, DOCUMENT).expect("settings");
        path
    }

    fn slot_path(server: &str, section: &str, name: &str) -> Vec<String> {
        ["mcpServers", server, section, name]
            .iter()
            .map(|key| (*key).to_string())
            .collect()
    }

    fn json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json")
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[test]
    fn literal_slots_are_found_by_name_and_references_are_not() {
        let dir = tempfile::tempdir().expect("dir");
        let found = mcp_literal_secrets(&settings(&dir)).expect("scan");
        assert_eq!(
            found,
            vec![
                McpLiteralSecret {
                    server: "github".to_string(),
                    section: "env",
                    name: "GITHUB_PERSONAL_ACCESS_TOKEN".to_string(),
                },
                McpLiteralSecret {
                    server: "remote".to_string(),
                    section: "headers",
                    name: "Authorization".to_string(),
                },
            ]
        );
        assert!(!format!("{found:?}").contains(LITERAL));
        assert_eq!(
            mcp_literal_secrets(&dir.path().join("absent.json")).expect("no file"),
            Vec::new()
        );
    }

    #[test]
    fn a_move_rewrites_the_slot_atomically_keeps_an_owner_only_backup_and_the_rest_of_the_document() {
        let dir = tempfile::tempdir().expect("dir");
        let path = settings(&dir);
        let before = json(&path);
        #[cfg(unix)]
        let inode_before = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&path).expect("meta"));

        let backup = move_mcp_literal_to_reference(
            &path,
            &slot_path("github", "env", "GITHUB_PERSONAL_ACCESS_TOKEN"),
            "GITHUB_PERSONAL_ACCESS_TOKEN",
            &|_| Ok(LITERAL.to_string()),
        )
        .expect("moved");

        let after = json(&path);
        assert_eq!(
            after["mcpServers"]["github"]["env"]["GITHUB_PERSONAL_ACCESS_TOKEN"],
            "${GITHUB_PERSONAL_ACCESS_TOKEN}"
        );
        let mut expected = before.clone();
        expected["mcpServers"]["github"]["env"]["GITHUB_PERSONAL_ACCESS_TOKEN"] =
            serde_json::json!("${GITHUB_PERSONAL_ACCESS_TOKEN}");
        assert_eq!(after, expected, "everything else is as it was");
        assert_eq!(backup.parent(), path.parent(), "the backup sits beside the file");
        assert!(
            backup
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("settings.json.bak-")),
            "{}",
            backup.display()
        );
        assert_eq!(std::fs::read_to_string(&backup).expect("backup"), DOCUMENT);
        #[cfg(unix)]
        {
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&backup), 0o600);
            let inode_after = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&path).expect("meta"));
            assert_ne!(inode_before, inode_after, "replaced by rename, not written in place");
        }
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .expect("dir")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("zo-move"))
            .collect();
        assert!(left.is_empty(), "no temporary file is left: {left:?}");
    }

    #[test]
    fn a_move_refuses_a_mismatch_a_reference_a_changed_file_and_a_path_that_is_not_a_slot() {
        let dir = tempfile::tempdir().expect("dir");
        let path = settings(&dir);
        let token = slot_path("github", "env", "GITHUB_PERSONAL_ACCESS_TOKEN");
        let refusals = [
            move_mcp_literal_to_reference(&path, &token, "GITHUB_PERSONAL_ACCESS_TOKEN", &|_| {
                Ok("fixture-some-other-value".to_string())
            }),
            move_mcp_literal_to_reference(&path, &token, "GITHUB_PERSONAL_ACCESS_TOKEN", &|_| {
                Err("nothing holds it.".to_string())
            }),
            move_mcp_literal_to_reference(
                &path,
                &slot_path("github", "env", "OTHER_TOKEN"),
                "OTHER_TOKEN",
                &|_| Ok("${OTHER_TOKEN}".to_string()),
            ),
            move_mcp_literal_to_reference(&path, &slot_path("github", "args", "0"), "X", &|_| {
                Ok(LITERAL.to_string())
            }),
            move_mcp_literal_to_reference(&path, &token, "not-a-name", &|_| Ok(LITERAL.to_string())),
            // The file changes between the read and the write.
            move_mcp_literal_to_reference(&path, &token, "GITHUB_PERSONAL_ACCESS_TOKEN", &|_| {
                let changed = DOCUMENT.replace("github.example", "github.changed");
                std::fs::write(&path, changed).expect("change the file");
                Ok(LITERAL.to_string())
            }),
        ];
        let whys: Vec<String> = refusals
            .into_iter()
            .map(|refusal| refusal.expect_err("refused"))
            .collect();
        assert!(whys[0].contains("is not the value in the slot"), "{}", whys[0]);
        assert!(whys[1].contains("does not resolve"), "{}", whys[1]);
        assert!(whys[2].contains("already holds a reference"), "{}", whys[2]);
        assert!(whys[3].contains("does not name an env or headers slot"), "{}", whys[3]);
        assert!(whys[4].contains("is not a reference name"), "{}", whys[4]);
        assert!(whys[5].contains("changed while it was being moved"), "{}", whys[5]);
        for why in &whys {
            assert!(!why.contains(LITERAL), "no value in a refusal: {why}");
        }
        let still = std::fs::read_to_string(&path).expect("read");
        assert!(still.contains("github.changed") && still.contains(LITERAL), "nothing was written");
        let backups = std::fs::read_dir(dir.path())
            .expect("dir")
            .filter(|entry| {
                entry
                    .as_ref()
                    .is_ok_and(|entry| entry.file_name().to_string_lossy().contains(".bak-"))
            })
            .count();
        assert_eq!(backups, 0, "a refused move keeps no backup");
    }
}
