//! The headless core profile's switch (`ZO_EXPERIMENT_HEADLESS_PROFILE=core`,
//! [`runtime::HeadlessProfile`]): an experiment arm, read where a session
//! opens and before its prompt is built.
//!
//! The switch is heeded only for a run no person and no window is part of: a
//! new root session nobody attends — not interactive, stdin and stdout no
//! terminal — that no pane, ledger or event channel holds, that names no tool
//! list of its own, and that carries nothing of the person's own the core
//! profile would leave out (tools they attached, hooks, an output style, a
//! memory index, a second brain). Any other run is not read for it and goes on
//! as it was, byte for byte, and one stderr line says why, so an arm that did
//! not take is never taken for one that did.

use std::io::IsTerminal;
use std::path::Path;

use runtime::{HeadlessProfile, HEADLESS_PROFILE_ENV};

use crate::cli_args::AllowedToolSet;

/// What the opening knows of a run when it reads the switch.
#[allow(clippy::struct_excessive_bools)] // each bool is one fact of the run the switch is read against, not a state
pub(super) struct Opening<'a> {
    pub(super) cwd: &'a Path,
    pub(super) mcp_config: Option<&'a Path>,
    /// No parent's registry and no teammate harness.
    pub(super) root: bool,
    /// Reopened: the session sends the prompt it stored.
    pub(super) resumed: bool,
    /// The launch names the session's tools (`--allowed-tools`).
    pub(super) launch_list: bool,
    /// Nobody attends the run (`OpenOptions::headless`).
    pub(super) headless: bool,
    /// Nobody can answer it: its stdin is no terminal.
    pub(super) nobody_answers: bool,
    /// It reaches the ZeroCode ledger.
    pub(super) ledger: bool,
    /// The launch named `--events-bind`.
    pub(super) events_bind_named: bool,
}

/// The profile a new session opens on: `None` without the switch; with it,
/// the profile it names when nothing says a person or a window may be part of
/// the run, else `None` and one stderr line that says which.
pub(super) fn chosen(opening: &Opening<'_>) -> Option<HeadlessProfile> {
    let value = std::env::var(HEADLESS_PROFILE_ENV).ok().filter(|value| !value.is_empty())?;
    if let Some(why) = why_not(opening) {
        eprintln!("zo: {HEADLESS_PROFILE_ENV} is set, but {why}; keeping the default prompt and tools");
        return None;
    }
    let profile = HeadlessProfile::named(&value);
    if profile.is_none() {
        let known = HeadlessProfile::ALL.map(HeadlessProfile::name).join(", ");
        eprintln!("zo: {HEADLESS_PROFILE_ENV}={value:?} is not a value zo knows ({known}); keeping the default prompt and tools");
    }
    profile
}

/// The tools a new session on `profile` is given, said out loud once: an arm
/// that applies silently cannot be told from one that never took.
pub(super) fn opening_tools(profile: HeadlessProfile, model: &str) -> AllowedToolSet {
    let tools = match profile {
        HeadlessProfile::Core => tools::headless_core_tool_set(model),
    };
    let names: Vec<&str> = tools.iter().map(|name| core_types::tool_names::wire_tool_name(name.as_str())).collect();
    eprintln!(
        "zo: experiment arm — {HEADLESS_PROFILE_ENV}={}: the headless {} prompt and its {} tools ({})",
        profile.name(),
        profile.name(),
        names.len(),
        names.join(", ")
    );
    tools
}

/// Why the switch is not read for this run, in words — the first condition
/// that says a person or a window may be part of it, the ones the opening
/// already knows first and the ones that read the person's settings last —
/// or `None` when no condition does.
fn why_not(opening: &Opening<'_>) -> Option<String> {
    if let Some(why) = Shape::of(opening).first_sign() {
        return Some(why.to_string());
    }
    if super::runtime_builder::person_attached_tools(opening.cwd, opening.mcp_config) {
        return Some("the person attached tools of their own (an MCP server, a plugin tool or a skill), or the settings do not load".to_string());
    }
    if super::runtime_builder::person_hooks(opening.cwd, opening.mcp_config) {
        return Some("hooks run in it (the settings' or a plugin's)".to_string());
    }
    match runtime::persons_prompt_section(opening.cwd) {
        Ok(None) => None,
        Ok(Some(section)) => Some(format!("its prompt carries {section}")),
        Err(_) => Some("the settings do not load".to_string()),
    }
}

