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
}

/// The look and its three switches, fixed for the process.
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
        Self {
            style,
            flush_code: preferences.code_blocks_flush.unwrap_or(false),
            hints_in_divider: preferences.turn_hints_in_divider.unwrap_or(false),
            reduce_motion: preferences.reduce_motion.unwrap_or(false),
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
