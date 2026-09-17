//! The commit graph the source control panel draws — Orca's git history,
//! measured whole (SourceControl-e46DLHZz.js, 1.4.169).
//!
//! Three layers, all protocol:
//!
//! - the DATA is one `git log` in a fixed format
//!   (`GIT_HISTORY_COMMIT_FORMAT`, out/main/index.js:106478 — hash, author,
//!   email, dates, parents, `%(decorate)` with a unit separator, body — NUL
//!   between records, `--topo-order`, limit+1 to learn `hasMore`);
//! - the LAYOUT is `buildGitHistoryViewModels` (SourceControl:2773): each
//!   row threads its parent's lanes through, the first parent takes the
//!   commit's own lane, extra parents open new lanes coloured by rotation
//!   over five lane colours, and the three special refs pin their colours
//!   (current → `git-graph-ref`, upstream → `git-graph-remote-ref`, base →
//!   `git-graph-base-ref`); synthetic "Incoming Changes"/"Outgoing Changes"
//!   rows are spliced around the merge base (:2637/:2698);
//! - the GEOMETRY is `GitHistoryGraphSvg` (:2886): 11px lanes, 24px rows,
//!   5px curve radius, a 3.5px dot — HEAD wears a halo, a merge wears a
//!   ring, a boundary row wears a dashed circle.
//!
//! The window draws elements from the geometry computed here — the same
//! rule as the mermaid renderer: layout in Rust, no markup on the path.

use serde::{Deserialize, Serialize};

/// The three pinned ref colours and the five rotating lane colours —
/// Orca's own identifiers, which are CSS custom property names.
pub const REF_COLOR: &str = "git-graph-ref";
pub const REMOTE_REF_COLOR: &str = "git-graph-remote-ref";
pub const BASE_REF_COLOR: &str = "git-graph-base-ref";
pub const LANE_COLORS: [&str; 5] = [
    "git-graph-lane-1",
    "git-graph-lane-2",
    "git-graph-lane-3",
    "git-graph-lane-4",
    "git-graph-lane-5",
];

/// The two synthetic rows' ids (:2587-2588).
pub const INCOMING_ID: &str = "git-history-incoming-changes";
pub const OUTGOING_ID: &str = "git-history-outgoing-changes";

/// Row geometry (:2866-2871).
pub const SWIMLANE_HEIGHT: f64 = 24.0;
pub const SWIMLANE_WIDTH: f64 = 11.0;
const CURVE_RADIUS: f64 = 5.0;
const NODE_Y: f64 = SWIMLANE_HEIGHT / 2.0;
const CIRCLE_RADIUS: f64 = 3.5;
const CIRCLE_STROKE: f64 = 1.5;

/// One ref a commit wears — a branch, a remote branch, a tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryRef {
    pub id: String,
    pub name: String,
    pub revision: String,
    /// `branches`, `remote branches`, `tags`, `commits` — Orca's four
    /// category words, verbatim.
    pub category: String,
    /// Filled by the layout for the refs the colour map knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// One commit, as the log parser produces it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: String,
    pub parent_ids: Vec<String>,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub display_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<i64>,
    #[serde(default)]
    pub references: Vec<HistoryRef>,
}

/// What kind of row this is: the head commit, an ordinary one, or one of
/// the two synthetic boundary rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RowKind {
    Head,
    Node,
    IncomingChanges,
    OutgoingChanges,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LaneNode {
    id: String,
    color: String,
}

/// One stroke of the graph, ready for a `<path>` element.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphPath {
    pub d: String,
    /// A colour identifier — the window spends it as `var(--{color})`.
    pub color: String,
    pub stroke_width: f64,
}

/// One circle of the node marker.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphCircle {
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
    /// A colour identifier, or `background` for the theme's own ground.
    pub fill: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dash: Option<String>,
}

/// One drawn row: the commit, its kind, and its geometry.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryRow {
    pub item: HistoryItem,
    pub kind: RowKind,
    pub width: f64,
    pub height: f64,
    pub paths: Vec<GraphPath>,
    pub circles: Vec<GraphCircle>,
}

struct ViewModel {
    item: HistoryItem,
    kind: RowKind,
    input: Vec<LaneNode>,
    output: Vec<LaneNode>,
}

/// The whole build: items in, drawn rows out.
#[allow(clippy::too_many_arguments)]
pub fn history_rows(
    items: Vec<HistoryItem>,
    current: Option<&HistoryRef>,
    remote: Option<&HistoryRef>,
    base: Option<&HistoryRef>,
    add_incoming: bool,
    add_outgoing: bool,
    merge_base: Option<&str>,
) -> Vec<HistoryRow> {
    let models = build_view_models(
        items,
        current,
        remote,
        base,
        add_incoming,
        add_outgoing,
        merge_base,
    );
    models.into_iter().map(render_row).collect()
}

