use std::collections::BTreeSet;
use std::error::Error as StdError;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use reqwest::{Client, RequestBuilder, Response};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{
    from_value, maybe_enforce_permission_check, to_pretty_json, ToolContext, ToolError, ToolSpec,
};
use crate::http_bridge::run_http;
use runtime::PermissionMode;

const MAX_WEB_RESPONSE_BYTES: u64 = 256 * 1024;

#[derive(Debug, Deserialize)]
pub(crate) struct WebFetchInput {
    pub url: String,
    /// Required by the schema (Claude Code parity) and by deserialization — a
    /// call without it is rejected. It states what the caller is looking for,
    /// but it intentionally does NOT alter the output: the full page body is
    /// returned for the model to read, with no server-side summary and no magic
    /// "title"/"summary" substring branching (removed 2026-07). Kept read-free on
    /// purpose, hence the allow.
    #[allow(dead_code)]
    pub prompt: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WebSearchInput {
    pub query: String,
    pub allowed_domains: Option<Vec<String>>,
    pub blocked_domains: Option<Vec<String>>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct WebSearchOutput {
    pub query: String,
    pub results: Vec<WebSearchResultItem>,
    #[serde(rename = "durationSeconds")]
    pub duration_seconds: f64,
}

#[derive(Debug, serde::Serialize)]
#[serde(untagged)]
pub(crate) enum WebSearchResultItem {
    SearchResult {
        tool_use_id: String,
        content: Vec<SearchHit>,
    },
    Commentary(String),
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct SearchHit {
    pub title: String,
    pub url: String,
}

pub(crate) fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "WebFetch",
            description:
                "Fetch a URL and return its readable page body as lightweight markdown \
                 (headings, lists, links preserved), with a Title/URL/Status header. \
                 The full body is returned up to a size cap; a larger page is digested \
                 (head+tail) and the complete text stays recoverable via \
                 retrieve_tool_output. `prompt` states what you are looking for — the \
                 body is returned for you to read (it is not summarised for you).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "format": "uri" },
                    "prompt": { "type": "string" }
                },
                "required": ["url", "prompt"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
        ToolSpec {
            name: "WebSearch",
            description: "Search the web for current information and return cited results.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "minLength": 2 },
                    "allowed_domains": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "blocked_domains": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            required_permission: PermissionMode::ReadOnly,
        },
    ]
}

pub(crate) fn dispatch(
    _ctx: &ToolContext,
    enforcer: Option<&runtime::permission_enforcer::PermissionEnforcer>,
    name: &str,
    input: &Value,
) -> Option<Result<String, ToolError>> {
    match name {
        "WebFetch" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<WebFetchInput>(input).and_then(|input| run_web_fetch(&input))
            }),
        ),
        "WebSearch" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<WebSearchInput>(input).and_then(|input| run_web_search(&input))
            }),
        ),
        _ => None,
    }
}

pub(crate) fn run_web_fetch(input: &WebFetchInput) -> Result<String, ToolError> {
    execute_web_fetch(input)
}

pub(crate) fn run_web_search(input: &WebSearchInput) -> Result<String, ToolError> {
    to_pretty_json(execute_web_search(input)?)
}

/// Fetch `input.url` and return its readable body as plain-text markdown with a
/// short metadata header. The full converted body is returned here; the dispatch
/// truncation seam caps the model-facing size (head+tail digest) and preserves
/// the whole body as a recoverable artifact — this function does not itself cap.
fn execute_web_fetch(input: &WebFetchInput) -> Result<String, ToolError> {
    run_http(async {
        let client = shared_http_client();
        let request_url = normalize_fetch_url(&input.url)?;
        let response = send_web_request(
            client.get(request_url.clone()),
            WebRequestKind::Fetch,
            request_url.clone(),
        )
        .await?;

        let status = response.status();
        let final_url = response.url().to_string();
        let code = status.as_u16();
        let code_text = status.canonical_reason().unwrap_or("Unknown").to_string();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let (body, bytes) = read_response_text_limited(response).await?;
        let title = extract_title(&body, &content_type);
        let normalized = normalize_fetched_content(&body, &content_type);

        Ok(render_fetch_result(&FetchMeta {
            url: &final_url,
            title: title.as_deref(),
            code,
            code_text: &code_text,
            content_type: &content_type,
            bytes,
            body: &normalized,
        }))
    })
}

/// Assembled inputs for [`render_fetch_result`]. Grouping them keeps the render
/// call readable and lets the pure formatter be unit-tested without a network.
struct FetchMeta<'a> {
    url: &'a str,
    title: Option<&'a str>,
    code: u16,
    code_text: &'a str,
    content_type: &'a str,
    bytes: usize,
    body: &'a str,
}

/// Format the final `WebFetch` result: a metadata header (Title/URL cyan-label
/// lines the TUI recognises, plus a compact status line) followed by a blank
/// line and the readable page body. Honest by construction — the header exposes
/// what was fetched and the body is returned verbatim; nothing is summarised and
/// there is no magic `prompt` branching (the old "title"/"summary" substring
/// special-cases were removed — every prompt gets the same body + header).
fn render_fetch_result(meta: &FetchMeta<'_>) -> String {
    let mut header = String::new();
    if let Some(title) = meta.title {
        let title = title.trim();
        if !title.is_empty() {
            header.push_str("Title: ");
            header.push_str(&preview_text(title, 300));
            header.push('\n');
        }
    }
    header.push_str("URL: ");
    header.push_str(meta.url);
    header.push('\n');

    header.push_str("Status: ");
    header.push_str(&meta.code.to_string());
    header.push(' ');
    header.push_str(meta.code_text);
    let content_type = meta.content_type.split(';').next().unwrap_or("").trim();
    if !content_type.is_empty() {
        header.push_str(" · ");
        header.push_str(content_type);
    }
    header.push_str(" · ");
    header.push_str(&human_byte_size(meta.bytes));
    header.push('\n');

    let body = meta.body.trim();
    if body.is_empty() {
        format!("{header}\n(no readable content)")
    } else {
        format!("{header}\n{body}")
    }
}

