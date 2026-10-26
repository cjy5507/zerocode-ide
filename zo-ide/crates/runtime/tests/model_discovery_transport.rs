//! Model discovery names a request that got no response by the HTTP client's
//! own error, here on real sockets (t-77146).
//!
//! The unit tests of `model_discovery` decide the same classification with
//! no socket and no clock (`FailureKind::of_transport`). These two keep the
//! client's real errors in view: that a refused connection reaches
//! `Fault::transport` as `is_connect` and a request nothing answers as
//! `is_timeout`. They need a loopback socket, so they are an integration
//! test of their own and stay out of the unit run.

use std::time::Duration;

use runtime::model_discovery::{FailureKind, Fault};

/// One GET of `url` that gives up after `limit`.
fn get(url: String, limit: Duration) -> reqwest::Error {
    api::sync_bridge::run_blocking(async move {
        reqwest::Client::builder()
            .timeout(limit)
            .build()
            .expect("a client")
            .get(url)
            .send()
            .await
    })
    .expect_err("nothing answers")
}

/// A socket that accepts the connection and never answers: only the clock
/// can end the request, so a slow machine ends it later, never otherwise.
#[test]
fn a_request_nothing_answers_is_a_timeout_with_no_number() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let url = format!("http://{}/", listener.local_addr().expect("its address"));
    let fault = Fault::transport(&get(url, Duration::from_millis(100)));
    assert_eq!((fault.kind, fault.http_status), (FailureKind::Timeout, None), "{}", fault.detail);
    drop(listener);
}

/// A port nobody listens on is refused at once. The limit is long so that a
/// loaded machine reads the refusal before the clock ends the request; the
/// one thing left to chance is another process taking the released port
/// between the drop and the connect, so the gate does not run it: run it by
/// name with `--ignored`.
#[test]
#[ignore = "a released loopback port can be taken by another process before the connect"]
fn a_refused_connection_is_network_with_no_number() {
    let closed = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let url = format!("http://{}/", closed.local_addr().expect("its address"));
    drop(closed);
    let fault = Fault::transport(&get(url, Duration::from_secs(30)));
    assert_eq!((fault.kind, fault.http_status), (FailureKind::Network, None), "{}", fault.detail);
}
