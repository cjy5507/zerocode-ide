//! A browser profile's imported cookies are seated when its pane is born, and
//! the settings card says how that went (t-79190, review t-79415). The Rust
//! unit tests hold the counting; `ui/tests/settings.mjs` drives the card. These
//! contracts hold what neither sees from outside — that the window listens for
//! the names the backend emits, that every word the card and the notice speak
//! stands in every catalog, and that the injection never waits on an answer
//! that will not come.

use super::support::{WINDOW_PARTS, block_after, strip_comments, window_source};

const CHROMIUM: &str = include_str!("../../src/chromium_browser.rs");

fn part(name: &str) -> &'static str {
    WINDOW_PARTS
        .iter()
        .find(|(held, _)| *held == name)
        .map(|(_, text)| *text)
        .unwrap_or_else(|| panic!("{name} is not a window part"))
}

/// The card's words (`shell-settings.js`) and the incomplete-injection notice
/// (`shell-browser.js`) read their keys, and each key stands in the four
/// catalogs that carry it; Korean is the default each `t()` call holds.
#[test]
fn the_injection_words_stand_in_every_catalog_and_the_window_reads_them() {
    let code = strip_comments(window_source());
    for (file, key) in [
        ("shell-settings.js", "settings.browser.profileApplied"),
        (
            "shell-settings.js",
            "settings.browser.profileAppliedRefused",
        ),
        ("shell-settings.js", "settings.browser.profileRefusedHint"),
        ("shell-settings.js", "settings.browser.profileDropped"),
        ("shell-browser.js", "browser.cookieApplyIncomplete"),
    ] {
        for language in ["en", "ja", "zh", "es"] {
            let block = block_after(&code, &format!("  {language}: {{"));
            assert!(
                block.contains(&format!("\"{key}\":")),
                "`{key}` is missing from the `{language}` catalog"
            );
        }
        assert!(
            part(file).contains(&format!("\"{key}\"")),
            "`{key}` is in the catalogs but {file} does not read it"
        );
    }
}

/// The window repaints the cards when an injection ends and tells the person
/// when some cookies did not sit; both listen for the names the backend emits.
#[test]
fn the_window_listens_for_the_events_the_pane_emits() {
    let browser = part("shell-browser.js");
    for event in [
        "browser:cookie-import-applied",
        "browser:cookie-import-error",
    ] {
        assert!(
            CHROMIUM.contains(&format!("\"{event}\"")),
            "the pane no longer emits `{event}`"
        );
        assert!(
            browser.contains(&format!("listen(\"{event}\"")),
            "the window no longer listens for `{event}`"
        );
    }
    let applied = block_after(browser, "listen(\"browser:cookie-import-applied\"");
    assert!(
        applied.contains("refreshBrowserProfiles()") && applied.contains("paintBrowserProfiles"),
        "an injection ends and the cards keep saying 「적용 대기」:\n{applied}"
    );
    let incomplete = block_after(browser, "listen(\"browser:cookie-import-error\"");
    assert!(
        incomplete.contains("said.applied")
            && incomplete.contains("said.refused")
            && incomplete.contains("said.retained"),
        "the notice no longer builds its sentence from the three counts:\n{incomplete}"
    );
    let card = part("shell-settings.js");
    assert!(
        card.contains("applied.dropped"),
        "the card does not say how many cookies were skipped before trying"
    );
}

/// An injection ends however the store answers: a callback let go without an
/// answer counts as unsent (`CookieReply`), a pane closed under the injection
/// still settles and logs, and the staging is settled through the slot that
/// remembers which import it drank.
#[test]
fn the_injection_ends_when_the_store_does_not_answer() {
    assert!(
        CHROMIUM.contains("CookieSetCallback::new(reply.clone())")
            && !CHROMIUM.contains("CookieSetCallback::new(batch"),
        "the set-cookie callback no longer carries a reply that counts when it is let go"
    );
    let closed = block_after(CHROMIUM, "let Some(manager) = manager else {");
    assert!(
        closed.contains("finish_navigation()"),
        "a pane closed under the injection leaves no result and no log line:\n{closed}"
    );
    let settle = block_after(CHROMIUM, "fn finish_navigation(");
    assert!(
        settle.contains("&batch.slot,"),
        "the injection settles the staging without the slot that knows which import it drank:\n{settle}"
    );
}
