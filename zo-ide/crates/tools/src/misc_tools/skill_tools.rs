use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ToolError;

// --- Input/Output structs ---

#[derive(Debug, Deserialize)]
pub(crate) struct SkillInput {
    pub skill: String,
    pub args: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SkillDistillInput {
    pub slug: String,
    pub description: String,
    pub body: String,
    #[serde(default)]
    pub name: Option<String>,
    /// Re-distill (augment) an existing draft of the same slug: bump its
    /// `version` and rewrite the body, keeping `state: proposed` for re-review.
    /// Defaults to false, which refuses to overwrite an existing skill.
    #[serde(default)]
    pub update: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SkillReviewInput {
    pub slug: String,
    pub action: SkillReviewAction,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SkillReviewAction {
    Approve,
    Discard,
}

#[derive(Debug, Serialize)]
pub(crate) struct SkillOutput {
    pub(crate) skill: String,
    pub(crate) path: String,
    pub(crate) args: Option<String>,
    pub(crate) description: Option<String>,
    /// Which trusted source the skill was loaded from (`project-zo`,
    /// `global-zo`, `claude-user`/`claude-plugin`, `codex-user`/`codex-plugin`).
    /// Additive — the input schema and every prior output field are unchanged.
    pub(crate) origin: String,
    pub(crate) prompt: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct SkillDistillOutput {
    pub(crate) slug: String,
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) state: String,
    pub(crate) version: u32,
}

#[derive(Debug, Serialize)]
pub(crate) struct SkillReviewOutput {
    pub(crate) slug: String,
    pub(crate) path: String,
    pub(crate) action: String,
    pub(crate) state: String,
}

// --- Execution ---

pub(crate) fn execute_skill(input: SkillInput) -> Result<SkillOutput, ToolError> {
    let resolved = match resolve_skill_path(&input.skill) {
        Ok(resolved) => resolved,
        Err(ToolError::NotFound(_)) => {
            // Headless zo has no window installer. Its three publication skills
            // are still readable, from the very same bytes the window installs.
            let name = input.skill.trim().trim_start_matches(['/', '$']);
            if crate::artifact_tools::DESIGN_SKILLS.contains(&name) {
                if let Some(skill) = zerocode_core::skill_install::bundled_skill(name) {
                    return Ok(SkillOutput {
                        skill: input.skill, path: format!("bundled/{}/SKILL.md", skill.name),
                        args: input.args, description: parse_skill_description(skill.content),
                        origin: "bundled".into(), prompt: skill.content.into(),
                    });
                }
            }
            return Err(ToolError::NotFound(format!("unknown skill: {name}")));
        }
        Err(error) => return Err(error),
    };
    let prompt = std::fs::read_to_string(&resolved.path)?;
    if is_proposed_skill(&prompt) {
        return Err(ToolError::InvalidInput(format!(
            "skill `{}` is proposed and must be approved before use",
            input.skill
        )));
    }
    let description = parse_skill_description(&prompt);

    Ok(SkillOutput {
        skill: input.skill,
        path: resolved.path.display().to_string(),
        args: input.args,
        description,
        origin: resolved.origin,
        prompt,
    })
}

/// A distilled skill parked in `state: proposed` — a draft the review gate
/// blocks from use, waiting on a decision nothing surfaces on its own.
#[derive(Debug, Clone)]
pub struct ProposedSkill {
    pub slug: String,
    pub origin: String,
    pub path: String,
}

/// Every trusted skill candidate currently in `state: proposed`, in catalog
/// precedence order. `/refine` renders these so a `SkillDistill` draft can
/// never strand silently: the gate that keeps a proposed skill unusable is
/// only honest if something eventually shows the human the queue.
#[must_use]
pub fn stranded_proposed_skills(cwd: &Path) -> Vec<ProposedSkill> {
    runtime::SkillCatalog::discover(cwd)
        .candidates()
        .iter()
        .filter_map(|candidate| {
            let contents = std::fs::read_to_string(&candidate.skill_md).ok()?;
            is_proposed_skill(&contents).then(|| ProposedSkill {
                slug: candidate.dir_name.clone(),
                origin: candidate.source.origin_label().to_string(),
                path: candidate.skill_md.display().to_string(),
            })
        })
        .collect()
}

pub(crate) fn execute_skill_distill(
    input: &SkillDistillInput,
) -> Result<SkillDistillOutput, ToolError> {
    let slug = normalize_skill_slug(&input.slug)?;
    let description = non_empty("description", &input.description)?;
    let body = non_empty("body", &input.body)?;
    let name = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&slug)
        .to_string();

    let cwd = std::env::current_dir()?;
    let skill_dir = cwd.join(".zo").join("skills").join(&slug);
    let skill_path = skill_dir.join("SKILL.md");

    if skill_path.exists() {
        if !input.update {
            return Err(ToolError::InvalidInput(format!(
                "skill draft already exists at {}; pass `update: true` to re-distill (augment) it",
                skill_path.display()
            )));
        }
        // Re-distill: bump the version, rewrite the (model-merged) body, and keep
        // `state: proposed` so the evolved draft is re-reviewed. git tracks the
        // diff for rewind-style review.
        let existing = std::fs::read_to_string(&skill_path)?;
        let version = parse_skill_frontmatter_field(&existing, "version")
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(1)
            .saturating_add(1);
        let contents = render_proposed_skill(&name, description, version, body);
        write_atomic_replace(&skill_path, &contents)?;
        return Ok(SkillDistillOutput {
            slug,
            name,
            path: skill_path.display().to_string(),
            state: "proposed".to_string(),
            version,
        });
    }

    reject_duplicate_skill(&cwd, &slug, &name, description, body)?;

    std::fs::create_dir_all(&skill_dir)?;
    let contents = render_proposed_skill(&name, description, 1, body);
    write_atomic_new(&skill_path, &contents)?;

    Ok(SkillDistillOutput {
        slug,
        name,
        path: skill_path.display().to_string(),
        state: "proposed".to_string(),
        version: 1,
    })
}

/// Render a proposed `SKILL.md`: frontmatter (name/description/version/state)
/// plus the trimmed body. Shared by initial distillation and re-distillation so
/// the on-disk shape stays identical.
pub(crate) fn render_proposed_skill(name: &str, description: &str, version: u32, body: &str) -> String {
    format!(
        "---\nname: {}\ndescription: {}\nversion: {version}\nstate: proposed\n---\n\n{}\n",
        yaml_scalar(name),
        yaml_scalar(description),
        body.trim()
    )
}

pub(crate) fn execute_skill_review(
    input: &SkillReviewInput,
) -> Result<SkillReviewOutput, ToolError> {
    let slug = normalize_skill_slug(&input.slug)?;
    let skill_path = std::env::current_dir()?
        .join(".zo")
        .join("skills")
        .join(&slug)
        .join("SKILL.md");
    let contents = std::fs::read_to_string(&skill_path)
        .map_err(|_| ToolError::NotFound(format!("unknown proposed skill: {slug}")))?;
    if !is_proposed_skill(&contents) {
        return Err(ToolError::InvalidInput(format!(
            "skill `{slug}` is not proposed and cannot be reviewed with SkillReview"
        )));
    }

    match input.action {
        SkillReviewAction::Approve => {
            let approved = approve_skill_contents(&contents)?;
            write_atomic_replace(&skill_path, &approved)?;
            Ok(SkillReviewOutput {
                slug,
                path: skill_path.display().to_string(),
                action: "approve".to_string(),
                state: "active".to_string(),
            })
        }
        SkillReviewAction::Discard => {
            std::fs::remove_file(&skill_path)?;
            if let Some(parent) = skill_path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
            Ok(SkillReviewOutput {
                slug,
                path: skill_path.display().to_string(),
                action: "discard".to_string(),
                state: "discarded".to_string(),
            })
        }
    }
}

/// A resolved skill: its `SKILL.md` path plus the origin label of the trusted
/// source it came from.
struct ResolvedSkill {
    path: PathBuf,
    origin: String,
}

fn resolve_skill_path(skill: &str) -> Result<ResolvedSkill, ToolError> {
    let requested = skill.trim().trim_start_matches('/').trim_start_matches('$');
    if requested.is_empty() {
        return Err(ToolError::InvalidInput("skill must not be empty".into()));
    }

    // The catalog is the single source of trusted roots shared with the prompt
    // index and the per-turn router: project Zo → global Zo → enabled Claude →
    // enabled Codex, with provider roots canonicalized and containment-checked.
    // A `current_dir` failure still resolves global Zo and provider skills,
    // which do not depend on the working directory.
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let catalog = runtime::SkillCatalog::discover(&cwd);
    match catalog.resolve(requested) {
        Some(candidate) => Ok(ResolvedSkill {
            path: candidate.skill_md.clone(),
            origin: candidate.source.origin_label().to_string(),
        }),
        None => Err(ToolError::NotFound(format!("unknown skill: {requested}"))),
    }
}

pub(crate) fn normalize_skill_slug(raw: &str) -> Result<String, ToolError> {
    let slug = raw.trim().trim_start_matches('/').trim_start_matches('$');
    if slug.is_empty() {
        return Err(ToolError::InvalidInput("slug must not be empty".into()));
    }
    if slug.starts_with('.') || slug.ends_with('-') || slug.contains("--") {
        return Err(ToolError::InvalidInput(format!(
            "invalid skill slug `{slug}`"
        )));
    }
    if !slug
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(ToolError::InvalidInput(format!(
            "invalid skill slug `{slug}`"
        )));
    }
    Ok(slug.to_string())
}

fn reject_duplicate_skill(
    cwd: &Path,
    slug: &str,
    name: &str,
    description: &str,
    body: &str,
) -> Result<(), ToolError> {
    let new_tokens = tokenize_skill_text(&format!("{slug} {name} {description} {body}"));
    for path in discover_project_skill_files(cwd) {
        let Some(existing_slug) = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
        else {
            continue;
        };
        if existing_slug.eq_ignore_ascii_case(slug) {
            return duplicate_skill_error(&path);
        }

        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        if parse_skill_frontmatter_field(&contents, "name")
            .as_deref()
            .is_some_and(|existing_name| {
                existing_name.eq_ignore_ascii_case(name) || existing_name.eq_ignore_ascii_case(slug)
            })
        {
            return duplicate_skill_error(&path);
        }

        let existing_tokens = tokenize_skill_text(&format!("{existing_slug} {contents}"));
        if strong_token_overlap(&new_tokens, &existing_tokens) {
            return duplicate_skill_error(&path);
        }
    }

    Ok(())
}

fn discover_project_skill_files(cwd: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let mut cursor = Some(cwd);
    while let Some(dir) = cursor {
        roots.push(dir.join(".zo").join("skills"));
        cursor = dir.parent();
    }

    let mut files = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        files.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path().join("SKILL.md"))
                .filter(|path| path.is_file()),
        );
    }
    files.sort();
    files
}

