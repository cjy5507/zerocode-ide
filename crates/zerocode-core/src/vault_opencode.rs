//! OpenCode's session store, which is a database rather than a directory.
//!
//! Every other agent in [`crate::vault`] keeps one file per conversation, so
//! one walk finds them all and one parser reads them. OpenCode 1.17 moved its
//! sessions into `~/.local/share/opencode/opencode.db` and left the old
//! `storage/session/*.json` layout behind — on this machine the file layout is
//! gone entirely (`storage/` holds only `migration` and `session_diff`) while
//! the database holds 86 sessions, 44 of them top-level. A vault that walks
//! directories therefore reports that OpenCode has never been used, which is
//! what ours did.
//!
//! ## What lives here and what does not
//!
//! The rules: which row is a session worth showing, what its card is named,
//! which text is its preview. No SQL and no `rusqlite` — the reader that runs
//! the statements is [`zerocode-shell`'s `vault_opencode_scan`], next to the
//! ledger reader that already opens these same files. The split is the one
//! [`crate::usage_stats_opencode`] keeps for the token ledger, and for the same
//! reason: a rule with a database behind it is a rule nobody tests.
//!
//! ## The card is finished by the same function as every other card
//!
//! [`crate::vault::card`] composes the id, falls back to a title, and writes
//! the resume line. Nothing of that is repeated here, because a card built
//! twice by two functions is two kinds of row in one list — and the resume
//! spelling in particular (`opencode --session <id>`) was already in
//! [`crate::vault::resume_invocation`], waiting for a source that never came.
//!
//! ## Where we do not follow the original
//!
//! * **The preview shows the OPENING of the conversation, not its end.**
//!   Orca's SQLite reader takes the newest five text parts
//!   (`session-scanner-opencode-sqlite.ts:19-27`), while its file readers keep
//!   what they met first. We keep the opening for every agent, so a panel of
//!   mixed rows answers one question rather than two — and the title falls out
//!   of the FIRST user line, which a newest-first window would replace with a
//!   late turn.
//! * **The model keeps its provider.** Orca's vault shows the bare id
//!   (`extractModelId`) while its usage pane shows `provider/model`. Ours says
//!   `openai/gpt-5.5-fast` in both, through
//!   [`crate::usage_stats_opencode::named_model`] — one model naming rule for
//!   the whole app, and OpenCode routes the same model id through several
//!   providers, so the prefix is information rather than noise.

use crate::civil::iso_utc_of;
use crate::transcript::clamp;
use crate::vault::{PREVIEW_LINES, Parsed, PreviewMessage, VaultSession};

/// The slug these cards wear — Orca's spelling (`AI_VAULT_AGENTS`), which is
/// also the agent id, the resume branch's arm and the settings key.
pub const SLUG: &str = "opencode";

/// One row of the `session` table, as the reader hands it over.
///
/// Every field is optional in the schema sense: OpenCode has shipped several
/// generations of this table and the reader selects `NULL` for a column that
/// is not there yet, rather than refusing to read a database it half knows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    /// Where the agent was running — the resume's `cd`.
    pub directory: Option<String>,
    /// The `model` column, holding `{"id":…,"providerID":…}`.
    pub model_json: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
    /// How many user and assistant messages the session holds.
    pub message_count: usize,
}

/// One text part of one message, in the order the conversation had them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// `user` or `assistant`; the reader asks for no others.
    pub role: String,
    /// The `part.data` blob, which holds the text under `$.text`.
    pub data: String,
}

/// The stamp this session is sorted and shown by.
///
/// `time_updated` when it is set, `time_created` otherwise — Orca's rule
/// (`rowToCandidate`), and it matters because a session that was created and
/// never spoken to again carries a zero there.
#[must_use]
pub const fn touched_ms(row: &SessionRow) -> i64 {
    if row.updated_ms > 0 {
        row.updated_ms
    } else {
        row.created_ms
    }
}

