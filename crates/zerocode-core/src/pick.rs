//! The picker helper's contract: what the window says on `argv`, what the
//! helper says on stdout.
//!
//! The native file panel does not open inside the window any more. On macOS
//! `NSOpenPanel` is a remote view — the main thread waits, synchronously, for
//! `com.apple.appkit.xpc.openAndSavePanelService` to come up and connect,
//! and on macOS 26 that service raises an AutoFill shield on top — so three
//! times a window that had merely asked for a folder stopped painting and
//! was force-quit (t-2488 2026-09-05 02:49; 09-06 17:15; 09-07 11:38). The
//! panel now belongs to a helper process, `zerocode-pick`, bundled beside the
//! window's executable (as `zerocode-mirror` is), which opens it in ITS OWN
//! process and answers on stdout. The window spawns it, waits on the async
//! runtime, and stays free to paint, to say the panel is late, and to kill
//! it. Design: `docs/design/folder-panel-off-the-main-thread.md`.
//!
//! Both ends read this one module — the window to encode a request and
//! decode the answer, the helper to decode the request and encode the
//! answer — so the two cannot drift apart. The shape is deliberately the
//! plainest thing a shell script could fake, which is how the window's
//! tests drive it:
//!
//! ```text
//! zerocode-pick --kind folder|folders|file|files
//!               [--start <dir>] [--title <words>] [--filter <name>=<ext>,<ext>]...
//! ```
//!
//! stdout: one absolute path per line; nothing at all when the person
//! dismissed the panel, which is an answer and not an error. Exit 0 either
//! way; a non-zero exit is the helper saying it could not open the panel.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The helper's file name, without the platform's executable suffix.
pub const HELPER_NAME: &str = "zerocode-pick";

/// What kind of thing the panel is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickKind {
    Folder,
    Folders,
    File,
    Files,
}

impl PickKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Folder => "folder",
            Self::Folders => "folders",
            Self::File => "file",
            Self::Files => "files",
        }
    }

    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "folder" => Some(Self::Folder),
            "folders" => Some(Self::Folders),
            "file" => Some(Self::File),
            "files" => Some(Self::Files),
            _ => None,
        }
    }

    /// Whether the panel may answer with more than one path.
    #[must_use]
    pub fn many(self) -> bool {
        matches!(self, Self::Folders | Self::Files)
    }

    /// Whether the panel chooses folders rather than files.
    #[must_use]
    pub fn folders(self) -> bool {
        matches!(self, Self::Folder | Self::Folders)
    }
}

/// One entry of the panel's file-type menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

/// What the window asks the helper for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickRequest {
    pub kind: PickKind,
    /// Where the panel opens. `None` leaves it to the platform.
    pub start: Option<PathBuf>,
    pub title: Option<String>,
    pub filters: Vec<PickFilter>,
}

impl PickRequest {
    #[must_use]
    pub fn new(kind: PickKind) -> Self {
        Self {
            kind,
            start: None,
            title: None,
            filters: Vec::new(),
        }
    }

    #[must_use]
    pub fn starting_at(mut self, dir: impl Into<PathBuf>) -> Self {
        self.start = Some(dir.into());
        self
    }

    #[must_use]
    pub fn titled(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    #[must_use]
    pub fn filtered(mut self, name: impl Into<String>, extensions: &[&str]) -> Self {
        self.filters.push(PickFilter {
            name: name.into(),
            extensions: extensions.iter().map(|one| (*one).to_string()).collect(),
        });
        self
    }

    /// The request as the helper's arguments.
    #[must_use]
    pub fn argv(&self) -> Vec<OsString> {
        let mut argv = vec![OsString::from("--kind"), OsString::from(self.kind.as_str())];
        if let Some(start) = &self.start {
            argv.push(OsString::from("--start"));
            argv.push(start.as_os_str().to_os_string());
        }
        if let Some(title) = &self.title {
            argv.push(OsString::from("--title"));
            argv.push(OsString::from(title));
        }
        for filter in &self.filters {
            argv.push(OsString::from("--filter"));
            argv.push(OsString::from(format!(
                "{}={}",
                filter.name,
                filter.extensions.join(",")
            )));
        }
        argv
    }

    /// The helper's reading of its arguments. Anything it does not know is
    /// refused with the word that was not understood — a helper that
    /// guessed would open the wrong panel silently.
    pub fn parse_argv<I>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut kind = None;
        let mut request = Self::new(PickKind::Folder);
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let flag = flag.to_string_lossy().into_owned();
            let mut value = || args.next().ok_or_else(|| format!("{flag}: 값이 없습니다"));
            match flag.as_str() {
                "--kind" => {
                    let word = value()?.to_string_lossy().into_owned();
                    kind = Some(
                        PickKind::parse(&word)
                            .ok_or_else(|| format!("--kind {word}: 모르는 종류입니다"))?,
                    );
                }
                "--start" => request.start = Some(PathBuf::from(value()?)),
                "--title" => request.title = Some(value()?.to_string_lossy().into_owned()),
                "--filter" => {
                    let spec = value()?.to_string_lossy().into_owned();
                    let (name, extensions) = spec.split_once('=').ok_or_else(|| {
                        format!("--filter {spec}: 이름=확장자,확장자 꼴이어야 합니다")
                    })?;
                    request.filters.push(PickFilter {
                        name: name.to_string(),
                        extensions: extensions
                            .split(',')
                            .filter(|one| !one.is_empty())
                            .map(str::to_string)
                            .collect(),
                    });
                }
                other => return Err(format!("{other}: 모르는 인자입니다")),
            }
        }
        request.kind = kind.ok_or_else(|| "--kind 가 없습니다".to_string())?;
        Ok(request)
    }
}

