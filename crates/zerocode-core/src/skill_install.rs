//! Installing the skills this product ships, from the binary itself.
//!
//! `skill.rs` reads and never writes — those are directories the person and
//! their agents own. This module is the one deliberate exception, and it is
//! narrow on purpose: it writes only the skills carried inside this binary
//! (every directory under `skills/`, as `build.rs` lists them), only into the global roots
//! each detected agent actually reads, and only where no copy stands or the copy
//! holds the text of an earlier release (`skill_releases.tsv`). A copy with any
//! other text, a person's edit or somebody else's skill, is left as it is, and
//! the outcome says `Kept`. A skill's reference files (`BundledSkill::files`)
//! are written beside its `SKILL.md`, by the same rule file by file: a release
//! whose reference file alone changed reaches a copy already installed
//! (t-85243), and a reference file with a text no release had stays, named in
//! the outcome's `detail`. An empty reference file is what a full disk left, and
//! reads as gone. Whether the copy as a whole is ours is still read from
//! `SKILL.md` alone: a hand-edited one keeps every file beside it. Each file is
//! put down whole under another name and then takes its own, so a write that
//! stops part way leaves the text that was there.
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

/// Every text each bundled skill's files have had on main, one line per text,
/// oldest first: `git log --follow --raw` over `skills/<name>/SKILL.md`, as
/// `<name>\t<git blob id>`, and over each reference file, as
/// `<name>/<file>\t<git blob id>`. A file holding one of these texts is an
/// earlier release of ours and is replaced; any other text is kept.
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

/// The skills folder of each home [`agent_homes`] names — or, for an agent
/// that reads the common store by itself while the store stands
/// ([`crate::library`]), the store alone. A copy in the agent's own folder
/// beside the store's would be the same skill listed twice, and the window's
/// store keeper moves a folder there into the store anyway.
fn skill_roots(
    agent: &str,
    sources: &[SkillSource],
    launched: &[(String, PathBuf)],
    store: Option<&Path>,
) -> Vec<PathBuf> {
    if let Some(store) = store.filter(|_| reads_the_store(agent)) {
        return vec![store.to_path_buf()];
    }
    agent_homes(agent, sources, launched)
        .into_iter()
        .map(|home| home.join("skills"))
        .collect()
}

/// Whether `agent` reads the common store whatever launched it. Claude Code
/// reads it only through the window's own home, which is that home's
/// business; zo only in the window's terminals, so its own folder keeps its
/// copy for a terminal outside the window.
fn reads_the_store(agent: &str) -> bool {
    crate::library::agents::agent(agent)
        .is_some_and(|known| known.skills.reads_the_store_everywhere())
}

