//! The transcript file a pane's conversation is read from (herdr 4, t-26597).
//!
//! The rule is `zerocode_core::pane_transcript::bind`, asked through the
//! window's memory of each pane's answer (`bind_remembered`). This file only
//! gathers the facts from the disk and from the pane's process, so every road
//! to a transcript the window reads goes through the one answer.

use super::*;

use std::path::{Path, PathBuf};

use zerocode_core::SessionKey;
use zerocode_core::pane_transcript::{Absent, Facts, bind_remembered};
use zerocode_core::vault::{self, AgentSource};

use crate::terminal_registry::{HeldTerminal, lock_pty};

/// The most bytes of a file the first line may take. A first line longer than
/// this names no session id, which is the safe answer.
const FIRST_LINE_BYTES: u64 = 64 * 1024;

/// The most bytes of a file's end a screen match reads.
const SCREEN_TEXT_BYTES: u64 = 256 * 1024;

/// The facts of one pane, read from the disk under `home` and from the
/// pane's process when it is live. `read_env` says whether the environment
/// may move a store's root (`CODEX_HOME` and the like): the window reads it,
/// and a synthetic bundle does not, so its answer never depends on the
/// machine's own transcripts.
pub(crate) struct PaneFacts {
    home: Option<PathBuf>,
    pane: Option<HeldTerminal>,
    read_env: bool,
}

