//! What an agent's transcript says it made (t-3233, design §2).
//!
//! The gallery looked empty because its only sources were automation evidence
//! and worker reports, while the same machine's Claude transcripts held twelve
//! distinct `claude.ai/code/artifact/…` links and a stream of pages written
//! straight into projects. This module reads those transcripts and says what
//! they contain — and nothing else: no disk walk, no store, no clock. The
//! shell's `artifact_transcripts` decides WHICH files to read and WHEN
//! (boot backfill, the hook road); this file turns lines into facts.
//!
//! Two shapes are read, both measured against real files:
//!
//! - **Claude** (`~/.claude/projects/<slug>/*.jsonl`): an assistant line whose
//!   `message.content[]` holds a `tool_use` (`name`, `input`, `id`), and a
//!   user line with the matching `tool_result` (`tool_use_id`) beside a
//!   top-level `toolUseResult` object. The `Artifact` tool's result carries
//!   `url`·`title`·`path`; `Write`'s carries `type` (`create`|`update`) and
//!   `filePath`; every line carries `timestamp`·`cwd`·`sessionId`.
//! - **zo** (`~/.zo/sessions/*.jsonl`): `{"type":"message","message":{"role",
//!   "blocks":[…]}}` where a block is `ContentBlock::to_json`'s spelling —
//!   `tool_use` with `name` and `input` as a JSON **string**, `tool_result`
//!   with `output` as a string (`write_file`'s is `WriteFileOutput`'s JSON:
//!   `type` then `filePath`). No timestamps, no cwd: the caller's project
//!   root stands in.
//!
//! A page counts only when the writer's OWN result says it created the file
//! (t-3952): an `Edit`, a `MultiEdit`, an `edit_file` or a `Write` over a file
//! that was already there changes the project's source — it does not make an
//! artifact. (The window's own `ui/index.html`, edited in place, was a gallery
//! page until this rule.) A page that was created and is edited later stays
//! one row: the store follows its file's stamp, not the transcript.
//! The created file must also have an extension in
//! [`crate::artifact::PAGE_EXTENSIONS`] and sit under the project root the
//! line names (`cwd`) or the caller handed in — a scratchpad HTML is the
//! source of a claude.ai artifact, not a page of the project, and a sibling
//! directory is another project.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::artifact::is_page_extension;
use crate::civil::epoch_ms_of_iso;

/// Whose transcript is being read — the two spellings above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    Claude,
    Zo,
}

/// A claude.ai artifact the transcript saw published.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemoteFact {
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub favicon: Option<String>,
    /// The file the Artifact tool published — kept for the drawer, never
    /// registered as a page (it is the artifact's source, wherever it lies).
    pub source_path: Option<PathBuf>,
    /// The result line's `timestamp`, as epoch milliseconds.
    pub at_ms: Option<i64>,
    pub session: Option<String>,
    pub project: Option<PathBuf>,
}

/// A page an agent created in its project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageFact {
    pub path: PathBuf,
    pub at_ms: Option<i64>,
    pub session: Option<String>,
    pub project: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    Remote(RemoteFact),
    Page(PageFact),
}

/// The tool names that publish a claude.ai artifact.
const ARTIFACT_TOOLS: &[&str] = &["Artifact"];
/// The word a writer's result uses for a file that did not exist before the
/// call — Claude's `toolUseResult.type`, zo's `WriteFileOutput.type`. Their
/// other word, `update`, and every edit result (which has no `type`) are
/// changes to a file the project already had.
const CREATED: &str = "create";
/// The result key naming the written file, in both speakers' spelling.
const FILE_PATH_KEY: &str = "filePath";
/// The one host whose links are artifacts.
const ARTIFACT_URL_PREFIX: &str = "https://claude.ai/code/artifact/";

/// The facts in `lines`, in the order the transcript states them. A page
/// written twice is one fact (the later time wins); an artifact published
/// twice to one url is one fact too.
#[must_use]
pub fn extract<I, S>(speaker: Speaker, lines: I, project: Option<&Path>) -> Vec<Fact>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    extract_incremental(
        speaker,
        lines,
        project,
        &mut ExtractionState::default(),
        crate::artifact::Limits::default().transcript_pending_max,
    )
}

