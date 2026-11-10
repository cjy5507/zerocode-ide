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

/// How long the queue's 끼워 넣기 waits for a zo channel to take the words
/// (t-86909): as long as a mail notice (`zo_notice::NOTICE_DEADLINE`) — the
/// channel is on this machine, and a person is looking at the row.
const STEER_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// Put the person's words into the turn a zo pane is running, over the pane's
/// own channel (t-86909): `session.steer` with no `kind`, which zo answers
/// with the turn it folded them into, or refuses (`-32003`) when none runs —
/// it never opens a turn from a root's idle composer. No key is typed at the
/// pane, so there is no menu to fall on and no draft to join. One blocking
/// exchange, like every call on a pane's channel.
///
/// A refusal is the page's word ([`zerocode_core::agent::turn_refusal::IDLE`] and its
/// kin). A channel that does not answer in time is the word for that too
/// ([`zerocode_core::agent::turn_refusal::UNKNOWN`]): the words may have gone in,
/// so the row stays queued, says it does not know in the person's language, and
/// is not sent a second time on its own (t-86909 r2).
pub(crate) fn steer_turn(channel: ZoChannel, session: String, text: String) -> Result<(), String> {
    steer_turn_within(channel, session, text, STEER_DEADLINE)
}

/// [`steer_turn`] with its deadline given, so a test can wait for a channel
/// that never answers without waiting the two seconds.
fn steer_turn_within(
    channel: ZoChannel,
    session: String,
    text: String,
    deadline: std::time::Duration,
) -> Result<(), String> {
    with_client_deadline(
        channel.addr,
        channel.token,
        steer_refusal,
        Some(deadline),
        async move |client| {
            client
                .steer(&session, &text, None)
                .await
                .map(|_| ())
                .map_err(steer_after_the_request)
        },
    )
    .map_err(|said| {
        if said == "connection_reload_timeout" {
            zerocode_core::agent::turn_refusal::UNKNOWN.to_string()
        } else {
            said
        }
    })
}

/// A failure of the exchange once the request has gone to the channel (t-86909 r3, P3-5):
/// zo's own refusals are the page's words ([`steer_refusal`]); a connection that drops, a
/// line that cannot be read or a transport error AFTER the request was written may have
/// come after zo took the words in, so they are the word for "may have gone in" — the row
/// is not sent again at the turn's end. A channel that could not be reached at all took
/// nothing and says so in its own words ([`steer_refusal`] as `unconnected`).
fn steer_after_the_request(error: zerocode_harness::HarnessError) -> String {
    match error {
        zerocode_harness::HarnessError::Rpc { .. } => steer_refusal(error),
        _ => zerocode_core::agent::turn_refusal::UNKNOWN.to_string(),
    }
}

