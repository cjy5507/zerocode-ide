import CoreGraphics
import Foundation

/// One window a look or an action may be aimed at, as the window server lists
/// it (`CGWindowListCopyWindowInfo`).
public struct WindowCandidate: Equatable, Sendable {
    public let windowId: CGWindowID
    public let layer: Int
    public let bounds: CGRect
    public let title: String?
    public let alpha: CGFloat
    public let isOnScreen: Bool
    public let sharingState: Int?

    public init(
        windowId: CGWindowID,
        layer: Int,
        bounds: CGRect,
        title: String?,
        alpha: CGFloat,
        isOnScreen: Bool,
        sharingState: Int?
    ) {
        self.windowId = windowId
        self.layer = layer
        self.bounds = bounds
        self.title = title
        self.alpha = alpha
        self.isOnScreen = isOnScreen
        self.sharingState = sharingState
    }

    public var score: Int {
        var value = Int(bounds.width * bounds.height)
        if layer == 0 { value += 1_000_000_000 }
        if title != nil && title?.isEmpty == false { value += 10_000_000 }
        if isOnScreen { value += 1_000_000 }
        if alpha >= 0.99 { value += 100_000 }
        return value
    }
}

/// One of an app's own windows as its accessibility tree gives it (t-66753
/// C1): the process it belongs to, where it is, its title, the window number
/// the app publishes (`AXWindowNumber`, when it does) and whether it is
/// minimized.
public struct AccessibilityWindow: Equatable, Sendable {
    public let pid: pid_t
    public let number: CGWindowID?
    public let frame: CGRect?
    public let title: String?
    public let minimized: Bool

    public init(pid: pid_t, number: CGWindowID?, frame: CGRect?, title: String?, minimized: Bool) {
        self.pid = pid
        self.number = number
        self.frame = frame
        self.title = title
        self.minimized = minimized
    }
}

/// The windows of one app a look or an action may be aimed at.
public enum WindowList {
    /// The smallest side, in points, of a window worth aiming at.
    public static let minimumSide: CGFloat = 48

    /// `pid`'s windows in `infos` — the window server's list, front to back
    /// as `CGWindowListCopyWindowInfo` answers it — the best first.
    public static func candidates(infos: [[String: Any]], pid: pid_t) -> [WindowCandidate] {
        infos.compactMap { info in
            guard let ownerPid = info[kCGWindowOwnerPID as String] as? pid_t, ownerPid == pid,
                  let number = info[kCGWindowNumber as String] as? NSNumber,
                  let layer = info[kCGWindowLayer as String] as? Int,
                  let boundsDictionary = info[kCGWindowBounds as String] as? NSDictionary,
                  let bounds = CGRect(dictionaryRepresentation: boundsDictionary),
                  bounds.width >= minimumSide,
                  bounds.height >= minimumSide
            else {
                return nil
            }
            let alpha = info[kCGWindowAlpha as String] as? CGFloat ?? 1
            guard alpha > 0.01 else { return nil }
            let sharing = (info[kCGWindowSharingState as String] as? NSNumber).map { $0.intValue }
            if sharing == 0 { return nil }
            let isOnScreen = (info[kCGWindowIsOnscreen as String] as? Bool) ?? true
            return WindowCandidate(
                windowId: CGWindowID(number.uint32Value),
                layer: layer,
                bounds: bounds,
                title: info[kCGWindowName as String] as? String,
                alpha: alpha,
                isOnScreen: isOnScreen,
                sharingState: sharing
            )
        }
        .sorted { lhs, rhs in
            lhs.score > rhs.score
        }
    }

    /// The windows a read of the app `pid` may look at (t-66753 C1): the
    /// ones the window server lists, else — when it lists none of the app's,
    /// as it may without Screen Recording — the app's own accessibility
    /// windows, asked for only then: that process's alone, seen and big
    /// enough, the best first. They stand in for a read, never for an action,
    /// whose checks ask the window server; a window the app publishes no
    /// number for stands in as window 0.
    public static func forReading(
        listed: [WindowCandidate],
        pid: pid_t,
        accessibility: () -> [AccessibilityWindow]
    ) -> [WindowCandidate] {
        guard listed.isEmpty else { return listed }
        return accessibility()
            .filter { $0.pid == pid && !$0.minimized }
            .compactMap { window in
                guard let frame = window.frame, frame.width >= minimumSide, frame.height >= minimumSide else {
                    return nil
                }
                return WindowCandidate(
                    windowId: window.number ?? 0,
                    layer: 0,
                    bounds: frame,
                    title: window.title,
                    alpha: 1,
                    isOnScreen: true,
                    sharingState: nil
                )
            }
            .sorted { lhs, rhs in
                lhs.score > rhs.score
            }
    }
}
