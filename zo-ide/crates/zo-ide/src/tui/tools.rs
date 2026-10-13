//! 도구 턴의 셀 — codex `tui/src/exec_cell/`(model.rs·render.rs) 와
//! `history_cell/{patches,mcp}.rs` 의 문법을 그대로 옮긴 것.
//!
//! # 어휘 (t-48216)
//!
//! 호출마다 같은 한 줄을 입는다: `<상태> <동사> <대상>   <결과>`. 상태는 도는
//! 동안 점자 스피너, 끝나면 `✓`, 실패하면 빨간 `✗`, 거절되면 흐린 `✗` 다.
//! 대상은 동사 뒤 한 칸에서 시작하고(동사를 열로 채우지 않는다, t-51389),
//! 결과는 마지막 열에서 끝난다. 끝난 호출의 대상은 줄에서 잘리지 않는다 — 줄에
//! 못 들어간 나머지는 줄 아래 네 칸 들여 걸린다. 출력의 글과 diff 가 시작하는
//! 그 열이라, 어느 동사 아래든 걸린 줄이 한 열에 선다. 명령의 나머지는 출력과
//! 같은 다섯 행 예산을 지키고(앞 행들과 남은 행 수), 접기 표지가 켜진 터미널의
//! 커밋된 셀만 접힌 본문에 전부 둔다.
//!
//! | zo 도구 | 줄 |
//! |---|---|
//! | Bash | `✓ Run <cmd>      3 lines` |
//! | Read·Grep·Glob | `✓ Read <path>` · `✓ Search <q> in <path>` · `✓ List <glob>` |
//! | Edit·Write | `✓ Edit <path>      +A -B` 과 그 아래 diff |
//! | 웹 검색 | `✓ Search <q> on the web` |
//! | 스폰 | `✓ Spawn <label> [role] (request)` 과 그 아래 진행·결과 한 줄 |
//! | 그 외(MCP 등) | `✓ Call <name>(<args>)` 과 그 아래 결과 |
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
pub(super) const OUTPUT_MAX_ROWS: usize = 5;
/// An upstream formatter only reports that it omitted *some* source output.
/// Count that unknown tail conservatively as one line, in the same elision
/// grammar the visible-row cap uses, instead of adding a second
/// `… (truncated)` dialect to the cell.
const SOURCE_TRUNCATION_NOTE: &str = "… +1 lines";
/// Re-injected agent reports need enough context to be useful in history.
const AGENT_RESULT_MAX_ROWS: usize = 20;
/// codex `multi_agents.rs::COLLAB_AGENT_RESPONSE_PREVIEW_GRAPHEMES` — how much
/// of a finished agent's message stands beside `Completed`. zo counts scalar
/// values rather than graphemes (no segmentation crate); the value is codex's.
const COLLAB_AGENT_RESPONSE_PREVIEW_GRAPHEMES: usize = 240;
/// codex `multi_agents.rs::COLLAB_PROMPT_PREVIEW_GRAPHEMES` — how much of the
/// delegated prompt a spawn cell keeps under `└`.
const COLLAB_PROMPT_PREVIEW_GRAPHEMES: usize = 160;