fn duplicate_skill_error(path: &Path) -> Result<(), ToolError> {
    let existing_slug = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .unwrap_or("");
    Err(ToolError::InvalidInput(format!(
        "similar skill already exists at {path}; augment it by re-distilling into slug `{existing_slug}` with `update: true` instead of creating a duplicate",
        path = path.display()
    )))
}

fn tokenize_skill_text(text: &str) -> HashSet<String> {
    text.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter_map(|token| {
            let token = token.trim().to_ascii_lowercase();
            (token.len() >= 3 && !SKILL_DUPLICATE_STOP_WORDS.contains(&token.as_str()))
                .then_some(token)
        })
        .collect()
}

fn strong_token_overlap(left: &HashSet<String>, right: &HashSet<String>) -> bool {
    const MIN_SHARED_TOKENS: usize = 6;
    const MIN_OVERLAP_PERCENT: usize = 70;

    let smaller = left.len().min(right.len());
    if smaller < MIN_SHARED_TOKENS {
        return false;
    }
    let shared = left.intersection(right).count();
    shared >= MIN_SHARED_TOKENS && shared * 100 >= smaller * MIN_OVERLAP_PERCENT
}

const SKILL_DUPLICATE_STOP_WORDS: &[&str] = &[
    "and", "for", "from", "the", "this", "that", "with", "your", "skill", "steps",
];

