//! The artifact tab's flow (t-68458, the second draft the person passed,
//! artifact p-4958d4720ba9e103 v3): the tab opens on the project the person is
//! looking at, its first view is that project's work task by task, and every
//! word a line says comes from one table the backend's facts are read through.

use std::path::Path;

use super::support::{block_after, window_source};

fn shell_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// A window file read when the test runs, not when it compiles: a file the
/// tab has yet to grow fails its assertion instead of the build.
fn ui_source(name: &str) -> String {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    std::fs::read_to_string(ui.join(name)).unwrap_or_else(|why| panic!("ui/{name}: {why}"))
}

/// One top-level `[…]` list, from `opens` to the line that closes it — a
/// table's own rows and nothing after it (`block_after` ends at a brace).
fn list_after<'a>(source: &'a str, opens: &str) -> &'a str {
    let start = source
        .find(opens)
        .unwrap_or_else(|| panic!("`{opens}` is gone"));
    let rest = &source[start + opens.len()..];
    let end = ["\n]);", "\n];"]
        .iter()
        .filter_map(|close| rest.find(close))
        .min()
        .unwrap_or_else(|| panic!("`{opens}` never closes"));
    &rest[..end]
}

/// The words of one `[…]` list a block declares, each quoted once.
fn quoted(block: &str) -> Vec<&str> {
    block
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|word| !word.is_empty())
        .collect()
}

/// Every stage word the desk can give a task has the flow's words and mark:
/// the backend sends the desk's one reading (`stage_of`), and a word the
/// flow's table does not know would stand on a line as a raw tag.
#[test]
fn every_stage_the_desk_gives_a_task_has_the_flows_words() {
    let desk = shell_source("orchestration/desk.rs");
    let stages = quoted(list_after(&desk, "pub(crate) const STAGES: [&str; 15] = ["));
    assert_eq!(
        stages.len(),
        15,
        "the desk's stage words could not be read: {stages:?}"
    );
    let flow = ui_source("shell-artifact-flow.js");
    let table = block_after(&flow, "const ARTIFACT_FLOW_STAGES = Object.freeze({");
    for stage in &stages {
        assert!(
            table.contains(&format!("\n  {stage}: ")),
            "the desk gives a task the stage `{stage}` and the flow has no words for it:\n{table}"
        );
    }
    // The backend's flow reads the stage through the desk's one reading — no
    // second copy of the rule.
    let backend = shell_source("artifact_flow.rs");
    assert!(
        backend.contains("use crate::orchestration::desk::stage_of;")
            && backend.contains("let stage = stage_of(run, task);"),
        "the flow reads a task's stage some other way than the desk's"
    );
    assert!(
        block_after(&desk, "pub(crate) fn stage_of(").contains("staged(run, task).0"),
        "the desk's one stage reading moved"
    );
}

/// The six steps are one list, read by a line's track and by its unfolded
/// steps alike, and the groups are the three the draft passed.
#[test]
fn the_six_steps_and_three_groups_are_one_list_each() {
    let flow = ui_source("shell-artifact-flow.js");
    let steps = list_after(&flow, "const ARTIFACT_FLOW_STEPS = Object.freeze([");
    let names: Vec<&str> = steps
        .split("step: \"")
        .skip(1)
        .filter_map(|tail| tail.split('"').next())
        .collect();
    assert_eq!(
        names,
        ["task", "try", "handin", "review", "land", "ship"],
        "{steps}"
    );
    let groups = list_after(&flow, "const ARTIFACT_FLOW_GROUPS = Object.freeze([");
    let groups: Vec<&str> = groups
        .split("group: \"")
        .skip(1)
        .filter_map(|tail| tail.split('"').next())
        .collect();
    assert_eq!(groups, ["now", "next", "shipped"]);
    for reader in [
        "function artifactFlowTrack(",
        "function artifactFlowStepRows(",
    ] {
        assert!(
            block_after(&flow, reader).contains("ARTIFACT_FLOW_STEPS"),
            "`{reader}` spells the steps itself instead of reading the one list"
        );
    }
}

