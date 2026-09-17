//! 경로 문자열을 **가운데서** 자른다 — codex
//! `tui/src/text_formatting.rs::center_truncate_path` 그대로다.
//!
//! 부팅 카드의 `directory:` 행이 유일한 손님이다. codex
//! `history_cell/session.rs::format_directory_inner` 는 폭이 넘칠 때만 이것을
//! 부르고, 그 폭은 `inner_width - "directory: ".len()` 이다.
//!
//! 규칙(원본 주석): "keeping leading and trailing segments where possible and
//! inserting a single Unicode ellipsis between them. If an individual segment
//! cannot fit, it is front-truncated with an ellipsis."
//!
//! 원본은 조각을 **grapheme** 단위로 뒤에서부터 담는다
//! (`unicode-segmentation`). 우리 트리는 어디서나 `char` + `unicode-width` 로
//! 폭을 세므로([`super::ansi::char_width`], [`super::wrap`]) 여기서도 `char`
//! 단위다 — 결합문자 없는 경로에서 둘은 같은 답을 낸다.

use super::ansi::{char_width, str_width};

fn display_width(text: &str) -> usize {
    str_width(text)
}

/// 한 조각을 **앞에서** 잘라 `…` 를 머리에 붙인다 — 원본 `front_truncate`.
fn front_truncate(original: &str, allowed_width: usize) -> String {
    if allowed_width == 0 {
        return String::new();
    }
    if display_width(original) <= allowed_width {
        return original.to_string();
    }
    if allowed_width == 1 {
        return "…".to_string();
    }

    let mut kept = Vec::new();
    let mut used_width = 1; // 머리의 `…` 자리를 미리 뗀다.
    for ch in original.chars().rev() {
        let ch_width = char_width(ch);
        if used_width + ch_width > allowed_width {
            break;
        }
        used_width += ch_width;
        kept.push(ch);
    }
    kept.reverse();
    let mut truncated = String::from("…");
    truncated.extend(kept);
    truncated
}

/// 한 조각. `original` 은 되자를 때 쓰는 원문이고 `text` 는 지금 값이다.
struct Segment<'a> {
    original: &'a str,
    text: String,
    truncatable: bool,
    is_suffix: bool,
}

