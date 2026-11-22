//! t-3994: the harness capability table (agent-orchestrator's
//! `ports/agent.go`, carried over as data). Every write door — launch,
//! resume, vault reopen, worker split, automation, mail paste, restart nudge,
//! quick command — asks `zerocode_core::capabilities` for what its agent can
//! do, and none of them spells an agent's name. The two P0s this closes were
//! doors that knew better than the table: a launch prompt on `claude
//! --prefill`, and advice typed at a shell whose agent had left.

use std::path::Path;

use super::support::{block_after, shipped_backend, strip_comments, strip_rust_comments};

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
        ("composer_words_for", None, "fn composer_words_for("),
        ("start_settle_for", None, "fn start_settle_for("),
        ("enter_retry_for", None, "fn enter_retry_for("),
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

/// The model roads branch on no agent's name (t-124593; the person,
/// 2026-11-21: 「하드코딩금지 중복코드 금지 지금 모든 모델 오케스트레이션이
/// 일반화 되야해」): how a summons names a model and an effort, where an
/// agent's models are learned and read, the lineup a summons picks from, the
/// composer's model menu, the auto summons' candidates and the trust flag a
/// worker split adds each read the agent's row. A CLI added later is a row,
/// and a branch added here fails this.
#[test]
fn the_model_roads_branch_on_no_agents_name() {
    let branches = agent_name_branches();
    let core_orchestration = strip_rust_comments(&core_source("orchestration.rs"));
    let slash_catalog = shipped_code(&shell_source("slash_catalog.rs"));
    let backend = shipped_code(shipped_backend());
    // The neighbours on the same road (the review's I2, t-124593): the
    // family rule, the gauge, the auto summons' choice, the catalog's launch
    // line, the settings' summon row and a CLI's own lineup. The window's
    // orchestration file is read whole (a test-only item stands near its
    // top), comments set aside.
    let core_lineup = shipped_code(&core_source("summon_difficulty/lineup.rs"));
    let shell_orchestration = strip_rust_comments(&shell_source("orchestration.rs"));
    let catalog = &shell_orchestration[shell_orchestration
        .find("impl Launcher for Catalog")
        .expect("the catalog's launcher")..];
    let settings = shipped_code(&shell_source("typesafe_settings.rs"));
    let summon_agent = &settings[settings
        .find("impl SummonAgent {")
        .expect("the settings' summon row")..];
    let whole = [
        ("cli_models.rs", shipped_code(&core_source("cli_models.rs"))),
        (
            "summon_lineup.rs",
            shipped_code(&shell_source("summon_lineup.rs")),
        ),
    ];
    let roads = [
        (
            "tuning_of",
            block_after(&core_orchestration, "fn tuning_of("),
        ),
        (
            "launch_tuning",
            block_after(&core_orchestration, "fn launch_tuning("),
        ),
        (
            "takes_effort",
            block_after(&core_orchestration, "pub fn takes_effort("),
        ),
        (
            "ladder_efforts",
            block_after(&core_orchestration, "pub fn ladder_efforts("),
        ),
        (
            "carries_model",
            block_after(&core_orchestration, "fn carries_model("),
        ),
        (
            "served_by_zo",
            block_after(&core_orchestration, "fn served_by_zo("),
        ),
        (
            "lists_model",
            block_after(&core_orchestration, "fn lists_model("),
        ),
        (
            "listed_by_a_cli",
            block_after(&core_orchestration, "fn listed_by_a_cli("),
        ),
        (
            "agent_row",
            block_after(&core_orchestration, "fn agent_row("),
        ),
        (
            "agent_lineup",
            block_after(&slash_catalog, "pub fn agent_lineup("),
        ),
        (
            "agent_models_from",
            block_after(&slash_catalog, "fn agent_models_from("),
        ),
        (
            "agent_models_from_listed",
            block_after(&slash_catalog, "pub fn agent_models_from_listed("),
        ),
        (
            "agent_models",
            block_after(&slash_catalog, "pub fn agent_models("),
        ),
        (
            "takes_a_model",
            block_after(&slash_catalog, "fn takes_a_model("),
        ),
        (
            "trust_flag_words",
            block_after(&backend, "fn trust_flag_words("),
        ),
        (
            "runs_model",
            block_after(&core_orchestration, "pub fn runs_model("),
        ),
        (
            "quota_gauge_for",
            block_after(&core_orchestration, "pub fn quota_gauge_for("),
        ),
        (
            "native_agent",
            block_after(&core_orchestration, "pub fn native_agent("),
        ),
        (
            "summonable",
            block_after(&core_orchestration, "pub fn summonable("),
        ),
        (
            "Lineup::from_listed",
            block_after(&core_lineup, "pub fn from_listed("),
        ),
        (
            "Catalog::command_for",
            block_after(catalog, "fn command_for("),
        ),
        ("SummonAgent::of", block_after(summon_agent, "fn of(")),
    ];
    for (road, text) in whole
        .iter()
        .map(|(file, text)| (*file, text.as_str()))
        .chain(roads)
    {
        for spelled in &branches {
            assert!(
                !text.contains(spelled.as_str()),
                "{road} branches on `{spelled}` — the agent's row answers this"
            );
        }
    }
}

