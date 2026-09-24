//! t-3994: the harness capability table (agent-orchestrator's
//! `ports/agent.go`, carried over as data). Every write door — launch,
//! resume, vault reopen, worker split, automation, mail paste, restart nudge,
//! quick command — asks `zerocode_core::capabilities` for what its agent can
//! do, and none of them spells an agent's name. The two P0s this closes were
//! doors that knew better than the table: a launch prompt on `claude
//! --prefill`, and advice typed at a shell whose agent had left.

use std::path::Path;

use super::support::{block_after, shipped_backend, strip_rust_comments};

pub(super) fn shell_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

fn core_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../zerocode-core/src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// The shipped half of one file, comments stripped, so a `// "claude" =>`
/// in prose is not read as a branch.
fn shipped_code(source: &str) -> String {
    let (shipped, _) = source.split_once("#[cfg(test)]").unwrap_or((source, ""));
    strip_rust_comments(shipped)
}

/// Every spelling of "this agent, by name" a door could branch on: the
/// catalogue's ids as string literals beside a comparison or a match arm,
/// antigravity's binary name, and the lane enum's variants.
pub(super) fn agent_name_branches() -> Vec<String> {
    let mut spelled = Vec::new();
    let ids = zerocode_core::AGENT_SPECS
        .iter()
        .map(|spec| spec.id)
        .chain(["agy"]);
    for id in ids {
        for shape in [
            format!("\"{id}\" =>"),
            format!("== \"{id}\""),
            format!("!= \"{id}\""),
            format!("\"{id}\" =="),
            format!("\"{id}\" !="),
            format!("Some(\"{id}\")"),
        ] {
            spelled.push(shape);
        }
    }
    for kind in zerocode_core::ALL_AGENTS {
        spelled.push(format!("AgentKind::{kind:?}"));
    }
    spelled
}

/// The write doors, each with the source it ships in and the line that
/// opens it. `None` for the source means the joined shipped backend.
fn write_doors() -> Vec<(&'static str, Option<&'static str>, &'static str)> {
    vec![
        // The launcher: a person's prompt, a recipe, a notes-menu resume.
        ("launch_agent_tab", None, "fn launch_agent_tab("),
        // The resume roads: the interactive wake and the worker reseat share
        // the builder; the fresh road is for a conversation never written.
        ("resume_command", None, "fn resume_command("),
        ("fresh_command", None, "fn fresh_command("),
        ("resume_session", None, "fn resume_session("),
        ("wake_conversation", None, "fn wake_conversation<"),
        ("ResumeDoor", None, "impl WakeWindow for ResumeDoor<'_> {"),
        ("vault_resume_base", None, "fn vault_resume_base("),
        // The readiness readers every delivery goes through.
        ("ready_signal_for", None, "fn ready_signal_for("),
        ("ready_quiet_for", None, "fn ready_quiet_for("),
        ("ready_timeout_for", None, "fn ready_timeout_for("),
        ("composer_clear_for", None, "fn composer_clear_for("),
        // The worker split and the ledger's typed roads: the dispatch
        // paste, the mail pointer, the ssh send.
        ("split", None, "fn split("),
        ("paste", None, "fn paste("),
        ("point", None, "fn point("),
        ("ssh_register_delivery", None, "fn ssh_register_delivery("),
        // Automation: the scheduled spawn and the reused session.
        ("start_automation", None, "fn start_automation("),
        ("remember_reused_run", None, "fn remember_reused_run("),
        ("reused_run_delivery", None, "fn reused_run_delivery("),
        // The restart nudge: both nudge roads, and the one fallback. Whether
        // a wake is nudged at all is the goodbye's word about a worker (t-7812
        // E), not a row's; which road its words take is the row's.
        ("place_words", None, "fn place_words("),
        ("deliver_composer", None, "fn deliver_composer("),
        ("note_resolution", None, "fn note_resolution("),
        // Quick commands, where a prompt is saved to be launched later.
        ("validate_quick_command", None, "fn validate_quick_command("),
        ("second_brain_setup", None, "fn second_brain_setup("),
        // The coordinator's worker launcher lives outside the joined backend.
        (
            "Catalog",
            Some("orchestration.rs"),
            "impl Launcher for Catalog {",
        ),
        // The between-turn effort door (t-5637): the beat's pass that types
        // a picker's command and keys at a worker's composer.
        (
            "step_effort::sweep",
            Some("orchestration/step_effort.rs"),
            "pub(super) fn sweep(",
        ),
        (
            "step_effort::advance",
            Some("orchestration/step_effort.rs"),
            "fn advance(",
        ),
    ]
}

