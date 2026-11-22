import Foundation

/// Facts that can prevent a background press. A failed AX read cannot
/// prove a control enabled, a submenu empty, or a shortcut absent.
public enum PressCertainty {
    public enum Reading<Value> {
        case read(Value)
        case unread
    }

    public static func value<Value>(_ reading: Reading<Value?>) -> Value? {
        if case let .read(value) = reading { return value }
        return nil
    }

    public enum Refusal: Equatable, Error {
        case targetChanged
        case searchUnfinished
        case disabled
        case enabledUnread
        case opensMenu
        case childrenUnread
        case observedWindowChanged
        case windowUnbound
        case personChanged
    }

    public struct ApplicationInstance: Equatable {
        public let pid: Int32
        public let started: UInt64
        public let location: URL

        public init(pid: Int32, started: UInt64, location: URL) {
            self.pid = pid
            self.started = started
            self.location = location
        }
    }

    /// Keep AX identity across the person's approval. A process ID alone can
    /// be reused, and the same title can appear on multiple AX elements.
    public struct Target<Identity> {
        public let application: ApplicationInstance
        public let window: Identity
        public let item: Identity
        public let defaultButton: Identity?
        public let label: String

        public init(application: ApplicationInstance, window: Identity, item: Identity, defaultButton: Identity? = nil, label: String) {
            self.application = application
            self.window = window
            self.item = item
            self.defaultButton = defaultButton
            self.label = label
        }

        public func matches(_ other: Self?, same: (Identity, Identity) -> Bool) -> Bool {
            guard let other, application == other.application, label == other.label,
                  same(window, other.window), same(item, other.item) else { return false }
            switch (defaultButton, other.defaultButton) {
            case (nil, nil): return true
            case let (left?, right?): return same(left, right)
            default: return false
            }
        }
    }

    public static func requireTarget<Identity>(_ expected: Target<Identity>, actual: Target<Identity>?, same: (Identity, Identity) -> Bool) throws {
        guard expected.matches(actual, same: same) else { throw Refusal.targetChanged }
    }

    public static func requireCompleteSearch(_ finished: Bool) throws {
        guard finished else { throw Refusal.searchUnfinished }
    }

    public static func requireEnabled(_ enabled: Reading<Bool?>, mustAnswer: Bool = true) throws {
        switch enabled {
        case .read(true?): return
        case .read(false?): throw Refusal.disabled
        case .read(nil) where !mustAnswer: return
        case .read(nil), .unread: throw Refusal.enabledUnread
        }
    }

    public static func requireClosedCommand<Child>(role: String, action: String, children: () -> [Child]?) throws {
        if action == "AXCancel" { return }
        if MenuBarCommand.menuOpenerRoles.contains(role) || action == "AXShowMenu" { throw Refusal.opensMenu }
        guard role == "AXMenuItem" else { return }
        guard let children = children() else { throw Refusal.childrenUnread }
        guard children.isEmpty else { throw Refusal.opensMenu }
    }

    public static func requireWindow<Identity>(
        readFor: Identity?, target: Identity, destinations: () -> (focused: Identity?, main: Identity?),
        same: (Identity, Identity) -> Bool
    ) throws {
        guard let readFor, same(readFor, target) else { throw Refusal.observedWindowChanged }
        let (focused, main) = destinations()
        guard MenuBarCommand.boundTo(target, focused: focused, main: main, same: same) else { throw Refusal.windowUnbound }
    }

    public static func requireElementWindow<Identity>(
        readFor: Identity?, target: Identity, targetPID: Int32, elementPID: () -> Int32?,
        onSurface: Bool, menuWindows: (() -> (focused: Identity?, main: Identity?))?,
        containingWindow: () -> Identity?, same: (Identity, Identity) -> Bool,
        sheetParent: (Identity) -> Identity?
    ) throws {
        // A remote renderer can answer for a control inside the app's window.
        // Menus and surface elements lack the containing-window check.
        if onSurface || menuWindows != nil {
            guard elementPID() == targetPID else { throw Refusal.targetChanged }
        }
        if let menuWindows {
            try requireWindow(readFor: readFor, target: target, destinations: menuWindows, same: same)
        } else {
            guard let readFor, same(readFor, target) else { throw Refusal.observedWindowChanged }
            if !onSurface {
                try requireContainingWindow(containingWindow(), target: target, same: same, sheetParent: sheetParent)
            }
        }
    }

    /// AX can name a sheet as the window of a control inside it. The
    /// sheet must still be attached to the window we read.
    public static func requireContainingWindow<Identity>(
        _ window: Identity?, target: Identity, same: (Identity, Identity) -> Bool,
        sheetParent: (Identity) -> Identity?
    ) throws {
        guard let window else { throw Refusal.observedWindowChanged }
        if same(window, target) { return }
        guard let parent = sheetParent(window), same(parent, target) else {
            throw Refusal.observedWindowChanged
        }
    }

    public struct Person {
        public let foreground: Int32
        public let events: [UInt32]

        public init(foreground: Int32, events: [UInt32]) {
            self.foreground = foreground
            self.events = events
        }

        public func requireUnchanged(_ current: Self?, targetPID: Int32) throws {
            guard let current, current.foreground == foreground else { throw Refusal.personChanged }
            // HID counters are system-wide. A change matters only while
            // the target app is in front.
            if foreground == targetPID, current.events != events { throw Refusal.personChanged }
        }
    }
}
