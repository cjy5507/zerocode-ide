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

// -- one file named by more than one operation --------------------------------
//
// The operations of a patch apply in the order they are written, each to what
// the ones before it left, and nothing is written until all of them have been
// computed.

fn seeded(dir: &std::path::Path, name: &str, text: &str) -> PathBuf {
    let file = dir.join(name);
    fs::write(&file, text).expect("seed");
    file
}

/// Two `*** Update File` sections for one file: the second is matched against
/// what the first left — its context line exists only after the first ran.
#[test]
fn a_second_section_for_one_file_works_on_what_the_first_left() {
    let dir = patch_dir("apply-patch-twice");
    let file = seeded(&dir, "a.txt", "one\ntwo\nthree\nfour\n");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    let output = apply(
        &ctx,
        &format!(
            "*** Begin Patch\n*** Update File: {0}\n one\n-two\n+TWO\n*** Update File: {0}\n TWO\n-three\n+THREE\n*** End Patch",
            file.display()
        ),
    )
    .expect("both sections apply");

    assert_eq!(fs::read_to_string(&file).expect("read"), "one\nTWO\nTHREE\nfour\n");
    let report: serde_json::Value = serde_json::from_str(&output).expect("json report");
    assert_eq!(report["files"].as_array().map(Vec::len), Some(1), "one entry a file: {output}");
    assert_eq!(
        report["summary"].as_str().map(|text| text.matches("\nM ").count()),
        Some(2),
        "a line an operation: {output}"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_file_the_patch_added_is_updated_by_a_later_section() {
    let dir = patch_dir("apply-patch-add-update");
    let file = dir.join("b.txt");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    apply(
        &ctx,
        &format!(
            "*** Begin Patch\n*** Add File: {0}\n+alpha\n+beta\n*** Update File: {0}\n alpha\n-beta\n+BETA\n*** End Patch",
            file.display()
        ),
    )
    .expect("the update finds the file the patch itself added");

    assert_eq!(fs::read_to_string(&file).expect("read"), "alpha\nBETA\n");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_file_updated_and_then_deleted_is_gone_and_reported_once_as_deleted() {
    let dir = patch_dir("apply-patch-update-delete");
    let file = seeded(&dir, "a.txt", "one\ntwo\n");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    let output = apply(
        &ctx,
        &format!(
            "*** Begin Patch\n*** Update File: {0}\n-two\n+TWO\n*** Delete File: {0}\n*** End Patch",
            file.display()
        ),
    )
    .expect("update then delete applies");

    assert!(!file.exists(), "{} should be gone", file.display());
    let report: serde_json::Value = serde_json::from_str(&output).expect("json report");
    let files = report["files"].as_array().expect("files");
    assert_eq!((files.len(), files[0]["kind"].as_str()), (1, Some("delete")), "{output}");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_file_deleted_and_then_added_holds_the_added_text() {
    let dir = patch_dir("apply-patch-delete-add");
    let file = seeded(&dir, "a.txt", "old\n");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    apply(
        &ctx,
        &format!("*** Begin Patch\n*** Delete File: {0}\n*** Add File: {0}\n+new\n*** End Patch", file.display()),
    )
    .expect("delete then add applies");

    assert_eq!(fs::read_to_string(&file).expect("read"), "new\n");
    let _ = fs::remove_dir_all(dir);
}

/// A rename inside the patch, then a section for the new name.
#[test]
fn a_moved_file_is_updated_where_it_went() {
    let dir = patch_dir("apply-patch-move-update");
    let from = seeded(&dir, "from.txt", "x\nkeep\n");
    let to = dir.join("to.txt");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    let output = apply(
        &ctx,
        &format!(
            "*** Begin Patch\n*** Update File: {}\n*** Move to: {}\n-x\n+y\n*** Update File: {}\n y\n-keep\n+kept\n*** End Patch",
            from.display(),
            to.display(),
            to.display()
        ),
    )
    .expect("move then update applies");

    assert_eq!(
        (from.exists(), fs::read_to_string(&to).ok()),
        (false, Some("y\nkept\n".to_string()))
    );
    let report: serde_json::Value = serde_json::from_str(&output).expect("json report");
    assert_eq!(report["files"].as_array().map(Vec::len), Some(1), "{output}");
    let _ = fs::remove_dir_all(dir);
}

/// A section for a file, then a rename of it that carries a second change.
#[test]
fn an_updated_file_is_moved_with_both_changes() {
    let dir = patch_dir("apply-patch-update-move");
    let from = seeded(&dir, "a.txt", "x\nkeep\n");
    let to = dir.join("b.txt");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    let output = apply(
        &ctx,
        &format!(
            "*** Begin Patch\n*** Update File: {}\n-x\n+y\n*** Update File: {}\n*** Move to: {}\n y\n-keep\n+kept\n*** End Patch",
            from.display(),
            from.display(),
            to.display()
        ),
    )
    .expect("update then move applies");

    assert_eq!(
        (from.exists(), fs::read_to_string(&to).ok()),
        (false, Some("y\nkept\n".to_string()))
    );
    let report: serde_json::Value = serde_json::from_str(&output).expect("json report");
    let files = report["files"].as_array().expect("files");
    assert_eq!(files.len(), 1, "{output}");
    assert_eq!(
        files[0]["filePath"].as_str().map(|path| path.ends_with("b.txt")),
        Some(true),
        "the entry names the file where it ended up: {output}"
    );
    let _ = fs::remove_dir_all(dir);
}

/// The whole patch is computed before the first write: a later section that
/// does not match what the earlier ones left refuses the patch, and the disk
/// keeps every byte — the file two sections named, and the file an earlier
/// operation would have added.
#[test]
fn a_later_section_that_does_not_match_leaves_every_file_as_it_was() {
    let dir = patch_dir("apply-patch-twice-atomic");
    let file = seeded(&dir, "a.txt", "one\ntwo\nthree\n");
    let added = dir.join("added.txt");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    // The second section still asks for `two`, which the first has replaced.
    let error = apply(
        &ctx,
        &format!(
            "*** Begin Patch\n*** Add File: {1}\n+new\n*** Update File: {0}\n-two\n+TWO\n*** Update File: {0}\n-two\n+2\n*** End Patch",
            file.display(),
            added.display()
        ),
    )
    .expect_err("the second section no longer matches");

    assert!(error.to_string().contains("not in the file"), "{error}");
    assert_eq!(fs::read_to_string(&file).expect("read"), "one\ntwo\nthree\n");
    assert!(!added.exists(), "nothing of the patch was written");
    let _ = fs::remove_dir_all(dir);
}

/// What an earlier operation removed is not there for a later one, and what
/// an earlier one added is not added again.
#[test]
fn an_operation_on_what_the_patch_already_removed_or_added_is_refused() {
    let dir = patch_dir("apply-patch-gone");
    let file = seeded(&dir, "a.txt", "one\n");
    let fresh = dir.join("fresh.txt");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    let gone = apply(
        &ctx,
        &format!(
            "*** Begin Patch\n*** Delete File: {0}\n*** Update File: {0}\n-one\n+ONE\n*** End Patch",
            file.display()
        ),
    )
    .expect_err("an update of a file the patch deleted");
    assert!(gone.to_string().contains("gone by this point of the patch"), "{gone}");
    assert_eq!(fs::read_to_string(&file).expect("read"), "one\n", "the delete was not carried out either");

    let twice = apply(
        &ctx,
        &format!("*** Begin Patch\n*** Add File: {0}\n+a\n*** Add File: {0}\n+b\n*** End Patch", fresh.display()),
    )
    .expect_err("the same file added twice");
    assert!(twice.to_string().contains("already exists"), "{twice}");
    assert!(!fresh.exists(), "nothing was added");

    let deleted_twice = apply(
        &ctx,
        &format!("*** Begin Patch\n*** Delete File: {0}\n*** Delete File: {0}\n*** End Patch", file.display()),
    )
    .expect_err("the same file deleted twice");
    assert!(deleted_twice.to_string().contains("not a file to delete"), "{deleted_twice}");
    assert!(file.exists(), "the first delete was not carried out either");
    let _ = fs::remove_dir_all(dir);
}

/// Lines that are not in the file are answered with the place where they
/// nearly are and the lines the file holds there, so the next patch can be
/// written from the refusal. Nothing is written.
#[test]
fn a_hunk_that_is_nearly_in_the_file_is_answered_with_the_lines_that_are() {
    let dir = patch_dir("apply-patch-closest");
    let file = seeded(&dir, "a.txt", "alpha\nbeta\ngamma\ndelta\n");
    let ctx = crate::tests::tool_context_outside_cwd_windows();

    let error = apply(
        &ctx,
        &format!(
            "*** Begin Patch\n*** Update File: {}\n alpha\n-beta2\n+BETA\n gamma\n*** End Patch",
            file.display()
        ),
    )
    .expect_err("`beta2` is not in the file")
    .to_string();

    assert!(error.contains("not in the file"), "{error}");
    assert!(
        error.contains("The closest place is lines 1-3 of the file (2 of 3 lines are there)."),
        "{error}"
    );
    assert!(error.ends_with("alpha\nbeta\ngamma"), "{error}");
    assert_eq!(fs::read_to_string(&file).expect("read"), "alpha\nbeta\ngamma\ndelta\n");
    let _ = fs::remove_dir_all(dir);
}
