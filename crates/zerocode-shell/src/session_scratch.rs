//! An agent session's own folder, read for the person (t-83039).
//!
//! An answer may name a file its agent wrote beside itself: Claude Code keeps
//! one folder per session under its temp root —
//! `/tmp/claude-<uid>/<project folder>/<session id>/`, its scratchpad and its
//! background tasks' output — and a person who presses that name wants to
//! read it. The project's file door refuses it, as it must: a path outside the
//! checkout is not the project's to read or write. This is a narrower door
//! beside it, and it does not widen that one.
//!
//! It reads a file only inside the folder of a session this window holds in
//! its session table, and the folder's two names come from that session's own
//! transcript (`<project folder>/<session id>.jsonl`), never from the path it
//! is asked about. It reads only a regular text file within the table's text
//! cap, for a tab nobody can write. A path anywhere else — a key, a keychain, a
//! browser's cookies — is refused before it is opened.

use std::path::{Component, Path, PathBuf};

use zerocode_core::ProviderSession;

use crate::artifact_runtime::{DocumentText, TextRefusal, bounded_text};

/// The folder Claude Code's per-user temp root stands in: `/tmp/claude-<uid>`,
/// as the CLI's own scratchpad lines name it.
const CLAUDE_TEMP_PARENT: &str = "/tmp";

/// Why a path was not read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScratchRefusal {
    /// Not inside the folder of any session this window holds — never opened.
    Outside,
    /// Inside one, but not a regular text file within the cap; the words say
    /// which.
    Unreadable(String),
}

/// This user's Claude Code temp root, as the disk names it (`/tmp` is
/// `/private/tmp` on macOS) — `None` where there is none.
pub(crate) fn claude_temp_root() -> Option<PathBuf> {
    let uid = crate::pane_leftovers::own_user();
    Path::new(CLAUDE_TEMP_PARENT)
        .join(format!("claude-{uid}"))
        .canonicalize()
        .ok()
}

/// The folders of the sessions this window holds, under `temp`. A session
/// names one only when its transcript is Claude Code's shape — its file is
/// `<session id>.jsonl` — and the id is one plain name: a session id that is a
/// path, or `..`, names no folder.
pub(crate) fn session_folders<'a>(
    temp: &Path,
    sessions: impl IntoIterator<Item = &'a ProviderSession>,
) -> Vec<PathBuf> {
    sessions
        .into_iter()
        .filter_map(|session| {
            let transcript = Path::new(session.transcript_path.as_deref()?);
            if transcript.file_stem()? != session.id.as_str() {
                return None;
            }
            let id = Path::new(&session.id);
            let mut parts = id.components();
            if !matches!(parts.next(), Some(Component::Normal(_))) || parts.next().is_some() {
                return None;
            }
            let project = transcript.parent()?.file_name()?;
            Some(temp.join(project).join(id))
        })
        .collect()
}

