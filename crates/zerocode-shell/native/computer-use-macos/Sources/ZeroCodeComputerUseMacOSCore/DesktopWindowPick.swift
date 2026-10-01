import CoreGraphics

/// Which accessibility window of an app is the one the system numbers
/// `target` (t-15085). Many Cocoa windows publish no `AXWindowNumber`, so the
/// number alone left a window move or raise refused as "no accessibility
/// element"; where the number is missing the window is told by where it
/// stands and how large it is, and only when that tells exactly one.
public enum DesktopWindowPick {
    /// The most two frames may differ by, edge for edge and in size, and still
    /// be one window: the accessibility tree and the window list round a
    /// fractional frame differently. Points, so the same at any display scale.
    public static let frameTolerance: CGFloat = 2

    public enum Pick: Equatable {
        /// The index in the windows given.
        case found(Int)
        /// Nothing tells which window it is: no number and no frame that fits.
        case none
        /// Two or more windows without a number stand at that frame: refused,
        /// never a guess that moves the other one.
        case ambiguous
    }

    public static func framesMatch(_ lhs: CGRect, _ rhs: CGRect) -> Bool {
        abs(lhs.minX - rhs.minX) <= frameTolerance && abs(lhs.minY - rhs.minY) <= frameTolerance
            && abs(lhs.width - rhs.width) <= frameTolerance && abs(lhs.height - rhs.height) <= frameTolerance
    }

    /// A window that carries a number is found by it or is another window;
    /// only one that carries none is told by its frame against `targetBounds`
    /// (the window list's bounds of `target`, nil when not read).
    public static func pick(
        target: UInt32, targetBounds: CGRect?, windows: [(number: UInt32?, frame: CGRect?)]
    ) -> Pick {
        if let numbered = windows.firstIndex(where: { $0.number == target }) { return .found(numbered) }
        guard let targetBounds else { return .none }
        let fitting = windows.indices.filter {
            windows[$0].number == nil && windows[$0].frame.map { framesMatch($0, targetBounds) } == true
        }
        switch fitting.count {
        case 0: return .none
        case 1: return .found(fitting[0])
        default: return .ambiguous
        }
    }
}
