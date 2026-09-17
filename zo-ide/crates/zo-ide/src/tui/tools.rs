//! 도구 턴의 셀 — codex `tui/src/exec_cell/`(model.rs·render.rs) 와
//! `history_cell/{patches,mcp}.rs` 의 문법을 그대로 옮긴 것.
//!
//! # 어휘 (codex 실측)
//!
//! | zo 도구 | 도는 중 | 커밋 | 출처 |
//! |---|---|---|---|
//! | Bash | `• Running <cmd>` | `• Ran <cmd>` | `exec_cell/render.rs::command_call_display_lines` |
//! | Read·Grep·Glob | `• Exploring` + `  └ Read x` | `• Explored` + 같은 목록 | `exec_cell/render.rs::exploring_display_lines` |
//! | 연속 명령 | — | `• Ran N commands` | `exec_cell/render.rs::compact_group_display_lines` |
//! | Edit·Write | `• Editing <path>` | `• Edited <path> (+A -B)` | `diff_render.rs::render_changes_block` |
//! | 그 외(MCP 등) | `• Calling <name>(<args>)` | `• Called …` | `history_cell/mcp.rs::render_lines` |
//!
//! `Editing` 만 원본에 없는 낱말이다 — codex 의 패치 셀은 승인 게이트를 거쳐
//! 한 번에 커밋되므로 라이브 형태가 아예 없다. 그래서 원본이 스스로 쓰는
//! `-ing`/`-ed` 짝(`Running`/`Ran`·`Calling`/`Called`·`Exploring`/`Explored`)을
//! 그대로 이어 붙였다.
//!
//! # 트랜스크립트 힌트를 뺀 이유
//!
//! codex 는 접힌 셀 뒤에 `· ctrl + t to view transcript` 를 붙인다
//! (`ui_consts.rs::TRANSCRIPT_HINT`). 그 문구는 **오버레이가 있을 때**의
//! 안내이고 이번 라운드에는 오버레이를 만들지 않으므로 생략한다. 대신 단일
//! 명령의 출력 미리보기 규칙(아래 `OUTPUT_MAX_ROWS`)을 원본대로 지켜
//! 무엇이 실행됐는지는 셀에서 읽을 수 있게 한다.
//!
//! # 히스토리 재그리기 없이 라이브 셀을 두는 방법
//!
//! codex 는 도는 셀을 `transcript.active_cell` — **뷰포트**의 한 칸 — 에 두고
//! 매 프레임 다시 그린다(`chatwidget/rendering.rs::as_renderable`: active cell
//! 먼저, 그 아래 `top: 1` 을 띄운 bottom pane). 끝나면
//! `flush_active_cell()` 이 그것을 히스토리 셀로 넘긴다. 우리도 같다 —
//! [`ToolGroup::lines`] 하나가 두 자리를 다 그린다.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use runtime::message_stream::{
    AgentResultStatus, BashResult, DiffLineKind, DiffView, TodoResultItem, ToolPreview,
    ToolResultBody,
};

use super::ansi::{Color, Line, Span, Style};
use super::cells::{prefixed, Prefix};
use super::folds::{self, FoldIds, FoldMode};
use super::palette;
use super::shimmer::activity_marker;
use super::wrap::wrap_line;

/// 한 셀에 묶는 도구 호출 수 상한 — codex `exec_cell/model.rs::MAX_GROUPED_COMMANDS`.
const MAX_GROUPED_CALLS: usize = 32;
/// 도구 출력 미리보기가 차지할 수 있는 화면 행 수.
///
/// codex 는 두 단으로 자른다: `output_lines` 가 논리 줄을
/// `TOOL_CALL_MAX_LINES`(5) 로, 그 뒤 `truncate_lines_middle` 이 화면 행을
/// `EXEC_DISPLAY_LAYOUT.output_max_lines`(5) 로. 두 상한이 같은 5 이고 사람이
/// 보는 것은 행이므로 여기서는 한 단으로 접는다 — 머리 절반·중간 생략·꼬리
/// 절반이라는 원본의 모양과 "생략 수는 논리 줄로 센다"는 원본의 계약은 그대로다.
const OUTPUT_MAX_ROWS: usize = 5;
/// An upstream formatter only reports that it omitted *some* source output.
/// Count that unknown tail conservatively as one line, in the same elision
/// grammar the visible-row cap uses, instead of adding a second
/// `… (truncated)` dialect to the cell.
const SOURCE_TRUNCATION_NOTE: &str = "… +1 lines";
/// Re-injected agent reports need enough context to be useful in history.
const AGENT_RESULT_MAX_ROWS: usize = 20;
/// 명령이 여러 줄일 때 헤더 아래로 흐르는 행 수 상한 — 같은 레이아웃의
/// `command_continuation_max_lines`(2).
const COMMAND_CONTINUATION_MAX_ROWS: usize = 2;

/// 도구 하나가 무엇을 하는지 — codex 의 `ParsedCommand`/셀 종류에 대응한다.
#[derive(Debug, Clone)]
pub enum ToolKind {
    /// Bash. `Running`/`Ran`. `background` is a command that ran as a task
    /// (`run_in_background`) and came back down the completion road: its
    /// header carries one dim word beside the command and nothing else
    /// differs from the foreground cell (t-3177).
    Command { command: String, background: bool },
    /// Read·Grep·Glob 류. 한 호출이 여러 항목을 낼 수 있다(codex `parsed`).
    Explore(Vec<Explored>),
    /// Edit·Write. `Editing`/`Edited`.
    Edit { path: String },
    /// `TodoWrite` / `TaskList` result. `Updated Plan` with an uncapped checklist.
    Plan(Vec<TodoResultItem>),
    /// Web search/fetch activity. `Searching the web` / `Searched the web`.
    WebSearch { detail: String },
    /// Agent-family spawn. A running one shows the helper's live row; the
    /// completion says `Spawned`.
    Spawn {
        label: String,
        role: Option<String>,
        request: Option<String>,
        prompt: String,
        /// The helpers' live rows, already rendered. Workflow and
        /// `SpawnMultiAgent` helpers share this outer call id, so one call may
        /// own several rows. The poller rewrites them once a second at most;
        /// the cell repaints every frame, so the strings are not built here.
        progress: Vec<String>,
    },
    /// A re-injected sub-agent report. `Completed` / `Failed`, with a larger
    /// body budget, and what the run cost when the helper reported it.
    AgentResult {
        label: String,
        summary: Option<String>,
    },
    /// 그 밖의 도구 — codex 의 MCP 셀 문법을 쓴다.
    Call { name: String, detail: String },
}

impl ToolKind {
    /// 같은 셀로 묶일 수 있는 종류인가. codex 에서 묶이는 것은 `ExecCell`
    /// (명령·탐색)뿐이고 MCP 셀과 패치 셀은 각각 제 히스토리 셀이 된다.
    const fn groupable(&self) -> bool {
        matches!(self, Self::Command { .. } | Self::Explore(_))
    }

    const fn is_explore(&self) -> bool {
        matches!(self, Self::Explore(_))
    }

    const fn is_edit(&self) -> bool {
        matches!(self, Self::Edit { .. })
    }

    const fn is_spawn(&self) -> bool {
        matches!(self, Self::Spawn { .. })
    }

    /// 도구 호출 preview 를 셀 종류로 옮긴다.
    #[must_use]
    pub fn from_preview(_name: &str, preview: &ToolPreview, summary: &str) -> Self {
        match preview {
            ToolPreview::Bash { command } => Self::Command {
                command: sanitize(command, summary),
                background: false,
            },
            ToolPreview::Read { path, range } => {
                // 범위는 남긴다 — 한 파일의 두 창이 같은 행으로 접히지 않게
                // 하려고 zo 가 preview 에 실어 보내는 값이다. codex 도
                // `Read` 항목을 `.unique()` 로 접으므로 구분이 필요하다.
                let name = match range {
                    Some((start, end)) => format!("{path}:{start}-{end}"),
                    None => path.clone(),
                };
                Self::Explore(vec![Explored::Read {
                    name: sanitize(&name, summary),
                }])
            }
            ToolPreview::Glob { pattern } => Self::Explore(vec![Explored::List {
                path: sanitize(pattern, summary),
            }]),
            ToolPreview::Grep { pattern, path } => Self::Explore(vec![Explored::Search {
                query: sanitize(pattern, summary),
                path: path.as_deref().map(|path| sanitize(path, "")),
            }]),
            ToolPreview::Write { path, .. } | ToolPreview::Edit { path, .. } => Self::Edit {
                path: sanitize(path, summary),
            },
            ToolPreview::Search { query } => Self::WebSearch {
                detail: sanitize(query, summary),
            },
            ToolPreview::Generic {
                name,
                input_summary,
            } if is_spawn_tool(name) => spawn_kind(input_summary),
            ToolPreview::Generic {
                name,
                input_summary,
            } => Self::Call {
                name: name.clone(),
                detail: sanitize(input_summary, ""),
            },
        }
    }

    /// A result body that changes the history-cell grammar after completion.
    #[must_use]
    pub fn from_result(body: &ToolResultBody) -> Option<Self> {
        match body {
            ToolResultBody::Todos(items) => Some(Self::Plan(items.clone())),
            _ => None,
        }
    }

    /// Name whose result formatter matches this preview kind.
    ///
    /// Ordinary calls keep their announced name. `CapabilityInvoke` is only a
    /// wire-level address, so its unwrapped preview supplies the effective
    /// name used when the matching result arrives.
    pub(super) fn result_formatter_name<'a>(&'a self, announced_name: &'a str) -> &'a str {
        if announced_name != "CapabilityInvoke" {
            return announced_name;
        }
        match self {
            Self::Command { .. } => "Bash",
            Self::Explore(items) => match items.first() {
                Some(Explored::Read { .. }) => "Read",
                Some(Explored::List { .. }) => "Glob",
                Some(Explored::Search { .. }) => "Grep",
                None => announced_name,
            },
            Self::Edit { .. } => "Edit",
            Self::WebSearch { .. } => "WebSearch",
            Self::Spawn { .. } => "Agent",
            Self::Call { name, .. } => name,
            Self::Plan(_) | Self::AgentResult { .. } => announced_name,
        }
    }
}

fn sanitize(text: &str, fallback: &str) -> String {
    let cleaned = crate::util::ansi::sanitize_inline(text.trim());
    if cleaned.is_empty() {
        crate::util::ansi::sanitize_inline(fallback.trim())
    } else {
        cleaned
    }
}

fn tool_activity_marker(elapsed: Duration) -> Span {
    let level = palette::terminal_palette().map_or_else(
        palette::color_level_from_env,
        palette::TerminalPalette::color_level,
    );
    activity_marker_for_level(elapsed, level)
}

fn activity_marker_for_level(elapsed: Duration, level: palette::ColorLevel) -> Span {
    if level == palette::ColorLevel::Ansi16 {
        if (elapsed.as_millis() / 600).is_multiple_of(2) {
            Span::raw("•")
        } else {
            Span::dim("◦")
        }
    } else {
        activity_marker(elapsed)
    }
}

fn is_spawn_tool(name: &str) -> bool {
    crate::session::plain_session::is_spawn_family_tool(name)
}

fn spawn_kind(summary: &str) -> ToolKind {
    let (title, prompt) = summary.split_once(" · ").unwrap_or((summary, ""));
    let (title, request) = title
        .strip_suffix(')')
        .and_then(|title| title.rsplit_once(" ("))
        .map_or((title, None), |(title, request)| (title, Some(request)));
    let (label, role) = title
        .strip_suffix(']')
        .and_then(|title| title.rsplit_once(" ["))
        .map_or((title, None), |(label, role)| (label, Some(role)));

    ToolKind::Spawn {
        label: sanitize(label, "agent"),
        role: role.map(|role| sanitize(role, "")).filter(|role| !role.is_empty()),
        request: request
            .map(|request| sanitize(request, ""))
            .filter(|request| !request.is_empty()),
        prompt: sanitize(prompt, ""),
        progress: Vec::new(),
    }
}

/// 한 스폰 호출을 그리는 데 필요한 것들 — [`ToolKind::Spawn`] 의 빌린 모습.
struct SpawnCell<'a> {
    label: &'a str,
    role: Option<&'a str>,
    request: Option<&'a str>,
    prompt: &'a str,
    progress: &'a [String],
}

/// `Exploring`/`Explored` 목록의 한 항목 — codex `ParsedCommand` 의 세 갈래.
#[derive(Debug, Clone)]
pub enum Explored {
    Read { name: String },
    List { path: String },
    Search { query: String, path: Option<String> },
}

/// 파일 변경 요약 — codex `diff_render.rs` 의 한 행.
#[derive(Debug, Clone)]
pub struct Change {
    /// `Added`·`Deleted`·`Edited`.
    pub verb: &'static str,
    pub path: String,
    pub added: usize,
    pub removed: usize,
    /// The change itself, kept so the cell can draw it. Rendering is
    /// width-dependent, so the hunks travel rather than finished lines.
    pub view: DiffView,
}

struct EditGroupEntry<'a> {
    path: String,
    change: Option<&'a Change>,
    /// The first line of what the tool said when the edit FAILED — the entry
    /// then stands in the group as a failure with its reason, never as
    /// `Edited … (+0 -0)`.
    failure: Option<String>,
}

impl Change {
    /// 결과 diff 에서 동사와 증감을 읽는다. codex 는 `FileChange::Add` →
    /// `Added`, `Delete` → `Deleted`, 그 외 → `Edited` 로 가른다.
    #[must_use]
    pub fn from_diff(diff: &DiffView, fallback_path: &str) -> Self {
        let verb = match (diff.old_path.as_ref(), diff.new_path.as_ref()) {
            (None, Some(_)) => "Added",
            (Some(_), None) => "Deleted",
            _ => "Edited",
        };
        let path = diff
            .new_path
            .clone()
            .or_else(|| diff.old_path.clone())
            .unwrap_or_else(|| fallback_path.to_string());
        let mut added = 0;
        let mut removed = 0;
        for hunk in &diff.hunks {
            for line in &hunk.lines {
                match line.kind {
                    DiffLineKind::Added => added += 1,
                    DiffLineKind::Removed => removed += 1,
                    DiffLineKind::Context => {}
                }
            }
        }
        Self {
            verb,
            path: crate::util::ansi::sanitize_inline(&path),
            added,
            removed,
            view: diff.clone(),
        }
    }

    fn display_path(&self) -> String {
        match (&self.view.old_path, &self.view.new_path) {
            (Some(old), Some(new)) if old != new => format!("{old} → {new}"),
            _ => self.path.clone(),
        }
    }
}

