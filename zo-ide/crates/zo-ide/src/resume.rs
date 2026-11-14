//! Session-list and continuation helpers for the plain `zo` front-end.
//!
//! The existing session registry already owns discovery, ordering, transcript
//! parsing, and the `latest` resolution rules. This module gives that internal
//! result a public, small API for the IDE picker and renders the no-argument
//! `--resume` view. A transcript does not carry its workspace path, so new
//! sessions write a tiny `.cwd` sidecar; old sessions remain listable with an
//! explicit "not recorded" location.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::session_registry;

/// A session row suitable for a picker or a non-interactive resume listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeSession {
    pub id: String,
    pub name: Option<String>,
    pub path: PathBuf,
    pub modified_epoch_millis: u128,
    /// 트랜스크립트 머리글의 `created_at_ms` — `/resume` 의 `Sort: [Created]`
    /// 가 읽는 축이고 수정 시각과는 다르다(오래 산 세션은 둘이 몇 시간 벌어진다).
    pub created_epoch_millis: u128,
    pub cwd: Option<PathBuf>,
    pub first_prompt: Option<String>,
}

/// Alias that reads naturally at call sites that refer to the rows as a list.
pub type SessionSummary = ResumeSession;

/// Every limit of the resume list, in one table (t-2947).
pub mod limits {
    /// Rows printed by the short no-argument `--resume` view. The TUI
    /// searches all eligible sessions and has independent result pages.
    pub const LIST_LIMIT: usize = 20;
    /// Characters of the first prompt a row carries. Read from the head of
    /// the transcript, never from further in.
    pub const FIRST_PROMPT_PREVIEW_CHARS: usize = 60;
    /// Characters of the one-line summary the `--resume` view prints per row.
    pub const SUMMARY_LINE_CHARS: usize = 80;
}

/// Maximum number of rows printed by the no-argument resume view.
pub const DEFAULT_LIST_LIMIT: usize = limits::LIST_LIMIT;

/// Return recent sessions for the current workspace, newest first.
pub fn list_recent_sessions() -> Result<Vec<ResumeSession>, Box<dyn std::error::Error>> {
    list_recent_sessions_limited(DEFAULT_LIST_LIMIT)
}

/// Return at most `limit` recent sessions for the current workspace, newest
/// first. The registry fills the limit with usable transcript heads, without
/// loading the conversations in full.
/// 최근 세션 목록 — `<id>.system-prompt.json` 사이드카는 세션이 아니므로 뺀다.
pub fn list_recent_sessions_limited(
    limit: usize,
) -> Result<Vec<ResumeSession>, Box<dyn std::error::Error>> {
    session_registry::list_managed_sessions_limited(Some(limit)).map(|sessions| {
        sessions
            .into_iter()
            .map(|session| ResumeSession {
                cwd: load_session_cwd(&session.path),
                id: session.id,
                name: session.name,
                path: session.path,
                modified_epoch_millis: session.modified_epoch_millis,
                created_epoch_millis: session.created_epoch_millis,
                first_prompt: Some(session.first_user_text.unwrap_or_else(|| crate::tui::strings::MCP_IMAGE_ITEM.to_string())),
            })
            .collect()
    })
}

/// Search every eligible transcript head, then page the matching summaries.
/// This never loads full conversations. TUI callers run it on a worker thread.
pub fn search_sessions(query: &str, offset: usize, limit: usize) -> Result<Vec<ResumeSession>, Box<dyn std::error::Error>> {
    search_sessions_cancellable(query, offset, limit, None, &std::sync::atomic::AtomicBool::new(false))
}

pub(crate) fn search_sessions_cancellable(query: &str, offset: usize, limit: usize, cwd_filter: Option<&Path>, cancelled: &std::sync::atomic::AtomicBool) -> Result<Vec<ResumeSession>, Box<dyn std::error::Error>> {
    Ok(session_registry::search_session_summaries(query, offset, limit, cwd_filter, cancelled)?.into_iter().map(|session| ResumeSession {
        cwd: load_session_cwd(&session.path), id: session.id, name: session.name, path: session.path,
        modified_epoch_millis: session.modified_epoch_millis, created_epoch_millis: session.created_epoch_millis,
        first_prompt: Some(session.first_user_text.unwrap_or_else(|| crate::tui::strings::MCP_IMAGE_ITEM.to_string())),
    }).collect())
}