/// The common store's skills folder when it has been made — asked once, as a
/// call starts: a store an install makes part way through does not move the
/// agents after it.
fn standing_store(sources: &[SkillSource]) -> Option<PathBuf> {
    let store = sources.iter().find(|source| source.id == "home-agents")?;
    store.path.is_dir().then(|| store.path.clone())
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

/// What the bytes of one installed file are, against the text this build
/// carries and the lines `book_name` has in the release book. Only an earlier
/// release is ours to replace: the rule before this one (the frontmatter names
/// the skill and the body says "ZeroCode") also matched a copy somebody had
/// edited, and wrote over it.
fn text_state(book_name: &str, carried: &str, bytes: &[u8]) -> CopyState {
    if bytes == carried.as_bytes() {
        CopyState::Current
    } else if released(book_name, &git_blob_id(bytes)) {
        CopyState::Stale
    } else {
        CopyState::Edited
    }
}

/// Whether `bytes` is a `SKILL.md` text this build or an earlier release of
/// it shipped under `name` — the window's own copy, which the common store
/// keeps one of and never asks the person about.
#[must_use]
pub fn ships_this_text(name: &str, bytes: &[u8]) -> bool {
    bundled_skill(name).is_some_and(|skill| {
        matches!(
            copy_state(skill, bytes),
            CopyState::Current | CopyState::Stale
        )
    })
}

fn copy_state(skill: &BundledSkill, bytes: &[u8]) -> CopyState {
    text_state(skill.name, skill.content, bytes)
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
    let store = standing_store(&sources);
    let mut copies: Vec<SkillCopy> = Vec::new();
    for (agent, label) in detected {
        for root in skill_roots(agent, &sources, launched, store.as_deref()) {
            let dir = root.join(skill.name);
            let path = dir.join(SKILL_FILE);
            // Two agents that read one root (claude and openclaude, or the
            // shared `.agents` home) hold one copy there, not two.
            if copies.iter().any(|copy| copy.path == path) {
                continue;
            }
            let state = match std::fs::read(&path) {
                Ok(bytes) => match copy_state(skill, &bytes) {
                    // Our text with a reference file gone or an earlier
                    // release's is an incomplete install, and an install
                    // completes it.
                    CopyState::Current if !references_to_write(skill, &dir).is_empty() => {
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
    let store = standing_store(&sources);
    let mut outcomes = Vec::new();
    for (agent, label) in detected {
        let roots = skill_roots(agent, &sources, launched, store.as_deref());
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

/// What one reference file of `skill` is in `dir`. A file that cannot be read
/// is not ours to touch. An empty one holds nothing of anybody's: a write that
/// ran out of disk leaves one, and no install would mend it if it read as a
/// person's edit.
fn reference_state(skill: &BundledSkill, dir: &Path, name: &str, carried: &str) -> CopyState {
    match std::fs::read(dir.join(name)) {
        Ok(bytes) => match text_state(&format!("{}/{name}", skill.name), carried, &bytes) {
            CopyState::Edited if bytes.is_empty() => CopyState::Missing,
            state => state,
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => CopyState::Missing,
        Err(_) => CopyState::Edited,
    }
}

/// The reference files of `skill` an install writes into `dir`: the ones gone
/// and the ones holding an earlier release's text.
fn references_to_write(
    skill: &BundledSkill,
    dir: &Path,
) -> Vec<&'static (&'static str, &'static str)> {
    skill
        .files
        .iter()
        .filter(|(name, carried)| {
            matches!(
                reference_state(skill, dir, name, carried),
                CopyState::Missing | CopyState::Stale
            )
        })
        .collect()
}

/// Where the whole new text of `path` is put before it takes that name.
fn whole_write_scratch(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

/// The file a write to `path` lands in: through any link standing there, as
/// a plain write goes, and a rename would replace the link instead.
fn through_links(path: &Path) -> PathBuf {
    let mut at = path.to_path_buf();
    // The bound is a loop of links, which `read` has already refused.
    for _ in 0..32 {
        let Ok(next) = std::fs::read_link(&at) else {
            break;
        };
        at = at
            .parent()
            .map_or_else(|| next.clone(), |dir| dir.join(&next));
    }
    at
}

/// Put `text` at `path` whole. The text goes under another name beside it and
/// then takes the name, so a disk with no room leaves the file that was there.
fn write_whole(path: &Path, text: &str) -> io::Result<()> {
    let target = through_links(path);
    let scratch = whole_write_scratch(&target);
    let written = std::fs::write(&scratch, text).and_then(|()| {
        if let Ok(standing) = std::fs::metadata(&target) {
            std::fs::set_permissions(&scratch, standing.permissions())?;
        }
        std::fs::rename(&scratch, &target)
    });
    if written.is_err() {
        // Nothing to do about a scrap that will not go, and the write's own
        // error is the one to report.
        let _ = std::fs::remove_file(&scratch);
    }
    written
}

/// Write the reference files [`references_to_write`] names.
fn write_references(skill: &BundledSkill, dir: &Path) -> io::Result<usize> {
    let to_write = references_to_write(skill, dir);
    for (name, text) in &to_write {
        write_whole(&dir.join(name), text)?;
    }
    Ok(to_write.len())
}

/// What an outcome says of the reference files an install left because their
/// text is somebody's; empty when it left none.
fn kept_references_note(skill: &BundledSkill, dir: &Path) -> String {
    let kept: Vec<&str> = skill
        .files
        .iter()
        .filter(|(name, carried)| reference_state(skill, dir, name, carried) == CopyState::Edited)
        .map(|(name, _)| *name)
        .collect();
    if kept.is_empty() {
        String::new()
    } else {
        format!("손댄 참고 파일을 그대로 두었습니다: {}", kept.join(", "))
    }
}

/// A copy whose `SKILL.md` is the one this build carries: bring its reference
/// files up, and leave the ones that are current or somebody's.
fn complete_references(
    skill: &BundledSkill,
    dir: &Path,
    path: PathBuf,
    outcome: impl Fn(PathBuf, InstallState, String) -> SkillInstallOutcome,
) -> SkillInstallOutcome {
    match write_references(skill, dir) {
        Ok(0) => {
            let kept = kept_references_note(skill, dir);
            outcome(path, InstallState::Unchanged, kept)
        }
        Ok(_) => {
            let kept = kept_references_note(skill, dir);
            outcome(path, InstallState::Written, kept)
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
        .and_then(|()| write_references(skill, dir))
        .and_then(|_| write_whole(path, skill.content));
    match written {
        Ok(()) => {
            let kept = kept_references_note(skill, dir);
            if !kept.is_empty() {
                if !detail.is_empty() {
                    detail.push_str(" · ");
                }
                detail.push_str(&kept);
            }
            outcome(path.to_path_buf(), InstallState::Written, detail)
        }
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

    /// The name of the orchestration skill's verb list, and a text an earlier
    /// release shipped for it (blob 5ddd03741).
    const VERBS: &str = "verbs.md";
    const VERBS_5DDD03741: &str =
        include_str!("../tests/fixtures/orchestration-verbs-5ddd03741.md");

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
    /// file with a text no release had is never rewritten.
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

    /// A build whose reference file alone changed reaches a copy already
    /// installed: its `SKILL.md` is this build's, its `verbs.md` the text of an
    /// earlier release, and an install brings that file up and nothing else
    /// (t-85243). Before, a reference file was written only when it was gone,
    /// so the copy kept the old text until the next change to `SKILL.md`.
    #[test]
    fn a_reference_file_of_an_earlier_release_is_replaced_though_its_skill_file_is_current() {
        let skill = bundled_skill("orchestration").unwrap();
        let (_, carried) = skill
            .files
            .iter()
            .find(|(name, _)| *name == VERBS)
            .expect("the orchestration skill carries verbs.md");
        assert_ne!(*carried, VERBS_5DDD03741, "the fixture is an earlier text");
        let home = tempfile::tempdir().unwrap();
        let agents = detected(&[("claude", "Claude")]);
        install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();
        let dir = claude_skill_dir(home.path());
        std::fs::write(dir.join(VERBS), VERBS_5DDD03741).unwrap();

        let copies = bundled_copies("orchestration", home.path(), &[], &agents).unwrap();
        assert_eq!(
            copies[0].state,
            CopyState::Stale,
            "a copy holding an earlier release's reference file reads as current"
        );
        let outcomes = install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();
        assert_eq!(outcomes[0].state, InstallState::Written);
        assert_eq!(
            &std::fs::read_to_string(dir.join(VERBS)).unwrap(),
            carried,
            "the installed verbs.md stayed at the earlier release"
        );
        let copies = bundled_copies("orchestration", home.path(), &[], &agents).unwrap();
        assert_eq!(copies[0].state, CopyState::Current);
        let again = install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();
        assert_eq!(again[0].state, InstallState::Unchanged);
    }

    /// A reference file with a text no release had is somebody's, on both
    /// roads an install takes: beside a current `SKILL.md`, and beside one of
    /// an earlier release, which is replaced while the file stays to the byte.
    /// A file of an earlier release beside it is replaced on that same pass.
    #[test]
    fn a_hand_edited_reference_file_is_kept_on_both_roads() {
        const MINE: &str = "- 내 볼트는 ~/Notes 다.\n";
        let current = bundled_skill("second-brain").unwrap().content;
        // Two reference files: `verbs.md` has lines in the release book under
        // this skill's name, and `mine.md` has none.
        let skill = BundledSkill {
            name: "orchestration",
            content: bundled_skill("orchestration").unwrap().content,
            files: &[
                (VERBS, "this build's verbs\n"),
                ("mine.md", "this build's\n"),
            ],
        };
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(skill.name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(VERBS), VERBS_5DDD03741).unwrap();
        std::fs::write(dir.join("mine.md"), MINE).unwrap();

        // The road of a current `SKILL.md`.
        std::fs::write(dir.join(SKILL_FILE), skill.content).unwrap();
        let outcome = install_one(&skill, "claude", "Claude", root.path());
        assert_eq!(outcome.state, InstallState::Written, "{}", outcome.detail);
        assert_eq!(
            std::fs::read_to_string(dir.join(VERBS)).unwrap(),
            "this build's verbs\n"
        );
        assert_eq!(std::fs::read_to_string(dir.join("mine.md")).unwrap(), MINE);
        let outcome = install_one(&skill, "claude", "Claude", root.path());
        assert_eq!(
            outcome.state,
            InstallState::Unchanged,
            "a kept file is no reason to write again"
        );

        // The road of a `SKILL.md` of an earlier release.
        let earlier = bundled_skill("second-brain").unwrap();
        let carried_files = BundledSkill {
            name: earlier.name,
            content: current,
            files: &[("mine.md", "this build's\n")],
        };
        let dir = root.path().join(earlier.name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_FILE), SECOND_BRAIN_9F3B7DBCE).unwrap();
        std::fs::write(dir.join("mine.md"), MINE).unwrap();
        let outcome = install_one(&carried_files, "claude", "Claude", root.path());
        assert_eq!(outcome.state, InstallState::Written, "{}", outcome.detail);
        assert_eq!(
            std::fs::read_to_string(dir.join(SKILL_FILE)).unwrap(),
            current
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("mine.md")).unwrap(),
            MINE,
            "a new SKILL.md took a person's reference file with it"
        );
    }

    /// A reference file a full disk left empty is no one's text: an install
    /// writes it, and the copy reads as incomplete until then (t-85243 r2).
    #[test]
    fn an_empty_reference_file_is_missing_and_an_install_writes_it() {
        let home = tempfile::tempdir().unwrap();
        let agents = detected(&[("claude", "Claude")]);
        let skill = bundled_skill("orchestration").unwrap();
        install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();
        let verbs = claude_skill_dir(home.path()).join(VERBS);
        std::fs::write(&verbs, "").unwrap();

        let copies = bundled_copies("orchestration", home.path(), &[], &agents).unwrap();
        assert_eq!(copies[0].state, CopyState::Stale);

        let outcomes = install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();
        assert_eq!(outcomes[0].state, InstallState::Written);
        let carried = skill
            .files
            .iter()
            .find(|(name, _)| *name == VERBS)
            .unwrap()
            .1;
        assert_eq!(std::fs::read_to_string(&verbs).unwrap(), carried);
    }

    /// A reference file is replaced whole: a write that cannot finish leaves
    /// the text that was there, and no scrap of its own (t-85243 r2).
    #[test]
    fn a_reference_write_that_fails_leaves_the_file_as_it_was() {
        let home = tempfile::tempdir().unwrap();
        let agents = detected(&[("claude", "Claude")]);
        install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();
        let dir = claude_skill_dir(home.path());
        let verbs = dir.join(VERBS);
        std::fs::write(&verbs, VERBS_5DDD03741).unwrap();
        // A directory where the whole new text would be put first: the same
        // failure as a disk with no room, without filling a disk.
        let blocked = whole_write_scratch(&verbs);
        std::fs::create_dir(&blocked).unwrap();

        let outcomes = install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();

        assert_eq!(
            outcomes[0].state,
            InstallState::Failed,
            "{}",
            outcomes[0].detail
        );
        assert_eq!(std::fs::read_to_string(&verbs).unwrap(), VERBS_5DDD03741);

        std::fs::remove_dir(&blocked).unwrap();
        let outcomes = install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();
        assert_eq!(outcomes[0].state, InstallState::Written);
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            !names.iter().any(|name| name.ends_with(".tmp")),
            "{names:?}"
        );
    }

    /// A link stays a link: the text goes to the file it points at, as a plain
    /// write sends it (t-85243 r2).
    #[cfg(unix)]
    #[test]
    fn a_linked_reference_file_is_written_through_its_link() {
        let home = tempfile::tempdir().unwrap();
        let agents = detected(&[("claude", "Claude")]);
        install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();
        let verbs = claude_skill_dir(home.path()).join(VERBS);
        let elsewhere = home.path().join("verbs-kept-elsewhere.md");
        std::fs::write(&elsewhere, VERBS_5DDD03741).unwrap();
        std::fs::remove_file(&verbs).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &verbs).unwrap();

        let outcomes = install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();

        assert_eq!(
            outcomes[0].state,
            InstallState::Written,
            "{}",
            outcomes[0].detail
        );
        assert!(
            std::fs::symlink_metadata(&verbs)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let carried = bundled_skill("orchestration")
            .unwrap()
            .files
            .iter()
            .find(|(name, _)| *name == VERBS)
            .unwrap()
            .1;
        assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), carried);
    }

    /// A reference file kept because somebody changed it is named in the
    /// outcome, on every road an install takes (t-85243 r2): a new `SKILL.md`
    /// may not agree with what it kept. The others are not named.
    #[test]
    fn a_kept_reference_file_is_named_in_the_outcome() {
        const MINE: &str = "- 내 볼트는 ~/Notes 다.\n";
        let skill = BundledSkill {
            name: "orchestration",
            content: bundled_skill("orchestration").unwrap().content,
            files: &[
                (VERBS, "this build's verbs\n"),
                ("mine.md", "this build's\n"),
            ],
        };
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(skill.name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_FILE), skill.content).unwrap();
        std::fs::write(dir.join("mine.md"), MINE).unwrap();

        // A current `SKILL.md`, with something else to write.
        let outcome = install_one(&skill, "claude", "Claude", root.path());
        assert_eq!(outcome.state, InstallState::Written, "{}", outcome.detail);
        assert!(outcome.detail.contains("mine.md"), "{}", outcome.detail);
        assert!(!outcome.detail.contains(VERBS), "{}", outcome.detail);

        // A current `SKILL.md`, with nothing to write.
        let outcome = install_one(&skill, "claude", "Claude", root.path());
        assert_eq!(outcome.state, InstallState::Unchanged);
        assert!(outcome.detail.contains("mine.md"), "{}", outcome.detail);

        // A `SKILL.md` of an earlier release.
        let earlier = BundledSkill {
            name: "second-brain",
            content: bundled_skill("second-brain").unwrap().content,
            files: &[("mine.md", "this build's\n")],
        };
        let dir = root.path().join(earlier.name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(SKILL_FILE), SECOND_BRAIN_9F3B7DBCE).unwrap();
        std::fs::write(dir.join("mine.md"), MINE).unwrap();
        let outcome = install_one(&earlier, "claude", "Claude", root.path());
        assert_eq!(outcome.state, InstallState::Written, "{}", outcome.detail);
        assert!(outcome.detail.contains("mine.md"), "{}", outcome.detail);

        // Nothing kept, nothing said.
        std::fs::write(
            root.path().join(earlier.name).join("mine.md"),
            "this build's\n",
        )
        .unwrap();
        let outcome = install_one(&earlier, "claude", "Claude", root.path());
        assert_eq!(outcome.state, InstallState::Unchanged);
        assert_eq!(outcome.detail, "");
    }

    /// A hand-edited `SKILL.md` keeps the whole copy: its reference files stay
    /// even when they hold an earlier release's text, because the person may
    /// be reading the two together.
    #[test]
    fn a_hand_edited_skill_file_keeps_its_reference_files_too() {
        let home = tempfile::tempdir().unwrap();
        let agents = detected(&[("claude", "Claude")]);
        install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();
        let dir = claude_skill_dir(home.path());
        std::fs::write(dir.join(SKILL_FILE), "my own guide\n").unwrap();
        std::fs::write(dir.join(VERBS), VERBS_5DDD03741).unwrap();
        let outcomes = install_bundled_skill("orchestration", home.path(), &[], &agents).unwrap();
        assert_eq!(outcomes[0].state, InstallState::Kept);
        assert_eq!(
            std::fs::read_to_string(dir.join(VERBS)).unwrap(),
            VERBS_5DDD03741
        );
    }

    /// Every bundled skill that carries reference files tells, in its
    /// `SKILL.md`, when to read each one, and the one-folder rule holds: a
    /// reference file is Markdown beside `SKILL.md`, so a skill that gains a
    /// file is installed with it without a list of names in Rust (t-81132).
    #[test]
    fn every_bundled_reference_file_is_named_in_its_skill_file() {
        let with_files: Vec<&str> = BUNDLED_SKILLS
            .iter()
            .filter(|skill| !skill.files.is_empty())
            .map(|skill| skill.name)
            .collect();
        for needed in ["orchestration", "computer-use"] {
            assert!(
                with_files.contains(&needed),
                "{needed} carries no reference files"
            );
        }
        for skill in BUNDLED_SKILLS {
            for (name, text) in skill.files {
                assert!(
                    skill.content.contains(&format!("`{name}`")),
                    "{}: SKILL.md does not tell when to read {name}",
                    skill.name
                );
                assert!(!text.trim().is_empty(), "{}: {name} is empty", skill.name);
            }
        }
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

    /// The text of every bundled skill, and of every reference file beside it,
    /// is in the release book, so the copy this build writes reads as an
    /// earlier release, and is replaced, once a later build carries other
    /// text. A SKILL.md or a reference file changed without its line fails
    /// here, and the message is the line to add.
    #[test]
    fn every_bundled_skill_text_is_a_recorded_release() {
        let unrecorded: Vec<String> = BUNDLED_SKILLS
            .iter()
            .flat_map(|skill| {
                let own = (skill.name.to_string(), skill.content);
                let references = skill
                    .files
                    .iter()
                    .map(|(name, text)| (format!("{}/{name}", skill.name), *text));
                std::iter::once(own).chain(references)
            })
            .map(|(name, text)| (name, git_blob_id(text.as_bytes())))
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
        assert!(
            released(
                "orchestration/verbs.md",
                "5ddd03741b16e730e3e2b02988d0e4d283213eb1"
            ),
            "an earlier reference file is no longer in the book"
        );
        assert_eq!(
            git_blob_id(VERBS_5DDD03741.as_bytes()),
            "5ddd03741b16e730e3e2b02988d0e4d283213eb1"
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
    /// Once the common store stands, an agent that reads it by itself gets
    /// the shipped skill there, once — not a second copy in its own folder,
    /// which the window's store keeper would only move in again. Claude Code
    /// (whose window home IS the store) and an agent that reads only its own
    /// folder keep theirs.
    #[test]
    fn a_shipped_skill_goes_into_the_store_for_every_agent_that_reads_it() {
        let home = tempfile::tempdir().unwrap();
        let store = home.path().join(".agents/skills");
        std::fs::create_dir_all(&store).unwrap();
        let rows: Vec<(String, String)> = ["claude", "codex", "grok", "kiro", "antigravity"]
            .iter()
            .map(|id| ((*id).to_string(), (*id).to_string()))
            .collect();
        let skill = BUNDLED_SKILLS[0];
        let outcomes = install_bundled_skill(skill.name, home.path(), &[], &rows).unwrap();
        let at = |agent: &str| -> Vec<PathBuf> {
            outcomes
                .iter()
                .filter(|outcome| outcome.agent == agent)
                .map(|outcome| outcome.path.clone())
                .collect()
        };
        let in_store = store.join(skill.name).join(SKILL_FILE);
        assert_eq!(at("codex"), vec![in_store.clone()]);
        assert_eq!(at("grok"), vec![in_store.clone()]);
        assert_eq!(
            at("claude"),
            vec![
                home.path()
                    .join(".claude/skills")
                    .join(skill.name)
                    .join(SKILL_FILE)
            ]
        );
        assert_eq!(
            at("kiro"),
            vec![
                home.path()
                    .join(".kiro/skills")
                    .join(skill.name)
                    .join(SKILL_FILE)
            ]
        );
        assert!(!home.path().join(".codex/skills").join(skill.name).exists());
        let copies = bundled_copies(skill.name, home.path(), &[], &rows).unwrap();
        assert_eq!(
            copies.iter().filter(|copy| copy.path == in_store).count(),
            1,
            "two agents that read the store hold one copy there"
        );
    }
}