/// No write door decides anything by an agent's name. The table decides;
/// the door asks. This is the contract that keeps the next `claude`-shaped
/// special case from landing in a door instead of on the agent's row.
#[test]
fn no_write_door_branches_on_an_agents_name() {
    let backend = shipped_code(shipped_backend());
    // Its tests live in `orchestration/tests.rs`, so the whole file ships;
    // an inline `#[cfg(test)]` early in it would truncate the split.
    let orchestration = strip_rust_comments(&shell_source("orchestration.rs"));
    let step_effort = shipped_code(&shell_source("orchestration/step_effort.rs"));
    let branches = agent_name_branches();
    for (door, source, opens) in write_doors() {
        let text = match source {
            None => &backend,
            Some("orchestration.rs") => &orchestration,
            Some("orchestration/step_effort.rs") => &step_effort,
            Some(other) => panic!("{door} names an unread source {other}"),
        };
        let block = block_after(text, opens);
        for spelled in &branches {
            assert!(
                !block.contains(spelled.as_str()),
                "the `{door}` door decides by an agent's name (`{spelled}`) instead \
                 of asking the capability table:\n{block}"
            );
        }
        assert!(
            !block.contains("Injection::") && !block.contains("spec.injection"),
            "the `{door}` door re-reads the injection mode itself — the submit \
             road is the table's to spell:\n{block}"
        );
    }
}

/// Each door reads the decision it used to spell off the agent's row.
#[test]
fn every_write_door_reads_its_decision_off_the_table() {
    let backend = shipped_backend();
    let orchestration = shell_source("orchestration.rs");

    // The launcher: the submit road, the store-resume road, the trust menu,
    // the spawn road, and the typed road's readiness — all the row's.
    let launching = block_after(backend, "fn launch_agent_tab(");
    for needed in [
        "let caps = spec.capabilities();",
        "prompt_injection(spec, &prompt)",
        "Some(Injected::Argv(words)) => argv.extend(words),",
        "Some(Injected::AfterStart(text)) => typed_after_start = Some(text),",
        "caps.resume.store",
        "store.id.accepts(session)",
        "argv.push(store.flag.to_string());",
        "caps.resume.launch_args_without_selectors(&plan.args)",
        "caps.trust_menu()",
        "caps.spawn == SpawnRoad::SocketPane",
        "ready_signal_for(Some(spec.id))",
    ] {
        assert!(
            launching.contains(needed),
            "the launcher stopped asking the table for `{needed}`:\n{launching}"
        );
    }

    // The resume builder splices the row's own selectors, for every agent,
    // so the one selector it writes is the only one on the line.
    let resuming = block_after(backend, "fn resume_command(");
    assert!(
        resuming.contains("let caps = spec.capabilities();")
            && resuming.contains("caps.resume.launch_args_without_selectors(&plan.args)"),
        "the resume builder splices selectors by name again:\n{resuming}"
    );
    let waking = block_after(backend, "impl WakeWindow for ResumeDoor<'_> {");
    assert!(
        waking.contains("caps.trust_menu()")
            && waking.contains("caps.spawn == SpawnRoad::SocketPane"),
        "the wake road spells its trust menu or spawn road by name:\n{waking}"
    );
    let reopening = block_after(backend, "fn vault_resume_base(");
    assert!(
        reopening.contains("caps.resume.vault_carries_launch_args")
            && reopening.contains("caps.resume.launch_args_without_selectors(&plan.args)"),
        "the vault reopen decides its base by name:\n{reopening}"
    );

    // The readiness readers are the table's, through the one door.
    for reader in [
        "fn ready_signal_for(",
        "fn ready_quiet_for(",
        "fn ready_timeout_for(",
        "fn composer_clear_for(",
    ] {
        let reading = block_after(backend, reader);
        assert!(
            reading.contains("zerocode_core::agent_capabilities"),
            "`{reader}` reads something other than the capability table:\n{reading}"
        );
    }

    // The worker split: the trust menu, the pointer route, and the parked
    // channel acknowledgement are each the row's answer.
    let splitting = block_after(backend, "fn split(");
    for needed in [
        "caps.and_then(|caps| caps.trust_menu())",
        "caps.is_some_and(|caps| caps.pointer_route == Some(PointerRoute::CodexAppServer))",
        "caps.is_some_and(|caps| caps.submit_ack == SubmitAck::Channel)",
    ] {
        assert!(
            splitting.contains(needed),
            "the worker split stopped asking the table for `{needed}`:\n{splitting}"
        );
    }

    // Quick commands ask whether the agent takes a prompt at start.
    for (door, opens) in [
        ("validate_quick_command", "fn validate_quick_command("),
        ("second_brain_setup", "fn second_brain_setup("),
    ] {
        let saving = block_after(backend, opens);
        assert!(
            saving.contains(".capabilities().takes_prompt_at_start()"),
            "`{door}` re-reads the injection mode:\n{saving}"
        );
    }

    // The coordinator's launcher puts the words on the table's road.
    let summoning = block_after(&orchestration, "impl Launcher for Catalog {");
    assert!(
        summoning.contains("match prompt_injection(spec, prompt) {"),
        "the worker launcher spells the submit road itself:\n{summoning}"
    );
}

