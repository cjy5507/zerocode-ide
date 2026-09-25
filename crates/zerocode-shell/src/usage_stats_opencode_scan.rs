//! The read that feeds [`zerocode_core::usage_stats_opencode`], and its cache.
//!
//! The core module decides what a row means; this one finds the databases and
//! runs the queries. Same split as the other two ledgers, for the same reason:
//! every rule a person could disagree with is pure and tested next to itself,
//! and what is left here is I/O.
//!
//! ## Read-only, on a database somebody else is writing
//!
//! OpenCode may be running while this reads. Every guard that makes that safe
//! — read-only flags, `query_only`, a busy timeout, and asking the schema what
//! it holds — lives in [`crate::sqlite_read`], because the session vault next
//! door opens the same files and two copies of those promises would be one
//! promise nobody keeps.
//!
//! ## Several files, one set of sessions
//!
//! `opencode-backup.db` beside `opencode.db` holds the same sessions. The live
//! file is read first (see [`crate::opencode_home::databases`]) and claims the
//! sessions it saw; a copy read afterwards skips them. Without that, a machine
//! with one backup reports twice the tokens it spent.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::{Connection, Row};
use serde::{Deserialize, Serialize};

use crate::scan_cell::ScanCell;
use zerocode_core::usage_ledger::{self, Entry, Ledger};
use zerocode_core::usage_stats::WorktreeRef;
use zerocode_core::usage_stats_opencode as opencode_stats;

/// A ceiling on one database's rows, so a machine with a decade of history
/// cannot hold this thread for minutes. Reached only by a database far larger
/// than any seen: the one this was written against holds 5271.
const MAX_ROWS: usize = 200_000;

/// What one OpenCode read learned.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OpenCodeScan {
    pub ledger: Ledger,
    pub scanned_at: i64,
    /// How many database files were read.
    pub databases: usize,
    pub rows: usize,
    pub capped: bool,
    /// Set when a database was found but could not be read — a locked file, a
    /// schema this reader does not know. The pane says so rather than showing
    /// a confident zero.
    pub error: Option<String>,
}

/// Reads every OpenCode database on this machine and merges the answer.
#[must_use]
pub fn scan(worktrees: &[WorktreeRef], offset_minutes: i32, now_ms: i64) -> OpenCodeScan {
    let mut ledger = Ledger::default();
    let mut budget = MAX_ROWS;
    let mut claimed: HashSet<String> = HashSet::new();
    let mut databases = 0usize;
    let mut rows = 0usize;
    let mut error = None;
    for path in crate::opencode_home::databases() {
        databases += 1;
        match read_one(&path, worktrees, offset_minutes, &mut claimed, &mut budget) {
            Ok((one, read)) => {
                rows += read;
                usage_ledger::merge(&mut ledger, one);
            }
            // The first failure is the one reported: a later one is usually the
            // same cause, and a pane has room for one sentence.
            Err(said) => error = error.or(Some(said)),
        }
    }
    usage_ledger::finalize(&mut ledger);
    OpenCodeScan {
        ledger,
        scanned_at: now_ms,
        databases,
        rows,
        capped: budget == 0,
        error,
    }
}

/// Reads one database, skipping sessions an earlier file already counted.
fn read_one(
    path: &Path,
    worktrees: &[WorktreeRef],
    offset_minutes: i32,
    claimed: &mut HashSet<String>,
    budget: &mut usize,
) -> Result<(Ledger, usize), String> {
    let db = crate::sqlite_read::open_read_only(path)?;
    if !crate::sqlite_read::table_exists(&db, "session") {
        return Ok((Ledger::default(), 0));
    }

    // Found once: recognising the table costs a pass over every message blob,
    // and the shape decision and the read would otherwise each pay for it.
    let source = message_source(&db);
    let turns = match shape_of(&db, source.as_ref()) {
        opencode_stats::Shape::Messages => message_turns(&db, source.as_ref(), budget)?,
        opencode_stats::Shape::SessionTotals => session_turns(&db, budget)?,
    };
    let read = turns.len();
    let kept: Vec<Entry> = turns
        .into_iter()
        .filter(|turn| !claimed.contains(&turn.session_id))
        .collect();
    for turn in &kept {
        claimed.insert(turn.session_id.clone());
    }
    Ok((
        usage_ledger::aggregate(kept, worktrees, offset_minutes),
        read,
    ))
}

/// Which shape this database's numbers say to read.
fn shape_of(db: &Connection, source: Option<&MessageSource>) -> opencode_stats::Shape {
    let Some(source) = source else {
        return opencode_stats::Shape::SessionTotals;
    };
    if !has_session_totals(db) {
        return opencode_stats::Shape::Messages;
    }
    opencode_stats::richer_shape(source.tokens, counted(db, SESSION_SUM))
}

