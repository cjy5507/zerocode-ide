//! The `@` mention popup — Codex 0.155.1 `bottom_pane/mentions_v2/*` (the
//! unified popup Codex shows by default), `bottom_pane/popup_consts.rs`, and
//! the composer's own `@`-token machinery from `bottom_pane/chat_composer.rs`
//! (`current_at_token` · `on_file_search_result` ·
//! `handle_key_event_with_mentions_v2_popup` · `insert_selected_file_path` ·
//! `dismiss_completed_prefixed_token`).
//!
//! Rows come from two places, as in Codex: the file search
//! (`crate::session::file_search`) and a catalog of non-file candidates —
//! skills here, where Codex also lists plugins and background tasks. What is
//! zo's own is one more row kind, [`MentionType::Page`]: a second-brain page
//! shown as `wiki/<page>`, inserted as `@wiki/<page>` and unfolded into the
//! prompt on submit ([`expand_page_mentions`]).
//!
//! Keys, rows, highlight, footer and words are Codex's. The one wording
//! divergence is deliberate: the third search mode is called
//! [`MODE_TOOLS`] (`Skills`) because that is what it holds — zo has no
//! plugins, and Codex's `Plugins` would name a thing that is not there.

use std::ops::Range;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use runtime::file_search::{fuzzy_match, FileMatch, MatchType, SearchRoot};
use runtime::SkillIndexEntry;

use super::ansi::{Color, Line, Span, Style};
use super::composer::{is_line_break, Composer};
use super::palette;
use super::view::MAX_POPUP_ROWS;
use crate::session::file_search::FileSearchManager;

/// Codex `mentions_v2/candidate.rs::TAG_WIDTH` — the right-aligned tag column
/// is as wide as its widest word.
pub const TAG_WIDTH: usize = "Plugin".len();
/// Codex `mentions_v2/popup.rs::FileSearch::empty_message` while results are
/// on their way.
pub const LOADING: &str = "loading...";
/// Codex `mentions_v2/popup.rs::FileSearch::empty_message` once they arrived
/// empty.
pub const NO_MATCHES: &str = "no matches";
/// Codex `mentions_v2/render.rs::build_line` — the selected row's gutter.
pub const GUTTER_SELECTED: &str = "> ";
/// Codex `mentions_v2/render.rs::build_line` — every other row's gutter.
pub const GUTTER: &str = "  ";
/// Codex `mentions_v2/render.rs::content_line` — between a row's name and
/// its path or description.
pub const COLUMN_GAP: usize = 2;
/// Codex `mentions_v2/render.rs::path_spans` — the path column of a file at
/// the root.
pub const ROOT_PATH: &str = "./";
/// Codex `mentions_v2/footer.rs::footer_hint_line`, key by key.
pub const FOOTER_INSERT: &str = " insert · ";
pub const FOOTER_CLOSE: &str = " close · ";
pub const FOOTER_MODES: &str = " switch search modes";
pub const KEY_ENTER: &str = "enter";
pub const KEY_ESC: &str = "esc";
pub const KEY_LEFT: &str = "←";
pub const KEY_RIGHT: &str = "→";
pub const KEY_SLASH: &str = "/";
/// Codex `mentions_v2/footer.rs::search_mode_indicator_line` — between two
/// mode words.
pub const MODE_GAP: &str = "  ";
/// Codex `mentions_v2/search_mode.rs::label` — `All Results`.
pub const MODE_RESULTS: &str = "All Results";
/// Codex `mentions_v2/search_mode.rs::label` — `Filesystem Only`.
pub const MODE_FILES: &str = "Filesystem Only";
/// zo's word for Codex's `Plugins` mode: the rows it holds are skills.
pub const MODE_TOOLS: &str = "Skills";
/// Codex `mentions_v2/candidate.rs::MentionType::label`.
pub const TAG_SKILL: &str = "Skill";
pub const TAG_FILE: &str = "File";
pub const TAG_DIRECTORY: &str = "Dir";
/// zo: the tag of a second-brain page.
pub const TAG_PAGE: &str = "Vault";
/// Codex `mentions_v2/search_catalog.rs::skill_candidate` — a skill is
/// inserted as `$<name>`.
pub const SKILL_SIGIL: char = '$';
/// The mention sigil, Codex `completion_target`'s `'@'`.
pub const MENTION_SIGIL: char = '@';
/// zo: the header over a page's body when a submitted prompt unfolds one.
pub const PAGE_ATTACHMENT_HEADER: &str = "second brain page";

/// Codex `mentions_v2/candidate.rs::MentionType`, plus zo's `Page`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MentionType {
    Skill,
    File,
    Directory,
    Page,
}

impl MentionType {
    /// Rows the file search produced — Codex `is_filesystem`, with a page
    /// counted in: it is a file on disk, shown as name plus path.
    #[must_use]
    pub const fn is_filesystem(self) -> bool {
        matches!(self, Self::File | Self::Directory | Self::Page)
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Skill => TAG_SKILL,
            Self::File => TAG_FILE,
            Self::Directory => TAG_DIRECTORY,
            Self::Page => TAG_PAGE,
        }
    }

    /// Codex `MentionType::span` — the tag, padded to [`TAG_WIDTH`].
    fn tag(self) -> Span {
        let style = match self {
            Self::Skill => Style::new().dim(),
            Self::File => Style::new().fg(palette::MENTION_FILE),
            Self::Directory => Style::new(),
            Self::Page => Style::new().fg(palette::MENTION_PAGE),
        };
        Span::new(format!("{:<width$}", self.label(), width = TAG_WIDTH), style)
    }

    /// Codex `filter.rs::sort_rows::type_order` — skills before files; a page
    /// is a file.
    const fn order(self) -> u8 {
        match self {
            Self::Skill => 1,
            Self::File | Self::Directory | Self::Page => 3,
        }
    }
}

/// Codex `mentions_v2/candidate.rs::Selection`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    /// A file: inserted as its path, quoted when it holds whitespace.
    File(PathBuf),
    /// A skill (`$name`) or a page (`@wiki/…`): inserted as it is.
    Tool { insert_text: String },
}

/// Codex `mentions_v2/candidate.rs::Candidate` — a non-file row before any
/// query touched it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub display_name: String,
    pub description: Option<String>,
    pub search_terms: Vec<String>,
    pub mention_type: MentionType,
    pub selection: Selection,
}

impl Candidate {
    fn to_result(&self, match_indices: Option<Vec<usize>>, score: u32) -> SearchResult {
        SearchResult {
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            mention_type: self.mention_type,
            selection: self.selection.clone(),
            match_indices,
            score,
        }
    }
}

/// Codex `mentions_v2/candidate.rs::SearchResult` — one row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    pub display_name: String,
    pub description: Option<String>,
    pub mention_type: MentionType,
    pub selection: Selection,
    pub match_indices: Option<Vec<usize>>,
    pub score: u32,
}

