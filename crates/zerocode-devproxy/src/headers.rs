//! Which header fields cross the hop, and which stop at it.
//!
//! The original copies every incoming header by object spread and overwrites
//! only `host` (`localhost-worktree-label-proxy.ts:244-248`), which is safe
//! there only because Node's own client re-derives framing and refuses a few
//! shapes on the way out. Hyper does neither: it honours the headers it is
//! handed, so a copied `content-length` beside a streamed body desynchronises
//! the hop and a copied `connection` describes a connection that no longer
//! exists once the message has been forwarded.
//!
//! So the rule here is allow-copy rather than deny-copy: start empty, and take
//! only fields that are the message's own rather than the connection's.

use hyper::header::{CONNECTION, HeaderMap, HeaderName, HeaderValue};

/// Fields that describe the single connection a message arrived on, not the
/// message itself (RFC 9110 §7.6.1). Forwarding any of them describes this
/// proxy's own socket to a server that cannot see it.
const HOP_BY_HOP: [HeaderName; 8] = [
    hyper::header::CONNECTION,
    hyper::header::PROXY_AUTHENTICATE,
    hyper::header::PROXY_AUTHORIZATION,
    hyper::header::TE,
    hyper::header::TRAILER,
    hyper::header::TRANSFER_ENCODING,
    hyper::header::UPGRADE,
    // `keep-alive` has no constant of its own in `http`.
    HeaderName::from_static("keep-alive"),
];

/// Also stopped on the way upstream: the framing the body itself decides, and
/// the forwarding claims a page could otherwise forge.
///
/// A dev server that trusts `X-Forwarded-For` to decide who may reach it would
/// otherwise trust whatever any tab on this machine typed into a fetch.
const CLIENT_ONLY: [HeaderName; 5] = [
    hyper::header::CONTENT_LENGTH,
    HeaderName::from_static("x-forwarded-for"),
    HeaderName::from_static("x-forwarded-host"),
    HeaderName::from_static("x-forwarded-proto"),
    HeaderName::from_static("forwarded"),
];

/// Every field the incoming `Connection` header names, lowercased. Those are
/// hop-by-hop by declaration rather than by list, and a proxy that forwards
/// them hands the next hop a promise about a socket it does not hold.
fn connection_named(headers: &HeaderMap) -> Vec<HeaderName> {
    headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|token| HeaderName::try_from(token.trim().to_ascii_lowercase()).ok())
        .collect()
}

/// Whether a field stops at this hop.
fn stops_here(name: &HeaderName, named: &[HeaderName], client_side: bool) -> bool {
    HOP_BY_HOP.contains(name) || named.contains(name) || (client_side && CLIENT_ONLY.contains(name))
}

/// The headers to send upstream: the message's own fields, and nothing that
/// belonged to the connection they arrived on. `host` is the caller's to set,
/// because only the caller knows which target this hop is for.
pub fn forwardable_request(incoming: &HeaderMap) -> HeaderMap {
    copy_message_fields(incoming, true)
}

/// The headers to send back to the browser, under the same rule — an upstream
/// `connection: close` otherwise closes the browser's socket to us rather than
/// ours to the dev server.
pub fn forwardable_response(incoming: &HeaderMap) -> HeaderMap {
    copy_message_fields(incoming, false)
}

fn copy_message_fields(incoming: &HeaderMap, client_side: bool) -> HeaderMap {
    let named = connection_named(incoming);
    let mut kept = HeaderMap::with_capacity(incoming.len());
    for (name, value) in incoming {
        if name == hyper::header::HOST && client_side {
            continue;
        }
        if stops_here(name, &named, client_side) {
            continue;
        }
        // `append`, not `insert`: `set-cookie` arrives once per cookie and an
        // insert would serve one of them and drop the rest.
        kept.append(name.clone(), value.clone());
    }
    kept
}