fn non_empty<'a>(field: &str, value: &'a str) -> Result<&'a str, ToolError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidInput(format!(
            "{field} must not be empty"
        )));
    }
    Ok(trimmed)
}

/// Write `contents` to a uniquely-named sibling temp file of `path`, returning
/// the temp path for the caller to finalize (hard-link for create, rename for
/// replace). Shared prologue of [`write_atomic_new`] and [`write_atomic_replace`].
fn write_skill_temp(path: &Path, contents: &str) -> Result<PathBuf, ToolError> {
    let parent = path
        .parent()
        .ok_or_else(|| ToolError::InvalidInput("skill path has no parent".to_string()))?;
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temp_path = parent.join(format!(".SKILL.md.{unique}.tmp"));
    let mut temp_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)?;
    temp_file.write_all(contents.as_bytes())?;
    temp_file.sync_all()?;
    Ok(temp_path)
}

pub(crate) fn write_atomic_new(path: &Path, contents: &str) -> Result<(), ToolError> {
    let temp_path = write_skill_temp(path, contents)?;
    match std::fs::hard_link(&temp_path, path) {
        Ok(()) => {
            let _ = std::fs::remove_file(&temp_path);
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let _ = std::fs::remove_file(&temp_path);
            Err(ToolError::InvalidInput(format!(
                "skill draft already exists at {}",
                path.display()
            )))
        }
        Err(error) => {
            let _ = std::fs::remove_file(&temp_path);
            Err(error.into())
        }
    }
}