/// The run's shape as the opening and this process know it, before any file
/// is read.
#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools)] // each bool is one sign of a person or a window, not a state
struct Shape {
    child: bool,
    resumed: bool,
    launch_list: bool,
    attended: bool,
    stdin_terminal: bool,
    stdout_terminal: bool,
    pane: bool,
    ledger: bool,
    events: bool,
}

impl Shape {
    fn of(opening: &Opening<'_>) -> Self {
        let set = |name: &str| std::env::var(name).is_ok_and(|value| !value.trim().is_empty());
        Self {
            child: !opening.root,
            resumed: opening.resumed,
            launch_list: opening.launch_list,
            attended: !opening.headless,
            stdin_terminal: !opening.nobody_answers,
            stdout_terminal: std::io::stdout().is_terminal(),
            // A bare shell pane carries the key too, so it does not prove an
            // agent pane (`crate::ide::reporter`); the switch only steps back
            // for it, which is never a wrong turn.
            pane: set(crate::ide::reporter::ENV_PANE_KEY),
            ledger: opening.ledger,
            // What would open a channel, read as settings: the channel itself
            // opens after the prompt is built (`ide::events`).
            events: opening.events_bind_named
                || set(crate::ide::events::BIND_ENV)
                || set(crate::ide::events::ADDR_FILE_ENV),
        }
    }

    fn first_sign(self) -> Option<&'static str> {
        [
            (self.child, "this session runs under a parent (a pane child or a teammate)"),
            (self.resumed, "this session is reopened and keeps the prompt and tools it stored"),
            (self.launch_list, "the launch names its tools (--allowed-tools)"),
            (self.attended, "a person attends this run"),
            (self.stdin_terminal, "its stdin is a terminal"),
            (self.stdout_terminal, "its stdout is a terminal"),
            (self.pane, "a ZeroCode pane launched it (ZEROCODE_PANE_KEY)"),
            (self.ledger, "it reaches the ZeroCode ledger"),
            (self.events, "an event channel is asked for (--events-bind, ZO_EVENTS_BIND or ZO_EVENTS_ADDR_FILE)"),
        ]
        .into_iter()
        .find_map(|(holds, why)| holds.then_some(why))
    }
}

#[cfg(test)]
mod tests {
    use super::Shape;

    /// A run of no person's and no window's has no reason; every fact that
    /// says otherwise is its own reason, alone or first among several.
    #[test]
    fn each_sign_of_a_person_or_a_window_is_its_own_reason() {
        assert_eq!(Shape::default().first_sign(), None);
        let signs: [fn(&mut Shape); 9] = [
            |shape| shape.child = true,
            |shape| shape.resumed = true,
            |shape| shape.launch_list = true,
            |shape| shape.attended = true,
            |shape| shape.stdin_terminal = true,
            |shape| shape.stdout_terminal = true,
            |shape| shape.pane = true,
            |shape| shape.ledger = true,
            |shape| shape.events = true,
        ];
        let mut reasons = Vec::new();
        for sign in signs {
            let mut shape = Shape::default();
            sign(&mut shape);
            reasons.push(shape.first_sign().expect("a sign is a reason"));
        }
        let distinct: std::collections::BTreeSet<_> = reasons.iter().collect();
        assert_eq!(distinct.len(), signs.len(), "each sign has its own words: {reasons:?}");
        let mut all = Shape::default();
        for sign in signs {
            sign(&mut all);
        }
        assert_eq!(all.first_sign(), Some(reasons[0]), "several signs name the first");
    }
}
