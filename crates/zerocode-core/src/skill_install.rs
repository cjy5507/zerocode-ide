//! Installing the skills this product ships, from the binary itself.
//!
//! `skill.rs` reads and never writes — those are directories the person and
//! their agents own. This module is the one deliberate exception, and it is
//! narrow on purpose: it writes only the skills carried inside this binary
//! (`skills/computer-use`, `skills/orchestration`, `skills/second-brain`), only into the global root
//! each detected agent actually reads, and only over a file that is already
//! ours. Anything else that lives at the destination is left as it is and
//! reported.
//!
//! Why the app carries its own copy: the manual road
//! (`skill::skill_install_command`, `npx skills add <repository>`) fetches
//! from a repository not every person can reach, and it needs Node on PATH.
//! The person asked for the install to happen inside the app ("우리 자체에
//! 설치 될 수 있게", 2026-09-02). The npx road stays as the typed fallback.
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::skill::{
    SKILL_FILE, SkillSource, SourceKind, discovery_sources, orchestration_source_ids,
};

/// A skill shipped inside this binary.
#[derive(Debug, Clone, Copy)]
pub struct BundledSkill {
    pub name: &'static str,
    pub content: &'static str,
}

// Every skill this build carries, generated from the source directory.
include!(concat!(env!("OUT_DIR"), "/bundled_skills.rs"));

#[must_use]
pub fn bundled_skill(name: &str) -> Option<&'static BundledSkill> {
    BUNDLED_SKILLS
        .iter()
        .find(|skill| skill.name == name.trim())
}

/// What happened at one agent's root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallState {
    /// The file was written (new, or an older copy of ours replaced).
    Written,
    /// The same bytes were already there.
    Unchanged,
    /// A file that is not ours sits there; it was left untouched.
    Kept,
    /// No root is known for the agent, or the write failed.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillInstallOutcome {
    pub agent: String,
    pub label: String,
    /// The `SKILL.md` the outcome is about; empty when no root is known.
    pub path: PathBuf,
    pub state: InstallState,
    /// Why, when the state needs a reason.
    pub detail: String,
}

/// The declared global root an agent reads. Existing private roots are reused;
/// a shared root is accepted only when the capability table names that location.
/// An unknown agent has no install root and cannot cause a write.
#[must_use]
pub fn install_root<'a>(agent: &str, sources: &'a [SkillSource]) -> Option<&'a SkillSource> {
    if let Some(own) = sources
        .iter()
        .find(|source| source.kind == SourceKind::Home && source.owner.as_deref() == Some(agent))
    {
        return Some(own);
    }
    if let Some(relative) = crate::skill::install_target(agent).and_then(|target| target.extra_home)
        && let Some(shared) = sources
            .iter()
            .find(|source| source.kind == SourceKind::Home && source.path.ends_with(relative))
    {
        return Some(shared);
    }
    orchestration_source_ids(agent).iter().find_map(|id| {
        sources.iter().find(|source| {
            source.id == *id && source.kind == SourceKind::Home && source.owner.is_some()
        })
    })
}

/// The agent's own home root: the directory its skills folder sits in.
///
/// Named here rather than derived at each caller because two different things
/// are written into it now — the bundled skills below, and the second brain's
/// global guide block (`second_brain::guide_targets`) — and a second answer to
/// "where does this agent live" is how those two would land in different
/// directories for the same agent.
#[must_use]
pub fn agent_home(agent: &str, sources: &[SkillSource]) -> Option<PathBuf> {
    install_root(agent, sources)?
        .path
        .parent()
        .map(Path::to_path_buf)
}

/// Whether a `SKILL.md` at the destination is one of ours — the frontmatter
/// names the skill and the body speaks of this product. Anything else is
/// somebody else's file and is kept.
fn is_ours(existing: &str, name: &str) -> bool {
    let named = existing
        .lines()
        .take(8)
        .any(|line| line.trim() == format!("name: {name}"));
    named && existing.contains("ZeroCode")
}

/// Install one bundled skill for every detected agent. `detected` is
/// `(agent id, label)` in the picker's order; the outcomes keep that order.
///
/// # Errors
/// The name is not a skill this build carries.
pub fn install_bundled_skill(
    name: &str,
    home: &Path,
    detected: &[(String, String)],
) -> Result<Vec<SkillInstallOutcome>, String> {
    let skill = bundled_skill(name)
        .ok_or_else(|| format!("{name}: 이 빌드가 담고 있는 스킬이 아닙니다"))?;
    let sources = discovery_sources(home, &[]);
    Ok(detected
        .iter()
        .map(|(agent, label)| install_one(skill, agent, label, &sources))
        .collect())
}

