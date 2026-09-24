import Foundation

/// The one table of perception limits (`zerocode_core::computer_use_protocol::game_state::LIMITS`).
/// The window sends it with the plan; the helper keeps no copy.
public struct PerceptionLimits: Codable, Equatable, Sendable {
    public let max_anchors: UInt64
    public let max_blobs: UInt64
    public let max_cells: UInt64
    public let max_classes: UInt64
    public let max_confirm: UInt64
    public let max_detector_samples: UInt64
    public let max_gate: UInt64
    public let max_lattice: UInt64
    public let max_tick_ns: UInt64
    public let max_tick_samples: UInt64
    public let max_tolerance: UInt64
    public let min_cell_samples: UInt64
}

/// The colour space a palette is written in. The kernel converts nothing: it reads only
/// frames delivered in the palette's space.
public enum PerceptionPaletteSpace: String, Codable, Sendable {
    case srgb
}

/// One colour a sample may be: every channel within `tolerance` of this one.
public struct PerceptionColorClass: Codable, Equatable, Sendable {
    public let r: UInt8
    public let g: UInt8
    public let b: UInt8
    public let tolerance: UInt8

    /// No sample can be both: on some channel the two lie further apart than their
    /// tolerances reach together.
    public func apart(_ other: PerceptionColorClass) -> Bool {
        let reach = Int(tolerance) + Int(other.tolerance)
        return [(r, other.r), (g, other.g), (b, other.b)].contains { pair in abs(Int(pair.0) - Int(pair.1)) > reach }
    }
}

extension PerceptionColorClass {
    public init(from decoder: Decoder) throws {
        let fields = try decoder.container(keyedBy: PerceptionWireKey.self)
        try fields.only(["r", "g", "b", "tolerance"])
        self.init(r: try fields.decode(UInt8.self, forKey: "r"),
                  g: try fields.decode(UInt8.self, forKey: "g"),
                  b: try fields.decode(UInt8.self, forKey: "b"),
                  tolerance: try fields.decode(UInt8.self, forKey: "tolerance"))
    }
}

/// What a cells detector answers, numbered from 1: cells in reading order, classes in
/// palette order.
public enum PerceptionReadout: Codable, Equatable, Sendable {
    /// The class of one cell.
    case cell(index: UInt64)
    /// The number of the first cell of a class; 0 when every cell is known and none is.
    case first(class: UInt64)
    /// How many cells are of a class.
    case count(class: UInt64)

    public init(from decoder: Decoder) throws {
        let fields = try decoder.container(keyedBy: PerceptionWireKey.self)
        switch try fields.decode(String.self, forKey: "op") {
        case "cell":
            try fields.only(["op", "index"])
            self = .cell(index: try fields.decode(UInt64.self, forKey: "index"))
        case "first":
            try fields.only(["op", "class"])
            self = .first(class: try fields.decode(UInt64.self, forKey: "class"))
        case "count":
            try fields.only(["op", "class"])
            self = .count(class: try fields.decode(UInt64.self, forKey: "class"))
        default:
            throw DecodingError.dataCorruptedError(forKey: "op", in: fields, debugDescription: "unknown readout")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var fields = encoder.container(keyedBy: PerceptionWireKey.self)
        switch self {
        case .cell(let index):
            try fields.encode("cell", forKey: "op")
            try fields.encode(index, forKey: "index")
        case .first(let klass):
            try fields.encode("first", forKey: "op")
            try fields.encode(klass, forKey: "class")
        case .count(let klass):
            try fields.encode("count", forKey: "op")
            try fields.encode(klass, forKey: "class")
        }
    }
}

/// `rows` × `columns` pitch boxes over the ROI; a tile of the given share of the pitch sits
/// centred in each, and `lattice` × `lattice` samples are read inside the tile less its inset.
public struct PerceptionCells: Equatable, Sendable {
    public let rows: UInt64
    public let columns: UInt64
    public let tile_width_permille: UInt64
    public let tile_height_permille: UInt64
    public let inset_permille: UInt64
    public let lattice: UInt64
    public let min_share_permille: UInt64
    public let readout: PerceptionReadout
}

/// Samples every `step` reference pixels; touching samples of `class` are one blob when there
/// are at least `min_samples` of them, and a blob keeps its track while it moves at most `gate`
/// reference pixels per capture.
public struct PerceptionBlobs: Equatable, Sendable {
    public let `class`: UInt64
    public let step: UInt64
    public let min_samples: UInt64
    public let gate: UInt64
    public let max_blobs: UInt64
}

public enum PerceptionLayout: Codable, Equatable, Sendable {
    case cells(PerceptionCells)
    case blobs(PerceptionBlobs)

