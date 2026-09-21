//! `›` 컴포저 — 한 줄 편집기(여러 줄 붙여넣기 포함).
//!
//! UTF-8 안전이 요점이다. 커서는 **바이트 오프셋**이지만 이동은 언제나 문자
//! 경계로만 한다. 한글은 한 음절이 한 `char` 이고 표시 폭이 둘이므로, 화면
//! 좌표 계산은 `unicode-width` 로 따로 잰다 — IME 조합이 끝나 확정된 글자만
//! 여기 들어온다(터미널이 조합 중 바이트를 부분으로 흘려도 crossterm 이
//! 완성된 `KeyCode::Char` 로만 준다).

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use super::ansi::{char_width, Color, Line, Span, Style};
use super::clipboard_paste::normalize_pasted_path;
use super::effort_effect::{idle_border_style, EffortEffect, EffortTier};
use super::palette;

/// 컴포저 caret **글자 하나**. 캡처의 `›` 그대로.
///
/// 뒤의 빈칸은 여기 없다 — codex 는 그 둘을 다른 스팬으로 낸다. 마커는
/// `buf.set_span(textarea_rect.x - LIVE_PREFIX_COLS, …, &prompt, …)` 로
/// 두 칸 왼쪽에 **한 글자만** 찍히고(`bottom_pane/chat_composer.rs`,
/// `prompt` 는 `"›".bold()` 또는 tier 의 `prompt_glyph()` — `"›"`/`"»"`),
/// 남는 한 칸은 아무 스타일도 없는 텍스트 영역의 여백이다
/// (`ui_consts.rs::LIVE_PREFIX_COLS = 2`).
///
/// 캡처가 그 경계를 그대로 증언한다 —
/// `codex-tui-v0.150.1-status.bin`·`…-inventory.bin` 의 컴포저 행:
///
/// ```text
/// ESC[1m › ESC[22m " " ESC[2m Ask Codex to do anything
/// ```
///
/// tier 색이 붙은 갈래도 같다(`codex-tui-v0.149.1-turn.bin`):
///
/// ```text
/// ESC[1m ESC[38;2;255;178;66;49m › ESC[22m ESC[39;49m " " ESC[2m Ask Codex to do anything
/// ```
///
/// 굵기와 색은 `›` 에서 **끝난다**. 빈칸까지 한 스팬으로 묶으면 그 칸이 색을
/// 물려받아 SGR 이 한 칸 길어진다 — 눈에는 같아 보여도 바이트가 다르고,
/// 그것이 `docs/codex-tui-mechanics.md` 의 잔여 ① 이었다.
pub const CARET: &str = "›";
/// 마커와 본문 사이의 빈칸 — 스타일 없는 스팬 하나. [`CARET`] 참고.
pub const CARET_GAP: &str = " ";
/// caret 이 먹는 **칸** 수. `›` 는 UTF-8 3바이트지만 한 칸이고, 뒤 빈칸까지
/// 두 칸이다 — 바이트로 재면 본문 자리가 두 칸 좁아진다.
pub const CARET_WIDTH: usize = 2;

/// 컴포저가 제출할 텍스트와 이미지 첨부. 이미지는 PNG 임시 파일로 보관하다가
/// 세션 입력 경계에서 base64 이미지 블록으로 바뀐다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub text: String,
    pub image_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalImage {
    placeholder: String,
    path: PathBuf,
}

const HISTORY_FILE_NAME: &str = "history.jsonl";

/// Codex-compatible global history row. Keeping the same three fields makes
/// `~/.zo/history.jsonl` inspectable with the same tools as Codex's file.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct HistoryEntry {
    session_id: String,
    ts: u64,
    text: String,
}

#[derive(Debug)]
struct HistoryFile {
    path: PathBuf,
    session_id: String,
}

/// How long the composer will wait for a pasted path to prove it is an image.
///
/// Generous for a local decode (measured in microseconds even for a 2 GiB
/// file) and short enough that a path which cannot answer never becomes a
/// freeze the keyboard cannot escape.
const IMAGE_PROBE_BUDGET: std::time::Duration = std::time::Duration::from_millis(100);

/// Whether `path` decodes as an image, giving up after `budget`.
///
/// The probe runs on its own thread because both halves of it — the metadata
/// call and the open — can block indefinitely on a path the user did not
/// choose carefully. On timeout the thread is left behind rather than waited
/// on: it is a single thread parked in the kernel that ends when the mount or
/// reader does, and leaking one is strictly better than freezing the UI.
fn decodes_as_image_within(path: &std::path::Path, budget: std::time::Duration) -> bool {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let probe = path.to_path_buf();
    if std::thread::Builder::new()
        .name("zo-image-probe".to_string())
        .spawn(move || {
            // A FIFO, socket, or device node is never an image, and asking the
            // decoder to open one is what hangs. Reject by file type first.
            let regular = std::fs::metadata(&probe).is_ok_and(|meta| meta.is_file());
            let _ = tx.send(regular && image::image_dimensions(&probe).is_ok());
        })
        .is_err()
    {
        return false;
    }
    rx.recv_timeout(budget).unwrap_or(false)
}

/// 편집 상태 + 제출 히스토리.
#[derive(Debug, Default)]
pub struct Composer {
    text: String,
    /// 커서의 바이트 오프셋. 언제나 문자 경계에 있다.
    cursor: usize,
    history: Vec<String>,
    /// `None` for secondary/test composers; the interactive app binds this to
    /// the one global `~/.zo/history.jsonl` store.
    history_file: Option<HistoryFile>,
    /// 히스토리를 훑는 중이면 그 인덱스와, 훑기 전 편집 중이던 원문.
    browsing: Option<usize>,
    draft: String,
    images: Vec<LocalImage>,
}

