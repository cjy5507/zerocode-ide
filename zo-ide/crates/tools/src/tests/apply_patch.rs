//! `apply_patch`: codex-cli's envelope run through the registry.

use super::*;

/// A fresh directory for one test, so no two tests share a file.
fn patch_dir(name: &str) -> PathBuf {
    let dir = temp_path(name);
    fs::create_dir_all(&dir).expect("create test dir");
    dir
}

fn apply(ctx: &ToolContext, patch: &str) -> Result<String, ToolError> {
    execute_tool(ctx, core_types::tool_names::APPLY_PATCH, &json!({ "input": patch }))
}

#[test]
fn adds_a_file_with_the_plus_lines() {
    let dir = patch_dir("apply-patch-add");
    let file = dir.join("new.txt");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    apply(&ctx, &format!("*** Begin Patch\n*** Add File: {}\n+one\n+two\n*** End Patch", file.display()))
        .expect("add applies");

    assert_eq!(fs::read_to_string(&file).expect("read added"), "one\ntwo\n");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn updates_a_file_it_was_never_shown_through_a_read() {
    let dir = patch_dir("apply-patch-update");
    let file = dir.join("a.txt");
    fs::write(&file, "one\ntwo\nthree\n").expect("seed");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    apply(&ctx, &format!("*** Begin Patch\n*** Update File: {}\n one\n-two\n+TWO\n*** End Patch", file.display()))
        .expect("update applies without a prior read");

    assert_eq!(fs::read_to_string(&file).expect("read updated"), "one\nTWO\nthree\n");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn deletes_a_file() {
    let dir = patch_dir("apply-patch-delete");
    let file = dir.join("gone.txt");
    fs::write(&file, "bye\n").expect("seed");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    apply(&ctx, &format!("*** Begin Patch\n*** Delete File: {}\n*** End Patch", file.display()))
        .expect("delete applies");

    assert!(!file.exists(), "{} should be gone", file.display());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn moves_an_updated_file() {
    let dir = patch_dir("apply-patch-move");
    let from = dir.join("from.txt");
    let to = dir.join("sub").join("to.txt");
    fs::write(&from, "x\n").expect("seed");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    apply(
        &ctx,
        &format!(
            "*** Begin Patch\n*** Update File: {}\n*** Move to: {}\n-x\n+y\n*** End Patch",
            from.display(),
            to.display()
        ),
    )
    .expect("move applies");

    assert_eq!(
        (from.exists(), fs::read_to_string(&to).ok()),
        (false, Some("y\n".to_string()))
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_hunk_that_does_not_match_writes_nothing_at_all() {
    let dir = patch_dir("apply-patch-atomic");
    let added = dir.join("added.txt");
    let kept = dir.join("kept.txt");
    fs::write(&kept, "real\n").expect("seed");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    let error = apply(
        &ctx,
        &format!(
            "*** Begin Patch\n*** Add File: {}\n+new\n*** Update File: {}\n-imagined\n+changed\n*** End Patch",
            added.display(),
            kept.display()
        ),
    )
    .expect_err("an unmatched hunk refuses the patch");

    assert!(
        error.to_string().contains("not in the file") && !added.exists(),
        "{error}; added exists: {}",
        added.exists()
    );
    assert_eq!(fs::read_to_string(&kept).expect("read kept"), "real\n");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn refuses_to_add_a_file_that_exists() {
    let dir = patch_dir("apply-patch-exists");
    let file = dir.join("there.txt");
    fs::write(&file, "keep me\n").expect("seed");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    let error = apply(&ctx, &format!("*** Begin Patch\n*** Add File: {}\n+clobber\n*** End Patch", file.display()))
        .expect_err("add over an existing file is refused");

    assert!(error.to_string().contains("already exists"), "{error}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn reports_every_file_it_changed_under_files() {
    let dir = patch_dir("apply-patch-report");
    let file = dir.join("r.txt");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    let output = apply(&ctx, &format!("*** Begin Patch\n*** Add File: {}\n+r\n*** End Patch", file.display()))
        .expect("add applies");

    let report: serde_json::Value = serde_json::from_str(&output).expect("json report");
    assert_eq!(report["files"][0]["kind"], "add", "{output}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn an_edit_deny_rule_holds_for_a_patch_that_edits_the_same_path() {
    use runtime::permission_enforcer::PermissionEnforcer;
    use runtime::{PermissionPolicy, RuntimePermissionRuleConfig};

    let dir = patch_dir("apply-patch-deny");
    let secret = dir.join(".env");
    fs::write(&secret, "TOKEN=1\n").expect("seed");
    let rules = RuntimePermissionRuleConfig::new(Vec::new(), vec!["edit_file(*.env)".to_string()], Vec::new());
    let policy = mvp_tool_specs()
        .iter()
        .fold(PermissionPolicy::new(runtime::PermissionMode::WorkspaceWrite), |policy, spec| {
            policy.with_tool_requirement(spec.name, spec.required_permission)
        })
        .with_permission_rules(&rules);
    let mut registry = crate::tests::builtin_registry_outside_cwd_windows();
    registry.set_enforcer(PermissionEnforcer::new(policy));

    let error = registry
        .execute(
            core_types::tool_names::APPLY_PATCH,
            &json!({
                "input": format!("*** Begin Patch\n*** Update File: {}\n-TOKEN=1\n+TOKEN=2\n*** End Patch", secret.display())
            }),
        )
        .expect_err("the deny rule on edit_file covers the patch");

    assert!(error.to_string().contains("edit_file"), "{error}");
    assert_eq!(fs::read_to_string(&secret).expect("read secret"), "TOKEN=1\n");
    let _ = fs::remove_dir_all(dir);
}