/// 도구 하나가 무엇을 하는지 — codex 의 `ParsedCommand`/셀 종류에 대응한다.
#[derive(Debug, Clone)]
pub enum ToolKind {
    /// Bash: a `Run` row. `background` is a command that ran as a task
    /// (`run_in_background`) and came back down the completion road: its
    /// row carries one dim word beside the command and nothing else
    /// differs from the foreground row (t-3177).
    Command { command: String, background: bool },
    /// Read·Grep·Glob 류. 한 호출이 여러 항목을 낼 수 있다(codex `parsed`).
    Explore(Vec<Explored>),
    /// Edit·Write: an `Edit`, `Create` or `Delete` row and its diff.
    Edit { path: String },
    /// `TodoWrite` / `TaskList` result. `Updated Plan` with an uncapped checklist.
    Plan(Vec<TodoResultItem>),
    /// Web search/fetch activity: a `Search` row whose target is the query
    /// on the web.
    WebSearch { detail: String },
    /// Agent-family spawn: a `Spawn` row. A running one shows the helper's
    /// live row under it.
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
    /// 그 밖의 도구 — `Call` 줄에 이름과 인자를 쓴다.
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
            (Some(old), Some(new)) if old != new => format!("{} → {}", shown_path(old), shown_path(new)),
            _ => shown_path(&self.path),
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

fn change_count_spans(added: usize, removed: usize) -> Vec<Span> {
    vec![
        Span::new(format!("+{added}"), Style::new().fg(Color::GREEN)),
        Span::raw(" "),
        Span::new(format!("-{removed}"), Style::new().fg(Color::RED)),
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
    /// A finished command's exit code — a failed row's result.
    pub exit_code: Option<i32>,
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
            exit_code: match body {
                ToolResultBody::Bash(result) => Some(result.exit_code),
                _ => None,
            },
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
    /// its rendered lines; mutations clear this cache below. Keyed by call id:
    /// a group holds several edits.
    diff_cache: RefCell<Vec<(String, usize, Vec<Line>)>>,
}

impl ToolGroup {
    #[must_use]
    pub fn new(call: ToolCall) -> Self {
        Self { calls: vec![call], diff_cache: RefCell::default() }
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
        self.diff_cache.get_mut().clear();
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
        self.diff_cache.get_mut().clear();
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
        self.diff_cache.get_mut().clear();
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
                    exit_code: None,
                });
            }
        }
        self.diff_cache.get_mut().clear();
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
        self.lines_in(width, elapsed)
    }

    fn lines_in(&self, width: usize, elapsed: Duration) -> Vec<Line> {
        self.calls
            .iter()
            .flat_map(|call| self.call_lines(call, width, elapsed, false))
            .collect()
    }

    /// 한 호출의 줄들 — 종류마다 다른 문법. `whole` 은 끝난 명령의 글을
    /// 예산 없이 전부 그린다 — 사람이 펼치는 접기 본문 안에서만.
    fn call_lines(
        &self,
        call: &ToolCall,
        width: usize,
        elapsed: Duration,
        whole: bool,
    ) -> Vec<Line> {
        match &call.kind {
            ToolKind::Command {
                command,
                background,
            } => self.command_row(call, command, *background, width, elapsed, whole),
            ToolKind::Explore(items) => items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let (verb, target) = explored_target(item);
                    // An explore call with several items reports once, on its first row.
                    let result = call
                        .done
                        .as_ref()
                        .filter(|_| index == 0)
                        .map(|done| explored_result(item, done))
                        .unwrap_or_default();
                    tool_row(self.row_glyph(call, elapsed), verb, target, result, width)
                })
                .collect(),
            ToolKind::Edit { path } => self.edit_row(call, path, width, elapsed),
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

    fn cached_diff_lines(&self, id: &str, view: &DiffView, width: usize) -> Vec<Line> {
        {
            let cache = self.diff_cache.borrow();
            let hit = cache
                .iter()
                .find(|(cached, cached_width, _)| cached == id && *cached_width == width);
            if let Some((_, _, lines)) = hit {
                return lines.clone();
            }
        }
        let lines = super::diff::hunk_lines(view, width);
        let mut cache = self.diff_cache.borrow_mut();
        cache.retain(|(cached, _, _)| cached != id);
        cache.push((id.to_string(), width, lines.clone()));
        lines
    }

    /// `✓ Run <cmd>      3 lines` and the output under it; while the
    /// command runs, the tail of what it has printed so far. What a finished
    /// command's row cannot show hangs under it within the output's budget
    /// ([`command_text_rows`]) — all of it when `whole`.
    fn command_row(
        &self,
        call: &ToolCall,
        command: &str,
        background: bool,
        width: usize,
        elapsed: Duration,
        whole: bool,
    ) -> Vec<Line> {
        let result = call.done.as_ref().map(command_result).unwrap_or_default();
        let marker =
            background.then(|| Span::dim(format!(" {}", super::strings::COMMAND_BACKGROUND)));
        let (mut target, rest) = if call.done.is_some() {
            let lines = super::highlight::highlight_block_to_lines(command, "bash")
                .unwrap_or_else(|| command.lines().map(Line::from_text).collect());
            let budget = target_budget(super::strings::TOOL_RUN, &result, width)
                .saturating_sub(marker.as_ref().map_or(0, Span::width));
            split_target(lines, budget)
        } else {
            let first = command.lines().next().unwrap_or_default();
            let target = super::highlight::highlight_block_to_lines(first, "bash")
                .and_then(|lines| lines.into_iter().next())
                .map_or_else(|| vec![Span::raw(first.to_string())], |line| line.spans);
            (target, Vec::new())
        };
        target.extend(marker);
        let mut out = vec![tool_row(
            self.row_glyph(call, elapsed),
            super::strings::TOOL_RUN,
            target,
            result,
            width,
        )];
        let hanging = hung(&rest, width);
        out.extend(if whole { hanging } else { command_text_rows(hanging) });
        match call.done.as_ref() {
            Some(done) => out.extend(output_block(&done.output, width)),
            None => out.extend(live_tail(&call.id, width)),
        }
        out
    }

    /// `✓ Edit <path>      +A -B` and the change's diff hung under it.
    fn edit_row(&self, call: &ToolCall, path: &str, width: usize, elapsed: Duration) -> Vec<Line> {
        let done = call.done.as_ref();
        let change = done.filter(|done| done.ok).and_then(|done| done.change.as_ref());
        let verb = change.map_or(super::strings::TOOL_EDIT, |change| match change.verb {
            "Added" => super::strings::TOOL_CREATE,
            "Deleted" => super::strings::TOOL_DELETE,
            _ => super::strings::TOOL_EDIT,
        });
        let target = change.map_or_else(|| shown_path(path), Change::display_path);
        let result = match (done, change) {
            (Some(done), _) if !done.ok => failure_result(done),
            (_, Some(change)) => change_count_spans(change.added, change.removed),
            _ => Vec::new(),
        };
        let mut out = vec![tool_row(
            self.row_glyph(call, elapsed),
            verb,
            vec![Span::raw(target)],
            result,
            width,
        )];
        if let Some(done) = done.filter(|done| !done.ok && !done.output.is_empty()) {
            let (first, rest) = done.output.split_once('\n').unwrap_or((done.output.as_str(), ""));
            let mut reason = failure_reason(first, path);
            if !rest.is_empty() {
                reason = format!("{reason}\n{rest}");
            }
            out.extend(output_block(&reason, width));
        }
        if let Some(change) = change {
            let drawn = self.cached_diff_lines(
                &call.id,
                &change.view,
                width.saturating_sub(DIFF_INDENT).max(1),
            );
            out.extend(change_body(change, drawn).into_iter().map(|mut line| {
                line.spans.insert(0, Span::raw(" ".repeat(DIFF_INDENT)));
                line
            }));
        }
        out
    }

    /// A running call's spinner frame, or how its call ended.
    fn row_glyph(&self, call: &ToolCall, elapsed: Duration) -> Span {
        match call.done.as_ref() {
            None => {
                let ticks = elapsed.max(self.age(call)).as_millis() / SPINNER_FRAME_MILLIS;
                let frame = usize::try_from(ticks).unwrap_or_default() % SPINNER_FRAMES.len();
                Span::new(SPINNER_FRAMES[frame], Style::new().fg(palette::TOOL_LABEL))
            }
            Some(done) if done.ok => {
                Span::new(super::strings::TOOL_DONE, Style::new().fg(palette::TOOL_OK).bold())
            }
            Some(done) if done.declined => Span::dim(super::strings::TOOL_FAILED),
            Some(_) => Span::new(super::strings::TOOL_FAILED, Style::new().fg(palette::TOOL_FAIL).bold()),
        }
    }

    /// `✓ Call <name>(<args>)` and what the call returned under it; an
    /// empty result adds no row.
    fn mcp_lines(
        &self,
        call: &ToolCall,
        name: &str,
        detail: &str,
        width: usize,
        elapsed: Duration,
    ) -> Vec<Line> {
        let mut invocation = vec![Span::new(
            name.to_string(),
            Style::new().fg(palette::TOOL_LABEL),
        )];
        if !detail.is_empty() {
            invocation.push(Span::raw("("));
            invocation.push(Span::dim(detail.to_string()));
            invocation.push(Span::raw(")"));
        }
        let result = call.done.as_ref().filter(|done| !done.ok).map(failure_result).unwrap_or_default();
        let mut out =
            self.uncut_row(call, super::strings::TOOL_CALL, invocation, result, width, elapsed);
        if let Some(done) = call.done.as_ref() {
            let said = mcp_envelope_text(&done.output);
            let output = said.as_deref().unwrap_or(&done.output);
            if !output.is_empty() {
                out.extend(output_block(output, width));
            }
        }
        out
    }

    /// `✓ Search <query> on the web` — a file search's row, with the web
    /// where a file search names its path.
    fn web_search_lines(
        &self,
        call: &ToolCall,
        detail: &str,
        width: usize,
        elapsed: Duration,
    ) -> Vec<Line> {
        let mut target = Vec::new();
        if !detail.is_empty() {
            target.push(Span::raw(detail.to_string()));
            target.push(Span::dim(" on "));
        }
        target.push(Span::dim(super::strings::THE_WEB));
        let result = call.done.as_ref().filter(|done| !done.ok).map(failure_result).unwrap_or_default();
        self.uncut_row(call, super::strings::TOOL_SEARCH, target, result, width, elapsed)
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
        let mut target = vec![Span::new(
            spawn.label.to_string(),
            Style::new().fg(Color::CYAN).bold(),
        )];
        if let Some(role) = spawn.role {
            target.push(Span::dim(" "));
            target.push(Span::raw(format!("[{role}]")));
        }
        if let Some(request) = spawn.request {
            target.push(Span::dim(" "));
            target.push(Span::new(
                format!("({request})"),
                Style::new().fg(Color::MAGENTA),
            ));
        }
        let result = match call.done.as_ref() {
            // A policy verdict is not a failure: the dispatch kept the work
            // inline and said so (`ToolResultBody::Declined`), and a person
            // reading "failed" here looked for a fault that was not there
            // (2026-09-15, "이것도 오류인거같은데").
            Some(done) if done.declined => vec![Span::dim(super::strings::SPAWN_DECLINED_RESULT)],
            Some(done) if !done.ok => failure_result(done),
            _ => Vec::new(),
        };

        let mut out =
            self.uncut_row(call, super::strings::TOOL_SPAWN, target, result, width, elapsed);
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
        } else if let Some(preview) = call.done.as_ref().and_then(|done| completed_preview(&done.output)) {
            // The helper answered in this call: codex's `Completed - <message>`
            // row (`multi_agents.rs::status_summary_spans`) takes the slot.
            out.extend(prefixed(&[preview], width, &Prefix::tool_output(), true));
        } else if !spawn.prompt.is_empty() {
            out.extend(prefixed(
                &[Line::from_text(truncate_text(spawn.prompt, COLLAB_PROMPT_PREVIEW_GRAPHEMES))],
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

    /// A row whose target is not cut once its call is done: what the row
    /// cannot show then hangs under it. A running call keeps to one row.
    fn uncut_row(
        &self,
        call: &ToolCall,
        verb: &str,
        target: Vec<Span>,
        result: Vec<Span>,
        width: usize,
        elapsed: Duration,
    ) -> Vec<Line> {
        let (target, rest) = if call.done.is_some() {
            split_target(vec![Line::new(target)], target_budget(verb, &result, width))
        } else {
            (target, Vec::new())
        };
        let mut out = vec![tool_row(self.row_glyph(call, elapsed), verb, target, result, width)];
        out.extend(hung(&rest, width));
        out
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
        if self.calls.is_empty() {
            return Vec::new();
        }
        let mut out = vec![Line::empty()];
        // With markers enabled each call's row heads a fold of what hangs
        // under it, keeping codex's density: the row and one
        // `  └ … +N lines` teaser, N being the rows the fold hides. The
        // teaser is a row of the region drawn only while it is collapsed
        // (`teaser=1`), so opening the row shows the body in its place.
        for call in &self.calls {
            // A fold the person opens holds a long command's whole text.
            let mut cell = self.call_lines(call, width, Duration::ZERO, mode.markers_enabled());
            if mode.markers_enabled() && cell.len() >= 2 {
                let folded = cell.len() - 1;
                cell.insert(1, teaser_line(folded, width));
                out.extend(folds::wrap_cell_with_teaser(cell, 1, ids));
            } else {
                out.extend(cell);
            }
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
/// `✓ Check <command>      passed after the last edit` — a check the
/// harness saw pass after the turn's last edit, shaped like a call's row.
#[must_use]
pub fn check_row(command: &str, width: usize) -> Line {
    let target = super::highlight::highlight_block_to_lines(command, "bash")
        .and_then(|lines| lines.into_iter().next())
        .map_or_else(|| vec![Span::raw(command.to_string())], |line| line.spans);
    tool_row(
        Span::new(super::strings::TOOL_DONE, Style::new().fg(palette::TOOL_OK).bold()),
        super::strings::TOOL_CHECK,
        target,
        vec![Span::dim(super::strings::CHECK_PASSED)],
        width,
    )
}

/// How far the rows under a call hang in — the column its output's text
/// (`  └ `) and an edit's diff ([`DIFF_INDENT`]) start on, whatever the verb.
const HANG_COLUMNS: usize = 4;
/// How far an edit's diff hangs in under its row.
const DIFF_INDENT: usize = 4;
/// A running call's spinner, one frame per [`SPINNER_FRAME_MILLIS`].
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPINNER_FRAME_MILLIS: u128 = 80;
/// How much of a running command's output its row reads, and how many of the
/// last lines it shows.
const LIVE_TAIL_BYTES: usize = 4096;
const LIVE_TAIL_ROWS: usize = 4;

/// `<glyph> <Verb> <target>   <result>` — the target follows its verb after
/// one space, the result ends on the last column, and the target gives way
/// to it when the row is short.
fn tool_row(glyph: Span, verb: &str, target: Vec<Span>, result: Vec<Span>, width: usize) -> Line {
    let mut spans = vec![glyph, Span::raw(" "), Span::bold(format!("{verb} "))];
    let head: usize = spans.iter().map(Span::width).sum();
    let result_width: usize = result.iter().map(Span::width).sum();
    let target = Line::new(target).truncated(target_budget(verb, &result, width));
    let used = head + target.width();
    spans.extend(target.spans);
    if result_width > 0 {
        spans.push(Span::raw(" ".repeat(width.saturating_sub(used + result_width).max(1))));
        spans.extend(result);
    }
    Line::new(spans).truncated(width)
}

/// The columns a row leaves its target: the width less the glyph, the verb
/// and the space after each, and less the result with the space before it.
fn target_budget(verb: &str, result: &[Span], width: usize) -> usize {
    let result_width: usize = result.iter().map(Span::width).sum();
    let reserved = if result_width == 0 { 0 } else { result_width + 1 };
    width.saturating_sub(2 + super::ansi::str_width(verb) + 1 + reserved)
}

/// A target that is not cut, split for its row: the stretch of its first line
/// that fits in `budget` columns, then every row after it.
fn split_target(lines: Vec<Line>, budget: usize) -> (Vec<Span>, Vec<Line>) {
    let mut lines = lines.into_iter();
    let first = lines.next().unwrap_or_else(Line::empty);
    let mut stretches = wrap_line(&first, budget.max(1), &Span::raw("")).into_iter();
    let head = stretches.next().map(|line| line.spans).unwrap_or_default();
    (head, stretches.chain(lines).collect())
}

/// Rows hung under a row, [`HANG_COLUMNS`] in, wrapped at the edge.
fn hung(rows: &[Line], width: usize) -> Vec<Line> {
    let indent = Span::raw(" ".repeat(HANG_COLUMNS));
    prefixed(rows, width, &Prefix::new(indent.clone(), indent), true)
}

/// A finished command's hung rows held to the output's budget: its first
/// rows and a count of the rest. A command reads better from its head than
/// around a middle cut — the head says what ran, and a heredoc's tail is
/// mostly its closing word; codex holds a command's own rows the same way
/// (`ExecCell::limit_lines_from_start`, here [`limit_from_start`]).
fn command_text_rows(rows: Vec<Line>) -> Vec<Line> {
    if rows.len() <= OUTPUT_MAX_ROWS {
        return rows;
    }
    let mut kept = limit_from_start(&rows, OUTPUT_MAX_ROWS - 1);
    if let Some(count) = kept.last_mut() {
        count.spans.insert(0, Span::raw(" ".repeat(HANG_COLUMNS)));
    }
    kept
}

/// What an MCP tool's result envelope says, for the rows under its call: the
/// text items in order, an image or another item as a word — `None` when
/// the output is not an envelope (a `content` array of typed items), and
/// empty when the envelope says nothing. Display only: the session file and
/// the wire keep the envelope.
///
/// The runtime hands the cell the envelope pretty-printed and capped
/// (`format_generic_result`), so a long one arrives cut inside its text or an
/// image's data; it is closed where the cap left it ([`closed_json`]) and
/// read as far as it goes, the cap's own `… +1 lines` after it. Its last item
/// may then lack its `type`, or its text: an untyped one with text shows the
/// text, and one with neither is left out.
fn mcp_envelope_text(output: &str) -> Option<String> {
    let (printed, cut) = match output.strip_suffix(SOURCE_TRUNCATION_NOTE) {
        Some(printed) => (printed.trim_end_matches('\n'), true),
        None => (output, false),
    };
    if !printed.starts_with('{') {
        return None;
    }
    let envelope: serde_json::Value = if cut {
        serde_json::from_str(&closed_json(printed)).ok()?
    } else {
        serde_json::from_str(printed).ok()?
    };
    let items = envelope.get("content")?.as_array()?;
    let mut said = Vec::with_capacity(items.len() + 1);
    for (index, item) in items.iter().enumerate() {
        let text = item.get("text").and_then(serde_json::Value::as_str);
        let word = match item.get("type").and_then(serde_json::Value::as_str) {
            Some("text") => text,
            Some("image") => Some(super::strings::MCP_IMAGE_ITEM),
            Some(_) => Some(super::strings::MCP_RESOURCE_ITEM),
            None => text.filter(|_| cut),
        };
        match word {
            Some(word) => said.push(word),
            // The cap cut the last item before what it needs was printed —
            // keys print in their own order, `text` or `data` may come first.
            None if cut && index + 1 == items.len() => {}
            None => return None,
        }
    }
    // A cut envelope always says it was cut, even when nothing before the cut
    // could be shown.
    if cut {
        said.push(SOURCE_TRUNCATION_NOTE);
    }
    if said.iter().all(|text| text.trim().is_empty()) {
        return Some(String::new());
    }
    Some(said.join("\n"))
}

/// `printed` — a JSON document cut short — closed so it parses: an open
/// string ended (a dangling backslash dropped), a trailing comma dropped,
/// and the open arrays and objects closed in order. A cap cuts the pretty
/// print inside a string or between whole lines, and either closes cleanly.
fn closed_json(printed: &str) -> String {
    let mut open = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    for ch in printed.chars() {
        if in_string {
            match ch {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => open.push('}'),
            '[' => open.push(']'),
            '}' | ']' => {
                open.pop();
            }
            _ => {}
        }
    }
    let mut closed = printed.to_string();
    if in_string {
        if escaped {
            closed.pop();
        }
        closed.push('"');
    }
    let kept = closed.trim_end().trim_end_matches(',').len();
    closed.truncate(kept);
    closed.extend(open.iter().rev());
    closed
}

fn explored_target(item: &Explored) -> (&'static str, Vec<Span>) {
    match item {
        Explored::Read { name } => (super::strings::TOOL_READ, vec![Span::raw(shown_path(name))]),
        Explored::List { path } => (super::strings::TOOL_LIST, vec![Span::raw(shown_path(path))]),
        Explored::Search { query, path } => {
            let mut target = vec![Span::raw(query.clone())];
            if let Some(path) = path {
                target.push(Span::dim(" in "));
                target.push(Span::raw(shown_path(path)));
            }
            (super::strings::TOOL_SEARCH, target)
        }
    }
}

fn explored_result(item: &Explored, done: &Outcome) -> Vec<Span> {
    if !done.ok {
        return failure_result(done);
    }
    let rows = done.output.lines().filter(|line| !line.trim().is_empty()).count();
    let text = match item {
        Explored::Read { .. } => kept_lines(&done.output),
        Explored::List { .. } => super::strings::entry_count(rows),
        Explored::Search { .. } => super::strings::match_count(rows),
    };
    vec![Span::dim(text)]
}

/// How many lines a command printed — nothing when it printed nothing, since
/// the `(no output)` row under it already says so.
fn command_result(done: &Outcome) -> Vec<Span> {
    if !done.ok {
        return failure_result(done);
    }
    if done.output.trim().is_empty() {
        return Vec::new();
    }
    vec![Span::dim(kept_lines(&done.output))]
}

/// How many lines an output holds; one cut at its source had more than it kept.
fn kept_lines(output: &str) -> String {
    match output.strip_suffix(SOURCE_TRUNCATION_NOTE) {
        Some(kept) => super::strings::lines_at_least(kept.lines().count()),
        None => super::strings::line_count(output.lines().count()),
    }
}

/// `declined`, `exit 101`, or `failed` when there is no exit code to name.
fn failure_result(done: &Outcome) -> Vec<Span> {
    if done.declined {
        return vec![Span::dim(super::strings::TOOL_DECLINED_RESULT)];
    }
    let text = done
        .exit_code
        .filter(|code| *code != 0)
        .map_or_else(|| super::strings::TOOL_FAILED_RESULT.to_string(), super::strings::exit_code);
    vec![Span::new(text, Style::new().fg(palette::TOOL_FAIL))]
}

/// The last lines a running command has printed, `  │ ` under its row.
fn live_tail(id: &str, width: usize) -> Vec<Line> {
    let Some(tail) = runtime::live_output::snapshot(id, LIVE_TAIL_BYTES) else {
        return Vec::new();
    };
    let printed = format!("{}\n{}", tail.stdout_tail, tail.stderr_tail);
    let printed = runtime::live_output::sanitize_live_output(&printed);
    let rows: Vec<&str> = printed.lines().filter(|line| !line.trim().is_empty()).collect();
    rows[rows.len().saturating_sub(LIVE_TAIL_ROWS)..]
        .iter()
        .map(|row| Line::new(vec![Span::dim("  │ "), Span::dim((*row).to_string())]).truncated(width))
        .collect()
}

/// A path under the working directory, shown from there.
fn shown_path(path: &str) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| {
            std::path::Path::new(path)
                .strip_prefix(cwd)
                .ok()
                .filter(|rest| !rest.as_os_str().is_empty())
                .map(|rest| rest.display().to_string())
        })
        .unwrap_or_else(|| path.to_string())
}

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

/// 같은 미리보기를, 접두어를 골라서.
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

/// The `Completed - <message preview>` row of a spawn whose helper answered
/// inside the call — codex `multi_agents.rs::status_summary_spans` for a
/// `Completed` agent state. `None` for a detached spawn, whose immediate
/// result is the runtime's JSON envelope (`agentId`, `status`) and whose
/// report lands later as its own `AgentResult` cell.
fn completed_preview(output: &str) -> Option<Line> {
    let output = strip_agent_result_harness(output);
    let trimmed = output.trim();
    if trimmed.is_empty()
        || trimmed.ends_with(
            runtime::message_stream::anthropic::tools::DETACHED_SPAWN_RECEIPT,
        )
    {
        return None;
    }
    if trimmed.starts_with('{') && serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        return None;
    }
    let message = trimmed.split_whitespace().collect::<Vec<_>>().join(" ");
    let preview = truncate_text(&message, COLLAB_AGENT_RESPONSE_PREVIEW_GRAPHEMES);
    Some(Line::new(vec![
        Span::new("Completed", Style::new().fg(palette::TOOL_OK)),
        Span::dim(" - "),
        Span::raw(preview),
    ]))
}

/// codex `text_formatting::truncate_text`: the first `max` units of `text`
/// with an ellipsis when anything was cut; the whole text otherwise.
fn truncate_text(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{}…", head.trim_end())
    } else {
        head
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
    // A background command's completion names no agent: the card is the
    // command cell its log draws (`background_command_cell`), and the verdict
    // is the log's exit code — a log with none ended on a signal.
    if runtime::background_log::is_notification_header(header) {
        let log = strip_agent_result_harness(body);
        let status = match runtime::background_log::exit_code(log) {
            Some(0) => AgentResultStatus::Completed,
            _ => AgentResultStatus::Failed,
        };
        return Some((
            runtime::background_log::BACKGROUND_BASH.to_string(),
            status,
            None,
            log.to_string(),
        ));
    }
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
        AgentResultStatus::Completed | AgentResultStatus::Running => 0,
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

pub(crate) fn is_runtime_agent_notification_preamble(line: &str) -> bool {
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
pub(super) fn limit_from_start(lines: &[Line], keep: usize) -> Vec<Line> {
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
            exit_code: None,
        }
    }

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::plain).collect()
    }

    /// `left`, then spaces, then `right` ending on the last column.
    fn edge(left: &str, right: &str, width: usize) -> String {
        let used = left.chars().count() + right.chars().count();
        format!("{left}{}{right}", " ".repeat(width.saturating_sub(used)))
    }

    #[test]
    fn a_running_command_spins_and_a_finished_one_wears_a_check() {
        let mut group = ToolGroup::new(bash("a", "ls -la"));
        assert_eq!(plain(&group.lines(60, Duration::ZERO))[0], "⠋ Run ls -la");
        assert!(group.complete("a", ok("total 16\nsrc")));
        let lines = group.lines(60, Duration::ZERO);
        assert_eq!(
            plain(&lines),
            vec![
                edge("✓ Run ls -la", "2 lines", 60),
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

    #[test]
    fn long_output_keeps_a_head_and_a_tail_around_the_ellipsis() {
        let output = (1..=20).map(|n| n.to_string()).collect::<Vec<_>>().join("\n");
        let mut group = ToolGroup::new(bash("a", "seq 20"));
        assert!(group.complete("a", ok(&output)));
        let rows = plain(&group.lines(60, Duration::ZERO));
        assert_eq!(
            rows,
            vec![
                edge("✓ Run seq 20", "20 lines", 60),
                "  └ 1".to_string(),
                "    2".to_string(),
                "    … +16 lines".to_string(),
                "    19".to_string(),
                "    20".to_string(),
            ]
        );
    }

    #[test]
    fn a_live_read_is_one_spinning_row() {
        let group = ToolGroup::new(read("a", "README.md"));
        assert_eq!(plain(&group.lines(60, Duration::ZERO)), vec!["⠋ Read README.md".to_string()]);
    }

    #[test]
    fn consecutive_reads_are_one_row_each() {
        let mut group = ToolGroup::new(read("a", "a.rs"));
        assert!(group.accepts(&ToolKind::Explore(vec![Explored::Read {
            name: "b.rs".to_string()
        }])));
        group.push(read("b", "b.rs"));
        group.push(read("c", "a.rs"));
        // 아직 도는 묶음 — 호출마다 제 줄과 스피너가 있다.
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec![
                "⠋ Read a.rs".to_string(),
                "⠋ Read b.rs".to_string(),
                "⠋ Read a.rs".to_string(),
            ]
        );
    }

    #[test]
    fn a_finished_explore_group_keeps_one_row_per_call() {
        let mut group = ToolGroup::new(read("a", "a.rs"));
        group.push(read("b", "b.rs"));
        assert!(group.complete("a", ok("")));
        assert!(group.complete("b", ok("")));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec![edge("✓ Read a.rs", "0 lines", 60), edge("✓ Read b.rs", "0 lines", 60)]
        );
    }

    #[test]
    fn a_single_finished_read_is_one_row() {
        let mut group = ToolGroup::new(read("a", "README.md"));
        assert!(group.complete("a", ok("")));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec![edge("✓ Read README.md", "0 lines", 60)]
        );
    }

    #[test]
    fn two_finished_commands_are_two_rows() {
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
            vec![
                edge("✓ Run ls -la", "1 line", 60),
                "  └ x".to_string(),
                edge("✓ Run pwd", "1 line", 60),
                "  └ /tmp".to_string(),
            ]
        );
    }

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
            vec![
                edge("✓ Run ls -la", "1 line", 60),
                "  └ total 8".to_string(),
                edge("✓ Read README.md", "1 line", 60),
            ]
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
                exit_code: None,
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
            "⠋ Edit a.rs"
        );
        assert!(group.complete(
            "a",
            Outcome::from_result(false, &ToolResultBody::Diff(diff), "a.rs")
        ));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO))[0],
            edge("✓ Edit a.rs", "+2 -1", 60)
        );
    }

    #[test]
    fn other_tools_wear_the_call_row() {
        let mut group = ToolGroup::new(ToolCall::new(
            "a".to_string(),
            ToolKind::Call {
                name: "TodoWrite".to_string(),
                detail: "3 items".to_string(),
            },
        ));
        // The row every call wears: the spinner, then ✓ or ✗, the verb, and
        // the invocation as its target while it runs and once it is done.
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO))[0],
            "⠋ Call TodoWrite(3 items)"
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
                "✓ Call TodoWrite(3 items)".to_string(),
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
            "⠋ Call TodoWrite"
        );
    }

    #[test]
    fn a_capability_invoke_read_preview_renders_as_a_read_row() {
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
            vec!["⠋ Read src/lib.rs".to_string()]
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
            vec!["✓ Call Sleep".to_string()]
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

    /// A finished call keeps its whole invocation: what the row cannot show
    /// hangs under it, four columns in, as a long command's text does.
    #[test]
    fn a_long_call_keeps_its_whole_invocation_under_its_row() {
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

        assert!(rows[0].starts_with("✓ Call Lookup("), "rows were {rows:?}");
        assert!(rows.len() >= 3, "rows were {rows:?}");
        assert!(
            rows[1..rows.len() - 1].iter().all(|row| row.starts_with("    ")),
            "rows were {rows:?}"
        );
        assert!(rows.iter().all(|row| !row.contains('…')), "rows were {rows:?}");
        assert!(rows.iter().any(|row| row.contains("epsilon)")), "rows were {rows:?}");
        assert_eq!(rows.last().expect("output row"), "  └ ok");
    }

    #[test]
    fn web_search_wears_the_search_row() {
        let preview = preview_tool_input("WebSearch", &json!({ "query": "rust async" }));
        let summary = preview_summary(&preview);
        let kind = ToolKind::from_preview("WebSearch", &preview, &summary);
        let mut group = ToolGroup::new(ToolCall::new("a".to_string(), kind));

        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec!["⠋ Search rust async on the web".to_string()]
        );

        let body = format_tool_result_from_raw("WebSearch", "raw search result", false);
        assert!(group.complete("a", Outcome::from_result(false, &body, "")));
        let lines = group.lines(60, Duration::ZERO);
        assert_eq!(plain(&lines), vec!["✓ Search rust async on the web".to_string()]);
        assert_eq!(lines[0].spans[0].style.fg, Some(super::palette::TOOL_OK));
    }

    #[test]
    fn failed_spawn_shows_the_refusal_instead_of_the_delegated_prompt() {
        let preview = preview_tool_input("Agent", &json!({"description":"look up remote", "prompt":"delegated prompt"}));
        let kind = ToolKind::from_preview("Agent", &preview, &preview_summary(&preview));
        let mut group = ToolGroup::new(ToolCall::new("a".to_string(), kind));
        let reason = "Small delegated task: do it inline. Do not retry this spawn.";
        let body = ToolResultBody::Declined { reason: reason.to_owned() };
        group.complete("a", Outcome::from_result(true, &body, reason));
        let lines = group.lines(100, Duration::ZERO);
        let rows = plain(&lines);
        let text = rows.join("\n");
        assert!(rows[0].starts_with("✗ Spawn "), "{text}");
        assert!(rows[0].ends_with("declined — kept inline") && text.contains(reason), "{text}");
        // A policy verdict is not a failure: it does not say so, and its ✗ is
        // dimmed rather than red.
        assert!(!text.contains("failed"), "a verdict is not a failure:\n{text}");
        assert!(lines[0].spans[0].style.dim && lines[0].spans[0].style.fg.is_none(), "{:?}", lines[0].spans[0]);
        // A spawn that really failed still says so.
        let kind = ToolKind::from_preview("Agent", &preview, &preview_summary(&preview));
        let mut failed = ToolGroup::new(ToolCall::new("b".to_string(), kind));
        let body = format_tool_result_from_raw("Agent", "execution error: child died", true);
        failed.complete("b", Outcome::from_result(true, &body, "child died"));
        let rows = plain(&failed.lines(100, Duration::ZERO));
        assert!(rows[0].starts_with("✗ Spawn ") && rows[0].ends_with("failed"), "{rows:?}");
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
            vec!["⠋ Spawn robie [worker] (gpt-5.1 high)".to_string()]
        );
        assert!(group.set_spawn_progress(&[(
            "a".to_string(),
            "12 tool uses · 1m 20s · Read · src/main.rs".to_string(),
        )]));
        assert_eq!(
            plain(&group.lines(100, Duration::ZERO)),
            vec![
                "⠋ Spawn robie [worker] (gpt-5.1 high)".to_string(),
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
                "✓ Spawn robie [worker] (gpt-5.1 high)".to_string(),
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
                "⠋ Spawn scout".to_string(),
                "  └ 3 tool uses · 9s · reading".to_string(),
                "⠋ Spawn verifier".to_string(),
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
                "⠋ Spawn scout".to_string(),
                "  └ 3 tool uses · 9s · reading".to_string(),
                "⠋ Spawn verifier".to_string(),
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
                exit_code: None,
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
                exit_code: None,
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
        let why = format!("  └ {}", super::super::strings::TOOL_CALL_INTERRUPTED);
        assert_eq!(
            lines,
            vec![
                edge("✗ Edit src/a.rs", "failed", 140),
                why.clone(),
                edge("✗ Edit src/b.rs", "failed", 140),
                why,
            ],
            "each interrupted edit says why"
        );
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
        assert_eq!(lines[0], edge("✓ Create src/a.ts", "+2 -0", 140), "{lines:?}");
        let failed_row = edge("✗ Edit src/b.ts", "failed", 140);
        let at = lines
            .iter()
            .position(|line| *line == failed_row)
            .expect("the refused write stands as a failure");
        let failed = &lines[at + 1];
        assert!(
            failed.starts_with("  └ exists but has not been read in this conversation"),
            "the reason hangs under the path, not the path again: {failed}"
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
        assert_eq!(lines[0], edge("✓ Create src/big.ts", "+60 -0", 120), "{lines:?}");
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
        assert_eq!(lines[0], edge("✓ Edit ui/tokens.css", "+2 -1", 100), "the edited file keeps its counts");
        let failed_row = edge("✗ Edit ui/shell.css", "failed", 100);
        let at = lines.iter().position(|line| *line == failed_row).expect("the failed file stands");
        assert_eq!(
            lines[at + 1], "  └ io error: edit[5] failed: old_string not found in file",
            "the failed file stands with its reason: {lines:?}"
        );
        assert!(!lines.iter().any(|line| line.contains("+0 -0")), "{lines:?}");
    }

    #[test]
    fn a_failed_edit_wears_a_cross_and_keeps_its_reason() {
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
                edge("✗ Edit src/lib.rs", "failed", 80),
                "  └ context mismatch".to_string(),
                "    try again".to_string(),
            ]
        );
    }

    #[test]
    fn consecutive_edits_are_one_row_each_in_call_order() {
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
        assert_eq!(rows[0], edge("✓ Edit b.rs", "+2 -0", 80));
        let a_row = rows.iter().position(|row| *row == edge("✓ Edit a.rs", "+1 -1", 80)).expect("a row");
        assert!(a_row > 0, "each edit is its own row, in the order the calls ran: {rows:?}");
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
            edge("✓ Edit old.rs → new.rs", "+0 -0", 80)
        );
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

    /// A finished command keeps its whole text: what its row cannot show hangs
    /// under the row, four columns in, wrapped at the edge.
    #[test]
    fn a_long_command_keeps_what_its_row_cannot_show() {
        let mut group = ToolGroup::new(bash("a", "alpha beta gamma delta epsilon zeta eta theta"));
        assert!(group.complete("a", ok("")));
        let rows = plain(&group.lines(30, Duration::ZERO));
        assert_eq!(
            rows,
            vec![
                "✓ Run alpha beta gamma delta".to_string(),
                "    epsilon zeta eta theta".to_string(),
                "  └ (no output)".to_string(),
            ]
        );
    }

    /// The lines of a finished multi-line command stay under its row, a blank
    /// line blank, as long as they fit the output's budget, as these do. While
    /// it runs, the row shows the first line and the rows under it are the
    /// live tail's.
    #[test]
    fn a_multi_line_command_keeps_every_line_under_its_row() {
        let script = "python3 - <<'EOF'\nimport sys\n\nprint(sys.version)\nEOF";
        let mut group = ToolGroup::new(bash("a", script));
        assert_eq!(plain(&group.lines(60, Duration::ZERO)), vec!["⠋ Run python3 - <<'EOF'".to_string()]);
        assert!(group.complete("a", ok("3.12.1")));
        assert_eq!(
            plain(&group.lines(60, Duration::ZERO)),
            vec![
                edge("✓ Run python3 - <<'EOF'", "1 line", 60),
                "    import sys".to_string(),
                String::new(),
                "    print(sys.version)".to_string(),
                "    EOF".to_string(),
                "  └ 3.12.1".to_string(),
            ]
        );
    }

    /// The verb is a word, not a column (t-51389, 「ran call 등 도구 옆
    /// 빈공백이」): one space after it, the target right after, the result
    /// still on the last column — and what hangs under a row starts on one
    /// column whatever the verb, the column the output's text starts on.
    #[test]
    fn the_target_follows_its_verb_after_one_space() {
        let mut group = ToolGroup::new(bash("a", "cargo test"));
        assert!(group.complete("a", ok("ok")));
        group.push(read("b", "src/lib.rs"));
        assert_eq!(
            plain(&group.lines(40, Duration::ZERO)),
            vec![
                edge("✓ Run cargo test", "1 line", 40),
                "  └ ok".to_string(),
                "⠋ Read src/lib.rs".to_string(),
            ]
        );
        let finished = |kind: ToolKind| {
            let mut group = ToolGroup::new(ToolCall::new("c".to_string(), kind));
            assert!(group.complete("c", ok("")));
            plain(&group.lines(24, Duration::ZERO))
        };
        assert_eq!(
            finished(ToolKind::Command {
                command: "alpha beta gamma delta epsilon".to_string(),
                background: false,
            }),
            vec![
                "✓ Run alpha beta gamma".to_string(),
                "    delta epsilon".to_string(),
                "  └ (no output)".to_string(),
            ]
        );
        assert_eq!(
            finished(ToolKind::Call {
                name: "Lookup".to_string(),
                detail: "alpha beta gamma delta".to_string(),
            }),
            vec![
                "✓ Call Lookup(alpha beta".to_string(),
                "    gamma delta)".to_string(),
            ]
        );
    }

    /// A long command's text does not fill the screen (t-51389, 「출력부분도」):
    /// under its row it keeps the output's budget — its first rows and a
    /// count of the rest — in the live cell and in a committed one on a bare
    /// terminal, while with fold markers the committed cell keeps every row
    /// inside the fold the person opens.
    #[test]
    fn a_long_heredoc_keeps_to_the_output_budget_under_its_row() {
        use crate::tui::folds::{FoldIds, FoldMode};
        let prints = (1..=40).map(|n| format!("print({n})")).collect::<Vec<_>>();
        let script = format!("python3 - <<'PY'\n{}\nPY", prints.join("\n"));
        let mut group = ToolGroup::new(bash("a", &script));
        assert!(group.complete("a", ok("40")));

        let live = plain(&group.lines(60, Duration::ZERO));
        assert_eq!(
            live,
            vec![
                edge("✓ Run python3 - <<'PY'", "1 line", 60),
                "    print(1)".to_string(),
                "    print(2)".to_string(),
                "    print(3)".to_string(),
                "    print(4)".to_string(),
                "    … +37 lines".to_string(),
                "  └ 40".to_string(),
            ]
        );
        let bare = plain(&group.committed(60, &mut FoldIds::default(), FoldMode::Bare));
        assert_eq!(bare[0], "", "the gap before the cell");
        assert_eq!(bare[1..], live[..], "a bare terminal commits what the live cell drew");

        let marked = plain(&group.committed(60, &mut FoldIds::default(), FoldMode::Markers));
        assert_eq!(marked[2], "  └ … +42 lines", "the fold hides every row under the row");
        assert!(
            marked.contains(&"    print(40)".to_string()) && marked.contains(&"    PY".to_string()),
            "the opened fold has the whole command: {marked:#?}"
        );
        assert_eq!(marked.len(), 45, "the gap, the row, the teaser, 41 rows, the output");
    }

    /// An MCP tool answers with an envelope — `content` items beside
    /// `structuredContent` — and the rows under its call show what the items
    /// say, not the envelope (t-51389): the text, an image as a word, nothing
    /// for an envelope with nothing in it, and for a long one the runtime cut
    /// at its display cap, the text as far as it goes.
    #[test]
    fn an_mcp_call_shows_what_its_envelope_says() {
        let call = |name: &str, raw: &str| {
            let mut group = ToolGroup::new(ToolCall::new(
                "m".to_string(),
                ToolKind::Call { name: name.to_string(), detail: String::new() },
            ));
            let body = format_tool_result_from_raw(name, raw, false);
            assert!(group.complete("m", Outcome::from_result(false, &body, "")));
            plain(&group.lines(80, Duration::ZERO))
        };
        assert_eq!(
            call(
                "mcp__playwright__browser_take_screenshot",
                r####"{"content":[{"type":"text","text":"### Result\nTook the viewport screenshot and saved it as page-1.png"},{"type":"image","data":"iVBORw0KGgo=","mimeType":"image/png"}],"structuredContent":null}"####,
            ),
            vec![
                "✓ Call mcp__playwright__browser_take_screenshot".to_string(),
                "  └ ### Result".to_string(),
                "    Took the viewport screenshot and saved it as page-1.png".to_string(),
                "    [image]".to_string(),
            ]
        );
        assert_eq!(
            call("mcp__playwright__browser_resize", r#"{"content":[],"structuredContent":null}"#),
            vec!["✓ Call mcp__playwright__browser_resize".to_string()]
        );
        let snapshot = (1..=300)
            .map(|n| format!("- generic [ref=e{n}]: row {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let page = json!({
            "content": [{"type": "text", "text": format!("### Page state\n{snapshot}")}],
            "structuredContent": null,
        });
        let rows = call("mcp__playwright__browser_navigate", &page.to_string());
        assert_eq!(rows[1], "  └ ### Page state", "{rows:#?}");
        assert_eq!(rows[2], "    - generic [ref=e1]: row 1", "{rows:#?}");
        assert!(rows[3].starts_with("    … +"), "{rows:#?}");
        assert!(rows.iter().all(|row| !row.contains("\"content\"")), "{rows:#?}");
    }

    /// The cap can cut an envelope's last item before its `type` is printed:
    /// keys print in their own order, and `text` comes before `type` when they
    /// are sorted or when the server sent them so. The text still shows, and
    /// the cap's `… +1 lines` after it (t-51389).
    #[test]
    fn a_cut_envelope_item_shows_its_text_before_its_type_was_printed() {
        let printed = "{\n  \"content\": [\n    {\n      \"text\": \"### Page state\\n- generic [ref=e1]: row 1\\n- generic [ref=e2]: ro";
        let mut group = ToolGroup::new(ToolCall::new(
            "m".to_string(),
            ToolKind::Call { name: "mcp__playwright__browser_navigate".to_string(), detail: String::new() },
        ));
        assert!(group.complete("m", ok(&format!("{printed}\n{}", super::SOURCE_TRUNCATION_NOTE))));
        assert_eq!(
            plain(&group.lines(80, Duration::ZERO)),
            vec![
                "✓ Call mcp__playwright__browser_navigate".to_string(),
                "  └ ### Page state".to_string(),
                "    - generic [ref=e1]: row 1".to_string(),
                "    - generic [ref=e2]: ro".to_string(),
                "    … +1 lines".to_string(),
            ]
        );
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
                edge("✓ Run ls -la", "1 line", 60),
                "  └ … +1 lines".to_string(),
                "  └ total 16".to_string(),
            ]
        );
        // 앞 빈 줄은 구간 밖이다.
        assert_eq!(rows[0].lead, None);
        assert_eq!(
            rows[1].lead.as_deref(),
            Some("\u{1b}]7788;begin;0;collapsed,teaser=1;✓ Run ls -la 1 line\u{1b}\\")
        );
        assert_eq!(rows[3].trail.as_deref(), Some("\u{1b}]7788;end;0\u{1b}\\"));
    }

    /// 맨 터미널에서는 헤더 한 줄뿐이라 손잡이를 달지 않는다 — 접을 본문이 없다.
    #[test]
    fn bare_mode_commits_the_rows_without_fold_markers() {
        use crate::tui::folds::{FoldIds, FoldMode};
        let mut group = ToolGroup::new(bash("a", "ls"));
        assert!(group.complete("a", ok("x")));
        group.push(bash("b", "pwd"));
        assert!(group.complete("b", ok("/tmp")));
        let mut ids = FoldIds::default();
        let rows = group.committed(60, &mut ids, FoldMode::Bare);
        assert_eq!(
            plain(&rows),
            vec![
                String::new(),
                edge("✓ Run ls", "1 line", 60),
                "  └ x".to_string(),
                edge("✓ Run pwd", "1 line", 60),
                "  └ /tmp".to_string(),
            ]
        );
        assert!(rows.iter().all(|row| row.lead.is_none() && row.trail.is_none()));
    }

    /// marker opt-in에서는 같은 묶음이 본문을 달고 접힌다 — 호출마다 `  └ <cmd>` 와
    /// 그 출력이 네 칸 거터로 이어진다.
    #[test]
    fn marker_mode_folds_each_calls_output_under_its_row() {
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
                edge("✓ Run ls", "1 line", 60),
                "  └ … +1 lines".to_string(),
                "  └ x".to_string(),
                edge("✓ Run pwd", "1 line", 60),
                "  └ … +1 lines".to_string(),
                "  └ /tmp".to_string(),
            ]
        );
        // 호출마다 제 줄이 begin 을 물고(접힌 동안 보이는 밀도 줄 하나를 teaser 로
        // 선언), 그 호출의 마지막 본문 줄이 end 를 문다.
        assert_eq!(rows[0].lead, None);
        assert_eq!(
            rows[1].lead.as_deref(),
            Some("\u{1b}]7788;begin;0;collapsed,teaser=1;✓ Run ls 1 line\u{1b}\\")
        );
        assert_eq!(rows[3].trail.as_deref(), Some("\u{1b}]7788;end;0\u{1b}\\"));
        assert_eq!(
            rows[4].lead.as_deref(),
            Some("\u{1b}]7788;begin;1;collapsed,teaser=1;✓ Run pwd 1 line\u{1b}\\")
        );
        assert_eq!(rows[6].trail.as_deref(), Some("\u{1b}]7788;end;1\u{1b}\\"));
    }

    /// 탐색 묶음의 본문은 codex 의 목록 줄 그대로다 — 출력은 원본도 안 보인다.
    #[test]
    fn a_marked_read_row_has_nothing_to_fold() {
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
                edge("✓ Read README.md", "1 line", 60),
                edge("✓ Run pwd", "1 line", 60),
                "  └ … +1 lines".to_string(),
                "  └ /tmp".to_string(),
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
                edge("✓ Run seq", "9 lines", 60),
                "  └ … +5 lines".to_string(),
                "  └ line1".to_string(),
                "    line2".to_string(),
                "    … +5 lines".to_string(),
                "    line8".to_string(),
                "    line9".to_string(),
                edge("✓ Run pwd", "1 line", 60),
                "  └ … +1 lines".to_string(),
                "  └ /tmp".to_string(),
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
        let rows = plain(&group.lines(60, Duration::ZERO));
        assert!(!rows.iter().any(|row| row.contains("… +")), "no teaser in the live cell: {rows:?}");
        assert_eq!(
            rows,
            vec![
                edge("✓ Run ls", "1 line", 60),
                "  └ x".to_string(),
                edge("✓ Run pwd", "1 line", 60),
                "  └ /tmp".to_string(),
            ]
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
                edge("✓ Run seq 8", "8+ lines", 60),
                "  └ line1".to_string(),
                "    line2".to_string(),
                "    … +5 lines".to_string(),
                "    line7".to_string(),
                "    line8".to_string(),
            ]
        );
    }
}

