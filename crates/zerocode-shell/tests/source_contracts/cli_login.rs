//! t-6003: one login road for every agent whose own CLI signs itself in.
//!
//! Claude and Codex each had a login button that ran the CLI's login verb in
//! a home this window manages and waited for the credential file. Grok and
//! Kimi are the first two rows of a TABLE that says the same thing per
//! provider — the home variable, the credential witness, the verb or the
//! slash command, the completion judgement — and every door (the settings
//! row, the status bar's sign-in, the readiness invalidation, the tests) is
//! driven off that table. Adding a provider is one row and one fixture.
//!
//! What these contracts keep: Codex logs in through the shared runner and
//! owns no polling loop of its own; the table reuses the usage modules'
//! provider facts rather than copying them; the command doors and the window
//! branch on the row's answers and never on an agent's name; the status bar
//! learns which providers have a sign-in road from the table; and the new
//! words exist in all four catalogs.

use super::agent_capabilities::{agent_name_branches, shell_source};
use super::support::{block_after, strip_comments, strip_rust_comments, window_source};

/// The shipped half of one shell source, comments stripped.
fn shipped(name: &str) -> String {
    let source = shell_source(name);
    // The test MODULE, not the first `#[cfg(test)]`: `codex_accounts.rs`
    // keeps a test-only fixture builder mid-file, and a cut there would drop
    // the login road under test with it.
    let (code, _) = source
        .split_once("#[cfg(test)]\nmod tests")
        .unwrap_or((&source, ""));
    strip_rust_comments(code)
}

#[test]
fn codex_logs_in_through_the_shared_runner_and_owns_no_loop_of_its_own() {
    let codex = shipped("codex_accounts.rs");
    assert!(
        codex.contains("cli_login::run_login(") && codex.contains("cli_login::command("),
        "codex_accounts.rs no longer logs in through the shared runner"
    );
    for own in [
        "try_wait()",
        "POST_AUTH_GRACE",
        "AUTH_POLL",
        "LOGIN_TIMEOUT",
        "thread::spawn",
        ".stdout.take()",
    ] {
        assert!(
            !codex.contains(own),
            "codex_accounts.rs grew a login loop of its own again: `{own}`"
        );
    }
    // And the runner is where those live now — once.
    let runner = shipped("cli_login.rs");
    for owned in [
        "LOGIN_TIMEOUT",
        "WITNESS_POLL",
        "POST_AUTH_GRACE",
        "try_wait()",
    ] {
        assert!(runner.contains(owned), "the shared runner lost `{owned}`");
    }
}

#[test]
fn the_table_borrows_every_provider_fact_from_the_usage_modules() {
    let table = shipped("cli_login.rs");
    let rows = block_after(&table, "pub(crate) const CLI_LOGINS: &[CliLogin] = &[");
    assert!(
        rows.contains("usage_grok::") && rows.contains("usage_kimi::"),
        "a row stopped reading its home and witness off the usage module:\n{rows}"
    );
    // The facts a usage gauge already owns must not be spelled again here.
    // (`auth.json` is not on this list any more: a dozen other CLIs keep
    // their login in a file of that name and nothing in this window reads
    // theirs, so those rows carry the path themselves — which is what the
    // table is for. Grok's copy of it would still be a second copy, and the
    // two row blocks below are where that is checked.)
    for copied in [
        "\".grok\"",
        "\"GROK_HOME\"",
        "\".kimi-code\"",
        "\"kimi-code.json\"",
        "\"KIMI_CODE_HOME\"",
        "auth.x.ai",
    ] {
        assert!(
            !table.contains(copied),
            "cli_login.rs carries a second copy of a provider fact: {copied}"
        );
    }
    for (agent, module) in [("grok", "usage_grok::"), ("kimi", "usage_kimi::")] {
        let whole = block_after(rows, &format!("agent: \"{agent}\","));
        // Only that row: `block_after` runs to the end of the table, and the
        // rows after it are other providers' facts.
        let row = whole.split("CliLogin {").next().unwrap_or(whole);
        assert!(
            row.contains(module),
            "the {agent} row stopped reading its facts off {module}:\n{row}"
        );
        for spelled in ["auth.json", "credentials/", "Holds::"] {
            assert!(
                !row.contains(spelled),
                "the {agent} row spells `{spelled}`, which its gauge already owns:\n{row}"
            );
        }
    }
    // Every row names a catalog agent, or carries the three facts the
    // catalog would have held for it (the two provider CLIs this window
    // does not launch).
    let named = zerocode_core::AGENT_SPECS
        .iter()
        .filter(|spec| rows.contains(&format!("agent: \"{}\",", spec.id)))
        .count();
    let outside = rows.matches("cli: Cli::Own {").count();
    let rows_written = rows.matches("agent: \"").count();
    assert_eq!(
        named + outside,
        rows_written,
        "a login row names an agent neither the catalog nor the row itself knows:\n{rows}"
    );
    assert!(
        rows_written >= 29,
        "the survey's OAuth rows are no longer all here ({rows_written})"
    );
}

