//! The raw inbox feed (t-43992, design §4's second unit): an external page an
//! agent read in this window, dropped into the vault's `raw/` as one new file —
//! the compiler's input. The 「raw 취합」 turn eats it, and the health card
//! counts it until the log names it (`second_brain_lint`'s unlogged raw row).
//!
//! The window decides when: the person's opt-in switch, a configured vault, a
//! whole-page read. This file decides what lands: an external `http(s)` page
//! only, one file per source address, and never a file already in `raw/`
//! opened for writing.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::IpAddr;
use std::path::Path;

use url::{Host, Url};

use crate::civil;
use crate::localhost_label::is_loopback_hostname;
use crate::second_brain::{MAX_STATUS_ENTRIES, RAW_DIR};
use crate::second_brain_graph::{MARKDOWN_SUFFIX, parse_frontmatter, split_frontmatter, unquote};

/// Bytes of a page's text one capture keeps. A longer page lands cut at this
/// bound and its frontmatter says so (`truncated: true`).
pub const MAX_CAPTURE_BYTES: usize = 512 * 1024;
/// Characters of a title the frontmatter keeps.
pub const MAX_TITLE_CHARS: usize = 300;
/// Characters of a file name's slug.
pub const MAX_SLUG_CHARS: usize = 64;
/// Names tried for one day's slug (`-2`, `-3`, …) before the feed gives up.
pub const MAX_NAME_TRIES: u32 = 100;
/// Bytes read from the head of a raw file for its `source:` line.
const SOURCE_HEAD_BYTES: u64 = 4096;
/// The slug of a page whose title and address both name nothing.
const FALLBACK_SLUG: &str = "page";
/// Host names a person's own machine or network answers to: the loopback
/// names' subdomains (RFC 6761) and multicast DNS (RFC 6762).
const LOCAL_NAME_SUFFIXES: [&str; 2] = [".localhost", ".local"];

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
    let Some(root) = vault.filter(|_| enabled) else {
        return Ok(Fed::Off);
    };
    let Some(source) = external_source(page.url) else {
        return Ok(Fed::NotExternal);
    };
    let raw = root.join(RAW_DIR);
    match held_source(&raw, &source)? {
        Scan::Held(path) => return Ok(Fed::Held(path)),
        Scan::Full => return Ok(Fed::Full),
        Scan::Absent => {}
    }
    fs::create_dir_all(&raw)?;
    let stem = format!(
        "{}-{}",
        civil::iso_date_of(now_ms, offset_minutes),
        slug_of(page.title, &source)
    );
    let file = page_file(page, &source, &civil::iso_utc_of(now_ms));
    for attempt in 1..=MAX_NAME_TRIES {
        let name = if attempt == 1 {
            format!("{stem}{MARKDOWN_SUFFIX}")
        } else {
            format!("{stem}-{attempt}{MARKDOWN_SUFFIX}")
        };
        let path = raw.join(&name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut opened) => {
                if let Err(error) = opened.write_all(file.as_bytes()) {
                    // The file is the one this call created: a half-written
                    // capture is removed rather than left for the turn to eat.
                    drop(opened);
                    let _ = fs::remove_file(&path);
                    return Err(error);
                }
                return Ok(Fed::Written(format!("{RAW_DIR}/{name}")));
            }
            // Another capture of the same source may have landed between the
            // scan and this name (two reads at once): it is held, not doubled.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if source_of(&path).is_some_and(|held| same_source(&held, &source)) {
                    return Ok(Fed::Held(format!("{RAW_DIR}/{name}")));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("{RAW_DIR}/{stem}: every name up to -{MAX_NAME_TRIES} is taken"),
    ))
}

/// The address an external page is filed under — no credentials, no
/// fragment — or nothing for anything that is not an external `http(s)` page:
/// another scheme, a loopback or local name, a private or link-local address.
#[must_use]
pub fn external_source(url: &str) -> Option<String> {
    let mut parsed = Url::parse(url.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let external = match parsed.host()? {
        Host::Domain(name) => {
            !is_loopback_hostname(name)
                && !LOCAL_NAME_SUFFIXES
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
        }
        Host::Ipv4(address) => is_public(IpAddr::V4(address)),
        Host::Ipv6(address) => is_public(IpAddr::V6(address)),
    };
    if !external {
        return None;
    }
    parsed.set_fragment(None);
    parsed.set_username("").ok()?;
    parsed.set_password(None).ok()?;
    Some(parsed.into())
}

/// An address a page outside the person's machine and network can have.
fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => {
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast())
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public(IpAddr::V4(v4)),
            None => {
                !(v6.is_loopback()
                    || v6.is_unspecified()
                    || v6.is_unique_local()
                    || v6.is_unicast_link_local())
            }
        },
    }
}

