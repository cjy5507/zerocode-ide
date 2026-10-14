//! The newest request the current client sent, read back off the records
//! its seats wrote (t-53792): what is observed, what is said to be absent,
//! and what cannot be said — a cache answer, a rider, a refusal, another
//! client's request and a row that names no client are never this client's
//! use.

use judgment_provider::{Protocol, Provider, ProviderKind};
use zerocode_core::jev::summary::CLIENT_ROUTE;
use zerocode_core::jev::{JevMode, JevUse, MAIL_TRIAGE, ROUTE_USE_APPLIED, STALL};

use super::*;
use crate::systemone::latest::{self, Coverage, LastRequest, LatestRequest, Reason, Scope, State};

fn local(base: &str) -> Provider {
    Provider {
        kind: ProviderKind::Local,
        protocol: Protocol::SystemOne,
        base_url: base.to_string(),
        model: "fixture-decision".to_string(),
        requires_auth: false,
    }
}

/// A zo home whose settings select `provider`, and the wire that reads them.
fn selecting(provider: &Provider) -> (tempfile::TempDir, Wire) {
    let home = tempfile::tempdir().expect("a zo home");
    let path = home.path().join("settings.json");
    std::fs::write(
        &path,
        json!({"smart": {(judgment_provider::SETTINGS_KEY): provider}}).to_string(),
    )
    .expect("settings");
    (home, Wire::at("http://127.0.0.1:1", "", Some(path)))
}

/// `rows` appended to `seat`'s record where `wire` files it.
fn file(wire: &Wire, seat: &JevUse, rows: &[Value]) {
    append_rows(&ledger_of(wire, seat).expect("the provider's folder"), rows);
}

/// A row of a request that left, put to `route`.
fn sent(at: i64, route: Option<&str>, outcome: &str) -> Value {
    let mut row = json!({"at": at, "outcome": outcome, "requests": 1, "redactedLines": 0});
    if let Some(route) = route {
        row[CLIENT_ROUTE.canonical] = json!(route);
    }
    row
}

/// The newest request this client sent is the one read, with what its
/// feature wrote of it; a cache answer, a row riding another's request,
/// another client's request, a label and a transition after it are not.
#[test]
fn the_newest_request_this_client_sent_is_read_with_what_its_feature_wrote() {
    let provider = local("http://127.0.0.1:9");
    let (_home, wire) = selecting(&provider);
    let route = route_of(&wire).expect("a route");
    let mut stall = sent(100, Some(&route), "answered");
    stall["elapsedMs"] = json!(300);
    file(&wire, &STALL, &[stall]);
    let mut failed = sent(200, Some(&route), "http_503");
    failed["elapsedMs"] = json!(9);
    failed["applied"] = json!(false);
    let mut cached = sent(300, Some(&route), "answered");
    cached["cached"] = json!(true);
    cached["requests"] = json!(0);
    let mut riding = sent(400, Some(&route), "answered");
    riding["requests"] = json!(0);
    let mut other = provider.clone();
    other.model = "another-decision-model".to_string();
    let foreign = sent(500, Some(&other.route_fingerprint()), "answered");
    file(
        &wire,
        &MAIL_TRIAGE,
        &[
            failed,
            cached,
            riding,
            foreign,
            json!({"at": 600, "kind": "label", "label": "letter-1"}),
            json!({"at": 700, "transition": "rise"}),
        ],
    );
    assert_eq!(
        latest::read(&wire, 9_000),
        LatestRequest {
            provider_identity: Some(provider.identity()),
            observed_at_ms: 9_000,
            scope: Scope::Machine,
            coverage: Coverage::WindowRequests,
            state: State::Observed,
            reason: None,
            unattributed_newer: false,
            last_request: Some(LastRequest {
                feature: MAIL_TRIAGE.id,
                at_ms: Some(200),
                outcome: "http_503".to_string(),
                elapsed_ms: Some(9),
                applied: Some(false),
            }),
        }
    );
}