/// The reason a write or edit was refused, without the words the row already
/// carries: the runtime's `invalid input: write_file: /abs/path …` prefix
/// repeats the tool and the path, and pushed the sentence that matters
/// (`exists but has not been read …`) off the right edge (2026-09-07).
fn failure_reason(first_line: &str, path: &str) -> String {
    let mut rest = first_line.trim();
    if let Some(after) = rest.strip_prefix("invalid input: ") {
        rest = after;
    }
    if let Some((tool, tail)) = rest.split_once(": ") {
        if !tool.is_empty() && !tool.contains(' ') && !tool.contains('/') {
            rest = tail;
        }
    }
    if let Some((head, tail)) = rest.split_once(' ') {
        if head == path || head.ends_with(&format!("/{path}")) {
            rest = tail;
        }
    }
    rest.to_string()
}

/// How many lines of a NEWLY written file the transcript shows before saying
/// how many more there are — a create is the whole file, and the whole file is
/// not a diff anyone reads in a transcript.
const NEW_FILE_HEAD_LINES: usize = 12;

/// The drawn body of a change: an edit's hunks as they are, a created file's
/// first lines and a count of the rest.
fn change_body(change: &Change, drawn: Vec<Line>) -> Vec<Line> {
    if change.verb != "Added" || drawn.len() <= NEW_FILE_HEAD_LINES {
        return drawn;
    }
    let shown = NEW_FILE_HEAD_LINES;
    let more = change.added.saturating_sub(shown);
    let mut head: Vec<Line> = drawn.into_iter().take(shown).collect();
    head.push(Line::new(vec![Span::dim(super::strings::more_lines(more))]));
    head
}

/// `✘ Failed to apply patch` — codex's patch-failure header, one spelling for
/// the single edit and the group alike.
fn patch_failure_line(width: usize) -> Line {
    Line::new(vec![Span::new(
        "✘ Failed to apply patch",
        Style::new().fg(Color::MAGENTA).bold(),
    )])
    .truncated(width)
}

fn change_count_spans(added: usize, removed: usize) -> Vec<Span> {
    vec![
        Span::raw("("),
        Span::new(format!("+{added}"), Style::new().fg(Color::GREEN)),
        Span::raw(" "),
        Span::new(format!("-{removed}"), Style::new().fg(Color::RED)),
        Span::raw(")"),
    ]
}

/// 끝난 도구 호출의 결과.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub ok: bool,
    /// The dispatch policy declined the call: `ok` is false, but nothing
    /// failed — the cell heads it as a verdict (`ToolResultBody::Declined`).
    pub declined: bool,
    /// 미리보기로 보일 출력 원문.
    pub output: String,
    /// Edit·Write 결과의 파일 변경 요약(있으면 헤더가 이것을 쓴다).
    pub change: Option<Change>,
}

impl Outcome {
    /// `RenderBlock::ToolResult` 를 결과로 옮긴다.
    #[must_use]
    pub fn from_result(is_error: bool, body: &ToolResultBody, path: &str) -> Self {
        let ok = !is_error
            && match body {
                ToolResultBody::Bash(result) => result.exit_code == 0,
                _ => true,
            };
        let change = match body {
            ToolResultBody::Diff(diff) => Some(Change::from_diff(diff, path)),
            _ => None,
        };
        Self {
            ok,
            declined: matches!(body, ToolResultBody::Declined { .. }),
            output: body_text(body),
            change,
        }
    }
}

/// 도구 호출 하나 — codex `ExecCall`.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub kind: ToolKind,
    pub started: Instant,
    pub done: Option<Outcome>,
}

impl ToolCall {
    #[must_use]
    pub fn new(id: String, kind: ToolKind) -> Self {
        Self {
            id,
            kind,
            started: Instant::now(),
            done: None,
        }
    }

    fn succeeded(&self) -> bool {
        self.done.as_ref().is_some_and(|done| done.ok)
    }

    fn failed(&self) -> bool {
        self.done.as_ref().is_some_and(|done| !done.ok)
    }
}

/// 한 자리(뷰포트의 활성 셀 / 히스토리의 한 셀)를 차지하는 도구 호출 묶음 —
/// codex `ExecCell`.
#[derive(Debug, Default)]
pub struct ToolGroup {
    calls: Vec<ToolCall>,
    /// Diff syntax parsing is expensive and this cell is drawn every frame.
    /// A completed edit's diff is immutable, so only a width change invalidates
    /// its rendered lines; mutations clear this cache below.
    diff_cache: RefCell<Option<(usize, Vec<Line>)>>,
}

impl ToolGroup {
    #[must_use]
    pub fn new(call: ToolCall) -> Self {
        Self { calls: vec![call], diff_cache: RefCell::new(None) }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// Stable call ids represented by this cell.
    pub(super) fn call_ids(&self) -> impl Iterator<Item = &str> {
        self.calls.iter().map(|call| call.id.as_str())
    }

    /// Whether this cell still owns `id`, completed or running.
    #[must_use]
    pub(super) fn contains_call(&self, id: &str) -> bool {
        self.calls.iter().any(|call| call.id == id)
    }

    /// 아직 도는 호출이 있는가 — codex `ExecCell::is_active`.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.calls.iter().any(|call| call.done.is_none())
    }

    /// 전부 탐색 호출인가 — codex `ExecCell::is_exploring_cell`.
    #[must_use]
    pub fn is_exploring(&self) -> bool {
        !self.calls.is_empty() && self.calls.iter().all(|call| call.kind.is_explore())
    }

    /// 전부 edit/write 호출인가 — multi-file patch history cell 후보.
    #[must_use]
    pub fn is_editing(&self) -> bool {
        !self.calls.is_empty() && self.calls.iter().all(|call| call.kind.is_edit())
    }

    /// 전부 스폰 호출인가 — 한 메시지가 띄운 헬퍼들이 한 칸을 나눠 쓴다.
    #[must_use]
    pub fn is_spawning(&self) -> bool {
        !self.calls.is_empty() && self.calls.iter().all(|call| call.kind.is_spawn())
    }

    /// 가장 오래 도는 호출의 시작 시각 — 활동 표시가 이걸 훑는다.
    fn active_started(&self) -> Option<Instant> {
        self.calls
            .iter()
            .find(|call| call.done.is_none())
            .map(|call| call.started)
    }

    /// 이 종류가 이 셀에 붙을 수 있는가 — codex `ExecCell::add_call` 의 판정.
    /// 탐색은 탐색 뒤에 이어지고(`continues_exploration`), 그 밖의 명령은
    /// `continues_compact_group` 을 통과해야 이어진다. 실패한 호출이 있거나
    /// 상한을 넘겼으면 도는 중이 아닐 때 닫는다.
    ///
    /// # 원본과 다른 한 곳 — 아직 안 끝난 앞 호출
    ///
    /// codex 의 `continues_compact_group` 은 앞의 호출이 **전부 성공으로
    /// 끝났을 것**을 요구한다(`duration.is_some() && exit_code == 0`). codex 는
    /// exec 이 한 번에 하나만 돌기 때문에 그래도 된다 — `exec_begin` 다음은
    /// 언제나 그 호출의 `exec_end` 다.
    ///
    /// zo 의 런타임은 다르다. 한 assistant 메시지가 낸 도구 호출을 **먼저 전부
    /// announce** 하고 그 뒤에 결과를 하나씩 준다. 원본 규칙을 그대로 쓰면 두
    /// 번째 announce 가 아직 도는 첫 셀을 실패로 닫아 버린다 — PTY 실측에서
    /// `• Ran ls -la` + `└ (no output)` 가 먼저 떨어지고 뒤늦게 온 결과가
    /// `• Called bash(ls -la)` 라는 고아 셀로 또 나왔다. 그래서 여기서는
    /// "끝났고 성공" 대신 **"실패하지 않았다"** 로 읽는다. 접히는 개수는
    /// 그대로 앞에서부터 성공한 것만 세므로(`Self::compact_group_lines`)
    /// 커밋된 모양은 codex 캡처의 `• Ran 2 commands` 와 같다.
    #[must_use]
    pub fn accepts(&self, kind: &ToolKind) -> bool {
        if self.calls.is_empty() {
            return false;
        }
        if kind.is_edit() || self.is_editing() {
            return kind.is_edit()
                && self.is_editing()
                && self.calls.len() < MAX_GROUPED_CALLS
                && !self.calls.iter().any(ToolCall::failed);
        }
        // A parallel delegation announces its helpers back to back. They share
        // one cell so all of their live rows stay on screen at once — the
        // viewport has a single active slot, and a second cell would evict the
        // first (and close its still-running helper as failed).
        if kind.is_spawn() || self.is_spawning() {
            return kind.is_spawn() && self.is_spawning() && self.calls.len() < MAX_GROUPED_CALLS;
        }
        if !kind.groupable() {
            return false;
        }
        if !self.calls.iter().all(|call| call.kind.groupable()) {
            return false;
        }
        let active = self.is_active();
        if self.calls.len() >= MAX_GROUPED_CALLS && !active {
            return false;
        }
        if self.calls.iter().any(ToolCall::failed) && !active {
            return false;
        }
        let continues_exploration = kind.is_explore()
            && (self.is_exploring()
                || self
                    .calls
                    .last()
                    .is_some_and(|call| call.done.is_none() && call.kind.is_explore()));
        let continues_compact_group = !self.calls.iter().any(ToolCall::failed);
        continues_exploration || continues_compact_group
    }

    /// Install the newest live rows, keyed by the `tool_use` id each helper
    /// stamped on its own manifest. A call whose helper is not in `rows` loses
    /// its row rather than keeping a stale one.
    ///
    /// Returns whether anything changed, so a caller can skip a repaint.
    pub(super) fn set_spawn_progress(&mut self, rows: &[(String, String)]) -> bool {
        let mut changed = false;
        for call in &mut self.calls {
            if call.done.is_some() {
                continue;
            }
            let ToolKind::Spawn { progress, .. } = &mut call.kind else {
                continue;
            };
            let next: Vec<String> = rows
                .iter()
                .filter(|(id, _)| *id == call.id)
                .map(|(_, row)| row.clone())
                .collect();
            // Compare before allocating: an unchanged helper is the ordinary
            // case, once a second, for as long as it runs.
            if *progress != next {
                *progress = next;
                changed = true;
            }
        }
        changed
    }

    pub fn push(&mut self, call: ToolCall) {
        self.calls.push(call);
        self.diff_cache.get_mut().take();
    }

    /// 결과를 붙인다. 그 id 의 호출이 없으면 거짓 — codex 도
    /// `complete_call` 의 거짓을 "라우팅 불일치" 신호로 쓴다.
    pub fn complete(&mut self, id: &str, outcome: Outcome) -> bool {
        self.complete_as(id, outcome, None)
    }

    /// Remove a completed transient call after its transcript entry is stored.
    pub fn discard(&mut self, id: &str) -> bool {
        let Some(index) = self.calls.iter().position(|call| call.id == id) else {
            return false;
        };
        self.calls.remove(index);
        self.diff_cache.get_mut().take();
        true
    }

    /// Complete a call and replace its preview kind with a result-specific cell.
    pub fn complete_with_kind(&mut self, id: &str, outcome: Outcome, kind: ToolKind) -> bool {
        self.complete_as(id, outcome, Some(kind))
    }

    fn complete_as(&mut self, id: &str, outcome: Outcome, kind: Option<ToolKind>) -> bool {
        let Some(call) = self.calls.iter_mut().rev().find(|call| call.id == id) else {
            return false;
        };
        if let Some(kind) = kind {
            call.kind = kind;
        }
        call.done = Some(outcome);
        self.diff_cache.get_mut().take();
        true
    }

    /// 아직 안 끝난 호출을 실패로 닫는다(턴 취소·패닉) — codex `mark_failed`.
    ///
    /// The closed call carries its reason as its output, so its ✘ row says
    /// why ([`super::strings::TOOL_CALL_INTERRUPTED`]) instead of standing as
    /// a bare failure the tool never reported. This is the ONLY road to a
    /// failed call without an error result, and a running cell is never
    /// retired by another announce (the app's live-slot rule), so a
    /// result with `is_error: false` is never drawn as a failure (t-3063).
    pub fn mark_failed(&mut self) {
        for call in &mut self.calls {
            if call.done.is_none() {
                call.done = Some(Outcome {
                    declined: false,
                    ok: false,
                    output: super::strings::TOOL_CALL_INTERRUPTED.to_string(),
                    change: None,
                });
            }
        }
        self.diff_cache.get_mut().take();
    }

    /// 이 셀을 지금 히스토리로 넘겨야 하는가 — codex `ExecCell::should_flush`.
    /// 전부 성공으로 끝난 묶음은 **넘기지 않는다**. 다음 명령이 와서
    /// `Ran N commands` 로 접힐 수 있게 열어 두는 것이 그 규칙의 목적이다.
    #[must_use]
    pub fn should_flush(&self) -> bool {
        if self.calls.is_empty() {
            return false;
        }
        let active = self.is_active();
        if self.calls.iter().any(ToolCall::failed) {
            return !active;
        }
        if self.calls.len() >= MAX_GROUPED_CALLS {
            return !active;
        }
        if self.calls.iter().all(ToolCall::succeeded) {
            return false;
        }
        !self.is_exploring() && !active
    }

    /// 이 셀의 줄들. 뷰포트의 활성 칸과 히스토리 셀이 같은 함수를 쓴다 —
    /// 도는 중이면 활동 표시와 `-ing` 낱말이, 끝났으면 결과 불릿과 `-ed`
    /// 낱말이 나온다(codex `HistoryCell for ExecCell::display_lines`).
    #[must_use]
    pub fn lines(&self, width: usize, elapsed: Duration) -> Vec<Line> {
        self.lines_in(width, elapsed, FoldMode::Bare)
    }

    /// 같은 줄들을, 접기 모드를 알고. 뷰포트의 활성 칸은 언제나
    /// [`FoldMode::Bare`] 다 — 매 프레임 다시 그리는 자리에는 마커가 없고,
    /// 본문을 달면 칸만 높아진다. 접기 본문은 커밋될 때만 붙는다
    /// ([`Self::committed`]).
    fn lines_in(&self, width: usize, elapsed: Duration, mode: FoldMode) -> Vec<Line> {
        if self.calls.is_empty() {
            return Vec::new();
        }
        if self.is_editing() && self.calls.len() > 1 {
            return self.edit_group_lines(width, elapsed);
        }
        if self.is_spawning() {
            return self
                .calls
                .iter()
                .flat_map(|call| self.call_lines(call, width, elapsed))
                .collect();
        }
        if self.calls.len() > 1 && (!self.is_exploring() || !self.is_active()) {
            return self.compact_group_lines(width, elapsed, mode);
        }
        if self.is_exploring() {
            return Self::exploring_lines(&self.calls, width, elapsed);
        }
        self.call_lines(&self.calls[0], width, elapsed)
    }