impl Composer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Interactive composer bound to the canonical per-user Zo history.
    #[must_use]
    pub(crate) fn for_session(session_id: &str, seed: Vec<String>) -> Self {
        Self::with_history_file(
            runtime::default_config_home().join(HISTORY_FILE_NAME),
            session_id,
            seed,
        )
    }

    fn with_history_file(
        path: PathBuf,
        session_id: impl Into<String>,
        seed: Vec<String>,
    ) -> Self {
        let session_id = session_id.into();
        let mut history = load_history(&path)
            .into_iter()
            .map(|entry| entry.text)
            .collect::<Vec<_>>();
        extend_history(&mut history, seed);
        Self {
            history,
            history_file: Some(HistoryFile { path, session_id }),
            ..Self::default()
        }
    }

    /// Rebind `/resume` to its session id and authoritative transcript seed.
    /// Reloading also includes prompts appended by another Zo process since
    /// this composer started.
    pub(crate) fn bind_session(&mut self, session_id: &str, seed: Vec<String>) {
        let path = self.history_file.as_ref().map_or_else(
            || runtime::default_config_home().join(HISTORY_FILE_NAME),
            |history| history.path.clone(),
        );
        *self = Self::with_history_file(path, session_id, seed);
    }

    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn insert_char(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.browsing = None;
    }

    pub fn insert_str(&mut self, text: &str) {
        self.text.insert_str(self.cursor, text);
        self.cursor += text.len();
        self.browsing = None;
    }

    /// Replace `range` with `text`, cursor and attachments kept sane — the
    /// `@` popup's completion road (codex `TextArea::replace_range`).
    ///
    /// The cursor stays where it points: before the range it does not move,
    /// inside or at the range's end it lands after the replacement, past the
    /// range it shifts by the length difference. An image placeholder that
    /// the range cuts through loses its attachment, as a deletion would.
    pub fn replace_range(&mut self, range: std::ops::Range<usize>, text: &str) {
        let range = range.start.min(self.text.len())..range.end.min(self.text.len());
        let range = self.boundary_before(range.start)..self.boundary_before(range.end);
        if self.cursor >= range.end {
            self.cursor = self.cursor - range.end + range.start + text.len();
        } else if self.cursor > range.start {
            self.cursor = range.start + text.len();
        }
        self.text.replace_range(range, text);
        self.remove_missing_images();
        self.browsing = None;
    }

    /// Put the cursor at byte `at`, pulled back to a character boundary.
    pub fn set_cursor(&mut self, at: usize) {
        self.cursor = self.boundary_before(at);
    }

    /// The `@token` the cursor is on — codex
    /// `completion_target::current_prefixed_token_range(textarea, '@', allow_empty)`.
    ///
    /// A token is one whitespace-delimited word that starts with `@`. The
    /// cursor is on it anywhere from its `@` to just past its last character,
    /// and — codex's "token affinity" — one horizontal separator past it too,
    /// which is where a completion leaves the cursor. A cursor that stands
    /// right before a word belongs to that word, not the one before. The
    /// range covers the `@`; the string does not. A second `@` inside the word
    /// stays part of it (`@scope/pkg@latest`, `@icon@2x.png`), a word with `@`
    /// in its middle is no token (`foo@bar`), and a bare `@` is the empty
    /// query only while nothing follows it — `test @ world` is prose.
    #[must_use]
    pub fn at_token(&self) -> Option<(std::ops::Range<usize>, String)> {
        let cursor = self.cursor.min(self.text.len());
        let before = self.text[..cursor].chars().next_back();
        let at = self.text[cursor..].chars().next();
        let start = if before.is_some_and(|ch| !ch.is_whitespace()) {
            self.word_start(cursor)
        } else if at.is_some_and(|ch| !ch.is_whitespace()) {
            cursor
        } else {
            // On or past one separator: the word it follows, if any.
            let separator = before.filter(|ch| ch.is_whitespace() && !is_line_break(*ch))?;
            let word_end = cursor - separator.len_utf8();
            if self.text[..word_end]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace)
            {
                return None;
            }
            self.word_start(word_end)
        };
        let end = self.text[start..]
            .char_indices()
            .find(|(_, ch)| ch.is_whitespace())
            .map_or(self.text.len(), |(at, _)| start + at);
        let token = self.text[start..end].strip_prefix('@')?;
        if token.is_empty() && self.text[end..].chars().any(|ch| !ch.is_whitespace()) {
            return None;
        }
        Some((start..end, token.to_string()))
    }

    /// The byte where the word that `inside` is in (or just past) begins.
    fn word_start(&self, inside: usize) -> usize {
        self.text[..inside]
            .char_indices()
            .rev()
            .find(|(_, ch)| ch.is_whitespace())
            .map_or(0, |(at, ch)| at + ch.len_utf8())
    }

    /// `at`, or the start of the character it points into.
    fn boundary_before(&self, at: usize) -> usize {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    /// Text the person pasted, made safe for a composer that writes its
    /// bytes straight to the terminal — see `normalize_pasted_text`.
    pub fn insert_pasted(&mut self, text: &str) {
        self.insert_str(&normalize_pasted_text(text));
    }

    /// 앞 글자 지우기. 문자 경계로만 물러난다.
    pub fn backspace(&mut self) {
        let Some(previous) = self.previous_boundary(self.cursor) else {
            return;
        };
        if self.remove_image_intersecting(previous..self.cursor) {
            return;
        }
        self.text.replace_range(previous..self.cursor, "");
        self.cursor = previous;
        self.browsing = None;
    }

    /// 커서 위 글자 지우기.
    pub fn delete(&mut self) {
        let Some(next) = self.next_boundary(self.cursor) else {
            return;
        };
        if self.remove_image_intersecting(self.cursor..next) {
            return;
        }
        self.text.replace_range(self.cursor..next, "");
        self.browsing = None;
    }

    pub fn left(&mut self) {
        if let Some(previous) = self.previous_boundary(self.cursor) {
            self.cursor = previous;
        }
    }

    pub fn right(&mut self) {
        if let Some(next) = self.next_boundary(self.cursor) {
            self.cursor = next;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.len();
    }

    /// 낱말 하나 왼쪽으로 — 공백을 먼저 건너뛰고 그다음 비공백을 지난다.
    pub fn word_left(&mut self) {
        while let Some(previous) = self.previous_boundary(self.cursor) {
            if self.text[previous..self.cursor].starts_with(' ') {
                self.cursor = previous;
            } else {
                break;
            }
        }
        while let Some(previous) = self.previous_boundary(self.cursor) {
            if self.text[previous..self.cursor].starts_with(' ') {
                break;
            }
            self.cursor = previous;
        }
    }

    pub fn word_right(&mut self) {
        while let Some(next) = self.next_boundary(self.cursor) {
            if self.text[self.cursor..next].starts_with(' ') {
                self.cursor = next;
            } else {
                break;
            }
        }
        while let Some(next) = self.next_boundary(self.cursor) {
            if self.text[self.cursor..next].starts_with(' ') {
                break;
            }
            self.cursor = next;
        }
    }

    /// 커서 앞의 낱말 하나 지우기 (Ctrl-W).
    pub fn kill_word_left(&mut self) {
        let end = self.cursor;
        self.word_left();
        self.text.replace_range(self.cursor..end, "");
        self.remove_missing_images();
        self.browsing = None;
    }

    /// 커서 앞 전부 지우기 (Ctrl-U).
    pub fn kill_to_start(&mut self) {
        self.text.replace_range(..self.cursor, "");
        self.cursor = 0;
        self.remove_missing_images();
        self.browsing = None;
    }

    /// 커서 뒤 전부 지우기 (Ctrl-K).
    pub fn kill_to_end(&mut self) {
        self.text.truncate(self.cursor);
        self.remove_missing_images();
        self.browsing = None;
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.browsing = None;
        self.images.clear();
    }

    /// Replace the current draft with a previously queued submission.
    ///
    /// The text already contains Codex-style image placeholders, so restoring
    /// must rebuild the attachment records without inserting the labels a
    /// second time.
    pub fn restore_submission(&mut self, submission: Submission) {
        self.text = submission.text;
        self.cursor = self.text.len();
        self.browsing = None;
        self.images = submission
            .image_paths
            .into_iter()
            .enumerate()
            .map(|(index, path)| LocalImage {
                placeholder: image_placeholder(index + 1),
                path,
            })
            .collect();
    }

    /// Insert a Codex-style local image placeholder and hold its PNG for submission.
    pub fn attach_image(&mut self, path: PathBuf) {
        let placeholder = image_placeholder(self.images.len() + 1);
        self.insert_str(&placeholder);
        self.images.push(LocalImage { placeholder, path });
    }

    /// Attach a pasted path only when the image decoder recognizes its file.
    ///
    /// Returns `false` for ordinary paths, missing files, and multi-token
    /// pastes so the event handler can insert the original text unchanged.
    ///
    /// The decode probe is **bounded**. It runs on the event loop, and opening
    /// an arbitrary pasted path can block forever — a FIFO, a stalled NFS/SMB
    /// mount, a device node. Measured: a paste of a FIFO path froze the whole
    /// TUI with no way back from the keyboard, and killing it from outside left
    /// the user's shell in raw mode because the teardown never ran. A real
    /// image answers far inside the budget (a 2 GiB file returned with no
    /// delay — the decoder reads the header, not the body), so anything that
    /// does not answer in time is text as far as the composer is concerned.
    pub fn handle_paste_image_path(&mut self, pasted: &str) -> bool {
        let Some(path) = normalize_pasted_path(pasted) else {
            return false;
        };
        if !decodes_as_image_within(&path, IMAGE_PROBE_BUDGET) {
            return false;
        }
        self.attach_image(path);
        true
    }

    /// 제출 — 본문과 이미지 첨부를 꺼내고 텍스트는 히스토리에 남긴다.
    pub fn submit(&mut self) -> Submission {
        let text = std::mem::take(&mut self.text);
        let images = std::mem::take(&mut self.images);
        self.cursor = 0;
        self.browsing = None;
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            if self.history.last().map(String::as_str) != Some(trimmed) {
                self.history.push(trimmed.to_string());
            }
            if let Some(history) = &self.history_file {
                if let Err(error) = append_history(history, trimmed) {
                    eprintln!(
                        "zo: could not append composer history {}: {error}",
                        history.path.display()
                    );
                }
            }
        }
        Submission {
            text,
            image_paths: images.into_iter().map(|image| image.path).collect(),
        }
    }

    /// 이전 제출로. 히스토리 끝이면 아무것도 하지 않는다.
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let index = match self.browsing {
            None => {
                self.draft = self.text.clone();
                self.history.len() - 1
            }
            Some(0) => return,
            Some(index) => index - 1,
        };
        self.browsing = Some(index);
        self.text = self.history[index].clone();
        self.cursor = self.text.len();
        self.images.clear();
    }

    /// 다음 제출로. 끝까지 오면 훑기 전 원고로 돌아간다.
    pub fn history_next(&mut self) {
        let Some(index) = self.browsing else {
            return;
        };
        if index + 1 >= self.history.len() {
            self.browsing = None;
            self.text = std::mem::take(&mut self.draft);
            self.cursor = self.text.len();
            self.images.clear();
            return;
        }
        self.browsing = Some(index + 1);
        self.text = self.history[index + 1].clone();
        self.cursor = self.text.len();
        self.images.clear();
    }

    fn previous_boundary(&self, from: usize) -> Option<usize> {
        if from == 0 {
            return None;
        }
        self.text[..from].char_indices().next_back().map(|(at, _)| at)
    }

    fn next_boundary(&self, from: usize) -> Option<usize> {
        self.text[from..]
            .chars()
            .next()
            .map(|ch| from + ch.len_utf8())
    }

    /// Placeholders are atomic like Codex text elements: deleting any byte in
    /// one removes that attachment, then the remaining labels close the gap.
    fn remove_image_intersecting(&mut self, deleted: std::ops::Range<usize>) -> bool {
        let Some((index, start, end)) = self.images.iter().enumerate().find_map(|(index, image)| {
            self.text
                .find(&image.placeholder)
                .map(|start| (index, start, start + image.placeholder.len()))
                .filter(|(_, start, end)| deleted.start < *end && deleted.end > *start)
        }) else {
            return false;
        };
        self.text.replace_range(start..end, "");
        self.cursor = start;
        self.images.remove(index);
        self.renumber_images();
        self.browsing = None;
        true
    }

    fn renumber_images(&mut self) {
        for (index, image) in self.images.iter_mut().enumerate() {
            let replacement = image_placeholder(index + 1);
            if image.placeholder == replacement {
                continue;
            }
            let Some(start) = self.text.find(&image.placeholder) else {
                continue;
            };
            let end = start + image.placeholder.len();
            let old_len = image.placeholder.len();
            self.text.replace_range(start..end, &replacement);
            if self.cursor >= end {
                self.cursor = if replacement.len() >= old_len {
                    self.cursor + replacement.len() - old_len
                } else {
                    self.cursor - (old_len - replacement.len())
                };
            } else if self.cursor > start {
                self.cursor = start + replacement.len();
            }
            image.placeholder = replacement;
        }
    }

    fn remove_missing_images(&mut self) {
        self.images
            .retain(|image| self.text.contains(&image.placeholder));
        self.renumber_images();
    }

    /// 화면에 그릴 줄들과, 커서의 뷰포트 상대 좌표 `(col, row)`.
    ///
    /// Codex 0.150.1에는 컴포저 테두리가 없지만 사용자가 경계를 명시적으로
    /// 요청했으므로 이 블록에만 둥근 상자를 그리는 의도된 이탈이다. 상자 두
    /// 칸을 먼저 떼고, 그 안에서 다시 caret 두 칸을 떼므로 기존 본문·이미지
    /// placeholder 폭 계약은 그대로다. 피커·히스토리 셀에는 이 선이 없다.
    #[must_use]
    pub fn render(&self, width: usize, placeholder: &str) -> (Vec<Line>, (u16, u16)) {
        self.render_with_effort(width, placeholder, None, None, Instant::now())
    }

    #[must_use]
    pub fn render_with_effort(
        &self,
        width: usize,
        placeholder: &str,
        tier: Option<EffortTier>,
        effect: Option<&EffortEffect>,
        now: Instant,
    ) -> (Vec<Line>, (u16, u16)) {
        self.render_with_effort_for_palette(
            width,
            placeholder,
            tier,
            effect,
            now,
            palette::terminal_palette(),
        )
    }

    fn render_with_effort_for_palette(
        &self,
        width: usize,
        placeholder: &str,
        tier: Option<EffortTier>,
        effect: Option<&EffortEffect>,
        now: Instant,
        terminal_palette: Option<palette::TerminalPalette>,
    ) -> (Vec<Line>, (u16, u16)) {
        let caret = tier.map_or_else(
            || Span::new(CARET, Style::new().bold()),
            EffortTier::caret_span,
        );
        let composer_style = composer_style_for_palette(terminal_palette);
        // 여섯 칸보다 좁으면 양쪽 선, 두 칸 caret, 폭 2짜리 글자를 함께 세울
        // 수 없다. 이때만 테두리를 접고 줄과 커서를 주어진 폭 안으로 제한한다.
        if width < 6 {
            let (lines, (col, row)) = self.render_content(width, placeholder, caret);
            let lines = lines
                .into_iter()
                .map(|line| line.truncated(width).styled(composer_style))
                .collect();
            let col = col.min(u16::try_from(width.saturating_sub(1)).unwrap_or(u16::MAX));
            return (lines, (col, row));
        }

        let inner_width = width - 2;
        let (content, (col, row)) = self.render_content(inner_width, placeholder, caret);
        let border_style = effect.map_or_else(idle_border_style, |effect| {
            effect.border_style_at(now)
        });
        let mut lines = Vec::with_capacity(content.len() + 2);
        lines.push(Line::new(vec![Span::new(
            format!("╭{}╮", "─".repeat(inner_width)),
            border_style,
        )]));
        for mut line in content {
            let padding = inner_width.saturating_sub(line.width());
            line.spans
                .insert(0, Span::new("│", border_style));
            if padding > 0 {
                line.spans.push(Span::raw(" ".repeat(padding)));
            }
            line.spans.push(Span::new("│", border_style));
            lines.push(line);
        }
        lines.push(Line::new(vec![Span::new(
            format!("╰{}╯", "─".repeat(inner_width)),
            border_style,
        )]));
        for line in &mut lines {
            line.style = composer_style;
        }
        (lines, (col.saturating_add(1), row.saturating_add(1)))
    }

    fn render_content(
        &self,
        width: usize,
        placeholder: &str,
        caret: Span,
    ) -> (Vec<Line>, (u16, u16)) {
        let content = width.saturating_sub(CARET_WIDTH).max(1);
        if self.text.is_empty() {
            let line = Line::new(vec![
                caret,
                Span::raw(CARET_GAP),
                Span::dim(placeholder.to_string()),
            ])
            .truncated(width);
            return (vec![line], (2, 0));
        }

        // 커서 좌표는 본문을 실제로 훑으며 잰다 — 접는 규칙과 어긋나지 않도록
        // 같은 순회 안에서 판단한다.
        let mut rows: Vec<Vec<Span>> = vec![Vec::new()];
        let mut used = 0usize;
        let mut cursor_at = (2u16, 0u16);
        let mut offset = 0usize;
        for ch in self.text.chars() {
            if offset == self.cursor {
                cursor_at = position(rows.len() - 1, used);
            }
            if ch == '\n' {
                rows.push(Vec::new());
                used = 0;
                offset += ch.len_utf8();
                continue;
            }
            let cell = char_width(ch);
            if used + cell > content {
                rows.push(Vec::new());
                used = 0;
            }
            match rows.last_mut().and_then(|row| row.last_mut()) {
                Some(last) if last.style == Style::new() => last.text.push(ch),
                _ => {
                    if let Some(row) = rows.last_mut() {
                        row.push(Span::raw(ch.to_string()));
                    }
                }
            }
            used += cell;
            offset += ch.len_utf8();
        }
        if offset == self.cursor {
            cursor_at = position(rows.len() - 1, used);
        }

        let lines = rows
            .into_iter()
            .enumerate()
            .map(|(index, spans)| {
                let mut line = Line::new(spans);
                if index == 0 {
                    // 마커와 빈칸은 다른 스팬이다 — 스타일이 `›` 에서 끝난다.
                    line = line.prefixed(Span::raw(CARET_GAP)).prefixed(caret.clone());
                } else {
                    line = line.prefixed(Span::raw("  "));
                }
                line
            })
            .collect();
        (lines, cursor_at)
    }
}

