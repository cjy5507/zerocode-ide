//! Claude Code's Remote Control on the Claude panes a person opens — the
//! first step of reaching this window's agents from a phone.
//!
//! `claude --remote-control` registers the session with the person's own
//! claude.ai account over outbound HTTPS only: nothing listens here, and the
//! Claude app and claude.ai/code show the session, answer it and carry its
//! pushes. Claude Code skips a push while its own terminal has the focus,
//! which this window reports (`CSI ?1004h`).
//!
//! The window adds the flag at the doors a person opens a Claude pane through
//! and at no other: a summoned worker reports to its coordinator, not to a
//! phone, and a conversation that is re-entered reconnects the remote session
//! it already recorded.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::AgentKind;
use crate::capabilities::{CLAUDE_RESUME_SELECTORS, args_without_selectors};

/// The flag that starts an interactive Claude Code session with Remote Control.
pub const REMOTE_CONTROL_FLAG: &str = "--remote-control";

/// The short spelling of [`REMOTE_CONTROL_FLAG`], which a person may have
/// typed into an agent's launch arguments.
pub const REMOTE_CONTROL_SHORT_FLAG: &str = "--rc";

/// The flag naming the remote session until its first prompt names it.
pub const SESSION_NAME_PREFIX_FLAG: &str = "--remote-control-session-name-prefix";

/// The prefix a checkout whose folder name has nothing usable gets.
pub const FALLBACK_SESSION_NAME_PREFIX: &str = "zerocode";

/// The longest session name prefix, in characters: a phone's session list
/// shows a short line per session.
pub const MAX_SESSION_NAME_PREFIX_CHARS: usize = 40;

/// The flags that ask Claude Code to print one answer and exit — a run with
/// no interactive session to control.
const PRINT_FLAGS: &[&str] = &["-p", "--print"];

/// Whether the Claude panes a person opens start with Remote Control.
///
/// Stored as a word, `off` or `mine`. A word this build does not hold reads
/// as [`ClaudeRemoteControl::Off`], so an unknown setting never starts a
/// remote session and never makes the settings document unreadable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", from = "String")]
pub enum ClaudeRemoteControl {
    /// No pane gets the flag.
    Off,
    /// The Claude panes a person opens get it; a summoned worker never does.
    ///
    /// The default because this window's owner asked for phone access by
    /// name — the same answer [`crate::agent_teams::TeamsMode::Panes`] gives.
    #[default]
    Mine,
}

impl ClaudeRemoteControl {
    /// The setting `word` names, or `None` for a word it does not hold.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "off" => Some(Self::Off),
            "mine" => Some(Self::Mine),
            _ => None,
        }
    }
}

impl From<String> for ClaudeRemoteControl {
    fn from(word: String) -> Self {
        Self::from_word(&word).unwrap_or(Self::Off)
    }
}

/// The words to put right after the program so a person's pane of `agent`
/// starts with Remote Control — or none.
///
/// `args` are the words after the program. The answer is empty when the
/// setting is off, when `agent` is not Claude Code, when the line already
/// says anything about Remote Control, when it prints one answer and exits,
/// and when it re-enters a conversation: a resumed conversation reconnects
/// the remote session it recorded, and a second flag would open another.
/// Words after a bare `--` are data and are not read.
///
/// The prefix flag always rides along. `--remote-control` takes an optional
/// name, so whatever followed it alone — a launch's first prompt — would be
/// read as the session's name; a flag after it is never taken as one.
///
/// ```
/// use std::path::Path;
/// use zerocode_core::remote_control::{ClaudeRemoteControl, launch_words};
///
/// let words = launch_words(ClaudeRemoteControl::Mine, "claude", &[], Path::new("/work/shop"));
/// assert_eq!(
///     words,
///     ["--remote-control", "--remote-control-session-name-prefix", "shop"]
/// );
/// ```
#[must_use]
pub fn launch_words(
    setting: ClaudeRemoteControl,
    agent: &str,
    args: &[String],
    checkout: &Path,
) -> Vec<String> {
    if setting == ClaudeRemoteControl::Off || AgentKind::from_slug(agent) != Some(AgentKind::Claude)
    {
        return Vec::new();
    }
    let before_data = args
        .iter()
        .position(|word| word == "--")
        .map_or(args, |end| &args[..end]);
    if before_data.iter().any(|word| names_remote_control(word))
        || before_data
            .iter()
            .any(|word| PRINT_FLAGS.contains(&word.as_str()))
        || re_enters_a_conversation(before_data)
    {
        return Vec::new();
    }
    vec![
        REMOTE_CONTROL_FLAG.to_string(),
        SESSION_NAME_PREFIX_FLAG.to_string(),
        session_name_prefix(checkout),
    ]
}

