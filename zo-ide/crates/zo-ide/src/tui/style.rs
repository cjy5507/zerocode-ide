//! How the screen looks — decided once, before the first frame, and read by
//! every painter after that without a lock or a disk read.
//!
//! Two looks share one renderer:
//!
//! - [`TuiStyle::Marks`], zo's own since 1.1.133: `⏺ Read(src/lib.rs)` with
//!   its result hung under `⎿`, `> ` before the person's words, `✻` for
//!   thinking, `✓ Done in 5s · …` where a turn ends and `⏵⏵` for the mode —
//!   the grammar most people now read an agent's work in.
//! - [`TuiStyle::Codex`], the look zo was first drawn from, byte for byte
//!   codex's. The golden tests pin it, so it is what a process that never
//!   chose draws: a test, the pipe front.
//!
//! The person picks with `tuiStyle` in zo's settings (`"marks"` or
//! `"codex"`); `ZO_TUI_STYLE` overrides it for one run. Three switches ride
//! beside it, all off unless set: code blocks drawn flush left so a mouse
//! copy carries no indent (`codeBlocksFlush`), the after-answer hint folded
//! into the turn's divider (`turnHintsInDivider`), and no motion in the
//! working line (`reduceMotion`).

use std::sync::OnceLock;

use crate::preferences::Preferences;

/// Which grammar the transcript is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TuiStyle {
    /// codex's bytes — what an undecided process draws.
    #[default]
    Codex,
    /// `⏺ Tool(target)` / `⎿ result` — the interactive front's default.
    Marks,
}

impl TuiStyle {
    /// The setting's word, as `tuiStyle` and `ZO_TUI_STYLE` spell it.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "marks" => Some(Self::Marks),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    /// The glyphs this look draws its rows with.
    #[must_use]
    pub const fn glyphs(self) -> Glyphs {
        match self {
            Self::Codex => Glyphs {
                answer: "• ",
                user: "› ",
                thinking: "• ",
                tool_done: super::strings::TOOL_DONE,
                tool_failed: super::strings::TOOL_FAILED,
                result_first: "  └ ",
                result_rest: "    ",
                permission_mark: super::strings::PERMISSION_MARK,
                working: "•",
                dot: "•",
            },
            // The composer keeps zo's `›` in this look too: the window reads
            // `│› ` to know the field is empty before it types at a running
            // turn (`zerocode-shell` `composer_empty::observed`).
            Self::Marks => Glyphs {
                answer: "⏺ ",
                user: "> ",
                thinking: "✻ ",
                tool_done: "⏺",
                tool_failed: "⏺",
                result_first: "  ⎿ ",
                result_rest: "    ",
                permission_mark: "⏵⏵",
                working: "✻",
                dot: "·",
            },
        }
    }
}

/// The marks a look puts at the head of its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyphs {
    /// Before an answer's first row.
    pub answer: &'static str,
    /// Before the person's words.
    pub user: &'static str,
    /// Before the thinking header.
    pub thinking: &'static str,
    /// A finished tool's row.
    pub tool_done: &'static str,
    /// A failed tool's row (the colour says which).
    pub tool_failed: &'static str,
    /// The first row hung under a tool's row.
    pub result_first: &'static str,
    /// Every later hung row — as wide as [`Self::result_first`].
    pub result_rest: &'static str,
    /// Before the footer's permission word (`Full Access`).
    pub permission_mark: &'static str,
    /// The working row's mark, which the shimmer runs over.
    pub working: &'static str,
    /// Between the working row's seconds and its interrupt key.
    pub dot: &'static str,
}

/// How a finished turn stands in the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConversationView {
    /// A row a step, as every step happened — codex's transcript, and what
    /// `ctrl+o` opens.
    #[default]
    Full,
    /// The turn's work folded into one cell above its answer.
    Turns,
}

impl ConversationView {
    /// The setting's word, as `conversationView` spells it.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word.trim().to_ascii_lowercase().as_str() {
            "turns" => Some(Self::Turns),
            "full" => Some(Self::Full),
            _ => None,
        }
    }

    /// The other view — what `ctrl+o` turns to.
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::Full => Self::Turns,
            Self::Turns => Self::Full,
        }
    }
}

