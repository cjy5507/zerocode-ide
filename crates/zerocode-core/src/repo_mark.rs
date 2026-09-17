//! A repository's colour mark — the smallest thing a person can say about a
//! repository, and the only one they say with a colour.
//!
//! Orca keeps a `repoBadgeColor` per repository and draws it as `RepoBadgeMark`
//! — a 6px square (`block size-1.5 shrink-0`) beside the repository's name,
//! wherever a name appears next to somebody else's: the sidebar header, the
//! workspace card's meta row, the jump palette, the automations page. When a
//! window holds four checkouts of three repositories, the square is what says
//! which is which before the name has been read.
//!
//! Measured from `normalizeRepoBadgeColor`/`REPO_COLORS`
//! (`I18nProvider-4EBrmTGg.js:23340`, `:25441-25457`). Two facts carry the
//! whole feature:
//!
//! 1. **The palette is eight colours** and the first is a neutral grey, which
//!    is also the value Orca falls back to when a repository has chosen
//!    nothing.
//! 2. **The stored value is normalised, not trusted.** `#EF4444`, `ef4444` and
//!    `#abc` are all colours somebody could put in a settings file by hand or
//!    paste out of a design tool; a picker that only ever writes its own eight
//!    strings still has to READ those.
//!
//! **What is not carried:** Orca's `resolveRepoBadgeColor`, which answers the
//! neutral grey for a repository with no colour. That function is why an
//! unmarked repository in Orca still draws a grey square, and the window here
//! draws NO square instead — see the sidebar's own note. A repository nobody
//! has marked has not chosen grey; it has said nothing, and inventing a mark
//! for it spends the one pixel of meaning this feature has on noise.

/// The eight colours the picker offers, in the order it offers them.
///
/// Hex values, which are data — a colour is a number and there is no other way
/// to spell this one. The order is Orca's, because it is the order of a colour
/// wheel and a person who reaches for "the third one" is reaching by position.
///
/// The first is the neutral one: grey says "marked, deliberately unremarkable",
/// which a person sorting eleven repositories into three colours needs a name
/// for.
pub const REPO_MARK_PALETTE: [&str; 8] = [
    "#737373", // neutral grey, and the one Orca falls back to
    "#ef4444", // red
    "#f97316", // orange
    "#eab308", // yellow
    "#22c55e", // green
    "#14b8a6", // teal
    "#8b5cf6", // purple
    "#ec4899", // pink
];

