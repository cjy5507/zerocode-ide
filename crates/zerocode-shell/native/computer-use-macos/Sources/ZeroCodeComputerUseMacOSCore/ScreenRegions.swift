import CoreGraphics
import Foundation

extension CGRect {
    /// What of this rectangle `other` leaves showing: itself when the two do
    /// not overlap, nothing when `other` hides all of it, else up to four
    /// pieces that do not overlap — the bands above and below the overlap,
    /// and beside it between them (`zerocode_core` `Rect::minus`).
    public func minus(_ other: CGRect) -> [CGRect] {
        let cut = intersection(other)
        guard !cut.isNull, cut.width > 0, cut.height > 0 else { return [self] }
        return [
            CGRect(x: minX, y: minY, width: width, height: cut.minY - minY),
            CGRect(x: minX, y: cut.maxY, width: width, height: maxY - cut.maxY),
            CGRect(x: minX, y: cut.minY, width: cut.minX - minX, height: cut.height),
            CGRect(x: cut.maxX, y: cut.minY, width: maxX - cut.maxX, height: cut.height),
        ].filter { $0.width > 0 && $0.height > 0 }
    }

    /// What of this rectangle none of `others` covers.
    public func remainder(minus others: [CGRect]) -> [CGRect] {
        others.reduce([self]) { pieces, other in pieces.flatMap { $0.minus(other) } }
    }
}

/// What a desktop OCR read leaves out: the parts of the screen ZeroCode's
/// own windows show, as the window sends them (`ownRegion`,
/// `zerocode_core::computer_use_protocol::eye::own_region`). The agent's own
/// words scroll there; they are not the app's answer.
public struct LeftOut: Equatable, Sendable {
    public let rects: [CGRect]

    public init(rects: [CGRect]) {
        self.rects = rects
    }

    /// Whether a recognized line is ZeroCode's: its centre is on the region —
    /// the point a click on the line would land on, which the hands refuse.
    public func shows(_ line: CGRect) -> Bool {
        let centre = CGPoint(x: line.midX, y: line.midY)
        return rects.contains { $0.contains(centre) }
    }

    /// Whether a repaint fell wholly on the region.
    public func covers(_ rect: CGRect) -> Bool {
        !rects.isEmpty && rect.remainder(minus: rects).isEmpty
    }

    /// What of `area` is not left out.
    public func remainder(of area: CGRect) -> [CGRect] {
        area.remainder(minus: rects)
    }
}

/// A display-spanning window at the operating system's Dock level has a
/// transparent background. Its actual accessibility surfaces still cover
/// other windows; elevated windows at other levels are not assumed clear.
public enum DesktopOverlay {
    public static func isOverlay(layer: Int, bounds: CGRect, displays: [CGRect]) -> Bool {
        layer == Int(CGWindowLevelForKey(.dockWindow)) && displays.contains { bounds.contains($0) }
    }

    /// The overlay's visible accessibility surfaces still cover what is
    /// behind them. A full-display container provides no useful boundary.
    public static func coveringFrames(_ frames: [CGRect], inside bounds: CGRect) -> [CGRect] {
        frames.map { $0.intersection(bounds) }
            .filter { !$0.isNull && $0.width > 0 && $0.height > 0 && $0 != bounds }
    }
}
