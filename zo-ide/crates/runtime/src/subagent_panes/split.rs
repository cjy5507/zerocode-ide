//! Opening one helper's pane: a `split-window` that is given up at a bound
//! (t-19898).
//!
//! `Command::output()` has no limit, so a hung tmux server — or a window whose
//! main thread is busy — held the spawn of a helper for as long as it hung, and
//! ending the client midway can leave what the server had already cut: a pane
//! running a child that nobody waits for. The split now runs as an [`Ask`] with
//! the tmux's own split bound, and one that is given up is cleaned up in two
//! layers, because neither reaches every case alone.
//!
//! - **The note** ([`SPLIT_GIVEN_UP_FILE`]), written first, in the helper's
//!   directory. A child looks for it before it reads its brief and once more
//!   before its first model request, and leaves without working. It reaches a
//!   pane the server opens AFTER the parent gave up — a tmux that recovers and
//!   runs the command it had queued, a window that was busy — which nothing can
//!   look up at the time, and it asks nothing of the multiplexer. A refusal
//!   leaves it too: the window refuses after its own deadline
//!   ("tmux: timed out waiting for the window") and its hook bridge says the
//!   request it had admitted "may still finish", so a refusal is not always the
//!   last word on the pane.
//! - **The look.** The panes are listed with what each started with, and one
//!   whose start command carries this helper's id is closed. The id is the last
//!   word of the directory the pane was told to work in (`--teammate <dir>`),
//!   unique per helper, so the parent's own pane and another helper's are never
//!   touched. A tmux that cannot say what a pane started with — the window's
//!   cannot: `format_context` in `zerocode_core::agent_teams` has no
//!   `pane_start_command` — gets the look and no guess. A pane that is new since
//!   the split began is not this helper's for being new: splits run side by
//!   side, and the other's pane stands at the same moment.
//!
//! A tmux that answers nothing at all is asked once, for as long as any
//! question is waited for ([`TMUX_ASK_BOUND`]), and is then left alone: nothing
//! of ours is left running on it, no `kill-pane` is sent to it, and the note
//! stands.

use std::path::Path;
use std::time::Duration;

use super::tmux_ask::{Ask, Capture, Said};
use super::{split_argv, SplitSpec, Tmux, SPLIT_GIVEN_UP_FILE, TMUX_ASK_BOUND};

/// What the look asks `list-panes` to print for each pane: its id, then the
/// command it was started with.
const LOOK_FORMAT: &str = "#{pane_id} #{pane_start_command}";

/// What a given-up split did about a pane tmux may have cut for it.
enum Cleanup {
    /// The look was answered, and no pane stood that was started for the helper.
    Nothing,
    /// Panes stood that were started for the helper; `closed` is whether tmux
    /// confirmed that each was ended.
    Panes { ids: Vec<String>, closed: bool },
    /// tmux did not answer the look.
    Unanswered,
}

/// Cut a pane and start a teammate in it. The pane's id, or the words of why
/// there is none ([`Tmux::split`]).
pub(super) fn split(tmux: &Tmux, spec: &SplitSpec<'_>) -> Result<String, String> {
    let argv = split_argv(spec);
    let words: Vec<&str> = argv.iter().map(String::as_str).collect();
    let capture = Capture {
        output: true,
        reasons: true,
    };
    let ask = Ask::start_within(&tmux.program, &words, capture, None, tmux.split_bound);
    match ask.answer_within(tmux.split_bound) {
        Some(Said::Yes(printed)) if !printed.trim().is_empty() => Ok(printed.trim().to_string()),
        // The server cut a pane and did not say which: one nobody holds the id of.
        Some(Said::Yes(_)) => Err(unnamed_words(&clean_up(tmux, spec))),
        // tmux's own words, as they were. The helper is withdrawn all the same,
        // and nothing is looked for: a pane cut late has no start command to
        // look up yet, and a tmux that refused has cut none.
        Some(Said::No) => {
            withdraw(spec.directory);
            Err(ask
                .refusal()
                .unwrap_or_else(|| "tmux split-window failed".to_string()))
        }
        Some(Said::Nothing) | None => {
            // Only a tmux that ran can have cut a pane.
            if let Some(words) = ask.could_not_run() {
                return Err(words);
            }
            // Ended here, so that nothing of this split still runs on the tmux
            // by the time the look is asked of it.
            drop(ask);
            Err(given_up_words(tmux.split_bound, &clean_up(tmux, spec)))
        }
    }
}

/// Withdraw the helper, then look for the pane that may have been cut for it
/// and close it.
fn clean_up(tmux: &Tmux, spec: &SplitSpec<'_>) -> Cleanup {
    withdraw(spec.directory);
    let look = Ask::start(&tmux.program, &["list-panes", "-F", LOOK_FORMAT], true, None);
    let Some(Said::Yes(listing)) = look.answer_within(TMUX_ASK_BOUND) else {
        return Cleanup::Unanswered;
    };
    let ids = panes_started_for(&listing, spec.agent_id);
    if ids.is_empty() {
        return Cleanup::Nothing;
    }
    // Every one is asked, whatever the last answered.
    let closed = ids.iter().fold(true, |all, id| tmux.kill_pane(id) && all);
    Cleanup::Panes { ids, closed }
}

/// Leave the note a child reads ([`SPLIT_GIVEN_UP_FILE`]). A directory that is
/// not there has no brief for a child to read, and is not made for this. A pane
/// that opens after a failed split finds it before it finds a brief.
fn withdraw(directory: &Path) {
    if directory.is_dir() {
        let _ = std::fs::write(
            directory.join(SPLIT_GIVEN_UP_FILE),
            "the parent gave up waiting for this helper's pane\n",
        );
    }
}

