//! An agent's real mark, fetched once and kept.
//!
//! Orca draws these from its own bundle: inline SVGs for the nine it
//! hand-drew and bundled PNGs for the rest (`AgentIcon`,
//! agent-catalog-1Y3pTpm8.js:452-490). We cannot ship those files — they are a
//! third party's assets — and re-drawing a vendor's mark as path data in our
//! source would put the trademark in the product just the same. So the mark
//! comes from the vendor at RUNTIME, which is the same thing a browser does
//! with a favicon and the only version guaranteed to be the vendor's current
//! one.
//!
//! The window used to fetch it directly and that is why every agent wore a
//! letter tile: `https://www.google.com/s2/favicons?domain=…` answers **301 to
//! `t0.gstatic.com`**, and a CSP source with a path does not carry across a
//! redirect — the entry point was allowed and the destination was not. The
//! browser test suite has no CSP at all, which is exactly why the screenshots
//! showed real marks while the app showed letters. A gate now pins the shape
//! that cannot have this bug: the webview asks the backend, and the CSP admits
//! no remote image host at all.
//!
//! Fetching here buys three things beyond the fix: the marks survive going
//! offline, the CSP gets *tighter* rather than wider, and one cache serves
//! every surface that draws an agent.

use std::path::{Path, PathBuf};

use base64::Engine as _;

/// How long the fetch may take. A mark is decoration on a list that has
/// already painted its letters, so it must never be the reason a panel waits.
const FETCH_TIMEOUT_SECS: &str = "5";

/// The size asked for. 64 covers the 28px chip at 2x, which is the largest
/// this window draws an agent at.
const ICON_PIXELS: u32 = 64;
pub(crate) const CACHE_DIR_NAME: &str = "agent-icons";
const ICON_FILE_EXTENSION: &str = "png";

/// A domain we will put in a URL and in a filename.
///
/// Both uses are why this is strict rather than merely trimmed: the value
/// arrives from the agent catalogue today, and a catalogue is the kind of thing
/// that later reads from a config file. `..` in a filename walks out of the
/// cache directory, and a space or a quote in a URL is an argument boundary.
/// Letters, digits, dot and dash — every real domain, and nothing else.
fn tidy_domain(domain: &str) -> Option<String> {
    let lowered = domain.trim().to_ascii_lowercase();
    if lowered.is_empty() || lowered.len() > 253 {
        return None;
    }
    let shaped = lowered
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    // A leading dot or dash is not a domain, and `..` cannot appear even
    // though the character class above would admit each dot on its own.
    let sane = shaped
        && !lowered.starts_with('.')
        && !lowered.starts_with('-')
        && !lowered.contains("..")
        && lowered.contains('.');
    sane.then_some(lowered)
}

fn cache_dir(cache_root: &Path) -> PathBuf {
    cache_root.join(CACHE_DIR_NAME)
}

fn cache_file(cache_root: &Path, domain: &str) -> PathBuf {
    cache_dir(cache_root).join(format!("{domain}.{ICON_FILE_EXTENSION}"))
}

/// The bytes as a `data:` URL, which is what the CSP already admits for the
/// image viewer.
fn as_data_uri(bytes: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:image/png;base64,{encoded}")
}

/// Is this actually a PNG? The service answers 200 with an HTML error page
/// when it is unhappy, and a cache full of error pages is a cache that never
/// heals — every later read would find a file and hand the webview a broken
/// image instead of the letter tile that at least says which agent it is.
fn looks_like_png(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a])
}

/// Ask the vendor's favicon for a mark, following redirects.
///
/// `curl` rather than an HTTP crate, deliberately: this process already shells
/// out to `lsof` and to `gh` for the same reason — the platform's own tool is
/// present on all three targets, and one decorative image is not worth linking
/// a second TLS stack into a window that otherwise talks to nothing remote.
/// `-f` so a 404 is a failure rather than an error page written to the cache.
fn fetch(domain: &str) -> Option<Vec<u8>> {
    let url = format!("https://www.google.com/s2/favicons?domain={domain}&sz={ICON_PIXELS}");
    let output = crate::proc::quiet_command("curl")
        .args([
            "-sfL",
            "--max-time",
            FETCH_TIMEOUT_SECS,
            "-o",
            "-",
            url.as_str(),
        ])
        .output()
        .ok()?;
    if !output.status.success() || !looks_like_png(&output.stdout) {
        return None;
    }
    Some(output.stdout)
}