/// An answer is not an application: a request is read as applied only where
/// its feature wrote so, under any of the three spellings it writes it in.
#[test]
fn an_answer_is_read_as_applied_only_where_its_feature_wrote_so() {
    let (_home, wire) = selecting(&local("http://127.0.0.1:9"));
    let route = route_of(&wire).expect("a route");
    let newest = |row: Value| {
        file(&wire, &STALL, &[row]);
        latest::read(&wire, 0).last_request.map(|last| last.applied)
    };
    assert_eq!(newest(sent(1, Some(&route), "answered")), Some(None));
    let mut acted = sent(2, Some(&route), "answered");
    acted["routeUse"] = json!(ROUTE_USE_APPLIED);
    assert_eq!(newest(acted), Some(Some(true)));
    let mut recorded = sent(3, Some(&route), "answered");
    recorded["routeUse"] = json!(JevMode::Shadow.key());
    assert_eq!(newest(recorded), Some(Some(false)));
    let mut pressed = sent(4, Some(&route), "answered");
    pressed["pressed"] = json!(true);
    assert_eq!(newest(pressed), Some(Some(true)));
}

/// A request row that names no client — one written before rows named it,
/// one written before the door, one that names a client but no count — is
/// never this client's: alone it leaves the reading unknown, and newer than
/// the request read it is said to be.
#[test]
fn a_request_that_names_no_client_is_never_read_as_this_clients() {
    let (_home, wire) = selecting(&local("http://127.0.0.1:9"));
    let route = route_of(&wire).expect("a route");
    file(
        &wire,
        &STALL,
        &[
            sent(100, None, "answered"),
            json!({"at": 50, "outcome": "answered"}),
        ],
    );
    let reading = latest::read(&wire, 0);
    assert_eq!(
        (reading.state, reading.reason, reading.last_request),
        (State::Unknown, Some(Reason::Unattributed), None)
    );
    assert_eq!(reading.coverage, Coverage::WindowRequests);
    let mut uncounted = sent(150, Some(&route), "answered");
    uncounted.as_object_mut().expect("a row").remove("requests");
    file(&wire, &MAIL_TRIAGE, &[uncounted]);
    assert_eq!(latest::read(&wire, 0).state, State::Unknown);

    file(&wire, &MAIL_TRIAGE, &[sent(200, Some(&route), "answered")]);
    let reading = latest::read(&wire, 0);
    assert_eq!(
        (reading.state, reading.unattributed_newer),
        (State::Observed, false)
    );
    assert_eq!(reading.last_request.map(|last| last.at_ms), Some(Some(200)));
    file(&wire, &STALL, &[sent(300, None, "http_503")]);
    let reading = latest::read(&wire, 0);
    assert_eq!(
        (reading.state, reading.unattributed_newer),
        (State::Observed, true)
    );
    assert_eq!(
        reading.last_request.map(|last| (last.feature, last.at_ms)),
        Some((MAIL_TRIAGE.id, Some(200)))
    );
}

/// Records that hold nothing — or only refusals, cache answers and other
/// clients' requests, every one naming what it is — say this client was not
/// observed.
#[test]
fn records_that_hold_only_other_clients_or_nothing_sent_read_as_none_observed() {
    let provider = local("http://127.0.0.1:9");
    let (_home, wire) = selecting(&provider);
    let empty = latest::read(&wire, 5);
    assert_eq!(
        (empty.state, empty.reason, empty.coverage),
        (State::NoneObserved, None, Coverage::WindowRequests)
    );
    assert_eq!(empty.provider_identity, Some(provider.identity()));
    let route = route_of(&wire).expect("a route");
    let refused =
        json!({"at": 1, "outcome": Refused::NoKey.token(), "requests": 0, "redactedLines": 0});
    let mut cached = sent(2, Some(&route), "answered");
    cached["cached"] = json!(true);
    cached["requests"] = json!(0);
    let mut other = provider.clone();
    other.base_url = "http://127.0.0.1:10".to_string();
    file(
        &wire,
        &STALL,
        &[
            refused,
            cached,
            sent(3, Some(&other.route_fingerprint()), "answered"),
        ],
    );
    let reading = latest::read(&wire, 5);
    assert_eq!(
        (
            reading.state,
            reading.reason,
            reading.unattributed_newer,
            reading.last_request
        ),
        (State::NoneObserved, None, false, None)
    );
}

