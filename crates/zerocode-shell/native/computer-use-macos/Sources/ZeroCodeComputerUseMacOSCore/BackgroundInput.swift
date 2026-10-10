import Foundation

public enum BackgroundInput {
    public static let methods: Set<String> = [
        "getAppState", "readText", "findElements", "waitFor", "click", "setValue", "scroll",
    ]

    public enum Refusal: String, Error {
        case requiresForeground = "requires_foreground"
        case invalidArgument = "invalid_argument"
    }

    public static func validate(method: String, requested: Bool, restoresWindow: Bool, hasApp: Bool) -> Refusal? {
        guard requested else { return nil }
        guard !restoresWindow else { return .invalidArgument }
        guard methods.contains(method) else { return .requiresForeground }
        return hasApp ? nil : .invalidArgument
    }

    public static func admitsTarget(_ target: Int32, foreground: Int32?) -> Bool {
        guard target > 0, let foreground, foreground > 0 else { return false }
        return target != foreground
    }

    public static func clickAction(actions: [String], textEntry: Bool, plainLeftClick: Bool) -> String? {
        guard !textEntry, plainLeftClick else { return nil }
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