/// What the remote session is called until its first prompt names it: the
/// checkout's folder name, so a phone's list says which checkout an idle
/// pane sits in.
///
/// Letters, digits, `.`, `_` and `-` are kept, in any script; every other
/// run of characters becomes one `-`. The prefix never starts with `-` —
/// an argument parser would read it as a flag — and is at most
/// [`MAX_SESSION_NAME_PREFIX_CHARS`] characters. A name with nothing left
/// is [`FALLBACK_SESSION_NAME_PREFIX`].
#[must_use]
pub fn session_name_prefix(checkout: &Path) -> String {
    let name = checkout
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let mut prefix = String::with_capacity(name.len());
    let mut kept = 0;
    for ch in name.chars() {
        if kept == MAX_SESSION_NAME_PREFIX_CHARS {
            break;
        }
        let ch = if ch.is_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            ch
        } else {
            '-'
        };
        if ch == '-' && (prefix.is_empty() || prefix.ends_with('-')) {
            continue;
        }
        prefix.push(ch);
        kept += 1;
    }
    let prefix = prefix.trim_end_matches('-');
    if prefix.is_empty() {
        FALLBACK_SESSION_NAME_PREFIX.to_string()
    } else {
        prefix.to_string()
    }
}

/// Whether one word already says something about Remote Control — the flag,
/// its short spelling or the prefix, alone or as `flag=value`.
fn names_remote_control(word: &str) -> bool {
    [
        REMOTE_CONTROL_FLAG,
        REMOTE_CONTROL_SHORT_FLAG,
        SESSION_NAME_PREFIX_FLAG,
    ]
    .iter()
    .any(|flag| {
        word.strip_prefix(flag)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('='))
    })
}

