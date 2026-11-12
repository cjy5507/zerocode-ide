//! The dreamer's question about a session, asked before an idle pass.
//!
//! Where the person turned it on (`dreamLessons` in their own settings, off
//! otherwise), an idle pass that is due first asks a model what the person's
//! words in this session teach (`decision_core::said_lessons`), and what the
//! reading takes is kept as a candidate (`runtime::memory::record_said_lesson`).
//!
//! **A candidate is all it becomes.** It is kept beside the person's patterns,
//! where no pass reads it: a lesson is a model's sentence about what a person
//! said, and nothing yet checks that sentence against their words. Until
//! something does, no lesson drawn this way becomes a memory entry, however
//! many sessions show it. The lessons already kept ride along with the
//! question, so a session that shows one again names it and the count of
//! sessions behind each is true.
//!
//! It is asked here, in the session's own process, and not in the pass's:
//! the request goes out on the login this session already holds, as the fold
//! of an over-cap tool result does, and a slow provider costs the lessons and
//! not the pass. Only the idle pass asks — a pass at a compaction or at the
//! session's end has no time for it — and neither a pane the ledger summoned
//! as a worker nor a run fed by a pipe (`-p`) ever does: what is said there
//! is a briefing, or a program's lines, not a person's habits. It asks when a
//! pass is due by the clock, whether or not the project has anything yet for
//! a pass to read (`dreamer_hook::idle_pass`).
//!
//! What a session was asked about is marked before the request leaves
//! (`runtime::memory::mark_said_asked`), by what was said: nothing the
//! person said is shown to a model twice, whatever became of the request.

use std::path::Path;

use decision_core::said_lessons::{worth_asking, Digest, DigestLimits, Offered, Pending, SECS_BETWEEN_ASKS};
use runtime::memory::{
    mark_said_asked, record_said_lesson, said_asked, said_lessons_known, said_words_mark,
};
use runtime::ConversationMessage;

/// The most lessons earlier sessions left that one question shows, the
/// newest: those that wait for a second session, and those already kept.
const PENDING_SHOWN: usize = 24;

/// What became of the question before one idle pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Asking {
    /// The person has not turned it on, or this is a summoned worker's pane.
    Off,
    /// The session could not be read.
    Unread,
    /// Too little is new, or it was asked about a moment ago.
    NotWorthIt,
    /// What would be asked about could not be marked, so nothing was sent.
    Unmarked,
    /// The request, or the reading of its reply, failed.
    Failed(String),
    /// Asked: how many lessons the reading took, and how many of those the
    /// patterns took.
    Asked { taken: usize, recorded: usize },
}

impl Asking {
    /// The log's word for it; `None` where nothing was looked at.
    fn audit(&self) -> Option<String> {
        match self {
            Self::Off => None,
            Self::Unread => Some("lessons=unread".to_string()),
            Self::NotWorthIt => Some("lessons=not_asked".to_string()),
            Self::Unmarked => Some("lessons=unmarked".to_string()),
            Self::Failed(_) => Some("lessons=failed".to_string()),
            Self::Asked { taken, recorded } => Some(format!("lessons=asked taken={taken} recorded={recorded}")),
        }
    }
}

/// Before an idle pass that is due: ask what this session's words teach,
/// where the person turned that on and is at this session (`attended`: its
/// input is a terminal — a run fed by a pipe is spoken to by a program).
/// `transcript` is the session's own file, as the session that calls this
/// holds it. Returns what became of it, and says it in the log by its counts
/// — never by a lesson's words.
pub(crate) fn ask_before_a_pass(
    cwd: &Path,
    session_id: &str,
    transcript: &Path,
    model: &str,
    attended: bool,
) -> Asking {
    let now = now_secs();
    let asking = if !attended || !turned_on(cwd) {
        Asking::Off
    } else if asked_a_moment_ago(cwd, session_id, now) {
        // Known from the mark alone: the session's file is not read to learn
        // that it is too soon.
        Asking::NotWorthIt
    } else {
        match messages_in(transcript) {
            Some(messages) => ask_about(cwd, session_id, &messages, now, |digest, pending| {
                tools::said_lessons::ask(digest, pending, model, tools::said_lessons::PASS_ASK_WITHIN)
                    .map(|asked| asked.read.taken)
            }),
            None => Asking::Unread,
        }
    };
    if let Some(audit) = asking.audit() {
        super::dreamer_hook::log_status(session_id, &format!("idle {audit}"));
    }
    asking
}

/// Whether the question is asked here: the person's own settings turn it on,
/// and this is not a pane the ledger summoned as a worker. Only the person's
/// settings files are opened — the one source the switch is taken from — so
/// a session with the asking off reads no repository settings for it.
fn turned_on(cwd: &Path) -> bool {
    !runtime::kill_guard::in_a_summoned_worker_pane()
        && runtime::ConfigLoader::default_for(cwd).persons_dream_lessons()
}

