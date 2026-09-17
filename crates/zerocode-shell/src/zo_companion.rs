//! zo rides along with the app (t-3191, docs/design/versioned-auto-update.md
//! §2.4): the release bundle carries `Resources/bin/zo`, and at boot the
//! window makes `~/.local/bin/zo` that binary when it is missing or older —
//! `.zo.new` beside it, then one rename, so a zo that is running keeps its
//! inode. A zo a person installed themselves (a *higher* version) is left
//! alone and said so in the settings pane. Versions are semver, never
//! strings: 1.3.0 follows the older zo's 1.2.7 (§2.7).
//!
//! The judgement ([`plan`]) is pure; the edges are three small functions —
//! read the installed version by running `zo --version`, copy-then-rename,
//! find the bundled binary — and [`run_at_boot`] strings them together once.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where a person's zo lives, relative to the home directory — the path the
/// release lane swaps too (`RELEASE_ZO_BIN`, tools/release/lane.sh), and the
/// one the older lineage's `install.sh` wrote.
const ZO_BIN: &[&str] = &[".local", "bin", "zo"];
/// The staging name beside it; one rename away.
const ZO_NEW: &str = ".zo.new";
/// Where the bundle carries zo, under the app's resource directory
/// (`tauri.release.conf.json` `bundle.resources`).
const BUNDLED_ZO: &[&str] = &["bin", "zo"];

pub(crate) fn zo_path_under(home: &Path) -> PathBuf {
    ZO_BIN
        .iter()
        .fold(home.to_path_buf(), |path, part| path.join(part))
}

/// The bundled zo, if this build carries one. A dev build (`cargo run`,
/// the base `tauri.conf.json`) does not, and has nothing to say.
pub(crate) fn bundled_zo(resource_dir: Option<&Path>) -> Option<PathBuf> {
    let path = BUNDLED_ZO
        .iter()
        .fold(resource_dir?.to_path_buf(), |path, part| path.join(part));
    path.is_file().then_some(path)
}

/// The first semver word of `zo --version`'s output. `zo 1.3.0` today; the
/// older lineage's installer looked for a `  Version 1.2.7` line — either
/// shape, whichever comes first.
pub(crate) fn parse_zo_version(output: &str) -> Option<semver::Version> {
    output
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')')
        .map(|word| word.trim_start_matches('v'))
        .find_map(|word| semver::Version::parse(word).ok())
}

/// What sits at `~/.local/bin/zo`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Installed {
    Missing,
    Version(semver::Version),
    /// A file is there but `--version` did not yield a version; the text is
    /// what it said (or the error), for the report.
    Unreadable(String),
}

/// Run `<bin> --version` and read it. A binary that will not run or will not
/// say is [`Installed::Unreadable`], never a panic and never a guess.
pub(crate) fn installed_version(bin: &Path) -> Installed {
    if !bin.exists() {
        return Installed::Missing;
    }
    match crate::proc::quiet_command(bin).arg("--version").output() {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout);
            match parse_zo_version(&text) {
                Some(version) => Installed::Version(version),
                None => Installed::Unreadable(
                    format!(
                        "{}{}",
                        text.trim(),
                        String::from_utf8_lossy(&output.stderr).trim()
                    )
                    .chars()
                    .take(200)
                    .collect(),
                ),
            }
        }
        Err(error) => Installed::Unreadable(error.to_string()),
    }
}

/// The judgement (§2.4), pure over two versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    /// Nothing there (or nothing readable): the bundle's zo goes in.
    Install,
    /// An older zo: replaced by rename; the version it had.
    Replace(semver::Version),
    /// The same version already.
    Same,
    /// A newer zo somebody installed: left alone; the version it has.
    LeaveNewer(semver::Version),
}

pub(crate) fn plan(app: &semver::Version, installed: Installed) -> Plan {
    match installed {
        Installed::Missing | Installed::Unreadable(_) => Plan::Install,
        Installed::Version(held) => match held.cmp(app) {
            std::cmp::Ordering::Less => Plan::Replace(held),
            std::cmp::Ordering::Equal => Plan::Same,
            std::cmp::Ordering::Greater => Plan::LeaveNewer(held),
        },
    }
}

