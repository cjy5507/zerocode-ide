//! Regression: the foreground `tool_registry` is built with a workspace root
//! but **no `PermissionEnforcer`** (gating happens at the runtime layer, not the
//! registry — see `runtime_builder.rs` / `runtime_support.rs`). The file-tool
//! workspace-boundary relaxation must therefore read the session permission mode
//! from the shared `ToolContext` cell, not only from a registry enforcer.
//!
//! Before the fix, a danger-full-access user was wrongly denied an outside
//! `read_file`/`write_file`/`edit_file` with "escapes workspace boundary", even
//! though `bash cat` / `read_image` could reach the same path.
use crate::{GlobalToolRegistry, ToolContext};
use runtime::PermissionMode;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

fn td(name: &str) -> std::path::PathBuf {
    let u = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("zo-fgbound-{name}-{u}"))
}

/// Build a registry exactly like the foreground CLI does: a workspace root is
/// set on the context, the session permission mode is recorded on the context,
/// and **no enforcer is installed on the registry**.
fn foreground_registry(ws: &std::path::Path, mode: PermissionMode) -> GlobalToolRegistry {
    let ctx = ToolContext::new().with_workspace_root(ws.to_path_buf());
    ctx.set_permission_mode(mode);
    GlobalToolRegistry::builtin().with_context(ctx)
}

#[test]
fn fullaccess_foreground_allows_outside_read_write_edit_without_registry_enforcer() {
    let ws = td("ok-ws");
    std::fs::create_dir_all(&ws).expect("ws");
    let sib = td("ok-sib");
    std::fs::create_dir_all(&sib).expect("sib");

    for mode in [PermissionMode::DangerFullAccess, PermissionMode::Allow] {
        let reg = foreground_registry(&ws, mode);

        // read
        let rpath = sib.join(format!("read-{}.md", mode.as_str()));
        std::fs::write(&rpath, "outside readable").expect("seed read");
        let read = reg.execute("read_file", &json!({ "path": rpath.to_string_lossy() }));
        assert!(
            read.as_ref().is_ok_and(|o| o.contains("outside readable")),
            "{mode:?}: outside read must be allowed, got {read:?}"
        );

        // write (to a fresh outside path)
        let wpath = sib.join(format!("write-{}.md", mode.as_str()));
        let write = reg.execute(
            "write_file",
            &json!({ "path": wpath.to_string_lossy(), "content": "hello outside" }),
        );
        assert!(
            write.is_ok() && wpath.exists(),
            "{mode:?}: outside write must be allowed, got {write:?}"
        );

        // edit — read-before-edit 가드(CC 패리티) 때문에 기존 파일은 먼저
        // read_file로 관측해야 편집이 허용된다.
        let epath = sib.join(format!("edit-{}.md", mode.as_str()));
        std::fs::write(&epath, "alpha").expect("seed edit");
        reg.execute("read_file", &json!({ "path": epath.to_string_lossy() }))
            .expect("read the seeded file before editing it");
        let edit = reg.execute(
            "edit_file",
            &json!({ "path": epath.to_string_lossy(), "old_string": "alpha", "new_string": "omega" }),
        );
        assert!(
            edit.is_ok(),
            "{mode:?}: outside edit must be allowed, got {edit:?}"
        );
        assert_eq!(std::fs::read_to_string(&epath).unwrap(), "omega");
    }

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&sib);
}

#[test]
fn non_fullaccess_foreground_still_denies_outside_access() {
    // The security invariant the fix must NOT weaken: in workspace-write /
    // read-only mode (no registry enforcer either), outside access stays denied.
    let ws = td("deny-ws");
    std::fs::create_dir_all(&ws).expect("ws");
    let sib = td("deny-sib");
    std::fs::create_dir_all(&sib).expect("sib");

    for mode in [PermissionMode::WorkspaceWrite, PermissionMode::ReadOnly] {
        let reg = foreground_registry(&ws, mode);

        let rpath = sib.join("secret.md");
        std::fs::write(&rpath, "secret").expect("seed");
        let read = reg.execute("read_file", &json!({ "path": rpath.to_string_lossy() }));
        assert!(
            matches!(read, Err(crate::ToolError::PermissionDenied { .. })),
            "{mode:?}: outside read must stay denied, got {read:?}"
        );

        let wpath = sib.join("blocked.md");
        let write = reg.execute(
            "write_file",
            &json!({ "path": wpath.to_string_lossy(), "content": "x" }),
        );
        assert!(
            matches!(write, Err(crate::ToolError::PermissionDenied { .. })),
            "{mode:?}: outside write must stay denied, got {write:?}"
        );
        assert!(
            !wpath.exists(),
            "{mode:?}: denied write must not create the file"
        );
    }

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&sib);
}

#[test]
fn no_session_mode_falls_back_to_boundary_when_below_full_access() {
    // When neither a registry enforcer nor a session mode is present (pure
    // harness/test path) the boundary stays enforced — outside access denied.
    let ws = td("nomode-ws");
    std::fs::create_dir_all(&ws).expect("ws");
    let sib = td("nomode-sib");
    std::fs::create_dir_all(&sib).expect("sib");
    let rpath = sib.join("x.md");
    std::fs::write(&rpath, "x").expect("seed");

    // workspace root set, NO permission mode recorded, NO enforcer.
    let reg = GlobalToolRegistry::builtin()
        .with_context(ToolContext::new().with_workspace_root(ws.clone()));
    let read = reg.execute("read_file", &json!({ "path": rpath.to_string_lossy() }));
    assert!(
        matches!(read, Err(crate::ToolError::PermissionDenied { .. })),
        "no session mode + no enforcer ⇒ boundary enforced, got {read:?}"
    );

    let _ = std::fs::remove_dir_all(&ws);
    let _ = std::fs::remove_dir_all(&sib);
}
