//! t-3996: the agent readiness snapshot (agent-orchestrator's
//! `AgentReadinessProvider` — `EnsureAgentReadiness(agent, purpose)`,
//! `Invalidate…`, `Recheck` — and its `AgentReadinessSnapshot`), carried
//! over as ONE observation per agent that the window's picker, the ledger's
//! `agent-list` and the worker summons all read, each by its own freshness.
//!
//! What these contracts keep: the probe asks the capability table which
//! witnesses an agent has and spells no agent's name; it reuses the login
//! checks the account modules already own and never starts a login; the
//! freshness numbers come from one table with its settings overlay; every
//! verb that moves a login invalidates the snapshot; and the summons names
//! the snapshot's evidence in its refusal.

use super::agent_capabilities::{agent_name_branches, shell_source};
use super::support::{
    BACKEND_PARTS, block_after, shipped_backend, strip_rust_comments, window_source,
};

fn backend_part(name: &str) -> &'static str {
    BACKEND_PARTS
        .iter()
        .find(|(held, _)| *held == name)
        .map(|(_, text)| *text)
        .unwrap_or_else(|| panic!("{name} is not in BACKEND_PARTS"))
}

/// The shipped half of one part, comments stripped.
fn shipped_part(name: &str) -> String {
    let text = backend_part(name);
    let (shipped, _) = text.split_once("#[cfg(test)]").unwrap_or((text, ""));
    strip_rust_comments(shipped)
}

/// The probe reads the witness kinds off the agent's row and decides nothing
/// by the agent's name — the `auth_probe` column's first consumer.
#[test]
fn the_probe_asks_the_table_for_its_witnesses_and_spells_no_agent_name() {
    let probing = shipped_part("readiness_runtime.rs");
    assert!(
        probing.contains("agent_capabilities(agent)") && probing.contains(".auth_probe"),
        "the readiness probe no longer asks the capability table:\n{probing}"
    );
    assert!(
        probing.contains("AuthProbeKind::Keychain")
            && probing.contains("AuthProbeKind::GhAuthStatus"),
        "the probe no longer reads which witness kinds the row allows:\n{probing}"
    );
    for spelled in agent_name_branches() {
        assert!(
            !probing.contains(spelled.as_str()),
            "the readiness probe decides by an agent's name (`{spelled}`):\n{probing}"
        );
    }
}

/// The witnesses are the ones the account modules already keep — the
/// identity file, the scoped keychain item, `gh auth status` — and the probe
/// never runs a login, a materialization, or the CLI's own status command.
#[test]
fn the_probe_reuses_the_existing_login_checks_and_starts_no_login() {
    let probing = shipped_part("readiness_runtime.rs");
    for reused in [
        "accounts::login_look(",
        "accounts::signed_in(",
        "accounts::keychain_says(",
        "codex_accounts::identity_in(",
        "gh::integration_status(",
    ] {
        assert!(
            probing.contains(reused),
            "the probe stopped reusing `{reused}` and grew a check of its own:\n{probing}"
        );
    }
    for forbidden in [
        "materialize(",
        "login_alive",
        "login_status_in(",
        "[\"auth\", \"status\"]",
        "relogin",
        "add_account(",
        "Command::new(",
    ] {
        assert!(
            !probing.contains(forbidden),
            "the readiness probe reaches `{forbidden}` — it may only look:\n{probing}"
        );
    }
}

/// The freshness numbers are one table with its settings overlay, and no
/// interval is spelled in the runtime.
#[test]
fn the_freshness_numbers_come_from_the_table_with_its_overlay() {
    let probing = shipped_part("readiness_runtime.rs");
    assert!(
        probing.contains("Limits::default().overlaid(&"),
        "the readiness numbers stopped coming from the table with its overlay:\n{probing}"
    );
    for literal in [
        "300_000",
        "300000",
        "30_000",
        "30000",
        "from_secs(",
        "from_millis(",
    ] {
        assert!(
            !probing.contains(literal),
            "a freshness interval (`{literal}`) is spelled outside `readiness::Limits`:\n{probing}"
        );
    }
    let backend = shipped_backend();
    assert!(
        backend.contains("readiness_limits: readiness_runtime::limits_of(&document.readiness)"),
        "the settings snapshot no longer carries the readiness table"
    );
    assert!(
        backend.contains("readiness_runtime::configure(readiness_runtime::limits_of("),
        "a settings commit no longer hands the runtime its overlaid table"
    );
}

/// The summons reads the snapshot at launch freshness, and a refusal names
/// the snapshot's evidence beside the sentence it already had.
#[test]
fn the_summons_reads_the_launch_snapshot_and_names_its_evidence() {
    let backend = shipped_backend();
    let cutting = block_after(backend, "fn split(");
    let snapshot_at = cutting
        .find("readiness_runtime::ensure(agent, Purpose::Launch)")
        .unwrap_or_else(|| panic!("the summons no longer reads the launch snapshot:\n{cutting}"));
    let door_at = cutting
        .find("accounts::require_unattended_login(")
        .expect("the unattended login door left the split");
    assert!(
        snapshot_at < door_at,
        "the snapshot is read after the door it was meant to inform:\n{cutting}"
    );
    assert!(
        cutting.contains("readiness_runtime::refusal_with_evidence("),
        "the summons refuses without naming the snapshot's evidence:\n{cutting}"
    );
}

