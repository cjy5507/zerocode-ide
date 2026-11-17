//! Each conversation's launch limits, kept in the window's data folder so a
//! resume after a restart gives them back (t-112277).
//!
//! The window reopens a pane's conversation with the agent's own resume, and
//! the tool limits its launch put on it were nowhere on that line: a zo pane
//! opened on a short tool list — a review, a helper pane cut from its
//! parent's brief, a list typed into a pane's shell — came back on every
//! tool. The agent says what it was launched under in its `SessionStart`
//! ([`zerocode_core::launch_limits`]); this keeps that beside the
//! conversation, and the resume builder asks here for the words that give it
//! back.
//!
//! The conversation's newest launch decides: a `SessionStart` that says none
//! takes the record away. A record that cannot be read, or that holds limits
//! no command line carries, refuses the resume rather than opening it wider
//! than it was opened ([`resume_words`]). One file per conversation, named by
//! a digest of it — the session id is private restart material and never a
//! file name — so one broken record refuses one conversation and no other.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zerocode_core::ProviderSession;
use zerocode_core::launch_limits::{LaunchLimits, LimitFlags, Reported};

/// The folder of the window's data root the records live in.
const FOLDER: &str = "conversation-limits";

/// Why a conversation whose record is broken is not reopened, said to the
/// person in the pane where it stood ([`unreadable`] adds how to go on).
pub(crate) const UNREADABLE: &str =
    "이 대화가 열릴 때 걸린 도구 제한 기록을 읽지 못해, 제한 없이 열지 않았습니다";

/// [`UNREADABLE`], with the way back that needs no line of the window's:
/// the record named, to take away — the next opening is then on every tool,
/// the person's own choice. A line typed by hand (`resume_line`, which then
/// carries no limits) goes on on every tool too, and its start takes the
/// broken record away.
fn unreadable(path: &Path) -> String {
    format!(
        "{UNREADABLE} — 도구 전체로 이어 가려면 기록 {}을 지운 뒤 다시 여세요",
        path.display()
    )
}

/// One conversation's record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
enum Record {
    /// The limits its newest launch put on it.
    Limits(LaunchLimits),
    /// Its newest launch said limits in a shape no command line carries.
    Unplain,
}

/// The agent's spelling for giving limits back, off its row.
fn flags_of(agent: &str) -> Option<LimitFlags> {
    zerocode_core::agent_capabilities(agent).and_then(|caps| caps.resume.limits)
}

/// Where `agent`'s `session` keeps its record under `root`.
pub(crate) fn record_path(root: &Path, agent: &str, session: &ProviderSession) -> PathBuf {
    let name = zerocode_core::orchestration::receipt_actor(agent, session.key, &session.id)
        .replace(':', "-");
    root.join(FOLDER).join(format!("{name}.json"))
}

/// Keep what a `SessionStart` of `agent` said about `session`'s limits —
/// written whole or not at all, and taken away by a report of none. An agent
/// with no measured spelling keeps nothing.
///
/// # Errors
/// The record could not be written or taken away.
pub(crate) fn keep(
    root: &Path,
    agent: &str,
    session: &ProviderSession,
    reported: &Reported,
) -> io::Result<()> {
    if flags_of(agent).is_none() {
        return Ok(());
    }
    let path = record_path(root, agent, session);
    let record = match reported {
        Reported::None => {
            return match std::fs::remove_file(&path) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                gone => gone,
            };
        }
        Reported::Limits(limits) => Record::Limits(limits.clone()),
        Reported::Unplain => Record::Unplain,
    };
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let bytes = serde_json::to_vec(&record).map_err(io::Error::other)?;
    zerocode_core::atomic_file::write_atomically_sealed(&path, &bytes, Some(0o600), |_| Ok(()))
}

