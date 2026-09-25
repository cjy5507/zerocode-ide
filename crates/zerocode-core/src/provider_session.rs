//! The agent's OWN session id, and how to resume it.
//!
//! A hook event carries more than a state: it carries the id the agent's vendor
//! knows this conversation by. That is the fact that turns a pane from "a
//! terminal that had an agent in it" into "this conversation" — a window can
//! close a tab and reopen the SAME session, because the id is the vendor's and
//! outlives the process.
//!
//! Measured off Orca 1.4.169 (`extractAgentProviderSession` and
//! `getAgentResumeArgv`, chunks/daemon-ready-identity-CjFvutLo.js:130-258).
//!
//! Three things in that measurement are load-bearing:
//!
//! 1. **The key differs from the id.** Antigravity's is a `conversation_id`, not
//!    a `session_id`, and resume refuses a mismatch — a resume built from the
//!    wrong key would hand an agent an id it will not recognize.
//! 2. **The field name differs per vendor** (`session_id`, `sessionID`,
//!    `sessionId`, `conversationId`), sometimes with two spellings for one
//!    agent. A reader that knew one of them silently finds no session.
//! 3. **Five agents have none at all**, and that is an answer rather than a gap:
//!    amp, cursor, command-code, copilot and hermes do not put a resumable id in
//!    their events. Offering "resume" for them would be a control that fails.
//!
//! And one thing that is ours: **the id goes on a command line**, so it is
//! validated before it is believed. Orca's own bounds — 512 characters, no
//! control bytes, and never leading `-` — are exactly the argv guards: a value
//! starting with a dash becomes a FLAG rather than an id.

use serde::{Deserialize, Serialize};

use crate::agent::AgentKind;

/// Orca's own ceiling. A session id is a vendor's identifier, not a document.
pub const SESSION_ID_MAX: usize = 512;

/// Which kind of identifier this is. The two Orca distinguishes, because resume
/// refuses a mismatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKey {
    SessionId,
    ConversationId,
}

/// One agent's own handle on a conversation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderSession {
    pub key: SessionKey,
    pub id: String,
    /// Where the agent is writing this conversation down, when it says. Carried
    /// because one agent resumes BY the transcript path rather than by the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
}

impl ProviderSession {
    /// This report, keeping what `held` already knew about where the same
    /// conversation is being written down.
    ///
    /// A report replaces what came before it — a person who quits one agent in
    /// a terminal and starts another must not leave the window remembering the
    /// old one. But most events carry only the session's name: zo puts a
    /// transcript path in `SessionStart` and `Stop` and in nothing else, so
    /// replacing wholesale threw the path away on the person's first message
    /// and left the conversation view with nothing to read. Same kind of
    /// identifier and same id is the same conversation, still writing to the
    /// same file; anything else is a different one and carries nothing over.
    #[must_use]
    pub fn carrying_forward(&self, held: Option<&Self>) -> Self {
        let mut carried = self.clone();
        if carried.transcript_path.is_some() {
            return carried;
        }
        if let Some(held) = held
            && held.key == self.key
            && held.id == self.id
        {
            carried.transcript_path = held.transcript_path.clone();
        }
        carried
    }
}

impl std::fmt::Debug for ProviderSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Both strings are private restart material. Keep the identifier kind
        // and byte sizes for diagnosis, but make the leaf safe on its own so a
        // derived Debug on WorkerRow or LedgerProjection cannot disclose them.
        formatter
            .debug_struct("ProviderSession")
            .field("key", &self.key)
            .field("id_bytes", &self.id.len())
            .field(
                "transcript_bytes",
                &self.transcript_path.as_ref().map(String::len),
            )
            .finish()
    }
}

/// Is this string usable as a session id?
///
/// Every clause is a guard rather than a preference — this value reaches an argv:
///
/// - empty is not an id;
/// - over [`SESSION_ID_MAX`] is not an id, it is a payload someone put in the
///   wrong field;
/// - a leading `-` would be read by the agent's own parser as a FLAG;
/// - control bytes have no business in an identifier and every business in an
///   injection attempt.
pub fn is_usable_session_id(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty()
        && trimmed.len() <= SESSION_ID_MAX
        && !trimmed.starts_with('-')
        && !trimmed.chars().any(|c| (c as u32) <= 31 || c as u32 == 127)
}

