//! `/resume` 피커 — codex `tui/src/resume_picker.rs` 를 우리 데이터로 옮긴 것.
//!
//! 정본은 `/private/tmp/codex-src`(rust-v0.150.1)의 `resume_picker.rs`(6,911줄)
//! 와 캡처 `docs/captures/codex-tui-v0.149.1-resume-picker.bin`(29.19s 프레임)
//! 이다. 그 화면은 alt-screen 오버레이(`ESC[?1049h`)이고 zo 에는 alt-screen 도
//! pager 도 없어서(모듈 머리말의 화면 관리 계약) **뷰포트**를 통째로 쓰는
//! 자리에 같은 문법으로 앉힌다. 캡처가 증언하는 뼈대:
//!
//! ```text
//! ESC[?1049h ESC[?1007h ESC[1;1H ESC[J
//! ESC[1;2H  ESC[1m ESC[38;5;6;49m Resume a previous session
//! ESC[3;2H  ESC[22m ESC[2m ESC[2m ESC[39;49m Type to search
//! ESC[3;46H Filter: [Cwd] All    Status: [Active] Archived    Sort: [Updated] Created
//! ESC[5;3H  ESC[3m Loading sessions…
//! ESC[37;1H ESC[23m ──…── 0 / 0… · 0% ─
//! ESC[38;1H  enter resume   esc exit   ctrl+c exit   tab focus sort/filter   ←/→ change option
//! ESC[39;1H  ctrl+o comfortable view   ctrl+t transcript   ctrl+e expand   ↑/↓ browse
//! ```
//!
//! `draw_picker` 의 세로 배치는 header(1) · gap(1) · search(1) · gap(1) ·
//! list(나머지) · footer(4) 이고 `PICKER_CHROME_HEIGHT = 8` 이 그 합이다.
//! 우리는 그 앞에 zo 뷰포트 피커의 관례인 빈 줄 둘을 두고, footer 는 네 줄 중
//! 실제로 그려지는 셋(구분선 + 힌트 두 줄)만 쓴다 — codex 의 넷째 줄은 고정
//! 높이 `Constraint::Length(4)` 가 남긴 빈 자리이지 그려지는 행이 아니다.
//!
//! # 우리 데이터로 성립하는 것만 옮겼다
//!
//! `ResumeSession` 이 드는 것은 id · 경로 · 수정 시각 · cwd · 첫 프롬프트 ·
//! 메시지 수다. 그 위에서:
//!
//! | codex | 여기 | 왜 |
//! | --- | --- | --- |
//! | 검색(`Type to search`) | 옮김 | `matches_query` 의 다섯 축 중 셋(첫 프롬프트·id·cwd)이 우리 것이다 |
//! | `Filter: [Cwd] All` | 옮김 | 목록은 워크스페이스로 좁혀지지 않는다(`session_search_dirs` 가 전역 프로젝트 디렉터리들도 훑는다) — 그래서 이 토글이 실제로 행을 바꾼다 |
//! | `Sort: [Updated] Created` | 옮김 | 생성 시각은 트랜스크립트 머리글에 이미 있다(`{"type":"session_meta",…,"created_at_ms":…}` → `Session::created_at_ms`). 실측: 실제 세션 파일들에서 그것은 언제나 mtime 이하이고 오래 산 세션은 37,001초까지 벌어진다 — 두 정렬이 정말 다른 순서를 낸다 |
//! | 스크롤 퍼센트 구분선 | 옮김 | `picker_footer_scroll_percent` 그대로 |
//! | `ctrl+o` 밀도 전환 | 옮김 | comfortable 의 메타 줄은 날짜와 cwd 다 |
//! | `Status: [Active] Archived` | **안 옮김** | zo 에 보관(archive) 개념이 없다. 두 값 중 하나가 언제나 빈 목록인 토글은 컨트롤이 아니라 거짓말이다 |
//! | 페이지네이션 | **안 옮김** | codex 는 app-server 에서 페이지를 더 받아 오고 우리는 디스크에서 한 번에 스무 개를 읽는다(`resume::DEFAULT_LIST_LIMIT`). 화면에 보이는 부분(`N / M · P%`·`↑ more`·`↓ more`)은 옮겼다 |
//! | `ctrl+t`/`ctrl+e` 트랜스크립트 미리보기 | **안 옮김** | codex 에서 이 둘은 한 기능이다 — `toggle_selected_expansion` 이 상세 블록을 펴면서 `PickerLoadRequest::Preview` 로 대화를 **비동기로** 불러온다. 우리 피커에는 그 로더 자리가 없고, `ctrl+t` 는 이미 zo 의 트랜스크립트 페이저에 묶여 있다. 상세 블록만 옮기면 `Conversation:` 이라 적어 놓고 그 아래가 늘 비는 화면이 된다 |
//! | 세션 이름으로 재개 | **안 옮김** | `SessionHandle` 은 id 와 경로뿐이라 `resume_hint_for_resumable_thread` 의 `<name> (<id>)` 갈래가 성립하지 않는다 |
//!
//! 행 배경(zebra·선택)은 codex처럼 OSC로 측정한 터미널 배경색이 있을 때만
//! 낸다. 어두운 배경의 zebra는 흰색 5.5%, 선택 행은 12%를 섞고, 밝은
//! 배경에서는 검정을 각각 4%/12% 섞는다.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use unicode_width::UnicodeWidthStr;

use crate::resume::ResumeSession;

use super::ansi::{Color, Line, Span, Style};
use super::palette;
use super::paths::center_truncate_path;

/// codex `SESSION_META_DATE_WIDTH`.
const DATE_WIDTH: usize = 12;
/// codex `SESSION_META_INDENT_WIDTH`.
const META_INDENT: usize = 2;
/// codex `SESSION_META_FIELD_GAP_WIDTH`.
const META_GAP: usize = 2;
/// codex `SESSION_META_MIN_CWD_WIDTH` · `SESSION_META_MAX_CWD_WIDTH`.
const MIN_CWD_WIDTH: usize = 30;
const MAX_CWD_WIDTH: usize = 72;
/// codex `SESSION_META_CWD_ICON`.
const CWD_ICON: &str = "⌁";
/// codex `FOOTER_HINT_LEFT_PADDING` · `FOOTER_HINT_GAP`.
const HINT_LEFT_PADDING: usize = 1;
const HINT_GAP: usize = 3;
/// codex `FOOTER_COMPACT_BREAKPOINT`.
const HINT_COMPACT_BREAKPOINT: usize = 120;
/// 목록 앞의 들여쓰기 — codex 는 `list.x + 2` 로 리스트 영역을 밀어 둔다.
const LIST_INDENT: &str = "  ";

/// 목록 말고 화면이 쓰는 행 수: 빈 줄 둘 · 제목 · gap · 검색 · gap ·
/// 구분선 · 힌트 두 줄.
const CHROME_HEIGHT: usize = 9;

/// codex `SessionFilterMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    Cwd,
    All,
}

impl FilterMode {
    /// codex `SessionFilterMode::toggle` — 걸 cwd 가 없으면 토글도 없다.
    const fn toggle(self, filter_cwd: Option<&PathBuf>) -> Self {
        if filter_cwd.is_none() {
            return Self::All;
        }
        match self {
            Self::Cwd => Self::All,
            Self::All => Self::Cwd,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Cwd => "Cwd",
            Self::All => "All",
        }
    }
}

