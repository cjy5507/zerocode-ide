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

/// The pointer's own picture on the window list (t-12979): where the window
/// server draws the cursor as a window of its own, it lists it first, under
/// the pointer — read 09-30 on a Mac with a rotated display: owner
/// "Window Server", the cursor's level, 23×22 and 28×40. Known by all three
/// facts, never by one: a bigger window at that level, or another app's, is
/// a window all the same. The window's `is_pointer_picture`
/// (crates/zerocode-core/src/computer_use_protocol/pointer.rs) judges the
/// same, held to one table (crates/zerocode-core/fixtures/pointer-picture/
/// examples.json).
public enum DesktopPointerPicture {
    /// The window server's name as the window list gives an owner
    /// (`kCGWindowOwnerName`), as read: its process is `WindowServer`.
    public static let ownerName = "Window Server"
    /// The largest picture read at the normal pointer size, 28×40, at the
    /// largest size the pointer takes, four times.
    public static let maxSidePoints: CGFloat = 160

    public static func isPointerPicture(
        layer: Int, ownerName: String, bounds: CGRect, cursorLevel: Int = Int(CGWindowLevelForKey(.cursorWindow))
    ) -> Bool {
        layer == cursorLevel && ownerName == Self.ownerName
            && bounds.width <= maxSidePoints && bounds.height <= maxSidePoints
    }
}

/// Whose window a press at `point` lands on, `infos` front to back as
/// `CGWindowListCopyWindowInfo` answers: the frontmost at or above the
/// document layer that is seen and holds the point — an overlay answered by
/// what takes input there (`overlayHit`, looking behind it when nothing
/// does), and the pointer's own picture passed over (t-12979).
public enum DesktopFrontOwner {
    public static func pid(
        at point: CGPoint, infos: [[String: Any]], displays: [CGRect],
        cursorLevel: Int = Int(CGWindowLevelForKey(.cursorWindow)), overlayHit: (CGPoint) -> pid_t?
    ) -> pid_t? {
        for info in infos {
            guard let layer = info[kCGWindowLayer as String] as? Int, layer >= 0,
                  let alpha = info[kCGWindowAlpha as String] as? CGFloat, alpha > 0.01,
                  let boundsDictionary = info[kCGWindowBounds as String] as? NSDictionary,
                  let bounds = CGRect(dictionaryRepresentation: boundsDictionary),
                  bounds.contains(point),
                  let ownerPid = info[kCGWindowOwnerPID as String] as? pid_t
            else { continue }
            let ownerName = (info[kCGWindowOwnerName as String] as? String) ?? ""
            if DesktopPointerPicture.isPointerPicture(layer: layer, ownerName: ownerName, bounds: bounds, cursorLevel: cursorLevel) {
                continue
            }
            if DesktopOverlay.isOverlay(layer: layer, bounds: bounds, displays: displays) {
                // Its background is transparent but its icons receive input.
                if let actual = overlayHit(point) { return actual }
                continue
            }
            return ownerPid
        }
        return nil
    }
}
