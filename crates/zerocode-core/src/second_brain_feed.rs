//! The raw inbox feed (t-43992, design §4's second unit): an external page an
//! agent read in this window, dropped into the vault's `raw/` as one new file —
//! the compiler's input. The 「raw 취합」 turn eats it, and the health card
//! counts it until the log names it (`second_brain_lint`'s unlogged raw row).
//!
//! The window decides when: the person's opt-in switch, a configured vault, a
//! whole-page read. This file decides what lands: an external `http(s)` page
//! only, one file per source address, and never a file already in `raw/`
//! opened for writing.

use std::io;
use std::path::Path;

/// Bytes of a page's text one capture keeps. A longer page lands cut at this
/// bound and its frontmatter says so (`truncated: true`).
pub const MAX_CAPTURE_BYTES: usize = 512 * 1024;

/// One page as an agent read it.
#[derive(Debug, Clone, Copy)]
pub struct Capture<'a> {
    /// The address the page was read at, as the browser reported it.
    pub url: &'a str,
    pub title: &'a str,
    pub text: &'a str,
    /// Who read it: the agent and the pane, as the window names them.
    pub by: &'a str,
}

/// What the feed did with one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fed {
    /// The switch is off, or no vault is set: nothing read, nothing written.
    Off,
    /// Not an external web page: not `http`/`https`, or a local host.
    NotExternal,
    /// `raw/` already holds this source, at this vault-relative path.
    Held(String),
    /// `raw/` holds more entries than the feed reads looking for the source,
    /// so it writes nothing rather than risk a second copy.
    Full,
    /// A new file, at this vault-relative path.
    Written(String),
}

