//! Deterministic auto-lane extraction for workflow specs.
//!
//! This is deliberately provider-free: it only honors explicit lane language in
//! the prompt, then leaves execution to the normal [`PhaseSource::Fanout`] path.

const LANE_MARKERS: &[&str] = &["auto lanes:", "lanes:", "workstreams:", "tracks:"];
const MAX_LANES: usize = 8;

/// Infer a conservative fan-out list from an otherwise single phase prompt.
/// Returns `None` unless the prompt explicitly asks for lane/workstream style
/// decomposition, so ordinary prose mentioning several files stays single-agent.
pub(crate) fn infer_auto_lane_fanout(prompt: &str) -> Option<Vec<String>> {
    let lower = prompt.to_ascii_lowercase();
    let has_orchestration_signal = ["parallel", "fanout", "fan-out", "split", "separate"]
        .iter()
        .any(|needle| lower.contains(needle));
    if !has_orchestration_signal {
        return None;
    }

    let lane_text = marker_lane_text(prompt, &lower)
        .or_else(|| slash_lane_text(prompt, &lower))?;
    let lanes = split_lanes(lane_text);
    (lanes.len() >= 2).then_some(lanes)
}

fn marker_lane_text<'a>(prompt: &'a str, lower: &str) -> Option<&'a str> {
    let (marker_start, marker_len) = LANE_MARKERS
        .iter()
        .filter_map(|marker| lower.find(marker).map(|idx| (idx, marker.len())))
        .min_by_key(|(idx, _)| *idx)?;
    let start = marker_start + marker_len;
    let rest = prompt.get(start..)?.trim();
    Some(rest.split(['\n', '.']).next().unwrap_or(rest).trim())
}

fn slash_lane_text<'a>(prompt: &'a str, lower: &str) -> Option<&'a str> {
    if !lower.contains("lanes") && !lower.contains("workstreams") {
        return None;
    }
    prompt
        .split_whitespace()
        .find(|token| token.matches('/').count() >= 1)
        .map(|token| token.trim_matches(|ch: char| {
            !ch.is_ascii_alphanumeric() && ch != '/' && ch != '-' && ch != '_'
        }))
}

fn split_lanes(text: &str) -> Vec<String> {
    let normalized = text
        .replace(" and ", ",")
        .replace(" AND ", ",")
        .replace(['/', '|', ';'], ",");
    let mut lanes = Vec::new();
    for raw in normalized.split(',') {
        let lane = clean_lane(raw);
        if lane.is_empty() || is_generic_lane(&lane) || lanes.contains(&lane) {
            continue;
        }
        lanes.push(lane);
        if lanes.len() == MAX_LANES {
            break;
        }
    }
    lanes
}

fn clean_lane(raw: &str) -> String {
    raw.trim()
        .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-' && ch != '_')
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch.to_ascii_lowercase())
            } else if matches!(ch, '-' | '_' | ' ') {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(48)
        .collect()
}

fn is_generic_lane(lane: &str) -> bool {
    matches!(lane, "lane" | "lanes" | "task" | "tasks" | "work" | "agent" | "agents")
}

#[cfg(test)]
mod tests {
    use super::infer_auto_lane_fanout;

    #[test]
    fn infers_explicit_marker_lanes() {
        assert_eq!(
            infer_auto_lane_fanout("split into parallel lanes: parser, executor, docs"),
            Some(vec!["parser".to_string(), "executor".to_string(), "docs".to_string()]),
        );
    }

    #[test]
    fn ignores_plain_lists_without_orchestration_signal() {
        assert_eq!(infer_auto_lane_fanout("touch parser, executor, docs"), None);
    }
}
