use runtime::{LaneRouteMetadata, RouteAutoClassifierMode};

use super::metadata::TaskRouteMetadata;
use super::planner::AgentNeedPlan;
use super::shape_words;
use super::shape::{RouteShapeInput, RouteShape};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct RouteShapeEvidence {
    pub requested_shape: Option<RouteShape>,
    pub independent_lanes: usize,
    pub has_findings: bool,
    pub ambiguous_ownership: bool,
    pub unsafe_request: bool,
    pub lane: Option<LaneRouteMetadata>,
    pub audit_notes: Vec<String>,
}

impl RouteShapeEvidence {
    pub(super) fn apply_to<'a>(
        &self,
        mut input: RouteShapeInput<'a>,
    ) -> RouteShapeInput<'a> {
        input.requested_shape = self.requested_shape;
        input.independent_lanes = self.independent_lanes;
        input.has_findings = self.has_findings;
        input.ambiguous_ownership = self.ambiguous_ownership;
        input.unsafe_request = self.unsafe_request;
        input
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct RouteEvidenceInput<'a> {
    pub subagent_type: Option<&'a str>,
    pub name: Option<&'a str>,
    pub description: &'a str,
    pub prompt: &'a str,
    pub workflow_member: bool,
    pub fanout_position: Option<(usize, usize)>,
    pub auto_classifier: RouteAutoClassifierMode,
}

pub(super) fn infer_route_shape_evidence(input: &RouteEvidenceInput<'_>) -> RouteShapeEvidence {
    let haystack = normalized_haystack(input);
    // Deliberately Assisted-ONLY: trusting authored lane/shape markers in
    // task text is the operator's explicit opt-in ("my fan-out prompts carry
    // markers"), which Probed does not imply — Probed's contract is the live
    // self-assessment probe (fused in `smart_model_for_fields`), nothing
    // about marker provenance. Keeping the two orthogonal also keeps this
    // predicate from accreting a mode list every new classifier mode must
    // remember to join (or silently regress).
    let assisted = input.auto_classifier == RouteAutoClassifierMode::Assisted;
    let independent_lanes = inferred_lane_count(&haystack, input.fanout_position, assisted);
    let requested_shape =
        shape_words::requested_route_shape(&requested_shape_haystack(input), &haystack, assisted);
    let has_findings = has_repairable_finding_signal(&haystack);
    let ambiguous_ownership = contains_any(&haystack, &[
        "ambiguous ownership",
        "unclear ownership",
        "same file",
        "same function",
        "tightly coupled",
        "coupled lanes",
    ]);
    let unsafe_request = contains_any(&haystack, &[
        "unsafe external",
        "data loss",
        "destructive",
        "unreviewable merge",
        "force push",
    ]);
    let lane = lane_metadata(input, independent_lanes);
    let mut audit_notes = Vec::new();
    if assisted {
        audit_notes.push("smart-assisted-classifier:provider-free-deterministic".to_string());
    } else if input.auto_classifier == RouteAutoClassifierMode::Probed {
        // Probed does not join the assisted marker contract (above), but its
        // participation still shows in the audit trail.
        audit_notes.push(input.auto_classifier.audit_note().to_string());
    }
    if let Some(shape) = requested_shape {
        audit_notes.push(format!("smart-route-requested-shape:{}", shape.label()));
    }
    if independent_lanes >= 2 {
        audit_notes.push(format!("smart-route-independent-lanes:{independent_lanes}"));
    }
    if let Some(lane) = &lane {
        audit_notes.push(format!("smart-route-lane:{}", lane.domain));
    }
    if has_findings {
        audit_notes.push("smart-route-findings:repairable".to_string());
    }
    if ambiguous_ownership {
        audit_notes.push("smart-route-ownership:ambiguous".to_string());
    }
    if unsafe_request {
        audit_notes.push("smart-route-request:unsafe".to_string());
    }

    RouteShapeEvidence {
        requested_shape,
        independent_lanes,
        has_findings,
        ambiguous_ownership,
        unsafe_request,
        lane,
        audit_notes,
    }
}

pub(super) fn shape_input_with_evidence<'a>(
    metadata: &'a TaskRouteMetadata,
    needs: &'a [AgentNeedPlan],
    evidence: &RouteShapeEvidence,
) -> RouteShapeInput<'a> {
    evidence.apply_to(RouteShapeInput::new(metadata, needs))
}

/// What the parent claims this unit IS — the only text a REQUESTED shape is
/// read from.
///
/// The brief is deliberately absent. A brief routinely quotes the person ("the
/// user wrote: 「병렬로 분석해」"), and folding it in let one lane of a fan-out
/// read its own parent's context as a request to fan out again. The mirror
/// image of [`super::turn::assess_agent_task`], which reads the prompt and
/// ignores the labels: there the prompt is the only thing the child actually
/// runs; here the labels are the only thing anyone claims about the child.
/// Everything else in the evidence — lane counting, findings, ownership,
/// safety — keeps reading [`normalized_haystack`], because those are
/// STRUCTURE, not a request.
fn requested_shape_haystack(input: &RouteEvidenceInput<'_>) -> String {
    format!(
        "{} {} {}",
        input.subagent_type.unwrap_or_default(),
        input.name.unwrap_or_default(),
        input.description,
    )
    .to_ascii_lowercase()
}