    /// `• Ran N commands` — codex `compact_group_display_lines`. N 은 앞에서부터
    /// 성공으로 끝난 호출의 개수이고, 그 뒤 호출들은 낱개로 이어 그린다.
    ///
    /// [`FoldMode::Markers`] 에서는 그 헤더 아래에 접힐 본문을 단다
    /// ([`Self::grouped_body`]) — 원본은 접힌 명령들의 출력을 히스토리에 아예
    /// 남기지 않지만, marker opt-in에서는 손잡이로 그것을 펼칠 수 있다.
    fn compact_group_lines(&self, width: usize, elapsed: Duration, mode: FoldMode) -> Vec<Line> {
        let completed = self
            .calls
            .iter()
            .take_while(|call| call.succeeded())
            .count();
        let mut out = Vec::new();
        if completed > 0 {
            let noun = if completed == 1 { "command" } else { "commands" };
            out.push(
                Line::new(vec![
                    Span::new("•", Style::new().fg(palette::TOOL_OK).bold()),
                    Span::raw(" "),
                    Span::bold(format!("Ran {completed} {noun}")),
                ])
                .truncated(width),
            );
            if mode.markers_enabled() {
                out.extend(Self::grouped_body(&self.calls[..completed], width));
            }
        }
        for call in &self.calls[completed..] {
            if call.kind.is_explore() {
                out.extend(Self::exploring_lines(std::slice::from_ref(call), width, elapsed));
            } else {
                out.extend(self.call_lines(call, width, elapsed));
            }
        }
        out
    }

    /// 한 호출의 줄들 — 종류마다 다른 문법.
    fn call_lines(&self, call: &ToolCall, width: usize, elapsed: Duration) -> Vec<Line> {
        match &call.kind {
            ToolKind::Command {
                command,
                background,
            } => self.command_lines(call, command, *background, width, elapsed),
            ToolKind::Explore(_) => Self::exploring_lines(std::slice::from_ref(call), width, elapsed),
            ToolKind::Edit { path } => self.edit_lines(call, path, width, elapsed),
            ToolKind::Plan(items) => super::cells::plan_update_cell(items, width),
            ToolKind::WebSearch { detail } => {
                self.web_search_lines(call, detail, width, elapsed)
            }
            ToolKind::Spawn {
                label,
                role,
                request,
                prompt,
                progress,
            } => self.spawn_lines(
                call,
                &SpawnCell {
                    label,
                    role: role.as_deref(),
                    request: request.as_deref(),
                    prompt,
                    progress,
                },
                width,
                elapsed,
            ),
            ToolKind::AgentResult { label, summary } => {
                Self::agent_result_lines(call, label, summary.as_deref(), width)
            }
            ToolKind::Call { name, detail } => {
                self.mcp_lines(call, name, detail, width, elapsed)
            }
        }
    }

    /// `• Ran <cmd>` + `  │ ` 이어지는 명령 줄 + `  └ ` 출력 —
    /// codex `command_call_display_lines`, including bash syntax highlighting.
    ///
    /// A background command's cell is this same cell with one dim word
    /// ([`super::strings::COMMAND_BACKGROUND`]) after the command on the
    /// header row; the word's width is taken off the command's first-row
    /// budget so it is never the part the truncation drops.
    fn command_lines(
        &self,
        call: &ToolCall,
        command: &str,
        background: bool,
        width: usize,
        elapsed: Duration,
    ) -> Vec<Line> {
        let verb = if call.done.is_none() { "Running" } else { "Ran" };
        let mut header = vec![
            self.bullet(call, elapsed),
            Span::raw(" "),
            Span::bold(verb),
            Span::raw(" "),
        ];
        let marker =
            background.then(|| Span::dim(format!(" {}", super::strings::COMMAND_BACKGROUND)));
        let head_width = header
            .iter()
            .chain(marker.iter())
            .map(Span::width)
            .sum::<usize>();
        let first_budget = width.saturating_sub(head_width).max(1);
        let mut highlighted = super::highlight::highlight_block_to_lines(command, "bash")
            .unwrap_or_else(|| command.lines().map(Line::from_text).collect());
        if highlighted.is_empty() {
            highlighted.push(Line::empty());
        }
        let mut source = highlighted.into_iter();
        let first_line = source.next().unwrap_or_else(Line::empty);
        let mut rows = wrap_line(&first_line, first_budget, &Span::raw("")).into_iter();
        if let Some(first) = rows.next() {
            header.extend(first.spans);
        }
        header.extend(marker);
        let mut out = vec![Line::new(header).truncated(width)];

        let mut continuation: Vec<Line> = rows.collect();
        let continuation_width = width.saturating_sub(4).max(1);
        for line in source {
            continuation.extend(wrap_line(
                &line,
                continuation_width,
                &Span::raw(""),
            ));
        }
        if !continuation.is_empty() {
            let kept = limit_from_start(&continuation, COMMAND_CONTINUATION_MAX_ROWS);
            out.extend(prefixed(
                &kept,
                width,
                &Prefix::new(Span::dim("  │ "), Span::dim("  │ ")),
                true,
            ));
        }
        if let Some(done) = call.done.as_ref() {
            out.extend(output_block(&done.output, width));
        }
        out
    }

    /// `• Exploring` / `• Explored` + `  └ Read a, b` — codex
    /// `exploring_display_lines`. 연달아 오는 Read 항목은 한 행에 쉼표로
    /// 모으고 같은 이름은 접는다(원본의 `.unique()`).
    fn exploring_lines(calls: &[ToolCall], width: usize, elapsed: Duration) -> Vec<Line> {
        let active = calls.iter().any(|call| call.done.is_none());
        let marker = if active {
            tool_activity_marker(elapsed)
        } else {
            Span::dim("•")
        };
        let word = if active { "Exploring" } else { "Explored" };
        let mut out = vec![Line::new(vec![marker, Span::raw(" "), Span::bold(word)]).truncated(width)];

        let body = Self::explored_items(calls);
        out.extend(prefixed(&body, width, &Prefix::tool_output(), true));
        out
    }

    /// `Read a, b` · `List <glob>` · `Search <q> in <path>` — 탐색 목록의
    /// 알맹이(접두어 없이). `Exploring`/`Explored` 셀과 접힌 묶음의 본문이
    /// 같은 문법을 쓰도록 여기 한 군데에 둔다.
    fn explored_items(calls: &[ToolCall]) -> Vec<Line> {
        let items: Vec<&Explored> = calls
            .iter()
            .flat_map(|call| match &call.kind {
                ToolKind::Explore(items) => items.as_slice(),
                _ => &[],
            })
            .collect();

        let mut body: Vec<Line> = Vec::new();
        let mut index = 0usize;
        while index < items.len() {
            if let Explored::Read { .. } = items[index] {
                let mut names: Vec<&str> = Vec::new();
                while let Some(Explored::Read { name }) = items.get(index) {
                    if !names.contains(&name.as_str()) {
                        names.push(name);
                    }
                    index += 1;
                }
                let mut spans = vec![
                    Span::new("Read", Style::new().fg(palette::TOOL_LABEL)),
                    Span::raw(" "),
                ];
                for (position, name) in names.iter().enumerate() {
                    if position > 0 {
                        spans.push(Span::dim(", "));
                    }
                    spans.push(Span::raw((*name).to_string()));
                }
                body.push(Line::new(spans));
                continue;
            }
            let (label, mut spans) = match &items[index] {
                Explored::List { path } => ("List", vec![Span::raw(path.clone())]),
                Explored::Search { query, path } => (
                    "Search",
                    match path {
                        Some(path) => vec![
                            Span::raw(query.clone()),
                            Span::dim(" in "),
                            Span::raw(path.clone()),
                        ],
                        None => vec![Span::raw(query.clone())],
                    },
                ),
                Explored::Read { .. } => unreachable!("read runs are consumed above"),
            };
            let mut line = vec![
                Span::new(label, Style::new().fg(palette::TOOL_LABEL)),
                Span::raw(" "),
            ];
            line.append(&mut spans);
            body.push(Line::new(line));
            index += 1;
        }
        body
    }

    /// 접힌 `• Ran N commands` 아래로 숨는 본문 — 호출마다 `  └ <cmd>` 한 줄과
    /// 그 아래 출력 미리보기(거터 `    `). 미리보기 규칙은 단일 명령 셀과 똑같다
    /// ([`OUTPUT_MAX_ROWS`]·[`trim_logical`]) — 마커를 모르는 터미널에 이 바이트가
    /// 흘러도 codex 한 셀의 분량을 넘지 않아야 한다는 계약 때문이다.
    ///
    /// 묶음에 드는 종류는 명령과 탐색뿐이다([`ToolKind::groupable`]). 탐색은
    /// 원본이 출력을 아예 안 보이므로 목록 줄만 낸다.
    fn grouped_body(calls: &[ToolCall], width: usize) -> Vec<Line> {
        let mut out = Vec::new();
        for call in calls {
            match &call.kind {
                ToolKind::Command { command, .. } => {
                    out.extend(prefixed(
                        &[Line::from_text(command.clone())],
                        width,
                        &Prefix::tool_output(),
                        true,
                    ));
                    if let Some(done) = call.done.as_ref() {
                        out.extend(output_rows(&done.output, width, &Prefix::gutter()));
                    }
                }
                ToolKind::Explore(_) => {
                    let items = Self::explored_items(std::slice::from_ref(call));
                    out.extend(prefixed(&items, width, &Prefix::tool_output(), true));
                }
                // 패치 셀과 MCP 셀은 묶이지 않는다 — `accepts` 가 막는다.
                ToolKind::Edit { .. }
                | ToolKind::Plan(_)
                | ToolKind::WebSearch { .. }
                | ToolKind::Spawn { .. }
                | ToolKind::AgentResult { .. }
                | ToolKind::Call { .. } => {}
            }
        }
        out
    }

    /// `• Edited <path> (+A -B)` — codex `diff_render.rs::render_changes_block`
    /// 의 단일 파일 헤더. 도는 중에는 아직 diff 가 없으므로 `• Editing <path>`.
    fn edit_lines(
        &self,
        call: &ToolCall,
        path: &str,
        width: usize,
        elapsed: Duration,
    ) -> Vec<Line> {
        // codex draws a finished spawn with a dim marker
        // (`multi_agents.rs`); only the live one shimmers.
        let mut header = match call.done {
            None => vec![self.bullet(call, elapsed), Span::raw(" ")],
            Some(_) => vec![Span::dim("• ")],
        };
        match call.done.as_ref() {
            None => {
                header.push(Span::bold("Editing"));
                header.push(Span::raw(" "));
                header.push(Span::raw(path.to_string()));
                vec![Line::new(header).truncated(width)]
            }
            Some(done) if !done.ok => {
                let mut out = vec![patch_failure_line(width)];
                if !done.output.is_empty() {
                    out.extend(output_block(&done.output, width));
                }
                out
            }
            Some(done) => {
                let change = done.change.as_ref();
                header.push(Span::bold(change.map_or("Edited", |change| change.verb)));
                header.push(Span::raw(" "));
                header.push(Span::raw(
                    change.map_or_else(|| path.to_string(), Change::display_path),
                ));
                if let Some(change) = change {
                    header.push(Span::raw(" ("));
                    header.push(Span::new(
                        format!("+{}", change.added),
                        Style::new().fg(Color::GREEN),
                    ));
                    header.push(Span::raw(" "));
                    header.push(Span::new(
                        format!("-{}", change.removed),
                        Style::new().fg(Color::RED),
                    ));
                    header.push(Span::raw(")"));
                }
                let mut out = vec![Line::new(header).truncated(width)];
                if let Some(change) = change {
                    out.extend(change_body(change, self.cached_diff_lines(&change.view, width)));
                }
                out
            }
        }
    }

    fn edit_group_lines(&self, width: usize, elapsed: Duration) -> Vec<Line> {
        if self.is_active() {
            let mut paths: Vec<String> = self
                .calls
                .iter()
                .filter_map(|call| match &call.kind {
                    ToolKind::Edit { path } => Some(path.clone()),
                    _ => None,
                })
                .collect();
            paths.sort();
            let noun = if paths.len() == 1 { "file" } else { "files" };
            let mut out = vec![Line::new(vec![
                tool_activity_marker(elapsed),
                Span::raw(" "),
                Span::bold("Editing"),
                Span::raw(format!(" {} {noun}", paths.len())),
            ])
            .truncated(width)];
            let body: Vec<Line> = paths.into_iter().map(Line::from_text).collect();
            out.extend(prefixed(&body, width, &Prefix::tool_output(), true));
            return out;
        }

        let mut entries: Vec<EditGroupEntry<'_>> = self
            .calls
            .iter()
            .filter_map(|call| {
                let ToolKind::Edit { path } = &call.kind else {
                    return None;
                };
                let done = call.done.as_ref();
                let change = done.and_then(|done| done.change.as_ref());
                let failure = done
                    .filter(|done| !done.ok)
                    .map(|done| failure_reason(done.output.lines().next().unwrap_or_default(), path));
                Some(EditGroupEntry {
                    path: change.map_or_else(|| path.clone(), Change::display_path),
                    change,
                    failure,
                })
            })
            .collect();
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        // The header counts the files that were edited; a failed one is not
        // among them and is named below with its reason ("Failed to apply
        // patch" without a reason, and `(+0 -0)` for a failure, 2026-09-06).
        let edited = entries.iter().filter(|entry| entry.failure.is_none()).count();
        let added = entries
            .iter()
            .filter_map(|entry| entry.change)
            .map(|change| change.added)
            .sum::<usize>();
        let removed = entries
            .iter()
            .filter_map(|entry| entry.change)
            .map(|change| change.removed)
            .sum::<usize>();
        let mut out = Vec::new();
        if edited == 0 {
            out.push(patch_failure_line(width));
        } else {
            let noun = if edited == 1 { "file" } else { "files" };
            let mut header = vec![
                Span::dim("• "),
                Span::bold("Edited"),
                Span::raw(format!(" {edited} {noun} ")),
            ];
            header.extend(change_count_spans(added, removed));
            out.push(Line::new(header).truncated(width));
        }

        for (index, entry) in entries.iter().enumerate() {
            if let Some(reason) = &entry.failure {
                let mut row = vec![
                    Span::new("  └ ✘ ", Style::new().fg(Color::MAGENTA).bold()),
                    Span::raw(entry.path.clone()),
                ];
                if !reason.is_empty() {
                    row.push(Span::dim(format!(" — {reason}")));
                }
                out.push(Line::new(row).truncated(width));
            } else {
                let (added, removed) = entry
                    .change
                    .map_or((0, 0), |change| (change.added, change.removed));
                let mut row = vec![Span::dim("  └ "), Span::raw(entry.path.clone()), Span::raw(" ")];
                row.extend(change_count_spans(added, removed));
                out.push(Line::new(row).truncated(width));
                if let Some(change) = entry.change {
                    let diff = change_body(
                        change,
                        super::diff::hunk_lines(&change.view, width.saturating_sub(4).max(1)),
                    );
                    out.extend(prefixed(&diff, width, &Prefix::gutter(), true));
                }
            }
            if index + 1 < entries.len() {
                out.push(Line::empty());
            }
        }
        out
    }