/// Return the most recent session in the current workspace, if one exists.
pub fn latest_session() -> Result<Option<ResumeSession>, Box<dyn std::error::Error>> {
    Ok(list_recent_sessions_limited(1)?.into_iter().next())
}

/// The id that `--continue` should use for the current workspace.
pub fn continue_session_id() -> Result<Option<String>, Box<dyn std::error::Error>> {
    Ok(latest_session()?.map(|session| session.id))
}

/// Render a deterministic, append-only recent-session list for `--resume`
/// without an id. It intentionally does not open a runtime or contact a
/// provider, so asking for the list is safe in a pipe and in an unauthenticated
/// environment.
pub fn render_recent_sessions(limit: usize) -> Result<String, Box<dyn std::error::Error>> {
    let sessions = list_recent_sessions_limited(limit)?;
    let mut lines = vec![
        "Recent sessions".to_string(),
        "  Use `zo --resume <id>` to restore one; `zo --continue` chooses the newest conversation.".to_string(),
    ];
    if sessions.is_empty() {
        lines.push("  (none found in the current workspace)".to_string());
        return Ok(lines.join("\n"));
    }

    for session in sessions {
        let cwd = session
            .cwd
            .as_deref()
            .map_or_else(|| "(not recorded)".to_string(), |path| path.display().to_string());
        let prompt = session
            .first_prompt
            .as_deref()
            .map_or_else(|| "(no prompt)".to_string(), one_line_summary);
        lines.push(format!(
            "  {id:<24} {time}  cwd={cwd}  prompt={prompt}",
            id = session.id,
            time = format_timestamp(session.modified_epoch_millis),
        ));
    }
    Ok(lines.join("\n"))
}

/// Path of the workspace sidecar associated with a transcript.
///
/// The extension is deliberately neither `jsonl` nor `json`, because the
/// canonical registry treats those extensions as resumable transcript files.
#[must_use]
pub fn session_cwd_path(session_path: &Path) -> PathBuf {
    session_path.with_extension(runtime::session_control::SESSION_CWD_EXTENSION)
}

/// Record a session's workspace once, without overwriting an existing record.
/// This is crate-visible because `PlainSession` owns session creation; the
/// path helper remains public for picker integrations and tests.
pub(crate) fn write_session_cwd_if_missing(session_path: &Path, cwd: &Path) -> io::Result<()> {
    let path = session_cwd_path(session_path);
    if path.exists() {
        return Ok(());
    }
    let metadata = SessionCwd {
        cwd: cwd.to_string_lossy().into_owned(),
    };
    let payload = serde_json::to_vec(&metadata).map_err(io::Error::other)?;
    crate::write_atomic(&path, &payload)
}

pub(crate) fn load_session_cwd(session_path: &Path) -> Option<PathBuf> {
    let raw = fs::read_to_string(session_cwd_path(session_path)).ok()?;
    let metadata = serde_json::from_str::<SessionCwd>(&raw).ok()?;
    (!metadata.cwd.trim().is_empty()).then(|| PathBuf::from(metadata.cwd))
}

fn one_line_summary(prompt: &str) -> String {
    const MAX_CHARS: usize = limits::SUMMARY_LINE_CHARS;

    let mut summary = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.chars().count() > MAX_CHARS {
        summary = summary.chars().take(MAX_CHARS - 1).collect();
        summary.push('…');
    }
    summary
}