/// Compact, float-free byte figure (`812B` / `12KB` / `3MB`) for the status line.
fn human_byte_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{}KB", bytes / 1024)
    } else {
        format!("{}MB", bytes / (1024 * 1024))
    }
}

fn execute_web_search(input: &WebSearchInput) -> Result<WebSearchOutput, ToolError> {
    let started = Instant::now();
    run_http(async {
        let client = shared_http_client();
        if let Some(direct_url) = direct_search_url(&input.query)? {
            return execute_direct_url_search(input, &client, direct_url, started).await;
        }

        let search_url = build_search_url(&input.query)?;
        let display_url = display_url_without_query(&search_url);
        let response = send_web_request(
            client.get(search_url),
            WebRequestKind::SearchBackend,
            display_url,
        )
        .await?;

        let final_url = response.url().clone();
        let (html, _) = read_response_text_limited(response).await?;
        let mut hits = extract_search_hits(&html);

        if hits.is_empty() && final_url.host_str().is_some() {
            hits = extract_search_hits_from_generic_links(&html);
        }

        Ok(finish_web_search(input, hits, started, None))
    })
}

async fn execute_direct_url_search(
    input: &WebSearchInput,
    client: &Client,
    direct_url: String,
    started: Instant,
) -> Result<WebSearchOutput, ToolError> {
    let response = send_web_request(
        client.get(direct_url.clone()),
        WebRequestKind::DirectSearch,
        direct_url,
    )
    .await?;
    let final_url = response.url().to_string();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let (body, _) = read_response_text_limited(response).await?;
    let title = extract_title(&body, &content_type)
        .map_or_else(|| final_url.clone(), |title| preview_text(&title, 160));
    let hits = vec![SearchHit {
        title,
        url: final_url.clone(),
    }];

    Ok(finish_web_search(input, hits, started, Some(&final_url)))
}

fn finish_web_search(
    input: &WebSearchInput,
    mut hits: Vec<SearchHit>,
    started: Instant,
    direct_url: Option<&str>,
) -> WebSearchOutput {
    apply_search_filters_and_limits(input, &mut hits);

    let summary = if hits.is_empty() {
        match direct_url {
            Some(url) => format!(
                "Direct URL search for {url:?} succeeded, but no results matched the domain filters."
            ),
            None => format!("No web search results matched the query {:?}.", input.query),
        }
    } else {
        let rendered_hits = hits
            .iter()
            .map(|hit| format!("- [{}]({})", hit.title, hit.url))
            .collect::<Vec<_>>()
            .join("\n");
        match direct_url {
            Some(url) => format!(
                "Direct URL result for {url:?}. Include a Sources section in the final answer.\n{rendered_hits}"
            ),
            None => format!(
                "Search results for {:?}. Include a Sources section in the final answer.\n{}",
                input.query, rendered_hits
            ),
        }
    };

    WebSearchOutput {
        query: input.query.clone(),
        results: vec![
            WebSearchResultItem::Commentary(summary),
            WebSearchResultItem::SearchResult {
                tool_use_id: String::from("web_search_1"),
                content: hits,
            },
        ],
        duration_seconds: started.elapsed().as_secs_f64(),
    }
}

fn apply_search_filters_and_limits(input: &WebSearchInput, hits: &mut Vec<SearchHit>) {
    if let Some(allowed) = input.allowed_domains.as_ref() {
        hits.retain(|hit| host_matches_list(&hit.url, allowed));
    }
    if let Some(blocked) = input.blocked_domains.as_ref() {
        hits.retain(|hit| !host_matches_list(&hit.url, blocked));
    }

    dedupe_hits(hits);
    hits.truncate(8);
}

/// Process-wide shared `reqwest::Client` for `WebFetch` / `WebSearch`.
///
/// Building a client per call re-initialises the TLS backend and root-cert
/// store and discards the connection pool every time. Caching one client in a
/// `OnceLock` keeps TLS sessions and keep-alive connections warm across tool
/// invocations; `reqwest::Client` is `Arc`-backed so `clone()` is free. This
/// mirrors `api::providers::shared_http_client` and `http_bridge`'s singleton
/// runtime. Timeout / redirect / user-agent behaviour is unchanged.
fn shared_http_client() -> Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            Client::builder()
                .timeout(Duration::from_secs(20))
                // Re-validate every redirect hop against the SSRF guard: a public
                // page can 30x toward an internal address, so `Policy::limited`
                // alone is not enough. Cap at 10 hops as before.
                .redirect(reqwest::redirect::Policy::custom(|attempt| {
                    if attempt.previous().len() >= 10 {
                        return attempt.error("too many redirects");
                    }
                    match guard_against_ssrf(attempt.url()) {
                        Ok(()) => attempt.follow(),
                        Err(error) => attempt.error(error.to_string()),
                    }
                }))
                .user_agent("zo-rust-tools/0.1")
                .build()
                .unwrap_or_else(|_| Client::new())
        })
        .clone()
}