/// Whitespace that ends a line — a separator a completion never rests on
/// (codex `advance_past_completion_separator`'s list).
#[must_use]
pub const fn is_line_break(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\r' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

fn extend_history(history: &mut Vec<String>, entries: impl IntoIterator<Item = String>) {
    for entry in entries {
        let entry = entry.trim();
        if !entry.is_empty() && history.last().map(String::as_str) != Some(entry) {
            history.push(entry.to_string());
        }
    }
}

fn load_history(path: &Path) -> Vec<HistoryEntry> {
    let Ok(contents) = core_types::paths::read_private_file(path) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&contents)
        .lines()
        .filter_map(|line| serde_json::from_str::<HistoryEntry>(line).ok())
        .filter(|entry| !entry.text.trim().is_empty())
        .collect()
}

fn append_history(history: &HistoryFile, text: &str) -> std::io::Result<()> {
    if let Some(parent) = history
        .path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        if parent.is_absolute() {
            runtime::secure_fs::ensure_private_dir_absolute(parent, 1)?;
        } else {
            std::fs::create_dir_all(parent)?;
            core_types::paths::restrict_permissions_owner_only(parent)?;
        }
    }
    let record = HistoryEntry {
        session_id: history.session_id.clone(),
        ts: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs()),
        text: text.to_string(),
    };
    let mut line = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
    line.push(b'\n');
    core_types::paths::append_private_file(&history.path, &line)
}