fn install_one(
    skill: &BundledSkill,
    agent: &str,
    label: &str,
    sources: &[SkillSource],
) -> SkillInstallOutcome {
    let outcome = |path: PathBuf, state: InstallState, detail: String| SkillInstallOutcome {
        agent: agent.to_string(),
        label: label.to_string(),
        path,
        state,
        detail,
    };
    let Some(root) = install_root(agent, sources) else {
        return outcome(
            PathBuf::new(),
            InstallState::Failed,
            "이 에이전트가 읽는 스킬 폴더를 모릅니다".to_string(),
        );
    };
    let dir = root.path.join(skill.name);
    let path = dir.join(SKILL_FILE);
    // A plain file where the skill's directory should stand is somebody
    // else's: reading "<file>/SKILL.md" would only say "not a directory".
    if let Standing::Other = standing_of(&dir) {
        return outcome(
            path,
            InstallState::Kept,
            "스킬 폴더 자리에 다른 파일이 있어 그대로 두었습니다".to_string(),
        );
    }
    match std::fs::read_to_string(&path) {
        Ok(existing) if existing == skill.content => {
            outcome(path, InstallState::Unchanged, String::new())
        }
        Ok(existing) if !is_ours(&existing, skill.name) => outcome(
            path,
            InstallState::Kept,
            "다른 파일이 이미 있어 그대로 두었습니다".to_string(),
        ),
        Ok(_) => write_skill(&dir, &path, skill.content, outcome),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            write_skill(&dir, &path, skill.content, outcome)
        }
        Err(error) => outcome(path, InstallState::Failed, error.to_string()),
    }
}

/// What stands where the skill's directory should be, before we write into
/// it. A link whose target is gone is a dead link — a repository install
/// (`npx skills add` links each agent's folder to one checkout) whose
/// checkout was removed since; `create_dir_all` answers it with "File exists"
/// and nothing ever gets installed there, which is how Qwen Code and Grok
/// stayed at "1 required skill missing" however often the person pressed
/// install (2026-09-26). It is ours to replace. A link that still reaches a
/// directory is followed and written through. A plain file is somebody
/// else's and is kept.
enum Standing {
    /// Nothing, or a directory (reached directly or through a live link).
    Room,
    /// A symbolic link whose target no longer exists.
    DeadLink,
    /// A regular file (or anything else that is not a directory).
    Other,
}

fn standing_of(dir: &Path) -> Standing {
    match std::fs::symlink_metadata(dir) {
        Err(_) => Standing::Room,
        Ok(meta) if meta.file_type().is_symlink() => {
            if dir.is_dir() {
                Standing::Room
            } else {
                Standing::DeadLink
            }
        }
        Ok(meta) if meta.is_dir() => Standing::Room,
        Ok(_) => Standing::Other,
    }
}

