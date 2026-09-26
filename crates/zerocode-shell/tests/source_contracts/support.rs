use std::sync::OnceLock;

/// All of the backend shipped by this window, even after it is split into files.
pub(crate) const BACKEND_PARTS: &[(&str, &str)] = &[
    ("sftp_runtime.rs", include_str!("../../src/sftp_runtime.rs")),
    ("cmd/sftp.rs", include_str!("../../src/cmd/sftp.rs")),
    ("crash.rs", include_str!("../../src/crash.rs")),
    ("crumbs.rs", include_str!("../../src/crumbs.rs")),
    ("hang_sample.rs", include_str!("../../src/hang_sample.rs")),
    (
        "hang_watchdog.rs",
        include_str!("../../src/hang_watchdog.rs"),
    ),
    (
        "explorer_policy.rs",
        include_str!("../../src/explorer_policy.rs"),
    ),
    (
        "explorer_runtime.rs",
        include_str!("../../src/explorer_runtime.rs"),
    ),
    (
        "file_tree_ops.rs",
        include_str!("../../src/file_tree_ops.rs"),
    ),
    ("file_tree_io.rs", include_str!("../../src/file_tree_io.rs")),
    (
        "file_tree_hooks.rs",
        include_str!("../../src/file_tree_hooks.rs"),
    ),
    (
        "project_search.rs",
        include_str!("../../src/project_search.rs"),
    ),
    (
        "zo_integration_runtime.rs",
        include_str!("../../src/zo_integration_runtime.rs"),
    ),
    ("main.rs", include_str!("../../src/main.rs")),
    (
        "checks_runtime.rs",
        include_str!("../../src/checks_runtime.rs"),
    ),
    (
        "readiness_runtime.rs",
        include_str!("../../src/readiness_runtime.rs"),
    ),
    (
        "skills_runtime.rs",
        include_str!("../../src/skills_runtime.rs"),
    ),
    ("cmd/skills.rs", include_str!("../../src/cmd/skills.rs")),
    (
        "agent_tools_runtime.rs",
        include_str!("../../src/agent_tools_runtime.rs"),
    ),
    (
        "evidence_runtime.rs",
        include_str!("../../src/evidence_runtime.rs"),
    ),
    ("exit_runtime.rs", include_str!("../../src/exit_runtime.rs")),
    (
        "artifact_runtime.rs",
        include_str!("../../src/artifact_runtime.rs"),
    ),
    (
        "automation_runtime.rs",
        include_str!("../../src/automation_runtime.rs"),
    ),
    (
        "browser_guest_runtime.rs",
        include_str!("../../src/browser_guest_runtime.rs"),
    ),
    (
        "browser_runtime.rs",
        include_str!("../../src/browser_runtime.rs"),
    ),
    (
        "pane_cwd_runtime.rs",
        include_str!("../../src/pane_cwd_runtime.rs"),
    ),
    ("pane_runtime.rs", include_str!("../../src/pane_runtime.rs")),
    ("pick_runtime.rs", include_str!("../../src/pick_runtime.rs")),
    (
        "restart_nudge_runtime.rs",
        include_str!("../../src/restart_nudge_runtime.rs"),
    ),
    (
        "project_runtime.rs",
        include_str!("../../src/project_runtime.rs"),
    ),
    ("scm_runtime.rs", include_str!("../../src/scm_runtime.rs")),
    (
        "shell_runtime.rs",
        include_str!("../../src/shell_runtime.rs"),
    ),
    (
        "system_runtime.rs",
        include_str!("../../src/system_runtime.rs"),
    ),
    (
        "update_runtime.rs",
        include_str!("../../src/update_runtime.rs"),
    ),
    ("cmd/system.rs", include_str!("../../src/cmd/system.rs")),
    ("cmd/update.rs", include_str!("../../src/cmd/update.rs")),
    (
        "worktree_runtime.rs",
        include_str!("../../src/worktree_runtime.rs"),
    ),
    (
        "worktree_evidence_runtime.rs",
        include_str!("../../src/worktree_evidence_runtime.rs"),
    ),
    (
        "usage_runtime.rs",
        include_str!("../../src/usage_runtime.rs"),
    ),
    (
        "terminal_prefs_runtime.rs",
        include_str!("../../src/terminal_prefs_runtime.rs"),
    ),
    (
        "window_runtime.rs",
        include_str!("../../src/window_runtime.rs"),
    ),
    ("wire_runtime.rs", include_str!("../../src/wire_runtime.rs")),
    (
        "settings_runtime.rs",
        include_str!("../../src/settings_runtime.rs"),
    ),
    ("cmd/mod.rs", include_str!("../../src/cmd/mod.rs")),
    (
        "zerocode-shell-cmd-jira/src/lib.rs",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zerocode-shell-cmd-jira/src/lib.rs"
        )),
    ),
    (
        "zerocode-shell-cmd-linear/src/lib.rs",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../zerocode-shell-cmd-linear/src/lib.rs"
        )),
    ),
    ("cmd/remote.rs", include_str!("../../src/cmd/remote.rs")),
    ("cmd/browser.rs", include_str!("../../src/cmd/browser.rs")),
    ("cmd/usage.rs", include_str!("../../src/cmd/usage.rs")),
    (
        "cmd/appearance.rs",
        include_str!("../../src/cmd/appearance.rs"),
    ),
    ("cmd/fs.rs", include_str!("../../src/cmd/fs.rs")),
    (
        "cmd/onboarding.rs",
        include_str!("../../src/cmd/onboarding.rs"),
    ),
    (
        "cmd/agent_launch.rs",
        include_str!("../../src/cmd/agent_launch.rs"),
    ),
    (
        "cmd/api_routers.rs",
        include_str!("../../src/cmd/api_routers.rs"),
    ),
    ("cmd/typesafe.rs", include_str!("../../src/cmd/typesafe.rs")),
    (
        "cmd/type_value.rs",
        include_str!("../../src/cmd/type_value.rs"),
    ),
    (
        "cmd/worker_room.rs",
        include_str!("../../src/cmd/worker_room.rs"),
    ),
    (
        "cmd/artifacts.rs",
        include_str!("../../src/cmd/artifacts.rs"),
    ),
    ("cmd/board.rs", include_str!("../../src/cmd/board.rs")),
    ("cmd/console.rs", include_str!("../../src/cmd/console.rs")),
    ("cmd/wire.rs", include_str!("../../src/cmd/wire.rs")),
    ("cmd/review.rs", include_str!("../../src/cmd/review.rs")),
    (
        "cmd/github_pr.rs",
        include_str!("../../src/cmd/github_pr.rs"),
    ),
    (
        "cmd/repo_policy.rs",
        include_str!("../../src/cmd/repo_policy.rs"),
    ),
    (
        "cmd/integration_prefs.rs",
        include_str!("../../src/cmd/integration_prefs.rs"),
    ),
    (
        "cmd/workspace.rs",
        include_str!("../../src/cmd/workspace.rs"),
    ),
    ("cmd/project.rs", include_str!("../../src/cmd/project.rs")),
    ("cmd/terminal.rs", include_str!("../../src/cmd/terminal.rs")),
    ("cmd/scm.rs", include_str!("../../src/cmd/scm.rs")),
    (
        "cmd/second_brain.rs",
        include_str!("../../src/cmd/second_brain.rs"),
    ),
    (
        "cmd/supply_chain.rs",
        include_str!("../../src/cmd/supply_chain.rs"),
    ),
    ("cmd/worktree.rs", include_str!("../../src/cmd/worktree.rs")),
    ("cmd/settings.rs", include_str!("../../src/cmd/settings.rs")),
    ("cmd/session.rs", include_str!("../../src/cmd/session.rs")),
];

