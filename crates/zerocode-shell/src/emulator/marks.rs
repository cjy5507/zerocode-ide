//! Mobile trees share the desktop's numbering, legend and pin contract.

use std::sync::Mutex;
use std::time::Instant;

use serde_json::{Value, json};
use zerocode_core::computer_use::{EmulatorPlatform, MARK_LOOKS_KEPT, MARK_PIN_TOLERANCE_POINTS};
use zerocode_core::computer_use_protocol::frame::ShotFrame;
use zerocode_core::computer_use_protocol::marks::{
    self as shared, ElementFace, FaceFrame, ITEMS_KEY, LEGEND_KEY, LOOK_ID_KEY, MarkInput,
    MarkPlan, Pin,
};
use zerocode_core::computer_use_protocol::render::Rect;
use zerocode_core::computer_use_protocol::{ProviderError, cache, error_code};

use crate::computer_use::observe::Kept;

/// A transport address is not a persistent device identity: Android can
/// reuse the same emulator serial for a different AVD after a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Device {
    pub address: String,
    pub identity: String,
}

/// Fold either backend's tree once, retaining its preorder indexes and the
/// nearest named ancestor. Both marks and the last-moment check read this.
pub(crate) fn faces(platform: EmulatorPlatform, tree: &Value) -> Vec<ElementFace> {
    let fields = match platform {
        EmulatorPlatform::Ios => Fields {
            role: "type",
            names: &["AXLabel", "AXValue"],
            compound_names: false,
            identity: &[
                "type",
                "AXUniqueId",
                "AXLabel",
                "AXValue",
                "role_description",
                "enabled",
            ],
        },
        EmulatorPlatform::Android => Fields {
            role: "className",
            names: &["contentDesc", "text"],
            compound_names: true,
            identity: &[
                "className",
                "resourceId",
                "packageName",
                "text",
                "contentDesc",
                "enabled",
            ],
        },
    };
    let mut pending = vec![(tree, None, Vec::new())];
    let mut result = Vec::new();
    let mut index = 0;
    while let Some((node, context, mut lineage)) = pending.pop() {
        if let Some(roots) = node.as_array() {
            pending.extend(
                roots
                    .iter()
                    .rev()
                    .map(|root| (root, context.clone(), lineage.clone())),
            );
            continue;
        }
        // A recycled child identifier under a different app/row is a
        // different control even when that parent's displayed name is equal.
        // Keep the exported ancestor identities, never a guessed bundle id.
        lineage.push(Value::Array(
            fields
                .identity
                .iter()
                .map(|key| node.get(key).cloned().unwrap_or(Value::Null))
                .collect(),
        ));
        let name = fields.name(node);
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            let context = name.clone().or_else(|| context.clone());
            pending.extend(
                children
                    .iter()
                    .rev()
                    .map(|child| (child, context.clone(), lineage.clone())),
            );
        }
        let at = index;
        index += 1;
        let Some(frame) = node_frame(platform, node) else {
            continue;
        };
        let raw_role = text(node, fields.role).unwrap_or_default();
        let role = if platform == EmulatorPlatform::Ios && !raw_role.starts_with("AX") {
            format!("AX{raw_role}")
        } else {
            raw_role
        };
        let enabled = node.get("enabled").and_then(Value::as_bool) == Some(true);
        let clickable = node.get("clickable").and_then(Value::as_bool) == Some(true);
        // The iOS exporter hit-tests each element's centre (`hit_at_centre`,
        // AccessibilityBridge.swift): true — what is on top there is the
        // element, something inside it or around it, so the centre is its
        // own; false — something else is on top (the floating search field
        // over the last Settings row), or the centre is off the screen. The
        // numbering reads one thing of `visible`: whether the centre is on it.
        // So the centre's answer is carried as all of the frame, or none of
        // it — the full frame is still not claimed hit-tested. Android's
        // exporter answers no such question; its `visible` stays unknown and
        // the numbering refuses any centre another candidate could hold.
        let visible = match platform {
            EmulatorPlatform::Ios => {
                node.get(HIT_AT_CENTRE_KEY)
                    .and_then(Value::as_bool)
                    .map(|answered| {
                        if answered {
                            FaceFrame::from(frame)
                        } else {
                            FaceFrame {
                                x: frame.x,
                                y: frame.y,
                                width: 0.0,
                                height: 0.0,
                            }
                        }
                    })
            }
            EmulatorPlatform::Android => None,
        };
        result.push(ElementFace {
            index: at,
            role,
            name,
            // Neither exporter supplies a placeholder. AXValue is a value,
            // not a placeholder, and must not be relabelled as one (F16).
            placeholder: None,
            traits: if enabled {
                Vec::new()
            } else {
                vec!["disabled".into()]
            },
            // iOS exports roles and enabled, but no action list or traits.
            // Its known pressable roles are already in the shared MARK_ROLES.
            // Android's clickable is positive evidence of a press action.
            actions: if platform == EmulatorPlatform::Android && clickable {
                vec![zerocode_core::computer_use::MACOS_PRESS_ACTIONS[0].into()]
            } else {
                Vec::new()
            },
            x: frame.x,
            y: frame.y,
            width: frame.width,
            height: frame.height,
            signature: Value::Array(lineage).to_string(),
            visible,
            context,
        });
    }
    result
}

