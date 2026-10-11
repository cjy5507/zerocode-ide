//! The evidence rules of t-26584, read at their own boundaries: each bar's
//! line is exclusive or inclusive as the rule says, and each miss names itself.

use std::path::Path;

use serde_json::{Value, json};

use super::*;
use crate::jev::summary::{agreement_rows, agreement_rows_counting_all};

/// A paired mark on a label row: the candidate and the current judgment on one
/// fact, with the cost and latency each spent.
fn paired(candidate: bool, current: bool, tokens: (u64, u64), ms: (u64, u64)) -> Value {
    json!({
        "at": 1,
        "label": "a",
        "labelSource": "executed",
        "agreed": candidate,
        "baselineAgreed": current,
        "inputTokens": tokens.0,
        "currentInputTokens": tokens.1,
        "elapsedMs": ms.0,
        "currentElapsedMs": ms.1,
    })
}

#[test]
fn every_promoting_seat_with_a_ledger_names_its_label_source() {
    let unnamed: Vec<&str> = crate::jev::JEV_USES
        .iter()
        .filter(|seat| seat.promotes)
        .filter(|seat| seat_label_source(seat.id).is_none())
        .map(|seat| seat.id)
        .collect();
    assert!(
        unnamed.is_empty(),
        "seats with no label source: {unnamed:?}"
    );
}

#[test]
fn a_label_written_to_a_seats_ledger_takes_the_seats_source_and_author() {
    let ledger = Path::new("state").join("claim-check.jsonl");
    let mut row = json!({"at": 1, "label": "claim:1", "requestAt": 1, "agreed": true});
    stamp_for_ledger(&ledger, &mut row);
    assert_eq!(row["labelSource"], json!("executed"));
    assert_eq!(row["labelAuthor"], json!("claim"));
    assert_eq!(source_of(&row), Some(LabelSource::Executed));
}

#[test]
fn a_writers_own_source_is_kept_and_a_request_row_is_not_stamped() {
    let ledger = Path::new("state").join("worker-placement.jsonl");
    let mut own =
        json!({"at": 1, "label": "w", "requestAt": 1, "agreed": true, "labelSource": "executed"});
    stamp_for_ledger(&ledger, &mut own);
    assert_eq!(
        own["labelSource"],
        json!("executed"),
        "the writer's own word stands"
    );
    let mut request = json!({"at": 1, "outcome": "answered", "agreed": true});
    stamp_for_ledger(&ledger, &mut request);
    assert!(
        request.get("labelSource").is_none(),
        "a request is no label: {request}"
    );
}

#[test]
fn a_ledger_no_seat_keeps_names_no_author() {
    assert!(seat_of_ledger(Path::new("state").join("no-such-seat.jsonl").as_path()).is_none());
}

#[test]
fn a_proxy_pair_is_no_pair() {
    let proxy = json!({"at": 1, "label": "a", "labelSource": "proxy", "agreed": true, "baselineAgreed": false});
    let tally = pair_tally([&proxy]);
    assert_eq!(tally.pairs, 0, "a proxy may not be a pair: {tally:?}");
    let agreement = agreement_rows([&proxy], i64::MIN);
    assert_eq!((agreement.compared, agreement.proxied), (0, 1));
    let counted = agreement_rows_counting_all([&proxy], i64::MIN);
    assert_eq!(
        counted.compared, 1,
        "the reading before the rules counts it"
    );
}

#[test]
fn the_quality_line_is_exclusive_at_two_percent() {
    let at_line: Vec<Value> = (0..1_000)
        .map(|n| paired(n < 520, n < 500, (100, 100), (100, 100)))
        .collect();
    let tally = pair_tally(at_line.iter());
    assert_eq!(
        bar_miss(&tally, &PROMOTION_BAR),
        Some(BarMiss::Quality {
            gain_permille: 20,
            over_permille: 20
        })
    );
    let over: Vec<Value> = (0..1_000)
        .map(|n| paired(n < 521, n < 500, (100, 100), (100, 100)))
        .collect();
    assert_eq!(bar_miss(&pair_tally(over.iter()), &PROMOTION_BAR), None);
}

#[test]
fn the_cost_line_is_exclusive_at_one_percent_and_the_latency_line_at_five() {
    let cost_at_line: Vec<Value> = (0..30)
        .map(|_| paired(true, false, (1_010, 1_000), (100, 100)))
        .collect();
    assert!(matches!(
        bar_miss(&pair_tally(cost_at_line.iter()), &PROMOTION_BAR),
        Some(BarMiss::Cost { .. })
    ));
    let cost_under: Vec<Value> = (0..30)
        .map(|_| paired(true, false, (1_009, 1_000), (100, 100)))
        .collect();
    assert_eq!(
        bar_miss(&pair_tally(cost_under.iter()), &PROMOTION_BAR),
        None
    );
    let latency_at_line: Vec<Value> = (0..30)
        .map(|_| paired(true, false, (100, 100), (1_050, 1_000)))
        .collect();
    assert!(matches!(
        bar_miss(&pair_tally(latency_at_line.iter()), &PROMOTION_BAR),
        Some(BarMiss::Latency { .. })
    ));
    let latency_under: Vec<Value> = (0..30)
        .map(|_| paired(true, false, (100, 100), (1_049, 1_000)))
        .collect();
    assert_eq!(
        bar_miss(&pair_tally(latency_under.iter()), &PROMOTION_BAR),
        None
    );
}