pub(crate) fn shipped_backend() -> &'static str {
    static JOINED: OnceLock<String> = OnceLock::new();
    JOINED
        .get_or_init(|| {
            BACKEND_PARTS
                .iter()
                .map(|(_, text)| {
                    text.split_once("#[cfg(test)]")
                        .map_or(*text, |(before, _)| before)
                })
                .collect::<Vec<_>>()
                .join("\n")
                .replace("pub(super) ", "")
        })
        .as_str()
}

/// One CSS rule, from its selector to the line that closes it.
///
/// [`block_after`] stops at a top-level `}` *or* `});`, which is right for
/// this file's JavaScript and one line too generous for a stylesheet
/// nested inside a media query. A rule ends at the first line that is
/// exactly `}`, at any indent.
pub(crate) fn block_after_css<'a>(source: &'a str, opens: &str) -> &'a str {
    // Matched at the start of a line, or `.panel-search {` would find
    // itself inside the more specific `.field .panel-search {` further
    // down and answer a question about the wrong rule.
    let start = source
        .find(&format!("\n{opens}"))
        .map(|at| at + 1)
        .unwrap_or_else(|| panic!("`{opens}` is gone from ui/shell.css"));
    let rest = &source[start..];
    let end = rest
        .match_indices('\n')
        .find(|(at, _)| rest[at + 1..].lines().next().unwrap_or("").trim() == "}")
        .map_or(rest.len(), |(at, _)| at);
    &rest[..end]
}

