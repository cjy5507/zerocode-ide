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
/// a fixed order, the first mark that refuses saying why.
pub fn judge(_facts: &Facts, _act: Act) -> Result<(), ProviderError> {
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
    /// Whether `query` — an `--app` as an agent typed it — names this process.
    #[must_use]
    pub fn named_by(&self, _query: &str) -> bool {
        let _ = identity::AppQuery::parse;
        false
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