fn write_skill(
    dir: &Path,
    path: &Path,
    content: &str,
    outcome: impl Fn(PathBuf, InstallState, String) -> SkillInstallOutcome,
) -> SkillInstallOutcome {
    let mut detail = String::new();
    match standing_of(dir) {
        Standing::Room => {}
        Standing::DeadLink => {
            if let Err(error) = std::fs::remove_file(dir) {
                return outcome(path.to_path_buf(), InstallState::Failed, error.to_string());
            }
            detail = "끊어진 링크를 지우고 설치했습니다".to_string();
        }
        Standing::Other => {
            return outcome(
                path.to_path_buf(),
                InstallState::Kept,
                "스킬 폴더 자리에 다른 파일이 있어 그대로 두었습니다".to_string(),
            );
        }
    }
    let written = std::fs::create_dir_all(dir).and_then(|()| std::fs::write(path, content));
    match written {
        Ok(()) => outcome(path.to_path_buf(), InstallState::Written, detail),
        Err(error) => outcome(path.to_path_buf(), InstallState::Failed, error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detected(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(agent, label)| ((*agent).to_string(), (*label).to_string()))
            .collect()
    }

    /// The skill's directory as one agent's root holds it before an install.
    fn claude_skill_dir(home: &Path) -> PathBuf {
        home.join(".claude").join("skills").join("orchestration")
    }

    #[cfg(unix)]
    #[test]
    fn a_dead_link_where_the_skill_should_stand_is_replaced_by_the_skill() {
        let home = tempfile::tempdir().unwrap();
        let dir = claude_skill_dir(home.path());
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        // `npx skills add` linked this agent's folder to a checkout that is gone.
        std::os::unix::fs::symlink("../../.agents/skills/orchestration", &dir).unwrap();
        assert!(!dir.exists() && std::fs::symlink_metadata(&dir).is_ok());
        let outcomes = install_bundled_skill(
            "orchestration",
            home.path(),
            &detected(&[("claude", "Claude")]),
        )
        .unwrap();
        assert_eq!(
            outcomes[0].state,
            InstallState::Written,
            "{}",
            outcomes[0].detail
        );
        assert!(outcomes[0].detail.contains("끊어진 링크"));
        assert!(
            dir.is_dir()
                && !std::fs::symlink_metadata(&dir)
                    .unwrap()
                    .file_type()
                    .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(&outcomes[0].path).unwrap(),
            bundled_skill("orchestration").unwrap().content
        );
        // The second press finds the same bytes and changes nothing.
        let again = install_bundled_skill(
            "orchestration",
            home.path(),
            &detected(&[("claude", "Claude")]),
        )
        .unwrap();
        assert_eq!(again[0].state, InstallState::Unchanged);
    }

    #[cfg(unix)]
    #[test]
    fn a_live_link_to_a_directory_is_written_through_not_replaced() {
        let home = tempfile::tempdir().unwrap();
        let dir = claude_skill_dir(home.path());
        let shared = home
            .path()
            .join(".agents")
            .join("skills")
            .join("orchestration");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&shared, &dir).unwrap();
        let outcomes = install_bundled_skill(
            "orchestration",
            home.path(),
            &detected(&[("claude", "Claude")]),
        )
        .unwrap();
        assert_eq!(
            outcomes[0].state,
            InstallState::Written,
            "{}",
            outcomes[0].detail
        );
        assert!(
            std::fs::symlink_metadata(&dir)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the live link stays"
        );
        assert!(
            shared.join(SKILL_FILE).is_file(),
            "written through the link into the shared root"
        );
    }

    #[test]
    fn a_plain_file_where_the_skill_should_stand_is_kept_and_named() {
        let home = tempfile::tempdir().unwrap();
        let dir = claude_skill_dir(home.path());
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::fs::write(&dir, "not a folder").unwrap();
        let outcomes = install_bundled_skill(
            "orchestration",
            home.path(),
            &detected(&[("claude", "Claude")]),
        )
        .unwrap();
        assert_eq!(
            outcomes[0].state,
            InstallState::Kept,
            "{}",
            outcomes[0].detail
        );
        assert!(outcomes[0].detail.contains("다른 파일"));
        assert_eq!(std::fs::read_to_string(&dir).unwrap(), "not a folder");
    }

    #[test]
    fn artifact_skills_install_through_the_shared_bundle_for_each_agent() {
        let home = tempfile::tempdir().unwrap();
        for name in ["artifact-design", "dataviz", "artifact-diagramming"] {
            let outcomes = install_bundled_skill(
                name,
                home.path(),
                &detected(&[("claude", "Claude"), ("codex", "Codex"), ("zo", "zo")]),
            )
            .unwrap();
            assert_eq!(outcomes.len(), 3);
            for outcome in outcomes {
                assert_eq!(outcome.state, InstallState::Written);
                assert_eq!(
                    std::fs::read_to_string(&outcome.path).unwrap(),
                    bundled_skill(name).unwrap().content
                );
            }
        }
    }

    #[test]
    fn the_bundled_skills_are_the_checkouts_own_files() {
        for skill in BUNDLED_SKILLS {
            assert!(
                skill.content.starts_with("---\nname: "),
                "{} has no frontmatter",
                skill.name
            );
            assert!(
                is_ours(skill.content, skill.name),
                "{} would not be recognised as ours",
                skill.name
            );
        }
        assert!(bundled_skill("computer-use").is_some());
        assert!(bundled_skill("orchestration").is_some());
        assert!(bundled_skill("second-brain").is_some());
        assert!(bundled_skill("playwright").is_none());
    }

    #[test]
    fn a_bundled_skill_lands_in_each_agents_own_root_and_only_there() {
        let home = tempfile::tempdir().unwrap();
        let outcomes = install_bundled_skill(
            "computer-use",
            home.path(),
            &detected(&[
                ("claude", "Claude"),
                ("codex", "Codex"),
                ("zo", "ZO"),
                ("mystery", "Mystery"),
            ]),
        )
        .unwrap();
        let states: Vec<(&str, InstallState)> = outcomes
            .iter()
            .map(|row| (row.agent.as_str(), row.state))
            .collect();
        assert_eq!(
            states,
            vec![
                ("claude", InstallState::Written),
                ("codex", InstallState::Written),
                ("zo", InstallState::Written),
                ("mystery", InstallState::Failed),
            ]
        );
        for (agent, dir) in [("claude", ".claude"), ("codex", ".codex"), ("zo", ".zo")] {
            let expected = home
                .path()
                .join(dir)
                .join("skills")
                .join("computer-use")
                .join(SKILL_FILE);
            let row = outcomes.iter().find(|row| row.agent == agent).unwrap();
            assert_eq!(row.path, expected);
            assert_eq!(
                std::fs::read_to_string(&expected).unwrap(),
                bundled_skill("computer-use").unwrap().content
            );
        }
        // The shared `.agents` convention is not written: that is somebody
        // else's installer's place.
        assert!(!home.path().join(".agents").exists());
        // Nothing for an agent whose root the table does not know.
        assert!(outcomes[3].path.as_os_str().is_empty());

        // Again: the same bytes are already there.
        let again = install_bundled_skill(
            "computer-use",
            home.path(),
            &detected(&[("claude", "Claude")]),
        )
        .unwrap();
        assert_eq!(again[0].state, InstallState::Unchanged);
    }

    #[test]
    fn the_second_brain_skill_installs_for_claude_codex_and_zo() {
        let home = tempfile::tempdir().unwrap();
        let outcomes = install_bundled_skill(
            "second-brain",
            home.path(),
            &detected(&[("claude", "Claude"), ("codex", "Codex"), ("zo", "ZO")]),
        )
        .unwrap();

        assert!(
            outcomes
                .iter()
                .all(|outcome| outcome.state == InstallState::Written)
        );
        for agent_home in [".claude", ".codex", ".zo"] {
            let installed = home
                .path()
                .join(agent_home)
                .join("skills/second-brain/SKILL.md");
            assert_eq!(
                std::fs::read_to_string(installed).unwrap(),
                bundled_skill("second-brain").unwrap().content
            );
        }
    }

    #[test]
    fn a_file_that_is_not_ours_is_kept_and_an_older_copy_of_ours_is_replaced() {
        let home = tempfile::tempdir().unwrap();
        let theirs = home.path().join(".claude/skills/orchestration");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(
            theirs.join(SKILL_FILE),
            "---\nname: orchestration\n---\nSomebody's own guide.\n",
        )
        .unwrap();
        let ours_old = home.path().join(".codex/skills/orchestration");
        std::fs::create_dir_all(&ours_old).unwrap();
        std::fs::write(
            ours_old.join(SKILL_FILE),
            "---\nname: orchestration\n---\nAn older ZeroCode orchestration guide.\n",
        )
        .unwrap();

        let outcomes = install_bundled_skill(
            "orchestration",
            home.path(),
            &detected(&[("claude", "Claude"), ("codex", "Codex")]),
        )
        .unwrap();
        assert_eq!(outcomes[0].state, InstallState::Kept);
        assert_eq!(
            std::fs::read_to_string(theirs.join(SKILL_FILE)).unwrap(),
            "---\nname: orchestration\n---\nSomebody's own guide.\n",
            "a foreign file was rewritten"
        );
        assert_eq!(outcomes[1].state, InstallState::Written);
        assert_eq!(
            std::fs::read_to_string(ours_old.join(SKILL_FILE)).unwrap(),
            bundled_skill("orchestration").unwrap().content
        );
    }

    #[test]
    fn a_name_this_build_does_not_carry_is_refused() {
        let home = tempfile::tempdir().unwrap();
        assert!(
            install_bundled_skill(
                "../etc/passwd",
                home.path(),
                &detected(&[("claude", "Claude")])
            )
            .is_err()
        );
        assert!(!home.path().join(".claude").exists());
    }
}