/// Codex `mentions_v2/search_mode.rs::SearchMode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchMode {
    Results,
    FilesystemOnly,
    Tools,
}

impl SearchMode {
    /// The footer's order, Codex `search_mode_indicator_line`.
    pub const ALL: [Self; 3] = [Self::Results, Self::FilesystemOnly, Self::Tools];

    #[must_use]
    pub const fn previous(self) -> Self {
        match self {
            Self::Results => Self::Tools,
            Self::FilesystemOnly => Self::Results,
            Self::Tools => Self::FilesystemOnly,
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Results => Self::FilesystemOnly,
            Self::FilesystemOnly => Self::Tools,
            Self::Tools => Self::Results,
        }
    }

    #[must_use]
    pub const fn accepts(self, mention_type: MentionType) -> bool {
        match self {
            Self::Results => true,
            Self::FilesystemOnly => mention_type.is_filesystem(),
            Self::Tools => matches!(mention_type, MentionType::Skill),
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Results => MODE_RESULTS,
            Self::FilesystemOnly => MODE_FILES,
            Self::Tools => MODE_TOOLS,
        }
    }
}

/// Codex `bottom_pane/scroll_state.rs::ScrollState` — a wrapping selection
/// and the window that keeps it visible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScrollState {
    pub selected_idx: Option<usize>,
    pub scroll_top: usize,
}

impl ScrollState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn clamp_selection(&mut self, len: usize) {
        if len == 0 {
            self.selected_idx = None;
            self.scroll_top = 0;
        } else {
            match self.selected_idx {
                Some(idx) if idx >= len => self.selected_idx = Some(len - 1),
                None => self.selected_idx = Some(0),
                Some(_) => {}
            }
        }
    }

    pub fn move_up_wrap(&mut self, len: usize) {
        if len == 0 {
            self.selected_idx = None;
            return;
        }
        self.selected_idx = Some(match self.selected_idx {
            Some(0) | None => len - 1,
            Some(idx) => idx - 1,
        });
    }

    pub fn move_down_wrap(&mut self, len: usize) {
        if len == 0 {
            self.selected_idx = None;
            return;
        }
        self.selected_idx = Some(match self.selected_idx {
            Some(idx) if idx + 1 < len => idx + 1,
            _ => 0,
        });
    }

    pub fn ensure_visible(&mut self, len: usize, visible: usize) {
        if len == 0 || visible == 0 {
            self.scroll_top = 0;
            return;
        }
        let Some(selected) = self.selected_idx else {
            return;
        };
        if selected < self.scroll_top {
            self.scroll_top = selected;
        } else if selected >= self.scroll_top + visible {
            self.scroll_top = selected + 1 - visible;
        }
    }
}

/// Codex `mentions_v2/popup.rs::FileSearch` — the file half of the rows.
/// Codex also keeps the query its matches answer (`display_query`); nothing
/// reads it there either, and here the compiler would say so.
#[derive(Debug, Default)]
struct FileSearch {
    pending_query: String,
    waiting: bool,
    matches: Vec<FileMatch>,
}

impl FileSearch {
    fn set_query(&mut self, query: &str) {
        if query.is_empty() {
            self.pending_query.clear();
            self.waiting = false;
            self.matches.clear();
        } else if query != self.pending_query {
            self.pending_query = query.to_string();
            self.waiting = true;
        }
    }

    fn set_matches(&mut self, query: &str, matches: Vec<FileMatch>) {
        if query != self.pending_query {
            return;
        }
        self.matches = matches.into_iter().take(MAX_POPUP_ROWS).collect();
        self.waiting = false;
    }

    const fn should_show_matches(&self) -> bool {
        !self.matches.is_empty()
    }

    const fn empty_message(&self) -> &'static str {
        if self.waiting {
            LOADING
        } else {
            NO_MATCHES
        }
    }
}

/// Codex `mentions_v2/popup.rs::Popup` — the popup's state.
#[derive(Debug)]
pub struct MentionPopup {
    query: String,
    file_search: FileSearch,
    candidates: Vec<Candidate>,
    rows: Vec<SearchResult>,
    search_mode: SearchMode,
    state: ScrollState,
    /// The vault's page root, when there is one — a match from it is a
    /// [`MentionType::Page`].
    page_root: Option<PathBuf>,
}

impl MentionPopup {
    #[must_use]
    pub fn new(candidates: Vec<Candidate>, query: &str, page_root: Option<PathBuf>) -> Self {
        let mut file_search = FileSearch::default();
        file_search.set_query(query);
        let mut popup = Self {
            query: query.to_string(),
            file_search,
            candidates,
            rows: Vec::new(),
            search_mode: SearchMode::Results,
            state: ScrollState::default(),
            page_root,
        };
        popup.refresh_rows();
        popup
    }

    pub fn set_query(&mut self, query: &str) {
        if self.query == query {
            return;
        }
        self.query = query.to_string();
        self.file_search.set_query(query);
        self.refresh_rows();
    }

    pub fn set_file_matches(&mut self, query: &str, matches: Vec<FileMatch>) {
        self.file_search.set_matches(query, matches);
        self.refresh_rows();
    }

    #[must_use]
    pub fn selected(&self) -> Option<Selection> {
        let idx = self.state.selected_idx?;
        self.rows.get(idx).map(|row| row.selection.clone())
    }

    #[must_use]
    pub fn rows(&self) -> &[SearchResult] {
        &self.rows
    }

    #[must_use]
    pub const fn search_mode(&self) -> SearchMode {
        self.search_mode
    }

    #[must_use]
    pub const fn selected_index(&self) -> Option<usize> {
        self.state.selected_idx
    }