#[test]
fn the_bar_wants_thirty_pairs_and_priced_pairs() {
    let few: Vec<Value> = (0..29)
        .map(|_| paired(true, false, (100, 100), (100, 100)))
        .collect();
    assert_eq!(
        bar_miss(&pair_tally(few.iter()), &PROMOTION_BAR),
        Some(BarMiss::Pairs {
            pairs: 29,
            wanted: 30
        })
    );
    let unpriced: Vec<Value> = (0..30)
        .map(|_| json!({"at": 1, "label": "a", "labelSource": "executed", "agreed": true, "baselineAgreed": false}))
        .collect();
    assert_eq!(
        bar_miss(&pair_tally(unpriced.iter()), &PROMOTION_BAR),
        Some(BarMiss::Unpriced {
            priced: 0,
            wanted: 30
        })
    );
}

#[test]
fn a_calibration_needs_its_records_and_its_floor_before_its_checks() {
    assert_eq!(
        calibration_miss(None, &Acted::default(), true),
        Some(CalibrationMiss::NoRecord)
    );
    let thin = Acted {
        marks: 29,
        agreed: 29,
        ..Acted::default()
    };
    assert_eq!(
        calibration_miss(Some(500), &thin, true),
        Some(CalibrationMiss::Records {
            records: 29,
            wanted: 30
        })
    );
    let weak = Acted {
        marks: 30,
        agreed: 17,
        answered: 30,
        acted_answers: 30,
        ..Acted::default()
    };
    assert!(matches!(
        calibration_miss(Some(500), &weak, true),
        Some(CalibrationMiss::LowerBound { .. })
    ));
}

#[test]
fn the_abstain_ceiling_is_inclusive_at_thirty_percent() {
    let at_line = Acted {
        answered: 10,
        acted_answers: 7,
        marks: 30,
        agreed: 30,
        paired_marks: 30,
        paired_agreed: 30,
        paired_baseline_agreed: 25,
    };
    assert_eq!(
        calibration_miss(Some(500), &at_line, true),
        None,
        "30% abstained is at the ceiling"
    );
    let over = Acted {
        acted_answers: 6,
        ..at_line
    };
    assert!(matches!(
        calibration_miss(Some(500), &over, true),
        Some(CalibrationMiss::Abstain {
            share_permille: 400,
            ceiling_permille: 300
        })
    ));
}

#[test]
fn the_lift_is_inclusive_at_one_point_two_over_the_cheapest_reader() {
    let base = Acted {
        answered: 30,
        acted_answers: 30,
        marks: 30,
        agreed: 30,
        paired_marks: 30,
        paired_agreed: 30,
        paired_baseline_agreed: 25,
    };
    assert_eq!(
        calibration_miss(Some(500), &base, true),
        None,
        "30/25 is exactly 1.2"
    );
    let short = Acted {
        paired_baseline_agreed: 26,
        ..base
    };
    assert!(matches!(
        calibration_miss(Some(500), &short, true),
        Some(CalibrationMiss::Lift { .. })
    ));
}

#[test]
fn a_bundle_needs_its_rows_and_its_pin_and_its_canary() {
    let person = |n: usize, agreed: bool| json!({"at": 1, "label": format!("person-{n}"), "labelSource": "person", "agreed": agreed});
    let rows: Vec<Value> = (0..60).map(|n| person(n, true)).collect();
    let policy = HUMAN_BUNDLE;
    let unpinned = bundle_of(&rows, "placement", &policy);
    assert!(!unpinned.pinned);
    assert_eq!(bundle_miss(&unpinned, &policy), Some(BundleMiss::Unpinned));
    let mut pinned_rows = rows.clone();
    pinned_rows.push(bundle_pin_for(&rows, "placement"));
    let pinned = bundle_of(&pinned_rows, "placement", &policy);
    assert!(
        pinned.pinned,
        "the pin names this bundle's hash: {pinned:?}"
    );
    assert_eq!(
        bundle_miss(&pinned, &policy),
        None,
        "sixty agreeing rows clear the canary"
    );
    let few = bundle_of(&rows[..19], "placement", &policy);
    assert_eq!(
        bundle_miss(&few, &policy),
        Some(BundleMiss::Rows {
            rows: 19,
            wanted: 20
        })
    );
    let other_seat = bundle_of(&pinned_rows, "notify", &policy);
    assert!(!other_seat.pinned, "a pin names one seat");
    let disagreeing: Vec<Value> = (0..60).map(|n| person(n, n % 2 == 0)).collect();
    let mut half = disagreeing.clone();
    half.push(bundle_pin_for(&disagreeing, "placement"));
    assert!(matches!(
        bundle_miss(&bundle_of(&half, "placement", &policy), &policy),
        Some(BundleMiss::Canary { .. })
    ));
}
