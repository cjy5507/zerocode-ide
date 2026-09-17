//! The objective checks a `/goal --check` (or `/loop --check`) names by word
//! — one table, read by the parser, which refuses a typo in a known prefix at
//! parse time, and by the controller, which turns a word into the command
//! its gate runs. A value outside the table is a command, run as written.

/// A `--check` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalCheck {
    CargoFmt,
    CargoCheck,
    CargoTest,
    CargoClippy,
    GitDiff,
    GitDiffCheck,
    /// A pattern the workspace must hold.
    Grep(String),
    /// A window whose title holds this fragment is on the screen.
    ScreenWindow(String),
    /// This text is on the screen, read off its pixels.
    ScreenText(String),
    /// A command, run as written.
    Command(String),
}

impl GoalCheck {
    /// Read a `--check` value; a known prefix with an unknown or empty rest
    /// is refused with the usage that prefix takes, never demoted to a
    /// command that could only fail (or a rubric no command checks).
    pub fn parse(check: &str) -> Result<Self, &'static str> {
        let check = check.trim();
        let rest = |prefix: &str| check.strip_prefix(prefix).map(str::trim);
        let named = |value: &str, usage: &'static str| {
            if value.is_empty() {
                Err(usage)
            } else {
                Ok(value.to_string())
            }
        };
        if let Some(kind) = rest("cargo:") {
            return match kind {
                "fmt" => Ok(Self::CargoFmt),
                "check" => Ok(Self::CargoCheck),
                "test" => Ok(Self::CargoTest),
                "clippy" => Ok(Self::CargoClippy),
                _ => Err("--check cargo:<fmt|check|test|clippy>"),
            };
        }
        if let Some(kind) = rest("git:") {
            return match kind {
                "diff" => Ok(Self::GitDiff),
                "diff-check" => Ok(Self::GitDiffCheck),
                _ => Err("--check git:<diff|diff-check>"),
            };
        }
        if let Some(pattern) = check.strip_prefix("grep:") {
            return named(pattern.trim(), "--check grep:<pattern>").map(|_| Self::Grep(pattern.to_string()));
        }
        if let Some(what) = rest("screen:") {
            if let Some(title) = what.strip_prefix("window:") {
                return named(title.trim(), SCREEN_USAGE).map(Self::ScreenWindow);
            }
            if let Some(text) = what.strip_prefix("text:") {
                return named(text.trim(), SCREEN_USAGE).map(Self::ScreenText);
            }
            return Err(SCREEN_USAGE);
        }
        if check.is_empty() {
            return Err("--check <validator>");
        }
        Ok(Self::Command(check.to_string()))
    }
}

const SCREEN_USAGE: &str = "--check screen:<window|text>:<title or text>";

#[cfg(test)]
mod tests {
    use super::GoalCheck;

    #[test]
    fn a_check_word_is_read_once_and_a_typo_in_a_known_prefix_is_refused() {
        assert_eq!(GoalCheck::parse("cargo:test"), Ok(GoalCheck::CargoTest));
        assert_eq!(GoalCheck::parse(" git:diff-check "), Ok(GoalCheck::GitDiffCheck));
        assert_eq!(GoalCheck::parse("grep:TODO"), Ok(GoalCheck::Grep("TODO".into())));
        assert_eq!(
            GoalCheck::parse("screen:window:Preferences"),
            Ok(GoalCheck::ScreenWindow("Preferences".into()))
        );
        assert_eq!(
            GoalCheck::parse("screen:text: Order placed "),
            Ok(GoalCheck::ScreenText("Order placed".into()))
        );
        assert_eq!(GoalCheck::parse("just verify"), Ok(GoalCheck::Command("just verify".into())));
        for typo in ["cargo:tests", "git:status", "grep: ", "screen:window:", "screen:button:OK", "screen:", " "] {
            assert!(GoalCheck::parse(typo).is_err(), "{typo:?}");
        }
    }
}
