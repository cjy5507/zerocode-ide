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
use crate::mcp_references::ReferenceShape;

use super::PersonsOwnSettingsFile;
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
/// out that a move can take: the name names one, and the value holds no
/// reference in the grammar a launch resolves by
/// ([`crate::mcp_references::reference_shape`]). A value that mixes a
/// reference with other text (`Token ${X}`, `${USER}:${PASS}`) or carries a
/// default (`${X:-text}`) is not listed: no single name's value equals it, so
/// only a person can split it.
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
    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(format!("{} could not be read.", path.display())),
    };
    let root = parse_json_object_contents(path, &contents)
        .map_err(|_| format!("{} is not a JSON object.", path.display()))?;
    let Some(servers) = root.get(mcp_keys::SERVERS).and_then(JsonValue::as_object) else {
        return Ok(Vec::new());
    };
    let mut found = Vec::new();
    for (server, definition) in servers {
        let Some(definition) = definition.as_object() else {
            continue;
        };
        for section in SECRET_SECTIONS {
            let Some(slots) = definition.get(section).and_then(JsonValue::as_object) else {
                continue;
            };
            for (name, value) in slots {
                if value
                    .as_str()
                    .is_some_and(|value| slot_holds_literal_secret(name, value))
                {
                    found.push(McpLiteralSecret {
                        server: server.clone(),
                        section,
                        name: name.clone(),
                    });
                }
            }
        }
    }
    Ok(found)
}

/// Rewrite the slot at `key_path` of the document at `path` to `${name}`, and
/// answer where the document as it was is kept.
///
/// `path` must be the person's own settings file next to `workspace`
/// ([`super::is_persons_own_settings_file`]), and not itself a link: the file
/// checked is the file read, and the file read is the file replaced.
/// `resolve` answers what `name` resolves to (or one sentence why it does not);
/// it is asked after the document is read and before it is written. The
/// rewrite happens only when that value is byte-equal to the slot's and the
/// slot holds no reference.
///
/// Every writer of a settings file in zo holds [`crate::file_ops::SettingsFileLock`]
/// across its read and its write, so they never cross. An editor that does not
/// take the lock is met by the last check: once the backup and the new document
/// are on disk beside the file, the file must still be the same file with the
/// same bytes, or both are removed and nothing is replaced. What is left is the
/// instant between that check and the rename; an edit saved inside it is lost.
///
/// # Errors
///
/// One sentence naming why nothing was written. Never a value.
pub fn move_mcp_literal_to_reference(
    path: &Path,
    workspace: &Path,
    key_path: &[String],
    name: &str,
    resolve: &dyn Fn(&str) -> Result<String, String>,
) -> Result<PathBuf, String> {
    move_with(path, workspace, key_path, name, resolve, &|| {})
}

/// [`move_mcp_literal_to_reference`], with `prepared` called once the backup
/// and the new document are written and before the last check, where a test
/// stands in for an editor that saves without the lock.
fn move_with(
    path: &Path,
    workspace: &Path,
    key_path: &[String],
    name: &str,
    resolve: &dyn Fn(&str) -> Result<String, String>,
    prepared: &dyn Fn(),
) -> Result<PathBuf, String> {
    let [servers_key, server, section, slot] = key_path else {
        return Err(not_a_slot());
    };
    if servers_key != mcp_keys::SERVERS || !SECRET_SECTIONS.contains(&section.as_str()) {
        return Err(not_a_slot());
    }
    if !crate::mcp_references::is_reference_name(name) {
        return Err(format!("{name} is not a reference name."));
    }
    let _guard = crate::file_ops::SettingsFileLock::acquire(path)
        .map_err(|_| "another process is writing the settings file; nothing was written.".to_string())?;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => return Err("the settings file is not a regular file; nothing was written.".to_string()),
        Err(_) => return Err("the settings file could not be read.".to_string()),
    }
    let checked = PersonsOwnSettingsFile::check(path, workspace).ok_or_else(|| {
        "the settings file is in the workspace or is one of its project files; only your own is rewritten."
            .to_string()
    })?;
    let before = checked.read(path).ok_or_else(changed)?;
    let contents = String::from_utf8(before.clone())
        .map_err(|_| "the settings file is not UTF-8 text.".to_string())?;
    let mut root = parse_json_object_contents(path, &contents)
        .map_err(|_| "the settings file is not a JSON object.".to_string())?;

    let literal = slot_value(&mut root, server, section, slot).ok_or_else(not_a_slot)?;
    let JsonValue::String(literal_text) = &*literal else {
        return Err("the slot does not hold text.".to_string());
    };
    if zerocode_core::credential::is_reference_value(literal_text) {
        return Err("the slot already holds a reference; nothing was written.".to_string());
    }
    match ReferenceShape::Plain {
        ReferenceShape::Whole | ReferenceShape::Defaulted => {
            return Err("the slot already holds a reference; nothing was written.".to_string());
        }
        ReferenceShape::Mixed => {
            return Err(
                "the slot mixes a reference with other text; edit it by hand, nothing was written."
                    .to_string(),
            );
        }
        ReferenceShape::Plain => {}
    }
    let value = resolve(name).map_err(|why| format!("{name} does not resolve: {why}"))?;
    if value.as_bytes() != literal_text.as_bytes() {
        return Err(format!(
            "the value {name} resolves to is not the value in the slot; nothing was written."
        ));
    }
    drop(value);
    *literal = JsonValue::String(format!("${{{name}}}"));

    let rendered = JsonValue::Object(root.clone()).render_pretty();
    let read_back = parse_json_object_contents(path, &rendered)
        .map_err(|_| "the rewritten settings did not survive the JSON round-trip; nothing was written.".to_string())?;
    let server_survives = read_back
        .get(mcp_keys::SERVERS)
        .and_then(JsonValue::as_object)
        .and_then(|servers| servers.get(server.as_str()))
        .is_some_and(|value| parse_mcp_server_config(server, value, &mcp_server_context(path, server)).is_ok());
    if read_back != root || !server_survives {
        return Err("the rewritten settings did not survive the JSON round-trip; nothing was written.".to_string());
    }

    let backup = write_backup(path, &before)?;
    let temp = match write_beside(path, rendered.as_bytes()) {
        Ok(temp) => temp,
        Err(why) => {
            let _ = std::fs::remove_file(&backup);
            return Err(why);
        }
    };
    let unchanged = checked.still_holds(path, workspace)
        && std::fs::read(path).ok().as_deref() == Some(before.as_slice());
    prepared();
    if !unchanged {
        let _ = std::fs::remove_file(&temp);
        let _ = std::fs::remove_file(&backup);
        return Err(changed());
    }
    if std::fs::rename(&temp, path).is_err() {
        let _ = std::fs::remove_file(&temp);
        let _ = std::fs::remove_file(&backup);
        return Err("the settings file could not be replaced; it is as it was.".to_string());
    }
    Ok(backup)
}

