//! A bounded, cancellable index of a zo transcript and its compacted history.
//! Only record locations survive a scan. A selected page is decoded on demand.

use std::collections::BTreeMap;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc};

use zerocode_core::transcript::{Detail, TranscriptTurn};
use super::transcript::Entry;
use crate::session::plain_session::ReplayItem;

const RECORD_BYTES: u64 = 32 * 1024 * 1024;
const INDEX_RECORDS: usize = 100_000;
const PAGE_RECORDS: usize = 32;
const PAGE_CHARS: usize = 64 * 1024;

#[derive(Debug, Clone)]
struct Record {
    path: PathBuf,
    offset: u64,
    bytes: u64,
    part: usize,
    hash: u64,
    user: bool,
    matched: bool,
}

#[derive(Debug)]
pub(crate) struct Index {
    records: Vec<Record>,
    pub(crate) matches: Vec<usize>,
    pub(crate) turns: Vec<usize>,
    pub(crate) limited: bool,
}

impl Index {
    pub(crate) fn len(&self) -> usize { self.records.len() }
}

#[derive(Debug)]
pub(crate) enum Reply {
    Index(Arc<Index>),
    Page { entries: Vec<Entry>, start: usize, next: usize },
}

#[derive(Debug)]
pub(crate) struct Job {
    receiver: mpsc::Receiver<Result<Reply, String>>,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Job {
    fn drop(&mut self) { self.cancelled.store(true, Ordering::Relaxed); }
}

impl Job {
    fn spawn(work: impl FnOnce(&AtomicBool) -> Result<Reply, String> + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancelled);
        let _ = std::thread::Builder::new().name("zo-history-reader".into()).spawn(move || {
            let result = work(&stop);
            if !stop.load(Ordering::Relaxed) { let _ = sender.send(result); }
        });
        Self { receiver, cancelled }
    }

    pub(crate) fn scan(path: PathBuf, query: String) -> Self {
        Self::spawn(move |stop| scan(&path, &query, stop).map(|index| Reply::Index(Arc::new(index))))
    }

    pub(crate) fn page(index: Arc<Index>, start: usize, query: String) -> Self {
        Self::spawn(move |stop| read_page(&index, start, &query, stop))
    }

    pub(crate) fn poll(&self) -> Option<Result<Reply, String>> {
        match self.receiver.try_recv() {
            Ok(reply) => Some(reply),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("history reader stopped".into())),
        }
    }
}

fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hash);
    hash.finish()
}

/// Return the consumed length even for an oversized or torn record, keeping
/// both memory and forward progress bounded.
fn next_record(reader: &mut impl BufRead, stop: &AtomicBool) -> Result<(Vec<u8>, u64, bool), String> {
    let mut bytes = Vec::new();
    let read = reader.by_ref().take(RECORD_BYTES + 1).read_until(b'\n', &mut bytes).map_err(|error| error.to_string())?;
    let mut consumed = read as u64;
    if read == 0 || bytes.last() == Some(&b'\n') { return Ok((bytes, consumed, false)); }
    let oversized = consumed > RECORD_BYTES;
    if oversized {
        bytes.clear();
        loop {
            if stop.load(Ordering::Relaxed) { return Err("history read cancelled".into()); }
            let read = reader.by_ref().take(32 * 1024).read_until(b'\n', &mut bytes).map_err(|error| error.to_string())?;
            consumed += read as u64;
            if read == 0 || bytes.last() == Some(&b'\n') { break; }
            bytes.clear();
        }
    }
    // A final record with no newline may still be being written.
    Ok((Vec::new(), consumed, true))
}

fn scan(path: &Path, query: &str, stop: &AtomicBool) -> Result<Index, String> {
    let mut records = BTreeMap::new();
    let mut floor = 0_u64;
    let needle = query.to_lowercase();
    let mut limited = false;
    for (source, vault) in [(path.to_path_buf(), false), (core_types::session::vault_path_for(path), true)] {
        let file = match File::open(&source) {
            Ok(file) => file,
            Err(error) if vault && error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.to_string()),
        };
        let size = file.metadata().map_err(|error| error.to_string())?.len();
        let mut reader = BufReader::new(file.take(size));
        let mut offset = 0;
        loop {
            if stop.load(Ordering::Relaxed) { return Err("history read cancelled".into()); }
            let (bytes, consumed, skipped) = next_record(&mut reader, stop)?;
            if consumed == 0 { break; }
            let start = offset;
            offset += consumed;
            limited |= skipped;
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue; };
            if !vault && value.get("type").and_then(serde_json::Value::as_str) == Some("session_meta") {
                floor = value.pointer("/compaction/first_kept_message_index").and_then(serde_json::Value::as_u64).unwrap_or(0);
            }
            let Some(sequence) = value.get(if vault { "vault_seq" } else { "turn_index" }).and_then(serde_json::Value::as_u64) else { continue; };
            if vault && sequence >= floor { continue; }
            let text = String::from_utf8_lossy(&bytes);
            for (part, turn) in zerocode_core::transcript::turns_in_with(&text, Detail::Whole).into_iter().enumerate() {
                let matched = !needle.is_empty() && [turn.text.as_str(), turn.tool.as_ref().map_or("", |tool| tool.input.as_str())]
                    .iter().any(|text| text.to_lowercase().contains(&needle));
                records.insert((sequence, part), Record {
                    path: source.clone(), offset: start, bytes: consumed, part,
                    hash: fingerprint(&bytes), user: turn.role == "user", matched,
                });
                if records.len() > INDEX_RECORDS { records.pop_first(); limited = true; }
            }
        }
    }
    let records: Vec<_> = records.into_values().collect();
    let matches = records.iter().enumerate().filter_map(|(at, record)| record.matched.then_some(at)).collect();
    let turns = records.iter().enumerate().filter_map(|(at, record)| record.user.then_some(at)).collect();
    Ok(Index { records, matches, turns, limited })
}