/// The iOS exporter's answer for an element's centre — the key
/// `AccessibilityBridge.swift` writes (a source contract holds the spelling).
pub(crate) const HIT_AT_CENTRE_KEY: &str = "hit_at_centre";

struct Fields {
    role: &'static str,
    names: &'static [&'static str],
    compound_names: bool,
    identity: &'static [&'static str],
}

impl Fields {
    fn own_name(&self, node: &Value) -> Option<String> {
        self.names.iter().find_map(|key| text(node, key))
    }

    fn name(&self, node: &Value) -> Option<String> {
        self.own_name(node).or_else(|| {
            // Android often puts a row's press on its layout and its title
            // on a non-interactive child. Preserve those exported words in
            // tree order, stopping at separately clickable controls. This
            // does not synthesize an absent iOS AXLabel/AXValue (F16).
            if !self.compound_names || node.get("clickable").and_then(Value::as_bool) != Some(true)
            {
                return None;
            }
            let mut pending: Vec<_> = node.get("children")?.as_array()?.iter().rev().collect();
            let mut words = Vec::new();
            while let Some(child) = pending.pop() {
                if child.get("clickable").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                if let Some(name) = self.own_name(child) {
                    words.push(name);
                } else if let Some(children) = child.get("children").and_then(Value::as_array) {
                    pending.extend(children.iter().rev());
                }
            }
            (!words.is_empty()).then(|| words.join(" "))
        })
    }
}

fn text(node: &Value, key: &str) -> Option<String> {
    node.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
}

fn node_frame(platform: EmulatorPlatform, node: &Value) -> Option<Rect> {
    let (frame, keys) = match platform {
        EmulatorPlatform::Ios => (node.get("frame")?, ["x", "y", "width", "height"]),
        EmulatorPlatform::Android => (node.get("bounds")?, ["left", "top", "right", "bottom"]),
    };
    let number = |key: &str| frame.get(key)?.as_f64().filter(|value| value.is_finite());
    let (x, y, a, b) = (
        number(keys[0])?,
        number(keys[1])?,
        number(keys[2])?,
        number(keys[3])?,
    );
    let (width, height) = if platform == EmulatorPlatform::Android {
        (a - x, b - y)
    } else {
        (a, b)
    };
    (width > 0.0 && height > 0.0).then(|| Rect::new(x, y, width, height))
}

/// Raw device geometry, before the last conversion into normalized tap units.
#[derive(Debug)]
pub(super) struct Snapshot {
    pub faces: Vec<ElementFace>,
    pub screen: Rect,
    /// Keep exported metadata alongside the folded faces; never infer it from a label.
    pub raw: Value,
}

impl Snapshot {
    pub fn new(platform: EmulatorPlatform, tree: &Value, screen: Rect) -> Result<Self, String> {
        if ![screen.x, screen.y, screen.width, screen.height]
            .into_iter()
            .all(f64::is_finite)
            || screen.width <= 1.0
            || screen.height <= 1.0
            || screen.width > f64::from(u32::MAX)
            || screen.height > f64::from(u32::MAX)
        {
            return Err("the accessibility tree has no usable device frame".into());
        }
        if truncated(tree) {
            return Err("the accessibility tree is truncated; no marks were issued".into());
        }
        let mut faces = faces(platform, tree);
        for face in &mut faces {
            face.x -= screen.x;
            face.y -= screen.y;
            if let Some(seen) = &mut face.visible {
                seen.x -= screen.x;
                seen.y -= screen.y;
            }
        }
        Ok(Self {
            faces,
            screen,
            raw: tree.clone(),
        })
    }