/// One agent's mark as a `data:` URL, from the cache or from the vendor.
///
/// `None` means the window keeps its letter tile — which is a real answer, not
/// a failure to report: a machine that is offline and has never seen this
/// agent has no mark to show, and a letter naming the agent beats a broken
/// image icon.
pub fn agent_icon(cache_root: &Path, domain: &str) -> Option<String> {
    let domain = tidy_domain(domain)?;
    let file = cache_file(cache_root, &domain);
    if let Ok(held) = std::fs::read(&file)
        && looks_like_png(&held)
    {
        return Some(as_data_uri(&held));
    }
    let fetched = fetch(&domain)?;
    // Written after the answer is known good, and a write that fails does not
    // fail the call: the mark is already in hand, and the next window will try
    // the network again.
    if let Some(parent) = file.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&file, &fetched);
    Some(as_data_uri(&fetched))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_domain_that_could_leave_the_cache_directory_is_refused() {
        // Both uses are covered by one test because both are why the check
        // exists: a filename that walks up, and a URL argument that ends.
        for hostile in [
            "../../etc/passwd",
            "claude.ai/../../secret",
            "claude.ai&x=1",
            "claude ai.com",
            "claude.ai\"",
            "..",
            ".claude.ai",
            "-claude.ai",
            "claude..ai",
            "localhost",
            "",
            "   ",
        ] {
            assert_eq!(
                tidy_domain(hostile),
                None,
                "`{hostile}` was accepted as a domain"
            );
        }
    }

    #[test]
    fn a_real_domain_survives_and_is_lowercased() {
        assert_eq!(tidy_domain("Claude.AI"), Some("claude.ai".into()));
        assert_eq!(
            tidy_domain("my-agent.example.co.uk"),
            Some("my-agent.example.co.uk".into())
        );
    }

    #[test]
    fn only_a_png_is_trusted_or_cached() {
        // The service answers 200 with HTML when it is unhappy; caching that
        // would hand the window a broken image forever.
        assert!(looks_like_png(&[
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00
        ]));
        assert!(!looks_like_png(b"<html><body>error"));
        assert!(!looks_like_png(b""));
        assert!(!looks_like_png(&[0x89, b'P', b'N']));
    }

    #[test]
    fn the_uri_is_the_one_the_policy_already_admits() {
        let uri = as_data_uri(&[0x89, b'P', b'N', b'G']);
        assert!(
            uri.starts_with("data:image/png;base64,"),
            "the mark stopped travelling as the data URL the CSP allows: {uri}"
        );
        assert_eq!(uri, "data:image/png;base64,iVBORw==");
    }

    #[test]
    fn a_cached_mark_is_read_from_the_injected_root_rather_than_fetched_again() {
        let cache = tempfile::tempdir().expect("no cache dir");
        // The cache is keyed by the tidied domain, so two spellings of one
        // host cannot end up as two files.
        let one = cache_file(cache.path(), &tidy_domain("Claude.AI").unwrap());
        let two = cache_file(cache.path(), &tidy_domain("claude.ai").unwrap());
        assert_eq!(one, two);
        assert_eq!(
            one,
            cache
                .path()
                .join(CACHE_DIR_NAME)
                .join(format!("claude.ai.{ICON_FILE_EXTENSION}")),
            "the cache escaped its injected root or stopped naming files by domain"
        );
        std::fs::create_dir_all(one.parent().expect("cache directory")).expect("make cache");
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00];
        std::fs::write(&one, png).expect("write cached icon");
        assert_eq!(
            agent_icon(cache.path(), "Claude.AI"),
            Some(as_data_uri(&png)),
            "a cached icon under the injected root was not used"
        );
    }
}