fn normalized_haystack(input: &RouteEvidenceInput<'_>) -> String {
    format!(
        "{} {} {} {} {}",
        input.subagent_type.unwrap_or_default(),
        input.name.unwrap_or_default(),
        input.description,
        input.prompt,
        if input.workflow_member { "workflow_member" } else { "" },
    )
    .to_ascii_lowercase()
}

fn inferred_lane_count(text: &str, fanout_position: Option<(usize, usize)>, assisted: bool) -> usize {
    let fanout_lanes = fanout_position.map_or(0, |(_, total)| total);
    let deterministic = fanout_lanes
        .max(explicit_lane_count(text))
        .max(slash_lane_count(text));
    if assisted {
        deterministic.max(assisted_marker_lane_count(text))
    } else {
        deterministic
    }
}

fn assisted_marker_lane_count(text: &str) -> usize {
    let Some(marker_start) = shape_words::ASSISTED_LANE_MARKERS
        .iter()
        .filter_map(|marker| text.find(marker).map(|idx| idx + marker.len()))
        .min()
    else {
        return 0;
    };
    text.get(marker_start..)
        .map(|tail| {
            tail.split(['\n', '.'])
                .next()
                .unwrap_or(tail)
                .replace(" and ", ",")
                .replace(['/', '|', ';'], ",")
                .split(',')
                .filter(|part| is_lane_token(part.trim()))
                .count()
        })
        .filter(|count| *count >= 2)
        .unwrap_or(0)
}

fn explicit_lane_count(text: &str) -> usize {
    for (word, count) in [
        ("two", 2),
        ("three", 3),
        ("four", 4),
        ("five", 5),
        ("2", 2),
        ("3", 3),
        ("4", 4),
        ("5", 5),
    ] {
        if contains_any(text, &[&format!("{word} lanes"), &format!("{word} tasks"), &format!("{word} separate")]) {
            return count;
        }
    }
    0
}

fn slash_lane_count(text: &str) -> usize {
    text.split_whitespace()
        .filter(|token| token.contains('/'))
        .map(|token| {
            token
                .trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '/' && ch != '-')
                .split('/')
                .filter(|part| is_lane_token(part))
                .count()
        })
        .max()
        .filter(|count| *count >= 2)
        .unwrap_or(0)
}

fn has_repairable_finding_signal(text: &str) -> bool {
    contains_any(text, &["finding", "verdict=fail", "verdict fail", "failed verifier"])
        && contains_any(text, &["fix", "repair", "reverify", "re-verify"])
}

fn lane_metadata(input: &RouteEvidenceInput<'_>, independent_lanes: usize) -> Option<LaneRouteMetadata> {
    let domain = input
        .name
        .and_then(domain_from_label)
        .or_else(|| domain_from_label(input.subagent_type.unwrap_or_default()))
        .or_else(|| domain_from_description(input.description));
    let position = input.fanout_position.filter(|(_, total)| *total >= 2);
    let domain = match (domain, position) {
        (Some(domain), _) => domain,
        // A concrete fan-out position is lane evidence by itself: sibling
        // spread (lane anti-affinity in the selector) must work even when no
        // domain label is inferable from the agent's name/type/description.
        (None, Some(_)) => "parallel".to_string(),
        (None, None) => return None,
    };
    let mut lane = LaneRouteMetadata::new(domain);
    if let Some((index, total)) = position {
        lane = lane.with_position(index, total);
    } else if independent_lanes >= 2 {
        lane = lane.with_position(0, independent_lanes);
    }
    Some(lane)
}

fn domain_from_description(description: &str) -> Option<String> {
    let lower = description.trim().to_ascii_lowercase();
    for prefix in ["implement ", "verify ", "review ", "debug ", "fix "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let first = rest.split_whitespace().next().unwrap_or_default();
            return domain_from_label(first);
        }
    }
    None
}

fn domain_from_label(label: &str) -> Option<String> {
    let label = label.trim().trim_start_matches("custom:");
    let label = label.rsplit_once(':').map_or(label, |(_, tail)| tail).trim();
    if label.is_empty() {
        return None;
    }
    let normalized = label
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
        .to_string();
    if normalized.is_empty() || is_generic_domain(&normalized) || normalized.chars().count() > 48 {
        return None;
    }
    Some(normalized)
}

fn is_lane_token(token: &str) -> bool {
    let token = token.trim_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-');
    token.len() >= 2 && !is_generic_domain(token)
}

fn is_generic_domain(value: &str) -> bool {
    matches!(
        value,
        "agent"
            | "general-purpose"
            | "verification"
            | "verifier"
            | "review"
            | "reviewer"
            | "code-reviewer"
            | "debugger"
            | "plan"
            | "explore"
    )
}

fn contains_any(text: &str, needles: &[&str]) -> bool { needles.iter().any(|needle| text.contains(needle)) }
