use zerocode_core::skill::{self, Policy, SourceKind};

fn fixture() -> (tempfile::TempDir, skill::SkillSource) {
    let temp = tempfile::tempdir().unwrap();
    let source = skill::SkillSource {
        id: "fixture".into(),
        label: "Fixture".into(),
        path: temp.path().into(),
        kind: SourceKind::Home,
        providers: vec!["codex".into()],
        owner: Some("codex".into()),
    };
    (temp, source)
}
fn put(root: &std::path::Path, dir: &str, version: &str, body: &str) {
    std::fs::create_dir_all(root.join(dir)).unwrap();
    std::fs::write(
        root.join(dir).join("SKILL.md"),
        format!("---\nname: example\nversion: {version}\n---\n{body}"),
    )
    .unwrap();
}
#[test]
fn families_compare_numeric_versions_and_hash_the_whole_body() {
    let (_temp, source) = fixture();
    put(&source.path, "a", "1.9.0", "One");
    put(&source.path, "b", "1.10.0", "Two");
    let families = skill::families(skill::scan(&source));
    assert_eq!(families.len(), 1);
    assert!(families[0].conflict);
    assert_eq!(families[0].latest_version.as_deref(), Some("1.10.0"));
    assert_ne!(
        families[0].variants[0].digest,
        families[0].variants[1].digest
    );
}
#[test]
fn policy_overlay_is_bounded_and_bundle_names_come_from_the_directory() {
    let policy = Policy::overlay(
        &serde_json::json!({"max_skills_per_root": 3, "max_depth": 0, "unknown": 1}),
    );
    assert_eq!(policy.max_skills_per_root, 3);
    assert!(policy.max_depth > 0);
    let expected: std::collections::BTreeSet<_> =
        std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../skills"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().join("SKILL.md").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
    assert_eq!(
        policy
            .required_names()
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        expected
    );
}
#[test]
fn bounded_scan_detects_nested_edits_without_rereading_unchanged_bodies() {
    let (_temp, source) = fixture();
    put(&source.path, "nested/a", "1.0.0", "One");
    let first = skill::scan_incremental(&source, &Policy::default(), None);
    let warm = skill::scan_incremental(&source, &Policy::default(), Some(first));
    assert_eq!(warm.reads, 0);
    put(&source.path, "nested/a", "2.0.0", "A changed body");
    let edited = skill::scan_incremental(&source, &Policy::default(), Some(warm));
    assert_eq!(edited.skills[0].version.as_deref(), Some("2.0.0"));
    let policy = Policy::overlay(&serde_json::json!({"max_skills_per_root": 1}));
    put(&source.path, "b", "1.0.0", "Two");
    let capped = skill::scan_incremental(&source, &policy, None);
    assert_eq!(capped.skills.len(), 1);
    assert!(capped.capped);
}
#[test]
fn bundle_output_parser_ignores_ansi_descriptions_and_failure_lines() {
    let parsed = skill::parse_bundle_output(
        "\u{1b}[32m│\u{1b}[0m Available Skills\n│\n│  web-design\n│    Build beautiful pages.\n│\n│  rust-review\n│    Review Rust code.\n",
        &Policy::default(),
    );
    assert_eq!(parsed.names, ["web-design", "rust-review"]);
    assert_eq!(
        skill::parse_bundle_output("npm error code E404\nmore context", &Policy::default())
            .error
            .as_deref(),
        Some("npm error code E404")
    );
}
#[test]
fn install_builder_rejects_shell_metacharacters_and_keeps_one_flag_per_name() {
    assert!(skill::skill_install_command(&["x; touch /tmp/no"], &["codex"]).is_none());
    assert!(skill::skill_update_command("x\nwhoami").is_none());
    let command = skill::skill_install_command(&["a", "b"], &["codex"]).unwrap();
    assert!(command.contains("--skill a --skill b --agent codex --global"));
    assert!(skill::bundle_list_command("https://example.com/x;evil").is_none());
}

/// Release-mode fixture used by the task report. Warm reads and retained
/// summary size are deterministic gates; latency is printed for paired runs.
#[test]
fn skills_scan_measurement() {
    let (_temp, source) = fixture();
    for i in 0..300 {
        let dir = source.path.join(format!("skill-{i:03}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: skill-{i:03}\nversion: 1.2.3\ndescription: A focused workflow with repeatable checks.\n---\n{}", "Instructions.\n".repeat(100))).unwrap();
    }
    let begin = std::time::Instant::now();
    let cold = skill::scan_incremental(&source, &Policy::default(), None);
    let cold_us = begin.elapsed().as_micros();
    let summary_bytes = serde_json::to_vec(&cold.skills).unwrap().len();
    assert_eq!(cold.skills.len(), 300);
    assert_eq!(cold.reads, 300);
    let begin = std::time::Instant::now();
    let warm = skill::scan_incremental(&source, &Policy::default(), Some(cold));
    let warm_us = begin.elapsed().as_micros();
    assert_eq!(warm.reads, 0);
    assert!(summary_bytes < Policy::default().max_evidence_bytes);
    println!(
        "SKILLS_SCAN cold_us={cold_us} warm_us={warm_us} summary_bytes={summary_bytes} skills={} warm_reads={}",
        warm.skills.len(),
        warm.reads
    );
}