/// One conversation, spelled one way — the identity a wake is judged by.
///
/// Every door that re-enters a conversation (a restored leaf, a sidebar row, a
/// tab's menu) names it with the record it holds, and the judge that keeps one
/// process on one transcript compares that record with what the live panes
/// reported. Compared raw, two spellings of one conversation are two
/// conversations and the second process is let through. So the id is spelled
/// the way every payload reader already spells it — trimmed, and only when it
/// is an id at all ([`is_usable_session_id`]) — and the agent and the key
/// travel with it, because resume refuses a mismatch of either
/// ([`resume_argv`]). The transcript path is no part of it: two records of one
/// conversation can disagree about whether they know its file.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ConversationKey {
    agent: AgentKind,
    key: SessionKey,
    id: String,
}

impl std::fmt::Debug for ConversationKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The id is the same private restart material `ProviderSession` keeps
        // out of its own Debug.
        formatter
            .debug_struct("ConversationKey")
            .field("agent", &self.agent)
            .field("key", &self.key)
            .field("id_bytes", &self.id.len())
            .finish()
    }
}

/// The conversation `session` names for the agent `slug`, or `None` when the
/// slug is no agent this crate drives or the id is not an id.
#[must_use]
pub fn conversation_key(slug: &str, session: &ProviderSession) -> Option<ConversationKey> {
    let agent = AgentKind::from_slug(slug)?;
    let id = session.id.trim();
    is_usable_session_id(id).then(|| ConversationKey {
        agent,
        key: session.key,
        id: id.to_string(),
    })
}

fn read_id(record: &serde_json::Map<String, serde_json::Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        let Some(text) = record.get(*key).and_then(serde_json::Value::as_str) else {
            continue;
        };
        if is_usable_session_id(text) {
            return Some(text.trim().to_string());
        }
    }
    None
}

/// Which field(s) this agent puts its session id in, and under which key —
/// `None` for the agents that publish no resumable id at all.
///
/// Written as a table returning the SPELLINGS rather than as per-agent code,
/// because that is what the measurement is: one shape, eleven vendors' names
/// for it.
/// Whether this agent puts a resumable id in its events at all.
///
/// The five that do not are named in this module's header, and the difference
/// matters beyond resume: an agent that WILL report a session must not be given
/// a stand-in identity in the meantime, because the stand-in would change the
/// moment the real one arrives and everything filed under it would be
/// stranded — the same restart bug, one launch smaller.
#[must_use]
pub const fn publishes_session(agent: AgentKind) -> bool {
    session_fields(agent).is_some()
}

/// Which key this agent files its conversations under — the table's answer
/// for a record this window builds itself, such as a row of an agent's own
/// session store, rather than reads off a payload.
#[must_use]
pub const fn session_key_of(agent: AgentKind) -> Option<SessionKey> {
    match session_fields(agent) {
        Some((key, _)) => Some(key),
        None => None,
    }
}

/// The same question, asked with a catalog slug — and answerable for every
/// slug the catalog can summon, not only the fourteen that have hooks.
///
/// The distinction this exists to draw: a slug OUTSIDE [`AgentKind`] has no
/// hook script, so no session is ever coming from it — and "no session yet"
/// and "no session ever" earn opposite treatment. The first waits (a stand-in
/// identity would be stranded the moment the real one arrived); the second
/// takes the launch-token identity NOW, because the launch is all such an
/// agent will ever have. Answering the enum's absence as "wait" was the road
/// by which two dozen summonable vendors could be seated, briefed, and then
/// refused the very `worker_done` their briefing told them to run — forever.
#[must_use]
pub fn session_will_come(slug: &str) -> bool {
    AgentKind::from_slug(slug).is_some_and(publishes_session)
}

const fn session_fields(agent: AgentKind) -> Option<(SessionKey, &'static [&'static str])> {
    match agent {
        // Two spellings for one agent, both measured. A reader that knew one
        // would silently find no session for the other.
        AgentKind::Grok | AgentKind::Devin => {
            Some((SessionKey::SessionId, &["sessionId", "session_id"]))
        }
        AgentKind::Claude
        | AgentKind::Codex
        | AgentKind::Droid
        | AgentKind::Kimi
        // This project's own harness POSTs claude-shaped hooks natively
        // (zo-ide/crates/zo-ide/src/ide/reporter.rs): `SessionStart` and
        // `Stop` carry `session_id` and `transcript_path`.
        | AgentKind::Zo => Some((SessionKey::SessionId, &["session_id"])),
        // The one that is NOT a session id. Resume refuses the mismatch, which
        // is the whole reason the key travels beside the value.
        AgentKind::Antigravity => Some((SessionKey::ConversationId, &["conversationId"])),
        AgentKind::Opencode => Some((SessionKey::SessionId, &["sessionID"])),
        // Amp, Cursor, CommandCode and Copilot publish no resumable id.
        AgentKind::Amp | AgentKind::Cursor | AgentKind::CommandCode | AgentKind::Copilot => None,
    }
}