/// The sum of everything the session table thinks was spent.
const SESSION_SUM: &str = "SELECT coalesce(sum(tokens_input + tokens_output + tokens_reasoning \
     + tokens_cache_read), 0) FROM session";

fn counted(db: &Connection, sql: &str) -> i64 {
    db.query_one(sql, [], |row| row.get(0)).unwrap_or(0)
}

/// Which table holds the per-message turns, and how an assistant row is
/// recognised in it.
///
/// `session_message` is the newer name and `message` the older one, and a
/// machine can have both with only one of them filled — this one has an empty
/// `session_message` beside a `message` holding 5271 rows, so a reader that
/// knows a single name reports zero and calls it "OpenCode not used".
struct MessageSource {
    table: &'static str,
    assistant: &'static str,
    /// The four counters added up over every assistant row in this table, which
    /// is what says whether these rows account for the whole ledger.
    tokens: i64,
}

/// The four counters added up over one table's assistant rows.
fn token_sum(table: &str, assistant: &str) -> String {
    format!(
        "SELECT coalesce(sum(coalesce(json_extract(m.data, '$.tokens.input'), 0) \
         + coalesce(json_extract(m.data, '$.tokens.output'), 0) \
         + coalesce(json_extract(m.data, '$.tokens.reasoning'), 0) \
         + coalesce(json_extract(m.data, '$.tokens.cache.read'), 0)), 0) \
         FROM {table} m WHERE {assistant}"
    )
}

fn message_source(db: &Connection) -> Option<MessageSource> {
    for table in ["session_message", "message"] {
        if !crate::sqlite_read::table_exists(db, table) {
            continue;
        }
        // The newer table types its rows in a column; the older one only in the
        // blob. Either way, a row with no input count is not a spend.
        let assistant = if crate::sqlite_read::column_exists(db, table, "type") {
            "m.type = 'assistant'"
        } else {
            "json_extract(m.data, '$.role') = 'assistant'"
        };
        let tokens = counted(db, &token_sum(table, assistant));
        if tokens > 0 {
            return Some(MessageSource {
                table,
                assistant,
                tokens,
            });
        }
    }
    None
}

fn has_session_totals(db: &Connection) -> bool {
    [
        "cost",
        "tokens_input",
        "tokens_output",
        "tokens_reasoning",
        "tokens_cache_read",
    ]
    .iter()
    .all(|column| crate::sqlite_read::column_exists(db, "session", column))
}

/// One turn per assistant message.
fn message_turns(
    db: &Connection,
    source: Option<&MessageSource>,
    budget: &mut usize,
) -> Result<Vec<Entry>, String> {
    let Some(source) = source else {
        return Ok(Vec::new());
    };
    let sql = format!(
        "SELECT m.session_id, m.time_created, m.time_updated, m.data, s.directory, {}, {} \
         FROM {} m JOIN session s ON s.id = m.session_id{} WHERE {} \
         ORDER BY m.time_created, m.id LIMIT ?1",
        project_worktree(db),
        session_model(db),
        source.table,
        project_join(db),
        source.assistant,
    );
    read_rows(db, &sql, budget, |row| {
        Ok(opencode_stats::entry_of_message(
            &opencode_stats::MessageRow {
                session_id: row.get(0)?,
                time_created: whole(row, 1),
                time_updated: row.get(2).ok(),
                data: row.get(3)?,
                directory: row.get(4).ok(),
                worktree: row.get(5).ok(),
                session_model: row.get(6).ok(),
            },
        ))
    })
}

/// One turn per session, from the totals it keeps itself.
fn session_turns(db: &Connection, budget: &mut usize) -> Result<Vec<Entry>, String> {
    if !has_session_totals(db) {
        return Ok(Vec::new());
    }
    let sql = format!(
        "SELECT s.id, s.time_created, s.time_updated, s.directory, {}, {}, \
         s.cost, s.tokens_input, s.tokens_output, s.tokens_reasoning, s.tokens_cache_read \
         FROM session s{} \
         WHERE s.tokens_input + s.tokens_output + s.tokens_reasoning + s.tokens_cache_read > 0 \
         ORDER BY s.time_created, s.id LIMIT ?1",
        project_worktree(db),
        session_model(db),
        project_join(db),
    );
    read_rows(db, &sql, budget, |row| {
        Ok(opencode_stats::entry_of_session(
            &opencode_stats::SessionRow {
                session_id: row.get(0)?,
                time_created: whole(row, 1),
                time_updated: row.get(2).ok(),
                directory: row.get(3).ok(),
                worktree: row.get(4).ok(),
                session_model: row.get(5).ok(),
                cost: row.get::<_, f64>(6).unwrap_or(0.0),
                tokens_input: whole(row, 7),
                tokens_output: whole(row, 8),
                tokens_reasoning: whole(row, 9),
                tokens_cache_read: whole(row, 10),
            },
        ))
    })
}

