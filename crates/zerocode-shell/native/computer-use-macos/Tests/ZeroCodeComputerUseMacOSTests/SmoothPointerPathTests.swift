import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class SmoothPointerPathTests: XCTestCase {
    func testAStepIsAPlainJumpToTheTarget() {
        let pts = SmoothPointerPath.points(from: .init(x: 0, y: 0), to: .init(x: 100, y: 50), steps: 1)
        XCTAssertEqual(pts, [.init(x: 100, y: 50)])
    }

    func testAStepPathEndsExactlyOnTheTargetAndEasesInAndOut() {
        let pts = SmoothPointerPath.points(from: .init(x: 0, y: 0), to: .init(x: 100, y: 0), steps: 10)
        XCTAssertEqual(pts.count, 10)
        XCTAssertEqual(pts.last, .init(x: 100, y: 0))
        // Eased: the first step moves less than a linear tenth, the middle step about half.
        XCTAssertLessThan(pts[0].x, 10)
        XCTAssertEqual(pts[4].x, 50, accuracy: 0.001)
        // Monotone: no backtracking along the way.
        for (a, b) in zip(pts, pts.dropFirst()) { XCTAssertLessThanOrEqual(a.x, b.x) }
    }

    func testAHandsReachArcsByItsBowAndEndsOnTheTarget() {
        let start = SmoothPointerPath.Point(x: 0, y: 0)
        let end = SmoothPointerPath.Point(x: 200, y: 0)
        let arc = SmoothPointerPath.handPoints(from: start, to: end, steps: 20, bow: 0.1)
        XCTAssertEqual(arc.last, end)
        // Halfway in time is halfway along, and the middle stands a tenth of the
        // distance off the line, on the bow's side.
        XCTAssertEqual(arc[9].x, 100, accuracy: 0.001)
        XCTAssertEqual(arc[9].y, 20, accuracy: 0.001)
        XCTAssertLessThan(SmoothPointerPath.handPoints(from: start, to: end, steps: 20, bow: -0.1)[9].y, -19)
        // A long reach stands off no more than the cap.
        let far = SmoothPointerPath.handPoints(from: start, to: .init(x: 2_000, y: 0), steps: 20, bow: 0.1)
        XCTAssertEqual(far[9].y, SmoothPointerPath.maxBowPoints, accuracy: 0.001)
        XCTAssertEqual(SmoothPointerPath.handPoints(from: start, to: start, steps: 5, bow: 0.1).last, start)
        // Never back the way it came.
        for (a, b) in zip(arc, arc.dropFirst()) { XCTAssertLessThanOrEqual(a.x, b.x + 1e-9) }
    }

    func testAHandStartsAndSettlesSlowerThanItMovesInTheMiddle() {
        XCTAssertEqual(SmoothPointerPath.handShare(0), 0)
        XCTAssertEqual(SmoothPointerPath.handShare(1), 1)
        XCTAssertEqual(SmoothPointerPath.handShare(0.5), 0.5, accuracy: 1e-12)
        XCTAssertLessThan(SmoothPointerPath.handShare(0.1), 0.01)
        XCTAssertGreaterThan(SmoothPointerPath.handShare(0.9), 0.99)
    }

    func testAReachTakesLongerTheFartherItGoesWithinAgentDesktopsRange() {
        XCTAssertEqual(SmoothPointerPath.glideSeconds(distance: 0), 0.09)
        let near = SmoothPointerPath.glideSeconds(distance: 72)
        XCTAssertEqual(near, 0.09 + 0.06 * 2, accuracy: 1e-9, "72 pt is two doublings of a 24 pt control")
        XCTAssertLessThan(near, SmoothPointerPath.glideSeconds(distance: 300))
        XCTAssertEqual(SmoothPointerPath.glideSeconds(distance: 5_000), 0.32)
    }

    /// A clock that moves only when the wait sleeps, and a stop that comes at
    /// a chosen moment of it.
    private func waitOut(dueNs: UInt64, stopAtNs: UInt64?) -> (SmoothPointerPath.Arrival, slept: [UInt64]) {
        var now: UInt64 = 0
        var slept: [UInt64] = []
        let arrival = SmoothPointerPath.waitOut(
            dueNs: dueNs, pollNs: 15,
            nowNs: { now },
            isStopped: { stopAtNs.map { now >= $0 } ?? false },
            sleepNs: { slept.append($0); now += $0 }
        )
        return (arrival, slept)
    }

    func testAMoveWaitsItsTimeOutInPollsAndArrives() {
        let (arrival, slept) = waitOut(dueNs: 40, stopAtNs: nil)
        XCTAssertEqual(arrival, .arrived)
        XCTAssertEqual(slept, [15, 15, 10], "the last sleep ends on the due time, not past it")
        XCTAssertEqual(waitOut(dueNs: 0, stopAtNs: nil).0, .arrived, "a move with nothing to wait for goes at once")
    }

    func testAStopDuringTheLastSleepStillSendsNothing() {
        // The stop comes after the last poll looked; the wait looks once more.
        XCTAssertEqual(waitOut(dueNs: 40, stopAtNs: 35).0, .stopped)
        XCTAssertEqual(waitOut(dueNs: 40, stopAtNs: 40).0, .stopped)
        XCTAssertEqual(waitOut(dueNs: 40, stopAtNs: 10).0, .stopped)
        XCTAssertEqual(waitOut(dueNs: 0, stopAtNs: 0).0, .stopped, "already stopped: nothing goes even without a move")
    }

    func testZeroOrNegativeStepsStillReachTheTarget() {
        XCTAssertEqual(SmoothPointerPath.points(from: .init(x: 1, y: 1), to: .init(x: 2, y: 2), steps: 0).last, .init(x: 2, y: 2))
    }
}
