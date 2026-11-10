//! What reading Codex rollouts costs the usage scans, whole and as digests
//! ([`zerocode_core::rollout_digest`]) — and whether the two read alike.
//!
//! The measure behind the digest cache: every rollout under the folders named
//! is read the way the scans read it (a line at a time, capped), once whole
//! and once as its digest, by the ledger's own reader and by the token
//! panel's view of a line. It prints sizes, times and whether every answer
//! matched — never a line of anybody's rollout.
//!
//! ```text
//! cargo run --release -p zerocode-core --example rollout_digest_cost -- \
//!   ~/.codex/sessions ~/.codex/archived_sessions
//! ```

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use zerocode_core::usage_stats_codex::RolloutReader;
use zerocode_core::{codex_delta, record_file, rollout_digest};

/// The scans' own line cap (`token_scan::MAX_RECORD_BYTES`).
const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;

fn rollouts_under(root: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            rollouts_under(&path, found);
        } else if path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| {
                name.starts_with("rollout-") && record_file::names_record(name, ".jsonl")
            })
        {
            found.push(path);
        }
    }
}

/// Every complete line of `path`, the way the scans read one.
fn for_each_line(path: &Path, mut read: impl FnMut(&str)) -> u64 {
    let Ok(file) = record_file::open(path) else {
        return 0;
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut bytes = 0_u64;
    loop {
        line.clear();
        let got = match (&mut reader)
            .take(MAX_RECORD_BYTES)
            .read_until(b'\n', &mut line)
        {
            Ok(0) | Err(_) => break,
            Ok(got) => got as u64,
        };
        bytes += got;
        if got >= MAX_RECORD_BYTES {
            continue;
        }
        if let Ok(text) = std::str::from_utf8(&line) {
            read(text);
        }
    }
    bytes
}

/// What the token panel's reader takes from a line (`codex_tokens`,
/// `absorb_rollout`): the same marks, the same paths, the same normalizer.
fn token_view(line: &str) -> Option<String> {
    if !rollout_digest::MARKS.iter().any(|mark| line.contains(mark)) {
        return None;
    }
    let row: serde_json::Value = serde_json::from_str(line).ok()?;
    let payload = row.get("payload").unwrap_or(&serde_json::Value::Null);
    let said = |name: &str| {
        payload
            .get(name)
            .and_then(serde_json::Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let kind = row.get("type").and_then(serde_json::Value::as_str);
    if kind == Some("session_meta") {
        return Some(format!(
            "meta|{:?}|{:?}",
            said("id").or_else(|| said("session_id")),
            said("cwd")
        ));
    }
    if kind == Some("turn_context") {
        return Some(format!("context|{:?}|{:?}", said("model"), said("cwd")));
    }
    if payload.get("type").and_then(serde_json::Value::as_str) != Some("token_count") {
        return None;
    }
    let info = payload.get("info")?;
    let total = codex_delta::normalize(info.get("total_token_usage"));
    let last = codex_delta::normalize(info.get("last_token_usage"));
    let at = row.get("timestamp").and_then(serde_json::Value::as_str);
    Some(format!("tokens|{at:?}|{total:?}|{last:?}"))
}

#[derive(Default)]
struct Pass {
    took: Duration,
    ledger_events: usize,
    ledger_print: u64,
    token_views: usize,
    token_print: u64,
}

/// A running fingerprint of everything a reader answered, in order.
fn fold(print: &mut u64, text: &str) {
    for byte in text.bytes() {
        *print = (*print ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    *print = print.rotate_left(7);
}

fn main() {
    let mut files = Vec::new();
    for root in std::env::args().skip(1) {
        rollouts_under(Path::new(&root), &mut files);
    }
    // Oldest first, as the ledger's scan takes them: the original claims a
    // record and the fork is the copy.
    files.sort();
    if files.is_empty() {
        eprintln!("usage: rollout_digest_cost <folder of rollouts>…");
        std::process::exit(2);
    }

    // Whole: what each scan pays today.
    let started = Instant::now();
    let mut decoded = 0_u64;
    for path in &files {
        decoded += for_each_line(path, |_| {});
    }
    let decode_only = started.elapsed();

    let mut whole_ledger = Pass::default();
    let started = Instant::now();
    let mut claimed = HashSet::new();
    for path in &files {
        let stem = record_file::record_stem(path).unwrap_or_default();
        let mut reader = RolloutReader::new(stem, &mut claimed);
        for_each_line(path, |line| {
            reader.line(line.trim_end_matches(['\n', '\r']))
        });
        for event in reader.finish() {
            whole_ledger.ledger_events += 1;
            fold(&mut whole_ledger.ledger_print, &format!("{event:?}"));
        }
    }
    whole_ledger.took = started.elapsed();

    let mut whole_tokens = Pass::default();
    let started = Instant::now();
    for path in &files {
        for_each_line(path, |line| {
            if let Some(view) = token_view(line) {
                whole_tokens.token_views += 1;
                fold(&mut whole_tokens.token_print, &view);
            }
        });
    }
    whole_tokens.took = started.elapsed();

    // The digests, made once.
    let started = Instant::now();
    let mut digests: Vec<String> = Vec::with_capacity(files.len());
    let mut kept_lines = 0_usize;
    for path in &files {
        let mut digest = String::new();
        for_each_line(path, |line| {
            if let Some(kept) = rollout_digest::digest_line(line) {
                digest.push_str(&kept);
                digest.push('\n');
                kept_lines += 1;
            }
        });
        digests.push(digest);
    }
    let digesting = started.elapsed();
    let digest_bytes: usize = digests.iter().map(String::len).sum();

    // As digests: what each scan pays once they are kept.
    let mut digest_ledger = Pass::default();
    let started = Instant::now();
    let mut claimed = HashSet::new();
    for (path, digest) in files.iter().zip(&digests) {
        let stem = record_file::record_stem(path).unwrap_or_default();
        let mut reader = RolloutReader::new(stem, &mut claimed);
        for line in digest.lines() {
            reader.line(line);
        }
        for event in reader.finish() {
            digest_ledger.ledger_events += 1;
            fold(&mut digest_ledger.ledger_print, &format!("{event:?}"));
        }
    }
    digest_ledger.took = started.elapsed();

    let mut digest_tokens = Pass::default();
    let started = Instant::now();
    for digest in &digests {
        for line in digest.lines() {
            if let Some(view) = token_view(line) {
                digest_tokens.token_views += 1;
                fold(&mut digest_tokens.token_print, &view);
            }
        }
    }
    digest_tokens.took = started.elapsed();

    // Kept compressed, as a rollout at rest is: what the cache holds on disk
    // and what reading it back costs.
    let started = Instant::now();
    let squeezed: Vec<Vec<u8>> = digests
        .iter()
        .map(|digest| record_file::compressed(digest.as_bytes()))
        .collect();
    let squeezing = started.elapsed();
    let squeezed_bytes: usize = squeezed.iter().map(Vec::len).sum();
    let started = Instant::now();
    let mut read_back = true;
    for (digest, held) in digests.iter().zip(&squeezed) {
        let mut text = String::new();
        let decoded = ruzstd::decoding::StreamingDecoder::new(held.as_slice())
            .ok()
            .and_then(|mut decoder| decoder.read_to_string(&mut text).ok());
        read_back &= decoded.is_some() && text == *digest;
    }
    let unsqueezing = started.elapsed();

    let on_disk: u64 = files
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|held| held.len())
        .sum();
    let compressed = files
        .iter()
        .filter(|path| record_file::is_compressed(path))
        .count();
    let megabytes = |bytes: u64| bytes as f64 / 1e6;
    println!(
        "rollouts {} ({} compressed), {:.1} MB on disk, {:.1} MB decoded",
        files.len(),
        compressed,
        megabytes(on_disk),
        megabytes(decoded)
    );
    println!(
        "digests  {} lines, {:.1} MB ({:.2}% of decoded), made in {:.2} s",
        kept_lines,
        megabytes(digest_bytes as u64),
        100.0 * digest_bytes as f64 / decoded.max(1) as f64,
        digesting.as_secs_f64()
    );
    println!(
        "kept     {:.1} MB compressed ({:.1}x), written in {:.2} s, read back in {:.3} s, whole {}",
        megabytes(squeezed_bytes as u64),
        digest_bytes as f64 / squeezed_bytes.max(1) as f64,
        squeezing.as_secs_f64(),
        unsqueezing.as_secs_f64(),
        read_back
    );
    println!("decode alone          {:>8.2} s", decode_only.as_secs_f64());
    println!(
        "ledger reader  whole  {:>8.2} s   digest {:>7.3} s   events {} / {}   same {}",
        whole_ledger.took.as_secs_f64(),
        digest_ledger.took.as_secs_f64(),
        whole_ledger.ledger_events,
        digest_ledger.ledger_events,
        whole_ledger.ledger_events == digest_ledger.ledger_events
            && whole_ledger.ledger_print == digest_ledger.ledger_print
    );
    println!(
        "token reader   whole  {:>8.2} s   digest {:>7.3} s   records {} / {}   same {}",
        whole_tokens.took.as_secs_f64(),
        digest_tokens.took.as_secs_f64(),
        whole_tokens.token_views,
        digest_tokens.token_views,
        whole_tokens.token_views == digest_tokens.token_views
            && whole_tokens.token_print == digest_tokens.token_print
    );
}
