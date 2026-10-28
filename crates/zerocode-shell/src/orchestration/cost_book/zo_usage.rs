use std::collections::BTreeSet;
use std::io::Read;

use zerocode_core::orchestration::Ledger;
use zerocode_core::orchestration::task_cost::{
    ReadGaps, SessionBook, ZoRequestUsage, reported_sessions,
};

pub(super) fn read(ledger: &Ledger, sessions: &mut SessionBook, now_ms: i64) {
    sessions.clear_source(zerocode_core::orchestration::task_cost::UsageSource::Zo);
    let Some(settings) = crate::api_routers::zo_settings_path() else {
        return;
    };
    let Some(home) = settings.parent() else {
        return;
    };
    let mut ids = BTreeSet::new();
    for run in ledger.runs() {
        for attempt in &run.dispatches {
            if run
                .worker(&attempt.worker)
                .is_none_or(|worker| worker.agent != "zo")
            {
                continue;
            }
            for session in reported_sessions(run, attempt) {
                if session.id.is_empty()
                    || !session
                        .id
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
                    || !ids.insert(session.id.clone())
                {
                    continue;
                }
                let file = home
                    .join("cache/prompt-cache")
                    .join(&session.id)
                    .join("requests.jsonl");
                let Some(journal) = read_rows(&file) else {
                    continue;
                };
                sessions.read_zo_usage(&session.id, &journal.rows, journal.gaps, now_ms);
            }
        }
    }
}

/// The most bytes of one session's request journal read at once.
const READ_LIMIT: u64 = 16 * 1024 * 1024;

/// The rows read from one session's request journal and what was left out.
struct Journal {
    rows: Vec<ZoRequestUsage>,
    gaps: ReadGaps,
}

fn read_rows(file: &std::path::Path) -> Option<Journal> {
    read_rows_within(file, READ_LIMIT)
}

fn read_rows_within(file: &std::path::Path, cap: u64) -> Option<Journal> {
    let file = crate::durable_file::open_plain_file(file).ok()?;
    if file.metadata().ok()?.len() > cap {
        return None;
    }
    let mut contents = String::new();
    file.take(cap + 1).read_to_string(&mut contents).ok()?;
    if contents.len() as u64 > cap {
        return None;
    }
    let rows = contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .ok()?;
    Some(Journal {
        rows,
        gaps: ReadGaps::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(seq: u64) -> String {
        format!(
            "{{\"seq\":{seq},\"ts_unix_ms\":{},\"model\":\"gpt-5.6-sol\",\"input_uncached\":10,\"cache_read\":20,\"cache_creation\":0,\"output\":5}}\n",
            1_000 + seq
        )
    }

    fn journal(contents: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let folder = tempfile::tempdir().expect("a temp folder");
        let file = folder.path().join("requests.jsonl");
        std::fs::write(&file, contents).expect("the journal");
        (folder, file)
    }

    fn seqs(journal: &Journal) -> Vec<u64> {
        journal.rows.iter().map(|row| row.seq).collect()
    }

    /// A journal longer than the limit is read from its end: the newest rows
    /// come back whole, the piece of a row the window starts inside does not,
    /// and the journal says its oldest rows were left out.
    #[test]
    fn a_journal_over_the_limit_is_read_from_its_end() {
        let text: String = (1..=200).map(row).collect();
        let (_folder, file) = journal(text.as_bytes());
        let limit = 40 * row(100).len() as u64 + 7;
        let read = read_rows_within(&file, limit).expect("the newest rows");
        let seqs = seqs(&read);
        assert_eq!(seqs.last(), Some(&200));
        assert!((39..=40).contains(&seqs.len()), "{} rows", seqs.len());
        assert!(seqs.windows(2).all(|pair| pair[1] == pair[0] + 1));
        assert_eq!(
            read.gaps,
            ReadGaps {
                older_rows_dropped: true,
                skipped_lines: 0
            }
        );
    }

    /// One line that is not a usage row costs that line and nothing else.
    #[test]
    fn a_broken_line_costs_only_itself() {
        let text = format!("{}not json\n{}{}", row(1), row(2), row(3));
        let (_folder, file) = journal(text.as_bytes());
        let read = read_rows_within(&file, READ_LIMIT).expect("the other rows");
        assert_eq!(seqs(&read), [1, 2, 3]);
        assert_eq!(
            read.gaps,
            ReadGaps {
                older_rows_dropped: false,
                skipped_lines: 1
            }
        );
        let mut bytes = row(1).into_bytes();
        bytes.extend_from_slice(b"\xff\xfe\n");
        bytes.extend_from_slice(row(2).as_bytes());
        let (_folder, file) = journal(&bytes);
        let read = read_rows_within(&file, READ_LIMIT).expect("the other rows");
        assert_eq!(seqs(&read), [1, 2]);
        assert_eq!(read.gaps.skipped_lines, 1);
    }

    /// A whole journal is whole; so is one whose last row is still being
    /// written — it is not a skipped line, it is not a row yet.
    #[test]
    fn a_whole_journal_and_a_row_in_the_middle_of_being_written_leave_no_gap() {
        let text = format!("{}{}", row(1), row(2));
        let (_folder, file) = journal(text.as_bytes());
        let read = read_rows_within(&file, READ_LIMIT).expect("rows");
        assert_eq!((seqs(&read), read.gaps), (vec![1, 2], ReadGaps::default()));
        let torn = format!("{}{}{{\"seq\":3,\"ts_un", row(1), row(2));
        let (_folder, file) = journal(torn.as_bytes());
        let read = read_rows_within(&file, READ_LIMIT).expect("rows");
        assert_eq!((seqs(&read), read.gaps), (vec![1, 2], ReadGaps::default()));
    }
}
