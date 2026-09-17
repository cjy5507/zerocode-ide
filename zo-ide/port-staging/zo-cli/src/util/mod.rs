//! Cross-cutting helpers shared between the binary and library targets.
//!
//! Modules here must stay leaf-level: no dependency on `tui`, `session`,
//! or any other higher-level subsystem. Anything that pulls in
//! ratatui/crossterm/runtime types belongs in those subsystems, not in
//! `util`.

pub mod ansi;

/// Format `value` with a thin space grouping every three digits (`1 234 567`),
/// matching the token-budget readouts in the effort picker and `/effort`.
///
/// Lives in `util` (the lib's leaf home reachable from both the library `tui`
/// and the binary `session` targets) so the two surfaces share one formatter.
#[must_use]
pub fn format_thousands(value: u32) -> String {
    let raw = value.to_string();
    let mut out = String::with_capacity(raw.len() + raw.len() / 3);
    for (i, ch) in raw.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::format_thousands;

    #[test]
    fn format_thousands_groups_digits_with_thin_space() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(999), "999");
        assert_eq!(format_thousands(1_024), "1 024");
        assert_eq!(format_thousands(32_000), "32 000");
        assert_eq!(format_thousands(1_234_567), "1 234 567");
    }
}
