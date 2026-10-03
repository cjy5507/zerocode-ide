use super::*;
use crate::civil::MS_PER_HOUR;

const NOW: i64 = 1_800_000_000_000;

fn close(left: f64, right: f64) {
    assert!((left - right).abs() < 1e-9, "{left} != {right}");
}

#[test]
fn a_day_adds_what_each_call_cost_and_a_minute_is_one_bucket() {
    let mut day = DaySpend::default();
    day.add(NOW, 1.5);
    day.add(NOW + 1_000, 0.25);
    day.add(NOW + BUCKET_MS, 2.0);
    close(day.spent(NOW + BUCKET_MS), 3.75);
    assert_eq!(day.buckets.len(), 2, "two minutes, two buckets");
}

#[test]
fn only_the_last_day_counts_and_what_aged_out_is_let_go() {
    let mut day = DaySpend::default();
    day.add(NOW, 10.0);
    day.add(NOW + 12 * MS_PER_HOUR, 4.0);
    close(day.spent(NOW + 23 * MS_PER_HOUR), 14.0);
    close(day.spent(NOW + MS_PER_DAY + BUCKET_MS), 4.0);
    day.add(NOW + MS_PER_DAY + 2 * BUCKET_MS, 1.0);
    assert_eq!(
        day.buckets.len(),
        2,
        "the first bucket went on the next add"
    );
    close(day.spent(NOW + MS_PER_DAY + 2 * BUCKET_MS), 5.0);
}

#[test]
fn a_clock_that_stepped_back_never_reorders_the_ledger() {
    let mut day = DaySpend::default();
    day.add(NOW + 5 * BUCKET_MS, 1.0);
    day.add(NOW, 2.0);
    assert_eq!(day.buckets.len(), 1);
    close(day.spent(NOW + 5 * BUCKET_MS), 3.0);
}

#[test]
fn a_cost_that_is_not_dollars_is_nothing() {
    let mut day = DaySpend::default();
    day.add(NOW, f64::NAN);
    day.add(NOW, f64::INFINITY);
    day.add(NOW, -1.0);
    assert_eq!(day, DaySpend::default());
    close(day.spent(NOW), 0.0);
}

#[test]
fn the_day_survives_a_restart() {
    let mut day = DaySpend::default();
    day.add(NOW, 7.0);
    day.add(NOW + 3 * BUCKET_MS, 1.0);
    let kept: DaySpend =
        serde_json::from_str(&serde_json::to_string(&day).expect("serializes")).expect("reads");
    assert_eq!(kept, day);
    close(kept.spent(NOW + 4 * BUCKET_MS), 8.0);
}

#[test]
fn a_whole_days_calls_fill_no_more_than_a_days_buckets() {
    let mut day = DaySpend::default();
    for second in 0..(2 * 24 * 3_600) {
        day.add(NOW + second * 1_000, 0.01);
    }
    assert!(day.buckets.len() <= 24 * 60 + 1, "{}", day.buckets.len());
}