#[test]
fn versions_order_prerelease_numbers_and_leave_unknown_versions_unordered() {
    use std::cmp::Ordering;
    assert_eq!(
        skill::compare_versions("1.2.0-rc.2", "1.2.0-rc.10"),
        Some(Ordering::Less)
    );
    assert_eq!(
        skill::compare_versions("v1.2.0", "1.2.0-rc.10"),
        Some(Ordering::Greater)
    );
    assert_eq!(
        skill::compare_versions("1.2.0+build.5", "1.2.0+build.1"),
        Some(Ordering::Equal)
    );
    assert_eq!(skill::compare_versions("latest", "1.0.0"), None);
}

#[test]
fn installed_agent_coverage_distinguishes_private_and_shared_roots() {
    let (_temp, mut source) = fixture();
    source.id = "home-codex".into();
    put(&source.path, "review", "1.0.0", "Review");
    let mut variant = skill::scan(&source).remove(0);
    assert!(skill::agent_reads_skill("codex", &variant));
    assert!(!skill::agent_reads_skill("claude", &variant));
    variant.source_id = "repo-agents-fixture".into();
    variant.source_kind = SourceKind::Repo;
    variant.providers = vec!["agent-skills".into()];
    assert!(skill::agent_reads_skill("codex", &variant));
    assert!(skill::agent_reads_skill("claude", &variant));
}

// Captured from an actual read-only skills@1.5.23 --list invocation.
// Reproduction and source references: fixtures/skills-1.5.23-list.md.
#[test]
fn skills_1_5_23_list_handles_clack_padding_plugin_headers_and_ascii_guides() {
    let output = include_str!("fixtures/skills-1.5.23-list.txt");
    assert_eq!(
        skill::parse_bundle_output(output, &Policy::default()).names,
        ["web-design", "rust-review"]
    );
    let ascii = output.replace('│', "|").replace('◇', "o").replace('└', "—");
    assert_eq!(
        skill::parse_bundle_output(&ascii, &Policy::default()).names,
        ["web-design", "rust-review"]
    );
    let plain = "Available Skills\nGeneral\n  web-design\n    Design\n  rust-review\n    Review\n";
    assert_eq!(
        skill::parse_bundle_output(plain, &Policy::default()).names,
        ["web-design", "rust-review"]
    );
    assert_eq!(
        skill::parse_bundle_output(
            "■  Repository unavailable\n│  More context",
            &Policy::default()
        )
        .error
        .as_deref(),
        Some("Repository unavailable")
    );
}

#[test]
fn cli_targets_use_verified_aliases_and_never_substitute_for_zo() {
    let command =
        skill::skill_install_command(&["review"], &["claude", "openclaude", "codex"]).unwrap();
    assert_eq!(command.matches("--agent claude-code").count(), 1);
    assert!(command.contains("--agent codex"));
    assert!(skill::skill_install_command(&["review"], &["zo"]).is_none());
    assert_eq!(skill::cli_agent("kimi"), Some("kimi-code-cli"));
    for agent in &zerocode_core::agent::AGENT_SPECS {
        assert!(
            skill::install_target(agent.id).is_some(),
            "{} needs an explicit capability record",
            agent.id
        );
    }
}
#[test]
fn a_cli_target_home_is_scanned_and_accepts_the_native_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let sources = skill::discovery_sources(temp.path(), &[]);
    let root = zerocode_core::skill_install::install_root("qwen-code", &sources).unwrap();
    assert_eq!(root.path, temp.path().join(".qwen/skills"));
    let name = Policy::default().required_names().remove(0);
    let result = zerocode_core::skill_install::install_bundled_skill(
        &name,
        temp.path(),
        &[("qwen-code".into(), "Qwen".into())],
    )
    .unwrap();
    assert_eq!(
        result[0].state,
        zerocode_core::skill_install::InstallState::Written
    );
    let found = skill::scan(root);
    assert_eq!(found.len(), 1);
    assert!(skill::agent_reads_skill("qwen-code", &found[0]));
}
