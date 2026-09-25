//! Pixel game state: what a colour detector reads, checked before any
//! helper runs it. A reflex plan's colour detector carries a [`ColorSpec`];
//! the plan validator calls [`validate_color`] for each one, and the macOS
//! helper runs the same rules in Swift (`Perception/PerceptionSpec.swift`)
//! before its kernel reads a pixel. Both read the cases in
//! `fixtures/game-state/spec_cases.json` and the limits in one table,
//! [`LIMITS`], which the window sends to the helper ([`limits_wire`]); the
//! helper keeps no copy.
//!
//! Pixels never cross the socket. The kernel runs beside the frame in the
//! helper and answers observations, which are data: a lease, and with it
//! any input, is the runtime's to issue.

use serde::{Deserialize, Serialize};

use super::reflex::{CoordinateSpace, PixelExtent, Roi, Scale};

pub mod ax;
pub mod learn;

/// The one table of perception limits. Fields are declared in byte order, so
/// the struct's own serialization is the canonical wire the helper checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerceptionLimits {
    /// Anchors one spec may name.
    pub max_anchors: u64,
    /// Blobs one blob detector may follow; more is `ambiguous`.
    pub max_blobs: u64,
    /// Cells in one board: a 16 × 16 grid.
    pub max_cells: u64,
    /// Palette classes in one spec: the kernel's inner loop is linear in it.
    pub max_classes: u64,
    /// Fresh captures a value may be made to wait for before it is known.
    pub max_confirm: u64,
    /// Samples one observation of one detector may read.
    pub max_detector_samples: u64,
    /// Reference pixels a blob may move per capture and keep its track.
    pub max_gate: u64,
    /// Samples along one axis of a cell.
    pub max_lattice: u64,
    /// Host nanoseconds one tick's observations may take together: the
    /// perception share of a 16.6 ms frame.
    pub max_tick_ns: u64,
    /// Samples one tick's observations may read together: as many as the
    /// game-state probe read within `max_tick_ns` at p95 on a loaded machine
    /// (`tools/game-state-probe`). Twice as many did not fit.
    pub max_tick_samples: u64,
    /// Chebyshev distance a class may accept on each channel.
    pub max_tolerance: u64,
    /// Samples a cell needs before its colour means anything.
    pub min_cell_samples: u64,
}

pub const LIMITS: PerceptionLimits = PerceptionLimits {
    max_anchors: 8,
    max_blobs: 16,
    max_cells: 256,
    max_classes: 8,
    max_confirm: 8,
    max_detector_samples: 262_144,
    max_gate: 256,
    max_lattice: 8,
    max_tick_ns: 5_000_000,
    max_tick_samples: 262_144,
    max_tolerance: 64,
    min_cell_samples: 4,
};

/// The limits as the window sends them to the helper: compact JSON, keys in
/// byte order, integers only.
#[must_use]
pub fn limits_wire(limits: &PerceptionLimits) -> Vec<u8> {
    serde_json::to_vec(limits).expect("integer limits serialize")
}

/// The colour space a palette is written in. The kernel converts nothing:
/// it reads only frames delivered in the palette's space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteSpace {
    Srgb,
}

/// One colour a sample may be: every channel within `tolerance` of this
/// one (Chebyshev distance).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorClass {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub tolerance: u8,
}

impl ColorClass {
    /// No sample can be both: on some channel the two lie further apart
    /// than their tolerances reach together.
    #[must_use]
    pub fn apart(&self, other: &Self) -> bool {
        let reach = u16::from(self.tolerance) + u16::from(other.tolerance);
        [(self.r, other.r), (self.g, other.g), (self.b, other.b)]
            .into_iter()
            .any(|(a, b)| u16::from(a.abs_diff(b)) > reach)
    }
}

/// What a cells detector answers, numbered from 1: cell numbers run in
/// reading order and class numbers in palette order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Readout {
    /// The class of one cell.
    Cell { index: u64 },
    /// The number of the first cell of a class; 0 when every cell is known
    /// and none is. Unknown while any cell before it is unknown.
    First { class: u64 },
    /// How many cells are of a class. Unknown while any cell is unknown.
    Count { class: u64 },
}