/// codex `ThreadSortKey` 중 우리 데이터로 성립하는 둘.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    UpdatedAt,
    CreatedAt,
}

impl SortKey {
    const fn toggle(self) -> Self {
        match self {
            Self::UpdatedAt => Self::CreatedAt,
            Self::CreatedAt => Self::UpdatedAt,
        }
    }

    /// codex `sort_key_label`.
    const fn label(self) -> &'static str {
        match self {
            Self::UpdatedAt => "Updated",
            Self::CreatedAt => "Created",
        }
    }
}

/// codex `SessionListDensity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Density {
    Dense,
    Comfortable,
}

impl Density {
    const fn toggle(self) -> Self {
        match self {
            Self::Dense => Self::Comfortable,
            Self::Comfortable => Self::Dense,
        }
    }

    /// codex `row_separator_height` — comfortable 만 행 사이를 한 줄 띄운다.
    const fn separator_height(self) -> usize {
        match self {
            Self::Dense => 0,
            Self::Comfortable => 1,
        }
    }
}

/// codex `ToolbarControl` — `Status` 를 뺀 둘.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolbarControl {
    Filter,
    Sort,
}

impl ToolbarControl {
    const fn next(self) -> Self {
        match self {
            Self::Filter => Self::Sort,
            Self::Sort => Self::Filter,
        }
    }
}

/// 피커에 실을 세션 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    /// `PlainSession::open` 에 넘길 세션 id.
    pub id: String,
    /// codex `Row::cwd` — comfortable 메타 줄과 `Filter: [Cwd]` 의 기준.
    pub cwd: Option<PathBuf>,
    /// codex `Row::preview`(→ `display_preview`) — 세션의 첫 프롬프트 한 줄.
    pub preview: String,
    /// codex `Row::created_at` — 트랜스크립트 머리글의 `created_at_ms`.
    pub created_millis: u128,
    /// codex `Row::updated_at` — 트랜스크립트 파일의 수정 시각.
    pub updated_millis: u128,
    /// Another zo holds this session's writer lease right now — who, as a
    /// clause (`zo pid 96468 in ~/…, for 27 min`). Picking it would only be
    /// refused; the row says so before the person does.
    pub held_by: Option<String>,
}

impl SessionRow {
    /// codex `Row::matches_query` 의 우리 축들. 원본은 첫 프롬프트·스레드
    /// 이름·스레드 id·git 브랜치·cwd 다섯을 훑고, 우리는 이름과 브랜치가
    /// 없으니 셋이다. 소문자 비교도 원본 그대로.
    fn matches_query(&self, query: &str) -> bool {
        if self.preview.to_lowercase().contains(query) {
            return true;
        }
        if self.id.to_lowercase().contains(query) {
            return true;
        }
        self.cwd.as_ref().is_some_and(|cwd| {
            cwd.to_string_lossy().to_lowercase().contains(query)
        })
    }

    /// 정렬·표시에 쓸 시각 — codex `render_dense_session_lines` 의
    /// `match state.sort_key { CreatedAt => created, _ => updated }`.
    const fn sort_millis(&self, key: SortKey) -> u128 {
        match key {
            SortKey::UpdatedAt => self.updated_millis,
            SortKey::CreatedAt => self.created_millis,
        }
    }
}

/// codex `resume_picker.rs::format_relative_time` 그대로. 밀리초를 초로 줄여
/// 비교하므로 `reference` 가 `ts` 보다 이르면 `now` 다(원본의 `.max(0)`).
#[must_use]
pub fn relative_time(reference_millis: u128, ts_millis: u128) -> String {
    let seconds = reference_millis.saturating_sub(ts_millis) / 1_000;
    if seconds == 0 {
        return "now".to_string();
    }
    if seconds < 60 {
        return format!("{seconds}s ago");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    format!("{}d ago", hours / 24)
}

/// 첫 프롬프트를 한 줄로. 개행·연속 공백을 한 칸으로 줄이고 비면 codex 의
/// 빈 미리보기 자리(`Row::preview` 가 빈 문자열일 때의 대체)를 쓴다.
#[must_use]
pub fn one_line(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        "(no prompt)".to_string()
    } else {
        joined
    }
}

/// codex `status/helpers.rs::format_directory_display` — 홈 아래면 `~/…`.
#[must_use]
pub fn directory_display(directory: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if let Ok(rest) = directory.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display());
        }
    }
    directory.display().to_string()
}

/// A held session's mark on its title line — the one place both densities
/// show, so the dense list says it too.
const HELD_ICON: &str = "●";
fn held_title(row: &SessionRow) -> String {
    if row.held_by.is_some() {
        format!("{HELD_ICON} {}", row.preview)
    } else {
        row.preview.clone()
    }
}

/// 세션 목록을 피커 행으로. 순서는 목록 그대로다 —
/// `list_recent_sessions_limited` 가 이미 최신순이고, codex 의 기본 정렬
/// (`Sort: [Updated]`)도 그렇다.
#[must_use]
pub fn rows(sessions: &[ResumeSession]) -> Vec<SessionRow> {
    sessions
        .iter()
        .map(|session| SessionRow {
            created_millis: session.created_epoch_millis,
            id: session.id.clone(),
            cwd: session.cwd.clone(),
            preview: one_line(session.first_prompt.as_deref().unwrap_or_default()),
            updated_millis: session.modified_epoch_millis,
            held_by: core_types::session::writer_lease_holder(&session.path)
                .map(|holder| holder.said()),
        })
        .collect()
}

/// 지금 시각(epoch 밀리초) — 목록의 상대 시각 기준점.
#[must_use]
pub fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis())
}

/// 떠 있는 `/resume` 화면 — codex `PickerState` 중 우리 것.
#[derive(Debug, Clone)]
pub struct SessionPicker {
    all: Vec<SessionRow>,
    /// `all` 안 색인들. 필터와 검색을 통과한 행만, 정렬된 순서로.
    filtered: Vec<usize>,
    query: String,
    filter: FilterMode,
    sort: SortKey,
    density: Density,
    focus: ToolbarControl,
    /// `Filter: [Cwd]` 의 기준 — 이 창의 작업 디렉터리.
    filter_cwd: Option<PathBuf>,
    reference_millis: u128,
    /// `filtered` 안 색인.
    selected: usize,
    /// `filtered` 안 색인. 창 높이는 그리는 순간에야 알기 때문에
    /// `ensure_selected_visible` 이 그때 잡는다 — `Cell` 인 이유이고,
    /// `EffortEffect::started_at` 이 같은 이유로 `Cell` 이다.
    scroll_top: Cell<usize>,
}

impl SessionPicker {
    /// codex 의 기본값으로 연다: `Filter: [Cwd]`(걸 cwd 가 있을 때),
    /// `Sort: [Updated]`, dense.
    #[must_use]
    pub fn new(all: Vec<SessionRow>, filter_cwd: Option<PathBuf>, reference_millis: u128) -> Self {
        let filter = if filter_cwd.is_some() {
            FilterMode::Cwd
        } else {
            FilterMode::All
        };
        let mut picker = Self {
            all,
            filtered: Vec::new(),
            query: String::new(),
            filter,
            sort: SortKey::UpdatedAt,
            density: Density::Dense,
            focus: ToolbarControl::Filter,
            filter_cwd,
            reference_millis,
            selected: 0,
            scroll_top: Cell::new(0),
        };
        picker.apply_filter();
        picker
    }

