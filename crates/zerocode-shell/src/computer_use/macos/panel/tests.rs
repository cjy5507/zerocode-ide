use std::cell::Cell;
use std::io::{BufRead as _, Write as _};
use std::os::unix::net::UnixStream;

use serde_json::{Value, json};

use super::{Delivery, Mailbox, Update};
use crate::computer_use::macos::{PanelEndpoint, Wire};

fn endpoint(name: &str) -> PanelEndpoint {
    PanelEndpoint {
        socket: name.into(),
        token: "fixture".into(),
    }
}

fn update(revision: u64, endpoint: Option<PanelEndpoint>) -> Update {
    Update {
        state: json!({"revision": revision, "target": "sample", "generation": 0}),
        endpoint,
    }
}

#[test]
fn only_the_latest_pending_presentation_is_delivered() {
    let mailbox = Mailbox::default();
    for revision in 1..=3 {
        mailbox.publish(&update(revision, None).state, None);
    }
    assert_eq!(mailbox.next().state["revision"], 3);
    assert!(mailbox.pending.lock().unwrap().latest.is_none());
    // Even after delivery, a late caller cannot replace a newer presentation.
    mailbox.publish(&update(2, None).state, None);
    assert!(mailbox.pending.lock().unwrap().latest.is_none());
}

#[test]
fn queued_presentations_coalesce_while_one_authenticated_connection_is_busy() {
    let (client, server) = UnixStream::pair().unwrap();
    let reply = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(server.try_clone().unwrap());
        let mut writer = server;
        let mut heard = Vec::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 {
                return heard;
            }
            let request: Value = serde_json::from_str(&line).unwrap();
            writeln!(
                writer,
                "{}",
                json!({"id": request["id"], "ok": true, "result": {}})
            )
            .unwrap();
            heard.push(request);
        }
    });
    let mut client = Some(client);
    let connections = Cell::new(0);
    let mut delivery = Delivery::default();
    let mailbox = Mailbox::default();
    mailbox.publish(&update(1, None).state, Some(endpoint("panel-fixture")));
    for _ in 0..2 {
        delivery.send(
            mailbox.next(),
            |_| {
                connections.set(connections.get() + 1);
                // New notices arrive while the first delivery is connecting.
                for revision in 2..=3 {
                    mailbox.publish(
                        &update(revision, None).state,
                        Some(endpoint("panel-fixture")),
                    );
                }
                let wire = Wire::new(client.take()?, "fixture".into()).unwrap();
                wire.stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                Some(wire)
            },
            || panic!("an endpoint needs no launch"),
        );
    }
    drop(delivery);
    let heard = reply.join().unwrap();
    assert_eq!(connections.get(), 1);
    assert_eq!(heard.len(), 2);
    for (index, request) in heard.iter().enumerate() {
        assert_eq!(request["id"], index + 1);
        assert_eq!(request["token"], "fixture");
        assert_eq!(request["method"], "sharePanel");
        assert_eq!(request["params"]["revision"], [1, 3][index]);
    }
}

#[test]
fn a_failed_connection_is_tried_once_per_new_presentation() {
    let mut delivery = Delivery::default();
    let connections = Cell::new(0);
    for (revision, name) in [(1, "first"), (2, "first"), (3, "second")] {
        delivery.send(
            update(revision, Some(endpoint(name))),
            |_| {
                connections.set(connections.get() + 1);
                None
            },
            || panic!("an endpoint needs no launch"),
        );
    }
    assert_eq!(
        connections.get(),
        3,
        "one attempt per new presentation, without a retry loop"
    );
}

#[test]
fn a_failed_helper_start_is_remembered_across_notices() {
    let mut delivery = Delivery::default();
    let launches = Cell::new(0);
    for revision in 1..=3 {
        delivery.send(
            update(revision, None),
            |_| panic!("no endpoint"),
            || {
                launches.set(launches.get() + 1);
                true // A launch was attempted and failed; there is still no endpoint.
            },
        );
    }
    assert_eq!(launches.get(), 1);
}

#[test]
fn a_target_change_allows_one_new_helper_start_including_a_return_to_the_old_target() {
    let mut delivery = Delivery::default();
    let launches = Cell::new(0);
    for (index, target) in ["sample", "sample", "another", "another", "sample", "sample"]
        .into_iter()
        .enumerate()
    {
        let mut notice = update(index as u64 + 1, None);
        notice.state["target"] = target.into();
        delivery.send(
            notice,
            |_| panic!("no endpoint"),
            || {
                launches.set(launches.get() + 1);
                true
            },
        );
    }
    assert_eq!(
        launches.get(),
        3,
        "one start per target transition, not per notice"
    );
}

#[test]
fn a_new_generation_allows_one_helper_start_after_an_endpoint_is_forgotten() {
    let mut delivery = Delivery::default();
    let launches = Cell::new(0);
    for (revision, generation, endpoint) in [
        (1, 0, None),
        (2, 0, Some(endpoint("panel-fixture"))),
        (3, 0, None),
        (4, 1, None),
        (5, 1, None),
        (6, 2, None),
        (7, 2, None),
    ] {
        let mut notice = update(revision, endpoint);
        notice.state["generation"] = generation.into();
        delivery.send(
            notice,
            |_| None,
            || {
                launches.set(launches.get() + 1);
                true
            },
        );
    }
    assert_eq!(
        launches.get(),
        3,
        "one start per generation, including after helper loss"
    );
}

#[test]
fn a_busy_provider_is_not_counted_as_a_failed_launch() {
    let mut delivery = Delivery::default();
    delivery.send(update(1, None), |_| panic!("no endpoint"), || false);
    assert!(!delivery.startup_attempted);
    let launches = Cell::new(0);
    for revision in 2..=3 {
        delivery.send(
            update(revision, None),
            |_| panic!("no endpoint"),
            || {
                launches.set(launches.get() + 1);
                true
            },
        );
    }
    assert_eq!(launches.get(), 1);
}