    #[cfg(any(target_os = "macos", test))]
    pub fn ios(tree: &Value) -> Result<Self, String> {
        let screen = tree
            .as_array()
            .and_then(|roots| roots.first())
            .and_then(|root| node_frame(EmulatorPlatform::Ios, root))
            .ok_or("the iOS accessibility root has no device frame")?;
        Self::new(EmulatorPlatform::Ios, tree, screen)
    }
}

fn truncated(tree: &Value) -> bool {
    tree.get("truncated").and_then(Value::as_bool) == Some(true)
        || tree
            .as_array()
            .or_else(|| tree.get("children").and_then(Value::as_array))
            .is_some_and(|children| children.iter().any(truncated))
}

fn numbered(faces: &[ElementFace], screen: Rect) -> MarkPlan {
    // Snapshot validates finite positive dimensions bounded by u32::MAX.
    let window = Rect::new(0.0, 0.0, screen.width, screen.height);
    shared::plan_where(
        &MarkInput {
            faces,
            window,
            frame: ShotFrame::UNIT,
            picture: (screen.width.ceil() as u32, screen.height.ceil() as u32),
            occluders: Vec::new(),
        },
        |face| {
            // A centre the exporter answered for (`visible`, from
            // `hit_at_centre`) is judged by that answer alone. A tree that
            // proves no z-order — Android's, an iOS tree before the exporter
            // was asked — is judged as before: when another candidate
            // contains this centre, choosing which one receives the tap
            // would invent that missing evidence (the floating search field
            // over the last Settings row: 0 marks, 2026-09-21). Refuse the
            // ambiguous number instead.
            if face.visible.is_some() {
                return true;
            }
            let centre = face.local();
            !faces.iter().any(|other| {
                other.index != face.index
                    && shared::is_mark_candidate(other, window)
                    && other.seen().contains_point(centre.mid_x(), centre.mid_y())
            })
        },
    )
}

/// The request carries its tolerance from the shared policy to the backend.
/// The backend re-reads and checks it in the same blocking task as the tap.
#[derive(Clone)]
pub(super) struct PinnedTap {
    index: usize,
    pin: Pin,
    screen: Rect,
    platform: EmulatorPlatform,
    made: Instant,
    identity: String,
}

impl PinnedTap {
    pub fn on_device<T>(
        &self,
        identity: &str,
        press: impl FnOnce() -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        if self.identity != identity {
            return Err(wrong_device());
        }
        press()
    }

    /// The device gate spans the caller's fresh snapshot and this tap. The
    /// captured stream must still be alive at the last input boundary; a
    /// replacement stream cannot authorize the old stream's pending press.
    pub fn perform_in(
        &self,
        input: &super::session::SessionInput<'_>,
        faces: &[ElementFace],
        screen: Rect,
        tap: impl FnOnce(f64, f64) -> Result<(), ProviderError>,
    ) -> Result<(), ProviderError> {
        self.perform(faces, screen, |x, y| {
            if input.is_alive() {
                tap(x, y)
            } else {
                Err(self.broken())
            }
        })
    }

    fn broken(&self) -> ProviderError {
        ProviderError::new(
            error_code::PIN_BROKEN,
            shared::pin_broken(self.index).message,
        )
    }

    fn perform(
        &self,
        faces: &[ElementFace],
        screen: Rect,
        tap: impl FnOnce(f64, f64) -> Result<(), ProviderError>,
    ) -> Result<(), ProviderError> {
        self.perform_with_policy(faces, screen, crate::computer_use::confirm::policy(), tap)
    }