/// 조각들을 구분자로 다시 잇는다 — 원본 `assemble`.
fn assemble(sep: char, leading: bool, segments: &[Segment<'_>]) -> String {
    let mut result = String::new();
    if leading {
        result.push(sep);
    }
    for segment in segments {
        if !result.is_empty() && !result.ends_with(sep) {
            result.push(sep);
        }
        result.push_str(segment.text.as_str());
    }
    result
}

/// 조각 묶음이 `max_width` 에 들어갈 때까지 뒤쪽부터 앞자르기를 시도한다 —
/// 원본 `fit_segments`. 들어가면 완성된 문자열, 못 들어가면 `None`.
fn fit_segments(
    sep: char,
    has_leading_sep: bool,
    max_width: usize,
    segment_count: usize,
    segments: &mut Vec<Segment<'_>>,
    allow_front_truncate: bool,
) -> Option<String> {
    loop {
        let candidate = assemble(sep, has_leading_sep, segments);
        let width = display_width(candidate.as_str());
        if width <= max_width {
            return Some(candidate);
        }

        if !allow_front_truncate {
            return None;
        }

        // 꼬리 조각을 먼저, 그다음 머리 조각을 — 둘 다 뒤에서 앞으로.
        let mut indices: Vec<usize> = Vec::new();
        for (idx, seg) in segments.iter().enumerate().rev() {
            if seg.truncatable && seg.is_suffix {
                indices.push(idx);
            }
        }
        for (idx, seg) in segments.iter().enumerate().rev() {
            if seg.truncatable && !seg.is_suffix {
                indices.push(idx);
            }
        }

        if indices.is_empty() {
            return None;
        }

        let mut changed = false;
        for idx in indices {
            let original_width = display_width(segments[idx].original);
            if original_width <= max_width && segment_count > 2 {
                continue;
            }
            let seg_width = display_width(segments[idx].text.as_str());
            let other_width = width.saturating_sub(seg_width);
            let allowed_width = max_width.saturating_sub(other_width).max(1);
            let new_text = front_truncate(segments[idx].original, allowed_width);
            if new_text != segments[idx].text {
                segments[idx].text = new_text;
                changed = true;
                break;
            }
        }

        if !changed {
            return None;
        }
    }
}

/// 머리 `left` 개·꼬리 `right` 개 조합을 원본 우선순위로 늘어놓는다.
///
/// 원본은 꼬리를 `min(2, n-1)` 개 지키는 조합을 먼저 보고(`prioritized`),
/// 그다음 나머지(`fallback`)를 본다. 각 무리 안에서는 머리가 긴 것 →
/// 꼬리가 긴 것 → 합이 큰 것 순이다.
fn combos_in_priority_order(segment_count: usize) -> Vec<(usize, usize)> {
    let mut combos: Vec<(usize, usize)> = Vec::new();
    for left in 1..=segment_count {
        let min_right = usize::from(left != segment_count);
        for right in min_right..=(segment_count - left) {
            combos.push((left, right));
        }
    }
    let desired_suffix = if segment_count > 1 {
        std::cmp::min(2, segment_count - 1)
    } else {
        0
    };
    let mut prioritized: Vec<(usize, usize)> = Vec::new();
    let mut fallback: Vec<(usize, usize)> = Vec::new();
    for combo in combos {
        if combo.1 >= desired_suffix {
            prioritized.push(combo);
        } else {
            fallback.push(combo);
        }
    }
    let sort_combos = |items: &mut Vec<(usize, usize)>| {
        items.sort_by(|(left_a, right_a), (left_b, right_b)| {
            left_b
                .cmp(left_a)
                .then_with(|| right_b.cmp(right_a))
                .then_with(|| (left_b + right_b).cmp(&(left_a + right_a)))
        });
    };
    sort_combos(&mut prioritized);
    sort_combos(&mut fallback);
    prioritized.into_iter().chain(fallback).collect()
}

/// 경로를 폭 `max_width` 에 맞춰 가운데서 자른다.
#[must_use]
pub fn center_truncate_path(path: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if display_width(path) <= max_width {
        return path.to_string();
    }

    let sep = std::path::MAIN_SEPARATOR;
    let has_leading_sep = path.starts_with(sep);
    let has_trailing_sep = path.ends_with(sep);
    let mut raw_segments: Vec<&str> = path.split(sep).collect();
    if has_leading_sep && !raw_segments.is_empty() && raw_segments[0].is_empty() {
        raw_segments.remove(0);
    }
    if has_trailing_sep
        && !raw_segments.is_empty()
        && raw_segments.last().is_some_and(|last| last.is_empty())
    {
        raw_segments.pop();
    }

    if raw_segments.is_empty() {
        if has_leading_sep {
            let root = sep.to_string();
            if display_width(root.as_str()) <= max_width {
                return root;
            }
        }
        return "…".to_string();
    }

    let segment_count = raw_segments.len();
    for (left_count, right_count) in combos_in_priority_order(segment_count) {
        let mut segments: Vec<Segment<'_>> = raw_segments[..left_count]
            .iter()
            .map(|seg| Segment {
                original: seg,
                text: (*seg).to_string(),
                truncatable: true,
                is_suffix: false,
            })
            .collect();

        let need_ellipsis = left_count + right_count < segment_count;
        if need_ellipsis {
            segments.push(Segment {
                original: "…",
                text: "…".to_string(),
                truncatable: false,
                is_suffix: false,
            });
        }

        if right_count > 0 {
            segments.extend(raw_segments[segment_count - right_count..].iter().map(|seg| {
                Segment {
                    original: seg,
                    text: (*seg).to_string(),
                    truncatable: true,
                    is_suffix: true,
                }
            }));
        }

        let allow_front_truncate = need_ellipsis || segment_count <= 2;
        if let Some(candidate) = fit_segments(
            sep,
            has_leading_sep,
            max_width,
            segment_count,
            &mut segments,
            allow_front_truncate,
        ) {
            return candidate;
        }
    }

    front_truncate(path, max_width)
}

#[cfg(test)]
mod tests {
    use super::center_truncate_path;

    /// codex `test_center_truncate_doesnt_truncate_short_path`.
    #[test]
    fn a_short_path_is_left_alone() {
        let sep = std::path::MAIN_SEPARATOR;
        let path = format!("{sep}Users{sep}codex{sep}Public");
        assert_eq!(center_truncate_path(&path, 40), path);
    }

    /// codex `test_center_truncate_truncates_long_path`.
    #[test]
    fn a_long_path_loses_its_middle() {
        let sep = std::path::MAIN_SEPARATOR;
        let path = format!("~{sep}hello{sep}the{sep}fox{sep}is{sep}very{sep}fast");
        assert_eq!(
            center_truncate_path(&path, 24),
            format!("~{sep}hello{sep}the{sep}…{sep}very{sep}fast")
        );
    }

    /// codex `test_center_truncate_truncates_long_windows_path`.
    #[test]
    fn a_drive_letter_path_keeps_its_head() {
        let sep = std::path::MAIN_SEPARATOR;
        let path = format!(
            "C:{sep}Users{sep}codex{sep}Projects{sep}super{sep}long{sep}windows{sep}path{sep}file.txt"
        );
        assert_eq!(
            center_truncate_path(&path, 36),
            format!("C:{sep}Users{sep}codex{sep}…{sep}path{sep}file.txt")
        );
    }

    /// codex `test_center_truncate_handles_long_segment`.
    #[test]
    fn one_oversized_segment_is_front_truncated() {
        let sep = std::path::MAIN_SEPARATOR;
        let path = format!("~{sep}supercalifragilisticexpialidocious");
        assert_eq!(
            center_truncate_path(&path, 18),
            format!("~{sep}…cexpialidocious")
        );
    }

    // -- 폭 경계 --------------------------------------------------------

    /// 폭 0 은 빈 문자열이다 — 원본의 첫 분기.
    #[test]
    fn zero_width_is_empty() {
        assert_eq!(center_truncate_path("/a/b/c", 0), "");
    }

    /// 폭이 정확히 경로 폭이면 원문 그대로, 하나 모자라면 자른다.
    #[test]
    fn the_boundary_is_inclusive() {
        let sep = std::path::MAIN_SEPARATOR;
        let path = format!("{sep}Users{sep}codex{sep}Public"); // 19칸
        assert_eq!(center_truncate_path(&path, 19), path);
        let cut = center_truncate_path(&path, 18);
        assert_ne!(cut, path);
        assert!(cut.chars().count() <= 18);
    }

    /// 어떤 폭이든 결과는 그 폭을 넘지 않는다 — 마지막 `front_truncate`
    /// 폴백까지 포함해서.
    #[test]
    fn no_width_ever_overflows() {
        let sep = std::path::MAIN_SEPARATOR;
        let path = format!("{sep}Users{sep}codex{sep}Projects{sep}zo{sep}crates{sep}zo-ide");
        for width in 1..=display_width(&path) {
            let cut = center_truncate_path(&path, width);
            assert!(
                display_width(&cut) <= width,
                "width {width}: {cut:?} is {} wide",
                display_width(&cut)
            );
        }
    }

    /// 폭 1 은 `…` 한 글자다.
    #[test]
    fn width_one_is_a_lone_ellipsis() {
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(center_truncate_path(&format!("{sep}Users{sep}codex"), 1), "…");
    }

    /// 뿌리(`/`)만 남는 경로 — 원본의 `raw_segments.is_empty()` 갈래.
    #[test]
    fn the_root_survives_when_it_fits() {
        let sep = std::path::MAIN_SEPARATOR;
        let root = sep.to_string();
        assert_eq!(center_truncate_path(&format!("{sep}{sep}{sep}"), 2), root);
        assert_eq!(center_truncate_path(&format!("{sep}{sep}{sep}"), 0), "");
    }

    /// 폭을 세는 자는 글자 수가 아니라 표시 폭이다 — CJK 두 칸.
    #[test]
    fn wide_characters_count_two_columns() {
        let sep = std::path::MAIN_SEPARATOR;
        let path = format!("~{sep}문서{sep}프로젝트{sep}메모.txt");
        for width in 1..=display_width(&path) {
            let cut = center_truncate_path(&path, width);
            assert!(display_width(&cut) <= width, "width {width}: {cut:?}");
        }
    }

    fn display_width(text: &str) -> usize {
        text.chars().map(super::super::ansi::char_width).sum()
    }
}
