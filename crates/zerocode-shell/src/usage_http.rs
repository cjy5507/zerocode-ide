//! The one road a usage reader takes to a server, and the named failure it
//! comes back with.
//!
//! Three providers ask three different endpoints for the same kind of answer,
//! and before this each carried its own client, its own ten seconds, and its
//! own idea of what a refusal was. One HTTP layer formatted the status into a
//! sentence (`"HTTP 401"`) and the refresh
//! road recovered it with `error.contains("401")` — control flow through a
//! human string, which stops working the day somebody translates it or a
//! server puts a 401 in its prose.
//!
//! So the road is one, and what comes back off it is a [`Failure`] carrying
//! the kind ([`zerocode_core::usage_limit`]) rather than a sentence to be
//! read again later. Provider N+1 is a credential reader and a body parser;
//! this file is everything between.

use std::time::Duration;

use zerocode_core::civil::days_from_civil;
use zerocode_core::usage_limit;

/// How long a server gets to answer.
///
/// Ten seconds, shared by the usage readers.
/// Kimi and MiniMax give fifteen (`minimax-fetcher.ts:21`) — a provider that
/// needs its own budget passes one rather than widening this for everybody.
pub(crate) const API_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub(crate) struct Failure {
    /// What it failed of, and which roads that leaves open.
    pub(crate) recovery: usage_limit::Recovery,
    /// Unix ms before which a refetch should not be attempted — the server's
    /// own `Retry-After`, stamped absolute the way Orca stamps it
    /// (`claude-fetcher.ts:663,670`).
    pub(crate) retry_at_ms: Option<i64>,
    /// What went wrong, for a log rather than for a status bar.
    pub(crate) message: String,
    /// Whether the vendor's terminal road should be SKIPPED after this.
    ///
    /// Orca keeps this apart from the classifier's `shouldAttemptCliFallback`
    /// on purpose, and so does this: the classifier answers "would renewing a
    /// token help", while this answers "has the API already given the
    /// user-visible answer". A 401 says yes to the first and yes to the
    /// second — the CLI would spawn an agent process only to be told the same
    /// thing (`claude-oauth-usage-error.ts:19-21`).
    pub(crate) skip_cli_fallback: bool,
}

impl Failure {
    /// Nothing on file to authenticate with.
    pub(crate) fn no_credentials() -> Self {
        Self {
            recovery: usage_limit::classify_absent_credentials(false, false, false),
            retry_at_ms: None,
            message: "no credentials on file".to_string(),
            skip_cli_fallback: false,
        }
    }

    /// Authenticated, and the account has no plan figures to report.
    pub(crate) fn no_plan() -> Self {
        Self {
            recovery: usage_limit::Recovery {
                kind: usage_limit::FailureKind::UsageUnavailable,
                cli_fallback: false,
                delegated_refresh: false,
                terminal: true,
            },
            retry_at_ms: None,
            message: "no plan figures in the response".to_string(),
            skip_cli_fallback: false,
        }
    }

    /// The request never reached a server, or the client could not be built.
    pub(crate) fn transport(message: &str) -> Self {
        Self {
            recovery: usage_limit::classify_transport(message, false),
            retry_at_ms: None,
            message: message.to_string(),
            skip_cli_fallback: false,
        }
    }

    /// The answer arrived and would not parse.
    pub(crate) fn unreadable(message: &str) -> Self {
        Self {
            recovery: usage_limit::classify_transport(message, true),
            retry_at_ms: None,
            message: message.to_string(),
            skip_cli_fallback: false,
        }
    }
}

/// One GET, ten seconds, JSON or a named failure. Blocking on purpose: the
/// scans run on their own thread, and `tauri::async_runtime::block_on` is the
/// One request, sent and read as JSON — or a named failure.
///
/// Blocking on purpose: the scans run on their own thread, and
/// `tauri::async_runtime::block_on` is the same bridge the hook bridge start
/// uses.
///
/// The whole `Response` used to be dropped on any non-success, which took the
/// STATUS CODE with it — so a 429, a 500 and a name that would not resolve
/// arrived at the caller as the same nothing, and the bar answered all three
/// by blanking and asking again on the ordinary cadence.
/// The longest page body this road will hold.
///
/// Orca checks the same 10 MB — but AFTER `await res.text()`, so the whole
/// page is already in memory by the time the check runs
/// (`opencode-go-page-scraper.ts:127-132`). Refusing before the read is the
/// point of having a limit at all.
pub(crate) const MAX_BODY_BYTES: u64 = 10_000_000;