/// One stored value read as a colour, or `None` for a repository with no mark.
///
/// Orca's `normalizeRepoBadgeColor`, rule for rule: trim, accept an optional
/// `#`, accept three or six hex digits and nothing else, lower the case, and
/// **expand three digits to six** (`abc` → `#aabbcc`) so one colour has one
/// spelling. The last step is the one that matters twice over: the picker
/// compares the stored value against the palette to show which swatch is
/// chosen, and `#777` and `#777777` are the same colour that no string
/// comparison would ever call equal.
///
/// A value outside the palette is kept rather than refused — Orca's own
/// `REPO_COLORS.find(…) ?? normalized` ends the same way, and it is what lets
/// a settings file carry a colour somebody chose from a full picker. So this
/// answers `Some` for any readable colour and `None` only for a value that is
/// not one, which is exactly the shape a caller needs: `None` means "draw
/// nothing", never "draw the default".
pub fn normalize_repo_mark(value: Option<&str>) -> Option<String> {
    let said = value?.trim();
    // The `#` is optional on the way in and mandatory on the way out: what is
    // stored is spent as a CSS colour, where a bare `ef4444` is not one.
    let digits = said.strip_prefix('#').unwrap_or(said);
    if !matches!(digits.len(), 3 | 6) {
        return None;
    }
    if !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let short = digits.len() == 3;
    let mut spelt = String::with_capacity(7);
    spelt.push('#');
    for byte in digits.bytes() {
        let digit = char::from(byte.to_ascii_lowercase());
        spelt.push(digit);
        // `abc` is CSS shorthand for `aabbcc` — each digit doubled, not the
        // string repeated (`abcabc` is a different colour).
        if short {
            spelt.push(digit);
        }
    }
    Some(spelt)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three digits are CSS shorthand for six, doubled digit by digit.
    ///
    /// The failure this pins is silent and total: `#abc` handed to CSS renders
    /// the right colour, so a window that stores it unexpanded looks correct
    /// and only the picker breaks — no swatch reads as chosen, because the
    /// palette is spelled in six digits and nothing compares equal.
    #[test]
    fn a_three_digit_colour_is_expanded_to_six() {
        assert_eq!(
            normalize_repo_mark(Some("#abc")).as_deref(),
            Some("#aabbcc")
        );
        assert_eq!(normalize_repo_mark(Some("abc")).as_deref(), Some("#aabbcc"));
        assert_eq!(
            normalize_repo_mark(Some("#777")).as_deref(),
            Some("#777777")
        );
        // Doubled per digit, not repeated as a string.
        assert_eq!(
            normalize_repo_mark(Some("#123")).as_deref(),
            Some("#112233")
        );
    }

    /// One colour, one spelling: the case is lowered and the `#` is put on.
    #[test]
    fn a_colour_is_spelt_one_way_however_it_arrived() {
        assert_eq!(
            normalize_repo_mark(Some("#EF4444")).as_deref(),
            Some("#ef4444")
        );
        assert_eq!(
            normalize_repo_mark(Some("EF4444")).as_deref(),
            Some("#ef4444")
        );
        assert_eq!(
            normalize_repo_mark(Some("#Ef44Aa")).as_deref(),
            Some("#ef44aa")
        );
        assert_eq!(
            normalize_repo_mark(Some("  #ec4899  ")).as_deref(),
            Some("#ec4899")
        );
        assert_eq!(
            normalize_repo_mark(Some("#ABC")).as_deref(),
            Some("#aabbcc")
        );
    }

    /// Anything that is not a colour is no mark at all — never a default one.
    ///
    /// `None` is the answer a caller draws nothing for. If a bad value fell
    /// through to the neutral grey instead, a typo in a settings file would
    /// mark every repository that has it, and the person who typed it would
    /// see a mark appear rather than the one they meant.
    #[test]
    fn a_value_that_is_not_a_colour_is_no_mark() {
        for said in [
            "xyz",        // hex digits it is not
            "",           // an emptied field
            "   ",        // and one holding only space
            "#",          // the prefix alone
            "#12",        // too few
            "#1234",      // between the two lengths
            "#12345",     // one short of six
            "#1234567",   // one long
            "##abc",      // the prefix twice
            "red",        // a name, which CSS takes and this does not
            "rgb(0,0,0)", // a function
            "#12 34 56",  // spaces inside
            "0x123456",   // another language's spelling
        ] {
            assert_eq!(
                normalize_repo_mark(Some(said)),
                None,
                "`{said}` is not a colour and must not become a mark"
            );
        }
        assert_eq!(normalize_repo_mark(None), None);
    }

    /// Every palette colour survives the round trip unchanged, in any case.
    ///
    /// This is what makes the picker's "which swatch is chosen" question
    /// answerable by string equality: the value the picker writes is the value
    /// the normaliser gives back.
    #[test]
    fn every_palette_colour_is_already_normal() {
        for colour in REPO_MARK_PALETTE {
            assert_eq!(
                normalize_repo_mark(Some(colour)).as_deref(),
                Some(colour),
                "the palette carries a colour its own normaliser rewrites"
            );
            assert_eq!(
                normalize_repo_mark(Some(&colour.to_uppercase())).as_deref(),
                Some(colour),
                "the same colour shouted must come back in the palette's spelling"
            );
        }
        // The palette itself: eight distinct colours, each spelled the one way
        // the round trip above depends on.
        assert_eq!(REPO_MARK_PALETTE.len(), 8);
        for colour in REPO_MARK_PALETTE {
            assert_eq!(colour.len(), 7, "`{colour}` is not `#rrggbb`");
            assert!(colour.starts_with('#'), "`{colour}` has no `#`");
            assert_eq!(
                REPO_MARK_PALETTE
                    .iter()
                    .filter(|one| **one == colour)
                    .count(),
                1,
                "`{colour}` is in the palette twice, so the picker offers the same swatch twice"
            );
        }
    }

    /// A colour outside the palette is kept, not refused.
    ///
    /// Orca ends its normaliser with `REPO_COLORS.find(…) ?? normalized`, so a
    /// repository whose colour came from the full picker keeps it. Refusing
    /// here would quietly erase such a mark the first time this window read
    /// the file.
    #[test]
    fn a_colour_the_palette_does_not_offer_is_still_a_colour() {
        assert_eq!(
            normalize_repo_mark(Some("#123456")).as_deref(),
            Some("#123456")
        );
        assert_eq!(
            normalize_repo_mark(Some("#FFF")).as_deref(),
            Some("#ffffff")
        );
        assert!(!REPO_MARK_PALETTE.contains(&"#123456"));
    }
}