/// The widest prose and tool rows are drawn when `maxProseWidth` says nothing.
pub const DEFAULT_PROSE_WIDTH: usize = 100;

/// The look and its switches, fixed for the process.
///
/// Each switch is a setting of its own that a person turns alone, so they
/// stay booleans: an enum of them would have to name every combination.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Look {
    pub style: TuiStyle,
    /// Code blocks start at column 0, so a mouse copy carries no indent.
    pub flush_code: bool,
    /// The after-answer hint (`/find · /diff · /rewind`) rides in the
    /// turn's divider instead of a row of its own.
    pub hints_in_divider: bool,
    /// The working line holds still: no shimmer, no spinner frames.
    pub reduce_motion: bool,
    /// An empty conversation is a start page — the field in the middle, the
    /// conversations to continue under it (`tui::start`). The marks look's
    /// default; codex's opens on its card alone.
    pub start_page: bool,
    /// How a finished turn stands when the session opens; `ctrl+o` turns it.
    pub view: ConversationView,
    /// The widest prose and tool rows are drawn, or `None` for the pane's
    /// width. Tables, code and command output keep the pane's width.
    pub prose_width: Option<usize>,
    /// A finished thinking block stands as one row — how long it took and
    /// what it was about — until `/thinking` asks for it whole. The marks
    /// look's, when `showThinking` says nothing.
    pub thinking_line: bool,
}

impl Look {
    /// `width` cut to the reading width: what prose and tool rows wrap to.
    #[must_use]
    pub fn reading_width(self, width: usize) -> usize {
        self.prose_width.map_or(width, |cap| width.min(cap))
    }
}

/// `ZO_TUI_STYLE`: one run's look, over the setting.
pub const STYLE_ENV: &str = "ZO_TUI_STYLE";

static LOOK: OnceLock<Look> = OnceLock::new();

impl Look {
    /// The interactive front's look: the environment's word, else the
    /// setting's, else [`TuiStyle::Marks`].
    #[must_use]
    pub fn chosen(preferences: &Preferences, env_style: Option<&str>) -> Self {
        let style = env_style
            .and_then(TuiStyle::parse)
            .or_else(|| preferences.tui_style.as_deref().and_then(TuiStyle::parse))
            .unwrap_or(TuiStyle::Marks);
        // The new layouts are the marks look's. Codex's bytes are pinned by
        // the goldens, so that look takes none of them whatever the settings
        // say.
        let marks = style == TuiStyle::Marks;
        Self {
            style,
            flush_code: preferences.code_blocks_flush.unwrap_or(false),
            hints_in_divider: preferences.turn_hints_in_divider.unwrap_or(false),
            reduce_motion: preferences.reduce_motion.unwrap_or(false),
            start_page: marks && preferences.start_screen.as_deref().map(str::trim) != Some("line"),
            view: if marks {
                preferences
                    .conversation_view
                    .as_deref()
                    .and_then(ConversationView::parse)
                    .unwrap_or(ConversationView::Turns)
            } else {
                ConversationView::Full
            },
            prose_width: marks
                .then(|| preferences.max_prose_width.unwrap_or(DEFAULT_PROSE_WIDTH))
                .filter(|columns| *columns > 0),
            thinking_line: marks && preferences.show_thinking.is_none(),
        }
    }
}

/// Fix this process's look. The interactive front calls it once, before its
/// first frame; a second call changes nothing.
pub fn init(preferences: &Preferences) {
    let env_style = std::env::var(STYLE_ENV).ok();
    let _ = LOOK.set(Look::chosen(preferences, env_style.as_deref()));
}

/// This process's look — codex's until [`init`] ran.
#[must_use]
pub fn current() -> Look {
    LOOK.get().copied().unwrap_or_default()
}

/// This process's glyphs.
#[must_use]
pub fn glyphs() -> Glyphs {
    current().style.glyphs()
}

/// Names, to a fresh run of this test binary, that it is the run a test
/// asked for — where the look can be fixed once without reaching any other
/// test.
#[cfg(test)]
const FRESH_RUN_ENV: &str = "ZO_TUI_LOOK_FRESH_RUN";