/// The ids of the panes of a `list-panes` listing (id, then start command, a
/// line each) that were started for `agent_id`: a zo teammate whose command
/// names the helper's id as a word of its own.
fn panes_started_for(listing: &str, agent_id: &str) -> Vec<String> {
    listing
        .lines()
        .filter_map(|line| {
            let (pane, command) = line.trim().split_once(char::is_whitespace)?;
            let started_for_it =
                is_pane_id(pane) && command.contains("--teammate") && names_word(command, agent_id);
            started_for_it.then(|| pane.to_string())
        })
        .collect()
}

/// A tmux pane id: `%` and digits.
fn is_pane_id(word: &str) -> bool {
    word.strip_prefix('%')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
}

/// Whether `word` stands in `text` as a word of its own — not the beginning or
/// the end of a longer id (`agent-4` is not in `agent-40`).
fn names_word(text: &str, word: &str) -> bool {
    let joins = |character: char| character.is_ascii_alphanumeric() || character == '-' || character == '_';
    !word.is_empty()
        && text.match_indices(word).any(|(at, found)| {
            let before = text[..at].chars().next_back();
            let after = text[at + found.len()..].chars().next();
            !before.is_some_and(joins) && !after.is_some_and(joins)
        })
}

/// What a person or a model reads when a split was given up. It says what was
/// done about the pane that may have been cut, and nothing it did not do.
fn given_up_words(bound: Duration, cleanup: &Cleanup) -> String {
    let within = format!(
        "tmux did not answer split-window within {}",
        seconds_or_millis(bound)
    );
    let late = "a pane tmux opens for it later closes without working";
    match cleanup {
        Cleanup::Nothing => format!("{within}, so this helper was withdrawn; {late}"),
        Cleanup::Unanswered => {
            format!("{within}, nor the look for a pane it may have cut, so this helper was withdrawn; {late}")
        }
        Cleanup::Panes { ids, closed } => format!(
            "{within}; the pane it had already cut ({}) {}, and this helper was withdrawn",
            ids.join(", "),
            if *closed { "was closed" } else { "was asked to close" }
        ),
    }
}

/// The words of a split that answered with no pane id.
fn unnamed_words(cleanup: &Cleanup) -> String {
    let said = "tmux cut a pane and did not say which";
    match cleanup {
        Cleanup::Panes { ids, closed } => format!(
            "{said}; it was found by its command and {} ({})",
            if *closed { "closed" } else { "asked to close" },
            ids.join(", ")
        ),
        Cleanup::Nothing | Cleanup::Unanswered => format!("{said}; the helper was withdrawn"),
    }
}

/// A bound in the unit it is a whole number of: `12 s`, `400 ms`.
fn seconds_or_millis(bound: Duration) -> String {
    let millis = bound.as_millis();
    if millis.is_multiple_of(1000) {
        format!("{} s", millis / 1000)
    } else {
        format!("{millis} ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pane is the helper's only when its start command is a zo teammate whose
    /// directory ends in the helper's id: not a longer id that begins with it, not
    /// a pane that merely mentions the id, not a line that names no pane.
    #[test]
    fn only_a_pane_started_for_this_helper_is_picked() {
        let listing = "%1 /bin/zsh\n\
                       %7 /opt/zo --teammate /store/agent-other --model m\n\
                       %12 /opt/zo --teammate /store/agent-60\n\
                       %13 /opt/zo --teammate /store/agent-600\n\
                       %14 \"/opt/zo\" \"--teammate\" \"/store/agent-60\" --resume-transcript /store/agent-60/t.jsonl\n\
                       %15 vim agent-60\n\
                       %16\n\
                       not-a-pane /opt/zo --teammate /store/agent-60\n";
        assert_eq!(panes_started_for(listing, "agent-60"), ["%12", "%14"]);
        assert!(panes_started_for(listing, "").is_empty(), "an empty id names every pane");
    }

    #[test]
    fn a_pane_id_is_a_percent_sign_and_digits() {
        assert!(is_pane_id("%12"));
        for not in ["%", "12", "%1a", "pane", ""] {
            assert!(!is_pane_id(not), "{not:?}");
        }
    }

    #[test]
    fn a_bound_is_worded_in_the_unit_it_is_a_whole_number_of() {
        assert_eq!(seconds_or_millis(Duration::from_secs(12)), "12 s");
        assert_eq!(seconds_or_millis(Duration::from_millis(400)), "400 ms");
        assert_eq!(seconds_or_millis(Duration::from_millis(2500)), "2500 ms");
    }

    /// The words claim only what was done: a pane is "closed" when tmux said it
    /// was, and "asked to close" when it was only asked.
    #[test]
    fn the_words_claim_only_what_was_done() {
        let bound = Duration::from_secs(12);
        let nothing = given_up_words(bound, &Cleanup::Nothing);
        assert_eq!(
            nothing,
            "tmux did not answer split-window within 12 s, so this helper was withdrawn; \
             a pane tmux opens for it later closes without working"
        );
        let silent = given_up_words(bound, &Cleanup::Unanswered);
        assert!(silent.contains("nor the look") && !silent.contains("closed"), "{silent}");
        let closed = given_up_words(bound, &Cleanup::Panes { ids: vec!["%12".to_string()], closed: true });
        assert!(closed.contains("(%12) was closed"), "{closed}");
        let asked = given_up_words(bound, &Cleanup::Panes { ids: vec!["%12".to_string()], closed: false });
        assert!(asked.contains("was asked to close") && !asked.contains("was closed"), "{asked}");
        assert!(unnamed_words(&Cleanup::Nothing).starts_with("tmux cut a pane and did not say which"));
    }
}
