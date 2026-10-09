//! The transcript file a pane's conversation is read from (herdr 4, t-26597).
//!
//! The rule is `zerocode_core::pane_transcript::bind`. This file only gathers
//! its facts from the disk and from the pane's process, so every road to a
//! transcript the window reads goes through the one answer.

use std::path::{Path, PathBuf};

use zerocode_core::SessionKey;
use zerocode_core::pane_transcript::{Absent, Facts, bind};
use zerocode_core::vault::{self, AgentSource};

use crate::terminal_registry::{HeldTerminal, lock_pty};

/// The most bytes of a file the first line may take. A first line longer than
/// this names no session id, which is the safe answer.
const FIRST_LINE_BYTES: u64 = 64 * 1024;

/// The most bytes of a file's end a screen match reads.
const SCREEN_TEXT_BYTES: u64 = 256 * 1024;

/// The facts of one pane, read from the disk under `home` and from the
/// pane's process when it is live.
pub(crate) struct PaneFacts {
    home: Option<PathBuf>,
    pane: Option<HeldTerminal>,
}

impl Facts for PaneFacts {
    fn exists(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn roots(&self, source: &AgentSource) -> Vec<PathBuf> {
        let Some(home) = self.home.as_deref() else {
            return Vec::new();
        };
        let mut roots: Vec<PathBuf> =
            vault::roots(source, home, vault::env_home(source).as_deref())
                .into_iter()
                .map(|root| root.canonicalize().unwrap_or(root))
                .collect();
        roots.sort();
        roots.dedup();
        roots
    }

    fn session_files(&self, source: &AgentSource) -> Option<Vec<PathBuf>> {
        vault::session_files(source, &self.roots(source))
    }

    fn first_line_id(&self, path: &Path) -> Option<String> {
        vault::session_meta_id(&first_line(path)?)
    }

    fn open_files(&self) -> Vec<PathBuf> {
        let Some(held) = self.pane.as_ref() else {
            return Vec::new();
        };
        let Some(pid) = lock_pty(held).pid() else {
            return Vec::new();
        };
        open_files_of(pid)
    }

    fn screen_lines(&self) -> Vec<String> {
        let Some(held) = self.pane.as_ref() else {
            return Vec::new();
        };
        let text = lock_pty(held).terminal().grid().visible_text();
        text.lines().map(str::to_string).collect()
    }

    fn text_of(&self, path: &Path) -> Option<String> {
        tail_text(path, SCREEN_TEXT_BYTES)
    }
}

/// The transcript file one pane's conversation is read from, or why there is
/// none. Reads the agent's report and session id the pane holds, then the
/// rule.
pub(crate) fn pane_transcript(
    state: &crate::AppState,
    term: crate::TermId,
) -> Result<PathBuf, Absent> {
    let slug = state
        .agent_terms()
        .get(&term)
        .copied()
        .ok_or(Absent::NoAdapter)?;
    let (reported, session_id) = match state.pane_sessions().get(&term) {
        Some(session) => (
            session.transcript_path.clone(),
            (session.key == SessionKey::SessionId).then(|| session.id.clone()),
        ),
        None => (None, None),
    };
    let facts = PaneFacts {
        home: dirs::home_dir(),
        pane: state.terminals().handle(term),
    };
    bind(
        slug,
        reported.as_deref().map(Path::new),
        session_id.as_deref(),
        &facts,
    )
    .map(|(path, _)| path)
}

/// The first line of a file, read through at most [`FIRST_LINE_BYTES`].
fn first_line(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let mut head = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(FIRST_LINE_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    let end = head.iter().position(|byte| *byte == b'\n')?;
    Some(String::from_utf8_lossy(&head[..end]).into_owned())
}

/// The last `bytes` of a file, as text.
fn tail_text(path: &Path, bytes: u64) -> Option<String> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(bytes))).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    Some(String::from_utf8_lossy(&tail).into_owned())
}