/// The transcript path, when the payload names one. Two spellings, measured.
fn read_transcript(record: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    for key in ["transcript_path", "transcriptPath"] {
        let Some(text) = record.get(key).and_then(serde_json::Value::as_str) else {
            continue;
        };
        let trimmed = text.trim();
        // Same control-byte guard: this one reaches a command line too.
        if !trimmed.is_empty() && !trimmed.chars().any(|c| (c as u32) <= 31 || c as u32 == 127) {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Pull this agent's own session out of a hook payload.
///
/// `None` when the agent publishes none, when the payload is not an object, or
/// when what is in the field would not be safe on a command line.
pub fn session_in_payload(agent: AgentKind, payload: &str) -> Option<ProviderSession> {
    session_in_parsed(agent, &crate::payload::HookPayload::of(payload))
}

/// [`session_in_payload`], for a caller that already paid the parse.
#[must_use]
pub fn session_in_parsed(
    agent: AgentKind,
    payload: &crate::payload::HookPayload<'_>,
) -> Option<ProviderSession> {
    let (key, fields) = session_fields(agent)?;
    let record = payload.tree()?.as_object()?;
    let id = read_id(record, fields)?;
    Some(ProviderSession {
        key,
        id,
        transcript_path: read_transcript(record),
    })
}

/// Claude's launch args with any resume/continue selector spliced out.
///
/// Orca's cold-restore guard (`buildClaudeResumeLaunchCommand`, #12982):
/// exactly ONE authoritative selector may reach the command line, and it is
/// the one [`resume_argv`] writes. A `--continue` or a stale `--resume abc`
/// somebody saved into their launch args would otherwise ride beside it, and
/// claude answers two selectors by picking one of them — not necessarily the
/// session this door was asked to reopen.
///
/// The selectors are Claude's row ([`CLAUDE_RESUME_SELECTORS`]) and the
/// splice is the capability table's; a resume door asks its agent's row
/// (`Resume::launch_args_without_selectors`) rather than this name.
///
/// [`CLAUDE_RESUME_SELECTORS`]: crate::capabilities::CLAUDE_RESUME_SELECTORS
#[must_use]
pub fn claude_args_without_selectors(args: &[String]) -> Vec<String> {
    crate::capabilities::args_without_selectors(crate::capabilities::CLAUDE_RESUME_SELECTORS, args)
}

/// The word Codex re-enters a thread by — `codex resume <id>` — named once,
/// for [`resume_argv`] and for the window road that must know a launch line
/// is a resume before it rewrites one (t-7812: a remote app-server route put
/// in front of it is refused by Codex).
pub const CODEX_RESUME_SUBCOMMAND: &str = "resume";

/// The command line that resumes this session, or `None` when it cannot be
/// built.
///
/// The key is CHECKED, not assumed: a conversation id handed to `--resume` is an
/// id the agent will reject, and an agent with no resume flag gets no command
/// rather than a guessed one.
pub fn resume_argv(agent: AgentKind, session: &ProviderSession) -> Option<Vec<String>> {
    if session.key != SessionKey::SessionId && agent != AgentKind::Antigravity {
        return None;
    }
    // Re-validated at the point of use. The value came off a payload, may have
    // been round-tripped through disk since, and this is the line where it stops
    // being data and starts being argv.
    if !is_usable_session_id(&session.id) {
        return None;
    }
    let id = session.id.clone();
    let argv = match agent {
        AgentKind::Claude => vec!["claude", "--resume"],
        AgentKind::Codex => vec!["codex", CODEX_RESUME_SUBCOMMAND],
        AgentKind::Droid => vec!["droid", "--resume"],
        AgentKind::Grok => vec!["grok", "--resume"],
        AgentKind::Devin => vec!["devin", "--resume"],
        AgentKind::Opencode => vec!["opencode", "--session"],
        // Ids look like `session-<millis>-<n>`, which the usable-id rule
        // already admits.
        AgentKind::Zo => vec!["zo", "--resume"],
        AgentKind::Antigravity => {
            if session.key != SessionKey::ConversationId {
                return None;
            }
            vec!["agy", "--conversation"]
        }
        // Kimi reports a session but Orca offers no resume for it, and this
        // window does not invent one — a flag we guessed at is a launch that
        // fails in a pane.
        _ => return None,
    };
    let mut argv: Vec<String> = argv.into_iter().map(str::to_string).collect();
    argv.push(id);
    Some(argv)
}

/// Whether this record names a conversation that was never written down.
///
/// Claude's `--resume <id>` opens the transcript its SessionStart hook named,
/// and that file appears with the first message: a pane that started and
/// never spoke carries an id with no file behind it. Resuming it ends in "No
/// conversation found with session ID" and exit 1 — and the window, which
/// keeps a record through every wake so a bad morning cannot erase it,
/// re-resumed one such pane at thirteen restarts (09-11..09-12, each pane
/// ending a second after it came back). `absent` answers for the named path,
/// and only a definite absence counts: a record with no path is resumed as
/// before. The other agents find their conversations by their own rules and
/// are not judged here.
#[must_use]
pub fn conversation_never_written(
    agent: AgentKind,
    session: &ProviderSession,
    absent: impl FnOnce(&str) -> bool,
) -> bool {
    agent == AgentKind::Claude
        && session.key == SessionKey::SessionId
        && session.transcript_path.as_deref().is_some_and(absent)
}

/// The resume spelling that also hands the agent its next instruction, for
/// the vendors whose CLI takes a positional prompt beside the selector.
///
/// This is the difference between reopening a conversation and CONTINUING
/// one: a window restart kills the process mid-turn, `--resume` alone brings
/// the transcript back over an empty composer, and the person has to type
/// "go on" at every pane they did not stop. The nudge rides the argv so the
/// cut turn restarts the moment the pane is up. `None` says this vendor has
/// no spelling for it that we know to be true — the caller resumes plain,
/// which is exactly the behaviour before this road existed. A guessed flag
/// is a launch that fails in a pane, the same rule the table above keeps.
pub fn resume_argv_continuing(
    agent: AgentKind,
    session: &ProviderSession,
    nudge: &str,
) -> Option<Vec<String>> {
    if nudge.is_empty() {
        return None;
    }
    let road = crate::agent_spec(agent.slug())?.resume_nudge;
    if road != crate::NudgeRoad::Argv {
        return None;
    }
    let mut argv = resume_argv(agent, session)?;
    argv.push(nudge.to_string());
    Some(argv)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only two of zo's events carry a transcript path — `SessionStart` and
    /// `Stop` (`zo-ide/.../ide/reporter.rs`). Every other one it posts names
    /// the session and nothing else, so a pane whose report simply replaced
    /// the last went blank the moment the person typed: the window had the
    /// path, then a `UserPromptSubmit` took it away and `pane_log` had
    /// nothing to read. Claude never showed it, because its hooks carry the
    /// path every time.
    #[test]
    fn a_later_report_of_the_same_session_does_not_take_its_transcript_away() {
        let session = |path: Option<&str>| ProviderSession {
            key: SessionKey::SessionId,
            id: "0f5a1b6c".to_string(),
            transcript_path: path.map(str::to_string),
        };
        let written = "/Users/dev/.zo/projects/p/sessions/session-1.jsonl";

        let started = session(Some(written));
        let typed = session(None);
        assert_eq!(
            typed
                .carrying_forward(Some(&started))
                .transcript_path
                .as_deref(),
            Some(written),
            "the same conversation kept writing to the same file"
        );

        // A report that names a path is the newer truth about where it writes.
        let moved = session(Some("/tmp/elsewhere.jsonl"));
        assert_eq!(
            moved
                .carrying_forward(Some(&started))
                .transcript_path
                .as_deref(),
            Some("/tmp/elsewhere.jsonl")
        );

        // A different conversation is a different conversation: nothing of the
        // old one may be carried into it, or a pane whose agent was swapped
        // would read the departed one's file.
        let other = ProviderSession {
            key: SessionKey::SessionId,
            id: "99999999".to_string(),
            transcript_path: None,
        };
        assert_eq!(other.carrying_forward(Some(&started)).transcript_path, None);

        // Different kinds of identifier are never the same conversation.
        let by_path = ProviderSession {
            key: SessionKey::ConversationId,
            id: "0f5a1b6c".to_string(),
            transcript_path: None,
        };
        assert_eq!(
            by_path.carrying_forward(Some(&started)).transcript_path,
            None
        );

        // And with nothing held, a report stands as it came.
        assert_eq!(typed.carrying_forward(None), typed);
    }

    #[test]
    fn a_claude_pane_that_never_spoke_has_no_conversation_to_resume() {
        let named = "/c/projects/-Users-dev/cd026e19-e32b-42f3-875c-ca5fc83a4317.jsonl";
        let record = |key, path: Option<&str>| ProviderSession {
            key,
            id: "cd026e19-e32b-42f3-875c-ca5fc83a4317".to_string(),
            transcript_path: path.map(str::to_string),
        };
        let claude = record(SessionKey::SessionId, Some(named));
        let mut asked = None;
        assert!(
            conversation_never_written(AgentKind::Claude, &claude, |path| {
                asked = Some(path.to_string());
                true
            }),
            "an id with no transcript behind it is nothing to resume"
        );
        assert_eq!(
            asked.as_deref(),
            Some(named),
            "the probe is asked about the record's own file"
        );
        assert!(
            !conversation_never_written(AgentKind::Claude, &claude, |_| false),
            "a written conversation is resumed"
        );
        assert!(
            !conversation_never_written(
                AgentKind::Claude,
                &record(SessionKey::SessionId, None),
                |_| true
            ),
            "a record that names no file is not evidence of anything"
        );
        assert!(
            !conversation_never_written(AgentKind::Codex, &claude, |_| true),
            "codex finds its rollouts by its own rule"
        );
        assert!(
            !conversation_never_written(
                AgentKind::Claude,
                &record(SessionKey::ConversationId, Some(named)),
                |_| true
            ),
            "only a session id opens a claude transcript"
        );
    }

    /// The judge that keeps one process on one transcript asks this key of the
    /// wake's record and of every live pane's report. A second spelling of one
    /// conversation must meet the first, and nothing that resume itself would
    /// tell apart may.
    #[test]
    fn one_conversation_is_one_key_however_its_records_spell_it() {
        let record = |key, id: &str, path: Option<&str>| ProviderSession {
            key,
            id: id.to_string(),
            transcript_path: path.map(str::to_string),
        };
        let id = "session-1789537852228-0";
        let durable = conversation_key(
            AgentKind::Zo.slug(),
            &record(
                SessionKey::SessionId,
                id,
                Some("/Users/dev/.zo/projects/Users-dev/sessions/session-1789537852228-0.jsonl"),
            ),
        )
        .expect("a zo session id names a conversation");
        assert_eq!(
            conversation_key(
                AgentKind::Zo.slug(),
                &record(SessionKey::SessionId, &format!(" {id}\n"), None)
            ),
            Some(durable.clone()),
            "the spacing a record carries and whether it knows the file do not \
             make a second conversation"
        );
        assert_ne!(
            conversation_key(
                AgentKind::Claude.slug(),
                &record(SessionKey::SessionId, id, None)
            ),
            Some(durable.clone()),
            "another agent's conversation is another conversation"
        );
        assert_ne!(
            conversation_key(
                AgentKind::Zo.slug(),
                &record(SessionKey::ConversationId, id, None)
            ),
            Some(durable.clone()),
            "resume refuses a key mismatch, so the key keeps them apart too"
        );
        assert_eq!(
            conversation_key(
                AgentKind::Zo.slug(),
                &record(SessionKey::SessionId, "--resume", None)
            ),
            None,
            "a flag is not an id"
        );
        assert_eq!(
            conversation_key("no-such-agent", &record(SessionKey::SessionId, id, None)),
            None,
            "a slug no agent answers to names nothing"
        );
        let said = format!("{durable:?}");
        assert!(
            !said.contains(id),
            "the key printed the private id it carries: {said}"
        );
    }

    #[test]
    fn a_restart_nudge_takes_the_road_in_the_agent_catalog() {
        let session = ProviderSession {
            key: SessionKey::SessionId,
            id: "session-restart".to_string(),
            transcript_path: None,
        };
        let nudge = "continue the interrupted turn";

        assert_eq!(
            crate::agent_spec(AgentKind::Codex.slug())
                .expect("codex spec")
                .resume_nudge,
            crate::NudgeRoad::Composer,
        );
        assert_eq!(
            resume_argv_continuing(AgentKind::Codex, &session, nudge),
            None,
            "Codex does not submit a positional resume prompt",
        );
        assert_eq!(
            crate::agent_spec(AgentKind::Claude.slug())
                .expect("claude spec")
                .resume_nudge,
            crate::NudgeRoad::Argv,
        );
        let claude = resume_argv_continuing(AgentKind::Claude, &session, nudge)
            .expect("Claude accepts a positional resume prompt");
        assert_eq!(claude.last().map(String::as_str), Some(nudge));
    }

    /// Which agents will report a session, said out loud.
    ///
    /// The receipt road hangs a decision on this: an agent that WILL report one
    /// gets no stand-in identity in the meantime, because a stand-in would
    /// change the moment the real session arrived and strand everything filed
    /// under it. An agent that never reports one gets the launch instead,
    /// because the launch is all it will ever have.
    ///
    /// Both halves are named here. A change that quietly moved an agent from
    /// one column to the other would change who is refused while they start.
    #[test]
    fn the_agents_that_publish_a_session_are_the_ones_that_can_be_resumed() {
        for reporting in [AgentKind::Claude, AgentKind::Codex] {
            assert!(
                publishes_session(reporting),
                "{} stopped publishing a session, so it would be given a \
                 stand-in identity that changes under it",
                reporting.slug()
            );
        }
        for silent in [AgentKind::Amp, AgentKind::Cursor] {
            assert!(
                !publishes_session(silent),
                "{} is now believed to publish a session, so it would be \
                 refused every mutation while it waits for one that never comes",
                silent.slug()
            );
            assert_eq!(session_key_of(silent), None);
        }
        // A record built from a store rather than read off a payload files its
        // conversation under the same key the payload reader would.
        let stored = |agent: AgentKind, body: &str| {
            session_in_payload(agent, body).map(|session| session.key)
        };
        assert_eq!(
            session_key_of(AgentKind::Claude),
            stored(AgentKind::Claude, r#"{"session_id":"abc-123"}"#)
        );
        assert_eq!(
            session_key_of(AgentKind::Antigravity),
            stored(AgentKind::Antigravity, r#"{"conversationId":"c-1"}"#)
        );
    }

    #[test]
    fn each_vendors_own_spelling_is_read_and_the_key_travels_with_it() {
        let claude = session_in_payload(
            AgentKind::Claude,
            r#"{"session_id":"abc-123","transcript_path":"/t/a.jsonl"}"#,
        )
        .expect("claude session");
        assert_eq!(claude.key, SessionKey::SessionId);
        assert_eq!(claude.id, "abc-123");
        assert_eq!(claude.transcript_path.as_deref(), Some("/t/a.jsonl"));

        // OpenCode's capital D. A reader that knew only `session_id` would
        // silently find nothing here.
        let opencode = session_in_payload(AgentKind::Opencode, r#"{"sessionID":"ses_9"}"#)
            .expect("opencode session");
        assert_eq!(opencode.id, "ses_9");

        // Both of grok's spellings.
        for body in [r#"{"sessionId":"g1"}"#, r#"{"session_id":"g1"}"#] {
            assert_eq!(
                session_in_payload(AgentKind::Grok, body).map(|one| one.id),
                Some("g1".to_string()),
                "{body}"
            );
        }

        // The one that is not a session id at all.
        let antigravity =
            session_in_payload(AgentKind::Antigravity, r#"{"conversationId":"conv-1"}"#)
                .expect("antigravity session");
        assert_eq!(antigravity.key, SessionKey::ConversationId);
    }

    /// Five agents publish no resumable id. That is an answer, not a gap —
    /// offering resume for them would be a control that fails.
    #[test]
    fn an_agent_that_publishes_no_session_reports_none() {
        for silent in [
            AgentKind::Amp,
            AgentKind::Cursor,
            AgentKind::CommandCode,
            AgentKind::Copilot,
        ] {
            assert!(
                session_in_payload(silent, r#"{"session_id":"x"}"#).is_none(),
                "{silent:?} invented a session it does not publish"
            );
        }
    }

    /// The id reaches an argv, so every guard is a guard rather than a taste.
    #[test]
    fn an_id_that_could_not_be_an_argument_is_refused() {
        // A leading dash is read as a FLAG by the agent's own parser.
        assert!(!is_usable_session_id("--dangerously-skip-permissions"));
        // Control bytes have no business in an identifier.
        assert!(!is_usable_session_id("abc\ndef"));
        assert!(!is_usable_session_id("abc\0def"));
        // Empty, whitespace, and a payload in the wrong field.
        assert!(!is_usable_session_id(""));
        assert!(!is_usable_session_id("   "));
        assert!(!is_usable_session_id(&"x".repeat(SESSION_ID_MAX + 1)));
        // And the ordinary shapes pass.
        assert!(is_usable_session_id("0198c0d9-7000-8000-abcdef012345"));
        assert!(is_usable_session_id("ses_01JABCDEF"));

        // Refused at extraction, not merely at use.
        assert!(session_in_payload(AgentKind::Claude, r#"{"session_id":"-rf"}"#).is_none());
        assert!(session_in_payload(AgentKind::Claude, "not json").is_none());
        assert!(session_in_payload(AgentKind::Claude, r#"{"session_id":42}"#).is_none());
    }

    /// A provider session is nested inside the authority's deny-unknown worker
    /// row. The nested value has to be just as strict: otherwise an older
    /// window accepts a field written by a newer one, drops it, and writes the
    /// shortened conversation record back on its next ledger mutation.
    #[test]
    fn a_persisted_session_refuses_fields_this_build_cannot_preserve() {
        let future = r#"{
            "key":"session_id",
            "id":"session-from-the-future",
            "transcript_path":"/tmp/session.jsonl",
            "resume_token":"newer-build-only"
        }"#;
        assert!(
            serde_json::from_str::<ProviderSession>(future).is_err(),
            "the nested durable session silently discarded a newer field"
        );
    }

    #[test]
    fn resume_uses_each_agents_own_flag_and_refuses_a_key_mismatch() {
        let session = ProviderSession {
            key: SessionKey::SessionId,
            id: "s-1".into(),
            transcript_path: None,
        };
        assert_eq!(
            resume_argv(AgentKind::Claude, &session),
            Some(vec!["claude".into(), "--resume".into(), "s-1".into()])
        );
        // Codex's is a SUBCOMMAND, not a flag.
        assert_eq!(
            resume_argv(AgentKind::Codex, &session),
            Some(vec!["codex".into(), "resume".into(), "s-1".into()])
        );
        // Antigravity's binary is `agy` and its flag names conversations.
        let conversation = ProviderSession {
            key: SessionKey::ConversationId,
            id: "c-1".into(),
            transcript_path: None,
        };
        assert_eq!(
            resume_argv(AgentKind::Antigravity, &conversation),
            Some(vec!["agy".into(), "--conversation".into(), "c-1".into()])
        );
        // A key mismatch in either direction builds nothing: an id the agent
        // will reject is worse than no offer.
        assert!(resume_argv(AgentKind::Antigravity, &session).is_none());
        assert!(resume_argv(AgentKind::Claude, &conversation).is_none());
        // And an agent with no resume gets none rather than a guessed flag.
        assert!(resume_argv(AgentKind::Kimi, &session).is_none());
        assert!(resume_argv(AgentKind::Amp, &session).is_none());

        // Re-validated at the point of use: the value may have been round
        // tripped through disk since it was extracted.
        let hostile = ProviderSession {
            key: SessionKey::SessionId,
            id: "-rf".into(),
            transcript_path: None,
        };
        assert!(resume_argv(AgentKind::Claude, &hostile).is_none());
    }

    /// A mid-turn resume carries its continue nudge — only for the vendors
    /// whose CLI takes a positional prompt beside the selector, and never as
    /// a guessed flag. Everyone else answers `None` and the caller falls
    /// back to the plain resume, which is the pre-existing behaviour.
    #[test]
    fn a_continuing_resume_rides_the_nudge_only_where_the_vendor_takes_one() {
        let session = ProviderSession {
            key: SessionKey::SessionId,
            id: "s-1".into(),
            transcript_path: None,
        };
        assert_eq!(
            resume_argv_continuing(AgentKind::Claude, &session, "go on"),
            Some(vec![
                "claude".into(),
                "--resume".into(),
                "s-1".into(),
                "go on".into()
            ])
        );
        assert_eq!(
            resume_argv_continuing(AgentKind::Codex, &session, "go on"),
            None,
            "Codex restores the session here and takes the nudge through its composer"
        );
        // An empty nudge is a request for nothing.
        assert!(resume_argv_continuing(AgentKind::Claude, &session, "").is_none());
        // And the session is still validated underneath.
        let hostile = ProviderSession {
            key: SessionKey::SessionId,
            id: "-rf".into(),
            transcript_path: None,
        };
        assert!(resume_argv_continuing(AgentKind::Claude, &hostile, "go on").is_none());
    }

    /// One authoritative selector (#12982): the base a resume opens with is
    /// scrubbed of the selectors a person may have saved into their launch
    /// args, and of nothing else.
    #[test]
    fn a_resumes_base_keeps_its_flags_and_loses_its_selectors() {
        let words =
            |line: &str| -> Vec<String> { line.split_whitespace().map(str::to_string).collect() };
        // The yolo default rides through untouched.
        assert_eq!(
            claude_args_without_selectors(&words("--dangerously-skip-permissions")),
            words("--dangerously-skip-permissions")
        );
        // A stale selector loses its id too; a flag after one is not its id.
        assert_eq!(
            claude_args_without_selectors(&words("--resume abc --model opus")),
            words("--model opus")
        );
        assert_eq!(
            claude_args_without_selectors(&words("--resume --model opus")),
            words("--model opus")
        );
        // Bare continues and joined spellings go the same way.
        assert_eq!(
            claude_args_without_selectors(&words("--continue -c -r --resume=abc --verbose")),
            words("--verbose")
        );
    }

    /// Every slug the catalog can summon gets an identity road: the ones a
    /// session will come from wait for it, and every other — hook vendors
    /// with no resumable id and the whole catalog beyond the hook enum —
    /// takes the launch identity now. "No session YET" and "no session EVER"
    /// must never share an answer, because the answer decides whether a
    /// seated worker can ever say `worker_done`.
    #[test]
    fn every_summonable_slug_has_an_identity_road() {
        let mut waiting = 0;
        let mut launch_keyed = 0;
        for spec in &crate::agent::AGENT_SPECS {
            match session_will_come(spec.id) {
                true => {
                    // Only a hook vendor that really publishes may make a
                    // caller wait — for everyone else "wait" is "forever".
                    let kind = AgentKind::from_slug(spec.id)
                        .expect("a waiting slug outside the hook enum waits forever");
                    assert!(publishes_session(kind), "{} waits for nothing", spec.id);
                    waiting += 1;
                }
                false => launch_keyed += 1,
            }
        }
        assert_eq!(waiting + launch_keyed, crate::agent::AGENT_SPECS.len());
        // The class this road exists for is not empty and not the majority
        // by accident: the catalog is wider than the hook enum, and every
        // vendor in the difference lives on the launch identity.
        let beyond_hooks = crate::agent::AGENT_SPECS
            .iter()
            .filter(|spec| AgentKind::from_slug(spec.id).is_none())
            .count();
        assert!(
            beyond_hooks > 0,
            "the catalog no longer exceeds the hook enum"
        );
        assert!(
            launch_keyed >= beyond_hooks,
            "a slug outside the hook enum was told a session is coming"
        );
        // Exact set equality, both directions, because the counts above
        // cannot see the one drift this test exists for: a publishing
        // vendor whose CATALOG id diverged from its enum slug would slide
        // into the launch class with every counter still balanced — and its
        // worker would strand the moment the real session arrived. So: the
        // slugs that wait are exactly the publishing slugs, and every
        // publishing slug is summonable under exactly that spelling.
        let waits: std::collections::BTreeSet<&str> = crate::agent::AGENT_SPECS
            .iter()
            .map(|spec| spec.id)
            .filter(|slug| session_will_come(slug))
            .collect();
        let publishes: std::collections::BTreeSet<&str> = crate::agent::ALL_AGENTS
            .into_iter()
            .filter(|kind| publishes_session(*kind))
            .map(AgentKind::slug)
            .collect();
        assert_eq!(
            waits, publishes,
            "the catalog and the hook enum disagree about who a session \
             will come from — a publishing vendor summoned under a diverged \
             spelling would strand its worker"
        );
        for slug in &publishes {
            assert!(
                crate::agent::AGENT_SPECS
                    .iter()
                    .any(|spec| spec.id == *slug),
                "{slug} publishes a session but cannot be summoned under \
                 that spelling"
            );
        }
    }
}
