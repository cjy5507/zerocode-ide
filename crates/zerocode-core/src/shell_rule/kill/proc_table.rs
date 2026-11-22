//! The process table read from Linux's `/proc`, without starting `ps`
//! (t-125701): a container that ships no `ps` (procps) still has a table, so
//! a kill of a process the session started is judged instead of refused as
//! unreadable. The rows are the ones `ps -o pid=,ppid=,pgid=,command=` gives.

use super::ProcessRow;
use std::time::Duration;

#[cfg(any(target_os = "linux", all(unix, test)))]
use super::LISTING_MAX_BYTES;

// Linux exposes 52 stat fields. The process group is field 5.
#[cfg(any(target_os = "linux", test))]
const STAT_FIELDS_AFTER_PGID: usize = 47;

/// One row from a process's `/proc/<pid>/stat` text and its
/// `/proc/<pid>/cmdline` bytes; `None` for a stat that does not read.
///
/// The command is the stat's own name in brackets when the command line is
/// empty (a kernel thread, a zombie), as `ps` prints it. The name sits in
/// parentheses and may itself hold spaces and parentheses, so the fields are
/// read past the last `)`: state, then the parent, then the group.
#[cfg(any(target_os = "linux", test))]
pub(super) fn row(pid: u32, stat: &str, cmdline: &[u8]) -> Option<ProcessRow> {
    let open = stat.find('(')?;
    if stat[..open].trim().parse::<u32>().ok()? != pid {
        return None;
    }
    let close = stat.rfind(')')?;
    let name = stat.get(open + 1..close)?;
    let mut fields = stat.get(close + 1..)?.split_whitespace();
    let state = fields.next()?;
    let ppid = fields.next()?.parse().ok()?;
    let pgid = fields.next()?.parse().ok()?;
    if state.len() != 1 || fields.take(STAT_FIELDS_AFTER_PGID).count() != STAT_FIELDS_AFTER_PGID {
        return None;
    }
    let words: Vec<String> = cmdline
        .split(|byte| *byte == 0)
        .filter(|word| !word.is_empty())
        .map(|word| String::from_utf8_lossy(word).into_owned())
        .collect();
    let command = if words.is_empty() {
        format!("[{name}]")
    } else {
        words.join(" ")
    };
    Some(ProcessRow {
        pid,
        ppid,
        pgid,
        command,
    })
}

/// Read a complete snapshot within the remaining listing budget. Kernel reads
/// can block inside an individual syscall, so the caller waits separately and
/// at most one sampler can remain in flight after a deadline.
#[cfg(target_os = "linux")]
pub(super) fn read(within: Duration) -> Result<Vec<ProcessRow>, String> {
    use std::sync::atomic::{AtomicBool, Ordering};

    static READING: AtomicBool = AtomicBool::new(false);

    struct Sampling;
    impl Drop for Sampling {
        fn drop(&mut self) {
            READING.store(false, Ordering::Release);
        }
    }

    if within.is_zero() {
        return Err("/proc had no listing time left".to_string());
    }
    if READING.swap(true, Ordering::Acquire) {
        return Err("/proc is still being sampled".to_string());
    }
    let sampling = Sampling;
    let started = std::time::Instant::now();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("zo-proc-listing".to_string())
        .spawn(move || {
            let result = read_from(
                std::path::Path::new("/proc"),
                within.saturating_sub(started.elapsed()),
            );
            drop(sampling);
            let _ = send.send(result);
        })
        .map_err(|error| format!("/proc sampler did not start: {error}"))?;
    receive
        .recv_timeout(within.saturating_sub(started.elapsed()))
        .map_err(|error| {
            format!(
                "/proc did not finish within {} ms: {error}",
                within.as_millis()
            )
        })?
}

/// Any listed process that disappears or cannot be read invalidates the
/// snapshot. Decisions about groups and patterns require the entire table.
#[cfg(any(target_os = "linux", all(unix, test)))]
fn read_from(root: &std::path::Path, within: Duration) -> Result<Vec<ProcessRow>, String> {
    let started = std::time::Instant::now();
    let check_time = || {
        if started.elapsed() >= within {
            Err(format!(
                "/proc did not finish within {} ms",
                within.as_millis()
            ))
        } else {
            Ok(())
        }
    };
    check_time()?;
    let entries =
        std::fs::read_dir(root).map_err(|error| format!("/proc did not list: {error}"))?;
    let mut rows = Vec::new();
    let mut remaining = LISTING_MAX_BYTES;
    for entry in entries {
        check_time()?;
        let entry = entry.map_err(|error| format!("/proc entry did not read: {error}"))?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let directory = entry.path();
        let stat = listing_file(&directory.join("stat"), &mut remaining)?;
        check_time()?;
        let cmdline = listing_file(&directory.join("cmdline"), &mut remaining)?;
        let stat = std::str::from_utf8(&stat)
            .map_err(|error| format!("/proc/{pid}/stat did not read: {error}"))?;
        if !stat.ends_with('\n') || (!cmdline.is_empty() && !cmdline.ends_with(&[0])) {
            return Err(format!("/proc/{pid} gave a truncated row"));
        }
        let row =
            row(pid, stat, &cmdline).ok_or_else(|| format!("/proc/{pid} gave an invalid row"))?;
        rows.push(row);
    }
    check_time()?;
    if rows.is_empty() {
        return Err("/proc listed no processes".to_string());
    }
    Ok(rows)
}