    pub fn move_up(&mut self) {
        let len = self.rows.len();
        self.state.move_up_wrap(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    pub fn move_down(&mut self) {
        let len = self.rows.len();
        self.state.move_down_wrap(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    pub fn previous_search_mode(&mut self) {
        self.search_mode = self.search_mode.previous();
        self.refresh_rows();
    }

    pub fn next_search_mode(&mut self) {
        self.search_mode = self.search_mode.next();
        self.refresh_rows();
    }

    /// Codex `calculate_required_height`: the visible rows (at least one, for
    /// the empty message), a blank row, and the footer.
    #[must_use]
    pub fn required_height(&self) -> usize {
        self.rows.len().clamp(1, MAX_POPUP_ROWS) + 2
    }

    /// Codex `refresh_rows` — rebuilds cached rows and keeps the selection
    /// valid after search inputs change.
    fn refresh_rows(&mut self) {
        self.rows = filtered_candidates(
            &self.candidates,
            &self.file_search.matches,
            &self.query,
            self.search_mode,
            self.file_search.should_show_matches(),
            self.page_root.as_deref(),
        );
        let len = self.rows.len();
        self.state.clamp_selection(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    /// Codex `mentions_v2/render.rs::render_popup` — the rows, one blank row,
    /// and the footer, for a `width`-column viewport. `focus` is the selected
    /// row's colour (Codex `accent_style`), the same one the slash popup
    /// uses.
    #[must_use]
    pub fn lines(&self, width: usize, focus: Color) -> Vec<Line> {
        let mut out = self.row_lines(width, focus);
        out.push(Line::empty());
        out.push(footer_line(width, self.search_mode, focus));
        out
    }

    /// Codex `render.rs::render_rows`.
    fn row_lines(&self, width: usize, focus: Color) -> Vec<Line> {
        if self.rows.is_empty() {
            return vec![Line::new(vec![
                Span::raw(GUTTER),
                Span::new(self.file_search.empty_message(), Style::new().italic()),
            ])
            .truncated(width)];
        }
        let visible_items = MAX_POPUP_ROWS.min(self.rows.len());
        let mut start_idx = self.state.scroll_top.min(self.rows.len().saturating_sub(1));
        if let Some(selected) = self.state.selected_idx {
            if selected < start_idx {
                start_idx = selected;
            } else if visible_items > 0 {
                let bottom = start_idx + visible_items - 1;
                if selected > bottom {
                    start_idx = selected + 1 - visible_items;
                }
            }
        }
        let primary_column_width = self
            .rows
            .iter()
            .skip(start_idx)
            .take(visible_items)
            .map(primary_text_width)
            .max()
            .unwrap_or(0);
        self.rows
            .iter()
            .enumerate()
            .skip(start_idx)
            .take(visible_items)
            .map(|(idx, row)| {
                build_line(
                    row,
                    Some(idx) == self.state.selected_idx,
                    width,
                    primary_column_width,
                    focus,
                )
            })
            .collect()
    }
}

/// Codex `mentions_v2/filter.rs::filtered_candidates`.
fn filtered_candidates(
    candidates: &[Candidate],
    file_matches: &[FileMatch],
    query: &str,
    search_mode: SearchMode,
    show_file_matches: bool,
    page_root: Option<&Path>,
) -> Vec<SearchResult> {
    let filter = query.trim();
    let mut out = Vec::new();
    for candidate in candidates {
        if !search_mode.accepts(candidate.mention_type) {
            continue;
        }
        if filter.is_empty() {
            out.push(candidate.to_result(None, 0));
            continue;
        }
        if let Some((indices, score)) = best_tool_match(candidate, filter) {
            out.push(candidate.to_result(indices, score));
        }
    }
    if show_file_matches {
        out.extend(
            file_matches
                .iter()
                .map(|file_match| file_match_to_row(file_match, page_root))
                .filter(|row| search_mode.accepts(row.mention_type)),
        );
    }
    sort_rows(&mut out, filter);
    out
}

/// Codex `filter.rs::best_tool_match`: the display name with its indices, or
/// the best of the other search terms without them.
fn best_tool_match(candidate: &Candidate, filter: &str) -> Option<(Option<Vec<usize>>, u32)> {
    if let Some((indices, score)) = fuzzy_match(&candidate.display_name, filter) {
        return Some((Some(indices), score));
    }
    candidate
        .search_terms
        .iter()
        .filter(|term| *term != &candidate.display_name)
        .filter_map(|term| fuzzy_match(term, filter).map(|(_, score)| score))
        .max()
        .map(|score| (None, score))
}

/// Codex `filter.rs::sort_rows`: by kind, then within the kind, then name.
fn sort_rows(rows: &mut [SearchResult], filter: &str) {
    rows.sort_by(|a, b| {
        a.mention_type
            .order()
            .cmp(&b.mention_type.order())
            .then_with(|| compare_within_rank(a, b, filter))
            .then_with(|| a.display_name.cmp(&b.display_name))
    });
}

/// Codex `filter.rs::compare_within_rank`. Both halves read "better match
/// first": file rows by the engine's score, tool rows with a name match ahead
/// of a term-only match and then by score.
fn compare_within_rank(a: &SearchResult, b: &SearchResult, filter: &str) -> std::cmp::Ordering {
    if a.mention_type.is_filesystem() && b.mention_type.is_filesystem() {
        return b.score.cmp(&a.score);
    }
    if filter.is_empty() {
        return a.display_name.cmp(&b.display_name);
    }
    a.match_indices
        .is_none()
        .cmp(&b.match_indices.is_none())
        .then_with(|| b.score.cmp(&a.score))
}

/// Codex `filter.rs::file_match_to_row`, telling a vault page by its root.
fn file_match_to_row(file_match: &FileMatch, page_root: Option<&Path>) -> SearchResult {
    let mention_type = match file_match.match_type {
        MatchType::File if page_root.is_some_and(|root| root == file_match.root) => {
            MentionType::Page
        }
        MatchType::File => MentionType::File,
        MatchType::Directory => MentionType::Directory,
    };
    let display_name = file_match.path.to_string_lossy().into_owned();
    let selection = if mention_type == MentionType::Page {
        Selection::Tool {
            insert_text: format!("{MENTION_SIGIL}{display_name}"),
        }
    } else {
        Selection::File(file_match.path.clone())
    };
    SearchResult {
        display_name,
        description: None,
        mention_type,
        selection,
        match_indices: file_match
            .indices
            .as_ref()
            .map(|indices| indices.iter().map(|idx| *idx as usize).collect()),
        score: file_match.score,
    }
}

/// Codex `mentions_v2/search_catalog.rs::build_search_catalog`, the skills
/// half — zo has no plugins and lists no background tasks here.
#[must_use]
pub fn build_search_catalog(skills: &[SkillIndexEntry]) -> Vec<Candidate> {
    skills.iter().map(skill_candidate).collect()
}

/// Codex `search_catalog.rs::skill_candidate`.
fn skill_candidate(skill: &SkillIndexEntry) -> Candidate {
    let description = skill
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .map(str::to_string);
    Candidate {
        display_name: skill.name.clone(),
        description,
        search_terms: vec![skill.name.clone()],
        mention_type: MentionType::Skill,
        selection: Selection::Tool {
            insert_text: format!("{SKILL_SIGIL}{}", skill.name),
        },
    }
}

// ---------------------------------------------------------------------------
// Rows — Codex `mentions_v2/render.rs`
// ---------------------------------------------------------------------------

/// Codex `render.rs::build_line`.
fn build_line(
    row: &SearchResult,
    selected: bool,
    width: usize,
    primary_column_width: usize,
    focus: Color,
) -> Line {
    let tag = row.mention_type.tag();
    let tag_width = tag.width();
    let gutter = if selected { GUTTER_SELECTED } else { GUTTER };
    let gutter_width = gutter.len();
    let content_width =
        width.saturating_sub(gutter_width.saturating_add(tag_width).saturating_add(COLUMN_GAP));
    let content = content_line(row, primary_column_width).truncated(content_width);
    let rendered_content_width = content.width();
    let mut spans = vec![Span::raw(gutter)];
    spans.extend(content.spans);
    let padding = width.saturating_sub(
        gutter_width
            .saturating_add(rendered_content_width)
            .saturating_add(tag_width),
    );
    if padding > 0 {
        spans.push(Span::dim(" ".repeat(padding)));
    }
    spans.push(tag);
    if selected {
        let style = Style::new().bold().fg(focus);
        for span in &mut spans {
            span.style = style;
        }
    }
    Line::new(spans)
}

/// Codex `render.rs::content_line`.
fn content_line(row: &SearchResult, primary_column_width: usize) -> Line {
    let mut spans = primary_spans(row);
    if let Some(secondary) = secondary_spans(row) {
        let padding = primary_column_width
            .saturating_sub(primary_text_width(row))
            .saturating_add(COLUMN_GAP);
        spans.push(Span::dim(" ".repeat(padding)));
        spans.extend(secondary);
    }
    Line::new(spans)
}

/// Codex `render.rs::primary_spans`: a file's name, or a tool's name with
/// its matched characters in bold.
fn primary_spans(row: &SearchResult) -> Vec<Span> {
    if let Some(file_name) = file_name(row) {
        let style = match row.mention_type {
            MentionType::File => Style::new().fg(palette::MENTION_FILE),
            MentionType::Page => Style::new().fg(palette::MENTION_PAGE),
            MentionType::Directory | MentionType::Skill => Style::new(),
        };
        return vec![Span::new(file_name, style)];
    }
    let name_style = match row.mention_type {
        MentionType::Skill => Style::new().dim(),
        MentionType::File | MentionType::Directory | MentionType::Page => Style::new(),
    };
    highlighted(&row.display_name, row.match_indices.as_deref(), name_style, None)
}

/// Codex `render.rs::secondary_line`: a file's path (and description), or a
/// tool's description.
fn secondary_spans(row: &SearchResult) -> Option<Vec<Span>> {
    if file_name(row).is_some() {
        let mut spans = path_spans(row);
        if let Some(description) = row
            .description
            .as_deref()
            .filter(|description| !description.is_empty())
        {
            spans.push(Span::dim(" ".repeat(COLUMN_GAP)));
            spans.push(Span::dim(description));
        }
        return Some(spans);
    }
    row.description
        .as_deref()
        .filter(|description| !description.is_empty())
        .map(|description| vec![Span::dim(description)])
}

/// Codex `render.rs::path_spans`: the directory part, dim, with matched
/// characters in bold; `./` for a file at the root.
fn path_spans(row: &SearchResult) -> Vec<Span> {
    let file_name_start = file_name_start(row);
    let path_style = Style::new().dim();
    if file_name_start == 0 {
        return vec![Span::new(ROOT_PATH, path_style)];
    }
    if file_name_start == usize::MAX {
        return vec![Span::raw(row.display_name.clone())];
    }
    match row.match_indices.as_deref() {
        Some(indices) => highlighted(&row.display_name, Some(indices), path_style, Some(file_name_start)),
        None => vec![Span::new(prefix_chars(&row.display_name, file_name_start), path_style)],
    }
}

/// `text` (its first `take` characters, or all of it) in `style`, the
/// characters at `indices` in bold — one span per run, so a name with no
/// match stays one span.
fn highlighted(text: &str, indices: Option<&[usize]>, style: Style, take: Option<usize>) -> Vec<Span> {
    let bold = style.bold();
    let mut spans: Vec<Span> = Vec::new();
    let mut idx_iter = indices.unwrap_or(&[]).iter().peekable();
    for (char_idx, ch) in text.chars().enumerate() {
        if take.is_some_and(|take| char_idx >= take) {
            break;
        }
        let matched = idx_iter.peek().is_some_and(|next| **next == char_idx);
        if matched {
            idx_iter.next();
        }
        let this = if matched { bold } else { style };
        match spans.last_mut() {
            Some(last) if last.style == this => last.text.push(ch),
            _ => spans.push(Span::new(ch.to_string(), this)),
        }
    }
    spans
}

fn prefix_chars(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

/// Codex `render.rs::primary_text_width`.
fn primary_text_width(row: &SearchResult) -> usize {
    file_name(row)
        .map_or_else(|| row.display_name.chars().count(), |name| name.chars().count())
}

/// Codex `render.rs::file_name`.
fn file_name(row: &SearchResult) -> Option<String> {
    let start = file_name_start(row);
    if start == usize::MAX {
        return None;
    }
    Some(row.display_name.chars().skip(start).collect())
}

/// Codex `render.rs::file_name_start` — the character index the file name
/// starts at, `0` for a root file, `usize::MAX` for a row that is not a path.
fn file_name_start(row: &SearchResult) -> usize {
    if !row.mention_type.is_filesystem() {
        return usize::MAX;
    }
    row.display_name
        .rfind(['/', '\\'])
        .map_or(0, |idx| row.display_name[..=idx].chars().count())
}

/// Codex `mentions_v2/footer.rs::render_footer`: the key hints on the left,
/// the search-mode indicator on the right, indented by [`GUTTER`].
fn footer_line(width: usize, search_mode: SearchMode, focus: Color) -> Line {
    let right = mode_indicator_spans(search_mode, focus);
    let right_width: usize = right.iter().map(Span::width).sum();
    let gap = usize::from(right_width > 0);
    let inner = width.saturating_sub(GUTTER.len());
    let left_width = inner.saturating_sub(right_width).saturating_sub(gap);
    let left = Line::new(vec![
        Span::raw(KEY_ENTER),
        Span::dim(FOOTER_INSERT),
        Span::raw(KEY_ESC),
        Span::dim(FOOTER_CLOSE),
        Span::raw(KEY_LEFT),
        Span::dim(KEY_SLASH),
        Span::raw(KEY_RIGHT),
        Span::dim(FOOTER_MODES),
    ])
    .truncated(left_width);
    let mut spans = vec![Span::raw(GUTTER)];
    let left_rendered = left.width();
    spans.extend(left.spans);
    if right_width > 0 && right_width <= inner {
        let padding = inner.saturating_sub(left_rendered).saturating_sub(right_width);
        spans.push(Span::raw(" ".repeat(padding)));
        spans.extend(right);
    }
    Line::new(spans).truncated(width)
}

/// Codex `footer.rs::search_mode_indicator_line`: `[Active]  Other  Other`.
fn mode_indicator_spans(active: SearchMode, focus: Color) -> Vec<Span> {
    let mut spans = Vec::with_capacity(SearchMode::ALL.len() * 2 - 1);
    for (index, mode) in SearchMode::ALL.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::dim(MODE_GAP));
        }
        if mode == active {
            let colour = if mode == SearchMode::Tools {
                palette::MENTION_PAGE
            } else {
                focus
            };
            spans.push(Span::new(
                format!("[{}]", mode.label()),
                Style::new().bold().fg(colour),
            ));
        } else {
            spans.push(Span::dim(format!(" {} ", mode.label())));
        }
    }
    spans
}

// ---------------------------------------------------------------------------
// The composer's side — Codex `chat_composer.rs`
// ---------------------------------------------------------------------------

/// Codex `chat_composer.rs::DismissedToken` — the one `@` occurrence the
/// person closed the popup on. It stays closed while that token still stands
/// between the same non-blank text before and after it; an edit inside the
/// token, or a second identical token elsewhere, opens the popup again, and
/// leading or trailing whitespace changes nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DismissedToken {
    before: String,
    text: String,
    after: String,
}

impl DismissedToken {
    #[must_use]
    pub fn new(composer: &Composer, range: &Range<usize>, token: &str) -> Self {
        let (before, after) = Self::neighbours(composer, range);
        Self {
            before,
            text: token.to_string(),
            after,
        }
    }

    #[must_use]
    pub fn matches(&self, composer: &Composer, range: &Range<usize>, token: &str) -> bool {
        if self.text != token {
            return false;
        }
        let (before, after) = Self::neighbours(composer, range);
        self.before == before && self.after == after
    }

    /// The text on either side of `range`, without its whitespace.
    fn neighbours(composer: &Composer, range: &Range<usize>) -> (String, String) {
        let text = composer.text();
        let start = range.start.min(text.len());
        let end = range.end.clamp(start, text.len());
        (text[..start].trim().to_string(), text[end..].trim().to_string())
    }
}

/// What a key did to the popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MentionKey {
    /// The popup took the key.
    Consumed,
    /// Enter with nothing to insert: the popup closed and the line submits
    /// as it would without a popup (Codex `submit_without_popup`).
    Submit,
    /// Not a popup key — the composer handles it.
    Passed,
}

/// The `@` machinery of one composer: the popup, the token it was closed on,
/// and the query the file search is running (Codex `popups.active` ·
/// `dismissed_mention_token` · `current_file_query`).
#[derive(Debug, Default)]
pub struct Mentions {
    popup: Option<MentionPopup>,
    dismissed: Option<DismissedToken>,
    current_file_query: Option<String>,
}

impl Mentions {
    #[must_use]
    pub const fn popup(&self) -> Option<&MentionPopup> {
        self.popup.as_ref()
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.popup.is_some()
    }

    /// The popup is gone and nothing is remembered — a new conversation.
    pub fn close(&mut self) {
        self.popup = None;
        self.dismissed = None;
        self.current_file_query = None;
    }

    /// Codex `sync_popups` → `sync_mentions_v2_popup`: after any change of
    /// the composer, the popup follows the `@` token under the cursor —
    /// opened with `catalog()` the moment one appears, closed when it goes.
    pub(crate) fn sync(
        &mut self,
        composer: &Composer,
        search: &FileSearchManager,
        catalog: impl FnOnce() -> Vec<Candidate>,
    ) {
        let Some((range, token)) = composer.at_token() else {
            if self.current_file_query.take().is_some() {
                search.on_user_query("");
            }
            self.dismissed = None;
            self.popup = None;
            return;
        };
        if self
            .dismissed
            .as_ref()
            .is_some_and(|dismissed| dismissed.matches(composer, &range, &token))
        {
            return;
        }
        if token.is_empty() {
            search.on_user_query("");
            self.current_file_query = None;
        } else {
            let new_popup = self.popup.is_none();
            if new_popup {
                // A fresh popup has no cached matches, and the manager can
                // retain an identical query. Reset it before issuing the query
                // so results arrive.
                search.on_user_query("");
            }
            if new_popup || self.current_file_query.as_deref() != Some(token.as_str()) {
                search.on_user_query(&token);
                self.current_file_query = Some(token.clone());
            }
        }
        if let Some(popup) = self.popup.as_mut() {
            popup.set_query(&token);
        } else {
            self.popup = Some(MentionPopup::new(
                catalog(),
                &token,
                search.page_root().map(Path::to_path_buf),
            ));
        }
        self.dismissed = None;
    }

    /// Codex `on_file_search_result`: a snapshot lands only while the person
    /// is still on a token that starts with its query.
    pub fn on_file_search_result(&mut self, composer: &Composer, query: &str, matches: Vec<FileMatch>) {
        let Some((_, token)) = composer.at_token() else {
            return;
        };
        if !token.starts_with(query) {
            return;
        }
        if let Some(popup) = self.popup.as_mut() {
            popup.set_file_matches(query, matches);
        }
    }

    /// Codex `handle_key_event_with_mentions_v2_popup`.
    #[must_use]
    pub fn key(&mut self, key: &KeyEvent, composer: &mut Composer) -> MentionKey {
        let Some(popup) = self.popup.as_mut() else {
            return MentionKey::Passed;
        };
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let plain = key.modifiers.is_empty();
        match key.code {
            KeyCode::Up => {
                popup.move_up();
                MentionKey::Consumed
            }
            KeyCode::Char('p') if control => {
                popup.move_up();
                MentionKey::Consumed
            }
            KeyCode::Down => {
                popup.move_down();
                MentionKey::Consumed
            }
            KeyCode::Char('n') if control => {
                popup.move_down();
                MentionKey::Consumed
            }
            KeyCode::Left if plain => {
                popup.previous_search_mode();
                MentionKey::Consumed
            }
            KeyCode::Right if plain => {
                popup.next_search_mode();
                MentionKey::Consumed
            }
            KeyCode::Esc => {
                if let Some((range, token)) = composer.at_token() {
                    self.dismissed = Some(DismissedToken::new(composer, &range, &token));
                }
                self.popup = None;
                MentionKey::Consumed
            }
            KeyCode::Tab => {
                let selected = popup.selected();
                self.complete(composer, selected);
                MentionKey::Consumed
            }
            KeyCode::Enter if plain => {
                let selected = popup.selected();
                let submit = selected.is_none();
                self.complete(composer, selected);
                if submit {
                    MentionKey::Submit
                } else {
                    MentionKey::Consumed
                }
            }
            _ => MentionKey::Passed,
        }
    }

    /// The `close_popup` tail of Codex's key handler: put the selection into
    /// the composer over the `@` token, then close.
    fn complete(&mut self, composer: &mut Composer, selected: Option<Selection>) {
        if let (Some(selected), Some((range, _))) = (selected, composer.at_token()) {
            match selected {
                Selection::File(path) => {
                    self.insert_selected_path(composer, range, &path.to_string_lossy());
                }
                Selection::Tool { insert_text } => {
                    self.insert_selected_mention(composer, range, &insert_text);
                }
            }
        }
        self.popup = None;
    }

    /// Codex `insert_selected_path`: the path replaces the whole `@token`,
    /// quoted when it holds whitespace so the prompt's own tokenizer keeps it
    /// one word, and the cursor rests after one separator.
    fn insert_selected_path(&mut self, composer: &mut Composer, token_range: Range<usize>, path: &str) {
        let needs_quotes = path.chars().any(char::is_whitespace);
        let inserted = if needs_quotes && !path.contains('"') {
            format!("\"{path}\"")
        } else {
            path.to_string()
        };
        self.insert_completion(composer, token_range, &inserted);
    }

    /// Codex `insert_selected_mention` — a skill or a page, text as given.
    fn insert_selected_mention(&mut self, composer: &mut Composer, token_range: Range<usize>, insert_text: &str) {
        self.insert_completion(composer, token_range, insert_text);
    }

    fn insert_completion(&mut self, composer: &mut Composer, token_range: Range<usize>, inserted: &str) {
        let start = token_range.start;
        composer.replace_range(token_range, inserted);
        let inserted_range = start..start.saturating_add(inserted.len());
        composer.set_cursor(inserted_range.end);
        advance_past_completion_separator(composer);
        self.dismiss_completed_prefixed_token(composer, MENTION_SIGIL, &inserted_range, inserted);
    }

    /// Codex `dismiss_completed_prefixed_token`: a completion that still
    /// starts with the sigil (`@wiki/…`) sits under a cursor whose token
    /// affinity would reopen the popup for it at once — that exact
    /// occurrence is dismissed.
    fn dismiss_completed_prefixed_token(
        &mut self,
        composer: &Composer,
        prefix: char,
        inserted_range: &Range<usize>,
        inserted_text: &str,
    ) {
        let Some(completed) = inserted_text.strip_prefix(prefix) else {
            return;
        };
        let Some((current_range, current_token)) = composer.at_token() else {
            return;
        };
        if current_range != *inserted_range || current_token != completed {
            return;
        }
        self.dismissed = Some(DismissedToken::new(composer, &current_range, &current_token));
    }
}

/// Codex `advance_past_completion_separator`: leaves the cursor after one
/// horizontal separator following a completion.
fn advance_past_completion_separator(composer: &mut Composer) {
    let cursor = composer.cursor();
    let existing_separator_len = composer.text()[cursor..]
        .chars()
        .next()
        .filter(|c| c.is_whitespace() && !is_line_break(*c))
        .map(char::len_utf8);
    if let Some(separator_len) = existing_separator_len {
        let after_separator = cursor + separator_len;
        let separator_precedes_suffix = composer.text()[after_separator..]
            .chars()
            .next()
            .is_some_and(|c| !c.is_whitespace());
        if separator_precedes_suffix {
            composer.replace_range(cursor..cursor, " ");
            composer.set_cursor(cursor + 1);
        } else {
            composer.set_cursor(after_separator);
        }
    } else {
        composer.replace_range(cursor..cursor, " ");
        composer.set_cursor(cursor + 1);
    }
}

// ---------------------------------------------------------------------------
// zo: a page mention becomes its body on submit
// ---------------------------------------------------------------------------

/// The `@wiki/<page>` mentions of `text`, unfolded: the text as typed, then
/// each page's body under a [`PAGE_ATTACHMENT_HEADER`] line. A word is a page
/// when it starts with the sigil and the page root's prefix, ends in the page
/// suffix, and names a file under the vault's page root — never a path that
/// climbs out of it. Text with no such word comes back untouched.
#[must_use]
pub fn expand_page_mentions(text: &str, roots: &[SearchRoot]) -> String {
    let Some(root) = roots.iter().find(|root| root.pages_only) else {
        return text.to_string();
    };
    let mut seen: Vec<String> = Vec::new();
    let mut attachments = String::new();
    for word in text.split_whitespace() {
        let word = word.trim_matches('"');
        let Some(mention) = word.strip_prefix(MENTION_SIGIL) else {
            continue;
        };
        let Some(relative) = mention.strip_prefix(root.prefix.as_str()) else {
            continue;
        };
        if !relative.ends_with(runtime::second_brain::PAGE_SUFFIX) || seen.iter().any(|page| page == mention) {
            continue;
        }
        let relative_path = Path::new(relative);
        if relative_path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(root.dir.join(relative_path)) else {
            continue;
        };
        seen.push(mention.to_string());
        attachments.push_str("\n\n[");
        attachments.push_str(PAGE_ATTACHMENT_HEADER);
        attachments.push_str(": ");
        attachments.push_str(mention);
        attachments.push_str("]\n");
        attachments.push_str(body.trim_end());
    }
    if attachments.is_empty() {
        return text.to_string();
    }
    format!("{text}{attachments}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::ansi::write_spans;
    use std::fs;

    fn file_match(path: &str, root: &Path, indices: Option<Vec<u32>>, score: u32) -> FileMatch {
        FileMatch {
            score,
            path: PathBuf::from(path),
            match_type: MatchType::File,
            root: root.to_path_buf(),
            full_path: root.join(path),
            indices,
        }
    }

    fn skill(name: &str, description: &str) -> SkillIndexEntry {
        SkillIndexEntry::new(
            name.to_string(),
            Some(description.to_string()),
            PathBuf::from(format!("/skills/{name}/SKILL.md")),
        )
    }

    #[test]
    fn a_file_row_is_name_then_dim_path_then_the_file_tag() {
        let root = Path::new("/repo");
        let mut popup = MentionPopup::new(Vec::new(), "comp", None);
        popup.set_file_matches(
            "comp",
            vec![file_match("src/tui/composer.rs", root, Some(vec![8, 9, 10, 11]), 90)],
        );
        let lines = popup.lines(120, palette::COMMAND_TOKEN);
        assert_eq!(lines.len(), 3, "one row, a blank, the footer");
        let row = lines[0].plain();
        assert!(row.starts_with("> composer.rs  src/tui/"), "{row:?}");
        assert!(row.ends_with("File  "), "{row:?}");
        assert_eq!(row.chars().count(), 120);
        assert_eq!(lines[1], Line::empty());
        let footer = lines[2].plain();
        assert!(footer.starts_with("  enter insert · esc close · ←/→ switch search modes"), "{footer:?}");
        assert!(footer.ends_with("[All Results]   Filesystem Only    Skills "), "{footer:?}");
        assert_eq!(footer.chars().count(), 120);
        // Narrow: the hints give way first, with an ellipsis, and the modes stay.
        let narrow = popup.lines(60, palette::COMMAND_TOKEN)[2].plain();
        assert!(narrow.contains('…'), "{narrow:?}");
        assert!(narrow.ends_with("[All Results]   Filesystem Only    Skills "), "{narrow:?}");
    }

    #[test]
    fn matched_characters_in_the_path_are_bold_and_the_name_is_cyan() {
        let root = Path::new("/repo");
        let mut popup = MentionPopup::new(Vec::new(), "tui", None);
        popup.set_file_matches(
            "tui",
            vec![
                file_match("src/tui/app.rs", root, Some(vec![4, 5, 6]), 80),
                file_match("src/tui/view.rs", root, Some(vec![4, 5, 6]), 70),
            ],
        );
        popup.move_down();
        assert_eq!(popup.selected_index(), Some(1));
        let lines = popup.lines(60, palette::COMMAND_TOKEN);
        let mut rendered = String::new();
        write_spans(&lines[0], &mut rendered);
        assert!(rendered.contains("\u{1b}[38;5;6;49mapp.rs"), "the name is cyan: {rendered:?}");
        assert!(rendered.contains("\u{1b}[1mtui\u{1b}[22m"), "matched path characters are bold: {rendered:?}");
        // The name column is as wide as the widest visible name (`view.rs`),
        // so the shorter name is padded to it before the two-column gap.
        assert!(lines[0].plain().starts_with("  app.rs   src/tui/"), "{:?}", lines[0].plain());
        assert!(lines[1].plain().starts_with("> view.rs  src/tui/"), "{:?}", lines[1].plain());
        let mut selected = String::new();
        write_spans(&lines[1], &mut selected);
        assert!(selected.starts_with("\u{1b}[1m\u{1b}[38;5;6;49m> view.rs"), "the selected row is bold cyan whole: {selected:?}");
    }

    #[test]
    fn a_root_file_shows_the_dot_slash_path() {
        let root = Path::new("/repo");
        let mut popup = MentionPopup::new(Vec::new(), "car", None);
        popup.set_file_matches("car", vec![file_match("Cargo.toml", root, Some(vec![0, 1, 2]), 90)]);
        let row = popup.lines(40, palette::COMMAND_TOKEN)[0].plain();
        assert!(row.starts_with("> Cargo.toml  ./"), "{row:?}");
    }

    #[test]
    fn the_empty_states_say_loading_then_no_matches() {
        let mut popup = MentionPopup::new(Vec::new(), "zzz", None);
        assert_eq!(popup.lines(40, palette::COMMAND_TOKEN)[0].plain(), format!("  {LOADING}"));
        popup.set_file_matches("zzz", Vec::new());
        assert_eq!(popup.lines(40, palette::COMMAND_TOKEN)[0].plain(), format!("  {NO_MATCHES}"));
        assert_eq!(popup.required_height(), 3);
        assert!(popup.selected().is_none());
    }

    #[test]
    fn stale_results_for_an_older_query_are_ignored() {
        let root = Path::new("/repo");
        let mut popup = MentionPopup::new(Vec::new(), "comp", None);
        popup.set_query("compo");
        popup.set_file_matches("comp", vec![file_match("src/composer.rs", root, None, 1)]);
        assert!(popup.rows().is_empty());
        popup.set_file_matches("compo", vec![file_match("src/composer.rs", root, None, 1)]);
        assert_eq!(popup.rows().len(), 1);
    }

    #[test]
    fn the_window_is_eight_rows_and_the_selection_wraps() {
        let root = Path::new("/repo");
        let matches: Vec<FileMatch> = (0..12)
            .map(|n| file_match(&format!("row{n:02}.rs"), root, None, 100 - n))
            .collect();
        let mut popup = MentionPopup::new(Vec::new(), "row", None);
        popup.set_file_matches("row", matches);
        assert_eq!(popup.rows().len(), MAX_POPUP_ROWS, "the popup keeps one page of results");
        assert_eq!(popup.required_height(), MAX_POPUP_ROWS + 2);
        assert_eq!(popup.lines(40, palette::COMMAND_TOKEN).len(), MAX_POPUP_ROWS + 2);
        popup.move_up();
        assert_eq!(popup.selected_index(), Some(MAX_POPUP_ROWS - 1));
        popup.move_down();
        assert_eq!(popup.selected_index(), Some(0));
    }

    #[test]
    fn skills_stand_before_files_and_the_modes_filter_them() {
        let root = Path::new("/repo");
        let catalog = build_search_catalog(&[skill("typesafe-ai", "typed judgments")]);
        let mut popup = MentionPopup::new(catalog, "ty", None);
        popup.set_file_matches("ty", vec![file_match("src/types.rs", root, None, 50)]);
        let rows = popup.rows();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].mention_type, MentionType::Skill);
        assert_eq!(rows[0].selection, Selection::Tool { insert_text: "$typesafe-ai".to_string() });
        assert_eq!(rows[1].mention_type, MentionType::File);
        let skill_row = popup.lines(60, palette::COMMAND_TOKEN)[0].plain();
        assert!(skill_row.contains("typesafe-ai  typed judgments"), "{skill_row:?}");
        assert!(skill_row.ends_with("Skill "), "{skill_row:?}");

        popup.next_search_mode();
        assert_eq!(popup.search_mode(), SearchMode::FilesystemOnly);
        assert_eq!(popup.rows().len(), 1);
        assert_eq!(popup.rows()[0].mention_type, MentionType::File);
        popup.next_search_mode();
        assert_eq!(popup.search_mode(), SearchMode::Tools);
        assert_eq!(popup.rows()[0].mention_type, MentionType::Skill);
        popup.next_search_mode();
        assert_eq!(popup.search_mode(), SearchMode::Results);
        popup.previous_search_mode();
        assert_eq!(popup.search_mode(), SearchMode::Tools);
        let footer = popup.lines(60, palette::COMMAND_TOKEN)[2].plain();
        assert!(footer.ends_with("[Skills]"), "{footer:?}");
    }

    #[test]
    fn a_bare_at_lists_the_catalog_without_any_file_search() {
        let catalog = build_search_catalog(&[skill("beta", ""), skill("alpha", "first")]);
        let popup = MentionPopup::new(catalog, "", None);
        let names: Vec<&str> = popup.rows().iter().map(|row| row.display_name.as_str()).collect();
        assert_eq!(names, ["alpha", "beta"], "an empty filter sorts by name");
        assert!(popup.rows()[1].description.is_none(), "a blank description is none");
    }

    #[test]
    fn a_vault_page_is_tagged_vault_and_inserts_with_its_sigil() {
        let repo = Path::new("/repo");
        let vault = Path::new("/vault/wiki");
        let mut popup = MentionPopup::new(Vec::new(), "alpha", Some(vault.to_path_buf()));
        popup.set_file_matches(
            "alpha",
            vec![
                file_match("src/alpha.rs", repo, None, 90),
                FileMatch {
                    score: 80,
                    path: PathBuf::from("wiki/alpha.md"),
                    match_type: MatchType::File,
                    root: vault.to_path_buf(),
                    full_path: vault.join("alpha.md"),
                    indices: None,
                },
            ],
        );
        let page = &popup.rows()[1];
        assert_eq!(page.mention_type, MentionType::Page);
        assert_eq!(page.selection, Selection::Tool { insert_text: "@wiki/alpha.md".to_string() });
        let row = popup.lines(60, palette::COMMAND_TOKEN)[1].plain();
        assert!(row.starts_with("  alpha.md  wiki/"), "{row:?}");
        assert!(row.ends_with("Vault "), "{row:?}");
    }

    #[test]
    fn a_long_row_is_cut_with_an_ellipsis_and_keeps_its_tag() {
        let root = Path::new("/repo");
        let mut popup = MentionPopup::new(Vec::new(), "x", None);
        popup.set_file_matches(
            "x",
            vec![file_match("a/very/long/directory/chain/that/goes/on/and/on/x.rs", root, None, 1)],
        );
        let row = popup.lines(30, palette::COMMAND_TOKEN)[0].plain();
        assert_eq!(row.chars().count(), 30, "{row:?}");
        assert!(row.contains('…'), "{row:?}");
        assert!(row.ends_with("File  "), "{row:?}");
    }

    fn composer_with(text: &str, cursor: usize) -> Composer {
        let mut composer = Composer::new();
        composer.insert_str(text);
        composer.set_cursor(cursor);
        composer
    }

    #[test]
    fn a_completed_file_path_replaces_the_token_and_leaves_one_separator() {
        let mut mentions = Mentions::default();
        let mut composer = composer_with("look at @comp please", "look at @comp".len());
        mentions.popup = Some(MentionPopup::new(Vec::new(), "comp", None));
        mentions
            .popup
            .as_mut()
            .expect("popup")
            .set_file_matches("comp", vec![file_match("src/composer.rs", Path::new("/repo"), None, 1)]);
        let outcome = mentions.key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &mut composer);
        assert_eq!(outcome, MentionKey::Consumed);
        assert_eq!(composer.text(), "look at src/composer.rs  please");
        assert_eq!(composer.cursor(), "look at src/composer.rs ".len());
        assert!(!mentions.is_open());
    }