fn composer_style_for_palette(terminal_palette: Option<palette::TerminalPalette>) -> Style {
    let Some(terminal_palette) = terminal_palette else {
        return Style::new();
    };
    let background = terminal_palette.background();
    let (overlay, alpha) = if terminal_palette.has_light_background() {
        ((0, 0, 0), 0.04)
    } else {
        ((255, 255, 255), 0.12)
    };
    let (red, green, blue) = palette::blend_rgb(overlay, background, alpha);
    Style::new().bg(Color::Rgb(red, green, blue))
}

fn image_placeholder(number: usize) -> String {
    format!("[Image #{number}]")
}

/// How many columns a pasted tab becomes. The composer has no tab stops —
/// it counts cells — so a raw tab would let the terminal choose a column the
/// composer never saw.
pub(crate) const PASTE_TAB_SPACES: usize = 4;

/// Pasted text, as the composer can hold it. The composer writes its text to
/// the terminal byte for byte and measures every char with `unicode-width`,
/// which counts control characters as nothing; a raw tab, carriage return or
/// escape therefore moves the terminal's cursor without moving the
/// composer's, and the caret parts from the glyphs. Line ends become `\n`
/// (CRLF and a lone CR alike), a tab becomes [`PASTE_TAB_SPACES`] spaces, and
/// every other C0 control and DEL is dropped — an SGR escape pasted from a
/// terminal must not restyle the composer.
pub(crate) fn normalize_pasted_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' => out.push('\n'),
            '\t' => out.extend(std::iter::repeat_n(' ', PASTE_TAB_SPACES)),
            '\u{1b}' => skip_escape_sequence(&mut chars),
            control if control.is_control() => {}
            other => out.push(other),
        }
    }
    out
}