/// The files a process holds open. Only macOS has the window's lsof road
/// (`system_runtime::run_lsof`); elsewhere no file is named this way, and the
/// rule falls through to the screen.
#[cfg(target_os = "macos")]
fn open_files_of(pid: u32) -> Vec<PathBuf> {
    let pid = pid.to_string();
    crate::system_runtime::run_lsof(&["-nP", "-p", pid.as_str(), "-Fn"])
        .map(|raw| parse_open_names(&raw))
        .unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
fn open_files_of(_pid: u32) -> Vec<PathBuf> {
    Vec::new()
}

/// lsof's field output: one `n<name>` line per name. A socket or a pipe is
/// named by something that is not a path, and it is not a file.
#[cfg(target_os = "macos")]
fn parse_open_names(raw: &str) -> Vec<PathBuf> {
    raw.lines()
        .filter_map(|line| line.strip_prefix('n'))
        .filter(|name| name.starts_with('/'))
        .map(PathBuf::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// One session file, written with its own id on its first line.
    fn write_session(path: &Path, id_line: &str) {
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("folder");
        std::fs::write(path, format!("{id_line}\n{{\"type\":\"message\"}}\n")).expect("file");
    }

    /// The synthetic bundle: three zo panels and three Codex panels, all in
    /// one folder's stores, each with its own session id and no report of
    /// its transcript. A newer zo file belongs to no panel. The "newest file"
    /// column is what the forbidden rule would have named.
    #[test]
    fn panels_in_one_folder_each_name_their_own_transcript_without_a_report() {
        let dir = tempfile::tempdir().expect("a temporary home");
        let home = dir.path().canonicalize().expect("a real home");
        let zo_folder = home.join(".zo/projects/Users-dev-repo/sessions");
        let codex_folder = home.join(".codex/sessions/2026/10/10");

        let mut expected: Vec<(&'static str, String, PathBuf)> = Vec::new();
        for n in 0..3 {
            let id = format!("session-1000-{n}");
            let path = zo_folder.join(format!("{id}.jsonl"));
            write_session(
                &path,
                &format!("{{\"type\":\"session_meta\",\"session_id\":\"{id}\"}}"),
            );
            expected.push(("zo", id, path));
        }
        // The newest zo file, for no panel.
        std::thread::sleep(Duration::from_millis(15));
        let decoy = zo_folder.join("session-1000-9.jsonl");
        write_session(
            &decoy,
            "{\"type\":\"session_meta\",\"session_id\":\"session-1000-9\"}",
        );
        for n in 0..3 {
            let id = format!("cdx-{n}");
            let path = codex_folder.join(format!("rollout-2026-10-10T09-00-0{n}.jsonl"));
            write_session(
                &path,
                &format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\"}}}}"),
            );
            expected.push(("codex", id, path));
            std::thread::sleep(Duration::from_millis(15));
        }

        let facts = PaneFacts {
            home: Some(home.clone()),
            pane: None,
        };
        let mut right = 0;
        let mut wrong = 0;
        let mut none = 0;
        let started = Instant::now();
        for (slug, id, want) in &expected {
            match bind(slug, None, Some(id.as_str()), &facts) {
                Ok((path, _)) if &path == want => right += 1,
                Ok(_) => wrong += 1,
                Err(_) => none += 1,
            }
        }
        let ms_per_join = started.elapsed().as_secs_f64() * 1000.0 / expected.len() as f64;

        // What "the newest file of the folder" would have named, per panel.
        let newest = |folder: &Path| -> PathBuf {
            std::fs::read_dir(folder)
                .expect("folder")
                .flatten()
                .map(|entry| entry.path())
                .max_by_key(|path| {
                    std::fs::metadata(path)
                        .and_then(|meta| meta.modified())
                        .ok()
                })
                .expect("a file")
        };
        let zo_newest = newest(&zo_folder);
        let codex_newest = newest(&codex_folder);
        let forbidden_wrong = expected
            .iter()
            .filter(|(slug, _, want)| {
                let guess = if *slug == "zo" {
                    &zo_newest
                } else {
                    &codex_newest
                };
                guess != want
            })
            .count();

        println!(
            "t-26597 synthetic bundle: panels={} right={right} wrong={wrong} none={none} \
             newest_rule_wrong={forbidden_wrong} ms_per_join={ms_per_join:.3}",
            expected.len()
        );

        // A restart holds no state, so a second pass with a new facts value
        // names the same files.
        let after_restart = PaneFacts {
            home: Some(home),
            pane: None,
        };
        for (slug, id, want) in &expected {
            assert_eq!(
                bind(slug, None, Some(id.as_str()), &after_restart).map(|(path, _)| path),
                Ok(want.clone()),
                "after a restart the {slug} panel {id} names its own file"
            );
        }

        assert_eq!(wrong, 0, "no panel names another panel's file");
        assert_eq!(right, expected.len(), "every panel names its own file");
    }
}