/// The tab opens on the flow, and on the project the person is looking at
/// until they pick another; a flow with nothing in it says so itself rather
/// than handing the person an empty gallery.
#[test]
fn the_tab_opens_on_the_flow_of_the_project_in_view() {
    let window = window_source();
    let tabs = list_after(window, "const ARTIFACT_TABS = Object.freeze([");
    assert!(
        tabs.trim_start().starts_with("{ tab: \"flow\""),
        "the flow is not the tab's first view:\n{tabs}"
    );
    let filter = block_after(window, "const artifactFilter = {");
    assert!(filter.contains("tab: ARTIFACT_FLOW_TAB"), "{filter}");
    // The flow stands on the project the person picked in the head, else on the
    // one the sidebar is looking at; the full list keeps its own filters.
    let project = block_after(window, "function artifactFlowProject(");
    assert!(
        project.contains("artifactFilter.project") && project.contains("activeProjectPath"),
        "the flow no longer stands on the project in view:\n{project}"
    );
    let fallback = block_after(window, "function artifactFallbackTab(");
    assert!(
        fallback.contains("artifactFilter.tab === ARTIFACT_FLOW_TAB"),
        "an empty flow falls back like any tab:\n{fallback}"
    );
}

/// The flow is read off the window's thread — an async command over the
/// ledger image the window already holds, git through the host boundary with
/// its answer kept by the base's commit, and the catalog's memory — the way
/// t-67102 keeps the main thread off the disk.
#[test]
fn the_flow_is_read_off_the_main_thread_and_kept_by_the_bases_commit() {
    let commands = shell_source("cmd/artifacts.rs");
    let command = block_after(&commands, "pub(crate) fn artifact_flow(");
    let head = &commands[..commands
        .find("pub(crate) fn artifact_flow(")
        .expect("the flow command")];
    assert!(
        head.trim_end().ends_with("#[tauri::command(async)]"),
        "the flow command runs on the main thread"
    );
    assert!(command.contains("crate::artifact_flow::read("), "{command}");
    let flow = shell_source("artifact_flow.rs");
    let read = block_after(&flow, "pub(crate) fn read(");
    assert!(
        read.contains("crate::orchestration::ledger_image()"),
        "the flow reads the ledger some other way than the image this window holds:\n{read}"
    );
    let git = block_after(&flow, "fn git_facts(");
    assert!(
        git.contains("Host::for_workspace(root)")
            && git.contains("FLOW.git_fresh_ms")
            && git.contains("one.oid == oid"),
        "git is asked outside the host boundary, or its answer is not kept by the base's commit:\n{git}"
    );
    assert!(
        !flow.contains("std::process::Command") && !flow.contains("std::fs::read"),
        "the flow spawns git or reads a file itself"
    );
    for road in ["main.rs", "cmd/mod.rs"] {
        assert!(
            shell_source(road).contains("artifact_flow,"),
            "`{road}` does not register the flow command"
        );
    }
}

/// git runs only in a repository this window lists (t-69070 P2). The command
/// hands the read the projects the window lists — the stored ones and the one
/// it stands in, as `project_catalog` lists them — and the read vouches the
/// asked folder before git is asked: made canonical and keyed as the ledger
/// keys a project, matched to a listed checkout's shared git directory, and
/// git asked at that listed root, never in the folder the webview named. A
/// vouch is kept only for the same ask against the same list, and only for as
/// long as git's answer stands.
#[test]
fn git_is_asked_only_at_a_repository_the_window_lists() {
    let commands = shell_source("cmd/artifacts.rs");
    let command = block_after(&commands, "pub(crate) fn artifact_flow(");
    assert!(
        command.contains("stored_projects(state.config_root())")
            && command.contains("state.active_project_context().repo_root"),
        "the flow command does not hand the read the projects the window lists:\n{command}"
    );
    let flow = shell_source("artifact_flow.rs");
    let vouch = block_after(&flow, "pub(crate) fn vouch(");
    assert!(
        vouch.contains("project_root(")
            && vouch.contains("common_of(")
            && vouch.contains(".canonicalize()"),
        "the asked folder is not matched to a listed repository:\n{vouch}"
    );
    let kept = block_after(&flow, "fn vouch_kept(");
    assert!(
        kept.contains("one.root == ask.root")
            && kept.contains("one.roots == ask.roots")
            && kept.contains("one.listed == listed")
            && kept.contains("FLOW.git_fresh_ms")
            && kept.contains("vouch(ask, listed)"),
        "a kept vouch answers for another ask, another list or a later time:\n{kept}"
    );
    let read = block_after(&flow, "pub(crate) fn read(");
    let vouched = read
        .find("vouch_kept(")
        .unwrap_or_else(|| panic!("the read does not vouch the asked folder:\n{read}"));
    let asked = read
        .find("git_facts(")
        .unwrap_or_else(|| panic!("the read no longer asks git:\n{read}"));
    assert!(
        vouched < asked
            && read.contains("git_facts(repo, now_ms)")
            && read.contains("GitAsked::Outside"),
        "git is asked before the folder is vouched, somewhere other than the listed root, \
         or the flow does not say it was not:\n{read}"
    );
}

