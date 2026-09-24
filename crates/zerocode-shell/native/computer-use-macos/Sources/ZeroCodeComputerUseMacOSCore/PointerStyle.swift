import Foundation

/// How a pointer gets to where it acts (docs/design/computer-use-full-operator.md
/// §1.3, realtime v1 §5.7): along a visible path, eased or straight, over the
/// plan's duration — `instant` is the explicit exception, one waypoint and no
/// glide. The style is the plan's (`ReflexPointerStyle`) held to the window's
/// reflex table (`ReflexLimits`); the helper keeps no number of its own.
public struct PointerStyle: Equatable, Sendable {
    public enum Curve: Equatable, Sendable {
        case linear
        case cosine
    }

    public let durationNs: UInt64
    public let curve: Curve
    public let instant: Bool

    /// The plan's style under the window's table: the bounds
    /// `ReflexContract.validate` holds a plan to, read again here because a
    /// style is also built for a single run the plan did not validate.
    public init(_ style: ReflexPointerStyle, limits: ReflexLimits) throws {
        guard style.duration_ms > 0, style.duration_ms <= limits.max_pointer_duration_ms,
              !style.instant || style.duration_ms == limits.instant_duration_ms
        else { throw ReflexContractError.budget }
        let nanoseconds = style.duration_ms.multipliedReportingOverflow(by: 1_000_000)
        guard !nanoseconds.overflow else { throw ReflexContractError.budget }
        durationNs = nanoseconds.partialValue
        curve = style.curve == .cosine ? .cosine : .linear
        instant = style.instant
    }

    /// The share of the way a path has come at `fraction` of its time.
    public func share(_ fraction: Double) -> Double {
        let t = min(max(fraction, 0), 1)
        switch curve {
        case .linear: return t
        case .cosine: return 0.5 - 0.5 * cos(Double.pi * t)
        }
    }
}

/// One point a pointer passes, and when it is due on the host clock.
public struct PointerWaypoint: Equatable, Sendable {
    public let x: Double
    public let y: Double
    public let dueNs: UInt64

    public init(x: Double, y: Double, dueNs: UInt64) {
        self.x = x
        self.y = y
        self.dueNs = dueNs
    }
}

/// The due-time contract both a plan's glide and today's stepped moves keep:
/// every waypoint has a time, a late hand posts the newest one due instead of
/// catching up on the ones it missed, and the last lands exactly on the end.
public enum PointerSchedule {
    /// A glide from `start` to `end` beginning at `startNs`: one waypoint per
    /// `tickNs` of the style's duration, the last exactly on `end` at
    /// `startNs + durationNs`. A pointer already there has no waypoint at all —
    /// it does not wait out a path to nowhere — and an instant style is one
    /// waypoint due at once. `tickNs` is the window's (`pointer_tick_ns`), and
    /// the table keeps `max_pointer_duration_ms / pointer_tick_ns` inside a
    /// lease's children (`ReflexTable.check`), so the count stays bounded.
    public static func glide(
        from start: SmoothPointerPath.Point,
        to end: SmoothPointerPath.Point,
        style: PointerStyle,
        tickNs: UInt64,
        startNs: UInt64
    ) -> [PointerWaypoint] {
        if start == end { return [] }
        if style.instant || tickNs == 0 || style.durationNs <= tickNs {
            let due = style.instant ? startNs : startNs &+ style.durationNs
            return [PointerWaypoint(x: end.x, y: end.y, dueNs: due)]
        }
        let (whole, part) = style.durationNs.quotientAndRemainder(dividingBy: tickNs)
        let count = whole + (part > 0 ? 1 : 0)
        return (1...count).map { index in
            let last = index == count
            let share = style.share(Double(index) / Double(count))
            return PointerWaypoint(
                x: last ? end.x : start.x + (end.x - start.x) * share,
                y: last ? end.y : start.y + (end.y - start.y) * share,
                dueNs: startNs &+ style.durationNs * index / count
            )
        }
    }

    /// Today's stepped move (`mouseMove --steps`, a desktop drag): `count`
    /// eased or straight points, the first at once and each next `stepNs`
    /// after the one before — the baseline the plan's glide is measured
    /// against. A count under one is the plain jump.
    public static func steps(
        from start: SmoothPointerPath.Point,
        to end: SmoothPointerPath.Point,
        count: Int,
        eased: Bool,
        stepNs: UInt64,
        startNs: UInt64
    ) -> [PointerWaypoint] {
        let total = max(1, count)
        return (1...total).map { index in
            let fraction = Double(index) / Double(total)
            let share = eased ? SmoothPointerPath.share(step: index, of: total) : fraction
            return PointerWaypoint(
                x: index == total ? end.x : start.x + (end.x - start.x) * share,
                y: index == total ? end.y : start.y + (end.y - start.y) * share,
                dueNs: startNs &+ UInt64(index - 1) &* stepNs
            )
        }
    }

    /// The waypoint to post now after `posted` (the index of the last one
    /// posted, -1 before the first): the newest one due, so a hand woken late
    /// skips to where it should be rather than bursting the missed ones. Nil
    /// when the next is not due yet.
    public static func due(_ path: [PointerWaypoint], after posted: Int, nowNs: UInt64) -> Int? {
        var found: Int?
        var index = posted + 1
        while index < path.count, path[index].dueNs <= nowNs {
            found = index
            index += 1
        }
        return found
    }
}