#[derive(Debug, Clone, Copy)]
enum WebRequestKind {
    Fetch,
    SearchBackend,
    DirectSearch,
    BodyRead,
}

async fn send_web_request(
    request: RequestBuilder,
    kind: WebRequestKind,
    display_url: String,
) -> Result<Response, ToolError> {
    request
        .send()
        .await
        .map_err(|error| web_request_error(kind, &display_url, &error))
}

fn web_request_error(kind: WebRequestKind, display_url: &str, error: &reqwest::Error) -> ToolError {
    let location = error.url().map_or_else(
        || compact_url_for_error(display_url),
        display_url_without_query,
    );
    let category = if error.is_timeout() {
        "request timed out"
    } else if error.is_connect() {
        "network connection failed"
    } else if error.is_redirect() {
        "redirect handling failed"
    } else if error.is_request() {
        "request could not be built or sent"
    } else if error.is_body() || error.is_decode() {
        "response body could not be read"
    } else {
        "request failed"
    };
    let source = error
        .source()
        .map(|source| format!(" ({})", compact_error_detail(&source.to_string())))
        .unwrap_or_default();
    let hint = match kind {
        WebRequestKind::Fetch | WebRequestKind::DirectSearch => {
            "Check local network/proxy/VPN access for this URL."
        }
        WebRequestKind::SearchBackend => {
            "The search backend may be blocked; use WebFetch for known URLs or retry when network access is available."
        }
        WebRequestKind::BodyRead => {
            "The server closed or corrupted the response body; retry the request."
        }
    };
    ToolError::Execution(format!(
        "{} failed for {location}: {category}{source}. {hint}",
        kind.operation_label()
    ))
}

impl WebRequestKind {
    fn operation_label(self) -> &'static str {
        match self {
            Self::Fetch => "web fetch request",
            Self::SearchBackend => "web search backend request",
            Self::DirectSearch => "direct URL search request",
            Self::BodyRead => "web response body read",
        }
    }
}

fn direct_search_url(query: &str) -> Result<Option<String>, ToolError> {
    let trimmed = query.trim();
    let Ok(parsed) = reqwest::Url::parse(trimmed) else {
        return Ok(None);
    };
    if matches!(parsed.scheme(), "http" | "https") {
        normalize_fetch_url(trimmed).map(Some)
    } else {
        Ok(None)
    }
}

fn display_url_without_query(url: &reqwest::Url) -> String {
    let mut url = url.clone();
    url.set_query(None);
    url.set_fragment(None);
    compact_url_for_error(url.as_str())
}

fn compact_url_for_error(url: &str) -> String {
    const MAX_URL_CHARS: usize = 140;
    let mut compact = url.trim().to_string();
    if compact.chars().count() > MAX_URL_CHARS {
        compact = format!(
            "{}…",
            compact.chars().take(MAX_URL_CHARS).collect::<String>()
        );
    }
    compact
}

fn compact_error_detail(detail: &str) -> String {
    const MAX_DETAIL_CHARS: usize = 180;
    let detail = collapse_whitespace(detail);
    if detail.chars().count() <= MAX_DETAIL_CHARS {
        return detail;
    }
    format!(
        "{}…",
        detail.chars().take(MAX_DETAIL_CHARS).collect::<String>()
    )
}

async fn read_response_text_limited(response: Response) -> Result<(String, usize), ToolError> {
    let bytes = read_response_bytes_limited(response).await?;
    let length = bytes.len();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    Ok((body, length))
}

async fn read_response_bytes_limited(mut response: Response) -> Result<Vec<u8>, ToolError> {
    let mut bytes = Vec::new();
    let max_bytes = usize::try_from(MAX_WEB_RESPONSE_BYTES)
        .map_err(|error| ToolError::Execution(error.to_string()))?;

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| web_request_error(WebRequestKind::BodyRead, "response body", &error))?
    {
        let remaining = max_bytes.saturating_sub(bytes.len());
        if remaining == 0 {
            break;
        }
        let take_len = chunk.len().min(remaining);
        bytes.extend_from_slice(&chunk[..take_len]);
        if take_len < chunk.len() || bytes.len() >= max_bytes {
            break;
        }
    }

    Ok(bytes)
}

fn normalize_fetch_url(url: &str) -> Result<String, ToolError> {
    let parsed =
        reqwest::Url::parse(url).map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    let normalized = if parsed.scheme() == "http" {
        let host = parsed.host_str().unwrap_or_default();
        if host != "localhost" && host != "127.0.0.1" && host != "::1" {
            let mut upgraded = parsed.clone();
            upgraded
                .set_scheme("https")
                .map_err(|()| ToolError::Execution("failed to upgrade URL to https".into()))?;
            upgraded
        } else {
            parsed
        }
    } else {
        parsed
    };
    guard_against_ssrf(&normalized)?;
    Ok(normalized.to_string())
}