/// The source from `opens` up to the top-level line that closes it.
///
/// Enough to ask a question about one block without a parser: every caller
/// below names something declared at the top level of `ui/shell.js`, so
/// the first line that is exactly `}` or `});` ends it.
pub(crate) fn block_after<'a>(source: &'a str, opens: &str) -> &'a str {
    let start = source
        .find(opens)
        .unwrap_or_else(|| panic!("`{opens}` is gone from ui/shell.js"));
    let line_start = source[..start].rfind('\n').map_or(0, |at| at + 1);
    let indent = &source[line_start..start]
        [..source[line_start..start].len() - source[line_start..start].trim_start().len()];
    let rest = &source[start..];
    let end = rest
        .match_indices('\n')
        .find(|(at, _)| {
            let line = rest[at + 1..].lines().next().unwrap_or("");
            // `};` closes a top-level `const X = {…}` the same way the
            // other two close a function and a call.
            line == format!("{indent}}}")
                || line == format!("{indent}}});")
                || line == format!("{indent}}};")
        })
        .map_or(rest.len(), |(at, _)| at);
    &rest[..end]
}

/// One row of the window's usage-provider table, from its id to the line
/// that closes the row.
///
/// Per-row rather than per-table, because the point of that table is that
/// each vendor's rules sit on its OWN row: an assertion against the whole
/// table would pass while a rule sat on the wrong one.
pub(crate) fn provider_row<'a>(window: &'a str, id: &str) -> &'a str {
    let table = block_after(window, "const USAGE_STATS_PROVIDERS = [");
    let start = table
        .find(&format!("id: \"{id}\","))
        .unwrap_or_else(|| panic!("`{id}` left the usage provider table"));
    let rest = &table[start..];
    let end = rest.find("\n  },").unwrap_or(rest.len());
    &rest[..end]
}

/// The settings coordinator plus the small domain applicators it calls.
///
/// Source-contract tests care that a canonical snapshot reaches the live
/// window, not whether that work happens inline in the coordinator. Keep
/// that architectural detail out of every individual setting assertion.
pub(crate) fn settings_snapshot_application_source(window: &str) -> String {
    let mut source = block_after(window, "function applySettingsSnapshot(snapshot) {").to_owned();
    for opens in [
        "function applyAppearanceSettingsSnapshot(snapshot, first) {",
        "function applyEditingSettingsSnapshot(snapshot, first) {",
        "function applyTerminalSettingsSnapshot(snapshot, first) {",
        "function applyAgentSettingsSnapshot(snapshot) {",
        "function applyNavigationSettingsSnapshot(snapshot) {",
        "function applyWorkflowSettingsSnapshot(snapshot, first) {",
    ] {
        source.push('\n');
        source.push_str(block_after(window, opens));
    }
    source
}