/// The flow's buttons speak through `data-tip`, never a native `title`; its
/// words go through `t()` in all four catalogs; the module is one of the
/// window's parts; and its classes stay apart from the flow console's.
#[test]
fn the_flow_is_a_window_part_with_tips_and_its_own_classes() {
    let flow = ui_source("shell-artifact-flow.js");
    assert!(
        !flow.contains("\"title\"") && !flow.contains(".title =") && !flow.contains("title:"),
        "the flow's buttons carry a native tooltip instead of `data-tip`"
    );
    assert!(
        include_str!("../../src/ui_source.rs").contains("\"shell-artifact-flow.js\","),
        "the flow is not a window part"
    );
    assert!(include_str!("support.rs").contains("\"shell-artifact-flow.js\","));
    assert!(include_str!("../../build.rs").contains("\"shell-artifact-flow.js\","));
    assert!(
        include_str!("../../../../scripts/build-ui-dist.mjs")
            .contains("\"shell-artifact-flow.js\",")
    );
    assert!(
        include_str!("../../../../ui/index.html")
            .contains("<script defer src=\"./shell-artifact-flow.js\"></script>")
    );
    let styles = include_str!("../../../../ui/shell.css");
    let rules = styles
        .split("/* ---- 아티팩트 · 흐름 (t-68458)")
        .nth(1)
        .and_then(|rest| rest.split("/* ---- 아티팩트 · 흐름 끝 (t-68458)").next())
        .expect("the flow's rules left ui/shell.css");
    for console in [".flow-step", ".flow-chip", ".flow-detail", ".flow-row"] {
        assert!(
            !rules.contains(&format!("{console} ")) && !rules.contains(&format!("{console},")),
            "the flow's rules reuse the flow console's class `{console}`"
        );
    }
    let i18n = include_str!("../../../../ui/shell-i18n.js");
    for key in [
        "artifacts.tab.flow",
        "artifacts.flow.stageMerged",
        "artifacts.flow.groupNext",
    ] {
        assert_eq!(
            i18n.matches(&format!("\"{key}\":")).count(),
            4,
            "`{key}` must be in en/ja/zh/es"
        );
    }
}

/// A line's sentence, landing and release say where they were read — the
/// backend's sources are words the answer carries, never a guess the window
/// makes.
#[test]
fn a_line_names_where_its_facts_came_from() {
    let flow = shell_source("artifact_flow.rs");
    for word in ["Summary,", "Merge,", "Notes,", "Title,", "Git,", "Ledger,"] {
        assert!(flow.contains(word), "the flow's sources lost `{word}`");
    }
    let assemble = block_after(&flow, "pub(crate) fn assemble(");
    assert!(
        assemble.contains("flow.sources = sources_of(&flow);"),
        "the flow no longer counts where its lines' facts came from:\n{assemble}"
    );
    let window = ui_source("shell-artifact-flow.js");
    assert!(
        block_after(&window, "function artifactFlowStepRows(").contains("land.told === \"git\""),
        "a landing's moment does not say who told it"
    );
}
