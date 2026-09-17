#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CommandCategory {
    Session,
    Workspace,
    Discovery,
    Analysis,
    Appearance,
    Control,
}

impl CommandCategory {
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Session => "Session & visibility",
            Self::Workspace => "Workspace & git",
            Self::Discovery => "Discovery & debugging",
            Self::Analysis => "Analysis & automation",
            Self::Appearance => "Appearance & input",
            Self::Control => "Communication & control",
        }
    }

    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Session => "\u{25c9}",
            Self::Workspace => "\u{25c6}",
            Self::Discovery => "\u{25c8}",
            Self::Analysis => "\u{25ca}",
            Self::Appearance => "\u{25cb}",
            Self::Control => "\u{25cf}",
        }
    }

    /// ASCII sibling of [`Self::glyph`] for `NO_COLOR`/dumb terminals. Every
    /// category collapses to `*` except Appearance (`o`), mirroring the
    /// prototype's mono glyph set (`zo-theme.js` `GLYPH_NC`). Every sibling
    /// is exactly one display cell so the palette header stays aligned.
    #[must_use]
    pub fn glyph_ascii(self) -> &'static str {
        match self {
            Self::Appearance => "o",
            _ => "*",
        }
    }

    #[must_use]
    pub fn from_prefix(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "session" | "sess" => Some(Self::Session),
            "workspace" | "git" | "ws" => Some(Self::Workspace),
            "discovery" | "debug" | "disc" => Some(Self::Discovery),
            "analysis" | "auto" => Some(Self::Analysis),
            "appearance" | "ui" | "style" => Some(Self::Appearance),
            "control" | "ctrl" | "comm" => Some(Self::Control),
            _ => None,
        }
    }

    #[must_use]
    pub const fn all() -> &'static [CommandCategory] {
        &[
            Self::Session,
            Self::Workspace,
            Self::Discovery,
            Self::Analysis,
            Self::Appearance,
            Self::Control,
        ]
    }
}

impl std::fmt::Display for CommandCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

#[cfg(test)]
mod tests {
    use super::CommandCategory;

    #[test]
    fn category_prefix_matching_is_case_insensitive_and_trimmed() {
        assert_eq!(
            CommandCategory::from_prefix(" Session "),
            Some(CommandCategory::Session)
        );
        assert_eq!(
            CommandCategory::from_prefix("WS"),
            Some(CommandCategory::Workspace)
        );
        assert_eq!(
            CommandCategory::from_prefix("Debug"),
            Some(CommandCategory::Discovery)
        );
    }

    #[test]
    fn every_category_has_a_single_cell_ascii_glyph_sibling() {
        for &cat in CommandCategory::all() {
            let ascii = cat.glyph_ascii();
            assert_eq!(
                ascii.chars().count(),
                1,
                "{cat:?} ascii glyph must be 1 cell"
            );
            assert!(ascii.is_ascii(), "{cat:?} sibling must be ASCII: {ascii:?}");
        }
        // Appearance keeps a distinct `o`; the rest collapse to `*` (prototype parity).
        assert_eq!(CommandCategory::Appearance.glyph_ascii(), "o");
        assert_eq!(CommandCategory::Session.glyph_ascii(), "*");
        assert_eq!(CommandCategory::Control.glyph_ascii(), "*");
    }
}