    /// 지금 고른 세션의 id.
    #[must_use]
    pub fn selected_id(&self) -> Option<&str> {
        self.filtered
            .get(self.selected)
            .and_then(|index| self.all.get(*index))
            .map(|row| row.id.as_str())
    }

    /// 지금 목록에 남은 세션들의 id — 필터·검색·정렬을 통과한 순서 그대로.
    ///
    /// 화면이 아니라 **무엇이 남았는가**를 묻는 자리다: 크롬만 그리고 행은
    /// 그대로인 툴바는 옮긴 것이 아니므로, 핀이 그걸 물을 수 있어야 한다.
    #[must_use]
    pub fn visible_ids(&self) -> Vec<&str> {
        self.filtered
            .iter()
            .map(|index| self.all[*index].id.as_str())
            .collect()
    }

    /// 검색어가 있으면 참 — `esc` 가 나가기 전에 먼저 지울 것인지 가른다
    /// (codex 의 `esc_label`: 검색어가 있으면 `clear search`).
    #[must_use]
    pub fn has_query(&self) -> bool {
        !self.query.is_empty()
    }

    pub fn push_char(&mut self, ch: char) {
        self.query.push(ch);
        self.selected = 0;
        self.apply_filter();
    }

    pub fn backspace(&mut self) {
        if self.query.pop().is_some() {
            self.selected = 0;
            self.apply_filter();
        }
    }

    /// codex `clear_query_preserving_selection` — 고른 행을 id 로 다시 찾는다.
    pub fn clear_query(&mut self) {
        let selected_id = self.selected_id().map(ToOwned::to_owned);
        self.query.clear();
        self.apply_filter();
        self.reselect(selected_id.as_deref());
    }

    /// 목록을 다시 만든 뒤 같은 세션으로 커서를 돌려놓는다 — codex
    /// `clear_query_preserving_selection` 이 `seen_key` 로 하는 일이다.
    fn reselect(&mut self, id: Option<&str>) {
        let Some(id) = id else { return };
        if let Some(index) = self
            .filtered
            .iter()
            .position(|index| self.all[*index].id == id)
        {
            self.selected = index;
        }
    }

    pub fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn down(&mut self) {
        self.selected = (self.selected + 1).min(self.filtered.len().saturating_sub(1));
    }

    /// codex `focus_next_toolbar_control`(tab).
    pub fn focus_next(&mut self) {
        self.focus = self.focus.next();
    }

    /// codex `change_focused_toolbar_value`(←/→).
    pub fn change_option(&mut self) {
        match self.focus {
            ToolbarControl::Filter => {
                let next = self.filter.toggle(self.filter_cwd.as_ref());
                if next != self.filter {
                    self.filter = next;
                    self.selected = 0;
                    self.apply_filter();
                }
            }
            ToolbarControl::Sort => {
                // 정렬을 바꿔도 사람이 보던 세션은 그대로 있어야 한다 —
                // 바뀌는 것은 그 세션이 목록의 어디에 있는가다.
                let selected_id = self.selected_id().map(ToOwned::to_owned);
                self.sort = self.sort.toggle();
                self.apply_filter();
                self.reselect(selected_id.as_deref());
            }
        }
    }

    /// codex `toggle_density`(ctrl+o). 원본은 이 값을 `config.toml` 에
    /// 남기지만(`persist_density`) 우리는 세션 안에서만 든다 — 설정 파일에
    /// 새 열쇠를 만드는 것은 이 잔여가 아니다.
    pub fn toggle_density(&mut self) {
        self.density = self.density.toggle();
    }

