import CoreGraphics

/// Which accessibility window of an app is the one the system numbers
/// `target` (t-15085). Many Cocoa windows publish no `AXWindowNumber`, so the
/// number alone left a window move or raise refused as "no accessibility
/// element"; where the number is missing the window is told by where it
/// stands and how large it is, and only when that tells exactly one. Two
/// that stand at one frame — two full-size windows of a browser, stacked —
/// are told apart by the window-server number the accessibility library
/// links each to, then by what they are called, when exactly one of them is
/// called what the window list calls the target; never by which comes first.
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
        /// Two or more windows without a number stand at that frame and their
        /// names do not tell which is the target: refused, never a guess that
        /// reads or moves the other one.
        case ambiguous
    }

    public static func framesMatch(_ lhs: CGRect, _ rhs: CGRect) -> Bool {
        abs(lhs.minX - rhs.minX) <= frameTolerance && abs(lhs.minY - rhs.minY) <= frameTolerance
            && abs(lhs.width - rhs.width) <= frameTolerance && abs(lhs.height - rhs.height) <= frameTolerance
    }

    /// A window that carries a number is found by it or is another window;
    /// only one that carries none is told by its frame against `targetBounds`
    /// (the window list's bounds of `target`, nil when not read). When more
    /// than one fits, `linked` — the window-server number the accessibility
    /// library links the window at an index to — decides, if every one of
    /// them has a link, exactly one is linked to `target`, and no name points
    /// at another; else
    /// `targetTitle` — the window list's name for `target` — if exactly one
    /// of them carries it. Neither is asked for anything else, so a window
    /// that was renamed since is still found alone at its frame, and a pick
    /// with no shared frame is what it was before the link was asked
    /// (CU slowness 11-20, t-121376: two Chrome windows at one frame with
    /// one title were refused for every act that named either).
    public static func pick(
        target: UInt32, targetBounds: CGRect?, targetTitle: String? = nil,
        windows: [(number: UInt32?, frame: CGRect?, title: String?)],
        linked: (Int) -> UInt32? = { _ in nil }
    ) -> Pick {
        if let numbered = windows.firstIndex(where: { $0.number == target }) { return .found(numbered) }
        guard let targetBounds else { return .none }
        let fitting = windows.indices.filter {
            windows[$0].number == nil && windows[$0].frame.map { framesMatch($0, targetBounds) } == true
        }
        switch fitting.count {
        case 0: return .none
        case 1: return .found(fitting[0])
        default:
            let named = targetTitle?.isEmpty == false ? fitting.filter { windows[$0].title == targetTitle } : []
            // A link decides only when every window at the frame has one: a
            // twin whose link is not known could not be told from the
            // target afterwards either, when keys are sent (t-121748).
            let links = fitting.map(linked)
            let byLink = links.contains(nil) ? [] : zip(fitting, links).filter { $0.1 == target }.map { $0.0 }
            if byLink.count == 1 {
                // The link is the window server's own; a name that points at
                // another of them contradicts it, and nothing is guessed.
                return named.count == 1 && named[0] != byLink[0] ? .ambiguous : .found(byLink[0])
            }
            return named.count == 1 ? .found(named[0]) : .ambiguous
        }
    }

    /// Whether a window standing at `frame`, whose window-server number is
    /// `number` (its own, else the one the accessibility library links it
    /// to; nil when neither says), is a twin of `target` and not `target`:
    /// at the target's very frame, and known to be another window. A proof
    /// that reads frames cannot tell such a twin from the target, so it asks
    /// this first; a number that is not known leaves the frames to decide as
    /// they did (CU slowness 11-20, t-121376).
    public static func isAnotherAtTheSameFrame(frame: CGRect, number: UInt32?, target: UInt32, targetBounds: CGRect) -> Bool {
        guard let number, framesMatch(frame, targetBounds) else { return false }
        return number != target
    }

    /// How much of the smaller of two frames must lie inside the other for a
    /// window to cover where a target stands.
    public static let coverShare: CGFloat = 0.75

    /// Whether `frame` covers most of `target`, or `target` most of `frame`.
    public static func covers(_ frame: CGRect, _ target: CGRect) -> Bool {
        let shared = frame.intersection(target)
        guard !shared.isNull else { return false }
        return area(shared) >= min(area(frame), area(target)) * coverShare
    }

    private static func area(_ rect: CGRect) -> CGFloat {
        max(rect.width, 0) * max(rect.height, 0)
    }

    /// The window that holds a target none of them is — a sheet's, a
    /// popover's or a menu's parent: the one that covers where the target
    /// stands. Several are told apart as frames are, by the target's name
    /// when exactly one of them carries it, then by which one the app is
    /// focused in; never by which comes first.
    public static func pickCovering(
        targetBounds: CGRect, targetTitle: String?,
        windows: [(frame: CGRect?, title: String?, focused: Bool)]
    ) -> Pick {
        let covering = windows.indices.filter { windows[$0].frame.map { covers($0, targetBounds) } == true }
        switch covering.count {
        case 0: return .none
        case 1: return .found(covering[0])
        default:
            if let targetTitle, !targetTitle.isEmpty {
                let named = covering.filter { windows[$0].title == targetTitle }
                if named.count == 1 { return .found(named[0]) }
            }
            let focused = covering.filter { windows[$0].focused }
            return focused.count == 1 ? .found(focused[0]) : .ambiguous
        }
    }

    /// What a refusal says of a window that cannot be told from another: what
    /// stands there and what can be done about it — never "no such window",
    /// which sends its reader looking for a permission that is not missing.
    public static func ambiguousSaid(_ target: UInt32) -> String {
        "window \(target) stands exactly where another window of the same app stands, and the app tells them apart by neither a window number nor a different title: move or resize one of them (or ask the person to), then list the windows again"
    }

    /// What a refusal says of a target that is no window of its own in the
    /// app's list while several windows cover where it stands and neither a
    /// name nor the focus tells which holds it.
    public static func coveredSaid(_ target: UInt32) -> String {
        "window \(target) is not one of the app's own accessibility windows, and more than one of them covers where it stands with neither a name nor the keyboard focus to tell which holds it: look at the app without a window selector, or focus the window that holds it and look again"
    }
}
