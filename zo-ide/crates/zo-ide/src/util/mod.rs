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
    group_digits(u64::from(value), ' ')
}

/// 세 자리마다 `separator` 를 끼운 십진 표기.
///
/// 구분자만 다른 같은 알고리즘이 세 벌 있었다(파이프 렌더러의 `tokens used`,
/// TUI 세션 요약, 그리고 여기). 값은 호출부가 고른다 — 어떤 화면이 쉼표를
/// 쓰고 어떤 화면이 가는 공백을 쓰는지는 캡처가 정한 계약이라 그대로 둔다.
#[must_use]
pub fn group_digits(value: u64, separator: char) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(separator);
        }
        out.push(ch);
    }
    out
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