    /// codex `apply_filter` — 필터, 그다음 검색, 그다음 정렬.
    fn apply_filter(&mut self) {
        let query = self.query.to_lowercase();
        let mut filtered: Vec<usize> = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, row)| self.row_matches_filter(row))
            .filter(|(_, row)| query.is_empty() || row.matches_query(&query))
            .map(|(index, _)| index)
            .collect();
        let sort = self.sort;
        // 최신이 위. 시각이 같으면 id 로 갈라 순서를 결정적으로 둔다
        // (`recent_session_candidates` 의 경로 tiebreak 과 같은 이유).
        filtered.sort_by(|left, right| {
            let left = &self.all[*left];
            let right = &self.all[*right];
            right
                .sort_millis(sort)
                .cmp(&left.sort_millis(sort))
                .then_with(|| right.id.cmp(&left.id))
        });
        self.filtered = filtered;
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
        if self.filtered.is_empty() {
            self.scroll_top.set(0);
        }
    }

    /// codex `row_matches_filter`.
    fn row_matches_filter(&self, row: &SessionRow) -> bool {
        if self.filter == FilterMode::All {
            return true;
        }
        let Some(filter_cwd) = self.filter_cwd.as_ref() else {
            return true;
        };
        let Some(row_cwd) = row.cwd.as_ref() else {
            return false;
        };
        paths_match(row_cwd, filter_cwd)
    }

    /// 한 행이 먹는 줄 수 — dense 는 하나, comfortable 은 제목 + 메타 줄들.
    fn row_height(&self, row: &SessionRow, width: usize) -> usize {
        match self.density {
            Density::Dense => 1,
            Density::Comfortable => 1 + self.meta_lines(row, width).len(),
        }
    }

    /// codex `rendered_height_between`.
    fn height_between(&self, start: usize, end_inclusive: usize, width: usize) -> usize {
        let Some(slice) = self.filtered.get(start..=end_inclusive) else {
            return 0;
        };
        slice
            .iter()
            .map(|index| self.row_height(&self.all[*index], width))
            .sum::<usize>()
            + self.density.separator_height() * end_inclusive.saturating_sub(start)
    }

    /// codex `available_content_rows` — `↑ more`/`↓ more` 가 먹는 줄을 뺀다.
    fn content_rows(&self, viewport: usize) -> usize {
        viewport
            .saturating_sub(usize::from(self.scroll_top.get() > 0))
            .saturating_sub(usize::from(self.selected + 1 < self.filtered.len()))
            .max(1)
    }

    /// codex `ensure_selected_visible`. 그리기 직전에 부른다 — 창 높이는
    /// 그때에야 안다.
    fn ensure_selected_visible(&self, viewport: usize, width: usize) {
        if self.filtered.is_empty() {
            self.scroll_top.set(0);
            return;
        }
        if self.selected < self.scroll_top.get() {
            self.scroll_top.set(self.selected);
        }
        while self.height_between(self.scroll_top.get(), self.selected, width)
            > self.content_rows(viewport)
            && self.scroll_top.get() < self.selected
        {
            self.scroll_top.set(self.scroll_top.get() + 1);
        }
    }

    /// codex `has_more_below`.
    fn has_more_below(&self, viewport: usize, width: usize) -> bool {
        if self.filtered.is_empty() {
            return false;
        }
        let capacity = self.content_rows(viewport);
        let mut used = 0usize;
        for (offset, index) in self.filtered[self.scroll_top.get()..].iter().enumerate() {
            let height = self.row_height(&self.all[*index], width);
            let separator = usize::from(offset > 0) * self.density.separator_height();
            if used + separator + height > capacity {
                return true;
            }
            used += separator + height;
        }
        false
    }

    /// 화면 전체.
    #[must_use]
    pub fn lines(&self, width: usize, max_rows: usize) -> Vec<Line> {
        let list_rows = max_rows.saturating_sub(CHROME_HEIGHT).max(1);
        self.ensure_selected_visible(list_rows, width);

        let mut out = vec![Line::empty(), Line::empty()];
        // 캡처 1행: `ESC[1m ESC[38;5;6;49m Resume a previous session`.
        out.push(
            Line::new(vec![
                Span::raw(LIST_INDENT),
                Span::new(
                    "Resume a previous session",
                    Style::new().bold().fg(palette::COMMAND_TOKEN),
                ),
            ])
            .truncated(width),
        );
        out.push(Line::empty());
        out.push(self.search_line(width));
        out.push(Line::empty());
        out.extend(self.list_lines(width, list_rows));
        out.push(self.separator_line(width, list_rows));
        out.extend(self.hint_lines(width));
        out
    }

    /// codex `search_line` — 왼쪽에 검색, 오른쪽에 툴바, 사이는 최소 두 칸.
    fn search_line(&self, width: usize) -> Line {
        let inner = width.saturating_sub(META_INDENT);
        let (search_text, search_style) = if self.query.is_empty() {
            ("Type to search".to_string(), Style::new().dim())
        } else {
            (format!("Search: {}", self.query), Style::new())
        };
        let toolbar = self.toolbar_spans();
        let toolbar_width: usize = toolbar.iter().map(Span::width).sum();
        let search_width = UnicodeWidthStr::width(search_text.as_str());
        let spacer = inner
            .saturating_sub(search_width + toolbar_width)
            .max(META_GAP);
        let mut spans = vec![
            Span::raw(LIST_INDENT),
            Span::new(search_text, search_style),
            Span::raw(" ".repeat(spacer)),
        ];
        spans.extend(toolbar);
        Line::new(spans).truncated(width)
    }

    /// codex `toolbar_line` — `Status` 컨트롤만 빠진 것.
    fn toolbar_spans(&self) -> Vec<Span> {
        let mut spans = Vec::new();
        let filter_focused = self.focus == ToolbarControl::Filter;
        if self.filter_cwd.is_none() {
            // codex `filter_control_spans` 의 compact 갈래: 걸 cwd 가 없으면
            // 값 하나만 낸다.
            spans.push(Span::dim("Filter:"));
            spans.push(toolbar_value(self.filter.label(), true, filter_focused));
        } else {
            spans.push(Span::dim("Filter: "));
            spans.push(toolbar_value(
                FilterMode::Cwd.label(),
                self.filter == FilterMode::Cwd,
                filter_focused,
            ));
            spans.push(toolbar_value(
                FilterMode::All.label(),
                self.filter == FilterMode::All,
                filter_focused,
            ));
        }
        spans.push(Span::dim("   "));
        let sort_focused = self.focus == ToolbarControl::Sort;
        spans.push(Span::dim("Sort: "));
        spans.push(toolbar_value(
            SortKey::UpdatedAt.label(),
            self.sort == SortKey::UpdatedAt,
            sort_focused,
        ));
        spans.push(toolbar_value(
            SortKey::CreatedAt.label(),
            self.sort == SortKey::CreatedAt,
            sort_focused,
        ));
        spans
    }

    /// codex `render_list`.
    fn list_lines(&self, width: usize, viewport: usize) -> Vec<Line> {
        if self.filtered.is_empty() {
            // codex `render_empty_state_line`.
            let text = if self.query.is_empty() {
                "No sessions yet"
            } else {
                "No results for your search"
            };
            let mut out = vec![
                Line::new(vec![
                    Span::raw(LIST_INDENT),
                    Span::new(text, Style::new().italic().dim()),
                ])
                .truncated(width),
            ];
            out.resize(viewport, Line::empty());
            return out;
        }

        let more_above = self.scroll_top.get() > 0;
        let more_below = self.has_more_below(viewport, width);
        let capacity = viewport
            .saturating_sub(usize::from(more_above))
            .saturating_sub(usize::from(more_below));
        let mut out = Vec::with_capacity(viewport);
        if more_above {
            out.push(Line::new(vec![Span::raw(LIST_INDENT), Span::dim("↑ more")]));
        }

        let mut used = 0usize;
        for (offset, index) in self.filtered[self.scroll_top.get()..].iter().enumerate() {
            if used >= capacity {
                break;
            }
            let row = &self.all[*index];
            let row_index = self.scroll_top.get() + offset;
            let selected = row_index == self.selected;
            let is_zebra = row_index.is_multiple_of(2);
            for line in self.row_lines(
                row,
                selected,
                is_zebra,
                width,
                palette::terminal_palette(),
            ) {
                if used >= capacity {
                    break;
                }
                out.push(line);
                used += 1;
            }
            if self.density == Density::Comfortable
                && used < capacity
                && self.scroll_top.get() + offset + 1 < self.filtered.len()
            {
                out.push(Line::empty());
                used += 1;
            }
        }
        while used < capacity {
            out.push(Line::empty());
            used += 1;
        }
        if more_below {
            out.push(Line::new(vec![Span::raw(LIST_INDENT), Span::dim("↓ more")]));
        }
        out
    }

    /// codex `render_session_lines`.
    fn row_lines(
        &self,
        row: &SessionRow,
        selected: bool,
        is_zebra: bool,
        width: usize,
        terminal_palette: Option<palette::TerminalPalette>,
    ) -> Vec<Line> {
        let marker = if selected {
            // codex `selection_marker`: `"❯ "` 를 선택 스타일 + bold 로.
            Span::new("❯ ", selected_style().bold())
        } else {
            Span::raw("  ")
        };
        let title_style = if selected {
            selected_style()
        } else {
            Style::new()
        };
        let date = relative_time(self.reference_millis, row.sort_millis(self.sort));
        let mut lines = match self.density {
            Density::Dense => {
                // codex `dense_summary_line`: marker · date(dim, 열 폭) · title.
                let available = width.saturating_sub(META_INDENT + marker.width());
                let title_width = available.saturating_sub(DATE_WIDTH);
                vec![
                    Line::new(vec![
                        Span::raw(LIST_INDENT),
                        marker,
                        Span::dim(column_text(&date, DATE_WIDTH)),
                        Span::new(column_text(&held_title(row), title_width), title_style),
                    ])
                    .truncated(width),
                ]
            }
            Density::Comfortable => {
                // codex `render_comfortable_session_lines`: 제목 줄 + 메타 줄들.
                let title_width = width.saturating_sub(META_INDENT + marker.width());
                let mut out = vec![
                    Line::new(vec![
                        Span::raw(LIST_INDENT),
                        marker,
                        Span::new(truncate(&held_title(row), title_width), title_style),
                    ])
                    .truncated(width),
                ];
                out.extend(self.meta_lines(row, width));
                out
            }
        };
        if selected || is_zebra {
            let style = row_background_style(terminal_palette, selected);
            if selected || style.bg.is_some() {
                for line in &mut lines {
                    apply_row_style(line, style, width);
                }
            }
        }
        lines
    }

    /// codex `render_footer_lines` — 날짜와, `Filter: [All]` 일 때 cwd.
    ///
    /// 원본은 브랜치 조각도 늘 싣고 없으면 `no branch` 라 적는데, 우리
    /// `ResumeSession` 에는 브랜치 자리가 아예 없다. 채우지 않는 열을 매 행마다
    /// "없음" 이라고 적는 것은 옮기는 게 아니라 늘리는 것이라 조각째 뺐다.
    fn meta_lines(&self, row: &SessionRow, width: usize) -> Vec<Line> {
        let date = relative_time(self.reference_millis, row.sort_millis(self.sort));
        let mut parts: Vec<(Option<&str>, String)> = vec![(None, date)];
        if self.filter == FilterMode::All {
            let cwd = row
                .cwd
                .as_deref()
                .map_or_else(|| "no cwd".to_string(), directory_display);
            parts.push((Some(CWD_ICON), cwd));
        }
        if let Some(held_by) = row.held_by.as_deref() {
            parts.push((Some(HELD_ICON), format!("open in another zo: {held_by}")));
        }
        let cwd_width = cwd_column_width(width);
        let mut spans = vec![Span::raw(LIST_INDENT)];
        for (index, (icon, text)) in parts.iter().enumerate() {
            if index > 0 {
                spans.push(Span::dim(" ".repeat(META_GAP)));
            }
            let padded = index + 1 < parts.len();
            let target = match (icon, padded) {
                (None, true) => Some(DATE_WIDTH),
                (Some(_), true) => Some(cwd_width),
                _ => None,
            };
            let mut used = 0usize;
            if let Some(icon) = icon {
                spans.push(Span::dim(*icon));
                spans.push(Span::dim(" "));
                used += UnicodeWidthStr::width(*icon) + 1;
            }
            let body_width = target
                .unwrap_or_else(|| width.saturating_sub(META_INDENT))
                .saturating_sub(used);
            let body = truncate_path_like(text, body_width, icon.is_some());
            used += UnicodeWidthStr::width(body.as_str());
            spans.push(Span::dim(body));
            if let Some(target) = target {
                if target > used {
                    spans.push(Span::dim(" ".repeat(target - used)));
                }
            }
        }
        vec![Line::new(spans).truncated(width)]
    }

    /// codex `render_picker_footer_separator` + `picker_footer_progress_label`.
    ///
    /// 원본은 대시 줄을 그린 다음 그 위에 라벨을 덧그린다(오른쪽 끝에서 한 칸
    /// 안). 우리 줄 모델에는 겹쳐 그리기가 없으므로 같은 글자가 나오도록
    /// 나눠서 낸다 — 보이는 바이트는 같다.
    fn separator_line(&self, width: usize, viewport: usize) -> Line {
        let label = self.progress_label(width, viewport);
        let label_width = UnicodeWidthStr::width(label.as_str());
        if label.is_empty() || label_width + 1 >= width {
            return Line::new(vec![Span::dim("─".repeat(width))]);
        }
        Line::new(vec![
            Span::dim("─".repeat(width - label_width - 1)),
            Span::dim(label),
            Span::dim("─".to_string()),
        ])
    }

    fn progress_label(&self, width: usize, viewport: usize) -> String {
        let position = if self.filtered.is_empty() {
            0
        } else {
            self.selected + 1
        };
        let total = self.filtered.len();
        let percent = self.scroll_percent(viewport, width);
        [
            format!(" {position} / {total} · {percent}% "),
            format!(" {position}/{total} · {percent}% "),
            format!(" {percent}% "),
        ]
        .into_iter()
        .find(|label| UnicodeWidthStr::width(label.as_str()) < width)
        .unwrap_or_default()
    }

    /// codex `picker_footer_scroll_percent`.
    fn scroll_percent(&self, viewport: usize, width: usize) -> u8 {
        if self.filtered.is_empty() {
            return 100;
        }
        let last = self.filtered.len() - 1;
        let content_rows = self.content_rows(viewport);
        let total_height = self.height_between(0, last, width);
        let max_scroll = total_height.saturating_sub(content_rows);
        if max_scroll == 0 {
            return 100;
        }
        let remaining = self.height_between(self.scroll_top.get(), last, width);
        if remaining <= content_rows {
            return 100;
        }
        let skipped = if self.scroll_top.get() == 0 {
            0
        } else {
            self.height_between(0, self.scroll_top.get() - 1, width)
        };
        // codex 는 `(skipped / max_scroll * 100.0).round()` 을 f32 로 센다.
        // 여기서는 정수로 같은 값을 낸다 — 둘 다 양수라 `.round()` 는
        // 반올림-올림이고, `(x * 200 + m) / (2 * m)` 이 그것이다. f32 를 피하는
        // 이유는 우리 clippy 가 usize→f32 를 막기 때문이고, 정수 쪽이 폭에
        // 상관없이 정확하다.
        let skipped = skipped.min(max_scroll);
        let percent = (skipped * 200 + max_scroll) / (2 * max_scroll);
        u8::try_from(percent).unwrap_or(100)
    }

    /// codex `footer_hint_lines` — 두 줄, 우리가 실제로 묶은 키만.
    fn hint_lines(&self, width: usize) -> Vec<Line> {
        let esc_label = if self.query.is_empty() {
            "exit"
        } else {
            "clear search"
        };
        let esc_compact = if self.query.is_empty() {
            "exit"
        } else {
            "clear"
        };
        let (density_label, density_compact) = match self.density {
            Density::Dense => ("comfortable view", "comfy"),
            Density::Comfortable => ("dense view", "dense"),
        };
        vec![
            hint_line(
                &[
                    ("enter", "resume", "resume"),
                    ("esc", esc_label, esc_compact),
                    ("ctrl+c", "exit", "exit"),
                    ("tab", "focus sort/filter", "focus"),
                    ("←/→", "change option", "option"),
                ],
                width,
            ),
            hint_line(
                &[
                    ("ctrl+o", density_label, density_compact),
                    ("↑/↓", "browse", "browse"),
                ],
                width,
            ),
        ]
    }
}