/// The colour a commit's label pins, when one of its refs is in the map
/// (`getLabelColorIdentifier`, :2749).
fn label_color(item: &HistoryItem, map: &[(String, String)]) -> Option<String> {
    if item.id == INCOMING_ID {
        return Some(REMOTE_REF_COLOR.to_string());
    }
    if item.id == OUTGOING_ID {
        return Some(REF_COLOR.to_string());
    }
    for reference in &item.references {
        if let Some((_, color)) = map.iter().find(|(id, _)| *id == reference.id) {
            return Some(color.clone());
        }
    }
    None
}

/// The measured ref ordering on a row (`compareGitHistoryRefs`, :2760):
/// current first, then upstream, then base, then anything coloured.
fn ref_order(
    reference: &HistoryRef,
    current: Option<&HistoryRef>,
    remote: Option<&HistoryRef>,
    base: Option<&HistoryRef>,
) -> u8 {
    if current.is_some_and(|r| r.id == reference.id) {
        return 1;
    }
    if remote.is_some_and(|r| r.id == reference.id) {
        return 2;
    }
    if base.is_some_and(|r| r.id == reference.id) {
        return 3;
    }
    if reference.color.is_some() {
        return 4;
    }
    99
}

/// `buildGitHistoryViewModels` (:2773), lane for lane.
fn build_view_models(
    items: Vec<HistoryItem>,
    current: Option<&HistoryRef>,
    remote: Option<&HistoryRef>,
    base: Option<&HistoryRef>,
    add_incoming: bool,
    add_outgoing: bool,
    merge_base: Option<&str>,
) -> Vec<ViewModel> {
    // The colour map pins the three special refs (`buildDefaultGitHistoryColorMap`).
    let mut map: Vec<(String, String)> = Vec::new();
    if let Some(current) = current {
        map.push((current.id.clone(), REF_COLOR.to_string()));
    }
    if let Some(remote) = remote {
        map.push((remote.id.clone(), REMOTE_REF_COLOR.to_string()));
    }
    if let Some(base) = base {
        map.push((base.id.clone(), BASE_REF_COLOR.to_string()));
    }

    let all = items.clone();
    let mut color_index: i64 = -1;
    let mut models: Vec<ViewModel> = Vec::new();
    for item in items {
        let kind = if current.is_some_and(|r| r.revision == item.id) {
            RowKind::Head
        } else {
            RowKind::Node
        };
        let input: Vec<LaneNode> = models.last().map(|m| m.output.clone()).unwrap_or_default();
        let mut output: Vec<LaneNode> = Vec::new();
        let mut first_parent_added = false;
        if !item.parent_ids.is_empty() {
            for node in &input {
                if node.id == item.id {
                    if !first_parent_added {
                        output.push(LaneNode {
                            id: item.parent_ids[0].clone(),
                            color: label_color(&item, &map).unwrap_or_else(|| node.color.clone()),
                        });
                        first_parent_added = true;
                    }
                    continue;
                }
                output.push(node.clone());
            }
        }
        let start = usize::from(first_parent_added);
        for index in start..item.parent_ids.len() {
            let pinned = if index == 0 {
                label_color(&item, &map)
            } else {
                all.iter()
                    .find(|held| held.id == item.parent_ids[index])
                    .and_then(|parent| label_color(parent, &map))
            };
            let color = pinned.unwrap_or_else(|| {
                color_index = (color_index + 1).rem_euclid(LANE_COLORS.len() as i64);
                LANE_COLORS[color_index as usize].to_string()
            });
            output.push(LaneNode {
                id: item.parent_ids[index].clone(),
                color,
            });
        }
        // Refs take their pinned colour; a mapped ref whose colour is still
        // unresolved reads it off the lane under the dot (:2823).
        let mut references = item.references.clone();
        for reference in &mut references {
            reference.color = map
                .iter()
                .find(|(id, _)| *id == reference.id)
                .map(|(_, color)| color.clone());
        }
        let mut sorted = references;
        sorted.sort_by_key(|reference| ref_order(reference, current, remote, base));
        let mut item = item;
        item.references = sorted;
        models.push(ViewModel {
            item,
            kind,
            input,
            output,
        });
    }
    add_boundary_rows(
        &mut models,
        current,
        remote,
        add_incoming,
        add_outgoing,
        merge_base,
    );
    models
}

