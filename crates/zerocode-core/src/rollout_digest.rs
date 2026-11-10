//! The part of a Codex rollout the usage scans read — a rollout's digest.
//!
//! Two scans read every rollout on the disk: the token panel's
//! (`codex_tokens`) and the usage ledger's ([`crate::usage_stats_codex`]).
//! Both read three kinds of record and a few fields of each, and nothing
//! else: the header that names the session and its folder, the turn headers
//! that move the folder and the model, and the `token_count` events with
//! their two usage objects. Measured on one machine on 2026-11-10 — 790
//! rollouts, 1,178 MB as Codex compressed them, 3,785 MB decoded — those
//! records are 92,596 lines of the corpus, and what the scans read OF them is
//! a fraction of that again: a `session_meta` line carries the session's whole
//! instructions (21 KB on average), a `token_count` line its rate limits.
//!
//! A rollout at rest does not change — Codex appends while a session runs
//! and, since 0.164, compresses the file where it lies once it has been idle a
//! week — so what the scans read of it can be read once. This module is the
//! pure half: the line a record is cut down to, and the stamp a kept digest
//! is checked against. Finding the files and keeping the digests is the
//! window's (`zerocode-shell`, `token_scan`).
//!
//! ## What a digest line is
//!
//! The record PRUNED, never rebuilt: the same JSON with only the paths either
//! reader takes, each value as the rollout wrote it. Both readers parse a
//! line into a tree and ask it for named paths, so a tree that answers those
//! paths alike is the same line to them — which is what makes the rule
//! checkable: every path a reader asks for is in [`digest_line`], and a
//! reader that starts asking for another adds it here and bumps [`VERSION`].
//!
//! - top: `timestamp`, `type`, `payload`
//! - `payload`: `type`, `id`, `session_id`, `cwd`, `model`, `model_name`,
//!   `metadata.model`, `info`
//! - `info`: `total_token_usage` and `last_token_usage` whole, `model`,
//!   `model_name`, `metadata.model`
//!
//! Two shapes are kept because the readers tell them apart. An `info` that is
//! there and is not an object — a rate-limit ping writes `null` — stays as
//! `null`: one reader counts that event and folds it, and an absent `info` is
//! not counted at all. And a line is kept when its `type` is one of the two
//! headers OR its payload is a `token_count`, whatever the `type` beside it:
//! one reader asks for `event_msg` there and the other does not.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The format of a kept digest. A digest written under another number is not
/// read: it is made again.
pub const VERSION: u32 = 1;

/// The three spellings a line carries when it is one of the records the scans
/// read. A line with none of them is none of them, and is not parsed.
pub const MARKS: [&str; 3] = ["\"token_count\"", "\"turn_context\"", "\"session_meta\""];

const SESSION_META: &str = "session_meta";
const TURN_CONTEXT: &str = "turn_context";
const TOKEN_COUNT: &str = "token_count";

/// The paths kept of a payload, beside its `metadata` and its `info`.
const PAYLOAD_KEYS: [&str; 6] = ["type", "id", "session_id", "cwd", "model", "model_name"];
/// The paths kept of a `token_count`'s `info`, beside its `metadata`.
const INFO_KEYS: [&str; 4] = [
    "total_token_usage",
    "last_token_usage",
    "model",
    "model_name",
];

/// What a rollout's line is in its digest: the record pruned to what the
/// scans read, or nothing when the scans read nothing of it.
///
/// `line` is one line of the rollout, with or without its line break.
#[must_use]
pub fn digest_line(line: &str) -> Option<String> {
    if !MARKS.iter().any(|mark| line.contains(mark)) {
        return None;
    }
    let Value::Object(record) = serde_json::from_str::<Value>(line).ok()? else {
        return None;
    };
    let kind = record.get("type").and_then(Value::as_str);
    let payload = record.get("payload");
    let counts_tokens = payload
        .and_then(|held| held.get("type"))
        .and_then(Value::as_str)
        == Some(TOKEN_COUNT);
    if !matches!(kind, Some(SESSION_META | TURN_CONTEXT)) && !counts_tokens {
        return None;
    }
    let mut kept = Map::new();
    for name in ["timestamp", "type"] {
        if let Some(value) = record.get(name) {
            kept.insert(name.to_string(), value.clone());
        }
    }
    if let Some(payload) = payload {
        kept.insert("payload".to_string(), pruned_payload(payload));
    }
    serde_json::to_string(&Value::Object(kept)).ok()
}

