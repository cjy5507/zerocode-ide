//! Installing the skills this product ships, from the binary itself.
//!
//! `skill.rs` reads and never writes — those are directories the person and
//! their agents own. This module is the one deliberate exception, and it is
//! narrow on purpose: it writes only the skills carried inside this binary
//! (every directory under `skills/`, as `build.rs` lists them), only into the global roots
//! each detected agent actually reads, and only where no copy stands or the copy
//! holds the text of an earlier release (`skill_releases.tsv`). A copy with any
//! other text, a person's edit or somebody else's skill, is left as it is and
//! reported. A skill's reference files (`BundledSkill::files`) are written
//! beside its `SKILL.md`; whether a copy is ours is still read from `SKILL.md`
//! alone.
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
    /// The reference files `content` points to, as `(file name, text)`:
    /// they are installed beside `SKILL.md`, under the same directory.
    pub files: &'static [(&'static str, &'static str)],
}

// Every skill this build carries, generated from the source directory.
include!(concat!(env!("OUT_DIR"), "/bundled_skills.rs"));

#[cfg(test)]
impl BundledSkill {
    /// `SKILL.md` and its reference files in one text, as a test reads a guide
    /// whose parts moved into files beside it.
    pub(crate) fn whole_text(&self) -> String {
        let mut text = self.content.to_string();
        for (_, file) in self.files {
            text.push('\n');
            text.push_str(file);
        }
        text
    }
}

/// Every text each bundled skill's `SKILL.md` has had on main, one
/// `<name>\t<git blob id>` line per text, oldest first: `git log --follow
/// --raw` over `skills/<name>/SKILL.md`. A copy holding one of these texts is
/// an earlier release of ours and is replaced; any other text is kept.
const RELEASES: &str = include_str!("skill_releases.tsv");

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

/// What one copy of a bundled skill is, read against every text this product
/// has shipped for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CopyState {
    /// The same bytes this build carries.
    Current,
    /// The text of an earlier release: ours, and replaced by an install.
    Stale,
    /// Any other text: a person's edit, or somebody else's skill of the same
    /// name. Never written over.
    Edited,
    /// No copy there.
    Missing,
}

/// One copy of a bundled skill, in one root an agent reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillCopy {
    pub agent: String,
    pub label: String,
    /// The `SKILL.md` the copy is, or would be.
    pub path: PathBuf,
    pub state: CopyState,
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

/// Every home `agent` reads: its own under the person's home, then each home
/// a window launches it into, from `launched` as `(agent id, home)`.
///
/// A Claude pane in the window runs with `CLAUDE_CONFIG_DIR` at an app-owned
/// home and never reads `~/.claude`; a Codex pane runs with its own
/// `CODEX_HOME`. Written under the person's home alone, the bundled skills and
/// the second brain's guide block never reached those panes (t-55424).
#[must_use]
pub fn agent_homes(
    agent: &str,
    sources: &[SkillSource],
    launched: &[(String, PathBuf)],
) -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = agent_home(agent, sources).into_iter().collect();
    for (id, home) in launched {
        if id == agent && !homes.contains(home) {
            homes.push(home.clone());
        }
    }
    homes
}

/// The skills folder of each home [`agent_homes`] names.
fn skill_roots(
    agent: &str,
    sources: &[SkillSource],
    launched: &[(String, PathBuf)],
) -> Vec<PathBuf> {
    agent_homes(agent, sources, launched)
        .into_iter()
        .map(|home| home.join("skills"))
        .collect()
}