/// One setting's complete canonical wire: the command patches the shared
/// document, returns its authoritative snapshot, and the renderer applies
/// that same snapshot both after a write and on the first frame.
///
/// Keeping this assertion in one place is intentional. Before the
/// repository migration, every setting test pinned a different direct
/// `invoke`/`report.field` spelling; those tests all went stale together
/// when the duplicate per-setting state was removed.
#[allow(clippy::too_many_arguments)] // Each argument names one independent wire assertion.
pub(crate) fn assert_canonical_setting_round_trip(
    backend: &str,
    window: &str,
    command: &str,
    backend_key: &str,
    document_write: &str,
    setter_opens: &str,
    wire_key: &str,
    snapshot_apply: &str,
) {
    let (shipped, _) = backend.split_once("#[cfg(test)]").unwrap_or((backend, ""));
    let command_body = block_after(shipped, &format!("fn {command}("));
    assert!(
        command_body.contains("Result<SettingsSnapshot, String>")
            && command_body.contains("commit_setting(")
            && command_body.contains(backend_key)
            && command_body.contains(document_write),
        "`{command}` no longer patches `{wire_key}` through the canonical settings document:\n{command_body}"
    );

    let setter = block_after(window, setter_opens);
    let exact_key = format!("\"{wire_key}\"");
    let field_key = format!("`{wire_key}.${{");
    assert!(
        setter.contains("commitSetting(")
            && (setter.contains(&exact_key) || setter.contains(&field_key))
            && setter.contains(&format!("\"{command}\"")),
        "the renderer no longer sends `{wire_key}` through the shared settings writer:\n{setter}"
    );

    let applying = settings_snapshot_application_source(window);
    assert!(
        applying.contains(&format!("hasSetting(snapshot, \"{wire_key}\")"))
            && applying.contains(snapshot_apply),
        "an authoritative snapshot no longer applies `{wire_key}` to the live window:\n{applying}"
    );

    let boot = block_after(window, "async function boot() {");
    assert!(
        boot.contains("applySettingsSnapshot(report);"),
        "the first frame bypasses the canonical settings snapshot:\n{boot}"
    );
    let transport = block_after(window, "async function commitSetting(");
    assert!(
        transport.contains("const snapshot = await invoke(command, args);")
            && transport.contains("const snapshot = await fetchSettingsSnapshot();")
            && transport.contains("const mutationEpoch = ++settingsMutationEpoch;")
            && transport.contains("settingsMutationsInFlight += 1;")
            && transport.contains("settingsMutationsInFlight -= 1;")
            && transport.contains("drainDeferredSettingsRefresh();")
            && transport
                .matches("applySettingsMutationSnapshot(key, generation, mutationEpoch, snapshot);")
                .count()
                >= 2,
        "settings writes no longer apply the authoritative answer or recover it after a lost/refused acknowledgement:\n{transport}"
    );
    let mutation_apply = block_after(window, "function applySettingsMutationSnapshot(");
    assert!(
        mutation_apply.contains("settingsMutationGenerations.get(key) === generation")
            && mutation_apply.contains("settingsMutationEpoch === mutationEpoch")
            && mutation_apply.contains("settingsMutationsInFlight === 1")
            && mutation_apply.contains("return applySettingsSnapshot(snapshot);")
            && mutation_apply.contains("deferSettingsRefresh();"),
        "a mutation full snapshot can overwrite overlapping optimistic state or be dropped without convergence:\n{mutation_apply}"
    );
    let refresh = block_after(window, "async function refreshSettingsSnapshot(");
    assert!(
        refresh.contains("const mutationEpoch = settingsMutationEpoch;")
            && refresh.contains("const safeAtStart = settingsMutationsInFlight === 0;")
            && refresh.contains("generation === settingsReadGeneration")
            && refresh.contains("settingsMutationsInFlight === 0")
            && refresh.contains("settingsMutationEpoch === mutationEpoch")
            && refresh.contains("deferSettingsRefresh();")
            && window.contains("settingsRefreshDeferred = true;")
            && window.contains("void refreshSettingsSnapshot();"),
        "a generic settings read can repaint across optimistic work or be dropped without convergence:\n{refresh}"
    );
    assert!(
        shipped.contains("#[serde(flatten)]\n    settings: SettingsSnapshot")
            && shipped.contains("#[serde(flatten)]\n    document: SettingsDocument"),
        "boot and later reads no longer carry the same flattened settings document"
    );
}

