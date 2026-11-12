//! Narrowing the "always allow" rules an older zo saved wider than the command
//! the person approved (t-105662).
//!
//! Up to v1.1.143 "always allow" stored the approved command as the rule's
//! subject with only `\ ( )` escaped, so a command carrying `*`, `?` or `[`
//! became a glob: approving `cargo test build/*` also allowed
//! `cargo test build/x && cargo publish`. v1.1.144 writes the subject
//! literally ([`crate::permissions`]), but the rules already saved stay in each
//! project's `.zo/settings.local.json`. Once per project, before the session
//! reads its permission rules ([`super::ConfigLoader::load_for_session`]), the
//! allow rules there that read as a glob are rewritten as the literal command
//! they were approved for. The approved command is still allowed; anything
//! else asks again.
//!
//! What is left alone: deny and ask rules (a wide refusal is the safe side), a
//! `prefix:*` rule, a rule whose subject is `*`, a bare tool name, a glob
//! already in the literal form, every other settings file, and — after the
//! project is marked done — any glob the person writes by hand.
//!
//! The marker, the notice for the next session a person watches, and the file
//! as it was all live in the project's zo state folder, outside the
//! repository, so nothing new can be committed by mistake. Any failure changes
//! nothing and leaves no marker, so the next session tries again; it never
//! stops a session from starting.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use core_types::paths::{write_private_file, ParentDirPolicy};

use super::parsers::{parse_json_object_contents, parse_optional_permission_rules};
use super::{
    trusted_uncommitted_zo_file_snapshot, zo_project_state_dir, ConfigError,
    RuntimePermissionRuleConfig,
};
use crate::json::JsonValue;
use crate::permissions::{narrowed_allow_rule, narrowed_ordered_allow_rule};

/// The one file narrowed: where "always allow" writes its rules.
const SETTINGS_FILE: &str = "settings.local.json";
/// The folder under a project's zo state that holds the marker, the notice and
/// the kept originals.
const STATE_FOLDER: &str = "allow-always-narrow";
/// Present once the project has been narrowed (or had nothing to narrow).
const DONE_MARKER: &str = "done";
/// What the next session a person watches says, until it has said it.
const NOTICE_FILE: &str = "notice.json";
/// How many narrowed rules the notice names; the rest it counts.
const NOTICE_RULES_SHOWN: usize = 5;

/// One rule the narrowing rewrote: as it was, and as it now reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NarrowedRule {
    pub before: String,
    pub after: String,
}

/// What [`narrow_broad_allow_always_rules`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowAlwaysNarrowing {
    /// The project was marked done before; nothing was read.
    AlreadyDone,
    /// No rule needed narrowing; the project is now marked done.
    NothingToNarrow,
    /// These rules were narrowed; the file as it was is kept at `backup`.
    Narrowed {
        rules: Vec<NarrowedRule>,
        backup: PathBuf,
    },
    /// Nothing was changed and no marker was left, so the next session tries
    /// again: the file is not the person's trusted one, it changed while it
    /// was read, it does not parse, or a write failed.
    Skipped,
}

/// Narrow the broad allow rules in `<cwd>/.zo/settings.local.json`, once per
/// project. See the module documentation for what is narrowed and what is not.
#[must_use]
pub fn narrow_broad_allow_always_rules(cwd: &Path) -> AllowAlwaysNarrowing {
    narrow_in(cwd, &state_folder(cwd))
}

/// The notice for a session a person watches, taken so it is said once: how
/// many rules were narrowed, which, and where the original is kept. `None`
/// when there is nothing to say, or when the notice could not be removed (it
/// is then not said at all rather than said on every start).
#[must_use]
pub fn take_allow_always_narrow_notice(cwd: &Path) -> Option<String> {
    take_notice_in(&state_folder(cwd))
}

fn state_folder(cwd: &Path) -> PathBuf {
    zo_project_state_dir(cwd).join(STATE_FOLDER)
}

fn narrow_in(cwd: &Path, folder: &Path) -> AllowAlwaysNarrowing {
    let path = cwd.join(".zo").join(SETTINGS_FILE);
    narrow_with(
        cwd,
        folder,
        &mut || trusted_uncommitted_zo_file_snapshot(cwd, &path, SETTINGS_FILE),
        &mut |bytes| write_private_file(&path, bytes, &ParentDirPolicy::LeaveParent),
    )
}