/// The id `git hash-object` prints for these bytes.
fn git_blob_id(bytes: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", bytes.len()));
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whether `blob` is a text `name` has shipped with.
fn released(name: &str, blob: &str) -> bool {
    RELEASES
        .lines()
        .any(|line| line.split_once('\t') == Some((name, blob)))
}

/// What the bytes of a copy are. Only an earlier release is ours to replace:
/// the rule before this one (the frontmatter names the skill and the body says
/// "ZeroCode") also matched a copy somebody had edited, and wrote over it.
fn copy_state(skill: &BundledSkill, bytes: &[u8]) -> CopyState {
    if bytes == skill.content.as_bytes() {
        CopyState::Current
    } else if released(skill.name, &git_blob_id(bytes)) {
        CopyState::Stale
    } else {
        CopyState::Edited
    }
}

/// Every copy of one bundled skill the detected agents read, and what each
/// copy is. Read only.
///
/// # Errors
/// The name is not a skill this build carries.
pub fn bundled_copies(
    name: &str,
    home: &Path,
    launched: &[(String, PathBuf)],
    detected: &[(String, String)],
) -> Result<Vec<SkillCopy>, String> {
    let skill = bundled_skill(name)
        .ok_or_else(|| format!("{name}: 이 빌드가 담고 있는 스킬이 아닙니다"))?;
    let sources = discovery_sources(home, &[]);
    let mut copies: Vec<SkillCopy> = Vec::new();
    for (agent, label) in detected {
        for root in skill_roots(agent, &sources, launched) {
            let dir = root.join(skill.name);
            let path = dir.join(SKILL_FILE);
            // Two agents that read one root (claude and openclaude, or the
            // shared `.agents` home) hold one copy there, not two.
            if copies.iter().any(|copy| copy.path == path) {
                continue;
            }
            let state = match std::fs::read(&path) {
                Ok(bytes) => match copy_state(skill, &bytes) {
                    // Our text with a reference file gone is an incomplete
                    // install, and an install completes it.
                    CopyState::Current if !missing_references(skill, &dir).is_empty() => {
                        CopyState::Stale
                    }
                    state => state,
                },
                Err(error) if error.kind() == io::ErrorKind::NotFound => CopyState::Missing,
                // A plain file where the skill's folder should stand, or a
                // copy that cannot be read: not ours to touch either way.
                Err(_) => CopyState::Edited,
            };
            copies.push(SkillCopy {
                agent: agent.clone(),
                label: label.clone(),
                path,
                state,
            });
        }
    }
    Ok(copies)
}

/// Install one bundled skill for every detected agent, in every root it
/// reads. `detected` is `(agent id, label)` in the picker's order; the
/// outcomes keep that order, one for each root.
///
/// # Errors
/// The name is not a skill this build carries.
pub fn install_bundled_skill(
    name: &str,
    home: &Path,
    launched: &[(String, PathBuf)],
    detected: &[(String, String)],
) -> Result<Vec<SkillInstallOutcome>, String> {
    let skill = bundled_skill(name)
        .ok_or_else(|| format!("{name}: 이 빌드가 담고 있는 스킬이 아닙니다"))?;
    let sources = discovery_sources(home, &[]);
    let mut outcomes = Vec::new();
    for (agent, label) in detected {
        let roots = skill_roots(agent, &sources, launched);
        if roots.is_empty() {
            outcomes.push(SkillInstallOutcome {
                agent: agent.clone(),
                label: label.clone(),
                path: PathBuf::new(),
                state: InstallState::Failed,
                detail: "이 에이전트가 읽는 스킬 폴더를 모릅니다".to_string(),
            });
        }
        for root in roots {
            outcomes.push(install_one(skill, agent, label, &root));
        }
    }
    Ok(outcomes)
}

fn install_one(skill: &BundledSkill, agent: &str, label: &str, root: &Path) -> SkillInstallOutcome {
    let outcome = |path: PathBuf, state: InstallState, detail: String| SkillInstallOutcome {
        agent: agent.to_string(),
        label: label.to_string(),
        path,
        state,
        detail,
    };
    let dir = root.join(skill.name);
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
    match std::fs::read(&path) {
        Ok(existing) => match copy_state(skill, &existing) {
            CopyState::Current => complete_references(skill, &dir, path, outcome),
            CopyState::Stale => write_skill(&dir, &path, skill, outcome),
            CopyState::Edited | CopyState::Missing => outcome(
                path,
                InstallState::Kept,
                "다른 파일이 이미 있어 그대로 두었습니다".to_string(),
            ),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            write_skill(&dir, &path, skill, outcome)
        }
        Err(error) => outcome(path, InstallState::Failed, error.to_string()),
    }
}

/// The reference files of `skill` that are not in `dir`.
fn missing_references(
    skill: &BundledSkill,
    dir: &Path,
) -> Vec<&'static (&'static str, &'static str)> {
    skill
        .files
        .iter()
        .filter(|(name, _)| !dir.join(name).exists())
        .collect()
}