    fn perform_with_policy(
        &self,
        faces: &[ElementFace],
        screen: Rect,
        policy: crate::computer_use::confirm::Policy,
        tap: impl FnOnce(f64, f64) -> Result<(), ProviderError>,
    ) -> Result<(), ProviderError> {
        let broken = || self.broken();
        if cache::is_expired(self.made, Instant::now()) || !self.screen.matches_within(&screen, 0.0)
        {
            return Err(broken());
        }
        let face = faces
            .iter()
            .find(|face| face.index == self.index)
            .ok_or_else(broken)?;
        if !self.pin.holds(
            Some(&face.signature),
            Some(face.words()),
            face.context.as_deref(),
            Some(face.local()),
        ) || !numbered(faces, screen)
            .marks
            .iter()
            .any(|mark| mark.element_index == self.index)
        {
            return Err(broken());
        }
        self.press(face, screen, policy, tap)
    }

    /// The point a press proven at its centre asks about: the pinned frame's
    /// centre, in the tree's own points (the look's screen origin put back).
    #[cfg(any(target_os = "macos", test))]
    pub fn centre(&self) -> (f64, f64) {
        (
            self.screen.x + self.pin.frame.mid_x(),
            self.screen.y + self.pin.frame.mid_y(),
        )
    }

    /// Whether the control the mark was drawn on still stands where it did in
    /// `faces` — by the rule a point proves it with: its own identity and
    /// its words and frame within the pin (t-6385). A read after the press
    /// without it has moved, however the reads before it compare.
    #[cfg(any(target_os = "macos", test))]
    pub fn stands_in(&self, faces: &[ElementFace]) -> bool {
        let pinned = lineage_ends(&self.pin.signature);
        pinned.is_some()
            && faces.iter().any(|face| {
                lineage_ends(&face.signature) == pinned
                    && self
                        .pin
                        .holds_at_point(Some(face.words()), Some(face.local()))
            })
    }

    /// [`Self::perform_at_centre`] inside the device gate, as
    /// [`Self::perform_in`] is: the stream the look was taken on must still
    /// be alive at the tap.
    #[cfg(any(target_os = "macos", test))]
    pub fn perform_at_centre_in(
        &self,
        input: &super::session::SessionInput<'_>,
        answer: &Value,
        tap: impl FnOnce(f64, f64) -> Result<(), ProviderError>,
    ) -> Option<Result<(), ProviderError>> {
        self.perform_at_centre(answer, crate::computer_use::confirm::policy(), |x, y| {
            if input.is_alive() {
                tap(x, y)
            } else {
                Err(self.broken())
            }
        })
    }

    /// A press proven at the one point it lands on (t-6385) instead of on the
    /// whole tree read again: 633 ms a press on iOS, 3.5 ms a point
    /// (t-6350). `answer` is what the exporter found on top at
    /// [`Self::centre`] — the application's node holding that one element,
    /// the tree's own shape — read by the fold every tree is read by.
    ///
    /// The element is the control the mark was drawn on when the look's
    /// application is still the one in front, the element's own identity is
    /// the one the look pinned (the first and last links of the lineage), and
    /// its words and frame hold as the whole pin holds them
    /// ([`Pin::holds_at_point`]). The links between — the parents, the row a
    /// star sits in — are what one point cannot show, and every press the
    /// point cannot prove answers `None`: the tree is read and decides
    /// exactly as it did before. A screen that turned, a look that aged and a
    /// person's guarded step are refused here as the tree refuses them.
    #[cfg(any(target_os = "macos", test))]
    fn perform_at_centre(
        &self,
        answer: &Value,
        policy: crate::computer_use::confirm::Policy,
        tap: impl FnOnce(f64, f64) -> Result<(), ProviderError>,
    ) -> Option<Result<(), ProviderError>> {
        if cache::is_expired(self.made, Instant::now()) {
            return Some(Err(self.broken()));
        }
        let found = Snapshot::ios(answer).ok()?;
        if !self.screen.matches_within(&found.screen, 0.0) {
            return Some(Err(self.broken()));
        }
        // The application is the root; the element is the one node under it.
        let face = found.faces.iter().find(|face| face.index > 0)?;
        let same_control = matches!(
            (lineage_ends(&face.signature), lineage_ends(&self.pin.signature)),
            (Some(seen), Some(pinned)) if seen == pinned
        ) && self
            .pin
            .holds_at_point(Some(face.words()), Some(face.local()));
        same_control.then(|| self.press(face, found.screen, policy, tap))
    }