/// A record that is there and cannot be read leaves the reading unknown,
/// whatever the others hold.
#[test]
fn a_record_that_does_not_read_says_unknown() {
    let (_home, wire) = selecting(&local("http://127.0.0.1:9"));
    let route = route_of(&wire).expect("a route");
    file(&wire, &STALL, &[sent(1, Some(&route), "answered")]);
    std::fs::create_dir_all(ledger_of(&wire, &MAIL_TRIAGE).expect("a record"))
        .expect("a folder where a record belongs");
    let reading = latest::read(&wire, 0);
    assert_eq!(
        (reading.state, reading.reason, reading.last_request),
        (State::Unknown, Some(Reason::RecordsUnreadable), None)
    );
}

#[test]
fn malformed_records_cannot_prove_absence_or_an_older_requests_recency() {
    let (_home, wire) = selecting(&local("http://127.0.0.1:9"));
    let route = route_of(&wire).expect("a route");
    let ledger = ledger_of(&wire, &STALL).expect("a ledger");
    std::fs::create_dir_all(ledger.parent().expect("a folder")).expect("create folder");
    for broken in ["{\"outcome\":", "null", "[]", "\"not a record\""] {
        for prefix in [
            String::new(),
            format!("{}\n", sent(1, Some(&route), "answered")),
        ] {
            std::fs::write(&ledger, format!("{prefix}{broken}\n")).expect("write broken record");
            let reading = latest::read(&wire, 2);
            assert_eq!(
                (reading.state, reading.reason, reading.last_request),
                (State::Unknown, Some(Reason::RecordsUnreadable), None),
                "a malformed retained record must leave the newest request unknown"
            );
        }
    }
}

#[test]
fn a_tail_without_a_complete_record_is_unknown_not_no_requests() {
    let (_home, wire) = selecting(&local("http://127.0.0.1:9"));
    let ledger = ledger_of(&wire, &STALL).expect("a ledger");
    std::fs::create_dir_all(ledger.parent().expect("a folder")).expect("create folder");
    std::fs::write(&ledger, "x".repeat(300 * 1024)).expect("write oversized record");
    let reading = latest::read(&wire, 2);
    assert_eq!(
        (reading.state, reading.reason, reading.last_request),
        (State::Unknown, Some(Reason::RecordsUnreadable), None)
    );
}

/// Settings that do not read name no model and no records: nothing is read.
#[test]
fn settings_that_do_not_read_name_no_identity_and_read_no_record() {
    let home = tempfile::tempdir().expect("a zo home");
    let path = home.path().join("settings.json");
    std::fs::write(
        &path,
        json!({"smart": {(judgment_provider::SETTINGS_KEY): {"kind": "unknown"}}}).to_string(),
    )
    .expect("settings");
    assert_eq!(
        latest::read(&Wire::at("http://127.0.0.1:1", "", Some(path)), 7),
        LatestRequest {
            provider_identity: None,
            observed_at_ms: 7,
            scope: Scope::Machine,
            coverage: Coverage::None,
            state: State::Unknown,
            reason: Some(Reason::SettingsUnreadable),
            unattributed_newer: false,
            last_request: None,
        }
    );
    let homeless = latest::read(&Wire::at("http://127.0.0.1:1", "", None), 7);
    assert_eq!(
        (homeless.state, homeless.reason, homeless.coverage),
        (
            State::Unknown,
            Some(Reason::SettingsUnreadable),
            Coverage::None
        )
    );
}

