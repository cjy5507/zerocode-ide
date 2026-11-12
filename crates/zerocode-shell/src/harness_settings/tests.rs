use super::*;
use zerocode_core::continue_gate::Mode;
use zerocode_core::launch_budget::{DEFAULT_CONCURRENT, DEFAULT_PER_DAY, DEFAULT_PER_HOUR};

#[test]
fn a_person_who_never_chose_gets_the_defaults_and_they_end_nobody() {
    let harness = HarnessSettings::default();
    assert_eq!(harness.gate.mode, Mode::Notify);
    assert_eq!(harness.gate.task_usd, None);
    assert_eq!(harness.gate.day_usd, None);
    assert_eq!(harness.launches.concurrent, Some(DEFAULT_CONCURRENT));
    assert_eq!(harness.launches.per_hour, Some(DEFAULT_PER_HOUR));
    assert_eq!(harness.launches.per_day, Some(DEFAULT_PER_DAY));
}

#[test]
fn what_the_pane_sends_is_read_and_a_missing_half_is_the_default() {
    let sent = serde_json::json!({
        "gate": {"mode": "stop", "task_usd": 12.5, "day_usd": 80},
        "launches": {"concurrent": 2, "per_hour": null, "per_day": 500},
    });
    let harness = HarnessSettings::parse(&sent).expect("a settings record");
    assert_eq!(harness.gate.mode, Mode::Stop);
    assert_eq!(harness.gate.task_usd, Some(12.5));
    assert_eq!(harness.gate.day_usd, Some(80.0));
    assert_eq!(harness.launches.concurrent, Some(2));
    assert_eq!(harness.launches.per_hour, None, "a blank is no ceiling");
    assert_eq!(harness.launches.per_day, Some(500));

    assert_eq!(
        HarnessSettings::parse(&serde_json::json!({})).expect("an empty record"),
        HarnessSettings::default()
    );
    let only_launches = HarnessSettings::parse(&serde_json::json!({"launches": {"per_day": 9}}))
        .expect("half a record");
    assert_eq!(only_launches.gate, Settings::default());
    assert_eq!(only_launches.launches.per_day, Some(9));
    assert_eq!(
        only_launches.launches.concurrent,
        Some(DEFAULT_CONCURRENT),
        "a ceiling left out stays at its default"
    );
}

#[test]
fn a_bad_budget_is_none_and_a_bad_ceiling_refuses_the_save() {
    let strange = HarnessSettings::parse(&serde_json::json!({
        "gate": {"mode": "stop", "task_usd": -5, "day_usd": "lots"},
    }))
    .expect("a record with a strange budget");
    assert_eq!(
        strange.gate.mode,
        Mode::Stop,
        "the mode survives its neighbours"
    );
    assert_eq!(strange.gate.task_usd, None);
    assert_eq!(strange.gate.day_usd, None);

    for bad in [
        serde_json::json!({"launches": {"per_hour": -1}}),
        serde_json::json!({"launches": {"per_hour": 1.5}}),
        serde_json::json!({"launches": {"per_day": "many"}}),
    ] {
        assert!(HarnessSettings::parse(&bad).is_err(), "{bad}");
    }
}

#[test]
fn the_record_round_trips_through_the_settings_document() {
    let harness = HarnessSettings {
        gate: Settings {
            mode: Mode::Off,
            task_usd: Some(3.0),
            day_usd: None,
        },
        launches: Limits {
            concurrent: None,
            per_hour: Some(10),
            per_day: Some(100),
        },
        alerts: Alerts { landing: false },
        workers: Workers {
            refuse_prompts: true,
        },
        mail: Mail { peers: false },
    };
    let text = serde_json::to_string(&harness).expect("serializes");
    let back: HarnessSettings = serde_json::from_str(&text).expect("reads back");
    assert_eq!(back, harness);
    let old_document: HarnessSettings = serde_json::from_str("{}").expect("an old document");
    assert_eq!(old_document, HarnessSettings::default());
}

/// Whether a summoned worker refuses a permission prompt nobody is watching is
/// the person's to turn on: a document that never said, a half-written record
/// and a word that is not a plain `true` all leave the worker asking.
#[test]
fn a_worker_refuses_unwatched_prompts_only_on_a_plain_yes() {
    assert!(!HarnessSettings::default().workers.refuse_prompts);
    let parsed = |sent: serde_json::Value| {
        HarnessSettings::parse(&sent)
            .expect("a settings record")
            .workers
            .refuse_prompts
    };
    assert!(parsed(
        serde_json::json!({"workers": {"refuse_prompts": true}})
    ));
    for unsaid in [
        serde_json::json!({}),
        serde_json::json!({"workers": {}}),
        serde_json::json!({"workers": {"refuse_prompts": false}}),
        serde_json::json!({"workers": {"refuse_prompts": "yes"}}),
        serde_json::json!({"workers": {"refuse_prompts": 1}}),
        serde_json::json!({"workers": true}),
    ] {
        assert!(!parsed(unsaid.clone()), "{unsaid}");
    }
    // The other halves do not move with it.
    let whole = HarnessSettings::parse(&serde_json::json!({
        "workers": {"refuse_prompts": true},
        "alerts": {"landing": false},
    }))
    .expect("a settings record");
    assert!(whole.workers.refuse_prompts && !whole.alerts.landing);
    assert_eq!(whole.gate, Settings::default());
}

/// The panes' mail is on for a person who never chose (사람 결정 2026-11-12),
/// and only a plain `false` turns it off — a bad word turns nothing off.
#[test]
fn the_panes_mail_is_on_unless_the_person_said_a_plain_off() {
    assert!(HarnessSettings::default().mail.peers);
    let read = |sent: serde_json::Value| {
        HarnessSettings::parse(&sent)
            .expect("a settings record")
            .mail
            .peers
    };
    assert!(read(serde_json::json!({})));
    assert!(read(serde_json::json!({"mail": {"peers": "no"}})));
    assert!(read(serde_json::json!({"mail": {"peers": true}})));
    assert!(!read(serde_json::json!({"mail": {"peers": false}})));
    // And a stored record written before the switch existed reads it on.
    let stored: HarnessSettings =
        serde_json::from_value(serde_json::json!({"alerts": {"landing": false}}))
            .expect("an older record");
    assert!(stored.mail.peers);
}