/// A payload with only the paths the scans read. One that is not an object
/// has none of them, and is kept as it is: there and not an object is what
/// the readers see either way.
fn pruned_payload(payload: &Value) -> Value {
    let Value::Object(held) = payload else {
        return payload.clone();
    };
    let mut kept = Map::new();
    for name in PAYLOAD_KEYS {
        if let Some(value) = held.get(name) {
            kept.insert(name.to_string(), value.clone());
        }
    }
    if let Some(model) = model_under(held.get("metadata")) {
        kept.insert("metadata".to_string(), model);
    }
    if let Some(info) = held.get("info") {
        kept.insert("info".to_string(), pruned_info(info));
    }
    Value::Object(kept)
}

/// A `token_count`'s `info` with only the paths the scans read — and `null`
/// for one that is there and is not an object, which both readers read as
/// "present, carrying nothing".
fn pruned_info(info: &Value) -> Value {
    let Value::Object(held) = info else {
        return Value::Null;
    };
    let mut kept = Map::new();
    for name in INFO_KEYS {
        if let Some(value) = held.get(name) {
            kept.insert(name.to_string(), value.clone());
        }
    }
    if let Some(model) = model_under(held.get("metadata")) {
        kept.insert("metadata".to_string(), model);
    }
    Value::Object(kept)
}

/// `{"model": …}` of a `metadata` object that names one.
fn model_under(metadata: Option<&Value>) -> Option<Value> {
    let model = metadata?.as_object()?.get("model")?;
    let mut kept = Map::new();
    kept.insert("model".to_string(), model.clone());
    Some(Value::Object(kept))
}

/// What a kept digest was made from: the rollout's size and the moment it was
/// last written. A rollout whose stamp differs has changed, and its digest is
/// made again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    /// The rollout's length in bytes, as it lies on the disk.
    pub len: u64,
    /// Its modification time, in nanoseconds since the epoch.
    pub modified_ns: u64,
}

/// The first line of a kept digest: what it was made from, and of what.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Header {
    zerocode_rollout_digest: u32,
    len: u64,
    modified_ns: u64,
    /// The rollout's own path, so a digest whose rollout has gone can be
    /// found and removed, and one filed under a colliding name is not read.
    source: String,
}

/// The header line of a digest made from `source` as `stamp` describes it.
#[must_use]
pub fn header_line(stamp: Stamp, source: &str) -> String {
    serde_json::to_string(&Header {
        zerocode_rollout_digest: VERSION,
        len: stamp.len,
        modified_ns: stamp.modified_ns,
        source: source.to_string(),
    })
    .unwrap_or_default()
}

