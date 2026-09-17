//! Workflow-to-skill projection (C4).
//!
//! Projects a saved workflow-library spec into a proposed skill draft. It never
//! approves or activates skills; the existing `SkillReview` gate remains the
//! only activation path.

use std::fs;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Value};

use super::library::{self, LibraryEnvelope};
use super::spec::WorkflowSpec;
use super::{phase_source_label, workflow_mode_label};
use crate::misc_tools::{
    normalize_skill_slug, parse_skill_frontmatter_field, render_proposed_skill, write_atomic_new,
    write_atomic_replace,
};
use crate::ToolError;

#[derive(Debug, Deserialize)]
struct WorkflowSkillProjectInput {
    name: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    update: bool,
}

pub(crate) fn run(input: &Value) -> Result<String, ToolError> {
    let input: WorkflowSkillProjectInput = serde_json::from_value(input.clone())
        .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    let envelope = library::load_envelope_for_projection(input.name.trim())?;
    let cwd = std::env::current_dir()?;
    let output = project(&cwd, &envelope, &input)?;
    crate::to_pretty_json(output)
}

fn project(
    cwd: &Path,
    envelope: &LibraryEnvelope,
    input: &WorkflowSkillProjectInput,
) -> Result<Value, ToolError> {
    let library_name = non_empty("name", input.name.trim())?;
    let slug_source = input.slug.as_deref().unwrap_or(library_name);
    let slug = normalize_skill_slug(slug_source)?;
    let normalized = WorkflowSpec::from_value(&envelope.spec)?.validate()?;
    let description = input
        .description
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&normalized.description);
    let skill_dir = cwd.join(".zo").join("skills").join(&slug);
    let skill_path = skill_dir.join("SKILL.md");
    // Refuse to write through symlinked components: a symlinked
    // `.zo/skills/<slug>` (or SKILL.md) would let a WorkspaceWrite tool
    // drop the draft outside the workspace. Checked before create_dir_all for
    // the directory prefix and re-checked on the leaf file after.
    for candidate in [
        &cwd.join(".zo"),
        &cwd.join(".zo").join("skills"),
        &skill_dir,
    ] {
        reject_symlink_path(candidate)?;
    }
    fs::create_dir_all(&skill_dir)?;
    reject_symlink_path(&skill_path)?;

    let version = if skill_path.exists() {
        if !input.update {
            return Err(ToolError::InvalidInput(format!(
                "skill draft already exists at {}; pass `update: true` to update it",
                skill_path.display()
            )));
        }
        let existing = fs::read_to_string(&skill_path)?;
        parse_skill_frontmatter_field(&existing, "version")
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(1)
            .saturating_add(1)
    } else {
        1
    };

    let body = render_skill_body(library_name, &normalized);
    let contents = render_proposed_skill(&slug, description, version, &body);
    if skill_path.exists() {
        write_atomic_replace(&skill_path, &contents)?;
    } else {
        write_atomic_new(&skill_path, &contents)?;
    }

    Ok(json!({
        "name": library_name,
        "slug": slug,
        "path": skill_path.display().to_string(),
        "state": "proposed",
        "version": version,
        "workflow": normalized.name,
        "phase_count": normalized.phases.len(),
    }))
}

/// Reject a path whose own metadata says "symlink". Uses `symlink_metadata`
/// (never follows), and treats a missing path as fine — it will be created
/// beneath already-verified parents.
fn reject_symlink_path(path: &Path) -> Result<(), ToolError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(ToolError::InvalidInput(format!(
            "refusing to write through symlink at {}",
            path.display()
        ))),
        _ => Ok(()),
    }
}

fn render_skill_body(library_name: &str, workflow: &super::spec::NormalizedWorkflow) -> String {
    use std::fmt::Write as _;

    let mut body = String::new();
    body.push_str("## Procedure\n\n");
    body.push_str("Run the stored workflow through the `Workflow` tool. Pass the user's task or skill arguments as `input`:\n\n");
    body.push_str("```json\n");
    let _ = write!(
        body,
        "{{\n  \"library\": \"{}\",\n  \"input\": \"<task args>\"\n}}\n",
        escape_json_string(library_name)
    );
    body.push_str("```\n\n");
    body.push_str("Do not edit or activate this skill directly; use `SkillReview` if the proposed draft should become active.\n\n");
    body.push_str("## Workflow outline\n\n");
    let _ = writeln!(body, "- Mode: `{}`", workflow_mode_label(workflow.mode));
    for phase in &workflow.phases {
        let _ = writeln!(
            body,
            "- `{}` — `{}`",
            phase.id,
            phase_source_label(&phase.source)
        );
    }
    body
}

fn escape_json_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn non_empty<'a>(field: &str, value: &'a str) -> Result<&'a str, ToolError> {
    if value.is_empty() {
        return Err(ToolError::InvalidInput(format!("`{field}` must not be empty")));
    }
    Ok(value)
}