/// Runs one query under the row budget and keeps the turns it yielded.
fn read_rows(
    db: &Connection,
    sql: &str,
    budget: &mut usize,
    of_row: impl Fn(&Row<'_>) -> rusqlite::Result<Option<Entry>>,
) -> Result<Vec<Entry>, String> {
    let said = |error: rusqlite::Error| error.to_string();
    let mut statement = db.prepare(sql).map_err(said)?;
    let mut found = statement
        .query(rusqlite::params![*budget as i64])
        .map_err(said)?;
    let mut turns = Vec::new();
    while let Some(row) = found.next().map_err(said)? {
        *budget = budget.saturating_sub(1);
        if let Some(turn) = of_row(row).map_err(said)? {
            turns.push(turn);
        }
    }
    Ok(turns)
}

/// A count, whatever type the column turned out to hold. Text appears where an
/// integer is expected in the older generations of this schema.
fn whole(row: &Row<'_>, at: usize) -> i64 {
    match row.get::<_, rusqlite::types::Value>(at) {
        Ok(rusqlite::types::Value::Integer(held)) => held,
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a stamp or a count beyond i64 is a corrupt row, and saturates"
        )]
        Ok(rusqlite::types::Value::Real(held)) => held as i64,
        Ok(rusqlite::types::Value::Text(held)) => held.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// The project's checkout path, when this generation has a project table to
/// join. A session's own directory is preferred; this is the fallback.
fn project_worktree(db: &Connection) -> &'static str {
    if joins_project(db) {
        "p.worktree"
    } else {
        "NULL AS worktree"
    }
}

fn project_join(db: &Connection) -> &'static str {
    if joins_project(db) {
        " LEFT JOIN project p ON p.id = s.project_id"
    } else {
        ""
    }
}

fn joins_project(db: &Connection) -> bool {
    crate::sqlite_read::table_exists(db, "project")
        && crate::sqlite_read::column_exists(db, "session", "project_id")
}

/// The model column, in the generations that have one.
fn session_model(db: &Connection) -> &'static str {
    if crate::sqlite_read::column_exists(db, "session", "model") {
        "s.model AS session_model"
    } else {
        "NULL AS session_model"
    }
}

static OPENCODE: ScanCell<OpenCodeScan> = ScanCell::new();

/// The held OpenCode read, if there is one.
#[must_use]
pub fn held() -> Option<OpenCodeScan> {
    OPENCODE.held()
}

/// The held OpenCode read, where it lies ([`ScanCell::with_held`]).
pub fn with_held<R>(read: impl FnOnce(&OpenCodeScan) -> R) -> Option<R> {
    OPENCODE.with_held(read)
}

/// Whether a read is running right now.
#[must_use]
pub fn scanning() -> bool {
    OPENCODE.running()
}