/// codex `resume_picker.rs::paths_match` → `path_utils::paths_match_after_normalization`
/// (`canonicalize` 하고, 어느 한쪽이라도 못 하면 그대로 비교).
///
/// 문자열 비교로는 못 쓴다는 것이 실측으로 나왔다: macOS 의 `/var` 는
/// `/private/var` 로 가는 심링크라, 사이드카에 `/var/folders/…/work` 라 적힌
/// 세션은 zo 자신이 `/private/var/folders/…/work` 에서 돌 때 `Filter: [Cwd]` 에
/// 걸리지 않았다 — 이 창의 세션인데도 목록에서 사라졌다
/// (`docs/captures/pty-driver-zo-r28-resume.py` 로 재현).
fn paths_match(left: &Path, right: &Path) -> bool {
    if let (Ok(left), Ok(right)) = (left.canonicalize(), right.canonicalize()) {
        return left == right;
    }
    left == right
}

/// codex `toolbar_value`.
fn toolbar_value(label: &str, active: bool, focused: bool) -> Span {
    if active {
        let value = format!("[{label}]");
        if focused {
            Span::new(value, Style::new().fg(palette::TOOLBAR_FOCUS))
        } else {
            Span::raw(value)
        }
    } else {
        Span::dim(format!(" {label} "))
    }
}