/// Whether `session_id` was asked about less than the wait between askings
/// before `now_secs`.
fn asked_a_moment_ago(cwd: &Path, session_id: &str, now_secs: u64) -> bool {
    said_asked(cwd, session_id)
        .is_some_and(|asked| now_secs.saturating_sub(asked.asked_at_secs) < SECS_BETWEEN_ASKS)
}

fn messages_in(transcript: &Path) -> Option<std::sync::Arc<Vec<ConversationMessage>>> {
    Some(core_types::Session::load_from_path(transcript).ok()?.messages)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Ask through `ask` what is new in `messages` — what the person said that no
/// asking has covered — when that is worth asking about `now_secs`, and
/// record what the reading took.
///
/// The order is the promise: what is new is marked as asked about first, and
/// only a mark that was written lets the request leave. Everything new is
/// marked, the things said that the digest had no room for among them — the
/// digest keeps the newest, and the older ones are not asked about later
/// either.
pub(crate) fn ask_about(
    cwd: &Path,
    session_id: &str,
    messages: &[ConversationMessage],
    now_secs: u64,
    ask: impl FnOnce(&Digest, &[Pending]) -> Result<Vec<Offered>, String>,
) -> Asking {
    let mut asked = said_asked(cwd, session_id).unwrap_or_default();
    let since_asked = (asked.asked_at_secs > 0).then(|| now_secs.saturating_sub(asked.asked_at_secs));
    let new: Vec<_> = runtime::patch_review::said_with_what_came_before(messages)
        .into_iter()
        .filter(|said| !asked.sent.contains(&said_words_mark(&said.words)))
        .collect();
    if !worth_asking(new.len(), since_asked, false) {
        return Asking::NotWorthIt;
    }
    asked.asked_at_secs = now_secs;
    asked.sent.extend(new.iter().map(|said| said_words_mark(&said.words)));
    if !mark_said_asked(cwd, session_id, &asked) {
        return Asking::Unmarked;
    }
    let pending: Vec<Pending> = said_lessons_known(cwd, PENDING_SHOWN)
        .into_iter()
        .enumerate()
        .map(|(index, known)| Pending {
            id: format!("c{}", index + 1),
            kind: known.kind,
            text: known.text,
        })
        .collect();
    match ask(&Digest::of(&new, DigestLimits::default()), &pending) {
        Err(error) => Asking::Failed(error),
        Ok(taken) => Asking::Asked {
            taken: taken.len(),
            // A lesson that names a candidate carries the candidate's own
            // kind and line, so the patterns see the same lesson again.
            recorded: taken
                .iter()
                .filter(|lesson| record_said_lesson(cwd, session_id, lesson.kind, &lesson.text))
                .count(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use decision_core::dreamer::LessonKind;
    use runtime::ContentBlock;

    use super::*;

    /// A workspace and a config home under one temp folder, with the config
    /// home named in the environment while `f` runs.
    fn in_a_home<T>(label: &str, settings: Option<&str>, f: impl FnOnce(&Path) -> T) -> T {
        let _lock = crate::test_env_lock();
        let root = std::env::temp_dir().join(format!(
            "zo-ide-said-lessons-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let cwd = root.join("workspace");
        let home = root.join("home").join(".zo");
        std::fs::create_dir_all(&cwd).expect("workspace");
        std::fs::create_dir_all(&home).expect("config home");
        if let Some(settings) = settings {
            std::fs::write(home.join("settings.json"), settings).expect("settings");
        }
        let before = std::env::var_os(core_types::paths::ZO_CONFIG_HOME_ENV);
        std::env::set_var(core_types::paths::ZO_CONFIG_HOME_ENV, &home);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&cwd)));
        match before {
            Some(value) => std::env::set_var(core_types::paths::ZO_CONFIG_HOME_ENV, value),
            None => std::env::remove_var(core_types::paths::ZO_CONFIG_HOME_ENV),
        }
        let _ = std::fs::remove_dir_all(&root);
        match result {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    fn session(said: &[&str]) -> Vec<ConversationMessage> {
        said.iter()
            .flat_map(|words| {
                [
                    ConversationMessage::user_text(*words),
                    ConversationMessage::assistant(vec![ContentBlock::Text {
                        text: format!("done: {words}"),
                    }]),
                ]
            })
            .collect()
    }

    fn patterns_file(cwd: &Path) -> PathBuf {
        runtime::memory::paths::global_project_memory_dir(cwd, "user-patterns").join("patterns.jsonl")
    }

    const SAID: [&str; 3] = ["Answer in Korean from now on", "Never push to main here", "Run the gate before a hand-in"];

    /// With the setting absent, or set anywhere but the person's own file,
    /// nothing is looked at: no session is read, nothing is marked, and no
    /// asker is reached. The same in a pane the ledger summoned, and in a
    /// run fed by a pipe.
    #[test]
    fn off_unless_the_person_turned_it_on_and_never_in_a_summoned_workers_pane() {
        // Said off in the test's own settings, which outrank the machine's:
        // the person whose machine runs this may have turned theirs on.
        in_a_home("off", Some(r#"{"dreamLessons":false}"#), |cwd| {
            assert!(!turned_on(cwd));
            assert_eq!(ask_before_a_pass(cwd, "s-0", &cwd.join("no-such-session.jsonl"), "model-x", true), Asking::Off);
            assert_eq!(Asking::Off.audit(), None);
            assert!(!patterns_file(cwd).parent().expect("folder").exists(), "nothing was written");
            assert!(said_lessons_known(cwd, 10).is_empty());
        });
        in_a_home("on", Some(r#"{"dreamLessons":true}"#), |cwd| {
            // The answer must not depend on the pane the suite runs in: first
            // a pane a person opened, then one the ledger summoned.
            let (pane, leader) = (zerocode_core::agent_teams::TEAM_PANE_VAR, zerocode_core::agent_teams::TEAM_LEADER_VAR);
            let before = [pane, leader].map(|name| (name, std::env::var_os(name)));
            std::env::remove_var(pane);
            std::env::remove_var(leader);
            let a_persons_pane = turned_on(cwd);
            // Turned on, the session's file is read — and one that is not there is not asked about.
            let nowhere = cwd.join("no-such-session.jsonl");
            let unread = ask_before_a_pass(cwd, "s-0", &nowhere, "model-x", true);
            // A run fed by a pipe is not asked, whatever the setting says.
            let piped = ask_before_a_pass(cwd, "s-0", &nowhere, "model-x", false);
            // A session asked about a moment ago is not read at all.
            let just_asked = runtime::memory::SaidAsked { asked_at_secs: now_secs(), sent: Vec::new() };
            assert!(mark_said_asked(cwd, "s-9", &just_asked));
            let too_soon = ask_before_a_pass(cwd, "s-9", &nowhere, "model-x", true);
            std::env::set_var(pane, "%3");
            std::env::set_var(leader, "%1");
            let a_workers_pane = (turned_on(cwd), ask_before_a_pass(cwd, "s-0", &nowhere, "model-x", true));
            for (name, value) in before {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
            assert!(a_persons_pane);
            assert_eq!(unread, Asking::Unread);
            assert_eq!(piped, Asking::Off, "a program's lines are not a person's habits");
            assert_eq!(too_soon, Asking::NotWorthIt, "its mark says it is too soon, before its file is opened");
            assert_eq!(a_workers_pane, (false, Asking::Off), "a summoned worker's pane never asks");
        });
    }

    /// A session with enough that is new is asked once, about those words
    /// alone; what the reading took goes to the patterns; and the words are
    /// marked before the request leaves, so they are never shown again —
    /// not half an hour later, and not after a request that failed.
    #[test]
    fn what_is_new_is_marked_first_asked_once_and_never_shown_again() {
        in_a_home("asked", None, |cwd| {
            let messages = session(&SAID);
            let now = 1_800_000_000;
            let shown = RefCell::new(Vec::new());
            let taken = vec![
                Offered { same_as: None, kind: LessonKind::Preference, text: "Answer in Korean".to_string(), said: 1 },
                Offered { same_as: None, kind: LessonKind::Constraint, text: "Never push to main".to_string(), said: 2 },
            ];
            let asking = ask_about(cwd, "s-1", &messages, now, |digest, pending| {
                // The mark is on disk before the request leaves.
                let marked = said_asked(cwd, "s-1").expect("marked before asking");
                assert_eq!(marked.asked_at_secs, now);
                assert_eq!(marked.sent.len(), 3);
                assert!(pending.is_empty(), "no earlier session left a candidate");
                shown.borrow_mut().push(digest.text());
                Ok(taken.clone())
            });
            assert_eq!(asking, Asking::Asked { taken: 2, recorded: 2 });
            assert_eq!(asking.audit().as_deref(), Some("lessons=asked taken=2 recorded=2"));
            let text = &shown.borrow()[0];
            assert!(text.starts_with("[1] the person: Answer in Korean from now on\n"), "{text}");
            assert!(text.contains("[3] the assistant had said: done: Never push to main here\n"), "{text}");
            let known = said_lessons_known(cwd, 10);
            assert_eq!(
                known.iter().map(|lesson| (lesson.kind, lesson.text.as_str(), lesson.sessions)).collect::<Vec<_>>(),
                [(LessonKind::Constraint, "Never push to main", 1), (LessonKind::Preference, "Answer in Korean", 1)],
                "both wait for a second session, newest first"
            );

            // Nothing new: not asked again, however long after.
            let again = ask_about(cwd, "s-1", &messages, now + 86_400, |_, _| panic!("the same words were sent twice"));
            assert_eq!(again, Asking::NotWorthIt);

            // Three more things said, a minute on: too soon. Half an hour on: asked, about those three alone.
            let mut longer = messages.clone();
            longer.extend(session(&["Keep commits small", "Tests before the fix", "Plain Korean in reports"]));
            let soon = ask_about(cwd, "s-1", &longer, now + 60, |_, _| panic!("asked a minute after the last asking"));
            assert_eq!(soon, Asking::NotWorthIt);
            let later = ask_about(cwd, "s-1", &longer, now + 1_800, |digest, pending| {
                assert_eq!(digest.len(), 3, "only what is new is shown");
                assert!(!digest.text().contains("Answer in Korean"), "{}", digest.text());
                assert_eq!(pending.len(), 2, "the candidates the first asking left are shown beside it");
                Err("timed out after 20s".to_string())
            });
            assert_eq!(later, Asking::Failed("timed out after 20s".to_string()));
            let after_failure = ask_about(cwd, "s-1", &longer, now + 86_400, |_, _| panic!("words of a failed request were sent again"));
            assert_eq!(after_failure, Asking::NotWorthIt);
        });
    }

    /// A second session that shows a candidate answers with its id; the
    /// candidate's own line is kept again, under the second session, and the
    /// count behind it is two. No pass makes it a memory entry: a candidate
    /// is all a lesson a session's words offered becomes.
    #[test]
    fn a_candidate_a_second_session_shows_again_is_counted_and_no_pass_makes_it_memory() {
        in_a_home("kept", None, |cwd| {
            let now = 1_800_000_000;
            let first = ask_about(cwd, "s-1", &session(&SAID), now, |_, _| {
                Ok(vec![Offered { same_as: None, kind: LessonKind::Constraint, text: "Never push to main".to_string(), said: 2 }])
            });
            assert_eq!(first, Asking::Asked { taken: 1, recorded: 1 });
            // The same session, later, showing it again: still one session.
            let mut longer = session(&SAID);
            longer.extend(session(&["Do not push to main, I said", "Branch first", "Then hand in"]));
            let same_session = ask_about(cwd, "s-1", &longer, now + 3_600, |_, pending| {
                Ok(vec![Offered { same_as: Some(pending[0].id.clone()), kind: pending[0].kind, text: pending[0].text.clone(), said: 1 }])
            });
            assert_eq!(same_session, Asking::Asked { taken: 1, recorded: 1 });
            assert_eq!(said_lessons_known(cwd, 10)[0].sessions, 1, "one session, however often it shows it");

            // Another session shows it: two sessions stand behind it.
            let second = ask_about(cwd, "s-2", &session(&["Work on a branch", "main is protected here", "Open a hand-in"]), now + 7_200, |_, pending| {
                assert_eq!(pending.len(), 1);
                assert_eq!((pending[0].id.as_str(), pending[0].text.as_str()), ("c1", "Never push to main"));
                Ok(vec![Offered { same_as: Some("c1".to_string()), kind: pending[0].kind, text: pending[0].text.clone(), said: 2 }])
            });
            assert_eq!(second, Asking::Asked { taken: 1, recorded: 1 });
            assert_eq!(said_lessons_known(cwd, 10)[0].sessions, 2);

            // A pass reads none of it: nothing is kept, and the person's
            // patterns — which a pass does read — were never written.
            let report = runtime::memory::dream_at_cwd(cwd, None).expect("a pass");
            assert!(report.applied.is_empty(), "a candidate became memory: {:?}", report.applied);
            assert!(!patterns_file(cwd).exists(), "a lesson the words offered was written among the person's patterns");

            // It still rides along: a third session that shows it names it,
            // and is not left to say it in new words that would stand as a
            // second lesson.
            let third = ask_about(cwd, "s-3", &session(&["No direct pushes", "Use a branch", "Then a hand-in"]), now + 10_800, |_, pending| {
                assert_eq!(pending.len(), 1, "the lesson is shown beside the session");
                Ok(vec![Offered { same_as: Some("c1".to_string()), kind: pending[0].kind, text: pending[0].text.clone(), said: 1 }])
            });
            assert_eq!(third, Asking::Asked { taken: 1, recorded: 1 });
            let known = said_lessons_known(cwd, 10);
            assert_eq!((known.len(), known[0].sessions), (1, 3), "one lesson, shown by three sessions");
        });
    }
}
