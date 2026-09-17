//! The judge behind [`super::ActionJudge`] that actually asks
//! (docs/design/jev-browser-action-20260917.md §2.1): a stopped walk's
//! question, sent down the window's one System One wire
//! ([`crate::systemone`]) and read back through the question that was asked.
//!
//! The question, the answer space and every validation rule come from
//! `zerocode_core::browser_action`; the socket, the deadline, the failure words
//! and the Jev door every request passes are the wire's. This file holds the
//! browser row's name for the request and nothing else.

use std::path::{Path, PathBuf};

use serde_json::Value;
use zerocode_core::browser_action::ActionAsk;
use zerocode_core::jev::BROWSER;

use super::{ActionJudge, BROWSER_ACTION_DEADLINE, Judged, Spent};
use crate::api_routers::RouterKeys;
use crate::systemone::{SCHEMA, Wire, request_body};

/// What a successful body says about the question, or why it says nothing.
/// Every shape rule is `browser_action`'s: this reads the envelope and hands
/// the answers straight to the question that was asked.
#[must_use]
pub fn read_body(ask: &ActionAsk, body: &str) -> Judged {
    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        return Judged::Refused(SCHEMA.to_string());
    };
    let Some(answers) = parsed.get("answers") else {
        return Judged::Refused(SCHEMA.to_string());
    };
    match ask.read(answers) {
        Ok(choice) => Judged::Chose(choice),
        Err(_) => Judged::Refused(SCHEMA.to_string()),
    }
}

/// The request body, as the endpoint takes it.
#[must_use]
pub fn request_of(ask: &ActionAsk) -> Value {
    request_body(&ask.state, &ask.questions)
}

/// Where a test points the door: zo's settings file and the workspace the
/// stopped walk runs for.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Doorway {
    pub settings: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
}

/// The judge that actually asks. Built once per recovery, so a walk that is
/// barred or never stops does not read the keychain at all.
pub struct LiveJudge {
    wire: Wire,
    /// The workspace the stopped walk runs for — the folder it was asked
    /// from, as the Computer Use door said it; `None` consents to nothing.
    workspace: Option<PathBuf>,
    spent: Option<Spent>,
}

impl LiveJudge {
    /// Read the key the settings pane keeps; the door reads zo's settings file
    /// for the walk's `workspace`.
    #[must_use]
    pub fn new(keys: &dyn RouterKeys, workspace: Option<&Path>) -> Self {
        Self {
            wire: Wire::new(keys),
            workspace: workspace.map(Path::to_path_buf),
            spent: None,
        }
    }

    /// A judge pointed at one origin with one key, behind the door `doorway`
    /// names — how a test crosses a real socket.
    #[cfg(test)]
    #[must_use]
    pub fn at(base: &str, key: &str, doorway: Doorway) -> Self {
        Self {
            wire: Wire::at(base, key, doorway.settings),
            workspace: doorway.workspace,
            spent: None,
        }
    }

    /// Whether this judge can ask at all — what a caller reads to skip a look
    /// it would only throw away.
    #[must_use]
    pub fn armed(&self) -> bool {
        self.wire.armed()
    }
}

impl ActionJudge for LiveJudge {
    fn choose(&mut self, ask: &ActionAsk) -> Judged {
        let asked = self.wire.ask(
            &BROWSER,
            self.workspace.as_deref(),
            request_of(ask),
            BROWSER_ACTION_DEADLINE,
        );
        self.spent = Some(asked.spent);
        match asked.answer {
            Ok(body) => read_body(ask, &body),
            Err(token) => Judged::Refused(token),
        }
    }

    fn spent(&self) -> Option<Spent> {
        self.spent
    }
}

#[cfg(test)]
mod tests;
