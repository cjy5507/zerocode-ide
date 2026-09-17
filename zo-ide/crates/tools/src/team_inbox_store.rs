//! Local-first `TeamInbox` store shared by manual tools and runtime digests.
//!
//! This module implements the storage substrate used by `team_tools.rs` for
//! manual `TeamInbox` operations and by `crates/runtime/src/team_inbox_digest.rs`
//! for turn-start low-trust digest reads. `SQLite` is the authoritative state
//! store, JSONL is an event-sourced audit/recovery log, and large bodies are
//! stored by content address in a TeamInbox-owned artifact namespace.
#![allow(dead_code)] // Runtime-facing store APIs are compiled before every caller is wired in tools.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::artifacts::{ArtifactKind, ArtifactRef};

/// Where the team inbox store lives; the session points it at the project's
/// home so the working tree stays clean (see `artifacts::ARTIFACT_STORE_ENV`).
pub const STORE_ENV: &str = "ZO_TEAM_INBOX_STORE";
const DB_FILE: &str = "team_inbox.sqlite3";
const JSONL_FILE: &str = "team_inbox.jsonl";
const ARTIFACTS_DIR: &str = "artifacts";
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const ARTIFACT_PREVIEW_CHARS: usize = 400;
static ARTIFACT_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub(crate) enum TeamInboxStoreError {
    #[error("TeamInbox store is read-only; retry when SQLite is available")]
    ReadOnly,
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("TeamInbox store is unavailable: {0}")]
    Unavailable(String),
    #[error("invalid TeamInbox input: {0}")]
    InvalidInput(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StoreMode {
    ReadWrite,
    ReadOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TeamInboxPriority {
    Low,
    Normal,
    High,
}

impl TeamInboxPriority {
    fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
        }
    }

    fn from_str(value: &str) -> Result<Self, TeamInboxStoreError> {
        match value {
            "low" => Ok(Self::Low),
            "normal" => Ok(Self::Normal),
            "high" => Ok(Self::High),
            other => Err(TeamInboxStoreError::InvalidInput(format!(
                "unknown priority: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TeamUpdate {
    pub seq: i64,
    pub id: String,
    pub channel: String,
    pub source: String,
    pub created_at_unix: u64,
    pub priority: TeamInboxPriority,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_ref: Option<ArtifactRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NewTeamUpdate {
    pub id: String,
    pub channel: String,
    pub source: String,
    pub created_at_unix: u64,
    pub priority: TeamInboxPriority,
    pub summary: String,
    pub body: Option<String>,
    pub task_id: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TeamInboxDeliveryState {
    Pending,
    Injected,
    Acked,
    Failed,
    Stale,
}

impl TeamInboxDeliveryState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Injected => "injected",
            Self::Acked => "acked",
            Self::Failed => "failed",
            Self::Stale => "stale",
        }
    }

    fn from_str(value: &str) -> Result<Self, TeamInboxStoreError> {
        match value {
            "pending" => Ok(Self::Pending),
            "injected" => Ok(Self::Injected),
            "acked" => Ok(Self::Acked),
            "failed" => Ok(Self::Failed),
            "stale" => Ok(Self::Stale),
            other => Err(TeamInboxStoreError::InvalidInput(format!(
                "unknown delivery state: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeliveryRecord {
    pub update_id: String,
    pub consumer_id: String,
    pub state: TeamInboxDeliveryState,
    pub turn_id: Option<String>,
    pub retry_count: u32,
    pub updated_at_unix: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TeamInboxChannel {
    pub channel: String,
    pub update_count: i64,
    pub last_seq: i64,
    pub last_created_at_unix: u64,
    pub cursor_seq: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum TeamInboxEvent {
    Post { update: TeamUpdate },
    CursorAdvance {
        consumer_id: String,
        channel: String,
        last_seen_seq: i64,
    },
    CursorDelete {
        consumer_id: String,
        channel: String,
    },
    Delivery {
        update_id: String,
        consumer_id: String,
        state: TeamInboxDeliveryState,
        turn_id: Option<String>,
        retry_count: u32,
        updated_at_unix: u64,
    },
}

#[derive(Default)]
struct ReadOnlySnapshot {
    updates: Vec<TeamUpdate>,
    cursors: HashMap<(String, String), i64>,
    deliveries: HashMap<(String, String), DeliveryRecord>,
}

#[derive(Default)]
struct JsonlReplay {
    snapshot: ReadOnlySnapshot,
    malformed_line_count: usize,
}

#[derive(Default)]
struct JsonlEvents {
    events: Vec<TeamInboxEvent>,
    malformed_line_count: usize,
}

pub(crate) struct TeamInboxStore {
    conn: Option<Connection>,
    mode: StoreMode,
    jsonl_path: PathBuf,
    artifact_dir: PathBuf,
    read_only: ReadOnlySnapshot,
    jsonl_malformed_line_count: usize,
}

impl TeamInboxStore {
    pub(crate) fn open_at(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let jsonl_path = root.join(JSONL_FILE);
        let artifact_dir = root.join(ARTIFACTS_DIR);
        let replay = replay_jsonl(&jsonl_path).unwrap_or_default();

        let conn = open_sqlite(&root).ok();
        let mode = if conn.is_some() {
            StoreMode::ReadWrite
        } else {
            StoreMode::ReadOnly
        };
        let mut store = Self {
            conn,
            mode,
            jsonl_path,
            artifact_dir,
            read_only: replay.snapshot,
            jsonl_malformed_line_count: replay.malformed_line_count,
        };
        if store.conn.is_some() {
            let _ = store.flush_outbox();
            let _ = store.reconcile_jsonl_events_if_empty();
        }
        store
    }

    pub(crate) fn open_read_only_at(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let jsonl_path = root.join(JSONL_FILE);
        let artifact_dir = root.join(ARTIFACTS_DIR);
        let replay = replay_jsonl(&jsonl_path).unwrap_or_default();
        Self {
            conn: open_sqlite_read_only(&root).ok(),
            mode: StoreMode::ReadOnly,
            jsonl_path,
            artifact_dir,
            read_only: replay.snapshot,
            jsonl_malformed_line_count: replay.malformed_line_count,
        }
    }

    pub(crate) fn mode(&self) -> StoreMode {
        self.mode
    }

    /// Count malformed non-blank JSONL records skipped during this store open.
    /// `SQLite` remains authoritative whenever it is available.
    pub(crate) fn malformed_jsonl_line_count(&self) -> usize {
        self.jsonl_malformed_line_count
    }

    pub(crate) fn post_update(
        &mut self,
        input: NewTeamUpdate,
    ) -> Result<TeamUpdate, TeamInboxStoreError> {
        validate_update_input(&input)?;
        let artifact_dir = self.artifact_dir.clone();
        let update = {
            let conn = self.conn_mut()?;
            let tx = conn.transaction()?;

            let body = input.body;
            let body_ref = body.as_deref().map(body_artifact_ref);
            let update_without_seq = TeamUpdate {
                seq: 0,
                id: input.id,
                channel: input.channel,
                source: input.source,
                created_at_unix: input.created_at_unix,
                priority: input.priority,
                summary: input.summary,
                body_ref,
                task_id: input.task_id,
                status: input.status,
            };

            let inserted = tx.execute(
                "INSERT OR IGNORE INTO updates
                 (id, channel, source, created_at_unix, priority, summary, body_ref_json, task_id, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    update_without_seq.id,
                    update_without_seq.channel,
                    update_without_seq.source,
                    to_sql_i64("created_at_unix", update_without_seq.created_at_unix)?,
                    update_without_seq.priority.as_str(),
                    update_without_seq.summary,
                    serialize_optional(update_without_seq.body_ref.as_ref())?,
                    update_without_seq.task_id,
                    update_without_seq.status,
                ],
            )? == 1;
            let update = query_update_in_tx(&tx, &update_without_seq.id)?;
            if inserted {
                if let Some(body) = body.as_deref() {
                    write_body_artifact(&artifact_dir, body)?;
                }
                enqueue_event_in_tx(&tx, &TeamInboxEvent::Post { update: update.clone() })?;
            }
            tx.commit()?;
            update
        };
        self.flush_outbox()?;
        Ok(update)
    }

    pub(crate) fn join_channel_from_now(
        &mut self,
        consumer_id: &str,
        channel: &str,
    ) -> Result<i64, TeamInboxStoreError> {
        require_non_empty("consumer_id", consumer_id)?;
        require_non_empty("channel", channel)?;
        let tail = {
            let conn = self.conn_mut()?;
            let tx = conn.transaction()?;
            let tail = channel_tail_seq_in_tx(&tx, channel)?;
            upsert_cursor_in_tx(&tx, consumer_id, channel, tail)?;
            enqueue_event_in_tx(
                &tx,
                &TeamInboxEvent::CursorAdvance {
                    consumer_id: consumer_id.to_owned(),
                    channel: channel.to_owned(),
                    last_seen_seq: tail,
                },
            )?;
            tx.commit()?;
            tail
        };
        self.flush_outbox()?;
        Ok(tail)
    }

    pub(crate) fn list_channels(
        &self,
        consumer_id: Option<&str>,
    ) -> Result<Vec<TeamInboxChannel>, TeamInboxStoreError> {
        if let Some(consumer_id) = consumer_id {
            require_non_empty("consumer_id", consumer_id)?;
        }
        match &self.conn {
            Some(conn) => list_channels_sqlite(conn, consumer_id),
            None => Ok(list_channels_snapshot(&self.read_only, consumer_id)),
        }
    }

    /// Delete the `(consumer_id, channel)` cursor row, keeping delivery
    /// history. Tool-layer callers are guarded against `session:` consumers
    /// (`reject_reserved_team_inbox_consumer` in `team_tools.rs`); this store
    /// method itself does not enforce that invariant, and the runtime's
    /// turn-settle path upserts cursors unconditionally — so a store-level
    /// leave of a live `session:` consumer would be resurrected by the next
    /// settled turn. Keep non-tool callers away from reserved consumers.
    pub(crate) fn leave_channel(
        &mut self,
        consumer_id: &str,
        channel: &str,
    ) -> Result<(), TeamInboxStoreError> {
        require_non_empty("consumer_id", consumer_id)?;
        require_non_empty("channel", channel)?;
        {
            let conn = self.conn_mut()?;
            let tx = conn.transaction()?;
            let deleted = tx.execute(
                "DELETE FROM cursors WHERE consumer_id = ?1 AND channel = ?2",
                params![consumer_id, channel],
            )?;
            if deleted == 0 {
                return Err(TeamInboxStoreError::InvalidInput(format!(
                    "consumer_id {consumer_id:?} is not joined to channel {channel:?}; cannot leave"
                )));
            }
            enqueue_event_in_tx(
                &tx,
                &TeamInboxEvent::CursorDelete {
                    consumer_id: consumer_id.to_owned(),
                    channel: channel.to_owned(),
                },
            )?;
            tx.commit()?;
        }
        self.flush_outbox()?;
        Ok(())
    }

    pub(crate) fn unread_updates(
        &self,
        consumer_id: &str,
        channel: &str,
        limit: usize,
    ) -> Result<Vec<TeamUpdate>, TeamInboxStoreError> {
        require_non_empty("consumer_id", consumer_id)?;
        require_non_empty("channel", channel)?;
        if limit == 0 {
            return Ok(Vec::new());
        }
        match &self.conn {
            Some(conn) => unread_sqlite(conn, consumer_id, channel, limit),
            None => Ok(unread_snapshot(&self.read_only, consumer_id, channel, limit)),
        }
    }

    pub(crate) fn cursor(
        &self,
        consumer_id: &str,
        channel: &str,
    ) -> Result<Option<i64>, TeamInboxStoreError> {
        require_non_empty("consumer_id", consumer_id)?;
        require_non_empty("channel", channel)?;
        match &self.conn {
            Some(conn) => cursor_row(conn, consumer_id, channel),
            None => Ok(self
                .read_only
                .cursors
                .get(&(consumer_id.to_owned(), channel.to_owned()))
                .copied()),
        }
    }

    pub(crate) fn update(&self, update_id: &str) -> Result<Option<TeamUpdate>, TeamInboxStoreError> {
        require_non_empty("update_id", update_id)?;
        match &self.conn {
            Some(conn) => query_update_optional(conn, update_id),
            None => Ok(self
                .read_only
                .updates
                .iter()
                .find(|update| update.id == update_id)
                .cloned()),
        }
    }

    pub(crate) fn mark_injected(
        &mut self,
        consumer_id: &str,
        update_id: &str,
        turn_id: &str,
        updated_at_unix: u64,
    ) -> Result<(), TeamInboxStoreError> {
        require_non_empty("turn_id", turn_id)?;
        self.write_delivery(
            consumer_id,
            update_id,
            TeamInboxDeliveryState::Injected,
            Some(turn_id.to_owned()),
            updated_at_unix,
        )
    }

    pub(crate) fn ack_update(
        &mut self,
        consumer_id: &str,
        channel: &str,
        update_id: &str,
        turn_id: &str,
        updated_at_unix: u64,
    ) -> Result<i64, TeamInboxStoreError> {
        require_non_empty("consumer_id", consumer_id)?;
        require_non_empty("channel", channel)?;
        require_non_empty("update_id", update_id)?;
        require_non_empty("turn_id", turn_id)?;
        let advanced_to = {
            let conn = self.conn_mut()?;
            let tx = conn.transaction()?;
            let retry_count = validate_ack_in_tx(&tx, consumer_id, channel, update_id, turn_id)?;
            upsert_delivery_in_tx(
                &tx,
                consumer_id,
                update_id,
                TeamInboxDeliveryState::Acked,
                Some(turn_id),
                retry_count,
                updated_at_unix,
            )?;
            enqueue_event_in_tx(
                &tx,
                &TeamInboxEvent::Delivery {
                    update_id: update_id.to_owned(),
                    consumer_id: consumer_id.to_owned(),
                    state: TeamInboxDeliveryState::Acked,
                    turn_id: Some(turn_id.to_owned()),
                    retry_count,
                    updated_at_unix,
                },
            )?;
            let advanced_to = contiguous_terminal_seq_in_tx(&tx, consumer_id, channel)?;
            upsert_cursor_in_tx(&tx, consumer_id, channel, advanced_to)?;
            enqueue_event_in_tx(
                &tx,
                &TeamInboxEvent::CursorAdvance {
                    consumer_id: consumer_id.to_owned(),
                    channel: channel.to_owned(),
                    last_seen_seq: advanced_to,
                },
            )?;
            tx.commit()?;
            advanced_to
        };
        self.flush_outbox()?;
        Ok(advanced_to)
    }

    pub(crate) fn record_failure(
        &mut self,
        consumer_id: &str,
        channel: &str,
        update_id: &str,
        turn_id: &str,
        updated_at_unix: u64,
        max_retries: u32,
    ) -> Result<TeamInboxDeliveryState, TeamInboxStoreError> {
        require_non_empty("consumer_id", consumer_id)?;
        require_non_empty("channel", channel)?;
        require_non_empty("update_id", update_id)?;
        require_non_empty("turn_id", turn_id)?;
        if max_retries == 0 {
            return Err(TeamInboxStoreError::InvalidInput(
                "max_retries must be greater than zero".into(),
            ));
        }
        let final_state = {
            let conn = self.conn_mut()?;
            let tx = conn.transaction()?;
            ensure_update_in_channel_in_tx(&tx, update_id, channel)?;
            let retry_count = validate_failure_in_tx(&tx, consumer_id, update_id, turn_id)?
                .saturating_add(1);
            let state = if retry_count >= max_retries {
                TeamInboxDeliveryState::Stale
            } else {
                TeamInboxDeliveryState::Failed
            };
            upsert_delivery_in_tx(
                &tx,
                consumer_id,
                update_id,
                state,
                Some(turn_id),
                retry_count,
                updated_at_unix,
            )?;
            enqueue_event_in_tx(
                &tx,
                &TeamInboxEvent::Delivery {
                    update_id: update_id.to_owned(),
                    consumer_id: consumer_id.to_owned(),
                    state,
                    turn_id: Some(turn_id.to_owned()),
                    retry_count,
                    updated_at_unix,
                },
            )?;
            if state == TeamInboxDeliveryState::Stale {
                let advanced_to = contiguous_terminal_seq_in_tx(&tx, consumer_id, channel)?;
                upsert_cursor_in_tx(&tx, consumer_id, channel, advanced_to)?;
                enqueue_event_in_tx(
                    &tx,
                    &TeamInboxEvent::CursorAdvance {
                        consumer_id: consumer_id.to_owned(),
                        channel: channel.to_owned(),
                        last_seen_seq: advanced_to,
                    },
                )?;
            }
            tx.commit()?;
            state
        };
        self.flush_outbox()?;
        Ok(final_state)
    }

    pub(crate) fn mark_stale(
        &mut self,
        consumer_id: &str,
        channel: &str,
        update_id: &str,
        updated_at_unix: u64,
    ) -> Result<i64, TeamInboxStoreError> {
        require_non_empty("consumer_id", consumer_id)?;
        require_non_empty("channel", channel)?;
        require_non_empty("update_id", update_id)?;
        let advanced_to = {
            let conn = self.conn_mut()?;
            let tx = conn.transaction()?;
            ensure_update_in_channel_in_tx(&tx, update_id, channel)?;
            let existing = query_delivery_in_tx(&tx, consumer_id, update_id)?;
            if matches!(
                existing.as_ref().map(|record| record.state),
                Some(TeamInboxDeliveryState::Acked)
            ) {
                return Err(TeamInboxStoreError::InvalidInput(format!(
                    "update {update_id} is already acked and cannot be marked stale"
                )));
            }
            let retry_count = existing.map_or(0, |record| record.retry_count);
            upsert_delivery_in_tx(
                &tx,
                consumer_id,
                update_id,
                TeamInboxDeliveryState::Stale,
                None,
                retry_count,
                updated_at_unix,
            )?;
            enqueue_event_in_tx(
                &tx,
                &TeamInboxEvent::Delivery {
                    update_id: update_id.to_owned(),
                    consumer_id: consumer_id.to_owned(),
                    state: TeamInboxDeliveryState::Stale,
                    turn_id: None,
                    retry_count,
                    updated_at_unix,
                },
            )?;
            let advanced_to = contiguous_terminal_seq_in_tx(&tx, consumer_id, channel)?;
            upsert_cursor_in_tx(&tx, consumer_id, channel, advanced_to)?;
            enqueue_event_in_tx(
                &tx,
                &TeamInboxEvent::CursorAdvance {
                    consumer_id: consumer_id.to_owned(),
                    channel: channel.to_owned(),
                    last_seen_seq: advanced_to,
                },
            )?;
            tx.commit()?;
            advanced_to
        };
        self.flush_outbox()?;
        Ok(advanced_to)
    }

    pub(crate) fn delivery(
        &self,
        consumer_id: &str,
        update_id: &str,
    ) -> Result<Option<DeliveryRecord>, TeamInboxStoreError> {
        match &self.conn {
            Some(conn) => query_delivery(conn, consumer_id, update_id),
            None => Ok(self
                .read_only
                .deliveries
                .get(&(consumer_id.to_owned(), update_id.to_owned()))
                .cloned()),
        }
    }

    fn write_delivery(
        &mut self,
        consumer_id: &str,
        update_id: &str,
        state: TeamInboxDeliveryState,
        turn_id: Option<String>,
        updated_at_unix: u64,
    ) -> Result<(), TeamInboxStoreError> {
        require_non_empty("consumer_id", consumer_id)?;
        require_non_empty("update_id", update_id)?;
        {
            let conn = self.conn_mut()?;
            let tx = conn.transaction()?;
            ensure_update_exists_in_tx(&tx, update_id)?;
            let existing = query_delivery_in_tx(&tx, consumer_id, update_id)?;
            reject_terminal_overwrite(existing.as_ref(), update_id, state)?;
            let retry_count = existing.map_or(0, |record| record.retry_count);
            upsert_delivery_in_tx(
                &tx,
                consumer_id,
                update_id,
                state,
                turn_id.as_deref(),
                retry_count,
                updated_at_unix,
            )?;
            enqueue_event_in_tx(
                &tx,
                &TeamInboxEvent::Delivery {
                    update_id: update_id.to_owned(),
                    consumer_id: consumer_id.to_owned(),
                    state,
                    turn_id,
                    retry_count,
                    updated_at_unix,
                },
            )?;
            tx.commit()?;
        }
        self.flush_outbox()?;
        Ok(())
    }

    fn conn_mut(&mut self) -> Result<&mut Connection, TeamInboxStoreError> {
        if self.mode != StoreMode::ReadWrite {
            return Err(TeamInboxStoreError::ReadOnly);
        }
        self.conn.as_mut().ok_or(TeamInboxStoreError::ReadOnly)
    }

    fn reconcile_jsonl_events_if_empty(&mut self) -> Result<(), TeamInboxStoreError> {
        let events = read_events(&self.jsonl_path)?.events;
        let Some(conn) = self.conn.as_mut() else {
            return Ok(());
        };
        if sqlite_has_state(conn)? {
            return Ok(());
        }
        let tx = conn.transaction()?;
        for event in events {
            match event {
                TeamInboxEvent::Post { update } => insert_update_with_seq_in_tx(&tx, &update)?,
                TeamInboxEvent::CursorAdvance {
                    consumer_id,
                    channel,
                    last_seen_seq,
                } => upsert_cursor_in_tx(&tx, &consumer_id, &channel, last_seen_seq)?,
                TeamInboxEvent::CursorDelete {
                    consumer_id,
                    channel,
                } => delete_cursor_in_tx(&tx, &consumer_id, &channel)?,
                TeamInboxEvent::Delivery {
                    update_id,
                    consumer_id,
                    state,
                    turn_id,
                    retry_count,
                    updated_at_unix,
                } => upsert_delivery_in_tx(
                    &tx,
                    &consumer_id,
                    &update_id,
                    state,
                    turn_id.as_deref(),
                    retry_count,
                    updated_at_unix,
                )?,
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn flush_outbox(&mut self) -> Result<(), TeamInboxStoreError> {
        let jsonl_path = self.jsonl_path.clone();
        let Some(conn) = self.conn.as_mut() else {
            return Err(TeamInboxStoreError::ReadOnly);
        };
        flush_outbox_to_jsonl(conn, &jsonl_path)
    }
}

fn to_sql_i64(name: &str, value: u64) -> Result<i64, TeamInboxStoreError> {
    i64::try_from(value).map_err(|_| {
        TeamInboxStoreError::InvalidInput(format!("{name} is too large for SQLite INTEGER"))
    })
}

fn to_sql_usize_limit(limit: usize) -> Result<i64, TeamInboxStoreError> {
    i64::try_from(limit)
        .map_err(|_| TeamInboxStoreError::InvalidInput("limit is too large".into()))
}

fn i64_cell_to_u64(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value = row.get::<_, i64>(index)?;
    u64::try_from(value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(err),
        )
    })
}

fn i64_cell_to_u32(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u32> {
    let value = row.get::<_, i64>(index)?;
    u32::try_from(value).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(err),
        )
    })
}

fn default_store_dir() -> PathBuf {
    std::env::var(STORE_ENV).map_or_else(
        |_| PathBuf::from(".zo").join("team_inbox"),
        PathBuf::from,
    )
}

fn open_sqlite(root: &Path) -> Result<Connection, TeamInboxStoreError> {
    fs::create_dir_all(root)?;
    let conn = Connection::open(root.join(DB_FILE))?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    ensure_wal_enabled(&conn)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS updates (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            channel TEXT NOT NULL,
            source TEXT NOT NULL,
            created_at_unix INTEGER NOT NULL,
            priority TEXT NOT NULL,
            summary TEXT NOT NULL,
            body_ref_json TEXT,
            task_id TEXT,
            status TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_team_inbox_updates_channel_seq
            ON updates(channel, seq);
        CREATE TABLE IF NOT EXISTS cursors (
            consumer_id TEXT NOT NULL,
            channel TEXT NOT NULL,
            last_seen_seq INTEGER NOT NULL,
            PRIMARY KEY (consumer_id, channel)
        );
        CREATE TABLE IF NOT EXISTS deliveries (
            update_id TEXT NOT NULL,
            consumer_id TEXT NOT NULL,
            state TEXT NOT NULL,
            turn_id TEXT,
            retry_count INTEGER NOT NULL DEFAULT 0,
            updated_at_unix INTEGER NOT NULL,
            PRIMARY KEY (update_id, consumer_id)
        );
        CREATE TABLE IF NOT EXISTS jsonl_outbox (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_json TEXT NOT NULL
        );",
    )?;
    Ok(conn)
}

fn open_sqlite_read_only(root: &Path) -> Result<Connection, TeamInboxStoreError> {
    let db_path = root.join(DB_FILE);
    let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    Ok(conn)
}

fn sqlite_has_state(conn: &Connection) -> Result<bool, TeamInboxStoreError> {
    for table in ["updates", "cursors", "deliveries"] {
        let count: i64 = conn.query_row(
            &format!("SELECT COUNT(*) FROM {table}"),
            [],
            |row| row.get(0),
        )?;
        if count > 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

fn ensure_wal_enabled(conn: &Connection) -> Result<(), TeamInboxStoreError> {
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
        Ok(())
    } else {
        Err(TeamInboxStoreError::Unavailable(format!(
            "SQLite journal_mode is {mode:?}, expected WAL"
        )))
    }
}

fn enqueue_event_in_tx(
    tx: &rusqlite::Transaction<'_>,
    event: &TeamInboxEvent,
) -> Result<(), TeamInboxStoreError> {
    tx.execute(
        "INSERT INTO jsonl_outbox (event_json) VALUES (?1)",
        params![serde_json::to_string(event)?],
    )?;
    Ok(())
}

fn flush_outbox_to_jsonl(conn: &mut Connection, path: &Path) -> Result<(), TeamInboxStoreError> {
    let rows = {
        let mut stmt = conn.prepare("SELECT id, event_json FROM jsonl_outbox ORDER BY id ASC")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    for (id, event_json) in rows {
        append_json_line_to(path, &event_json)?;
        conn.execute("DELETE FROM jsonl_outbox WHERE id = ?1", params![id])?;
    }
    Ok(())
}

fn serialize_optional<T: Serialize>(value: Option<&T>) -> Result<Option<String>, TeamInboxStoreError> {
    value
        .map(serde_json::to_string)
        .transpose()
        .map_err(Into::into)
}

fn deserialize_optional<T: for<'de> Deserialize<'de>>(
    value: Option<String>,
) -> Result<Option<T>, TeamInboxStoreError> {
    value
        .map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(Into::into)
}

fn query_update_in_tx(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
) -> Result<TeamUpdate, TeamInboxStoreError> {
    tx.query_row(
        "SELECT seq, id, channel, source, created_at_unix, priority, summary,
                body_ref_json, task_id, status
         FROM updates WHERE id = ?1",
        params![id],
        row_to_update,
    )
    .map_err(Into::into)
}

fn query_update_optional(
    conn: &Connection,
    id: &str,
) -> Result<Option<TeamUpdate>, TeamInboxStoreError> {
    conn.query_row(
        "SELECT seq, id, channel, source, created_at_unix, priority, summary,
                body_ref_json, task_id, status
         FROM updates WHERE id = ?1",
        params![id],
        row_to_update,
    )
    .optional()
    .map_err(Into::into)
}

fn query_update_optional_in_tx(
    tx: &rusqlite::Transaction<'_>,
    id: &str,
) -> Result<Option<TeamUpdate>, TeamInboxStoreError> {
    tx.query_row(
        "SELECT seq, id, channel, source, created_at_unix, priority, summary,
                body_ref_json, task_id, status
         FROM updates WHERE id = ?1",
        params![id],
        row_to_update,
    )
    .optional()
    .map_err(Into::into)
}

fn insert_update_with_seq_in_tx(
    tx: &rusqlite::Transaction<'_>,
    update: &TeamUpdate,
) -> Result<(), TeamInboxStoreError> {
    tx.execute(
        "INSERT OR IGNORE INTO updates
         (seq, id, channel, source, created_at_unix, priority, summary, body_ref_json, task_id, status)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            update.seq,
            update.id,
            update.channel,
            update.source,
            to_sql_i64("created_at_unix", update.created_at_unix)?,
            update.priority.as_str(),
            update.summary,
            serialize_optional(update.body_ref.as_ref())?,
            update.task_id,
            update.status,
        ],
    )?;
    Ok(())
}

fn row_to_update(row: &rusqlite::Row<'_>) -> rusqlite::Result<TeamUpdate> {
    let priority: String = row.get(5)?;
    let body_ref_json: Option<String> = row.get(7)?;
    Ok(TeamUpdate {
        seq: row.get(0)?,
        id: row.get(1)?,
        channel: row.get(2)?,
        source: row.get(3)?,
        created_at_unix: i64_cell_to_u64(row, 4)?,
        priority: TeamInboxPriority::from_str(&priority).map_err(to_sql_err)?,
        summary: row.get(6)?,
        body_ref: deserialize_optional(body_ref_json).map_err(to_sql_err)?,
        task_id: row.get(8)?,
        status: row.get(9)?,
    })
}

fn list_channels_sqlite(
    conn: &Connection,
    consumer_id: Option<&str>,
) -> Result<Vec<TeamInboxChannel>, TeamInboxStoreError> {
    if let Some(consumer_id) = consumer_id {
        let mut stmt = conn.prepare(
            "SELECT u.channel, COUNT(*) AS update_count, MAX(u.seq) AS last_seq,
                    MAX(u.created_at_unix) AS last_created_at_unix, c.last_seen_seq
             FROM updates u
             LEFT JOIN cursors c ON c.channel = u.channel AND c.consumer_id = ?1
             GROUP BY u.channel
             ORDER BY u.channel ASC",
        )?;
        let rows = stmt.query_map(params![consumer_id], row_to_channel)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    } else {
        let mut stmt = conn.prepare(
            "SELECT u.channel, COUNT(*) AS update_count, MAX(u.seq) AS last_seq,
                    MAX(u.created_at_unix) AS last_created_at_unix, NULL AS last_seen_seq
             FROM updates u
             GROUP BY u.channel
             ORDER BY u.channel ASC",
        )?;
        let rows = stmt.query_map([], row_to_channel)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }
}

fn row_to_channel(row: &rusqlite::Row<'_>) -> rusqlite::Result<TeamInboxChannel> {
    Ok(TeamInboxChannel {
        channel: row.get(0)?,
        update_count: row.get(1)?,
        last_seq: row.get(2)?,
        last_created_at_unix: i64_cell_to_u64(row, 3)?,
        cursor_seq: row.get(4)?,
    })
}

fn unread_sqlite(
    conn: &Connection,
    consumer_id: &str,
    channel: &str,
    limit: usize,
) -> Result<Vec<TeamUpdate>, TeamInboxStoreError> {
    let cursor = cursor_for(conn, consumer_id, channel)?;
    let mut stmt = conn.prepare(
        "SELECT u.seq, u.id, u.channel, u.source, u.created_at_unix, u.priority, u.summary,
                u.body_ref_json, u.task_id, u.status
         FROM updates u
         LEFT JOIN deliveries d ON d.update_id = u.id AND d.consumer_id = ?2
         WHERE u.channel = ?1
           AND u.seq > ?3
           AND (d.state IS NULL OR d.state NOT IN ('acked', 'stale'))
         ORDER BY u.seq ASC
         LIMIT ?4",
    )?;
    let limit = to_sql_usize_limit(limit)?;
    let rows = stmt.query_map(params![channel, consumer_id, cursor, limit], row_to_update)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn cursor_for(
    conn: &Connection,
    consumer_id: &str,
    channel: &str,
) -> Result<i64, TeamInboxStoreError> {
    cursor_row(conn, consumer_id, channel)?.map_or_else(
        || channel_tail_seq(conn, channel),
        Ok,
    )
}

fn cursor_row(
    conn: &Connection,
    consumer_id: &str,
    channel: &str,
) -> Result<Option<i64>, TeamInboxStoreError> {
    conn.query_row(
        "SELECT last_seen_seq FROM cursors WHERE consumer_id = ?1 AND channel = ?2",
        params![consumer_id, channel],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn channel_tail_seq(conn: &Connection, channel: &str) -> Result<i64, TeamInboxStoreError> {
    conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM updates WHERE channel = ?1",
        params![channel],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn channel_tail_seq_in_tx(
    tx: &rusqlite::Transaction<'_>,
    channel: &str,
) -> Result<i64, TeamInboxStoreError> {
    tx.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM updates WHERE channel = ?1",
        params![channel],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn delete_cursor_in_tx(
    tx: &rusqlite::Transaction<'_>,
    consumer_id: &str,
    channel: &str,
) -> Result<(), TeamInboxStoreError> {
    tx.execute(
        "DELETE FROM cursors WHERE consumer_id = ?1 AND channel = ?2",
        params![consumer_id, channel],
    )?;
    Ok(())
}

fn upsert_cursor_in_tx(
    tx: &rusqlite::Transaction<'_>,
    consumer_id: &str,
    channel: &str,
    last_seen_seq: i64,
) -> Result<(), TeamInboxStoreError> {
    tx.execute(
        "INSERT INTO cursors (consumer_id, channel, last_seen_seq)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(consumer_id, channel)
         DO UPDATE SET last_seen_seq = excluded.last_seen_seq",
        params![consumer_id, channel, last_seen_seq],
    )?;
    Ok(())
}

fn upsert_delivery_in_tx(
    tx: &rusqlite::Transaction<'_>,
    consumer_id: &str,
    update_id: &str,
    state: TeamInboxDeliveryState,
    turn_id: Option<&str>,
    retry_count: u32,
    updated_at_unix: u64,
) -> Result<(), TeamInboxStoreError> {
    require_non_empty("consumer_id", consumer_id)?;
    require_non_empty("update_id", update_id)?;
    tx.execute(
        "INSERT INTO deliveries
         (update_id, consumer_id, state, turn_id, retry_count, updated_at_unix)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(update_id, consumer_id) DO UPDATE SET
             state = excluded.state,
             turn_id = excluded.turn_id,
             retry_count = excluded.retry_count,
             updated_at_unix = excluded.updated_at_unix",
        params![
            update_id,
            consumer_id,
            state.as_str(),
            turn_id,
            retry_count,
            to_sql_i64("updated_at_unix", updated_at_unix)?,
        ],
    )?;
    Ok(())
}

fn query_delivery(
    conn: &Connection,
    consumer_id: &str,
    update_id: &str,
) -> Result<Option<DeliveryRecord>, TeamInboxStoreError> {
    conn.query_row(
        "SELECT update_id, consumer_id, state, turn_id, retry_count, updated_at_unix
         FROM deliveries WHERE consumer_id = ?1 AND update_id = ?2",
        params![consumer_id, update_id],
        row_to_delivery,
    )
    .optional()
    .map_err(Into::into)
}

fn query_delivery_in_tx(
    tx: &rusqlite::Transaction<'_>,
    consumer_id: &str,
    update_id: &str,
) -> Result<Option<DeliveryRecord>, TeamInboxStoreError> {
    tx.query_row(
        "SELECT update_id, consumer_id, state, turn_id, retry_count, updated_at_unix
         FROM deliveries WHERE consumer_id = ?1 AND update_id = ?2",
        params![consumer_id, update_id],
        row_to_delivery,
    )
    .optional()
    .map_err(Into::into)
}

fn ensure_update_exists_in_tx(
    tx: &rusqlite::Transaction<'_>,
    update_id: &str,
) -> Result<TeamUpdate, TeamInboxStoreError> {
    query_update_optional_in_tx(tx, update_id)?.ok_or_else(|| {
        TeamInboxStoreError::InvalidInput(format!("unknown update id: {update_id}"))
    })
}

fn ensure_update_in_channel_in_tx(
    tx: &rusqlite::Transaction<'_>,
    update_id: &str,
    channel: &str,
) -> Result<TeamUpdate, TeamInboxStoreError> {
    let update = ensure_update_exists_in_tx(tx, update_id)?;
    if update.channel == channel {
        Ok(update)
    } else {
        Err(TeamInboxStoreError::InvalidInput(format!(
            "update {update_id} belongs to channel {:?}, not {:?}",
            update.channel, channel
        )))
    }
}

fn validate_ack_in_tx(
    tx: &rusqlite::Transaction<'_>,
    consumer_id: &str,
    channel: &str,
    update_id: &str,
    turn_id: &str,
) -> Result<u32, TeamInboxStoreError> {
    ensure_update_in_channel_in_tx(tx, update_id, channel)?;
    let delivery = query_delivery_in_tx(tx, consumer_id, update_id)?.ok_or_else(|| {
        TeamInboxStoreError::InvalidInput(format!(
            "update {update_id} was not injected for consumer {consumer_id}"
        ))
    })?;
    if delivery.state != TeamInboxDeliveryState::Injected {
        return Err(TeamInboxStoreError::InvalidInput(format!(
            "update {update_id} is {:?}, not injected",
            delivery.state
        )));
    }
    if delivery.turn_id.as_deref() != Some(turn_id) {
        return Err(TeamInboxStoreError::InvalidInput(format!(
            "turn_id mismatch for update {update_id}"
        )));
    }
    Ok(delivery.retry_count)
}

fn validate_failure_in_tx(
    tx: &rusqlite::Transaction<'_>,
    consumer_id: &str,
    update_id: &str,
    turn_id: &str,
) -> Result<u32, TeamInboxStoreError> {
    let delivery = query_delivery_in_tx(tx, consumer_id, update_id)?.ok_or_else(|| {
        TeamInboxStoreError::InvalidInput(format!(
            "update {update_id} was not injected for consumer {consumer_id}"
        ))
    })?;
    if delivery.state != TeamInboxDeliveryState::Injected {
        return Err(TeamInboxStoreError::InvalidInput(format!(
            "update {update_id} is {:?}, not injected",
            delivery.state
        )));
    }
    if delivery.turn_id.as_deref() != Some(turn_id) {
        return Err(TeamInboxStoreError::InvalidInput(format!(
            "turn_id mismatch for update {update_id}"
        )));
    }
    Ok(delivery.retry_count)
}

fn reject_terminal_overwrite(
    existing: Option<&DeliveryRecord>,
    update_id: &str,
    requested: TeamInboxDeliveryState,
) -> Result<(), TeamInboxStoreError> {
    let Some(existing) = existing else {
        return Ok(());
    };
    if matches!(existing.state, TeamInboxDeliveryState::Acked | TeamInboxDeliveryState::Stale)
        && existing.state != requested
    {
        return Err(TeamInboxStoreError::InvalidInput(format!(
            "update {update_id} is terminal {:?} and cannot transition to {:?}",
            existing.state, requested
        )));
    }
    Ok(())
}

fn row_to_delivery(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeliveryRecord> {
    let state: String = row.get(2)?;
    Ok(DeliveryRecord {
        update_id: row.get(0)?,
        consumer_id: row.get(1)?,
        state: TeamInboxDeliveryState::from_str(&state).map_err(to_sql_err)?,
        turn_id: row.get(3)?,
        retry_count: i64_cell_to_u32(row, 4)?,
        updated_at_unix: i64_cell_to_u64(row, 5)?,
    })
}

fn contiguous_terminal_seq_in_tx(
    tx: &rusqlite::Transaction<'_>,
    consumer_id: &str,
    channel: &str,
) -> Result<i64, TeamInboxStoreError> {
    let current = tx
        .query_row(
            "SELECT last_seen_seq FROM cursors WHERE consumer_id = ?1 AND channel = ?2",
            params![consumer_id, channel],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(0);

    let mut stmt = tx.prepare(
        "SELECT seq, id FROM updates
         WHERE channel = ?1 AND seq > ?2
         ORDER BY seq ASC",
    )?;
    let mut rows = stmt.query(params![channel, current])?;
    let mut advanced = current;
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get(0)?;
        let id: String = row.get(1)?;
        let state = tx
            .query_row(
                "SELECT state FROM deliveries WHERE consumer_id = ?1 AND update_id = ?2",
                params![consumer_id, id],
                |delivery_row| delivery_row.get::<_, String>(0),
            )
            .optional()?;
        if matches!(
            state.as_deref(),
            Some("acked" | "stale")
        ) {
            advanced = seq;
        } else {
            break;
        }
    }
    Ok(advanced)
}

fn list_channels_snapshot(
    snapshot: &ReadOnlySnapshot,
    consumer_id: Option<&str>,
) -> Vec<TeamInboxChannel> {
    let mut by_channel: HashMap<String, TeamInboxChannel> = HashMap::new();
    for update in &snapshot.updates {
        let entry = by_channel
            .entry(update.channel.clone())
            .or_insert_with(|| TeamInboxChannel {
                channel: update.channel.clone(),
                update_count: 0,
                last_seq: update.seq,
                last_created_at_unix: update.created_at_unix,
                cursor_seq: None,
            });
        entry.update_count += 1;
        if update.seq > entry.last_seq {
            entry.last_seq = update.seq;
        }
        if update.created_at_unix > entry.last_created_at_unix {
            entry.last_created_at_unix = update.created_at_unix;
        }
    }
    if let Some(consumer_id) = consumer_id {
        for channel in by_channel.values_mut() {
            channel.cursor_seq = snapshot
                .cursors
                .get(&(consumer_id.to_owned(), channel.channel.clone()))
                .copied();
        }
    }
    let mut channels = by_channel.into_values().collect::<Vec<_>>();
    channels.sort_by(|left, right| left.channel.cmp(&right.channel));
    channels
}

fn unread_snapshot(
    snapshot: &ReadOnlySnapshot,
    consumer_id: &str,
    channel: &str,
    limit: usize,
) -> Vec<TeamUpdate> {
    let cursor = snapshot
        .cursors
        .get(&(consumer_id.to_owned(), channel.to_owned()))
        .copied()
        .unwrap_or_else(|| {
            snapshot
                .updates
                .iter()
                .filter(|update| update.channel == channel)
                .map(|update| update.seq)
                .max()
                .unwrap_or(0)
        });
    snapshot
        .updates
        .iter()
        .filter(|update| {
            update.channel == channel
                && update.seq > cursor
                && !matches!(
                    snapshot
                        .deliveries
                        .get(&(consumer_id.to_owned(), update.id.clone()))
                        .map(|record| record.state),
                    Some(TeamInboxDeliveryState::Acked | TeamInboxDeliveryState::Stale)
                )
        })
        .take(limit)
        .cloned()
        .collect()
}

fn replay_jsonl(path: &Path) -> Result<JsonlReplay, TeamInboxStoreError> {
    let events = read_events(path)?;
    let mut snapshot = ReadOnlySnapshot::default();
    for event in events.events {
        snapshot.apply(event);
    }
    snapshot.updates.sort_by_key(|update| update.seq);
    Ok(JsonlReplay {
        snapshot,
        malformed_line_count: events.malformed_line_count,
    })
}

fn read_events(path: &Path) -> Result<JsonlEvents, TeamInboxStoreError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(JsonlEvents::default()),
        Err(err) => return Err(err.into()),
    };
    let reader = std::io::BufReader::new(file);
    let mut parsed = JsonlEvents::default();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<TeamInboxEvent>(&line) {
            Ok(event) => parsed.events.push(event),
            Err(_) => parsed.malformed_line_count += 1,
        }
    }
    Ok(parsed)
}

impl ReadOnlySnapshot {
    fn apply(&mut self, event: TeamInboxEvent) {
        match event {
            TeamInboxEvent::Post { update } => {
                if !self.updates.iter().any(|stored| stored.id == update.id) {
                    self.updates.push(update);
                }
            }
            TeamInboxEvent::CursorAdvance {
                consumer_id,
                channel,
                last_seen_seq,
            } => {
                self.cursors
                    .insert((consumer_id, channel), last_seen_seq);
            }
            TeamInboxEvent::CursorDelete {
                consumer_id,
                channel,
            } => {
                self.cursors.remove(&(consumer_id, channel));
            }
            TeamInboxEvent::Delivery {
                update_id,
                consumer_id,
                state,
                turn_id,
                retry_count,
                updated_at_unix,
            } => {
                self.deliveries.insert(
                    (consumer_id.clone(), update_id.clone()),
                    DeliveryRecord {
                        update_id,
                        consumer_id,
                        state,
                        turn_id,
                        retry_count,
                        updated_at_unix,
                    },
                );
            }
        }
    }
}

pub(crate) fn append_event_to(path: &Path, event: &TeamInboxEvent) -> Result<(), TeamInboxStoreError> {
    append_json_line_to(path, &serde_json::to_string(event)?)
}

fn append_json_line_to(path: &Path, line: &str) -> Result<(), TeamInboxStoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut record = String::with_capacity(line.len() + 1);
    record.push_str(line);
    record.push('\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(record.as_bytes())?;
    Ok(())
}

fn body_artifact_ref(content: &str) -> ArtifactRef {
    ArtifactRef {
        sha256: sha256_hex(content.as_bytes()),
        size_bytes: content.len() as u64,
        kind: ArtifactKind::Generic,
        preview: content.chars().take(ARTIFACT_PREVIEW_CHARS).collect(),
    }
}

fn write_body_artifact(dir: &Path, content: &str) -> Result<(), TeamInboxStoreError> {
    fs::create_dir_all(dir)?;
    let artifact_ref = body_artifact_ref(content);
    let path = dir.join(&artifact_ref.sha256);
    if !path.exists() {
        let tmp = unique_artifact_tmp_path(&path);
        fs::write(&tmp, content)?;
        fs::rename(&tmp, &path)?;
    }
    Ok(())
}

fn unique_artifact_tmp_path(path: &Path) -> PathBuf {
    let nonce = ARTIFACT_TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_extension(format!("tmp.{}.{}", std::process::id(), nonce))
}

fn validate_update_input(input: &NewTeamUpdate) -> Result<(), TeamInboxStoreError> {
    require_non_empty("id", &input.id)?;
    require_non_empty("channel", &input.channel)?;
    require_non_empty("source", &input.source)?;
    require_non_empty("summary", &input.summary)?;
    Ok(())
}

fn require_non_empty(name: &str, value: &str) -> Result<(), TeamInboxStoreError> {
    if value.trim().is_empty() {
        return Err(TeamInboxStoreError::InvalidInput(format!(
            "{name} must not be empty"
        )));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn to_sql_err(error: TeamInboxStoreError) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Barrier};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "zo-team-inbox-{tag}-{}-{n}",
            std::process::id()
        ))
    }

    fn new_update(id: &str, channel: &str) -> NewTeamUpdate {
        NewTeamUpdate {
            id: id.to_owned(),
            channel: channel.to_owned(),
            source: "session-a".into(),
            created_at_unix: 42,
            priority: TeamInboxPriority::Normal,
            summary: format!("summary {id}"),
            body: None,
            task_id: None,
            status: None,
        }
    }

    #[test]
    fn post_update_persists_sqlite_and_jsonl() {
        let dir = temp_dir("persist");
        let mut store = TeamInboxStore::open_at(&dir);
        let posted = store.post_update(new_update("u1", "ci")).expect("post");
        assert_eq!(posted.seq, 1);
        assert_eq!(posted.id, "u1");
        assert!(dir.join(JSONL_FILE).exists(), "jsonl audit is written");

        let reopened = TeamInboxStore::open_at(&dir);
        let unread = reopened
            .unread_updates("new-consumer", "ci", 10)
            .expect("read");
        assert!(unread.is_empty(), "from-now default skips existing backlog");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn duplicate_post_returns_existing_row_without_new_seq() {
        let dir = temp_dir("dedup");
        let mut store = TeamInboxStore::open_at(&dir);
        let first = store.post_update(new_update("same", "ci")).expect("first");
        let mut second_input = new_update("same", "ci");
        second_input.summary = "different summary ignored by dedup".into();
        let second = store.post_update(second_input).expect("second");
        assert_eq!(first, second);

        let conn = Connection::open(dir.join(DB_FILE)).expect("open db");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM updates", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_duplicate_posts_are_idempotent() {
        let dir = temp_dir("concurrent-dedup");
        drop(TeamInboxStore::open_at(&dir));
        let thread_count = 4;
        let barrier = Arc::new(Barrier::new(thread_count));
        let handles = (0..thread_count)
            .map(|_| {
                let dir = dir.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut store = TeamInboxStore::open_at(&dir);
                    barrier.wait();
                    store.post_update(new_update("same", "ci"))
                })
            })
            .collect::<Vec<_>>();

        let results = handles
            .into_iter()
            .map(|handle| handle.join().expect("thread join").expect("post"))
            .collect::<Vec<_>>();
        assert!(results.iter().all(|update| update.id == "same"));
        assert!(results.iter().all(|update| update.seq == 1));

        let conn = Connection::open(dir.join(DB_FILE)).expect("open db");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM updates", [], |row| row.get(0))
            .expect("count");
        assert_eq!(count, 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn join_channel_from_now_skips_backlog_but_reads_later_updates() {
        let dir = temp_dir("from-now");
        let mut store = TeamInboxStore::open_at(&dir);
        store.post_update(new_update("old", "ci")).expect("old");
        let tail = store
            .join_channel_from_now("session-1", "ci")
            .expect("join");
        assert_eq!(tail, 1);
        store.post_update(new_update("new", "ci")).expect("new");

        let unread = store
            .unread_updates("session-1", "ci", 10)
            .expect("unread");
        assert_eq!(unread.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(), ["new"]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ack_advances_only_over_contiguous_acked_updates() {
        let dir = temp_dir("ack");
        let mut store = TeamInboxStore::open_at(&dir);
        store.join_channel_from_now("s", "ci").expect("join");
        store.post_update(new_update("u1", "ci")).expect("u1");
        store.post_update(new_update("u2", "ci")).expect("u2");

        store.mark_injected("s", "u1", "turn-1", 90).expect("inject u1");
        store.mark_injected("s", "u2", "turn-1", 91).expect("inject u2");

        let advanced = store
            .ack_update("s", "ci", "u2", "turn-1", 100)
            .expect("ack u2");
        assert_eq!(advanced, 0, "must not skip unacked u1");
        let unread = store.unread_updates("s", "ci", 10).expect("unread");
        assert_eq!(unread.iter().map(|u| u.id.as_str()).collect::<Vec<_>>(), ["u1"]);

        let advanced = store
            .ack_update("s", "ci", "u1", "turn-1", 101)
            .expect("ack u1");
        assert_eq!(advanced, 2, "both contiguous updates are now acked");
        assert!(store.unread_updates("s", "ci", 10).expect("unread").is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ack_rejects_unknown_wrong_channel_not_injected_and_wrong_turn() {
        let dir = temp_dir("ack-validate");
        let mut store = TeamInboxStore::open_at(&dir);
        store.join_channel_from_now("s", "ci").expect("join ci");
        store.post_update(new_update("ci-1", "ci")).expect("ci");
        store
            .post_update(new_update("review-1", "review"))
            .expect("review");

        assert!(matches!(
            store.ack_update("s", "ci", "missing", "turn-1", 100),
            Err(TeamInboxStoreError::InvalidInput(_))
        ));
        assert!(matches!(
            store.ack_update("s", "ci", "ci-1", "turn-1", 100),
            Err(TeamInboxStoreError::InvalidInput(_))
        ));
        store
            .mark_injected("s", "ci-1", "turn-1", 101)
            .expect("inject ci");
        store
            .mark_injected("s", "review-1", "turn-r", 102)
            .expect("inject review");
        assert!(matches!(
            store.ack_update("s", "ci", "review-1", "turn-r", 103),
            Err(TeamInboxStoreError::InvalidInput(_))
        ));
        assert!(matches!(
            store.ack_update("s", "ci", "ci-1", "wrong-turn", 104),
            Err(TeamInboxStoreError::InvalidInput(_))
        ));
        assert_eq!(
            store.ack_update("s", "ci", "ci-1", "turn-1", 105).expect("ack"),
            1
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_and_retry_cap_stop_redelivery_and_advance_cursor() {
        let dir = temp_dir("stale");
        let mut store = TeamInboxStore::open_at(&dir);
        store.join_channel_from_now("s", "ci").expect("join");
        store.post_update(new_update("u1", "ci")).expect("u1");
        store.post_update(new_update("u2", "ci")).expect("u2");
        store.mark_injected("s", "u1", "turn-1", 10).expect("inject u1");
        store.mark_injected("s", "u2", "turn-2", 11).expect("inject u2");

        let state = store
            .record_failure("s", "ci", "u1", "turn-1", 12, 2)
            .expect("first failure");
        assert_eq!(state, TeamInboxDeliveryState::Failed);
        assert_eq!(
            store.unread_updates("s", "ci", 10).expect("unread")
                .iter()
                .map(|u| u.id.as_str())
                .collect::<Vec<_>>(),
            ["u1", "u2"]
        );

        store
            .mark_injected("s", "u1", "turn-1b", 13)
            .expect("reinject u1");
        let state = store
            .record_failure("s", "ci", "u1", "turn-1b", 14, 2)
            .expect("retry cap stale");
        assert_eq!(state, TeamInboxDeliveryState::Stale);
        assert_eq!(
            store.unread_updates("s", "ci", 10).expect("unread")
                .iter()
                .map(|u| u.id.as_str())
                .collect::<Vec<_>>(),
            ["u2"]
        );
        assert_eq!(store.mark_stale("s", "ci", "u2", 14).expect("stale u2"), 2);
        assert!(store.unread_updates("s", "ci", 10).expect("unread").is_empty());
        let _ = fs::remove_dir_all(dir);
    }


    #[test]
    fn list_channels_reports_counts_and_optional_cursor() {
        let dir = temp_dir("list-channels");
        let mut store = TeamInboxStore::open_at(&dir);
        store.post_update(new_update("ci-1", "ci")).expect("ci 1");
        store.post_update(new_update("ci-2", "ci")).expect("ci 2");
        store
            .post_update(new_update("review-1", "review"))
            .expect("review");
        store.join_channel_from_now("reader", "ci").expect("join ci");

        let channels = store.list_channels(None).expect("list channels");
        assert_eq!(channels.iter().map(|c| c.channel.as_str()).collect::<Vec<_>>(), ["ci", "review"]);
        assert_eq!(channels[0].update_count, 2);
        assert_eq!(channels[0].last_seq, 2);
        assert_eq!(channels[0].cursor_seq, None);

        let with_consumer = store
            .list_channels(Some("reader"))
            .expect("list with consumer");
        assert_eq!(with_consumer[0].channel, "ci");
        assert_eq!(with_consumer[0].cursor_seq, Some(2));
        assert_eq!(with_consumer[1].channel, "review");
        assert_eq!(with_consumer[1].cursor_seq, None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn list_channels_read_only_replays_jsonl() {
        let dir = temp_dir("list-channels-jsonl");
        fs::create_dir_all(dir.join(DB_FILE)).expect("db path directory forces read-only mode");
        let update = TeamUpdate {
            seq: 7,
            id: "from-jsonl".into(),
            channel: "ci".into(),
            source: "session-a".into(),
            created_at_unix: 123,
            priority: TeamInboxPriority::Normal,
            summary: "jsonl summary".into(),
            body_ref: None,
            task_id: None,
            status: None,
        };
        append_event_to(&dir.join(JSONL_FILE), &TeamInboxEvent::Post { update }).expect("post event");
        append_event_to(
            &dir.join(JSONL_FILE),
            &TeamInboxEvent::CursorAdvance {
                consumer_id: "reader".into(),
                channel: "ci".into(),
                last_seen_seq: 7,
            },
        )
        .expect("cursor event");

        let store = TeamInboxStore::open_read_only_at(&dir);
        let channels = store
            .list_channels(Some("reader"))
            .expect("list channels");
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].channel, "ci");
        assert_eq!(channels[0].update_count, 1);
        assert_eq!(channels[0].last_seq, 7);
        assert_eq!(channels[0].last_created_at_unix, 123);
        assert_eq!(channels[0].cursor_seq, Some(7));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn leave_channel_deletes_cursor_and_replays_delete_event() {
        let dir = temp_dir("leave-channel");
        let mut store = TeamInboxStore::open_at(&dir);
        store.post_update(new_update("u1", "ci")).expect("post");
        store.join_channel_from_now("reader", "ci").expect("join");
        assert_eq!(store.cursor("reader", "ci").expect("cursor"), Some(1));
        store.leave_channel("reader", "ci").expect("leave");
        assert_eq!(store.cursor("reader", "ci").expect("cursor"), None);
        drop(store);

        let replayed = TeamInboxStore::open_read_only_at(&dir);
        assert_eq!(replayed.cursor("reader", "ci").expect("cursor"), None);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn leave_channel_rejects_never_joined_channel() {
        let dir = temp_dir("leave-never-joined");
        let mut store = TeamInboxStore::open_at(&dir);
        let error = store
            .leave_channel("reader", "ci")
            .expect_err("leave without join should reject");
        assert!(matches!(error, TeamInboxStoreError::InvalidInput(_)));
        assert!(error.to_string().contains("not joined"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn sqlite_outbox_flushes_committed_events_on_reopen() {
        let dir = temp_dir("outbox");
        {
            let mut conn = open_sqlite(&dir).expect("open sqlite");
            let tx = conn.transaction().expect("tx");
            let update = TeamUpdate {
                seq: 1,
                id: "queued".into(),
                channel: "ci".into(),
                source: "session-a".into(),
                created_at_unix: 1,
                priority: TeamInboxPriority::Normal,
                summary: "queued summary".into(),
                body_ref: None,
                task_id: None,
                status: None,
            };
            insert_update_with_seq_in_tx(&tx, &update).expect("insert update");
            enqueue_event_in_tx(&tx, &TeamInboxEvent::Post { update }).expect("enqueue");
            tx.commit().expect("commit");
        }
        assert!(!dir.join(JSONL_FILE).exists(), "simulated crash before JSONL flush");

        let _store = TeamInboxStore::open_at(&dir);
        assert!(dir.join(JSONL_FILE).exists(), "healthy reopen flushes committed outbox");
        let conn = Connection::open(dir.join(DB_FILE)).expect("open db");
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM jsonl_outbox", [], |row| row.get(0))
            .expect("outbox count");
        assert_eq!(remaining, 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn reopen_flushes_outbox_before_any_jsonl_replay() {
        let dir = temp_dir("outbox-order");
        let mut store = TeamInboxStore::open_at(&dir);
        store.join_channel_from_now("s", "ci").expect("join");
        store.post_update(new_update("u1", "ci")).expect("post");
        store.mark_injected("s", "u1", "turn-1", 10).expect("inject");
        drop(store);

        {
            let mut conn = Connection::open(dir.join(DB_FILE)).expect("open db");
            let tx = conn.transaction().expect("tx");
            upsert_delivery_in_tx(
                &tx,
                "s",
                "u1",
                TeamInboxDeliveryState::Acked,
                Some("turn-1"),
                0,
                11,
            )
            .expect("ack delivery");
            enqueue_event_in_tx(
                &tx,
                &TeamInboxEvent::Delivery {
                    update_id: "u1".into(),
                    consumer_id: "s".into(),
                    state: TeamInboxDeliveryState::Acked,
                    turn_id: Some("turn-1".into()),
                    retry_count: 0,
                    updated_at_unix: 11,
                },
            )
            .expect("enqueue delivery");
            upsert_cursor_in_tx(&tx, "s", "ci", 1).expect("cursor");
            enqueue_event_in_tx(
                &tx,
                &TeamInboxEvent::CursorAdvance {
                    consumer_id: "s".into(),
                    channel: "ci".into(),
                    last_seen_seq: 1,
                },
            )
            .expect("enqueue cursor");
            tx.commit().expect("commit without JSONL flush");
        }

        let reopened = TeamInboxStore::open_at(&dir);
        let delivery = reopened
            .delivery("s", "u1")
            .expect("delivery query")
            .expect("delivery exists");
        assert_eq!(delivery.state, TeamInboxDeliveryState::Acked);
        assert!(reopened.unread_updates("s", "ci", 10).expect("unread").is_empty());
        let conn = Connection::open(dir.join(DB_FILE)).expect("open db");
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM jsonl_outbox", [], |row| row.get(0))
            .expect("outbox count");
        assert_eq!(remaining, 0);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn open_sqlite_sets_wal_mode() {
        let dir = temp_dir("wal");
        let conn = open_sqlite(&dir).expect("open sqlite");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal mode");
        assert_eq!(mode.to_ascii_lowercase(), "wal");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn terminal_delivery_state_cannot_regress_to_injected_or_failed() {
        let dir = temp_dir("terminal");
        let mut store = TeamInboxStore::open_at(&dir);
        store.join_channel_from_now("s", "ci").expect("join");
        store.post_update(new_update("u1", "ci")).expect("u1");
        store.post_update(new_update("u2", "ci")).expect("u2");
        store.mark_injected("s", "u1", "turn-1", 10).expect("inject u1");
        store.mark_injected("s", "u2", "turn-2", 11).expect("inject u2");
        assert_eq!(store.ack_update("s", "ci", "u2", "turn-2", 12).expect("ack u2"), 0);

        assert!(matches!(
            store.record_failure("s", "ci", "u2", "turn-2", 13, 3),
            Err(TeamInboxStoreError::InvalidInput(_))
        ));
        assert!(matches!(
            store.mark_injected("s", "u2", "late-turn", 14),
            Err(TeamInboxStoreError::InvalidInput(_))
        ));
        assert!(matches!(
            store.mark_stale("s", "ci", "u2", 15),
            Err(TeamInboxStoreError::InvalidInput(_))
        ));
        assert_eq!(
            store.unread_updates("s", "ci", 10).expect("unread")
                .iter()
                .map(|u| u.id.as_str())
                .collect::<Vec<_>>(),
            ["u1"]
        );
        assert_eq!(store.ack_update("s", "ci", "u1", "turn-1", 16).expect("ack u1"), 2);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn body_is_stored_in_team_inbox_artifact_namespace() {
        let dir = temp_dir("body");
        let mut store = TeamInboxStore::open_at(&dir);
        let mut input = new_update("with-body", "handoff");
        input.body = Some("full low-trust body".into());
        let posted = store.post_update(input).expect("post");
        let body_ref = posted.body_ref.expect("body ref");
        assert_eq!(body_ref.kind, ArtifactKind::Generic);
        assert!(dir.join(ARTIFACTS_DIR).join(&body_ref.sha256).exists());
        assert!(!PathBuf::from(".zo").join("artifacts").join(&body_ref.sha256).exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mark_injected_records_delivery_state() {
        let dir = temp_dir("delivery");
        let mut store = TeamInboxStore::open_at(&dir);
        store.post_update(new_update("u1", "ci")).expect("post");
        store
            .mark_injected("session-1", "u1", "turn-1", 99)
            .expect("mark injected");

        let delivery = store
            .delivery("session-1", "u1")
            .expect("query delivery")
            .expect("delivery exists");
        assert_eq!(delivery.state, TeamInboxDeliveryState::Injected);
        assert_eq!(delivery.turn_id.as_deref(), Some("turn-1"));
        assert_eq!(delivery.updated_at_unix, 99);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn duplicate_post_does_not_write_unreferenced_body_artifact() {
        let dir = temp_dir("body-dedup");
        let mut store = TeamInboxStore::open_at(&dir);
        let mut first = new_update("same-body-id", "handoff");
        first.body = Some("body A".into());
        let posted = store.post_update(first).expect("first post");
        let original_ref = posted.body_ref.expect("first body ref");

        let mut duplicate = new_update("same-body-id", "handoff");
        duplicate.body = Some("body B".into());
        let duplicate_result = store.post_update(duplicate).expect("duplicate post");
        assert_eq!(duplicate_result.body_ref.as_ref(), Some(&original_ref));

        let duplicate_body_ref = body_artifact_ref("body B");
        assert!(!dir.join(ARTIFACTS_DIR).join(&duplicate_body_ref.sha256).exists());
        let artifact_count = fs::read_dir(dir.join(ARTIFACTS_DIR))
            .expect("artifact dir")
            .count();
        assert_eq!(artifact_count, 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_jsonl_records_are_counted_without_blocking_replay() {
        let dir = temp_dir("malformed-jsonl");
        let path = dir.join(JSONL_FILE);
        append_json_line_to(&path, "{malformed").expect("malformed audit record");
        let update = TeamUpdate {
            seq: 1,
            id: "valid".into(),
            channel: "ci".into(),
            source: "session-a".into(),
            created_at_unix: 1,
            priority: TeamInboxPriority::Normal,
            summary: "valid JSONL event".into(),
            body_ref: None,
            task_id: None,
            status: None,
        };
        append_event_to(&path, &TeamInboxEvent::Post { update }).expect("valid audit record");

        let store = TeamInboxStore::open_read_only_at(&dir);
        assert_eq!(store.malformed_jsonl_line_count(), 1);
        assert_eq!(
            store
                .update("valid")
                .expect("query replayed event")
                .map(|update| update.id),
            Some("valid".to_string())
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn read_only_degraded_mode_replays_jsonl_and_rejects_writes() {
        let dir = temp_dir("readonly");
        fs::create_dir_all(dir.join(DB_FILE)).expect("db path directory forces open failure");
        let update = TeamUpdate {
            seq: 7,
            id: "from-jsonl".into(),
            channel: "ci".into(),
            source: "session-a".into(),
            created_at_unix: 1,
            priority: TeamInboxPriority::High,
            summary: "jsonl summary".into(),
            body_ref: None,
            task_id: None,
            status: None,
        };
        append_event_to(&dir.join(JSONL_FILE), &TeamInboxEvent::Post { update }).expect("append");
        append_event_to(
            &dir.join(JSONL_FILE),
            &TeamInboxEvent::CursorAdvance {
                consumer_id: "s".into(),
                channel: "ci".into(),
                last_seen_seq: 0,
            },
        )
        .expect("cursor");

        let mut store = TeamInboxStore::open_at(&dir);
        assert_eq!(store.mode(), StoreMode::ReadOnly);
        let unread = store.unread_updates("s", "ci", 10).expect("replay read");
        assert_eq!(unread.len(), 1);
        assert!(matches!(
            store.post_update(new_update("blocked", "ci")),
            Err(TeamInboxStoreError::ReadOnly)
        ));
        assert!(matches!(
            store.ack_update("s", "ci", "from-jsonl", "turn", 10),
            Err(TeamInboxStoreError::ReadOnly)
        ));
        let _ = fs::remove_dir_all(dir);
    }
}