/// A channel's refusal of a steer, in the page's words: no turn runs
/// ([`zerocode_core::agent::turn_refusal::IDLE`]), or this zo does not know the method
/// ([`zerocode_core::agent::turn_refusal::UNSUPPORTED`]). Anything else is its own words.
fn steer_refusal(error: zerocode_harness::HarnessError) -> String {
    use zerocode_harness::{HarnessError, ServeErrorKind};
    match &error {
        HarnessError::Rpc {
            kind: ServeErrorKind::SteerDenied,
            ..
        } => zerocode_core::agent::turn_refusal::IDLE.to_string(),
        HarnessError::Rpc {
            kind: ServeErrorKind::MethodNotFound,
            ..
        } => zerocode_core::agent::turn_refusal::UNSUPPORTED.to_string(),
        _ => error.to_string(),
    }
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

    fn refused(code: i64, message: &str) -> zerocode_harness::HarnessError {
        zerocode_harness::HarnessError::Rpc {
            method: "session.steer".to_string(),
            code,
            kind: zerocode_harness::ServeErrorKind::from_code(code),
            message: message.to_string(),
        }
    }

    /// What a zo channel's refusal of a steer becomes (t-86909): the page's
    /// word for "no turn runs" when zo denies the steer (`-32003`), its word
    /// for "no road" when this zo does not know the method, and anything else
    /// — a dead channel, a malformed line — in its own words, which the page
    /// shows as they came and keeps the message queued.
    #[test]
    fn a_zo_channels_refusal_of_a_steer_is_the_pages_word() {
        assert_eq!(
            steer_refusal(refused(-32003, "no turn is running in this pane to steer")),
            zerocode_core::agent::turn_refusal::IDLE
        );
        assert_eq!(
            steer_refusal(refused(-32601, "method not found")),
            zerocode_core::agent::turn_refusal::UNSUPPORTED
        );
        assert!(steer_refusal(refused(-32602, "session.steer needs text")).contains("needs text"));
        assert!(
            steer_refusal(zerocode_harness::HarnessError::Closed).contains("closed the connection")
        );
    }
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    /// A zo channel that takes the connection and never answers a steer (t-86909 r2): past its
    /// deadline the words may have gone in, and the page is told so by the word — the sentence
    /// is the window's, in the person's language, not a Korean one the backend wrote.
    #[test]
    fn a_zo_channel_that_does_not_answer_a_steer_in_time_is_the_unknown_word() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut line = String::new();
            // The request arrives and nothing is said back until the client gives up.
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            std::thread::sleep(Duration::from_secs(2));
            drop(stream);
        });
        let said = steer_turn_within(
            zo_channel_entry(addr, Some("fixture-pane-bearer".into())),
            "silent".to_string(),
            "추가 지시".to_string(),
            Duration::from_millis(300),
        );
        server.join().unwrap();
        assert_eq!(
            said,
            Err(zerocode_core::agent::turn_refusal::UNKNOWN.to_string())
        );
    }

    /// A zo channel that took the request and then dropped the connection (t-86909 r3, P3-5): the
    /// words may have gone in. The page is told the one word for that, so the row is not sent a
    /// second time at the turn's end; before, the harness's English "closed the connection
    /// mid-request" reached the person and the row went out again.
    #[test]
    fn a_zo_channel_that_closes_after_the_request_went_is_the_unknown_word_too() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut line = String::new();
            let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
            // The request was read; nothing is answered, the connection just ends.
            drop(stream);
        });
        let said = steer_turn_within(
            zo_channel_entry(addr, Some("fixture-pane-bearer".into())),
            "dropped".to_string(),
            "추가 지시".to_string(),
            Duration::from_secs(5),
        );
        server.join().unwrap();
        assert_eq!(
            said,
            Err(zerocode_core::agent::turn_refusal::UNKNOWN.to_string())
        );
    }

    /// A channel nobody listens on took nothing: that is not the unknown word. The row may be
    /// steered again once the channel is back, and the person is told what failed.
    #[test]
    fn a_zo_channel_that_could_not_be_reached_is_not_the_unknown_word() {
        // Port 0, which nothing can listen on — a bind to port 0 takes another port — so
        // the connection fails at once on every system and there is no port for another
        // test to take. A listener bound and dropped gave its port back, and under a full
        // suite (v1.1.139's first lane) another test's listener took it, the steer waited
        // out its five seconds and came back as the unknown word. A socket bound to a
        // port that never listens does not do instead: this Mac drops a connection to it
        // without refusing it, and the steer waits out its five seconds all the same.
        let addr = "127.0.0.1:0".to_string();
        let said = steer_turn_within(
            zo_channel_entry(addr, Some("fixture-pane-bearer".into())),
            "nobody".to_string(),
            "추가 지시".to_string(),
            Duration::from_secs(5),
        );
        let Err(said) = said else {
            panic!("a channel nobody listens on took a steer");
        };
        assert_ne!(said, zerocode_core::agent::turn_refusal::UNKNOWN);
        assert!(!said.is_empty());
    }

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
            // On macOS the accepted stream inherits the listener's non-blocking
            // mode, and a read before the notice's bytes land answered
            // `WouldBlock` (l81g g2c): the fixture's peer is waited for.
            stream.set_nonblocking(false).unwrap();
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