/// Whether this is the fresh run `test` (its path inside the crate) asked
/// for. In the asking run it starts that run, asserts it passed, and answers
/// `false`, so the test body runs only where a look may be fixed.
#[cfg(test)]
pub(crate) fn in_fresh_run(test: &str) -> bool {
    if std::env::var_os(FRESH_RUN_ENV).is_some() {
        return true;
    }
    let output = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .arg(test)
        .args(["--exact", "--nocapture", "--test-threads=1"])
        .env(FRESH_RUN_ENV, "1")
        .env_remove(STYLE_ENV)
        .output()
        .expect("run the test alone");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(" 1 passed;"),
        "{test}, run alone:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    false
}

#[cfg(test)]
mod tests {
    use super::{Look, TuiStyle};
    use crate::preferences::Preferences;

    #[test]
    fn an_undecided_process_draws_codexs_look() {
        assert_eq!(Look::default().style, TuiStyle::Codex);
    }

    #[test]
    fn the_interactive_front_defaults_to_marks() {
        assert_eq!(Look::chosen(&Preferences::default(), None).style, TuiStyle::Marks);
    }

    #[test]
    fn the_setting_picks_codex_and_the_environment_overrides_it() {
        let codex = Preferences { tui_style: Some("codex".into()), ..Preferences::default() };
        assert_eq!(Look::chosen(&codex, None).style, TuiStyle::Codex);
        assert_eq!(Look::chosen(&codex, Some("MARKS")).style, TuiStyle::Marks);
        assert_eq!(Look::chosen(&codex, Some("neither")).style, TuiStyle::Codex);
    }

    #[test]
    fn every_hung_row_lines_up_under_the_first() {
        for style in [TuiStyle::Codex, TuiStyle::Marks] {
            let glyphs = style.glyphs();
            assert_eq!(glyphs.result_first.chars().count(), glyphs.result_rest.chars().count(), "{style:?}");
        }
    }

    #[test]
    fn the_new_layouts_are_the_marks_looks_and_a_setting_away() {
        use super::ConversationView;
        let marks = Look::chosen(&Preferences::default(), None);
        assert!(marks.start_page);
        assert_eq!(marks.view, ConversationView::Turns);
        assert_eq!(marks.prose_width, Some(super::DEFAULT_PROSE_WIDTH));
        assert_eq!(marks.reading_width(201), 100);
        assert_eq!(marks.reading_width(80), 80);
        assert!(marks.thinking_line);
        let asked = Preferences { show_thinking: Some(true), ..Preferences::default() };
        assert!(!Look::chosen(&asked, None).thinking_line, "an explicit showThinking keeps the whole block");

        let set = Preferences {
            start_screen: Some("line".into()),
            conversation_view: Some("full".into()),
            max_prose_width: Some(0),
            ..Preferences::default()
        };
        let look = Look::chosen(&set, None);
        assert!(!look.start_page);
        assert_eq!(look.view, ConversationView::Full);
        assert_eq!(look.prose_width, None);
        assert_eq!(look.reading_width(201), 201);

        // Codex's look keeps codex's layout, whatever the settings say.
        let codex = Look::chosen(&Preferences::default(), Some("codex"));
        assert!(!codex.start_page);
        assert_eq!(codex.view, ConversationView::Full);
        assert_eq!(codex.prose_width, None);
        assert!(!codex.thinking_line);
        assert_eq!(Look::default(), Look { style: TuiStyle::Codex, ..Look::default() });
    }

    #[test]
    fn the_switches_are_off_unless_set() {
        let look = Look::chosen(&Preferences::default(), None);
        assert!(!look.flush_code && !look.hints_in_divider && !look.reduce_motion);
        let set = Preferences {
            code_blocks_flush: Some(true),
            turn_hints_in_divider: Some(true),
            reduce_motion: Some(true),
            ..Preferences::default()
        };
        let look = Look::chosen(&set, None);
        assert!(look.flush_code && look.hints_in_divider && look.reduce_motion);
    }
}