    #[test]
    fn a_path_with_whitespace_is_quoted_and_a_trailing_token_gets_a_space() {
        let mut mentions = Mentions::default();
        let mut composer = composer_with("@my", 3);
        mentions.popup = Some(MentionPopup::new(Vec::new(), "my", None));
        mentions
            .popup
            .as_mut()
            .expect("popup")
            .set_file_matches("my", vec![file_match("my notes/todo.md", Path::new("/repo"), None, 1)]);
        let outcome = mentions.key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut composer);
        assert_eq!(outcome, MentionKey::Consumed);
        assert_eq!(composer.text(), "\"my notes/todo.md\" ");
        assert_eq!(composer.cursor(), composer.text().len());
    }

    #[test]
    fn enter_with_nothing_selected_closes_and_submits() {
        let mut mentions = Mentions::default();
        let mut composer = composer_with("@zzz", 4);
        mentions.popup = Some(MentionPopup::new(Vec::new(), "zzz", None));
        let outcome = mentions.key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &mut composer);
        assert_eq!(outcome, MentionKey::Submit);
        assert_eq!(composer.text(), "@zzz");
        assert!(!mentions.is_open());
    }

    #[test]
    fn a_completed_page_mention_keeps_its_sigil_and_does_not_reopen() {
        let vault = Path::new("/vault/wiki");
        let mut mentions = Mentions::default();
        let mut composer = composer_with("@alp", 4);
        mentions.popup = Some(MentionPopup::new(Vec::new(), "alp", Some(vault.to_path_buf())));
        mentions.popup.as_mut().expect("popup").set_file_matches(
            "alp",
            vec![FileMatch {
                score: 1,
                path: PathBuf::from("wiki/alpha.md"),
                match_type: MatchType::File,
                root: vault.to_path_buf(),
                full_path: vault.join("alpha.md"),
                indices: None,
            }],
        );
        let outcome = mentions.key(&KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &mut composer);
        assert_eq!(outcome, MentionKey::Consumed);
        assert_eq!(composer.text(), "@wiki/alpha.md ");
        let (range, token) = composer.at_token().expect("the cursor still has affinity to the token");
        assert_eq!(token, "wiki/alpha.md");
        assert!(mentions
            .dismissed
            .as_ref()
            .expect("the completed token is dismissed")
            .matches(&composer, &range, &token));
    }

    #[test]
    fn esc_dismisses_this_occurrence_until_the_token_changes() {
        let mut mentions = Mentions::default();
        let mut composer = composer_with("@ma", 3);
        mentions.popup = Some(MentionPopup::new(Vec::new(), "ma", None));
        assert_eq!(mentions.key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &mut composer), MentionKey::Consumed);
        assert!(!mentions.is_open());
        let (range, token) = composer.at_token().expect("token");
        let dismissed = mentions.dismissed.clone().expect("dismissed");
        assert!(dismissed.matches(&composer, &range, &token));
        // Leading whitespace shifts the range, not the token: still dismissed.
        composer.set_cursor(0);
        composer.insert_str("   ");
        composer.set_cursor(composer.text().len());
        let (range, token) = composer.at_token().expect("token");
        assert!(dismissed.matches(&composer, &range, &token));
        // A second identical token later is another occurrence.
        composer.insert_str(" @ma");
        let (range, token) = composer.at_token().expect("token");
        assert!(!dismissed.matches(&composer, &range, &token));
        // Typing into the token changes it.
        composer.set_cursor("   @ma".len());
        composer.insert_char('i');
        let (range, token) = composer.at_token().expect("token");
        assert!(!dismissed.matches(&composer, &range, &token));
    }

    #[test]
    fn page_mentions_unfold_into_the_prompt_and_nothing_else_moves() {
        let vault = tempfile::tempdir().expect("vault");
        let wiki = vault.path().join("wiki");
        fs::create_dir_all(wiki.join("deep")).expect("wiki");
        fs::write(wiki.join("alpha.md"), "# Alpha\n\nbody\n").expect("page");
        fs::write(wiki.join("deep/beta.md"), "beta body").expect("page");
        let roots = vec![SearchRoot::repo("/repo"), SearchRoot::pages(&wiki, "wiki/")];
        let text = "read @wiki/alpha.md and \"@wiki/deep/beta.md\" and @wiki/alpha.md again";
        let expanded = expand_page_mentions(text, &roots);
        assert!(expanded.starts_with(text), "{expanded:?}");
        assert_eq!(
            expanded,
            format!("{text}\n\n[{PAGE_ATTACHMENT_HEADER}: wiki/alpha.md]\n# Alpha\n\nbody\n\n[{PAGE_ATTACHMENT_HEADER}: wiki/deep/beta.md]\nbeta body")
        );
        assert_eq!(expand_page_mentions("no pages here", &roots), "no pages here");
        assert_eq!(
            expand_page_mentions("@wiki/../secret.md", &roots),
            "@wiki/../secret.md",
            "a path that climbs out of the vault is text"
        );
        assert_eq!(expand_page_mentions("@wiki/missing.md", &roots), "@wiki/missing.md");
        assert_eq!(
            expand_page_mentions("@wiki/alpha.md", &[SearchRoot::repo("/repo")]),
            "@wiki/alpha.md",
            "no vault, no unfolding"
        );
    }
}