/// Step over the sequence an ESC opens, so `ESC[31m` leaves no `[31m`
/// behind: a CSI runs to its final byte (`@`..`~`), an OSC to BEL or ST, and
/// any other escape takes exactly one following char.
fn skip_escape_sequence(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        Some('[') => {
            for ch in chars.by_ref() {
                if ('@'..='~').contains(&ch) {
                    break;
                }
            }
        }
        Some(']') => {
            while let Some(ch) = chars.next() {
                if ch == '\u{7}' {
                    break;
                }
                if ch == '\u{1b}' && chars.peek() == Some(&'\\') {
                    chars.next();
                    break;
                }
            }
        }
        _ => {}
    }
}

/// 본문 안 `(행, 열)` 을 caret 두 칸을 더한 화면 좌표로.
fn position(row: usize, column: usize) -> (u16, u16) {
    let col = u16::try_from(column + 2).unwrap_or(u16::MAX);
    let row = u16::try_from(row).unwrap_or(u16::MAX);
    (col, row)
}

#[cfg(test)]
mod tests {
    /// codex `test_current_at_token_*` — the whitespace word under the cursor,
    /// when it starts with `@`; the range covers the sigil, the token does not.
    #[test]
    fn the_at_token_is_the_word_under_the_cursor_that_starts_with_the_sigil() {
        let cases: &[(&str, usize, Option<&str>)] = &[
            ("@hello", 3, Some("hello")),
            ("@file.txt", 4, Some("file.txt")),
            ("hello @world test", 8, Some("world")),
            ("@İstanbul", 3, Some("İstanbul")),
            ("@诶", 2, Some("诶")),
            ("hello", 2, None),
            ("@", 1, Some("")),
            ("@ hello", 2, None),
            ("test @ world", 6, None),
            ("@test", 0, Some("test")),
            ("@test", 5, Some("test")),
            ("@file1 @file2", 0, Some("file1")),
            ("@file1 @file2", 8, Some("file2")),
            ("", 0, None),
            ("aaa@aaa", 4, None),
            ("aaa @aaa", 5, Some("aaa")),
            ("test　@İstanbul", 8, Some("İstanbul")),
            ("@ЙЦУ　@诶", 10, Some("诶")),
            ("test\t@file", 6, Some("file")),
            ("npx -y @kaeawc/auto-mobile@latest", 12, Some("kaeawc/auto-mobile@latest")),
            ("@icons/icon@2x.png", 8, Some("icons/icon@2x.png")),
            ("foo@bar", 3, None),
            // Token affinity: one separator past the token still means it.
            ("@wiki/alpha.md ", 15, Some("wiki/alpha.md")),
            ("@ma  @scope", 4, Some("ma")),
            ("@ma  @scope", 5, Some("scope")),
            ("look at src/x.rs  please", 17, None),
            ("@\n", 2, None),
        ];
        for (input, cursor, expected) in cases {
            let mut composer = Composer::new();
            composer.insert_str(input);
            composer.set_cursor(*cursor);
            let token = composer.at_token().map(|(_, token)| token);
            assert_eq!(token.as_deref(), *expected, "input {input:?} cursor {cursor}");
        }
        let mut composer = Composer::new();
        composer.insert_str("hello @world test");
        composer.set_cursor(8);
        let (range, _) = composer.at_token().expect("token");
        assert_eq!(&composer.text()[range], "@world");
    }