#[cfg(any(target_os = "linux", all(unix, test)))]
fn listing_file(path: &std::path::Path, remaining: &mut u64) -> Result<Vec<u8>, String> {
    use std::io::Read as _;

    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| file.take(*remaining + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("{} did not read: {error}", path.display()))?;
    *remaining = remaining
        .checked_sub(bytes.len() as u64)
        .ok_or_else(|| format!("/proc exceeded {LISTING_MAX_BYTES} bytes"))?;
    Ok(bytes)
}

/// No `/proc` here: the table does not read.
#[cfg(not(target_os = "linux"))]
pub(super) fn read(_within: Duration) -> Result<Vec<ProcessRow>, String> {
    Err("this host has no /proc to read its processes from".to_string())
}

#[cfg(test)]
mod tests {
    use super::row;

    fn stat_row(prefix: &str) -> String {
        format!("{prefix}{}\n", " 0".repeat(super::STAT_FIELDS_AFTER_PGID))
    }

    #[test]
    fn a_stat_line_reads_into_the_row_ps_would_print() {
        let stat = stat_row("799 (python3) S 1 799");
        let row = row(799, &stat, b"python3\0/app/server.py\0--port\08080\0").expect("a row");
        assert_eq!((row.pid, row.ppid, row.pgid), (799, 1, 799));
        assert_eq!(row.command, "python3 /app/server.py --port 8080");
        assert_eq!(row.name(), "python3");
    }

    #[test]
    fn a_name_with_spaces_and_parentheses_still_finds_the_parent_and_group() {
        let stat = stat_row("42 (my (odd) prog) R 7 40");
        let row = row(42, &stat, b"").expect("a row");
        assert_eq!((row.pid, row.ppid, row.pgid), (42, 7, 40));
        assert_eq!(
            row.command, "[my (odd) prog]",
            "an empty command line prints the name in brackets"
        );
    }

    #[test]
    fn a_stat_that_does_not_read_gives_no_row() {
        assert!(row(1, "", b"").is_none());
        assert!(row(1, "1 (init) S", b"init\0").is_none());
        assert!(row(1, "1 (init) S x 1", b"init\0").is_none());
        assert!(row(1, "2 (init) S 0 1", b"init\0").is_none());
    }

    #[cfg(unix)]
    fn fake_proc() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let pid = directory.path().join("42");
        std::fs::create_dir(&pid).unwrap();
        std::fs::write(pid.join("stat"), stat_row("42 (example) S 1 42")).unwrap();
        std::fs::write(pid.join("cmdline"), b"example\0").unwrap();
        directory
    }

    #[cfg(unix)]
    #[test]
    fn friction_a_missing_or_truncated_row_invalidates_the_table() {
        let within = std::time::Duration::from_secs(1);
        let directory = fake_proc();
        assert!(super::read_from(directory.path(), within).is_ok());
        let pid = directory.path().join("42");
        std::fs::write(pid.join("cmdline"), b"example").unwrap();
        assert!(super::read_from(directory.path(), within).is_err());
        std::fs::write(pid.join("cmdline"), b"").unwrap();
        assert!(
            super::read_from(directory.path(), within).is_ok(),
            "a zombie has an empty cmdline"
        );
        std::fs::remove_file(pid.join("cmdline")).unwrap();
        assert!(super::read_from(directory.path(), within).is_err());
        std::fs::write(pid.join("cmdline"), b"example\0").unwrap();
        std::fs::write(pid.join("stat"), "42 (example) S 1 42\n").unwrap();
        assert!(super::read_from(directory.path(), within).is_err());
        std::fs::remove_file(pid.join("stat")).unwrap();
        assert!(super::read_from(directory.path(), within).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn friction_proc_cannot_exceed_the_listing_time_or_size() {
        let directory = fake_proc();
        let error = super::read_from(directory.path(), std::time::Duration::ZERO).unwrap_err();
        assert!(error.contains("did not finish"));
        let cmdline = directory.path().join("42/cmdline");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(cmdline)
            .unwrap();
        file.set_len(super::LISTING_MAX_BYTES + 1).unwrap();
        let error =
            super::read_from(directory.path(), std::time::Duration::from_secs(2)).unwrap_err();
        assert!(error.contains("exceeded"));
    }

    /// On Linux the table this reads holds this test's own process, with the
    /// parent and group the kernel gives it.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_table_read_from_proc_holds_this_process_with_its_parent_and_group() {
        let me = std::process::id();
        let rows = super::read(std::time::Duration::from_secs(1)).expect("/proc lists");
        let mine = rows
            .iter()
            .find(|row| row.pid == me)
            .expect("this process is listed");
        assert_eq!(mine.ppid, std::os::unix::process::parent_id());
        let group = u32::try_from(nix::unistd::getpgrp().as_raw()).expect("pgid fits");
        assert_eq!(mine.pgid, group);
    }
}
