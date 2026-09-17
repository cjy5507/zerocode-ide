//! Reading OpenCode's sessions out of its database, for the vault panel.
//!
//! The rules — what a card is named, which text is its preview, how it is
//! resumed — are in [`zerocode_core::vault_opencode`]. What is left here is the
//! three statements that fetch them, and the same file-claiming this crate's
//! token ledger does for the same reason: `opencode-backup.db` beside
//! `opencode.db` holds the same conversations, and a panel that reads both
//! shows every session twice.
//!
//! ## Three statements, not one per session
//!
//! Orca asks the database once per session, on demand
//! (`parseOpenCodeSqliteSession`), which is right for a list that parses a row
//! only when it is scrolled into view. Ours builds every card the panel can
//! show in one pass, so the same shape would be four hundred round trips. It is
//! instead three: the sessions, their message counts, and their preview lines —
//! each one statement, whatever the session count.
//!
//! The preview statement ranks parts per session with a window function, which
//! reads every text part of the sessions it was asked about. The bound is the
//! session limit (two hundred), and the blobs that make this table large are
//! tool output — a part type the statement never selects.
//!
//! ## What is skipped, and what is an error
//!
//! A database without a `session` table is an older generation of this store,
//! or a file that is not this store at all: skipped in silence, because a
//! machine that has one is not a machine with a broken vault. A database that
//! cannot be OPENED is reported, because a locked or unreadable file is a
//! history the panel would otherwise say does not exist.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::Connection;

use zerocode_core::vault::{ScanIssue, VaultSession};
use zerocode_core::vault_opencode::{self as rules, Line, SessionRow};

/// What one read of this machine's OpenCode databases found.
#[derive(Debug, Default)]
pub struct OpenCodeVault {
    pub sessions: Vec<VaultSession>,
    pub issues: Vec<ScanIssue>,
}

/// Every OpenCode session this machine can show, newest first.
///
/// `limit` is applied per database, and the caller merges this list with the
/// walked stores' ([`zerocode_core::vault::merge`]) — each leg holding its own
/// newest `limit` is what makes that merge exact.
#[must_use]
pub fn scan(limit: usize) -> OpenCodeVault {
    let mut found = OpenCodeVault::default();
    let mut claimed: HashSet<String> = HashSet::new();
    for path in crate::opencode_home::databases() {
        match read_one(&path, limit, &mut claimed) {
            Ok(mut sessions) => found.sessions.append(&mut sessions),
            Err(reason) => found.issues.push(ScanIssue {
                agent: rules::SLUG.to_string(),
                path: path.to_string_lossy().into_owned(),
                reason,
                count: 1,
            }),
        }
    }
    found
}

/// One database's sessions, skipping the ones an earlier file already claimed.
fn read_one(
    path: &Path,
    limit: usize,
    claimed: &mut HashSet<String>,
) -> Result<Vec<VaultSession>, String> {
    let db = crate::sqlite_read::open_read_only(path)?;
    if !readable(&db) {
        return Ok(Vec::new());
    }
    let rows: Vec<SessionRow> = sessions(&db, limit)?
        .into_iter()
        .filter(|row| claimed.insert(row.id.clone()))
        .collect();
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    let mut counts = counted(&db, &ids)?;
    let mut previews = previewed(&db, &ids)?;
    let shown = path.to_string_lossy();
    Ok(rows
        .into_iter()
        .map(|mut row| {
            row.message_count = counts.remove(&row.id).unwrap_or(0);
            let lines = previews.remove(&row.id).unwrap_or_default();
            rules::card(&shown, &row, &lines)
        })
        .collect())
}

/// Whether this file holds the table this reader knows.
fn readable(db: &Connection) -> bool {
    crate::sqlite_read::table_exists(db, "session")
        && crate::sqlite_read::column_exists(db, "session", "time_created")
        && crate::sqlite_read::column_exists(db, "session", "time_updated")
}