/// `true` when the operator has opted into fetching local/private targets
/// (a dev server, an internal mirror). Off by default so the SSRF guard holds.
fn web_local_access_allowed() -> bool {
    std::env::var("ZO_WEB_ALLOW_LOCAL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// SSRF guard: refuse non-HTTP schemes and any URL whose host resolves to a
/// loopback / link-local / private / reserved address. Resolving the host (not
/// just inspecting the literal) defends against a domain that points at an
/// internal address; applied at entry AND on every redirect hop so a 30x to
/// `169.254.169.254` (cloud IMDS), RFC1918, or localhost is blocked. This is a
/// best-effort guard — a DNS rebind between this check and reqwest's own
/// connect resolution is not covered (would require a custom connector). Opt
/// out for deliberate local fetches with `ZO_WEB_ALLOW_LOCAL=1`.
fn guard_against_ssrf(url: &reqwest::Url) -> Result<(), ToolError> {
    if web_local_access_allowed() {
        return Ok(());
    }
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(ToolError::InvalidInput(format!(
                "WebFetch refuses non-HTTP scheme: {other}"
            )));
        }
    }
    let host = url
        .host_str()
        .ok_or_else(|| ToolError::InvalidInput("WebFetch URL has no host".into()))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let mut resolved = (host, port)
        .to_socket_addrs()
        .map_err(|error| ToolError::Execution(format!("cannot resolve {host}: {error}")))?
        .peekable();
    if resolved.peek().is_none() {
        return Err(ToolError::Execution(format!("cannot resolve {host}")));
    }
    for addr in resolved {
        if !is_public_ip(addr.ip()) {
            return Err(ToolError::InvalidInput(format!(
                "WebFetch refuses a non-public address ({}) — set ZO_WEB_ALLOW_LOCAL=1 to allow local targets",
                addr.ip()
            )));
        }
    }
    Ok(())
}

/// `false` for any address that must never be reachable via a fetched URL:
/// loopback, link-local (incl. cloud IMDS `169.254.0.0/16`), private/RFC1918,
/// CGNAT, documentation, unspecified, and their IPv6 equivalents.
fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || o[0] == 0
                // carrier-grade NAT 100.64.0.0/10
                || (o[0] == 100 && (o[1] & 0xc0) == 64))
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(mapped));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // unique local fc00::/7
                || (first & 0xfe00) == 0xfc00
                // link-local fe80::/10
                || (first & 0xffc0) == 0xfe80)
        }
    }
}

fn build_search_url(query: &str) -> Result<reqwest::Url, ToolError> {
    if let Ok(base) = std::env::var("ZO_WEB_SEARCH_BASE_URL") {
        let mut url = reqwest::Url::parse(&base)
            .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
        url.query_pairs_mut().append_pair("q", query);
        return Ok(url);
    }

    let mut url = reqwest::Url::parse("https://html.duckduckgo.com/html/")
        .map_err(|error| ToolError::Execution(error.to_string()))?;
    url.query_pairs_mut().append_pair("q", query);
    Ok(url)
}

fn normalize_fetched_content(body: &str, content_type: &str) -> String {
    if content_type.contains("html") {
        html_to_markdown(body)
    } else {
        body.trim().to_string()
    }
}

/// Extract the document `<title>` from raw HTML. `None` for non-HTML content or
/// when no non-empty title is present. Tolerates attributes on the tag
/// (`<title data-x="…">`).
fn extract_title(raw_body: &str, content_type: &str) -> Option<String> {
    if !content_type.contains("html") {
        return None;
    }
    let open = find_ci(raw_body, "<title")?;
    let after_open = open + raw_body[open..].find('>')? + 1;
    let close_rel = find_ci(&raw_body[after_open..], "</title>")?;
    let title = collapse_whitespace(&decode_html_entities(
        &raw_body[after_open..after_open + close_rel],
    ));
    (!title.is_empty()).then_some(title)
}

/// Flatten HTML to a single collapsed line of text (tags dropped, entities
/// decoded). Used for short fields — search-hit anchor titles and the like —
/// where structure is noise. `html_to_markdown` is the structure-preserving
/// path for page bodies.
fn html_inline_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut previous_was_space = false;

    for ch in html.chars() {
        match ch {
            '<' => {
                if !text.is_empty() && !previous_was_space {
                    text.push(' ');
                    previous_was_space = true;
                }
                in_tag = true;
            }
            '>' => in_tag = false,
            _ if in_tag => {}
            '&' => {
                text.push('&');
                previous_was_space = false;
            }
            ch if ch.is_whitespace() => {
                if !previous_was_space {
                    text.push(' ');
                    previous_was_space = true;
                }
            }
            _ => {
                text.push(ch);
                previous_was_space = false;
            }
        }
    }

    collapse_whitespace(&decode_html_entities(&text))
}

/// Convert an HTML document into lightweight, agent-readable markdown.
///
/// This is a deliberately small, dependency-free readability pass (no DOM): it
/// drops non-content elements (`script`/`style`/`head`/`svg`/comments), maps
/// block elements to line breaks, headings to `#`, list items to `- `, and
/// anchors to `[text](href)`, while collapsing insignificant whitespace so the
/// page reads like prose instead of one flat line. It is best-effort, not a
/// spec-compliant parser: `<pre>` whitespace is not preserved and inline
/// emphasis markup is dropped, which keeps the output clean for a model to read.
fn html_to_markdown(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut pending_href: Option<String> = None;
    let mut rest = html;

    while let Some(lt) = rest.find('<') {
        append_inline_text(&mut out, &rest[..lt]);
        rest = &rest[lt..]; // now positioned at '<'

        // HTML comment: skip to the closing `-->`.
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = after.find("-->").map_or("", |end| &after[end + 3..]);
            continue;
        }

        let Some(gt) = rest.find('>') else {
            // Unterminated tag: treat the remainder as text and stop.
            append_inline_text(&mut out, &rest[1..]);
            rest = "";
            break;
        };
        let raw_tag = &rest[1..gt];
        rest = &rest[gt + 1..];

        let (name, closing) = parse_tag(raw_tag);
        if name.is_empty() {
            continue;
        }

        // Elements whose *content* is not readable body text: skip to the close.
        if !closing
            && matches!(
                name.as_str(),
                "script" | "style" | "head" | "noscript" | "svg" | "template" | "title"
            )
        {
            rest = skip_to_element_close(rest, &name);
            continue;
        }

        apply_structural_tag(&mut out, &name, closing, raw_tag, &mut pending_href);
    }
    append_inline_text(&mut out, rest);

    finalize_markdown(&out)
}