/// A copy whose `SKILL.md` is the one this build carries: write the reference
/// files it lacks, and leave the ones that stand as they are.
fn complete_references(
    skill: &BundledSkill,
    dir: &Path,
    path: PathBuf,
    outcome: impl Fn(PathBuf, InstallState, String) -> SkillInstallOutcome,
) -> SkillInstallOutcome {
    let missing = missing_references(skill, dir);
    if missing.is_empty() {
        return outcome(path, InstallState::Unchanged, String::new());
    }
    for (name, text) in missing {
        if let Err(error) = std::fs::write(dir.join(name), text) {
            return outcome(path, InstallState::Failed, error.to_string());
        }
    }
    outcome(path, InstallState::Written, String::new())
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
    skill: &BundledSkill,
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
    // The reference files first and `SKILL.md` last: a write that stops half
    // way leaves the old `SKILL.md`, which an install reads as stale and
    // writes again.
    let written = std::fs::create_dir_all(dir)
        .and_then(|()| {
            skill
                .files
                .iter()
                .try_for_each(|(name, text)| std::fs::write(dir.join(name), text))
        })
        .and_then(|()| std::fs::write(path, skill.content));
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

    /// A text an earlier release shipped (blob 9f3b7dbce): every second-brain
    /// copy on the person's machine held it before setup ran on 2026-10-15.
    const SECOND_BRAIN_9F3B7DBCE: &str =
        include_str!("../tests/fixtures/second-brain-9f3b7dbce.md");

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
            &[],
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
            &[],
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
            &[],
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
            &[],
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
                &[],
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
        }
        assert!(bundled_skill("computer-use").is_some());
        assert!(bundled_skill("orchestration").is_some());
        assert!(bundled_skill("second-brain").is_some());
        assert!(
            bundled_skill(crate::agent_emulator::MOBILE_SKILL_NAME).is_some(),
            "this build carries no {} skill",
            crate::agent_emulator::MOBILE_SKILL_NAME
        );
        assert!(bundled_skill("playwright").is_none());
    }

    #[test]
    fn a_bundled_skill_lands_in_each_agents_own_root_and_only_there() {
        let home = tempfile::tempdir().unwrap();
        let outcomes = install_bundled_skill(
            "computer-use",
            home.path(),
            &[],
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
            &[],
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
            &[],
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
        let theirs = home.path().join(".claude/skills/second-brain");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(
            theirs.join(SKILL_FILE),
            "---\nname: second-brain\n---\nSomebody's own guide.\n",
        )
        .unwrap();
        let ours_old = home.path().join(".codex/skills/second-brain");
        std::fs::create_dir_all(&ours_old).unwrap();
        std::fs::write(ours_old.join(SKILL_FILE), SECOND_BRAIN_9F3B7DBCE).unwrap();

        let outcomes = install_bundled_skill(
            "second-brain",
            home.path(),
            &[],
            &detected(&[("claude", "Claude"), ("codex", "Codex")]),
        )
        .unwrap();
        assert_eq!(outcomes[0].state, InstallState::Kept);
        assert_eq!(
            std::fs::read_to_string(theirs.join(SKILL_FILE)).unwrap(),
            "---\nname: second-brain\n---\nSomebody's own guide.\n",
            "a foreign file was rewritten"
        );
        assert_eq!(outcomes[1].state, InstallState::Written);
        assert_eq!(
            std::fs::read_to_string(ours_old.join(SKILL_FILE)).unwrap(),
            bundled_skill("second-brain").unwrap().content
        );
    }

    /// A skill's reference files are installed beside its `SKILL.md`, under
    /// the same name, and the text of `SKILL.md` names each one (t-80606).
    #[test]
    fn a_skills_reference_files_install_beside_its_skill_file() {
        let skill = bundled_skill("orchestration").unwrap();
        assert!(
            !skill.files.is_empty(),
            "the orchestration skill has no reference files"
        );
        for (name, text) in skill.files {
            assert!(
                skill.content.contains(&format!("`{name}`")),
                "SKILL.md does not tell when to read {name}"
            );
            assert!(!text.trim().is_empty(), "{name} is empty");
        }
        let home = tempfile::tempdir().unwrap();
        let outcomes = install_bundled_skill(
            "orchestration",
            home.path(),
            &[],
            &detected(&[("claude", "Claude")]),
        )
        .unwrap();
        assert_eq!(outcomes[0].state, InstallState::Written);
        let dir = claude_skill_dir(home.path());
        for (name, text) in skill.files {
            assert_eq!(
                &std::fs::read_to_string(dir.join(name)).unwrap(),
                text,
                "{name} was not installed as carried"
            );
        }
        let again = install_bundled_skill(
            "orchestration",
            home.path(),
            &[],
            &detected(&[("claude", "Claude")]),
        )
        .unwrap();
        assert_eq!(again[0].state, InstallState::Unchanged);
    }

    /// Our `SKILL.md` with a reference file gone is an incomplete install: it
    /// reads stale, an install writes the missing file only, and a reference
    /// file that stands is never rewritten.
    #[test]
    fn a_current_copy_missing_a_reference_file_is_completed_and_the_rest_left() {
        let skill = bundled_skill("orchestration").unwrap();
        let (gone, _) = skill.files[0];
        let (kept, _) = skill.files[1];
        let home = tempfile::tempdir().unwrap();
        let agents = detected(&[("claude", "Claude")]);
        install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();
        let dir = claude_skill_dir(home.path());
        std::fs::remove_file(dir.join(gone)).unwrap();
        std::fs::write(dir.join(kept), "somebody's own notes\n").unwrap();

        let copies = bundled_copies("orchestration", home.path(), &[], &agents).unwrap();
        assert_eq!(copies[0].state, CopyState::Stale);
        let outcomes = install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();
        assert_eq!(outcomes[0].state, InstallState::Written);
        assert!(
            dir.join(gone).is_file(),
            "the missing reference was not written"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(kept)).unwrap(),
            "somebody's own notes\n",
            "a reference file that stood was rewritten"
        );
        let copies = bundled_copies("orchestration", home.path(), &[], &agents).unwrap();
        assert_eq!(copies[0].state, CopyState::Current);
    }

    /// A copy is told apart by its git blob id, the id `git hash-object`
    /// prints, so the release book is `git log` output and each of its lines
    /// can be checked against the history it came from. The bytes this build
    /// carries are current; the text of an earlier release is stale; anything
    /// else (a line the person added, or somebody else's skill of the same
    /// name) is hand-edited; no copy at all is missing (t-55424).
    #[test]
    fn a_copy_reads_current_stale_or_hand_edited_by_its_git_blob() {
        assert_eq!(
            git_blob_id(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        assert_eq!(
            git_blob_id(SECOND_BRAIN_9F3B7DBCE.as_bytes()),
            "9f3b7dbce63228d46cb1f51864e4231186f213f3"
        );
        let home = tempfile::tempdir().unwrap();
        let current = bundled_skill("second-brain").unwrap().content;
        let edited = format!("{current}\n- 내 볼트는 ~/Notes 다.\n");
        for (root, text) in [
            (".claude/skills", current),
            (".codex/skills", SECOND_BRAIN_9F3B7DBCE),
            (".zo/skills", edited.as_str()),
        ] {
            let dir = home.path().join(root).join("second-brain");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(SKILL_FILE), text).unwrap();
        }
        let copies = bundled_copies(
            "second-brain",
            home.path(),
            &[],
            &detected(&[
                ("claude", "Claude"),
                ("codex", "Codex"),
                ("zo", "ZO"),
                ("antigravity", "Antigravity"),
            ]),
        )
        .unwrap();
        let read: Vec<(&str, CopyState)> = copies
            .iter()
            .map(|copy| (copy.agent.as_str(), copy.state))
            .collect();
        assert_eq!(
            read,
            vec![
                ("claude", CopyState::Current),
                ("codex", CopyState::Stale),
                ("zo", CopyState::Edited),
                ("antigravity", CopyState::Missing),
            ]
        );
        assert_eq!(
            copies[3].path,
            home.path()
                .join(".gemini/antigravity/skills/second-brain")
                .join(SKILL_FILE)
        );
        // Reading writes nothing.
        assert!(!home.path().join(".gemini").exists());
    }

    /// An install writes only where a copy is missing or an earlier release.
    /// A copy whose text no release had was changed by somebody, and it stays
    /// to the byte, although it still names the skill and speaks of ZeroCode,
    /// which was all the old rule asked before it wrote over a file (t-55424).
    #[test]
    fn a_hand_edited_copy_is_never_overwritten() {
        let home = tempfile::tempdir().unwrap();
        let current = bundled_skill("second-brain").unwrap().content;
        let mine = format!("{current}\n- 내 볼트는 ~/Notes 다.\n");
        let edited = home.path().join(".claude/skills/second-brain");
        let stale = home.path().join(".codex/skills/second-brain");
        for (dir, text) in [(&edited, mine.as_str()), (&stale, SECOND_BRAIN_9F3B7DBCE)] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(SKILL_FILE), text).unwrap();
        }
        let outcomes = install_bundled_skill(
            "second-brain",
            home.path(),
            &[],
            &detected(&[("claude", "Claude"), ("codex", "Codex"), ("zo", "ZO")]),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(edited.join(SKILL_FILE)).unwrap(),
            mine,
            "a hand-edited copy was written over"
        );
        let states: Vec<(&str, InstallState)> = outcomes
            .iter()
            .map(|outcome| (outcome.agent.as_str(), outcome.state))
            .collect();
        assert_eq!(
            states,
            vec![
                ("claude", InstallState::Kept),
                ("codex", InstallState::Written),
                ("zo", InstallState::Written),
            ]
        );
        assert_eq!(
            std::fs::read_to_string(stale.join(SKILL_FILE)).unwrap(),
            current
        );
    }

    /// The text of every bundled skill is in the release book, so the copy
    /// this build writes reads as an earlier release, and is replaced, once a
    /// later build carries other text. A SKILL.md changed without its line
    /// fails here, and the message is the line to add.
    #[test]
    fn every_bundled_skill_text_is_a_recorded_release() {
        let unrecorded: Vec<String> = BUNDLED_SKILLS
            .iter()
            .map(|skill| (skill.name, git_blob_id(skill.content.as_bytes())))
            .filter(|(name, blob)| !released(name, blob))
            .map(|(name, blob)| format!("{name}\t{blob}"))
            .collect();
        assert!(
            unrecorded.is_empty(),
            "add these lines to crates/zerocode-core/src/skill_releases.tsv:\n{}",
            unrecorded.join("\n")
        );
        assert!(
            released("second-brain", "9f3b7dbce63228d46cb1f51864e4231186f213f3"),
            "an earlier release is no longer in the book"
        );
    }

    /// The six agents the person named — Claude Code, Codex, zo, agy (the
    /// catalog's `antigravity`), Kimi and Grok — each get EVERY bundled skill in
    /// the root they read, and each reads it from there (t-32786, t-36920).
    /// Every one has a root: none is left unwired. Kimi has no root of its own
    /// and reads the shared `.agents` home. Two skills are named outright, so a
    /// skill this build lost is a failure here and not a loop that ran over
    /// fewer skills.
    #[test]
    fn every_bundled_skill_installs_for_each_of_the_six_agents_in_the_root_it_reads() {
        for named in ["plain-report", crate::agent_emulator::MOBILE_SKILL_NAME] {
            assert!(
                bundled_skill(named).is_some(),
                "this build carries no {named} skill"
            );
        }
        let six = [
            ("claude", ".claude/skills"),
            ("codex", ".codex/skills"),
            ("zo", ".zo/skills"),
            ("antigravity", ".gemini/antigravity/skills"),
            ("kimi", ".agents/skills"),
            ("grok", ".grok/skills"),
        ];
        let rows: Vec<(String, String)> = six
            .iter()
            .map(|(agent, _)| {
                let spec = crate::agent::agent_spec(agent)
                    .unwrap_or_else(|| panic!("{agent} is not a catalog row"));
                (spec.id.to_string(), spec.name.to_string())
            })
            .collect();
        for skill in BUNDLED_SKILLS {
            let home = tempfile::tempdir().unwrap();
            let outcomes = install_bundled_skill(skill.name, home.path(), &[], &rows).unwrap();
            assert_eq!(outcomes.len(), six.len(), "{}", skill.name);
            let sources = discovery_sources(home.path(), &[]);
            for ((agent, root), outcome) in six.into_iter().zip(&outcomes) {
                assert_eq!(outcome.agent, agent);
                assert_eq!(
                    outcome.state,
                    InstallState::Written,
                    "{} for {agent}: {}",
                    skill.name,
                    outcome.detail
                );
                assert_eq!(
                    outcome.path,
                    home.path().join(root).join(skill.name).join(SKILL_FILE),
                    "{} for {agent}",
                    skill.name
                );
                assert_eq!(
                    std::fs::read_to_string(&outcome.path).unwrap(),
                    skill.content,
                    "{} for {agent}",
                    skill.name
                );
                let own = install_root(agent, &sources)
                    .unwrap_or_else(|| panic!("{agent} has no skills root"));
                let found = crate::skill::scan(own);
                let read = found
                    .iter()
                    .find(|candidate| candidate.name == skill.name)
                    .unwrap_or_else(|| {
                        panic!("{agent}: {} is not found in its own root", skill.name)
                    });
                assert!(
                    crate::skill::agent_reads_skill(agent, read),
                    "{agent} does not read the root {} was installed into",
                    skill.name
                );
            }
        }
    }

    /// The skill's two lists are the table's rows, as the table renders them:
    /// a row added to the lint's table reaches the agents that read the skill,
    /// and the limits the skill names are the lint's own constants.
    #[test]
    fn the_plain_report_skill_lists_exactly_the_rows_of_the_lint_table() {
        use crate::plain_text::{EN_SENTENCE_WORDS_MAX, KO_SENTENCE_CHARS_MAX, Language, rules};
        assert!(
            bundled_skill("plain-report").is_some(),
            "this build carries no plain-report skill"
        );
        let content = bundled_skill("plain-report").map_or("", |skill| skill.content);
        for (code, language) in [("ko", Language::Korean), ("en", Language::English)] {
            let begin = format!("<!-- plain-rules:{code}:begin -->\n");
            let end = format!("<!-- plain-rules:{code}:end -->");
            let block = content
                .split_once(&begin)
                .and_then(|(_, rest)| rest.split_once(&end))
                .map_or("", |(block, _)| block);
            // The first two lines of the block are the header and its rule.
            let listed: Vec<&str> = block.lines().skip(2).collect();
            let rendered: Vec<String> = rules()
                .iter()
                .filter(|rule| rule.language == language)
                .map(|rule| format!("| {} | {} |", rule.shown, rule.plain))
                .collect();
            assert!(!rendered.is_empty(), "the table has no {code} rows");
            assert_eq!(
                listed,
                rendered,
                "the {code} list of the skill drifted from the table; its rows should read:\n{}",
                rendered.join("\n")
            );
        }
        assert!(
            content.contains(&format!("{KO_SENTENCE_CHARS_MAX}자 안에서")),
            "the skill names a Korean limit other than the lint's"
        );
        assert!(
            content.contains(&format!("Use {EN_SENTENCE_WORDS_MAX} words or fewer")),
            "the skill names an English limit other than the lint's"
        );
    }

    #[test]
    fn a_name_this_build_does_not_carry_is_refused() {
        let home = tempfile::tempdir().unwrap();
        assert!(
            install_bundled_skill(
                "../etc/passwd",
                home.path(),
                &[],
                &detected(&[("claude", "Claude")])
            )
            .is_err()
        );
        assert!(!home.path().join(".claude").exists());
    }
}
