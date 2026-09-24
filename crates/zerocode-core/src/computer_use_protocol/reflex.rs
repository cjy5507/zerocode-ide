//! Versioned, bounded live-reflex plan. Decoding is deliberately separate from
//! validation: a wire value has no authority to post input.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::game_state::{self, ColorSpec, Layout};

/// 2: a colour detector carries its [`ColorSpec`] inside the plan and its
/// hash (t-6765/t-6768). A version-1 plan is refused, never read as a colour
/// detector with nothing to read the ROI with.
pub const VERSION: u32 = 2;
/// The clock a macOS frame, lease and observation are stamped in: host
/// uptime nanoseconds (`mach_absolute_time`), the clock ScreenCaptureKit
/// reports a frame's display time in.
pub const HOST_UPTIME_CLOCK: u64 = 1;
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
    /// The rate a run asks its display stream for: the realtime v1 request,
    /// whose latest frame measured 17.4 ms old at p95 against 33 ms at 30
    /// (t-6723, 800 × 500 fixture) — a request, not a delivered rate.
    pub frames_per_second: u64,
    /// One glide waypoint per this many nanoseconds: today's 8 ms step
    /// (`mouseMove --steps`, the desktop drag), the baseline R4 calibrates
    /// against, not a tuned value.
    pub pointer_tick_ns: u64,
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
    frames_per_second: 60,
    pointer_tick_ns: 8_000_000,
};

// The longest glide plus a press and its release fits one lease's children.
const _: () = assert!(
    LIMITS
        .max_pointer_duration_ms
        .saturating_mul(1_000_000)
        .div_ceil(LIMITS.pointer_tick_ns)
        .saturating_add(2)
        <= LIMITS.max_expanded_actions
);

/// The reflex table as the window sends it to the helper with a run's start:
/// canonical JSON, the only form the helper reads (`ReflexContract.decodeLimits`).
#[must_use]
pub fn limits_wire() -> Vec<u8> {
    canonical_json(&serde_json::to_value(LIMITS).expect("integer limits serialize"))
}

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
    /// What a colour detector reads its ROI with; required for `color`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<ColorSpec>,
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
    /// A colour detector without its spec, or a spec
    /// `game_state::validate_color` refuses.
    Perception,
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
        // A colour detector's structure is its spec's layout: one patch at
        // the scale it was written at, and the spec's own samples bound.
        let Some(color) = &detector.color else {
            return Err(ReflexError::Perception);
        };
        if detector.patches != 1 || detector.scale.numerator != 1 || detector.scale.denominator != 1
        {
            return Err(ReflexError::Unsupported);
        }
        game_state::validate_color(color, &detector.roi, &game_state::LIMITS)
            .map_err(|_| ReflexError::Perception)?;
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

/// Why a detector's value is unknown on a frame. Unknown never authorizes
/// input, and `not(unknown)` is unknown too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unknown {
    /// The frame cannot be read: not ready, no capture time, not BGRA, its
    /// buffer not the extent it claims.
    Frame,
    ColorSpace,
    /// The ROI has no place in the frame (`game_state::frame_roi`).
    Extent,
    Orientation,
    /// The tick's samples or time ran out before this detector.
    Budget,
    Ambiguous,
    Occluded,
    Scene,
    /// Not yet the spec's `confirm` fresh captures in agreement.
    Unconfirmed,
}

/// The frame an observation was made on, as the runtime handed it over. The
/// runtime observes only frames its [`FrameCursor`] accepted, so the capture
/// time is known here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameRef {
    pub run_id: String,
    pub stream_epoch: u64,
    pub capture_seq: u64,
    pub repaint_seq: u64,
    pub geometry_epoch: u64,
    pub plan_epoch: u64,
    pub owner_epoch: u64,
    pub clock_domain: u64,
    pub captured_host_ns: u64,
}

impl FrameRef {
    /// The reference to `frame`, or None when its capture time is unknown.
    #[must_use]
    pub fn of(frame: &FrameFacts) -> Option<Self> {
        Some(Self {
            run_id: frame.run_id.clone(),
            stream_epoch: frame.stream_epoch,
            capture_seq: frame.capture_seq,
            repaint_seq: frame.repaint_seq,
            geometry_epoch: frame.geometry_epoch,
            plan_epoch: frame.plan_epoch,
            owner_epoch: frame.owner_epoch,
            clock_domain: frame.clock_domain,
            captured_host_ns: frame.captured_host_ns?,
        })
    }
}

/// What a sighting points at: a track that keeps its number while the
/// target moves and changes it on another scene, its hitbox in frame pixels
/// (the spatial proof), the point to aim at, how fast it moves (pixels a
/// second) and how far the kernel is unsure of the point (pixels).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub track_id: u64,
    pub roi: Roi,
    pub point_x: i64,
    pub point_y: i64,
    pub velocity_x: i64,
    pub velocity_y: i64,
    pub uncertainty: u64,
}