/// The narrowing, with the trusted read and the settings write passed in so a
/// file changing between reads and a failing write can be shown to change
/// nothing.
fn narrow_with(
    cwd: &Path,
    folder: &Path,
    snapshot: &mut dyn FnMut() -> Option<String>,
    write_settings: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
) -> AllowAlwaysNarrowing {
    // Before any git work: a marked project costs one `stat` per session.
    if std::fs::symlink_metadata(folder.join(DONE_MARKER)).is_ok() {
        return AllowAlwaysNarrowing::AlreadyDone;
    }
    let path = cwd.join(".zo").join(SETTINGS_FILE);
    // "Always allow" never creates this file, so a project without one holds
    // no rule an older zo saved; a glob written into it later is the person's.
    if matches!(std::fs::symlink_metadata(&path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        return mark_done(folder, AllowAlwaysNarrowing::NothingToNarrow);
    }
    let Some(original) = snapshot() else {
        return AllowAlwaysNarrowing::Skipped;
    };
    let (rendered, rules) = match narrow_allow_always_document(&path, &original) {
        Ok(Some(narrowed)) => narrowed,
        Ok(None) => return mark_done(folder, AllowAlwaysNarrowing::NothingToNarrow),
        Err(_) => return AllowAlwaysNarrowing::Skipped,
    };
    let Some(backup) = keep_original(folder, &original) else {
        return AllowAlwaysNarrowing::Skipped;
    };
    // Never write over an edit made while the candidate was built.
    if snapshot().as_deref() != Some(original.as_str()) {
        let _ = std::fs::remove_file(&backup);
        return AllowAlwaysNarrowing::Skipped;
    }
    if write_settings(rendered.as_bytes()).is_err() {
        // The write truncates in place, so a failure may have left part of a
        // file: put the original back, and drop the kept copy only once that
        // worked — otherwise it is the one whole copy there is.
        if write_settings(original.as_bytes()).is_ok() {
            let _ = std::fs::remove_file(&backup);
        }
        return AllowAlwaysNarrowing::Skipped;
    }
    // Read back through the trusted gate the next session uses. A mismatch is
    // an edit landing right after the write; that edit is the person's, so it
    // is not written over — the copy stays, and the next session tries again.
    if snapshot().as_deref() != Some(rendered.as_str()) {
        return AllowAlwaysNarrowing::Skipped;
    }
    let notice = JsonValue::Object(
        [
            ("settings".to_string(), JsonValue::String(path.display().to_string())),
            ("backup".to_string(), JsonValue::String(backup.display().to_string())),
            (
                "rules".to_string(),
                JsonValue::Array(
                    rules
                        .iter()
                        .map(|rule| JsonValue::String(rule.before.clone()))
                        .collect(),
                ),
            ),
        ]
        .into_iter()
        .collect(),
    );
    // The notice before the marker: a marker without a notice would narrow
    // silently, while a notice without a marker only re-checks a file that has
    // nothing broad left in it.
    let _ = write_private_file(
        &folder.join(NOTICE_FILE),
        notice.render().as_bytes(),
        &ParentDirPolicy::CreateAndRestrict,
    );
    mark_done(folder, AllowAlwaysNarrowing::Narrowed { rules, backup })
}

/// Leave the marker and answer `outcome`, or [`AllowAlwaysNarrowing::Skipped`]
/// when the marker cannot be written — an unmarked project is tried again.
/// A narrowing already written stays written either way.
fn mark_done(folder: &Path, outcome: AllowAlwaysNarrowing) -> AllowAlwaysNarrowing {
    let marked =
        write_private_file(&folder.join(DONE_MARKER), b"", &ParentDirPolicy::CreateAndRestrict)
            .is_ok();
    if marked || matches!(outcome, AllowAlwaysNarrowing::Narrowed { .. }) {
        outcome
    } else {
        AllowAlwaysNarrowing::Skipped
    }
}

/// Keep `original` byte for byte in `folder`, readable by the person alone,
/// under a name no earlier attempt used; `None` when it cannot be kept whole.
fn keep_original(folder: &Path, original: &str) -> Option<PathBuf> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let backup = folder.join(format!("{SETTINGS_FILE}.before-narrowing-{millis}"));
    if std::fs::symlink_metadata(&backup).is_ok() {
        return None;
    }
    write_private_file(&backup, original.as_bytes(), &ParentDirPolicy::CreateAndRestrict).ok()?;
    if std::fs::read(&backup).ok()? == original.as_bytes() {
        Some(backup)
    } else {
        let _ = std::fs::remove_file(&backup);
        None
    }
}