    fn cached_diff_lines(&self, view: &DiffView, width: usize) -> Vec<Line> {
        {
            let cache = self.diff_cache.borrow();
            if let Some((cached_width, lines)) = cache.as_ref() {
                if *cached_width == width {
                    return lines.clone();
                }
            }
        }
        let lines = super::diff::hunk_lines(view, width);
        *self.diff_cache.borrow_mut() = Some((width, lines.clone()));
        lines
    }

    /// `• Calling <name>(<args>)` / `• Called …` — codex
    /// `history_cell/mcp.rs::render_lines` + `format_mcp_invocation`
    /// (이름은 시안, 인자는 dim, 결과는 `  └ ` 아래).
    fn mcp_lines(
        &self,
        call: &ToolCall,
        name: &str,
        detail: &str,
        width: usize,
        elapsed: Duration,
    ) -> Vec<Line> {
        if call.done.is_none() {
            let mut live = vec![
                self.bullet(call, elapsed),
                Span::raw(" "),
                Span::bold("Calling"),
                Span::raw(" "),
                Span::new(name.to_string(), Style::new().fg(palette::TOOL_LABEL)),
            ];
            if !detail.is_empty() {
                live.push(Span::raw(" "));
                live.push(Span::dim(detail.to_string()));
            }
            return vec![Line::new(live).truncated(width)];
        }

        let mut header = vec![
            self.bullet(call, elapsed),
            Span::raw(" "),
            Span::bold("Called"),
            Span::raw(" "),
        ];
        let mut invocation = vec![Span::new(
            name.to_string(),
            Style::new().fg(palette::TOOL_LABEL),
        )];
        if !detail.is_empty() {
            invocation.push(Span::raw("("));
            invocation.push(Span::dim(detail.to_string()));
            invocation.push(Span::raw(")"));
        }
        let invocation = Line::new(invocation);
        let reserved = Line::new(header.clone()).width();
        let inline_invocation = invocation.width() <= width.saturating_sub(reserved);

        let mut out = if inline_invocation {
            header.extend(invocation.spans);
            vec![Line::new(header).truncated(width)]
        } else {
            header.pop();
            let mut lines = vec![Line::new(header).truncated(width)];
            let wrapped = wrap_line(
                &invocation,
                width.saturating_sub(4).max(1),
                &Span::raw("    "),
            );
            lines.extend(prefixed(
                &wrapped,
                width,
                &Prefix::tool_output(),
                true,
            ));
            lines
        };

        if let Some(done) = call.done.as_ref().filter(|done| !done.output.is_empty()) {
            let prefix = if inline_invocation {
                Prefix::tool_output()
            } else {
                Prefix::gutter()
            };
            out.extend(output_rows(&done.output, width, &prefix));
        }
        out
    }

    /// `Searching the web <detail>` / `Searched the web for <detail>` — codex
    /// `history_cell/search.rs::WebSearchCell::display_lines`.
    fn web_search_lines(
        &self,
        call: &ToolCall,
        detail: &str,
        width: usize,
        elapsed: Duration,
    ) -> Vec<Line> {
        let completed = call.done.is_some();
        let marker = if completed {
            Span::dim("•")
        } else {
            tool_activity_marker(elapsed.max(self.age(call)))
        };
        let mut spans = vec![
            marker,
            Span::raw(" "),
            Span::bold(if completed {
                "Searched the web"
            } else {
                "Searching the web"
            }),
        ];
        if !detail.is_empty() {
            spans.push(Span::raw(if completed { " for " } else { " " }));
            spans.push(Span::raw(detail.to_string()));
        }
        wrap_line(&Line::new(spans), width.max(1), &Span::raw("  "))
    }

    /// Collab spawn — codex `multi_agents.rs::spawn_end`, plus Claude Code's
    /// live task row while the helper is still working.
    ///
    /// The `Agent` tool is synchronous, so this cell is what the person looks
    /// at for as long as the helper runs. An empty cell for those minutes is
    /// exactly the "is anything happening?" question the row answers.
    fn spawn_lines(
        &self,
        call: &ToolCall,
        spawn: &SpawnCell<'_>,
        width: usize,
        elapsed: Duration,
    ) -> Vec<Line> {
        let mut header = vec![self.bullet(call, elapsed), Span::raw(" ")];
        match call.done.as_ref() {
            // A policy verdict is not a failure: the dispatch kept the work
            // inline and said so (`ToolResultBody::Declined`), and a person
            // reading "failed" here looked for a fault that was not there
            // (2026-09-15, "이것도 오류인거같은데").
            Some(done) if done.declined => {
                header.push(Span::bold("Spawn declined — kept inline"));
            }
            Some(done) if !done.ok => header.push(Span::bold("Agent spawn failed")),
            done => {
                header.push(Span::bold(if done.is_some() {
                    "Spawned "
                } else {
                    "Spawning "
                }));
                header.push(Span::new(
                    spawn.label.to_string(),
                    Style::new().fg(Color::CYAN).bold(),
                ));
                if let Some(role) = spawn.role {
                    header.push(Span::dim(" "));
                    header.push(Span::raw(format!("[{role}]")));
                }
                if let Some(request) = spawn.request {
                    header.push(Span::dim(" "));
                    header.push(Span::new(
                        format!("({request})"),
                        Style::new().fg(Color::MAGENTA),
                    ));
                }
            }
        }

        let mut out = wrap_line(&Line::new(header), width.max(1), &Span::raw("  "));
        // While it runs, the live row owns the `└` slot; the prompt is what
        // the finished cell keeps, and the two never both need saying.
        if call.done.is_none() {
            if !spawn.progress.is_empty() {
                let rows: Vec<Line> = spawn
                    .progress
                    .iter()
                    .rev()
                    .take(OUTPUT_MAX_ROWS)
                    .rev()
                    .map(|row| Line::from_text(row.clone()))
                    .collect();
                out.extend(prefixed(&rows, width, &Prefix::tool_output(), true));
            }
        } else if let Some(done) = call.done.as_ref().filter(|done| !done.ok) {
            out.extend(output_block(&done.output, width));
        } else if !spawn.prompt.is_empty() {
            out.extend(prefixed(
                &[Line::from_text(spawn.prompt.to_string())],
                width,
                &Prefix::tool_output(),
                true,
            ));
        }
        out
    }

    /// Re-injected sub-agent completion — codex
    /// `multi_agents.rs::sub_agent_activity_title_line` plus a report body.
    fn agent_result_lines(
        call: &ToolCall,
        label: &str,
        summary: Option<&str>,
        width: usize,
    ) -> Vec<Line> {
        let Some(done) = call.done.as_ref() else {
            return Vec::new();
        };
        let header = agent_result_header(label, done.ok, summary);
        let mut out = wrap_line(&header, width.max(1), &Span::raw("  "));
        if label == runtime::background_log::BACKGROUND_BASH {
            let (_, notice) = runtime::background_log::split_completion_notice(&done.output);
            if let Some(notice) = notice {
                // A capped log no longer has a command head. Its lifecycle
                // fact still belongs on screen, ahead of the retained tail.
                out.extend(output_block(&sanitize(notice, ""), width));
                return out;
            }
        }
        if !done.ok && !done.output.trim().is_empty() {
            // A successful background completion is a compact status card in
            // normal history. The complete task output remains in Ctrl+T;
            // failures keep their actual cause in the main scrollback.
            out.extend(agent_result_rows(&done.output, width));
        }
        out
    }

    /// 셀 머리의 점 — 도는 중이면 shimmer 가 훑고, 끝나면 성패 색이 된다
    /// (codex `activity_marker` / `"•".green().bold()` / `"•".red().bold()`).
    fn bullet(&self, call: &ToolCall, elapsed: Duration) -> Span {
        match call.done.as_ref() {
            None => tool_activity_marker(elapsed.max(self.age(call))),
            Some(done) if done.ok => Span::new("•", Style::new().fg(palette::TOOL_OK).bold()),
            Some(_) => Span::new("•", Style::new().fg(palette::TOOL_FAIL).bold()),
        }
    }

    /// 히스토리로 커밋할 줄들 — 앞 빈 줄 하나 + 셀, 그리고 본문이 있으면
    /// 접기 구간(`docs/fold-markers.md`, OSC 7788)으로 감싼다.
    ///
    /// 감싸는 것은 [`super::folds::wrap_cell`] 이고 규칙은 계약 그대로다:
    /// `begin(collapsed, summary=<헤더 문안>)` → 헤더 줄 → 본문 → `end`.
    /// 헤더 한 줄뿐인 셀(`• Ran 2 commands`)은 접을 것이 없으므로 마커도 없다.
    /// 앞 빈 줄은 구간 **밖**이다 — 접었을 때 셀 사이의 숨이 사라지면 안 된다.
    ///
    /// 도는 중인 셀은 뷰포트의 활성 칸이 그리므로([`Self::lines`]) 여기 오지
    /// 않는다. 커밋된 셀에는 활동 표시가 없어 `elapsed` 도 필요 없다.
    #[must_use]
    pub fn committed(&self, width: usize, ids: &mut FoldIds, mode: FoldMode) -> Vec<Line> {
        let mut cell = self.lines_in(width, Duration::ZERO, mode);
        if cell.is_empty() {
            return Vec::new();
        }
        let mut out = vec![Line::empty()];
        // With markers enabled the folded cell keeps codex's density: the header and one
        // `  └ … +N lines` teaser, N being the rows the fold hides. The
        // teaser is a row of the region drawn only while it is collapsed
        // (`teaser=1`), so opening the cell shows the body in its place.
        if mode.markers_enabled() && cell.len() >= 2 {
            let folded = cell.len() - 1;
            cell.insert(1, teaser_line(folded, width));
            out.extend(folds::wrap_cell_with_teaser(cell, 1, ids));
        } else {
            out.extend(cell);
        }
        out
    }

    /// 이 호출이 돈 시간 — 활동 표시의 위상에 쓰인다.
    fn age(&self, call: &ToolCall) -> Duration {
        self.active_started()
            .filter(|started| *started <= call.started)
            .map_or(Duration::ZERO, |started| started.elapsed())
    }
}

/// The one line a folded cell shows under its header: `  └ … +N lines`,
/// `N` being the body rows the fold hides. Same grammar as the in-body
/// elision marker, so a person reads one vocabulary whether the cell is
/// folded or its preview was capped.
fn teaser_line(folded_rows: usize, width: usize) -> Line {
    Line::new(vec![
        Prefix::tool_output().initial,
        Span::new(format!("… +{folded_rows} lines"), Style::new().dim()),
    ])
    .truncated(width)
}

/// `  └ ` 아래의 출력 미리보기. 비어 있으면 codex 처럼 `(no output)`.
fn output_block(output: &str, width: usize) -> Vec<Line> {
    output_rows(output, width, &Prefix::tool_output())
}

/// 같은 미리보기를, 접두어를 골라서. 접힌 묶음의 본문은 `  └ ` 를 명령 줄이
/// 이미 썼으므로 네 칸 거터([`Prefix::gutter`])로 이어 붙인다.
fn output_rows(output: &str, width: usize, prefix: &Prefix) -> Vec<Line> {
    preview_rows(
        output,
        width,
        prefix,
        OUTPUT_MAX_ROWS,
        Style::new().dim(),
        true,
    )
}

fn agent_result_rows(output: &str, width: usize) -> Vec<Line> {
    let output = strip_agent_result_harness(output);
    let logical: Vec<Line> = output
        .trim_end_matches('\n')
        .lines()
        .map(Line::from_text)
        .collect();
    if logical.is_empty() {
        return Vec::new();
    }

    // Completion reports are conclusions, logs, or command output: the newest
    // rows carry the verdict. Keep that tail instead of the ordinary tool
    // preview's symmetric head/tail split. The marker consumes one display row
    // so the card remains within the same fixed budget even after wrapping.
    let prefix = Prefix::tool_output();
    let rows = prefixed(&logical, width, &prefix, true);
    if rows.len() <= AGENT_RESULT_MAX_ROWS {
        return rows;
    }
    let keep = AGENT_RESULT_MAX_ROWS.saturating_sub(1);
    let omitted = rows.len().saturating_sub(keep);
    let mut out = vec![
        Line::new(vec![
            prefix.initial,
            Span::raw(format!("… +{omitted} lines")),
        ])
        .truncated(width),
    ];
    out.extend(rows.into_iter().skip(omitted));
    out
}

/// Drop host-only task-notification framing from an agent-result display.
///
/// Both recognized lines remain byte-for-byte in the conversation and wire:
/// the runtime's mid-turn host preamble and the completion producer's
/// `SendMessage` continuation header. They are instructions for the model,
/// not agent output for the human-facing card. Only leading, one-line bracket
/// blocks with those contracts are removed so an agent report that merely
/// mentions a task notification later in its body is left untouched.
#[must_use]
pub(crate) fn strip_agent_result_harness(mut output: &str) -> &str {
    loop {
        output = trim_leading_newlines(output);
        let (line, rest) = leading_line(output);
        if !is_agent_result_harness_line(line) {
            return output;
        }
        output = rest;
    }
}

