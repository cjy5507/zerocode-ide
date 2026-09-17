//! Opening somebody else's database without ever being the reason it changed.
//!
//! Two readers in this crate open OpenCode's SQLite files — the token ledger
//! ([`crate::usage_stats_opencode_scan`]) and the session vault
//! ([`crate::vault_opencode_scan`]) — and a third will arrive the next time a
//! vendor moves a store into a database. They all need the same three things,
//! and the original keeps them in one place too
//! (`src/main/opencode-usage/schema-helpers.ts`, imported by both of its
//! scanners).
//!
//! ## Why the guards, one by one
//!
//! * **Read-only flags.** Nothing in this window may be the reason a person's
//!   conversation history changed. The flag is the contract.
//! * **`query_only`.** The same promise a second time, on the connection rather
//!   than the file handle, so a mistake in a `SELECT` list — a call to a
//!   function with a side effect — is refused by the engine instead of running.
//!   Orca calls this belt-and-suspenders and it is exactly that: cheap, and the
//!   thing it prevents is unrecoverable.
//! * **A busy timeout.** OpenCode may be running. A write in flight is
//!   something to wait out, not a read to report as broken.
//! * **Asking what the schema holds.** OpenCode has shipped several
//!   generations of these tables; a reader that assumes the newest one reports
//!   an empty history on a machine that has not upgraded, which reads as "you
//!   have never used this".
//!
//! A read-only connection still follows the write-ahead log, so what it sees is
//! the committed present rather than the last checkpoint.

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

/// How long to wait for a writer to finish before giving up on a statement.
const BUSY_WAIT: Duration = Duration::from_millis(3_000);

/// Opens a database this process will only ever read.
///
/// The error carries the path, because a machine can hold several of these
/// files and "unable to open database file" names none of them.
pub fn open_read_only(path: &Path) -> Result<Connection, String> {
    let said = |error: rusqlite::Error| format!("{}: {error}", path.display());
    let db = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(said)?;
    db.busy_timeout(BUSY_WAIT).map_err(said)?;
    db.pragma_update(None, "query_only", true).map_err(said)?;
    // Opening does not read the file: SQLite validates the header on the first
    // statement, so a text file named `opencode.db` opens fine and fails later
    // — where the failure looks like "this store holds no sessions" rather than
    // "this file is not a store". One trivial statement here turns that into
    // the error it is, for every reader.
    db.query_one("SELECT COUNT(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    })
    .map_err(said)?;
    Ok(db)
}

/// Whether this database has that table.
#[must_use]
pub fn table_exists(db: &Connection, table: &str) -> bool {
    db.query_one(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .is_ok()
}

/// Whether that table has that column.
///
/// The table name is interpolated because `PRAGMA` takes no parameters — every
/// caller passes a literal from its own file, never anything a person or a row
/// could have named.
#[must_use]
pub fn column_exists(db: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut statement) = db.prepare(&format!("PRAGMA table_info({table})")) else {
        return false;
    };
    let Ok(mut rows) = statement.query([]) else {
        return false;
    };
    while let Ok(Some(row)) = rows.next() {
        if row.get::<_, String>(1).is_ok_and(|name| name == column) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn made(name: &str, statements: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("zerocode-sqlite-read-{name}.db"));
        let _ = std::fs::remove_file(&path);
        let db = Connection::open(&path).expect("a new database");
        db.execute_batch(statements).expect("the fixture schema");
        drop(db);
        path
    }

    /// A read-only connection refuses to write, by flag and by pragma.
    #[test]
    fn nothing_opened_here_can_change_the_file() {
        let path = made("readonly", "CREATE TABLE session (id TEXT);");
        let db = open_read_only(&path).expect("the fixture opens");
        let wrote = db.execute("INSERT INTO session (id) VALUES ('x')", []);
        assert!(wrote.is_err(), "a read-only connection accepted a write");
        assert_eq!(
            db.query_one("PRAGMA query_only", [], |row| row.get::<_, i64>(0)),
            Ok(1),
            "query_only was not set"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The schema questions answer about what is there, not about what is named.
    #[test]
    fn the_schema_is_asked_rather_than_assumed() {
        let path = made(
            "schema",
            "CREATE TABLE session (id TEXT, time_created INTEGER);",
        );
        let db = open_read_only(&path).expect("the fixture opens");
        assert!(table_exists(&db, "session"));
        assert!(!table_exists(&db, "message"));
        assert!(column_exists(&db, "session", "time_created"));
        assert!(!column_exists(&db, "session", "time_archived"));
        // A table that is not there has no columns, and asking must not panic.
        assert!(!column_exists(&db, "nowhere", "id"));
        let _ = std::fs::remove_file(&path);
    }

    /// A file that is not a database is an error naming the file.
    ///
    /// Both ways of not being one: absent, and present but not a database. The
    /// second is the one that needs a statement to find out, and the one that
    /// otherwise reads as an empty history.
    #[test]
    fn an_unopenable_file_says_which_one() {
        let missing = std::env::temp_dir().join("zerocode-sqlite-read-absent.db");
        let _ = std::fs::remove_file(&missing);
        let said = open_read_only(&missing).expect_err("a missing file cannot open");
        assert!(said.contains("zerocode-sqlite-read-absent.db"), "{said}");

        let garbage = std::env::temp_dir().join("zerocode-sqlite-read-garbage.db");
        std::fs::write(&garbage, b"not a database").expect("write");
        let said = open_read_only(&garbage).expect_err("text is not a database");
        assert!(said.contains("zerocode-sqlite-read-garbage.db"), "{said}");
        let _ = std::fs::remove_file(&garbage);
    }
}