impl Facts for PaneFacts {
    fn exists(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn roots(&self, source: &AgentSource) -> Vec<PathBuf> {
        let Some(home) = self.home.as_deref() else {
            return Vec::new();
        };
        let named = if self.read_env {
            vault::env_home(source)
        } else {
            None
        };
        let mut roots: Vec<PathBuf> = vault::roots(source, home, named.as_deref())
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

    fn session_candidates(&self, source: &AgentSource, id: &str) -> Option<Vec<PathBuf>> {
        vault::session_candidates(source, &self.roots(source), id)
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
/// rule, through the window's memory of the pane's answer.
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
        read_env: true,
    };
    bind_remembered(
        state.pane_memo(),
        term,
        slug,
        reported.as_deref().map(Path::new),
        session_id.as_deref(),
        &facts,
    )
    .map(|(path, _)| path)
}

/// The first line of a file, read through at most [`FIRST_LINE_BYTES`].
fn first_line(path: &Path) -> Option<String> {
    use std::io::{BufRead as _, Read as _};
    let capped = std::fs::File::open(path).ok()?.take(FIRST_LINE_BYTES);
    let mut line = Vec::new();
    // Read up to the newline and no further: a session file is megabytes, and
    // its first line is the only part an id lookup needs.
    std::io::BufReader::new(capped)
        .read_until(b'\n', &mut line)
        .ok()?;
    if line.pop() != Some(b'\n') {
        return None;
    }
    Some(String::from_utf8_lossy(&line).into_owned())
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

/// lsof's flags for one process's open files: no name lookups (`-nP`), the
/// process picked by its id (`-p`), and the field output (`-Fn`) that
/// [`parse_open_names`] reads.
#[cfg(target_os = "macos")]
const LSOF_NO_LOOKUPS: &str = "-nP";
#[cfg(target_os = "macos")]
const LSOF_PROCESS: &str = "-p";
#[cfg(target_os = "macos")]
const LSOF_NAME_FIELDS: &str = "-Fn";

/// The files a process holds open. Only macOS has the window's lsof road
/// (`system_runtime::run_lsof`); elsewhere no file is named this way, and the
/// rule falls through to the screen.
#[cfg(target_os = "macos")]
fn open_files_of(pid: u32) -> Vec<PathBuf> {
    let pid = pid.to_string();
    crate::system_runtime::run_lsof(&[
        LSOF_NO_LOOKUPS,
        LSOF_PROCESS,
        pid.as_str(),
        LSOF_NAME_FIELDS,
    ])
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
            read_env: false,
        };
        let mut right = 0;
        let mut wrong = 0;
        let mut none = 0;
        let started = Instant::now();
        for (slug, id, want) in &expected {
            let one = Instant::now();
            let answer = bind(slug, None, Some(id.as_str()), &facts);
            println!("t-26597 join {slug} {id} us={}", one.elapsed().as_micros());
            match answer {
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
            read_env: false,
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

    // ------------------------------------------- the memory and real sizes (t-42948)

    use std::cell::Cell;
    use std::sync::Mutex;
    use zerocode_core::pane_transcript::{Memo, Via, bind, bind_remembered};

    /// The one screen line the synthetic panel shows. It is long enough to name
    /// a file, and only one of the 200 screen files holds it.
    const SCREEN_LINE: &str = "Refactor the drain test so it stops flaking";

    /// A session id shaped like a uuid, so a file name can end with it.
    fn uuid(n: usize) -> String {
        format!("{n:08x}-0000-4000-8000-{n:012x}")
    }

    /// A Codex rollout's first line, `bytes` long, naming `id`.
    fn codex_first_line(id: &str, bytes: usize) -> String {
        let padding = "x".repeat(bytes.saturating_sub(id.len() + 48));
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\",\"instructions\":\"{padding}\"}}}}"
        )
    }

    fn write_text(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("folder");
        std::fs::write(path, text).expect("file");
    }

    /// `count` Codex rollouts in one day's folder. Each is named by its id and
    /// holds that id on its first line. Returns every file's (id, path).
    fn write_codex_store(
        home: &Path,
        count: usize,
        first_line_bytes: usize,
    ) -> Vec<(String, PathBuf)> {
        let folder = home.join(".codex/sessions/2026/10/10");
        (0..count)
            .map(|n| {
                let id = uuid(n);
                let path = folder.join(format!("rollout-2026-10-10T09-00-00-{id}.jsonl"));
                let text = format!(
                    "{}\n{{\"type\":\"message\"}}\n",
                    codex_first_line(&id, first_line_bytes)
                );
                write_text(&path, &text);
                (id, path)
            })
            .collect()
    }

    /// `folders` × `per_folder` Claude session files. Each is `<id>.jsonl` under
    /// its own project folder, and `bytes` long.
    fn write_claude_store(
        home: &Path,
        folders: usize,
        per_folder: usize,
        bytes: usize,
    ) -> Vec<(String, PathBuf)> {
        let mut all = Vec::new();
        for folder in 0..folders {
            for n in 0..per_folder {
                let id = uuid(folder * per_folder + n);
                let path = home.join(format!(
                    ".claude/projects/Users-dev-project-{folder}/{id}.jsonl"
                ));
                write_text(&path, &"a".repeat(bytes));
                all.push((id, path));
            }
        }
        all
    }

    /// 200 Claude session files of 256 KiB each, in 20 project folders. The
    /// file numbered `holder` ends with [`SCREEN_LINE`]. No other file holds it.
    fn write_screen_store(home: &Path, holder: usize) -> Vec<PathBuf> {
        let size = 256 * 1024;
        (0..200)
            .map(|n| {
                let id = uuid(n);
                let path = home.join(format!(
                    ".claude/projects/Users-dev-project-{}/{id}.jsonl",
                    n / 10
                ));
                let last = if n == holder {
                    SCREEN_LINE
                } else {
                    "a line that is not on the screen"
                };
                let body = format!("{}\n{last}", "a".repeat(size - last.len() - 1));
                write_text(&path, &body);
                path
            })
            .collect()
    }

    /// The window's facts over a synthetic home, with each read counted and a
    /// screen the test gives. A panel with no PTY shows no screen of its own.
    struct Counted {
        facts: PaneFacts,
        screen: Vec<String>,
        walks: Cell<usize>,
        first_lines: Cell<usize>,
    }

    impl Counted {
        fn at(home: &Path, screen: &[&str]) -> Self {
            Self {
                facts: PaneFacts {
                    home: Some(home.to_path_buf()),
                    pane: None,
                    read_env: false,
                },
                screen: screen.iter().map(|line| line.to_string()).collect(),
                walks: Cell::new(0),
                first_lines: Cell::new(0),
            }
        }
    }

    impl Facts for Counted {
        fn exists(&self, path: &Path) -> bool {
            self.facts.exists(path)
        }

        fn roots(&self, source: &AgentSource) -> Vec<PathBuf> {
            self.facts.roots(source)
        }

        fn session_files(&self, source: &AgentSource) -> Option<Vec<PathBuf>> {
            self.walks.set(self.walks.get() + 1);
            self.facts.session_files(source)
        }

        fn session_candidates(&self, source: &AgentSource, id: &str) -> Option<Vec<PathBuf>> {
            self.walks.set(self.walks.get() + 1);
            self.facts.session_candidates(source, id)
        }

        fn first_line_id(&self, path: &Path) -> Option<String> {
            self.first_lines.set(self.first_lines.get() + 1);
            self.facts.first_line_id(path)
        }

        fn open_files(&self) -> Vec<PathBuf> {
            self.facts.open_files()
        }

        fn screen_lines(&self) -> Vec<String> {
            self.screen.clone()
        }

        fn text_of(&self, path: &Path) -> Option<String> {
            self.facts.text_of(path)
        }
    }

    /// A Codex panel names its file by the id in the file's name, so the id's
    /// file is the only first line read. Red on the code that reads every
    /// first line of the store.
    #[test]
    fn a_codex_panel_reads_one_first_line_in_a_400_file_store() {
        let dir = tempfile::tempdir().expect("a temporary home");
        let home = dir.path().canonicalize().expect("a real home");
        let store = write_codex_store(&home, 400, 24 * 1024);
        let (id, want) = &store[137];
        let facts = Counted::at(&home, &[]);
        assert_eq!(
            bind("codex", None, Some(id.as_str()), &facts).map(|(path, _)| path),
            Ok(want.clone())
        );
        assert!(
            facts.first_lines.get() <= 1,
            "the id's file is found by its name, so at most one first line is read; read {}",
            facts.first_lines.get()
        );
    }

    /// An id that no file name carries names no file, and reads no first line.
    #[test]
    fn a_codex_id_that_no_file_name_carries_reads_no_first_line() {
        let dir = tempfile::tempdir().expect("a temporary home");
        let home = dir.path().canonicalize().expect("a real home");
        write_codex_store(&home, 400, 24 * 1024);
        let facts = Counted::at(&home, &[]);
        assert_eq!(
            bind("codex", None, Some(uuid(9_999).as_str()), &facts),
            Err(Absent::NoSessionFile)
        );
        assert_eq!(
            facts.first_lines.get(),
            0,
            "no first line is read for an id that no name carries"
        );
    }

    /// A Codex file whose name carries no id is still found by its first line.
    #[test]
    fn a_codex_file_named_without_an_id_is_found_by_its_first_line() {
        let dir = tempfile::tempdir().expect("a temporary home");
        let home = dir.path().canonicalize().expect("a real home");
        let folder = home.join(".codex/sessions/2026/09/01");
        for n in 0..3 {
            let text = format!("{}\n", codex_first_line(&uuid(500 + n), 1024));
            write_text(&folder.join(format!("rollout-old-{n}.jsonl")), &text);
        }
        let facts = Counted::at(&home, &[]);
        assert_eq!(
            bind("codex", None, Some(uuid(501).as_str()), &facts).map(|(path, _)| path),
            Ok(folder.join("rollout-old-1.jsonl"))
        );
    }

    /// A Claude store past the 4000-file listing cap still names a panel's file.
    /// The file is named by its id, so its folder is looked at and no listing is
    /// made. Red on the code that lists the store first.
    #[test]
    fn a_claude_panel_finds_its_file_in_a_store_past_the_listing_cap() {
        let dir = tempfile::tempdir().expect("a temporary home");
        let home = dir.path().canonicalize().expect("a real home");
        let store = write_claude_store(&home, 41, 100, 64);
        let (id, want) = &store[2_050];
        let facts = Counted::at(&home, &[]);
        assert_eq!(
            bind("claude", None, Some(id.as_str()), &facts).map(|(path, _)| path),
            Ok(want.clone())
        );
    }

    /// The same Claude id in two project folders names neither file.
    #[test]
    fn a_claude_id_in_two_project_folders_names_neither() {
        let dir = tempfile::tempdir().expect("a temporary home");
        let home = dir.path().canonicalize().expect("a real home");
        let id = uuid(77);
        write_text(
            &home.join(format!(".claude/projects/Users-dev-a/{id}.jsonl")),
            "a",
        );
        write_text(
            &home.join(format!(".claude/projects/Users-dev-b/{id}.jsonl")),
            "a",
        );
        let facts = Counted::at(&home, &[]);
        assert_eq!(
            bind("claude", None, Some(id.as_str()), &facts),
            Err(Absent::TwoSessionFiles)
        );
    }

    /// One panel's join at a real size: the first join, then the 63 joins that
    /// follow it on the same panel. Prints the times in milliseconds, and returns
    /// the first answer.
    fn join_row(
        label: &str,
        facts: &Counted,
        slug: &str,
        id: Option<&str>,
    ) -> Result<(PathBuf, Via), Absent> {
        let memo = Mutex::new(Memo::<u32>::default());
        let started = Instant::now();
        let answer = bind_remembered(&memo, 7, slug, None, id, facts);
        let first_ms = started.elapsed().as_secs_f64() * 1000.0;
        let (listed_first, lines_first) = (facts.walks.get(), facts.first_lines.get());
        let started = Instant::now();
        for _ in 0..63 {
            let _ = bind_remembered(&memo, 7, slug, None, id, facts);
        }
        let next_mean_ms = started.elapsed().as_secs_f64() * 1000.0 / 63.0;
        println!(
            "t-42948 measure {label}: first_ms={first_ms:.3} next_mean_ms={next_mean_ms:.3} \
             listed_first={listed_first} first_lines_first={lines_first} \
             listed_all={} first_lines_all={} answer={:?}",
            facts.walks.get(),
            facts.first_lines.get(),
            answer.as_ref().map(|(_, via)| via),
        );
        answer
    }

    /// The join's cost at the sizes herdr meets (t-42948): 400 Codex rollouts
    /// with 24 KiB first lines; a store past the 4000-file cap (Claude and
    /// Codex); and 200 screen files of 256 KiB. Prints one line per scenario and
    /// checks each answer. Run it with
    /// `cargo test -p zerocode-shell measure_the_join_at_real_sizes -- --ignored --nocapture`.
    #[test]
    #[ignore = "measurement: run with --ignored --nocapture"]
    fn measure_the_join_at_real_sizes() {
        type Answer = Result<(PathBuf, Via), Absent>;
        let mut checks: Vec<(&str, Answer, Answer)> = Vec::new();

        {
            let dir = tempfile::tempdir().expect("a temporary home");
            let home = dir.path().canonicalize().expect("a real home");
            let store = write_codex_store(&home, 400, 24 * 1024);
            let facts = Counted::at(&home, &[]);
            let got = join_row(
                "codex 400 files, first line 24 KiB, id named by the file",
                &facts,
                "codex",
                Some(store[200].0.as_str()),
            );
            checks.push(("codex-400", got, Ok((store[200].1.clone(), Via::SessionId))));
        }
        {
            let dir = tempfile::tempdir().expect("a temporary home");
            let home = dir.path().canonicalize().expect("a real home");
            let store = write_claude_store(&home, 41, 100, 64);
            let facts = Counted::at(&home, &[]);
            let got = join_row(
                "claude 4100 files past the listing cap, id named by the file",
                &facts,
                "claude",
                Some(store[2_050].0.as_str()),
            );
            checks.push((
                "claude-4100",
                got,
                Ok((store[2_050].1.clone(), Via::SessionId)),
            ));
        }
        {
            let dir = tempfile::tempdir().expect("a temporary home");
            let home = dir.path().canonicalize().expect("a real home");
            let store = write_codex_store(&home, 4_100, 1024);
            let facts = Counted::at(&home, &[]);
            let got = join_row(
                "codex 4100 files past the listing cap, first line 1 KiB",
                &facts,
                "codex",
                Some(store[2_050].0.as_str()),
            );
            checks.push(("codex-4100", got, Err(Absent::NoSessionFile)));
        }
        {
            let dir = tempfile::tempdir().expect("a temporary home");
            let home = dir.path().canonicalize().expect("a real home");
            let paths = write_screen_store(&home, 123);
            let facts = Counted::at(&home, &[SCREEN_LINE]);
            let got = join_row(
                "screen 200 files of 256 KiB, the screen line held by one file",
                &facts,
                "claude",
                None,
            );
            checks.push((
                "screen-200-match",
                got,
                Ok((paths[123].clone(), Via::ScreenMatch)),
            ));
            let facts = Counted::at(&home, &["Nothing in this store says this sentence"]);
            let got = join_row(
                "screen 200 files of 256 KiB, the screen line held by no file",
                &facts,
                "claude",
                None,
            );
            checks.push(("screen-200-miss", got, Err(Absent::NoScreenMatch)));
        }

        for (label, got, want) in checks {
            assert_eq!(got, want, "{label}: the answer");
        }
    }
}