/// Semantic state saved beside a transcript's byte cursor. Completed calls
/// leave the queue; a bounded tail read can reset it when earlier bytes vanish.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExtractionState {
    pending: VecDeque<(String, PendingArtifact)>,
}

impl ExtractionState {
    fn remember(&mut self, id: String, held: PendingArtifact, cap: usize) {
        self.take(&id);
        self.pending.push_back((id, held));
        while self.pending.len() > cap {
            self.pending.pop_front();
        }
    }

    fn take(&mut self, id: &str) -> Option<PendingArtifact> {
        let index = self.pending.iter().position(|(key, _)| key == id)?;
        self.pending.remove(index).map(|(_, held)| held)
    }
}

/// Read another stretch while retaining calls whose results have not arrived.
#[must_use]
pub fn extract_incremental<I, S>(
    speaker: Speaker,
    lines: I,
    project: Option<&Path>,
    state: &mut ExtractionState,
    pending_max: usize,
) -> Vec<Fact>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out: Vec<Fact> = Vec::new();
    for line in lines {
        let line = line.as_ref();
        // Cheap gate before the parse: most lines are prose.
        if !line.contains("tool_use") {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match speaker {
            Speaker::Claude => claude_line(&value, project, state, pending_max, &mut out),
            Speaker::Zo => zo_line(&value, project, &mut out),
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct PendingArtifact {
    title: Option<String>,
    description: Option<String>,
    favicon: Option<String>,
    source_path: Option<PathBuf>,
}

fn text(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|held| !held.is_empty())
        .map(str::to_string)
}

fn claude_line(
    value: &serde_json::Value,
    project: Option<&Path>,
    state: &mut ExtractionState,
    pending_max: usize,
    out: &mut Vec<Fact>,
) {
    let at_ms = text(value, "timestamp").and_then(|stamp| epoch_ms_of_iso(&stamp));
    let session = text(value, "sessionId").or_else(|| text(value, "session_id"));
    let cwd = text(value, "cwd").map(PathBuf::from);
    let root = cwd.as_deref().or(project);
    let Some(content) = value
        .pointer("/message/content")
        .and_then(serde_json::Value::as_array)
    else {
        return;
    };
    // `toolUseResult` belongs to the line, not to one block: a `Write` that
    // created its file says so there, once.
    let answered = content
        .iter()
        .any(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("tool_result"));
    if answered
        && let Some(path) = value
            .get("toolUseResult")
            .and_then(created_file)
            .and_then(|file| page_under(&file, root))
    {
        push_page(
            out,
            PageFact {
                path,
                at_ms,
                session: session.clone(),
                project: root.map(Path::to_path_buf),
            },
        );
    }
    for block in content {
        match block.get("type").and_then(serde_json::Value::as_str) {
            Some("tool_use") => {
                let Some(name) = block.get("name").and_then(serde_json::Value::as_str) else {
                    continue;
                };
                if !ARTIFACT_TOOLS.contains(&name) {
                    continue;
                }
                let input = block
                    .get("input")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                if let Some(id) = text(block, "id") {
                    state.remember(
                        id,
                        PendingArtifact {
                            title: text(&input, "title"),
                            description: text(&input, "description"),
                            favicon: text(&input, "favicon"),
                            source_path: text(&input, "file_path").map(PathBuf::from),
                        },
                        pending_max,
                    );
                }
            }
            Some("tool_result") => {
                let Some(id) = text(block, "tool_use_id") else {
                    continue;
                };
                let Some(held) = state.take(&id) else {
                    continue;
                };
                let result = value.get("toolUseResult");
                let url = result.and_then(|held| text(held, "url")).or_else(|| {
                    block
                        .get("content")
                        .and_then(serde_json::Value::as_str)
                        .and_then(artifact_url_in)
                });
                let Some(url) = url else {
                    continue;
                };
                let title = result
                    .and_then(|held| text(held, "title"))
                    .or(held.title)
                    .or_else(|| {
                        held.source_path
                            .as_deref()
                            .and_then(Path::file_stem)
                            .map(|stem| stem.to_string_lossy().into_owned())
                    });
                push_remote(
                    out,
                    RemoteFact {
                        url,
                        title,
                        description: held.description,
                        favicon: held.favicon,
                        source_path: result
                            .and_then(|held| text(held, "path"))
                            .map(PathBuf::from)
                            .or(held.source_path),
                        at_ms,
                        session: session.clone(),
                        project: cwd.clone(),
                    },
                );
            }
            _ => {}
        }
    }
}

fn zo_line(value: &serde_json::Value, project: Option<&Path>, out: &mut Vec<Fact>) {
    if value.get("type").and_then(serde_json::Value::as_str) != Some("message") {
        return;
    }
    let Some(blocks) = value
        .pointer("/message/blocks")
        .and_then(serde_json::Value::as_array)
    else {
        return;
    };
    for block in blocks {
        if block.get("type").and_then(serde_json::Value::as_str) != Some("tool_result")
            || block.get("is_error").and_then(serde_json::Value::as_bool) == Some(true)
        {
            continue;
        }
        // zo spells the output as a string (`ContentBlock::to_json`).
        let Some(path) = block
            .get("output")
            .and_then(serde_json::Value::as_str)
            .and_then(created_file_in_output)
            .and_then(|file| page_under(&file, project))
        else {
            continue;
        };
        push_page(
            out,
            PageFact {
                path,
                at_ms: None,
                session: None,
                project: project.map(Path::to_path_buf),
            },
        );
    }
}

/// The file a writer's result says it created — `type` is [`CREATED`] and
/// `filePath` names it. `None` for an overwrite, an edit, or any other result.
fn created_file(result: &serde_json::Value) -> Option<String> {
    (text(result, "type").as_deref() == Some(CREATED))
        .then(|| text(result, FILE_PATH_KEY))
        .flatten()
}

/// [`created_file`] over zo's string output. An output cut short is no longer
/// JSON, but `WriteFileOutput` spells `type` and `filePath` first, so a head
/// that says `create` still names its file.
fn created_file_in_output(output: &str) -> Option<String> {
    if let Ok(result) = serde_json::from_str::<serde_json::Value>(output) {
        return created_file(&result);
    }
    let key = format!("\"{FILE_PATH_KEY}\"");
    let at = output.find(&key)?;
    let head: String = output[..at].split_whitespace().collect();
    if head != format!("{{\"type\":\"{CREATED}\",") {
        return None;
    }
    let value = output[at + key.len()..].trim_start().strip_prefix(':')?;
    serde_json::Deserializer::from_str(value)
        .into_iter::<String>()
        .next()?
        .ok()
        .filter(|file| !file.trim().is_empty())
}

/// The page a created file is, when its extension is in the table and it
/// sits under the root. `None` otherwise — including when there is no root
/// to judge by, because "under the project" cannot be answered without a
/// project.
fn page_under(file: &str, root: Option<&Path>) -> Option<PathBuf> {
    let root = root?;
    if !root.is_absolute() {
        return None;
    }
    let path = root.join(file);
    if !is_page_extension(&path) {
        return None;
    }
    // This pure check rules out lexical escapes. Keep the original spelling:
    // collapsing .. before resolving a symlink changes which file was written.
    // The store canonicalizes both paths and checks containment again on disk.
    normalized(&path)
        .starts_with(normalized(root))
        .then_some(path)
}

fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The first `claude.ai/code/artifact/<id>` link in a result's prose.
fn artifact_url_in(said: &str) -> Option<String> {
    let start = said.find(ARTIFACT_URL_PREFIX)?;
    let rest = &said[start..];
    let end = rest
        .find(|ch: char| ch.is_whitespace() || matches!(ch, ')' | ']' | '>' | '"' | '\''))
        .unwrap_or(rest.len());
    let url = &rest[..end];
    (url.len() > ARTIFACT_URL_PREFIX.len()).then(|| url.to_string())
}

fn push_page(out: &mut Vec<Fact>, fact: PageFact) {
    if let Some(Fact::Page(held)) = out
        .iter_mut()
        .find(|held| matches!(held, Fact::Page(page) if page.path == fact.path))
    {
        if fact.at_ms >= held.at_ms {
            *held = fact;
        }
        return;
    }
    out.push(Fact::Page(fact));
}

fn push_remote(out: &mut Vec<Fact>, fact: RemoteFact) {
    if let Some(Fact::Remote(held)) = out
        .iter_mut()
        .find(|held| matches!(held, Fact::Remote(remote) if remote.url == fact.url))
    {
        if fact.at_ms >= held.at_ms {
            *held = fact;
        }
        return;
    }
    out.push(Fact::Remote(fact));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real lines from this machine's transcripts, anonymised: an `Artifact`
    /// tool_use and its result, a `Write` that created a Markdown file under
    /// its cwd, a `Write` that created an HTML file in a SIBLING directory of
    /// its cwd, an `Edit` of the project's existing `ui/index.html`, and a
    /// `Write` over the existing `README.md` (`type: update`).
    const CLAUDE: &str = include_str!("../fixtures/artifact-transcript/claude.jsonl");
    /// A zo session as `~/.zo/sessions` writes it: the meta line, then
    /// messages whose blocks are `ContentBlock::to_json`'s spelling — a
    /// `write_file` that created its page, an `edit_file`, a `bash`.
    const ZO: &str = include_str!("../fixtures/artifact-transcript/zo.jsonl");

    fn pages(facts: &[Fact]) -> Vec<&Path> {
        facts
            .iter()
            .filter_map(|fact| match fact {
                Fact::Page(page) => Some(page.path.as_path()),
                Fact::Remote(_) => None,
            })
            .collect()
    }

    fn line_with<'a>(text: &'a str, words: &[&str]) -> &'a str {
        text.lines()
            .find(|line| words.iter().all(|word| line.contains(word)))
            .unwrap_or_else(|| panic!("no fixture line holds {words:?}"))
    }

    #[test]
    fn unfinished_artifact_calls_keep_only_the_tables_newest_seats() {
        let line = CLAUDE.lines().next().unwrap();
        let mut state = ExtractionState::default();
        for n in 0..3 {
            let mut value: serde_json::Value = serde_json::from_str(line).unwrap();
            value["message"]["content"][0]["id"] = serde_json::json!(format!("call-{n}"));
            let _ = extract_incremental(Speaker::Claude, [value.to_string()], None, &mut state, 2);
        }
        assert_eq!(
            state
                .pending
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["call-1", "call-2"]
        );
    }

    #[test]
    fn relative_writer_paths_use_the_session_root_and_cannot_traverse_outside_it() {
        // A relative `filePath` is joined to the session root, the way
        // file_tools::enforce_workspace_boundary joins zo's input; the spelling
        // is kept, and a lexical escape is refused.
        let created = line_with(ZO, &["tool_result", "docs/report.html"]);
        for (file, expected) in [
            ("docs/report.html", Some("/repo/docs/report.html")),
            ("./docs/../report.md", Some("/repo/./docs/../report.md")),
            ("../elsewhere/report.html", None),
            ("/repo/../elsewhere/report.html", None),
            ("/repo-sibling/report.html", None),
        ] {
            let line = created.replace("/Users/someone/project/docs/report.html", file);
            let facts = extract(Speaker::Zo, [line], Some(Path::new("/repo")));
            assert_eq!(
                pages(&facts),
                expected.map(Path::new).into_iter().collect::<Vec<_>>(),
                "{file}"
            );
        }
    }

    /// The window's own `ui/index.html` was a gallery page because an agent
    /// EDITED it (t-3952). Edits, a `Write` over an existing file, a failed
    /// write, and the writer's call without its result are not pages; only
    /// the writer's own `create` is.
    #[test]
    fn edits_and_overwrites_of_existing_files_are_not_pages() {
        let root = Path::new("/Users/someone/project");
        let claude = extract(Speaker::Claude, CLAUDE.lines(), Some(root));
        let claude_pages = pages(&claude);
        assert!(
            !claude_pages
                .iter()
                .any(|path| path.ends_with("ui/index.html")),
            "an Edit registered a page: {claude_pages:?}"
        );
        assert!(
            !claude_pages.iter().any(|path| path.ends_with("README.md")),
            "a Write over an existing file registered a page: {claude_pages:?}"
        );
        // The calls alone — every assistant line — register nothing.
        let calls: Vec<&str> = CLAUDE
            .lines()
            .filter(|line| line.contains("\"type\": \"assistant\""))
            .collect();
        assert!(pages(&extract(Speaker::Claude, calls, Some(root))).is_empty());

        let zo = extract(Speaker::Zo, ZO.lines(), Some(root));
        assert!(
            !pages(&zo)
                .iter()
                .any(|path| path.ends_with("notes/plan.md")),
            "an edit_file registered a page: {zo:?}"
        );
        let created = line_with(ZO, &["tool_result", "docs/report.html"]);
        let failed = created.replace("\"is_error\":false", "\"is_error\":true");
        assert_ne!(failed, created, "the fixture spells is_error compactly");
        assert!(pages(&extract(Speaker::Zo, [failed], Some(root))).is_empty());
        let updated = created.replace("\\\"create\\\"", "\\\"update\\\"");
        assert_ne!(updated, created, "the fixture's output says create");
        assert!(pages(&extract(Speaker::Zo, [updated], Some(root))).is_empty());
    }

    /// An output cut short is not JSON; its head still says what the call
    /// did, and only a head that says `create` names a page.
    #[test]
    fn a_cut_write_output_is_judged_by_its_head() {
        let head = "{\n  \"type\": \"create\",\n  \"filePath\": \"/repo/page.html\",\n  \"content\": \"<main>";
        assert_eq!(
            created_file_in_output(head).as_deref(),
            Some("/repo/page.html")
        );
        let updated = head.replace("create", "update");
        assert_eq!(created_file_in_output(&updated), None);
        let edit = "{\n  \"filePath\": \"/repo/page.html\",\n  \"oldString\": \"a";
        assert_eq!(created_file_in_output(edit), None);
        assert_eq!(created_file_in_output("File created successfully"), None);
    }

    /// The Artifact tool's result names the url, the title, the source path
    /// and the time; the input names the description and favicon.
    #[test]
    fn a_claude_artifact_result_becomes_one_remote_fact() {
        let facts = extract(Speaker::Claude, CLAUDE.lines(), None);
        let remote = facts
            .iter()
            .find_map(|fact| match fact {
                Fact::Remote(remote) => Some(remote),
                Fact::Page(_) => None,
            })
            .expect("the published artifact is a fact");
        assert_eq!(
            remote.url,
            "https://claude.ai/code/artifact/aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"
        );
        assert_eq!(
            remote.title.as_deref(),
            Some("왜 MSA가 아니라 모듈러 모놀리스인가")
        );
        assert!(
            remote
                .description
                .as_deref()
                .is_some_and(|held| held.starts_with("플랫폼 v1"))
        );
        assert_eq!(remote.favicon.as_deref(), Some("🧱"));
        assert!(
            remote
                .source_path
                .as_deref()
                .is_some_and(|path| path.ends_with("platform-v1-why-modular-monolith.html"))
        );
        assert_eq!(
            remote.at_ms,
            epoch_ms_of_iso("2026-09-04T21:27:04.061Z"),
            "the result's timestamp is the artifact's time"
        );
        assert_eq!(
            remote.session.as_deref(),
            Some("11111111-2222-4333-8444-555555555555")
        );
        assert_eq!(
            remote.project.as_deref(),
            Some(Path::new("/Users/someone/work"))
        );
        // The Artifact's source file is not a page: it lies in the scratchpad.
        assert!(
            !facts.iter().any(|fact| matches!(fact, Fact::Page(page) if page.path.to_string_lossy().contains("scratchpad"))),
            "the artifact's source file was registered as a page: {facts:?}"
        );
    }

    /// A file a `Write` created under its own cwd is a page, dated by the
    /// result that said so; one it created in a sibling directory is not, and
    /// neither is a page when no root can judge it.
    #[test]
    fn a_write_is_a_page_only_under_its_project_root() {
        let facts = extract(Speaker::Claude, CLAUDE.lines(), None);
        let pages: Vec<&PageFact> = facts
            .iter()
            .filter_map(|fact| match fact {
                Fact::Page(page) => Some(page),
                Fact::Remote(_) => None,
            })
            .collect();
        assert_eq!(pages.len(), 1, "{pages:?}");
        assert_eq!(
            pages[0].path,
            PathBuf::from("/Users/someone/project/DECISIONS.md")
        );
        assert_eq!(
            pages[0].project.as_deref(),
            Some(Path::new("/Users/someone/project"))
        );
        assert_eq!(pages[0].at_ms, epoch_ms_of_iso("2026-08-07T19:52:20.412Z"));
        assert_eq!(
            pages[0].session.as_deref(),
            Some("33333333-4444-4555-8666-777777777777")
        );

        // The same creation, judged against a root that DOES hold the sibling.
        let line = line_with(CLAUDE, &["toolUseResult", "zerocode-landing/index.html"]);
        let widened = extract(
            Speaker::Claude,
            [line],
            Some(Path::new("/Users/someone/2026")),
        );
        assert!(
            widened.is_empty(),
            "the line's own cwd outranks the caller's root: {widened:?}"
        );
        let stripped = line.replace(r#""cwd": "/Users/someone/2026/zerocode", "#, "");
        assert!(!stripped.contains("\"cwd\""));
        let by_caller = extract(
            Speaker::Claude,
            [stripped.as_str()],
            Some(Path::new("/Users/someone/2026/zerocode-landing")),
        );
        assert_eq!(by_caller.len(), 1, "{by_caller:?}");
        let rootless = extract(Speaker::Claude, [stripped.as_str()], None);
        assert!(rootless.is_empty(), "no root, no page: {rootless:?}");
    }

    /// zo's blocks spell the output as a string; the `write_file` result that
    /// says `create` is a page, the `edit_file` and `bash` blocks are not, and
    /// the caller's root judges.
    #[test]
    fn zo_created_files_are_pages_under_the_callers_root() {
        let facts = extract(
            Speaker::Zo,
            ZO.lines(),
            Some(Path::new("/Users/someone/project")),
        );
        assert_eq!(
            pages(&facts),
            vec![Path::new("/Users/someone/project/docs/report.html")]
        );
        assert!(
            facts
                .iter()
                .all(|fact| matches!(fact, Fact::Page(page) if page.at_ms.is_none()))
        );
        let elsewhere = extract(Speaker::Zo, ZO.lines(), Some(Path::new("/Users/other")));
        assert!(elsewhere.is_empty(), "{elsewhere:?}");
    }

    /// One url twice is one fact with the later time; one path twice is one
    /// fact; a result whose `toolUseResult` is missing still yields the url
    /// from the prose.
    #[test]
    fn repeats_fold_and_the_url_is_read_from_prose_when_the_result_object_is_missing() {
        let use_line = CLAUDE.lines().next().expect("use");
        let result_line = CLAUDE.lines().nth(1).expect("result");
        let later = result_line.replace("2026-09-04T21:27:04.061Z", "2026-09-05T00:00:00.000Z");
        let facts = extract(
            Speaker::Claude,
            [use_line, result_line, use_line, later.as_str()],
            None,
        );
        assert_eq!(facts.len(), 1, "{facts:?}");
        assert_eq!(
            match &facts[0] {
                Fact::Remote(remote) => remote.at_ms,
                Fact::Page(_) => None,
            },
            epoch_ms_of_iso("2026-09-05T00:00:00.000Z")
        );
        let no_object: String = {
            let value: serde_json::Value = serde_json::from_str(result_line).expect("json");
            let mut value = value;
            value
                .as_object_mut()
                .expect("object")
                .remove("toolUseResult");
            serde_json::to_string(&value).expect("json")
        };
        let from_prose = extract(Speaker::Claude, [use_line, no_object.as_str()], None);
        assert!(
            matches!(&from_prose[..], [Fact::Remote(remote)] if remote.url.ends_with("eeeeeeeeeeee")
                && remote.title.as_deref() == Some("platform-v1-why-modular-monolith")
                && remote.favicon.as_deref() == Some("🧱")),
            "{from_prose:?}"
        );
        assert_eq!(
            artifact_url_in("Published x at https://claude.ai/code/artifact/abc-1)\n\nmore"),
            Some("https://claude.ai/code/artifact/abc-1".to_string())
        );
        assert_eq!(artifact_url_in("nothing here"), None);
    }
}