/// Copy the bundled binary to `.zo.new` beside the target, then rename it
/// over the target. The old inode, if a zo is running from it, stays alive
/// under that process; the path names the new one from here on.
pub(crate) fn install(bundled: &Path, target: &Path) -> std::io::Result<()> {
    let dir = target.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "zo path has no parent")
    })?;
    std::fs::create_dir_all(dir)?;
    let staged = dir.join(ZO_NEW);
    std::fs::copy(bundled, &staged)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&staged, target)
}

/// What the boot did, as the settings pane says it (one line, or nothing).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    NoBundle,
    Installed,
    Same,
    LeftNewer,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Report {
    pub(crate) outcome: Outcome,
    /// The app's version, which is the bundled zo's (§2.1: one version).
    pub(crate) bundled: String,
    /// What `~/.local/bin/zo` said before the boot touched it, if anything.
    pub(crate) installed: Option<String>,
    /// The unreadable text or the failure, for the pane; empty otherwise.
    pub(crate) detail: String,
}

/// Once per boot, off the main thread: find the bundled zo, read the
/// installed one, judge, act, report.
pub(crate) fn run_at_boot(
    resource_dir: Option<&Path>,
    home: &Path,
    app: &semver::Version,
) -> Report {
    let bundled_version = app.to_string();
    let Some(bundled) = bundled_zo(resource_dir) else {
        return Report {
            outcome: Outcome::NoBundle,
            bundled: bundled_version,
            installed: None,
            detail: String::new(),
        };
    };
    let target = zo_path_under(home);
    let held = installed_version(&target);
    let (installed, detail) = match &held {
        Installed::Missing => (None, String::new()),
        Installed::Version(version) => (Some(version.to_string()), String::new()),
        Installed::Unreadable(text) => (None, text.clone()),
    };
    let outcome = match plan(app, held) {
        Plan::Same => Outcome::Same,
        Plan::LeaveNewer(_) => Outcome::LeftNewer,
        Plan::Install | Plan::Replace(_) => match install(&bundled, &target) {
            Ok(()) => Outcome::Installed,
            Err(error) => {
                return Report {
                    outcome: Outcome::Failed,
                    bundled: bundled_version,
                    installed,
                    detail: error.to_string(),
                };
            }
        },
    };
    Report {
        outcome,
        bundled: bundled_version,
        installed,
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> semver::Version {
        semver::Version::parse(text).unwrap()
    }

    /// `zo --version` prints `zo X.Y.Z`; the older lineage's `install.sh`
    /// expects a `  Version X.Y.Z` line (t-3187). Either is read; the first
    /// semver word wins; noise is nothing.
    #[test]
    fn the_installed_version_is_the_first_semver_word() {
        assert_eq!(parse_zo_version("zo 1.3.0\n"), Some(v("1.3.0")));
        assert_eq!(
            parse_zo_version("zo 1.3.0\n  Version 1.3.0\n"),
            Some(v("1.3.0"))
        );
        assert_eq!(parse_zo_version("  Version 1.2.7"), Some(v("1.2.7")));
        assert_eq!(
            parse_zo_version("zo 1.4.0-beta.1 (abc)"),
            Some(v("1.4.0-beta.1"))
        );
        assert_eq!(parse_zo_version("usage: zo [options]"), None);
        assert_eq!(parse_zo_version(""), None);
    }

    /// §2.4: missing or older is replaced; the same is left; a newer one a
    /// person installed is left and named; unreadable is replaced (what sits
    /// there is not a zo this app can speak for).
    #[test]
    fn the_plan_follows_semver() {
        let app = v("1.3.0");
        assert_eq!(plan(&app, Installed::Missing), Plan::Install);
        assert_eq!(
            plan(&app, Installed::Version(v("1.2.7"))),
            Plan::Replace(v("1.2.7"))
        );
        assert_eq!(plan(&app, Installed::Version(v("1.3.0"))), Plan::Same);
        assert_eq!(
            plan(&app, Installed::Version(v("1.3.1"))),
            Plan::LeaveNewer(v("1.3.1"))
        );
        assert_eq!(
            plan(&app, Installed::Version(v("2.0.0-beta.1"))),
            Plan::LeaveNewer(v("2.0.0-beta.1"))
        );
        assert_eq!(
            plan(&v("1.3.0-beta.2"), Installed::Version(v("1.3.0"))),
            Plan::LeaveNewer(v("1.3.0")),
            "a release outranks its own beta"
        );
        assert_eq!(
            plan(&app, Installed::Unreadable("garbage".into())),
            Plan::Install
        );
    }

    #[cfg(unix)]
    fn script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// The whole boot in a temporary home: no zo → installed from the bundle
    /// (`~/.local/bin` made, `.zo.new` gone, the executable bit kept); an
    /// older zo → replaced by rename; the same → left; a newer → left and
    /// named; no bundle → nothing to say.
    #[cfg(unix)]
    #[test]
    fn boot_installs_replaces_or_leaves_zo_in_a_temporary_home() {
        let fixture = tempfile::tempdir().unwrap();
        let home = fixture.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let resources = fixture.path().join("Resources");
        std::fs::create_dir_all(resources.join("bin")).unwrap();
        let bundled = script(&resources.join("bin"), "zo", "echo \"zo 1.3.0\"");
        let app = v("1.3.0");
        let target = zo_path_under(&home);
        assert!(target.ends_with(".local/bin/zo"));

        let report = run_at_boot(Some(&resources), &home, &app);
        assert_eq!(report.outcome, Outcome::Installed);
        assert_eq!(report.bundled, "1.3.0");
        assert_eq!(report.installed, None);
        assert_eq!(
            std::fs::read(&target).unwrap(),
            std::fs::read(&bundled).unwrap()
        );
        assert!(!target.with_file_name(".zo.new").exists());
        assert_eq!(installed_version(&target), Installed::Version(v("1.3.0")));

        // The same version stands.
        let again = run_at_boot(Some(&resources), &home, &app);
        assert_eq!(again.outcome, Outcome::Same);
        assert_eq!(again.installed.as_deref(), Some("1.3.0"));

        // An older zo is replaced: the file is the bundle's afterwards.
        script(target.parent().unwrap(), "zo", "echo \"zo 1.2.7\"");
        let replaced = run_at_boot(Some(&resources), &home, &app);
        assert_eq!(replaced.outcome, Outcome::Installed);
        assert_eq!(replaced.installed.as_deref(), Some("1.2.7"));
        assert_eq!(installed_version(&target), Installed::Version(v("1.3.0")));

        // A newer zo a person installed is left alone and named.
        script(target.parent().unwrap(), "zo", "echo \"zo 1.4.0\"");
        let left = run_at_boot(Some(&resources), &home, &app);
        assert_eq!(left.outcome, Outcome::LeftNewer);
        assert_eq!(left.installed.as_deref(), Some("1.4.0"));
        assert_eq!(installed_version(&target), Installed::Version(v("1.4.0")));

        // Something that is not a zo is replaced, and the report says what it said.
        script(target.parent().unwrap(), "zo", "echo \"not a version\"");
        let over = run_at_boot(Some(&resources), &home, &app);
        assert_eq!(over.outcome, Outcome::Installed);
        assert!(over.detail.contains("not a version"), "{}", over.detail);

        // No bundle (a dev build): nothing to say, nothing touched.
        std::fs::remove_file(&bundled).unwrap();
        let none = run_at_boot(Some(&resources), &home, &app);
        assert_eq!(none.outcome, Outcome::NoBundle);
        assert_eq!(installed_version(&target), Installed::Version(v("1.3.0")));
        assert_eq!(run_at_boot(None, &home, &app).outcome, Outcome::NoBundle);
        assert_eq!(
            installed_version(&fixture.path().join("absent")),
            Installed::Missing
        );
    }

    #[test]
    fn the_report_is_one_serializable_row() {
        let report = Report {
            outcome: Outcome::LeftNewer,
            bundled: "1.3.0".into(),
            installed: Some("1.4.0".into()),
            detail: String::new(),
        };
        assert_eq!(
            serde_json::to_value(&report).unwrap(),
            serde_json::json!({
                "outcome": "left_newer",
                "bundled": "1.3.0",
                "installed": "1.4.0",
                "detail": "",
            })
        );
    }
}
