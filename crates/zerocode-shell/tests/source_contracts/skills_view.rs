use super::support::{BACKEND_PARTS, block_after};

#[test]
fn skills_runtime_is_the_only_discovery_and_install_owner() {
    let runtime = include_str!("../../src/skills_runtime.rs");
    let commands = include_str!("../../src/cmd/skills.rs");
    let fs = include_str!("../../src/cmd/fs.rs");
    let settings = include_str!("../../../../ui/shell-settings.js");
    for name in ["skills_runtime.rs", "cmd/skills.rs"] {
        assert!(BACKEND_PARTS.iter().any(|(held, _)| *held == name));
    }
    assert!(runtime.contains("skill::scan_incremental"));
    assert!(!fs.contains("skill::discover") && !fs.contains("fn list_skills"));
    for duplicate in [
        "function skillMatches",
        "function skillProviders",
        "function skillRow",
        "function skillInstallSummary",
    ] {
        assert!(
            !settings.contains(duplicate),
            "{duplicate} still lives in Settings"
        );
    }
    for command in [
        "skills_list",
        "skills_rescan",
        "skill_detail",
        "skill_install_plan",
        "skill_reveal",
    ] {
        assert!(
            commands.contains(&format!("fn {command}(")),
            "{command} missing"
        );
    }
    let plan = block_after(runtime, "pub(crate) fn plan(");
    assert!(plan.contains("skill::skill_install_command_from"));
    assert!(!plan.contains("Command::new") && !plan.contains("type_prompt_at_term"));
}

#[test]
fn skills_usage_has_two_adapters_and_a_real_hook_ingress() {
    let runtime = include_str!("../../src/skills_runtime.rs");
    let pane = include_str!("../../src/pane_runtime.rs");
    assert!(runtime.contains("trait EvidenceAdapter"));
    assert!(runtime.contains("impl EvidenceAdapter for HookRingAdapter"));
    assert!(runtime.contains("impl EvidenceAdapter for ZoIndexAdapter"));
    assert!(runtime.contains("hooks-only"));
    assert!(pane.contains("skills_runtime::note_hook"));
    assert!(
        !runtime.contains("skills-index.json"),
        "do not invent a zo usage producer"
    );
    assert!(
        runtime.contains("policy.max_usage_events")
            && runtime.contains("policy.max_evidence_bytes")
    );
}