    public init(from decoder: Decoder) throws {
        let fields = try decoder.container(keyedBy: PerceptionWireKey.self)
        switch try fields.decode(String.self, forKey: "kind") {
        case "cells":
            try fields.only(["kind", "rows", "columns", "tile_width_permille", "tile_height_permille",
                             "inset_permille", "lattice", "min_share_permille", "readout"])
            self = .cells(PerceptionCells(
                rows: try fields.decode(UInt64.self, forKey: "rows"),
                columns: try fields.decode(UInt64.self, forKey: "columns"),
                tile_width_permille: try fields.decode(UInt64.self, forKey: "tile_width_permille"),
                tile_height_permille: try fields.decode(UInt64.self, forKey: "tile_height_permille"),
                inset_permille: try fields.decode(UInt64.self, forKey: "inset_permille"),
                lattice: try fields.decode(UInt64.self, forKey: "lattice"),
                min_share_permille: try fields.decode(UInt64.self, forKey: "min_share_permille"),
                readout: try fields.decode(PerceptionReadout.self, forKey: "readout")))
        case "blobs":
            try fields.only(["kind", "class", "step", "min_samples", "gate", "max_blobs"])
            self = .blobs(PerceptionBlobs(
                class: try fields.decode(UInt64.self, forKey: "class"),
                step: try fields.decode(UInt64.self, forKey: "step"),
                min_samples: try fields.decode(UInt64.self, forKey: "min_samples"),
                gate: try fields.decode(UInt64.self, forKey: "gate"),
                max_blobs: try fields.decode(UInt64.self, forKey: "max_blobs")))
        default:
            throw DecodingError.dataCorruptedError(forKey: "kind", in: fields, debugDescription: "unknown layout")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var fields = encoder.container(keyedBy: PerceptionWireKey.self)
        switch self {
        case .cells(let cells):
            try fields.encode("cells", forKey: "kind")
            try fields.encode(cells.rows, forKey: "rows")
            try fields.encode(cells.columns, forKey: "columns")
            try fields.encode(cells.tile_width_permille, forKey: "tile_width_permille")
            try fields.encode(cells.tile_height_permille, forKey: "tile_height_permille")
            try fields.encode(cells.inset_permille, forKey: "inset_permille")
            try fields.encode(cells.lattice, forKey: "lattice")
            try fields.encode(cells.min_share_permille, forKey: "min_share_permille")
            try fields.encode(cells.readout, forKey: "readout")
        case .blobs(let blobs):
            try fields.encode("blobs", forKey: "kind")
            try fields.encode(blobs.class, forKey: "class")
            try fields.encode(blobs.step, forKey: "step")
            try fields.encode(blobs.min_samples, forKey: "min_samples")
            try fields.encode(blobs.gate, forKey: "gate")
            try fields.encode(blobs.max_blobs, forKey: "max_blobs")
        }
    }
}

/// A point, in reference pixels, that must show a colour for the frame to be the scene the plan was
/// written for (`game_state::Anchor`); `class` 0 names the ground.
public struct PerceptionAnchor: Codable, Equatable, Sendable {
    public let x: UInt32
    public let y: UInt32
    public let `class`: UInt64
}

extension PerceptionAnchor {
    public init(from decoder: Decoder) throws {
        let fields = try decoder.container(keyedBy: PerceptionWireKey.self)
        try fields.only(["x", "y", "class"])
        self.init(x: try fields.decode(UInt32.self, forKey: "x"), y: try fields.decode(UInt32.self, forKey: "y"),
                  class: try fields.decode(UInt64.self, forKey: "class"))
    }
}

/// A colour detector's configuration (`game_state::ColorSpec`), carried by the plan and
/// covered by its hash. The ROI it reads is the detector's, written against the reference
/// extent.
public struct PerceptionColorSpec: Codable, Equatable, Sendable {
    public let space: PerceptionPaletteSpace
    public let classes: [PerceptionColorClass]
    /// The background between cells, or behind blobs.
    public let ground: PerceptionColorClass?
    public let reference_width: UInt32
    public let reference_height: UInt32
    public let layout: PerceptionLayout
    /// Fresh captures of one scene that must agree before a value is known.
    public let confirm: UInt64
    /// Points that must all show their colour before anything is read; the wire leaves the key out
    /// when there are none, as the window's serde type does.
    public let anchors: [PerceptionAnchor]
}

extension PerceptionColorSpec {
    public init(from decoder: Decoder) throws {
        let fields = try decoder.container(keyedBy: PerceptionWireKey.self)
        try fields.only(["space", "classes", "ground", "reference_width", "reference_height", "layout", "confirm",
                         "anchors"])
        // An optional field is left out or written in full — never `null`, never an empty list —
        // so the helper reads exactly what the window's serde type reads.
        let anchors = try fields.contains("anchors") ? fields.decode([PerceptionAnchor].self, forKey: "anchors") : []
        if fields.contains("anchors") && anchors.isEmpty {
            throw DecodingError.dataCorruptedError(forKey: "anchors", in: fields, debugDescription: "empty anchors")
        }
        self.init(space: try fields.decode(PerceptionPaletteSpace.self, forKey: "space"),
                  classes: try fields.decode([PerceptionColorClass].self, forKey: "classes"),
                  ground: try fields.contains("ground") ? fields.decode(PerceptionColorClass.self, forKey: "ground") : nil,
                  reference_width: try fields.decode(UInt32.self, forKey: "reference_width"),
                  reference_height: try fields.decode(UInt32.self, forKey: "reference_height"),
                  layout: try fields.decode(PerceptionLayout.self, forKey: "layout"),
                  confirm: try fields.decode(UInt64.self, forKey: "confirm"),
                  anchors: anchors)
    }