/// How a detector lays its samples over its ROI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Layout {
    /// The ROI split into `rows` × `columns` pitch boxes; a tile of the given
    /// share of the pitch sits centred in each, and `lattice` × `lattice`
    /// samples are read inside the tile less its inset on every side. A
    /// class is the cell's when it holds at least `min_share_permille` of
    /// them — a strict majority, so two classes can never both.
    Cells {
        rows: u64,
        columns: u64,
        tile_width_permille: u64,
        tile_height_permille: u64,
        inset_permille: u64,
        lattice: u64,
        min_share_permille: u64,
        readout: Readout,
    },
    /// Samples every `step` reference pixels; the samples of `class` that
    /// touch (4-neighbours on the lattice) are one blob when there are at
    /// least `min_samples` of them. A blob keeps its track while it moves at
    /// most `gate` reference pixels per capture.
    Blobs {
        class: u64,
        step: u64,
        min_samples: u64,
        gate: u64,
        max_blobs: u64,
    },
}

/// A point, in reference pixels, that must show a colour for the frame to be
/// the scene the plan was written for — a HUD, a frame edge, a corner mark.
/// `class` 0 names the ground. A board that looks the same turned half round
/// is told apart only by an anchor it does not share with its turned self.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub x: u32,
    pub y: u32,
    pub class: u64,
}

/// A colour detector's configuration, carried by the plan and covered by
/// its hash. The ROI it reads is the detector's, written against the
/// reference extent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorSpec {
    pub space: PaletteSpace,
    pub classes: Vec<ColorClass>,
    /// The background between cells, or behind blobs. With it a cell is known
    /// only while the gaps around it are ground, and a blob count of 0 is
    /// known only while every sample is ground or the class.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "written"
    )]
    pub ground: Option<ColorClass>,
    pub reference_width: u32,
    pub reference_height: u32,
    pub layout: Layout,
    /// Fresh captures of one scene that must agree before a value is known.
    pub confirm: u64,
    /// Points that must all show their colour before anything is read; when
    /// one misses, the frame is not this scene.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "written_list"
    )]
    pub anchors: Vec<Anchor>,
}

/// An optional field is either left out or written in full: the wire never
/// carries `null`, so a reader that took one would read more than the
/// helper does.
fn written<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

/// A list left empty is left out, for the same reason.
fn written_list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    let items = Vec::<T>::deserialize(deserializer)?;
    if items.is_empty() {
        return Err(serde::de::Error::custom("an empty list is never written"));
    }
    Ok(items)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecError {
    Budget,
    Geometry,
    Overlap,
    Reference,
    Threshold,
    Unsupported,
}

impl SpecError {
    /// The spelling the shared cases and the Swift validator use.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Budget => "budget",
            Self::Geometry => "geometry",
            Self::Overlap => "overlap",
            Self::Reference => "reference",
            Self::Threshold => "threshold",
            Self::Unsupported => "unsupported",
        }
    }
}

/// One axis of a cells layout: every pitch box along it is either the floor
/// or the ceiling of the span over the count, so checking both sizes checks
/// every cell.
struct Axis {
    span: u64,
    count: u64,
    tile_permille: u64,
    inset_permille: u64,
}

impl Axis {
    fn pitches(&self) -> [u64; 2] {
        [self.span / self.count, self.span.div_ceil(self.count)]
    }

    fn tile(&self, pitch: u64) -> u64 {
        pitch * self.tile_permille / 1000
    }

    /// The tile less its inset on both sides: where the lattice lies.
    fn sampled(&self, pitch: u64) -> u64 {
        let tile = self.tile(pitch);
        tile - 2 * (tile * self.inset_permille / 1000)
    }

    /// The gaps before and after the tile inside its pitch box.
    fn gaps(&self, pitch: u64) -> (u64, u64) {
        let spare = pitch - self.tile(pitch);
        (spare / 2, spare - spare / 2)
    }

    fn has_gaps(&self) -> bool {
        self.tile_permille < 1000
    }
}