/// The handshake fields an upgrade needs back, once the hop-by-hop filter has
/// correctly taken them away: the websocket exchange is defined in terms of
/// exactly these, and they are hop-by-hop precisely because each hop must
/// decide them for itself. This hop decides to say what its client said.
pub fn restore_upgrade_handshake(kept: &mut HeaderMap, incoming: &HeaderMap) {
    if let Some(upgrade) = incoming.get(hyper::header::UPGRADE) {
        kept.insert(hyper::header::UPGRADE, upgrade.clone());
        kept.insert(CONNECTION, HeaderValue::from_static("upgrade"));
    }
    for name in incoming.keys() {
        if name.as_str().starts_with("sec-websocket-") {
            for value in incoming.get_all(name) {
                kept.append(name.clone(), value.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(rows: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in rows {
            map.append(
                HeaderName::try_from(*name).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    /// The connection's own fields stop at this hop, in both directions.
    #[test]
    fn a_connection_field_never_crosses_the_hop() {
        let asked = headers(&[
            ("connection", "keep-alive"),
            ("keep-alive", "timeout=5"),
            ("proxy-authorization", "Basic hunter2"),
            ("transfer-encoding", "chunked"),
            ("accept", "text/html"),
        ]);
        let sent = forwardable_request(&asked);
        assert!(sent.get("connection").is_none());
        assert!(sent.get("keep-alive").is_none());
        assert!(sent.get("proxy-authorization").is_none());
        assert!(sent.get("transfer-encoding").is_none());
        assert_eq!(sent.get("accept").unwrap(), "text/html");

        let answered = headers(&[("connection", "close"), ("content-type", "text/html")]);
        let relayed = forwardable_response(&answered);
        assert!(relayed.get("connection").is_none());
        assert_eq!(relayed.get("content-type").unwrap(), "text/html");
    }

    /// A field the `Connection` header names is hop-by-hop by declaration,
    /// whatever it is called.
    #[test]
    fn whatever_connection_names_stops_with_it() {
        let asked = headers(&[
            ("connection", "x-custom-hop, upgrade"),
            ("x-custom-hop", "please forward me"),
            ("x-kept", "ordinary"),
        ]);
        let sent = forwardable_request(&asked);
        assert!(sent.get("x-custom-hop").is_none());
        assert_eq!(sent.get("x-kept").unwrap(), "ordinary");
    }

    /// The body decides framing, so a length copied off the old message is a
    /// promise about bytes this hop may not be sending.
    #[test]
    fn the_old_messages_framing_is_not_the_new_ones() {
        let sent = forwardable_request(&headers(&[("content-length", "12"), ("host", "evil")]));
        assert!(sent.get("content-length").is_none());
        assert!(sent.get("host").is_none());
        // Downstream, though, the length is the upstream's own truthful count
        // and the browser needs it.
        let relayed = forwardable_response(&headers(&[("content-length", "12")]));
        assert_eq!(relayed.get("content-length").unwrap(), "12");
    }

    /// A page cannot forge the forwarding claims a dev server might trust.
    #[test]
    fn a_tab_cannot_forge_its_own_forwarding_headers() {
        let sent = forwardable_request(&headers(&[
            ("x-forwarded-for", "10.0.0.1"),
            ("x-forwarded-host", "admin.internal"),
            ("forwarded", "for=10.0.0.1"),
        ]));
        assert!(sent.is_empty());
    }

    /// Every cookie survives, because the response carries one field per
    /// cookie and only an append keeps them all.
    #[test]
    fn every_set_cookie_line_survives_the_filter() {
        let relayed =
            forwardable_response(&headers(&[("set-cookie", "a=1"), ("set-cookie", "b=2")]));
        let cookies: Vec<_> = relayed.get_all("set-cookie").iter().collect();
        assert_eq!(cookies.len(), 2);
    }

    /// The handshake fields come back deliberately, after the general rule has
    /// correctly refused them.
    #[test]
    fn an_upgrade_handshake_is_put_back_on_purpose() {
        let asked = headers(&[
            ("connection", "Upgrade"),
            ("upgrade", "websocket"),
            ("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ("sec-websocket-version", "13"),
        ]);
        let mut sent = forwardable_request(&asked);
        assert!(sent.get("upgrade").is_none());
        restore_upgrade_handshake(&mut sent, &asked);
        assert_eq!(sent.get("upgrade").unwrap(), "websocket");
        assert_eq!(sent.get("connection").unwrap(), "upgrade");
        assert_eq!(
            sent.get("sec-websocket-key").unwrap(),
            "dGhlIHNhbXBsZSBub25jZQ=="
        );
    }
}