/// `(lowercased-name, is_closing)` for a tag body (the text between `<` and `>`).
fn parse_tag(raw_tag: &str) -> (String, bool) {
    let trimmed = raw_tag.trim();
    let (closing, rest) = trimmed
        .strip_prefix('/')
        .map_or((false, trimmed), |rest| (true, rest));
    let name = rest
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    (name, closing)
}

/// Advance past the matching `</name>` for an opening tag (positioned just after
/// its `>`). Consumes the remainder if no close is found.
fn skip_to_element_close<'a>(rest: &'a str, name: &str) -> &'a str {
    let needle = format!("</{name}");
    match find_ci(rest, &needle) {
        Some(pos) => match rest[pos..].find('>') {
            Some(gt) => &rest[pos + gt + 1..],
            None => "",
        },
        None => "",
    }
}

/// Map one structural tag to its markdown effect. Inline/unknown tags are no-ops
/// (their text flows through untouched).
fn apply_structural_tag(
    out: &mut String,
    name: &str,
    closing: bool,
    raw_tag: &str,
    pending_href: &mut Option<String>,
) {
    match name {
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            if closing {
                out.push('\n');
            } else {
                ensure_blank_line(out);
                let level = name[1..].parse::<usize>().unwrap_or(1).clamp(1, 6);
                for _ in 0..level {
                    out.push('#');
                }
                out.push(' ');
            }
        }
        "li" => {
            if !closing {
                ensure_newline(out);
                out.push_str("- ");
            }
        }
        "br" => out.push('\n'),
        "hr" => {
            ensure_newline(out);
            out.push_str("---\n");
        }
        "a" => {
            if closing {
                if let Some(href) = pending_href.take() {
                    out.push_str("](");
                    out.push_str(&href);
                    out.push(')');
                }
            } else if let Some(href) =
                attr_value(raw_tag, "href").filter(|href| is_renderable_href(href))
            {
                out.push('[');
                *pending_href = Some(decode_html_entities(href.trim()));
            } else {
                *pending_href = None;
            }
        }
        "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "nav" | "aside"
        | "ul" | "ol" | "table" | "blockquote" | "pre" | "figure" | "figcaption" | "dl"
        | "form" | "fieldset" => ensure_blank_line(out),
        "tr" | "thead" | "tbody" | "dt" | "dd" => ensure_newline(out),
        "td" | "th" => {
            if closing {
                out.push_str("  ");
            }
        }
        _ => {}
    }
}

/// Extract an attribute value (`name="…"` / `name='…'` / `name=token`) from a
/// tag body, case-insensitive on the attribute name.
fn attr_value(raw_tag: &str, attr: &str) -> Option<String> {
    let lowered = raw_tag.to_ascii_lowercase(); // ASCII-only case change keeps byte offsets aligned
    let mut from = 0;
    loop {
        let idx = lowered[from..].find(attr)? + from;
        let boundary_ok = idx == 0 || !lowered.as_bytes()[idx - 1].is_ascii_alphanumeric();
        let after = idx + attr.len();
        let remainder = raw_tag[after..].trim_start();
        if boundary_ok {
            if let Some(value) = remainder.strip_prefix('=') {
                return Some(extract_attr_value(value.trim_start()));
            }
        }
        from = after;
    }
}

fn extract_attr_value(value_part: &str) -> String {
    let mut chars = value_part.chars();
    match chars.next() {
        Some(quote @ ('"' | '\'')) => value_part[quote.len_utf8()..]
            .split(quote)
            .next()
            .unwrap_or("")
            .to_string(),
        _ => value_part
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string(),
    }
}

/// A link worth rendering: has a target that is not a bare fragment or a
/// `javascript:` handler.
fn is_renderable_href(href: &str) -> bool {
    let href = href.trim();
    !href.is_empty()
        && !href.starts_with('#')
        && !href.to_ascii_lowercase().starts_with("javascript:")
}

/// Append inter-tag text, decoding entities and collapsing all whitespace runs
/// (incl. source newlines, per HTML semantics) to a single space. Suppresses a
/// leading space right after a structural line break so lines do not start with
/// stray spaces.
fn append_inline_text(out: &mut String, raw: &str) {
    if raw.is_empty() {
        return;
    }
    let decoded = decode_html_entities(raw);
    let mut emitted_space = out.is_empty() || out.ends_with(|ch: char| ch.is_whitespace());
    for ch in decoded.chars() {
        if ch.is_whitespace() {
            if !emitted_space {
                out.push(' ');
                emitted_space = true;
            }
        } else {
            out.push(ch);
            emitted_space = false;
        }
    }
}

/// Ensure `out` ends with a newline (unless empty).
fn ensure_newline(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

/// Ensure `out` ends with a blank line (`\n\n`) unless empty, so a following
/// block starts its own paragraph. Runs of blank lines are collapsed later.
fn ensure_blank_line(out: &mut String) {
    if out.is_empty() {
        return;
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if !out.ends_with("\n\n") {
        out.push('\n');
    }
}

/// Trim trailing whitespace per line, collapse runs of blank lines to a single
/// blank line, and trim the ends.
fn finalize_markdown(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut blank_pending = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_pending = true;
            continue;
        }
        if !result.is_empty() && blank_pending {
            result.push('\n');
        }
        blank_pending = false;
        result.push_str(trimmed);
        result.push('\n');
    }
    result.trim().to_string()
}