/// `• Completed <label> (12 tool uses · 45.3k tokens · 1m 20s)`.
///
/// The history card and the unfolded transcript draw the same helper the same
/// way, so the header is built once. The parenthesis is Claude Code's
/// `Done (…)`; a run nothing was measured for leaves the line as it was.
#[must_use]
pub(super) fn agent_result_header(label: &str, ok: bool, summary: Option<&str>) -> Line {
    let (verb, color) = if ok {
        ("Completed", palette::TOOL_OK)
    } else {
        ("Failed", palette::TOOL_FAIL)
    };
    let mut spans = vec![
        Span::new("•", Style::new().fg(color).bold()),
        Span::raw(" "),
        Span::bold(format!("{verb} ")),
        Span::new(label.to_string(), Style::new().fg(Color::CYAN)),
    ];
    if let Some(summary) = summary.map(str::trim).filter(|summary| !summary.is_empty()) {
        spans.push(Span::dim(format!(" ({summary})")));
    }
    Line::new(spans)
}

/// Recover the display metadata from a persisted completion notification.
///
/// `ConversationMessage` intentionally stores only the exact model-facing
/// user-role text. Resume replay projects that known text contract back into
/// an `AgentResult` card without rewriting the persisted message.
#[must_use]
pub(crate) fn persisted_agent_result(
    output: &str,
) -> Option<(String, AgentResultStatus, Option<String>, String)> {
    let mut cursor = trim_leading_newlines(output);
    loop {
        let (line, rest) = leading_line(cursor);
        if is_runtime_agent_notification_preamble(line) {
            cursor = trim_leading_newlines(rest);
            continue;
        }
        break;
    }

    let (header, body) = leading_line(cursor);
    if !is_completion_notification_header(header) {
        return None;
    }
    let identity = header.strip_prefix("[task notification — background agent `")?;
    let (label, after_label) = identity.split_once("` (id: ")?;
    let (_, verdict) = after_label.split_once(") ")?;
    // The terminal word is followed by the optional run cost, so match the
    // word rather than `finished.` — a header written before the cost existed
    // must keep replaying as the completion it is.
    let status = if verdict.starts_with("finished") {
        AgentResultStatus::Completed
    } else {
        AgentResultStatus::Failed
    };
    Some((
        label.to_string(),
        status,
        run_cost_in_header(verdict),
        strip_agent_result_harness(body).to_string(),
    ))
}

/// The cell a finished background bash draws: the foreground command cell
/// for the same command, output and exit code, with one dim word in its
/// header (t-3177).
///
/// The completion is the task's log re-injected for the model — `$ command`
/// head, `[stderr]` tags, `[exit N]` closer — and that text stays as it is
/// in the transcript and on the wire. Only its drawing changes: the log is
/// taken apart ([`runtime::background_log::parse`]) into the shape a
/// foreground `bash` result has, and the ordinary [`Outcome::from_result`]
/// road turns that into the cell — the same exit-code rule, the same
/// preview budget, the same elision. The whole stream rides `stdout`: the
/// foreground formatter prints stdout then stderr, and the log's order is
/// the order the person's terminal would have shown.
///
/// `None` keeps the agent card: a completion that is not a background
/// task's, or a log that lost its `$ command` head (one over the registry's
/// cap keeps only its tail) — a header without the command would be a
/// task-id headline, which is the card's job, not this cell's.
#[must_use]
pub(crate) fn background_command_cell(
    label: &str,
    status: AgentResultStatus,
    body: &str,
) -> Option<(ToolKind, Outcome)> {
    if label != runtime::background_log::BACKGROUND_BASH {
        return None;
    }
    let (body, notice) = runtime::background_log::split_completion_notice(strip_agent_result_harness(body));
    let log = runtime::background_log::parse(body)?;
    // A log without a code (a signal, a watch error) still has the task's
    // verdict; the foreground failure cell shows no code word either.
    let exit_code = log.exit_code.unwrap_or(match status {
        AgentResultStatus::Completed => 0,
        AgentResultStatus::Failed => 1,
    });
    let result = BashResult {
        exit_code,
        stdout: log.output,
        stderr: String::new(),
        truncated: log.truncated,
    };
    let mut outcome = Outcome::from_result(false, &ToolResultBody::Bash(result), "");
    if let Some(notice) = notice {
        outcome.output = format!("{}\n{}", sanitize(notice, ""), outcome.output);
    }
    Some((
        ToolKind::Command {
            command: sanitize(&log.command, ""),
            background: true,
        },
        outcome,
    ))
}

/// The `(12 tool uses · 45.3k tokens · 1m 20s)` a stored header carries after
/// its terminal word, so a resumed card is not poorer than the live one.
/// `None` for a header written before the cost was recorded.
fn run_cost_in_header(verdict: &str) -> Option<String> {
    let open = verdict.find(" (")? + 2;
    let rest = verdict.get(open..)?;
    let close = rest.find(')')?;
    let cost = rest.get(..close)?.trim();
    (!cost.is_empty()).then(|| cost.to_string())
}

fn trim_leading_newlines(text: &str) -> &str {
    text.trim_start_matches(['\r', '\n'])
}

fn leading_line(text: &str) -> (&str, &str) {
    text.find('\n').map_or((text, ""), |end| {
        (text[..end].trim_end_matches('\r'), &text[end + 1..])
    })
}

fn is_agent_result_harness_line(line: &str) -> bool {
    is_runtime_agent_notification_preamble(line) || is_completion_notification_header(line)
        || runtime::background_log::is_notification_header(line)
}

fn is_runtime_agent_notification_preamble(line: &str) -> bool {
    is_task_notification_bracket(line)
        && line
            .to_ascii_lowercase()
            .contains("this is a host notification")
}

fn is_completion_notification_header(line: &str) -> bool {
    is_task_notification_bracket(line)
        && line
            .to_ascii_lowercase()
            .contains("use sendmessage with that id/name as `to`")
}