/// What a digest's first line says it was made from — the rollout's stamp and
/// path — or nothing for a line that is not a header of this [`VERSION`].
#[must_use]
pub fn read_header(line: &str) -> Option<(Stamp, String)> {
    let header: Header = serde_json::from_str(line).ok()?;
    (header.zerocode_rollout_digest == VERSION).then_some((
        Stamp {
            len: header.len,
            modified_ns: header.modified_ns,
        },
        header.source,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_stats_codex::{ReadState, read_record};

    /// A header the way Codex writes one: the session, its folder, and a
    /// great deal the scans never read.
    const META: &str = r#"{"timestamp":"2026-05-30T01:00:00.000Z","type":"session_meta","payload":{"id":"s-1","cwd":"/work/app","originator":"codex_cli_rs","cli_version":"0.164.0","instructions":"…long…","git":{"branch":"main"}}}"#;
    const CONTEXT: &str = r#"{"timestamp":"2026-05-30T01:00:01.000Z","type":"turn_context","payload":{"cwd":"/work/app/crates","model":"gpt-5.5","approval_policy":"never","sandbox_policy":{"mode":"workspace-write"},"summary":"auto"}}"#;
    const TOKENS: &str = r#"{"timestamp":"2026-05-30T01:00:02.000Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1200,"cached_input_tokens":1000,"output_tokens":80,"reasoning_output_tokens":30,"total_tokens":1280},"last_token_usage":{"input_tokens":1200,"cached_input_tokens":1000,"output_tokens":80,"reasoning_output_tokens":30,"total_tokens":1280},"model_context_window":272000},"rate_limits":{"primary":{"used_percent":12.5,"window_minutes":300}}}}"#;

    /// A record is pruned, not rebuilt: what is kept is what the rollout
    /// wrote, and what the scans never read is gone.
    #[test]
    fn a_record_is_cut_down_to_the_paths_the_scans_read() {
        let meta: Value = serde_json::from_str(&digest_line(META).expect("kept")).expect("JSON");
        assert_eq!(
            meta,
            serde_json::json!({
                "timestamp": "2026-05-30T01:00:00.000Z",
                "type": "session_meta",
                "payload": {"id": "s-1", "cwd": "/work/app"},
            })
        );
        let context: Value =
            serde_json::from_str(&digest_line(CONTEXT).expect("kept")).expect("JSON");
        assert_eq!(
            context["payload"],
            serde_json::json!({"cwd": "/work/app/crates", "model": "gpt-5.5"})
        );
        let tokens = digest_line(TOKENS).expect("kept");
        assert!(!tokens.contains("rate_limits") && !tokens.contains("model_context_window"));
        let tokens: Value = serde_json::from_str(&tokens).expect("JSON");
        assert_eq!(
            tokens["payload"]["info"]["last_token_usage"]["cached_input_tokens"],
            1000
        );
        assert!(digest_line(TOKENS).expect("kept").len() < TOKENS.len());
        // With its line break, as a reader hands it over.
        assert_eq!(digest_line(&format!("{META}\n")), digest_line(META));
    }

    /// Lines the scans read nothing of are not in a digest: every other
    /// record, a record that only quotes a mark, and what is not JSON.
    #[test]
    fn a_line_the_scans_read_nothing_of_is_left_out() {
        for line in [
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","content":"hi"}}"#,
            // A message that says the word is not the record.
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","content":"the \"token_count\" event"}}"#,
            r#"{"type":"event_msg","payload":{"type":"agent_message","message":"\"session_meta\""}}"#,
            r#"{"type":"session_meta","payload":{"id":"cut off"#,
            r#"["session_meta"]"#,
            "",
        ] {
            assert_eq!(digest_line(line), None, "{line}");
        }
    }

    /// The shapes the two readers tell apart survive the cut: an `info` that
    /// is there and empty, one that is absent, a `token_count` under another
    /// `type`, a payload that is not an object, and a model named in any of
    /// the places a record is known to put one.
    #[test]
    fn the_shapes_the_readers_tell_apart_are_kept_apart() {
        let said = |line: &str| -> Value {
            serde_json::from_str(&digest_line(line).expect("kept")).expect("JSON")
        };
        let ping = said(
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{}}}"#,
        );
        assert_eq!(ping["payload"].get("info"), Some(&Value::Null));
        let bare = said(r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count"}}"#);
        assert_eq!(bare["payload"].get("info"), None);
        let odd = said(
            r#"{"timestamp":"t","type":"event_msg","payload":{"type":"token_count","info":"soon"}}"#,
        );
        assert_eq!(odd["payload"].get("info"), Some(&Value::Null));
        let other = said(
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"token_count","info":{}}}"#,
        );
        assert_eq!(other["type"], "response_item");
        let stringly = said(r#"{"type":"session_meta","payload":"not an object"}"#);
        assert_eq!(stringly["payload"], "not an object");
        let headless = said(r#"{"type":"turn_context"}"#);
        assert_eq!(headless.get("payload"), None);

        for (payload, path) in [
            (r#"{"model_name":"m"}"#, "/payload/model_name"),
            (r#"{"info":{"model":"m"}}"#, "/payload/info/model"),
            (r#"{"info":{"model_name":"m"}}"#, "/payload/info/model_name"),
            (
                r#"{"info":{"metadata":{"model":"m","more":1}}}"#,
                "/payload/info/metadata/model",
            ),
            (
                r#"{"metadata":{"model":"m","more":1}}"#,
                "/payload/metadata/model",
            ),
        ] {
            let kept = said(&format!(r#"{{"type":"turn_context","payload":{payload}}}"#));
            assert_eq!(kept.pointer(path), Some(&Value::from("m")), "{payload}");
            assert!(!kept.to_string().contains("more"), "{payload}");
        }
    }

    /// One `token_count` event: a running total after `turns` turns of the
    /// same size, with everything a real one carries beside it.
    fn spent(second: u32, turns: u64) -> String {
        let usage = |times: u64| {
            format!(
                r#"{{"input_tokens":{},"cached_input_tokens":{},"output_tokens":{},"reasoning_output_tokens":{},"total_tokens":{}}}"#,
                1200 * times,
                1000 * times,
                80 * times,
                30 * times,
                1280 * times
            )
        };
        format!(
            r#"{{"timestamp":"2026-05-30T01:00:{second:02}.000Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{},"last_token_usage":{},"model_context_window":272000}},"rate_limits":{{"primary":{{"used_percent":12.5}}}}}}}}"#,
            usage(turns),
            usage(1)
        )
    }

    /// The ledger's reader reads a digest as it reads the rollout: the same
    /// increments, attributed alike, line for line — through a folder and a
    /// model that move, a rate-limit ping, a record repeated, and the lines
    /// in between that a digest does not hold.
    #[test]
    fn the_ledgers_reader_reads_a_digest_as_it_reads_the_rollout() {
        let rollout = [
            META.to_string(),
            r#"{"timestamp":"t","type":"response_item","payload":{"type":"message","content":"x"}}"#.to_string(),
            CONTEXT.to_string(),
            spent(2, 1),
            r#"{"timestamp":"2026-05-30T01:00:03.000Z","type":"event_msg","payload":{"type":"token_count","info":null}}"#.to_string(),
            spent(9, 2),
            // The same record again, as a resume writes it: no new spend.
            spent(9, 2),
            r#"{"timestamp":"2026-05-30T01:00:10.000Z","type":"turn_context","payload":{"model":"gpt-5.4-mini"}}"#.to_string(),
            spent(19, 3),
        ];
        let read = |lines: &mut dyn Iterator<Item = String>| {
            let mut state = ReadState {
                session_id: "fallback".to_string(),
                ..ReadState::default()
            };
            lines
                .filter_map(|line| read_record(&line, &mut state))
                .collect::<Vec<_>>()
        };
        let whole = read(&mut rollout.iter().cloned());
        let digest = read(&mut rollout.iter().filter_map(|line| digest_line(line)));
        assert_eq!(whole.len(), 3, "{whole:?}");
        // The entry has no equality of its own; its fields all print.
        assert_eq!(format!("{digest:?}"), format!("{whole:?}"));
        assert_eq!(whole[0].session_id, "s-1");
        assert_eq!(whole[0].cwd.as_deref(), Some("/work/app/crates"));
        assert_eq!(whole[1].total_tokens, 1280);
        assert_eq!(whole[2].model.as_deref(), Some("gpt-5.4-mini"));
    }

    /// A digest says what it was made from, and one of another format or
    /// with no header is not read as one.
    #[test]
    fn a_digest_names_the_rollout_it_was_made_from() {
        let stamp = Stamp {
            len: 1_234,
            modified_ns: 1_780_000_000_123_456_789,
        };
        let line = header_line(stamp, "/work/sessions/rollout-a.jsonl.zst");
        assert!(!line.contains('\n'));
        assert_eq!(
            read_header(&line),
            Some((stamp, "/work/sessions/rollout-a.jsonl.zst".to_string()))
        );
        let other = line.replace(
            &format!("\"zerocode_rollout_digest\":{VERSION}"),
            &format!("\"zerocode_rollout_digest\":{}", VERSION + 1),
        );
        assert_ne!(other, line);
        assert_eq!(read_header(&other), None);
        assert_eq!(read_header(META), None);
        assert_eq!(read_header(""), None);
    }
}