/// codex `selected_session_style` — 어두운 배경의 갈래(노랑).
fn selected_style() -> Style {
    Style::new().fg(palette::SESSION_SELECTED)
}

fn row_background_style(
    terminal_palette: Option<palette::TerminalPalette>,
    selected: bool,
) -> Style {
    let mut style = if selected {
        selected_style()
    } else {
        Style::new()
    };
    let Some(terminal_palette) = terminal_palette else {
        return style;
    };
    let background = terminal_palette.background();
    let (overlay, alpha) = if terminal_palette.has_light_background() {
        ((0, 0, 0), if selected { 0.12 } else { 0.04 })
    } else {
        ((255, 255, 255), if selected { 0.12 } else { 0.055 })
    };
    let (red, green, blue) = palette::blend_rgb(overlay, background, alpha);
    style.bg = Some(Color::Rgb(red, green, blue));
    style
}

fn apply_row_style(line: &mut Line, style: Style, width: usize) {
    let padding = width.saturating_sub(line.width());
    if padding > 0 {
        line.spans.push(Span::new(" ".repeat(padding), style));
    }
    line.style = line.style.patch(style);
    for span in &mut line.spans {
        span.style = span.style.patch(style);
    }
}

/// codex `dense_column_text` — 폭-1 로 자르고 폭까지 채운다.
fn column_text(text: &str, width: usize) -> String {
    let text = truncate(text, width.saturating_sub(1));
    let padding = width.saturating_sub(UnicodeWidthStr::width(text.as_str()));
    format!("{text}{}", " ".repeat(padding))
}

/// codex `truncate_text` — 넘치면 `…` 로 끝낸다.
fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let cell = UnicodeWidthStr::width(ch.to_string().as_str());
        if used + cell > width.saturating_sub(1) {
            break;
        }
        out.push(ch);
        used += cell;
    }
    out.push('…');
    out
}

/// 경로는 가운데를, 그 밖은 끝을 자른다 — codex 도 디렉터리에만
/// `center_truncate_path` 를 쓴다(`format_directory_display`).
fn truncate_path_like(text: &str, width: usize, is_path: bool) -> String {
    if UnicodeWidthStr::width(text) <= width {
        return text.to_string();
    }
    if is_path {
        center_truncate_path(text, width)
    } else {
        truncate(text, width)
    }
}

/// codex `cwd_column_width`.
fn cwd_column_width(width: usize) -> usize {
    let available = width.saturating_sub(META_INDENT + DATE_WIDTH + 2 * META_GAP);
    (available / 2).clamp(MIN_CWD_WIDTH, MAX_CWD_WIDTH)
}