/// The same, for a function declared INSIDE another one.
///
/// `block_after` ends at a line that is exactly `}`, which a nested
/// function never has — its body would run to the end of its parent. That
/// is harmless for "does this block contain X" and actively wrong for
/// "does this block NOT contain X", which is the shape of every assertion
/// about a paint path that must have stopped doing something. The closing
/// line of a function declared one level in is `  }`.
pub(crate) fn nested_block<'a>(source: &'a str, opens: &str) -> &'a str {
    let start = source
        .find(opens)
        .unwrap_or_else(|| panic!("`{opens}` is gone from ui/shell.js"));
    let rest = &source[start..];
    let end = rest
        .match_indices('\n')
        .find(|(at, _)| rest[at + 1..].lines().next().unwrap_or("") == "  }")
        .map_or(rest.len(), |(at, _)| at);
    &rest[..end]
}

/// The window between two markup tags, so a structure gate can name a
/// region instead of the whole file. `block_after` cuts Rust and CSS
/// braces; markup nests by tag and needs its own answer.
pub(crate) fn markup_between<'a>(source: &'a str, open: &str, close: &str) -> &'a str {
    let from = source
        .find(open)
        .unwrap_or_else(|| panic!("no {open} in the markup"));
    let rest = &source[from..];
    let to = rest
        .find(close)
        .unwrap_or_else(|| panic!("no {close} after {open}"));
    &rest[..to]
}