    /// The press itself, once a face is proven the control its mark was
    /// drawn on — on the tree or at its centre: a person's guarded step stops
    /// here, and the fresh centre becomes the door's own units.
    fn press(
        &self,
        face: &ElementFace,
        screen: Rect,
        policy: crate::computer_use::confirm::Policy,
        tap: impl FnOnce(f64, f64) -> Result<(), ProviderError>,
    ) -> Result<(), ProviderError> {
        if let Some(kind) = zerocode_core::computer_use::confirm_kind_of(face.words())
            .filter(|kind| policy.asks(*kind))
        {
            return Err(ProviderError::new(
                error_code::CONFIRMATION_REQUIRED,
                format!("{}: {}", kind.as_str(), face.words()),
            ));
        }
        // Android's existing device_point uses the last pixel (size - 1),
        // while the iOS HID door consumes a fraction of the root AX frame.
        // Invert those existing conversions and press the fresh centre.
        let edge = if self.platform == EmulatorPlatform::Android {
            1.0
        } else {
            0.0
        };
        let x = face.local().mid_x() / (screen.width - edge);
        let y = face.local().mid_y() / (screen.height - edge);
        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
            return Err(self.broken());
        }
        tap(x, y)
    }
}

/// The first and last links of a face's lineage (the signature [`faces`]
/// writes): the application the look was taken in, and the element's own
/// identity. The links between are its parents — what one point cannot show.
#[cfg(any(target_os = "macos", test))]
fn lineage_ends(signature: &str) -> Option<(Value, Value)> {
    let lineage: Value = serde_json::from_str(signature).ok()?;
    let lineage = lineage.as_array()?;
    Some((lineage.first()?.clone(), lineage.last()?.clone()))
}

/// Which proof a press by number went out on (t-6385), in the word its
/// answer names it by under [`CONFIRMED_BY_KEY`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Proof {
    /// The element on top at the one point the press lands on.
    Point,
    /// The whole tree, read again.
    Tree,
}

impl Proof {
    pub const fn word(self) -> &'static str {
        match self {
            Self::Point => "point",
            Self::Tree => "tree",
        }
    }
}

/// The key a mark click's answer names its [`Proof`] under.
pub(crate) const CONFIRMED_BY_KEY: &str = "confirmedBy";

/// What a mark click came to: the proof it went out on, and — where its door
/// waits for the screen it led to (iOS, t-6385) — how that screen settled.
pub(super) struct Pressed {
    pub proof: Proof,
    pub settled: Option<Settled>,
}

/// One face as a settling press compares two reads of a tree (t-6385): its
/// role and words, its frame to the whole point — a frame wobbles by a
/// fraction of one between two fetches of a still screen — and what its
/// centre answered.
pub(super) type FaceShape = (String, String, [i64; 4], Option<bool>);

/// A tree's faces as a settling press compares them, in the fold's order.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) fn shape(faces: &[ElementFace]) -> Vec<FaceShape> {
    faces
        .iter()
        .map(|face| {
            (
                face.role.clone(),
                face.words().to_string(),
                [face.x, face.y, face.width, face.height].map(|side| side.round() as i64),
                face.visible.map(|seen| seen.width > 0.0),
            )
        })
        .collect()
}

/// How a pressed screen's settling ended (t-6385), in the word a click's
/// answer and a walk's row name it by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) enum Settle {
    /// Not yet decided: read again.
    Reading,
    /// It changed after the press and then read the same twice running.
    Still,
    /// Nothing changed within the quiet window: the press moved nothing.
    Unmoved,
    /// The ceiling passed with the tree still changing.
    Ceiling,
}

impl Settle {
    pub const fn word(self) -> &'static str {
        match self {
            Self::Reading => "reading",
            Self::Still => "still",
            Self::Unmoved => "unmoved",
            Self::Ceiling => "ceiling",
        }
    }
}