fn is_task_notification_bracket(line: &str) -> bool {
    let line = line.trim();
    let prefix = "[task notification";
    line.ends_with(']')
        && line
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

fn preview_rows(
    output: &str,
    width: usize,
    prefix: &Prefix,
    max_rows: usize,
    style: Style,
    show_empty: bool,
) -> Vec<Line> {
    let mut logical: Vec<&str> = output.trim_end_matches('\n').lines().collect();
    let source_omitted = usize::from(
        logical
            .last()
            .is_some_and(|line| *line == SOURCE_TRUNCATION_NOTE),
    );
    if source_omitted > 0 {
        logical.pop();
    }
    if logical.is_empty() && source_omitted == 0 {
        if !show_empty {
            return Vec::new();
        }
        return prefixed(
            &[Line::new(vec![Span::dim("(no output)")])],
            width,
            prefix,
            true,
        );
    }
    let body = trim_logical(&logical, max_rows, style, source_omitted);
    let rows = prefixed(&body, width, prefix, true);
    if rows.len() <= max_rows {
        return rows;
    }
    // 접힌 뒤에도 넘치는 것은 긴 줄이 여러 행으로 감싸졌을 때다 — 원본도
    // "truncation is applied to on-screen lines rather than logical lines"
    // 라며 행을 기준으로 자른다.
    rows.into_iter().take(max_rows).collect()
}

/// 논리 줄을 머리 절반 · 중간 생략 · 꼬리 절반으로 접는다 — codex
/// `truncate_lines_middle` 의 배분(`head = available / 2`,
/// `tail = available - head`) 과 "생략 수는 논리 줄로 센다" 는 계약 그대로.
/// `source_omitted` 는 상류의 bool이 보장하는 최소 한 줄이며, 같은 가운데
/// 생략 행에 합쳐 셀이 두 가지 truncation 문법을 말하지 않게 한다.
fn trim_logical(
    logical: &[&str],
    max_rows: usize,
    style: Style,
    source_omitted: usize,
) -> Vec<Line> {
    let styled = |text: &str| Line::new(vec![Span::new(text.to_string(), style)]);
    let marker_rows = usize::from(source_omitted > 0);
    if logical.len() + marker_rows <= max_rows {
        let mut out: Vec<Line> = logical.iter().map(|line| styled(line)).collect();
        if source_omitted > 0 {
            out.push(styled(&format!("… +{source_omitted} lines")));
        }
        return out;
    }
    let available = max_rows.saturating_sub(1);
    let head = available / 2;
    let tail = available - head;
    let omitted = logical.len() - head - tail + source_omitted;
    let mut out: Vec<Line> = logical[..head].iter().map(|line| styled(line)).collect();
    out.push(styled(&format!("… +{omitted} lines")));
    if tail > 0 {
        out.extend(
            logical[logical.len() - tail..]
                .iter()
                .map(|line| styled(line)),
        );
    }
    out
}

/// 앞에서 `keep` 줄만 남기고 나머지를 `… +N lines` 한 줄로 접는다 — codex
/// `ExecCell::limit_lines_from_start`.
fn limit_from_start(lines: &[Line], keep: usize) -> Vec<Line> {
    if lines.len() <= keep {
        return lines.to_vec();
    }
    let mut out: Vec<Line> = lines[..keep].to_vec();
    out.push(Line::new(vec![Span::dim(format!(
        "… +{} lines",
        lines.len() - keep
    ))]));
    out
}

// ============================================================================
// 결과 본문 → 미리보기 원문
// ============================================================================

/// 도구 결과 본문의 원문. 색·아이콘·잘라내기는 셀이 얹는다.
#[must_use]
pub fn body_text(body: &ToolResultBody) -> String {
    match body {
        ToolResultBody::Bash(result) => bash_body_text(result),
        ToolResultBody::Text { content, truncated }
        | ToolResultBody::Generic {
            content, truncated, ..
        }
        | ToolResultBody::Read {
            content, truncated, ..
        } => with_truncation_note(content.trim_end_matches('\n'), *truncated),
        ToolResultBody::Listing { entries, truncated } => {
            with_truncation_note(&entries.join("\n"), *truncated)
        }
        ToolResultBody::Diff(diff) => diff_body_text(diff),
        ToolResultBody::Todos(items) => todos_body_text(items),
        ToolResultBody::Declined { reason } => reason.trim_end_matches('\n').to_string(),
    }
}

fn bash_body_text(result: &BashResult) -> String {
    let mut parts = Vec::new();
    let stdout = result.stdout.trim_end_matches('\n');
    if !stdout.is_empty() {
        parts.push(stdout.to_string());
    }
    let stderr = result.stderr.trim_end_matches('\n');
    if !stderr.is_empty() {
        parts.push(stderr.to_string());
    }
    with_truncation_note(&parts.join("\n"), result.truncated)
}

fn diff_body_text(diff: &DiffView) -> String {
    let mut lines = Vec::new();
    for hunk in &diff.hunks {
        lines.push(format!(
            "@@ -{},{} +{},{} @@",
            hunk.old_start, hunk.old_lines, hunk.new_start, hunk.new_lines
        ));
        for line in &hunk.lines {
            let marker = match line.kind {
                DiffLineKind::Added => '+',
                DiffLineKind::Removed => '-',
                DiffLineKind::Context => ' ',
            };
            lines.push(format!("{marker}{}", line.text));
        }
    }
    lines.join("\n")
}

fn todos_body_text(items: &[runtime::message_stream::TodoResultItem]) -> String {
    use runtime::message_stream::TodoResultStatus;
    items
        .iter()
        .map(|item| match item.status {
            TodoResultStatus::Pending => format!("[ ] {}", item.content),
            TodoResultStatus::InProgress => format!("[~] {}", item.active_form),
            TodoResultStatus::Completed => format!("[x] {}", item.content),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn with_truncation_note(text: &str, truncated: bool) -> String {
    let text = text.trim_end_matches('\n');
    if !truncated {
        return text.to_string();
    }
    if text.is_empty() {
        SOURCE_TRUNCATION_NOTE.to_string()
    } else {
        format!("{text}\n{SOURCE_TRUNCATION_NOTE}")
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use runtime::message_stream::anthropic::tools::{
        format_tool_result_from_raw, preview_summary, preview_tool_input,
    };
    use runtime::message_stream::{BashResult, ToolResultBody};
    use serde_json::json;

    use super::{persisted_agent_result, Explored, Outcome, ToolCall, ToolGroup, ToolKind};
    use runtime::message_stream::AgentResultStatus;
    use crate::tui::ansi::Line;

    fn bash(id: &str, command: &str) -> ToolCall {
        ToolCall::new(
            id.to_string(),
            ToolKind::Command {
                command: command.to_string(),
                background: false,
            },
        )
    }

    fn read(id: &str, name: &str) -> ToolCall {
        ToolCall::new(
            id.to_string(),
            ToolKind::Explore(vec![Explored::Read {
                name: name.to_string(),
            }]),
        )
    }

    fn ok(output: &str) -> Outcome {
        Outcome {
            declined: false,
            ok: true,
            output: output.to_string(),
            change: None,
        }
    }

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::plain).collect()
    }

    #[test]
    fn a_running_command_reads_running_and_a_finished_one_reads_ran() {
        let mut group = ToolGroup::new(bash("a", "ls -la"));
        assert_eq!(plain(&group.lines(60, Duration::ZERO))[0], "• Running ls -la");
        assert!(group.complete("a", ok("total 16\nsrc")));
        let lines = group.lines(60, Duration::ZERO);
        assert_eq!(
            plain(&lines),
            vec![
                "• Ran ls -la".to_string(),
                "  └ total 16".to_string(),
                "    src".to_string(),
            ]
        );
    }

    #[test]
    fn an_empty_output_says_no_output_like_codex() {
        let mut group = ToolGroup::new(bash("a", "true"));
        assert!(group.complete("a", ok("")));
        assert_eq!(plain(&group.lines(60, Duration::ZERO))[1], "  └ (no output)");
    }

    /// codex 의 출력 상한은 다섯 행이고, 넘치면 머리 둘 · 생략 · 꼬리 둘이다.
    #[test]
    fn long_output_keeps_a_head_and_a_tail_around_the_ellipsis() {
        let output = (1..=20).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        let mut group = ToolGroup::new(bash("a", "seq 20"));
        assert!(group.complete("a", ok(&output)));
        let rows = plain(&group.lines(60, Duration::ZERO));
        assert_eq!(
            rows,
            vec![
                "• Ran seq 20".to_string(),
                "  └ 1".to_string(),
                "    2".to_string(),
                "    … +16 lines".to_string(),
                "    19".to_string(),
                "    20".to_string(),
            ]
        );
    }

    /// 캡처(`codex-tui-v0.149.1-tool-turn.bin`, 883번째 줄)의 라이브 셀.
    #[test]
    fn a_live_explore_group_is_the_captured_exploring_cell() {
        let group = ToolGroup::new(read("a", "README.md"));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec!["• Exploring".to_string(), "  └ Read README.md".to_string()]
        );
    }

    /// 연달아 오는 Read 는 한 행에 쉼표로 모이고 같은 이름은 접힌다
    /// (codex `exploring_display_lines` 의 `.unique()`).
    #[test]
    fn consecutive_reads_collapse_into_one_comma_joined_row() {
        let mut group = ToolGroup::new(read("a", "a.rs"));
        assert!(group.accepts(&ToolKind::Explore(vec![Explored::Read {
            name: "b.rs".to_string()
        }])));
        group.push(read("b", "b.rs"));
        group.push(read("c", "a.rs"));
        // 아직 도는 탐색 묶음이라 `Exploring` 목록이 그려진다.
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec!["• Exploring".to_string(), "  └ Read a.rs, b.rs".to_string()]
        );
    }

    /// 끝난 **여러** 호출은 탐색이라도 접힌다 — codex 의 갈림은
    /// `HistoryCell for ExecCell::display_lines` 한 줄이다:
    /// `if calls.len() > 1 && (!is_exploring_cell() || !is_active())`
    /// → `compact_group_display_lines`. 그래서 캡처의 "ls -la + README 읽기"
    /// 도 `• Ran 2 commands` 한 줄로 남는다.
    #[test]
    fn a_finished_multi_call_group_collapses_even_when_it_only_explored() {
        let mut group = ToolGroup::new(read("a", "a.rs"));
        group.push(read("b", "b.rs"));
        assert!(group.complete("a", ok("")));
        assert!(group.complete("b", ok("")));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec!["• Ran 2 commands".to_string()]
        );
    }

    /// 한 호출뿐인 탐색은 끝나도 `Explored` 목록으로 남는다.
    #[test]
    fn a_single_finished_explore_keeps_the_explored_list() {
        let mut group = ToolGroup::new(read("a", "README.md"));
        assert!(group.complete("a", ok("")));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec!["• Explored".to_string(), "  └ Read README.md".to_string()]
        );
    }

    /// 캡처(같은 파일, 887번째 줄)의 접힌 셀. 트랜스크립트 힌트는 이번
    /// 라운드에서 뺐다.
    #[test]
    fn two_finished_commands_collapse_into_ran_two_commands() {
        let mut group = ToolGroup::new(bash("a", "ls -la"));
        assert!(group.complete("a", ok("x")));
        assert!(group.accepts(&ToolKind::Command {
            command: "pwd".to_string(),
            background: false,
        }));
        group.push(bash("b", "pwd"));
        assert!(group.complete("b", ok("/tmp")));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec!["• Ran 2 commands".to_string()]
        );
    }

    /// zo 는 한 메시지의 도구 호출을 먼저 전부 announce 한다 — 아직 도는 앞
    /// 호출이 다음 announce 를 밀어내면 안 된다.
    #[test]
    fn a_batch_announce_lands_in_one_cell() {
        let mut group = ToolGroup::new(bash("a", "ls -la"));
        let read_kind = ToolKind::Explore(vec![Explored::Read {
            name: "README.md".to_string(),
        }]);
        assert!(group.accepts(&read_kind), "the second announce must join");
        group.push(read("b", "README.md"));
        // 결과는 순서대로 온다 — id 로 라우팅되므로 고아 셀이 없다.
        assert!(group.complete("a", ok("total 8")));
        assert!(group.complete("b", ok("hello")));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec!["• Ran 2 commands".to_string()]
        );
    }

    /// 실패한 호출이 있으면 다음 명령은 붙지 않는다 — codex `add_call`.
    #[test]
    fn a_failed_call_closes_the_group() {
        let mut group = ToolGroup::new(bash("a", "false"));
        assert!(group.complete(
            "a",
            Outcome {
                declined: false,
                ok: false,
                output: "boom".to_string(),
                change: None,
            }
        ));
        assert!(!group.accepts(&ToolKind::Command {
            command: "pwd".to_string(),
            background: false,
        }));
        assert!(group.should_flush());
    }

    /// 전부 성공한 묶음은 열려 있다 — 다음 명령이 와서 접힐 수 있게.
    #[test]
    fn a_fully_successful_group_stays_open() {
        let mut group = ToolGroup::new(bash("a", "ls"));
        assert!(group.complete("a", ok("x")));
        assert!(!group.should_flush());
    }

    #[test]
    fn edits_and_other_tools_never_join_a_group() {
        let group = ToolGroup::new(bash("a", "ls"));
        assert!(!group.accepts(&ToolKind::Edit {
            path: "a.rs".to_string()
        }));
        assert!(!group.accepts(&ToolKind::Call {
            name: "Fetch".to_string(),
            detail: "x".to_string(),
        }));
    }

    #[test]
    fn a_finished_edit_carries_the_diff_counts() {
        use runtime::message_stream::{DiffHunk, DiffLine, DiffLineKind, DiffView};
        let diff = DiffView {
            old_path: Some("a.rs".to_string()),
            new_path: Some("a.rs".to_string()),
            language: None,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 2,
                lines: vec![
                    DiffLine {
                        kind: DiffLineKind::Removed,
                        text: "old".to_string(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Added,
                        text: "new".to_string(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Added,
                        text: "more".to_string(),
                    },
                ],
            }],
        };
        let mut group = ToolGroup::new(ToolCall::new(
            "a".to_string(),
            ToolKind::Edit {
                path: "a.rs".to_string(),
            },
        ));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO))[0],
            "• Editing a.rs"
        );
        assert!(group.complete(
            "a",
            Outcome::from_result(false, &ToolResultBody::Diff(diff), "a.rs")
        ));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO))[0],
            "• Edited a.rs (+2 -1)"
        );
    }

    #[test]
    fn other_tools_use_the_mcp_call_grammar() {
        let mut group = ToolGroup::new(ToolCall::new(
            "a".to_string(),
            ToolKind::Call {
                name: "TodoWrite".to_string(),
                detail: "3 items".to_string(),
            },
        ));
        // A running call is a bounded one-liner: name, then the detail dimmed
        // and truncated to the width; the invocation grammar with parentheses
        // is for the committed line.
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO))[0],
            "• Calling TodoWrite 3 items"
        );
        assert!(group.complete(
            "a",
            Outcome::from_result(
                false,
                &ToolResultBody::Text {
                    content: "done".to_string(),
                    truncated: false,
                },
                "",
            )
        ));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec![
                "• Called TodoWrite(3 items)".to_string(),
                "  └ done".to_string()
            ]
        );
    }

    #[test]
    fn a_todowrite_preview_does_not_repeat_its_name_as_arguments() {
        let preview = preview_tool_input(
            "TodoWrite",
            &json!({
                "todos": [{
                    "content": "Render the plan",
                    "activeForm": "Rendering the plan",
                    "status": "pending"
                }]
            }),
        );
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("TodoWrite", &preview, &summary);
        let group = ToolGroup::new(ToolCall::new("a".to_string(), kind));

        assert_eq!(
            plain(&group.lines(60, Duration::ZERO))[0],
            "• Calling TodoWrite"
        );
    }

    #[test]
    fn a_capability_invoke_read_preview_renders_as_exploration() {
        let preview = preview_tool_input(
            "CapabilityInvoke",
            &json!({
                "name": "Read",
                "input": { "file_path": "src/lib.rs" }
            }),
        );
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("CapabilityInvoke", &preview, &summary);
        let group = ToolGroup::new(ToolCall::new("a".to_string(), kind));

        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec!["• Exploring".to_string(), "  └ Read src/lib.rs".to_string()]
        );
    }

    #[test]
    fn a_todowrite_result_renders_every_step_in_an_uncapped_plan_cell() {
        let input = json!({
            "todos": [
                { "content": "Step 1", "activeForm": "Doing 1", "status": "completed" },
                { "content": "Step 2", "activeForm": "Doing 2", "status": "completed" },
                { "content": "Step 3", "activeForm": "Doing 3", "status": "in_progress" },
                { "content": "Step 4", "activeForm": "Doing 4", "status": "pending" },
                { "content": "Step 5", "activeForm": "Doing 5", "status": "pending" },
                { "content": "Step 6", "activeForm": "Doing 6", "status": "pending" },
                { "content": "Step 7", "activeForm": "Doing 7", "status": "pending" }
            ]
        });
        let preview = preview_tool_input("TodoWrite", &input);
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("TodoWrite", &preview, &summary);
        let mut group = ToolGroup::new(ToolCall::new("a".to_string(), kind));
        let body = format_tool_result_from_raw(
            "TodoWrite",
            &json!({ "newTodos": input["todos"].clone() }).to_string(),
            false,
        );
        let result_kind = ToolKind::from_result(&body).expect("typed plan result");
        assert!(group.complete_with_kind(
            "a",
            Outcome::from_result(false, &body, ""),
            result_kind,
        ));

        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec![
                "• Updated Plan".to_string(),
                "  └ ✔ Step 1".to_string(),
                "    ✔ Step 2".to_string(),
                "    □ Step 3".to_string(),
                "    □ Step 4".to_string(),
                "    □ Step 5".to_string(),
                "    □ Step 6".to_string(),
                "    □ Step 7".to_string(),
            ]
        );
    }

    #[test]
    fn an_empty_mcp_result_does_not_add_a_no_output_row() {
        let preview = preview_tool_input("Sleep", &json!({}));
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("Sleep", &preview, &summary);
        let mut group = ToolGroup::new(ToolCall::new("a".to_string(), kind));
        let body = format_tool_result_from_raw("Sleep", "", false);
        assert!(group.complete("a", Outcome::from_result(false, &body, "")));

        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec!["• Called Sleep".to_string()]
        );
    }

    #[test]
    fn a_successful_mcp_result_uses_the_success_status_color() {
        let preview = preview_tool_input("Sleep", &json!({}));
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("Sleep", &preview, &summary);
        let mut group = ToolGroup::new(ToolCall::new("a".to_string(), kind));
        let body = format_tool_result_from_raw("Sleep", "", false);
        assert!(group.complete("a", Outcome::from_result(false, &body, "")));
        let lines = group.lines(60, Duration::ZERO);

        assert_eq!(lines[0].spans[0].style.fg, Some(super::palette::TOOL_OK));
    }

    #[test]
    fn a_long_mcp_invocation_moves_below_the_header_and_wraps() {
        let preview = preview_tool_input(
            "Lookup",
            &json!({ "query": "alpha beta gamma delta epsilon" }),
        );
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("Lookup", &preview, &summary);
        let mut group = ToolGroup::new(ToolCall::new("a".to_string(), kind));
        let body = format_tool_result_from_raw("Lookup", "ok", false);
        assert!(group.complete("a", Outcome::from_result(false, &body, "")));
        let rows = plain(&group.lines(24, Duration::ZERO));

        assert_eq!(rows[0], "• Called");
        assert!(rows[1].starts_with("  └ Lookup("), "rows were {rows:?}");
        assert!(rows.iter().all(|row| !row.contains('…')), "rows were {rows:?}");
        assert!(rows.iter().any(|row| row.contains("epsilon)")), "rows were {rows:?}");
        assert_eq!(rows.last().expect("output row"), "    ok");
    }

    #[test]
    fn web_search_uses_the_search_activity_grammar() {
        let preview = preview_tool_input("WebSearch", &json!({ "query": "rust async" }));
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("WebSearch", &preview, &summary);
        let mut group = ToolGroup::new(ToolCall::new("a".to_string(), kind));

        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec!["• Searching the web rust async".to_string()]
        );

        let body = format_tool_result_from_raw("WebSearch", "raw search result", false);
        assert!(group.complete("a", Outcome::from_result(false, &body, "")));
        let lines = group.lines(60, Duration::ZERO);
        assert_eq!(plain(&lines), vec!["• Searched the web for rust async".to_string()]);
        assert!(lines[0].spans[0].style.dim);
        assert_eq!(lines[0].spans[0].style.fg, None);
    }

    #[test]
    fn failed_spawn_shows_the_refusal_instead_of_the_delegated_prompt() {
        let preview = preview_tool_input("Agent", &json!({"description":"look up remote", "prompt":"delegated prompt"}));
        let kind = ToolKind::from_preview("Agent", &preview, &preview_summary(&preview));
        let mut group = ToolGroup::new(ToolCall::new("a".to_string(), kind));
        let reason = "Small delegated task: do it inline. Do not retry this spawn.";
        let body = ToolResultBody::Declined { reason: reason.to_owned() };
        group.complete("a", Outcome::from_result(true, &body, reason));
        let text = plain(&group.lines(100, Duration::ZERO)).join("\n");
        assert!(text.contains("Spawn declined — kept inline") && text.contains(reason), "{text}");
        assert!(!text.contains("Agent spawn failed"), "a verdict is not a failure:\n{text}");
        // A spawn that really failed still says so.
        let kind = ToolKind::from_preview("Agent", &preview, &preview_summary(&preview));
        let mut failed = ToolGroup::new(ToolCall::new("b".to_string(), kind));
        let body = format_tool_result_from_raw("Agent", "execution error: child died", true);
        failed.complete("b", Outcome::from_result(true, &body, "child died"));
        let text = plain(&failed.lines(100, Duration::ZERO)).join("\n");
        assert!(text.contains("Agent spawn failed"), "{text}");
    }

    #[test]
    fn an_agent_call_is_silent_until_it_becomes_a_spawned_cell() {
        let preview = preview_tool_input(
            "Agent",
            &json!({
                "name": "robie",
                "subagent_type": "worker",
                "description": "Investigate the flaky orchestration test",
                "model": "gpt-5.1",
                "effort": "high"
            }),
        );
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("Agent", &preview, &summary);
        let mut group = ToolGroup::new(ToolCall::new("a".to_string(), kind));

        // While the helper works, the cell is what the person watches — it
        // names the helper and, once the poller has a manifest, its live row.
        assert_eq!(
            plain(&group.lines(100, Duration::ZERO)),
            vec!["• Spawning robie [worker] (gpt-5.1 high)".to_string()]
        );
        assert!(group.set_spawn_progress(&[(
            "a".to_string(),
            "12 tool uses · 1m 20s · Read · src/main.rs".to_string(),
        )]));
        assert_eq!(
            plain(&group.lines(100, Duration::ZERO)),
            vec![
                "• Spawning robie [worker] (gpt-5.1 high)".to_string(),
                "  └ 12 tool uses · 1m 20s · Read · src/main.rs".to_string(),
            ]
        );
        // The same row again must not rewrite anything: the cell repaints
        // every frame and the poller fires every second.
        assert!(!group.set_spawn_progress(&[(
            "a".to_string(),
            "12 tool uses · 1m 20s · Read · src/main.rs".to_string(),
        )]));

        let body = format_tool_result_from_raw(
            "Agent",
            r#"{"status":"running","subagentType":"worker"}"#,
            false,
        );
        assert!(group.complete("a", Outcome::from_result(false, &body, "")));
        assert_eq!(
            plain(&group.lines(100, Duration::ZERO)),
            vec![
                "• Spawned robie [worker] (gpt-5.1 high)".to_string(),
                "  └ Investigate the flaky orchestration test".to_string(),
            ]
        );
    }

    /// A message that delegates three times announces three spawns before any
    /// of them reports. The viewport has one live slot, so they must share a
    /// cell — otherwise each new announcement evicts the previous cell and
    /// closes a helper that is still working as failed.
    /// Resume replays the stored notification text, so the cost must survive
    /// the round trip — a resumed card that lost its numbers would disagree
    /// with the one the person saw live.
    #[test]
    fn a_persisted_completion_replays_with_its_cost() {
        let stored = "[task notification — background agent `runtime-scout` (id: agent-7) \
                      finished (12 tool uses · 45.3k tokens · 1m 20s). To follow up without \
                      losing its context, use SendMessage with that id/name as `to` to continue \
                      this agent]\n\nfound the bug";
        let (label, status, summary, body) =
            persisted_agent_result(stored).expect("a completion header");
        assert_eq!(label, "runtime-scout");
        assert_eq!(status, AgentResultStatus::Completed);
        assert_eq!(summary.as_deref(), Some("12 tool uses · 45.3k tokens · 1m 20s"));
        assert_eq!(body, "found the bug");
    }

    /// A header written before the cost was carried still replays as the
    /// completion it is — the terminal word, not the period after it, is what
    /// says so.
    #[test]
    fn a_persisted_completion_without_a_cost_still_replays() {
        let stored = "[task notification — background agent `runtime-scout` (id: agent-7) \
                      finished. To follow up without losing its context, use SendMessage with \
                      that id/name as `to` to continue this agent]\n\nfound the bug";
        let (_, status, summary, body) = persisted_agent_result(stored).expect("a completion");
        assert_eq!(status, AgentResultStatus::Completed);
        assert_eq!(summary, None);
        assert_eq!(body, "found the bug");
    }

    #[test]
    fn concurrent_spawns_share_one_cell_with_a_row_each() {
        let spawn = |label: &str| {
            let preview = preview_tool_input(
                "Agent",
                &json!({ "name": label, "description": "look into it" }),
            );
            let summary = preview_summary(&preview);
            ToolKind::from_preview("Agent", &preview, &summary)
        };
        let mut group = ToolGroup::new(ToolCall::new("a".to_string(), spawn("scout")));
        let second = spawn("verifier");
        assert!(group.accepts(&second), "a spawn joins a spawn cell");
        group.push(ToolCall::new("b".to_string(), second));

        assert!(group.set_spawn_progress(&[
            ("a".to_string(), "3 tool uses · 9s · reading".to_string()),
            ("b".to_string(), "1 tool use · 4s · thinking".to_string()),
        ]));
        assert_eq!(
            plain(&group.lines(100, Duration::ZERO)),
            vec![
                "• Spawning scout".to_string(),
                "  └ 3 tool uses · 9s · reading".to_string(),
                "• Spawning verifier".to_string(),
                "  └ 1 tool use · 4s · thinking".to_string(),
            ]
        );

        // A helper the poller no longer lists has no live row to show; a stale
        // one would claim work that is no longer happening.
        assert!(group.set_spawn_progress(&[(
            "a".to_string(),
            "3 tool uses · 9s · reading".to_string()
        )]));
        assert_eq!(
            plain(&group.lines(100, Duration::ZERO)),
            vec![
                "• Spawning scout".to_string(),
                "  └ 3 tool uses · 9s · reading".to_string(),
                "• Spawning verifier".to_string(),
            ]
        );
    }

    /// Workflow and `SpawnMultiAgent` own one outer tool call while several
    /// helpers run beneath it. Sharing that call id must not collapse their
    /// live rows to the first match.
    #[test]
    fn one_spawn_call_keeps_every_matching_helper_row() {
        let preview = preview_tool_input(
            "Workflow",
            &json!({
                "name": "two-reader review",
                "phases": [{"id": "read", "prompt": "inspect", "fanout": ["a", "b"]}]
            }),
        );
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("Workflow", &preview, &summary);
        let mut group = ToolGroup::new(ToolCall::new("workflow-1".to_string(), kind));

        assert!(group.set_spawn_progress(&[
            (
                "workflow-1".to_string(),
                "scout · 3 tool uses · 9s · Read · tui/view.rs".to_string(),
            ),
            (
                "workflow-1".to_string(),
                "reviewer · 1 tool use · 4s · Bash · cargo".to_string(),
            ),
        ]));
        let rows = plain(&group.lines(120, Duration::ZERO));
        assert!(
            rows.iter().any(|row| row.contains("scout · 3 tool uses")),
            "first helper missing: {rows:?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("reviewer · 1 tool use")),
            "second helper missing: {rows:?}"
        );
    }

    /// A read announced beside a delegation must not go red: the read is still
    /// running when the spawn is announced, and the viewport's single slot
    /// would otherwise be taken from it.
    #[test]
    fn a_spawn_never_joins_a_cell_whose_other_tool_is_still_running() {
        let group = ToolGroup::new(ToolCall::new(
            "read".to_string(),
            ToolKind::Explore(vec![Explored::Read {
                name: "src/main.rs".to_string(),
            }]),
        ));
        let preview =
            preview_tool_input("Agent", &json!({ "name": "scout", "description": "look" }));
        let summary = preview_summary(&preview);
        let spawn = ToolKind::from_preview("Agent", &preview, &summary);
        assert!(!group.accepts(&spawn));
        assert!(group.is_active());
    }

    #[test]
    fn a_capped_background_log_still_shows_its_exit_fact() {
        let log = format!("{}{}[exit 7]\n", runtime::background_log::OLDER_OUTPUT_TRUNCATED, "old output\n".repeat(100));
        let notice = runtime::background_log::completion_summary("task-test", &log);
        let mut group = ToolGroup::new(ToolCall::new("bg".to_string(), ToolKind::AgentResult {
            label: runtime::background_log::BACKGROUND_BASH.to_string(), summary: None,
        }));
        group.complete("bg", ok(&format!("{log}\n{notice}\n")));
        let rows = plain(&group.lines(100, Duration::ZERO)).join("\n");
        assert!(rows.contains("exited (code 7)"), "{rows}");
        assert_eq!(rows.matches("background task ").count(), 1);
    }

    #[test]
    fn an_agent_result_uses_a_completed_header_and_report_budget() {
        let mut group = ToolGroup::new(ToolCall::new(
            "agent-result".to_string(),
            ToolKind::AgentResult {
                label: "fable-reviewer".to_string(),
                summary: Some("12 tool uses · 45.3k tokens · 1m 20s".to_string()),
            },
        ));
        let report = (1..=8)
            .map(|line| format!("report line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(group.complete("agent-result", ok(&report)));

        assert_eq!(
            plain(&group.lines(80, Duration::ZERO)),
            vec!["• Completed fable-reviewer (12 tool uses · 45.3k tokens · 1m 20s)".to_string()]
        );
    }

    #[test]
    fn a_successful_agent_result_stays_compact_while_failure_keeps_its_report() {
        let mut group = ToolGroup::new(ToolCall::new(
            "agent-result".to_string(),
            ToolKind::AgentResult {
                label: "background bash".to_string(),
                summary: None,
            },
        ));
        let report = (1..=24)
            .map(|line| format!("output line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let output = format!(
            "[Task notification — a background agent you launched finished while this turn was still running. Its result follows. This is a host notification, not a user message: use the result now where it affects your current work, and account for it before ending the turn.]\n\n\
             [task notification — background agent `background bash` (id: bg-7) finished. To follow up without losing its context, use SendMessage with that id/name as `to` to continue this agent]\n\n\
             {report}"
        );
        assert!(group.complete("agent-result", ok(&output)));

        let rows = plain(&group.lines(80, Duration::ZERO));
        assert_eq!(rows, vec!["• Completed background bash".to_string()]);
        assert!(
            !rows.join("\n").contains("output line"),
            "successful task output belongs in the transcript, not the main scrollback"
        );

        let mut failed = ToolGroup::new(ToolCall::new(
            "agent-result-failed".to_string(),
            ToolKind::AgentResult {
                label: "background bash".to_string(),
                summary: None,
            },
        ));
        assert!(failed.complete(
            "agent-result-failed",
            Outcome {
                declined: false,
                ok: false,
                output: "[exit 1]\ntraceback line".to_string(),
                change: None,
            },
        ));
        assert_eq!(
            plain(&failed.lines(80, Duration::ZERO)),
            vec![
                "• Failed background bash".to_string(),
                "  └ [exit 1]".to_string(),
                "    traceback line".to_string(),
            ]
        );
    }

    #[test]
    fn a_failed_agent_result_uses_the_failure_status_color() {
        let mut group = ToolGroup::new(ToolCall::new(
            "agent-result".to_string(),
            ToolKind::AgentResult {
                label: "reviewer".to_string(),
                summary: None,
            },
        ));
        assert!(group.complete(
            "agent-result",
            Outcome {
                declined: false,
                ok: false,
                output: String::new(),
                change: None,
            },
        ));
        let lines = group.lines(80, Duration::ZERO);

        assert_eq!(plain(&lines), vec!["• Failed reviewer".to_string()]);
        assert_eq!(lines[0].spans[0].style.fg, Some(super::palette::TOOL_FAIL));
    }

    /// Edits closed without a result — the turn was cancelled or panicked, or
    /// a replay had no result for them — say so beside their ✘. A bare
    /// `✘ Failed to apply patch` over rows with no reason read as the tool's
    /// own refusal, which it never was (2026-09-07, t-3063).
    #[test]
    fn edits_closed_without_a_result_say_they_were_interrupted() {
        let announce = |id: &str, path: &str| {
            let preview = preview_tool_input(
                "edit_file",
                &json!({"path": path, "old_string": "a", "new_string": "b"}),
            );
            let summary = preview_summary(&preview);
            ToolCall::new(id.to_string(), ToolKind::from_preview("edit_file", &preview, &summary))
        };
        let mut group = ToolGroup::new(announce("e1", "src/a.rs"));
        group.push(announce("e2", "src/b.rs"));
        group.mark_failed();

        let lines = plain(&group.lines(140, Duration::ZERO));
        assert_eq!(lines[0], "✘ Failed to apply patch", "{lines:?}");
        for path in ["src/a.rs", "src/b.rs"] {
            let row = lines
                .iter()
                .find(|line| line.contains(&format!("✘ {path}")))
                .unwrap_or_else(|| panic!("{path} has no ✘ row: {lines:?}"));
            assert!(
                row.ends_with(&format!("— {}", super::super::strings::TOOL_CALL_INTERRUPTED)),
                "the row says nothing about why: {row:?}"
            );
        }
    }

    /// Two `MultiEdit` calls in one turn. Its result is the same
    /// `structuredPatch` shape `Edit` returns, so the group reads `Edited` and
    /// counts its lines, and a call that failed stands in the group as a
    /// failure with its reason — not as `Edited … (+0 -0)`, and not as a
    /// generic `Called MultiEdit(…)` ("diff 표시 +0 -0", "Failed to apply patch"
    /// without a reason, 2026-09-06).
    #[test]
    fn a_written_file_in_the_group_counts_its_lines_and_a_refusal_says_why() {
        let announce = |id: &str, path: &str| {
            let preview = preview_tool_input("write_file", &json!({"path": path, "content": "a\nb\n"}));
            let summary = preview_summary(&preview);
            ToolCall::new(id.to_string(), ToolKind::from_preview("write_file", &preview, &summary))
        };
        let mut group = ToolGroup::new(announce("w1", "src/a.ts"));
        group.push(announce("w2", "src/b.ts"));
        let created = json!({
            "type": "create", "filePath": "src/a.ts", "content": "a\nb\n",
            "structuredPatch": [{"oldStart": 0, "oldLines": 0, "newStart": 1, "newLines": 2, "lines": ["+a", "+b"]}]
        });
        let ok = format_tool_result_from_raw("write_file", &created.to_string(), false);
        assert!(group.complete("w1", Outcome::from_result(false, &ok, "src/a.ts")));
        let refused = format_tool_result_from_raw(
            "write_file",
            "invalid input: write_file: /Users/dev/x/src/b.ts exists but has not been read in this conversation. Read it with read_file first, then retry this change.",
            true,
        );
        assert!(group.complete("w2", Outcome::from_result(true, &refused, "src/b.ts")));

        let lines = plain(&group.lines(140, Duration::ZERO));
        assert_eq!(lines[0], "• Edited 1 file (+2 -0)", "{lines:?}");
        assert!(lines.iter().any(|line| line.contains("src/a.ts (+2 -0)")), "{lines:?}");
        let failed = lines
            .iter()
            .find(|line| line.contains("✘ src/b.ts"))
            .expect("the refused write stands as a failure");
        assert!(
            failed.contains("— exists but has not been read in this conversation"),
            "the reason follows the path, not the path again: {failed}"
        );
        assert!(!failed.contains("invalid input") && !failed.contains("/Users/dev"), "{failed}");
    }

    /// A newly written file is not pasted whole into the transcript: the
    /// group shows a short head of it and says how many lines follow.
    #[test]
    fn a_created_file_shows_a_short_head_not_the_whole_file() {
        let preview = preview_tool_input("write_file", &json!({"path": "src/big.ts", "content": "x"}));
        let summary = preview_summary(&preview);
        let mut group = ToolGroup::new(ToolCall::new(
            "w1".to_string(),
            ToolKind::from_preview("write_file", &preview, &summary),
        ));
        let body: Vec<String> = (1..=60).map(|n| format!("+line {n}")).collect();
        let created = json!({
            "type": "create", "filePath": "src/big.ts", "content": "",
            "structuredPatch": [{"oldStart": 0, "oldLines": 0, "newStart": 1, "newLines": 60, "lines": body}]
        });
        let ok = format_tool_result_from_raw("write_file", &created.to_string(), false);
        assert!(group.complete("w1", Outcome::from_result(false, &ok, "src/big.ts")));
        let lines = plain(&group.lines(120, Duration::ZERO));
        assert!(lines.iter().any(|line| line.contains("(+60 -0)")), "{lines:?}");
        assert!(
            lines.len() < 30,
            "a 60-line create is a head and a count, not 60 rows: {} rows",
            lines.len()
        );
        assert!(lines.iter().any(|line| line.contains("more line")), "{lines:?}");
    }

    #[test]
    fn multi_edit_results_are_edits_and_a_failed_one_says_why() {
        let announce = |id: &str, path: &str| {
            let preview = preview_tool_input(
                "MultiEdit",
                &json!({
                    "file_path": path,
                    "edits": [
                        { "old_string": "a", "new_string": "b" },
                        { "old_string": "c", "new_string": "d" }
                    ]
                }),
            );
            let summary = preview_summary(&preview);
            ToolCall::new(
                id.to_string(),
                ToolKind::from_preview("MultiEdit", &preview, &summary),
            )
        };
        let mut group = ToolGroup::new(announce("m1", "ui/tokens.css"));
        group.push(announce("m2", "ui/shell.css"));
        assert!(group.is_editing(), "MultiEdit announces as an edit");
        let applied = json!({
            "editsApplied": 2,
            "filePath": "ui/tokens.css",
            "structuredPatch": [{
                "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 3,
                "lines": [" :root {", "-  --a: 1;", "+  --a: 2;", "+  --b: 3;"]
            }]
        });
        let ok = format_tool_result_from_raw("MultiEdit", &applied.to_string(), false);
        assert!(group.complete("m1", Outcome::from_result(false, &ok, "ui/tokens.css")));
        let failed = format_tool_result_from_raw(
            "MultiEdit",
            "io error: edit[5] failed: old_string not found in file",
            true,
        );
        assert!(group.complete("m2", Outcome::from_result(true, &failed, "ui/shell.css")));

        let lines = plain(&group.lines(100, Duration::ZERO));
        assert_eq!(lines[0], "• Edited 1 file (+2 -1)", "{lines:?}");
        assert!(
            lines.iter().any(|line| line.contains("✘ ui/shell.css")
                && line.contains("edit[5] failed: old_string not found in file")),
            "the failed file stands with its reason: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.starts_with("  └ ui/tokens.css (+2 -1)")),
            "the edited file keeps its counts: {lines:?}"
        );
        assert!(!lines.iter().any(|line| line.contains("+0 -0")), "{lines:?}");
    }

    #[test]
    fn a_failed_edit_uses_the_patch_failure_cell() {
        let preview = preview_tool_input(
            "Edit",
            &json!({
                "file_path": "src/lib.rs",
                "old_string": "old",
                "new_string": "new"
            }),
        );
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("Edit", &preview, &summary);
        let mut group = ToolGroup::new(ToolCall::new("edit".to_string(), kind));
        let body = format_tool_result_from_raw("Edit", "context mismatch\ntry again", true);
        assert!(group.complete("edit", Outcome::from_result(true, &body, "src/lib.rs")));

        assert_eq!(
            plain(&group.lines(80, Duration::ZERO)),
            vec![
                "✘ Failed to apply patch".to_string(),
                "  └ context mismatch".to_string(),
                "    try again".to_string(),
            ]
        );
    }

    #[test]
    fn consecutive_edit_results_roll_up_into_a_sorted_multi_file_cell() {
        let b_preview = preview_tool_input("Edit", &json!({ "file_path": "b.rs" }));
        let b_summary = preview_summary(&b_preview);
        let b_kind = ToolKind::from_preview("Edit", &b_preview, &b_summary);
        let a_preview = preview_tool_input("Edit", &json!({ "file_path": "a.rs" }));
        let a_summary = preview_summary(&a_preview);
        let a_kind = ToolKind::from_preview("Edit", &a_preview, &a_summary);
        let mut group = ToolGroup::new(ToolCall::new("b".to_string(), b_kind));

        assert!(group.accepts(&a_kind));
        group.push(ToolCall::new("a".to_string(), a_kind));

        let b_body = format_tool_result_from_raw(
            "Edit",
            r#"{"filePath":"b.rs","structuredPatch":[{"oldStart":1,"oldLines":0,"newStart":1,"newLines":2,"lines":["+one","+two"]}]}"#,
            false,
        );
        let a_body = format_tool_result_from_raw(
            "Edit",
            r#"{"filePath":"a.rs","structuredPatch":[{"oldStart":1,"oldLines":1,"newStart":1,"newLines":1,"lines":["-old","+new"]}]}"#,
            false,
        );
        assert!(group.complete("b", Outcome::from_result(false, &b_body, "b.rs")));
        assert!(group.complete("a", Outcome::from_result(false, &a_body, "a.rs")));

        let rows = plain(&group.lines(80, Duration::ZERO));
        assert_eq!(rows[0], "• Edited 2 files (+3 -1)");
        let a_row = rows.iter().position(|row| row == "  └ a.rs (+1 -1)").expect("a row");
        let b_row = rows.iter().position(|row| row == "  └ b.rs (+2 -0)").expect("b row");
        assert!(a_row < b_row, "rows were {rows:?}");
    }

    #[test]
    fn a_renamed_edit_shows_the_old_and_new_paths() {
        use runtime::message_stream::DiffView;

        let preview = preview_tool_input("Edit", &json!({ "file_path": "old.rs" }));
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("Edit", &preview, &summary);
        let mut group = ToolGroup::new(ToolCall::new("rename".to_string(), kind));
        let body = ToolResultBody::Diff(DiffView {
            old_path: Some("old.rs".to_string()),
            new_path: Some("new.rs".to_string()),
            language: Some("rust".to_string()),
            hunks: Vec::new(),
        });
        assert!(group.complete(
            "rename",
            Outcome::from_result(false, &body, "old.rs"),
        ));

        assert_eq!(
            plain(&group.lines(80, Duration::ZERO))[0],
            "• Edited old.rs → new.rs (+0 -0)"
        );
    }

    #[test]
    fn ansi16_activity_markers_blink_between_bullet_and_circle() {
        use crate::tui::palette::ColorLevel;

        let bullet = super::activity_marker_for_level(Duration::ZERO, ColorLevel::Ansi16);
        let circle = super::activity_marker_for_level(
            Duration::from_millis(600),
            ColorLevel::Ansi16,
        );

        assert_eq!(bullet.text, "•");
        assert_eq!(circle.text, "◦");
        assert!(circle.style.dim);
    }

    #[test]
    fn bash_command_text_uses_syntax_highlight_colors() {
        let preview = preview_tool_input(
            "Bash",
            &json!({ "command": "echo \"hello\" && cargo test --lib" }),
        );
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("Bash", &preview, &summary);
        let group = ToolGroup::new(ToolCall::new("bash".to_string(), kind));
        let lines = group.lines(100, Duration::ZERO);

        assert!(
            lines[0].spans.iter().skip(4).any(|span| span.style.fg.is_some()),
            "command spans were {:?}",
            lines[0].spans
        );
    }

    /// 헤더에 안 들어가는 긴 명령은 `  │ ` 로 두 줄까지 흐르고 접힌다.
    #[test]
    fn a_long_command_continues_under_the_pipe_gutter() {
        let mut group = ToolGroup::new(bash("a", "alpha beta gamma delta epsilon zeta eta theta"));
        assert!(group.complete("a", ok("")));
        let rows = plain(&group.lines(20, Duration::ZERO));
        assert_eq!(rows[0], "• Ran alpha beta");
        assert!(rows[1].starts_with("  │ "), "rows were {rows:?}");
    }

    /// Marker mode의 커밋된 도구 셀은 접기 구간이 된다.
    #[test]
    fn a_committed_cell_with_output_carries_the_fold_markers() {
        use crate::tui::folds::{FoldIds, FoldMode};
        let mut group = ToolGroup::new(bash("a", "ls -la"));
        assert!(group.complete("a", ok("total 16")));
        let mut ids = FoldIds::default();
        let rows = group.committed(60, &mut ids, FoldMode::Markers);
        assert_eq!(
            plain(&rows),
            vec![
                String::new(),
                "• Ran ls -la".to_string(),
                "  └ … +1 lines".to_string(),
                "  └ total 16".to_string(),
            ]
        );
        // 앞 빈 줄은 구간 밖이다.
        assert_eq!(rows[0].lead, None);
        assert_eq!(
            rows[1].lead.as_deref(),
            Some("\u{1b}]7788;begin;0;collapsed,teaser=1;• Ran ls -la\u{1b}\\")
        );
        assert_eq!(rows[3].trail.as_deref(), Some("\u{1b}]7788;end;0\u{1b}\\"));
    }

    /// 맨 터미널에서는 헤더 한 줄뿐이라 손잡이를 달지 않는다 — 접을 본문이 없다.
    #[test]
    fn a_collapsed_group_header_alone_gets_no_fold() {
        use crate::tui::folds::{FoldIds, FoldMode};
        let mut group = ToolGroup::new(bash("a", "ls"));
        assert!(group.complete("a", ok("x")));
        group.push(bash("b", "pwd"));
        assert!(group.complete("b", ok("/tmp")));
        let mut ids = FoldIds::default();
        let rows = group.committed(60, &mut ids, FoldMode::Bare);
        assert_eq!(plain(&rows), vec![String::new(), "• Ran 2 commands".to_string()]);
        assert!(rows.iter().all(|row| row.lead.is_none() && row.trail.is_none()));
    }

    /// marker opt-in에서는 같은 묶음이 본문을 달고 접힌다 — 호출마다 `  └ <cmd>` 와
    /// 그 출력이 네 칸 거터로 이어진다.
    #[test]
    fn marker_mode_hides_group_output_under_the_header() {
        use crate::tui::folds::{FoldIds, FoldMode};
        let mut group = ToolGroup::new(bash("a", "ls"));
        assert!(group.complete("a", ok("x")));
        group.push(bash("b", "pwd"));
        assert!(group.complete("b", ok("/tmp")));
        let mut ids = FoldIds::default();
        let rows = group.committed(60, &mut ids, FoldMode::Markers);
        assert_eq!(
            plain(&rows),
            vec![
                String::new(),
                "• Ran 2 commands".to_string(),
                "  └ … +4 lines".to_string(),
                "  └ ls".to_string(),
                "    x".to_string(),
                "  └ pwd".to_string(),
                "    /tmp".to_string(),
            ]
        );
        // 헤더가 begin 을 물고(접힌 동안 보이는 밀도 줄 하나를 teaser 로 선언),
        // 마지막 본문 줄이 end 를 문다.
        assert_eq!(rows[0].lead, None);
        assert_eq!(
            rows[1].lead.as_deref(),
            Some("\u{1b}]7788;begin;0;collapsed,teaser=1;• Ran 2 commands\u{1b}\\")
        );
        assert_eq!(rows[6].trail.as_deref(), Some("\u{1b}]7788;end;0\u{1b}\\"));
    }

    /// 탐색 묶음의 본문은 codex 의 목록 줄 그대로다 — 출력은 원본도 안 보인다.
    #[test]
    fn a_marked_group_shows_explored_items_as_its_body() {
        use crate::tui::folds::{FoldIds, FoldMode};
        let mut group = ToolGroup::new(read("a", "README.md"));
        assert!(group.complete("a", ok("# hi")));
        group.push(bash("b", "pwd"));
        assert!(group.complete("b", ok("/tmp")));
        let mut ids = FoldIds::default();
        let rows = group.committed(60, &mut ids, FoldMode::Markers);
        assert_eq!(
            plain(&rows),
            vec![
                String::new(),
                "• Ran 2 commands".to_string(),
                "  └ … +3 lines".to_string(),
                "  └ Read README.md".to_string(),
                "  └ pwd".to_string(),
                "    /tmp".to_string(),
            ]
        );
    }

    /// 본문 미리보기는 단일 명령과 같은 한도를 지킨다 — 다섯 행, 머리 절반 ·
    /// `… +N lines` · 꼬리 절반.
    #[test]
    fn the_marked_body_keeps_the_single_command_preview_budget() {
        use crate::tui::folds::{FoldIds, FoldMode};
        let long = (1..=9).map(|n| format!("line{n}")).collect::<Vec<_>>().join("\n");
        let mut group = ToolGroup::new(bash("a", "seq"));
        assert!(group.complete("a", ok(&long)));
        group.push(bash("b", "pwd"));
        assert!(group.complete("b", ok("/tmp")));
        let mut ids = FoldIds::default();
        let rows = group.committed(60, &mut ids, FoldMode::Markers);
        assert_eq!(
            plain(&rows),
            vec![
                String::new(),
                "• Ran 2 commands".to_string(),
                "  └ … +8 lines".to_string(),
                "  └ seq".to_string(),
                "    line1".to_string(),
                "    line2".to_string(),
                "    … +5 lines".to_string(),
                "    line8".to_string(),
                "    line9".to_string(),
                "  └ pwd".to_string(),
                "    /tmp".to_string(),
            ]
        );
    }

    /// 뷰포트의 활성 칸은 marker mode에서도 codex 원본 그대로다 — 매 프레임 다시
    /// 그리는 자리에 본문을 달면 칸만 높아진다.
    #[test]
    fn the_live_cell_never_grows_a_marker_body() {
        let mut group = ToolGroup::new(bash("a", "ls"));
        assert!(group.complete("a", ok("x")));
        group.push(bash("b", "pwd"));
        assert!(group.complete("b", ok("/tmp")));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec!["• Ran 2 commands".to_string()]
        );
    }

    #[test]
    fn a_bash_result_reads_its_exit_code() {
        let body = ToolResultBody::Bash(BashResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "boom".to_string(),
            truncated: false,
        });
        let outcome = Outcome::from_result(false, &body, "");
        assert!(!outcome.ok);
        assert_eq!(outcome.output, "boom");
    }

    #[test]
    fn a_source_truncated_preview_uses_one_codex_elision_row() {
        let body = ToolResultBody::Bash(BashResult {
            exit_code: 0,
            stdout: (1..=8)
                .map(|line| format!("line{line}"))
                .collect::<Vec<_>>()
                .join("\n"),
            stderr: String::new(),
            truncated: true,
        });
        let mut group = ToolGroup::new(bash("a", "seq 8"));
        assert!(group.complete("a", Outcome::from_result(false, &body, "")));

        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec![
                "• Ran seq 8".to_string(),
                "  └ line1".to_string(),
                "    line2".to_string(),
                "    … +5 lines".to_string(),
                "    line7".to_string(),
                "    line8".to_string(),
            ]
        );
    }
}
