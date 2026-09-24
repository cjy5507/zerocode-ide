//! The ledger as tables — one typed row per semantic row.
//!
//! Schema four kept a whole ledger as one opaque blob under a
//! `CHECK (length(ledger_bytes) <= 16777216)`, which made the largest thing a
//! person could keep a property of the ENVELOPE rather than of the thing
//! kept: a single 20 MiB answer could not be stored at all, however healthy
//! the ledger holding it was. Tables have no such ceiling, and a row that is
//! wrong is a row the database can refuse before anything reads it.
//!
//! Two rules run through the whole file.
//!
//! **Order is meaning, so order is a column.** `next_dispatch` takes the FIRST
//! ready task and `Inbox::pending` is a queue — a store that handed the rows
//! back in whatever order the pages happened to sit in would be handing back a
//! DIFFERENT ledger that holds the same facts. Every row carries an `ordinal`
//! and every read is `ORDER BY ordinal`, children included.
//!
//! **The database is the earlier net, not the authority.** Shapes it can know
//! — a count that cannot be negative, a boolean that is not 0 or 1, a child
//! with no parent, columns that must be present together — it refuses here,
//! where a file is opened. What it does NOT state is any enum's spelling: a
//! `CHECK (status IN (...))` is the same list of words the Rust enum already
//! holds, and a list kept in two places drifts the day somebody adds a variant
//! — the new one would be refused by the database with no explanation. Words
//! are read back through the type that defines them, and a word this window
//! does not know is refused there. Everything that needs to see the whole
//! ledger at once stays in `validate_loaded`, which runs after this and has
//! the last word.

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use zerocode_core::orchestration::{
    AckedRow, AttachmentRow, Auto, BoundRow, CHECK_RENDERER, CheckV1, CoordinatorSeat, Delivery,
    DispatchRow, GateRow, HandoverPolicy, InboxRow, LedgerProjectionV1, MessageRow, Pinned,
    ResultAuthor, RunRow, RunSummary, ServedAnswer, ServedRow, TaskRow, Text, WorkerRow,
};

use crate::effect_journal::{EffectJournalError, from_sql_u64, to_sql_u64};

const LEDGER_TABLE_COUNT: usize = 18;
const TABLE_DIGEST_BYTES: usize = 32;

/// The format written before digest rows carried an explicit generation.
const TABLE_DIGEST_FORMAT_GENERATION_V1: u32 = 1;

/// The exact serializer and manual row encodings consumed by `table_digests`.
///
/// Bump this when that byte format deliberately changes. The canonical
/// populated-fixture test pins the generation and fingerprint together, so a
/// Serde rename or field-shape change cannot silently turn valid persisted
/// metadata into corruption.
const TABLE_DIGEST_FORMAT_GENERATION: u32 = TABLE_DIGEST_FORMAT_GENERATION_V1;