/// Drop one page into `vault`'s `raw/` when `enabled`.
///
/// `now_ms` stamps `captured_at`; `offset_minutes` east of UTC picks the day
/// the file is named for — the person's day, which only the window knows.
pub fn feed(
    vault: Option<&Path>,
    enabled: bool,
    page: &Capture<'_>,
    now_ms: i64,
    offset_minutes: i32,
) -> io::Result<Fed> {
    let _ = (vault, enabled, page, now_ms, offset_minutes);
    Ok(Fed::Off)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::civil;
    use crate::second_brain::RAW_DIR;

    /// Seoul, where 2026-10-12 01:02:03 UTC is 10:02 the same day.
    const SEOUL: i32 = 9 * 60;

    fn now() -> i64 {
        civil::days_from_civil(2026, 10, 12) * civil::MS_PER_DAY + 3_723_000
    }

    fn page<'a>(url: &'a str, title: &'a str) -> Capture<'a> {
        Capture {
            url,
            title,
            text: "본문 첫 줄\n둘째 줄",
            by: "claude · term-3",
        }
    }

    fn fed(root: &Path, url: &str, title: &str) -> Fed {
        feed(Some(root), true, &page(url, title), now(), SEOUL).unwrap()
    }

    fn raw_names(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(root.join(RAW_DIR))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn a_read_page_lands_in_raw_with_its_source_its_time_and_its_reader() {
        let root = tempfile::tempdir().unwrap();
        let answer = fed(
            root.path(),
            "https://user:secret@Example.com/post?id=7#section",
            "Some Title: A Story",
        );
        assert_eq!(
            answer,
            Fed::Written("raw/2026-10-12-some-title-a-story.md".to_string())
        );
        let written =
            fs::read_to_string(root.path().join("raw/2026-10-12-some-title-a-story.md")).unwrap();
        assert!(
            written.starts_with("---\nsource: https://example.com/post?id=7\n"),
            "no credentials and no fragment in the source: {written}"
        );
        let stamp = format!("\ncaptured_at: {}\n", civil::iso_utc_of(now()));
        assert!(written.contains(&stamp), "{written}");
        assert!(written.contains("\nby: \"claude · term-3\"\n"), "{written}");
        assert!(
            written.contains("\ntitle: \"Some Title: A Story\"\n"),
            "{written}"
        );
        assert!(
            written.ends_with("---\n\n본문 첫 줄\n둘째 줄\n"),
            "{written}"
        );
    }

    #[test]
    fn the_same_source_read_again_adds_nothing() {
        let root = tempfile::tempdir().unwrap();
        let first = fed(root.path(), "https://example.com/post?id=7", "Some Title");
        let again = feed(
            Some(root.path()),
            true,
            &page("https://example.com/post?id=7#later", "Another Title"),
            now() + civil::MS_PER_DAY,
            SEOUL,
        )
        .unwrap();
        assert_eq!(
            first,
            Fed::Written("raw/2026-10-12-some-title.md".to_string())
        );
        assert_eq!(again, Fed::Held("raw/2026-10-12-some-title.md".to_string()));
        assert_eq!(raw_names(root.path()), ["2026-10-12-some-title.md"]);
    }

    #[test]
    fn a_source_the_person_filed_by_hand_counts_as_held() {
        let root = tempfile::tempdir().unwrap();
        let clips = root.path().join(RAW_DIR).join("clips");
        fs::create_dir_all(&clips).unwrap();
        fs::write(
            clips.join("mine.md"),
            "---\nsource: \"https://Example.com/post#top\"\n---\nmine\n",
        )
        .unwrap();
        assert_eq!(
            fed(root.path(), "https://example.com/post", "Some Title"),
            Fed::Held("raw/clips/mine.md".to_string())
        );
    }

    #[test]
    fn with_the_switch_off_or_no_vault_nothing_is_read_or_written() {
        let root = tempfile::tempdir().unwrap();
        let off = feed(
            Some(root.path()),
            false,
            &page("https://example.com/", "T"),
            now(),
            SEOUL,
        );
        assert_eq!(off.unwrap(), Fed::Off);
        let nowhere = feed(None, true, &page("https://example.com/", "T"), now(), SEOUL);
        assert_eq!(nowhere.unwrap(), Fed::Off);
        assert!(!root.path().join(RAW_DIR).exists());
    }

    #[test]
    fn only_an_external_web_page_is_fed() {
        let root = tempfile::tempdir().unwrap();
        for url in [
            "file:///etc/hosts",
            "about:blank",
            "not a url",
            "http://localhost:3000/",
            "http://app.localhost/",
            "http://printer.local/",
            "http://127.0.0.1:8080/",
            "http://10.0.0.7/",
            "http://192.168.0.10/",
            "http://169.254.1.1/",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://[fe80::1]/",
            "http://[::ffff:127.0.0.1]/",
        ] {
            assert_eq!(fed(root.path(), url, "T"), Fed::NotExternal, "{url}");
        }
        assert!(!root.path().join(RAW_DIR).exists());
        assert_eq!(
            fed(root.path(), "https://93.184.215.14/", "T"),
            Fed::Written("raw/2026-10-12-t.md".to_string())
        );
    }

    #[test]
    fn a_file_already_in_raw_is_never_opened_for_writing() {
        let root = tempfile::tempdir().unwrap();
        let raw = root.path().join(RAW_DIR);
        fs::create_dir_all(&raw).unwrap();
        let theirs = "---\nsource: https://other.example/\n---\nthe person's own note\n";
        fs::write(raw.join("2026-10-12-some-title.md"), theirs).unwrap();
        assert_eq!(
            fed(root.path(), "https://example.com/post", "Some Title"),
            Fed::Written("raw/2026-10-12-some-title-2.md".to_string())
        );
        assert_eq!(
            fs::read_to_string(raw.join("2026-10-12-some-title.md")).unwrap(),
            theirs
        );
    }

    #[test]
    fn a_title_with_no_ascii_word_is_named_by_its_address() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            fed(
                root.path(),
                "https://news.example.com/articles/42",
                "에이전트 그래프 소식"
            ),
            Fed::Written("raw/2026-10-12-news-example-com-articles-42.md".to_string())
        );
    }

    #[test]
    fn a_page_longer_than_the_bound_lands_cut_and_says_so() {
        let root = tempfile::tempdir().unwrap();
        let long = "가".repeat(MAX_CAPTURE_BYTES);
        let capture = Capture {
            url: "https://example.com/long",
            title: "Long",
            text: &long,
            by: "codex · term-1",
        };
        let answer = feed(Some(root.path()), true, &capture, now(), SEOUL).unwrap();
        let Fed::Written(path) = answer else {
            panic!("{answer:?}");
        };
        let written = fs::read_to_string(root.path().join(path)).unwrap();
        assert!(written.contains("\ntruncated: true\n"), "the cut is said");
        let (_, body) = written.split_once("---\n\n").unwrap();
        let body = body.trim_end_matches('\n');
        assert!(body.len() <= MAX_CAPTURE_BYTES, "{}", body.len());
        assert!(
            !body.is_empty() && body.chars().all(|held| held == '가'),
            "cut on a character"
        );
    }
}
