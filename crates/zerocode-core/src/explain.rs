//! What "explain it with a picture" sends, and how it is made safe to send
//! (t-32787).
//!
//! RED: the types, the tables and the signatures are the ones the window will
//! read; every body below is a placeholder that answers the wrong thing, so
//! that each test fails at its assertion on today's tree. The next commit fills
//! the bodies in.

use std::time::Duration;

use crate::capabilities::OneShotRoad;

/// The most a caller may hand over at all, in bytes.
pub const RECEIVE_BYTES_MAX: usize = 512 * 1024;

/// The most of the material that goes into one request, in bytes.
pub const CONTENT_BYTES_MAX: usize = 64 * 1024;

/// How long a one-shot may run.
pub const ONE_SHOT_WALL: Duration = Duration::from_secs(300);

/// How many requests the window holds at once.
pub const ACTIVE_MAX: usize = 16;

/// What is being explained — the three places the window offers the action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Diff,
    Turn,
    Report,
}

impl Kind {
    pub const ALL: [Self; 3] = [Self::Diff, Self::Turn, Self::Report];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Diff => "diff",
            Self::Turn => "turn",
            Self::Report => "report",
        }
    }

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == word)
    }
}

/// The words a request fails with.
pub mod why {
    pub const PARKED: &str = "parked";
    pub const HOLDS_A_DRAFT: &str = "holds_a_draft";
    pub const NO_PANE: &str = "no_pane";
    pub const NO_AGENT: &str = "no_agent";
    pub const IN_FLIGHT: &str = "in_flight";
    pub const TOO_MANY: &str = "too_many";
    pub const NOT_READY: &str = "not_ready";
    pub const NO_PAGE: &str = "no_page";
    pub const CLI_MISSING: &str = "cli_missing";
    pub const QUOTA_WALL: &str = "quota_wall";
    pub const LOGIN_WALL: &str = "login_wall";
    pub const TIMED_OUT: &str = "timed_out";
    pub const NOT_HTML: &str = "not_html";
    pub const CLI_REFUSED: &str = "cli_refused";
    pub const BUDGET: &str = "budget";
    pub const NOT_PUBLISHED: &str = "not_published";

    pub const ALL: [&str; 16] = [
        PARKED,
        HOLDS_A_DRAFT,
        NO_PANE,
        NO_AGENT,
        IN_FLIGHT,
        TOO_MANY,
        NOT_READY,
        NO_PAGE,
        CLI_MISSING,
        QUOTA_WALL,
        LOGIN_WALL,
        TIMED_OUT,
        NOT_HTML,
        CLI_REFUSED,
        BUDGET,
        NOT_PUBLISHED,
    ];
}

/// The material of a request, ready to send.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Prepared {
    pub text: String,
    pub lines: usize,
    pub masked: usize,
    pub clipped: bool,
}

/// Placeholder: sends the raw text as it is.
#[must_use]
pub fn prepare(_kind: Kind, raw: &str) -> Prepared {
    Prepared {
        text: raw.to_string(),
        ..Prepared::default()
    }
}

/// Placeholder: every language is English.
#[must_use]
pub fn language_name(_code: &str) -> &'static str {
    "English"
}

/// Everything a request needs to be written down in words.
#[derive(Debug, Clone, Copy)]
pub struct Ask<'a> {
    pub kind: Kind,
    pub title: &'a str,
    pub language: &'a str,
    pub headline: &'a str,
    pub material: &'a Prepared,
}

/// Placeholder: says nothing.
#[must_use]
pub fn conversation_prompt(_ask: &Ask<'_>) -> String {
    String::new()
}

/// What a one-shot is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OneShotPrompt {
    pub system: String,
    pub user: String,
}

/// Placeholder: asks nothing.
#[must_use]
pub fn one_shot_prompt(_ask: &Ask<'_>) -> OneShotPrompt {
    OneShotPrompt {
        system: String::new(),
        user: String::new(),
    }
}

/// Why an answer is not a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAPage {
    Empty,
    NotHtml,
}

/// Placeholder: nothing is a page.
///
/// # Errors
///
/// Always.
pub fn page_from_output(_said: &str) -> Result<String, NotAPage> {
    Err(NotAPage::NotHtml)
}

/// Placeholder: runs nothing.
#[must_use]
pub fn one_shot_argv(_road: OneShotRoad, _system: &str) -> Vec<String> {
    Vec::new()
}

/// Placeholder: reads nothing.
#[must_use]
pub fn one_shot_stdin(_road: OneShotRoad, _prompt: &OneShotPrompt) -> String {
    String::new()
}

/// Placeholder: nothing is answered.
///
/// # Errors
///
/// Always.
pub fn read_one_shot(
    _road: OneShotRoad,
    _stdout: &str,
    _stderr_tail: &str,
    _success: bool,
) -> Result<String, String> {
    Err(String::new())
}

#[cfg(test)]
mod tests;