/// Whether a `source:` value someone wrote names the same page.
fn same_source(held: &str, source: &str) -> bool {
    external_source(held).map_or(held == source, |filed| filed == source)
}

enum Scan {
    Absent,
    Held(String),
    Full,
}

/// The raw file — under any folder of `raw/` — whose `source:` names
/// `source`, reading at most [`MAX_STATUS_ENTRIES`] entries. Links are not
/// followed: `raw/` is the vault's own, and a walk must end.
fn held_source(raw: &Path, source: &str) -> io::Result<Scan> {
    let mut folders = vec![raw.to_path_buf()];
    let mut read = 0usize;
    while let Some(folder) = folders.pop() {
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            read += 1;
            if read > MAX_STATUS_ENTRIES {
                return Ok(Scan::Full);
            }
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_dir() {
                folders.push(path);
                continue;
            }
            let markdown = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(MARKDOWN_SUFFIX));
            if kind.is_file()
                && markdown
                && source_of(&path).is_some_and(|held| same_source(&held, source))
            {
                let relative = path.strip_prefix(raw).unwrap_or(&path);
                let relative = relative.to_string_lossy().replace('\\', "/");
                return Ok(Scan::Held(format!("{RAW_DIR}/{relative}")));
            }
        }
    }
    Ok(Scan::Absent)
}

/// A raw file's `source:` value, read from its head only.
fn source_of(path: &Path) -> Option<String> {
    let mut head = Vec::new();
    File::open(path)
        .ok()?
        .take(SOURCE_HEAD_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    let text = String::from_utf8_lossy(&head);
    let (front, _) = split_frontmatter(&text);
    let fields = parse_frontmatter(front);
    let source = unquote(fields.get("source")?.trim()).trim();
    (!source.is_empty()).then(|| source.to_string())
}

/// A file name's slug: the title's ASCII words, or — for a title with none —
/// the address's host and path.
fn slug_of(title: &str, source: &str) -> String {
    let from_title = slugify(title);
    if !from_title.is_empty() {
        return from_title;
    }
    let address = Url::parse(source)
        .map(|url| format!("{} {}", url.host_str().unwrap_or_default(), url.path()))
        .unwrap_or_default();
    let from_address = slugify(&address);
    if from_address.is_empty() {
        FALLBACK_SLUG.to_string()
    } else {
        from_address
    }
}

/// Lowercase ASCII letters and digits, every other run one `-`, at most
/// [`MAX_SLUG_CHARS`].
fn slugify(text: &str) -> String {
    let mut slug = String::new();
    for held in text.chars() {
        if slug.len() >= MAX_SLUG_CHARS {
            break;
        }
        if held.is_ascii_alphanumeric() {
            slug.push(held.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_end_matches('-').to_string()
}

/// The file one capture writes: its frontmatter, then the page's text.
fn page_file(page: &Capture<'_>, source: &str, captured_at: &str) -> String {
    let text = page.text.trim();
    let mut cut = text.len().min(MAX_CAPTURE_BYTES);
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let title: String = page.title.trim().chars().take(MAX_TITLE_CHARS).collect();
    let mut file = format!(
        "---\nsource: {source}\ncaptured_at: {captured_at}\nby: {}\ntitle: {}\n",
        quoted(page.by),
        quoted(&title)
    );
    if cut < text.len() {
        file.push_str("truncated: true\n");
    }
    file.push_str("---\n\n");
    file.push_str(&text[..cut]);
    file.push('\n');
    file
}

/// A YAML double-quoted scalar on one line.
fn quoted(value: &str) -> String {
    let mut held = String::with_capacity(value.len() + 2);
    held.push('"');
    for mark in value.chars() {
        match mark {
            '\\' => held.push_str("\\\\"),
            '"' => held.push_str("\\\""),
            control if control.is_control() => held.push(' '),
            other => held.push(other),
        }
    }
    held.push('"');
    held
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