/// Whether a pressed screen has stopped changing, decided read by read
/// (t-6385).
///
/// The first read after the tap is the screen as the press found it: a tap
/// lands about 280 ms before the app paints anything (t-6350), so two reads
/// alike at once prove nothing. The screen has settled once a read differs
/// from that first one and the next reads the same again; it moved nothing
/// when no read differs within the quiet window; past the ceiling it is
/// answered as it last read, unsettled. A read that failed breaks a run of
/// alike reads and decides nothing.
#[derive(Debug)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) struct Settling<S> {
    quiet: std::time::Duration,
    ceiling: std::time::Duration,
    first: Option<S>,
    last: Option<S>,
    moved: bool,
    reads: usize,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl<S: Clone + PartialEq> Settling<S> {
    pub const fn new(quiet: std::time::Duration, ceiling: std::time::Duration) -> Self {
        Self {
            quiet,
            ceiling,
            first: None,
            last: None,
            moved: false,
            reads: 0,
        }
    }

    /// One more read, `since` the tap: what the settling has come to.
    pub fn read(&mut self, shape: Option<S>, since: std::time::Duration) -> Settle {
        self.reads += 1;
        let Some(shape) = shape else {
            self.last = None;
            return if since >= self.ceiling {
                Settle::Ceiling
            } else {
                Settle::Reading
            };
        };
        let repeated = self.last.as_ref() == Some(&shape);
        match &self.first {
            None => self.first = Some(shape.clone()),
            Some(first) if *first != shape => self.moved = true,
            Some(_) => {}
        }
        self.last = Some(shape);
        if self.moved && repeated {
            Settle::Still
        } else if !self.moved && since >= self.quiet {
            Settle::Unmoved
        } else if since >= self.ceiling {
            Settle::Ceiling
        } else {
            Settle::Reading
        }
    }

    pub const fn reads(&self) -> usize {
        self.reads
    }

    /// The screen has moved whatever the reads compare to: the pressed
    /// control is no longer where it stood — a press whose screen changed
    /// before its first read came back.
    pub const fn moved(&mut self) {
        self.moved = true;
    }
}

/// What a pressed screen did before the press answered (t-6385). Only an iOS
/// press waits for its screen yet.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) struct Settled {
    pub settle: Settle,
    pub reads: usize,
    pub ms: u64,
    /// The last tree it read: what the screen settled on, or was left on.
    pub last: Option<Snapshot>,
}

impl Settled {
    /// What a click's answer and a walk's row say of it.
    fn said(&self) -> Value {
        json!({ "ms": self.ms, "reads": self.reads, "settle": self.settle.word() })
    }
}

struct Table {
    platform: EmulatorPlatform,
    device: Device,
    plan: MarkPlan,
    screen: Rect,
    made: Instant,
}

impl Table {
    fn request(
        &self,
        platform: EmulatorPlatform,
        device: &Device,
        mark: usize,
    ) -> Result<PinnedTap, ProviderError> {
        if self.platform != platform
            || self.device != *device
            || cache::is_expired(self.made, Instant::now())
        {
            return Err(wrong_device());
        }
        let placed = self
            .plan
            .marks
            .iter()
            .find(|placed| placed.mark == mark)
            .ok_or_else(|| {
                ProviderError::invalid_argument(format!("mark {mark} is not on this look"))
            })?;
        Ok(PinnedTap {
            index: placed.element_index,
            pin: placed.pin(MARK_PIN_TOLERANCE_POINTS),
            screen: self.screen,
            platform,
            made: self.made,
            identity: self.device.identity.clone(),
        })
    }

    fn answer(&self, look: &str) -> Value {
        let (items, legend) = items_and_legend(&self.plan);
        json!({ LOOK_ID_KEY: look, ITEMS_KEY: items, LEGEND_KEY: legend,
            "candidates": self.plan.candidates, "omitted": self.plan.omitted })
    }
}

/// A plan's items and the legend a model reads them by — what a look
/// answers, and what a press's preview answers of the screen it settled on.
fn items_and_legend(plan: &MarkPlan) -> (Vec<Value>, String) {
    let items = shared::items(plan);
    let legend = items
        .iter()
        .filter_map(shared::legend_line)
        .collect::<Vec<_>>()
        .join("\n");
    (items, legend)
}

