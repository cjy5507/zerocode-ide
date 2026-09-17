//! OpenCode Go's plan usage, scraped off the page its own site renders.
//!
//! Every other provider on this bar answers a JSON endpoint. This one has no
//! API: the figures are embedded in a React Server Components payload on
//! `/workspace/<id>/go`, in a wire format where an object may be written
//! `key:$R[28]={…}` instead of `key:{…}` — and where the SAME key appears more
//! than once, sometimes as `null` inside another component's props
//! (`opencode-go-page-scraper.ts:1-8`). So the reader walks braces rather than
//! matching a pattern, and takes the first block that actually carries both
//! numbers as its own direct properties.
//!
//! This is a scraper against markup nobody here controls. It is written to
//! fail by answering `None` rather than by answering a wrong number, and the
//! tests below are mostly about the ways it could be fooled.

use crate::usage::{ProviderUsage, UsageWindow};
use crate::usage_http::{self, Failure};
use zerocode_core::usage_limit::FailureKind;

/// The site, its server-function endpoint, and the hash that names the
/// workspaces function (`opencode-go-usage-fetcher.ts:11-17`).
const BASE_URL: &str = "https://opencode.ai";
const SERVER_URL: &str = "https://opencode.ai/_server";
const WORKSPACES_SERVER_ID: &str =
    "def39973159c7f0483d8793a822b8dbb10d067e12c65455fcb4608459ba0234f";

/// The only two cookie names that carry session auth on opencode.ai. Sending
/// anything else pollutes the header and can leak another site's data
/// (`:19-21`).
const AUTH_COOKIE_NAMES: [&str; 2] = ["auth", "__Host-auth"];

/// The seal an Iron Session token starts with — the shape that tells a bare
/// token from a whole Cookie header (`:36-40`).
const IRON_SESSION_PREFIX: &str = "Fe26.2**";

/// The three windows the page reports, in minutes.
const ROLLING_WINDOW_MINUTES: u32 = 300;
const WEEKLY_WINDOW_MINUTES: u32 = 10_080;
const MONTHLY_WINDOW_MINUTES: u32 = 43_200;

/// How far past a key's colon the opening brace may be. React Flight puts an
/// assignment token (`$R[28]=`) in that gap; a longer scan would run into the
/// NEXT occurrence of the key (`:28-32`).
const BRACE_SCAN_WINDOW: usize = 30;

/// What a person pasted, made into a Cookie header.
///
/// People paste the token alone as often as the whole header, and a token with
/// no name looks non-empty while carrying no auth at all — a silent failure
/// (`normalizeCookieInput`, `:23-41`). A value that is neither a seal nor a
/// plain token is left exactly as typed, so it fails visibly instead of being
/// wrapped into something malformed.
pub(crate) fn normalize_cookie(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let named = AUTH_COOKIE_NAMES.iter().any(|name| {
        trimmed.len() > name.len()
            && trimmed[..name.len()].eq_ignore_ascii_case(name)
            && trimmed.as_bytes()[name.len()] == b'='
    });
    if trimmed.contains(';') || named {
        return trimmed.to_string();
    }
    let plain = trimmed
        .chars()
        .all(|held| held.is_ascii_alphanumeric() || matches!(held, '.' | '-' | '_'));
    if trimmed.starts_with(IRON_SESSION_PREFIX) || plain {
        return format!("auth={trimmed}");
    }
    trimmed.to_string()
}

/// The auth pairs out of a Cookie header, and nothing else.
pub(crate) fn auth_cookies(header: &str) -> Vec<(String, String)> {
    header
        .split(';')
        .filter_map(|pair| {
            let pair = pair.trim();
            let (name, value) = pair.split_once('=')?;
            let (name, value) = (name.trim(), value.trim());
            (AUTH_COOKIE_NAMES.contains(&name) && !value.is_empty())
                .then(|| (name.to_string(), value.to_string()))
        })
        .collect()
}