/// The sessions worth a card, newest first.
///
/// A subagent's session (`parent_id`) is not a conversation somebody had — it
/// is a step inside one, and listing it would put a card in the panel that the
/// person never opened. An archived session is one they put away. Both
/// predicates are Orca's (`buildSessionListQuery`), and both are conditional on
/// the column existing, because the generations that lack it have neither.
fn sessions(db: &Connection, limit: usize) -> Result<Vec<SessionRow>, String> {
    let title = column(db, "title");
    let directory = column(db, "directory");
    let model = column(db, "model");
    let own = |name: &str| {
        if crate::sqlite_read::column_exists(db, "session", name) {
            format!("AND {name} IS NULL")
        } else {
            String::new()
        }
    };
    let sql = format!(
        "SELECT id, {title} AS title, {directory} AS directory, {model} AS model, \
         time_created, time_updated \
         FROM session WHERE 1 = 1 {parent} {archived} \
         ORDER BY CASE WHEN time_updated > 0 THEN time_updated ELSE time_created END DESC, \
         id DESC LIMIT ?1",
        parent = own("parent_id"),
        archived = own("time_archived"),
    );
    let mut statement = db.prepare(&sql).map_err(said)?;
    let rows = statement
        .query_map([limit], |row| {
            Ok(SessionRow {
                id: row.get::<_, String>(0)?,
                title: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                directory: row.get::<_, Option<String>>(2)?,
                model_json: row.get::<_, Option<String>>(3)?,
                created_ms: row.get::<_, Option<i64>>(4)?.unwrap_or_default(),
                updated_ms: row.get::<_, Option<i64>>(5)?.unwrap_or_default(),
                message_count: 0,
            })
        })
        .map_err(said)?;
    rows.collect::<rusqlite::Result<Vec<SessionRow>>>()
        .map_err(said)
}

/// How many turns each of these sessions holds.
///
/// A count of what a person would call a message: their own turns and the
/// agent's, not the tool calls between them. Empty when this generation has no
/// `message` table, which is a count nobody can give rather than a zero.
fn counted(db: &Connection, ids: &[String]) -> Result<HashMap<String, usize>, String> {
    if !turns_exist(db) {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT session_id, COUNT(*) FROM message \
         WHERE session_id IN ({holes}) \
         AND json_extract(data, '$.role') IN ('user', 'assistant') \
         GROUP BY session_id",
        holes = holes(ids.len()),
    );
    let mut statement = db.prepare(&sql).map_err(said)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(ids), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })
        .map_err(said)?;
    rows.collect::<rusqlite::Result<HashMap<String, usize>>>()
        .map_err(said)
}

/// The opening lines of each of these sessions.
///
/// Ordered by the MESSAGE's time before the part's: a part can be written after
/// the turn it belongs to, and the question here is which turn came first.
fn previewed(db: &Connection, ids: &[String]) -> Result<HashMap<String, Vec<Line>>, String> {
    if !turns_exist(db) || !parts_exist(db) {
        return Ok(HashMap::new());
    }
    let sql = format!(
        "SELECT session_id, role, data FROM ( \
           SELECT m.session_id AS session_id, \
                  json_extract(m.data, '$.role') AS role, \
                  p.data AS data, \
                  row_number() OVER ( \
                    PARTITION BY m.session_id \
                    ORDER BY m.time_created ASC, p.time_created ASC, p.id ASC \
                  ) AS seat \
           FROM message m JOIN part p ON p.message_id = m.id \
           WHERE m.session_id IN ({holes}) \
             AND json_extract(m.data, '$.role') IN ('user', 'assistant') \
             AND json_extract(p.data, '$.type') = 'text' \
         ) WHERE seat <= {kept} ORDER BY seat ASC",
        holes = holes(ids.len()),
        kept = rules::PREVIEW_PARTS,
    );
    let mut statement = db.prepare(&sql).map_err(said)?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(ids), |row| {
            Ok((
                row.get::<_, String>(0)?,
                Line {
                    role: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    data: row.get::<_, String>(2)?,
                },
            ))
        })
        .map_err(said)?;
    let mut held: HashMap<String, Vec<Line>> = HashMap::new();
    for row in rows {
        let (session, line) = row.map_err(said)?;
        held.entry(session).or_default().push(line);
    }
    Ok(held)
}

/// A `message` table this reader can count.
fn turns_exist(db: &Connection) -> bool {
    crate::sqlite_read::table_exists(db, "message")
        && ["id", "session_id", "time_created", "data"]
            .iter()
            .all(|column| crate::sqlite_read::column_exists(db, "message", column))
}

