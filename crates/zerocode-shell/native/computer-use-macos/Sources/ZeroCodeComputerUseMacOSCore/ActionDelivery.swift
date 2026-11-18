import Foundation

/// Whether an act reached the app, and so whether it may be repeated. A repeat
/// is safe only when nothing was delivered: an act that may have landed can
/// click, type or send twice.
public enum ActionDelivery: String, Equatable, Sendable {
    /// Nothing reached the app. Repeating is safe.
    case notDelivered = "not_delivered"
    /// It was sent and the app took it, but no effect was read back.
    case deliveredUnverified = "delivered_unverified"
    /// It was sent and its effect was read back.
    case deliveredVerified = "delivered_verified"
    /// It was sent and the answer never came, so it may or may not have landed.
    case uncertain

    public var retry: String { self == .notDelivered ? "safe" : "unsafe" }

    /// What a finished act is, from its own read-back and from what the look
    /// after it saw change.
    public static func finished(readBackVerified: Bool, effectSeen: Bool) -> ActionDelivery {
        readBackVerified || effectSeen ? .deliveredVerified : .deliveredUnverified
    }

    /// What a refused or failed act is, from whether anything was sent first.
    public static func failed(anythingSent: Bool) -> ActionDelivery {
        anythingSent ? .uncertain : .notDelivered
    }
}

/// What the accessibility API said about an action, sorted by what it means for
/// delivery. A timeout or a generic failure from a press does not mean the press
/// did not happen: a button that opens a modal answers late, so a second road
/// tried after one of these can press twice.
public enum AXActionVerdict: Equatable, Sendable {
    case done
    /// The element does not offer this. Nothing happened; another road may go.
    case unsupported
    /// The action may have landed. Nothing else may be tried.
    case uncertain
    /// The element is gone. Nothing happened.
    case stale
    /// Accessibility is off for the helper. Nothing happened.
    case denied

    /// `raw` is the `AXError` value.
    public static func of(raw: Int32) -> AXActionVerdict {
        switch raw {
        case 0: return .done
        // actionUnsupported, attributeUnsupported, notImplemented, noValue, illegalArgument
        case -25206, -25205, -25208, -25212, -25201: return .unsupported
        // invalidUIElement, invalidUIElementObserver
        case -25202, -25203: return .stale
        // apiDisabled, notEnoughPrecision excluded: nothing reached the app when the API is off
        case -25211: return .denied
        // cannotComplete, failure, and anything not known
        default: return .uncertain
        }
    }
}

/// Whether a control can be pressed now, from what was read of it just now.
public enum Actionability: Equatable, Sendable {
    case ready
    /// Wait and read again: this can change by itself.
    case wait(String)
    /// Scrolled out of what its page shows, and it offers to be scrolled back:
    /// waiting changes nothing, and the press brings it into view itself.
    case outOfView

    public static func of(enabled: Bool?, frame: CGRect?, previousFrame: CGRect?, needsFrame: Bool, scrollsIntoView: Bool = false) -> Actionability {
        if enabled == false { return .wait("disabled") }
        guard needsFrame else { return .ready }
        guard let frame, frame.width > 0, frame.height > 0 else {
            return scrollsIntoView && cutToALine(frame) ? .outOfView : .wait("has no size")
        }
        if let previousFrame, !framesAgree(previousFrame, frame) { return .wait("moving") }
        return .ready
    }

    /// A frame cut to a line: one side has length and the other none. A browser
    /// answers so for a control scrolled out of its page — its frame clipped to
    /// the edge of what the page shows (a text field 305 x 0 at the foot of a
    /// sign-in page, 2026-11-17). A frame with no side at all is a control not
    /// laid out yet, or hidden: nothing says a scroll would show it.
    public static func cutToALine(_ frame: CGRect?) -> Bool {
        guard let frame else { return false }
        return (frame.width > 0) != (frame.height > 0)
    }

    /// Whether a frame is one a person could press in.
    public static func hasSize(_ frame: CGRect?) -> Bool {
        guard let frame else { return false }
        return frame.width > 0 && frame.height > 0
    }

    /// Whether a control scrolled into view has come to rest: a size, and the
    /// same frame as the read before. A page that scrolls smoothly shows the
    /// control part way in first; a click there could land beside it.
    public static func cameToRest(_ frame: CGRect?, after previous: CGRect?) -> Bool {
        guard let frame, let previous, hasSize(frame) else { return false }
        return framesAgree(previous, frame)
    }

    /// Half a point of drift is a rounding difference between two reads, not a move.
    public static func framesAgree(_ left: CGRect, _ right: CGRect) -> Bool {
        abs(left.minX - right.minX) <= 0.5 && abs(left.minY - right.minY) <= 0.5
            && abs(left.width - right.width) <= 0.5 && abs(left.height - right.height) <= 0.5
    }

    /// How long a press waits for a control by default, and how often it reads.
    public static var defaultWaitMs: Int { 2_000 }
    /// The longest a press may be told to wait. Waiting keeps this app's other
    /// requests in line, and must end well before the window gives up on the answer.
    public static var longestWaitMs: Int { 10_000 }
    public static var pollMs: Int { 50 }
    /// How long a control asked to scroll into view is read again until it
    /// comes to rest. The scroll is the app's own, smooth on some pages, and
    /// the frame comes after it; the wait is inside the act's turn. Not
    /// measured on a real browser.
    public static var intoViewWaitMs: Int { 1_000 }
}

/// One lock per app, so a slow or hung app holds up only the requests that name
/// it. Requests in one lane stay in order; requests in different lanes run at
/// once.
public final class LaneLocks: @unchecked Sendable {
    private let lock = NSLock()
    private var lanes: [String: NSLock] = [:]

    public init() {}

    /// The lock for `lane`, made the first time it is asked for.
    public func lock(for lane: String) -> NSLock {
        lock.lock()
        defer { lock.unlock() }
        if let existing = lanes[lane] { return existing }
        let made = NSLock()
        lanes[lane] = made
        return made
    }

    public var count: Int {
        lock.lock()
        defer { lock.unlock() }
        return lanes.count
    }
}

/// Chromium and Electron build their web accessibility tree when asked to.
public enum WebAccessibility {
    /// The attribute they offer as settable. A native app does not offer it.
    public static let manualAttribute = "AXManualAccessibility"
    /// How long a look waits for the attribute to read back as set.
    public static let readySeconds: TimeInterval = 0.4
    /// How long after it is turned on a control may still have no press to offer. A press
    /// before that answers success and does nothing, so a press waits for the offer.
    public static let settlesSeconds: TimeInterval = 8
    public static let pollMicros: UInt32 = 25_000
}

/// How long one call into an app may take before it is given up on. The system
/// default is six seconds per call, and one hung app could hold a walk for far
/// longer than that.
public enum AccessibilityTimeouts {
    public static let messagingSeconds: Float = 1.5
}