/// How many text parts the reader needs per session.
///
/// The card keeps [`PREVIEW_LINES`], so asking for more is a blob read per row
/// that nothing draws — and these blobs run to tens of kilobytes.
pub const PREVIEW_PARTS: usize = PREVIEW_LINES;

/// The text a `part.data` blob holds, or `None` when the part is not one.
///
/// The reader has already filtered to `$.type = 'text'` in SQL, so this is the
/// second half of the same question rather than a check of its own: a part of
/// that type whose `text` is missing or blank is a part with nothing to show.
#[must_use]
pub fn part_text(data: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(data).ok()?;
    let text = value.get("text")?.as_str()?;
    (!text.trim().is_empty()).then(|| clamp(text))
}

/// The card the panel draws for one database row.
///
/// `file_path` is the database, not a synthetic `<db>#<session>` string. Orca
/// invents that path so its dispatcher can route a candidate back to this
/// parser (`buildOpenCodeSqliteCandidatePath`) and then notes that the UI needs
/// a real one anyway; we have no dispatcher to route, and a path that does not
/// exist is a "Reveal in Finder" that fails.
#[must_use]
pub fn card(db_path: &str, row: &SessionRow, lines: &[Line]) -> VaultSession {
    let touched = touched_ms(row);
    let parsed = Parsed {
        session_id: row.id.clone(),
        title: row.title.clone(),
        cwd: row
            .directory
            .as_deref()
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
            .map(str::to_string),
        // The store records no branch. `None` rather than a guess from the
        // directory: the panel says "no branch" honestly, and a repository's
        // current branch is not the one this conversation ran on.
        branch: None,
        model: row
            .model_json
            .as_deref()
            .and_then(crate::usage_stats_opencode::named_model),
        created_at: stamp(row.created_ms),
        updated_at: stamp(row.updated_ms),
        message_count: row.message_count,
        preview: preview(lines),
        not_a_session: false,
        ..Parsed::default()
    };
    crate::vault::card(SLUG, db_path.to_string(), iso_utc_of(touched), None, parsed)
}

/// The conversation lines a card shows, oldest first — see the module note on
/// why this end of the conversation.
#[must_use]
pub fn preview(lines: &[Line]) -> Vec<PreviewMessage> {
    lines
        .iter()
        .filter_map(|line| {
            Some(PreviewMessage {
                role: line.role.clone(),
                text: part_text(&line.data)?,
            })
        })
        .take(PREVIEW_LINES)
        .collect()
}