/// The palette rules every spec and every adopted candidate meets: 1 to
/// `max_classes` classes, no tolerance over `max_tolerance`, and no two of
/// them, ground included, able to hold the same sample.
pub fn check_palette(
    classes: &[ColorClass],
    ground: Option<&ColorClass>,
    limits: &PerceptionLimits,
) -> Result<(), SpecError> {
    if classes.is_empty() || classes.len() as u64 > limits.max_classes {
        return Err(SpecError::Budget);
    }
    let palette: Vec<&ColorClass> = classes.iter().chain(ground).collect();
    if palette
        .iter()
        .any(|class| u64::from(class.tolerance) > limits.max_tolerance)
    {
        return Err(SpecError::Budget);
    }
    for (at, class) in palette.iter().enumerate() {
        if palette[at + 1..].iter().any(|other| !class.apart(other)) {
            return Err(SpecError::Overlap);
        }
    }
    Ok(())
}

/// Checks a colour detector's spec against its ROI and answers how many
/// samples one observation reads.
pub fn validate_color(
    spec: &ColorSpec,
    roi: &Roi,
    limits: &PerceptionLimits,
) -> Result<u64, SpecError> {
    let (width, height) = roi_size(roi).ok_or(SpecError::Geometry)?;
    let x = u64::try_from(roi.x).map_err(|_| SpecError::Geometry)?;
    let y = u64::try_from(roi.y).map_err(|_| SpecError::Geometry)?;
    let reference_width = u64::from(spec.reference_width);
    let reference_height = u64::from(spec.reference_height);
    if reference_width == 0
        || reference_height == 0
        || x.checked_add(width).is_none_or(|end| end > reference_width)
        || y.checked_add(height)
            .is_none_or(|end| end > reference_height)
    {
        return Err(SpecError::Geometry);
    }
    check_palette(&spec.classes, spec.ground.as_ref(), limits)?;
    if spec.confirm == 0 || spec.confirm > limits.max_confirm {
        return Err(SpecError::Budget);
    }
    let classes = spec.classes.len() as u64;
    if spec.anchors.len() as u64 > limits.max_anchors {
        return Err(SpecError::Budget);
    }
    if spec
        .anchors
        .iter()
        .any(|anchor| anchor.x >= spec.reference_width || anchor.y >= spec.reference_height)
    {
        return Err(SpecError::Geometry);
    }
    if spec
        .anchors
        .iter()
        .any(|anchor| anchor.class > classes || (anchor.class == 0 && spec.ground.is_none()))
    {
        return Err(SpecError::Reference);
    }
    let cost = match spec.layout {
        Layout::Cells {
            rows,
            columns,
            tile_width_permille,
            tile_height_permille,
            inset_permille,
            lattice,
            min_share_permille,
            readout,
        } => {
            let cells = rows.checked_mul(columns).ok_or(SpecError::Budget)?;
            if rows == 0 || columns == 0 || cells > limits.max_cells {
                return Err(SpecError::Budget);
            }
            if !(1..=1000).contains(&tile_width_permille)
                || !(1..=1000).contains(&tile_height_permille)
                || inset_permille > 499
            {
                return Err(SpecError::Geometry);
            }
            let samples = lattice.checked_mul(lattice).ok_or(SpecError::Budget)?;
            if lattice == 0 || lattice > limits.max_lattice || samples < limits.min_cell_samples {
                return Err(SpecError::Budget);
            }
            if !(501..=1000).contains(&min_share_permille) {
                return Err(SpecError::Threshold);
            }
            let in_range = match readout {
                Readout::Cell { index } => (1..=cells).contains(&index),
                Readout::First { class } | Readout::Count { class } => {
                    (1..=classes).contains(&class)
                }
            };
            if !in_range {
                return Err(SpecError::Reference);
            }
            let axes = [
                Axis {
                    span: width,
                    count: columns,
                    tile_permille: tile_width_permille,
                    inset_permille,
                },
                Axis {
                    span: height,
                    count: rows,
                    tile_permille: tile_height_permille,
                    inset_permille,
                },
            ];
            if axes.iter().any(|axis| {
                axis.pitches()
                    .into_iter()
                    .any(|pitch| axis.sampled(pitch) < lattice)
            }) {
                return Err(SpecError::Geometry);
            }
            let gap_sides = if spec.ground.is_some() {
                if axes.iter().all(|axis| !axis.has_gaps()) {
                    return Err(SpecError::Unsupported);
                }
                if axes.iter().filter(|axis| axis.has_gaps()).any(|axis| {
                    axis.pitches().into_iter().any(|pitch| {
                        let (before, after) = axis.gaps(pitch);
                        before == 0 || after == 0
                    })
                }) {
                    return Err(SpecError::Geometry);
                }
                2 * axes.iter().filter(|axis| axis.has_gaps()).count() as u64
            } else {
                0
            };
            gap_sides
                .checked_mul(lattice)
                .and_then(|gaps| gaps.checked_add(samples))
                .and_then(|per_cell| per_cell.checked_mul(cells))
                .ok_or(SpecError::Budget)?
        }
        Layout::Blobs {
            class,
            step,
            min_samples,
            gate,
            max_blobs,
        } => {
            if !(1..=classes).contains(&class) {
                return Err(SpecError::Reference);
            }
            if step == 0 || step > width.min(height) {
                return Err(SpecError::Geometry);
            }
            if min_samples == 0 {
                return Err(SpecError::Threshold);
            }
            if gate == 0 || gate > limits.max_gate || max_blobs == 0 || max_blobs > limits.max_blobs
            {
                return Err(SpecError::Budget);
            }
            width.div_ceil(step) * height.div_ceil(step)
        }
    };
    let cost = cost
        .checked_add(spec.anchors.len() as u64)
        .ok_or(SpecError::Budget)?;
    if cost > limits.max_detector_samples {
        return Err(SpecError::Budget);
    }
    Ok(cost)
}