fn changed() -> String {
    "the settings file changed while it was being moved; nothing was written.".to_string()
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

/// Write `contents` to a fresh owner-only file beside `path`, for a rename
/// over it: a reader sees the old document or the new one, never half.
fn write_beside(path: &Path, contents: &[u8]) -> Result<PathBuf, String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "the settings file has no name.".to_string())?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let temp = path.with_file_name(format!(".{file_name}.zo-move-{}-{nanos}", std::process::id()));
    if write_private_file(&temp, contents, &ParentDirPolicy::LeaveParent).is_err() {
        let _ = std::fs::remove_file(&temp);
        return Err("the settings file could not be replaced; it is as it was.".to_string());
    }
    Ok(temp)
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

    use super::{mcp_literal_secrets, move_mcp_literal_to_reference, move_with, McpLiteralSecret};

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
        "OTHER_TOKEN": "${OTHER_TOKEN}",
        "MIXED_TOKEN": "Token ${MIXED_TOKEN}",
        "PAIR_SECRET": "${PAIR_USER}:${PAIR_PASS}",
        "DEFAULTED_TOKEN": "${DEFAULTED_TOKEN:-fixture-default}"
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

    /// A workspace beside the settings file, holding none of it.
    fn workspace() -> TempDir {
        tempfile::tempdir().expect("workspace")
    }

    fn names_in(dir: &Path, part: &str) -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("dir")
            .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(part))
            .collect()
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

        let workspace = workspace();
        let backup = move_mcp_literal_to_reference(
            &path,
            workspace.path(),
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
        let workspace = workspace();
        let at = workspace.path();
        let refusals = [
            move_mcp_literal_to_reference(&path, at, &token, "GITHUB_PERSONAL_ACCESS_TOKEN", &|_| {
                Ok("fixture-some-other-value".to_string())
            }),
            move_mcp_literal_to_reference(&path, at, &token, "GITHUB_PERSONAL_ACCESS_TOKEN", &|_| {
                Err("nothing holds it.".to_string())
            }),
            move_mcp_literal_to_reference(
                &path,
                at,
                &slot_path("github", "env", "OTHER_TOKEN"),
                "OTHER_TOKEN",
                &|_| Ok("${OTHER_TOKEN}".to_string()),
            ),
            move_mcp_literal_to_reference(&path, at, &slot_path("github", "args", "0"), "X", &|_| {
                Ok(LITERAL.to_string())
            }),
            move_mcp_literal_to_reference(&path, at, &token, "not-a-name", &|_| Ok(LITERAL.to_string())),
            // The file changes between the read and the write.
            move_mcp_literal_to_reference(&path, at, &token, "GITHUB_PERSONAL_ACCESS_TOKEN", &|_| {
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

    #[test]
    fn a_value_with_a_reference_in_it_is_not_listed_and_is_not_moved() {
        let dir = tempfile::tempdir().expect("dir");
        let path = settings(&dir);
        let listed: Vec<String> = mcp_literal_secrets(&path)
            .expect("scan")
            .into_iter()
            .map(|slot| slot.name)
            .collect();
        for name in ["MIXED_TOKEN", "PAIR_SECRET", "DEFAULTED_TOKEN"] {
            assert!(!listed.contains(&name.to_string()), "{name} is listed: {listed:?}");
        }
        let workspace = workspace();
        let whys: Vec<String> = [
            ("DEFAULTED_TOKEN", "${DEFAULTED_TOKEN:-fixture-default}"),
            ("MIXED_TOKEN", "Token ${MIXED_TOKEN}"),
            ("PAIR_SECRET", "${PAIR_USER}:${PAIR_PASS}"),
        ]
        .into_iter()
        .map(|(name, text)| {
            move_mcp_literal_to_reference(
                &path,
                workspace.path(),
                &slot_path("github", "env", name),
                name,
                &|_| Ok(text.to_string()),
            )
            .expect_err("refused")
        })
        .collect();
        assert!(whys[0].contains("already holds a reference"), "{}", whys[0]);
        assert!(whys[1].contains("edit it by hand"), "{}", whys[1]);
        assert!(whys[2].contains("edit it by hand"), "{}", whys[2]);
        assert_eq!(std::fs::read_to_string(&path).expect("read"), DOCUMENT, "nothing was written");
        assert!(names_in(dir.path(), ".bak-").is_empty(), "a refused move keeps no backup");
    }

    #[test]
    fn an_edit_saved_once_the_backup_and_the_new_document_are_ready_is_kept() {
        let edited = DOCUMENT.replace("github.example", "github.edited-meanwhile");
        let token = slot_path("github", "env", "GITHUB_PERSONAL_ACCESS_TOKEN");
        // An editor that writes the file in place, and one that renames a new
        // file over it, neither taking the settings lock.
        for in_place in [true, false] {
            let dir = tempfile::tempdir().expect("dir");
            let path = settings(&dir);
            let workspace = workspace();
            let why = move_with(
                &path,
                workspace.path(),
                &token,
                "GITHUB_PERSONAL_ACCESS_TOKEN",
                &|_| Ok(LITERAL.to_string()),
                &|| {
                    if in_place {
                        std::fs::write(&path, &edited).expect("edit in place");
                    } else {
                        let saved = dir.path().join("editor-save");
                        std::fs::write(&saved, &edited).expect("editor save");
                        std::fs::rename(&saved, &path).expect("rename over");
                    }
                },
            )
            .expect_err("refused");
            assert!(why.contains("changed while it was being moved"), "{in_place}: {why}");
            assert_eq!(std::fs::read_to_string(&path).expect("read"), edited, "{in_place}: the edit is kept");
            assert!(names_in(dir.path(), ".bak-").is_empty(), "{in_place}: no backup is left");
            assert!(names_in(dir.path(), "zo-move").is_empty(), "{in_place}: no temporary file is left");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_move_refuses_a_link_a_file_in_the_workspace_and_a_project_file_by_another_name() {
        let token = slot_path("github", "env", "GITHUB_PERSONAL_ACCESS_TOKEN");
        let workspace = workspace();
        let project = workspace.path().join(".zo").join("settings.json");
        std::fs::create_dir_all(project.parent().expect("parent")).expect(".zo");
        std::fs::write(&project, DOCUMENT).expect("project");
        let home = tempfile::tempdir().expect("home");
        let link = home.path().join("settings.json");
        std::os::unix::fs::symlink(&project, &link).expect("link");
        let other_home = tempfile::tempdir().expect("other home");
        let hard = other_home.path().join("settings.json");
        std::fs::hard_link(&project, &hard).expect("hard link");

        let whys: Vec<String> = [&link, &project, &hard]
            .into_iter()
            .map(|path| {
                move_mcp_literal_to_reference(
                    path,
                    workspace.path(),
                    &token,
                    "GITHUB_PERSONAL_ACCESS_TOKEN",
                    &|_| Ok(LITERAL.to_string()),
                )
                .expect_err("refused")
            })
            .collect();
        assert!(whys[0].contains("is a link"), "{}", whys[0]);
        assert!(whys[1].contains("only your own is rewritten"), "{}", whys[1]);
        assert!(whys[2].contains("only your own is rewritten"), "{}", whys[2]);
        assert_eq!(std::fs::read_to_string(&project).expect("project"), DOCUMENT, "the project file is as it was");
        for dir in [workspace.path().join(".zo"), home.path().to_path_buf(), other_home.path().to_path_buf()] {
            assert!(names_in(&dir, ".bak-").is_empty(), "no backup in {}", dir.display());
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_workspace_that_holds_the_persons_home_is_no_repository() {
        use crate::config::PersonsOwnSettingsFile;

        let root = tempfile::tempdir().expect("root");
        let base = root.path().canonicalize().expect("canonical root");
        let home = base.join("home");
        let file = home.join(".zo").join("settings.json");
        std::fs::create_dir_all(file.parent().expect("parent")).expect(".zo");
        std::fs::write(&file, DOCUMENT).expect("settings");
        for workspace in [&home, &base] {
            assert!(
                PersonsOwnSettingsFile::check_beside(&file, workspace, Some(&home)).is_some(),
                "{}",
                workspace.display()
            );
        }
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("elsewhere");
        assert!(
            PersonsOwnSettingsFile::check_beside(&file, &home, Some(&elsewhere)).is_none(),
            "a workspace holding the file but not the home is the workspace's"
        );
    }
}