/// `addIncomingOutgoingChangesHistoryItems` (:2637).
fn add_boundary_rows(
    models: &mut Vec<ViewModel>,
    current: Option<&HistoryRef>,
    remote: Option<&HistoryRef>,
    add_incoming: bool,
    add_outgoing: bool,
    merge_base: Option<&str>,
) {
    let Some(merge_base) = merge_base else {
        return;
    };
    if current.map(|r| r.revision.as_str()) == remote.map(|r| r.revision.as_str()) {
        return;
    }
    if add_incoming
        && let Some(remote) = remote
        && remote.revision != merge_base
    {
        add_incoming_row(models, remote, merge_base);
    }
    if add_outgoing
        && let Some(current) = current
        && current.revision != merge_base
    {
        add_outgoing_row(models, current);
    }
}

fn display_id_len(models: &[ViewModel]) -> usize {
    models
        .first()
        .map_or(0, |model| model.item.display_id.len())
}

/// `addIncomingChangesHistoryItem` (:2649).
fn add_incoming_row(models: &mut Vec<ViewModel>, remote: &HistoryRef, merge_base: &str) {
    let before_index = models
        .iter()
        .rposition(|model| model.output.iter().any(|node| node.id == merge_base));
    let Some(after_index) = models.iter().position(|model| model.item.id == merge_base) else {
        return;
    };
    // A merge that already brought the remote in draws no boundary (:2662).
    if let Some(before_index) = before_index {
        let before = &models[before_index];
        if before.item.parent_ids.len() == 2
            && before.item.parent_ids.iter().any(|id| id == merge_base)
        {
            return;
        }
    }
    let boundary_input = |node: &LaneNode| -> LaneNode {
        if node.id == merge_base && node.color == REMOTE_REF_COLOR {
            LaneNode {
                id: INCOMING_ID.to_string(),
                color: node.color.clone(),
            }
        } else {
            node.clone()
        }
    };
    let mut input: Vec<LaneNode> = match before_index {
        Some(at) => models[at].output.iter().map(boundary_input).collect(),
        None => models[after_index].input.clone(),
    };
    let mut output: Vec<LaneNode> = models[after_index].input.clone();
    // `ensureIncomingRemoteLane` (:2607).
    if !output
        .iter()
        .any(|node| node.id == merge_base && node.color == REMOTE_REF_COLOR)
    {
        let local = output
            .iter()
            .position(|node| node.id == merge_base && node.color == REF_COLOR);
        let at = local.map_or(input.len(), |at| at + 1).min(output.len());
        output.insert(
            at,
            LaneNode {
                id: merge_base.to_string(),
                color: REMOTE_REF_COLOR.to_string(),
            },
        );
    }
    if !input
        .iter()
        .any(|node| node.id == INCOMING_ID && node.color == REMOTE_REF_COLOR)
    {
        let at = output
            .iter()
            .position(|node| node.id == merge_base && node.color == REMOTE_REF_COLOR)
            .unwrap_or(input.len())
            .min(input.len());
        input.insert(
            at,
            LaneNode {
                id: INCOMING_ID.to_string(),
                color: REMOTE_REF_COLOR.to_string(),
            },
        );
    }
    if let Some(at) = before_index {
        models[at].input = models[at].input.iter().map(boundary_input).collect();
        models[at].output = input.clone();
    }
    let id_len = display_id_len(models);
    let boundary = ViewModel {
        item: HistoryItem {
            id: INCOMING_ID.to_string(),
            display_id: "0".repeat(id_len),
            parent_ids: vec![merge_base.to_string()],
            author: Some(remote.name.clone()),
            subject: "Incoming Changes".to_string(),
            timestamp: None,
            references: Vec::new(),
        },
        kind: RowKind::IncomingChanges,
        input,
        output: output.clone(),
    };
    models.insert(after_index, boundary);
    models[after_index + 1].input = output;
}

/// `addOutgoingChangesHistoryItem` (:2698).
fn add_outgoing_row(models: &mut Vec<ViewModel>, current: &HistoryRef) {
    let revision = current.revision.clone();
    let Some(at) = models
        .iter()
        .position(|model| model.kind == RowKind::Head && model.item.id == revision)
    else {
        return;
    };
    let id_len = display_id_len(models);
    let input = models[at].input.clone();
    let mut output = input.clone();
    output.push(LaneNode {
        id: revision.clone(),
        color: REF_COLOR.to_string(),
    });
    let boundary = ViewModel {
        item: HistoryItem {
            id: OUTGOING_ID.to_string(),
            display_id: "0".repeat(id_len),
            parent_ids: vec![revision.clone()],
            author: Some(current.name.clone()),
            subject: "Outgoing Changes".to_string(),
            timestamp: None,
            references: Vec::new(),
        },
        kind: RowKind::OutgoingChanges,
        input,
        output,
    };
    models.insert(at, boundary);
    models[at + 1].input.push(LaneNode {
        id: revision,
        color: REF_COLOR.to_string(),
    });
}