/// The rows that are NOT here, and why — the three whose login already has a
/// card of its own on this screen. A row for one of them would be a second
/// button for one login.
#[test]
fn the_three_logins_with_a_card_of_their_own_are_not_rows_as_well() {
    let table = shipped("cli_login.rs");
    let rows = block_after(&table, "pub(crate) const CLI_LOGINS: &[CliLogin] = &[");
    for owned in ["claude", "codex", "antigravity"] {
        assert!(
            !rows.contains(&format!("agent: \"{owned}\",")),
            "`{owned}` has a card of its own and a row here as well"
        );
    }
    // And the card each of them is: the Claude accounts card asks the CLI
    // itself, the Codex card runs on this file's own runner, and the
    // window's Google login is the credential the Antigravity gauge reads.
    assert!(shipped("accounts.rs").contains("\"auth\", \"status\""));
    assert!(shipped("codex_accounts.rs").contains("cli_login::run_login("));
    assert!(shipped("google_login.rs").contains("google_code_assist_oauth"));
}

#[test]
fn the_login_doors_branch_on_the_row_and_never_on_an_agents_name() {
    let usage = shipped("cmd/usage.rs");
    let branches = agent_name_branches();
    for door in [
        "fn cli_login_list(",
        "fn cli_login_start(",
        "fn cli_login_logout(",
        "fn cli_login_wait(",
        "fn cli_login_witness(",
        "fn cli_login_witness_drop(",
    ] {
        let body = block_after(&usage, door);
        for branch in &branches {
            assert!(
                !body.contains(branch.as_str()),
                "{door} branches on an agent's name: {branch}\n{body}"
            );
        }
        assert!(
            body.contains("cli_login::"),
            "{door} no longer reads the login table:\n{body}"
        );
    }
    // Every verb that moves a login forgets the agent's readiness row.
    for door in [
        "fn cli_login_start(",
        "fn cli_login_logout(",
        "fn cli_login_wait(",
    ] {
        let body = block_after(&usage, door);
        assert!(
            body.contains("readiness_runtime::invalidate("),
            "{door} moved a login without invalidating the readiness snapshot:\n{body}"
        );
    }
    let runner = shipped("cli_login.rs");
    for branch in &branches {
        assert!(
            !runner.contains(branch.as_str()),
            "cli_login.rs branches on an agent's name outside its table: {branch}"
        );
    }
}