/// Whether a workspace id is one (`^(wrk|wk)_[A-Za-z0-9]+$`, `:158-162`).
pub(crate) fn is_workspace_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("wrk_").or_else(|| id.strip_prefix("wk_")) else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|held| held.is_ascii_alphanumeric())
}

/// Workspace ids out of the server function's JS-serialised answer.
///
/// Anchored on `id:` rather than on the id shape alone, because an unrelated
/// property holding a similar string would otherwise be read as a workspace
/// (`parseWorkspaceIds`, `:60-74`). Order is preserved and duplicates dropped:
/// the caller tries them in turn and the first that answers wins.
pub(crate) fn workspace_ids(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(offset) = text[at..].find("id") {
        let start = at + offset;
        at = start + 2;
        // `\bid\b`: the two letters must not be part of a longer word.
        let before_ok = start == 0 || !is_word_byte(bytes[start - 1]);
        let after = start + 2;
        if !before_ok || bytes.get(after).is_some_and(|held| is_word_byte(*held)) {
            continue;
        }
        let rest = text[after..].trim_start();
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(quote) = rest
            .chars()
            .next()
            .filter(|held| *held == '"' || *held == '\'')
        else {
            continue;
        };
        let body = &rest[quote.len_utf8()..];
        let Some(end) = body.find(quote) else {
            continue;
        };
        let id = &body[..end];
        if is_workspace_id(id) && !found.iter().any(|held| held == id) {
            found.push(id.to_string());
        }
    }
    found
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// A number written as a direct property of `text`'s outermost object.
///
/// Depth matters: without it the first match wins wherever it sits, and a
/// nested sub-object carrying the same field name hands back the wrong figure
/// (`extractTopLevelNumber`, `:79-113`).
fn top_level_number(text: &str, field: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ if depth == 1 => {
                if let Some(number) = number_at(text, at, field) {
                    return Some(number);
                }
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// `field: <number>` starting exactly at `at`, with `field` a whole word.
fn number_at(text: &str, at: usize, field: &str) -> Option<f64> {
    let bytes = text.as_bytes();
    if at > 0 && is_word_byte(bytes[at - 1]) {
        return None;
    }
    let rest = text.get(at..)?;
    let rest = rest.strip_prefix(field)?;
    if rest
        .as_bytes()
        .first()
        .is_some_and(|held| is_word_byte(*held))
    {
        return None;
    }
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    let digits: String = rest
        .chars()
        .take_while(|held| held.is_ascii_digit() || matches!(held, '-' | '.'))
        .collect();
    digits.parse::<f64>().ok().filter(|held| held.is_finite())
}

/// The brace-balanced block assigned to `key`, skipping the assignment token
/// React Flight puts in the way, and skipping any block that does not carry
/// both figures as its own (`extractUsageBlock`, `:16-70`).
pub(crate) fn usage_block<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(offset) = text[at..].find(key) {
        let start = at + offset;
        at = start + key.len();
        if start > 0 && is_word_byte(bytes[start - 1]) {
            continue;
        }
        let after = start + key.len();
        if bytes.get(after).is_some_and(|held| is_word_byte(*held)) {
            continue;
        }
        let Some(rest) = text[after..].trim_start().strip_prefix(':') else {
            continue;
        };
        // The brace must be near — a far one belongs to something else, and
        // `key:null` has none at all.
        let window = &rest[..rest.len().min(BRACE_SCAN_WINDOW)];
        let Some(brace) = window.find('{') else {
            continue;
        };
        let open = text.len() - rest.len() + brace;
        let Some(block) = balanced_block(text, open) else {
            continue;
        };
        if top_level_number(block, "usagePercent").is_some()
            && top_level_number(block, "resetInSec").is_some()
        {
            return Some(block);
        }
    }
    None
}

/// From the `{` at `open` to the `}` that closes it.
///
/// The depth walk does not skip string literals, exactly as the original's
/// does not — and the original says why in a comment worth keeping: the wire
/// format does not emit raw braces inside strings today, and this is a scraper
/// against markup nobody here controls, so it is fragile by nature (`:41-44`).
fn balanced_block(text: &str, open: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    for (offset, byte) in bytes[open..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return text.get(open..=open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// What one usage page said.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PageUsage {
    pub(crate) rolling: (f64, f64),
    pub(crate) weekly: (f64, f64),
    /// Absent on plans with no monthly budget.
    pub(crate) monthly: Option<(f64, f64)>,
}

/// The three windows out of a rendered page, or `None` when the two required
/// ones are not both there (`parseSubscriptionFromPageText`, `:120-172`).
pub(crate) fn read_page(text: &str) -> Option<PageUsage> {
    if text.is_empty() {
        return None;
    }
    let pair = |key: &str| -> Option<(f64, f64)> {
        let block = usage_block(text, key)?;
        Some((
            top_level_number(block, "usagePercent")?.clamp(0.0, 100.0),
            top_level_number(block, "resetInSec")?,
        ))
    };
    Some(PageUsage {
        rolling: pair("rollingUsage")?,
        weekly: pair("weeklyUsage")?,
        monthly: pair("monthlyUsage"),
    })
}

fn window_of(figures: (f64, f64), window_minutes: u32, now_ms: i64) -> UsageWindow {
    let (percent, reset_in_sec) = figures;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=100 before the cast"
    )]
    UsageWindow {
        used_percent: percent.round().clamp(0.0, 100.0) as u8,
        window_minutes,
        // The page says how long is LEFT, not when it ends — so the moment is
        // stamped here (`makeWindow`, `:76-88`).
        resets_at: Some(now_ms + (reset_in_sec * 1000.0) as i64),
        reset_description: None,
    }
}

/// The workspaces server function's URL and headers — the SST server-fn
/// protocol wants the hash in both the query and a header (`:174-189`).
fn workspaces_request(instance: &str) -> (String, Vec<(&'static str, String)>) {
    (
        format!("{SERVER_URL}?id={WORKSPACES_SERVER_ID}"),
        vec![
            ("X-Server-Id", WORKSPACES_SERVER_ID.to_string()),
            ("X-Server-Instance", format!("server-fn:{instance}")),
            (
                "Accept",
                "text/javascript, application/json;q=0.9, */*;q=0.8".to_string(),
            ),
            ("Origin", BASE_URL.to_string()),
            ("Referer", BASE_URL.to_string()),
        ],
    )
}

fn page_url(workspace: &str) -> String {
    format!("{BASE_URL}/workspace/{workspace}/go")
}

/// One reading of OpenCode Go's plan usage.
///
/// `cookie` is what the person pasted; `workspace` is their override, when
/// they set one. Nothing here writes anything.
pub fn scan(cookie: &str, workspace: Option<&str>, instance: &str, now_ms: i64) -> ProviderUsage {
    let answer = |status: &str, error: Option<String>, kind: Option<FailureKind>| ProviderUsage {
        provider: "opencode-go".to_string(),
        session: None,
        weekly: None,
        fable_weekly: None,
        monthly: None,
        buckets: None,
        updated_at: now_ms,
        error,
        status: status.to_string(),
        failure_kind: kind,
        retry_at_ms: None,
        plan_type: None,
        reset_credits: None,
        account: None,
    };
    let header = normalize_cookie(cookie);
    if header.is_empty() {
        // Not configured is a state, not a failure — the segment stays hidden
        // rather than wearing a permanent alert (`:96-106`).
        return answer(
            "unavailable",
            Some("세션 쿠키가 설정되지 않았습니다".to_string()),
            None,
        );
    }
    let pairs = auth_cookies(&header);
    if pairs.is_empty() {
        return answer(
            "error",
            Some(
                "auth 쿠키를 찾지 못했습니다 — opencode.ai 개발자 도구의 Cookie 헤더를 그대로 붙여넣으세요"
                    .to_string(),
            ),
            Some(FailureKind::MissingCredentials),
        );
    }
    // Only the auth pairs are sent. The header the person pasted may carry
    // another site's session, and none of it is this request's business.
    let sent = pairs
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");

    let candidates = match workspace.map(str::trim).filter(|held| !held.is_empty()) {
        Some(held) if !is_workspace_id(held) => {
            return answer(
                "error",
                Some("워크스페이스 id 형식이 아닙니다 — wrk_… 또는 wk_… 여야 합니다".to_string()),
                None,
            );
        }
        Some(held) => vec![held.to_string()],
        None => {
            let (url, headers) = workspaces_request(instance);
            let mut listed: Vec<(&str, String)> = headers
                .iter()
                .map(|(name, value)| (*name, value.clone()))
                .collect();
            listed.push(("Cookie", sent.clone()));
            let borrowed: Vec<(&str, &str)> = listed
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect();
            match usage_http::get_text(&url, &borrowed, now_ms) {
                Ok(text) => workspace_ids(&text),
                Err(Failure {
                    recovery,
                    retry_at_ms,
                    message,
                    ..
                }) => {
                    return ProviderUsage {
                        retry_at_ms,
                        ..answer("error", Some(message), Some(recovery.kind))
                    };
                }
            }
        }
    };
    if candidates.is_empty() {
        return answer(
            "error",
            Some(
                "워크스페이스를 찾지 못했습니다 — 설정에서 워크스페이스 id를 지정하세요"
                    .to_string(),
            ),
            None,
        );
    }
    // Each candidate gets its own round trip: a workspace that answers but
    // carries no usage must not stop the next one from being tried (`:232-236`).
    let mut last: Option<String> = None;
    for candidate in &candidates {
        let headers: Vec<(&str, &str)> = vec![
            (
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            ),
            ("Origin", BASE_URL),
            ("Referer", BASE_URL),
            ("Cookie", &sent),
        ];
        match usage_http::get_text(&page_url(candidate), &headers, now_ms) {
            Ok(text) => match read_page(&text) {
                Some(read) => {
                    return ProviderUsage {
                        session: Some(window_of(read.rolling, ROLLING_WINDOW_MINUTES, now_ms)),
                        weekly: Some(window_of(read.weekly, WEEKLY_WINDOW_MINUTES, now_ms)),
                        monthly: read
                            .monthly
                            .map(|held| window_of(held, MONTHLY_WINDOW_MINUTES, now_ms)),
                        ..answer("ok", None, None)
                    };
                }
                None => last = Some("사용량을 읽지 못했습니다".to_string()),
            },
            Err(failure) => last = Some(failure.message),
        }
    }
    answer(
        "error",
        Some(last.unwrap_or_else(|| "사용량을 읽지 못했습니다".to_string())),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pasted token becomes a header; a pasted header is left alone; and
    /// something that is neither is left to fail visibly.
    #[test]
    fn what_a_person_pastes_becomes_a_cookie_header_or_stays_as_typed() {
        assert_eq!(normalize_cookie("  "), "");
        // A bare Iron Session seal gets its name.
        assert_eq!(
            normalize_cookie("Fe26.2**abc123"),
            "auth=Fe26.2**abc123",
            "a bare seal was left nameless, which fails silently"
        );
        // A plain token too.
        assert_eq!(normalize_cookie("abc.123-x_y"), "auth=abc.123-x_y");
        // An already-named value is untouched, in either spelling.
        assert_eq!(normalize_cookie("auth=xyz"), "auth=xyz");
        assert_eq!(normalize_cookie("__Host-auth=xyz"), "__Host-auth=xyz");
        // A multi-pair header is a header.
        assert_eq!(normalize_cookie("a=1; auth=xyz"), "a=1; auth=xyz");
        // Something with spaces in it is neither — left as typed so it fails
        // where somebody can see it, rather than being wrapped into nonsense.
        assert_eq!(normalize_cookie("not a token"), "not a token");
    }

    /// Only the two auth names are sent on.
    #[test]
    fn every_cookie_but_the_two_that_authenticate_is_left_behind() {
        let sent = auth_cookies("session=other-site; auth=mine; tracking=x; __Host-auth=strict");
        assert_eq!(
            sent,
            vec![
                ("auth".to_string(), "mine".to_string()),
                ("__Host-auth".to_string(), "strict".to_string()),
            ],
            "a cookie belonging to another site was about to be sent"
        );
        // An empty value is not a credential.
        assert!(auth_cookies("auth=").is_empty());
        assert!(auth_cookies("nothing-here").is_empty());
    }

    /// Ids are anchored on the property name, not on their own shape.
    #[test]
    fn a_workspace_id_is_read_from_an_id_property_and_nowhere_else() {
        let text = r#"{parentId:"wrk_notmine",id:"wrk_abc123",other:{id:'wk_9'},id:"wrk_abc123"}"#;
        assert_eq!(
            workspace_ids(text),
            vec!["wrk_abc123".to_string(), "wk_9".to_string()],
            "the id anchor slipped, or a duplicate was kept"
        );
        // `parentId:` must not match `id:` — the word boundary is the guard.
        assert!(!workspace_ids(r#"{parentId:"wrk_x"}"#).contains(&"wrk_x".to_string()));
        // And the shape is checked.
        assert!(is_workspace_id("wrk_a1") && is_workspace_id("wk_Z9"));
        assert!(
            !is_workspace_id("wrk_") && !is_workspace_id("ws_a1") && !is_workspace_id("wrk_a-1")
        );
    }

    /// The three ways this page tries to fool a reader.
    #[test]
    fn the_scraper_refuses_the_null_the_nested_and_the_far_brace() {
        // 1. The same key appears first as `null`, then as the real object —
        //    and the real one is written with React Flight's assignment token.
        let flight = r#"props:{rollingUsage:null,x:1},data:rollingUsage:$R[28]={usagePercent:42,resetInSec:60}"#;
        let block = usage_block(flight, "rollingUsage").expect("the real block was skipped");
        assert_eq!(top_level_number(block, "usagePercent"), Some(42.0));

        // 2. A block whose figure is NESTED must not be mined for it: the
        //    depth-1 rule is what stops a sub-object's number being read as
        //    the object's own.
        let nested = "{meta:{usagePercent:99,resetInSec:1},usagePercent:7,resetInSec:2}";
        assert_eq!(top_level_number(nested, "usagePercent"), Some(7.0));

        // 3. A brace far past the colon belongs to something else.
        let far = format!(
            "rollingUsage:{}{{usagePercent:1,resetInSec:2}}",
            " ".repeat(40)
        );
        assert!(
            usage_block(&far, "rollingUsage").is_none(),
            "a distant brace was taken as this key's object"
        );

        // A block with only one of the two figures is not a usage block.
        assert!(usage_block("weeklyUsage:{usagePercent:5}", "weeklyUsage").is_none());
    }

    /// A whole page: two required windows, one optional, and the refusals.
    #[test]
    fn a_page_yields_three_windows_or_none_at_all() {
        let page = r#"
          <script>self.__flight.push("
            billing:{monthlyUsage:null},
            rollingUsage:$R[1]={usagePercent:12.6,resetInSec:1800},
            weeklyUsage:{usagePercent:48,resetInSec:86400},
            monthlyUsage:{usagePercent:73,resetInSec:604800}
          ")</script>"#;
        let read = read_page(page).expect("the page did not parse");
        assert_eq!(read.rolling, (12.6, 1800.0));
        assert_eq!(read.weekly, (48.0, 86400.0));
        assert_eq!(read.monthly, Some((73.0, 604800.0)));

        // The monthly one is optional — plans without it still read.
        let no_month =
            "rollingUsage:{usagePercent:1,resetInSec:2},weeklyUsage:{usagePercent:3,resetInSec:4}";
        let read = read_page(no_month).expect("a plan with no month did not parse");
        assert_eq!(read.monthly, None);

        // Either required window missing is no reading at all — a partial
        // answer here would be a bar drawn from half a page.
        assert!(read_page("rollingUsage:{usagePercent:1,resetInSec:2}").is_none());
        assert!(read_page("").is_none());
    }

    /// The window carries a MOMENT, not a countdown, and the percent is
    /// clamped before anybody sees it.
    #[test]
    fn a_countdown_becomes_a_moment_on_this_clock() {
        let now = 1_800_000_000_000;
        let window = window_of((250.0, 3600.0), ROLLING_WINDOW_MINUTES, now);
        assert_eq!(window.used_percent, 100, "a percentage over 100 was shown");
        assert_eq!(window.window_minutes, 300);
        assert_eq!(window.resets_at, Some(now + 3_600_000));
        // And a page that reports a negative percentage does not go below zero.
        assert_eq!(
            window_of((-5.0, 0.0), WEEKLY_WINDOW_MINUTES, now).used_percent,
            0
        );
    }

    /// A second reader, written to the same rules by a different route, is
    /// asked the same 400 pages.
    ///
    /// The generated corpus varies the ways the real payload varies: the
    /// React Flight assignment token, a `null` occurrence of the same key
    /// before the real one, a nested sub-object carrying the same field name,
    /// a missing monthly block, percentages outside 0..100. The oracle below
    /// finds its blocks by regex-and-window rather than by the walk the
    /// shipped reader uses, so agreement is two roads arriving together
    /// rather than one road checked against itself.
    #[test]
    fn a_second_reader_of_the_same_rules_agrees_over_four_hundred_pages() {
        // Deterministic, so a failure is reproducible: a tiny LCG rather than
        // a dependency, seeded once.
        let mut seed: u64 = 7;
        let mut next = move |bound: u64| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) % bound
        };

        // The oracle: same contract, different technique — take the text
        // after `key:`, allow a short window before the brace, count braces to
        // the close, and require both fields at depth one.
        fn oracle_number(block: &str, field: &str) -> Option<f64> {
            let mut depth = 0i32;
            for (at, ch) in block.char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ if depth == 1 => {
                        let rest = &block[at..];
                        if !rest.starts_with(field) {
                            continue;
                        }
                        if at > 0 && block.as_bytes()[at - 1].is_ascii_alphanumeric() {
                            continue;
                        }
                        let after = &rest[field.len()..];
                        if after.starts_with(|held: char| held.is_ascii_alphanumeric()) {
                            continue;
                        }
                        let after = after.trim_start().strip_prefix(':')?.trim_start();
                        let digits: String = after
                            .chars()
                            .take_while(|held| {
                                held.is_ascii_digit() || *held == '-' || *held == '.'
                            })
                            .collect();
                        return digits.parse().ok();
                    }
                    _ => {}
                }
            }
            None
        }
        fn oracle_pair(text: &str, key: &str) -> Option<(f64, f64)> {
            let needle = format!("{key}:");
            let mut from = 0;
            while let Some(offset) = text[from..].find(&needle) {
                let start = from + offset;
                from = start + needle.len();
                if start > 0 && text.as_bytes()[start - 1].is_ascii_alphanumeric() {
                    continue;
                }
                let window = &text[from..text.len().min(from + 30)];
                let Some(brace) = window.find('{') else {
                    continue;
                };
                let open = from + brace;
                let mut depth = 0i32;
                let mut close = None;
                for (at, ch) in text[open..].char_indices() {
                    match ch {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                close = Some(open + at);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let Some(close) = close else { continue };
                let block = &text[open..=close];
                if let (Some(percent), Some(seconds)) = (
                    oracle_number(block, "usagePercent"),
                    oracle_number(block, "resetInSec"),
                ) {
                    return Some((percent.clamp(0.0, 100.0), seconds));
                }
            }
            None
        }

        let mut read = 0;
        for _ in 0..400 {
            let mut parts: Vec<String> = Vec::new();
            if next(2) == 0 {
                parts.push("billing:{rollingUsage:null,weeklyUsage:null}".to_string());
            }
            if next(10) < 3 {
                parts.push(format!(
                    "meta:{{usagePercent:{},resetInSec:{}}}",
                    next(100),
                    next(9999) + 1
                ));
            }
            // Not a closure: it would borrow `next` a second time while the
            // arguments are still being drawn from it.
            fn block(key: &str, percent: i64, seconds: u64, assign: &str, inner: &str) -> String {
                format!("{key}:{assign}{{usagePercent:{percent},resetInSec:{seconds}{inner}}}")
            }
            let token = |held: u64| format!("$R[{held}]=");
            let decoy = |held: u64| format!(",inner:{{usagePercent:{held}}}");
            #[expect(clippy::cast_possible_wrap, reason = "small bounded values")]
            let rolling_percent = next(140) as i64 - 20;
            let rolling_seconds = next(99_999) + 1;
            let assign = if next(2) == 0 {
                token(next(99) + 1)
            } else {
                String::new()
            };
            let inner = if next(10) < 4 {
                decoy(next(100))
            } else {
                String::new()
            };
            parts.push(block(
                "rollingUsage",
                rolling_percent,
                rolling_seconds,
                &assign,
                &inner,
            ));
            #[expect(clippy::cast_possible_wrap, reason = "small bounded values")]
            let weekly_percent = next(101) as i64;
            let weekly_seconds = next(99_999) + 1;
            let assign = if next(2) == 0 {
                token(next(99) + 1)
            } else {
                String::new()
            };
            let inner = if next(10) < 4 {
                decoy(next(100))
            } else {
                String::new()
            };
            parts.push(block(
                "weeklyUsage",
                weekly_percent,
                weekly_seconds,
                &assign,
                &inner,
            ));
            if next(10) < 6 {
                #[expect(clippy::cast_possible_wrap, reason = "small bounded values")]
                let monthly_percent = next(101) as i64;
                let monthly_seconds = next(99_999) + 1;
                let assign = if next(2) == 0 {
                    token(next(99) + 1)
                } else {
                    String::new()
                };
                parts.push(block(
                    "monthlyUsage",
                    monthly_percent,
                    monthly_seconds,
                    &assign,
                    "",
                ));
            }
            if next(10) < 2 {
                parts.push("rollingUsage:null".to_string());
            }
            // Order varies: the real payload does not put these in one order.
            let cut = usize::try_from(next(u64::try_from(parts.len()).unwrap_or(1))).unwrap_or(0);
            parts.rotate_left(cut);
            let page = format!("<script>{}</script>", parts.join(","));

            let want = (
                oracle_pair(&page, "rollingUsage"),
                oracle_pair(&page, "weeklyUsage"),
            );
            let got = read_page(&page);
            match (want.0, want.1) {
                (Some(rolling), Some(weekly)) => {
                    let got =
                        got.unwrap_or_else(|| panic!("refused a page the oracle read:\n{page}"));
                    assert_eq!(got.rolling, rolling, "rolling disagreed:\n{page}");
                    assert_eq!(got.weekly, weekly, "weekly disagreed:\n{page}");
                    assert_eq!(
                        got.monthly,
                        oracle_pair(&page, "monthlyUsage"),
                        "monthly disagreed:\n{page}"
                    );
                    read += 1;
                }
                _ => assert!(got.is_none(), "read a page the oracle refused:\n{page}"),
            }
        }
        assert!(
            read > 300,
            "the corpus stopped exercising the reader: {read}"
        );
    }

    /// The server-function protocol wants its hash in two places at once.
    #[test]
    fn the_workspaces_call_names_its_function_in_the_query_and_the_header() {
        let (url, headers) = workspaces_request("abc-123");
        assert!(url.contains(WORKSPACES_SERVER_ID) && url.starts_with(SERVER_URL));
        let named = |name: &str| {
            headers
                .iter()
                .find(|(held, _)| *held == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(named("X-Server-Id").as_deref(), Some(WORKSPACES_SERVER_ID));
        assert_eq!(
            named("X-Server-Instance").as_deref(),
            Some("server-fn:abc-123"),
            "the instance token lost its prefix"
        );
        assert_eq!(named("Origin").as_deref(), Some(BASE_URL));
        assert_eq!(named("Referer").as_deref(), Some(BASE_URL));
        assert_eq!(page_url("wrk_x"), "https://opencode.ai/workspace/wrk_x/go");
    }
}
