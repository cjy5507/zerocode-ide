//! Versioned, bounded live-reflex plan. Decoding is deliberately separate from
//! validation: a wire value has no authority to post input.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const VERSION: u32 = 1;
pub const HEADING_REFLEX: &str = "## Reflex";
pub const HEADING_PERCEPTION: &str = "## Perception";
pub const HEADING_RULES: &str = "## Rules";

/// The only source of plan limits for the core and its CLI projection. Swift
/// checks the same values against the shared golden contract before use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReflexLimits {
    pub max_detectors: u64,
    pub max_rules: u64,
    pub max_macros: u64,
    pub max_actions: u64,
    pub max_expanded_actions: u64,
    pub max_roi_pixels: u64,
    pub max_work_pixels: u64,
    pub max_predicate_depth: u64,
    pub max_macro_depth: u64,
    pub max_cooldown_ms: u64,
    pub max_lease_ns: u64,
    pub max_scale_part: u64,
    pub max_pointer_duration_ms: u64,
    pub instant_duration_ms: u64,
    pub max_frame_age_ns: u64,
}

pub const LIMITS: ReflexLimits = ReflexLimits {
    max_detectors: 16,
    max_rules: 32,
    max_macros: 32,
    max_actions: 128,
    max_expanded_actions: 256,
    max_roi_pixels: 1_048_576,
    max_work_pixels: 4_194_304,
    max_predicate_depth: 8,
    max_macro_depth: 8,
    max_cooldown_ms: 60_000,
    max_lease_ns: 1_000_000_000,
    max_scale_part: 8,
    max_pointer_duration_ms: 1_000,
    instant_duration_ms: 1,
    max_frame_age_ns: 50_000_000,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub surface: Surface,
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    MacosDesktop,
    IosDevice,
    WindowsDesktop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinateSpace {
    Pixel,
    Point,
    Css,
    Device,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Roi {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub space: CoordinateSpace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scale {
    pub numerator: u64,
    pub denominator: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectorKind {
    Color,
    Template,
    Motion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Detector {
    pub id: String,
    pub kind: DetectorKind,
    pub roi: Roi,
    pub patches: u64,
    pub scale: Scale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Predicate {
    Known,
    Eq { value: i64 },
    Not { child: Box<Predicate> },
    Any { children: Vec<Predicate> },
    All { children: Vec<Predicate> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    True,
    False,
    Unknown,
}

impl Predicate {
    #[must_use]
    pub fn evaluate(&self, value: Option<i64>) -> Truth {
        match self {
            Self::Known => {
                if value.is_some() {
                    Truth::True
                } else {
                    Truth::Unknown
                }
            }
            Self::Eq { value: expected } => value.map_or(Truth::Unknown, |actual| {
                if actual == *expected {
                    Truth::True
                } else {
                    Truth::False
                }
            }),
            Self::Not { child } => match child.evaluate(value) {
                Truth::True => Truth::False,
                Truth::False => Truth::True,
                Truth::Unknown => Truth::Unknown,
            },
            Self::Any { children } => {
                let mut unknown = false;
                for child in children {
                    match child.evaluate(value) {
                        Truth::True => return Truth::True,
                        Truth::Unknown => unknown = true,
                        Truth::False => {}
                    }
                }
                if unknown {
                    Truth::Unknown
                } else {
                    Truth::False
                }
            }
            Self::All { children } => {
                let mut unknown = false;
                for child in children {
                    match child.evaluate(value) {
                        Truth::False => return Truth::False,
                        Truth::Unknown => unknown = true,
                        Truth::True => {}
                    }
                }
                if unknown { Truth::Unknown } else { Truth::True }
            }
        }
    }

    fn valid(&self, depth: u64) -> Result<(), &'static str> {
        if depth > LIMITS.max_predicate_depth {
            return Err("predicate_depth");
        }
        match self {
            Self::Any { children } | Self::All { children } => {
                if children.is_empty() || children.len() > LIMITS.max_actions as usize {
                    return Err("predicate_children");
                }
                for child in children {
                    child.valid(depth + 1)?;
                }
            }
            Self::Not { child } => child.valid(depth + 1)?,
            Self::Known | Self::Eq { .. } => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub detector: String,
    pub predicate: Predicate,
    pub macro_id: String,
    pub priority: i64,
    pub cooldown_ms: u64,
    pub max_fires: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Move,
    Click,
    Key,
    Macro,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub id: String,
    pub kind: ActionKind,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Macro {
    pub id: String,
    pub repeat: u64,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerCurve {
    Linear,
    Cosine,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointerStyle {
    pub duration_ms: u64,
    pub curve: PointerCurve,
    pub instant: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReflexPlan {
    pub version: u32,
    pub plan_hash: String,
    pub scope: Scope,
    pub detectors: Vec<Detector>,
    pub rules: Vec<Rule>,
    pub macros: Vec<Macro>,
    pub pointer: PointerStyle,
}

/// Successful JSON decoding alone does not confer execution permission.
#[derive(Debug, Clone)]
pub struct ValidatedPlan(ReflexPlan);

impl ValidatedPlan {
    #[must_use]
    pub fn plan(&self) -> &ReflexPlan {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReflexError {
    Wire,
    Version,
    Hash,
    Scope,
    Id,
    Duplicate,
    Reference,
    Cycle,
    Budget,
    Unsupported,
}

fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Canonical JSON has lexicographically sorted object keys and integer-only
/// fields. The hash covers every field except its own 64-character digest.
pub fn plan_hash(plan: &ReflexPlan) -> String {
    let mut without = plan.clone();
    without.plan_hash.clear();
    format!("{:x}", Sha256::digest(wire_bytes(&without)))
}

/// Canonical JSON: compact, integer-only, every object's keys in byte order
/// at every depth and arrays in their own order. A `serde_json::Map` promises
/// no order — Cargo turns serde_json's `preserve_order` on for every crate in a
/// build that includes one asking for it (the shell, hookd), and the map then
/// keeps insertion order — so the keys are sorted here, and the hash, the wire
/// and the receiver's canonical check all read this one form. Swift writes the
/// same bytes with `.sortedKeys`.
fn canonical_json(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&Canonical(value)).expect("a JSON value serializes")
}

struct Canonical<'a>(&'a serde_json::Value);

impl Serialize for Canonical<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            serde_json::Value::Object(map) => {
                let mut entries: Vec<_> = map.iter().collect();
                entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
                serializer.collect_map(
                    entries
                        .into_iter()
                        .map(|(key, item)| (key, Canonical(item))),
                )
            }
            serde_json::Value::Array(items) => serializer.collect_seq(items.iter().map(Canonical)),
            leaf => leaf.serialize(serializer),
        }
    }
}

/// Stable UTF-8 wire encoding consumed by the Swift helper. This sorted-key
/// JSON form preserves every integer exactly and rejects unknown fields when
/// decoded back into `ReflexPlan`.
pub fn wire_bytes(plan: &ReflexPlan) -> Vec<u8> {
    canonical_json(&serde_json::to_value(plan).expect("typed plan serializes"))
}

/// The receiver accepts only the canonical integer-only wire. Duplicate
/// keys, non-finite numbers, negative zero, unknown fields and trailing
/// tokens cannot be normalized into a different executable plan.
pub fn decode_wire(bytes: &[u8]) -> Result<ValidatedPlan, ReflexError> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| ReflexError::Wire)?;
    if canonical_json(&value) != bytes {
        return Err(ReflexError::Wire);
    }
    let plan: ReflexPlan = serde_json::from_slice(bytes).map_err(|_| ReflexError::Wire)?;
    validate(plan)
}

pub fn validate(plan: ReflexPlan) -> Result<ValidatedPlan, ReflexError> {
    if plan.version != VERSION {
        return Err(ReflexError::Version);
    }
    if plan_hash(&plan) != plan.plan_hash {
        return Err(ReflexError::Hash);
    }
    if plan.scope.target.is_empty()
        || plan.scope.target.len() > 128
        || !plan
            .scope
            .target
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
    {
        return Err(ReflexError::Scope);
    }
    if plan.detectors.is_empty()
        || plan.rules.is_empty()
        || plan.macros.is_empty()
        || plan.detectors.len() > LIMITS.max_detectors as usize
        || plan.rules.len() > LIMITS.max_rules as usize
        || plan.macros.len() > LIMITS.max_macros as usize
    {
        return Err(ReflexError::Budget);
    }
    if plan.pointer.duration_ms == 0
        || plan.pointer.duration_ms > LIMITS.max_pointer_duration_ms
        || (plan.pointer.instant && plan.pointer.duration_ms != LIMITS.instant_duration_ms)
    {
        return Err(ReflexError::Budget);
    }
    let mut detector_ids = BTreeSet::new();
    let mut work = 0_u64;
    for detector in &plan.detectors {
        if !identifier(&detector.id) {
            return Err(ReflexError::Id);
        }
        if !detector_ids.insert(detector.id.as_str()) {
            return Err(ReflexError::Duplicate);
        }
        if !matches!(detector.kind, DetectorKind::Color) {
            return Err(ReflexError::Unsupported);
        }
        if detector.roi.space != CoordinateSpace::Pixel
            || detector.roi.x < 0
            || detector.roi.y < 0
            || detector.roi.width <= 0
            || detector.roi.height <= 0
            || detector.patches == 0
            || detector.scale.denominator == 0
            || detector.scale.numerator == 0
            || detector.scale.denominator > LIMITS.max_scale_part
            || detector.scale.numerator > LIMITS.max_scale_part
        {
            return Err(ReflexError::Budget);
        }
        let pixels = u64::try_from(detector.roi.width)
            .ok()
            .and_then(|width| {
                u64::try_from(detector.roi.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or(ReflexError::Budget)?;
        if pixels > LIMITS.max_roi_pixels {
            return Err(ReflexError::Budget);
        }
        let numerator = pixels
            .checked_mul(detector.patches)
            .and_then(|n| n.checked_mul(detector.scale.numerator))
            .ok_or(ReflexError::Budget)?;
        let scaled = numerator
            .checked_add(detector.scale.denominator - 1)
            .ok_or(ReflexError::Budget)?
            / detector.scale.denominator;
        work = work.checked_add(scaled).ok_or(ReflexError::Budget)?;
        if work > LIMITS.max_work_pixels {
            return Err(ReflexError::Budget);
        }
    }
    let mut macro_ids = BTreeSet::new();
    let mut action_ids = BTreeSet::new();
    let mut actions = 0_u64;
    for item in &plan.macros {
        if !identifier(&item.id) {
            return Err(ReflexError::Id);
        }
        if !macro_ids.insert(item.id.as_str()) {
            return Err(ReflexError::Duplicate);
        }
        if item.repeat == 0 || item.actions.is_empty() {
            return Err(ReflexError::Budget);
        }
        actions = actions
            .checked_add(item.actions.len() as u64)
            .ok_or(ReflexError::Budget)?;
        if actions > LIMITS.max_actions {
            return Err(ReflexError::Budget);
        }
        for action in &item.actions {
            if !identifier(&action.id) || !identifier(&action.target) {
                return Err(ReflexError::Id);
            }
            if !action_ids.insert(action.id.as_str()) {
                return Err(ReflexError::Duplicate);
            }
            if matches!(action.kind, ActionKind::Move | ActionKind::Click)
                && !detector_ids.contains(action.target.as_str())
            {
                return Err(ReflexError::Reference);
            }
            if matches!(action.kind, ActionKind::Key) {
                return Err(ReflexError::Unsupported);
            }
        }
    }
    let by_id: BTreeMap<&str, &Macro> = plan
        .macros
        .iter()
        .map(|item| (item.id.as_str(), item))
        .collect();
    fn expanded(
        id: &str,
        by_id: &BTreeMap<&str, &Macro>,
        visiting: &mut BTreeSet<String>,
        depth: u64,
    ) -> Result<u64, ReflexError> {
        if depth > LIMITS.max_macro_depth {
            return Err(ReflexError::Budget);
        }
        if !visiting.insert(id.to_string()) {
            return Err(ReflexError::Cycle);
        }
        let item = by_id.get(id).ok_or(ReflexError::Reference)?;
        let mut count = 0_u64;
        for action in &item.actions {
            let next = if matches!(action.kind, ActionKind::Macro) {
                expanded(&action.target, by_id, visiting, depth + 1)?
            } else {
                1
            };
            count = count.checked_add(next).ok_or(ReflexError::Budget)?;
        }
        visiting.remove(id);
        let total = count.checked_mul(item.repeat).ok_or(ReflexError::Budget)?;
        if total > LIMITS.max_expanded_actions {
            return Err(ReflexError::Budget);
        }
        Ok(total)
    }
    // Every declaration must be a bounded, referentially valid DAG, even if
    // no rule currently reaches it. Rule max_fires is accounted for below.
    for item in &plan.macros {
        expanded(&item.id, &by_id, &mut BTreeSet::new(), 0)?;
    }
    let mut rule_ids = BTreeSet::new();
    let mut total = 0_u64;
    for rule in &plan.rules {
        if !identifier(&rule.id) {
            return Err(ReflexError::Id);
        }
        if !rule_ids.insert(rule.id.as_str()) {
            return Err(ReflexError::Duplicate);
        }
        if !detector_ids.contains(rule.detector.as_str()) {
            return Err(ReflexError::Reference);
        }
        rule.predicate.valid(0).map_err(|_| ReflexError::Budget)?;
        if rule.max_fires == 0 || rule.cooldown_ms > LIMITS.max_cooldown_ms {
            return Err(ReflexError::Budget);
        }
        let per_fire = expanded(&rule.macro_id, &by_id, &mut BTreeSet::new(), 0)?;
        total = total
            .checked_add(
                per_fire
                    .checked_mul(rule.max_fires)
                    .ok_or(ReflexError::Budget)?,
            )
            .ok_or(ReflexError::Budget)?;
        if total > LIMITS.max_expanded_actions {
            return Err(ReflexError::Budget);
        }
    }
    Ok(ValidatedPlan(plan))
}

/// R1 publishes schema support; no platform has a wired live provider yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReflexCapability {
    pub schema_version: u32,
    pub live_reflex: bool,
}

#[must_use]
pub const fn capability(_surface: Surface) -> ReflexCapability {
    ReflexCapability {
        schema_version: VERSION,
        live_reflex: false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PixelExtent {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointTransform {
    pub origin_x: i64,
    pub origin_y: i64,
    pub points_per_pixel: Scale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Up,
    Right,
    Down,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSpace {
    Srgb,
    DisplayP3,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameStatus {
    Ready,
    Stale,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameFacts {
    pub run_id: String,
    pub display_id: String,
    pub region: Roi,
    pub pixel_extent: PixelExtent,
    pub point_transform: PointTransform,
    pub orientation: Orientation,
    pub color_space: ColorSpace,
    pub status: FrameStatus,
    pub dirty: bool,
    pub capture_gap: u64,
    pub delivered_host_ns: Option<u64>,
    pub capture_seq: u64,
    pub repaint_seq: u64,
    pub stream_epoch: u64,
    pub owner_epoch: u64,
    pub geometry_epoch: u64,
    pub plan_epoch: u64,
    pub clock_domain: u64,
    pub captured_host_ns: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseInput {
    PointerMove,
    LeftClick,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionLease {
    pub run_id: String,
    pub action_id: String,
    pub target_id: String,
    pub target_roi: Roi,
    pub allowed_inputs: BTreeSet<LeaseInput>,
    pub owner_epoch: u64,
    pub stream_epoch: u64,
    pub geometry_epoch: u64,
    pub plan_epoch: u64,
    pub clock_domain: u64,
    pub source_capture_seq: u64,
    pub issued_host_ns: u64,
    pub valid_until_host_ns: u64,
    pub target_proof_until_host_ns: u64,
    pub max_children: u64,
    pub used_children: u64,
}

impl ActionLease {
    /// The frame must be ready and nonempty, share the lease's nonempty run
    /// and its epochs, and be a newer capture than `source_capture_seq`: the
    /// capture that issued the lease never satisfies it. An unknown capture
    /// time is never replaced by the delivery time. Expiry and proof ends are
    /// exclusive; the frame age and lease length limits in `LIMITS` are
    /// inclusive. Swift checks the same contract, pinned for both by the
    /// shared `fixtures/reflex-contract/lease_cases.json`.
    #[must_use]
    pub fn permits(&self, frame: &FrameFacts, now_host_ns: u64, input: LeaseInput) -> bool {
        !self.run_id.is_empty()
            && self.run_id == frame.run_id
            && self.allowed_inputs.contains(&input)
            && frame.status == FrameStatus::Ready
            && frame.pixel_extent.width > 0
            && frame.pixel_extent.height > 0
            && frame.captured_host_ns.is_some_and(|captured| {
                captured <= now_host_ns && now_host_ns - captured <= LIMITS.max_frame_age_ns
            })
            && self.issued_host_ns <= now_host_ns
            && self.valid_until_host_ns > self.issued_host_ns
            && self.valid_until_host_ns - self.issued_host_ns <= LIMITS.max_lease_ns
            && self.target_proof_until_host_ns <= self.valid_until_host_ns
            && !self.target_id.is_empty()
            && self.target_roi.width > 0
            && self.target_roi.height > 0
            && self.owner_epoch == frame.owner_epoch
            && self.stream_epoch == frame.stream_epoch
            && self.geometry_epoch == frame.geometry_epoch
            && self.plan_epoch == frame.plan_epoch
            && self.clock_domain == frame.clock_domain
            && frame.capture_seq > self.source_capture_seq
            && now_host_ns < self.valid_until_host_ns
            && now_host_ns < self.target_proof_until_host_ns
            && self.used_children < self.max_children
            && self.max_children <= LIMITS.max_expanded_actions
    }
}

fn entries<'a>(
    heading: &str,
    lines: &'a [&str],
    keys: &[&str],
) -> Result<Vec<(&'a str, &'a str)>, String> {
    let mut out = Vec::new();
    for line in lines.iter().copied().filter(|line| !line.is_empty()) {
        let (key, value) = line
            .strip_prefix("- ")
            .and_then(|line| line.split_once(": "))
            .ok_or_else(|| format!("{heading}: invalid line `{line}`"))?;
        if !keys.contains(&key) || value.is_empty() {
            return Err(format!("{heading}: invalid key or value `{line}`"));
        }
        out.push((key, value));
    }
    Ok(out)
}

fn singleton<'a>(entries: &'a [(&str, &'a str)], key: &str) -> Result<&'a str, String> {
    let mut found = entries
        .iter()
        .filter(|(name, _)| *name == key)
        .map(|(_, value)| *value);
    let value = found.next().ok_or_else(|| format!("missing `{key}`"))?;
    if found.next().is_some() {
        return Err(format!("duplicate `{key}`"));
    }
    Ok(value)
}

/// Three sections are all present or all absent. Empty sections and unknown
/// members of the Reflex namespace are errors, not a legacy Flow fallback.
pub fn read_sections(text: &str) -> Result<Option<ValidatedPlan>, String> {
    let headings = [HEADING_REFLEX, HEADING_PERCEPTION, HEADING_RULES];
    let mut counts = [0_u8; 3];
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("## "))
    {
        if let Some(at) = headings.iter().position(|heading| *heading == line) {
            counts[at] = counts[at]
                .checked_add(1)
                .ok_or("duplicate reflex section")?;
        } else if line.starts_with("## Reflex")
            || line.starts_with("## Perception")
            || line.starts_with("## Rules")
        {
            return Err(format!("unsupported reflex section `{line}`"));
        }
    }
    if counts == [0; 3] {
        return Ok(None);
    }
    if counts != [1; 3] {
        return Err(format!("reflex needs each of {headings:?} once"));
    }
    let get = |heading| crate::computer_recipe::section(text, heading).expect("counted section");
    let header_lines = get(HEADING_REFLEX);
    let header = entries(
        HEADING_REFLEX,
        &header_lines,
        &["version", "plan-hash", "scope", "pointer"],
    )?;
    if header.len() != 4 {
        return Err("reflex header needs four unique keys".to_string());
    }
    let version = singleton(&header, "version")?
        .parse()
        .map_err(|_| "invalid version")?;
    let plan_hash = singleton(&header, "plan-hash")?.to_string();
    let scope = serde_json::from_str(singleton(&header, "scope")?)
        .map_err(|err| format!("scope: {err}"))?;
    let pointer = serde_json::from_str(singleton(&header, "pointer")?)
        .map_err(|err| format!("pointer: {err}"))?;
    let perception_lines = get(HEADING_PERCEPTION);
    let perception = entries(HEADING_PERCEPTION, &perception_lines, &["detector"])?;
    let detectors = perception
        .into_iter()
        .map(|(_, value)| serde_json::from_str(value).map_err(|err| format!("detector: {err}")))
        .collect::<Result<Vec<_>, _>>()?;
    let rules_lines = get(HEADING_RULES);
    let rules_entries = entries(HEADING_RULES, &rules_lines, &["rule", "macro"])?;
    let rules = rules_entries
        .iter()
        .filter(|(key, _)| *key == "rule")
        .map(|(_, value)| serde_json::from_str(value).map_err(|err| format!("rule: {err}")))
        .collect::<Result<Vec<_>, _>>()?;
    let macros = rules_entries
        .iter()
        .filter(|(key, _)| *key == "macro")
        .map(|(_, value)| serde_json::from_str(value).map_err(|err| format!("macro: {err}")))
        .collect::<Result<Vec<_>, _>>()?;
    let plan = ReflexPlan {
        version,
        plan_hash,
        scope,
        detectors,
        rules,
        macros,
        pointer,
    };
    let validated = validate(plan).map_err(|err| format!("reflex validation: {err:?}"))?;
    Ok(Some(validated))
}

impl ReflexPlan {
    #[must_use]
    pub fn written_sections(&self) -> String {
        let mut out = format!(
            "\n{HEADING_REFLEX}\n\n- version: {}\n- plan-hash: {}\n- scope: {}\n- pointer: {}\n\n{HEADING_PERCEPTION}\n\n",
            self.version,
            self.plan_hash,
            serde_json::to_string(&self.scope).expect("scope"),
            serde_json::to_string(&self.pointer).expect("pointer")
        );
        for detector in &self.detectors {
            out.push_str(&format!(
                "- detector: {}\n",
                serde_json::to_string(detector).expect("detector")
            ));
        }
        out.push_str(&format!("\n{HEADING_RULES}\n\n"));
        for rule in &self.rules {
            out.push_str(&format!(
                "- rule: {}\n",
                serde_json::to_string(rule).expect("rule")
            ));
        }
        for item in &self.macros {
            out.push_str(&format!(
                "- macro: {}\n",
                serde_json::to_string(item).expect("macro")
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests;

/// One cursor belongs to one run: a frame from another run is refused, so a
/// new run needs a new cursor. Stream epochs never go back; within an epoch
/// the capture sequence advances and the repaint sequence does not regress,
/// and a later epoch starts a new sequence. Only ready, nonempty frames with
/// a known capture time are observed. The cursor has no clock and grants no
/// input: `ActionLease::permits` checks capture age and expiry at action time.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FrameCursor {
    run_id: Option<String>,
    stream_epoch: Option<u64>,
    capture_seq: u64,
    repaint_seq: u64,
}

impl FrameCursor {
    pub fn observe(&mut self, frame: &FrameFacts) -> bool {
        if frame.run_id.is_empty()
            || self.run_id.as_ref().is_some_and(|run| run != &frame.run_id)
            || frame.status != FrameStatus::Ready
            || frame.pixel_extent.width == 0
            || frame.pixel_extent.height == 0
            || frame.captured_host_ns.is_none()
            || frame.capture_seq == 0
            || self
                .stream_epoch
                .is_some_and(|epoch| frame.stream_epoch < epoch)
        {
            return false;
        }
        if self.stream_epoch == Some(frame.stream_epoch)
            && (frame.capture_seq <= self.capture_seq || frame.repaint_seq < self.repaint_seq)
        {
            return false;
        }
        self.run_id = Some(frame.run_id.clone());
        self.stream_epoch = Some(frame.stream_epoch);
        self.capture_seq = frame.capture_seq;
        self.repaint_seq = frame.repaint_seq;
        true
    }
}
