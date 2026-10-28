use std::collections::BTreeMap;

use serde::Deserialize;

use super::{SessionBook, SessionSpend, UsageSource};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ZoRequestUsage {
    pub seq: u64,
    pub ts_unix_ms: i64,
    pub model: String,
    pub input_uncached: i64,
    pub cache_read: i64,
    pub cache_creation: i64,
    #[serde(default)]
    pub output: i64,
    #[serde(default)]
    pub ttl: String,
}

/// What a request journal's reader could not hand over whole (t-79891): a
/// journal longer than the read limit is read from its end, and a line that
/// is not a usage row is skipped rather than dropping every row beside it.
/// Either way the rows read are a part, and the session says so.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadGaps {
    /// The journal's oldest rows were left unread.
    pub older_rows_dropped: bool,
    /// Lines in the part read that were not a usage row.
    pub skipped_lines: usize,
}

impl ReadGaps {
    #[must_use]
    pub fn is_whole(&self) -> bool {
        !self.older_rows_dropped && self.skipped_lines == 0
    }
}

impl SessionBook {
    pub fn read_zo_usage(
        &mut self,
        session: &str,
        rows: &[ZoRequestUsage],
        gaps: ReadGaps,
        scanned_at: i64,
    ) {
        self.scanned_at.insert(UsageSource::Zo, scanned_at);
        if rows.is_empty() && gaps.is_whole() {
            return;
        }
        let mut unique = BTreeMap::new();
        let mut spend = SessionSpend {
            gaps,
            ..SessionSpend::default()
        };
        let mut total_usd = Some(0.0);
        let mut first = i64::MAX;
        let mut last = i64::MIN;
        for row in rows {
            if let Some(previous) = unique.insert((row.seq, row.ts_unix_ms), row) {
                spend.invalid_usage |= previous != row;
            }
        }
        for row in unique.values() {
            first = first.min(row.ts_unix_ms);
            last = last.max(row.ts_unix_ms);
            let counters = [
                row.input_uncached,
                row.output,
                row.cache_read,
                row.cache_creation,
            ];
            if row.seq == 0 || row.ts_unix_ms < 0 || super::token_sum(counters).is_none() {
                spend.invalid_usage = true;
                continue;
            }
            for (sum, value) in [
                (&mut spend.input_tokens, row.input_uncached),
                (&mut spend.output_tokens, row.output),
                (&mut spend.cache_read_tokens, row.cache_read),
                (&mut spend.cache_write_tokens, row.cache_creation),
            ] {
                match sum.checked_add(value) {
                    Some(next) => *sum = next,
                    None => spend.invalid_usage = true,
                }
            }
            total_usd = total_usd
                .zip(price(row))
                .map(|(total, cost)| total + cost)
                .filter(|cost| cost.is_finite());
        }
        spend.usd = total_usd;
        self.sessions
            .insert((UsageSource::Zo, session.to_owned()), spend);
        self.bounds.insert(
            (UsageSource::Zo, session.to_owned()),
            (first <= last).then_some((first, last)),
        );
    }
}

fn price(row: &ZoRequestUsage) -> Option<f64> {
    let rate = model_prices::price(&row.model)?;
    if row.cache_creation > 0 && !matches!(row.ttl.as_str(), "" | "5m") {
        return None;
    }
    let input = row
        .input_uncached
        .checked_add(row.cache_read)?
        .checked_add(row.cache_creation)?;
    if input > 200_000 {
        return None;
    }
    Some(
        (f64::from(u32::try_from(row.input_uncached).ok()?) * rate.input
            + f64::from(u32::try_from(row.output).ok()?) * rate.output
            + f64::from(u32::try_from(row.cache_read).ok()?) * rate.cache_read
            + f64::from(u32::try_from(row.cache_creation).ok()?) * rate.cache_write)
            / 1_000_000.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_wire_model_is_priced_once_and_bad_duplicates_are_not_measurements() {
        let row = ZoRequestUsage {
            seq: 1,
            ts_unix_ms: 10,
            model: "gpt-5.6-sol".into(),
            input_uncached: 200,
            cache_read: 100,
            cache_creation: 0,
            output: 50,
            ttl: String::new(),
        };
        let mut book = SessionBook::default();
        let rows = [row.clone(), row.clone()];
        book.read_zo_usage("session", &rows, ReadGaps::default(), 20);
        let measured = book.spend(UsageSource::Zo, "session").unwrap();
        assert_eq!(measured.counters(), Some([200, 50, 100, 0]));
        assert_eq!(measured.usd, price(&row));
        let mut changed = row.clone();
        changed.output += 1;
        book.read_zo_usage("session", &[row, changed], ReadGaps::default(), 30);
        assert!(
            book.spend(UsageSource::Zo, "session")
                .unwrap()
                .counters()
                .is_none()
        );
    }
}