/// The witness a wait carries is a binding, not a number (astra R2b), and
/// the commands are the table's doors and nothing more: the store's rule —
/// one row, one file, one attempt, taken out once — and the refusal that
/// ends a wait live in `cli_login`'s door bodies, which the runner's own
/// tests walk against a sandboxed home, so what those tests prove is what
/// ships. And a snapshot the file would not let the window read is no
/// snapshot on any road of the table (R2c).
#[test]
fn the_login_doors_are_the_door_bodies_the_runner_tests_walk() {
    let usage = shipped("cmd/usage.rs");
    let witnessing = block_after(&usage, "fn cli_login_witness(");
    let dropping = block_after(&usage, "fn cli_login_witness_drop(");
    let waiting = block_after(&usage, "fn cli_login_wait(");
    assert!(
        witnessing.contains("cli_login::hold_witness(cli_login_baselines(), row, epoch_ms_now())")
            && dropping.contains("cli_login::witness_drop(cli_login_baselines(), witness)")
            && waiting.contains("cli_login::wait(")
            && waiting.contains("cli_login_baselines(),"),
        "a login door no longer hands its store to the door body the tests walk:\n{witnessing}\n{dropping}\n{waiting}"
    );
    for (door, body) in [
        ("cli_login_witness", witnessing),
        ("cli_login_witness_drop", dropping),
        ("cli_login_wait", waiting),
    ] {
        assert!(
            !body.contains(".lock()")
                && !body.contains(".take(")
                && !body.contains(".remove(")
                && !body.contains("Witness::"),
            "{door} reaches into the store or the file itself, past the door body:\n{body}"
        );
    }
    let runner = shipped("cli_login.rs");
    let witness_door = block_after(&runner, "pub(crate) fn hold_witness(");
    let witness_body = block_after(&runner, "fn hold_witness_in(");
    let wait_door = block_after(&runner, "pub(crate) fn wait(");
    let wait_body = block_after(&runner, "fn wait_in(");
    assert!(
        witness_door.contains("hold_witness_in(store, row, &row.home()?, now_ms)")
            && witness_body.contains("baseline_in(row, home, now_ms)?")
            && witness_body.contains(".hold(taken, now_ms)")
            && wait_door.contains("wait_in(")
            && wait_body.contains(".take(id, row, now_ms)?")
            && wait_body.contains("watch_in("),
        "the door bodies no longer take the baseline before the run and out of the store for the wait:\n{witness_body}\n{wait_body}"
    );
    let store = block_after(&runner, "impl Baselines {");
    assert!(
        store.contains("one.current_at(now_ms) && one.agent != taken.agent")
            && store.contains("held.agent != row.agent")
            && store.contains("self.held.remove(&id)")
            && store.contains("Some(taken) if current => Ok(taken)"),
        "the store lost one of its rules — the ceiling, the row, the one taking-out:\n{store}"
    );
    let watching = block_after(&runner, "fn watch_in(");
    assert!(
        watching.contains("Some(baseline) if !baseline.names(row, &file) =>")
            && watching.contains("None => Witness::read(&file, login)?")
            && !watching.contains("Witness::sliced("),
        "the watch compares a baseline against a file it was not taken of, or snapshots what it could not read:\n{watching}"
    );
    for (reader, body, read) in [
        (
            "fn baseline_in(",
            block_after(&runner, "fn baseline_in("),
            "login_read(&file, &login)?",
        ),
        (
            "fn login_headless_in(",
            block_after(&runner, "fn login_headless_in("),
            "Witness::read(",
        ),
        (
            "fn departed(",
            block_after(&runner, "fn departed("),
            "login_read(",
        ),
    ] {
        assert!(
            body.contains(read) && !body.contains("Witness::sliced("),
            "{reader} takes a file it could not read for a file with no login again:\n{body}"
        );
    }
}