/// Narrow the broad allow rules in `contents`, returning the document to write
/// and the rules rewritten, or `None` when no rule needs it.
///
/// Pure on purpose, like `merge_allow_always_rules` beside it: the form
/// follows the file (`permissions.allow`, or `"…=allow"` specs in the ordered
/// `permissions.rules`), each rule is rewritten where it stands so the order
/// holds, and the candidate is read back through the parser a session uses so
/// a document the renderer would change in any other way is refused.
fn narrow_allow_always_document(
    path: &Path,
    contents: &str,
) -> Result<Option<(String, Vec<NarrowedRule>)>, ConfigError> {
    let mut root = parse_json_object_contents(path, contents)?;
    let existing = parse_optional_permission_rules(&JsonValue::Object(root.clone()))?;
    let [allow_key, _, _, rules_key] = RuntimePermissionRuleConfig::LISTS;
    let ordered = !existing.rules().is_empty();

    let Some(JsonValue::Object(permissions)) = root.get_mut("permissions") else {
        return Ok(None);
    };
    let Some(JsonValue::Array(list)) = permissions.get_mut(if ordered { rules_key } else { allow_key })
    else {
        return Ok(None);
    };
    let mut narrowed = Vec::new();
    for value in list {
        let JsonValue::String(rule) = value else {
            continue;
        };
        let rewritten = if ordered {
            narrowed_ordered_allow_rule(rule)
        } else {
            narrowed_allow_rule(rule)
        }
        .map_err(|error| ConfigError::Parse(format!("{}: {error}", path.display())))?;
        if let Some(after) = rewritten {
            narrowed.push(NarrowedRule {
                before: std::mem::replace(rule, after.clone()),
                after,
            });
        }
    }
    if narrowed.is_empty() {
        return Ok(None);
    }

    let rendered = JsonValue::Object(root.clone()).render_pretty();
    let reread = parse_json_object_contents(path, &rendered)?;
    if reread != root {
        return Err(ConfigError::Parse(format!(
            "{}: the narrowed settings did not survive a JSON round-trip",
            path.display()
        )));
    }
    parse_optional_permission_rules(&JsonValue::Object(reread))?;
    Ok(Some((rendered, narrowed)))
}