/// Starts a read unless one is already running.
pub fn start(worktrees: Vec<WorktreeRef>, offset_minutes: i32, now_ms: i64) -> bool {
    OPENCODE.start(move || scan(&worktrees, offset_minutes, now_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database built here, read the way a real one is.
    ///
    /// The three shapes this reader has to know are three generations of one
    /// schema, and a machine only ever has one of them — so they are made
    /// rather than found, and the real file is what the module note reports.
    fn made(sql: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("opencode.db");
        let db = Connection::open(&path).expect("create");
        for statement in sql {
            db.execute_batch(statement).expect(statement);
        }
        drop(db);
        (dir, path)
    }

    const SESSION_TABLE: &str = "CREATE TABLE session (id TEXT PRIMARY KEY, project_id TEXT, \
         directory TEXT, model TEXT, time_created INTEGER, time_updated INTEGER, \
         cost REAL DEFAULT 0, tokens_input INTEGER DEFAULT 0, tokens_output INTEGER DEFAULT 0, \
         tokens_reasoning INTEGER DEFAULT 0, tokens_cache_read INTEGER DEFAULT 0);
         CREATE TABLE project (id TEXT PRIMARY KEY, worktree TEXT);";

    fn read(path: &std::path::Path) -> (Ledger, usize) {
        let mut claimed = HashSet::new();
        let mut budget = MAX_ROWS;
        read_one(path, &[], 9 * 60, &mut claimed, &mut budget).expect("read")
    }

    fn totals(ledger: &Ledger) -> (i64, i64, i64) {
        let mut input = 0;
        let mut cached = 0;
        let mut total = 0;
        for row in &ledger.daily_aggregates {
            input += row.input_tokens;
            cached += row.cached_input_tokens;
            total += row.total_tokens;
        }
        (input, cached, total)
    }

    /// The message rows are read when they account for everything, and the
    /// vendor's counters arrive unclipped.
    #[test]
    fn the_message_rows_are_read_when_they_hold_the_whole_ledger() {
        let (_dir, path) = made(&[
            SESSION_TABLE,
            "INSERT INTO project VALUES ('p1', '/w/repo');
             INSERT INTO session VALUES ('s1', 'p1', '/w/repo', NULL, 1777449250, 1777449260,
               0, 100, 10, 5, 900);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
               time_updated INTEGER, data TEXT);
             INSERT INTO message VALUES ('m1', 's1', 1777449250, NULL,
               '{\"role\":\"assistant\",\"modelID\":\"gpt-5.5\",\"providerID\":\"openai\",
                 \"tokens\":{\"total\":1015,\"input\":100,\"output\":10,\"reasoning\":5,
                 \"cache\":{\"read\":900}},\"time\":{\"completed\":1777449252541}}');",
        ]);
        let (ledger, rows) = read(&path);
        assert_eq!(rows, 1);
        assert_eq!(totals(&ledger), (100, 900, 1015));
        assert_eq!(ledger.sessions.len(), 1);
        assert_eq!(
            ledger.daily_aggregates[0].model.as_deref(),
            Some("openai/gpt-5.5"),
            "the message's own model did not reach the row"
        );
        assert_eq!(
            ledger.daily_aggregates[0].project_label, "w/repo",
            "the session's directory did not place the turn"
        );
    }

    /// Pruned messages under retained session totals: the coarse read wins.
    #[test]
    fn the_session_totals_are_read_when_the_rows_no_longer_account_for_them() {
        let (_dir, path) = made(&[
            SESSION_TABLE,
            "INSERT INTO project VALUES ('p1', '/w/repo');
             INSERT INTO session VALUES ('s1', 'p1', '/w/repo',
               '{\"providerID\":\"openai\",\"modelID\":\"gpt-5.5\"}', 1777449250, 1777449260,
               2.5, 100, 10, 5, 900);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
               time_updated INTEGER, data TEXT);
             INSERT INTO message VALUES ('m1', 's1', 1777449250, NULL,
               '{\"role\":\"assistant\",\"tokens\":{\"input\":1,\"output\":1},
                 \"time\":{\"completed\":1777449252541}}');",
        ]);
        let (ledger, _) = read(&path);
        assert_eq!(
            totals(&ledger),
            (100, 900, 1015),
            "the pruned message rows were believed over the session's own totals"
        );
        assert_eq!(
            ledger.daily_aggregates[0].estimated_cost_usd,
            Some(2.5),
            "the vendor's own cost did not reach the row"
        );
        assert_eq!(
            ledger.daily_aggregates[0].model.as_deref(),
            Some("openai/gpt-5.5"),
            "the session's model column was not read"
        );
    }

    /// A generation with the newer table name, and no `project` to join.
    #[test]
    fn the_newer_table_name_and_a_missing_project_are_both_read() {
        let (_dir, path) = made(
            &["CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT,
               time_created INTEGER, time_updated INTEGER);
             INSERT INTO session VALUES ('s1', '/w/repo', 1777449250, 1777449260);
             CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT,
               time_created INTEGER, time_updated INTEGER, data TEXT);
             INSERT INTO session_message VALUES ('m1', 's1', 'assistant', 1777449250, NULL,
               '{\"tokens\":{\"input\":7,\"output\":3,\"cache\":{\"read\":11}},
                 \"time\":{\"completed\":1777449252541}}');"],
        );
        let (ledger, rows) = read(&path);
        assert_eq!(rows, 1);
        assert_eq!(totals(&ledger), (7, 11, 21));
    }

    /// A database with no session table at all is empty rather than an error —
    /// a file named like OpenCode's that is something else must not paint a
    /// triangle on the pane.
    #[test]
    fn a_database_that_is_not_opencodes_reads_as_empty() {
        let (_dir, path) = made(&["CREATE TABLE notes (id TEXT);"]);
        let (ledger, rows) = read(&path);
        assert_eq!(rows, 0);
        assert!(ledger.sessions.is_empty());
    }

    /// A copy beside the live database repeats its sessions, and the second
    /// read of one must not bill it again.
    #[test]
    fn a_sibling_copy_does_not_bill_the_same_session_twice() {
        let (_dir, path) = made(&[
            SESSION_TABLE,
            "INSERT INTO session VALUES ('s1', NULL, '/w/repo', NULL, 1777449250, 1777449260,
               0, 100, 10, 5, 900);",
        ]);
        let mut claimed = HashSet::new();
        let mut budget = MAX_ROWS;
        let (first, _) = read_one(&path, &[], 9 * 60, &mut claimed, &mut budget).expect("read");
        let (again, _) = read_one(&path, &[], 9 * 60, &mut claimed, &mut budget).expect("read");
        assert_eq!(totals(&first), (100, 900, 1015));
        assert_eq!(
            totals(&again),
            (0, 0, 0),
            "the same session was counted by two databases"
        );
    }
}
