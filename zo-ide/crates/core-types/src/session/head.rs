//! The head of a transcript — what a listing needs, read without loading the
//! session (t-2947).
//!
//! A session file is append-only JSONL whose first record is the header and
//! whose next records are the messages, so everything `/resume` and
//! `session_recall` list — id, name, creation time, fork provenance, the first
//! prompt — sits in the first two records. [`TranscriptHead::read`] reads the
//! file line by line and stops the moment it has them; the 26 MB transcript
//! behind it is never touched. The full loader stays the oracle: a file whose
//! head is not JSONL (a legacy `.json` snapshot, a torn first line) is handed
//! to [`Session::load_from_path`] and summarised from the result, so the two
//! roads agree on every file the loader accepts.
//!
//! The one deliberate difference: corruption *after* the head. The loader
//! rejects the whole file; the head never reads that far and lists the row —
//! which is the bound working as intended.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::json::JsonValue;

use super::{
    current_time_millis, generate_session_id, parse_jsonl_record, ContentBlock,
    ConversationMessage, MessageRole, Session, SessionError, SessionFork, SessionJsonlRecord,
};

/// What a listing shows for one transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptHead {
    pub session_id: String,
    pub name: Option<String>,
    pub created_at_ms: u64,
    pub fork: Option<SessionFork>,
    /// The first non-empty text block of the first user message, cut to the
    /// `preview_chars` the reader asked for. `None` when no preview was asked,
    /// when nobody spoke into the session, or when the first user message has
    /// no text (an image alone).
    pub first_user_text: Option<String>,
}

impl TranscriptHead {
    /// Read the head of the transcript at `path`. With `preview_chars`
    /// `None` the read stops after the header; with `Some(n)` it goes on to
    /// the first user message and keeps at most `n` characters of its text.
    pub fn read(path: &Path, preview_chars: Option<usize>) -> Result<Self, SessionError> {
        let mut reader = BufReader::new(File::open(path)?);
        let mut line = String::new();
        let mut line_number = 0usize;
        let mut parsed_records = 0usize;
        let mut meta: Option<HeaderFields> = None;
        let mut first_user: Option<Option<String>> = None;
        loop {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                break;
            }
            line_number += 1;
            let record = match parse_jsonl_record(&line, line_number) {
                Ok(record) => record,
                // The first line is not a JSONL record: a legacy `.json`
                // snapshot, or a head too damaged to trust. The loader owns
                // that verdict.
                Err(_) if parsed_records == 0 && line_number == 1 => {
                    return Self::from_full_load(path, preview_chars);
                }
                Err(error) => {
                    if parsed_records > 0 && is_torn_tail(&line, &mut reader)? {
                        break;
                    }
                    return Err(error);
                }
            };
            let Some(record) = record else {
                continue;
            };
            parsed_records += 1;
            match record {
                SessionJsonlRecord::Meta {
                    record_session_id,
                    record_name,
                    record_created_at_ms,
                    record_fork,
                    ..
                } => {
                    meta = Some(HeaderFields {
                        session_id: record_session_id,
                        name: record_name,
                        created_at_ms: record_created_at_ms,
                        fork: record_fork,
                    });
                }
                SessionJsonlRecord::Message { message, .. } => {
                    if first_user.is_none() && message.role == MessageRole::User {
                        first_user = Some(preview_chars.and_then(|n| first_text(&message, n)));
                    }
                }
                SessionJsonlRecord::Compaction(_) => {}
            }
            if meta.is_some() && (preview_chars.is_none() || first_user.is_some()) {
                break;
            }
        }
        let header = meta.unwrap_or_else(HeaderFields::absent);
        Ok(Self {
            session_id: header.session_id,
            name: header.name,
            created_at_ms: header.created_at_ms,
            fork: header.fork,
            first_user_text: first_user.flatten(),
        })
    }

    /// The same head, taken from a fully loaded session — the oracle the
    /// streaming read is pinned against, and the road a non-JSONL file takes.
    #[must_use]
    pub fn of_session(session: &Session, preview_chars: Option<usize>) -> Self {
        let first_user_text = preview_chars.and_then(|n| {
            session
                .messages
                .iter()
                .find(|message| message.role == MessageRole::User)
                .and_then(|message| first_text(message, n))
        });
        Self {
            session_id: session.session_id.clone(),
            name: session.name.clone(),
            created_at_ms: session.created_at_ms,
            fork: session.fork.clone(),
            first_user_text,
        }
    }

    fn from_full_load(path: &Path, preview_chars: Option<usize>) -> Result<Self, SessionError> {
        Session::load_from_path(path).map(|session| Self::of_session(&session, preview_chars))
    }
}

struct HeaderFields {
    session_id: String,
    name: Option<String>,
    created_at_ms: u64,
    fork: Option<SessionFork>,
}

