//! The last check each saved model connection had, kept across window starts
//! (t-76500): the settings page draws 「연결됨 · 3시간 전 확인」 from it instead
//! of 「아직 확인 안 함」, and knows which connections are due a quiet check.
//!
//! One record per connection, by its id (ZO m-76513 ①): the state word, the
//! model count, when, the connection generation it was made against, and the
//! judgment checks of its models. Only words from [`Verdict`] are kept — never
//! a key, a header or a server's own words — so nothing secret can reach the
//! file. A record made against another generation says the setting changed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::durable_file;

pub const VERSION: u32 = 1;

/// How long a check stands before the window checks again without a press:
/// one model-list read per connection per six hours at most, whatever the
/// answer was (t-76500 B).
pub const QUIET_EVERY_MS: u64 = 6 * 60 * 60 * 1000;

/// A larger file is not ours to trust; it reads as no record. A write keeps
/// to the same limit, so a write that succeeded is always read back.
const MAX_BYTES: u64 = 256 * 1024;
const MAX_CONNECTIONS: usize = 64;
const MAX_JUDGMENTS: usize = 256;
const MAX_MODEL_CHARS: usize = 256;
const MAX_PROTOCOL_CHARS: usize = 32;

/// One read-modify-write at a time: two checks answering together must not
/// cost either its record.
static WRITING: Mutex<()> = Mutex::new(());

/// The states a check ends in that are worth keeping — the page's
/// `MODEL_STATES` words for an answer that came back. A check still running
/// is not an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Ok,
    Usable,
    Nokey,
    Unreadable,
    Badkey,
    Down,
    Schema,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Judgment {
    pub model: String,
    pub protocol: String,
    pub state: Verdict,
    pub checked_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<Verdict>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub judgments: Vec<Judgment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct File {
    version: u32,
    connections: BTreeMap<String, Record>,
}

/// What the page reads: the records, the clock they are read against, and
/// how long one stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Remembered {
    pub now_ms: u64,
    pub every_ms: u64,
    pub connections: BTreeMap<String, Record>,
}

/// A connection check, as the page answers it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckAnswer {
    pub id: String,
    pub generation: u64,
    pub state: Verdict,
    #[serde(default)]
    pub count: Option<u64>,
    /// A press checks the connection again so its models can be asked for
    /// judgment again: their kept judgment checks go with it. A quiet check
    /// keeps them.
    #[serde(default)]
    pub forget_judgments: bool,
}

/// A judgment check of one model of a connection, as the page answers it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JudgmentAnswer {
    pub id: String,
    pub generation: u64,
    pub model: String,
    pub protocol: String,
    pub state: Verdict,
}

fn read_file(path: &Path) -> BTreeMap<String, Record> {
    let Ok(meta) = std::fs::metadata(path) else {
        return BTreeMap::new();
    };
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return BTreeMap::new();
    }
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<File>(&bytes).ok())
        .filter(|file| file.version == VERSION)
        .map(|file| file.connections)
        .unwrap_or_default()
}

/// The records as the file holds them, read against `now_ms`. A missing,
/// unreadable or foreign file is no record.
#[must_use]
pub fn read(path: &Path, now_ms: u64) -> Remembered {
    Remembered {
        now_ms,
        every_ms: QUIET_EVERY_MS,
        connections: read_file(path),
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-' || ch == b'_')
}

/// What a write is making and the trim must not take: the judgment being
/// written, and with it the newest judgment of its connection.
struct Keep<'a> {
    id: &'a str,
    model: &'a str,
    protocol: &'a str,
}

/// Trim the oldest judgment checks until the file is one the next read
/// accepts, never the whole record. The clock is the system's and can be set
/// back, so the judgment being written (and the newest one of its connection)
/// is not a candidate however old its time looks: the others go first. When
/// that is not enough the write fails, so it never answers 「kept」 for a
/// judgment it dropped.
fn fit(file: &mut File, limit: u64, keep: Option<&Keep>) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(file).map_err(|error| error.to_string())?;
    while bytes.len() as u64 > limit {
        let oldest = file
            .connections
            .iter()
            .flat_map(|(id, record)| {
                let newest = record.judgments.iter().map(|kept| kept.checked_ms).max();
                record
                    .judgments
                    .iter()
                    .enumerate()
                    .filter(move |(_, kept)| {
                        !keep.is_some_and(|keep| {
                            keep.id == id
                                && (Some(kept.checked_ms) == newest
                                    || (kept.model == keep.model && kept.protocol == keep.protocol))
                        })
                    })
                    .map(move |(index, kept)| (kept.checked_ms, id, index))
            })
            .min_by_key(|(checked_ms, _, _)| *checked_ms)
            .map(|(_, id, index)| (id.clone(), index));
        let Some((id, index)) = oldest else {
            return Err("model_connection_checks_full".to_string());
        };
        if let Some(record) = file.connections.get_mut(&id) {
            record.judgments.remove(index);
        }
        bytes = serde_json::to_vec(file).map_err(|error| error.to_string())?;
    }
    Ok(bytes)
}

