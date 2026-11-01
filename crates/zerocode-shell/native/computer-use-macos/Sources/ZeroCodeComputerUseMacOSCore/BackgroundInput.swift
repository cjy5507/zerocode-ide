import Foundation

public enum BackgroundInput {
    public enum Focus: Equatable {
        case none
        case one(Int)
        case ambiguous
    }

    public static func focus<Indices: Sequence>(among indices: Indices) -> Focus where Indices.Element == Int {
        var chosen: Int?
        for index in indices {
            if let chosen, chosen != index { return .ambiguous }
            chosen = index
        }
        return chosen.map(Focus.one) ?? .none
    }

    public static let methods: Set<String> = [
        "getAppState", "readText", "findElements", "waitFor", "click", "setValue", "scroll",
        "typeText", "pressKey", "hotkey", "pasteText", "launchApp", "openTarget",
    ]

    public enum Refusal: String, Error {
        case requiresForeground = "requires_foreground"
        case invalidArgument = "invalid_argument"
    }

    public static func validate(method: String, requested: Bool, restoresWindow: Bool, hasApp: Bool) -> Refusal? {
        guard requested else { return nil }
        guard !restoresWindow else { return .invalidArgument }
        guard methods.contains(method) else { return .requiresForeground }
        // Opening a page or a file names no app: it comes up behind, whichever opens it.
        return hasApp || method == "openTarget" ? nil : .invalidArgument
    }

    /// Whether the shared desktop can do what the background road just refused.
    /// A field that cannot be written is the same field with the app in front.
    public static func foregroundMayTakeOver(method: String, refusal: String) -> Bool {
        switch refusal {
        case "requires_foreground", "focus_changed":
            return true
        case "value_not_settable":
            return method == "typeText" || method == "pasteText"
        default:
            return false
        }
    }

    public static func admitsTarget(_ target: Int32, foreground: Int32?) -> Bool {
        guard target > 0, let foreground, foreground > 0 else { return false }
        return target != foreground
    }

    /// The roles whose controls a web view comes to offer a press on.
    public static let pressableRoles: Set<String> = [
        "AXButton", "AXLink", "AXCheckBox", "AXRadioButton", "AXMenuItem", "AXTab", "AXPopUpButton",
    ]

    public static func clickAction(actions: [String], textEntry: Bool, plainLeftClick: Bool) -> String? {
        guard plainLeftClick else { return nil }
        if textEntry { return actions.contains("AXPress") ? "AXPress" : nil }
        return ["AXPress", "AXOpen", "AXConfirm"].first(where: actions.contains)
    }

    public struct Scroller {
        public let index: Int
        public let frame: CGRect

        public init(index: Int, frame: CGRect) {
            self.index = index
            self.frame = frame
        }
    }

    public static func scrollTarget(at point: CGPoint, among candidates: [Scroller]) -> Int? {
        guard point.x.isFinite, point.y.isFinite else { return nil }
        let eligible = candidates.filter {
            $0.frame.width.isFinite && $0.frame.height.isFinite
                && $0.frame.size.width > 0 && $0.frame.size.height > 0
                && $0.frame.contains(point)
        }.sorted {
            $0.frame.width * $0.frame.height < $1.frame.width * $1.frame.height
        }
        guard let first = eligible.first else { return nil }
        if eligible.dropFirst().contains(where: { !$0.frame.contains(first.frame) || $0.frame == first.frame }) {
            return nil
        }
        return first.index
    }
}