/// Every verb that moves a login — Claude, Codex, GitHub — invalidates the
/// snapshot, and a PATH re-read invalidates every row: a stale "authorized"
/// beside a login that just left is the exact lie this snapshot exists to
/// stop telling.
#[test]
fn every_verb_that_moves_a_login_invalidates_the_snapshot() {
    let usage = backend_part("cmd/usage.rs");
    for (verb, moved) in [
        ("fn add_claude_account(", "login_moved(Provider::Anthropic)"),
        (
            "fn relogin_claude_account(",
            "login_moved(Provider::Anthropic)",
        ),
        // The person's pick walks the one switch road (t-7538); its door
        // invalidates the snapshot, checked below.
        ("fn select_claude_account(", "person_switched("),
        ("fn use_system_claude_login(", "person_switched("),
        (
            "fn remove_claude_account(",
            "login_moved(Provider::Anthropic)",
        ),
        ("fn add_codex_account(", "login_moved(Provider::OpenAi)"),
        ("fn relogin_codex_account(", "login_moved(Provider::OpenAi)"),
        ("fn select_codex_account(", "login_moved(Provider::OpenAi)"),
        ("fn remove_codex_account(", "login_moved(Provider::OpenAi)"),
        ("fn relogin_codex_login(", "login_moved(Provider::OpenAi)"),
        ("fn logout_codex_login(", "login_moved(Provider::OpenAi)"),
    ] {
        let body = block_after(usage, verb);
        assert!(
            body.contains(moved),
            "`{verb}` moves a login without `{moved}`:\n{body}"
        );
    }
    let switch = include_str!("../../src/account_switch.rs");
    let door = block_after(switch, "fn selected(&self, to: Option<&str>, at_ms: i64) {");
    assert!(
        door.contains("login_moved(zerocode_core::account::Provider::Anthropic)"),
        "the switch road selects a Claude login without invalidating the snapshot:\n{door}"
    );
    let road = block_after(usage, "async fn person_switched(");
    assert!(
        road.contains("switch_by_person("),
        "the person's pick no longer walks the switch road:\n{road}"
    );
    let github = backend_part("cmd/integration_prefs.rs");
    for verb in ["fn github_select_account(", "fn github_disconnect("] {
        let body = block_after(github, verb);
        assert!(
            body.contains("readiness_runtime::gh_login_moved()"),
            "`{verb}` moves the gh login without invalidating the snapshot:\n{body}"
        );
    }
    let detecting = block_after(backend_part("scm_runtime.rs"), "fn detected_agents(");
    assert!(
        detecting.contains("readiness_runtime::installs_changed()"),
        "a PATH re-read leaves every binary verdict as it was:\n{detecting}"
    );
}

/// The ledger's `agent-list` and the window's picker read the same snapshot:
/// the launcher peeks (never probes while the ledger is held), the picker
/// asks at display freshness, and the row says 「로그인 필요」 before a launch.
#[test]
fn agent_list_and_the_window_read_one_snapshot() {
    let orchestration = shell_source("orchestration.rs");
    for launcher in [
        "impl Launcher for LiveCatalog {",
        "impl Launcher for Catalog {",
    ] {
        let answering = block_after(&orchestration, launcher);
        assert!(
            answering.contains("readiness_runtime::observe(agent)"),
            "`{launcher}` answers readiness from somewhere other than the one snapshot:\n{answering}"
        );
        assert!(
            !answering.contains("readiness_runtime::ensure("),
            "`{launcher}` probes while the ledger is held:\n{answering}"
        );
    }
    let listing = block_after(backend_part("cmd/session.rs"), "fn list_agents(");
    assert!(
        listing.contains("readiness_runtime::ensure_seen(") && listing.contains("Purpose::Display"),
        "the picker's rows no longer carry the display-fresh snapshot:\n{listing}"
    );

    let window = window_source();
    let row = block_after(window, "function agentRow(");
    assert!(
        row.contains("readiness?.auth === \"unauthorized\"")
            && row.contains("t(\"settings.agents.loginNeeded\", \"로그인 필요\")"),
        "the settings row no longer says 「로그인 필요」 off the snapshot:\n{row}"
    );
    let picker = block_after(window, "function paintWorktreeAgentPicker(");
    assert!(
        picker.contains("readiness?.auth === \"unauthorized\""),
        "the worktree agent picker offers a signed-out agent as if it were ready:\n{picker}"
    );
    for language in ["en", "ja", "zh", "es"] {
        let catalog = block_after(window, &format!("  {language}: {{"));
        assert!(
            catalog.contains("\"settings.agents.loginNeeded\":"),
            "`settings.agents.loginNeeded` is missing from the {language} catalog"
        );
    }
    for hand in ["function claudeLoginMoved(", "function codexLoginMoved("] {
        let moved = block_after(window, hand);
        assert!(
            moved.contains("refreshAgents()"),
            "`{hand}` moves a login without repainting the agent rows:\n{moved}"
        );
    }
}
