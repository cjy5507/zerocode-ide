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

    /// The same eased walk along a gentle arc, as a hand moves: `bow` is how far
    /// the arc's middle stands off the straight line, as a share of the
    /// distance (its sign picks the side), and the arc never stands off more
    /// than `maxBowPoints`. It still ends exactly on `end`.
    public static func points(from start: Point, to end: Point, steps: Int, bow: Double) -> [Point] {
        let count = max(1, steps)
        let dx = end.x - start.x
        let dy = end.y - start.y
        let distance = (dx * dx + dy * dy).squareRoot()
        let offset = distance > 0 ? max(-maxBowPoints, min(maxBowPoints, bow * distance)) : 0
        // The quadratic's control point: the midpoint pushed off along the normal,
        // twice as far as the arc's own middle stands.
        let control = distance > 0
            ? Point(x: (start.x + end.x) / 2 - dy / distance * offset * 2,
                    y: (start.y + end.y) / 2 + dx / distance * offset * 2)
            : start
        return (1...count).map { step in
            let t = share(step: step, of: count)
            if step == count { return end }
            let u = 1 - t
            return Point(x: u * u * start.x + 2 * u * t * control.x + t * t * end.x,
                         y: u * u * start.y + 2 * u * t * control.y + t * t * end.y)
        }
    }

    /// The farthest an arc's middle stands off its straight line, in points.
    public static let maxBowPoints = 48.0

    /// How long a pointer people watch takes to cross `distance` points: quick
    /// for a short hop, never a crawl for a long one (agent-desktop's travel is
    /// 90-320 ms). Nothing waits for it: it is only what a person sees.
    public static func glideSeconds(distance: Double) -> Double {
        min(maxGlideSeconds, max(minGlideSeconds, minGlideSeconds + max(0, distance) / glidePointsPerSecond))
    }

    public static let minGlideSeconds = 0.12
    public static let maxGlideSeconds = 0.36
    public static let glidePointsPerSecond = 2_400.0
}