/// One row per call (t-48216): a status glyph, the verb, the target and the
/// call's result at the right edge — never folded into `Ran N commands`.
#[cfg(test)]
mod row_tests {
    use std::time::Duration;

    use runtime::message_stream::{
        BashResult, DiffHunk, DiffLine, DiffLineKind, DiffView, ToolResultBody,
    };

    use super::{Explored, Outcome, ToolCall, ToolGroup, ToolKind};
    use crate::tui::ansi::{Color, Line};

    const WIDTH: usize = 60;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::plain).collect()
    }

    /// `left`, then spaces, then `right` ending on the last column.
    fn edge(left: &str, right: &str, width: usize) -> String {
        let used = left.chars().count() + right.chars().count();
        format!("{left}{}{right}", " ".repeat(width.saturating_sub(used)))
    }

    fn ok(output: &str) -> Outcome {
        let body = ToolResultBody::Text {
            content: output.to_string(),
            truncated: false,
        };
        Outcome::from_result(false, &body, "")
    }

    fn numbered(count: usize) -> String {
        (1..=count)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn command(id: &str, command: &str) -> ToolCall {
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

    #[test]
    fn a_read_and_a_search_are_two_rows_with_their_results_at_the_edge() {
        let mut group = ToolGroup::new(read("a", "src/app.py"));
        group.push(ToolCall::new(
            "b".to_string(),
            ToolKind::Explore(vec![Explored::Search {
                query: "do_(GET|POST)".to_string(),
                path: Some("src".to_string()),
            }]),
        ));
        assert!(group.complete("a", ok(&numbered(12))));
        assert!(group.complete("b", ok("src/app.py:7:    def do_GET(self):")));
        assert_eq!(
            plain(&group.lines(WIDTH, Duration::ZERO)),
            vec![
                edge("✓ Read src/app.py", "12 lines", WIDTH),
                edge("✓ Search do_(GET|POST) in src", "1 match", WIDTH),
            ]
        );
    }

    #[test]
    fn two_finished_commands_stay_two_rows_never_ran_two_commands() {
        let mut group = ToolGroup::new(command("a", "echo one"));
        group.push(command("b", "echo two"));
        assert!(group.complete("a", ok("one")));
        assert!(group.complete("b", ok("two")));
        let rows = plain(&group.lines(WIDTH, Duration::ZERO));
        assert!(rows.iter().all(|row| !row.contains("Ran 2")), "{rows:#?}");
        assert_eq!(
            rows,
            vec![
                edge("✓ Run echo one", "1 line", WIDTH),
                "  └ one".to_string(),
                edge("✓ Run echo two", "1 line", WIDTH),
                "  └ two".to_string(),
            ]
        );
    }

    #[test]
    fn a_failed_command_wears_a_cross_and_names_its_exit_code() {
        let mut group = ToolGroup::new(command("a", "cargo test"));
        let body = ToolResultBody::Bash(BashResult {
            exit_code: 101,
            stdout: String::new(),
            stderr: "error: test failed".to_string(),
            truncated: false,
        });
        assert!(group.complete("a", Outcome::from_result(false, &body, "")));
        let rows = plain(&group.lines(WIDTH, Duration::ZERO));
        assert_eq!(rows[0], edge("✗ Run cargo test", "exit 101", WIDTH));
        assert_eq!(rows[1], "  └ error: test failed");
    }

    #[test]
    fn a_running_call_spins_on_its_own_row() {
        let group = ToolGroup::new(read("a", "README.md"));
        let rows = plain(&group.lines(WIDTH, Duration::from_millis(250)));
        assert_eq!(rows.len(), 1, "{rows:#?}");
        let mut chars = rows[0].chars();
        let glyph = chars.next().expect("a status glyph");
        assert!(
            ('\u{2800}'..='\u{28ff}').contains(&glyph),
            "a running call spins a braille frame, got {glyph:?}"
        );
        assert_eq!(chars.as_str().trim_end(), " Read README.md");
    }

    #[test]
    fn the_spinner_advances_with_time() {
        let group = ToolGroup::new(read("a", "README.md"));
        let frames: std::collections::HashSet<char> = (0..12)
            .filter_map(|step| {
                let rows = plain(&group.lines(WIDTH, Duration::from_millis(step * 100)));
                rows[0].chars().next()
            })
            .collect();
        assert!(frames.len() > 1, "one frame forever is not a spinner: {frames:?}");
    }

    #[test]
    fn an_edit_row_counts_its_change_at_the_edge_and_hangs_its_diff_under_the_target() {
        let view = DiffView {
            old_path: Some("src/app.py".to_string()),
            new_path: Some("src/app.py".to_string()),
            language: None,
            hunks: vec![DiffHunk {
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 2,
                lines: vec![
                    DiffLine {
                        kind: DiffLineKind::Added,
                        text: "import json".to_string(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Context,
                        text: "TODOS = []".to_string(),
                    },
                ],
            }],
        };
        let mut group = ToolGroup::new(ToolCall::new(
            "e".to_string(),
            ToolKind::Edit {
                path: "src/app.py".to_string(),
            },
        ));
        assert!(group.complete(
            "e",
            Outcome::from_result(false, &ToolResultBody::Diff(view), "src/app.py")
        ));
        let lines = group.lines(WIDTH, Duration::ZERO);
        assert_eq!(
            plain(&lines),
            vec![
                edge("✓ Edit src/app.py", "+1 -0", WIDTH),
                "       1 + import json".to_string(),
                "    1  2   TODOS = []".to_string(),
            ]
        );
        let added = lines[0]
            .spans
            .iter()
            .find(|span| span.text == "+1")
            .expect("the added count");
        let removed = lines[0]
            .spans
            .iter()
            .find(|span| span.text == "-0")
            .expect("the removed count");
        assert_eq!(added.style.fg, Some(Color::GREEN));
        assert_eq!(removed.style.fg, Some(Color::RED));
    }

    #[test]
    fn each_edit_in_a_group_hangs_its_own_diff() {
        fn added(path: &str, texts: &[&str]) -> ToolResultBody {
            ToolResultBody::Diff(DiffView {
                old_path: Some(path.to_string()),
                new_path: Some(path.to_string()),
                language: None,
                hunks: vec![DiffHunk {
                    old_start: 1,
                    old_lines: 0,
                    new_start: 1,
                    new_lines: u32::try_from(texts.len()).expect("a short hunk"),
                    lines: texts
                        .iter()
                        .map(|text| DiffLine {
                            kind: DiffLineKind::Added,
                            text: (*text).to_string(),
                        })
                        .collect(),
                }],
            })
        }
        let edit = |id: &str, path: &str| {
            ToolCall::new(id.to_string(), ToolKind::Edit { path: path.to_string() })
        };
        let mut group = ToolGroup::new(edit("a", "alpha.txt"));
        group.push(edit("b", "beta.txt"));
        let alpha = added("alpha.txt", &["alpha one", "alpha two"]);
        let beta = added("beta.txt", &["beta one"]);
        assert!(group.complete("a", Outcome::from_result(false, &alpha, "alpha.txt")));
        assert!(group.complete("b", Outcome::from_result(false, &beta, "beta.txt")));
        assert_eq!(
            plain(&group.lines(WIDTH, Duration::ZERO)),
            vec![
                edge("✓ Edit alpha.txt", "+2 -0", WIDTH),
                "       1 + alpha one".to_string(),
                "       2 + alpha two".to_string(),
                edge("✓ Edit beta.txt", "+1 -0", WIDTH),
                "       1 + beta one".to_string(),
            ]
        );
    }

    #[test]
    fn a_path_inside_the_working_directory_is_shown_relative_to_it() {
        let absolute = std::env::current_dir()
            .expect("a working directory")
            .join("src")
            .join("app.py");
        let mut group = ToolGroup::new(read("a", &absolute.display().to_string()));
        assert!(group.complete("a", ok("x")));
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            plain(&group.lines(WIDTH, Duration::ZERO))[0],
            edge(&format!("✓ Read src{sep}app.py"), "1 line", WIDTH)
        );
    }

    #[test]
    fn a_running_command_shows_the_tail_of_its_live_output_and_names_itself_once() {
        let handle = runtime::live_output::register(Some("toolu_row_tail"));
        handle
            .writer()
            .append_stdout(b"test_create ... ok\ntest_list ... ok\ntest_delete ... ok\ntest_update ... ok\n");
        let group = ToolGroup::new(command(
            "toolu_row_tail",
            "for t in create list delete update; do echo test_$t ... ok; done",
        ));
        let rows = plain(&group.lines(100, Duration::from_secs(7)));
        assert_eq!(
            rows.iter().filter(|row| row.contains("for t in create")).count(),
            1,
            "{rows:#?}"
        );
        assert!(rows.iter().any(|row| row == "  │ test_delete ... ok"), "{rows:#?}");
        assert!(rows.iter().any(|row| row == "  │ test_update ... ok"), "{rows:#?}");
        drop(handle);
    }
}