/// `GitHistoryGraphSvg` (:2886), path for path.
fn render_row(model: ViewModel) -> HistoryRow {
    let item = &model.item;
    let input = &model.input;
    let output = &model.output;
    let input_index = input.iter().position(|node| node.id == item.id);
    let circle_index = input_index.unwrap_or(input.len());
    let circle_color = output
        .get(circle_index)
        .or_else(|| input.get(circle_index))
        .map_or(REF_COLOR, |node| node.color.as_str())
        .to_string();

    let w = SWIMLANE_WIDTH;
    let h = SWIMLANE_HEIGHT;
    let lane = |index: usize| w * (index as f64 + 1.0);
    let mut paths: Vec<GraphPath> = Vec::new();
    let mut output_at = 0usize;
    for (index, node) in input.iter().enumerate() {
        let color = node.color.clone();
        if node.id == item.id {
            if index != circle_index {
                paths.push(GraphPath {
                    d: format!(
                        "M {} 0 A {w} {w} 0 0 1 {} {NODE_Y} H {}",
                        lane(index),
                        w * index as f64,
                        lane(circle_index),
                    ),
                    color,
                    stroke_width: 1.0,
                });
            } else {
                output_at += 1;
            }
            continue;
        }
        if output_at < output.len() && node.id == output[output_at].id {
            if index == output_at {
                paths.push(GraphPath {
                    d: format!("M {} 0 V {h}", lane(index)),
                    color,
                    stroke_width: 1.0,
                });
            } else {
                paths.push(GraphPath {
                    d: format!(
                        "M {} 0 V 6 A {CURVE_RADIUS} {CURVE_RADIUS} 0 0 1 {} {} H {} A {CURVE_RADIUS} {CURVE_RADIUS} 0 0 0 {} {} V {h}",
                        lane(index),
                        lane(index) - CURVE_RADIUS,
                        h / 2.0,
                        lane(output_at) + CURVE_RADIUS,
                        lane(output_at),
                        h / 2.0 + CURVE_RADIUS,
                    ),
                    color,
                    stroke_width: 1.0,
                });
            }
            output_at += 1;
        }
    }
    for index in 1..item.parent_ids.len() {
        let parent_id = &item.parent_ids[index];
        let Some(parent_at) = output.iter().rposition(|node| &node.id == parent_id) else {
            continue;
        };
        paths.push(GraphPath {
            d: format!(
                "M {} {} A {w} {w} 0 0 1 {} {h} M {} {} H {}",
                w * parent_at as f64,
                h / 2.0,
                lane(parent_at),
                w * parent_at as f64,
                h / 2.0,
                lane(circle_index),
            ),
            color: output[parent_at].color.clone(),
            stroke_width: 1.0,
        });
    }
    if let Some(at) = input_index {
        paths.push(GraphPath {
            d: format!("M {} 0 V {}", lane(circle_index), h / 2.0),
            color: input[at].color.clone(),
            stroke_width: 1.0,
        });
    }
    if !item.parent_ids.is_empty() {
        paths.push(GraphPath {
            d: format!("M {} {} V {h}", lane(circle_index), h / 2.0),
            color: circle_color.clone(),
            stroke_width: 1.0,
        });
    }

    let cx = lane(circle_index);
    let cy = NODE_Y;
    let width = w * (input.len().max(output.len()).max(1) as f64 + 1.0);
    let boundary = matches!(
        model.kind,
        RowKind::IncomingChanges | RowKind::OutgoingChanges
    );
    let merge = item.parent_ids.len() > 1;
    let mut circles: Vec<GraphCircle> = Vec::new();
    let plain = |r: f64, fill: &str| GraphCircle {
        cx,
        cy,
        r,
        fill: fill.to_string(),
        stroke: None,
        stroke_width: None,
        dash: None,
    };
    match model.kind {
        RowKind::Head => {
            circles.push(GraphCircle {
                stroke: Some("background".to_string()),
                stroke_width: Some(CIRCLE_STROKE),
                ..plain(CIRCLE_RADIUS + 3.0, &circle_color)
            });
            circles.push(plain(CIRCLE_STROKE, "background"));
        }
        _ if boundary => {
            circles.push(GraphCircle {
                stroke: Some("background".to_string()),
                stroke_width: Some(CIRCLE_STROKE),
                ..plain(CIRCLE_RADIUS + 3.0, &circle_color)
            });
            circles.push(GraphCircle {
                stroke: Some("background".to_string()),
                stroke_width: Some(CIRCLE_STROKE + 1.0),
                ..plain(CIRCLE_RADIUS + 1.0, "background")
            });
            circles.push(GraphCircle {
                fill: "none".to_string(),
                stroke: Some(circle_color.clone()),
                stroke_width: Some(CIRCLE_STROKE - 1.0),
                dash: Some("4 2".to_string()),
                ..plain(CIRCLE_RADIUS + 1.0, "none")
            });
        }
        _ if merge => {
            circles.push(plain(CIRCLE_RADIUS + 1.0, &circle_color));
            circles.push(plain(CIRCLE_RADIUS - 1.5, "background"));
        }
        _ => circles.push(plain(CIRCLE_RADIUS, &circle_color)),
    }

    HistoryRow {
        item: model.item,
        kind: model.kind,
        width,
        height: h,
        paths,
        circles,
    }
}