/// Every table a v5 ledger lives in.
///
/// `ledger_id` repeats on every row so one database can hold more than one
/// ledger, and every table cascades from the head: a ledger that is deleted
/// leaves nothing of itself behind. Children are keyed by their parent's
/// `ordinal` rather than by its business key, because a served receipt's
/// caller is optional and a key that can be absent is not a key.
pub const LEDGER_TABLES_SQL: &str = "
    CREATE TABLE IF NOT EXISTS orchestration_ledger_heads (
        ledger_id TEXT PRIMARY KEY NOT NULL
            CHECK (length(ledger_id) BETWEEN 1 AND 512),
        revision INTEGER NOT NULL CHECK (revision >= 0),
        next_id INTEGER NOT NULL CHECK (next_id >= 0),
        bytes_held INTEGER NOT NULL CHECK (bytes_held >= 0),
        legacy_digest TEXT CHECK (
            legacy_digest IS NULL OR length(legacy_digest) = 64
        ),
        updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
        retention_days INTEGER NOT NULL DEFAULT 30 CHECK (retention_days > 0),
        swept_at_ms INTEGER NOT NULL DEFAULT 0 CHECK (swept_at_ms >= 0),
        table_digest_format_generation INTEGER CHECK (
            table_digest_format_generation >= 0
        ),
        table_digest_root BLOB CHECK (
            table_digest_root IS NULL OR length(table_digest_root) = 32
        )
    );
    CREATE TABLE IF NOT EXISTS ledger_runs (
        ledger_id TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        id TEXT NOT NULL,
        name TEXT NOT NULL,
        created_ms INTEGER NOT NULL,
        summary TEXT,
        coordinator TEXT,
        handover TEXT,
        PRIMARY KEY (ledger_id, ordinal),
        UNIQUE (ledger_id, id),
        FOREIGN KEY (ledger_id) REFERENCES orchestration_ledger_heads(ledger_id)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_run_autos (
        ledger_id TEXT NOT NULL,
        run_ordinal INTEGER NOT NULL CHECK (run_ordinal >= 0),
        max INTEGER NOT NULL CHECK (max >= 0),
        agent TEXT NOT NULL,
        team TEXT NOT NULL,
        pane TEXT NOT NULL,
        armed_ms INTEGER NOT NULL,
        PRIMARY KEY (ledger_id, run_ordinal),
        FOREIGN KEY (ledger_id, run_ordinal) REFERENCES ledger_runs(ledger_id, ordinal)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_tasks (
        ledger_id TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        run TEXT NOT NULL,
        id TEXT NOT NULL,
        spec TEXT NOT NULL,
        title TEXT NOT NULL,
        parent TEXT,
        status TEXT NOT NULL,
        result TEXT NOT NULL,
        failures INTEGER NOT NULL CHECK (failures >= 0),
        created_ms INTEGER NOT NULL,
        result_author TEXT,
        PRIMARY KEY (ledger_id, ordinal),
        UNIQUE (ledger_id, run, id),
        FOREIGN KEY (ledger_id) REFERENCES orchestration_ledger_heads(ledger_id)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_task_deps (
        ledger_id TEXT NOT NULL,
        task_ordinal INTEGER NOT NULL CHECK (task_ordinal >= 0),
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        dep TEXT NOT NULL,
        PRIMARY KEY (ledger_id, task_ordinal, ordinal),
        FOREIGN KEY (ledger_id, task_ordinal) REFERENCES ledger_tasks(ledger_id, ordinal)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_gates (
        ledger_id TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        run TEXT NOT NULL,
        id TEXT NOT NULL,
        task TEXT NOT NULL,
        question TEXT NOT NULL,
        status TEXT NOT NULL,
        resolution TEXT NOT NULL,
        created_ms INTEGER NOT NULL,
        resolved_ms INTEGER,
        held_for TEXT,
        PRIMARY KEY (ledger_id, ordinal),
        UNIQUE (ledger_id, run, id),
        FOREIGN KEY (ledger_id) REFERENCES orchestration_ledger_heads(ledger_id)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_gate_options (
        ledger_id TEXT NOT NULL,
        gate_ordinal INTEGER NOT NULL CHECK (gate_ordinal >= 0),
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        choice TEXT NOT NULL,
        PRIMARY KEY (ledger_id, gate_ordinal, ordinal),
        FOREIGN KEY (ledger_id, gate_ordinal) REFERENCES ledger_gates(ledger_id, ordinal)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_attachments (
        ledger_id TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        run TEXT NOT NULL,
        dispatch TEXT NOT NULL,
        home TEXT NOT NULL,
        worker TEXT NOT NULL,
        state TEXT NOT NULL,
        to_home TEXT NOT NULL,
        acked_seq INTEGER NOT NULL,
        imported_seq INTEGER NOT NULL,
        created_ms INTEGER NOT NULL,
        PRIMARY KEY (ledger_id, ordinal),
        UNIQUE (ledger_id, home, dispatch),
        FOREIGN KEY (ledger_id) REFERENCES orchestration_ledger_heads(ledger_id)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_dispatches (
        ledger_id TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        run TEXT NOT NULL,
        id TEXT NOT NULL,
        task TEXT NOT NULL,
        worker TEXT NOT NULL,
        started_ms INTEGER NOT NULL,
        ended_ms INTEGER,
        succeeded INTEGER CHECK (succeeded IS NULL OR succeeded IN (0, 1)),
        retry_of TEXT,
        remote TEXT,
        source TEXT,
        PRIMARY KEY (ledger_id, ordinal),
        UNIQUE (ledger_id, run, id),
        FOREIGN KEY (ledger_id) REFERENCES orchestration_ledger_heads(ledger_id)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_workers (
        ledger_id TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        run TEXT NOT NULL,
        id TEXT NOT NULL,
        team TEXT NOT NULL,
        agent TEXT NOT NULL,
        pane TEXT NOT NULL,
        state TEXT NOT NULL,
        started_ms INTEGER NOT NULL,
        dispatch TEXT,
        model TEXT,
        effort TEXT,
        session TEXT,
        ready_by_ms INTEGER,
        hook_unreachable_since_ms INTEGER,
        pane_missing_since_ms INTEGER,
        taken_over INTEGER NOT NULL DEFAULT 0 CHECK (taken_over IN (0, 1)),
        checkout TEXT,
        quiet_at INTEGER,
        archive TEXT,
        started_by TEXT,
        adopted_by INTEGER,
        on_quota_wall TEXT,
        quota_wait INTEGER NOT NULL DEFAULT 0 CHECK (quota_wait IN (0, 1)),
        PRIMARY KEY (ledger_id, ordinal),
        UNIQUE (ledger_id, run, id),
        FOREIGN KEY (ledger_id) REFERENCES orchestration_ledger_heads(ledger_id)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_messages (
        ledger_id TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        run TEXT NOT NULL,
        id TEXT NOT NULL,
        sender TEXT NOT NULL,
        recipient TEXT NOT NULL,
        kind TEXT NOT NULL,
        body TEXT NOT NULL,
        subject TEXT NOT NULL DEFAULT '',
        priority TEXT NOT NULL DEFAULT 'normal',
        payload TEXT NOT NULL DEFAULT '',
        thread TEXT,
        task TEXT,
        dispatch TEXT,
        author_seat TEXT,
        created_ms INTEGER NOT NULL,
        PRIMARY KEY (ledger_id, ordinal),
        UNIQUE (ledger_id, run, id),
        FOREIGN KEY (ledger_id) REFERENCES orchestration_ledger_heads(ledger_id)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_inboxes (
        ledger_id TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        run TEXT NOT NULL,
        address TEXT NOT NULL,
        open_delivery TEXT,
        open_holder TEXT,
        open_opened_ms INTEGER,
        PRIMARY KEY (ledger_id, ordinal),
        UNIQUE (ledger_id, run, address),
        FOREIGN KEY (ledger_id) REFERENCES orchestration_ledger_heads(ledger_id)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_inbox_pending (
        ledger_id TEXT NOT NULL,
        inbox_ordinal INTEGER NOT NULL CHECK (inbox_ordinal >= 0),
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        message TEXT NOT NULL,
        PRIMARY KEY (ledger_id, inbox_ordinal, ordinal),
        FOREIGN KEY (ledger_id, inbox_ordinal) REFERENCES ledger_inboxes(ledger_id, ordinal)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_inbox_open_messages (
        ledger_id TEXT NOT NULL,
        inbox_ordinal INTEGER NOT NULL CHECK (inbox_ordinal >= 0),
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        message TEXT NOT NULL,
        PRIMARY KEY (ledger_id, inbox_ordinal, ordinal),
        FOREIGN KEY (ledger_id, inbox_ordinal) REFERENCES ledger_inboxes(ledger_id, ordinal)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_bound (
        ledger_id TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        caller TEXT NOT NULL,
        run TEXT NOT NULL,
        PRIMARY KEY (ledger_id, ordinal),
        UNIQUE (ledger_id, caller),
        FOREIGN KEY (ledger_id) REFERENCES orchestration_ledger_heads(ledger_id)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_served (
        ledger_id TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        caller TEXT,
        request TEXT NOT NULL,
        fingerprint TEXT,
        renderer TEXT,
        inline TEXT,
        check_run TEXT,
        check_address TEXT,
        check_delivery TEXT,
        filed_ms INTEGER,
        expired INTEGER NOT NULL DEFAULT 0 CHECK (expired IN (0, 1)),
        PRIMARY KEY (ledger_id, ordinal),
        CHECK ((renderer IS NULL) = (inline IS NOT NULL)),
        CHECK ((renderer IS NULL) = (check_run IS NULL)),
        CHECK ((renderer IS NULL) = (check_address IS NULL)),
        FOREIGN KEY (ledger_id) REFERENCES orchestration_ledger_heads(ledger_id)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_served_messages (
        ledger_id TEXT NOT NULL,
        served_ordinal INTEGER NOT NULL CHECK (served_ordinal >= 0),
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        message TEXT NOT NULL,
        PRIMARY KEY (ledger_id, served_ordinal, ordinal),
        FOREIGN KEY (ledger_id, served_ordinal) REFERENCES ledger_served(ledger_id, ordinal)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_served_digests (
        ledger_id TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        digest BLOB NOT NULL CHECK (length(digest) = 32),
        PRIMARY KEY (ledger_id, ordinal),
        FOREIGN KEY (ledger_id, ordinal) REFERENCES ledger_served(ledger_id, ordinal)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_acked (
        ledger_id TEXT NOT NULL,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        run TEXT NOT NULL,
        address TEXT NOT NULL,
        delivery TEXT NOT NULL,
        holds_messages INTEGER NOT NULL CHECK (holds_messages IN (0, 1)),
        seq INTEGER NOT NULL CHECK (seq >= 0),
        current INTEGER NOT NULL CHECK (current IN (0, 1)),
        PRIMARY KEY (ledger_id, ordinal),
        FOREIGN KEY (ledger_id) REFERENCES orchestration_ledger_heads(ledger_id)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_acked_messages (
        ledger_id TEXT NOT NULL,
        acked_ordinal INTEGER NOT NULL CHECK (acked_ordinal >= 0),
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        message TEXT NOT NULL,
        PRIMARY KEY (ledger_id, acked_ordinal, ordinal),
        FOREIGN KEY (ledger_id, acked_ordinal) REFERENCES ledger_acked(ledger_id, ordinal)
            ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS ledger_table_digests (
        ledger_id TEXT NOT NULL,
        table_name TEXT NOT NULL,
        format_generation INTEGER NOT NULL CHECK (format_generation >= 0),
        digest BLOB NOT NULL CHECK (length(digest) = 32),
        PRIMARY KEY (ledger_id, table_name),
        FOREIGN KEY (ledger_id) REFERENCES orchestration_ledger_heads(ledger_id)
            ON DELETE CASCADE
    );
";

/// Add one ledger column when the current-schema store predates it.
fn ensure_ledger_column(
    connection: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> Result<bool, EffectJournalError> {
    let held: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            params![table, column],
            |row| row.get(0),
        )
        .map_err(|_| EffectJournalError::Database)?;
    if held != 0 {
        return Ok(false);
    }
    connection
        .execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {declaration};"
        ))
        .map_err(|_| EffectJournalError::Database)?;
    Ok(true)
}

/// Columns the ledger DDL has grown since a store may have been written.
///
/// The tables themselves replay through `IF NOT EXISTS`; a COLUMN needs this
/// second walk, because SQLite has no `ADD COLUMN IF NOT EXISTS`. Every entry
/// here is additive with a default, so an old row reads back exactly as it
/// was written — the SQL twin of a `#[serde(default)]` field, and the same
/// promise: a current-version store never needs a version bump for one of
/// these to begin arriving. The affected semantic table's old digest is
/// removed in the same transaction: its prior serializer did not include the
/// new field, while every unaffected table remains an integrity witness. The
/// aggregate root is invalidated too because it commits to every table digest.
pub fn ensure_ledger_columns(connection: &Connection) -> Result<(), EffectJournalError> {
    const GROWN: &[(&str, &str, &str)] = &[
        ("ledger_messages", "subject", "TEXT NOT NULL DEFAULT ''"),
        (
            "ledger_messages",
            "priority",
            "TEXT NOT NULL DEFAULT 'normal'",
        ),
        ("ledger_messages", "payload", "TEXT NOT NULL DEFAULT ''"),
        // Nullable with no default: a row written before seats were recorded
        // has no author to name, and `NULL` says exactly that. It reads back
        // as "nobody knows", which is how those rows already behaved.
        ("ledger_messages", "author_seat", "TEXT"),
        /* Who wrote a task's result, as one JSON document like the seat —
         * and NULL for every row written before authorship was recorded,
         * which the review reads as a claim by nobody known (t-6815). */
        ("ledger_tasks", "result_author", "TEXT"),
        ("ledger_dispatches", "retry_of", "TEXT"),
        ("ledger_workers", "model", "TEXT"),
        ("ledger_workers", "effort", "TEXT"),
        ("ledger_workers", "session", "TEXT"),
        ("ledger_workers", "ready_by_ms", "INTEGER"),
        ("ledger_workers", "hook_unreachable_since_ms", "INTEGER"),
        ("ledger_workers", "pane_missing_since_ms", "INTEGER"),
        ("ledger_gates", "held_for", "TEXT"),
        (
            "ledger_workers",
            "taken_over",
            "INTEGER NOT NULL DEFAULT 0 CHECK (taken_over IN (0, 1))",
        ),
        ("ledger_workers", "checkout", "TEXT"),
        /* Lineage, arriving the same additive way: a row written before a
         * worker's summoner was recorded has no honest parent to invent, and
         * NULL says exactly that. */
        ("ledger_workers", "started_by", "TEXT"),
        /* Which coordinator generation adopted an orphan. NULL for a worker
         * still under its summoner, and for every row written before seats
         * existed (t-2512). */
        ("ledger_workers", "adopted_by", "INTEGER"),
        ("ledger_dispatches", "remote", "TEXT"),
        ("ledger_dispatches", "source", "TEXT"),
        /* Who holds the open batch, and since when. Additive with NULL for
         * both, because a lease written before they were recorded has no
         * honest holder to invent — and `Ledger::deliver` reads that NULL as
         * "nobody wrote it down", which is what lets the next caller adopt it
         * rather than be treated as a stranger. */
        ("ledger_inboxes", "open_holder", "TEXT"),
        ("ledger_inboxes", "open_opened_ms", "INTEGER"),
        /* Retention, arriving the way every column above did: additive, with
         * a default that reads an old row back exactly as it was written. A
         * head with no policy column had no sweep either, so thirty days is
         * not a guess about what that window wanted — it is the first policy
         * it will ever have had. */
        (
            "orchestration_ledger_heads",
            "retention_days",
            "INTEGER NOT NULL DEFAULT 30 CHECK (retention_days > 0)",
        ),
        (
            "orchestration_ledger_heads",
            "swept_at_ms",
            "INTEGER NOT NULL DEFAULT 0 CHECK (swept_at_ms >= 0)",
        ),
        ("ledger_runs", "summary", "TEXT"),
        /* The coordinator seat, as one JSON document — the same shape the
         * summary takes, for the same reason: a seat is one fact with five
         * parts that are read and written together, and a store written
         * before seats existed reads back a run with none (t-2512). */
        ("ledger_runs", "coordinator", "TEXT"),
        ("ledger_served", "filed_ms", "INTEGER"),
        /* The handover standing order and a summons' own alternative, each
         * one JSON document like the seat — one fact whose parts are read
         * and written together — and NULL for every row written before the
         * wall was witnessed (t-3059). */
        ("ledger_runs", "handover", "TEXT"),
        ("ledger_workers", "on_quota_wall", "TEXT"),
        /* A summons' own `wait` beside its alternative (t-6427): a flag like
         * `taken_over`, and false for every row written before the word. */
        (
            "ledger_workers",
            "quota_wait",
            "INTEGER NOT NULL DEFAULT 0 CHECK (quota_wait IN (0, 1))",
        ),
        (
            "ledger_served",
            "expired",
            "INTEGER NOT NULL DEFAULT 0 CHECK (expired IN (0, 1))",
        ),
    ];
    /* Unversioned digest rows were written by format generation one. Derive
     * the migration default from the format's single named constant so the
     * SQL adoption rule cannot drift away from the writer. A fresh digest
     * table needs no default because every writer supplies the generation. */
    const FORMAT_GENERATION_COLUMN: &str = "format_generation";
    let declaration = format!(
        "INTEGER NOT NULL DEFAULT {} CHECK ({FORMAT_GENERATION_COLUMN} >= 0)",
        TABLE_DIGEST_FORMAT_GENERATION_V1
    );
    ensure_ledger_column(
        connection,
        "ledger_table_digests",
        FORMAT_GENERATION_COLUMN,
        &declaration,
    )?;

    /* The head's independently stored root survives loss of the digest table
     * itself. These nullable columns adopt an existing head only after its
     * surviving per-table witnesses pass the open-time verification below. */
    const HEAD_FORMAT_GENERATION_COLUMN: &str = "table_digest_format_generation";
    let declaration = format!("INTEGER CHECK ({HEAD_FORMAT_GENERATION_COLUMN} >= 0)");
    ensure_ledger_column(
        connection,
        "orchestration_ledger_heads",
        HEAD_FORMAT_GENERATION_COLUMN,
        &declaration,
    )?;
    const HEAD_ROOT_COLUMN: &str = "table_digest_root";
    let declaration = format!(
        "BLOB CHECK ({HEAD_ROOT_COLUMN} IS NULL OR length({HEAD_ROOT_COLUMN}) = \
         {TABLE_DIGEST_BYTES})"
    );
    ensure_ledger_column(
        connection,
        "orchestration_ledger_heads",
        HEAD_ROOT_COLUMN,
        &declaration,
    )?;

    for (table, column, declaration) in GROWN {
        if ensure_ledger_column(connection, table, column, declaration)?
            && let Some(table) = LedgerTable::from_name(table)
        {
            connection
                .execute(
                    "DELETE FROM ledger_table_digests WHERE table_name = ?1",
                    [table.name()],
                )
                .map_err(|_| EffectJournalError::Database)?;
            connection
                .execute(
                    "UPDATE orchestration_ledger_heads SET
                        table_digest_format_generation = NULL,
                        table_digest_root = NULL",
                    [],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
    }
    Ok(())
}

/// A ledger as it was found, and which revision it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub revision: u64,
    pub projection: LedgerProjectionV1,
    /// The head's own clock. Writes refuse to move it backwards, so a reader
    /// that will WRITE later must seed its clamp from this — a stamp seeded
    /// from zero re-refuses every mutation after the wall clock steps back.
    pub updated_at_ms: i64,
    /// Whether the head's derived byte count agrees with the rows.
    ///
    /// Ordinary readers require this to be true. Runtime boot may inspect a
    /// false value only long enough to validate and canonically rewrite a
    /// repairable legacy ledger.
    pub bytes_match: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoredLedgerHead {
    revision: i64,
    next_id: i64,
    bytes: i64,
    updated_at_ms: i64,
    retention_days: i64,
    swept_at_ms: i64,
}

/// How much prose a ledger is carrying, in bytes.
///
/// Kept on the head row rather than recomputed on demand: the number is wanted
/// to decide whether a mutation FITS, and a bound you have to walk the whole
/// ledger to learn is a bound that costs what it is protecting. Reads
/// recompute it once and refuse a head that disagrees — the stored number is a
/// convenience, never a second source of truth.
/// The strings a named alternative holds — counted for the seat's reason.
fn pinned_bytes(pinned: &Pinned) -> u64 {
    let plain = |held: &Option<String>| held.as_ref().map_or(0, |one| one.len() as u64);
    pinned.agent.len() as u64 + plain(&pinned.model) + plain(&pinned.effort)
}

fn bytes_held(projection: &LedgerProjectionV1) -> u64 {
    let text = |held: &Text| held.as_str().len() as u64;
    let maybe = |held: &Option<Text>| held.as_ref().map_or(0, text);
    let runs: u64 = projection
        .runs
        .iter()
        .map(|row| {
            text(&row.name)
                + row
                    .summary
                    .as_ref()
                    .map_or(0, |summary| summary.headlines.iter().map(text).sum::<u64>())
                /* The seat's two names count for the session's reason: a
                 * string the accounting skips is one a direct SQLite change
                 * could grow unwatched. */
                + row.coordinator.as_ref().map_or(0, |seat| {
                    seat.seat.len() as u64
                        + seat.actor.as_ref().map_or(0, |actor| actor.len() as u64)
                })
                + row.handover.as_ref().map_or(0, |policy| {
                    [&policy.on_quota_wall, &policy.on_classifier_decline]
                        .into_iter()
                        .flatten()
                        .map(pinned_bytes)
                        .sum::<u64>()
                })
        })
        .sum();
    let tasks: u64 = projection
        .tasks
        .iter()
        .map(|row| text(&row.spec) + text(&row.title) + text(&row.result))
        .sum();
    let plain = |held: &Option<String>| held.as_ref().map_or(0, |one| one.len() as u64);
    let workers: u64 = projection
        .workers
        .iter()
        .map(|row| {
            let session = row.session.as_ref().map_or(0, |session| {
                session.id.len() as u64
                    + session
                        .transcript_path
                        .as_ref()
                        .map_or(0, |path| path.len() as u64)
            });
            maybe(&row.archive)
                + plain(&row.model)
                + plain(&row.effort)
                + plain(&row.checkout)
                + session
                + row.on_quota_wall.as_ref().map_or(0, pinned_bytes)
        })
        .sum();
    let dispatches: u64 = projection
        .dispatches
        .iter()
        .map(|row| {
            plain(&row.retry_of)
                + plain(&row.source)
                + row.remote.as_ref().map_or(0, |seat| {
                    seat.outbox
                        .iter()
                        .map(|item| (item.body.as_str().len() + item.payload.as_str().len()) as u64)
                        .sum()
                })
        })
        .sum();
    let attachments: u64 = projection
        .attachments
        .iter()
        .map(|row| {
            row.to_home
                .iter()
                .map(|item| (item.body.as_str().len() + item.payload.as_str().len()) as u64)
                .sum::<u64>()
        })
        .sum();
    let messages: u64 = projection
        .messages
        .iter()
        .map(|row| text(&row.body) + text(&row.subject) + text(&row.payload))
        .sum();
    let gates: u64 = projection
        .gates
        .iter()
        .map(|row| {
            text(&row.question) + text(&row.resolution) + row.options.iter().map(text).sum::<u64>()
        })
        .sum();
    /* A caller's name counts here for the same reason it counts in `served`:
     * it is somebody's text. Bounded by `MAX_NAME` rather than `MAX_PROSE`,
     * so it moves the number without moving the risk — but a `Text` the
     * accounting skips is a `Text` that could grow unwatched. */
    let bound: u64 = projection.bound.iter().map(|row| text(&row.caller)).sum();
    /* And the seat holding an open batch, for the same reason: it is a name
     * this window put in a row, and a `Text`-sized value the accounting skips
     * is one a direct SQLite change could grow unwatched. */
    let leases: u64 = projection
        .inboxes
        .iter()
        .map(|row| row.open.as_ref().map_or(0, |open| plain(&open.holder)))
        .sum();
    let served: u64 = projection
        .served
        .iter()
        .map(|row| {
            maybe(&row.caller)
                + text(&row.request)
                + maybe(&row.fingerprint)
                + match &row.answer {
                    ServedAnswer::Inline(held) => held.len() as u64,
                    ServedAnswer::Check(_) => 0,
                }
        })
        .sum();
    runs + tasks + workers + dispatches + attachments + messages + gates + bound + served + leases
}

type TableDigest = [u8; TABLE_DIGEST_BYTES];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StoredTableDigest {
    format_generation: u32,
    digest: TableDigest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoredTableDigestRoot {
    Missing,
    Stale,
    Current(TableDigest),
}

impl StoredTableDigestRoot {
    fn from_sql(
        format_generation: Option<i64>,
        digest: Option<Vec<u8>>,
    ) -> Result<Self, EffectJournalError> {
        let Some(format_generation) = format_generation else {
            return if digest.is_none() {
                Ok(Self::Missing)
            } else {
                Err(EffectJournalError::Corrupt)
            };
        };
        let format_generation =
            u32::try_from(format_generation).map_err(|_| EffectJournalError::Corrupt)?;
        if format_generation != TABLE_DIGEST_FORMAT_GENERATION {
            return Ok(Self::Stale);
        }
        let digest = digest
            .ok_or(EffectJournalError::Corrupt)?
            .try_into()
            .map_err(|_| EffectJournalError::Corrupt)?;
        Ok(Self::Current(digest))
    }
}

/// The semantic tables whose rows make up a projection.
///
/// The digest rows are metadata, not another part of the projection. Keeping
/// this list typed makes the selective clear/write paths share one set of
/// names and prevents a new table from silently being hashed but never
/// rewritten (or the other way around).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LedgerTable {
    Runs,
    RunAutos,
    Tasks,
    TaskDeps,
    Gates,
    GateOptions,
    Attachments,
    Dispatches,
    Workers,
    Messages,
    Inboxes,
    InboxPending,
    InboxOpenMessages,
    Bound,
    Served,
    ServedMessages,
    Acked,
    AckedMessages,
}

impl LedgerTable {
    const ALL: [Self; LEDGER_TABLE_COUNT] = [
        Self::Runs,
        Self::RunAutos,
        Self::Tasks,
        Self::TaskDeps,
        Self::Gates,
        Self::GateOptions,
        Self::Attachments,
        Self::Dispatches,
        Self::Workers,
        Self::Messages,
        Self::Inboxes,
        Self::InboxPending,
        Self::InboxOpenMessages,
        Self::Bound,
        Self::Served,
        Self::ServedMessages,
        Self::Acked,
        Self::AckedMessages,
    ];

    /// Children are cleared before their parent, and are rewritten when a
    /// parent changes so a parent replacement cannot cascade away an
    /// otherwise unchanged child list.
    const CLEAR_ORDER: [Self; LEDGER_TABLE_COUNT] = [
        Self::AckedMessages,
        Self::Acked,
        Self::ServedMessages,
        Self::Served,
        Self::Bound,
        Self::InboxOpenMessages,
        Self::InboxPending,
        Self::Inboxes,
        Self::Messages,
        Self::Workers,
        Self::Dispatches,
        Self::TaskDeps,
        Self::Tasks,
        Self::GateOptions,
        Self::Gates,
        Self::Attachments,
        Self::RunAutos,
        Self::Runs,
    ];

    /// Tables reconciled row by row, in place, rather than cleared and
    /// rewritten. A parent updated in place fires no cascade, so its children
    /// need no `PARENT_CHILD` entry — the one exemption the DDL pin below
    /// reads from here instead of guessing.
    const RECONCILED_IN_PLACE: [Self; 1] = [Self::Served];

    const PARENT_CHILD: [(Self, Self); 6] = [
        (Self::Runs, Self::RunAutos),
        (Self::Tasks, Self::TaskDeps),
        (Self::Gates, Self::GateOptions),
        (Self::Inboxes, Self::InboxPending),
        (Self::Inboxes, Self::InboxOpenMessages),
        (Self::Acked, Self::AckedMessages),
    ];

    const fn index(self) -> usize {
        match self {
            Self::Runs => 0,
            Self::RunAutos => 1,
            Self::Tasks => 2,
            Self::TaskDeps => 3,
            Self::Gates => 4,
            Self::GateOptions => 5,
            Self::Attachments => 6,
            Self::Dispatches => 7,
            Self::Workers => 8,
            Self::Messages => 9,
            Self::Inboxes => 10,
            Self::InboxPending => 11,
            Self::InboxOpenMessages => 12,
            Self::Bound => 13,
            Self::Served => 14,
            Self::ServedMessages => 15,
            Self::Acked => 16,
            Self::AckedMessages => 17,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Runs => "ledger_runs",
            Self::RunAutos => "ledger_run_autos",
            Self::Tasks => "ledger_tasks",
            Self::TaskDeps => "ledger_task_deps",
            Self::Gates => "ledger_gates",
            Self::GateOptions => "ledger_gate_options",
            Self::Attachments => "ledger_attachments",
            Self::Dispatches => "ledger_dispatches",
            Self::Workers => "ledger_workers",
            Self::Messages => "ledger_messages",
            Self::Inboxes => "ledger_inboxes",
            Self::InboxPending => "ledger_inbox_pending",
            Self::InboxOpenMessages => "ledger_inbox_open_messages",
            Self::Bound => "ledger_bound",
            Self::Served => "ledger_served",
            Self::ServedMessages => "ledger_served_messages",
            Self::Acked => "ledger_acked",
            Self::AckedMessages => "ledger_acked_messages",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|table| table.name() == name)
    }

    fn reconciled_in_place(self) -> bool {
        Self::RECONCILED_IN_PLACE.contains(&self)
    }
}

fn table_digest(value: &impl serde::Serialize) -> Result<TableDigest, EffectJournalError> {
    let encoded = serde_json::to_vec(value).map_err(|_| EffectJournalError::Corrupt)?;
    Ok(Sha256::digest(encoded).into())
}

fn hash_optional_text(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
        None => hasher.update([0]),
    }
}

fn hash_optional_i64(hasher: &mut Sha256, value: Option<i64>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.to_le_bytes());
        }
        None => hasher.update([0]),
    }
}

fn served_row_digest(row: &ServedRow) -> TableDigest {
    let (
        caller,
        request,
        fingerprint,
        renderer,
        inline,
        check_run,
        check_address,
        check_delivery,
        filed_ms,
        expired,
    ) = served_values(row);
    let mut hasher = Sha256::new();
    hash_optional_text(&mut hasher, caller);
    hasher.update((request.len() as u64).to_le_bytes());
    hasher.update(request.as_bytes());
    hash_optional_text(&mut hasher, fingerprint);
    hash_optional_text(&mut hasher, renderer);
    hash_optional_text(&mut hasher, inline);
    hash_optional_text(&mut hasher, check_run);
    hash_optional_text(&mut hasher, check_address);
    hash_optional_text(&mut hasher, check_delivery);
    hash_optional_i64(&mut hasher, filed_ms);
    hasher.update([u8::from(expired)]);
    hasher.finalize().into()
}

fn served_row_digests(rows: &[ServedRow]) -> Vec<TableDigest> {
    rows.iter().map(served_row_digest).collect()
}

fn digest_rows(rows: &[TableDigest]) -> TableDigest {
    let mut hasher = Sha256::new();
    for row in rows {
        hasher.update(row);
    }
    hasher.finalize().into()
}

fn stored_table_digests(
    connection: &Connection,
    ledger_id: &str,
) -> Result<[Option<StoredTableDigest>; LedgerTable::ALL.len()], EffectJournalError> {
    let mut stored = [None; LedgerTable::ALL.len()];
    let mut statement = connection
        .prepare(
            "SELECT table_name, format_generation, digest FROM ledger_table_digests
              WHERE ledger_id = ?1",
        )
        .map_err(|_| EffectJournalError::Database)?;
    let mut rows = statement
        .query([ledger_id])
        .map_err(|_| EffectJournalError::Database)?;
    while let Some(row) = rows.next().map_err(|_| EffectJournalError::Database)? {
        let name: String = row.get(0).map_err(|_| EffectJournalError::Database)?;
        let Some(table) = LedgerTable::from_name(&name) else {
            continue;
        };
        let format_generation = row
            .get::<_, i64>(1)
            .map_err(|_| EffectJournalError::Database)?;
        let digest = row
            .get::<_, Vec<u8>>(2)
            .map_err(|_| EffectJournalError::Database)?
            .try_into()
            .map_err(|_| EffectJournalError::Corrupt)?;
        stored[table.index()] = Some(StoredTableDigest {
            format_generation: u32::try_from(format_generation)
                .map_err(|_| EffectJournalError::Corrupt)?,
            digest,
        });
    }
    Ok(stored)
}

/// Refuse rows that disagree with a digest from their successful write.
///
/// A missing digest remains the additive-upgrade boundary: stores from before
/// selective writes, and stores that predate a newly introduced table, have no
/// digest to compare until their next write seeds one. A digest from another
/// format generation is stale metadata rather than evidence about the current
/// serializer; it is removed only after every current-generation witness for
/// this ledger has passed, then the next write reseeds it.
fn require_table_integrity(
    connection: &Connection,
    ledger_id: &str,
    expected: &[TableDigest; LedgerTable::ALL.len()],
) -> Result<(), EffectJournalError> {
    let stored = stored_table_digests(connection, ledger_id)?;
    for table in LedgerTable::ALL {
        let index = table.index();
        if stored[index].is_some_and(|stored| {
            stored.format_generation == TABLE_DIGEST_FORMAT_GENERATION
                && stored.digest != expected[index]
        }) {
            return Err(EffectJournalError::Corrupt);
        }
    }
    connection
        .execute(
            "DELETE FROM ledger_table_digests
              WHERE ledger_id = ?1 AND format_generation <> ?2",
            params![ledger_id, i64::from(TABLE_DIGEST_FORMAT_GENERATION)],
        )
        .map_err(|_| EffectJournalError::Database)?;
    Ok(())
}

/// Refuse a write when a semantic table disappeared underneath the store.
///
/// Selective writes intentionally skip unchanged tables, so a missing table
/// cannot be discovered by its own DELETE anymore. The old full rewrite got
/// this check accidentally from walking every table; keep the failure closed
/// without paying for a row count or a full read of each table.
fn require_ledger_tables(connection: &Connection) -> Result<(), EffectJournalError> {
    let mut found = [false; LedgerTable::ALL.len()];
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name LIKE 'ledger_%'")
        .map_err(|_| EffectJournalError::Database)?;
    let mut rows = statement
        .query([])
        .map_err(|_| EffectJournalError::Database)?;
    while let Some(row) = rows.next().map_err(|_| EffectJournalError::Database)? {
        let name: String = row.get(0).map_err(|_| EffectJournalError::Database)?;
        if let Some(table) = LedgerTable::from_name(&name) {
            found[table.index()] = true;
        }
    }
    if found.into_iter().all(|present| present) {
        Ok(())
    } else {
        Err(EffectJournalError::Database)
    }
}

/// Hash each table independently, so a mutation to one small table does not
/// force the large receipt table through the write path.
fn table_digests(
    projection: &LedgerProjectionV1,
) -> Result<[TableDigest; LedgerTable::ALL.len()], EffectJournalError> {
    let run_autos = projection
        .runs
        .iter()
        .map(|row| row.auto.as_ref())
        .collect::<Vec<_>>();
    let task_deps = projection
        .tasks
        .iter()
        .map(|row| &row.deps)
        .collect::<Vec<_>>();
    let gate_options = projection
        .gates
        .iter()
        .map(|row| &row.options)
        .collect::<Vec<_>>();
    let inbox_pending = projection
        .inboxes
        .iter()
        .map(|row| &row.pending)
        .collect::<Vec<_>>();
    let inbox_open_messages = projection
        .inboxes
        .iter()
        .map(|row| row.open.as_ref().map(|open| &open.messages))
        .collect::<Vec<_>>();
    let served_messages = projection
        .served
        .iter()
        .map(|row| match &row.answer {
            ServedAnswer::Inline(_) => None,
            ServedAnswer::Check(about) => Some(&about.messages),
        })
        .collect::<Vec<_>>();
    let acked_messages = projection
        .acked
        .iter()
        .map(|row| row.messages.as_ref())
        .collect::<Vec<_>>();
    let served_rows = served_row_digests(&projection.served);

    Ok([
        table_digest(&projection.runs)?,
        table_digest(&run_autos)?,
        table_digest(&projection.tasks)?,
        table_digest(&task_deps)?,
        table_digest(&projection.gates)?,
        table_digest(&gate_options)?,
        table_digest(&projection.attachments)?,
        table_digest(&projection.dispatches)?,
        table_digest(&projection.workers)?,
        table_digest(&projection.messages)?,
        table_digest(&projection.inboxes)?,
        table_digest(&inbox_pending)?,
        table_digest(&inbox_open_messages)?,
        table_digest(&projection.bound)?,
        digest_rows(&served_rows),
        table_digest(&served_messages)?,
        table_digest(&projection.acked)?,
        table_digest(&acked_messages)?,
    ])
}

/// Find the table slices that differ from the last successful write.
///
/// Missing metadata means the database predates selective writes, so the
/// first write after an upgrade conservatively rewrites every table and seeds
/// all digests. Parent changes also select their children: SQLite's foreign
/// key cascade would otherwise remove a child that looked unchanged.
fn changed_tables(
    connection: &Connection,
    ledger_id: &str,
    expected: &[TableDigest; LedgerTable::ALL.len()],
) -> Result<[bool; LedgerTable::ALL.len()], EffectJournalError> {
    require_ledger_tables(connection)?;
    let stored = stored_table_digests(connection, ledger_id)?;
    let mut changed = [false; LedgerTable::ALL.len()];
    for table in LedgerTable::ALL {
        let index = table.index();
        changed[index] = stored[index].is_none_or(|stored| {
            stored.format_generation != TABLE_DIGEST_FORMAT_GENERATION
                || stored.digest != expected[index]
        });
    }
    for (parent, child) in LedgerTable::PARENT_CHILD {
        if changed[parent.index()] {
            changed[child.index()] = true;
        }
    }
    Ok(changed)
}

fn store_table_digests(
    connection: &Connection,
    ledger_id: &str,
    changed: &[bool; LedgerTable::ALL.len()],
    digests: &[TableDigest; LedgerTable::ALL.len()],
) -> Result<(), EffectJournalError> {
    for table in LedgerTable::ALL {
        let index = table.index();
        if !changed[index] {
            continue;
        }
        connection
            .execute(
                "INSERT INTO ledger_table_digests (
                    ledger_id, table_name, format_generation, digest
                 ) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (ledger_id, table_name) DO UPDATE SET
                    format_generation = excluded.format_generation,
                    digest = excluded.digest",
                params![
                    ledger_id,
                    table.name(),
                    i64::from(TABLE_DIGEST_FORMAT_GENERATION),
                    digests[index].as_slice()
                ],
            )
            .map_err(|_| EffectJournalError::Database)?;
    }
    Ok(())
}

/// Write a ledger into the tables, replacing whatever was there.
///
/// The head is moved with the same compare-and-set schema four used — the
/// revision the caller expects, or nothing happens — and changed rows are
/// reconciled inside the caller's transaction, so a ledger is never half of
/// two.
pub fn write(
    connection: &Connection,
    ledger_id: &str,
    expected_revision: u64,
    next_revision: u64,
    projection: &LedgerProjectionV1,
    at_ms: i64,
) -> Result<(), EffectJournalError> {
    if at_ms < 0 {
        return Err(EffectJournalError::InvalidInput { field: "now_ms" });
    }
    let held_bytes = to_sql_u64(bytes_held(projection))?;
    let current: Option<(i64, i64)> = connection
        .query_row(
            "SELECT revision, updated_at_ms FROM orchestration_ledger_heads
              WHERE ledger_id = ?1",
            [ledger_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| EffectJournalError::Database)?;
    if current.is_some_and(|(_, updated_at_ms)| at_ms < updated_at_ms) {
        return Err(EffectJournalError::TimestampRegression);
    }
    let digests = table_digests(projection)?;
    let digest_root = digest_rows(&digests);
    match current {
        Some(_) => {
            let changed = connection
                .execute(
                    "UPDATE orchestration_ledger_heads
                        SET revision = ?1, next_id = ?2, bytes_held = ?3, updated_at_ms = ?4,
                            retention_days = ?7, swept_at_ms = ?8,
                            table_digest_format_generation = ?9, table_digest_root = ?10
                      WHERE ledger_id = ?5 AND revision = ?6",
                    params![
                        to_sql_u64(next_revision)?,
                        to_sql_u64(projection.next_id)?,
                        held_bytes,
                        at_ms,
                        ledger_id,
                        to_sql_u64(expected_revision)?,
                        i64::from(projection.retention_days),
                        projection.swept_at_ms.max(0),
                        i64::from(TABLE_DIGEST_FORMAT_GENERATION),
                        digest_root.as_slice(),
                    ],
                )
                .map_err(|_| EffectJournalError::Database)?;
            if changed != 1 {
                return Err(EffectJournalError::RevisionMismatch);
            }
        }
        None if expected_revision == 0 => {
            connection
                .execute(
                    "INSERT INTO orchestration_ledger_heads (
                        ledger_id, revision, next_id, bytes_held, updated_at_ms,
                        retention_days, swept_at_ms,
                        table_digest_format_generation, table_digest_root
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        ledger_id,
                        to_sql_u64(next_revision)?,
                        to_sql_u64(projection.next_id)?,
                        held_bytes,
                        at_ms,
                        i64::from(projection.retention_days),
                        projection.swept_at_ms.max(0),
                        i64::from(TABLE_DIGEST_FORMAT_GENERATION),
                        digest_root.as_slice(),
                    ],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
        None => return Err(EffectJournalError::RevisionMismatch),
    }

    let changed = changed_tables(connection, ledger_id, &digests)?;
    clear_rows(connection, ledger_id, &changed)?;
    write_rows(connection, ledger_id, projection, &changed)?;
    store_table_digests(connection, ledger_id, &changed, &digests)
}

/// Everything belonging to one ledger, in the order the children must go
/// before their parents.
fn clear_rows(
    connection: &Connection,
    ledger_id: &str,
    changed: &[bool; LedgerTable::ALL.len()],
) -> Result<(), EffectJournalError> {
    for table in LedgerTable::CLEAR_ORDER {
        if !changed[table.index()] {
            continue;
        }
        /* Receipts are the one large append-heavy table. Their rows are
         * reconciled by ordinal below, so deleting the whole table here would
         * throw away the increment this path is meant to preserve. */
        if table.reconciled_in_place() {
            continue;
        }
        connection
            .execute(
                &format!("DELETE FROM {} WHERE ledger_id = ?1", table.name()),
                [ledger_id],
            )
            .map_err(|_| EffectJournalError::Database)?;
    }
    Ok(())
}

/// An index as a column value, refusing a ledger too long to number.
fn ordinal(at: usize) -> Result<i64, EffectJournalError> {
    i64::try_from(at).map_err(|_| EffectJournalError::Corrupt)
}

/// The columns a served row occupies, in the same pairing enforced by the
/// table checks and by `read_repairable`.
type ServedValues<'a> = (
    Option<&'a str>,
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    Option<i64>,
    bool,
);

fn served_values(row: &ServedRow) -> ServedValues<'_> {
    let (renderer, inline, check_run, check_address, check_delivery) = match &row.answer {
        ServedAnswer::Inline(held) => (None, Some(held.as_str()), None, None, None),
        ServedAnswer::Check(about) => (
            Some(CHECK_RENDERER),
            None,
            Some(about.run.as_str()),
            Some(about.address.as_str()),
            about.delivery.as_deref(),
        ),
    };
    (
        row.caller.as_ref().map(Text::as_str),
        row.request.as_str(),
        row.fingerprint.as_ref().map(Text::as_str),
        renderer,
        inline,
        check_run,
        check_address,
        check_delivery,
        row.filed_ms,
        row.expired,
    )
}

/// Reconcile receipts by ordinal instead of deleting and re-inserting the
fn write_served_rows(
    connection: &Connection,
    ledger_id: &str,
    rows: &[ServedRow],
) -> Result<(), EffectJournalError> {
    let expected = served_row_digests(rows);
    let mut existing = Vec::new();
    each(
        connection,
        "SELECT ordinal, digest FROM ledger_served_digests
           WHERE ledger_id = ?1 ORDER BY ordinal",
        ledger_id,
        |row| {
            existing.push((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?));
            Ok(())
        },
    )?;
    for (at, row) in rows.iter().enumerate() {
        let ordinal = ordinal(at)?;
        if existing.get(at).is_some_and(|(held, digest)| {
            *held == ordinal && digest.as_slice() == expected[at].as_slice()
        }) {
            continue;
        }
        let (
            caller,
            request,
            fingerprint,
            renderer,
            inline,
            check_run,
            check_address,
            check_delivery,
            filed_ms,
            expired,
        ) = served_values(row);
        connection
            .execute(
                "INSERT INTO ledger_served (
                    ledger_id, ordinal, caller, request, fingerprint, renderer,
                    inline, check_run, check_address, check_delivery, filed_ms,
                    expired
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT (ledger_id, ordinal) DO UPDATE SET
                    caller = excluded.caller,
                    request = excluded.request,
                    fingerprint = excluded.fingerprint,
                    renderer = excluded.renderer,
                    inline = excluded.inline,
                    check_run = excluded.check_run,
                    check_address = excluded.check_address,
                    check_delivery = excluded.check_delivery,
                    filed_ms = excluded.filed_ms,
                    expired = excluded.expired",
                params![
                    ledger_id,
                    ordinal,
                    caller,
                    request,
                    fingerprint,
                    renderer,
                    inline,
                    check_run,
                    check_address,
                    check_delivery,
                    filed_ms,
                    expired,
                ],
            )
            .map_err(|_| EffectJournalError::Database)?;
        connection
            .execute(
                "INSERT INTO ledger_served_digests (ledger_id, ordinal, digest)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT (ledger_id, ordinal) DO UPDATE SET digest = excluded.digest",
                params![ledger_id, ordinal, expected[at].as_slice()],
            )
            .map_err(|_| EffectJournalError::Database)?;
    }

    connection
        .execute(
            "DELETE FROM ledger_served WHERE ledger_id = ?1 AND ordinal >= ?2",
            params![ledger_id, ordinal(rows.len())?],
        )
        .map_err(|_| EffectJournalError::Database)?;
    connection
        .execute(
            "DELETE FROM ledger_served_digests WHERE ledger_id = ?1 AND ordinal >= ?2",
            params![ledger_id, ordinal(rows.len())?],
        )
        .map_err(|_| EffectJournalError::Database)?;
    Ok(())
}

fn write_rows(
    connection: &Connection,
    ledger_id: &str,
    projection: &LedgerProjectionV1,
    changed: &[bool; LedgerTable::ALL.len()],
) -> Result<(), EffectJournalError> {
    for (at, row) in projection.runs.iter().enumerate() {
        let at = ordinal(at)?;
        if changed[LedgerTable::Runs.index()] {
            connection
                .execute(
                    "INSERT INTO ledger_runs (
                        ledger_id, ordinal, id, name, created_ms, summary, coordinator, handover
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        ledger_id,
                        at,
                        row.id,
                        row.name.as_str(),
                        row.created_ms,
                        row.summary
                            .as_ref()
                            .map(serde_json::to_string)
                            .transpose()
                            .map_err(|_| EffectJournalError::Corrupt)?,
                        row.coordinator
                            .as_ref()
                            .map(serde_json::to_string)
                            .transpose()
                            .map_err(|_| EffectJournalError::Corrupt)?,
                        row.handover
                            .as_ref()
                            .map(serde_json::to_string)
                            .transpose()
                            .map_err(|_| EffectJournalError::Corrupt)?,
                    ],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
        if changed[LedgerTable::RunAutos.index()]
            && let Some(auto) = &row.auto
        {
            connection
                .execute(
                    "INSERT INTO ledger_run_autos (
                        ledger_id, run_ordinal, max, agent, team, pane, armed_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        ledger_id,
                        at,
                        i64::from(auto.max),
                        auto.agent,
                        auto.team,
                        auto.pane,
                        auto.armed_ms
                    ],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
    }
    for (at, row) in projection.tasks.iter().enumerate() {
        let at = ordinal(at)?;
        if changed[LedgerTable::Tasks.index()] {
            connection
                .execute(
                    "INSERT INTO ledger_tasks (
                        ledger_id, ordinal, run, id, spec, title, parent, status,
                        result, failures, created_ms, result_author
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        ledger_id,
                        at,
                        row.run,
                        row.id,
                        row.spec.as_str(),
                        row.title.as_str(),
                        row.parent,
                        word(&row.status)?,
                        row.result.as_str(),
                        i64::from(row.failures),
                        row.created_ms,
                        row.result_author
                            .as_ref()
                            .map(serde_json::to_string)
                            .transpose()
                            .map_err(|_| EffectJournalError::Corrupt)?,
                    ],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
        if changed[LedgerTable::TaskDeps.index()] {
            for (which, dep) in row.deps.iter().enumerate() {
                connection
                    .execute(
                        "INSERT INTO ledger_task_deps (ledger_id, task_ordinal, ordinal, dep)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![ledger_id, at, ordinal(which)?, dep],
                    )
                    .map_err(|_| EffectJournalError::Database)?;
            }
        }
    }
    for (at, row) in projection.gates.iter().enumerate() {
        let at = ordinal(at)?;
        if changed[LedgerTable::Gates.index()] {
            connection
                .execute(
                    "INSERT INTO ledger_gates (
                        ledger_id, ordinal, run, id, task, question, status,
                        resolution, created_ms, resolved_ms, held_for
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        ledger_id,
                        at,
                        row.run,
                        row.id,
                        row.task,
                        row.question.as_str(),
                        word(&row.status)?,
                        row.resolution.as_str(),
                        row.created_ms,
                        row.resolved_ms,
                        row.held_for,
                    ],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
        if changed[LedgerTable::GateOptions.index()] {
            for (which, choice) in row.options.iter().enumerate() {
                connection
                    .execute(
                        "INSERT INTO ledger_gate_options (ledger_id, gate_ordinal, ordinal, choice)
                         VALUES (?1, ?2, ?3, ?4)",
                        params![ledger_id, at, ordinal(which)?, choice.as_str()],
                    )
                    .map_err(|_| EffectJournalError::Database)?;
            }
        }
    }
    if changed[LedgerTable::Attachments.index()] {
        for (at, row) in projection.attachments.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO ledger_attachments (
                        ledger_id, ordinal, run, dispatch, home, worker, state,
                        to_home, acked_seq, imported_seq, created_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![
                        ledger_id,
                        ordinal(at)?,
                        row.run,
                        row.dispatch,
                        row.home,
                        row.worker,
                        word(&row.state)?,
                        serde_json::to_string(&row.to_home)
                            .map_err(|_| EffectJournalError::Corrupt)?,
                        row.acked_seq,
                        row.imported_seq,
                        row.created_ms,
                    ],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
    }
    if changed[LedgerTable::Dispatches.index()] {
        for (at, row) in projection.dispatches.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO ledger_dispatches (
                        ledger_id, ordinal, run, id, task, worker, started_ms,
                        ended_ms, succeeded, retry_of, remote, source
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        ledger_id,
                        ordinal(at)?,
                        row.run,
                        row.id,
                        row.task,
                        row.worker,
                        row.started_ms,
                        row.ended_ms,
                        row.succeeded,
                        row.retry_of,
                        row.remote
                            .as_ref()
                            .map(serde_json::to_string)
                            .transpose()
                            .map_err(|_| EffectJournalError::Corrupt)?,
                        row.source,
                    ],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
    }
    if changed[LedgerTable::Workers.index()] {
        for (at, row) in projection.workers.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO ledger_workers (
                        ledger_id, ordinal, run, id, team, agent, pane, state,
                        started_ms, dispatch, model, effort, session, ready_by_ms,
                        hook_unreachable_since_ms, pane_missing_since_ms,
                        taken_over, checkout, quiet_at, archive, started_by, adopted_by,
                        on_quota_wall, quota_wait
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
                    params![
                        ledger_id,
                        ordinal(at)?,
                        row.run,
                        row.id,
                        row.team,
                        row.agent,
                        row.pane,
                        word(&row.state)?,
                        row.started_ms,
                        row.dispatch,
                        row.model,
                        row.effort,
                        row.session
                            .as_ref()
                            .map(serde_json::to_string)
                            .transpose()
                            .map_err(|_| EffectJournalError::Corrupt)?,
                        row.ready_by_ms,
                        row.hook_unreachable_since_ms,
                        row.pane_missing_since_ms,
                        row.taken_over,
                        row.checkout,
                        row.quiet_at,
                        row.archive.as_ref().map(Text::as_str),
                        row.started_by,
                        row.adopted_by.map(i64::from),
                        row.on_quota_wall
                            .as_ref()
                            .map(serde_json::to_string)
                            .transpose()
                            .map_err(|_| EffectJournalError::Corrupt)?,
                        row.quota_wait,
                    ],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
    }
    if changed[LedgerTable::Messages.index()] {
        for (at, row) in projection.messages.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO ledger_messages (
                        ledger_id, ordinal, run, id, sender, recipient, kind, body,
                        subject, priority, payload, thread, task, dispatch,
                        author_seat, created_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                               ?15, ?16)",
                    params![
                        ledger_id,
                        ordinal(at)?,
                        row.run,
                        row.id,
                        row.from,
                        row.to,
                        word(&row.kind)?,
                        row.body.as_str(),
                        row.subject.as_str(),
                        word(&row.priority)?,
                        row.payload.as_str(),
                        row.thread,
                        row.task,
                        row.dispatch,
                        row.author_seat,
                        row.created_ms,
                    ],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
    }
    for (at, row) in projection.inboxes.iter().enumerate() {
        let at = ordinal(at)?;
        if changed[LedgerTable::Inboxes.index()] {
            connection
                .execute(
                    "INSERT INTO ledger_inboxes (
                        ledger_id, ordinal, run, address, open_delivery,
                        open_holder, open_opened_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        ledger_id,
                        at,
                        row.run,
                        row.address,
                        row.open.as_ref().map(|open| open.id.as_str()),
                        row.open.as_ref().and_then(|open| open.holder.as_deref()),
                        row.open.as_ref().and_then(|open| open.opened_ms)
                    ],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
        if changed[LedgerTable::InboxPending.index()] {
            for (which, message) in row.pending.iter().enumerate() {
                connection
                    .execute(
                        "INSERT INTO ledger_inbox_pending (
                            ledger_id, inbox_ordinal, ordinal, message
                         ) VALUES (?1, ?2, ?3, ?4)",
                        params![ledger_id, at, ordinal(which)?, message],
                    )
                    .map_err(|_| EffectJournalError::Database)?;
            }
        }
        if changed[LedgerTable::InboxOpenMessages.index()]
            && let Some(open) = &row.open
        {
            for (which, message) in open.messages.iter().enumerate() {
                connection
                    .execute(
                        "INSERT INTO ledger_inbox_open_messages (
                            ledger_id, inbox_ordinal, ordinal, message
                         ) VALUES (?1, ?2, ?3, ?4)",
                        params![ledger_id, at, ordinal(which)?, message],
                    )
                    .map_err(|_| EffectJournalError::Database)?;
            }
        }
    }
    if changed[LedgerTable::Bound.index()] {
        for (at, row) in projection.bound.iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO ledger_bound (ledger_id, ordinal, caller, run)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![ledger_id, ordinal(at)?, row.caller.as_str(), row.run],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
    }
    if changed[LedgerTable::Served.index()] {
        write_served_rows(connection, ledger_id, &projection.served)?;
    }
    if changed[LedgerTable::ServedMessages.index()] {
        for (at, row) in projection.served.iter().enumerate() {
            let about = match &row.answer {
                ServedAnswer::Inline(_) => None,
                ServedAnswer::Check(about) => Some(about),
            };
            let at = ordinal(at)?;
            for (which, message) in about
                .iter()
                .flat_map(|about| about.messages.iter())
                .enumerate()
            {
                connection
                    .execute(
                        "INSERT INTO ledger_served_messages (
                            ledger_id, served_ordinal, ordinal, message
                         ) VALUES (?1, ?2, ?3, ?4)",
                        params![ledger_id, at, ordinal(which)?, message],
                    )
                    .map_err(|_| EffectJournalError::Database)?;
            }
        }
    }
    for (at, row) in projection.acked.iter().enumerate() {
        let at = ordinal(at)?;
        if changed[LedgerTable::Acked.index()] {
            connection
                .execute(
                    "INSERT INTO ledger_acked (
                        ledger_id, ordinal, run, address, delivery, holds_messages,
                        seq, current
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        ledger_id,
                        at,
                        row.run,
                        row.address,
                        row.delivery,
                        row.messages.is_some(),
                        i64::from(row.seq),
                        row.current,
                    ],
                )
                .map_err(|_| EffectJournalError::Database)?;
        }
        if changed[LedgerTable::AckedMessages.index()] {
            for (which, message) in row.messages.iter().flatten().enumerate() {
                connection
                    .execute(
                        "INSERT INTO ledger_acked_messages (
                            ledger_id, acked_ordinal, ordinal, message
                         ) VALUES (?1, ?2, ?3, ?4)",
                        params![ledger_id, at, ordinal(which)?, message],
                    )
                    .map_err(|_| EffectJournalError::Database)?;
            }
        }
    }
    Ok(())
}

/// The word serde writes an enum as, which is the word the column holds.
///
/// Taken from the type's own `Serialize` rather than from a match written
/// here: a spelling written twice is a spelling that can disagree with itself,
/// and the file has to keep meaning what the projection meant.
fn word<T: serde::Serialize>(value: &T) -> Result<String, EffectJournalError> {
    match serde_json::to_value(value).map_err(|_| EffectJournalError::Corrupt)? {
        serde_json::Value::String(held) => Ok(held),
        _ => Err(EffectJournalError::Corrupt),
    }
}

/// The value a word names, refusing a word this window does not know.
fn from_word<T: serde::de::DeserializeOwned>(held: &str) -> Result<T, EffectJournalError> {
    serde_json::from_value(serde_json::Value::String(held.to_string()))
        .map_err(|_| EffectJournalError::Corrupt)
}

/// Every child list of one table, gathered by the parent it belongs to.
///
/// One statement rather than one per parent: a query per row is how a load
/// becomes quadratic in the thing that grows, which the ledger's own rules
/// learned the hard way.
///
/// The `ORDER BY` here cannot be caught failing today: every child table is
/// keyed by `(ledger_id, parent, ordinal)`, so the index SQLite reaches for
/// already hands the rows back in that order and dropping the clause changes
/// nothing observable. It stays because the order is the CONTRACT and not a
/// side effect of which index a planner picked this year.
fn children(
    connection: &Connection,
    table: &str,
    parent: &str,
    held_column: &str,
    ledger_id: &str,
) -> Result<std::collections::HashMap<i64, Vec<String>>, EffectJournalError> {
    let mut held: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
    let mut statement = connection
        .prepare(&format!(
            "SELECT {parent}, {held_column} FROM {table}
              WHERE ledger_id = ?1 ORDER BY {parent}, ordinal"
        ))
        .map_err(|_| EffectJournalError::Database)?;
    let mut rows = statement
        .query([ledger_id])
        .map_err(|_| EffectJournalError::Database)?;
    while let Some(row) = rows.next().map_err(|_| EffectJournalError::Database)? {
        let at: i64 = row.get(0).map_err(|_| EffectJournalError::Corrupt)?;
        let one: String = row.get(1).map_err(|_| EffectJournalError::Corrupt)?;
        held.entry(at).or_default().push(one);
    }
    Ok(held)
}

/// Which revision the tables are at, without reading them.
///
/// The owner actor asks this before every request to learn whether anything
/// but itself has moved the ledger. Reading the whole thing to answer a
/// yes-or-no question would make that check cost the size of the ledger — the
/// same mistake the boot rules made twice, one screen further in.
pub fn head_revision(
    connection: &Connection,
    ledger_id: &str,
) -> Result<Option<u64>, EffectJournalError> {
    connection
        .query_row(
            "SELECT revision FROM orchestration_ledger_heads WHERE ledger_id = ?1",
            [ledger_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|_| EffectJournalError::Database)?
        .map(from_sql_u64)
        .transpose()
}

/// The digest the legacy import stamped on this head, when one was.
pub fn legacy_digest(
    connection: &Connection,
    ledger_id: &str,
) -> Result<Option<String>, EffectJournalError> {
    connection
        .query_row(
            "SELECT legacy_digest FROM orchestration_ledger_heads WHERE ledger_id = ?1",
            [ledger_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| EffectJournalError::Database)
        .map(Option::flatten)
}

/// Stamp the digest of the legacy source this head was imported from.
///
/// Written once, by the import, in the same transaction as the rows — a head
/// that exists without it was born fresh, and that absence is data.
pub fn stamp_legacy_digest(
    connection: &Connection,
    ledger_id: &str,
    legacy_digest: &str,
) -> Result<(), EffectJournalError> {
    let changed = connection
        .execute(
            "UPDATE orchestration_ledger_heads SET legacy_digest = ?1 WHERE ledger_id = ?2",
            rusqlite::params![legacy_digest, ledger_id],
        )
        .map_err(|_| EffectJournalError::Database)?;
    if changed != 1 {
        return Err(EffectJournalError::RevisionMismatch);
    }
    Ok(())
}

/// Read one ledger back, in the order it was written.
///
/// The head says which revision this is and how much prose it was carrying;
/// the rows are gathered `ORDER BY ordinal` so what comes back is the ledger
/// that went in rather than one that merely holds the same facts. A head whose
/// `bytes_held` disagrees with the rows is refused: the number is a
/// convenience for deciding what fits, and a convenience that can lie is worse
/// than no convenience at all.
pub fn read(
    connection: &Connection,
    ledger_id: &str,
    schema: u32,
) -> Result<Option<Held>, EffectJournalError> {
    let held = read_repairable(connection, ledger_id, schema)?;
    if held.as_ref().is_some_and(|held| !held.bytes_match) {
        return Err(EffectJournalError::Corrupt);
    }
    Ok(held)
}

/// Verify every ledger head once while a current-schema store opens.
///
/// Current-schema open calls this after replaying additive DDL. The repairable
/// reader deliberately leaves its byte-count decision to runtime boot, but its
/// per-table digests still distinguish a newly introduced table (no stored
/// digest) from a populated table that disappeared (stored digest mismatch),
/// and the independent head root preserves that witness if the digest table
/// itself disappeared. Keeping this scan at open avoids reserializing the full
/// projection on every runtime read.
pub(crate) fn require_current_table_integrity(
    connection: &Connection,
    schema: u32,
) -> Result<(), EffectJournalError> {
    let mut statement = connection
        .prepare(
            "SELECT ledger_id, table_digest_format_generation, table_digest_root
               FROM orchestration_ledger_heads ORDER BY ledger_id",
        )
        .map_err(|_| EffectJournalError::Database)?;
    let ledger_heads = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })
        .map_err(|_| EffectJournalError::Database)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| EffectJournalError::Database)?;
    for (ledger_id, root_generation, stored_root) in ledger_heads {
        let stored_root = StoredTableDigestRoot::from_sql(root_generation, stored_root)?;
        let held = read_existing_repairable(connection, &ledger_id, schema)?;
        let expected = table_digests(&held.projection)?;
        let expected_root = digest_rows(&expected);
        if matches!(stored_root, StoredTableDigestRoot::Current(root) if root != expected_root) {
            return Err(EffectJournalError::Corrupt);
        }
        require_table_integrity(connection, &ledger_id, &expected)?;

        let replacement: Option<(Option<i64>, Option<&[u8]>)> = match stored_root {
            StoredTableDigestRoot::Missing => Some((
                Some(i64::from(TABLE_DIGEST_FORMAT_GENERATION)),
                Some(expected_root.as_slice()),
            )),
            StoredTableDigestRoot::Stale => Some((None, None)),
            StoredTableDigestRoot::Current(_) => None,
        };
        if let Some((format_generation, root)) = replacement {
            let changed = connection
                .execute(
                    "UPDATE orchestration_ledger_heads SET
                        table_digest_format_generation = ?1, table_digest_root = ?2
                      WHERE ledger_id = ?3",
                    params![format_generation, root, ledger_id],
                )
                .map_err(|_| EffectJournalError::Database)?;
            if changed != 1 {
                return Err(EffectJournalError::Database);
            }
        }
    }
    Ok(())
}

/// Read rows even when only the head's derived byte count is stale.
///
/// This is not a relaxed journal read: callers still receive `bytes_match`
/// and must validate the full projection before rewriting it. It exists for
/// boot recovery of a pre-retention receipt whose acknowledgement generation
/// never landed. Every ordinary journal path continues through [`read`] and
/// fails closed on a mismatch.
pub fn read_repairable(
    connection: &Connection,
    ledger_id: &str,
    schema: u32,
) -> Result<Option<Held>, EffectJournalError> {
    let head = stored_ledger_head(connection, ledger_id)
        .optional()
        .map_err(|_| EffectJournalError::Database)?;
    head.map(|head| read_repairable_from_head(connection, ledger_id, schema, head))
        .transpose()
}

fn stored_ledger_head(
    connection: &Connection,
    ledger_id: &str,
) -> rusqlite::Result<StoredLedgerHead> {
    connection.query_row(
        "SELECT revision, next_id, bytes_held, updated_at_ms,
                retention_days, swept_at_ms
           FROM orchestration_ledger_heads
          WHERE ledger_id = ?1",
        [ledger_id],
        |row| {
            Ok(StoredLedgerHead {
                revision: row.get(0)?,
                next_id: row.get(1)?,
                bytes: row.get(2)?,
                updated_at_ms: row.get(3)?,
                retention_days: row.get(4)?,
                swept_at_ms: row.get(5)?,
            })
        },
    )
}

fn read_existing_repairable(
    connection: &Connection,
    ledger_id: &str,
    schema: u32,
) -> Result<Held, EffectJournalError> {
    let head =
        stored_ledger_head(connection, ledger_id).map_err(|_| EffectJournalError::Database)?;
    read_repairable_from_head(connection, ledger_id, schema, head)
}

fn read_repairable_from_head(
    connection: &Connection,
    ledger_id: &str,
    schema: u32,
    head: StoredLedgerHead,
) -> Result<Held, EffectJournalError> {
    let StoredLedgerHead {
        revision,
        next_id,
        bytes,
        updated_at_ms,
        retention_days,
        swept_at_ms,
    } = head;

    let deps = children(
        connection,
        "ledger_task_deps",
        "task_ordinal",
        "dep",
        ledger_id,
    )?;
    let choices = children(
        connection,
        "ledger_gate_options",
        "gate_ordinal",
        "choice",
        ledger_id,
    )?;
    let pending = children(
        connection,
        "ledger_inbox_pending",
        "inbox_ordinal",
        "message",
        ledger_id,
    )?;
    let leased = children(
        connection,
        "ledger_inbox_open_messages",
        "inbox_ordinal",
        "message",
        ledger_id,
    )?;
    let answered = children(
        connection,
        "ledger_served_messages",
        "served_ordinal",
        "message",
        ledger_id,
    )?;
    let spent = children(
        connection,
        "ledger_acked_messages",
        "acked_ordinal",
        "message",
        ledger_id,
    )?;

    let mut autos: std::collections::HashMap<i64, Auto> = std::collections::HashMap::new();
    each(
        connection,
        "SELECT run_ordinal, max, agent, team, pane, armed_ms FROM ledger_run_autos
          WHERE ledger_id = ?1",
        ledger_id,
        |row| {
            let at: i64 = row.get(0)?;
            autos.insert(
                at,
                Auto {
                    max: row.get::<_, i64>(1)?.try_into().unwrap_or(u32::MAX),
                    agent: row.get(2)?,
                    team: row.get(3)?,
                    pane: row.get(4)?,
                    armed_ms: row.get(5)?,
                },
            );
            Ok(())
        },
    )?;

    let mut runs = Vec::new();
    each(
        connection,
        "SELECT ordinal, id, name, created_ms, summary, coordinator, handover FROM ledger_runs
          WHERE ledger_id = ?1 ORDER BY ordinal",
        ledger_id,
        |row| {
            let at: i64 = row.get(0)?;
            // A seat that will not parse is a corrupt row for the summary's
            // reason: read as `None` it would say the run has no coordinator,
            // and the next `run-use` would sit in a chair somebody holds.
            let coordinator = row
                .get::<_, Option<String>>(5)?
                .map(|held| serde_json::from_str::<CoordinatorSeat>(&held))
                .transpose()
                .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
            /* A summary that will not parse is a CORRUPT row, not a run with
             * no summary. Read as `None` it would say "this run never had its
             * rows taken", and the rows are gone either way — so the reader
             * would be handing back a run that quietly lost a month of
             * history and looks like it never had one. */
            let summary = row
                .get::<_, Option<String>>(4)?
                .map(|held| serde_json::from_str::<RunSummary>(&held))
                .transpose()
                .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
            // A policy that will not parse is a corrupt row for the seat's
            // reason: read as `None` it would say nobody armed a handover,
            // and the next wall would be news alone.
            let handover = row
                .get::<_, Option<String>>(6)?
                .map(|held| serde_json::from_str::<HandoverPolicy>(&held))
                .transpose()
                .map_err(|_| rusqlite::Error::ExecuteReturnedResults)?;
            runs.push(RunRow {
                id: row.get(1)?,
                name: Text::from(row.get::<_, String>(2)?),
                created_ms: row.get(3)?,
                auto: autos.remove(&at),
                summary,
                coordinator,
                handover,
            });
            Ok(())
        },
    )?;

    let mut tasks = Vec::new();
    let mut task_words = Vec::new();
    each(
        connection,
        "SELECT ordinal, run, id, spec, title, parent, status, result, failures, created_ms,
                result_author
           FROM ledger_tasks WHERE ledger_id = ?1 ORDER BY ordinal",
        ledger_id,
        |row| {
            let at: i64 = row.get(0)?;
            task_words.push(row.get::<_, String>(6)?);
            tasks.push((
                at,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, i64>(9)?,
                row.get::<_, Option<String>>(10)?,
            ));
            Ok(())
        },
    )?;
    let tasks = tasks
        .into_iter()
        .zip(task_words)
        .map(
            |((at, run, id, spec, title, parent, result, failures, created_ms, author), status)| {
                Ok(TaskRow {
                    run,
                    id,
                    spec: Text::from(spec),
                    title: Text::from(title),
                    deps: deps.get(&at).cloned().unwrap_or_default(),
                    parent,
                    status: from_word(&status)?,
                    result: Text::from(result),
                    failures: u32::try_from(failures).map_err(|_| EffectJournalError::Corrupt)?,
                    created_ms,
                    // A word this window does not know is refused, not
                    // guessed at — the same door the status column has.
                    result_author: author
                        .as_deref()
                        .map(serde_json::from_str::<ResultAuthor>)
                        .transpose()
                        .map_err(|_| EffectJournalError::Corrupt)?,
                })
            },
        )
        .collect::<Result<Vec<_>, EffectJournalError>>()?;

    let mut gates = Vec::new();
    let mut gate_words = Vec::new();
    each(
        connection,
        "SELECT ordinal, run, id, task, question, status, resolution, created_ms, resolved_ms,
                held_for
           FROM ledger_gates WHERE ledger_id = ?1 ORDER BY ordinal",
        ledger_id,
        |row| {
            let at: i64 = row.get(0)?;
            gate_words.push(row.get::<_, String>(5)?);
            gates.push((
                at,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<String>>(9)?,
            ));
            Ok(())
        },
    )?;
    let gates = gates
        .into_iter()
        .zip(gate_words)
        .map(
            |(
                (at, run, id, task, question, resolution, created_ms, resolved_ms, held_for),
                status,
            )| {
                Ok(GateRow {
                    run,
                    id,
                    task,
                    question: Text::from(question),
                    options: choices
                        .get(&at)
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(Text::from)
                        .collect(),
                    status: from_word(&status)?,
                    resolution: Text::from(resolution),
                    created_ms,
                    resolved_ms,
                    held_for,
                })
            },
        )
        .collect::<Result<Vec<_>, EffectJournalError>>()?;

    let mut dispatches = Vec::new();
    each(
        connection,
        "SELECT run, id, task, worker, started_ms, ended_ms, succeeded, retry_of, remote, source
           FROM ledger_dispatches WHERE ledger_id = ?1 ORDER BY ordinal",
        ledger_id,
        |row| {
            dispatches.push((
                DispatchRow {
                    run: row.get(0)?,
                    id: row.get(1)?,
                    task: row.get(2)?,
                    worker: row.get(3)?,
                    started_ms: row.get(4)?,
                    ended_ms: row.get(5)?,
                    succeeded: row.get(6)?,
                    retry_of: row.get(7)?,
                    remote: None,
                    source: row.get(9)?,
                },
                row.get::<_, Option<String>>(8)?,
            ));
            Ok(())
        },
    )?;
    let dispatches = dispatches
        .into_iter()
        .map(|(mut row, seat)| {
            row.remote = seat
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|_| EffectJournalError::Corrupt)?;
            Ok(row)
        })
        .collect::<Result<Vec<_>, EffectJournalError>>()?;

    let mut attachments = Vec::new();
    let mut attachment_words = Vec::new();
    each(
        connection,
        "SELECT run, dispatch, home, worker, state, to_home, acked_seq, imported_seq,
                created_ms
           FROM ledger_attachments WHERE ledger_id = ?1 ORDER BY ordinal",
        ledger_id,
        |row| {
            attachment_words.push((row.get::<_, String>(4)?, row.get::<_, String>(5)?));
            attachments.push((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
            ));
            Ok(())
        },
    )?;
    let attachments = attachments
        .into_iter()
        .zip(attachment_words)
        .map(
            |((run, dispatch, home, worker, acked_seq, imported_seq, created_ms), held)| {
                let (state, queue) = held;
                Ok(AttachmentRow {
                    run,
                    dispatch,
                    home,
                    worker,
                    state: from_word(&state)?,
                    to_home: serde_json::from_str(&queue)
                        .map_err(|_| EffectJournalError::Corrupt)?,
                    acked_seq,
                    imported_seq,
                    created_ms,
                })
            },
        )
        .collect::<Result<Vec<_>, EffectJournalError>>()?;

    let mut workers = Vec::new();
    let mut worker_words = Vec::new();
    each(
        connection,
        "SELECT run, id, team, agent, pane, state, started_ms, dispatch, model, effort,
                session, ready_by_ms, hook_unreachable_since_ms,
                pane_missing_since_ms, taken_over, checkout, quiet_at, archive,
                started_by, adopted_by, on_quota_wall, quota_wait
           FROM ledger_workers WHERE ledger_id = ?1 ORDER BY ordinal",
        ledger_id,
        |row| {
            worker_words.push(row.get::<_, String>(5)?);
            workers.push((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<String>>(7)?,
                (
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ),
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<i64>>(11)?,
                row.get::<_, Option<i64>>(12)?,
                row.get::<_, Option<i64>>(13)?,
                row.get::<_, bool>(14)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, Option<i64>>(16)?,
                row.get::<_, Option<String>>(17)?,
                (
                    row.get::<_, Option<String>>(18)?,
                    row.get::<_, Option<i64>>(19)?,
                    row.get::<_, Option<String>>(20)?,
                    row.get::<_, bool>(21)?,
                ),
            ));
            Ok(())
        },
    )?;
    let workers = workers
        .into_iter()
        .zip(worker_words)
        .map(
            |(
                (
                    run,
                    id,
                    team,
                    agent,
                    pane,
                    started_ms,
                    dispatch,
                    tuned,
                    session,
                    ready_by_ms,
                    hook_unreachable_since_ms,
                    pane_missing_since_ms,
                    taken_over,
                    checkout,
                    quiet_at,
                    archive,
                    lineage,
                ),
                state,
            )| {
                let (started_by, adopted_by, on_quota_wall, quota_wait) = lineage;
                /* A generation is a small positive count; a value SQLite hands
                 * back that a `u32` cannot hold was not written by this store. */
                let adopted_by = adopted_by
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|_| EffectJournalError::Corrupt)?;
                // An alternative that will not parse is a corrupt row, for
                // the session's reason.
                let on_quota_wall = on_quota_wall
                    .as_deref()
                    .map(serde_json::from_str::<Pinned>)
                    .transpose()
                    .map_err(|_| EffectJournalError::Corrupt)?;
                Ok(WorkerRow {
                    run,
                    id,
                    team,
                    agent,
                    pane,
                    state: from_word(&state)?,
                    started_ms,
                    dispatch,
                    model: tuned.0,
                    effort: tuned.1,
                    session: session
                        .as_deref()
                        .map(serde_json::from_str)
                        .transpose()
                        .map_err(|_| EffectJournalError::Corrupt)?,
                    ready_by_ms,
                    hook_unreachable_since_ms,
                    pane_missing_since_ms,
                    taken_over,
                    checkout,
                    quiet_at,
                    archive: archive.map(Text::from),
                    started_by,
                    adopted_by,
                    on_quota_wall,
                    quota_wait,
                })
            },
        )
        .collect::<Result<Vec<_>, EffectJournalError>>()?;

    let mut messages = Vec::new();
    let mut message_words = Vec::new();
    each(
        connection,
        "SELECT run, id, sender, recipient, kind, body, subject, priority, payload,
                thread, task, dispatch, author_seat, created_ms
           FROM ledger_messages WHERE ledger_id = ?1 ORDER BY ordinal",
        ledger_id,
        |row| {
            message_words.push((row.get::<_, String>(4)?, row.get::<_, String>(7)?));
            messages.push((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(5)?,
                (row.get::<_, String>(6)?, row.get::<_, String>(8)?),
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<String>>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, i64>(13)?,
            ));
            Ok(())
        },
    )?;
    let messages = messages
        .into_iter()
        .zip(message_words)
        .map(
            |(
                (
                    run,
                    id,
                    from,
                    to,
                    body,
                    (subject, payload),
                    thread,
                    task,
                    dispatch,
                    author_seat,
                    created_ms,
                ),
                (kind, priority),
            )| {
                Ok(MessageRow {
                    run,
                    id,
                    from,
                    to,
                    kind: from_word(&kind)?,
                    body: Text::from(body),
                    subject: Text::from(subject),
                    priority: from_word(&priority)?,
                    payload: Text::from(payload),
                    thread,
                    task,
                    dispatch,
                    author_seat,
                    created_ms,
                })
            },
        )
        .collect::<Result<Vec<_>, EffectJournalError>>()?;

    let mut inboxes = Vec::new();
    each(
        connection,
        "SELECT ordinal, run, address, open_delivery, open_holder, open_opened_ms
           FROM ledger_inboxes WHERE ledger_id = ?1 ORDER BY ordinal",
        ledger_id,
        |row| {
            let at: i64 = row.get(0)?;
            let delivery: Option<String> = row.get(3)?;
            let holder: Option<String> = row.get(4)?;
            let opened_ms: Option<i64> = row.get(5)?;
            inboxes.push(InboxRow {
                run: row.get(1)?,
                address: row.get(2)?,
                pending: pending.get(&at).cloned().unwrap_or_default(),
                open: delivery.map(|id| Delivery {
                    id,
                    messages: leased.get(&at).cloned().unwrap_or_default(),
                    holder,
                    opened_ms,
                }),
            });
            Ok(())
        },
    )?;

    let mut bound = Vec::new();
    each(
        connection,
        "SELECT caller, run FROM ledger_bound WHERE ledger_id = ?1 ORDER BY ordinal",
        ledger_id,
        |row| {
            bound.push(BoundRow {
                caller: Text::from(row.get::<_, String>(0)?),
                run: row.get(1)?,
            });
            Ok(())
        },
    )?;

    let mut served_raw = Vec::new();
    each(
        connection,
        "SELECT ordinal, caller, request, fingerprint, renderer, inline,
                check_run, check_address, check_delivery, filed_ms, expired
           FROM ledger_served WHERE ledger_id = ?1 ORDER BY ordinal",
        ledger_id,
        |row| {
            served_raw.push((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, bool>(10)?,
            ));
            Ok(())
        },
    )?;
    let mut served = Vec::with_capacity(served_raw.len());
    for (
        at,
        caller,
        request,
        fingerprint,
        renderer,
        inline,
        run,
        address,
        delivery,
        filed_ms,
        expired,
    ) in served_raw
    {
        let answer = match (renderer.as_deref(), inline, run, address) {
            (None, Some(held), None, None) => ServedAnswer::Inline(held),
            (Some(CHECK_RENDERER), None, Some(run), Some(address)) => {
                ServedAnswer::Check(CheckV1 {
                    run,
                    address,
                    delivery,
                    messages: answered.get(&at).cloned().unwrap_or_default(),
                })
            }
            _ => return Err(EffectJournalError::Corrupt),
        };
        served.push(ServedRow {
            caller: caller.map(Text::from),
            request: Text::from(request),
            answer,
            fingerprint: fingerprint.map(Text::from),
            filed_ms,
            expired,
        });
    }

    let mut acked = Vec::new();
    each(
        connection,
        "SELECT ordinal, run, address, delivery, holds_messages, seq, current
           FROM ledger_acked WHERE ledger_id = ?1 ORDER BY ordinal",
        ledger_id,
        |row| {
            let at: i64 = row.get(0)?;
            let holds: bool = row.get(4)?;
            acked.push((
                at,
                holds,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(5)?,
                row.get::<_, bool>(6)?,
            ));
            Ok(())
        },
    )?;
    let acked = acked
        .into_iter()
        .map(|(at, holds, run, address, delivery, seq, current)| {
            Ok(AckedRow {
                run,
                address,
                delivery,
                messages: holds.then(|| spent.get(&at).cloned().unwrap_or_default()),
                seq: u32::try_from(seq).map_err(|_| EffectJournalError::Corrupt)?,
                current,
            })
        })
        .collect::<Result<Vec<_>, EffectJournalError>>()?;

    let projection = LedgerProjectionV1 {
        schema,
        next_id: from_sql_u64(next_id)?,
        runs,
        tasks,
        dispatches,
        workers,
        messages,
        inboxes,
        bound,
        served,
        acked,
        gates,
        attachments,
        retention_days: u32::try_from(retention_days).map_err(|_| EffectJournalError::Corrupt)?,
        swept_at_ms,
    };
    let bytes_match = from_sql_u64(bytes)? == bytes_held(&projection);
    Ok(Held {
        revision: from_sql_u64(revision)?,
        projection,
        updated_at_ms,
        bytes_match,
    })
}

/// Walk one statement's rows, handing each to the caller.
///
/// The same three lines were about to be written eleven times.
fn each(
    connection: &Connection,
    sql: &str,
    ledger_id: &str,
    mut take: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<()>,
) -> Result<(), EffectJournalError> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|_| EffectJournalError::Database)?;
    let mut rows = statement
        .query([ledger_id])
        .map_err(|_| EffectJournalError::Database)?;
    while let Some(row) = rows.next().map_err(|_| EffectJournalError::Database)? {
        take(row).map_err(|_| EffectJournalError::Corrupt)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store with the v5 tables and nothing in them.
    fn a_store() -> Connection {
        let connection = Connection::open_in_memory().expect("a database");
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .expect("foreign keys");
        connection
            .execute_batch(LEDGER_TABLES_SQL)
            .expect("the v5 tables");
        connection
    }

    /// A current on-disk store carrying every child-table shape in the
    /// load-bearing fixture.
    fn a_populated_current_store() -> (
        tempfile::TempDir,
        std::path::PathBuf,
        crate::workflow_store::WorkflowStore,
    ) {
        let root = tempfile::tempdir().expect("store root");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700))
                .expect("private root");
        }
        let path = root.path().join("workflow.sqlite");
        let store = crate::workflow_store::WorkflowStore::open(&path).expect("current store");
        let projection = a_ledger_where_order_is_load_bearing();
        write(
            &store.connection().expect("ledger connection"),
            "main-ledger",
            0,
            1,
            &projection,
            10,
        )
        .expect("populated ledger");
        (root, path, store)
    }

    /// A ledger whose every list is longer than one and in an order that is
    /// NOT the order a database would hand back on its own.
    ///
    /// Reverse-alphabetical ids throughout, because a store that lost the
    /// order would most likely lose it INTO sorted order — the one wrong
    /// answer that looks tidy.
    fn a_ledger_where_order_is_load_bearing() -> LedgerProjectionV1 {
        LedgerProjectionV1 {
            schema: zerocode_core::orchestration::PROJECTION_SCHEMA,
            next_id: 40,
            retention_days: 7,
            swept_at_ms: 6,
            runs: vec![
                RunRow {
                    id: "run-2".to_string(),
                    name: Text::from("second".to_string()),
                    created_ms: 2,
                    auto: Some(Auto {
                        max: 3,
                        agent: "codex".to_string(),
                        team: "blue".to_string(),
                        pane: "%9".to_string(),
                        armed_ms: 5,
                    }),
                    summary: None,
                    /* `None` here for `started_by`'s reason (below): the
                     * canonical fixture pins that a seat column arriving
                     * additively moves no digest byte. The populated seat is
                     * exercised on its own in
                     * `a_coordinator_seat_and_an_adoption_survive_the_round_trip`. */
                    coordinator: None,
                    handover: None,
                },
                RunRow {
                    id: "run-1".to_string(),
                    name: Text::from("first".to_string()),
                    created_ms: 1,
                    auto: None,
                    /* A run whose rows a sweep took. It rides the fixture
                     * rather than a test of its own so that the ORDER rules
                     * this file exists for cover it too: a summary is a column
                     * like any other, and a column that only one test touches
                     * is a column the round trip does not really check. */
                    summary: Some(RunSummary {
                        tasks: 3,
                        completed: 2,
                        failed: 1,
                        dispatches: 4,
                        workers: 2,
                        messages: 9,
                        gates: 1,
                        headlines: vec![
                            Text::from("drain the gate".to_string()),
                            Text::from("pack the crate".to_string()),
                        ],
                        last_activity_ms: 11,
                        compacted_ms: 12,
                        sweeps: 2,
                    }),
                    coordinator: None,
                    handover: None,
                },
            ],
            tasks: vec![TaskRow {
                run: "run-2".to_string(),
                id: "t-9".to_string(),
                spec: Text::from("ship".to_string()),
                title: Text::from("the ship".to_string()),
                deps: vec!["t-3".to_string(), "t-1".to_string(), "t-2".to_string()],
                parent: None,
                status: serde_json::from_str("\"ready\"").expect("a status"),
                result: Text::from(String::new()),
                failures: 2,
                created_ms: 7,
                result_author: None,
            }],
            dispatches: vec![DispatchRow {
                run: "run-2".to_string(),
                id: "dp-9".to_string(),
                task: "t-9".to_string(),
                worker: "w-9".to_string(),
                started_ms: 8,
                ended_ms: Some(9),
                succeeded: Some(false),
                // The grown link, loaded: a round trip that only carried the
                // default would pin nothing about the retry lineage.
                retry_of: Some("dp-3".to_string()),
                source: None,
                // And the federated seat whole — queue bytes included, so a
                // store that dropped the outbox would fail the byte check.
                remote: Some(zerocode_core::orchestration::RemoteSeat {
                    server: "rack-1".to_string(),
                    absorbed_seq: 2,
                    outbox: vec![zerocode_core::orchestration::RelayItem {
                        seq: 3,
                        kind: serde_json::from_str("\"status\"").expect("a kind"),
                        body: Text::from("keep-going".to_string()),
                        payload: Text::from(String::new()),
                        message: "m-out-3".to_string(),
                    }],
                    exported_seq: 2,
                }),
            }],
            workers: vec![WorkerRow {
                run: "run-2".to_string(),
                id: "w-9".to_string(),
                team: "blue".to_string(),
                agent: "codex".to_string(),
                pane: "%9".to_string(),
                state: serde_json::from_str("\"active\"").expect("a state"),
                started_ms: 8,
                dispatch: Some("dp-9".to_string()),
                // And the grown launch receipt, loaded for the same reason.
                model: Some("m-big".to_string()),
                effort: Some("high".to_string()),
                session: Some(zerocode_core::ProviderSession {
                    key: zerocode_core::provider_session::SessionKey::SessionId,
                    id: "session-grown".to_string(),
                    transcript_path: Some("/tmp/session-grown.jsonl".to_string()),
                }),
                ready_by_ms: Some(61_000),
                hook_unreachable_since_ms: None,
                pane_missing_since_ms: None,
                taken_over: true,
                checkout: Some("/wt/grown".to_string()),
                quiet_at: None,
                archive: None,
                // Left empty ON PURPOSE. This fixture is what the table-digest
                // pin fingerprints, and an optional field that is skipped when
                // absent must not move those bytes — that is the whole promise
                // of growing a column additively. Naming a summoner here would
                // have forced a format-generation bump for a change that does
                // not need one, and hidden the very fact worth proving. The
                // populated path is exercised on its own below.
                started_by: None,
                adopted_by: None,
                on_quota_wall: None,
                quota_wait: false,
            }],
            attachments: vec![AttachmentRow {
                run: "run-2".to_string(),
                dispatch: "dp-far-1".to_string(),
                home: "home-fp-abcd".to_string(),
                worker: "w-9".to_string(),
                state: serde_json::from_str("\"ready\"").expect("a state"),
                to_home: vec![zerocode_core::orchestration::RelayItem {
                    seq: 5,
                    kind: serde_json::from_str("\"status\"").expect("a kind"),
                    body: Text::from("halfway".to_string()),
                    payload: Text::from(String::new()),
                    message: "m-2".to_string(),
                }],
                acked_seq: 4,
                imported_seq: 7,
                created_ms: 6,
            }],
            messages: vec![
                MessageRow {
                    run: "run-2".to_string(),
                    id: "m-2".to_string(),
                    from: "%9".to_string(),
                    to: "%1".to_string(),
                    kind: serde_json::from_str("\"status\"").expect("a kind"),
                    body: Text::from("second".to_string()),
                    // The grown fields, loaded: a round trip that only ever
                    // carried the defaults would pin nothing about them.
                    subject: Text::from("re: everything".to_string()),
                    priority: serde_json::from_str("\"urgent\"").expect("a priority"),
                    payload: Text::from("{\"freight\":true}".to_string()),
                    thread: None,
                    task: None,
                    dispatch: None,
                    author_seat: None,
                    created_ms: 9,
                },
                MessageRow {
                    run: "run-2".to_string(),
                    id: "m-1".to_string(),
                    from: "%9".to_string(),
                    to: "%1".to_string(),
                    kind: serde_json::from_str("\"status\"").expect("a kind"),
                    body: Text::from("first".to_string()),
                    subject: Text::default(),
                    priority: serde_json::from_str("\"normal\"").expect("a priority"),
                    payload: Text::default(),
                    thread: None,
                    task: None,
                    dispatch: None,
                    author_seat: None,
                    created_ms: 8,
                },
            ],
            inboxes: vec![InboxRow {
                run: "run-2".to_string(),
                address: "%1".to_string(),
                pending: vec!["m-2".to_string(), "m-1".to_string()],
                open: Some(Delivery {
                    id: "d-1".to_string(),
                    messages: vec!["m-2".to_string(), "m-1".to_string()],
                    /* A lease from before a holder was recorded, and it stays
                     * that way HERE on purpose: this fixture pins the digest
                     * bytes, both fields skip when absent, and a lease that
                     * carried them would move the fingerprint that says the
                     * format did not change. They are written and read back in
                     * `a_leases_holder_and_hour_survive_the_sqlite_round_trip`
                     * instead, the same split `started_by` already uses. */
                    holder: None,
                    opened_ms: None,
                }),
            }],
            bound: vec![BoundRow {
                caller: Text::from("%1".to_string()),
                run: "run-2".to_string(),
            }],
            served: vec![
                ServedRow {
                    caller: Some(Text::from("somebody".to_string())),
                    request: Text::from("r-2".to_string()),
                    answer: ServedAnswer::Check(CheckV1 {
                        run: "run-2".to_string(),
                        address: "%1".to_string(),
                        delivery: Some("d-1".to_string()),
                        messages: vec!["m-2".to_string(), "m-1".to_string()],
                    }),
                    fingerprint: Some(Text::from("f".repeat(64))),
                    filed_ms: Some(7),
                    expired: false,
                },
                ServedRow {
                    caller: None,
                    request: Text::from("r-1".to_string()),
                    answer: ServedAnswer::Inline("what it printed\n".to_string()),
                    fingerprint: None,
                    filed_ms: None,
                    expired: false,
                },
                // A tombstone: the key survives the round trip, the answer is
                // already gone, and a store that lost the flag would hand back
                // a receipt that replays as an empty answer instead of one
                // that refuses.
                ServedRow {
                    caller: Some(Text::from("somebody".to_string())),
                    request: Text::from("r-0".to_string()),
                    answer: ServedAnswer::Inline(String::new()),
                    fingerprint: Some(Text::from("e".repeat(64))),
                    filed_ms: Some(3),
                    expired: true,
                },
            ],
            acked: vec![
                AckedRow {
                    run: "run-2".to_string(),
                    address: "%1".to_string(),
                    delivery: "d-0".to_string(),
                    messages: Some(vec!["m-2".to_string(), "m-1".to_string()]),
                    seq: 1,
                    current: true,
                },
                AckedRow {
                    run: "run-2".to_string(),
                    address: "%1".to_string(),
                    delivery: "d-old".to_string(),
                    messages: None,
                    seq: 0,
                    current: false,
                },
            ],
            gates: vec![
                // Options out of alphabetical order, so a store that sorted
                // them would be caught keeping a different list.
                GateRow {
                    run: "run-2".to_string(),
                    id: "gate-2".to_string(),
                    task: "t-9".to_string(),
                    question: Text::from("which way?".to_string()),
                    options: vec![
                        Text::from("rpc".to_string()),
                        Text::from("rest".to_string()),
                    ],
                    status: serde_json::from_str("\"pending\"").expect("a status"),
                    resolution: Text::from(String::new()),
                    created_ms: 11,
                    resolved_ms: None,
                    held_for: None,
                },
                GateRow {
                    run: "run-2".to_string(),
                    id: "gate-1".to_string(),
                    task: "t-9".to_string(),
                    question: Text::from("sure?".to_string()),
                    options: Vec::new(),
                    status: serde_json::from_str("\"resolved\"").expect("a status"),
                    resolution: Text::from("sure".to_string()),
                    created_ms: 10,
                    resolved_ms: Some(12),
                    held_for: None,
                },
            ],
        }
    }

    /// What went in is what comes back — every list, in its own order.
    #[test]
    fn a_ledger_written_to_the_tables_comes_back_the_ledger_it_was() {
        let store = a_store();
        let projection = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");

        let held = read(&store, "one", projection.schema)
            .expect("it reads")
            .expect("it is there");
        assert_eq!(held.revision, 1);
        assert_eq!(held.projection, projection);
    }

    /// Who wrote a task's result survives the trip through SQLite, and an
    /// old row still reads back as one nobody is known to have written —
    /// which `Run::review_of` reads as a claim, never a fact (t-6815).
    ///
    /// Kept apart from the canonical fixture for the reason the summoner's
    /// test below is: the pin must stay byte-identical across this column's
    /// arrival.
    #[test]
    fn a_result_author_written_to_the_table_comes_back_and_an_older_row_stays_unknown() {
        let store = a_store();
        let mut projection = a_ledger_where_order_is_load_bearing();
        projection.dispatches[0].source = Some("abc1234".to_string());
        let mut reviewed = projection.tasks[0].clone();
        reviewed.id = "t-10".to_string();
        reviewed.result_author = Some(ResultAuthor::Coordinator {
            seat: "team-2/%1".to_string(),
            generation: Some(3),
            attempt: Some("dp-9".to_string()),
            source: Some("abc1234".to_string()),
        });
        let mut reported = projection.tasks[0].clone();
        reported.id = "t-11".to_string();
        reported.result_author = Some(ResultAuthor::Worker {
            worker: "w-9".to_string(),
            dispatch: Some("dp-9".to_string()),
        });
        projection.tasks.push(reviewed.clone());
        projection.tasks.push(reported.clone());
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");

        let held = read(&store, "one", projection.schema)
            .expect("it reads")
            .expect("it is there");
        assert_eq!(
            held.projection.dispatches[0].source.as_deref(),
            Some("abc1234")
        );
        let authors: Vec<Option<&ResultAuthor>> = held
            .projection
            .tasks
            .iter()
            .map(|task| task.result_author.as_ref())
            .collect();
        assert_eq!(
            authors,
            vec![
                None,
                reviewed.result_author.as_ref(),
                reported.result_author.as_ref()
            ],
            "the row written before authorship was recorded stays unknown, and \
             the coordinator's and the worker's rows come back as written"
        );

        // A word this window does not know is refused, not guessed at — the
        // same door the status column has.
        store
            .execute(
                "UPDATE ledger_tasks SET result_author = '{\"kind\":\"nobody\"}' WHERE id = 't-10'",
                [],
            )
            .expect("the tamper");
        let refused = read(&store, "one", projection.schema).expect_err("must refuse");
        assert!(
            matches!(refused, EffectJournalError::Corrupt),
            "{refused:?}"
        );
    }

    /// Lineage survives the trip through SQLite, and an old row still reads
    /// back as one nobody recorded a summoner for.
    ///
    /// Kept apart from the canonical fixture on purpose: that one pins the
    /// digest and must stay byte-identical across this column's arrival, so
    /// the column needs somewhere else to actually be written and read.
    #[test]
    fn a_summoner_written_to_the_table_comes_back_and_an_older_row_stays_empty() {
        let store = a_store();
        let mut projection = a_ledger_where_order_is_load_bearing();
        let mut child = projection.workers[0].clone();
        child.id = "w-10".to_string();
        child.started_by = Some(projection.workers[0].id.clone());
        projection.workers.push(child);
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");

        let held = read(&store, "one", projection.schema)
            .expect("it reads")
            .expect("it is there");
        let summoners: Vec<Option<&str>> = held
            .projection
            .workers
            .iter()
            .map(|worker| worker.started_by.as_deref())
            .collect();
        assert_eq!(
            summoners,
            vec![None, Some("w-9")],
            "the row written before summoners were recorded stays empty, and \
             the one that names its parent keeps it"
        );
    }

    /// Who holds an open batch, and since when, survives the trip through
    /// SQLite — and a lease written before either was recorded still reads
    /// back as one nobody wrote a holder down for.
    ///
    /// That `None` is load-bearing rather than cosmetic: `Ledger::deliver`
    /// reads it as "nobody wrote it down" and lets the next caller ADOPT the
    /// batch, where a guessed holder would make that caller a stranger and
    /// hand its own lease away on the next look.
    ///
    /// Kept apart from the canonical fixture for the reason `started_by` is:
    /// that one pins the digest bytes and must stay byte-identical across
    /// these columns' arrival, so they need somewhere else to be written and
    /// read.
    #[test]
    fn a_leases_holder_and_hour_survive_the_sqlite_round_trip() {
        let store = a_store();
        let mut projection = a_ledger_where_order_is_load_bearing();
        let mut owned = projection.inboxes[0].clone();
        owned.address = "run:run-2".to_string();
        owned.pending = Vec::new();
        let open = owned.open.as_mut().expect("the fixture leases a batch");
        open.id = "d-2".to_string();
        open.messages = vec!["m-1".to_string()];
        open.holder = Some("team-1/%1".to_string());
        open.opened_ms = Some(1_787_913_312_410);
        projection.inboxes.push(owned);
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");

        let held = read(&store, "one", projection.schema)
            .expect("it reads")
            .expect("it is there");
        let leases: Vec<(Option<&str>, Option<i64>)> = held
            .projection
            .inboxes
            .iter()
            .filter_map(|row| row.open.as_ref())
            .map(|open| (open.holder.as_deref(), open.opened_ms))
            .collect();
        assert_eq!(
            leases,
            vec![
                (None, None),
                (Some("team-1/%1"), Some(1_787_913_312_410_i64)),
            ],
            "a lease from before holders were recorded must stay empty, and \
             the one that names its seat must keep both halves"
        );
    }

    #[test]
    fn hook_delivery_failure_time_survives_the_sqlite_round_trip() {
        let store = a_store();
        let mut projection = a_ledger_where_order_is_load_bearing();
        projection.workers[0].hook_unreachable_since_ms = Some(60_500);
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");

        let held = read(&store, "one", projection.schema)
            .expect("it reads")
            .expect("it is there");
        assert_eq!(
            held.projection.workers[0].hook_unreachable_since_ms,
            Some(60_500)
        );
    }

    #[test]
    fn pane_missing_time_survives_the_sqlite_round_trip() {
        let store = a_store();
        let mut projection = a_ledger_where_order_is_load_bearing();
        projection.workers[0].pane_missing_since_ms = Some(70_500);
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");

        let held = read(&store, "one", projection.schema)
            .expect("it reads")
            .expect("it is there");
        assert_eq!(
            held.projection.workers[0].pane_missing_since_ms,
            Some(70_500)
        );
    }

    /// The worker a hold is for crosses the disk with the gate, and a gate a
    /// person opened keeps its `None`.
    #[test]
    fn a_hold_survives_the_sqlite_round_trip() {
        let store = a_store();
        let mut projection = a_ledger_where_order_is_load_bearing();
        projection.gates[0].held_for = Some("w-9".to_string());
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");

        let held = read(&store, "one", projection.schema)
            .expect("it reads")
            .expect("it is there");
        assert_eq!(held.projection.gates[0].held_for.as_deref(), Some("w-9"));
        assert_eq!(held.projection.gates[1].held_for, None);
    }

    /// This fingerprint is the persisted table-digest format contract. A
    /// deliberate serializer or manual-row encoding change must bump the
    /// generation and refresh both halves of this expectation together.
    ///
    ///
    /// Unchanged by t-2512 on purpose: the coordinator seat and the adoption
    /// generation arrived as `skip_serializing_if` columns, so a store written
    /// before them digests byte-for-byte as it did, and this pin is the proof.
    #[test]
    fn canonical_populated_fixture_pins_table_digest_format_generation() {
        let projection = a_ledger_where_order_is_load_bearing();
        let digests = table_digests(&projection).expect("canonical table digests");

        assert_eq!(
            (TABLE_DIGEST_FORMAT_GENERATION, digest_rows(&digests)),
            (
                1,
                [
                    254, 174, 178, 207, 0, 10, 223, 93, 246, 117, 52, 102, 200, 237, 234, 184, 161,
                    174, 107, 20, 180, 226, 124, 125, 128, 205, 200, 27, 118, 189, 38, 23,
                ],
            ),
            "table-digest bytes changed without an explicit format decision"
        );
    }

    /// A held seat and an adoption generation cross the disk whole, and a
    /// store aged to before either column existed reads the same rows back
    /// with neither (the migration half rides `current_schema_column_growth_
    /// keeps_a_populated_ledger_readable`, which drops the seat column too).
    #[test]
    fn a_coordinator_seat_and_an_adoption_survive_the_round_trip() {
        let store = a_store();
        let mut projection = a_ledger_where_order_is_load_bearing();
        projection.runs[0].coordinator = Some(CoordinatorSeat {
            seat: "blue/%1".to_string(),
            actor: Some("actor-of-the-blue-leader".to_string()),
            generation: 2,
            since_ms: 4,
            vacated_ms: None,
            handover: None,
        });
        projection.runs[1].coordinator = Some(CoordinatorSeat {
            seat: "red/%1".to_string(),
            actor: None,
            generation: 5,
            since_ms: 6,
            vacated_ms: Some(7),
            handover: None,
        });
        projection.workers[0].adopted_by = Some(2);
        // The handover standing order and a summons' own alternative ride
        // the same trip (t-3059): one JSON document each, like the seat.
        projection.runs[1].handover = Some(HandoverPolicy {
            on_quota_wall: Some(Pinned {
                agent: "claude".to_string(),
                model: Some("fable-5-1".to_string()),
                effort: Some("high".to_string()),
            }),
            wip_commit: true,
            on_transient_error: None,
            quota_wait: true,
            on_classifier_decline: Some(Pinned {
                agent: "claude".to_string(),
                model: Some("claude-opus-4-8".to_string()),
                effort: None,
            }),
            armed_ms: 8,
        });
        projection.workers[0].on_quota_wall = Some(Pinned {
            agent: "codex".to_string(),
            model: None,
            effort: None,
        });
        // And its own `wait` beside it (t-6427).
        projection.workers[0].quota_wait = true;
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");

        let held = read(&store, "one", projection.schema)
            .expect("it reads")
            .expect("it is there");
        assert_eq!(held.projection, projection);
    }

    /// The handover policy's and the alternative's names count toward the
    /// head's byte total, for the seat's reason: a string the accounting
    /// skips is one a direct SQLite change could grow unwatched.
    #[test]
    fn handover_bytes_are_part_of_the_head_integrity_count() {
        let mut projection = a_ledger_where_order_is_load_bearing();
        let without = bytes_held(&projection);
        projection.runs[0].handover = Some(HandoverPolicy {
            on_quota_wall: Some(Pinned {
                agent: "claude".to_string(),
                model: Some("fable-5-1".to_string()),
                effort: None,
            }),
            wip_commit: false,
            on_transient_error: None,
            quota_wait: false,
            on_classifier_decline: Some(Pinned {
                agent: "zo".to_string(),
                model: None,
                effort: None,
            }),
            armed_ms: 1,
        });
        projection.workers[0].on_quota_wall = Some(Pinned {
            agent: "codex".to_string(),
            model: Some("gpt-5".to_string()),
            effort: Some("high".to_string()),
        });
        assert_eq!(
            bytes_held(&projection) - without,
            ("claude".len()
                + "fable-5-1".len()
                + "zo".len()
                + "codex".len()
                + "gpt-5".len()
                + "high".len()) as u64,
            "handover text is invisible to bytes_held"
        );
    }

    #[test]
    fn unversioned_digest_rows_and_head_are_adopted_as_generation_one() {
        let (_root, path, store) = a_populated_current_store();
        store
            .connection()
            .expect("age the digest metadata")
            .execute_batch(
                "ALTER TABLE ledger_table_digests DROP COLUMN format_generation;
                 ALTER TABLE orchestration_ledger_heads DROP COLUMN table_digest_root;
                 ALTER TABLE orchestration_ledger_heads
                    DROP COLUMN table_digest_format_generation;",
            )
            .expect("the unversioned digest shape");
        drop(store);

        let reopened = crate::workflow_store::WorkflowStore::open(&path)
            .expect("generation-one metadata remains evidence");
        let adopted: (i64, i64, i64) = reopened
            .connection()
            .expect("inspect adopted metadata")
            .query_row(
                "SELECT COUNT(*), MIN(format_generation), MAX(format_generation)
                   FROM ledger_table_digests WHERE ledger_id = 'main-ledger'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("adopted digest generations");
        let adopted_root: (i64, Vec<u8>) = reopened
            .connection()
            .expect("inspect adopted root")
            .query_row(
                "SELECT table_digest_format_generation, table_digest_root
                   FROM orchestration_ledger_heads WHERE ledger_id = 'main-ledger'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("adopted aggregate root");
        let projection = a_ledger_where_order_is_load_bearing();
        let expected_root = digest_rows(&table_digests(&projection).expect("expected digests"));

        assert_eq!(
            (adopted, adopted_root),
            (
                (
                    i64::try_from(LedgerTable::ALL.len()).expect("table count fits SQLite"),
                    i64::from(TABLE_DIGEST_FORMAT_GENERATION_V1),
                    i64::from(TABLE_DIGEST_FORMAT_GENERATION_V1),
                ),
                (
                    i64::from(TABLE_DIGEST_FORMAT_GENERATION),
                    expected_root.to_vec(),
                ),
            )
        );
    }

    #[test]
    fn unversioned_digest_evidence_still_refuses_a_dropped_child() {
        let (_root, path, store) = a_populated_current_store();
        store
            .connection()
            .expect("damage the unversioned store")
            .execute_batch(
                "ALTER TABLE ledger_table_digests DROP COLUMN format_generation;
                 ALTER TABLE orchestration_ledger_heads DROP COLUMN table_digest_root;
                 ALTER TABLE orchestration_ledger_heads
                    DROP COLUMN table_digest_format_generation;
                 DROP TABLE ledger_task_deps;",
            )
            .expect("remove a populated child after the old writer's digest");
        drop(store);

        let reopened = crate::workflow_store::WorkflowStore::open(&path);
        assert!(matches!(
            reopened,
            Err(crate::workflow_store::WorkflowStoreError::Corrupt)
        ));
    }

    #[test]
    fn same_generation_digest_refuses_semantic_row_tamper_on_reopen() {
        let (_root, path, store) = a_populated_current_store();
        store
            .connection()
            .expect("tamper connection")
            .execute(
                "UPDATE ledger_task_deps SET dep = 'tampered-without-moving-the-head'
                  WHERE ledger_id = 'main-ledger'",
                [],
            )
            .expect("same-generation semantic tamper");
        drop(store);

        let reopened = crate::workflow_store::WorkflowStore::open(&path);
        assert!(matches!(
            reopened,
            Err(crate::workflow_store::WorkflowStoreError::Corrupt)
        ));
    }

    #[test]
    fn older_format_metadata_is_invalidated_and_reseeded_per_ledger() {
        let (_root, path, store) = a_populated_current_store();
        let first = a_ledger_where_order_is_load_bearing();
        let mut second = first.clone();
        second.next_id += 1;
        let connection = store.connection().expect("two-ledger connection");
        write(&connection, "second-ledger", 0, 1, &second, 10).expect("write the second ledger");
        let second_metadata_before: (i64, Vec<u8>, Vec<u8>) = connection
            .query_row(
                "SELECT heads.table_digest_format_generation,
                        heads.table_digest_root, digests.digest
                   FROM orchestration_ledger_heads AS heads
                   JOIN ledger_table_digests AS digests
                     ON digests.ledger_id = heads.ledger_id
                  WHERE heads.ledger_id = 'second-ledger' AND digests.table_name = ?1",
                [LedgerTable::TaskDeps.name()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("second ledger metadata");
        let older_generation = TABLE_DIGEST_FORMAT_GENERATION
            .checked_sub(1)
            .expect("generation one has an older sentinel");
        connection
            .execute(
                "UPDATE ledger_table_digests SET format_generation = ?1
                  WHERE ledger_id = 'main-ledger'",
                [i64::from(older_generation)],
            )
            .expect("age one ledger's table metadata");
        connection
            .execute(
                "UPDATE orchestration_ledger_heads
                    SET table_digest_format_generation = ?1
                  WHERE ledger_id = 'main-ledger'",
                [i64::from(older_generation)],
            )
            .expect("age one ledger's aggregate metadata");
        drop(connection);
        drop(store);

        let reopened = crate::workflow_store::WorkflowStore::open(&path)
            .expect("stale metadata is not current-format corruption");
        let connection = reopened.connection().expect("reopened connection");
        let stale_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM ledger_table_digests
                  WHERE ledger_id = 'main-ledger'",
                [],
                |row| row.get(0),
            )
            .expect("stale rows were invalidated");
        let stale_root: (Option<i64>, Option<Vec<u8>>) = connection
            .query_row(
                "SELECT table_digest_format_generation, table_digest_root
                   FROM orchestration_ledger_heads WHERE ledger_id = 'main-ledger'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("stale root was invalidated");
        let second_metadata_after: (i64, Vec<u8>, Vec<u8>) = connection
            .query_row(
                "SELECT heads.table_digest_format_generation,
                        heads.table_digest_root, digests.digest
                   FROM orchestration_ledger_heads AS heads
                   JOIN ledger_table_digests AS digests
                     ON digests.ledger_id = heads.ledger_id
                  WHERE heads.ledger_id = 'second-ledger' AND digests.table_name = ?1",
                [LedgerTable::TaskDeps.name()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("other ledger metadata remains");
        write(&connection, "main-ledger", 1, 2, &first, 11)
            .expect("the next write reseeds stale metadata");
        let reseeded: (i64, i64, i64, i64, Vec<u8>) = connection
            .query_row(
                "SELECT COUNT(*), MIN(digests.format_generation),
                        MAX(digests.format_generation),
                        heads.table_digest_format_generation, heads.table_digest_root
                   FROM orchestration_ledger_heads AS heads
                   JOIN ledger_table_digests AS digests
                     ON digests.ledger_id = heads.ledger_id
                  WHERE heads.ledger_id = 'main-ledger'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("reseeded metadata");
        let expected_root =
            digest_rows(&table_digests(&first).expect("expected current digests")).to_vec();

        assert_eq!(
            (stale_count, stale_root, second_metadata_after, reseeded,),
            (
                0,
                (None, None),
                second_metadata_before,
                (
                    i64::try_from(LedgerTable::ALL.len()).expect("table count fits SQLite"),
                    i64::from(TABLE_DIGEST_FORMAT_GENERATION),
                    i64::from(TABLE_DIGEST_FORMAT_GENERATION),
                    i64::from(TABLE_DIGEST_FORMAT_GENERATION),
                    expected_root,
                ),
            )
        );
    }

    #[test]
    fn stale_generation_does_not_hide_a_same_generation_mismatch() {
        let (_root, path, store) = a_populated_current_store();
        let older_generation = TABLE_DIGEST_FORMAT_GENERATION
            .checked_sub(1)
            .expect("generation one has an older sentinel");
        store
            .connection()
            .expect("mixed-generation damage connection")
            .execute(
                "UPDATE orchestration_ledger_heads
                    SET table_digest_format_generation = ?1
                  WHERE ledger_id = 'main-ledger'",
                [i64::from(older_generation)],
            )
            .expect("age the aggregate root");
        let connection = store.connection().expect("table damage connection");
        connection
            .execute(
                "UPDATE ledger_table_digests SET format_generation = ?1
                  WHERE ledger_id = 'main-ledger' AND table_name = ?2",
                params![i64::from(older_generation), LedgerTable::TaskDeps.name()],
            )
            .expect("age one table witness");
        connection
            .execute(
                "UPDATE ledger_gate_options SET choice = 'ftp'
                  WHERE ledger_id = 'main-ledger' AND ordinal = 0",
                [],
            )
            .expect("tamper a differently witnessed table");
        drop(connection);
        drop(store);

        let reopened = crate::workflow_store::WorkflowStore::open(&path);
        assert!(matches!(
            reopened,
            Err(crate::workflow_store::WorkflowStoreError::Corrupt)
        ));
    }

    #[test]
    fn runtime_read_does_not_consult_table_digest_metadata() {
        let store = a_store();
        let projection = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &projection, 10).expect("write the ledger");
        store
            .execute_batch("DROP TABLE ledger_table_digests;")
            .expect("remove open-time-only metadata");

        let held = read(&store, "one", projection.schema)
            .expect("runtime read does not run the open-time digest scan")
            .expect("the ledger remains");
        assert_eq!(held.projection, projection);
    }

    #[test]
    fn current_schema_reopen_refuses_missing_populated_queue_children() {
        for table in [
            LedgerTable::RunAutos,
            LedgerTable::TaskDeps,
            LedgerTable::GateOptions,
            LedgerTable::InboxPending,
            LedgerTable::InboxOpenMessages,
        ] {
            let (_root, path, store) = a_populated_current_store();
            store
                .connection()
                .expect("damage connection")
                .execute_batch(&format!("DROP TABLE {};", table.name()))
                .expect("drop populated child table");
            drop(store);

            let reopened = crate::workflow_store::WorkflowStore::open(&path);
            assert!(
                matches!(
                    reopened,
                    Err(crate::workflow_store::WorkflowStoreError::Corrupt)
                ),
                "{} reopened as {reopened:?}",
                table.name()
            );
        }
    }

    #[test]
    fn current_schema_reopen_refuses_missing_populated_receipt_children() {
        for table in [LedgerTable::ServedMessages, LedgerTable::AckedMessages] {
            let (_root, path, store) = a_populated_current_store();
            store
                .connection()
                .expect("damage connection")
                .execute_batch(&format!("DROP TABLE {};", table.name()))
                .expect("drop populated child table");
            drop(store);

            let reopened = crate::workflow_store::WorkflowStore::open(&path);
            assert!(
                matches!(
                    reopened,
                    Err(crate::workflow_store::WorkflowStoreError::Corrupt)
                ),
                "{} reopened as {reopened:?}",
                table.name()
            );
        }
    }

    #[test]
    fn current_schema_reopen_refuses_child_and_digest_table_co_loss() {
        let (_root, path, store) = a_populated_current_store();
        store
            .connection()
            .expect("damage connection")
            .execute_batch(
                "DROP TABLE ledger_task_deps;
                 DROP TABLE ledger_table_digests;",
            )
            .expect("remove both a child and its per-table witnesses");
        drop(store);

        let reopened = crate::workflow_store::WorkflowStore::open(&path);
        assert!(matches!(
            reopened,
            Err(crate::workflow_store::WorkflowStoreError::Corrupt)
        ));
    }

    #[test]
    fn current_schema_column_growth_keeps_a_populated_ledger_readable() {
        let (_root, path, store) = a_populated_current_store();
        let mut expected = a_ledger_where_order_is_load_bearing();
        expected.workers[0].session = None;
        store
            .connection()
            .expect("age the populated store")
            .execute_batch(&format!(
                "ALTER TABLE ledger_workers DROP COLUMN session;
                 ALTER TABLE ledger_workers DROP COLUMN pane_missing_since_ms;
                 ALTER TABLE ledger_inboxes DROP COLUMN open_holder;
                 ALTER TABLE ledger_inboxes DROP COLUMN open_opened_ms;
                 ALTER TABLE ledger_runs DROP COLUMN coordinator;
                 ALTER TABLE ledger_workers DROP COLUMN adopted_by;
                 ALTER TABLE ledger_runs DROP COLUMN handover;
                 ALTER TABLE ledger_workers DROP COLUMN on_quota_wall;
                 ALTER TABLE ledger_workers DROP COLUMN quota_wait;
                 UPDATE orchestration_ledger_heads SET bytes_held = {};",
                bytes_held(&expected)
            ))
            .expect("the populated shape before the session, pane, lease and seat columns existed");
        drop(store);

        let reopened = crate::workflow_store::WorkflowStore::open(&path)
            .expect("the additive column migration opens");
        let held = read(
            &reopened.connection().expect("read the migrated ledger"),
            "main-ledger",
            expected.schema,
        )
        .expect("the migrated ledger reads")
        .expect("the ledger remains");
        assert_eq!(held.projection, expected);
    }

    /// The head's derived byte count covers every private session byte too.
    /// Leaving this field out makes a direct SQLite change to the new column
    /// invisible to the store's only row-size consistency check.
    #[test]
    fn worker_session_bytes_are_part_of_the_head_integrity_count() {
        let mut projection = a_ledger_where_order_is_load_bearing();
        let session = projection.workers[0]
            .session
            .take()
            .expect("the fixture carries a provider session");
        let without = bytes_held(&projection);
        let expected = session.id.len()
            + session
                .transcript_path
                .as_ref()
                .map_or(0, std::string::String::len);
        projection.workers[0].session = Some(session);

        assert_eq!(
            bytes_held(&projection) - without,
            expected as u64,
            "provider-session text is invisible to bytes_held"
        );
    }

    /// The coordinator seat's names count toward the head's byte total, for
    /// the session's reason: a string the accounting skips is one a direct
    /// SQLite change could grow unwatched (t-2512).
    #[test]
    fn coordinator_seat_bytes_are_part_of_the_head_integrity_count() {
        let mut projection = a_ledger_where_order_is_load_bearing();
        let without = bytes_held(&projection);
        let seat = CoordinatorSeat {
            seat: "blue/%1".to_string(),
            actor: Some("actor-of-the-blue-leader".to_string()),
            generation: 2,
            since_ms: 4,
            vacated_ms: None,
            handover: None,
        };
        let expected = seat.seat.len() + seat.actor.as_ref().map_or(0, String::len);
        projection.runs[0].coordinator = Some(seat);
        assert_eq!(
            bytes_held(&projection) - without,
            expected as u64,
            "seat text is invisible to bytes_held"
        );
    }

    /// The byte-accounting assertion above crosses SQLite here. A session row
    /// changed without the head in the same transaction is not a generation
    /// this store wrote and must fail closed on read.
    #[test]
    fn a_session_change_without_its_head_count_is_refused() {
        let store = a_store();
        let projection = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &projection, 10).expect("write the session generation");
        let changed = zerocode_core::ProviderSession {
            key: zerocode_core::provider_session::SessionKey::SessionId,
            id: "a-different-and-longer-session-id".to_string(),
            transcript_path: Some("/a/different/and/longer/transcript.jsonl".to_string()),
        };
        store
            .execute(
                "UPDATE ledger_workers SET session = ?1 WHERE ledger_id = 'one'",
                [serde_json::to_string(&changed).expect("encode changed session")],
            )
            .expect("change only the worker row");

        assert!(
            matches!(
                read(&store, "one", projection.schema),
                Err(EffectJournalError::Corrupt)
            ),
            "a changed session was invisible to the head consistency check"
        );
    }

    /// A stale derived byte count remains corruption to every ordinary read;
    /// only the boot repair reader may carry the rows on to full validation.
    #[test]
    fn only_the_repair_reader_can_inspect_a_stale_derived_byte_count() {
        let store = a_store();
        let projection = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");
        store
            .execute(
                "UPDATE orchestration_ledger_heads
                    SET bytes_held = bytes_held + 1
                  WHERE ledger_id = 'one'",
                [],
            )
            .expect("age the derived count");

        assert!(
            matches!(
                read(&store, "one", projection.schema),
                Err(EffectJournalError::Corrupt)
            ),
            "an ordinary read accepted a stale bound"
        );
        let held = read_repairable(&store, "one", projection.schema)
            .expect("the repair reader parses rows")
            .expect("the ledger stands");
        assert!(!held.bytes_match);
        assert_eq!(held.projection, projection);
    }

    /// A ledger written twice is the second one, not both.
    #[test]
    fn writing_again_replaces_the_rows_rather_than_adding_to_them() {
        let store = a_store();
        let first = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &first, 10).expect("it writes");

        let mut second = first.clone();
        second.messages.truncate(1);
        second.inboxes[0].pending.truncate(1);
        second.acked[0].current = false;
        second.acked[1].current = true;
        write(&store, "one", 1, 2, &second, 11).expect("it writes again");

        let held = read(&store, "one", second.schema)
            .expect("it reads")
            .expect("it is there");
        assert_eq!(held.projection, second);
        assert_eq!(held.revision, 2);
    }

    /// The table list, its clear order and its parent/child pairs are three
    /// spellings of one fact the DDL already states in its foreign keys.
    ///
    /// Derived here from `LEDGER_TABLES_SQL` rather than repeated: a table
    /// added to the schema without a place in `ALL` would be written but never
    /// digested; a `REFERENCES` added without its pair in `PARENT_CHILD` would
    /// let a parent rewrite cascade away an unchanged child; a child listed
    /// after its parent in `CLEAR_ORDER` would be cleared by the cascade first
    /// and then again by hand. The digest tables are metadata, and a parent
    /// reconciled in place (`RECONCILED_IN_PLACE`) fires no cascade, so those
    /// two are the only foreign keys the pairs may leave out. One test, so
    /// the four cannot drift apart.
    #[test]
    fn the_typed_table_list_is_the_ddl_read_back() {
        use std::collections::BTreeSet;
        let mut tables = BTreeSet::new();
        let mut pairs = BTreeSet::new();
        let mut child: Option<String> = None;
        for line in LEDGER_TABLES_SQL.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("CREATE TABLE IF NOT EXISTS ") {
                let name = rest.split(' ').next().unwrap_or("").to_string();
                if name.starts_with("ledger_") && !name.ends_with("_digests") {
                    tables.insert(name.clone());
                }
                child = Some(name);
            } else if let Some(at) = line.find("REFERENCES ") {
                let parent = line[at + "REFERENCES ".len()..]
                    .split('(')
                    .next()
                    .unwrap_or("")
                    .to_string();
                if parent.starts_with("ledger_")
                    && let Some(child) = child.as_ref()
                    && !child.ends_with("_digests")
                    && !LedgerTable::from_name(&parent)
                        .is_some_and(LedgerTable::reconciled_in_place)
                {
                    pairs.insert((parent, child.clone()));
                }
            }
        }
        let listed: BTreeSet<String> = LedgerTable::ALL
            .into_iter()
            .map(|table| table.name().to_string())
            .collect();
        assert_eq!(
            listed, tables,
            "`ALL` and the DDL disagree on the projection tables"
        );
        let typed: BTreeSet<(String, String)> = LedgerTable::PARENT_CHILD
            .into_iter()
            .map(|(parent, child)| (parent.name().to_string(), child.name().to_string()))
            .collect();
        assert_eq!(
            typed, pairs,
            "`PARENT_CHILD` and the DDL's foreign keys disagree"
        );
        let clear_at = |wanted: LedgerTable| {
            LedgerTable::CLEAR_ORDER
                .iter()
                .position(|table| *table == wanted)
                .expect("every table has a place in the clear order")
        };
        for (parent, child) in LedgerTable::PARENT_CHILD {
            assert!(
                clear_at(child) < clear_at(parent),
                "{} is cleared after its parent {}",
                child.name(),
                parent.name()
            );
        }
        for (at, table) in LedgerTable::ALL.into_iter().enumerate() {
            assert_eq!(
                table.index(),
                at,
                "{} indexes away from its place in `ALL`",
                table.name()
            );
            assert_eq!(LedgerTable::from_name(table.name()), Some(table));
        }
    }

    /// A receipt-only mutation does not rewrite an unrelated parent table.
    ///
    /// The trigger makes the negative assertion executable: the old
    /// clear-and-reinsert path would delete the run before it reached the
    /// changed receipt, while the selective path leaves the run in place.
    #[test]
    fn a_changed_receipt_does_not_rewrite_unchanged_tables() {
        let store = a_store();
        let first = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &first, 10).expect("it writes");
        store
            .execute_batch(
                "CREATE TRIGGER reject_unchanged_run_delete
                    BEFORE DELETE ON ledger_runs
                    BEGIN SELECT RAISE(ABORT, 'unchanged run rewrite'); END;",
            )
            .expect("the guard");
        store
            .execute_batch(
                "CREATE TRIGGER reject_unchanged_receipt_update
                    BEFORE UPDATE ON ledger_served
                    BEGIN SELECT RAISE(ABORT, 'unchanged receipt rewrite'); END;",
            )
            .expect("the receipt guard");

        let mut second = first.clone();
        second.served.push(ServedRow {
            caller: None,
            request: Text::from("r-new"),
            answer: ServedAnswer::Inline("new answer".to_string()),
            fingerprint: None,
            filed_ms: Some(13),
            expired: false,
        });
        write(&store, "one", 1, 2, &second, 14).expect("only the receipt changed");
        store
            .execute_batch("DROP TRIGGER reject_unchanged_run_delete;")
            .expect("remove the guard");
        store
            .execute_batch("DROP TRIGGER reject_unchanged_receipt_update;")
            .expect("remove the receipt guard");

        let held = read(&store, "one", second.schema)
            .expect("it reads")
            .expect("it is there");
        assert_eq!(held.projection, second);
        assert_eq!(held.revision, 2);
    }

    /// The revision a writer expects is the revision it gets, or nothing.
    #[test]
    fn a_write_against_a_revision_that_moved_is_refused() {
        let store = a_store();
        let projection = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");

        let refused = write(&store, "one", 0, 2, &projection, 11).expect_err("must refuse");
        assert!(matches!(refused, EffectJournalError::RevisionMismatch));

        // And the ledger that is there is still the first one.
        let held = read(&store, "one", projection.schema)
            .expect("it reads")
            .expect("it is there");
        assert_eq!(held.revision, 1);
    }

    /// A ledger that is not there is only written when nobody expected one.
    #[test]
    fn a_first_write_that_expected_an_earlier_revision_is_refused() {
        let store = a_store();
        let projection = a_ledger_where_order_is_load_bearing();
        let refused = write(&store, "one", 1, 2, &projection, 10).expect_err("must refuse");
        assert!(matches!(refused, EffectJournalError::RevisionMismatch));
        assert_eq!(read(&store, "one", 1).expect("it reads"), None);
    }

    /// Time does not run backwards on a ledger.
    #[test]
    fn a_write_stamped_before_the_one_already_there_is_refused() {
        let store = a_store();
        let projection = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");
        let refused = write(&store, "one", 1, 2, &projection, 9).expect_err("must refuse");
        assert!(matches!(refused, EffectJournalError::TimestampRegression));
    }

    /// A head that disagrees with its own rows is not read as if it agreed.
    #[test]
    fn a_head_whose_byte_count_disagrees_with_the_rows_is_refused() {
        let store = a_store();
        let projection = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");
        store
            .execute(
                "UPDATE orchestration_ledger_heads SET bytes_held = bytes_held + 1",
                [],
            )
            .expect("the tamper");

        let refused = read(&store, "one", projection.schema).expect_err("must refuse");
        assert!(matches!(refused, EffectJournalError::Corrupt));
    }

    /// Rows that are SITTING in one order are read back in the order the
    /// column says.
    ///
    /// Every other test here writes its rows in ordinal order, so the order
    /// the pages happen to hold and the order the column names are the same
    /// number — and a read that dropped `ORDER BY` would pass all of them
    /// while being wrong. This one puts the rows in physically backwards, so
    /// the two orders disagree and only the column can be right.
    #[test]
    fn rows_lying_in_one_order_are_read_back_in_the_order_the_column_names() {
        let store = a_store();
        store
            .execute(
                "INSERT INTO orchestration_ledger_heads (
                    ledger_id, revision, next_id, bytes_held, updated_at_ms
                 ) VALUES ('one', 1, 9, 0, 10)",
                [],
            )
            .expect("a head");
        // Written second-then-first, and numbered the other way round.
        for (ordinal, id) in [(1_i64, "run-second"), (0, "run-first")] {
            store
                .execute(
                    "INSERT INTO ledger_runs (ledger_id, ordinal, id, name, created_ms)
                     VALUES ('one', ?1, ?2, '', 1)",
                    params![ordinal, id],
                )
                .expect("a run");
        }
        store
            .execute(
                "INSERT INTO ledger_inboxes (ledger_id, ordinal, run, address, open_delivery)
                 VALUES ('one', 0, 'run-first', '%1', NULL)",
                [],
            )
            .expect("an inbox");
        for (ordinal, message) in [(1_i64, "m-second"), (0, "m-first")] {
            store
                .execute(
                    "INSERT INTO ledger_inbox_pending (
                        ledger_id, inbox_ordinal, ordinal, message
                     ) VALUES ('one', 0, ?1, ?2)",
                    params![ordinal, message],
                )
                .expect("a pending message");
        }

        let held = read(&store, "one", 1)
            .expect("it reads")
            .expect("it is there");
        assert_eq!(
            held.projection
                .runs
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["run-first", "run-second"],
            "the runs came back in the order they were lying in"
        );
        assert_eq!(
            held.projection.inboxes[0].pending,
            vec!["m-first".to_string(), "m-second".to_string()],
            "the queue came back in the order it was lying in"
        );
    }

    /// A write that never commits leaves the ledger that was already there.
    ///
    /// `write` takes a connection rather than opening its own transaction, so
    /// this is the promise it makes to whoever owns the transaction: nothing
    /// it does stands on its own. A store that quietly committed the rows
    /// while the caller's transaction rolled back would hold a ledger nobody
    /// decided to write.
    #[test]
    fn a_write_that_is_rolled_back_leaves_the_ledger_that_was_there() {
        let mut store = a_store();
        let first = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &first, 10).expect("it writes");

        let transaction = store.transaction().expect("a transaction");
        let mut second = first.clone();
        second.messages.clear();
        second.next_id = 99;
        write(&transaction, "one", 1, 2, &second, 11).expect("it writes inside");
        transaction.rollback().expect("and is rolled back");

        let held = read(&store, "one", first.schema)
            .expect("it reads")
            .expect("it is there");
        assert_eq!(held.projection, first, "the rolled-back write stood anyway");
        assert_eq!(held.revision, 1);
    }

    /// A selective write that fails after touching its changed table leaves
    /// the previous projection whole when the caller rolls its transaction
    /// back. This is the same contract as the runtime's Immediate boundary,
    /// exercised on the new row-level receipt path itself.
    #[test]
    fn a_selective_write_that_fails_leaves_the_previous_ledger_whole() {
        let mut store = a_store();
        let first = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &first, 10).expect("it writes");
        store
            .execute_batch(
                "CREATE TRIGGER fail_served_update
                    AFTER UPDATE OF request ON ledger_served
                    BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END;",
            )
            .expect("the receipt fault");

        let mut second = first.clone();
        second.served[0].request = Text::from("changed request");
        let transaction = store.transaction().expect("a transaction");
        assert!(matches!(
            write(&transaction, "one", 1, 2, &second, 11),
            Err(EffectJournalError::Database)
        ));
        transaction.rollback().expect("the failed write rolls back");
        store
            .execute_batch("DROP TRIGGER fail_served_update;")
            .expect("remove the receipt fault");

        let held = read(&store, "one", first.schema)
            .expect("the old ledger still reads")
            .expect("it is there");
        assert_eq!(held.projection, first);
        assert_eq!(held.revision, 1);
    }

    /// A database that will not take writes is told so, not half written.
    #[test]
    fn a_write_into_a_database_that_refuses_writes_is_refused() {
        let store = a_store();
        let projection = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");

        store
            .execute_batch("PRAGMA query_only = ON;")
            .expect("a database that will not be written");
        let mut second = projection.clone();
        second.next_id = 99;
        let refused = write(&store, "one", 1, 2, &second, 11).expect_err("must refuse");
        assert!(
            matches!(refused, EffectJournalError::Database),
            "{refused:?}"
        );

        store
            .execute_batch("PRAGMA query_only = OFF;")
            .expect("writes again");
        let held = read(&store, "one", projection.schema)
            .expect("it reads")
            .expect("it is there");
        assert_eq!(held.projection, projection, "the refused write left a mark");
    }

    /// A word this window does not know is refused, not guessed at.
    ///
    /// The columns holding an enum carry the word the type itself writes, and
    /// nothing in the schema repeats that list — so this is the door that
    /// catches a file whose status or kind was edited into something no
    /// version of this window ever wrote.
    #[test]
    fn a_word_this_window_does_not_know_is_refused_rather_than_guessed() {
        for (table, column) in [
            ("ledger_tasks", "status"),
            ("ledger_messages", "kind"),
            ("ledger_messages", "priority"),
            ("ledger_gates", "status"),
        ] {
            let store = a_store();
            let projection = a_ledger_where_order_is_load_bearing();
            write(&store, "one", 0, 1, &projection, 10).expect("it writes");
            store
                .execute(
                    &format!("UPDATE {table} SET {column} = 'not-a-word-this-window-writes'"),
                    [],
                )
                .expect("the tamper");

            let refused = read(&store, "one", projection.schema).expect_err("must refuse");
            assert!(
                matches!(refused, EffectJournalError::Corrupt),
                "{table}.{column}: {refused:?}"
            );
        }
    }

    /// Every word the ledger has for a worker survives the disk.
    ///
    /// The column is TEXT and the spelling is the enum's own serialisation, so
    /// a new state needs no schema change — which is exactly why nothing would
    /// have failed had one been added and never stored. This walks
    /// [`WorkerState::ALL`], so the day a word is added it is either written
    /// and read back or this test says so.
    #[test]
    fn every_worker_state_the_ledger_knows_survives_a_round_trip() {
        use zerocode_core::orchestration::WorkerState;

        for state in WorkerState::ALL {
            let store = a_store();
            let mut projection = a_ledger_where_order_is_load_bearing();
            for worker in &mut projection.workers {
                worker.state = state;
            }
            write(&store, "one", 0, 1, &projection, 10).expect("it writes");
            let read_back = read(&store, "one", projection.schema)
                .expect("it reads")
                .expect("a ledger");
            assert!(
                read_back
                    .projection
                    .workers
                    .iter()
                    .all(|worker| worker.state == state),
                "{} did not survive the disk",
                state.as_str()
            );
        }
    }

    /// A receipt that is neither one shape nor the other is refused.
    ///
    /// The schema states the pairing — a renderer with no inline answer, an
    /// inline answer with no renderer — because that is a shape a database can
    /// know. This is the other half: a row that got past the pairing by being
    /// edited into a renderer this window has never heard of.
    #[test]
    fn a_receipt_row_this_window_cannot_read_is_refused() {
        let store = a_store();
        let projection = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");
        store
            .execute(
                "UPDATE ledger_served SET renderer = 'check-v2' WHERE renderer IS NOT NULL",
                [],
            )
            .expect("the tamper");

        let refused = read(&store, "one", projection.schema).expect_err("must refuse");
        assert!(
            matches!(refused, EffectJournalError::Corrupt),
            "{refused:?}"
        );
    }

    /// A row cannot claim a parent that is not there.
    #[test]
    fn a_child_row_with_no_parent_is_refused_by_the_database() {
        let store = a_store();
        let projection = a_ledger_where_order_is_load_bearing();
        write(&store, "one", 0, 1, &projection, 10).expect("it writes");

        let orphaned = store.execute(
            "INSERT INTO ledger_inbox_pending (ledger_id, inbox_ordinal, ordinal, message)
             VALUES ('one', 404, 0, 'm-1')",
            [],
        );
        assert!(orphaned.is_err(), "a pending row with no inbox was taken");

        let elsewhere = store.execute(
            "INSERT INTO ledger_runs (ledger_id, ordinal, id, name, created_ms)
             VALUES ('no-such-ledger', 0, 'run-1', 'name', 1)",
            [],
        );
        assert!(elsewhere.is_err(), "a run with no head was taken");
    }

    /// One database, two ledgers, and neither is the other.
    #[test]
    fn two_ledgers_in_one_database_do_not_read_each_other() {
        let store = a_store();
        let first = a_ledger_where_order_is_load_bearing();
        let mut second = first.clone();
        second.messages.clear();
        second.inboxes.clear();
        second.served.clear();
        second.acked.clear();
        second.next_id = 7;
        write(&store, "one", 0, 1, &first, 10).expect("it writes");
        write(&store, "two", 0, 1, &second, 10).expect("it writes");

        assert_eq!(
            read(&store, "one", first.schema)
                .expect("it reads")
                .expect("it is there")
                .projection,
            first
        );
        assert_eq!(
            read(&store, "two", second.schema)
                .expect("it reads")
                .expect("it is there")
                .projection,
            second
        );
    }
}