#[test]
fn the_window_paints_the_rows_off_the_table_and_types_the_rows_own_commands() {
    let window = strip_comments(window_source());
    let markup = include_str!("../../../../ui/index.html");
    assert!(
        markup.contains("id=\"cli-login-list\"") && markup.contains("id=\"cli-login-note\""),
        "the provider accounts pane lost its CLI login card"
    );
    let arriving = block_after(&window, "function showSettingsPane(");
    assert!(
        arriving.contains("void refreshCliLogins();"),
        "arriving at the accounts pane no longer re-reads the CLI login rows:\n{arriving}"
    );
    for road in [
        "invoke(\"cli_login_list\")",
        "invoke(\"cli_login_start\", { agent: row.agent })",
        "invoke(\"cli_login_logout\", { agent: row.agent })",
        "invoke(\"cli_login_witness\", { agent: row.agent })",
        "invoke(\"cli_login_witness_drop\", { witness })",
        "invoke(\"cli_login_wait\", { agent: row.agent, signedIn: true, witness })",
        "invoke(\"cli_login_wait\", { agent: row.agent, signedIn: false })",
    ] {
        assert!(window.contains(road), "the window no longer walks {road}");
    }
    // The TUI road opens the agent's own pane through the one launch door
    // and types the ROW's slash command — never a literal of its own.
    let opening = block_after(&window, "async function openCliPane(row) {");
    assert!(
        opening.contains("launchAgentTab({ agent: row.agent, prompt: \"\""),
        "the CLI login roads stopped opening the agent's pane through the launch door:\n{opening}"
    );
    let typing = block_after(
        &window,
        "async function typeCliLoginCommand(row, command) {",
    );
    assert!(
        typing.contains("openCliPane(row)")
            && typing.contains(
                "invoke(\"send_prompt\", { term, text: command, submit: true, agent: row.agent })"
            ),
        "the TUI login road stopped going through the launch door and send_prompt:\n{typing}"
    );
    // The run that renews a session (t-7170) is a process START in a pane
    // the window opens for it, and nowhere else (R1, R1b): the launch door
    // spawns the CLI bare in a new pane, or the table's bare line goes into
    // a new plain shell. No pane the window already holds is ever chosen,
    // written at, or asked about — the pane table's `idle` is the past.
    let running = block_after(&window, "async function runCliBare(row) {");
    assert!(
        running.contains("typeCliLoginShellLine(row, row.run_command)")
            && running.contains("openCliPane(row)"),
        "the run-once road no longer opens its own pane:\n{running}"
    );
    for reach in [
        "runningCliPaneOf",
        "paneAgents",
        "paneProgramLeft",
        "setActiveTab",
        "term_text",
        "send_prompt",
        "cli_login_run_at",
    ] {
        assert!(
            !running.contains(reach),
            "the run-once road reaches a pane it did not open (`{reach}`):\n{running}"
        );
    }
    // The walk's answer is not a row: no road writes the table it was
    // handed, and the walk re-reads once it is over (R2b).
    let waiting = block_after(&window, "async function waitForCliLogin(row, witness) {");
    let walking = block_after(
        &window,
        "async function walkCliLoginRoad(row, button, { road, command, walk }) {",
    );
    let starting = block_after(&window, "function startCliLogin(row, button) {");
    let leaving = block_after(&window, "async function logoutCliLogin(row) {");
    assert!(
        !waiting.contains("cliLogins =")
            && !starting.contains("cliLogins =")
            && !leaving.contains("cliLogins =")
            && walking.contains("await refreshCliLogins();")
            && !walking.contains("landed"),
        "a login road paints the table it was handed instead of re-reading it:\n{waiting}\n{walking}"
    );
    // The TUI road's word goes only to a pane the program is still in, as
    // the pane table says — never to a shell it left.
    let panes = block_after(&window, "function runningCliPaneOf(row) {");
    assert!(
        panes.contains("!paneProgramLeft(term)") && typing.contains("runningCliPaneOf(row)"),
        "the TUI road no longer picks its pane by the pane table's word:\n{panes}"
    );
    for block in [
        "function cliLoginRow(row) {",
        "function startCliLogin(row, button) {",
        "function runCliOnce(row, button) {",
        "async function runCliBare(row) {",
        "async function logoutCliLogin(row) {",
        "async function typeCliLoginCommand(row, command) {",
        "async function openCliPane(row) {",
    ] {
        let body = block_after(&window, block);
        for spec in zerocode_core::AGENT_SPECS {
            assert!(
                !body.contains(&format!("\"{}\"", spec.id)),
                "{block} spells an agent's name instead of reading the row:\n{body}"
            );
        }
        assert!(
            !body.contains("/login") && !body.contains("/logout"),
            "{block} spells a slash command the row already carries:\n{body}"
        );
    }
    let starting = block_after(&window, "function startCliLogin(row, button) {");
    assert!(
        starting.contains("row.road")
            && starting.contains("row.tui_login")
            && starting.contains("row.pane_command"),
        "the start door no longer reads the row's road:\n{starting}"
    );
    // The pane-verb road is the GitLab card's: a plain shell of this window
    // with the backend's line typed into it, so a device code printed to
    // stderr lands in front of the person.
    let shelling = block_after(&window, "async function typeCliLoginShellLine(row, line) {");
    assert!(
        shelling.contains("invoke(\"open_term_tab\", { rows: 24, cols: 96, plain: true })")
            && shelling.contains("invoke(\"term_text\", { term, text: `${line}\\r` })"),
        "the pane-verb road stopped typing the row's line into a plain shell:\n{shelling}"
    );
}