/// An epoch stamp as the panel's ISO string, and `None` for the zero that
/// means "this generation of the table did not record it".
fn stamp(epoch_ms: i64) -> Option<String> {
    (epoch_ms > 0).then(|| iso_utc_of(epoch_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> SessionRow {
        SessionRow {
            id: "ses_080bd9e65ffemYcw1p1XIbETRO".to_string(),
            title: "Greeting".to_string(),
            directory: Some("/Users/dev".to_string()),
            model_json: Some(r#"{"id":"gpt-5.5-fast","providerID":"openai"}"#.to_string()),
            created_ms: 1_784_546_484_634,
            updated_ms: 1_784_546_530_207,
            message_count: 4,
        }
    }

    fn line(role: &str, text: &str) -> Line {
        Line {
            role: role.to_string(),
            data: format!(r#"{{"type":"text","text":{}}}"#, json_string(text)),
        }
    }

    fn json_string(text: &str) -> String {
        serde_json::to_string(text).expect("a string is JSON")
    }

    /// The whole card, out of one measured row.
    #[test]
    fn a_database_row_becomes_the_same_kind_of_card_as_a_file() {
        let built = card(
            "/h/.local/share/opencode/opencode.db",
            &row(),
            &[line("user", "안녕"), line("assistant", "안녕하세요")],
        );
        assert_eq!(built.agent, SLUG);
        assert_eq!(built.session_id, "ses_080bd9e65ffemYcw1p1XIbETRO");
        assert_eq!(built.title, "Greeting");
        assert_eq!(built.cwd.as_deref(), Some("/Users/dev"));
        assert_eq!(built.model.as_deref(), Some("openai/gpt-5.5-fast"));
        assert_eq!(built.file_path, "/h/.local/share/opencode/opencode.db");
        assert_eq!(
            built.id,
            "opencode:ses_080bd9e65ffemYcw1p1XIbETRO:/h/.local/share/opencode/opencode.db"
        );
        assert_eq!(built.message_count, 4);
        assert_eq!(built.preview.len(), 2);
        assert_eq!(built.preview[0].text, "안녕");
        // The stamps are the row's own, and the sort stamp is the newer one.
        assert_eq!(
            built.created_at.as_deref(),
            Some("2026-07-20T11:21:24.634Z")
        );
        assert_eq!(built.modified_at, "2026-07-20T11:22:10.207Z");
        // The spelling `opencode --session` was already in the resume table.
        assert_eq!(
            built.resume.as_deref(),
            Some("cd '/Users/dev' && opencode --session 'ses_080bd9e65ffemYcw1p1XIbETRO'")
        );
        assert!(built.resumable());
    }

    /// A session created and never spoken to again is sorted by its creation.
    #[test]
    fn a_never_updated_session_is_dated_by_its_creation() {
        let mut untouched = row();
        untouched.updated_ms = 0;
        assert_eq!(touched_ms(&untouched), 1_784_546_484_634);
        let built = card("/h/opencode.db", &untouched, &[]);
        assert_eq!(built.modified_at, "2026-07-20T11:21:24.634Z");
        // `updated_at` is not invented; the panel's sort falls back to the
        // stamp above on its own (`VaultSession::sort_key`).
        assert_eq!(
            built.updated_at.as_deref(),
            Some("2026-07-20T11:21:24.634Z")
        );
    }

    /// An untitled session is named by what the person said, not by its id.
    #[test]
    fn an_untitled_session_is_named_by_its_first_words() {
        let mut untitled = row();
        untitled.title = String::new();
        let built = card(
            "/h/opencode.db",
            &untitled,
            &[line("user", "report.py가 죽습니다 고쳐주세요")],
        );
        assert_eq!(built.title, "report.py가 죽습니다 고쳐주세요");
    }

    /// A part with no text is not a preview line, and the cap is the card's.
    #[test]
    fn the_preview_keeps_three_lines_that_say_something() {
        let lines = vec![
            Line {
                role: "user".to_string(),
                data: r#"{"type":"text","text":"   "}"#.to_string(),
            },
            line("assistant", "하나"),
            Line {
                role: "user".to_string(),
                data: "not json".to_string(),
            },
            line("user", "둘"),
            line("assistant", "셋"),
            line("user", "넷"),
        ];
        let kept = preview(&lines);
        assert_eq!(kept.len(), PREVIEW_LINES);
        assert_eq!(
            kept.iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["하나", "둘", "셋"],
            "the opening of the conversation, blanks skipped"
        );
    }

    /// A generation of the table without the `model` column says nothing.
    #[test]
    fn a_model_nobody_recorded_is_not_guessed() {
        let mut plain = row();
        plain.model_json = None;
        assert!(card("/h/opencode.db", &plain, &[]).model.is_none());
        plain.model_json = Some("{}".to_string());
        assert!(card("/h/opencode.db", &plain, &[]).model.is_none());
    }

    /// A directory of blanks is no directory: the resume must not `cd ''`.
    #[test]
    fn a_blank_directory_is_no_directory() {
        let mut nowhere = row();
        nowhere.directory = Some("   ".to_string());
        let built = card("/h/opencode.db", &nowhere, &[]);
        assert!(built.cwd.is_none());
        assert_eq!(
            built.resume.as_deref(),
            Some("opencode --session 'ses_080bd9e65ffemYcw1p1XIbETRO'")
        );
    }
}
