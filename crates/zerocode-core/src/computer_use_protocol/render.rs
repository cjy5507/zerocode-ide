//! How an accessibility tree becomes the `treeText` an agent reads.
//!
//! A one-for-one port of the helper's `SnapshotRenderHeuristics` and
//! `TreeRenderer` (`native/computer-use-macos`): the same elision of
//! anonymous wrappers, the same compact controls whose children are folded
//! away, the same browser tab-strip compaction, the same line grammar
//! (`"{index} {role} ({traits}) {name}, Description: …, Value: …"`). The
//! platform supplies nodes through [`TreeSource`]; this module decides what
//! is written and which index each element gets, so a `--element-index` means
//! the same thing to every provider.
//!
//! Roles are spelled the macOS way (`AXButton`, `AXGroup`, …) on every
//! platform: they are a shared vocabulary the heuristics key on, not a claim
//! that the underlying API is AX. A Windows provider maps UIA control types
//! onto them once (`identity`-style tables live beside the adapter).

use std::collections::BTreeMap;

use super::{AppIdentity, MAX_TREE_DEPTH, MAX_TREE_NODES};
use crate::computer_use::MARK_CLIP_ROLES;

/// Everything the renderer wants to know about one element.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderNode {
    pub role: String,
    pub role_description: Option<String>,
    pub title: Option<String>,
    pub label: Option<String>,
    pub link_text: Option<String>,
    pub value: Option<String>,
    pub placeholder: Option<String>,
    pub url: Option<String>,
    pub traits: Vec<String>,
    pub raw_actions: Vec<String>,
    pub child_count: usize,
    pub summary: Option<String>,
    pub row_summary: Option<String>,
    pub web_area_depth: Option<usize>,
}

impl RenderNode {
    #[must_use]
    pub fn with_role(role: &str) -> Self {
        Self {
            role: role.to_string(),
            ..Self::default()
        }
    }
}

/// Which children of a browser tab strip survive rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabStripCompaction {
    pub retained_indexes: std::collections::BTreeSet<usize>,
    pub omitted_count: usize,
}

/// A rectangle in the provider's coordinate space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    #[must_use]
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub fn mid_x(&self) -> f64 {
        self.x + self.width / 2.0
    }

    #[must_use]
    pub fn mid_y(&self) -> f64 {
        self.y + self.height / 2.0
    }

    #[must_use]
    pub fn max_x(&self) -> f64 {
        self.x + self.width
    }

    #[must_use]
    pub fn max_y(&self) -> f64 {
        self.y + self.height
    }

    #[must_use]
    pub fn area(&self) -> f64 {
        self.width.max(0.0) * self.height.max(0.0)
    }

    /// The overlap, or `None` when the two do not meet (`CGRect.intersection`
    /// answering the null rect).
    #[must_use]
    pub fn intersection(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let max_x = self.max_x().min(other.max_x());
        let max_y = self.max_y().min(other.max_y());
        (max_x >= x && max_y >= y).then(|| Rect::new(x, y, max_x - x, max_y - y))
    }

    #[must_use]
    pub fn intersects(&self, other: &Rect) -> bool {
        self.intersection(other)
            .is_some_and(|overlap| overlap.width > 0.0 && overlap.height > 0.0)
    }

    /// `windowFramesMatch`: the same window within two units on every edge.
    #[must_use]
    pub fn matches_within(&self, other: &Rect, tolerance: f64) -> bool {
        (self.x - other.x).abs() <= tolerance
            && (self.y - other.y).abs() <= tolerance
            && (self.width - other.width).abs() <= tolerance
            && (self.height - other.height).abs() <= tolerance
    }

    /// Whether the point lies inside (edges included).
    #[must_use]
    pub fn contains_point(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.max_x() && y >= self.y && y <= self.max_y()
    }

    /// Whether `other` lies wholly inside this one (edges included).
    #[must_use]
    pub fn contains_rect(&self, other: &Rect) -> bool {
        other.x >= self.x
            && other.y >= self.y
            && other.max_x() <= self.max_x()
            && other.max_y() <= self.max_y()
    }

    /// Intersection over union: 1 for the same rectangle, 0 for two apart.
    #[must_use]
    pub fn iou(&self, other: &Rect) -> f64 {
        let overlap = self
            .intersection(other)
            .map_or(0.0, |overlap| overlap.area());
        let union = self.area() + other.area() - overlap;
        if union > 0.0 { overlap / union } else { 0.0 }
    }

    /// `isTargetWindowFocused`'s "mostly the same rectangle" test: the overlap
    /// covers at least three quarters of the smaller of the two.
    #[must_use]
    pub fn mostly_covers(&self, other: &Rect) -> bool {
        self.intersection(other)
            .is_some_and(|overlap| overlap.area() >= self.area().min(other.area()) * 0.75)
    }

    /// What of this rectangle `other` leaves showing: itself when the two do
    /// not overlap, nothing when `other` hides all of it, else up to four
    /// pieces that do not overlap — the bands above and below the overlap,
    /// and beside it between them.
    #[must_use]
    pub fn minus(&self, other: &Rect) -> Vec<Rect> {
        let Some(cut) = self.intersection(other).filter(|_| self.intersects(other)) else {
            return vec![*self];
        };
        [
            Rect::new(self.x, self.y, self.width, cut.y - self.y),
            Rect::new(self.x, cut.max_y(), self.width, self.max_y() - cut.max_y()),
            Rect::new(self.x, cut.y, cut.x - self.x, cut.height),
            Rect::new(cut.max_x(), cut.y, self.max_x() - cut.max_x(), cut.height),
        ]
        .into_iter()
        .filter(|piece| piece.width > 0.0 && piece.height > 0.0)
        .collect()
    }
}

/// The centres of the pieces `area` falls into when it is cut at every edge
/// of `cuts`: each piece lies wholly inside or wholly outside each cutting
/// rectangle, so what holds at its centre holds for all of it.
#[must_use]
pub fn piece_centres(area: Rect, cuts: &[Rect]) -> Vec<(f64, f64)> {
    let at = |start: f64, end: f64, edges: &mut dyn Iterator<Item = f64>| {
        let mut at: Vec<f64> = [start, end]
            .into_iter()
            .chain(edges.filter(|edge| *edge > start && *edge < end))
            .collect();
        at.sort_by(f64::total_cmp);
        at.dedup();
        at
    };
    let xs = at(
        area.x,
        area.max_x(),
        &mut cuts.iter().flat_map(|cut| [cut.x, cut.max_x()]),
    );
    let ys = at(
        area.y,
        area.max_y(),
        &mut cuts.iter().flat_map(|cut| [cut.y, cut.max_y()]),
    );
    xs.windows(2)
        .flat_map(|x| {
            ys.windows(2)
                .map(move |y| ((x[0] + x[1]) / 2.0, (y[0] + y[1]) / 2.0))
        })
        .collect()
}

