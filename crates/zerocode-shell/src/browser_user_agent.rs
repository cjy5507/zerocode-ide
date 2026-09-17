//! What the desktop browser pane says it is.
//!
//! WKWebView's default user agent stops at `(KHTML, like Gecko)` — no
//! `Version/… Safari/…` tokens — and a site that gates on the browser's name
//! reads that as no browser at all: Atlassian Cloud answered the window's own
//! Confluence tab with 「지원되지 않는 브라우저」 while GitLab beside it was
//! fine (2026-09-07 16:50). The pane IS Safari's engine, so it wears Safari's
//! name: the version of the Safari installed on this machine, read from its
//! bundle, else the table's. A pane opened with a reader of its own (the
//! mobile emulator's iPhone) keeps that reader; between the two sits the
//! per-site table (`settings.browser.user_agents`, browser-door-for-agents
//! §2.5), whose row for the URL's exact host names the agent for the pane
//! being born — a user agent is builder-time, so a row means 「다음 판부터」.

/// The WebKit build every Safari has reported since Safari 11 — frozen by
/// Apple so sites stop sniffing it.
const WEBKIT_BUILD: &str = "605.1.15";

/// The platform token Safari reports on every macOS since Catalina, likewise
/// frozen.
const MAC_PLATFORM: &str = "Macintosh; Intel Mac OS X 10_15_7";

/// The Safari version the pane claims when the machine's cannot be read: the
/// Safari of the OS this window is built and tested on.
const FALLBACK_SAFARI_VERSION: &str = "26.3";

/// Where the installed Safari states its version, as XML text.
#[cfg(target_os = "macos")]
const SAFARI_VERSION_PLIST: &str = "/Applications/Safari.app/Contents/version.plist";

/// The plist key that carries the marketing version (`26.3.1`).
const VERSION_KEY: &str = "<key>CFBundleShortVersionString</key>";

/// Safari's own desktop user agent for `safari_version` (`26.3.1` → `Version/26.3`,
/// as Safari itself trims it); the table's version when none is known.
pub(crate) fn desktop_user_agent(safari_version: Option<&str>) -> String {
    let version = safari_version
        .map(major_minor)
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| FALLBACK_SAFARI_VERSION.to_string());
    format!(
        "Mozilla/5.0 ({MAC_PLATFORM}) AppleWebKit/{WEBKIT_BUILD} (KHTML, like Gecko) \
         Version/{version} Safari/{WEBKIT_BUILD}"
    )
}

/// The user agent a desktop pane opened without a reader of its own wears on
/// this machine; `None` where the platform's own webview already names a
/// browser (WebView2 speaks as Chrome) or the Mac platform token would be a lie.
pub(crate) fn desktop_user_agent_for_this_machine() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let version = std::fs::read_to_string(SAFARI_VERSION_PLIST)
            .ok()
            .and_then(|text| safari_version_in_plist(&text));
        Some(desktop_user_agent(version.as_deref()))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// The per-site table's agent for a pane born on `host` (§2.5): the row
/// whose host is exactly this one — ASCII case and a trailing dot aside,
/// since a hand-edited file may spell a host in capitals — or none. Exact
/// host in v1: no parent-domain rows, so a row for `example.com` says
/// nothing about `app.example.com`, and a row can never surprise a sibling
/// site. The first row for a host wins when a file holds two.
pub(crate) fn site_user_agent<'a>(
    table: &'a [crate::SiteUserAgent],
    host: Option<&str>,
) -> Option<&'a str> {
    let host = host?.trim_end_matches('.');
    table
        .iter()
        .find(|row| row.host.trim_end_matches('.').eq_ignore_ascii_case(host))
        .map(|row| row.agent.as_str())
}

/// The `CFBundleShortVersionString` of a version.plist, read as text: the
/// `<string>` that follows the key.
pub(crate) fn safari_version_in_plist(text: &str) -> Option<String> {
    let (_, after_key) = text.split_once(VERSION_KEY)?;
    let (_, after_open) = after_key.split_once("<string>")?;
    let (value, _) = after_open.split_once("</string>")?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// `major.minor` of a dotted version, digits only; empty when there is no
/// leading number.
fn major_minor(version: &str) -> String {
    version
        .trim()
        .split('.')
        .take(2)
        .take_while(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::{desktop_user_agent, safari_version_in_plist, site_user_agent};
    use crate::SiteUserAgent;

    /// The table answers for the URL's EXACT host — ASCII case and a trailing
    /// dot aside — and for nothing else: no parent domain, no sibling, no
    /// host at all. The first row for a host wins when a hand-edited file
    /// holds two.
    #[test]
    fn the_site_table_answers_for_the_exact_host_only() {
        let table = [
            SiteUserAgent {
                host: "confluence.example.com".to_string(),
                agent: "UA-confluence".to_string(),
            },
            SiteUserAgent {
                host: "Example.COM".to_string(),
                agent: "UA-example".to_string(),
            },
            SiteUserAgent {
                host: "example.com".to_string(),
                agent: "UA-shadowed".to_string(),
            },
        ];
        assert_eq!(
            site_user_agent(&table, Some("confluence.example.com")),
            Some("UA-confluence")
        );
        assert_eq!(
            site_user_agent(&table, Some("CONFLUENCE.example.com.")),
            Some("UA-confluence"),
            "case and a trailing dot do not make a different host"
        );
        assert_eq!(
            site_user_agent(&table, Some("example.com")),
            Some("UA-example"),
            "the first row for a host wins"
        );
        assert_eq!(
            site_user_agent(&table, Some("app.example.com")),
            None,
            "a parent domain's row says nothing about a subdomain (v1 is exact)"
        );
        assert_eq!(site_user_agent(&table, Some("wiki.example.com")), None);
        assert_eq!(site_user_agent(&table, Some("xample.com")), None);
        assert_eq!(
            site_user_agent(&table, None),
            None,
            "about:blank has no host"
        );
        assert_eq!(site_user_agent(&[], Some("example.com")), None);
    }

    /// The pane wears Safari's full name — WebKit build, `Version/major.minor`
    /// and the `Safari/` token a name-gating site looks for — from the
    /// installed Safari, and from the table when none is known.
    #[test]
    fn the_desktop_pane_wears_safaris_name_with_the_installed_version() {
        let agent = desktop_user_agent(Some("26.3.1"));
        assert_eq!(
            agent,
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
             (KHTML, like Gecko) Version/26.3 Safari/605.1.15"
        );
        assert!(desktop_user_agent(None).contains("Version/26.3 Safari/605.1.15"));
        assert!(desktop_user_agent(Some("garbage")).contains("Version/26.3 Safari/"));
        assert!(desktop_user_agent(Some("18")).contains("Version/18 Safari/"));
    }

    /// The version is the `<string>` after the marketing-version key, not any
    /// other string in the file.
    #[test]
    fn the_installed_version_is_read_from_the_bundles_version_plist() {
        let plist = "<plist><dict>\n\t<key>BuildVersion</key>\n\t<string>4</string>\n\
                     \t<key>CFBundleShortVersionString</key>\n\t<string>26.3.1</string>\n\
                     \t<key>CFBundleVersion</key>\n\t<string>21622.3.14.11.7</string>\n</dict></plist>";
        assert_eq!(safari_version_in_plist(plist).as_deref(), Some("26.3.1"));
        assert_eq!(safari_version_in_plist("<plist/>"), None);
    }
}
