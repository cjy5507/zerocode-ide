import CoreGraphics
import Foundation

/// Whether a window is on the screen the person sees, as a fact something
/// reported — never a guess: `unknown` when nothing did (t-68324 ③).
public enum WindowPresence: String, Equatable, Sendable {
    case onScreen = "on_screen"
    case offScreen = "off_screen"
    case unknown = "unknown"

    /// What an answer through the tree says of where its window is.
    public static func sentence(for presence: WindowPresence) -> String {
        switch presence {
        case .onScreen:
            return "The window server lists this window as on the screen."
        case .offScreen:
            return "This window is not on the screen (its app is hidden, or the window server lists it only as off the screen, as it does a window of another Space): the tree is read, but nothing of it is shown."
        case .unknown:
            return "Where this window is — on the screen or not — is not known: the accessibility tree does not say and the window server lists no such window."
        }
    }
}

/// One window a look or an action may be aimed at, as the window server lists
/// it (`CGWindowListCopyWindowInfo`).
public struct WindowCandidate: Equatable, Sendable {
    public let windowId: CGWindowID
    public let layer: Int
    public let bounds: CGRect
    public let title: String?
    public let alpha: CGFloat
    public let presence: WindowPresence
    public let sharingState: Int?

    /// Known to be on the screen — `unknown` is not.
    public var isOnScreen: Bool { presence == .onScreen }

    public init(
        windowId: CGWindowID,
        layer: Int,
        bounds: CGRect,
        title: String?,
        alpha: CGFloat,
        isOnScreen: Bool,
        sharingState: Int?
    ) {
        self.init(
            windowId: windowId, layer: layer, bounds: bounds, title: title, alpha: alpha,
            presence: isOnScreen ? .onScreen : .offScreen, sharingState: sharingState
        )
    }

    public init(
        windowId: CGWindowID,
        layer: Int,
        bounds: CGRect,
        title: String?,
        alpha: CGFloat,
        presence: WindowPresence,
        sharingState: Int?
    ) {
        self.windowId = windowId
        self.layer = layer
        self.bounds = bounds
        self.title = title
        self.alpha = alpha
        self.presence = presence
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
    public let appHidden: Bool
    public let serverOnScreen: Bool?

    /// Where the window is, as far as something said: a hidden app shows no
    /// window; else what the window server said of its number; else unknown.
    public var presence: WindowPresence {
        if appHidden { return .offScreen }
        switch serverOnScreen {
        case true?: return .onScreen
        case false?: return .offScreen
        case nil: return .unknown
        }
    }

    public init(
        pid: pid_t, number: CGWindowID?, frame: CGRect?, title: String?, minimized: Bool,
        appHidden: Bool = false, serverOnScreen: Bool? = nil
    ) {
        self.pid = pid
        self.number = number
        self.frame = frame
        self.title = title
        self.minimized = minimized
        self.appHidden = appHidden
        self.serverOnScreen = serverOnScreen
    }
}

/// The windows of one app a look or an action may be aimed at.
public enum WindowList {
    /// What the window server says of `pid`'s windows by number: `true` for
    /// one its on-screen list holds, `false` for one only its full list
    /// holds (another Space, a hidden app's); a number it lists in neither is
    /// absent — nothing was said of it.
    public static func serverView(onScreen: [[String: Any]], all: [[String: Any]], pid: pid_t) -> [CGWindowID: Bool] {
        func numbers(_ infos: [[String: Any]]) -> [CGWindowID] {
            infos.compactMap { info in
                guard (info[kCGWindowOwnerPID as String] as? pid_t) == pid,
                      let number = info[kCGWindowNumber as String] as? NSNumber
                else { return nil }
                return CGWindowID(number.uint32Value)
            }
        }
        var view: [CGWindowID: Bool] = [:]
        for number in numbers(all) { view[number] = false }
        for number in numbers(onScreen) { view[number] = true }
        return view
    }

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
    /// number for stands in as window 0. Each says where it is only as far as
    /// something said (`AccessibilityWindow.presence`), never that it is shown.
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
                    presence: window.presence,
                    sharingState: nil
                )
            }
            .sorted { lhs, rhs in
                lhs.score > rhs.score
            }
    }
}