#[cfg(test)]
fn project_at_for_test(
    library_root: &Path,
    cwd: &Path,
    input: &WorkflowSkillProjectInput,
) -> Result<Value, ToolError> {
    let envelope = library::load_envelope_at_for_test(library_root, input.name.trim())?;
    project(cwd, &envelope, input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::misc_tools::{execute_skill, SkillInput};
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zo-workflow-skill-project-{tag}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn spec() -> Value {
        json!({
            "name": "saved-wf",
            "description": "Saved workflow description",
            "phases": [
                { "id": "plan", "prompt": "Plan {input}" },
                { "id": "review", "over": "plan", "prompt": "Review {item}" }
            ]
        })
    }

    #[test]
    fn skill_projection_writes_proposed_draft_respects_update_and_slug_validation() {
        let library_root = temp_dir("library");
        let cwd = temp_dir("cwd");
        library::save_at_for_test(&library_root, "saved-flow", spec(), false).expect("save library");

        let output = project_at_for_test(
            &library_root,
            &cwd,
            &WorkflowSkillProjectInput {
                name: "saved-flow".to_string(),
                slug: None,
                description: None,
                update: false,
            },
        )
        .expect("project skill");
        assert_eq!(output["slug"], "saved-flow");
        let path = cwd.join(".zo/skills/saved-flow/SKILL.md");
        let contents = fs::read_to_string(&path).expect("skill written");
        assert!(contents.contains("state: proposed"));
        assert!(contents.contains("description: \"Saved workflow description\""));
        assert!(contents.contains("\"library\": \"saved-flow\""));
        assert!(contents.contains("- `plan` — `single`"));
        assert!(contents.contains("- `review` — `over`"));

        let duplicate = project_at_for_test(
            &library_root,
            &cwd,
            &WorkflowSkillProjectInput {
                name: "saved-flow".to_string(),
                slug: None,
                description: None,
                update: false,
            },
        )
        .expect_err("existing skill requires update");
        assert!(duplicate.to_string().contains("update: true"));

        let updated = project_at_for_test(
            &library_root,
            &cwd,
            &WorkflowSkillProjectInput {
                name: "saved-flow".to_string(),
                slug: None,
                description: Some("Updated description".to_string()),
                update: true,
            },
        )
        .expect("update allowed");
        assert_eq!(updated["version"], 2);
        let updated_contents = fs::read_to_string(&path).expect("updated skill");
        assert!(updated_contents.contains("version: 2"));
        assert!(updated_contents.contains("description: \"Updated description\""));

        let invalid_slug = project_at_for_test(
            &library_root,
            &cwd,
            &WorkflowSkillProjectInput {
                name: "saved-flow".to_string(),
                slug: Some("BadSlug".to_string()),
                description: None,
                update: false,
            },
        )
        .expect_err("invalid slug rejected");
        assert!(invalid_slug.to_string().contains("invalid skill slug"));

        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(cwd);
    }

    #[test]
    fn projected_proposed_skill_is_blocked_by_execute_skill_gate() {
        let _guard = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let library_root = temp_dir("gate-library");
        let cwd = temp_dir("gate-cwd");
        let previous_cwd = std::env::current_dir().expect("cwd");
        library::save_at_for_test(&library_root, "saved-flow", spec(), false).expect("save library");
        project_at_for_test(
            &library_root,
            &cwd,
            &WorkflowSkillProjectInput {
                name: "saved-flow".to_string(),
                slug: Some("saved-flow".to_string()),
                description: None,
                update: false,
            },
        )
        .expect("project skill");

        std::env::set_current_dir(&cwd).expect("switch cwd");
        let err = execute_skill(SkillInput {
            skill: "saved-flow".to_string(),
            args: None,
        })
        .expect_err("proposed skill cannot execute");
        std::env::set_current_dir(previous_cwd).expect("restore cwd");
        assert!(err
            .to_string()
            .contains("skill `saved-flow` is proposed and must be approved before use"));

        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(cwd);
    }

    /// A symlinked `.zo/skills/<slug>` (or any prefix component) must be
    /// refused: following it would let this `WorkspaceWrite` tool drop `SKILL.md`
    /// outside the workspace.
    #[test]
    fn projection_refuses_symlinked_skill_path() {
        let library_root = temp_dir("library-symlink");
        let cwd = temp_dir("cwd-symlink");
        let outside = temp_dir("outside-target");
        library::save_at_for_test(&library_root, "saved-flow", spec(), false).expect("save library");

        fs::create_dir_all(cwd.join(".zo").join("skills")).expect("skills dir");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, cwd.join(".zo/skills/saved-flow"))
            .expect("symlink slug dir");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&outside, cwd.join(".zo/skills/saved-flow"))
            .expect("symlink slug dir (Windows requires Developer Mode or symlink privilege)");

        let err = project_at_for_test(
            &library_root,
            &cwd,
            &WorkflowSkillProjectInput {
                name: "saved-flow".to_string(),
                slug: None,
                description: None,
                update: false,
            },
        )
        .expect_err("symlinked slug dir refused");
        assert!(err.to_string().contains("symlink"), "unexpected: {err}");
        assert!(
            !outside.join("SKILL.md").exists(),
            "nothing may be written through the link"
        );

        let _ = fs::remove_dir_all(library_root);
        let _ = fs::remove_dir_all(cwd);
        let _ = fs::remove_dir_all(outside);
    }
}