/// A `part` table this reader can read text out of.
fn parts_exist(db: &Connection) -> bool {
    crate::sqlite_read::table_exists(db, "part")
        && ["id", "message_id", "time_created", "data"]
            .iter()
            .all(|column| crate::sqlite_read::column_exists(db, "part", column))
}

/// A session column, or the null that stands for a generation without it.
fn column(db: &Connection, name: &str) -> String {
    if crate::sqlite_read::column_exists(db, "session", name) {
        name.to_string()
    } else {
        "NULL".to_string()
    }
}

/// `?, ?, …` — one per id. The ids are values, never text in a statement.
fn holes(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<&str>>()
        .join(", ")
}

fn said(error: rusqlite::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database built here, read the way a real one is. The schema is the
    /// measured one, cut down to the columns this reader names.
    fn made(name: &str, sql: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(name);
        let db = Connection::open(&path).expect("create");
        for statement in sql {
            db.execute_batch(statement).expect(statement);
        }
        drop(db);
        (dir, path)
    }

    const SCHEMA: &str = "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, \
         directory TEXT, title TEXT, model TEXT, time_created INTEGER, time_updated INTEGER, \
         time_archived INTEGER);
         CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, \
         time_updated INTEGER, data TEXT);
         CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, \
         time_created INTEGER, time_updated INTEGER, data TEXT);";

    /// One conversation: a session, two turns, and a tool part nobody shows.
    const ONE: &str = "INSERT INTO session VALUES ('ses_a', NULL, '/w/repo', 'Greeting', \
           '{\"id\":\"gpt-5.5-fast\",\"providerID\":\"openai\"}', 1784546484634, 1784546530207, NULL);
         INSERT INTO message VALUES ('m1', 'ses_a', 1784546484634, NULL, '{\"role\":\"user\"}');
         INSERT INTO message VALUES ('m2', 'ses_a', 1784546490000, NULL, '{\"role\":\"assistant\"}');
         INSERT INTO part VALUES ('p1', 'm1', 'ses_a', 1784546484700, NULL, \
           '{\"type\":\"text\",\"text\":\"report.py가 죽습니다\"}');
         INSERT INTO part VALUES ('p2', 'm2', 'ses_a', 1784546491000, NULL, \
           '{\"type\":\"text\",\"text\":\"보겠습니다\"}');
         INSERT INTO part VALUES ('p3', 'm2', 'ses_a', 1784546492000, NULL, \
           '{\"type\":\"tool\",\"tool\":\"bash\"}');";

    fn read(path: &std::path::Path) -> Vec<VaultSession> {
        let mut claimed = HashSet::new();
        read_one(path, 200, &mut claimed).expect("read")
    }

    /// The whole card, out of a database shaped like the real one.
    #[test]
    fn a_session_becomes_a_card_the_panel_can_reopen() {
        let (_dir, path) = made("opencode.db", &[SCHEMA, ONE]);
        let cards = read(&path);
        assert_eq!(cards.len(), 1);
        let card = &cards[0];
        assert_eq!(card.agent, "opencode");
        assert_eq!(card.session_id, "ses_a");
        assert_eq!(card.title, "Greeting");
        assert_eq!(card.cwd.as_deref(), Some("/w/repo"));
        assert_eq!(card.model.as_deref(), Some("openai/gpt-5.5-fast"));
        assert_eq!(card.file_path, path.to_string_lossy());
        // Two turns counted, the tool part not shown, the person first.
        assert_eq!(card.message_count, 2);
        assert_eq!(
            card.preview
                .iter()
                .map(|line| (line.role.as_str(), line.text.as_str()))
                .collect::<Vec<_>>(),
            [
                ("user", "report.py가 죽습니다"),
                ("assistant", "보겠습니다")
            ]
        );
        assert_eq!(
            card.resume.as_deref(),
            Some("cd '/w/repo' && opencode --session 'ses_a'")
        );
    }

    /// A step inside a conversation and a conversation put away are not cards.
    #[test]
    fn a_subagent_and_an_archived_session_are_not_conversations() {
        let (_dir, path) = made(
            "opencode.db",
            &[
                SCHEMA,
                ONE,
                "INSERT INTO session VALUES ('ses_child', 'ses_a', '/w/repo', 'Sub', NULL,
                   1784546500000, 1784546500000, NULL);
                 INSERT INTO session VALUES ('ses_old', NULL, '/w/repo', 'Away', NULL,
                   1784546600000, 1784546600000, 1784546700000);",
            ],
        );
        let cards = read(&path);
        assert_eq!(
            cards
                .iter()
                .map(|one| one.session_id.as_str())
                .collect::<Vec<_>>(),
            ["ses_a"]
        );
    }

    /// The backup file holds the same conversations; the live one keeps them.
    #[test]
    fn a_second_file_does_not_show_the_same_session_twice() {
        let (_dir, live) = made("opencode.db", &[SCHEMA, ONE]);
        let (_spare, backup) = made(
            "opencode-backup.db",
            &[
                SCHEMA,
                ONE,
                "INSERT INTO session VALUES ('ses_b', NULL, '/w/other', 'Only here', NULL,
                   1784546400000, 1784546400000, NULL);",
            ],
        );
        let mut claimed = HashSet::new();
        let first = read_one(&live, 200, &mut claimed).expect("live");
        let second = read_one(&backup, 200, &mut claimed).expect("backup");
        assert_eq!(first.len(), 1, "the live file's own session");
        assert_eq!(
            second
                .iter()
                .map(|one| one.session_id.as_str())
                .collect::<Vec<_>>(),
            ["ses_b"],
            "the shared session was claimed, the unique one was not"
        );
    }

    /// Newest first, and no more than asked for.
    #[test]
    fn the_newest_sessions_are_the_ones_kept() {
        let (_dir, path) = made(
            "opencode.db",
            &[
                SCHEMA,
                "INSERT INTO session VALUES ('old', NULL, '/w', 'Old', NULL, 1, 1000, NULL);
                 INSERT INTO session VALUES ('new', NULL, '/w', 'New', NULL, 2, 3000, NULL);
                 INSERT INTO session VALUES ('mid', NULL, '/w', 'Mid', NULL, 3, 2000, NULL);
                 INSERT INTO session VALUES ('never', NULL, '/w', 'Never', NULL, 2500, 0, NULL);",
            ],
        );
        let mut claimed = HashSet::new();
        let cards = read_one(&path, 3, &mut claimed).expect("read");
        assert_eq!(
            cards
                .iter()
                .map(|one| one.session_id.as_str())
                .collect::<Vec<_>>(),
            ["new", "never", "mid"],
            "a session never updated is dated by its creation"
        );
    }

    /// A generation this reader does not know is skipped, not reported.
    #[test]
    fn a_database_without_sessions_is_not_a_failure() {
        let (_dir, path) = made("opencode.db", &["CREATE TABLE kv (k TEXT, v TEXT);"]);
        let mut claimed = HashSet::new();
        assert!(
            read_one(&path, 200, &mut claimed)
                .expect("no session table is not an error")
                .is_empty()
        );
    }

    /// Without the tables that hold the conversation, the card still stands.
    #[test]
    fn a_session_with_no_readable_turns_is_still_a_card() {
        let (_dir, path) = made(
            "opencode.db",
            &[
                "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, title TEXT,
                   time_created INTEGER, time_updated INTEGER);",
                "INSERT INTO session VALUES ('ses_a', '/w/repo', 'Greeting', 1000, 2000);",
            ],
        );
        let cards = read(&path);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].message_count, 0);
        assert!(cards[0].preview.is_empty());
        assert!(cards[0].model.is_none(), "no model column, no model");
        // Nothing was said in it as far as this reader knows, so the panel
        // must not offer to continue a conversation it cannot see.
        assert!(!cards[0].resumable());
    }

    /// A file that is not a database says which file it was.
    #[test]
    fn an_unreadable_file_is_reported_with_its_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("opencode.db");
        std::fs::write(&path, b"not a database").expect("write");
        let mut claimed = HashSet::new();
        let said = read_one(&path, 200, &mut claimed).expect_err("garbage cannot be read");
        assert!(said.contains("opencode.db"), "{said}");
    }
}