fn format_timestamp(modified_epoch_millis: u128) -> String {
    let seconds = u64::try_from(modified_epoch_millis / 1_000).unwrap_or(u64::MAX);
    let seconds_today = seconds % 86_400;
    let date = core_types::date::utc_date_from_unix_secs(seconds);
    format!(
        "{date} {:02}:{:02}:{:02}Z",
        seconds_today / 3_600,
        (seconds_today % 3_600) / 60,
        seconds_today % 60,
    )
}

#[derive(Debug, Deserialize, Serialize)]
struct SessionCwd {
    cwd: String,
}

/// Path of the agent-registry locator sidecar beside a transcript
/// (`<session>.registry`): the `registries/<sid>.json` record the session's
/// helper store is described by, so `/resume` from another directory comes
/// back to the same root (t-2511). The extension is neither `jsonl` nor
/// `json` for the same reason as [`session_cwd_path`].
#[must_use]
pub fn session_registry_locator_path(session_path: &Path) -> PathBuf {
    session_path.with_extension(runtime::session_control::SESSION_REGISTRY_EXTENSION)
}

/// Record where the session's agent registry lives. Overwrites only when the
/// locator actually moved, so a resumed session does no write at all.
pub(crate) fn write_session_registry_locator(session_path: &Path, locator: &Path) -> io::Result<()> {
    if load_session_registry_locator(session_path).as_deref() == Some(locator) {
        return Ok(());
    }
    let metadata = SessionRegistryLocator {
        locator: locator.to_string_lossy().into_owned(),
    };
    let payload = serde_json::to_vec(&metadata).map_err(io::Error::other)?;
    crate::write_atomic(&session_registry_locator_path(session_path), &payload)
}

/// The locator a session remembered, if it was born with a registry.
pub(crate) fn load_session_registry_locator(session_path: &Path) -> Option<PathBuf> {
    let raw = fs::read_to_string(session_registry_locator_path(session_path)).ok()?;
    let metadata = serde_json::from_str::<SessionRegistryLocator>(&raw).ok()?;
    (!metadata.locator.trim().is_empty()).then(|| PathBuf::from(metadata.locator))
}

#[derive(Debug, Deserialize, Serialize)]
struct SessionRegistryLocator {
    locator: String,
}

/// Where the model and effort THIS conversation runs on are kept: its
/// preferences file, `session-prefs/<stem>.json` beside the project's
/// `sessions/` folder.
///
/// `settings.json` holds one model and one effort, the defaults a NEW session
/// opens with, and every `/model` rewrites them. Reopening a conversation from
/// those defaults handed it whatever another conversation saved last: three
/// panes on Claude, Gemini Flash and a third model came back from a window
/// restart all on the third. The conversation's own choice lives here, so a
/// reopen — the window's restore, `zo --resume`, the in-pane `/resume` —
/// brings it back.
///
/// The retention sweep (`runtime::session_control`) deletes this file with
/// its transcript, and the previous zo CLI kept the same `model` and `effort`
/// fields here. A file beside the transcript would outlive it or die before
/// it: the sweep keeps every non-transcript file in `sessions/` forever, so
/// an expired project folder never empties, and it deletes a `json` one by
/// its own age while the conversation lives on.
///
/// `None` for a transcript outside a `sessions/` folder, such as an exported
/// file resumed by its path: a record there would put a new folder in the
/// person's own directory.
#[must_use]
pub fn session_selection_path(session_path: &Path) -> Option<PathBuf> {
    let sessions = session_path.parent()?;
    if sessions.file_name()? != "sessions" {
        return None;
    }
    let stem = session_path.file_stem()?.to_str()?;
    Some(
        sessions
            .parent()?
            .join(runtime::session_control::SESSION_PREFERENCES_DIR)
            .join(format!("{stem}.json")),
    )
}

/// The model and effort a conversation runs on, as its preferences file keeps
/// them.
///
/// Stored the way `settings.json` stores a choice (`crate::preferences`): a
/// family head by its alias, a `/fast` tier as its bare family. A session the
/// router chooses for records no model, so a reopen leaves it automatic.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct SessionSelection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    effort: Option<String>,
    /// The provider of the person's own that served `model` when this was
    /// recorded, by the name they gave it; none for a model a built-in
    /// provider serves. A reopen is held to it ([`Self::reopen_refusal`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
}