/// Case-insensitive (ASCII) substring search returning a byte index. `needle`
/// must already be lowercase; the match boundary is at an ASCII byte so it is a
/// valid `char` boundary in `haystack`.
fn find_ci(haystack: &str, needle_lower: &str) -> Option<usize> {
    let hay = haystack.as_bytes();
    let needle = needle_lower.as_bytes();
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&start| {
        hay[start..start + needle.len()]
            .iter()
            .zip(needle)
            .all(|(byte, want)| byte.to_ascii_lowercase() == *want)
    })
}

fn decode_html_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        // A valid entity ends at the next ';' within a short window; anything
        // longer is a stray '&' and is emitted literally.
        if let Some(semi) = tail[1..].find(';').filter(|&idx| idx <= 12) {
            let entity = &tail[1..=semi];
            if let Some(decoded) = decode_one_entity(entity) {
                out.push_str(&decoded);
                rest = &tail[1 + semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// Decode a single entity body (the text between `&` and `;`): named entities we
/// care about for readability, plus numeric (`#123`) and hex (`#x1F600`) forms.
fn decode_one_entity(entity: &str) -> Option<String> {
    let named = match entity {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        "nbsp" => Some(' '),
        "hellip" => Some('…'),
        "mdash" => Some('—'),
        "ndash" => Some('–'),
        "rsquo" | "#8217" => Some('\u{2019}'),
        "lsquo" => Some('\u{2018}'),
        "ldquo" => Some('\u{201C}'),
        "rdquo" => Some('\u{201D}'),
        "copy" => Some('©'),
        "reg" => Some('®'),
        "trade" => Some('™'),
        "middot" => Some('·'),
        "bull" => Some('•'),
        _ => None,
    };
    if let Some(ch) = named {
        return Some(ch.to_string());
    }
    // Numeric character references: &#NNN; (decimal) or &#xHHH; (hex).
    let digits = entity.strip_prefix('#')?;
    let code = digits.strip_prefix(['x', 'X']).map_or_else(
        || digits.parse::<u32>().ok(),
        |hex| u32::from_str_radix(hex, 16).ok(),
    )?;
    char::from_u32(code).map(|ch| ch.to_string())
}

fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn preview_text(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let shortened = input.chars().take(max_chars).collect::<String>();
    format!("{}…", shortened.trim_end())
}

fn extract_search_hits(html: &str) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    let mut remaining = html;

    while let Some(anchor_start) = remaining.find("result__a") {
        let after_class = &remaining[anchor_start..];
        let Some(href_idx) = after_class.find("href=") else {
            remaining = &after_class[1..];
            continue;
        };
        let href_slice = &after_class[href_idx + 5..];
        let Some((url, rest)) = extract_quoted_value(href_slice) else {
            remaining = &after_class[1..];
            continue;
        };
        let Some(close_tag_idx) = rest.find('>') else {
            remaining = &after_class[1..];
            continue;
        };
        let after_tag = &rest[close_tag_idx + 1..];
        let Some(end_anchor_idx) = after_tag.find("</a>") else {
            remaining = &after_tag[1..];
            continue;
        };
        let title = html_inline_text(&after_tag[..end_anchor_idx]);
        if let Some(decoded_url) = decode_duckduckgo_redirect(&url) {
            hits.push(SearchHit {
                title: title.trim().to_string(),
                url: decoded_url,
            });
        }
        remaining = &after_tag[end_anchor_idx + 4..];
    }

    hits
}

fn extract_search_hits_from_generic_links(html: &str) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    let mut remaining = html;

    while let Some(anchor_start) = remaining.find("<a") {
        let after_anchor = &remaining[anchor_start..];
        let Some(href_idx) = after_anchor.find("href=") else {
            remaining = &after_anchor[2..];
            continue;
        };
        let href_slice = &after_anchor[href_idx + 5..];
        let Some((url, rest)) = extract_quoted_value(href_slice) else {
            remaining = &after_anchor[2..];
            continue;
        };
        let Some(close_tag_idx) = rest.find('>') else {
            remaining = &after_anchor[2..];
            continue;
        };
        let after_tag = &rest[close_tag_idx + 1..];
        let Some(end_anchor_idx) = after_tag.find("</a>") else {
            remaining = &after_anchor[2..];
            continue;
        };
        let title = html_inline_text(&after_tag[..end_anchor_idx]);
        if title.trim().is_empty() {
            remaining = &after_tag[end_anchor_idx + 4..];
            continue;
        }
        let decoded_url = decode_duckduckgo_redirect(&url).unwrap_or(url);
        if decoded_url.starts_with("http://") || decoded_url.starts_with("https://") {
            hits.push(SearchHit {
                title: title.trim().to_string(),
                url: decoded_url,
            });
        }
        remaining = &after_tag[end_anchor_idx + 4..];
    }

    hits
}

fn extract_quoted_value(input: &str) -> Option<(String, &str)> {
    let quote = input.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &input[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some((rest[..end].to_string(), &rest[end + quote.len_utf8()..]))
}

fn decode_duckduckgo_redirect(url: &str) -> Option<String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        return Some(html_entity_decode_url(url));
    }

    let joined = if url.starts_with("//") {
        format!("https:{url}")
    } else if url.starts_with("/l/") || url == "/l" {
        format!("https://duckduckgo.com{url}")
    } else {
        return None;
    };

    let parsed = reqwest::Url::parse(&joined).ok()?;
    if parsed.path() == "/l/" || parsed.path() == "/l" {
        for (key, value) in parsed.query_pairs() {
            if key == "uddg" {
                return Some(html_entity_decode_url(value.as_ref()));
            }
        }
    }
    Some(joined)
}