    #[test]
    fn replace_range_moves_the_cursor_with_the_text_and_never_splits_a_character() {
        let mut composer = Composer::new();
        composer.insert_str("a @co b");
        composer.set_cursor("a @co b".len());
        composer.replace_range(2..5, "src/composer.rs");
        assert_eq!(composer.text(), "a src/composer.rs b");
        assert_eq!(composer.cursor(), composer.text().len());
        composer.set_cursor(0);
        composer.replace_range(2..17, "x");
        assert_eq!(composer.text(), "a x b");
        assert_eq!(composer.cursor(), 0, "a cursor before the range stays");
        let mut composer = Composer::new();
        composer.insert_str("한글");
        composer.set_cursor(1);
        assert_eq!(composer.cursor(), 0, "pulled back to the boundary");
        composer.replace_range(1..4, "x");
        assert_eq!(composer.text(), "x글", "the range is pulled to boundaries too");
    }

    use std::path::{Path, PathBuf};
    use std::time::Instant;

    use super::{Composer, EffortTier, PASTE_TAB_SPACES};
    use crate::tui::ansi::{write_spans_for_palette, Color, Line};
    use crate::tui::palette::{ColorLevel, TerminalPalette};

    fn write_png(path: &Path) {
        image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]))
            .save_with_format(path, image::ImageFormat::Png)
            .expect("write test PNG");
    }

    fn insert_pasted_text_or_image(composer: &mut Composer, pasted: &str) {
        if !composer.handle_paste_image_path(pasted) {
            composer.insert_pasted(pasted);
        }
    }

    #[test]
    fn typing_and_backspace_stay_on_character_boundaries() {
        let mut composer = Composer::new();
        for ch in "한글".chars() {
            composer.insert_char(ch);
        }
        assert_eq!(composer.text(), "한글");
        assert_eq!(composer.cursor(), 6);
        composer.backspace();
        assert_eq!(composer.text(), "한");
        assert_eq!(composer.cursor(), 3);
    }

    #[test]
    fn pasted_text_arrives_as_plain_spaces_and_newlines() {
        // A table row copied out of a mail: a tab, a CRLF, a lone CR and an
        // SGR escape. Written raw, the terminal jumps to its own tab stop and
        // back to column 0 while the composer counts those bytes as nothing —
        // the caret and the glyphs part ways (live report 2026-09-09, 「복붙하고
        // 인풋창에 포커스 위치가 이상해」).
        let mut composer = Composer::new();
        composer.insert_pasted("이 문제\t| 승인\r\n다음\r줄\u{1b}[31m끝\u{1b}]0;title\u{7}\u{7f}");
        let tab = " ".repeat(PASTE_TAB_SPACES);
        assert_eq!(composer.text(), format!("이 문제{tab}| 승인\n다음\n줄끝"));
        assert_eq!(composer.cursor(), composer.text().len());
        assert!(!composer.text().chars().any(|ch| ch.is_control() && ch != '\n'));
    }

    #[test]
    fn arrows_move_by_character_not_byte() {
        let mut composer = Composer::new();
        composer.insert_str("한a글");
        composer.home();
        composer.right();
        assert_eq!(composer.cursor(), 3);
        composer.right();
        assert_eq!(composer.cursor(), 4);
        composer.left();
        assert_eq!(composer.cursor(), 3);
    }

    #[test]
    fn word_motions_and_kills() {
        let mut composer = Composer::new();
        composer.insert_str("alpha beta gamma");
        composer.kill_word_left();
        assert_eq!(composer.text(), "alpha beta ");
        composer.kill_to_start();
        assert_eq!(composer.text(), "");
    }

    #[test]
    fn the_placeholder_shows_only_when_empty() {
        let composer = Composer::new();
        let (lines, cursor) = composer.render(40, "Ask zo to do anything");
        assert_eq!(lines[0].plain(), format!("╭{}╮", "─".repeat(38)));
        assert_eq!(
            lines[1].plain(),
            format!("│› Ask zo to do anything{}│", " ".repeat(15))
        );
        let caret = &lines[1].spans[1];
        assert!(caret.style.bold);
        assert_eq!(caret.style.fg, None, "an untiered caret inherits the terminal foreground");
        assert_eq!(lines[2].plain(), format!("╰{}╯", "─".repeat(38)));
        assert_eq!(cursor, (3, 1));
    }

    #[test]
    fn a_measured_dark_background_tints_the_whole_composer_but_not_its_border_colour() {
        let composer = Composer::new();
        let palette = TerminalPalette::new(
            (240, 240, 240),
            (20, 20, 20),
            ColorLevel::TrueColor,
        );
        let (lines, _) = composer.render_with_effort_for_palette(
            40,
            "Ask zo to do anything",
            None,
            None,
            Instant::now(),
            Some(palette),
        );

        assert!(lines
            .iter()
            .all(|line| line.style.bg == Some(Color::Rgb(48, 48, 48))));
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Rgb(128, 128, 128)));
        let mut rendered = String::new();
        write_spans_for_palette(&lines[0], Some(palette), &mut rendered);
        assert!(
            rendered.starts_with("\u{1b}[38;2;128;128;128;48;2;48;48;48m"),
            "border foreground and composer background must coexist: {rendered:?}"
        );
    }

    #[test]
    fn max_ultra_and_smart_keep_their_tier_specific_prompt_glyphs() {
        let composer = Composer::new();
        for (tier, glyph, colour) in [
            (EffortTier::Max, "›", Color::Rgb(255, 178, 66)),
            (EffortTier::Ultra, "»", Color::Rgb(186, 130, 255)),
            (EffortTier::Smart, "◆", Color::Rgb(80, 210, 190)),
        ] {
            let (lines, _) = composer.render_with_effort_for_palette(
                40,
                "",
                Some(tier),
                None,
                Instant::now(),
                None,
            );
            let caret = &lines[1].spans[1];
            assert_eq!(caret.text, glyph);
            assert_eq!(caret.style.fg, Some(colour));
            assert!(caret.style.bold);
        }
    }

    #[test]
    fn wide_glyphs_advance_the_cursor_by_two_columns() {
        let mut composer = Composer::new();
        composer.insert_str("한글");
        let (lines, cursor) = composer.render(40, "");
        assert_eq!(lines[1].plain(), format!("│› 한글{}│", " ".repeat(32)));
        assert_eq!(cursor, (7, 1));
    }

    #[test]
    fn long_input_wraps_under_the_caret() {
        let mut composer = Composer::new();
        composer.insert_str("abcdefghij");
        let (lines, cursor) = composer.render(8, "");
        let plain: Vec<String> = lines.iter().map(Line::plain).collect();
        assert_eq!(
            plain,
            vec![
                "╭──────╮",
                "│› abcd│",
                "│  efgh│",
                "│  ij  │",
                "╰──────╯",
            ]
        );
        assert_eq!(cursor, (5, 3));
    }

    #[test]
    fn pasted_newlines_become_extra_rows() {
        let mut composer = Composer::new();
        composer.insert_str("one\ntwo");
        let (lines, _) = composer.render(40, "");
        let plain: Vec<String> = lines.iter().map(Line::plain).collect();
        assert_eq!(plain.len(), 4);
        assert_eq!(plain[1], format!("│› one{}│", " ".repeat(33)));
        assert_eq!(plain[2], format!("│  two{}│", " ".repeat(33)));
    }

    #[test]
    fn very_narrow_widths_never_overflow_or_panic() {
        let mut composer = Composer::new();
        composer.insert_str("한글abc");
        for width in 0..=6 {
            let (lines, _) = composer.render(width, "");
            assert!(lines.iter().all(|line| line.width() <= width));
        }
    }

    #[test]
    fn history_walks_back_and_returns_to_the_draft() {
        let mut composer = Composer::new();
        composer.insert_str("first");
        assert_eq!(composer.submit().text, "first");
        composer.insert_str("draft");
        composer.history_prev();
        assert_eq!(composer.text(), "first");
        composer.history_next();
        assert_eq!(composer.text(), "draft");
    }

    #[test]
    fn persistent_history_round_trips_codex_jsonl() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("history.jsonl");
        let mut composer = Composer::with_history_file(
            path.clone(),
            "session-a",
            vec!["resumed question".to_string()],
        );

        composer.insert_str("new question");
        assert_eq!(composer.submit().text, "new question");
        composer.insert_str("new question");
        assert_eq!(composer.submit().text, "new question");

        let raw = std::fs::read_to_string(&path).expect("history file");
        let records = raw
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("history JSONL"))
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2, "Codex records each submitted prompt");
        assert_eq!(records[0]["session_id"], "session-a");
        assert!(records[0]["ts"].as_u64().is_some());
        assert_eq!(records[0]["text"], "new question");
        assert_eq!(records[0].as_object().expect("record object").len(), 3);
        assert_eq!(records[1]["text"], "new question");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                std::fs::metadata(&path)
                    .expect("history metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        let mut restored = Composer::with_history_file(
            path,
            "session-b",
            vec!["resumed question".to_string()],
        );
        restored.history_prev();
        assert_eq!(restored.text(), "resumed question");
        restored.history_prev();
        assert_eq!(restored.text(), "new question");
        restored.history_prev();
        assert_eq!(restored.text(), "new question");
    }

    #[test]
    fn image_placeholders_are_contiguous_and_delete_as_one_element() {
        let mut composer = Composer::new();
        composer.attach_image("/tmp/one.png".into());
        composer.attach_image("/tmp/two.png".into());
        assert_eq!(composer.text(), "[Image #1][Image #2]");

        composer.home();
        composer.delete();
        assert_eq!(composer.text(), "[Image #1]");
        let submission = composer.submit();
        assert_eq!(submission.text, "[Image #1]");
        assert_eq!(submission.image_paths, vec![PathBuf::from("/tmp/two.png")]);
    }

    #[test]
    fn pasted_image_path_attaches_a_codex_style_placeholder() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("image.png");
        write_png(&path);

        let mut composer = Composer::new();
        insert_pasted_text_or_image(&mut composer, path.to_str().expect("UTF-8 path"));

        assert_eq!(composer.text(), "[Image #1]");
        assert_eq!(composer.submit().image_paths, vec![path]);
    }

    #[test]
    fn quoted_pasted_image_path_attaches_a_codex_style_placeholder() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("quoted image.png");
        write_png(&path);
        let pasted = format!("\"{}\"", path.display());

        let mut composer = Composer::new();
        insert_pasted_text_or_image(&mut composer, &pasted);

        assert_eq!(composer.text(), "[Image #1]");
        assert_eq!(composer.submit().image_paths, vec![path]);
    }

    #[test]
    fn file_url_pasted_image_path_attaches_a_codex_style_placeholder() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("url image.png");
        write_png(&path);
        let pasted = url::Url::from_file_path(&path)
            .expect("file URL")
            .to_string();

        let mut composer = Composer::new();
        insert_pasted_text_or_image(&mut composer, &pasted);

        assert_eq!(composer.text(), "[Image #1]");
        assert_eq!(composer.submit().image_paths, vec![path]);
    }

    #[test]
    fn non_image_path_stays_normal_text() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("not-an-image.png");
        std::fs::write(&path, "not image data").expect("write text file");
        let pasted = path.to_str().expect("UTF-8 path");

        let mut composer = Composer::new();
        insert_pasted_text_or_image(&mut composer, pasted);

        assert_eq!(composer.text(), pasted);
        assert!(composer.submit().image_paths.is_empty());
    }

    #[test]
    fn multiple_pasted_paths_stay_normal_text() {
        let pasted = "/tmp/one.png /tmp/two.png";
        let mut composer = Composer::new();

        insert_pasted_text_or_image(&mut composer, pasted);

        assert_eq!(composer.text(), pasted);
        assert!(composer.submit().image_paths.is_empty());
    }

    #[test]
    fn missing_pasted_image_path_stays_normal_text() {
        let pasted = "/tmp/zerocode-no-such-image.png";
        let mut composer = Composer::new();

        insert_pasted_text_or_image(&mut composer, pasted);

        assert_eq!(composer.text(), pasted);
        assert!(composer.submit().image_paths.is_empty());
    }
}

