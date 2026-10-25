//! ZeroCode's own window, pressed by the window itself (t-73906).
//!
//! A desktop press, a key or a helper action aimed at ZeroCode stays refused
//! (`own_window`, the helper's `DesktopSelf` and `refuseOwnTarget`): an OS
//! event there takes the person's pointer, focus or keys, and the helper
//! pauses whenever the person has the target app in front — which ZeroCode,
//! the window they work in, nearly always is. So an action that names
//! ZeroCode's own app goes to the page instead: the page reports what each
//! control is ([`Facts`]), this module judges, and the page presses the
//! control in its own document — no pointer, focus or key event is made.
//!
//! The judgement reads what the window already marks a control with — the
//! one popup every question to the person stands in, a secret field, the
//! field the person is in, a terminal pane, the person-only mark, a setting —
//! never a list of screen names, and never a model's opinion.

use serde::{Deserialize, Serialize};

use super::{ProviderError, error_code, identity};

/// The provider methods the window answers for its own app. A look and the
/// four acts the page can make without an OS event.
pub const METHODS: &[&str] = &[
    "getAppState",
    "findElements",
    "readText",
    "click",
    "setValue",
    "scroll",
];

/// How long after the person's last key in a field the field stays theirs.
pub const PERSON_TYPING_QUIET_MS: u64 = 2_000;

/// How long the window waits for its page to answer one request. A press the
/// page did not answer in time is reported unknown, never as pressed.
pub const PAGE_ANSWER_MS: u64 = 3_000;

/// What a look says in place of text it does not carry: only how long it is,
/// in characters.
#[must_use]
pub fn withheld_text(chars: u64) -> String {
    format!("(withheld: {chars} characters)")
}

/// The helper's roles for a control a person writes text in.
pub const TEXT_ENTRY_ROLES: &[&str] = &["AXTextField", "AXTextArea", "AXComboBox"];

/// What a look shows of a control's value, from the page's answer: `value`
/// as it said it, `withheld` the length of text it held back. A secret
/// field shows nothing. Text the page held back, and the text of a field
/// whose facts mark it the person's — their focus or their keys in it, a
/// setting, a pane or a dialog that holds a secret — show only how long
/// they are: a draft, an address or a name the person wrote is theirs to
/// read. A choice, a state and any other text show as they are.
#[must_use]
pub fn shown_value(
    facts: &Facts,
    role: &str,
    value: Option<&str>,
    withheld: Option<u64>,
) -> Option<String> {
    if facts.secret {
        return None;
    }
    if let Some(chars) = withheld {
        return Some(withheld_text(chars));
    }
    let value = value?;
    let persons =
        facts.person_focus || facts.typed_ms_ago.is_some() || facts.setting || facts.beside_secret;
    if persons && TEXT_ENTRY_ROLES.contains(&role) && !value.is_empty() {
        let chars = u64::try_from(value.chars().count()).unwrap_or(u64::MAX);
        return Some(withheld_text(chars));
    }
    Some(value.to_string())
}

/// What the page acts with: a look reads, the others press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Act {
    Look,
    Click,
    SetValue,
    Scroll,
}

impl Act {
    /// The act a provider method asks of the page; `None` for a method the
    /// window does not answer for its own app.
    #[must_use]
    pub fn of_method(method: &str) -> Option<Self> {
        match method {
            "getAppState" | "findElements" | "readText" => Some(Self::Look),
            "click" => Some(Self::Click),
            "setValue" => Some(Self::SetValue),
            "scroll" => Some(Self::Scroll),
            _ => None,
        }
    }
}

/// What the page says of one control, read off its document at the moment
/// of the act. Each is a mark the window already draws, not a judgement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Facts {
    /// Inside the popup where the window puts every question to the person
    /// (a tool's permission, a question, a confirm, a secret, a Computer Use
    /// confirm or hand-over).
    pub asking: bool,
    /// A secret field itself: a password field or one the page marks for a
    /// password or a one-time code.
    pub secret: bool,
    /// Inside a settings pane or a dialog that holds a secret field.
    pub beside_secret: bool,
    /// Inside a terminal pane.
    pub terminal: bool,
    /// Inside a container the window marks as the person's alone.
    pub person_only: bool,
    /// Inside a settings pane's body (not the list of panes).
    pub setting: bool,
    /// The field the person's focus is in, or a control holding it.
    pub person_focus: bool,
    /// Milliseconds since the person last typed in this field, when they have.
    pub typed_ms_ago: Option<u64>,
}

/// Whether the page may make `act` on a control with these facts: refused in
/// a fixed order, the first mark that refuses saying why. A look reads every
/// control; a scroll moves no focus and no text, so the person's field does
/// not refuse it.
pub fn judge(facts: &Facts, act: Act) -> Result<(), ProviderError> {
    if act == Act::Look {
        return Ok(());
    }
    let marks = [
        (
            facts.asking,
            "it stands in the popup where the window asks the person; the answer is theirs",
        ),
        (facts.secret, "it is a secret field"),
        (
            facts.beside_secret,
            "it is in a pane or a dialog that holds a secret field",
        ),
        (
            facts.terminal,
            "it is in a terminal pane; a pane is driven through zerocode-orc",
        ),
        (facts.person_only, "the window keeps it for the person"),
        (
            facts.setting,
            "a setting is the person's to change; looking and moving between panes are open",
        ),
    ];
    if let Some((_, why)) = marks.iter().find(|(marked, _)| *marked) {
        return Err(ProviderError::new(
            error_code::SELF_SENSITIVE,
            format!("not pressed: {why}"),
        ));
    }
    let persons = facts.person_focus
        || facts
            .typed_ms_ago
            .is_some_and(|ago| ago < PERSON_TYPING_QUIET_MS);
    if persons && act != Act::Scroll {
        return Err(ProviderError::new(
            error_code::PERSON_TYPING,
            "not pressed: the person is in that field of ZeroCode's window; look again in a moment",
        ));
    }
    Ok(())
}

/// ZeroCode's own app as this process knows itself: its pid, its bundle
/// identifier and the names a person knows it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnApp {
    pub pid: u32,
    pub bundle_id: String,
    pub names: Vec<String>,
}

impl OwnApp {
    /// Whether `query` — an `--app` as an agent typed it — names this process:
    /// its pid, or its bundle identifier or one of its names the way the
    /// helper matches any app (any case, never a fragment).
    #[must_use]
    pub fn named_by(&self, query: &str) -> bool {
        match identity::AppQuery::parse(query) {
            Ok(identity::AppQuery::Pid(pid)) => pid == self.pid,
            Ok(identity::AppQuery::Named(name)) => identity::matches_any(
                &name,
                "",
                Some(&self.bundle_id),
                self.names.iter().map(String::as_str),
            ),
            Err(_) => false,
        }
    }
}

/// The refusal of an act the window does not make on its own app: a key, a
/// typed text, a paste, a drag or a secondary action would need an OS event
/// in the window the person works in.
#[must_use]
pub fn unsupported(method: &str) -> ProviderError {
    ProviderError::new(
        error_code::UNSUPPORTED_CAPABILITY,
        format!(
            "{method} is not made on ZeroCode's own window: the window presses its own controls without keys; use set-value for a field and click for a button"
        ),
    )
}

#[cfg(test)]
mod tests;