/// Whether the rectangles in `covers` together hide all of `area`.
#[must_use]
pub fn fully_covered(area: Rect, covers: &[Rect]) -> bool {
    piece_centres(area, covers)
        .into_iter()
        .all(|(x, y)| covers.iter().any(|cover| cover.contains_point(x, y)))
}

/// The role words that make an element text-like — the only ones whose
/// metadata is probed for the words that mark a secret field. The Swift
/// helper holds the same list (a source contract pins it).
pub const SECURE_TEXT_PROBE_ROLE_WORDS: &[&str] = &["text", "field", "password", "search", "combo"];
/// The words that mark a secret field when found in its role, subrole,
/// title, label or placeholder: its value is `[redacted]`, and keys typed
/// into it are watched. Only the platform's own secure-field subrole refuses
/// typing (`validate::secret_entry`) — a word can be a hint's label.
pub const SECURE_TEXT_WORDS: &[&str] = &[
    "secure",
    "password",
    "passcode",
    "verification code",
    "one-time code",
];

/// Text-like roles are the only ones whose metadata is probed for the words
/// that mark a secret field.
#[must_use]
pub fn should_probe_secure_text_metadata(role: &str) -> bool {
    let normalized = role.to_lowercase();
    SECURE_TEXT_PROBE_ROLE_WORDS
        .iter()
        .any(|word| normalized.contains(word))
}

/// Whether a text field's role, subrole, title, label or placeholder says
/// it holds a secret.
#[must_use]
pub fn looks_like_secure_text(haystack: &str) -> bool {
    let haystack = haystack.to_lowercase();
    SECURE_TEXT_WORDS.iter().any(|word| haystack.contains(word))
}

/// An attribute is worth asking for only when the element advertises it; an
/// element that advertises nothing is asked for everything.
#[must_use]
pub fn supports_attribute(
    attribute: &str,
    advertised: Option<&std::collections::HashSet<String>>,
) -> bool {
    advertised.is_none_or(|set| set.contains(attribute))
}

fn sanitize_str(value: &str) -> String {
    value.replace(['\n', '\r'], " ")
}

/// Newlines folded to spaces: one element, one line.
#[must_use]
pub fn sanitize(value: &str) -> String {
    sanitize_str(value)
}

fn clean(value: Option<&str>) -> Option<String> {
    let sanitized = sanitize_str(value?);
    (!sanitized.is_empty()).then_some(sanitized)
}

/// A value shortened for a verification preview or a text snippet.
#[must_use]
pub fn preview(value: &str, max_length: usize) -> String {
    let cleaned = sanitize_str(value);
    if cleaned.chars().count() <= max_length {
        return cleaned;
    }
    let mut shortened: String = cleaned.chars().take(max_length).collect();
    shortened.push_str("...");
    shortened
}