/// A body, read as text, refused before it is held if the server declares it
/// too large.
pub(crate) fn get_text(
    url: &str,
    headers: &[(&str, &str)],
    now_ms: i64,
) -> Result<String, Failure> {
    let mut request = client()?.get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    send_reading(request, now_ms, |response| {
        tauri::async_runtime::block_on(async {
            if response
                .content_length()
                .is_some_and(|held| held > MAX_BODY_BYTES)
            {
                return Err(Failure::unreadable("응답 본문이 너무 큽니다"));
            }
            let text = response
                .text()
                .await
                .map_err(|error| Failure::unreadable(&error.to_string()))?;
            if u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_BODY_BYTES {
                return Err(Failure::unreadable("응답 본문이 너무 큽니다"));
            }
            Ok(text)
        })
    })
}

fn send(request: reqwest::RequestBuilder, now_ms: i64) -> Result<serde_json::Value, Failure> {
    send_reading(request, now_ms, |response| {
        tauri::async_runtime::block_on(async {
            response
                .json::<serde_json::Value>()
                .await
                .map_err(|error| Failure::unreadable(&error.to_string()))
        })
    })
}

/// Everything the two readers share: one round trip, and one place where a
/// non-success answer becomes a typed failure with the server's own
/// `Retry-After` on it. Only the reading of a SUCCESSFUL body differs, which
/// is the closure.
fn send_reading<T>(
    request: reqwest::RequestBuilder,
    now_ms: i64,
    read: impl FnOnce(reqwest::Response) -> Result<T, Failure>,
) -> Result<T, Failure> {
    tauri::async_runtime::block_on(async {
        let response = request
            .send()
            .await
            .map_err(|error| Failure::transport(&error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let status = status.as_u16();
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            // The body is read for the ONE thing it decides: a 403 naming a
            // scope is a different failure from a 403 that means "expired"
            // (`claude-usage-error-classification.ts:20-22`).
            let body = response.text().await.unwrap_or_default();
            return Err(Failure {
                recovery: usage_limit::classify_http(status, &body),
                retry_at_ms: usage_limit::retry_after_ms(
                    status,
                    retry_after.as_deref(),
                    now_ms,
                    http_date_ms,
                )
                .map(|wait| now_ms + wait),
                message: format!("HTTP {status}"),
                skip_cli_fallback: usage_limit::skips_pty_fallback(status),
            });
        }
        Ok(response)
    })
    .and_then(read)
}

/// A client with this road's budget, or the failure that stopped it being
/// built.
fn client() -> Result<reqwest::Client, Failure> {
    reqwest::Client::builder()
        .timeout(API_TIMEOUT)
        .build()
        .map_err(|error| Failure::transport(&error.to_string()))
}

/// GET, with headers, answering JSON.
pub(crate) fn get_json(
    url: &str,
    headers: &[(&str, &str)],
    now_ms: i64,
) -> Result<serde_json::Value, Failure> {
    let mut request = client()?.get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    send(request, now_ms)
}

/// POST a JSON body as a bearer, answering JSON.
///
/// The second verb this road needs: quota is asked for rather than looked up,
/// and the project it is billed to is asked for the same way.
pub(crate) fn post_json_bearer(
    url: &str,
    access_token: &str,
    headers: &[(&str, &str)],
    body: &serde_json::Value,
    now_ms: i64,
) -> Result<serde_json::Value, Failure> {
    let mut request = client()?
        .post(url)
        .header("Authorization", format!("Bearer {access_token}"));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request.json(body);
    send(request, now_ms)
}

/// POST a form-encoded body, answering JSON.
///
/// The third shape this area needs and the only one that is not a bearer
/// asking a product API: OAuth token endpoints speak
/// `application/x-www-form-urlencoded`, so a refresh comes through here.
pub(crate) fn post_form(
    url: &str,
    form: String,
    now_ms: i64,
) -> Result<serde_json::Value, Failure> {
    let request = client()?
        .post(url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form);
    send(request, now_ms)
}

/// RFC 9110's IMF-fixdate — `Sun, 06 Nov 1994 08:49:37 GMT` — in Unix ms.
///
/// The one form `Retry-After` may take besides a count of seconds. Only the
/// fixed-width GMT spelling is read: RFC 9110 requires senders to use it, and
/// the two obsolete forms it allows recipients to accept have not been seen
/// from either endpoint this road talks to. The calendar itself is
/// [`days_from_civil`], which the ISO reader in `usage_oauth` shares — this is
/// a spelling, not a second arithmetic.
pub(crate) fn http_date_ms(said: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, after_day_name) = said.split_once(", ")?;
    let mut parts = after_day_name.split(' ');
    let day: u32 = parts.next()?.parse().ok()?;
    let month_name = parts.next()?;
    let year: i64 = parts.next()?.parse().ok()?;
    let clock = parts.next()?;
    if parts.next()? != "GMT" || parts.next().is_some() {
        return None;
    }
    let month = u32::try_from(MONTHS.iter().position(|held| *held == month_name)? + 1).ok()?;
    if day == 0 || day > 31 {
        return None;
    }
    let mut hands = clock.split(':');
    let hour: i64 = hands.next()?.parse().ok()?;
    let minute: i64 = hands.next()?.parse().ok()?;
    let second: i64 = hands.next()?.parse().ok()?;
    if hands.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + hour * 3_600 + minute * 60 + second) * 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Retry-After` may carry a date instead of a count of seconds, and the
    /// calendar it is read with is the one already here for the ISO road.
    #[test]
    fn a_retry_after_date_reads_as_the_instant_it_names() {
        // The RFC's own example instant: 784111777 seconds after the epoch.
        assert_eq!(
            http_date_ms("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(784_111_777_000)
        );
        assert_eq!(http_date_ms("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        // A leap day is a day, and a leap second is a second the clock allows.
        assert_eq!(
            http_date_ms("Sat, 29 Feb 2020 12:00:00 GMT"),
            Some(1_582_977_600_000)
        );
        assert!(http_date_ms("Fri, 31 Dec 1999 23:59:60 GMT").is_some());

        // Anything else is nothing rather than a guess: the obsolete spellings
        // RFC 9110 lets a recipient accept, a zone we did not ask for, a day
        // that is not one, and trailing rubbish.
        for said in [
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
            "Sun, 06 Nov 1994 08:49:37 PST",
            "Sun, 00 Nov 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 24:00:00 GMT",
            "Sun, 06 Foo 1994 08:49:37 GMT",
            "Sun, 06 Nov 1994 08:49:37 GMT extra",
            "120",
            "",
        ] {
            assert_eq!(http_date_ms(said), None, "{said} was read as a date");
        }
    }

    /// A failure carries what it failed OF, so the caller can decide with it.
    #[test]
    fn a_named_failure_carries_its_kind_and_the_servers_own_wait() {
        use zerocode_core::usage_limit::FailureKind;

        assert_eq!(
            Failure::no_credentials().recovery.kind,
            FailureKind::MissingCredentials
        );
        assert_eq!(
            Failure::no_plan().recovery.kind,
            FailureKind::UsageUnavailable
        );
        assert!(Failure::no_plan().recovery.terminal);
        assert_eq!(
            Failure::transport("error sending request: dns error")
                .recovery
                .kind,
            FailureKind::Network
        );
        assert_eq!(
            Failure::unreadable("expected value at line 1")
                .recovery
                .kind,
            FailureKind::Parse
        );
        // None of these can name a time to come back at — only a server can.
        assert!(Failure::no_credentials().retry_at_ms.is_none());
        assert!(Failure::transport("boom").retry_at_ms.is_none());
    }
}
