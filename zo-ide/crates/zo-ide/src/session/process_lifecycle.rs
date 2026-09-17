//! Process/session termination diagnostics.
//!
//! A live [`core_types::Session`] owns the transcript writer lease, so events
//! observed while the frontend can recover are appended through
//! [`core_types::Session::push_message`]. Raw append is reserved for the outer
//! process guard, after `run` has returned or unwound and the session owner has
//! already been dropped.

use std::any::Any;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use core_types::session::{ContentBlock, ConversationMessage, MessageRole, SessionError};
use core_types::Session;

pub(crate) const EVENT_PREFIX: &str = "[zo:process-event] ";
const MAX_REASON_CHARS: usize = 768;
const MAX_ID_CHARS: usize = 160;
const MAX_KIND_CHARS: usize = 64;

struct TrackedSession {
    id: String,
    path: PathBuf,
}

static TRACKED_SESSION: OnceLock<Mutex<Option<TrackedSession>>> = OnceLock::new();
static PANIC_HOOK_INSTALLED: OnceLock<()> = OnceLock::new();

fn tracked_session() -> &'static Mutex<Option<TrackedSession>> {
    TRACKED_SESSION.get_or_init(|| Mutex::new(None))
}

/// Installs the one process-wide panic observer and returns the main-stack
/// guard that records an uncaught unwind as a terminal exit.
#[must_use]
pub fn install() -> ProcessExitGuard {
    if PANIC_HOOK_INSTALLED.set(()).is_ok() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let detail = panic_payload(info.payload());
            let location = info.location().map_or_else(
                || "unknown".to_string(),
                |location| format!("{}:{}", location.file(), location.line()),
            );
            eprintln!(
                "zo: panic observed thread={} location={} detail={}",
                std::thread::current().name().unwrap_or("unnamed"),
                single_line(&location, MAX_ID_CHARS),
                single_line(&detail, MAX_REASON_CHARS),
            );
            previous(info);
        }));
    }
    ProcessExitGuard {
        finished: false,
        events_addr_file: None,
    }
}

/// Main-stack sentinel. Explicit exits call [`Self::finish`]; an uncaught
/// unwind reaches `Drop`, after the owned session has unwound and released its
/// transcript writer lease.
pub struct ProcessExitGuard {
    finished: bool,
    events_addr_file: Option<PathBuf>,
}

impl ProcessExitGuard {
    /// Emits the process exit line and appends the final process event to the
    /// active transcript (normal frontend teardown has its own session event).
    pub fn finish(&mut self, reason: &str, detail: Option<&str>) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.remove_events_addr_file();
        emit_exit_line(reason, detail);
        append_terminal_event_if_open(reason);
    }

    /// Registers the app-discovery file owned by this process. Both an
    /// explicit finish and an unwinding main stack remove it before exit.
    pub fn track_events_addr_file(&mut self, path: PathBuf) {
        if let Some(previous) = self.events_addr_file.replace(path) {
            remove_events_addr_file(&previous);
        }
    }

    fn remove_events_addr_file(&mut self) {
        if let Some(path) = self.events_addr_file.take() {
            remove_events_addr_file(&path);
        }
    }
}

impl Drop for ProcessExitGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let reason = if std::thread::panicking() {
            "panic"
        } else {
            "main_guard_dropped"
        };
        self.remove_events_addr_file();
        emit_exit_line(reason, None);
        append_terminal_event_if_open(reason);
    }
}

fn remove_events_addr_file(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => eprintln!(
            "zo: could not remove events discovery file {}: {error}",
            path.display()
        ),
    }
}

/// Updates the process guard's active transcript after a session successfully
/// opens. `/resume` may replace this value within the same process.
pub(crate) fn track_session(session_id: &str, path: &Path) {
    *tracked_session()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(TrackedSession {
        id: session_id.to_string(),
        path: path.to_path_buf(),
    });
}

/// Appends a recoverable in-process diagnostic through the session's writer.
pub(crate) fn record_event(
    session: &mut Session,
    session_id: &str,
    kind: &str,
    reason: &str,
) -> Result<(), SessionError> {
    session.push_message(event_message(session_id, kind, reason))
}