fn markdown_escaped(value: &str) -> String {
    sanitize_str(value)
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

/// The name a line leads with, by role: a title first; a link renders as
/// markdown; a web area or button falls back to its label; a row to its text.
#[must_use]
pub fn display_name(node: &RenderNode) -> Option<String> {
    if let Some(title) = clean(node.title.as_deref()) {
        return Some(title);
    }
    if node.role == "AXLink"
        && let Some(url) = clean(node.url.as_deref())
        && let Some(text) = clean(
            node.link_text
                .as_deref()
                .or(node.label.as_deref())
                .or(node.value.as_deref()),
        )
    {
        return Some(format!("[{}]({url})", markdown_escaped(&text)));
    }
    if node.role == "AXWebArea" {
        return clean(node.label.as_deref()).or_else(|| clean(node.value.as_deref()));
    }
    if matches!(node.role.as_str(), "AXButton" | "AXPopUpButton" | "AXImage") {
        return clean(node.label.as_deref());
    }
    if matches!(node.role.as_str(), "AXRow" | "AXCell" | "AXOutlineRow") {
        return clean(node.row_summary.as_deref());
    }
    clean(node.label.as_deref())
}

/// An action name without a platform prefix, for the comparisons below.
#[must_use]
pub fn bare_action(action: &str) -> &str {
    action.strip_prefix("AX").unwrap_or(action)
}

/// The actions worth listing as "Secondary Actions": the primary press and
/// the plumbing every element has are noise. Spelled for both vocabularies —
/// macOS `AXPress`/`AXScrollToVisible`, Windows `Invoke`/`ScrollIntoView`.
#[must_use]
pub fn meaningful_actions(raw_actions: &[String], role: &str) -> Vec<String> {
    const NOISY: &[&str] = &[
        "Press",
        "ShowDefaultUI",
        "ShowAlternateUI",
        "ShowMenu",
        "ScrollToVisible",
        "Confirm",
        "Raise",
        "Invoke",
        "ScrollIntoView",
    ];
    let has_vertical_page_scroll = raw_actions
        .iter()
        .any(|action| matches!(bare_action(action), "ScrollUpByPage" | "ScrollDownByPage"));
    raw_actions
        .iter()
        .filter(|action| {
            let bare = bare_action(action);
            if NOISY.contains(&bare) {
                return false;
            }
            if role == "AXMenu" || role == "AXMenuItem" {
                return bare != "Cancel" && bare != "Pick";
            }
            if role == "AXScrollArea"
                && has_vertical_page_scroll
                && matches!(bare, "ScrollLeftByPage" | "ScrollRightByPage")
            {
                return false;
            }
            true
        })
        .cloned()
        .collect()
}

/// An anonymous wrapper adds nothing an agent can act on; its children take
/// its place at the same depth. Web content keeps multi-child groups because
/// their structure carries meaning.
#[must_use]
pub fn should_elide(node: &RenderNode) -> bool {
    if node.role != "AXGroup" && node.role != "AXUnknown" {
        return false;
    }
    if display_name(node).is_some()
        || !node.traits.is_empty()
        || !meaningful_actions(&node.raw_actions, &node.role).is_empty()
        || clean(node.summary.as_deref()).is_some()
    {
        return false;
    }
    if node.web_area_depth.is_some() && node.child_count > 1 {
        return false;
    }
    true
}

const COMPACT_CONTROL_ROLES: &[&str] = &[
    "AXButton",
    "AXCheckBox",
    "AXComboBox",
    "AXDisclosureTriangle",
    "AXHeading",
    "AXMenuItem",
    "AXPopUpButton",
    "AXRadioButton",
    "AXStaticText",
    "AXTab",
];

/// A named compact control is one line; its inner labels would only repeat it.
#[must_use]
pub fn should_suppress_children(node: &RenderNode) -> bool {
    if node.role == "AXMenuBarItem" {
        return true;
    }
    let name = display_name(node);
    if node.role == "AXLink" && name.as_deref().is_some_and(|name| name.starts_with('[')) {
        return true;
    }
    let has_compact_label = name.is_some()
        || clean(node.value.as_deref()).is_some()
        || clean(node.summary.as_deref()).is_some();
    has_compact_label && COMPACT_CONTROL_ROLES.contains(&node.role.as_str())
}

fn is_browser_tab_strip_container(node: &RenderNode) -> bool {
    let role_text = role_text(node);
    let title = clean(node.title.as_deref())
        .unwrap_or_default()
        .to_lowercase();
    let label = clean(node.label.as_deref())
        .unwrap_or_default()
        .to_lowercase();
    let description = clean(node.role_description.as_deref())
        .unwrap_or_default()
        .to_lowercase();
    // `AXTabGroup` is the UIA `Tab` control a Windows browser keeps its strip
    // in; macOS browsers expose theirs as a scroll area. Both hold tab-like
    // children and both are only compacted for a known browser.
    role_text == "scroll area"
        || node.role == "AXTabGroup"
        || description == "tab bar"
        || title == "tab bar"
        || label == "tab bar"
}

fn is_browser_tab_like(node: &RenderNode) -> bool {
    let role_description = clean(node.role_description.as_deref())
        .unwrap_or_default()
        .to_lowercase();
    node.role == "AXTab" || role_description == "tab" || role_description == "tab item"
}

fn is_selected_browser_tab(node: &RenderNode) -> bool {
    node.traits.iter().any(|trait_| trait_ == "selected")
        || clean(node.value.as_deref()).as_deref() == Some("1")
}

/// Ten or more tabs with one selected: keep the selected one, count the rest.
#[must_use]
pub fn tab_strip_compaction(
    parent: &RenderNode,
    children: &[RenderNode],
) -> Option<TabStripCompaction> {
    if !is_browser_tab_strip_container(parent) {
        return None;
    }
    let tab_indexes: std::collections::BTreeSet<usize> = children
        .iter()
        .enumerate()
        .filter(|(_, child)| is_browser_tab_like(child))
        .map(|(index, _)| index)
        .collect();
    if tab_indexes.len() < 10 {
        return None;
    }
    let selected: std::collections::BTreeSet<usize> = tab_indexes
        .iter()
        .copied()
        .filter(|index| is_selected_browser_tab(&children[*index]))
        .collect();
    if selected.is_empty() {
        return None;
    }
    let mut retained: std::collections::BTreeSet<usize> = (0..children.len())
        .filter(|index| !tab_indexes.contains(index))
        .collect();
    retained.extend(selected.iter().copied());
    let omitted_count = tab_indexes.len() - selected.len();
    if omitted_count == 0 {
        return None;
    }
    Some(TabStripCompaction {
        retained_indexes: retained,
        omitted_count,
    })
}

fn split_camel_case(value: &str) -> String {
    let mut result = String::with_capacity(value.len() + 4);
    for character in value.chars() {
        if character.is_uppercase() && !result.is_empty() {
            result.push(' ');
        }
        result.push(character);
    }
    result
}

/// The role word a line shows: the platform's localized description when it
/// has one, the split role name otherwise, and a few fixed words.
#[must_use]
pub fn role_text(node: &RenderNode) -> String {
    if node.role == "AXGroup" || node.role == "AXUnknown" {
        return "container".into();
    }
    if node.role == "AXLink" {
        return "link".into();
    }
    if node.role == "AXWebArea" {
        return clean(node.role_description.as_deref()).unwrap_or_else(|| "html content".into());
    }
    if node.role == "AXMenuBarItem" {
        return String::new();
    }
    if let Some(value) = clean(node.role_description.as_deref()) {
        return value.to_lowercase();
    }
    if let Some(stripped) = node.role.strip_prefix("AX") {
        return split_camel_case(stripped).to_lowercase();
    }
    node.role.clone()
}

/// `AXZoomWindow` → "zoom the window", `AXScrollDownByPage` → "scroll down",
/// `Invoke` → "invoke".
#[must_use]
pub fn pretty_action(action: &str) -> String {
    if action == "AXZoomWindow" {
        return "zoom the window".into();
    }
    let stripped = bare_action(action).replace("ByPage", "");
    split_camel_case(&stripped).to_lowercase()
}

fn formatted_value_segment(
    role_text: &str,
    name: Option<&str>,
    value: Option<&str>,
) -> Option<String> {
    let value = clean(value)?;
    if name == Some(value.as_str()) {
        return None;
    }
    if role_text == "heading" && value.parse::<i64>().is_ok() {
        return None;
    }
    if matches!(
        role_text,
        "text" | "text entry area" | "scroll bar" | "value indicator"
    ) {
        return Some(format!(" {value}"));
    }
    Some(format!(", Value: {value}"))
}

/// One rendered line, without its indentation.
#[must_use]
pub fn line(index: usize, node: &RenderNode) -> String {
    let name = display_name(node);
    let role_text = role_text(node);
    let meaningful = meaningful_actions(&node.raw_actions, &node.role);
    let mut line = if role_text.is_empty() {
        index.to_string()
    } else {
        format!("{index} {role_text}")
    };
    if !node.traits.is_empty() {
        line.push_str(&format!(" ({})", node.traits.join(", ")));
    }
    if let Some(name) = &name {
        line.push(' ');
        line.push_str(&sanitize_str(name));
    }
    if node.role != "AXLink"
        && let Some(description) = clean(node.label.as_deref())
        && Some(&description) != name.as_ref()
    {
        line.push_str(&format!(", Description: {}", sanitize_str(&description)));
    }
    if let Some(segment) =
        formatted_value_segment(&role_text, name.as_deref(), node.value.as_deref())
    {
        line.push_str(&segment);
    }
    if let Some(placeholder) = clean(node.placeholder.as_deref())
        && Some(&placeholder) != name.as_ref()
        && Some(placeholder.as_str()) != node.value.as_deref()
    {
        if name.is_none() && clean(node.value.as_deref()).is_none() {
            line.push_str(&format!(" Placeholder: {}", sanitize_str(&placeholder)));
        } else {
            line.push_str(&format!(", Placeholder: {}", sanitize_str(&placeholder)));
        }
    }
    if let Some(summary) = clean(node.summary.as_deref())
        && Some(&summary) != name.as_ref()
    {
        line.push_str(&format!(", Text: {}", sanitize_str(&summary)));
    } else if let Some(row_summary) = clean(node.row_summary.as_deref())
        && Some(&row_summary) != name.as_ref()
    {
        line.push_str(&format!(", Text: {}", sanitize_str(&row_summary)));
    }
    if !meaningful.is_empty() {
        line.push_str(&format!(
            ", Secondary Actions: {}",
            meaningful
                .iter()
                .map(|action| pretty_action(action))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    line
}

/// What proves an element is still the element it was: identity, not state.
/// A text field whose value changed after focus is the same field.
#[must_use]
pub fn element_signature(node: &RenderNode) -> String {
    [
        node.role.clone(),
        node.role_description.clone().unwrap_or_default(),
        node.title.clone().unwrap_or_default(),
        node.label.clone().unwrap_or_default(),
        node.link_text.clone().unwrap_or_default(),
        node.url.clone().unwrap_or_default(),
        meaningful_actions(&node.raw_actions, &node.role).join(","),
    ]
    .join("\u{1f}")
}

/// The whole `treeText`: the app line, the window line, the tree, the focus.
#[must_use]
pub fn render_tree_text(
    app: &AppIdentity,
    title: &str,
    lines: &[String],
    focused: Option<&str>,
) -> String {
    let app_id = app
        .bundle_id
        .clone()
        .unwrap_or_else(|| app.name.replace(' ', "_"));
    let mut output = vec![
        format!("App={app_id} (pid {})", app.pid),
        format!(
            "Window: \"{}\", App: {}.",
            sanitize_str(title),
            sanitize_str(&app.name)
        ),
        String::new(),
    ];
    output.extend(lines.iter().cloned());
    output.push(String::new());
    output.push(focused.map_or_else(
        || "No UI element is currently focused.".to_string(),
        |focused| format!("The focused UI element is {focused}."),
    ));
    output.join("\n")
}

/// One element as the platform describes it to the walk.
#[derive(Debug, Clone)]
pub struct Described<H> {
    pub node: RenderNode,
    pub children: Vec<H>,
    /// The element's frame relative to the window, when it has one.
    pub local_frame: Option<Rect>,
}

/// The platform's side of the walk. `describe` is asked once per visited
/// element; `peek` is asked for a container's children when deciding whether
/// they are a browser tab strip and may answer more cheaply.
pub trait TreeSource {
    type Handle: Clone + PartialEq;

    fn describe(
        &mut self,
        handle: &Self::Handle,
        ancestors: &[Self::Handle],
    ) -> Described<Self::Handle>;

    fn peek(&mut self, handle: &Self::Handle) -> RenderNode {
        self.describe(handle, &[]).node
    }

    fn is_focused(&mut self, handle: &Self::Handle) -> bool;
}

/// One indexed element after the walk: what an action needs to find it again,
/// and the face a mark is drawn from (`marks::element_faces`).
#[derive(Debug, Clone)]
pub struct RenderedRecord<H> {
    pub index: usize,
    pub handle: H,
    pub local_frame: Option<Rect>,
    pub actions: Vec<String>,
    pub signature: String,
    pub role: String,
    /// The line's name ([`display_name`]): a row's text, a link's words.
    pub name: Option<String>,
    pub placeholder: Option<String>,
    pub traits: Vec<String>,
    /// The frame cut by every clipping container above it
    /// (`MARK_CLIP_ROLES`); zero-sized where it is cut off whole.
    pub visible: Option<Rect>,
    /// The nearest named element above it — the row a star sits in — so a
    /// control is known by where it is as well as what it is.
    pub context: Option<String>,
}

/// What a walk carries down to a child: the clip its containers make, and
/// the nearest name above it.
#[derive(Debug, Clone, Default)]
struct Inherited {
    clip: Option<Rect>,
    context: Option<String>,
}

/// A frame cut by a clip; a frame cut off whole keeps its corner and no size.
#[must_use]
pub fn clipped(frame: Rect, clip: Option<Rect>) -> Rect {
    match clip {
        None => frame,
        Some(clip) => frame
            .intersection(&clip)
            .unwrap_or(Rect::new(frame.x, frame.y, 0.0, 0.0)),
    }
}

/// The walk's answer.
#[derive(Debug, Clone)]
pub struct RenderedTree<H> {
    pub lines: Vec<String>,
    pub records: BTreeMap<usize, RenderedRecord<H>>,
    pub focused_element_id: Option<usize>,
    pub focused_summary: Option<String>,
    pub truncated: bool,
    pub max_depth_reached: bool,
}

struct Walk<'a, S: TreeSource> {
    source: &'a mut S,
    compact_browser_tabs: bool,
    lines: Vec<String>,
    records: BTreeMap<usize, RenderedRecord<S::Handle>>,
    focused_element_id: Option<usize>,
    focused_summary: Option<String>,
    truncated: bool,
    max_depth_reached: bool,
    next_index: usize,
}

/// Render a window's tree from its root. `compact_browser_tabs` is the
/// provider's "this is a known browser" answer.
pub fn walk<S: TreeSource>(
    source: &mut S,
    root: &S::Handle,
    compact_browser_tabs: bool,
) -> RenderedTree<S::Handle> {
    let mut walk = Walk {
        source,
        compact_browser_tabs,
        lines: Vec::new(),
        records: BTreeMap::new(),
        focused_element_id: None,
        focused_summary: None,
        truncated: false,
        max_depth_reached: false,
        next_index: 0,
    };
    walk.render(root, 0, &[], &Inherited::default());
    RenderedTree {
        lines: walk.lines,
        records: walk.records,
        focused_element_id: walk.focused_element_id,
        focused_summary: walk.focused_summary,
        truncated: walk.truncated,
        max_depth_reached: walk.max_depth_reached,
    }
}

impl<S: TreeSource> Walk<'_, S> {
    fn render(
        &mut self,
        handle: &S::Handle,
        depth: usize,
        ancestors: &[S::Handle],
        inherited: &Inherited,
    ) {
        if self.next_index >= MAX_TREE_NODES {
            self.truncated = true;
            return;
        }
        if depth >= MAX_TREE_DEPTH {
            self.truncated = true;
            self.max_depth_reached = true;
            return;
        }
        if ancestors.contains(handle) {
            return;
        }
        let Described {
            node,
            children,
            local_frame,
        } = self.source.describe(handle, ancestors);
        let mut lineage = ancestors.to_vec();
        lineage.push(handle.clone());
        if should_elide(&node) {
            for child in &children {
                self.render(child, depth, &lineage, inherited);
            }
            return;
        }
        let index = self.next_index;
        self.next_index += 1;
        let rendered = line(index, &node);
        self.lines.push(format!("{}{rendered}", "\t".repeat(depth)));
        let name = display_name(&node);
        let below = Inherited {
            clip: match local_frame {
                Some(frame) if MARK_CLIP_ROLES.contains(&node.role.as_str()) => {
                    Some(clipped(frame, inherited.clip))
                }
                _ => inherited.clip,
            },
            context: name.clone().or_else(|| inherited.context.clone()),
        };
        self.records.insert(
            index,
            RenderedRecord {
                index,
                handle: handle.clone(),
                local_frame,
                actions: node.raw_actions.clone(),
                signature: element_signature(&node),
                role: node.role.clone(),
                name,
                placeholder: clean(node.placeholder.as_deref()),
                traits: node.traits.clone(),
                visible: local_frame.map(|frame| clipped(frame, inherited.clip)),
                context: inherited.context.clone(),
            },
        );
        if self.source.is_focused(handle) {
            self.focused_element_id = Some(index);
            self.focused_summary = Some(rendered);
        }
        if node.summary.is_some() || should_suppress_children(&node) {
            return;
        }
        if self.compact_browser_tabs {
            let child_nodes: Vec<RenderNode> = children
                .iter()
                .map(|child| self.source.peek(child))
                .collect();
            if let Some(compaction) = tab_strip_compaction(&node, &child_nodes) {
                for (child_index, child) in children.iter().enumerate() {
                    if compaction.retained_indexes.contains(&child_index) {
                        self.render(child, depth + 1, &lineage, &below);
                    }
                }
                self.lines.push(format!(
                    "{}... {} inactive browser tabs omitted",
                    "\t".repeat(depth + 1),
                    compaction.omitted_count
                ));
                return;
            }
        }
        let child_line_start = self.lines.len();
        for child in &children {
            self.render(child, depth + 1, &lineage, &below);
        }
        if self.compact_browser_tabs {
            self.compact_rendered_browser_tabs(&node, child_line_start, depth + 1);
        }
    }

    /// The second chance at a tab strip: when the strip was not recognisable
    /// as a container up front but ten or more direct children rendered as
    /// tabs, fold the inactive ones after the fact.
    fn compact_rendered_browser_tabs(
        &mut self,
        parent: &RenderNode,
        start_line: usize,
        depth: usize,
    ) {
        if role_text(parent) != "scroll area" {
            return;
        }
        let indent = "\t".repeat(depth);
        let tab_line_indexes: Vec<usize> = (start_line..self.lines.len())
            .filter(|line_index| {
                is_direct_rendered_browser_tab_line(&self.lines[*line_index], &indent)
            })
            .collect();
        if tab_line_indexes.len() < 10 {
            return;
        }
        let active: std::collections::BTreeSet<usize> = tab_line_indexes
            .iter()
            .copied()
            .filter(|line_index| is_active_rendered_browser_tab_line(&self.lines[*line_index]))
            .collect();
        if active.is_empty() {
            return;
        }
        let insertion_index = tab_line_indexes[0];
        let mut omitted = 0;
        for line_index in tab_line_indexes.iter().rev() {
            if active.contains(line_index) {
                continue;
            }
            if let Some(record_index) = rendered_element_index(&self.lines[*line_index], &indent) {
                self.records.remove(&record_index);
                if self.focused_element_id == Some(record_index) {
                    self.focused_element_id = None;
                    self.focused_summary = None;
                }
            }
            self.lines.remove(*line_index);
            omitted += 1;
        }
        if omitted == 0 {
            return;
        }
        self.lines.insert(
            insertion_index,
            format!("{indent}... {omitted} inactive browser tabs omitted"),
        );
    }
}

fn is_direct_rendered_browser_tab_line(line: &str, indent: &str) -> bool {
    let Some(text) = line.strip_prefix(indent) else {
        return false;
    };
    if text.starts_with('\t') {
        return false;
    }
    // `^\d+ tab($| \(|,)`
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return false;
    }
    let rest = &text[digits..];
    let Some(after_tab) = rest.strip_prefix(" tab") else {
        return false;
    };
    after_tab.is_empty() || after_tab.starts_with(" (") || after_tab.starts_with(',')
}

fn is_active_rendered_browser_tab_line(line: &str) -> bool {
    line.contains("(selected") || line.contains("Value: 1")
}

fn rendered_element_index(line: &str, indent: &str) -> Option<usize> {
    let text = line.strip_prefix(indent)?;
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(role: &str) -> RenderNode {
        RenderNode::with_role(role)
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn a_rectangle_less_another_is_what_still_shows_in_pieces_that_do_not_overlap() {
        let screen = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert_eq!(
            screen.minus(&Rect::new(200.0, 0.0, 10.0, 10.0)),
            vec![screen],
            "apart"
        );
        assert_eq!(
            screen.minus(&Rect::new(100.0, 0.0, 10.0, 10.0)),
            vec![screen],
            "touching an edge hides nothing"
        );
        assert!(
            screen
                .minus(&Rect::new(-5.0, -5.0, 200.0, 200.0))
                .is_empty(),
            "hidden whole"
        );
        let left = screen.minus(&Rect::new(20.0, 30.0, 40.0, 20.0));
        assert_eq!(
            left,
            vec![
                Rect::new(0.0, 0.0, 100.0, 30.0),
                Rect::new(0.0, 50.0, 100.0, 50.0),
                Rect::new(0.0, 30.0, 20.0, 20.0),
                Rect::new(60.0, 30.0, 40.0, 20.0),
            ],
            "a hole leaves four bands"
        );
        let shown: f64 = left.iter().map(Rect::area).sum();
        assert!((shown - (100.0 * 100.0 - 40.0 * 20.0)).abs() < 1e-9);
        assert_eq!(
            screen.minus(&Rect::new(0.0, 0.0, 100.0, 33.0)),
            vec![Rect::new(0.0, 33.0, 100.0, 67.0)],
            "a band across the top leaves the rest"
        );
    }

    // ---- SnapshotRenderingTests.swift, case for case ----

    #[test]
    fn skips_unsupported_advertised_attributes() {
        let advertised: std::collections::HashSet<String> =
            strings(&["AXRole", "AXChildren"]).into_iter().collect();
        assert!(supports_attribute("AXRole", Some(&advertised)));
        assert!(!supports_attribute("AXTitle", Some(&advertised)));
        assert!(supports_attribute("AXTitle", None));
    }

    #[test]
    fn secure_text_metadata_only_probes_text_like_roles() {
        assert!(should_probe_secure_text_metadata("AXTextField"));
        assert!(should_probe_secure_text_metadata("AXSearchField"));
        assert!(!should_probe_secure_text_metadata("AXGroup"));
        assert!(!should_probe_secure_text_metadata("AXButton"));
        assert!(looks_like_secure_text("AXTextField AXSecureTextField"));
        assert!(!looks_like_secure_text("AXTextField Address"));
    }

    #[test]
    fn elides_anonymous_wrappers_but_keeps_web_area_containers() {
        let mut wrapper = node("AXGroup");
        wrapper.child_count = 1;
        assert!(should_elide(&wrapper));
        let mut web = node("AXGroup");
        web.child_count = 3;
        web.web_area_depth = Some(1);
        assert!(!should_elide(&web));
    }

    #[test]
    fn renders_markdown_links_and_suppresses_their_children() {
        let mut link = node("AXLink");
        link.link_text = Some("Skip [main]".into());
        link.url = Some("https://example.com/path".into());
        assert_eq!(
            line(7, &link),
            "7 link [Skip \\[main\\]](https://example.com/path)"
        );
        assert!(should_suppress_children(&link));
    }

    #[test]
    fn suppresses_children_for_named_compact_controls() {
        let mut button = node("AXButton");
        button.role_description = Some("button".into());
        button.label = Some("Install GitHub".into());
        button.child_count = 1;
        let mut heading = node("AXHeading");
        heading.role_description = Some("heading".into());
        heading.label = Some("Repository navigation".into());
        heading.value = Some("2".into());
        heading.child_count = 1;
        assert!(should_suppress_children(&button));
        assert!(should_suppress_children(&heading));
        assert_eq!(line(5, &heading), "5 heading Repository navigation");
    }

    #[test]
    fn keeps_children_for_rows_with_nested_controls() {
        let mut row = node("AXRow");
        row.role_description = Some("row".into());
        row.child_count = 3;
        row.row_summary = Some("Liked Songs".into());
        assert!(!should_suppress_children(&row));
    }

    #[test]
    fn filters_noisy_actions_and_formats_secondary_actions() {
        let mut window = node("AXWindow");
        window.title = Some("Document".into());
        window.raw_actions =
            strings(&["AXPress", "AXShowMenu", "AXScrollToVisible", "AXZoomWindow"]);
        assert_eq!(
            meaningful_actions(&window.raw_actions, &window.role),
            strings(&["AXZoomWindow"])
        );
        assert_eq!(
            line(1, &window),
            "1 window Document, Secondary Actions: zoom the window"
        );
    }

    #[test]
    fn suppresses_horizontal_scroll_when_vertical_scroll_exists() {
        let mut area = node("AXScrollArea");
        area.raw_actions = strings(&[
            "AXScrollUpByPage",
            "AXScrollDownByPage",
            "AXScrollLeftByPage",
            "AXScrollRightByPage",
        ]);
        assert_eq!(
            meaningful_actions(&area.raw_actions, &area.role),
            strings(&["AXScrollUpByPage", "AXScrollDownByPage"])
        );
        // The Windows spelling of the same strip is filtered the same way.
        let windows = strings(&[
            "ScrollUpByPage",
            "ScrollDownByPage",
            "ScrollLeftByPage",
            "Invoke",
        ]);
        assert_eq!(
            meaningful_actions(&windows, "AXScrollArea"),
            strings(&["ScrollUpByPage", "ScrollDownByPage"])
        );
    }

    #[test]
    fn text_fields_keep_distinct_value_and_placeholder() {
        let mut field = node("AXTextField");
        field.role_description = Some("text field".into());
        field.label = Some("Address".into());
        field.value = Some("https://example.com".into());
        field.placeholder = Some("Search".into());
        assert_eq!(
            line(3, &field),
            "3 text field Address, Value: https://example.com, Placeholder: Search"
        );
    }

    #[test]
    fn static_text_uses_compact_value_formatting_and_rows_use_their_summary() {
        let mut text = node("AXStaticText");
        text.role_description = Some("text".into());
        text.value = Some("Home".into());
        assert_eq!(line(4, &text), "4 text Home");
        let mut row = node("AXRow");
        row.role_description = Some("row".into());
        row.row_summary = Some("General Settings Enabled".into());
        assert_eq!(line(9, &row), "9 row General Settings Enabled");
    }

    fn tab(index: usize, role: &str, description: &str, selected: bool) -> RenderNode {
        let mut child = node(role);
        child.role_description = Some(description.into());
        child.title = Some(format!("Tab {index}"));
        if selected {
            child.traits = strings(&["selected"]);
        }
        child
    }

    #[test]
    fn compacts_large_browser_tab_strips_to_the_selected_tab() {
        let mut parent = node("AXScrollArea");
        parent.role_description = Some("tab bar".into());
        let children: Vec<RenderNode> = (0..12)
            .map(|i| tab(i, "AXRadioButton", "tab", i == 7))
            .collect();
        let compaction = tab_strip_compaction(&parent, &children).unwrap();
        assert_eq!(
            compaction
                .retained_indexes
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![7]
        );
        assert_eq!(compaction.omitted_count, 11);

        let mut scroll = node("AXScrollArea");
        scroll.role_description = Some("scroll area".into());
        let children: Vec<RenderNode> = (0..12)
            .map(|i| tab(i, "AXRadioButton", "tab", i == 11))
            .collect();
        let compaction = tab_strip_compaction(&scroll, &children).unwrap();
        assert_eq!(
            compaction
                .retained_indexes
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![11]
        );
        assert_eq!(compaction.omitted_count, 11);
    }

    #[test]
    fn uses_one_value_as_selected_browser_tab_fallback() {
        let mut parent = node("AXScrollArea");
        parent.role_description = Some("scroll area".into());
        let children: Vec<RenderNode> = (0..12)
            .map(|i| {
                let mut child = tab(i, "AXRadioButton", "tab", false);
                child.value = Some(if i == 4 { "1" } else { "0" }.into());
                child
            })
            .collect();
        let compaction = tab_strip_compaction(&parent, &children).unwrap();
        assert_eq!(
            compaction
                .retained_indexes
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![4]
        );
        assert_eq!(compaction.omitted_count, 11);
    }

    #[test]
    fn keeps_tab_collections_expanded_when_nothing_marks_them_a_strip() {
        let parent = node("AXGroup");
        let children: Vec<RenderNode> = (0..12)
            .map(|i| {
                let mut child = node("AXTab");
                child.title = Some(format!("Tab {i}"));
                child.value = Some("0".into());
                child
            })
            .collect();
        assert!(tab_strip_compaction(&parent, &children).is_none());
        let children: Vec<RenderNode> = (0..12)
            .map(|i| tab(i, "AXRadioButton", "tab", i == 2))
            .collect();
        assert!(tab_strip_compaction(&parent, &children).is_none());
        let mut group = node("AXTabGroup");
        group.role_description = Some("tab group".into());
        let children: Vec<RenderNode> = (0..3)
            .map(|i| tab(i, "AXRadioButton", "tab", i == 1))
            .collect();
        assert!(tab_strip_compaction(&group, &children).is_none());
    }

    #[test]
    fn a_windows_tab_control_with_many_tab_items_compacts_like_a_browser_strip() {
        let mut group = node("AXTabGroup");
        group.role_description = Some("tab".into());
        let children: Vec<RenderNode> = (0..14)
            .map(|i| tab(i, "AXTab", "tab item", i == 0))
            .collect();
        let compaction = tab_strip_compaction(&group, &children).unwrap();
        assert_eq!(
            compaction
                .retained_indexes
                .iter()
                .copied()
                .collect::<Vec<_>>(),
            vec![0]
        );
        assert_eq!(compaction.omitted_count, 13);
    }

    #[test]
    fn pretty_actions_and_role_text_read_the_same_on_both_vocabularies() {
        assert_eq!(pretty_action("AXScrollDownByPage"), "scroll down");
        assert_eq!(pretty_action("ScrollDownByPage"), "scroll down");
        assert_eq!(pretty_action("Invoke"), "invoke");
        assert_eq!(pretty_action("AXZoomWindow"), "zoom the window");
        assert_eq!(role_text(&node("AXPopUpButton")), "pop up button");
        assert_eq!(role_text(&node("AXGroup")), "container");
        assert_eq!(role_text(&node("AXMenuBarItem")), "");
        let mut described = node("AXButton");
        described.role_description = Some("단추".into());
        assert_eq!(role_text(&described), "단추");
    }

    #[test]
    fn the_signature_proves_identity_not_state() {
        let mut field = node("AXTextField");
        field.label = Some("Address".into());
        field.value = Some("before".into());
        field.raw_actions = strings(&["AXPress", "AXShowMenu", "AXScrollToVisible"]);
        let before = element_signature(&field);
        field.value = Some("after".into());
        field.placeholder = Some("Search".into());
        assert_eq!(element_signature(&field), before);
        field.label = Some("Search".into());
        assert_ne!(element_signature(&field), before);
    }

    #[test]
    fn tree_text_carries_the_app_line_the_window_line_and_the_focus() {
        let app = AppIdentity {
            name: "Notes App".into(),
            bundle_id: None,
            pid: 9,
        };
        let text = render_tree_text(
            &app,
            "Un\ntitled",
            &strings(&["0 window Untitled"]),
            Some("0 window Untitled"),
        );
        assert_eq!(
            text,
            "App=Notes_App (pid 9)\nWindow: \"Un titled\", App: Notes App.\n\n0 window Untitled\n\nThe focused UI element is 0 window Untitled."
        );
        let bundled = AppIdentity {
            name: "Notes".into(),
            bundle_id: Some("com.apple.Notes".into()),
            pid: 9,
        };
        assert!(
            render_tree_text(&bundled, "t", &[], None)
                .ends_with("No UI element is currently focused.")
        );
        assert!(
            render_tree_text(&bundled, "t", &[], None).starts_with("App=com.apple.Notes (pid 9)")
        );
    }

    // ---- the walk, against a fake tree ----

    #[derive(Clone, Debug)]
    struct Fake {
        node: RenderNode,
        children: Vec<usize>,
        frame: Option<Rect>,
    }

    struct FakeTree {
        nodes: Vec<Fake>,
        focused: Option<usize>,
        describes: usize,
    }

    impl TreeSource for FakeTree {
        type Handle = usize;

        fn describe(&mut self, handle: &usize, _ancestors: &[usize]) -> Described<usize> {
            self.describes += 1;
            let fake = &self.nodes[*handle];
            let mut node = fake.node.clone();
            node.child_count = fake.children.len();
            Described {
                node,
                children: fake.children.clone(),
                local_frame: fake.frame,
            }
        }

        fn is_focused(&mut self, handle: &usize) -> bool {
            self.focused == Some(*handle)
        }
    }

    fn fake(role: &str, title: Option<&str>, children: Vec<usize>) -> Fake {
        let mut node = node(role);
        node.title = title.map(str::to_string);
        Fake {
            node,
            children,
            frame: Some(Rect::new(0.0, 0.0, 10.0, 10.0)),
        }
    }

    #[test]
    fn the_walk_indexes_visible_elements_elides_wrappers_and_finds_the_focus() {
        let mut tree = FakeTree {
            nodes: vec![
                fake("AXWindow", Some("Doc"), vec![1]),
                fake("AXGroup", None, vec![2, 3]),
                fake("AXButton", Some("OK"), vec![4]),
                fake("AXStaticText", None, vec![]),
                fake("AXStaticText", Some("inner"), vec![]),
            ],
            focused: Some(2),
            describes: 0,
        };
        tree.nodes[3].node.value = Some("Hello".into());
        // The platform describes static text as "text" (the AX role
        // description); the compact value form keys on that word.
        tree.nodes[3].node.role_description = Some("text".into());
        let rendered = walk(&mut tree, &0, false);
        assert_eq!(
            rendered.lines,
            strings(&["0 window Doc", "\t1 button OK", "\t2 text Hello"])
        );
        assert_eq!(rendered.focused_element_id, Some(1));
        assert_eq!(rendered.focused_summary.as_deref(), Some("1 button OK"));
        assert_eq!(rendered.records.len(), 3);
        let button = &rendered.records[&1];
        assert_eq!(
            (button.role.as_str(), button.name.as_deref()),
            ("AXButton", Some("OK")),
            "a record carries the face a mark is drawn from"
        );
        assert_eq!(rendered.records[&1].handle, 2);
        assert!(!rendered.truncated);
        // The named button's inner text was never described: suppressed
        // children are not visited.
        assert_eq!(tree.describes, 4);
    }

    #[test]
    fn the_walk_stops_at_the_node_cap_and_refuses_cycles() {
        let mut nodes = vec![fake(
            "AXWindow",
            Some("Big"),
            (1..=MAX_TREE_NODES + 5).collect(),
        )];
        for index in 1..=MAX_TREE_NODES + 5 {
            nodes.push(fake("AXButton", Some(&format!("b{index}")), vec![]));
        }
        let mut tree = FakeTree {
            nodes,
            focused: None,
            describes: 0,
        };
        let rendered = walk(&mut tree, &0, false);
        assert!(rendered.truncated);
        assert!(!rendered.max_depth_reached);
        assert_eq!(rendered.records.len(), MAX_TREE_NODES);

        let mut cyclic = FakeTree {
            nodes: vec![
                fake("AXWindow", Some("Loop"), vec![1]),
                fake("AXButton", Some("b"), vec![0]),
            ],
            focused: None,
            describes: 0,
        };
        let rendered = walk(&mut cyclic, &0, false);
        assert_eq!(rendered.records.len(), 2);
    }

    #[test]
    fn the_walk_marks_the_depth_cap() {
        let mut nodes = Vec::new();
        for depth in 0..MAX_TREE_DEPTH + 3 {
            nodes.push(fake("AXList", Some(&format!("d{depth}")), vec![depth + 1]));
        }
        nodes.push(fake("AXButton", Some("leaf"), vec![]));
        let mut tree = FakeTree {
            nodes,
            focused: None,
            describes: 0,
        };
        let rendered = walk(&mut tree, &0, false);
        assert!(rendered.truncated);
        assert!(rendered.max_depth_reached);
        assert_eq!(rendered.records.len(), MAX_TREE_DEPTH);
    }

    #[test]
    fn the_walk_compacts_a_browser_tab_strip_only_for_browsers() {
        let mut nodes = vec![fake("AXWindow", Some("Chrome"), vec![1])];
        let mut strip = fake("AXScrollArea", None, (2..=13).collect());
        strip.node.role_description = Some("tab bar".into());
        nodes.push(strip);
        for index in 0..12 {
            let mut item = fake("AXRadioButton", Some(&format!("Tab {index}")), vec![]);
            item.node.role_description = Some("tab".into());
            if index == 3 {
                item.node.traits = strings(&["selected"]);
            }
            nodes.push(item);
        }
        let mut tree = FakeTree {
            nodes: nodes.clone(),
            focused: None,
            describes: 0,
        };
        let rendered = walk(&mut tree, &0, true);
        assert_eq!(
            rendered.lines,
            strings(&[
                "0 window Chrome",
                "\t1 tab bar",
                "\t\t2 tab (selected) Tab 3",
                "\t\t... 11 inactive browser tabs omitted",
            ])
        );
        assert_eq!(rendered.records.len(), 3);
        let mut plain = FakeTree {
            nodes,
            focused: None,
            describes: 0,
        };
        assert_eq!(walk(&mut plain, &0, false).records.len(), 14);
    }

    #[test]
    fn the_walk_folds_rendered_tabs_after_the_fact_inside_a_scroll_area() {
        // The tabs sit under an anonymous wrapper the walk elides, so the
        // scroll area's direct CHILD is not tab-like and the up-front
        // compaction passes — but the rendered lines one level down are
        // twelve nameless "N tab, Value: …" lines (the helper's regex
        // `^\d+ tab($| \(|,)`), which is what the second look folds.
        let mut nodes = vec![fake("AXWindow", Some("Safari"), vec![1])];
        let mut strip = fake("AXScrollArea", None, vec![2]);
        strip.node.role_description = Some("scroll area".into());
        nodes.push(strip);
        nodes.push(fake("AXGroup", None, (3..=14).collect()));
        for index in 0..12 {
            let mut item = fake("AXTab", None, vec![]);
            item.node.value = Some(if index == 5 { "1" } else { "0" }.into());
            nodes.push(item);
        }
        let mut tree = FakeTree {
            nodes,
            focused: Some(5),
            describes: 0,
        };
        let rendered = walk(&mut tree, &0, true);
        assert_eq!(
            rendered.lines,
            strings(&[
                "0 window Safari",
                "\t1 scroll area",
                "\t\t... 11 inactive browser tabs omitted",
                "\t\t7 tab, Value: 1",
            ])
        );
        assert_eq!(rendered.records.len(), 3);
        assert_eq!(
            rendered.focused_element_id, None,
            "the folded tab's focus is dropped"
        );
    }

    #[test]
    fn rects_meet_and_match_like_cgrect() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);
        let b = Rect::new(50.0, 50.0, 100.0, 100.0);
        assert!(a.intersects(&b));
        assert_eq!(a.intersection(&b), Some(Rect::new(50.0, 50.0, 50.0, 50.0)));
        assert!(a.intersection(&Rect::new(200.0, 0.0, 10.0, 10.0)).is_none());
        assert!(a.matches_within(&Rect::new(1.0, -1.0, 101.0, 99.0), 2.0));
        assert!(!a.matches_within(&Rect::new(3.0, 0.0, 100.0, 100.0), 2.0));
        assert!(a.mostly_covers(&Rect::new(10.0, 10.0, 80.0, 80.0)));
        assert!(!a.mostly_covers(&b));
        assert_eq!(a.mid_x(), 50.0);
    }
}
