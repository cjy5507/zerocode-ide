import Foundation

/// The points a human-paced pointer move passes through: eased in and out so
/// it starts and settles like a hand, not a teleport. Pure, so the schedule is
/// tested without posting an event. `steps` of 1 is the plain jump.
public enum SmoothPointerPath {
    public struct Point: Equatable, Sendable {
        public let x: Double
        public let y: Double
        public init(x: Double, y: Double) { self.x = x; self.y = y }
    }

    /// Ease-in-out share of the way for step `i` of `count`.
    public static func share(step: Int, of count: Int) -> Double {
        let t = Double(step) / Double(max(1, count))
        return 0.5 - 0.5 * cos(Double.pi * t)
    }

    /// Every intermediate point from `start` to `end`, ending exactly on `end`.
    public static func points(from start: Point, to end: Point, steps: Int) -> [Point] {
        let count = max(1, steps)
        return (1...count).map { step in
            let e = share(step: step, of: count)
            return Point(x: start.x + (end.x - start.x) * e, y: start.y + (end.y - start.y) * e)
        }
    }
}