    public func encode(to encoder: Encoder) throws {
        var fields = encoder.container(keyedBy: PerceptionWireKey.self)
        try fields.encode(space, forKey: "space")
        try fields.encode(classes, forKey: "classes")
        try fields.encodeIfPresent(ground, forKey: "ground")
        try fields.encode(reference_width, forKey: "reference_width")
        try fields.encode(reference_height, forKey: "reference_height")
        try fields.encode(layout, forKey: "layout")
        try fields.encode(confirm, forKey: "confirm")
        if !anchors.isEmpty { try fields.encode(anchors, forKey: "anchors") }
    }
}

/// The spellings the shared cases and `game_state::SpecError::code` use.
public enum PerceptionSpecError: String, Error, Sendable {
    case budget, geometry, overlap, reference, threshold, unsupported
}

/// A key of any spelling, so a decoder can refuse the fields it does not know — the
/// window's serde types deny them, and the helper must not read more permissively.
struct PerceptionWireKey: CodingKey, ExpressibleByStringLiteral {
    let stringValue: String
    var intValue: Int? { nil }

    init(stringValue: String) { self.stringValue = stringValue }
    init(stringLiteral value: String) { stringValue = value }
    init?(intValue: Int) { nil }
}

extension KeyedDecodingContainer where Key == PerceptionWireKey {
    func only(_ names: Set<String>) throws {
        if let extra = allKeys.first(where: { !names.contains($0.stringValue) }) {
            throw DecodingError.dataCorruptedError(forKey: extra, in: self, debugDescription: "unknown field")
        }
    }
}

/// One axis of a cells layout: every pitch box along it is the floor or the ceiling of the span
/// over the count, so checking both sizes checks every cell. The same arithmetic places the
/// kernel's cells.
struct PerceptionAxis {
    let span: UInt64
    let count: UInt64
    let tile_permille: UInt64
    let inset_permille: UInt64

    var pitches: [UInt64] { [span / count, (span + count - 1) / count] }

    func tile(_ pitch: UInt64) -> UInt64 { pitch * tile_permille / 1000 }

    /// The tile less its inset on both sides: where the lattice lies.
    func sampled(_ pitch: UInt64) -> UInt64 {
        let tile = tile(pitch)
        return tile - 2 * (tile * inset_permille / 1000)
    }

    /// The gaps before and after the tile inside its pitch box.
    func gaps(_ pitch: UInt64) -> (before: UInt64, after: UInt64) {
        let spare = pitch - tile(pitch)
        return (spare / 2, spare - spare / 2)
    }