/// Whether the line carries one of Claude Code's resume or continue
/// selectors — read by the same scrub the resume door uses, so the two can
/// never disagree about what a selector is.
fn re_enters_a_conversation(args: &[String]) -> bool {
    args_without_selectors(CLAUDE_RESUME_SELECTORS, args).len() != args.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &[&str]) -> Vec<String> {
        line.iter().map(|word| (*word).to_string()).collect()
    }

    fn launch(args: &[&str]) -> Vec<String> {
        launch_words(
            ClaudeRemoteControl::Mine,
            "claude",
            &words(args),
            Path::new("/work/zerocode"),
        )
    }

    mod launch_words {
        use super::*;

        #[test]
        fn a_fresh_claude_launch_gets_the_flag_and_the_checkouts_prefix() {
            assert_eq!(
                launch(&["--dangerously-skip-permissions"]),
                words(&[
                    "--remote-control",
                    "--remote-control-session-name-prefix",
                    "zerocode"
                ])
            );
        }

        #[test]
        fn the_word_after_the_flag_is_a_flag_so_no_prompt_is_taken_as_the_name() {
            let added = launch(&["fix the flaky test"]);
            assert!(
                added.get(1).is_some_and(|next| next.starts_with("--")),
                "the word after --remote-control must be a flag: {added:?}"
            );
        }

        #[test]
        fn the_setting_off_adds_nothing() {
            let added = launch_words(
                ClaudeRemoteControl::Off,
                "claude",
                &[],
                Path::new("/work/zerocode"),
            );
            assert!(added.is_empty(), "{added:?}");
        }

        #[test]
        fn an_agent_other_than_claude_gets_nothing() {
            let added = launch_words(
                ClaudeRemoteControl::Mine,
                "codex",
                &[],
                Path::new("/work/zerocode"),
            );
            assert!(added.is_empty(), "{added:?}");
        }

        #[test]
        fn an_unknown_agent_gets_nothing() {
            let added = launch_words(
                ClaudeRemoteControl::Mine,
                "not-an-agent",
                &[],
                Path::new("/work/zerocode"),
            );
            assert!(added.is_empty(), "{added:?}");
        }

        #[test]
        fn a_line_with_the_flag_already_is_left_as_written() {
            assert!(launch(&["--remote-control"]).is_empty());
        }

        #[test]
        fn a_line_with_the_flag_and_a_name_is_left_as_written() {
            assert!(launch(&["--remote-control=My Project"]).is_empty());
        }

        #[test]
        fn a_line_with_the_short_flag_is_left_as_written() {
            assert!(launch(&["--rc"]).is_empty());
        }

        #[test]
        fn a_line_with_its_own_prefix_is_left_as_written() {
            assert!(launch(&["--remote-control-session-name-prefix", "mine"]).is_empty());
        }

        #[test]
        fn a_flag_that_only_starts_like_the_short_one_is_not_it() {
            assert_eq!(launch(&["--rcfile"]).len(), 3);
        }

        #[test]
        fn a_resume_reconnects_by_itself_and_gets_nothing() {
            assert!(launch(&["--resume", "cd026e19-e32b-42f3-875c-ca5fc83a4317"]).is_empty());
        }

        #[test]
        fn a_short_resume_gets_nothing() {
            assert!(launch(&["-r", "cd026e19-e32b-42f3-875c-ca5fc83a4317"]).is_empty());
        }

        #[test]
        fn a_joined_resume_gets_nothing() {
            assert!(launch(&["--resume=cd026e19-e32b-42f3-875c-ca5fc83a4317"]).is_empty());
        }

        #[test]
        fn a_continue_gets_nothing() {
            assert!(launch(&["--continue"]).is_empty());
        }

        #[test]
        fn a_short_continue_gets_nothing() {
            assert!(launch(&["-c"]).is_empty());
        }

        #[test]
        fn a_print_run_gets_nothing() {
            assert!(launch(&["-p", "say hi"]).is_empty());
        }

        #[test]
        fn a_long_print_run_gets_nothing() {
            assert!(launch(&["--print", "say hi"]).is_empty());
        }

        #[test]
        fn words_after_the_data_separator_do_not_count() {
            assert_eq!(launch(&["--", "--resume"]).len(), 3);
        }
    }

    mod session_name_prefix {
        use super::*;

        #[test]
        fn is_the_checkout_folder_name() {
            assert_eq!(
                session_name_prefix(Path::new("/Users/dev/2026/zerocode-wt/t-4183")),
                "t-4183"
            );
        }

        #[test]
        fn folds_each_run_of_other_characters_into_one_dash() {
            assert_eq!(
                session_name_prefix(Path::new("/work/my project (2)")),
                "my-project-2"
            );
        }

        #[test]
        fn never_starts_with_a_dash() {
            assert_eq!(session_name_prefix(Path::new("/work/--odd")), "odd");
        }

        #[test]
        fn keeps_letters_of_any_script() {
            assert_eq!(session_name_prefix(Path::new("/work/가게-앱")), "가게-앱");
        }

        #[test]
        fn keeps_dots_and_underscores() {
            assert_eq!(session_name_prefix(Path::new("/work/v1.2_rc")), "v1.2_rc");
        }

        #[test]
        fn is_cut_to_the_limit() {
            let long = "a".repeat(MAX_SESSION_NAME_PREFIX_CHARS + 9);
            let prefix = session_name_prefix(&Path::new("/work").join(long));
            assert_eq!(prefix.chars().count(), MAX_SESSION_NAME_PREFIX_CHARS);
        }

        #[test]
        fn falls_back_when_nothing_is_left() {
            assert_eq!(
                session_name_prefix(Path::new("/work/(%)")),
                FALLBACK_SESSION_NAME_PREFIX
            );
        }

        #[test]
        fn falls_back_for_a_path_with_no_folder_name() {
            assert_eq!(
                session_name_prefix(Path::new("/")),
                FALLBACK_SESSION_NAME_PREFIX
            );
        }
    }

    mod setting {
        use super::*;

        #[test]
        fn the_default_is_mine() {
            assert_eq!(ClaudeRemoteControl::default(), ClaudeRemoteControl::Mine);
        }

        #[test]
        fn is_written_as_its_word() {
            let written = serde_json::to_string(&ClaudeRemoteControl::Off).expect("serializes");
            assert_eq!(written, "\"off\"");
        }

        #[test]
        fn reads_the_word_it_wrote() {
            let read: ClaudeRemoteControl = serde_json::from_str("\"mine\"").expect("reads");
            assert_eq!(read, ClaudeRemoteControl::Mine);
        }

        #[test]
        fn reads_an_unknown_word_as_off() {
            let read: ClaudeRemoteControl = serde_json::from_str("\"all\"").expect("reads");
            assert_eq!(read, ClaudeRemoteControl::Off);
        }

        #[test]
        fn from_word_refuses_a_word_it_does_not_hold() {
            assert_eq!(ClaudeRemoteControl::from_word("on"), None);
        }
    }
}
