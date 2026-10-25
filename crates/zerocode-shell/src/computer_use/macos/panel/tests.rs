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
        4,
        "initial start, one recovery, then one start per new generation"
    );
}

#[test]
fn a_lost_helper_gets_one_restart_in_the_same_share_but_not_a_restart_loop() {
    let mut delivery = Delivery::default();
    let launches = Cell::new(0);
    // The helper first starts, dies, restarts, and dies again. Neither the
    // target nor the ownership generation changes during these notices.
    for (index, name) in [
        None,
        Some("first"),
        None,
        None,
        Some("restarted"),
        None,
        None,
    ]
    .into_iter()
    .enumerate()
    {
        delivery.send(
            update(index as u64 + 1, name.map(endpoint)),
            |_| None,
            || {
                launches.set(launches.get() + 1);
                true
            },
        );
        assert_eq!(
            launches.get(),
            if index < 2 { 1 } else { 2 },
            "the next notice recovers once; repeated loss stays remembered"
        );
    }
}

#[test]
fn a_lost_helpers_restart_waits_for_a_busy_provider_without_spending_the_attempt() {
    let mut delivery = Delivery::default();
    delivery.send(update(1, None), |_| None, || true);
    delivery.send(update(2, Some(endpoint("first"))), |_| None, || true);
    let attempts = Cell::new(0);
    for revision in 3..=5 {
        delivery.send(
            update(revision, None),
            |_| None,
            || {
                attempts.set(attempts.get() + 1);
                revision != 3
            },
        );
    }
    assert_eq!(attempts.get(), 2, "busy once, then one actual launch");
}

#[test]
fn coalescing_presentations_keeps_a_helper_loss_until_delivery() {
    let mailbox = Mailbox::default();
    let mut delivery = Delivery::default();
    delivery.send(update(1, None), |_| None, || true);
    // The worker may not read the successful launch before its watch ends.
    for (revision, endpoint) in [(2, Some(endpoint("short-lived"))), (3, None), (4, None)] {
        mailbox.publish(&update(revision, None).state, endpoint);
    }
    let launches = Cell::new(0);
    delivery.send(
        mailbox.next(),
        |_| None,
        || {
            launches.set(launches.get() + 1);
            true
        },
    );
    assert_eq!(launches.get(), 1, "coalescing must retain the loss");
}

#[test]
fn an_ended_watch_forgets_its_endpoint_and_preserves_a_new_helpers_endpoint() {
    use crate::computer_use::macos::{PANEL_ENDPOINT, StopWatch, remember_panel};
    use std::os::unix::net::UnixListener;

    for replacement in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("watch.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let (asked, heard) = std::sync::mpsc::channel();
        let (close, closing) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "stopWatch");
            asked.send(()).unwrap();
            closing.recv().unwrap();
            // EOF is the helper's death, with no command on the main session.
        });
        remember_panel(&socket, "fixture");
        let watch = StopWatch::start(&socket, "fixture").unwrap();
        heard.recv().unwrap();
        let next_socket = directory.path().join("new.sock");
        if replacement {
            remember_panel(&next_socket, "new-fixture");
        }
        close.send(()).unwrap();
        server.join().unwrap();
        watch.thread.join().unwrap();
        let mut held = PANEL_ENDPOINT.lock().unwrap();
        let remaining = held.take();
        assert_eq!(
            remaining.as_ref().map(|endpoint| endpoint.socket.as_path()),
            replacement.then_some(next_socket.as_path()),
            "watch termination forgets only the matching helper"
        );
    }
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