/// The conversation view's road for a session its CLI writes as one document
/// (t-124593) is chosen by the shape of the store row that keeps the file
/// and read by the document's own schema — never by the agent's name, the
/// one test the pane's door kept before it (`Some("zo")` for zo's archive).
#[test]
fn the_document_road_branches_on_no_agents_name() {
    let branches = agent_name_branches();
    let core_live = shipped_code(&core_source("live_hook_turns.rs"));
    let core_transcript = shipped_code(&core_source("transcript.rs"));
    let core_binding = shipped_code(&core_source("pane_transcript.rs"));
    let core_vault = shipped_code(&core_source("vault.rs"));
    let terminal = shipped_code(&shell_source("cmd/terminal.rs"));
    let pane_runtime = shipped_code(&shell_source("pane_runtime.rs"));
    let binding = shipped_code(&shell_source("pane_transcript.rs"));
    let roads = [
        ("live_hook_turns.rs", core_live.as_str()),
        (
            "document_read",
            block_after(&core_transcript, "pub fn document_read("),
        ),
        (
            "step_turns",
            block_after(&core_transcript, "fn step_turns("),
        ),
        (
            "call_result",
            block_after(&core_transcript, "fn call_result<"),
        ),
        ("format_of", block_after(&core_binding, "pub fn format_of(")),
        (
            "written_whole",
            block_after(&core_vault, "pub const fn written_whole("),
        ),
        ("carded", block_after(&core_vault, "pub const fn carded(")),
        (
            "turn_write_settle",
            block_after(&core_vault, "pub fn turn_write_settle("),
        ),
        (
            "document_log_within",
            block_after(&terminal, "fn document_log_within("),
        ),
        (
            "document_read_within",
            block_after(&terminal, "fn document_read_within("),
        ),
        (
            "whole_document_log",
            block_after(&terminal, "pub(crate) fn whole_document_log<"),
        ),
        (
            "live_rows",
            block_after(&terminal, "pub(crate) fn live_rows("),
        ),
        (
            "hear_live_turn",
            block_after(&pane_runtime, "pub(crate) fn hear_live_turn<"),
        ),
        (
            "tell_when_settled",
            block_after(&pane_runtime, "pub(crate) fn tell_when_settled("),
        ),
        ("steps_back", block_after(&terminal, "fn steps_back(")),
        (
            "the pane's shape",
            block_after(&binding, "pub(crate) fn written_whole("),
        ),
    ];
    for (road, text) in roads {
        for spelled in &branches {
            assert!(
                !text.contains(spelled.as_str()),
                "{road} branches on `{spelled}` — the store row's shape answers this"
            );
        }
    }
    let pane = block_after(&terminal, "fn pane_log(");
    let document = block_after(&terminal, "pub(crate) fn whole_document_log<");
    assert!(
        pane.contains("crate::pane_transcript::written_whole(&state, term, &path)")
            && pane.contains("whole_document_log(")
            && document.contains("document_read_within(path, read, whole, LONG_LINE_CAP)")
            && document.contains("return document_log(path, read, whole);"),
        "the pane's door no longer picks its road by the shape of the file it named:\n{pane}\n{document}"
    );
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
        "fn composer_words_for(",
        "fn start_settle_for(",
        "fn enter_retry_for(",
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

fn pty_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../zerocode-pty/src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// What a door would have to spell to read or write the person's clipboard:
/// the pasteboard itself, the tools that reach it, the plugin, and a
/// synthesized Cmd+V.
const PASTEBOARD_SPELLINGS: [&str; 9] = [
    "NSPasteboard",
    "generalPasteboard",
    "pbcopy",
    "pbpaste",
    "arboard",
    "clipboard_manager",
    "clipboard()",
    "osascript",
    "CGEvent",
];

/// No delivery door touches the person's clipboard (t-17274): the words go
/// to the pty as bytes — a paste frame or typed keys, the row decides — and
/// nothing on the way reads or writes the pasteboard. The delivery machine
/// and its encodings take words and answer bytes; every door that hands a
/// composer words, from the restart's wake to the mail pointer, reaches
/// the pty through them and nothing else.
#[test]
fn no_delivery_door_reaches_the_pasteboard() {
    for file in ["ready.rs", "input.rs"] {
        let machine = strip_rust_comments(&pty_source(file));
        for spelled in PASTEBOARD_SPELLINGS {
            assert!(
                !machine.contains(spelled),
                "the delivery machine's `{file}` spells `{spelled}`"
            );
        }
    }
    let backend = shipped_code(shipped_backend());
    let orchestration = strip_rust_comments(&shell_source("orchestration.rs"));
    let transaction = shipped_code(&shell_source("prompt_transaction.rs"));
    let mut doors: Vec<(&str, &str)> = write_doors()
        .into_iter()
        .filter_map(|(door, source, opens)| source.is_none().then_some((door, opens)))
        .map(|(door, opens)| (door, block_after(&backend, opens)))
        .collect();
    for opens in [
        "fn type_prompt_at_term(",
        "fn prompt_delivery_for(",
        "fn press_enter_at_term(",
        "fn send(&self, term: TermId, text: &str) -> bool {",
        "fn paste_continuation(",
        "fn pasted(",
        "fn pasted_at(",
        "impl WakeReceipts for WindowReceipts {",
    ] {
        doors.push((opens, block_after(&backend, opens)));
    }
    doors.push((
        "deliver_continuation",
        block_after(&orchestration, "fn deliver_continuation("),
    ));
    for opens in ["pub(super) fn turn(", "pub(super) fn settle("] {
        doors.push((opens, block_after(&transaction, opens)));
    }
    for (door, block) in doors {
        for spelled in PASTEBOARD_SPELLINGS {
            assert!(
                !block.contains(spelled),
                "the `{door}` door spells `{spelled}`:\n{block}"
            );
        }
    }
}

/// No model a CLI lists is spelled in the shipped code (t-125261; the person,
/// 2026-11-22: 「devin 코드에 하드코딩 하는데 하드코딩금지, 중복 금지 최신 모델이
/// 업데이트 될때도 있는데 일반화 되야해 모델목록을 보고 거기서 뽑아야함」): every
/// name in the recorded list — a model's id and the name it shows, a family's
/// slug, id and name, its aliases — is read from the recording and looked
/// for in the shipped Rust of the core and the window and in the window's
/// scripts, and is spelled nowhere here; and the window's roads this task
/// drew compare no agent's id.
#[test]
fn no_listed_model_is_spelled_in_the_shipped_code() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let recording = root.join("../zerocode-core/tests/fixtures/devin-models-list-3000.14.json");
    let listed: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&recording).expect("the recorded list"))
            .expect("a list");
    let mut names = Vec::new();
    for family in listed["families"].as_array().expect("its families") {
        for key in ["family_label", "family_uid", "slug"] {
            names.extend(family[key].as_str().map(str::to_string));
        }
        for alias in family["aliases"].as_array().into_iter().flatten() {
            names.extend(alias.as_str().map(str::to_string));
        }
        for model in family["variants"].as_array().into_iter().flatten() {
            for key in ["model_uid", "label"] {
                names.extend(model[key].as_str().map(str::to_string));
            }
        }
    }
    assert!(
        names.len() >= 10,
        "the recording lists its models: {names:?}"
    );
    let mut shipped = Vec::new();
    for src in ["src", "../zerocode-core/src"] {
        shipped_rust(&root.join(src), &mut shipped);
    }
    for entry in std::fs::read_dir(root.join("../../ui")).expect("the window's scripts") {
        let path = entry.expect("a script").path();
        if path.extension().is_some_and(|ext| ext == "js") {
            let text = std::fs::read_to_string(&path).expect("a script");
            shipped.push((path.display().to_string(), strip_comments(&text)));
        }
    }
    assert!(shipped.len() > 100, "the shipped sources were not found");
    for (file, text) in &shipped {
        for name in &names {
            assert!(
                !spells(text, name),
                "{file} spells `{name}`, a model its CLI lists — the list says it"
            );
        }
    }
    let branches = agent_name_branches();
    let window = super::support::window_source();
    for opens in [
        "function syncProvisionalTurns(",
        "function appendComposerModelRows(",
        "async function pollHelperPages(",
    ] {
        let road = block_after(window, opens);
        for spelled in &branches {
            assert!(
                !road.contains(spelled.as_str()),
                "`{opens}` branches on `{spelled}` — the agent's row answers this"
            );
        }
    }
}

/// The shipped half of every Rust file under `dir`; a file of tests alone is
/// passed over.
fn shipped_rust(dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).expect("a source folder") {
        let path = entry.expect("a source").path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            if name != "tests" {
                shipped_rust(&path, out);
            }
        } else if name.ends_with(".rs") && name != "tests.rs" && !name.ends_with("_tests.rs") {
            let text = std::fs::read_to_string(&path).expect("a source file");
            out.push((path.display().to_string(), shipped_code(&text)));
        }
    }
}

/// Whether `text` holds `name` whole — not inside a longer name.
fn spells(text: &str, name: &str) -> bool {
    let part = |glyph: char| glyph.is_ascii_alphanumeric() || matches!(glyph, '-' | '_' | '.');
    text.match_indices(name).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(part)
            && !text[at + name.len()..].chars().next().is_some_and(part)
    })
}