/// A positive pixel-space ROI's width and height.
fn roi_size(roi: &Roi) -> Option<(u64, u64)> {
    if roi.space != CoordinateSpace::Pixel || roi.x < 0 || roi.y < 0 {
        return None;
    }
    let width = u64::try_from(roi.width).ok().filter(|width| *width > 0)?;
    let height = u64::try_from(roi.height)
        .ok()
        .filter(|height| *height > 0)?;
    Some((width, height))
}

/// The detector's ROI, written against the spec's reference extent, placed
/// in a frame's pixel extent — with the frame's scale over the reference.
/// Only a uniform rescale is followed: when the width and height ratios part
/// by more than the rounding of one pixel on each axis, the frame is another
/// shape and there is no ROI to read. Every consumer (the kernel, the
/// runtime's check of what the kernel answered) places the ROI through this
/// one function.
#[must_use]
pub fn frame_roi(roi: &Roi, spec: &ColorSpec, frame: &PixelExtent) -> Option<(Roi, Scale)> {
    let (width, height) = roi_size(roi)?;
    let (x, y) = (roi.x.unsigned_abs(), roi.y.unsigned_abs());
    let reference = (
        u64::from(spec.reference_width),
        u64::from(spec.reference_height),
    );
    let extent = (u64::from(frame.width), u64::from(frame.height));
    if reference.0 == 0 || reference.1 == 0 || extent.0 == 0 || extent.1 == 0 {
        return None;
    }
    if x + width > reference.0 || y + height > reference.1 {
        return None;
    }
    let skew = u128::from(extent.0 * reference.1).abs_diff(u128::from(extent.1 * reference.0));
    if skew > u128::from(reference.0.max(reference.1)) {
        return None;
    }
    let place = |at: u64, to: u64, from: u64| -> u64 {
        u64::try_from(u128::from(at) * u128::from(to) / u128::from(from))
            .expect("a placed coordinate fits in its extent")
    };
    let (left, right) = (
        place(x, extent.0, reference.0),
        place(x + width, extent.0, reference.0),
    );
    let (top, bottom) = (
        place(y, extent.1, reference.1),
        place(y + height, extent.1, reference.1),
    );
    if right <= left || bottom <= top {
        return None;
    }
    let common = gcd(extent.0, reference.0);
    Some((
        Roi {
            x: i64::try_from(left).ok()?,
            y: i64::try_from(top).ok()?,
            width: i64::try_from(right - left).ok()?,
            height: i64::try_from(bottom - top).ok()?,
            space: CoordinateSpace::Pixel,
        },
        Scale {
            numerator: extent.0 / common,
            denominator: reference.0 / common,
        },
    ))
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[cfg(test)]
mod tests;