/// The helper's answer: one path per line, byte-faithful on Unix. A path
/// with a newline in it cannot ride this shape and is refused rather than
/// split into two paths nobody chose.
pub fn format_answer(paths: &[PathBuf]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    for path in paths {
        let bytes = path_bytes(path);
        if bytes.contains(&b'\n') {
            return Err(format!(
                "{}: 줄바꿈이 든 경로는 답할 수 없습니다",
                path.display()
            ));
        }
        out.extend_from_slice(&bytes);
        out.push(b'\n');
    }
    Ok(out)
}

/// The window's reading of the helper's stdout. Empty lines are nothing;
/// a trailing carriage return is a console's, not the path's.
#[must_use]
pub fn parse_answer(stdout: &[u8]) -> Vec<PathBuf> {
    stdout
        .split(|byte| *byte == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.is_empty())
        .map(path_from_bytes)
        .collect()
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(OsStr::from_bytes(bytes))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::new(&String::from_utf8_lossy(bytes).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn a_request_rides_argv_and_comes_back_whole() {
        let request = PickRequest::new(PickKind::Files)
            .starting_at("/Users/me/src")
            .filtered("Warp YAML theme", &["yaml", "yml"])
            .filtered("Netscape cookies.txt", &["txt"]);
        let argv = request.argv();
        assert_eq!(
            argv,
            [
                "--kind",
                "files",
                "--start",
                "/Users/me/src",
                "--filter",
                "Warp YAML theme=yaml,yml",
                "--filter",
                "Netscape cookies.txt=txt",
            ]
            .map(OsString::from)
        );
        assert_eq!(PickRequest::parse_argv(argv).expect("parsed"), request);
        let bare = PickRequest::new(PickKind::Folder);
        assert_eq!(bare.argv(), ["--kind", "folder"].map(OsString::from));
        assert_eq!(PickRequest::parse_argv(bare.argv()).expect("parsed"), bare);
    }

    #[test]
    fn argv_the_helper_does_not_know_is_refused_with_its_own_words() {
        assert!(PickRequest::parse_argv(["--kind", "drawer"].map(OsString::from)).is_err());
        assert!(PickRequest::parse_argv(["--start", "/x"].map(OsString::from)).is_err());
        assert!(
            PickRequest::parse_argv(["--kind", "file", "--start"].map(OsString::from)).is_err()
        );
        assert!(
            PickRequest::parse_argv(["--kind", "file", "--filter", "noequals"].map(OsString::from))
                .is_err()
        );
        assert!(PickRequest::parse_argv(["--kind", "file", "--what"].map(OsString::from)).is_err());
    }

    #[test]
    fn every_kind_names_itself_and_says_whether_it_is_many() {
        for kind in [
            PickKind::Folder,
            PickKind::Folders,
            PickKind::File,
            PickKind::Files,
        ] {
            assert_eq!(PickKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(PickKind::parse("cupboard"), None);
        assert!(!PickKind::Folder.many() && !PickKind::File.many());
        assert!(PickKind::Folders.many() && PickKind::Files.many());
        assert!(PickKind::Folder.folders() && PickKind::Folders.folders());
        assert!(!PickKind::File.folders() && !PickKind::Files.folders());
    }

    #[test]
    fn the_answer_is_one_path_per_line_and_nothing_is_a_dismissal() {
        let paths = [PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b c")];
        let bytes = format_answer(&paths).expect("formatted");
        assert_eq!(bytes, b"/tmp/a\n/tmp/b c\n");
        assert_eq!(parse_answer(&bytes), paths);
        assert_eq!(parse_answer(b""), Vec::<PathBuf>::new());
        assert_eq!(parse_answer(b"\n\r\n"), Vec::<PathBuf>::new());
        // A stray carriage return (a Windows console) is not part of a path.
        assert_eq!(parse_answer(b"/tmp/a\r\n"), [PathBuf::from("/tmp/a")]);
        // A path with a newline in it cannot ride a line-per-path answer.
        assert!(format_answer(&[PathBuf::from("/tmp/a\nb")]).is_err());
    }
}