/// Records normal frontend teardown through the session writer.
pub(crate) fn record_session_end(
    session: &mut Session,
    session_id: &str,
    reason: &str,
) -> Result<(), SessionError> {
    record_event(session, session_id, "session_end", reason)
}

/// Removes host-only diagnostic messages before a resumed transcript is handed
/// to a provider. User/assistant text that merely contains the prefix is kept.
///
/// This is deliberately an in-memory view: the diagnostics remain in the
/// transcript as the post-mortem record, and merely opening a session must not
/// rewrite the file or move it to the top of the mtime-ordered resume list.
pub(crate) fn strip_process_events(session: &mut Session) -> usize {
    let messages = std::sync::Arc::make_mut(&mut session.messages);
    let before = messages.len();
    messages.retain(|message| !is_process_event(message));
    before.saturating_sub(messages.len())
}

pub(crate) fn panic_payload(payload: &(dyn Any + Send)) -> String {
    let detail = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload");
    single_line(detail, MAX_REASON_CHARS)
}

fn event_message(session_id: &str, kind: &str, reason: &str) -> ConversationMessage {
    let payload = serde_json::json!({
        "at_ms": epoch_millis(),
        "kind": single_line(kind, MAX_KIND_CHARS),
        "pid": std::process::id(),
        "reason": single_line(reason, MAX_REASON_CHARS),
        "session_id": single_line(session_id, MAX_ID_CHARS),
    });
    ConversationMessage {
        role: MessageRole::System,
        blocks: vec![ContentBlock::Text {
            text: format!("{EVENT_PREFIX}{payload}"),
        }],
        usage: None,
        thought_signature: None,
        reasoning_replay: None,
        model: None,
    }
}

fn is_process_event(message: &ConversationMessage) -> bool {
    message.role == MessageRole::System
        && matches!(
            message.blocks.as_slice(),
            [ContentBlock::Text { text }] if text.starts_with(EVENT_PREFIX)
        )
}

fn append_terminal_event_if_open(reason: &str) {
    let tracked = {
        let mut guard = tracked_session()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.take()
    };
    let Some(tracked) = tracked else {
        return;
    };
    if let Err(error) = append_event_to_path(&tracked.path, &tracked.id, "process_exit", reason) {
        eprintln!(
            "zo: could not append process exit to session {}: {error}",
            tracked.path.display()
        );
    }
}

fn append_event_to_path(
    path: &Path,
    session_id: &str,
    kind: &str,
    reason: &str,
) -> std::io::Result<()> {
    let message = event_message(session_id, kind, reason);
    let [ContentBlock::Text { text }] = message.blocks.as_slice() else {
        return Err(std::io::Error::other("process event was not text"));
    };
    let record = serde_json::json!({
        "type": "message",
        "updated_at_ms": epoch_millis(),
        "message": {
            "role": "system",
            "blocks": [{"type": "text", "text": text}],
        },
    });
    let mut line = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
    line.push(b'\n');
    let mut file = OpenOptions::new().append(true).open(path)?;
    file.write_all(&line)?;
    file.sync_data()
}

fn emit_exit_line(reason: &str, detail: Option<&str>) {
    let reason = single_line(reason, MAX_KIND_CHARS);
    if let Some(detail) = detail.filter(|detail| !detail.trim().is_empty()) {
        eprintln!(
            "zo: process exit reason={reason} detail={}",
            single_line(detail, MAX_REASON_CHARS)
        );
    } else {
        eprintln!("zo: process exit reason={reason}");
    }
}