    var hasGaps: Bool { tile_permille < 1000 }
}

public enum PerceptionSpecs {
    /// The palette rules every spec and every learned candidate meets (`game_state::check_palette`):
    /// 1 to `max_classes` classes, no tolerance over `max_tolerance`, and no two of them, ground
    /// included, able to hold the same sample.
    public static func checkPalette(_ classes: [PerceptionColorClass], ground: PerceptionColorClass?,
                                    limits: PerceptionLimits) throws {
        if classes.isEmpty || UInt64(classes.count) > limits.max_classes { throw PerceptionSpecError.budget }
        let palette = classes + (ground.map { [$0] } ?? [])
        if palette.contains(where: { UInt64($0.tolerance) > limits.max_tolerance }) { throw PerceptionSpecError.budget }
        for (at, item) in palette.enumerated() where palette[(at + 1)...].contains(where: { !item.apart($0) }) {
            throw PerceptionSpecError.overlap
        }
    }

    /// Checks a colour detector's spec against its ROI in the order the window does
    /// (`game_state::validate_color`) and answers how many samples one observation reads.
    public static func validate(_ spec: PerceptionColorSpec, roi: ReflexRoi, limits: PerceptionLimits) throws -> UInt64 {
        guard let (width, height) = size(roi) else { throw PerceptionSpecError.geometry }
        let referenceWidth = UInt64(spec.reference_width)
        let referenceHeight = UInt64(spec.reference_height)
        if referenceWidth == 0 || referenceHeight == 0 ||
            UInt64(roi.x) + width > referenceWidth || UInt64(roi.y) + height > referenceHeight {
            throw PerceptionSpecError.geometry
        }
        try checkPalette(spec.classes, ground: spec.ground, limits: limits)
        if spec.confirm == 0 || spec.confirm > limits.max_confirm { throw PerceptionSpecError.budget }
        let classes = UInt64(spec.classes.count)
        if UInt64(spec.anchors.count) > limits.max_anchors { throw PerceptionSpecError.budget }
        if spec.anchors.contains(where: { $0.x >= spec.reference_width || $0.y >= spec.reference_height }) {
            throw PerceptionSpecError.geometry
        }
        if spec.anchors.contains(where: { $0.class > classes || ($0.class == 0 && spec.ground == nil) }) {
            throw PerceptionSpecError.reference
        }
        let cost: UInt64
        switch spec.layout {
        case .cells(let layout):
            let (cells, cellsOverflow) = layout.rows.multipliedReportingOverflow(by: layout.columns)
            if cellsOverflow || layout.rows == 0 || layout.columns == 0 || cells > limits.max_cells {
                throw PerceptionSpecError.budget
            }
            if !(1...1000).contains(layout.tile_width_permille) || !(1...1000).contains(layout.tile_height_permille) ||
                layout.inset_permille > 499 {
                throw PerceptionSpecError.geometry
            }
            let (samples, samplesOverflow) = layout.lattice.multipliedReportingOverflow(by: layout.lattice)
            if samplesOverflow || layout.lattice == 0 || layout.lattice > limits.max_lattice ||
                samples < limits.min_cell_samples {
                throw PerceptionSpecError.budget
            }
            if !(501...1000).contains(layout.min_share_permille) { throw PerceptionSpecError.threshold }
            let inRange: Bool
            switch layout.readout {
            case .cell(let index): inRange = (1...cells).contains(index)
            case .first(let klass), .count(let klass): inRange = (1...classes).contains(klass)
            }
            if !inRange { throw PerceptionSpecError.reference }
            let axes = [
                PerceptionAxis(span: width, count: layout.columns, tile_permille: layout.tile_width_permille,
                               inset_permille: layout.inset_permille),
                PerceptionAxis(span: height, count: layout.rows, tile_permille: layout.tile_height_permille,
                               inset_permille: layout.inset_permille),
            ]
            if axes.contains(where: { axis in axis.pitches.contains { axis.sampled($0) < layout.lattice } }) {
                throw PerceptionSpecError.geometry
            }
            var gapSides: UInt64 = 0
            if spec.ground != nil {
                if axes.allSatisfy({ !$0.hasGaps }) { throw PerceptionSpecError.unsupported }
                let gapped = axes.filter(\.hasGaps)
                if gapped.contains(where: { axis in
                    axis.pitches.contains { let gaps = axis.gaps($0); return gaps.before == 0 || gaps.after == 0 }
                }) {
                    throw PerceptionSpecError.geometry
                }
                gapSides = 2 * UInt64(gapped.count)
            }
            let (gaps, gapsOverflow) = gapSides.multipliedReportingOverflow(by: layout.lattice)
            let (perCell, perCellOverflow) = gaps.addingReportingOverflow(samples)
            let (total, totalOverflow) = perCell.multipliedReportingOverflow(by: cells)
            if gapsOverflow || perCellOverflow || totalOverflow { throw PerceptionSpecError.budget }
            cost = total
        case .blobs(let layout):
            if !(1...classes).contains(layout.class) { throw PerceptionSpecError.reference }
            if layout.step == 0 || layout.step > min(width, height) { throw PerceptionSpecError.geometry }
            if layout.min_samples == 0 { throw PerceptionSpecError.threshold }
            if layout.gate == 0 || layout.gate > limits.max_gate || layout.max_blobs == 0 ||
                layout.max_blobs > limits.max_blobs {
                throw PerceptionSpecError.budget
            }
            cost = ((width + layout.step - 1) / layout.step) * ((height + layout.step - 1) / layout.step)
        }
        let (total, overflow) = cost.addingReportingOverflow(UInt64(spec.anchors.count))
        if overflow || total > limits.max_detector_samples { throw PerceptionSpecError.budget }
        return total
    }