/// The window with its prose removed, so a gate reads code and not
/// comments. `strip_literals` cannot be used here — it takes the strings
/// too, and the strings are the subject.
///
/// Written for **JavaScript**, which is what almost every gate here reads. A
/// `'` is left alone: in JS it opens a string, and consuming those would take
/// the subject of half these gates with it. For Rust sources use
/// [`strip_rust_comments`], which has the opposite problem to solve.
pub(crate) fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = rest.find(['/', '"']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if let Some(body) = tail.strip_prefix("/*") {
            let end = body.find("*/").map_or(body.len(), |to| to + 2);
            rest = &body[end..];
        } else if let Some(body) = tail.strip_prefix("//") {
            let end = body.find('\n').unwrap_or(body.len());
            rest = &body[end..];
        } else if tail.starts_with('"') {
            // Keep the string, and skip past it whole so a `//` inside one
            // does not read as the start of a comment.
            let mut chars = tail.char_indices().skip(1);
            let mut end = tail.len();
            while let Some((i, glyph)) = chars.next() {
                if glyph == '\\' {
                    chars.next();
                } else if glyph == '"' {
                    end = i + 1;
                    break;
                }
            }
            out.push_str(&tail[..end]);
            rest = &tail[end..];
        } else {
            out.push('/');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// A **Rust** source with its prose removed.
///
/// Differs from [`strip_comments`] over one character literal: `'"'`. Read as
/// an opening string quote it swallows every byte up to the next `"`, which on
/// a Rust file is usually several doc comments — and a gate whose comment
/// stripping quietly stopped working reports its subject as clean. Only the
/// unambiguous char-literal shapes are consumed (`'x'`, `'\x'`), so a lifetime
/// like `&'a str` is still left alone.
pub(crate) fn strip_rust_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = rest.find(['/', '"', '\'']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if let Some(body) = tail.strip_prefix("/*") {
            let end = body.find("*/").map_or(body.len(), |to| to + 2);
            rest = &body[end..];
        } else if let Some(body) = tail.strip_prefix("//") {
            let end = body.find('\n').unwrap_or(body.len());
            rest = &body[end..];
        } else if let Some(after_quote) = tail.strip_prefix('\'') {
            let mut glyphs = tail.char_indices().skip(1);
            let literal = match glyphs.next() {
                // `'\n'`, `'\\'` — the escape and then the closing quote.
                Some((_, '\\')) => glyphs.nth(1).filter(|(_, ch)| *ch == '\'').map(|(i, _)| i),
                // `'x'` — one character and the closing quote.
                Some(_) => glyphs.next().filter(|(_, ch)| *ch == '\'').map(|(i, _)| i),
                None => None,
            };
            match literal {
                Some(end) => {
                    out.push_str(&tail[..=end]);
                    rest = &tail[end + 1..];
                }
                // Not a char literal — a lifetime, or a loop label.
                None => {
                    out.push('\'');
                    rest = after_quote;
                }
            }
        } else if tail.starts_with('"') {
            let mut glyphs = tail.char_indices().skip(1);
            let mut end = tail.len();
            while let Some((at, glyph)) = glyphs.next() {
                if glyph == '\\' {
                    glyphs.next();
                } else if glyph == '"' {
                    end = at + 1;
                    break;
                }
            }
            out.push_str(&tail[..end]);
            rest = &tail[end..];
        } else {
            out.push('/');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

pub(crate) const WINDOW_PARTS: &[(&str, &str)] = &[
    (
        "shell-boot.js",
        include_str!("../../../../ui/shell-boot.js"),
    ),
    (
        "shell-i18n.js",
        include_str!("../../../../ui/shell-i18n.js"),
    ),
    (
        "shell-term-selection.js",
        include_str!("../../../../ui/shell-term-selection.js"),
    ),
    (
        "shell-term.js",
        include_str!("../../../../ui/shell-term.js"),
    ),
    (
        "shell-status.js",
        include_str!("../../../../ui/shell-status.js"),
    ),
    ("shell-doc.js", include_str!("../../../../ui/shell-doc.js")),
    (
        "shell-knowledge-3d.js",
        include_str!("../../../../ui/shell-knowledge-3d.js"),
    ),
    (
        "shell-knowledge.js",
        include_str!("../../../../ui/shell-knowledge.js"),
    ),
    (
        "shell-knowledge-supply.js",
        include_str!("../../../../ui/shell-knowledge-supply.js"),
    ),
    ("shell-scm.js", include_str!("../../../../ui/shell-scm.js")),
    (
        "shell-browser.js",
        include_str!("../../../../ui/shell-browser.js"),
    ),
    (
        "shell-workspace.js",
        include_str!("../../../../ui/shell-workspace.js"),
    ),
    (
        "shell-input.js",
        include_str!("../../../../ui/shell-input.js"),
    ),
    (
        "shell-explorer-search.js",
        include_str!("../../../../ui/shell-explorer-search.js"),
    ),
    (
        "shell-explorer-tree.js",
        include_str!("../../../../ui/shell-explorer-tree.js"),
    ),
    (
        "shell-path-browser.js",
        include_str!("../../../../ui/shell-path-browser.js"),
    ),
    (
        "shell-attach.js",
        include_str!("../../../../ui/shell-attach.js"),
    ),
    (
        "shell-composer.js",
        include_str!("../../../../ui/shell-composer.js"),
    ),
    (
        "shell-update.js",
        include_str!("../../../../ui/shell-update.js"),
    ),
    (
        "shell-computer.js",
        include_str!("../../../../ui/shell-computer.js"),
    ),
    (
        "shell-settings.js",
        include_str!("../../../../ui/shell-settings.js"),
    ),
    ("shell-jev.js", include_str!("../../../../ui/shell-jev.js")),
    (
        "shell-flow.js",
        include_str!("../../../../ui/shell-flow.js"),
    ),
    (
        "shell-remote.js",
        include_str!("../../../../ui/shell-remote.js"),
    ),
    (
        "shell-sftp.js",
        include_str!("../../../../ui/shell-sftp.js"),
    ),
    (
        "shell-conversation-view.js",
        include_str!("../../../../ui/shell-conversation-view.js"),
    ),
    (
        "shell-board.js",
        include_str!("../../../../ui/shell-board.js"),
    ),
    (
        "shell-board-live.js",
        include_str!("../../../../ui/shell-board-live.js"),
    ),
    (
        "shell-board-orbit.js",
        include_str!("../../../../ui/shell-board-orbit.js"),
    ),
    ("shell.js", include_str!("../../../../ui/shell.js")),
];

/// 파트를 순서대로 이어 붙인 한 건초더미. 한 번 만들고 계속 빌려준다.
///
/// 이어 붙이는 자리에 아무것도 끼우지 않는다 — 각 파트가 이미 개행으로
/// 끝나므로, 자르기가 순수한 이동인 한 이 문자열은 자르기 전 `ui/shell.js`와
/// **바이트까지 같다.** 줄을 세는 시험 아홉 개가 그 위에 서 있다.
pub(crate) fn window_source() -> &'static str {
    static JOINED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    JOINED
        .get_or_init(|| WINDOW_PARTS.iter().map(|(_, part)| *part).collect())
        .as_str()
}