/// The exact log format the main process asks git for
/// (out/main/index.js:106478): hash, author name, author email, author
/// time, commit time, parents, `%(decorate)` joined by a unit separator,
/// then the whole body — records separated by NUL under `-z`.
pub const COMMIT_FORMAT: &str =
    "%H%n%aN%n%aE%n%at%n%ct%n%P%n%(decorate:prefix=,suffix=,separator=%x1f)%n%B";

/// Commits out of one `-z` log in [`COMMIT_FORMAT`]. A record that does
/// not start with a hash is skipped, not an error — the log is the
/// authority and the parser only reads it.
pub fn parse_history_log(raw: &str) -> Vec<HistoryItem> {
    raw.split('\0')
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let mut lines = record.split('\n');
            let id = lines.next()?.trim().to_string();
            if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            let author = lines.next().unwrap_or("").trim().to_string();
            let _email = lines.next();
            let _authored = lines.next();
            let committed = lines.next().and_then(|at| at.trim().parse::<i64>().ok());
            let parent_ids: Vec<String> = lines
                .next()
                .unwrap_or("")
                .split_whitespace()
                .map(str::to_string)
                .collect();
            let references = parse_decorations(lines.next().unwrap_or(""), &id);
            let subject = lines
                .next()
                .map(|line| line.trim().to_string())
                .unwrap_or_default();
            Some(HistoryItem {
                display_id: id.chars().take(7).collect(),
                parent_ids,
                subject,
                author: (!author.is_empty()).then_some(author),
                timestamp: committed,
                references,
                id,
            })
        })
        .collect()
}

/// The decorations of one commit, parsed the way the main process parses
/// them (`parseGitDecorationRefs`, out/main/index.js:106486): plain `HEAD`
/// and a remote's own `HEAD` pointer are dropped, the four prefixes decide
/// the category, and the result sorts branches → remotes → tags.
pub fn parse_decorations(raw: &str, revision: &str) -> Vec<HistoryRef> {
    const SEPARATOR: char = '\u{1f}';
    if raw.trim().is_empty() {
        return Vec::new();
    }
    let parts: Vec<&str> = if raw.contains(SEPARATOR) {
        raw.split(SEPARATOR).collect()
    } else {
        raw.split(',').collect()
    };
    let mut refs = Vec::new();
    for part in parts {
        let text = part.trim();
        if text.is_empty() || text == "HEAD" {
            continue;
        }
        if let Some(rest) = text.strip_prefix("refs/remotes/")
            && rest.split('/').nth(1) == Some("HEAD")
        {
            continue;
        }
        if let Some(name) = text.strip_prefix("HEAD -> refs/heads/") {
            refs.push(HistoryRef {
                id: text["HEAD -> ".len()..].to_string(),
                name: name.to_string(),
                revision: revision.to_string(),
                category: "branches".to_string(),
                color: None,
            });
        } else if let Some(name) = text.strip_prefix("refs/heads/") {
            refs.push(HistoryRef {
                id: text.to_string(),
                name: name.to_string(),
                revision: revision.to_string(),
                category: "branches".to_string(),
                color: None,
            });
        } else if let Some(name) = text.strip_prefix("refs/remotes/") {
            refs.push(HistoryRef {
                id: text.to_string(),
                name: name.to_string(),
                revision: revision.to_string(),
                category: "remote branches".to_string(),
                color: None,
            });
        } else if let Some(name) = text.strip_prefix("tag: refs/tags/") {
            refs.push(HistoryRef {
                id: text["tag: ".len()..].to_string(),
                name: name.to_string(),
                revision: revision.to_string(),
                category: "tags".to_string(),
                color: None,
            });
        }
    }
    refs.sort_by(|left, right| {
        let order = |reference: &HistoryRef| -> u8 {
            if reference.id.starts_with("refs/heads/") {
                1
            } else if reference.id.starts_with("refs/remotes/") {
                2
            } else if reference.id.starts_with("refs/tags/") {
                3
            } else {
                99
            }
        };
        order(left)
            .cmp(&order(right))
            .then_with(|| left.name.cmp(&right.name))
    });
    refs
}

