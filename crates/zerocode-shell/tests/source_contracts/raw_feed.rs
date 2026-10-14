//! The raw inbox feed's road through the window (t-43992 unit 1): a page an
//! agent read whole goes to `zerocode_core::second_brain_feed` behind the
//! person's opt-in switch, handed over before the read's words are answered
//! and run off the answer's road — the agent's answer never waits on a vault.

use std::path::Path;

use super::support::block_after;

fn shell_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// Where `needle` first stands in `haystack`, or a failed assertion that says
/// what moved.
fn at(haystack: &str, needle: &str) -> usize {
    let found = haystack.find(needle);
    assert!(found.is_some(), "`{needle}` is gone:\n{haystack}");
    found.unwrap_or_default()
}

/// The block `marker` opens, or a failed assertion that says it is gone.
fn block<'a>(source: &'a str, marker: &str) -> &'a str {
    assert!(source.contains(marker), "`{marker}` is gone");
    block_after(source, marker)
}

/// A whole read — no selector, judged or plain — is the feed's; a selector's
/// read is a fragment of a page and is not. The page is handed over before
/// its words are answered, because answering consumes the report.
#[test]
fn a_page_read_whole_is_handed_to_the_feed_before_its_words_are_answered() {
    let tools = shell_source("agent_tools_runtime.rs");
    let answering = block(&tools, "async fn dispatch_browser_command(");
    let read = block(answering, "(\"read\", 2) | (\"read\", 3) => {");
    let whole = at(read, "if read.selector.is_none() {");
    let fed = at(
        read,
        "cmd::second_brain::feed_read_page(app, pane, &report);",
    );
    let answered = at(read, "page_said(&read.label, read_words(report))");
    assert!(
        whole < fed && fed < answered,
        "the feed is not a whole read's, or comes after the answer:\n{read}"
    );
}

/// No vault, no work: the published vault is read where the read runs, and
/// everything else — the switch in the settings file, the vault's `raw/` — on
/// a blocking thread of its own.
#[test]
fn the_feed_reads_the_switch_off_the_answers_road() {
    let second_brain = shell_source("cmd/second_brain.rs");
    let glue = block(&second_brain, "pub(crate) fn feed_read_page(");
    let vault = at(glue, "hooks::second_brain_vault()");
    let off_road = at(glue, "spawn_blocking(");
    let switch = at(glue, ".second_brain_raw_capture");
    let fed = at(glue, "zerocode_core::second_brain_feed::feed(");
    assert!(
        vault < off_road && off_road < switch && switch < fed,
        "the feed reads the switch or the vault on the answer's road:\n{glue}"
    );
}

/// The switch turns on only with a saved vault — "on" with nowhere to land
/// would promise files that never come — and turns off always. Its command
/// is one the window actually answers.
#[test]
fn the_switch_turns_on_only_with_a_vault_and_the_window_answers_it() {
    let second_brain = shell_source("cmd/second_brain.rs");
    let setter = block(
        &second_brain,
        "pub(crate) async fn set_second_brain_raw_capture(",
    );
    at(setter, "if enabled && saved_vault(&saved).is_none() {");
    at(setter, "&[setting_key::SECOND_BRAIN_RAW_CAPTURE]");
    at(setter, "settings.second_brain_raw_capture = enabled;");
    let cmd = shell_source("cmd/mod.rs");
    at(&cmd, "set_second_brain_raw_capture");
    let main = shell_source("main.rs");
    let handlers = block(&main, "tauri::generate_handler![");
    at(handlers, "set_second_brain_raw_capture,");
}

/// The window's log names no page (m-48177): the file and the health card
/// are the record of what landed; stderr says only that something failed.
#[test]
fn the_feed_names_no_page_in_the_window_s_log() {
    let second_brain = shell_source("cmd/second_brain.rs");
    let glue = block(&second_brain, "pub(crate) fn feed_read_page(");
    assert!(
        !glue.contains("raw feed wrote"),
        "the feed logs the page it wrote:\n{glue}"
    );
    at(glue, "raw feed failed");
}