impl SessionSelection {
    /// The record of a pinned `model` (`None` for a router-chosen one) and
    /// `effort`, in the stored spelling.
    #[must_use]
    pub fn of(model: Option<&str>, effort: Option<crate::effort::Effort>) -> Self {
        Self {
            model: model
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(|model| {
                    crate::preferences::pinned_model_id(&crate::preferences::persistable_model_id(
                        model,
                    ))
                }),
            effort: effort.map(|effort| effort.canonical().to_string()),
            provider: None,
        }
    }

    /// This record, naming the provider of the person's own that serves its
    /// model now — read the way the session's client is built
    /// ([`runtime::model_home::ModelHome::of`]) — or none, where a built-in
    /// provider serves it or the record names no model.
    #[must_use]
    pub fn at_the_provider_serving_it(mut self) -> Self {
        self.provider = self.model().and_then(|model| match runtime::model_home::ModelHome::of(&model) {
            runtime::model_home::ModelHome::Own(name) => Some(name),
            _ => None,
        });
        self
    }

    /// Why a conversation that ran at a provider of the person's own may not
    /// reopen on `model` — `None` where it may. A reopen builds the session
    /// afresh, from wherever the name reads now: once that provider no
    /// longer serves the model (its row gone from `providers`, or another of
    /// theirs listing the model first), the whole conversation would go
    /// there — a `gpt-…` id to `OpenAI`, a name nobody knows to whichever
    /// login the machine holds (152, review t-107693). Reopened on another
    /// model than the one it ran on, it goes where the person sent it.
    #[must_use]
    pub fn reopen_refusal(&self, model: &str) -> Option<String> {
        let provider = self.provider.as_deref()?;
        let ran_on = self.model()?;
        let model = model.trim();
        if ran_on != model && ran_on != crate::cli_args::resolve_model_alias(model) {
            return None;
        }
        match runtime::model_home::ModelHome::of(model) {
            runtime::model_home::ModelHome::Own(now) if now == provider => None,
            _ => Some(format!(
                "This conversation ran on `{model}` at your provider `{provider}`, which no longer serves it \
                 (its row is gone from `providers`, or another of your providers now lists the model first). \
                 zo does not reopen it anywhere else: put the row back, or reopen it on another model (`--model <name>`)."
            )),
        }
    }

    /// The recorded model, resolved the way a saved default is.
    #[must_use]
    pub fn model(&self) -> Option<String> {
        self.model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(crate::cli_args::resolve_model_alias)
    }

    /// The recorded effort, when it still names a level.
    #[must_use]
    pub fn effort(&self) -> Option<crate::effort::Effort> {
        self.effort.as_deref().and_then(crate::effort::Effort::from_token)
    }
}

/// Record the selection a conversation runs on. Writes only when it changed,
/// so reopening a conversation on the choice it already had writes nothing,
/// and writes nothing for a transcript outside a `sessions/` folder.
pub(crate) fn write_session_selection(
    session_path: &Path,
    selection: &SessionSelection,
) -> io::Result<()> {
    let Some(path) = session_selection_path(session_path) else {
        return Ok(());
    };
    if load_session_selection(session_path).as_ref() == Some(selection) {
        return Ok(());
    }
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder)?;
    }
    let payload = serde_json::to_vec(selection).map_err(io::Error::other)?;
    crate::write_atomic(&path, &payload)
}

/// The selection a conversation recorded, if it has run since records began.
pub(crate) fn load_session_selection(session_path: &Path) -> Option<SessionSelection> {
    let raw = fs::read_to_string(session_selection_path(session_path)?).ok()?;
    serde_json::from_str::<SessionSelection>(&raw).ok()
}

#[cfg(test)]
mod tests {
    use super::{
        SessionSelection, format_timestamp, load_session_selection, one_line_summary,
        session_selection_path, write_session_selection,
    };
    use crate::effort::Effort;