/// One file a commit touched, as `--name-status` spells it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitFile {
    pub path: String,
    /// The first letter of git's word: A(dded), M(odified), D(eleted),
    /// R(enamed), C(opied), T(ype changed). The rename score is dropped —
    /// "R100" and "R087" are one fact to a person reading a list.
    pub status: String,
    /// Where a rename or copy came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// Files out of one `-z` diff-tree (`--name-status -z`). Records are
/// `STATUS NUL path NUL`, and a rename or copy carries two paths:
/// `R<score> NUL old NUL new NUL`. NUL rather than tabs and newlines because
/// a path may contain both, and `-z` is git's own answer to that.
///
/// A record that runs out of fields is dropped, not an error — the stream is
/// git's and the parser only reads it (the same stance as
/// [`parse_history_log`]).
pub fn parse_commit_files(raw: &str) -> Vec<CommitFile> {
    let mut fields = raw.split('\0');
    let mut files = Vec::new();
    while let Some(status) = fields.next() {
        let status = status.trim();
        let Some(letter) = status.chars().next() else {
            continue;
        };
        let Some(first) = fields.next() else { break };
        let (origin, path) = if letter == 'R' || letter == 'C' {
            let Some(second) = fields.next() else { break };
            (Some(first.to_string()), second.to_string())
        } else {
            (None, first.to_string())
        };
        files.push(CommitFile {
            path,
            status: letter.to_string(),
            origin,
        });
    }
    files
}