impl Target {
    /// Where to press at `at_host_ns`: the point carried forward by the
    /// velocity since `captured_host_ns`, while the square of the
    /// uncertainty around it stays inside the hitbox carried the same way.
    /// None — do not press — when time runs backwards or past `max_age_ns`,
    /// a carry overflows, or the square leaves the hitbox.
    #[must_use]
    pub fn aim(
        &self,
        at_host_ns: u64,
        captured_host_ns: u64,
        max_age_ns: u64,
    ) -> Option<(i64, i64)> {
        let elapsed = at_host_ns.checked_sub(captured_host_ns)?;
        if elapsed > max_age_ns {
            return None;
        }
        let elapsed = i64::try_from(elapsed).ok()?;
        let carry = |velocity: i64| {
            velocity
                .checked_mul(elapsed)
                .map(|moved| moved / 1_000_000_000)
        };
        let (dx, dy) = (carry(self.velocity_x)?, carry(self.velocity_y)?);
        let (x, y) = (self.point_x.checked_add(dx)?, self.point_y.checked_add(dy)?);
        let reach = i64::try_from(self.uncertainty).ok()?;
        let (left, top) = (self.roi.x.checked_add(dx)?, self.roi.y.checked_add(dy)?);
        let (right, bottom) = (
            left.checked_add(self.roi.width)?,
            top.checked_add(self.roi.height)?,
        );
        (x.checked_sub(reach)? >= left
            && x.checked_add(reach)? < right
            && y.checked_sub(reach)? >= top
            && y.checked_add(reach)? < bottom)
            .then_some((x, y))
    }
}

/// One cell of a cells layout: its class numbered from 1 in palette order,
/// or 0 with the reason it is unknown, and the share its winner holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    pub class: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown: Option<Unknown>,
    pub share_permille: u64,
}

/// A detector's reading of one frame, as the kernel answers it (R5): data,
/// never permission. The runtime reads it only when [`Observation::admissible`]
/// and issues any lease itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub detector_id: String,
    pub frame: FrameRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown: Option<Unknown>,
    /// The predicate's input: known exactly when `unknown` is absent; zero
    /// means nothing is there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
    /// The frame's scale over the spec's reference extent.
    pub scale: Scale,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cells: Option<Vec<Cell>>,
    /// Samples this observation read.
    pub samples: u64,
}

fn roi_within(inner: &Roi, outer: &Roi) -> bool {
    inner.space == CoordinateSpace::Pixel
        && outer.space == CoordinateSpace::Pixel
        && inner.width > 0
        && inner.height > 0
        && inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x.checked_add(inner.width).is_some_and(|end| {
            outer
                .x
                .checked_add(outer.width)
                .is_some_and(|limit| end <= limit)
        })
        && inner.y.checked_add(inner.height).is_some_and(|end| {
            outer
                .y
                .checked_add(outer.height)
                .is_some_and(|limit| end <= limit)
        })
}

fn roi_holds(roi: &Roi, x: i64, y: i64) -> bool {
    x >= roi.x
        && y >= roi.y
        && roi.x.checked_add(roi.width).is_some_and(|end| x < end)
        && roi.y.checked_add(roi.height).is_some_and(|end| y < end)
}

impl Observation {
    /// Whether the runtime may take this as the kernel's word about `frame`
    /// for `detector` (a validated plan's): it names that detector and
    /// exactly that frame; known and unknown exclude each other and a known
    /// value is never negative; a target only on a known nonzero value, with
    /// a track, a hitbox inside the detector's ROI as placed in the frame
    /// (`game_state::frame_roi`, the one placement) and the aim point inside
    /// the hitbox; the scale is that placement's; cells only for a cells
    /// layout, one per cell, each known or unknown with a reason; no more
    /// samples than the tick allowed. Anything else acts on nothing.
    #[must_use]
    pub fn admissible(&self, detector: &Detector, frame: &FrameFacts, samples: u64) -> bool {
        let Some(color) = &detector.color else {
            return false;
        };
        if self.detector_id != detector.id
            || FrameRef::of(frame).as_ref() != Some(&self.frame)
            || self.samples > samples
            || self.unknown.is_some() == self.value.is_some()
            || self.value.is_some_and(|value| value < 0)
        {
            return false;
        }
        let Some((placed, scale)) =
            game_state::frame_roi(&detector.roi, color, &frame.pixel_extent)
        else {
            return self.value.is_none() && self.target.is_none() && self.cells.is_none();
        };
        if self.scale != scale {
            return false;
        }
        if let Some(target) = &self.target
            && (self.value.is_none_or(|value| value == 0)
                || target.track_id == 0
                || !roi_within(&target.roi, &placed)
                || !roi_holds(&target.roi, target.point_x, target.point_y))
        {
            return false;
        }
        if let Some(cells) = &self.cells {
            let Layout::Cells { rows, columns, .. } = color.layout else {
                return false;
            };
            let classes = color.classes.len() as u64;
            if rows.checked_mul(columns) != Some(cells.len() as u64)
                || cells.iter().any(|cell| {
                    cell.share_permille > 1000
                        || cell.class > classes
                        || (cell.class == 0) != cell.unknown.is_some()
                })
            {
                return false;
            }
        }
        true
    }
}