/// One file inside one of `folders`, read for a tab nobody can write.
///
/// Judged on the disk's own name for the path — a link inside a folder that
/// leads out of it is where it leads — and before anything is opened. A file
/// longer than `cap` is not cut: it is refused, because a tab that showed part
/// of it would have nowhere to send the person for the rest.
pub(crate) fn read_scratch(
    path: &str,
    folders: &[PathBuf],
    cap: u64,
) -> Result<DocumentText, ScratchRefusal> {
    let asked = Path::new(path);
    if !asked.is_absolute() {
        return Err(ScratchRefusal::Outside);
    }
    let Ok(real) = asked.canonicalize() else {
        return Err(ScratchRefusal::Outside);
    };
    let inside = folders
        .iter()
        .filter_map(|folder| folder.canonicalize().ok())
        .any(|folder| real.starts_with(folder));
    if !inside {
        return Err(ScratchRefusal::Outside);
    }
    let text = bounded_text(&real, cap).map_err(|refusal| {
        ScratchRefusal::Unreadable(match refusal {
            TextRefusal::Gone => "the file is gone".to_string(),
            TextRefusal::NotAFile => "not a regular file".to_string(),
            TextRefusal::Binary => "a binary file".to_string(),
            TextRefusal::Unread(error) => error,
        })
    })?;
    if text.truncated {
        return Err(ScratchRefusal::Unreadable(format!(
            "{} bytes, over the {cap}-byte cap",
            text.bytes
        )));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::SessionKey;

    const PROJECT: &str = "-Users-dev-zerocode";

    fn session(id: &str, transcript: &str) -> ProviderSession {
        ProviderSession {
            key: SessionKey::SessionId,
            id: id.to_string(),
            transcript_path: Some(transcript.to_string()),
        }
    }

    /// A temp root holding one session's folder with a scratch note in it,
    /// and the folders the window would name for `held`.
    struct Scene {
        _temp: tempfile::TempDir,
        root: PathBuf,
        folders: Vec<PathBuf>,
    }

    fn scene(held: &[ProviderSession]) -> Scene {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let scratch = root.join(PROJECT).join("s-1").join("scratchpad");
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("notes.md"), "# 메모\n\n본문").unwrap();
        let folders = session_folders(&root, held);
        Scene {
            _temp: temp,
            root,
            folders,
        }
    }

    fn held() -> Vec<ProviderSession> {
        vec![session(
            "s-1",
            &format!("/Users/dev/.claude/projects/{PROJECT}/s-1.jsonl"),
        )]
    }

    fn at(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn a_held_sessions_scratch_file_is_read_whole() {
        let scene = scene(&held());
        let note = scene.root.join(PROJECT).join("s-1/scratchpad/notes.md");
        let read = read_scratch(&at(&note), &scene.folders, 1024).unwrap();
        assert_eq!(read.text, "# 메모\n\n본문");
        assert!(!read.truncated);
    }

    #[test]
    fn the_folder_comes_from_the_transcript_and_a_session_not_held_names_none() {
        let scene = scene(&held());
        assert_eq!(scene.folders, vec![scene.root.join(PROJECT).join("s-1")]);
        let other = scene.root.join(PROJECT).join("s-2/scratchpad");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("notes.md"), "다른 세션").unwrap();
        assert_eq!(
            read_scratch(&at(&other.join("notes.md")), &scene.folders, 1024),
            Err(ScratchRefusal::Outside)
        );
    }

    #[test]
    fn an_id_that_is_a_path_or_a_transcript_of_another_shape_names_no_folder() {
        let root = Path::new("/private/tmp/claude-501");
        let odd = [
            session(
                "..",
                &format!("/Users/dev/.claude/projects/{PROJECT}/...jsonl"),
            ),
            session(
                "a/b",
                &format!("/Users/dev/.claude/projects/{PROJECT}/a/b.jsonl"),
            ),
            session(
                "s-1",
                "/Users/dev/.codex/sessions/2026/10/29/rollout-s-9.jsonl",
            ),
            ProviderSession {
                key: SessionKey::SessionId,
                id: "s-1".to_string(),
                transcript_path: None,
            },
        ];
        assert_eq!(session_folders(root, &odd), Vec::<PathBuf>::new());
    }

    /// The secret boundary: a key, a keychain and a browser's cookies are no
    /// session's folder, so they are refused as outside — before they are
    /// opened, which the unreadable one proves: reading it would have failed
    /// with the disk's own refusal instead.
    #[test]
    fn keys_keychains_and_cookies_are_outside_and_never_opened() {
        let scene = scene(&held());
        let home = scene.root.join("home");
        let secrets = [
            home.join(".ssh/id_ed25519"),
            home.join("Library/Keychains/login.keychain-db"),
            home.join("Library/Application Support/Google/Chrome/Default/Cookies"),
        ];
        for secret in &secrets {
            std::fs::create_dir_all(secret.parent().unwrap()).unwrap();
            std::fs::write(secret, "secret").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(secret, std::fs::Permissions::from_mode(0o000)).unwrap();
            }
            assert_eq!(
                read_scratch(&at(secret), &scene.folders, 1024),
                Err(ScratchRefusal::Outside),
                "{}",
                secret.display()
            );
        }
        for named in [
            "/Users/dev/.ssh/id_ed25519",
            "/Users/dev/Library/Keychains/login.keychain-db",
        ] {
            assert_eq!(
                read_scratch(named, &scene.folders, 1024),
                Err(ScratchRefusal::Outside)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_the_folder_that_leads_out_is_where_it_leads() {
        let scene = scene(&held());
        let outside = scene.root.join("outside.md");
        std::fs::write(&outside, "밖").unwrap();
        let link = scene.root.join(PROJECT).join("s-1/scratchpad/escape.md");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        assert_eq!(
            read_scratch(&at(&link), &scene.folders, 1024),
            Err(ScratchRefusal::Outside)
        );
    }

    #[test]
    fn a_relative_path_or_a_gone_one_is_outside() {
        let scene = scene(&held());
        assert_eq!(
            read_scratch("s-1/scratchpad/notes.md", &scene.folders, 1024),
            Err(ScratchRefusal::Outside)
        );
        let gone = scene.root.join(PROJECT).join("s-1/scratchpad/gone.md");
        assert_eq!(
            read_scratch(&at(&gone), &scene.folders, 1024),
            Err(ScratchRefusal::Outside)
        );
    }

    #[test]
    fn a_binary_a_folder_or_a_file_over_the_cap_is_refused_in_words() {
        let scene = scene(&held());
        let scratch = scene.root.join(PROJECT).join("s-1/scratchpad");
        std::fs::write(scratch.join("shot.png"), [0x89, b'P', b'N', b'G', 0, 1]).unwrap();
        std::fs::write(scratch.join("long.md"), "가".repeat(100)).unwrap();
        for (path, cap) in [
            (scratch.join("shot.png"), 1024),
            (scratch.clone(), 1024),
            (scratch.join("long.md"), 16),
        ] {
            assert!(
                matches!(
                    read_scratch(&at(&path), &scene.folders, cap),
                    Err(ScratchRefusal::Unreadable(_))
                ),
                "{}",
                path.display()
            );
        }
    }
}