/// Jev's records are one folder for every pin and address, so another pin
/// or another address is another client there too.
#[test]
fn another_jev_pin_or_address_is_another_client() {
    let home = tempfile::tempdir().expect("a zo home");
    let path = home.path().join("settings.json");
    std::fs::write(
        &path,
        json!({"smart": {"jevModel": "jev-1.13.0"}}).to_string(),
    )
    .expect("settings");
    let wire = Wire::at("http://127.0.0.1:1", "test-key", Some(path));
    let mut alias = Provider::jev(SYSTEMONE_MODEL);
    alias.base_url = "http://127.0.0.1:1".to_string();
    let mut elsewhere = Provider::jev("jev-1.13.0");
    elsewhere.base_url = "http://127.0.0.1:2".to_string();
    file(
        &wire,
        &STALL,
        &[
            sent(1, Some(&alias.route_fingerprint()), "answered"),
            sent(2, Some(&elsewhere.route_fingerprint()), "answered"),
        ],
    );
    let reading = latest::read(&wire, 0);
    assert_eq!(
        (reading.provider_identity.as_deref(), reading.state),
        (Some("jev-1.13.0"), State::NoneObserved)
    );
    file(
        &wire,
        &STALL,
        &[sent(3, route_of(&wire).as_deref(), "answered")],
    );
    assert_eq!(
        latest::read(&wire, 0).last_request.map(|last| last.at_ms),
        Some(Some(3))
    );
}

/// Each record is read only as far back as its newest bytes: a request
/// further back than that is not read, and is not said to be there.
#[test]
fn only_each_records_newest_bytes_are_read() {
    let (_home, wire) = selecting(&local("http://127.0.0.1:9"));
    let route = route_of(&wire).expect("a route");
    file(&wire, &STALL, &[sent(1, Some(&route), "answered")]);
    let mut other = local("http://127.0.0.1:9");
    other.model = "another-decision-model".to_string();
    let theirs = other.route_fingerprint();
    let row_bytes = sent(2, Some(&theirs), "answered").to_string().len() + 1;
    let past_the_tail =
        usize::try_from(zerocode_core::transcript::MAX_TAIL_BYTES).expect("a tail") / row_bytes + 2;
    let after: Vec<Value> = (2..)
        .take(past_the_tail)
        .map(|at| sent(at, Some(&theirs), "answered"))
        .collect();
    file(&wire, &STALL, &after);
    assert_eq!(latest::read(&wire, 0).state, State::NoneObserved);
    file(&wire, &STALL, &[sent(i64::MAX, Some(&route), "answered")]);
    assert_eq!(
        latest::read(&wire, 0).last_request.map(|last| last.at_ms),
        Some(Some(i64::MAX))
    );
}

/// A question the wire put to the endpoint is read back off the row its seat
/// wrote through the one writer every seat's row goes through.
#[test]
fn a_request_the_wire_sent_is_read_back_off_the_row_its_seat_wrote() {
    let home = tempfile::tempdir().expect("a zo home");
    let workspace = tempfile::tempdir().expect("a workspace");
    let path = home.path().join("settings.json");
    std::fs::write(
        &path,
        json!({"smart": {"jev": {"workspaces": [door::resolved_path(workspace.path())]}}})
            .to_string(),
    )
    .expect("settings");
    let endpoint = Endpoint::serving(
        "HTTP/1.1 200 OK",
        json!({"model": ANSWERING_VERSION, "answers": {},
               "usage": {"input_tokens": 3, "output_tokens": 0}})
        .to_string(),
        0,
    );
    let wire = Wire::at(&endpoint.base(), "test-key", Some(path));
    let asked = wire.ask(
        &STALL,
        Some(workspace.path()),
        request_body(&json!({"screen": "a quiet pane"}), &json!({})),
        Duration::from_secs(5),
    );
    assert!(asked.answer.is_ok(), "{:?}", asked.answer);
    let mut row = json!({"at": 42, "outcome": "answered", "elapsedMs": 7});
    asked.spent.stamp(&mut row);
    record_rows(
        &STALL,
        &ledger_of(&wire, &STALL).expect("the record"),
        &[row],
        42,
    );
    let reading = latest::read(&wire, 43);
    assert_eq!(reading.state, State::Observed, "{reading:?}");
    assert_eq!(
        reading.last_request,
        Some(LastRequest {
            feature: STALL.id,
            at_ms: Some(42),
            outcome: "answered".to_string(),
            elapsed_ms: Some(7),
            applied: None,
        })
    );
}