    /// The detector's ROI placed in a frame's pixel extent, with the frame's scale over the
    /// reference (`game_state::frame_roi`). Only a uniform rescale is followed: when the width
    /// and height ratios part by more than one pixel's rounding on each axis, there is no ROI.
    public static func frameRoi(_ roi: ReflexRoi, spec: PerceptionColorSpec,
                                frame: ReflexPixelExtent) -> (roi: ReflexRoi, scale: ReflexScale)? {
        guard let (width, height) = size(roi) else { return nil }
        let x = UInt64(roi.x), y = UInt64(roi.y)
        let referenceWidth = UInt64(spec.reference_width), referenceHeight = UInt64(spec.reference_height)
        let frameWidth = UInt64(frame.width), frameHeight = UInt64(frame.height)
        guard referenceWidth > 0, referenceHeight > 0, frameWidth > 0, frameHeight > 0,
              x + width <= referenceWidth, y + height <= referenceHeight else { return nil }
        let across = frameWidth * referenceHeight, down = frameHeight * referenceWidth
        guard (across > down ? across - down : down - across) <= max(referenceWidth, referenceHeight) else { return nil }
        let left = x * frameWidth / referenceWidth, right = (x + width) * frameWidth / referenceWidth
        let top = y * frameHeight / referenceHeight, bottom = (y + height) * frameHeight / referenceHeight
        guard right > left, bottom > top else { return nil }
        let common = gcd(frameWidth, referenceWidth)
        return (ReflexRoi(x: Int64(left), y: Int64(top), width: Int64(right - left), height: Int64(bottom - top),
                          space: .pixel),
                ReflexScale(numerator: frameWidth / common, denominator: referenceWidth / common))
    }

    /// Limits as the window sends them: only the canonical form is read, so a field the helper
    /// does not know cannot be dropped silently.
    public static func decodeLimits(_ data: Data) throws -> PerceptionLimits {
        let limits = try JSONDecoder().decode(PerceptionLimits.self, from: data)
        let canonical = try JSONSerialization.data(
            withJSONObject: JSONSerialization.jsonObject(with: JSONEncoder().encode(limits)),
            options: [.sortedKeys, .withoutEscapingSlashes])
        guard canonical == data else { throw ReflexContractError.wire }
        return limits
    }

    /// A positive pixel-space ROI's width and height.
    static func size(_ roi: ReflexRoi) -> (UInt64, UInt64)? {
        guard roi.space == .pixel, roi.x >= 0, roi.y >= 0, roi.width > 0, roi.height > 0 else { return nil }
        return (UInt64(roi.width), UInt64(roi.height))
    }

    static func gcd(_ a: UInt64, _ b: UInt64) -> UInt64 {
        var (a, b) = (a, b)
        while b != 0 { (a, b) = (b, a % b) }
        return a
    }
}