fn single_line(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(max_chars)
        .collect()
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File, FileTimes};
    use std::time::{Duration, UNIX_EPOCH};

    use core_types::session::{ContentBlock, ConversationMessage, MessageRole};
    use core_types::Session;

    use super::{
        append_event_to_path, event_message, record_session_end, strip_process_events,
        EVENT_PREFIX,
    };

    fn message_text(message: &ConversationMessage) -> &str {
        let [ContentBlock::Text { text }] = message.blocks.as_slice() else {
            panic!("process event must be one text block");
        };
        text
    }

    #[test]
    fn process_event_appends_as_a_loadable_session_jsonl_record() {
        let directory = tempfile::tempdir().expect("temporary session directory");
        let path = directory.path().join("session.jsonl");
        let session = Session::new();
        session.save_to_path(&path).expect("seed session");

        append_event_to_path(&path, "session-a", "process_exit", "error")
            .expect("append process event");

        let loaded = Session::load_from_path(&path).expect("event-bearing session reloads");
        let event = loaded.messages.last().expect("process event record");
        assert_eq!(event.role, MessageRole::System);
        assert!(message_text(event).starts_with(EVENT_PREFIX));
    }

    #[test]
    fn normal_session_end_uses_the_session_writer() {
        let directory = tempfile::tempdir().expect("temporary session directory");
        let path = directory.path().join("session.jsonl");
        let mut session = Session::new().with_persistence_path(path.clone());
        session.save_to_path(&path).expect("seed session");

        record_session_end(&mut session, "session-a", "exit").expect("record normal exit");

        let loaded = Session::load_from_path(&path).expect("closed session reloads");
        let event = loaded.messages.last().expect("session end record");
        assert!(message_text(event).contains(r#""kind":"session_end""#));
    }

    #[test]
    fn resume_strips_process_events_without_stripping_user_text() {
        let mut session = Session::new();
        session
            .push_message(ConversationMessage::user_text(format!(
                "please explain {EVENT_PREFIX}"
            )))
            .expect("user message");
        session
            .push_message(event_message("session-a", "session_end", "exit"))
            .expect("process event");

        assert_eq!(strip_process_events(&mut session), 1);
        assert_eq!(session.messages.len(), 1);
        assert_eq!(session.messages[0].role, MessageRole::User);
    }

    #[test]
    fn resume_stripping_is_memory_only_and_keeps_mtime() {
        let directory = tempfile::tempdir().expect("temporary session directory");
        let path = directory.path().join("session.jsonl");
        let mut session = Session::new().with_persistence_path(path.clone());
        session.save_to_path(&path).expect("seed session");
        record_session_end(&mut session, "session-a", "exit").expect("append process event");
        let original = fs::read(&path).expect("read original transcript");
        session.release_writer_lease();
        drop(session);

        let historical = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        File::open(&path)
            .expect("open transcript")
            .set_times(FileTimes::new().set_modified(historical))
            .expect("pin old mtime");
        let before = fs::metadata(&path)
            .expect("transcript metadata")
            .modified()
            .expect("transcript mtime");

        let mut resumed = Session::load_from_path(&path).expect("resume transcript");
        assert_eq!(strip_process_events(&mut resumed), 1);
        resumed
            .persist_appended_state_to_path(&path)
            .expect("resume open persist");

        assert_eq!(
            fs::metadata(&path)
                .expect("transcript metadata after resume")
                .modified()
                .expect("transcript mtime after resume"),
            before,
            "an in-memory provider view must not reorder the resume list by touching mtime"
        );
        assert_eq!(
            fs::read(&path).expect("read transcript after resume"),
            original,
            "host diagnostics stay on disk even though the provider view omits them"
        );
    }

    #[test]
    fn event_reason_is_single_line_and_bounded() {
        let message = event_message(
            "session-a",
            "panic",
            &format!("first\nsecond {}", "x".repeat(2_000)),
        );
        let text = message_text(&message);

        assert!(!text.contains('\n'));
        assert!(text.len() <= 1_200, "event record grew to {} bytes", text.len());
    }

    #[test]
    fn process_exit_removes_the_events_discovery_file() {
        let directory = tempfile::tempdir().expect("temporary runtime directory");
        let path = directory.path().join("zo-events-42424.addr");
        fs::write(&path, "127.0.0.1:49152\ntoken\nsession\n")
            .expect("seed discovery file");
        let mut exit = super::install();
        exit.track_events_addr_file(path.clone());

        exit.finish("test_exit", None);

        assert!(!path.exists(), "the process exit path leaked its discovery file");
    }
}