#[cfg(test)]
mod probe_tests {
    use super::{decodes_as_image_within, IMAGE_PROBE_BUDGET};
    use std::time::{Duration, Instant};

    /// A path that never answers must cost the budget and no more. Pasting a
    /// FIFO froze the whole TUI with no way back from the keyboard, and killing
    /// it from outside left the shell in raw mode.
    #[test]
    fn a_path_that_never_answers_gives_up_on_time() {
        let dir = std::env::temp_dir().join(format!("zo-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let fifo = dir.join("blocks.png");

        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !made {
            // No mkfifo here; the regular-file guard below still covers the
            // shape of the bug.
            std::fs::remove_dir_all(&dir).ok();
            return;
        }

        let started = Instant::now();
        assert!(!decodes_as_image_within(&fifo, IMAGE_PROBE_BUDGET));
        let waited = started.elapsed();
        assert!(
            waited < IMAGE_PROBE_BUDGET + Duration::from_millis(400),
            "the probe waited {waited:?}, past its {IMAGE_PROBE_BUDGET:?} budget"
        );

        std::fs::remove_file(&fifo).ok();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Nothing that is not a regular file is an image, and asking the decoder
    /// to open one is exactly what hangs.
    #[test]
    fn only_regular_files_reach_the_decoder() {
        let dir = std::env::temp_dir().join(format!("zo-probe-dir-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        assert!(!decodes_as_image_within(&dir, IMAGE_PROBE_BUDGET));
        assert!(!decodes_as_image_within(
            &dir.join("absent.png"),
            IMAGE_PROBE_BUDGET
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