    #[test]
    fn a_selection_record_round_trips_in_the_projects_session_prefs() {
        let dir = tempfile::tempdir().expect("temp dir");
        let transcript = dir.path().join("sessions").join("session-1-0.jsonl");
        assert_eq!(load_session_selection(&transcript), None, "no record yet");

        let flash = SessionSelection::of(Some("gemini-3.8-flash"), Some(Effort::High));
        write_session_selection(&transcript, &flash).expect("write");
        assert_eq!(load_session_selection(&transcript).as_ref(), Some(&flash));
        assert_eq!(flash.model().as_deref(), Some("gemini-3.8-flash"));
        assert_eq!(flash.effort(), Some(Effort::High));
        assert_eq!(
            session_selection_path(&transcript),
            Some(dir.path().join("session-prefs").join("session-1-0.json")),
        );
    }

    #[test]
    fn a_transcript_outside_a_sessions_folder_keeps_no_record() {
        let dir = tempfile::tempdir().expect("temp dir");
        let exported = dir.path().join("exported.jsonl");
        assert_eq!(session_selection_path(&exported), None);
        let record = SessionSelection::of(Some("gemini-3.8-flash"), Some(Effort::High));
        write_session_selection(&exported, &record).expect("nothing to write");
        assert_eq!(load_session_selection(&exported), None);
        assert_eq!(
            std::fs::read_dir(dir.path()).expect("list").count(),
            0,
            "the person's folder gains nothing"
        );
    }

    #[test]
    fn the_retention_sweep_keeps_a_record_as_long_as_its_conversation() {
        let root = tempfile::tempdir().expect("temp dir");
        let sessions = |project: &str| root.path().join(project).join("sessions");
        let live = sessions("acme-live").join("live-1.jsonl");
        let expired = sessions("acme-expired").join("expired-1.jsonl");
        let record = SessionSelection::of(Some("gemini-3.8-flash"), Some(Effort::High));
        for transcript in [&live, &expired] {
            std::fs::create_dir_all(transcript.parent().expect("sessions")).expect("sessions");
            std::fs::write(transcript, "{}\n").expect("transcript");
            write_session_selection(transcript, &record).expect("record");
        }
        let now = std::time::SystemTime::now();
        let year = std::time::Duration::from_secs(365 * 24 * 60 * 60);
        let set_age = |path: &std::path::Path, modified: std::time::SystemTime| {
            std::fs::File::options()
                .write(true)
                .open(path)
                .and_then(|file| file.set_modified(modified))
                .expect("set the file's age");
        };
        // The live conversation's record is old, its transcript fresh.
        set_age(&session_selection_path(&live).expect("record path"), now - year);
        set_age(&expired, now - year);

        let report =
            runtime::session_control::cleanup_expired_sessions_under(root.path(), now - year / 2);
        assert_eq!(report.removed_sessions, 1);
        assert_eq!(report.removed_prefs, 1, "the expired conversation's record went with it");
        assert_eq!(load_session_selection(&live).as_ref(), Some(&record));
        assert!(
            !root.path().join("acme-expired").exists(),
            "nothing is left to keep the expired project's folder"
        );
    }

    #[test]
    fn an_unchanged_selection_is_not_written_again() {
        let dir = tempfile::tempdir().expect("temp dir");
        let transcript = dir.path().join("sessions").join("session-2-0.jsonl");
        let record = SessionSelection::of(Some("gemini-3.8-flash"), Some(Effort::Xhigh));
        write_session_selection(&transcript, &record).expect("first write");
        let path = session_selection_path(&transcript).expect("record path");
        std::fs::write(&path, br#"{"model":"gemini-3.8-flash","effort":"xhigh"}"#)
            .expect("same record, other bytes");
        write_session_selection(&transcript, &record).expect("second write");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            r#"{"model":"gemini-3.8-flash","effort":"xhigh"}"#,
            "a reopen on the same choice leaves the file alone"
        );
    }

