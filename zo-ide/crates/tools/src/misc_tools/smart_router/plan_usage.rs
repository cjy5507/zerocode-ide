use std::collections::BTreeMap;
use std::path::Path;

use serde::de::DeserializeOwned;
use zerocode_core::orchestration::task_cost::ZoRequestUsage;

#[derive(Debug, Default, PartialEq)]
pub struct PlanUsage {
    pub tokens: u64,
    pub usd: f64,
    pub requests: usize,
    pub work_effort: Option<String>,
}

pub(super) fn read_rows<T: DeserializeOwned>(file: &Path) -> std::io::Result<Vec<T>> {
    let Some(contents) = runtime::secure_fs::read_regular_file_absolute_no_follow_bounded(file, 16 * 1024 * 1024)? else {
        return Ok(Vec::new());
    };
    contents.lines().filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(std::io::Error::other)).collect()
}

pub fn measure_plan_usage(cwd: &Path, session: &str, attempt: &str, model: &str, since_ms: u64) -> Option<PlanUsage> {
    let file = api::PromptCachePaths::for_session(session).requests_path;
    let rows: Vec<api::RequestLedgerRow> = read_rows(&file).ok()?;
    let mut usage = price_requests(&rows, attempt, model)?;
    let mut ledgers = std::collections::BTreeSet::new();
    for seat in zerocode_core::jev::JEV_USES {
        if !ledgers.insert(seat.ledger) { continue; }
        let rows: Vec<serde_json::Value> = read_rows(&runtime::jev_ledger_dir(cwd).join(seat.ledger)).ok()?;
        for row in rows {
            use zerocode_core::jev::summary::{AT, MODEL, summarize_rows};
            let at = AT.read(&row).and_then(serde_json::Value::as_u64);
            if at.is_some_and(|at| at < since_ms) { continue; }
            let tally = summarize_rows(std::iter::once(&row), i64::MIN);
            if tally.input_tokens == 0 && tally.unmetered_requests == 0 { continue; }
            let billed_attempt = row.get("attempt").and_then(serde_json::Value::as_str)?;
            if billed_attempt != attempt { continue; }
            if tally.unmetered_requests != 0 || tally.requests > 1 { return None; }
            let rate = api::systemone_rate(MODEL.read(&row)?.as_str()?)?;
            let tokens = u32::try_from(tally.input_tokens).ok()?;
            usage.tokens = usage.tokens.checked_add(u64::from(tokens))?;
            usage.usd += f64::from(tokens) * rate.input / 1_000_000.0;
        }
    }
    usage.usd.is_finite().then_some(usage)
}

fn price_requests(rows: &[api::RequestLedgerRow], attempt: &str, work_model: &str) -> Option<PlanUsage> {
    if attempt.is_empty() { return None; }
    let mut unique = BTreeMap::new();
    for row in rows.iter().filter(|row| row.attempt == attempt) {
        if row.seq == 0 || row.ts_unix_ms == 0 { return None; }
        if let Some(previous) = unique.insert((row.seq, row.ts_unix_ms), row) {
            if previous != row { return None; }
        }
    }
    if unique.is_empty() { return None; }
    let mut usage = PlanUsage::default();
    let mut efforts = std::collections::BTreeSet::new();
    for row in unique.values() {
        let request_usage = ZoRequestUsage {
            seq: row.seq,
            ts_unix_ms: i64::try_from(row.ts_unix_ms).ok()?,
            model: row.model.clone(),
            input_uncached: i64::from(row.input_uncached),
            cache_read: i64::from(row.cache_read),
            cache_creation: i64::from(row.cache_creation),
            cache_creation_1h: row.cache_creation_1h.map(u64::from),
            output: i64::from(row.output),
            ttl: row.ttl.clone(),
        };
        usage.usd += request_usage.cost_usd()?;
        let input = u64::from(row.input_uncached) + u64::from(row.cache_read) + u64::from(row.cache_creation);
        usage.tokens = usage.tokens.checked_add(input + u64::from(row.output))?;
        usage.requests += 1;
        if api::resolve_model_alias(&row.model) == api::resolve_model_alias(work_model) {
            efforts.insert(row.effort.clone());
        }
    }
    if efforts.len() != 1 || !usage.usd.is_finite() { return None; }
    usage.work_effort = efforts.into_iter().next().filter(|effort| !effort.is_empty());
    Some(usage)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> api::RequestLedgerRow {
        // July 20, 2026 UTC, before the model's promotional price started.
        serde_json::from_value(serde_json::json!({
            "seq": 1, "ts_unix_ms": 1_784_505_600_000_u64, "model": "gpt-5.6-sol", "provider": "openai",
            "cache_creation": 0, "cache_read": 100, "input_uncached": 200,
            "output": 50, "message_count": 2, "broke": false,
            "attempt": "session@1", "effort": "high"
        })).unwrap()
    }

    #[test]
    fn exact_attempt_bills_all_wire_buckets_once() {
        let row = request();
        let mut unrelated = row.clone();
        unrelated.attempt = "session@2".into();
        let usage = price_requests(&[row.clone(), row, unrelated], "session@1", "gpt-5.6-sol").unwrap();
        assert_eq!((usage.tokens, usage.requests), (350, 1));
        assert!((usage.usd - 0.00255).abs() < 1e-10);
        assert_eq!(usage.work_effort.as_deref(), Some("high"));
    }

    #[test]
    fn long_request_uses_its_model_rates_and_reported_cache_split() {
        let mut row = request();
        row.ts_unix_ms = 1_795_392_000_000; // November 23, 2026 UTC.
        row.model = "claude-haiku-5-5".into();
        row.provider = "anthropic".into();
        row.input_uncached = 100_000;
        row.cache_creation = 200;
        row.cache_creation_1h = Some(100);
        row.ttl = "5m+1h".into();
        let usage = price_requests(&[row], "session@1", "claude-haiku-5-5").unwrap();
        assert_eq!((usage.tokens, usage.requests), (100_350, 1));
        assert!((usage.usd - 0.050_292_5).abs() < 1e-10);
    }

    #[test]
    fn conflicting_duplicate_missing_price_and_mixed_ttl_are_unknown() {
        let row = request();
        let mut conflict = row.clone();
        conflict.output += 1;
        assert!(price_requests(&[row.clone(), conflict], "session@1", "gpt-5.6-sol").is_none());
        let mut unknown = row.clone();
        unknown.model = "unpriced-model".into();
        assert!(price_requests(&[unknown], "session@1", "gpt-5.6-sol").is_none());
        let mut mixed = row;
        mixed.cache_creation = 100;
        mixed.ttl = "5m+1h".into();
        assert!(price_requests(&[mixed], "session@1", "gpt-5.6-sol").is_none());
    }
}
