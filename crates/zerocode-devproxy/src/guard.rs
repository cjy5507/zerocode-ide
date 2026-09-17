//! What this proxy refuses before it looks a route up.
//!
//! The original leans on Node's parser for most of this: `llhttp` collapses a
//! duplicate `Host`, `new URL()` normalises `..` out of a path, and an
//! unhandled `'connect'` event closes the socket. Hyper hands all three
//! through to the service, so the refusals have to be written down.

use hyper::header::HOST;
use hyper::{HeaderMap, Method, Uri};

/// Why a request never reached a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// A tunnel to anywhere is not what a label serves.
    Method,
    /// Two `Host` headers, or one this hop cannot read.
    Host,
    /// A target that names a place rather than a path.
    Target,
}

impl Refusal {
    /// The status a refusal answers with.
    pub fn status(self) -> hyper::StatusCode {
        match self {
            Refusal::Method => hyper::StatusCode::METHOD_NOT_ALLOWED,
            Refusal::Host | Refusal::Target => hyper::StatusCode::BAD_REQUEST,
        }
    }
}

/// The one `Host` a request is allowed to carry.
///
/// Hyper delivers every copy; taking the first while the dev server takes the
/// last is how one request becomes two different requests, so a second copy is
/// a refusal rather than a choice.
pub fn single_host(headers: &HeaderMap) -> Result<&str, Refusal> {
    let mut hosts = headers.get_all(HOST).iter();
    let first = hosts.next().ok_or(Refusal::Host)?;
    if hosts.next().is_some() {
        return Err(Refusal::Host);
    }
    first.to_str().map_err(|_| Refusal::Host)
}

/// The path and query to forward, from a request target this hop trusts.
///
/// An absolute-form target keeps its path and loses its authority — the
/// authority of the hop is decided by the route, never by the request, which
/// is the same conclusion the original reaches by rebuilding the URL against
/// the registered target (`localhost-worktree-label-proxy.ts:236-242`).
/// A dot segment is refused rather than resolved: normalising it here would
/// hand the dev server a path its own traversal guards never saw.
pub fn forwardable_target(uri: &Uri) -> Result<String, Refusal> {
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .ok_or(Refusal::Target)?;
    if !path_and_query.starts_with('/') {
        return Err(Refusal::Target);
    }
    if has_dot_segment(path_and_query) {
        return Err(Refusal::Target);
    }
    Ok(path_and_query.to_string())
}

/// Whether a target walks upwards, spelled plainly or in percent-escapes.
fn has_dot_segment(path_and_query: &str) -> bool {
    let path = path_and_query
        .split(['?', '#'])
        .next()
        .unwrap_or(path_and_query)
        .to_ascii_lowercase();
    path.split('/')
        .any(|segment| matches!(segment, ".." | "%2e." | ".%2e" | "%2e%2e"))
}

/// Whether the method is one a labeled dev server can answer at all.
pub fn permitted_method(method: &Method) -> Result<(), Refusal> {
    if method == Method::CONNECT {
        return Err(Refusal::Method);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::{HeaderMap, HeaderValue};

    fn hosts(values: &[&str]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for value in values {
            map.append(HOST, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    /// One host, or none of them.
    #[test]
    fn a_second_host_header_is_refused_rather_than_resolved() {
        assert_eq!(
            single_host(&hosts(&["a.zerocode.localhost"])),
            Ok("a.zerocode.localhost")
        );
        assert_eq!(single_host(&hosts(&["a", "b"])), Err(Refusal::Host));
        assert_eq!(single_host(&HeaderMap::new()), Err(Refusal::Host));
    }

    /// A host this hop cannot read as text cannot be compared with a label
    /// either, so it is refused rather than converted lossily.
    #[test]
    fn a_host_that_is_not_text_is_refused() {
        let mut map = HeaderMap::new();
        map.append(HOST, HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap());
        assert_eq!(single_host(&map), Err(Refusal::Host));
    }

    /// The authority of the hop belongs to the route; an absolute-form target
    /// keeps only its path.
    #[test]
    fn an_absolute_form_target_keeps_only_its_path_and_query() {
        let uri: Uri = "http://evil.example/y?q=1".parse().unwrap();
        assert_eq!(forwardable_target(&uri).unwrap(), "/y?q=1");
    }

    /// A path that begins with two slashes is a path, not an authority — the
    /// forwarding builds its own URI, so this only has to stay a path.
    #[test]
    fn a_protocol_relative_path_stays_a_path() {
        let uri: Uri = "//evil.example/x".parse().unwrap();
        assert_eq!(forwardable_target(&uri).unwrap(), "//evil.example/x");
    }

    /// Walking upwards is refused however it is spelled, because normalising
    /// it here would hide it from the dev server's own guards.
    #[test]
    fn a_dot_dot_segment_is_refused_encoded_or_not() {
        for walked in [
            "/../etc/passwd",
            "/app/../../secret",
            "/%2e%2e/secret",
            "/%2E%2E/secret",
            "/app/%2e./x",
            "/app/.%2e/x",
        ] {
            let uri: Uri = walked.parse().unwrap();
            assert_eq!(forwardable_target(&uri), Err(Refusal::Target), "{walked}");
        }
        // A dot inside a name is not a dot segment.
        let ordinary: Uri = "/assets/..name/app..js?v=..".parse().unwrap();
        assert!(forwardable_target(&ordinary).is_ok());
    }

    /// A tunnel to anywhere is not what a label serves.
    #[test]
    fn a_connect_request_is_refused() {
        assert_eq!(permitted_method(&Method::CONNECT), Err(Refusal::Method));
        assert_eq!(
            Refusal::Method.status(),
            hyper::StatusCode::METHOD_NOT_ALLOWED
        );
        for allowed in [Method::GET, Method::POST, Method::OPTIONS, Method::DELETE] {
            assert_eq!(permitted_method(&allowed), Ok(()));
        }
    }
}
