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
            return Err(broken());
        }
        tap(x, y)
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
        let items = shared::items(&self.plan);
        let legend = items
            .iter()
            .filter_map(shared::legend_line)
            .collect::<Vec<_>>()
            .join("\n");
        json!({ LOOK_ID_KEY: look, ITEMS_KEY: items, LEGEND_KEY: legend,
            "candidates": self.plan.candidates, "omitted": self.plan.omitted })
    }
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
pub(crate) async fn observe(
    platform: EmulatorPlatform,
    device: Device,
) -> Result<Value, ProviderError> {
    let snapshot = snapshot(platform, &device).await?;
    let table = Table {
        platform,
        device,
        plan: numbered(&snapshot.faces, snapshot.screen),
        screen: snapshot.screen,
        made: Instant::now(),
    };
    let look = uuid::Uuid::new_v4().to_string();
    let answer = table.answer(&look);
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
    match platform {
        EmulatorPlatform::Ios => super::ios::click_mark_direct(device.address, request).await?,
        EmulatorPlatform::Android => {
            super::android::click_mark_direct(device.address, request).await?
        }
    }
    Ok(json!({ "performed": true, "mark": mark, LOOK_ID_KEY: look, LEGEND_KEY: legend }))
}

#[cfg(test)]
mod tests;