fn html_entity_decode_url(url: &str) -> String {
    decode_html_entities(url)
}

fn host_matches_list(url: &str, domains: &[String]) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    domains.iter().any(|domain| {
        let normalized = normalize_domain_filter(domain);
        !normalized.is_empty() && (host == normalized || host.ends_with(&format!(".{normalized}")))
    })
}

fn normalize_domain_filter(domain: &str) -> String {
    let trimmed = domain.trim();
    let candidate = reqwest::Url::parse(trimmed)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| trimmed.to_string());
    candidate
        .trim()
        .trim_start_matches('.')
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn dedupe_hits(hits: &mut Vec<SearchHit>) {
    let mut seen = BTreeSet::new();
    hits.retain(|hit| seen.insert(hit.url.clone()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn is_public_ip_classifies_internal_and_public() {
        // Internal / reserved — all blocked.
        for ip in [
            Ipv4Addr::new(169, 254, 169, 254), // cloud IMDS link-local
            Ipv4Addr::LOCALHOST,               // loopback
            Ipv4Addr::new(10, 1, 2, 3),        // RFC1918
            Ipv4Addr::new(192, 168, 0, 1),     // RFC1918
            Ipv4Addr::new(172, 16, 0, 1),      // RFC1918
            Ipv4Addr::new(100, 64, 0, 1),      // CGNAT
            Ipv4Addr::UNSPECIFIED,             // unspecified
        ] {
            assert!(!is_public_ip(IpAddr::V4(ip)), "{ip} must be non-public");
        }
        // Public — allowed.
        assert!(is_public_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(is_public_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        // IPv6 loopback / ULA / link-local blocked; public allowed.
        assert!(!is_public_ip("::1".parse().unwrap()));
        assert!(!is_public_ip("fc00::1".parse().unwrap()));
        assert!(!is_public_ip("fe80::1".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }

    #[test]
    fn ssrf_guard_blocks_internal_targets_and_bad_schemes() {
        let _guard = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::remove_var("ZO_WEB_ALLOW_LOCAL");
        for url in [
            "http://169.254.169.254/latest/meta-data/",
            "http://127.0.0.1:8080/",
            "https://10.0.0.5/",
            "https://192.168.1.1/",
            "http://[::1]/",
            "ftp://1.1.1.1/", // non-HTTP scheme
        ] {
            let parsed = reqwest::Url::parse(url).expect("valid test url");
            assert!(
                guard_against_ssrf(&parsed).is_err(),
                "expected SSRF/scheme block for {url}"
            );
        }
        // A public IP literal passes.
        let public = reqwest::Url::parse("https://1.1.1.1/").unwrap();
        assert!(guard_against_ssrf(&public).is_ok());
    }

    #[test]
    fn ssrf_guard_opt_out_allows_local() {
        let _guard = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::env::set_var("ZO_WEB_ALLOW_LOCAL", "1");
        let local = reqwest::Url::parse("http://127.0.0.1:3000/").unwrap();
        let allowed = guard_against_ssrf(&local).is_ok();
        std::env::remove_var("ZO_WEB_ALLOW_LOCAL");
        assert!(allowed, "ZO_WEB_ALLOW_LOCAL=1 must permit local targets");
    }

    #[test]
    fn preview_text_truncates_on_char_boundary() {
        assert_eq!(preview_text("abc한글def", 5), "abc한글…");
    }

    #[test]
    fn html_inline_text_decodes_basic_entities() {
        assert_eq!(
            html_inline_text("<h1>A&amp;B</h1><p>Hi&nbsp;there</p>"),
            "A&B Hi there"
        );
    }

    #[test]
    fn decode_html_entities_handles_named_numeric_and_hex() {
        assert_eq!(decode_html_entities("A&amp;B"), "A&B");
        assert_eq!(decode_html_entities("it&rsquo;s"), "it\u{2019}s");
        assert_eq!(decode_html_entities("a&#38;b"), "a&b"); // decimal
        assert_eq!(decode_html_entities("smile&#x1F600;!"), "smile\u{1F600}!"); // hex
        assert_eq!(decode_html_entities("m&mdash;n"), "m—n");
        // A stray ampersand with no valid entity is preserved literally.
        assert_eq!(decode_html_entities("Tom & Jerry"), "Tom & Jerry");
        assert_eq!(decode_html_entities("no entities here"), "no entities here");
    }

    #[test]
    fn html_to_markdown_preserves_structure_and_strips_noise() {
        let html = "<html><head><title>Doc</title>\
            <style>body{color:red}</style><script>var x = 1 < 2;</script></head>\
            <body><h1>Main Title</h1><p>Intro paragraph with a \
            <a href=\"https://example.com/page\">link</a>.</p>\
            <ul><li>First item</li><li>Second item</li></ul>\
            <h2>Section Two</h2><p>More text &amp; symbols.</p></body></html>";
        let md = html_to_markdown(html);

        assert!(md.contains("# Main Title"), "h1 → #: {md}");
        assert!(md.contains("## Section Two"), "h2 → ##: {md}");
        assert!(md.contains("- First item"), "li → - : {md}");
        assert!(md.contains("- Second item"));
        assert!(
            md.contains("[link](https://example.com/page)"),
            "anchor → markdown link: {md}"
        );
        assert!(md.contains("More text & symbols."), "entity decoded: {md}");
        // Non-content elements are stripped entirely.
        assert!(!md.contains("color:red"), "style stripped: {md}");
        assert!(!md.contains("var x"), "script stripped: {md}");
        assert!(!md.contains("Doc"), "head/title not in body: {md}");
        assert!(!md.contains('<'), "no raw tags leak through: {md}");
    }

    #[test]
    fn extract_title_reads_title_with_attributes() {
        assert_eq!(
            extract_title(
                "<html><head><title data-x=\"y\">Hello &amp; World</title></head></html>",
                "text/html; charset=utf-8"
            )
            .as_deref(),
            Some("Hello & World")
        );
        // Non-HTML content has no title.
        assert_eq!(extract_title("plain body", "text/plain"), None);
    }

    #[test]
    fn render_fetch_result_has_metadata_header_then_body() {
        // The header carries Title/URL/Status; the body follows a blank line. No
        // `prompt` branching exists any more — the "title"/"summary" magic
        // special-cases were removed, so every request returns header + body.
        let result = render_fetch_result(&FetchMeta {
            url: "https://example.com/doc",
            title: Some("Example Doc"),
            code: 200,
            code_text: "OK",
            content_type: "text/html; charset=utf-8",
            bytes: 12_800,
            body: "# Heading\n\nBody text here.",
        });

        assert!(result.starts_with("Title: Example Doc\n"));
        assert!(result.contains("URL: https://example.com/doc\n"));
        assert!(result.contains("Status: 200 OK · text/html · 12KB"));
        // A blank line separates the header from the body.
        assert!(result.contains("12KB\n\n# Heading"), "header/body separator: {result}");
        assert!(result.contains("Body text here."));
    }

    #[test]
    fn render_fetch_result_omits_title_line_for_untitled_content() {
        let result = render_fetch_result(&FetchMeta {
            url: "https://api.example.com/data.json",
            title: None,
            code: 200,
            code_text: "OK",
            content_type: "application/json",
            bytes: 42,
            body: "{\"ok\":true}",
        });
        assert!(result.starts_with("URL: https://api.example.com/data.json\n"));
        assert!(!result.contains("Title:"));
        assert!(result.contains("application/json"));
        assert!(result.contains("{\"ok\":true}"));
    }

    #[test]
    fn non_html_content_is_returned_verbatim() {
        assert_eq!(
            normalize_fetched_content("  {\"a\":1}  ", "application/json"),
            "{\"a\":1}"
        );
        assert_eq!(
            normalize_fetched_content("line1\nline2", "text/plain"),
            "line1\nline2"
        );
    }

    #[test]
    fn large_body_rides_truncation_and_artifact_seam() {
        // End-to-end contract WITHOUT a network: a fetched body larger than the
        // model-facing cap is head+tail digested by the shared truncation seam,
        // the full body stays recoverable via the artifact store, and the model
        // sees a recovery handle — exactly what dispatch wires up in production.
        use runtime::{truncate_tool_output, TruncationConfig};
        use std::fmt::Write as _;

        let mut body = String::from("HEAD_UNIQUE_MARKER page start\n\n");
        for n in 0..4_000 {
            let _ = write!(body, "Paragraph {n} with some readable prose content.\n\n");
        }
        body.push_str("TAIL_UNIQUE_MARKER page end\n");
        let full = render_fetch_result(&FetchMeta {
            url: "https://example.com/long",
            title: Some("Long Page"),
            code: 200,
            code_text: "OK",
            content_type: "text/html",
            bytes: body.len(),
            body: &body,
        });
        let cfg = TruncationConfig::default();
        assert!(full.chars().count() > cfg.default_max_chars);

        // Dispatch canonicalises the tool name to "WebFetch".
        let truncated = truncate_tool_output(&full, "WebFetch", &cfg);
        assert!(truncated.was_truncated);
        assert!(truncated.content.chars().count() <= cfg.default_max_chars);
        assert!(truncated.content.contains("HEAD_UNIQUE_MARKER"), "head kept");
        assert!(truncated.content.contains("TAIL_UNIQUE_MARKER"), "tail kept");
        assert!(truncated.content.contains("retrieve_tool_output"));

        // The full pre-truncation body is preserved recoverably by content hash.
        let dir = std::env::temp_dir().join(format!(
            "zo-webfetch-artifact-{}",
            std::process::id()
        ));
        let stored = crate::artifacts::store_transformed(Some(&dir), &full, false, true)
            .expect("oversized fetch body is stored");
        let recovered =
            std::fs::read_to_string(dir.join(&stored.sha256)).expect("artifact readable");
        assert_eq!(recovered, full, "the whole body is recoverable");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generic_link_extraction_keeps_http_urls() {
        let hits = extract_search_hits_from_generic_links(
            r#"<a href="https://example.com/page">Example</a><a href="/local">Skip</a>"#,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Example");
        assert_eq!(hits[0].url, "https://example.com/page");
    }

    #[test]
    fn host_filters_match_subdomains() {
        assert!(host_matches_list(
            "https://docs.example.com/page",
            &[String::from("example.com")]
        ));
        assert!(!host_matches_list(
            "https://example.org/page",
            &[String::from("example.com")]
        ));
    }
}