fn write(
    path: &Path,
    live: &BTreeSet<String>,
    now_ms: u64,
    keep: Option<&Keep>,
    change: impl FnOnce(&mut BTreeMap<String, Record>),
) -> Result<Remembered, String> {
    let _held = WRITING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut connections = read_file(path);
    change(&mut connections);
    // A connection that is no longer saved takes its record with it.
    connections.retain(|id, _| live.contains(id));
    if connections.len() > MAX_CONNECTIONS {
        return Err("model_connection_checks_full".to_string());
    }
    let mut file = File {
        version: VERSION,
        connections,
    };
    // The counts and lengths above do not bound the bytes (a model name of
    // 256 wide characters is 1 KiB).
    let bytes = fit(&mut file, MAX_BYTES, keep)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    durable_file::replace_bytes(path, &bytes).map_err(|error| error.to_string())?;
    Ok(Remembered {
        now_ms,
        every_ms: QUIET_EVERY_MS,
        connections: file.connections,
    })
}

/// The record a check of another generation leaves: the setting changed, so
/// nothing kept about the old one says anything about this one.
fn record_for<'a>(
    connections: &'a mut BTreeMap<String, Record>,
    id: &str,
    generation: u64,
) -> &'a mut Record {
    let record = connections.entry(id.to_string()).or_default();
    if record.generation != generation {
        *record = Record {
            generation,
            ..Record::default()
        };
    }
    record
}

/// Keep a connection check's answer, stamped `now_ms`.
pub fn remember_check(
    path: &Path,
    answer: &CheckAnswer,
    live: &BTreeSet<String>,
    now_ms: u64,
) -> Result<Remembered, String> {
    if !valid_id(&answer.id) || answer.state == Verdict::Usable {
        return Err("model_connection_check".to_string());
    }
    write(path, live, now_ms, None, |connections| {
        let record = record_for(connections, &answer.id, answer.generation);
        record.state = Some(answer.state);
        record.count = answer.count.filter(|_| answer.state == Verdict::Ok);
        record.checked_ms = Some(now_ms);
        if answer.forget_judgments {
            record.judgments.clear();
        }
    })
}

/// Keep a judgment check's answer for one model of a connection, stamped
/// `now_ms`; the model's earlier answer in the same format is replaced.
pub fn remember_judgment(
    path: &Path,
    answer: &JudgmentAnswer,
    live: &BTreeSet<String>,
    now_ms: u64,
) -> Result<Remembered, String> {
    let protocol_ok = !answer.protocol.is_empty()
        && answer.protocol.len() <= MAX_PROTOCOL_CHARS
        && answer
            .protocol
            .bytes()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == b'_');
    let model_ok = !answer.model.is_empty()
        && answer.model.chars().count() <= MAX_MODEL_CHARS
        && !answer.model.chars().any(char::is_control);
    if !valid_id(&answer.id) || !protocol_ok || !model_ok || answer.state == Verdict::Ok {
        return Err("model_connection_check".to_string());
    }
    let keep = Keep {
        id: &answer.id,
        model: &answer.model,
        protocol: &answer.protocol,
    };
    write(path, live, now_ms, Some(&keep), |connections| {
        let record = record_for(connections, &answer.id, answer.generation);
        record
            .judgments
            .retain(|kept| !(kept.model == answer.model && kept.protocol == answer.protocol));
        record.judgments.push(Judgment {
            model: answer.model.clone(),
            protocol: answer.protocol.clone(),
            state: answer.state,
            checked_ms: now_ms,
        });
        let over = record.judgments.len().saturating_sub(MAX_JUDGMENTS);
        record.judgments.drain(..over);
    })
}

#[cfg(test)]
#[path = "model_connection_checks_tests.rs"]
mod tests;