/// What a press answers of the screen it settled on when asked for a
/// preview (t-6385): the items and legend a look of that tree would answer,
/// numbered by the same plan — with no look id, since nothing in it can be
/// pressed.
pub(super) fn preview_of(snapshot: &Snapshot) -> Value {
    let (items, legend) = items_and_legend(&numbered(&snapshot.faces, snapshot.screen));
    json!({ ITEMS_KEY: items, LEGEND_KEY: legend })
}

static TABLES: Mutex<Kept<Table>> = Mutex::new(Kept::new(MARK_LOOKS_KEPT));

fn wrong_device() -> ProviderError {
    ProviderError::new(
        error_code::PIN_BROKEN,
        "the look belongs to another device or has expired; run marks again",
    )
}

pub(crate) fn backend_error(error: impl ToString) -> ProviderError {
    ProviderError::new("emulator_error", error.to_string())
}

/// The same fresh, complete, geometry-checked observation for marks and checks.
pub(super) async fn snapshot(
    platform: EmulatorPlatform,
    device: &Device,
) -> Result<Snapshot, ProviderError> {
    match platform {
        EmulatorPlatform::Ios => super::ios::marks_snapshot_direct(device.address.clone()).await,
        EmulatorPlatform::Android => {
            super::android::marks_snapshot_direct(device.address.clone(), device.identity.clone())
                .await
        }
    }
    .map_err(backend_error)
}

/// Number the device's current tree without capturing or drawing a picture.
///
/// `count`, when given, is counted in the same tree the way `find` counts it
/// (`checks::found`), under the answer's `count` — so a walk that looks at
/// the screen and asks whether its words are there reads the tree once
/// (t-6385; a separate `find` read the same screen again, 946 ms a step).
pub(crate) async fn observe(
    platform: EmulatorPlatform,
    device: Device,
    count: Option<&str>,
) -> Result<Value, ProviderError> {
    let snapshot = snapshot(platform, &device).await?;
    let counted = count.map(|subject| super::checks::found(&snapshot, subject));
    let table = Table {
        platform,
        device,
        plan: numbered(&snapshot.faces, snapshot.screen),
        screen: snapshot.screen,
        made: Instant::now(),
    };
    let look = uuid::Uuid::new_v4().to_string();
    let mut answer = table.answer(&look);
    if let Some(counted) = counted {
        answer[super::checks::COUNT_KEY] = json!(counted);
    }
    TABLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .keep(look, table);
    Ok(answer)
}

/// Recall the exact look, then let the backend prove the pin before input.
pub(crate) async fn click(
    platform: EmulatorPlatform,
    device: Device,
    mark: usize,
    look: &str,
    count: Option<&str>,
    preview: bool,
) -> Result<Value, ProviderError> {
    let (request, legend) = {
        let held = TABLES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let table = held.get(look).ok_or_else(|| {
            ProviderError::new(
                error_code::PIN_BROKEN,
                "the look is no longer retained; run marks again",
            )
        })?;
        let request = table.request(platform, &device, mark)?;
        let legend = shared::items(&table.plan)
            .iter()
            .find(|item| {
                item.get("mark")
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                    == Some(mark)
            })
            .and_then(shared::legend_line)
            .unwrap_or_default();
        (request, legend)
    };
    let pressed = match platform {
        EmulatorPlatform::Ios => super::ios::click_mark_direct(device.address, request).await?,
        EmulatorPlatform::Android => {
            super::android::click_mark_direct(device.address, request).await?
        }
    };
    let mut answer = json!({ "performed": true, "mark": mark, LOOK_ID_KEY: look,
        LEGEND_KEY: legend, CONFIRMED_BY_KEY: pressed.proof.word() });
    if let Some(settled) = &pressed.settled {
        answer[zerocode_core::agent_emulator::EMULATOR_SETTLE_KEY] = settled.said();
        // The caller's words, counted in the tree the press settled on — the
        // walk alone, so a count is proof they are there and a zero is not
        // proof they are absent (only a look's grid finds some elements).
        if let (Some(subject), Some(last)) = (count, &settled.last) {
            answer[super::checks::COUNT_KEY] = json!(super::checks::found(last, subject));
        }
        if preview && let Some(last) = &settled.last {
            answer[zerocode_core::computer_use::EMULATOR_PREVIEW_FLAG] = preview_of(last);
        }
    }
    Ok(answer)
}

#[cfg(test)]
mod tests;