fn excerpt(text: &str, query: &str, budget: usize) -> String {
    if text.chars().count() <= budget { return text.to_string(); }
    let needle = query.to_lowercase();
    let at = if needle.is_empty() { 0 } else { text.to_lowercase().find(&needle).unwrap_or(0).min(text.len()) };
    let mut start = at.saturating_sub(budget / 4);
    while !text.is_char_boundary(start) { start -= 1; }
    let body: String = text[start..].chars().take(budget).collect();
    format!("[stored record excerpt; original text remains in the saved record]\n{body}\n[…]")
}

fn entry(turn: TranscriptTurn, query: &str) -> Option<Entry> {
    let text = excerpt(&turn.text, query, PAGE_CHARS / 2);
    match turn.role.as_str() {
        "user" => Some(Entry::Replay(ReplayItem::User(text))),
        "assistant" => Some(Entry::Replay(ReplayItem::Assistant(text))),
        "thinking" => Some(Entry::Reasoning(text)),
        "tool" | "tool_result" => {
            let tool = turn.tool?;
            Some(Entry::Replay(ReplayItem::ToolCall {
                name: tool.name,
                input: if turn.role == "tool" { excerpt(&tool.input, query, PAGE_CHARS / 2) } else { String::new() },
                output: (turn.role == "tool_result").then_some(text),
                is_error: tool.is_error,
            }))
        }
        _ => None,
    }
}

fn read_page(index: &Index, start: usize, query: &str, stop: &AtomicBool) -> Result<Reply, String> {
    let mut entries = Vec::new();
    let mut chars = 0;
    let mut next = start;
    for record in index.records.iter().skip(start).take(PAGE_RECORDS) {
        if stop.load(Ordering::Relaxed) { return Err("history read cancelled".into()); }
        let mut file = File::open(&record.path).map_err(|error| error.to_string())?;
        file.seek(SeekFrom::Start(record.offset)).map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.take(record.bytes).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
        if fingerprint(&bytes) != record.hash { return Err("saved history changed; search again".into()); }
        let text = String::from_utf8_lossy(&bytes);
        if let Some(turn) = zerocode_core::transcript::turns_in_with(&text, Detail::Whole).into_iter().nth(record.part) {
            if let Some(item) = entry(turn, query) {
                chars += format!("{item:?}").len();
                entries.push(item);
            }
        }
        next += 1;
        if chars >= PAGE_CHARS { break; }
    }
    Ok(Reply::Page { entries, start, next })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_evicted_history_without_retaining_tool_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let user = |kind: &str, key: &str, index: u64, text: &str| {
            let mut record = serde_json::json!({"type":kind,"message":{"role":"user","blocks":[{"type":"text","text":text}]}});
            record[key] = index.into();
            format!("{record}\n")
        };
        std::fs::write(&path, format!("{{\"type\":\"session_meta\",\"compaction\":{{\"first_kept_message_index\":10}}}}\n{}", user("message", "turn_index", 10, "newest"))).unwrap();
        std::fs::write(core_types::session::vault_path_for(&path), user("vault", "vault_seq", 0, "오래된 needle 기록")).unwrap();
        let index = scan(&path, "needle", &AtomicBool::new(false)).unwrap();
        assert_eq!(index.matches, [0]);
        assert_eq!(index.turns, [0, 1]);
        let Reply::Page { entries, .. } = read_page(&index, 0, "needle", &AtomicBool::new(false)).unwrap() else { panic!("page"); };
        assert!(format!("{entries:?}").contains("오래된 needle"));
        std::fs::write(&path, "changed").unwrap();
        assert!(read_page(&index, 1, "", &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn excerpts_preserve_a_match_beyond_the_display_budget() {
        let text = format!("{}needle{}", "한글".repeat(50_000), "끝".repeat(50_000));
        let kept = excerpt(&text, "needle", PAGE_CHARS / 2);
        assert!(kept.contains("needle"));
        assert!(kept.chars().count() < PAGE_CHARS);
    }
}