/// The web page one commit lives at, built from the checkout's remote.
///
/// The two spellings a remote actually wears — scp (`git@host:owner/repo.git`)
/// and a URL (`https://…`, `ssh://git@…`) — normalize to the `https` host and
/// path, `.git` shed, `/commit/<id>` appended: GitHub's shape, which GitLab
/// and Gitea redirect from their own. `None` for anything else (a local path,
/// a bundle): a menu item that cannot act should not pretend it can.
#[must_use]
pub fn commit_web_url(remote: &str, id: &str) -> Option<String> {
    let remote = remote.trim();
    let base = if let Some(rest) = remote.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map_or(rest, |(_, host)| host);
        // A port is location, not path: `host:22/owner/repo` → `host/owner/repo`.
        match rest.split_once(':') {
            Some((host, tail)) => {
                format!(
                    "{host}{}",
                    tail.trim_start_matches(|c: char| c.is_ascii_digit())
                )
            }
            None => rest.to_string(),
        }
    } else if let Some(rest) = remote
        .strip_prefix("https://")
        .or_else(|| remote.strip_prefix("http://"))
    {
        rest.to_string()
    } else if remote.contains('@') && remote.contains(':') && !remote.contains("://") {
        let rest = remote.split_once('@').map(|(_, host)| host)?;
        rest.replacen(':', "/", 1)
    } else {
        return None;
    };
    let base = base.trim_end_matches('/').trim_end_matches(".git");
    if base.is_empty() || !base.contains('/') {
        return None;
    }
    Some(format!("https://{base}/commit/{id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(id: &str, parents: &[&str]) -> HistoryItem {
        HistoryItem {
            id: id.to_string(),
            parent_ids: parents.iter().map(|p| (*p).to_string()).collect(),
            subject: format!("commit {id}"),
            author: Some("dev".to_string()),
            display_id: id.chars().take(7).collect(),
            timestamp: Some(0),
            references: Vec::new(),
        }
    }

    fn branch(id: &str, name: &str, revision: &str) -> HistoryRef {
        HistoryRef {
            id: id.to_string(),
            name: name.to_string(),
            revision: revision.to_string(),
            category: "branches".to_string(),
            color: None,
        }
    }

    /// A linear chain: every row is one lane, the head wears its halo, and
    /// the current ref's colour runs the whole spine.
    #[test]
    fn a_linear_chain_is_one_lane_in_the_ref_color() {
        let current = branch("refs/heads/main", "main", "aaa");
        let rows = history_rows(
            vec![
                commit("aaa", &["bbb"]),
                commit("bbb", &["ccc"]),
                commit("ccc", &[]),
            ],
            Some(&current),
            None,
            None,
            false,
            false,
            None,
        );
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].kind, RowKind::Head);
        assert_eq!(rows[0].circles.len(), 2, "the head wears a halo");
        // Every dot sits in the first lane.
        for row in &rows {
            assert_eq!(row.circles[0].cx, SWIMLANE_WIDTH);
        }
        // The spine is the pinned ref colour — but only where the current
        // ref actually decorates: the head's out stroke carries it.
        let mut head_item = commit("aaa", &["bbb"]);
        head_item.references = vec![branch("refs/heads/main", "main", "aaa")];
        let rows = history_rows(
            vec![head_item, commit("bbb", &[])],
            Some(&current),
            None,
            None,
            false,
            false,
            None,
        );
        assert!(rows[0].paths.iter().any(|p| p.color == REF_COLOR));
    }

    /// A merge: the second parent opens a lane in the first rotation
    /// colour, the merge dot wears a ring, and the branch row shifts back.
    #[test]
    fn a_merge_opens_a_lane_and_wears_a_ring() {
        let rows = history_rows(
            vec![
                commit("m", &["a", "b"]),
                commit("a", &["base"]),
                commit("b", &["base"]),
                commit("base", &[]),
            ],
            None,
            None,
            None,
            false,
            false,
            None,
        );
        // The merge row: ring = two circles.
        assert_eq!(rows[0].circles.len(), 2);
        // Its second parent opened lane 2 in the first rotation colour.
        assert!(rows[0].paths.iter().any(|p| p.color == LANE_COLORS[0]));
        // Row `b` draws its dot in the second lane.
        let b_row = rows.iter().find(|row| row.item.id == "b").expect("b");
        assert_eq!(b_row.circles[0].cx, SWIMLANE_WIDTH * 2.0);
        // And the base row is wider than one lane on the way in.
        let base_row = rows.iter().find(|row| row.item.id == "base").expect("base");
        assert!(base_row.width >= SWIMLANE_WIDTH * 2.0);
    }

    /// The boundary rows: ahead and behind the upstream, the synthetic
    /// Incoming/Outgoing rows appear around the merge base with the
    /// measured ids, dashed circles, and zeroed display ids.
    #[test]
    fn incoming_and_outgoing_rows_are_spliced_in() {
        let current = branch("refs/heads/main", "main", "local");
        let mut remote = branch("refs/remotes/origin/main", "origin/main", "remote");
        remote.category = "remote branches".to_string();
        let rows = history_rows(
            vec![commit("local", &["base"]), commit("base", &[])],
            Some(&current),
            Some(&remote),
            None,
            true,
            true,
            Some("base"),
        );
        let ids: Vec<&str> = rows.iter().map(|row| row.item.id.as_str()).collect();
        assert_eq!(ids, vec![OUTGOING_ID, "local", INCOMING_ID, "base"]);
        let incoming = rows.iter().find(|r| r.item.id == INCOMING_ID).expect("in");
        assert_eq!(incoming.kind, RowKind::IncomingChanges);
        assert_eq!(incoming.item.display_id, "0".repeat("local".len()));
        assert!(incoming.circles.iter().any(|c| c.dash.is_some()));
        assert_eq!(incoming.item.author.as_deref(), Some("origin/main"));
        let outgoing = rows.iter().find(|r| r.item.id == OUTGOING_ID).expect("out");
        assert_eq!(outgoing.item.subject, "Outgoing Changes");
        // Up to date: no boundary rows at all.
        let same = branch("refs/remotes/origin/main", "origin/main", "local");
        let rows = history_rows(
            vec![commit("local", &["base"]), commit("base", &[])],
            Some(&current),
            Some(&same),
            None,
            true,
            true,
            Some("base"),
        );
        assert_eq!(rows.len(), 2);
    }

    /// The decoration parser: HEAD-arrow branches, plain branches, remote
    /// branches, tags — categorised and sorted; `HEAD` alone and a
    /// remote's HEAD pointer dropped.
    #[test]
    fn decorations_parse_into_orcas_categories() {
        let refs = parse_decorations(
            "tag: refs/tags/v1\u{1f}HEAD -> refs/heads/main\u{1f}refs/remotes/origin/main\u{1f}refs/remotes/origin/HEAD\u{1f}HEAD",
            "abc",
        );
        let said: Vec<(&str, &str)> = refs
            .iter()
            .map(|r| (r.name.as_str(), r.category.as_str()))
            .collect();
        assert_eq!(
            said,
            vec![
                ("main", "branches"),
                ("origin/main", "remote branches"),
                ("v1", "tags"),
            ]
        );
        // The comma spelling parses too — an older git without %x1f.
        let refs = parse_decorations("refs/heads/side, tag: refs/tags/v2", "abc");
        assert_eq!(refs.len(), 2);
    }

    /// The log parser: NUL-separated records in the measured format come
    /// back as commits — parents split, decorations categorised, the
    /// subject the first body line, a malformed record skipped.
    #[test]
    fn the_log_format_parses_record_for_record() {
        let raw = format!(
            "{a}\nAda\nada@x\n100\n200\naaa2 aaa3\nHEAD -> refs/heads/main\u{1f}tag: refs/tags/v1\nMerge the side\n\nBody text\0\n{b}\nBob\nbob@x\n50\n60\n\n\nFirst light\0\nnot-a-hash\nX",
            a = "a".repeat(40),
            b = "b".repeat(40),
        );
        let items = parse_history_log(&raw);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].parent_ids, vec!["aaa2", "aaa3"]);
        assert_eq!(items[0].subject, "Merge the side");
        assert_eq!(items[0].author.as_deref(), Some("Ada"));
        assert_eq!(items[0].timestamp, Some(200));
        assert_eq!(items[0].display_id, "a".repeat(7));
        assert_eq!(items[0].references.len(), 2);
        assert!(items[1].parent_ids.is_empty());
        assert_eq!(items[1].subject, "First light");
    }

    /// The ref ordering on a row: current, upstream, base, coloured, rest.
    #[test]
    fn refs_on_a_row_sort_current_remote_base() {
        let current = branch("refs/heads/main", "main", "aaa");
        let mut remote = branch("refs/remotes/origin/main", "origin/main", "aaa");
        remote.category = "remote branches".to_string();
        let mut item = commit("aaa", &[]);
        item.references = vec![
            branch("refs/heads/other", "other", "aaa"),
            remote.clone(),
            current.clone(),
        ];
        let rows = history_rows(
            vec![item],
            Some(&current),
            Some(&remote),
            None,
            false,
            false,
            None,
        );
        let names: Vec<&str> = rows[0]
            .item
            .references
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(names, vec!["main", "origin/main", "other"]);
        assert_eq!(rows[0].item.references[0].color.as_deref(), Some(REF_COLOR));
        assert_eq!(
            rows[0].item.references[1].color.as_deref(),
            Some(REMOTE_REF_COLOR)
        );
    }

    /// The three record shapes one `-z` name-status stream carries: a plain
    /// change, a rename with its two paths, and a path with a newline in it —
    /// the case `-z` exists for.
    #[test]
    fn commit_files_read_plain_renamed_and_hostile_paths() {
        let raw = "M\0src/main.rs\0R087\0old name.rs\0new\nname.rs\0A\0a\tb.txt\0";
        let files = parse_commit_files(raw);
        assert_eq!(
            files,
            vec![
                CommitFile {
                    path: "src/main.rs".to_string(),
                    status: "M".to_string(),
                    origin: None,
                },
                CommitFile {
                    path: "new\nname.rs".to_string(),
                    status: "R".to_string(),
                    origin: Some("old name.rs".to_string()),
                },
                CommitFile {
                    path: "a\tb.txt".to_string(),
                    status: "A".to_string(),
                    origin: None,
                },
            ]
        );
    }

    /// A truncated stream — a rename missing its second path — drops the
    /// broken record instead of inventing a file or panicking.
    #[test]
    fn a_truncated_name_status_stream_is_read_up_to_the_break() {
        let files = parse_commit_files("M\0kept.rs\0R100\0only-one-path");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "kept.rs");
        assert!(parse_commit_files("").is_empty());
    }

    /// Every spelling a remote actually wears lands on the same https page,
    /// and the ones that are not web-addressable answer nothing.
    #[test]
    fn a_commit_web_url_is_built_from_any_remote_spelling_or_not_at_all() {
        let id = "abc123";
        for remote in [
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo.git",
            "https://github.com/owner/repo",
            "ssh://git@github.com/owner/repo.git",
            "ssh://git@github.com:22/owner/repo.git",
        ] {
            assert_eq!(
                commit_web_url(remote, id).as_deref(),
                Some("https://github.com/owner/repo/commit/abc123"),
                "remote: {remote}"
            );
        }
        assert_eq!(commit_web_url("/home/dev/repo", id), None);
        assert_eq!(commit_web_url("../bundle.git", id), None);
        assert_eq!(commit_web_url("", id), None);
    }
}