    #[test]
    fn a_router_chosen_session_records_no_model() {
        let record = SessionSelection::of(None, Some(Effort::Smart));
        assert_eq!(record.model(), None);
        assert_eq!(record.effort(), Some(Effort::Smart));
        assert_eq!(
            serde_json::to_string(&record).expect("json"),
            r#"{"effort":"smart"}"#
        );
    }

    #[test]
    fn timestamp_is_stable_and_utc() {
        assert_eq!(format_timestamp(0), "1970-01-01 00:00:00Z");
        assert_eq!(
            format_timestamp(1_762_118_400_000),
            "2025-11-02 21:20:00Z"
        );
    }

    #[test]
    fn prompt_summary_is_single_line_and_bounded() {
        assert_eq!(one_line_summary(" first\n second  third "), "first second third");
        let long = "a".repeat(100);
        let summary = one_line_summary(&long);
        assert_eq!(summary.chars().count(), 80);
        assert!(summary.ends_with('…'));
    }

    /// A conversation that ran at a provider of the person's own reopens
    /// there or not at all (152): the record names that provider, and a
    /// reopen that would build the session elsewhere — the row gone, or
    /// another of the person's providers listing the model first — is
    /// refused. Reopened on another model, it goes where the person sent it.
    #[test]
    fn a_conversation_that_ran_at_a_persons_provider_reopens_there_or_not_at_all() {
        let _env_lock = crate::test_env_lock();
        let config_home = crate::support::temp_dir("reopen-own-provider");
        let _config_home = crate::support::EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = crate::support::EnvVarGuard::set("ZO_HOME", None);
        let _custom = crate::support::EnvVarGuard::set(api::CUSTOM_PROVIDERS_ENV, None);
        let declare = |json: &str| api::refresh_custom_providers_from_json(json).expect("custom providers");
        let own = |name: &str, models: &str| {
            format!(r#"{{"name":"{name}","base_url":"https://{name}.invalid/v1","requires_auth":false,"models":[{models}]}}"#)
        };
        declare(&format!("[{}]", own("zqown", r#""gpt-zq-oss","zq-shared""#)));

        let record = SessionSelection::of(Some("gpt-zq-oss"), Some(Effort::High)).at_the_provider_serving_it();
        assert_eq!(record.provider.as_deref(), Some("zqown"));
        assert_eq!(record.reopen_refusal("gpt-zq-oss"), None, "the row stands");
        let stored = serde_json::to_string(&record).expect("the record serializes");
        let read_back: SessionSelection = serde_json::from_str(&stored).expect("and reads back");
        assert_eq!(read_back, record);

        // The row is gone: the name reads as OpenAI's now.
        declare("[]");
        let why = record.reopen_refusal("gpt-zq-oss").expect("refused");
        assert!(why.contains("`zqown`") && why.contains("`gpt-zq-oss`"), "{why}");
        assert!(!why.contains("  "), "a line join left a run of spaces: {why}");
        // Reopened on another model, it goes where the person sent it.
        assert_eq!(record.reopen_refusal("claude-sonnet-5"), None);

        // Another of the person's providers now lists the model first.
        declare(&format!("[{},{}]", own("zqfirst", r#""zq-shared""#), own("zqown", r#""zq-shared""#)));
        let shared = SessionSelection {
            model: Some("zq-shared".to_string()),
            effort: None,
            provider: Some("zqown".to_string()),
        };
        assert!(shared.reopen_refusal("zq-shared").is_some());

        // A built-in provider's model, and a record from before this field, hold nothing.
        let built_in = SessionSelection::of(Some("claude-sonnet-5"), None).at_the_provider_serving_it();
        assert_eq!(built_in.provider, None);
        let older: SessionSelection =
            serde_json::from_str(r#"{"model":"gpt-zq-oss","effort":"high"}"#).expect("an older record");
        assert_eq!(older.reopen_refusal("gpt-zq-oss"), None);
        declare("[]");
    }
}
