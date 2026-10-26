//! Fixed mail notices over a pane's own authenticated events channel.

use super::*;
use std::sync::Arc;
use std::time::Duration;
use zerocode_hookd::session_notify::{NotificationOutcome, PointerNotice, SessionNotifier};

const NOTICE_DEADLINE: Duration = Duration::from_secs(2);

/// A steer capability alone cannot wake an idle root. Require the complete
/// notice contract rather than learning support by sending a person's text.
pub(crate) fn supported(support: &serde_json::Value) -> bool {
    let notice = &support["notice"];
    notice["supported"] == true
        && notice["method"] == zerocode_harness::method::STEER
        && notice["kind"] == "notice"
        && notice["scope"] == "idle-next-turn"
}

struct ZoNotifier {
    channel: ZoChannel,
    session: String,
    current: Box<dyn Fn() -> bool + Send + Sync>,
    /// How long one notice may take: [`NOTICE_DEADLINE`] in the window. A
    /// test gives a hang guard instead, so it judges the notice and not how
    /// fast a loaded machine answers in two seconds (l79j g2, load ~100).
    deadline: Duration,
}

impl SessionNotifier for ZoNotifier {
    fn notify(&self, notice: &PointerNotice) -> NotificationOutcome {
        if !(self.current)() {
            return NotificationOutcome::DefinitelyUnsent;
        }
        let session = self.session.clone();
        let text = notice.text().to_string();
        let result = with_client_deadline(
            self.channel.addr.clone(),
            self.channel.token.clone(),
            |error| error.to_string(),
            Some(self.deadline),
            async move |client| {
                client
                    .call(
                        zerocode_harness::method::STEER,
                        serde_json::json!({"id":session,"kind":"notice","text":text}),
                    )
                    .await
                    .map_err(|error| error.to_string())
            },
        );
        match result {
            Ok(value) if matches!(value["receipt"].as_str(), Some("queued" | "consumed")) => {
                NotificationOutcome::Confirmed
            }
            // A timeout can follow acceptance. The hub owns bounded retries of
            // this idempotent notice and never types into the composer.
            Ok(_) | Err(_) => NotificationOutcome::Unknown,
        }
    }
}

pub(crate) fn register(
    term: TermId,
    channel: ZoChannel,
    session: String,
    current: impl Fn() -> bool + Send + Sync + 'static,
) -> crate::orchestration_notify::RouteLease {
    crate::orchestration_notify::register_notice_route(
        term,
        Arc::new(ZoNotifier {
            channel,
            session,
            current: Box::new(current),
            deadline: NOTICE_DEADLINE,
        }),
    )
}

/// The subscriber owns the lease. Capability updates can add or revoke it;
/// ordinary model changes keep the same generation and notice watermark.
pub(crate) fn sync_route(
    route: &mut Option<crate::orchestration_notify::RouteLease>,
    app: &AppHandle,
    owner: ZoChannelOwner,
    epoch: u64,
    session: &str,
    channel: &ZoChannel,
) {
    let ZoChannelOwner::Term(term) = owner else {
        return;
    };
    if !crate::zo_integration_runtime::supports_notice(&app.state::<AppState>(), owner, epoch) {
        *route = None;
        return;
    }
    if route.is_some() {
        return;
    }
    let emitter = app.clone();
    let session = session.to_string();
    let checked_session = session.clone();
    *route = Some(register(term, channel.clone(), session, move || {
        let state = emitter.state::<AppState>();
        crate::zo_integration_runtime::supports_notice(&state, owner, epoch)
            && state.channel_owners().get(&checked_session) == Some(&owner)
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn a_notice_turn_does_not_acknowledge_a_human_or_worker_composer_submission() {
        let report = crate::shell_runtime::zo_channel_report(
            &serde_json::json!({"type":"turn", "phase":"start",
            "turn_id":8, "origin":"notice"}),
        )
        .expect("a notice turn");
        assert_eq!(report.state, zerocode_core::hook::HookState::Working);
        assert!(
            !report.acknowledges_prompt,
            "a notice was taken as evidence that the person's draft was submitted"
        );
    }

    #[test]
    fn a_notice_requires_the_idle_root_capability() {
        let supported_notice = serde_json::json!({"notice":{"supported":true,
            "method":"session.steer","kind":"notice","scope":"idle-next-turn"}});
        assert!(supported(&supported_notice));
        for (key, value) in [
            ("supported", serde_json::json!(false)),
            ("method", serde_json::json!("session.run_turn")),
            ("kind", serde_json::json!("prompt")),
            ("scope", serde_json::json!("active-root-turn")),
        ] {
            let mut bad = supported_notice.clone();
            bad["notice"][key] = value;
            assert!(!supported(&bad), "accepted an incompatible {key}");
        }
        assert!(!supported(&serde_json::json!({"steer":{"supported":true}})));
    }

    /// A bound that only stops a hung test: every wait here is for the
    /// fixture's own peer, and none of them judges how fast a machine is.
    const HANG_GUARD: Duration = Duration::from_secs(60);

    #[test]
    fn a_notice_wakes_an_idle_root_over_its_own_channel_and_leaves_the_draft() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        listener.set_nonblocking(true).unwrap();
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + HANG_GUARD;
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "the notice never connected");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept: {error}"),
                }
            };
            stream.set_read_timeout(Some(HANG_GUARD)).unwrap();
            stream.set_write_timeout(Some(HANG_GUARD)).unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: serde_json::Value = serde_json::from_str(&line).unwrap();
            writeln!(
                stream,
                "{}",
                serde_json::json!({"jsonrpc":"2.0","id":request["id"],
                "result":{"receipt":"queued","merged":1,"turn_id":null}})
            )
            .unwrap();
            sent.send(request).unwrap();
        });
        const TERM: u32 = 75_728;
        crate::human_input::typed(TERM);
        let draft = crate::human_input::line_of(TERM);
        let notifier = ZoNotifier {
            channel: zo_channel_entry(addr, Some("fixture-pane-bearer".into())),
            session: "idle-root".into(),
            current: Box::new(|| true),
            deadline: HANG_GUARD,
        };
        let notice = PointerNotice::new("run-1", "run:run-1", "m-1", 1).unwrap();
        let outcome = notifier.notify(&notice);
        let request = received.recv_timeout(HANG_GUARD).unwrap();
        server.join().unwrap();
        assert_eq!(outcome, NotificationOutcome::Confirmed);
        assert_eq!(request["method"], "session.steer");
        assert_eq!(request["token"], "fixture-pane-bearer");
        assert_eq!(
            request["params"],
            serde_json::json!({"id":"idle-root",
            "kind":"notice","text":notice.text()})
        );
        assert_eq!(crate::human_input::line_of(TERM), draft);
        crate::human_input::forget_term(TERM);
    }

    #[test]
    fn a_replaced_pane_sends_no_notice() {
        let notifier = ZoNotifier {
            channel: zo_channel_entry("192.0.2.1:9".into(), None),
            session: "gone".into(),
            current: Box::new(|| false),
            deadline: NOTICE_DEADLINE,
        };
        let notice = PointerNotice::new("run-1", "run:run-1", "m-1", 1).unwrap();
        assert_eq!(
            notifier.notify(&notice),
            NotificationOutcome::DefinitelyUnsent
        );
    }
}
