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

    /// A hand's reach to `end`: the minimum-jerk pace (fast off the mark,
    /// settling smoothly on the point, the standard model of a person's arm)
    /// along a gentle arc. `bow` is how far the arc's middle stands off the
    /// straight line, as a share of the distance (its sign picks the side),
    /// never more than `maxBowPoints`. The last point is exactly `end`.
    public static func handPoints(from start: Point, to end: Point, steps: Int, bow: Double) -> [Point] {
        let count = max(1, steps)
        let dx = end.x - start.x
        let dy = end.y - start.y
        let distance = (dx * dx + dy * dy).squareRoot()
        let offset = distance > 0 ? max(-maxBowPoints, min(maxBowPoints, bow * distance)) : 0
        // The quadratic's control point: the midpoint pushed off along the
        // normal, twice as far as the arc's own middle stands.
        let control = distance > 0
            ? Point(x: (start.x + end.x) / 2 - dy / distance * offset * 2,
                    y: (start.y + end.y) / 2 + dx / distance * offset * 2)
            : start
        return (1...count).map { step in
            if step == count { return end }
            let t = handShare(Double(step) / Double(count))
            let u = 1 - t
            return Point(x: u * u * start.x + 2 * u * t * control.x + t * t * end.x,
                         y: u * u * start.y + 2 * u * t * control.y + t * t * end.y)
        }
    }

    /// The minimum-jerk share of the way at `fraction` of the time.
    public static func handShare(_ fraction: Double) -> Double {
        let t = min(max(fraction, 0), 1)
        return t * t * t * (10 - 15 * t + 6 * t * t)
    }

    /// The farthest an arc's middle stands off its straight line, in points.
    public static let maxBowPoints = 48.0

    /// How long a hand takes to reach a point `distance` points away: longer
    /// for a longer reach, by less and less (Fitts), within agent-desktop's
    /// 90-320 ms travel. The act waits for it, so it stays short.
    public static func glideSeconds(distance: Double) -> Double {
        let reach = minGlideSeconds + glideSecondsPerDoubling * log2(1 + max(0, distance) / glideTargetPoints)
        return min(maxGlideSeconds, reach)
    }

    public static let minGlideSeconds = 0.09
    public static let maxGlideSeconds = 0.32
    public static let glideSecondsPerDoubling = 0.06
    /// The size of what a hand aims at, in Fitts' sense: a control's width.
    public static let glideTargetPoints = 24.0
    /// Closer than this the pointer already stands where the act lands (a
    /// fallback acting on the same point): it does not move, and the act does
    /// not wait.
    public static let arrivedPoints = 0.5

    /// How waiting out a move ended.
    public enum Arrival: Equatable, Sendable { case arrived, stopped }

    /// Waits until `dueNs`, looking for the person's stop every `pollNs` and
    /// once more when the time is up, so a stop that comes during the last
    /// sleep still sends nothing.
    public static func waitOut(
        dueNs: UInt64, pollNs: UInt64,
        nowNs: () -> UInt64, isStopped: () -> Bool, sleepNs: (UInt64) -> Void
    ) -> Arrival {
        while true {
            if isStopped() { return .stopped }
            let now = nowNs()
            guard now < dueNs else { return .arrived }
            sleepNs(min(max(1, pollNs), dueNs - now))
        }
    }
}