impl HeaderFields {
    /// What the loader invents for a transcript with no header record: a
    /// fresh id and "now".
    fn absent() -> Self {
        Self {
            session_id: generate_session_id(),
            name: None,
            created_at_ms: current_time_millis(),
            fork: None,
        }
    }
}

/// The first non-empty text block of `message`, cut to `chars` characters.
fn first_text(message: &ConversationMessage, chars: usize) -> Option<String> {
    message.blocks.iter().find_map(|block| match block {
        ContentBlock::Text { text } => {
            let cut: String = text.chars().take(chars).collect();
            (!cut.is_empty()).then_some(cut)
        }
        _ => None,
    })
}

/// The loader's torn-append rule, streamed: a line that is not JSON at all and
/// is the last content in the file is a torn trailing write, dropped rather
/// than fatal. Anything after it makes it interior corruption.
fn is_torn_tail(line: &str, reader: &mut BufReader<File>) -> Result<bool, SessionError> {
    if JsonValue::parse(line.trim()).is_ok() {
        return Ok(false);
    }
    let mut rest = String::new();
    loop {
        rest.clear();
        if reader.read_line(&mut rest)? == 0 {
            return Ok(true);
        }
        if !rest.trim().is_empty() {
            return Ok(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::TranscriptHead;
    use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session, SessionFork};

    const PREVIEW: usize = 60;

    fn scratch(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("zo-head-{tag}-{unique}"));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    fn text(role: MessageRole, text: &str) -> ConversationMessage {
        ConversationMessage {
            role,
            blocks: vec![ContentBlock::Text {
                text: text.to_owned(),
            }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
            model: None,
        }
    }

    fn image_then_text(text: &str) -> ConversationMessage {
        ConversationMessage::user_with_images(text, vec![("image/png".into(), "AAAA".into())])
    }

    fn session_with(messages: Vec<ConversationMessage>) -> Session {
        let mut session = Session::new();
        session.session_id = "sess-head".into();
        session.name = Some("named".into());
        session.fork = Some(SessionFork {
            parent_session_id: "sess-parent".into(),
            branch_name: Some("branch".into()),
        });
        session.messages = Arc::new(messages);
        session
    }

    /// The pin: the streamed head equals the head of the fully loaded session.
    fn assert_agrees(path: &std::path::Path, preview: Option<usize>) -> TranscriptHead {
        let head = TranscriptHead::read(path, preview).expect("head reads");
        let loaded = Session::load_from_path(path).expect("loader reads");
        assert_eq!(head, TranscriptHead::of_session(&loaded, preview));
        head
    }

    #[test]
    fn a_saved_session_reads_the_same_from_its_head() {
        let dir = scratch("saved");
        let path = dir.join("sess-head.jsonl");
        let long = "가".repeat(30) + &"b".repeat(50);
        session_with(vec![
            text(MessageRole::User, &long),
            text(MessageRole::Assistant, "reply"),
            text(MessageRole::User, "second prompt"),
        ])
        .save_to_path(&path)
        .expect("save");

        let head = assert_agrees(&path, Some(PREVIEW));
        assert_eq!(head.session_id, "sess-head");
        assert_eq!(head.name.as_deref(), Some("named"));
        assert_eq!(head.fork.as_ref().map(|f| f.parent_session_id.as_str()), Some("sess-parent"));
        let preview = head.first_user_text.expect("first prompt");
        assert_eq!(preview.chars().count(), PREVIEW, "cut in characters, not bytes");
        assert!(preview.starts_with("가"));

        let bare = assert_agrees(&path, None);
        assert_eq!(bare.first_user_text, None, "no preview asked, none read");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_preview_the_read_ends_at_the_header() {
        let dir = scratch("bounded");
        let path = dir.join("sess-head.jsonl");
        session_with(vec![text(MessageRole::User, "hello")])
            .save_to_path(&path)
            .expect("save");
        let mut contents = std::fs::read_to_string(&path).expect("read");
        // Interior corruption after the header, then a valid record: the loader
        // rejects the file, the header read never gets there.
        contents.push_str("not json at all\n");
        let message_line = contents.lines().nth(1).expect("message line").to_owned();
        contents.push_str(&message_line);
        contents.push('\n');
        std::fs::write(&path, contents).expect("rewrite");

        assert!(Session::load_from_path(&path).is_err());
        let head = TranscriptHead::read(&path, None).expect("header alone");
        assert_eq!(head.session_id, "sess-head");
        assert!(
            TranscriptHead::read(&path, Some(PREVIEW)).is_ok(),
            "the first user message sits before the corruption, so the preview read stops there too"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_user_message_decides_the_preview() {
        let dir = scratch("first-user");
        // The first user message carries only an image: no preview, even though
        // a later user message has text — exactly what the loader-based listing did.
        let image_only = dir.join("image-only.jsonl");
        session_with(vec![
            text(MessageRole::Assistant, "assistant first"),
            ConversationMessage::user_with_images("", vec![("image/png".into(), "AAAA".into())]),
            text(MessageRole::User, "later text"),
        ])
        .save_to_path(&image_only)
        .expect("save");
        assert_eq!(assert_agrees(&image_only, Some(PREVIEW)).first_user_text, None);

        // An image before the text: the text block is still the preview.
        let mixed = dir.join("mixed.jsonl");
        session_with(vec![image_then_text("after the image")])
            .save_to_path(&mixed)
            .expect("save");
        assert_eq!(
            assert_agrees(&mixed, Some(PREVIEW)).first_user_text.as_deref(),
            Some("after the image")
        );

        // Nobody spoke: header only.
        let empty = dir.join("empty.jsonl");
        session_with(Vec::new()).save_to_path(&empty).expect("save");
        assert_eq!(assert_agrees(&empty, Some(PREVIEW)).first_user_text, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blank_lines_and_a_late_header_read_like_the_loader() {
        let dir = scratch("late-header");
        let path = dir.join("late.jsonl");
        session_with(vec![text(MessageRole::User, "prompt")])
            .save_to_path(&path)
            .expect("save");
        let saved = std::fs::read_to_string(&path).expect("read");
        let mut lines = saved.lines();
        let header = lines.next().expect("header").to_owned();
        let message = lines.next().expect("message").to_owned();
        std::fs::write(&path, format!("\n{message}\n\n{header}\n\n")).expect("rewrite");

        let head = assert_agrees(&path, Some(PREVIEW));
        assert_eq!(head.session_id, "sess-head");
        assert_eq!(head.first_user_text.as_deref(), Some("prompt"));
        assert_eq!(assert_agrees(&path, None).session_id, "sess-head");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_torn_tail_is_dropped_and_a_torn_head_is_fatal_on_both_roads() {
        let dir = scratch("torn");
        let torn_tail = dir.join("tail.jsonl");
        session_with(Vec::new()).save_to_path(&torn_tail).expect("save");
        let mut contents = std::fs::read_to_string(&torn_tail).expect("read");
        contents.push_str("{\"type\":\"message\",\"turn_index\":0,\"message\":{\"role\":\"user\",\"blo");
        std::fs::write(&torn_tail, contents).expect("rewrite");
        assert_eq!(assert_agrees(&torn_tail, Some(PREVIEW)).first_user_text, None);

        let torn_head = dir.join("head.jsonl");
        std::fs::write(&torn_head, "{\"type\":\"session_meta\",\"sess").expect("write");
        assert!(Session::load_from_path(&torn_head).is_err());
        assert!(TranscriptHead::read(&torn_head, Some(PREVIEW)).is_err());

        let unknown = dir.join("unknown.jsonl");
        session_with(Vec::new()).save_to_path(&unknown).expect("save");
        let mut contents = std::fs::read_to_string(&unknown).expect("read");
        contents.push_str("{\"type\":\"weird\"}\n");
        std::fs::write(&unknown, contents).expect("rewrite");
        assert!(Session::load_from_path(&unknown).is_err());
        assert!(TranscriptHead::read(&unknown, Some(PREVIEW)).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_legacy_json_snapshot_goes_through_the_loader() {
        let dir = scratch("legacy");
        let path = dir.join("legacy.json");
        let session = session_with(vec![text(MessageRole::User, "old prompt")]);
        std::fs::write(&path, session.to_json().expect("json").render()).expect("write");
        let head = assert_agrees(&path, Some(PREVIEW));
        assert_eq!(head.session_id, "sess-head");
        assert_eq!(head.first_user_text.as_deref(), Some("old prompt"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_headerless_transcript_invents_what_the_loader_invents() {
        let dir = scratch("headerless");
        let path = dir.join("headerless.jsonl");
        session_with(vec![text(MessageRole::User, "prompt")])
            .save_to_path(&path)
            .expect("save");
        let saved = std::fs::read_to_string(&path).expect("read");
        let message = saved.lines().nth(1).expect("message").to_owned();
        std::fs::write(&path, format!("{message}\n")).expect("rewrite");

        let head = TranscriptHead::read(&path, Some(PREVIEW)).expect("head");
        let loaded = Session::load_from_path(&path).expect("loader");
        assert_eq!(head.first_user_text.as_deref(), Some("prompt"));
        assert_eq!(head.name, None);
        assert_eq!(head.fork, None);
        assert!(!head.session_id.is_empty());
        assert_ne!(head.session_id, loaded.session_id, "both invent a fresh id");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