#[test]
fn the_status_bar_learns_its_sign_in_roads_from_the_table() {
    let window = strip_comments(window_source());
    let row = block_after(&window, "function usageRosterRow(provider) {");
    // The hand stands for a confirmed sign-out and for an expired session
    // (t-7170), and only where the table gives it a road.
    assert!(
        row.contains("const signIn = usageSignIn(provider);")
            && row.contains("const offer = state.kind === \"sign-in\"")
            && row.contains("if (offer !== null && signIn) {")
            && !row.contains("provider.signIn()"),
        "the roster's sign-in button reads the provider record directly again:\n{row}"
    );
    let asking = block_after(&window, "function usageSignIn(provider) {");
    assert!(
        asking.contains("cliLoginPrograms.has(provider.id)"),
        "the sign-in road no longer consults the CLI login table:\n{asking}"
    );
    let learning = block_after(&window, "function noteCliLoginRows(rows) {");
    assert!(
        learning.contains("cliLoginPrograms.clear();")
            && learning.contains("cliLoginPrograms.set(row.agent, row.program ?? row.agent)"),
        "the table's rows no longer teach the status bar its sign-in roads:\n{learning}"
    );
    let painting = block_after(&window, "async function refreshCliLogins() {");
    assert!(
        painting.contains("noteCliLoginRows(cliLogins.rows"),
        "a fresh table no longer reaches the status bar:\n{painting}"
    );
    // Only the newest read of the table paints (t-7170 R2b): a read asked
    // earlier and answered later — a status command holds one for seconds
    // — never paints over a table read after it, on its answer or on its
    // failure.
    let (before_answer, after_answer) = painting
        .split_once("cliLogins = table;")
        .expect("the read paints the table it answered");
    assert!(
        painting.contains("const read = ++cliLoginReads;")
            && before_answer
                .matches("if (read !== cliLoginReads) return;")
                .count()
                == 2
            && !after_answer.contains("cliLogins ="),
        "a read of the table paints whenever it lands, not only when it is the newest:\n{painting}"
    );
}

#[test]
fn the_cli_login_words_exist_in_every_catalog() {
    let i18n = include_str!("../../../../ui/shell-i18n.js");
    for key in [
        "settings.cliLogins.heading",
        "settings.cliLogins.about",
        "settings.cliLogins.hint",
        "settings.cliLogins.signedOut",
        "settings.cliLogins.notInstalled",
        "settings.cliLogins.signingIn",
        "settings.cliLogins.waitingTui",
        "settings.cliLogins.waitingPane",
        "settings.cliLogins.logoutAsk",
        "settings.cliLogins.logoutBody",
        "settings.cliLogins.noRows",
        "settings.cliLogins.noLine",
        "settings.cliLogins.unreadable",
        "settings.cliLogins.unanswered",
    ] {
        assert_eq!(
            i18n.matches(&format!("\"{key}\":")).count(),
            4,
            "`{key}` is missing from one of the en/ja/zh/es catalogs"
        );
    }
}