/// codex `hint_line_for_row` + `fit_footer_hints`: wide → compact → 키만.
fn hint_line(hints: &[(&str, &str, &str)], width: usize) -> Line {
    let wide = width >= HINT_COMPACT_BREAKPOINT;
    for label in [wide.then_some(1u8), Some(2), Some(3)].into_iter().flatten() {
        let mut spans = vec![Span::dim(" ".repeat(HINT_LEFT_PADDING))];
        let mut used = HINT_LEFT_PADDING;
        for (index, (key, wide_label, compact_label)) in hints.iter().enumerate() {
            if index > 0 {
                spans.push(Span::dim(" ".repeat(HINT_GAP)));
                used += HINT_GAP;
            }
            spans.push(Span::raw((*key).to_string()));
            used += UnicodeWidthStr::width(*key);
            let text = match label {
                1 => Some(*wide_label),
                2 => Some(*compact_label),
                _ => None,
            };
            if let Some(text) = text {
                spans.push(Span::dim(" "));
                spans.push(Span::dim(text.to_string()));
                used += 1 + UnicodeWidthStr::width(text);
            }
        }
        if used <= width {
            return Line::new(spans);
        }
    }
    Line::empty()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        cwd_column_width, directory_display, one_line, relative_time, row_background_style,
        rows, truncate, Density, SessionPicker, SessionRow,
    };
    use crate::resume::ResumeSession;
    use crate::tui::ansi::Color;
    use crate::tui::palette::{ColorLevel, TerminalPalette};

    /// codex `format_relative_time` 의 다섯 구간.
    #[test]
    fn relative_time_follows_the_codex_ladder() {
        let now = 10_000_000_000u128;
        assert_eq!(relative_time(now, now), "now");
        assert_eq!(relative_time(now, now - 999), "now");
        assert_eq!(relative_time(now, now - 12_000), "12s ago");
        assert_eq!(relative_time(now, now - 59_000), "59s ago");
        assert_eq!(relative_time(now, now - 60_000), "1m ago");
        assert_eq!(relative_time(now, now - 3_599_000), "59m ago");
        assert_eq!(relative_time(now, now - 3_600_000), "1h ago");
        assert_eq!(relative_time(now, now - 86_399_000), "23h ago");
        assert_eq!(relative_time(now, now - 86_400_000), "1d ago");
        // 미래로 적힌 타임스탬프도 원본처럼 `now` 다(`.max(0)`).
        assert_eq!(relative_time(now, now + 5_000), "now");
    }

    #[test]
    fn a_prompt_becomes_one_line() {
        assert_eq!(one_line("  a\n\n  b  c "), "a b c");
        assert_eq!(one_line("   "), "(no prompt)");
    }

    #[test]
    fn rows_carry_the_id_the_columns_and_the_two_times() {
        let now = 10_000_000_000u128;
        let sessions = vec![ResumeSession {
            id: "session-9999999000000-0".to_string(),
            path: PathBuf::from("/tmp/s-1.jsonl"),
            modified_epoch_millis: now - 120_000,
            created_epoch_millis: 9_999_999_000_000,
            cwd: Some(PathBuf::from("/w/one")),
            first_prompt: Some("fix the\nparser".to_string()),
        }];
        let rows = rows(&sessions);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "session-9999999000000-0");
        assert_eq!(rows[0].preview, "fix the parser");
        assert_eq!(rows[0].updated_millis, now - 120_000);
        // 생성 시각은 트랜스크립트 머리글에서 온다 — 수정 시각과 다른 축이다.
        assert_eq!(rows[0].created_millis, 9_999_999_000_000);
        assert_eq!(rows[0].cwd.as_deref(), Some(std::path::Path::new("/w/one")));
    }

    /// 두 정렬은 정말 다른 순서다 — 오래 산 세션은 마지막에 쓴 것이 최근이어도
    /// **만들어진** 것은 한참 전이다.
    #[test]
    fn created_and_updated_are_different_orders() {
        let now = 10_000_000_000u128;
        let mut picker = SessionPicker::new(
            vec![
                SessionRow {
                    id: "long-lived".to_string(),
                    cwd: None,
                    preview: "still being written".to_string(),
                    created_millis: now - 86_400_000,
                    updated_millis: now - 1_000,
                    held_by: None,
                },
                SessionRow {
                    id: "short-lived".to_string(),
                    cwd: None,
                    preview: "opened and left".to_string(),
                    created_millis: now - 60_000,
                    updated_millis: now - 50_000,
                    held_by: None,
                },
            ],
            None,
            now,
        );
        // cwd 기준이 없으니 `[All]` 로 열리고, 기본 정렬은 Updated 다.
        assert_eq!(picker.visible_ids(), vec!["long-lived", "short-lived"]);
        picker.focus_next();
        picker.change_option();
        assert_eq!(picker.visible_ids(), vec!["short-lived", "long-lived"]);
    }

    /// 걸 cwd 가 없으면 `Filter` 는 토글되지 않는다 — codex
    /// `SessionFilterMode::toggle(None)`.
    #[test]
    fn without_a_cwd_the_filter_control_has_one_value() {
        let mut picker = SessionPicker::new(
            vec![SessionRow {
                id: "session-1-0".to_string(),
                cwd: None,
                preview: "only".to_string(),
                created_millis: 1,
                updated_millis: 1,
                held_by: None,
            }],
            None,
            10,
        );
        let before: Vec<String> = picker.visible_ids().iter().map(ToString::to_string).collect();
        picker.change_option();
        assert_eq!(picker.visible_ids(), before);
        let line = picker.lines(120, 40)[4].plain();
        // 값 하나짜리 갈래(codex `filter_control_spans` 의 compact 분기).
        assert!(line.contains("Filter:[All]"), "{line:?}");
    }

    /// 목록이 창보다 길면 구분선의 퍼센트가 움직이고 `↑ more`/`↓ more` 가 뜬다.
    #[test]
    fn a_long_list_scrolls_and_the_separator_reports_where_we_are() {
        let now = 10_000_000_000u128;
        let rows: Vec<SessionRow> = (0..40)
            .map(|index| SessionRow {
                id: format!("session-{}-0", 9_000_000_000_000u128 + index),
                cwd: None,
                preview: format!("session number {index}"),
                created_millis: 9_000_000_000_000 + index,
                updated_millis: now - index * 1_000,
                held_by: None,
            })
            .collect();
        let mut picker = SessionPicker::new(rows, None, now);

        let top = picker.lines(120, 20);
        let top_separator = top[top.len() - 3].plain();
        assert!(top_separator.ends_with(" 1 / 40 · 0% ─"), "{top_separator:?}");
        assert!(
            top.iter().any(|line| line.plain().contains("↓ more")),
            "a list longer than the window says so"
        );
        assert!(
            !top.iter().any(|line| line.plain().contains("↑ more")),
            "nothing is above the first row"
        );

        for _ in 0..39 {
            picker.down();
        }
        let bottom = picker.lines(120, 20);
        let bottom_separator = bottom[bottom.len() - 3].plain();
        assert!(
            bottom_separator.ends_with(" 40 / 40 · 100% ─"),
            "{bottom_separator:?}"
        );
        assert!(
            bottom.iter().any(|line| line.plain().contains("↑ more")),
            "the window has scrolled off the top"
        );
    }

    /// comfortable 은 행마다 제목 + 메타 줄을 쓰므로 같은 창에 절반이 담긴다.
    #[test]
    fn density_changes_how_many_rows_fit() {
        let now = 10_000_000_000u128;
        let rows: Vec<SessionRow> = (0..10)
            .map(|index| SessionRow {
                id: format!("session-{}-0", 9_000_000_000_000u128 + index),
                cwd: Some(PathBuf::from("/w/one")),
                preview: format!("session number {index}"),
                created_millis: 9_000_000_000_000 + index,
                updated_millis: now - index * 1_000,
                held_by: None,
            })
            .collect();
        let mut picker = SessionPicker::new(rows, None, now);
        let dense = picker.lines(120, 30);
        picker.toggle_density();
        let comfortable = picker.lines(120, 30);
        assert_eq!(dense.len(), comfortable.len(), "창 높이는 그대로다");
        let visible = |lines: &[super::Line]| {
            lines
                .iter()
                .filter(|line| line.plain().contains("session number"))
                .count()
        };
        assert!(
            visible(&comfortable) < visible(&dense),
            "dense {} vs comfortable {}",
            visible(&dense),
            visible(&comfortable)
        );
        assert_eq!(picker.density, Density::Comfortable);
    }

    #[test]
    fn resume_rows_blend_zebra_and_selection_backgrounds_from_the_terminal() {
        let dark = TerminalPalette::new(
            (240, 240, 240),
            (20, 20, 20),
            ColorLevel::TrueColor,
        );
        assert_eq!(
            row_background_style(Some(dark), false).bg,
            Some(Color::Rgb(32, 32, 32))
        );
        let selected = row_background_style(Some(dark), true);
        assert_eq!(selected.bg, Some(Color::Rgb(48, 48, 48)));
        assert_eq!(selected.fg, Some(super::palette::SESSION_SELECTED));

        let light = TerminalPalette::new(
            (20, 20, 20),
            (240, 240, 240),
            ColorLevel::TrueColor,
        );
        assert_eq!(
            row_background_style(Some(light), false).bg,
            Some(Color::Rgb(230, 230, 230))
        );
        assert_eq!(
            row_background_style(Some(light), true).bg,
            Some(Color::Rgb(211, 211, 211))
        );
    }

    #[test]
    fn comfortable_resume_rows_carry_the_background_through_full_width_padding() {
        let now = 10_000_000_000u128;
        let row = SessionRow {
            id: "session-1-0".to_string(),
            cwd: Some(PathBuf::from("/work")),
            preview: "zebra row".to_string(),
            created_millis: now - 60_000,
            updated_millis: now - 30_000,
            held_by: None,
        };
        let mut picker = SessionPicker::new(vec![row.clone()], None, now);
        picker.density = Density::Comfortable;
        let palette = TerminalPalette::new(
            (240, 240, 240),
            (20, 20, 20),
            ColorLevel::TrueColor,
        );

        let lines = picker.row_lines(&row, false, true, 80, Some(palette));
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|line| line.width() == 80));
        assert!(lines
            .iter()
            .all(|line| line.style.bg == Some(Color::Rgb(32, 32, 32))));
        assert!(lines.iter().all(|line| line
            .spans
            .iter()
            .all(|span| span.style.bg == Some(Color::Rgb(32, 32, 32)))));
    }

    /// `Filter: [Cwd]` 는 심링크를 지나서 비교해야 한다.
    ///
    /// 실측으로 나온 결함이다: macOS 의 `/var` 는 `/private/var` 로 가는
    /// 심링크라, 사이드카에 `/var/folders/…/work` 라 적힌 세션이 zo 자신이
    /// `/private/var/folders/…/work` 에서 돌 때 목록에서 통째로 사라졌다 —
    /// 이 창의 세션인데도. codex 는 `paths_match_after_normalization` 으로
    /// 그것을 지난다.
    #[cfg(unix)]
    #[test]
    fn the_cwd_filter_sees_through_a_symlink() {
        let root = std::env::temp_dir().join(format!(
            "zo-r28-cwd-filter-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let real = root.join("real");
        std::fs::create_dir_all(&real).expect("real directory");
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let picker = SessionPicker::new(
            vec![SessionRow {
                id: "session-1-0".to_string(),
                // 사이드카가 심링크 쪽 이름을 들고 있다.
                cwd: Some(link.clone()),
                preview: "this window's session".to_string(),
                created_millis: 1,
                updated_millis: 1,
                held_by: None,
            }],
            // zo 자신은 실제 경로에서 돈다.
            Some(real.clone()),
            10,
        );
        assert_eq!(
            picker.visible_ids(),
            vec!["session-1-0"],
            "심링크 이름과 실제 이름은 같은 디렉터리다"
        );

        // 정말 다른 디렉터리는 여전히 걸러진다.
        let other = root.join("other");
        std::fs::create_dir_all(&other).expect("other directory");
        let picker = SessionPicker::new(
            vec![SessionRow {
                id: "session-2-0".to_string(),
                cwd: Some(other),
                preview: "someone else's tree".to_string(),
                created_millis: 1,
                updated_millis: 1,
                held_by: None,
            }],
            Some(real),
            10,
        );
        assert!(picker.visible_ids().is_empty());

        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn the_home_directory_is_written_with_a_tilde() {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let Some(home) = home else { return };
        assert_eq!(directory_display(&home), "~");
        assert_eq!(
            directory_display(&home.join("work").join("zo")),
            format!("~{}work{}zo", std::path::MAIN_SEPARATOR, std::path::MAIN_SEPARATOR)
        );
        assert_eq!(
            directory_display(std::path::Path::new("/opt/elsewhere")),
            "/opt/elsewhere"
        );
    }

    /// codex `truncate_text` · `cwd_column_width` 의 경계.
    #[test]
    fn column_widths_follow_the_codex_arithmetic() {
        assert_eq!(truncate("abcdef", 10), "abcdef");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abcdef", 0), "");
        // (width - (2 + 12 + 4)) / 2, 30..=72 로 조인다.
        assert_eq!(cwd_column_width(40), 30);
        assert_eq!(cwd_column_width(120), 51);
        assert_eq!(cwd_column_width(400), 72);
    }

    /// t-2947 — the picker's cost must not depend on how long the newest
    /// transcripts are: twenty ~2 MB sessions at the head of a 2,000-session
    /// store, opened and filtered under the bound, fastest of N.
    #[test]
    fn opening_the_picker_and_typing_stays_under_the_bound_over_2000_sessions() {
        let pin = crate::SessionRootPin::new("picker-bound", &session_corpus::CorpusSpec::HEAVY_HEAD);
        let bounds = session_corpus::SearchBounds::T2947;
        let mut listed_rows = 0;
        let mut visible_rows = 0;
        let elapsed = bounds.min_of_runs(|| {
            let listed = crate::resume::list_recent_sessions_limited(crate::resume::DEFAULT_LIST_LIMIT)
                .expect("session list");
            listed_rows = listed.len();
            let mut picker = SessionPicker::new(rows(&listed), Some(pin.cwd.clone()), super::now_millis());
            picker.push_char('p');
            visible_rows = picker.visible_ids().len();
        });
        assert_eq!(listed_rows, crate::resume::DEFAULT_LIST_LIMIT, "a full page of rows");
        assert!(visible_rows > 0, "every prompt starts with `prompt`, so `p` keeps rows");
        assert!(
            elapsed < bounds.picker_open_and_first_keystroke,
            "picker open + first keystroke took {elapsed:?} over {} sessions (bound {:?}, min of {})",
            pin.manifest.len(),
            bounds.picker_open_and_first_keystroke,
            bounds.min_of
        );
    }

    /// t-2947 — a keystroke reads no file: the rows were read once, at open.
    /// Take the store away and the filter still answers from memory.
    #[test]
    fn a_keystroke_reads_no_file() {
        let pin = crate::SessionRootPin::new("keystroke", &session_corpus::CorpusSpec::REAL_MIX.with_sessions(60));
        let listed = crate::resume::list_recent_sessions_limited(crate::resume::DEFAULT_LIST_LIMIT)
            .expect("session list");
        let all = rows(&listed);
        let query = "prompt 1";
        let expected: Vec<String> = all
            .iter()
            .filter(|row| row.matches_query(query))
            .map(|row| row.id.clone())
            .collect();
        assert!(!expected.is_empty() && expected.len() < all.len(), "the query narrows the page");
        let mut picker = SessionPicker::new(all, None, super::now_millis());

        std::fs::remove_dir_all(&pin.sessions_dir).expect("store taken away");
        for ch in query.chars() {
            picker.push_char(ch);
        }
        let visible: Vec<String> = picker.visible_ids().into_iter().map(str::to_owned).collect();
        assert_eq!(visible, expected);
    }
}

/// Measurement harness (t-2947), not a gate: `ZO_SESSION_ROOT=<root> cargo test
/// -p zo-ide --lib -- --ignored --nocapture measure_resume_picker` prints how
/// long the `/resume` picker takes to open and to filter its first keystroke
/// over whatever store the root holds.
#[cfg(test)]
mod measure {
    use std::time::{Duration, Instant};

    use super::{now_millis, rows, SessionPicker};

    const RUNS: usize = 20;

    fn percentile(samples: &mut [Duration], percent: usize) -> Duration {
        samples.sort_unstable();
        samples[(samples.len() * percent / 100).min(samples.len() - 1)]
    }

    #[test]
    #[ignore = "measurement, not a gate — needs ZO_SESSION_ROOT"]
    fn measure_resume_picker_on_session_root() {
        let Some(root) = std::env::var_os("ZO_SESSION_ROOT") else {
            eprintln!("ZO_SESSION_ROOT unset; nothing measured");
            return;
        };
        let _guard = crate::test_cwd_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Only the root under measurement: an empty config home keeps the
        // developer's own per-project store out of the numbers.
        let home = std::env::temp_dir().join(format!("zo-measure-home-{}", std::process::id()));
        std::fs::create_dir_all(&home).expect("config home");
        let prior_home = std::env::var_os("ZO_CONFIG_HOME");
        std::env::set_var("ZO_CONFIG_HOME", &home);

        let mut opens = Vec::with_capacity(RUNS);
        let mut keystrokes = Vec::with_capacity(RUNS);
        let mut listed_rows = 0;
        for _ in 0..RUNS {
            let started = Instant::now();
            let listed = crate::resume::list_recent_sessions_limited(crate::resume::DEFAULT_LIST_LIMIT)
                .expect("session list");
            let mut picker = SessionPicker::new(rows(&listed), None, now_millis());
            opens.push(started.elapsed());
            listed_rows = listed.len();
            let started = Instant::now();
            picker.push_char('s');
            keystrokes.push(started.elapsed());
        }

        match prior_home {
            Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
            None => std::env::remove_var("ZO_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&home);

        println!(
            "resume picker over {} ({listed_rows} rows listed, {RUNS} runs)\n  open      p50 {:>8.2} ms  p95 {:>8.2} ms\n  keystroke p50 {:>8.3} ms  p95 {:>8.3} ms",
            root.to_string_lossy(),
            percentile(&mut opens, 50).as_secs_f64() * 1e3,
            percentile(&mut opens, 95).as_secs_f64() * 1e3,
            percentile(&mut keystrokes, 50).as_secs_f64() * 1e3,
            percentile(&mut keystrokes, 95).as_secs_f64() * 1e3,
        );
    }
}