/// The facts that moved onto the rows are read from there and nowhere
/// else: the lane enum's acknowledgement answer, the claude selector
/// splice, the vault roster, the trust presets and the store id gate are
/// each a reader of the table now, not a second copy of it.
#[test]
fn the_moved_facts_have_one_home_and_their_old_names_are_readers() {
    let agent = core_source("agent.rs");
    let acknowledging = block_after(&agent, "pub fn reports_prompt_submit(self) -> bool {");
    assert!(
        acknowledging.contains("spec.harness.submit_ack != SubmitAck::None")
            && !acknowledging.contains("Self::Zo"),
        "the acknowledgement answer is spelled beside the rows again:\n{acknowledging}"
    );
    assert_eq!(
        shipped_code(&agent)
            .matches("\n        harness: Harness")
            .count(),
        zerocode_core::AGENT_SPECS.len(),
        "every row carries its harness facts, and only rows do"
    );

    let sessions = core_source("provider_session.rs");
    let splicing = block_after(&sessions, "pub fn claude_args_without_selectors(");
    assert!(
        splicing.contains("args_without_selectors(") && !splicing.contains("\"--continue\""),
        "the claude splice spells its selectors a second time:\n{splicing}"
    );

    let vault = core_source("vault.rs");
    let carrying = block_after(&vault, "pub fn resume_carries_launch_args(");
    assert!(
        carrying.contains("agent_capabilities(") && !carrying.contains("\"claude\""),
        "the vault roster is a list of names again:\n{carrying}"
    );

    let presets = shell_source("agent_trust_presets.rs");
    assert!(
        presets.contains("pub type TrustPreset = zerocode_core::capabilities::TrustMenu;"),
        "the trust presets keep an enum of their own beside the table's"
    );
    let choosing = block_after(&presets, "pub fn preset_of(");
    assert!(
        choosing.contains("trust_menu()") && !choosing.contains("\"zo\" =>"),
        "the preset is chosen by name again:\n{choosing}"
    );

    let scm = shipped_backend();
    let gating = block_after(scm, "fn is_claude_session_id(");
    assert!(
        gating.contains("IdShape::Uuid.accepts(said)"),
        "the store id shape is spelled a second time:\n{gating}"
    );
}

/// The between-turn move roads — `/effort`, `/model`, the picker's keys and
/// its legend — are spelled on the catalog rows alone (t-5637). The beat's
/// pass reads `caps.moves.effort` and the composer chip reads the presence
/// row's `model_command`; neither carries a command of its own, so changing
/// what a CLI takes between turns is one row edit.
#[test]
fn the_between_turn_move_roads_are_spelled_on_the_rows_alone() {
    let rows = core_source("agent.rs");
    let table = core_source("capabilities.rs");
    let pass = shipped_code(&shell_source("orchestration/step_effort.rs"));
    let orchestration = strip_rust_comments(&shell_source("orchestration.rs"));
    let backend = shipped_code(shipped_backend());
    for spelled in [
        "\"/effort\"",
        "\"/model\"",
        "s for this session only",
        "\\x1b[C",
        "\\x1b[D",
    ] {
        assert!(
            rows.contains(spelled) || table.contains(spelled),
            "`{spelled}` is not on any row or in the table"
        );
        for (name, text) in [
            ("orchestration/step_effort.rs", pass.as_str()),
            ("orchestration.rs", orchestration.as_str()),
            ("the joined backend", backend.as_str()),
        ] {
            assert!(
                !text.contains(spelled),
                "{name} spells the move road `{spelled}` itself instead of reading the row"
            );
        }
    }
    // The pass reads the door off the row, and moves only down a road the
    // row says moves between turns.
    for needed in [
        "caps.moves.effort",
        "look.road.moves_between_turns()",
        "look.road.picker()",
        "look.moves.rung_from(",
    ] {
        assert!(
            pass.contains(needed),
            "the pass stopped reading `{needed}` off the row"
        );
    }
    // Every row answers all three columns: the table's type makes it so,
    // and the measured rows are the ones the doors read.
    for spec in &zerocode_core::AGENT_SPECS {
        let moves = spec.capabilities().moves;
        assert_eq!(moves, spec.harness.moves, "{}", spec.id);
        if let Some(road) = moves.effort
            && road.moves_between_turns()
        {
            assert!(
                !moves.ladder.is_empty(),
                "{} moves its effort between turns with no ladder to read a rung off",
                spec.id
            );
        }
    }
}
