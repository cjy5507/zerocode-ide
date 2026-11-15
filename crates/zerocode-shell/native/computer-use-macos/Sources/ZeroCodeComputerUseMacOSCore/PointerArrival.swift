import CoreGraphics
import Foundation

/// Where an act's drawn pointer was sent, in which app, and when it gets
/// there. The act does not wait for it: waiting was 240 of a press's 423 ms
/// (2026-11-15), and the person's rule is speed. The press ring does wait, so
/// a person still sees the pointer arrive and then the press.
public struct PointerArrival: Equatable, Sendable {
    /// The point the pointer was sent to, in screen points.
    public let point: CGPoint
    /// The app the act is in: an agent has one pointer for each app it works in.
    public let pid: pid_t
    /// The uptime, in nanoseconds, at which the pointer stands on the point.
    public let dueNs: UInt64

    public init(point: CGPoint, pid: pid_t, dueNs: UInt64) {
        self.point = point
        self.pid = pid
        self.dueNs = dueNs
    }

    /// The arrival of a pointer that set off at `startNs` on a move of
    /// `seconds`. A pointer already on the point (no move) is there at once.
    public init(point: CGPoint, pid: pid_t, setOffNs startNs: UInt64, moveSeconds seconds: Double) {
        let move = seconds.isFinite && seconds > 0 ? UInt64((seconds * 1_000_000_000).rounded()) : 0
        self.init(point: point, pid: pid, dueNs: startNs &+ move)
    }

    /// From `nowNs`, how long until the pointer is there: nothing once it is.
    public func secondsLeft(nowNs: UInt64) -> Double {
        nowNs >= dueNs ? 0 : Double(dueNs - nowNs) / 1_000_000_000
    }
}