/// The words a resume of `agent`'s `session` gives its limits back with:
/// none for a conversation that keeps no record, and `Err` with the
/// person's sentence ([`unreadable`]) for one whose record cannot be given
/// back — the resume is refused, never opened wider.
///
/// # Errors
/// The record is there and cannot be read, does not parse, or holds limits no
/// command line carries.
pub(crate) fn resume_words(
    root: &Path,
    agent: &str,
    session: &ProviderSession,
) -> Result<Vec<String>, String> {
    let Some(flags) = flags_of(agent) else {
        return Ok(Vec::new());
    };
    let path = record_path(root, agent, session);
    let raw = match std::fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(unreadable(&path)),
    };
    match serde_json::from_slice::<Record>(&raw) {
        Ok(Record::Limits(limits)) => limits.words(flags).ok_or_else(|| unreadable(&path)),
        Ok(Record::Unplain) | Err(_) => Err(unreadable(&path)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::SessionKey;

    fn session(id: &str) -> ProviderSession {
        ProviderSession {
            key: SessionKey::SessionId,
            id: id.to_string(),
            transcript_path: None,
        }
    }

    fn limits(names: &[&str], no_spawn: bool) -> LaunchLimits {
        LaunchLimits {
            allowed_tools: Some(names.iter().map(|name| (*name).to_string()).collect()),
            no_spawn,
        }
    }

    /// What a zo `SessionStart` said is what the next resume gives back, and
    /// a later one that says none takes it away: the newest launch decides.
    /// The file is named by a digest, never by the session id.
    #[test]
    fn a_conversation_keeps_the_limits_its_newest_launch_said() {
        let root = tempfile::tempdir().expect("a data root");
        let held = session("session-1794870000000-0");
        assert_eq!(resume_words(root.path(), "zo", &held), Ok(Vec::new()));

        keep(
            root.path(),
            "zo",
            &held,
            &Reported::Limits(limits(&["read_file", "grep_search"], true)),
        )
        .expect("kept");
        assert_eq!(
            resume_words(root.path(), "zo", &held),
            Ok(vec![
                "--allowed-tools".to_string(),
                "read_file,grep_search".to_string(),
                "--no-spawn".to_string(),
            ])
        );
        let files: Vec<String> = std::fs::read_dir(root.path().join(FOLDER))
            .expect("the folder")
            .map(|entry| {
                entry
                    .expect("an entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(files.len(), 1, "{files:?}");
        assert!(
            !files[0].contains("1794870000000") && !files[0].contains(':'),
            "the record is named by a digest a file system takes: {files:?}"
        );
        assert_eq!(
            resume_words(root.path(), "zo", &session("session-1794870000000-1")),
            Ok(Vec::new()),
            "another conversation keeps nothing of this one's"
        );

        keep(root.path(), "zo", &held, &Reported::None).expect("taken away");
        assert_eq!(resume_words(root.path(), "zo", &held), Ok(Vec::new()));
        keep(root.path(), "zo", &held, &Reported::None).expect("nothing to take away is no error");
    }

    /// A record that cannot be given back refuses the resume with the
    /// person's sentence, which names the record to take away — said
    /// unplain, cut short, or holding words no command line carries. Never
    /// the plain resume, which would open the conversation on every tool.
    #[test]
    fn a_record_that_cannot_be_given_back_refuses_the_resume() {
        let root = tempfile::tempdir().expect("a data root");
        let held = session("session-1794870000000-2");
        let path = record_path(root.path(), "zo", &held);
        let refused: Result<Vec<String>, String> = Err(unreadable(&path));
        assert!(
            refused
                .as_ref()
                .is_err_and(|said: &String| said.starts_with(UNREADABLE)
                    && said.contains(&path.display().to_string())),
            "{refused:?}"
        );
        keep(root.path(), "zo", &held, &Reported::Unplain).expect("kept");
        assert_eq!(resume_words(root.path(), "zo", &held), refused);

        let broken: [&[u8]; 3] = [
            b"{\"limits\":{\"allowed_tools\":[\"read_fi",
            b"",
            b"{\"limits\":{\"allowed_tools\":[\"--x\"]}}",
        ];
        for broken in broken {
            std::fs::write(&path, broken).expect("a broken record");
            assert_eq!(
                resume_words(root.path(), "zo", &held),
                refused,
                "{}",
                String::from_utf8_lossy(broken)
            );
        }
    }

    /// An agent whose row measured no spelling keeps nothing and is given
    /// nothing back: its resumes are the ones they always were.
    #[test]
    fn an_agent_with_no_spelling_keeps_no_limits() {
        let root = tempfile::tempdir().expect("a data root");
        let held = session("4a1c2b3d-0000-4000-8000-000000000000");
        keep(
            root.path(),
            "claude",
            &held,
            &Reported::Limits(limits(&["Read"], true)),
        )
        .expect("nothing kept");
        assert!(!root.path().join(FOLDER).exists());
        assert_eq!(resume_words(root.path(), "claude", &held), Ok(Vec::new()));
    }
}