fn take_notice_in(folder: &Path) -> Option<String> {
    let path = folder.join(NOTICE_FILE);
    let contents = std::fs::read_to_string(&path).ok()?;
    std::fs::remove_file(&path).ok()?;
    let JsonValue::Object(notice) = JsonValue::parse(&contents).ok()? else {
        return None;
    };
    let text = |key: &str| match notice.get(key) {
        Some(JsonValue::String(value)) => Some(value.clone()),
        _ => None,
    };
    let Some(JsonValue::Array(rules)) = notice.get("rules") else {
        return None;
    };
    let rules = rules
        .iter()
        .filter_map(|rule| match rule {
            JsonValue::String(rule) => Some(rule.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if rules.is_empty() {
        return None;
    }
    Some(narrow_notice_text(&text("settings")?, &rules, &text("backup")?))
}

/// The notice's words: the count, up to [`NOTICE_RULES_SHOWN`] rules as they
/// were with the rest counted, where the original is kept, and what to do if
/// a rule was wide on purpose.
fn narrow_notice_text(settings: &str, rules: &[String], backup: &str) -> String {
    let count = rules.len();
    let mut shown = rules
        .iter()
        .take(NOTICE_RULES_SHOWN)
        .map(|rule| format!("`{rule}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let rest = count.saturating_sub(NOTICE_RULES_SHOWN);
    if rest > 0 {
        let _ = write!(shown, " and {rest} more");
    }
    let noun = if count == 1 { "rule" } else { "rules" };
    format!(
        "zo narrowed {count} \"always allow\" {noun} in {settings} that an earlier zo saved wider \
         than the command approved: {shown}. The approved command is still allowed; anything \
         else asks again. The file as it was is kept at {backup}. If you wrote one of these \
         wide on purpose, write it again."
    )
}

#[cfg(test)]
mod tests {
    use super::{
        narrow_allow_always_document, narrow_in, narrow_notice_text, narrow_with,
        take_notice_in, AllowAlwaysNarrowing, NarrowedRule, DONE_MARKER, NOTICE_FILE,
        SETTINGS_FILE, STATE_FOLDER,
    };
    use crate::config::tests::{init_git_repo_with, overrides_stable};
    use crate::config::{ConfigLoader, RuntimeConfig};
    use crate::json::JsonValue;
    use crate::permissions::{PermissionMode, PermissionOutcome, PermissionPolicy};
    use crate::test_root::TestRoot;
    use std::fs;
    use std::path::{Path, PathBuf};

    const BROAD: &str = "bash(cargo test build/*)";
    const NARROWED: &str = "bash(cargo test build/[*])";
    const APPROVED: &str = "cargo test build/*";
    const WIDER: &str = "cargo test build/x && cargo publish";

    /// A project whose `.zo/settings.local.json` holds `settings`, uncommitted
    /// in a git repository (the person's trusted file, unless `commit`), with
    /// a zo state folder of its own.
    struct Project {
        _root: TestRoot,
        cwd: PathBuf,
        home: PathBuf,
        folder: PathBuf,
    }

    impl Project {
        fn new(settings: &str, commit: bool) -> Self {
            let root = TestRoot::new("allow-always-narrow");
            let cwd = root.join("project");
            let home = root.join("home").join(".zo");
            fs::create_dir_all(cwd.join(".zo")).expect("project .zo");
            fs::create_dir_all(&home).expect("home .zo");
            fs::write(cwd.join(".zo").join(SETTINGS_FILE), settings).expect("write settings");
            init_git_repo_with(&cwd, ".zo/settings.local.json", commit);
            let folder = root.join("state").join(STATE_FOLDER);
            Self { _root: root, cwd, home, folder }
        }

        fn settings_path(&self) -> PathBuf {
            self.cwd.join(".zo").join(SETTINGS_FILE)
        }

        fn settings(&self) -> String {
            fs::read_to_string(self.settings_path()).expect("read settings")
        }

        fn narrow(&self) -> AllowAlwaysNarrowing {
            narrow_in(&self.cwd, &self.folder)
        }

        fn load(&self) -> RuntimeConfig {
            ConfigLoader::new(&self.cwd, &self.home).load().expect("config loads")
        }

        fn marked(&self) -> bool {
            self.folder.join(DONE_MARKER).exists()
        }

        /// Every file in the state folder, sorted; empty when it is absent.
        fn state_files(&self) -> Vec<String> {
            let Ok(entries) = fs::read_dir(&self.folder) else {
                return Vec::new();
            };
            let mut names = entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            names.sort();
            names
        }
    }

    /// How a session built from `config` answers `command` with nobody to ask:
    /// `Allow` from a rule, `Deny` where it would have asked.
    fn decide(config: &RuntimeConfig, command: &str) -> PermissionOutcome {
        PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
            .with_permission_rules(config.permission_rules())
            .authorize("bash", &serde_json::json!({ "command": command }).to_string(), None)
    }

    fn asks(outcome: &PermissionOutcome) -> bool {
        matches!(outcome, PermissionOutcome::Deny { .. })
    }

    fn object(contents: &str) -> std::collections::BTreeMap<String, JsonValue> {
        match JsonValue::parse(contents).expect("json") {
            JsonValue::Object(object) => object,
            other => panic!("not an object: {other:?}"),
        }
    }

    fn list(contents: &str, key: &str) -> Vec<String> {
        let root = object(contents);
        let Some(JsonValue::Object(permissions)) = root.get("permissions") else {
            panic!("no permissions in {contents}");
        };
        let Some(JsonValue::Array(items)) = permissions.get(key) else {
            panic!("no permissions.{key} in {contents}");
        };
        items
            .iter()
            .map(|item| match item {
                JsonValue::String(rule) => rule.clone(),
                other => panic!("not a rule: {other:?}"),
            })
            .collect()
    }

    /// The broad rule stops allowing the wider command and keeps allowing the
    /// approved one, and the file read back by a later session says the same.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn a_broad_allow_rule_stops_allowing_the_wider_command_and_reads_back_the_same() {
        let _stable = overrides_stable();
        let project = Project::new(
            &format!(r#"{{"permissions":{{"allow":["bash(git status:*)","{BROAD}"]}}}}"#),
            false,
        );
        let before = project.load();
        assert_eq!(decide(&before, WIDER), PermissionOutcome::Allow, "the old rule is broad");

        let outcome = project.narrow();
        let AllowAlwaysNarrowing::Narrowed { rules, .. } = &outcome else {
            panic!("expected a narrowing, got {outcome:?}");
        };
        assert_eq!(
            rules,
            &[NarrowedRule { before: BROAD.to_string(), after: NARROWED.to_string() }]
        );
        assert_eq!(list(&project.settings(), "allow"), ["bash(git status:*)", NARROWED]);

        let after = project.load();
        assert!(asks(&decide(&after, WIDER)), "the wider command asks again");
        assert_eq!(decide(&after, APPROVED), PermissionOutcome::Allow);
        assert_eq!(decide(&after, "git status --short"), PermissionOutcome::Allow);
    }

    /// Only the broad allow rule moves: the person's own shapes, deny and ask
    /// rules, other keys and the order of every list stay as they were.
    #[test]
    fn only_the_broad_allow_rule_moves_in_an_allow_document() {
        let path = Path::new("/project/.zo/settings.local.json");
        let before = format!(
            r#"{{"model":"opus","permissions":{{"allow":["bash(git status:*)","{BROAD}","bash(*)","bash","bash(ls [*].rs)"],"deny":["bash(rm *)"],"ask":["bash(git push *)"],"defaultMode":"default"}},"env":{{"A":"1"}}}}"#
        );
        let (rendered, rules) = narrow_allow_always_document(path, &before)
            .expect("narrows")
            .expect("one broad rule");
        assert_eq!(rules.len(), 1);
        assert_eq!(
            list(&rendered, "allow"),
            ["bash(git status:*)", NARROWED, "bash(*)", "bash", "bash(ls [*].rs)"]
        );
        assert_eq!(list(&rendered, "deny"), ["bash(rm *)"]);
        assert_eq!(list(&rendered, "ask"), ["bash(git push *)"]);
        let (mut was, mut now) = (object(&before), object(&rendered));
        for root in [&mut was, &mut now] {
            let Some(JsonValue::Object(permissions)) = root.get_mut("permissions") else {
                panic!("permissions");
            };
            permissions.remove("allow");
        }
        assert_eq!(was, now, "every other key is as it was");
    }

    /// The ordered form narrows only `…=allow` specs, where they stand.
    #[test]
    fn only_the_broad_allow_spec_moves_in_an_ordered_document() {
        let path = Path::new("/project/.zo/settings.local.json");
        let before = format!(
            r#"{{"permissions":{{"rules":["bash(rm *)=deny","{BROAD}=allow","bash(git status:*)=allow","bash(*)=ask","bash(ls [*].rs)=allow"]}},"model":"opus"}}"#
        );
        let (rendered, _) = narrow_allow_always_document(path, &before)
            .expect("narrows")
            .expect("one broad rule");
        assert_eq!(
            list(&rendered, "rules"),
            [
                "bash(rm *)=deny".to_string(),
                format!("{NARROWED}=allow"),
                "bash(git status:*)=allow".to_string(),
                "bash(*)=ask".to_string(),
                "bash(ls [*].rs)=allow".to_string(),
            ]
        );
        assert_eq!(object(&rendered).get("model"), object(&before).get("model"));
    }

    /// A file holding only narrow rules is not written, and the project is
    /// marked: the literal form is never wrapped a second time.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn an_already_narrow_rule_stays_byte_for_byte() {
        let before = r#"{ "permissions": { "allow": ["bash(ls [*].rs)", "bash(git status:*)"] } }"#;
        let project = Project::new(before, false);
        assert_eq!(project.narrow(), AllowAlwaysNarrowing::NothingToNarrow);
        assert_eq!(project.settings(), before);
        assert!(project.marked());
        assert_eq!(project.state_files(), [DONE_MARKER]);
    }

    /// Once marked, a project is not read again: a second run changes no byte,
    /// and a glob the person writes by hand afterwards stays a glob.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn a_second_run_changes_nothing_and_a_glob_written_later_stays_a_glob() {
        let _stable = overrides_stable();
        let project = Project::new(&format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#), false);
        assert!(matches!(project.narrow(), AllowAlwaysNarrowing::Narrowed { .. }));
        let narrowed = project.settings();
        assert_eq!(project.narrow(), AllowAlwaysNarrowing::AlreadyDone);
        assert_eq!(project.settings(), narrowed);

        let by_hand = format!(r#"{{"permissions":{{"allow":["{NARROWED}","bash(npm run *)"]}}}}"#);
        fs::write(project.settings_path(), &by_hand).expect("person edits");
        assert_eq!(project.narrow(), AllowAlwaysNarrowing::AlreadyDone);
        assert_eq!(project.settings(), by_hand);
        assert_eq!(decide(&project.load(), "npm run build"), PermissionOutcome::Allow);
    }

    /// A committed file is not the person's trusted one: nothing is written,
    /// kept or marked, so a later session tries again.
    #[test]
    fn an_untrusted_file_is_left_as_it_was_and_unmarked() {
        let before = format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#);
        let project = Project::new(&before, true);
        assert_eq!(project.narrow(), AllowAlwaysNarrowing::Skipped);
        assert_eq!(project.settings(), before);
        assert!(project.state_files().is_empty(), "{:?}", project.state_files());
    }

    /// A file that changes between the read and the write is not written over.
    #[test]
    fn a_file_that_changes_while_it_is_read_is_left_as_it_was_and_unmarked() {
        let project = Project::new("{}", false);
        let before = format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#);
        let mut reads = [before.clone(), format!("{before} ")].into_iter();
        let mut writes = 0;
        let outcome = narrow_with(&project.cwd, &project.folder, &mut || reads.next(), &mut |_| {
            writes += 1;
            Ok(())
        });
        assert_eq!(outcome, AllowAlwaysNarrowing::Skipped);
        assert_eq!(writes, 0, "nothing is written over a newer edit");
        assert!(project.state_files().is_empty(), "{:?}", project.state_files());
    }

    /// A failing write puts the original back and leaves no marker; when even
    /// that fails, the kept copy stays because it is the only whole one.
    #[test]
    fn a_failed_write_puts_the_original_back_and_leaves_no_marker() {
        let before = format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#);

        let project = Project::new(&before, false);
        let mut written = Vec::new();
        let outcome = narrow_with(
            &project.cwd,
            &project.folder,
            &mut || Some(before.clone()),
            &mut |bytes| {
                written.push(bytes.to_vec());
                if written.len() == 1 {
                    Err(std::io::Error::other("disk full"))
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(outcome, AllowAlwaysNarrowing::Skipped);
        assert_eq!(written.last().map(Vec::as_slice), Some(before.as_bytes()));
        assert!(project.state_files().is_empty(), "{:?}", project.state_files());

        let project = Project::new(&before, false);
        let outcome = narrow_with(
            &project.cwd,
            &project.folder,
            &mut || Some(before.clone()),
            &mut |_| Err(std::io::Error::other("disk full")),
        );
        assert_eq!(outcome, AllowAlwaysNarrowing::Skipped);
        let kept = project.state_files();
        assert!(!project.marked() && kept.len() == 1, "{kept:?}");
        assert_eq!(fs::read_to_string(project.folder.join(&kept[0])).expect("kept"), before);
    }

    /// The file as it was is kept byte for byte, readable by the person alone,
    /// and outside the repository.
    #[test]
    #[cfg(unix)]
    fn the_original_is_kept_byte_for_byte_and_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let before = format!("{{\n  \"permissions\" : {{ \"allow\" : [ \"{BROAD}\" ] }}\n}}\n");
        let project = Project::new(&before, false);
        let AllowAlwaysNarrowing::Narrowed { backup, .. } = project.narrow() else {
            panic!("expected a narrowing");
        };
        assert!(backup.starts_with(&project.folder) && !backup.starts_with(&project.cwd));
        assert_eq!(fs::read(&backup).expect("kept"), before.as_bytes());
        let mode = fs::metadata(&backup).expect("kept").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let folder_mode = fs::metadata(&project.folder).expect("folder").permissions().mode() & 0o777;
        assert_eq!(folder_mode, 0o700);
    }

    /// A project with no settings file holds no saved rule; it is marked so a
    /// glob the person writes into a new file later stays theirs.
    #[test]
    fn a_project_without_the_file_is_marked_and_nothing_is_created() {
        let project = Project::new("{}", false);
        fs::remove_file(project.settings_path()).expect("remove settings");
        let outcome = narrow_with(&project.cwd, &project.folder, &mut || panic!("no read"), &mut |_| {
            panic!("no write")
        });
        assert_eq!(outcome, AllowAlwaysNarrowing::NothingToNarrow);
        assert!(project.marked() && !project.settings_path().exists());
    }

    /// The narrowing leaves a notice that waits — through any number of runs
    /// nobody watches — for the first session that says it, and is said once.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn the_notice_waits_for_the_first_watched_session_and_is_said_once() {
        let project = Project::new(&format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#), false);
        let AllowAlwaysNarrowing::Narrowed { backup, .. } = project.narrow() else {
            panic!("expected a narrowing");
        };
        // An unattended run later: the project is done, the notice still waits.
        assert_eq!(project.narrow(), AllowAlwaysNarrowing::AlreadyDone);
        assert!(project.folder.join(NOTICE_FILE).exists());

        let said = take_notice_in(&project.folder).expect("the notice");
        assert!(said.contains("narrowed 1 \"always allow\" rule "), "{said}");
        assert!(said.contains(&format!("`{BROAD}`")), "{said}");
        assert!(said.contains(&backup.display().to_string()), "{said}");
        assert!(said.contains(&project.settings_path().display().to_string()), "{said}");
        assert!(said.contains("write it again"), "{said}");
        assert_eq!(take_notice_in(&project.folder), None, "said once");
    }

    /// The notice names five rules and counts the rest.
    #[test]
    fn the_notice_names_five_rules_and_counts_the_rest() {
        let rules = (1..=7).map(|n| format!("bash(make t{n} *)")).collect::<Vec<_>>();
        let said = narrow_notice_text("/p/.zo/settings.local.json", &rules, "/s/kept");
        assert!(said.starts_with("zo narrowed 7 \"always allow\" rules in /p/.zo/settings.local.json"), "{said}");
        assert!(said.contains("`bash(make t5 *)` and 2 more."), "{said}");
        assert!(!said.contains("t6"), "{said}");
    }

    /// The session's own load narrows first, so the policy that very session
    /// builds is narrow (and the state folder is the project's zo state).
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn the_session_that_narrows_builds_a_narrow_policy() {
        let _stable = overrides_stable();
        let _env = crate::test_env_lock();
        let project = Project::new(&format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#), false);
        let state_root = project.cwd.parent().expect("root").join("zo-state");
        let prior = std::env::var_os(core_types::paths::ZO_STATE_DIR_ENV);
        std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, &state_root);

        let config = ConfigLoader::new(&project.cwd, &project.home).load_for_session();
        let folder = crate::config::zo_project_state_dir(&project.cwd).join(STATE_FOLDER);

        match prior {
            Some(value) => std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, value),
            None => std::env::remove_var(core_types::paths::ZO_STATE_DIR_ENV),
        }
        let config = config.expect("config loads");
        assert!(asks(&decide(&config, WIDER)), "the session that narrowed asks again");
        assert_eq!(decide(&config, APPROVED), PermissionOutcome::Allow);
        assert!(folder.starts_with(&state_root) && folder.join(DONE_MARKER).exists());
        assert!(folder.join(NOTICE_FILE).exists());
    }
}