pub(crate) fn write_atomic_replace(path: &Path, contents: &str) -> Result<(), ToolError> {
    let temp_path = write_skill_temp(path, contents)?;
    match std::fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = std::fs::remove_file(&temp_path);
            Err(error.into())
        }
    }
}

fn approve_skill_contents(contents: &str) -> Result<String, ToolError> {
    if contents
        .strip_prefix("---\n")
        .or_else(|| contents.strip_prefix("---\r\n"))
        .is_none()
    {
        return Err(ToolError::InvalidInput(
            "proposed skill is missing frontmatter".to_string(),
        ));
    }

    let mut approved = Vec::new();
    let mut in_frontmatter = false;
    let mut replaced = false;
    for (index, line) in contents.lines().enumerate() {
        let trimmed = line.trim();
        if index == 0 && trimmed == "---" {
            in_frontmatter = true;
            approved.push(line.to_string());
            continue;
        }
        if in_frontmatter && trimmed == "---" {
            in_frontmatter = false;
            approved.push(line.to_string());
            continue;
        }
        if in_frontmatter
            && line
                .split_once(':')
                .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case("state"))
        {
            approved.push("state: active".to_string());
            replaced = true;
            continue;
        }
        approved.push(line.to_string());
    }

    if !replaced {
        return Err(ToolError::InvalidInput(
            "proposed skill is missing state frontmatter".to_string(),
        ));
    }
    Ok(format!("{}\n", approved.join("\n")))
}

fn yaml_scalar(value: &str) -> String {
    let escaped = value
        .trim()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], " ");
    format!("\"{escaped}\"")
}

fn parse_skill_description(contents: &str) -> Option<String> {
    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("description:") {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn is_proposed_skill(contents: &str) -> bool {
    parse_skill_frontmatter_field(contents, "state")
        .as_deref()
        .is_some_and(|state| state.eq_ignore_ascii_case("proposed"))
}

pub(crate) fn parse_skill_frontmatter_field(contents: &str, field: &str) -> Option<String> {
    let after_open = contents
        .strip_prefix("---\n")
        .or_else(|| contents.strip_prefix("---\r\n"))?;

    for line in after_open.lines() {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case(field) {
            let value = trim_skill_frontmatter_scalar(value.trim());
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }

    None
}

fn trim_skill_frontmatter_scalar(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|stripped| stripped.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|stripped| stripped.strip_suffix('\''))
        })
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::discover_project_skill_files;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn project_skill_discovery_ignores_non_zo_roots() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("zo-skill-roots-{unique}"));
        let cwd = root.join("project");
        let zo_skill = cwd.join(".zo").join("skills").join("active");
        let other_skill = cwd
            .join(".other-tool")
            .join("skills")
            .join("ignored-other");
        let codex_skill = cwd.join(".codex").join("skills").join("ignored-codex");
        for path in [&zo_skill, &other_skill, &codex_skill] {
            std::fs::create_dir_all(path).expect("create skill root");
            std::fs::write(path.join("SKILL.md"), "---\nname: test\n---\nbody\n")
                .expect("write skill");
        }

        let files = discover_project_skill_files(&cwd);
        assert!(files.contains(&zo_skill.join("SKILL.md")));
        assert!(!files.contains(&other_skill.join("SKILL.md")));
        assert!(!files.contains(&codex_skill.join("SKILL.md")));

        let _ = std::fs::remove_dir_all(root);
    }

}
