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

    func testAnArcStandsOffTheLineByItsBowAndStillEndsOnTheTarget() {
        let start = SmoothPointerPath.Point(x: 0, y: 0)
        let end = SmoothPointerPath.Point(x: 200, y: 0)
        let arc = SmoothPointerPath.points(from: start, to: end, steps: 20, bow: 0.1)
        XCTAssertEqual(arc.last, end)
        // The middle stands a tenth of the distance off the line, on the bow's side.
        XCTAssertEqual(arc[9].y, 20, accuracy: 0.001)
        XCTAssertEqual(arc[9].x, 100, accuracy: 0.001)
        XCTAssertGreaterThan(SmoothPointerPath.points(from: start, to: end, steps: 20, bow: -0.1)[9].y * -1, 19)
        // A long move stands off no more than the cap.
        let far = SmoothPointerPath.points(from: start, to: .init(x: 2_000, y: 0), steps: 20, bow: 0.1)
        XCTAssertEqual(far[9].y, SmoothPointerPath.maxBowPoints, accuracy: 0.001)
        // No bow is the straight eased walk.
        XCTAssertEqual(SmoothPointerPath.points(from: start, to: end, steps: 20, bow: 0).map(\.y), Array(repeating: 0, count: 20))
        XCTAssertEqual(SmoothPointerPath.points(from: start, to: start, steps: 5, bow: 0.1).last, start)
    }

    func testAGlideIsQuickForAHopAndNeverACrawl() {
        XCTAssertEqual(SmoothPointerPath.glideSeconds(distance: 0), SmoothPointerPath.minGlideSeconds)
        XCTAssertEqual(SmoothPointerPath.glideSeconds(distance: 240), 0.22, accuracy: 0.001)
        XCTAssertEqual(SmoothPointerPath.glideSeconds(distance: 5_000), SmoothPointerPath.maxGlideSeconds)
    }

    func testZeroOrNegativeStepsStillReachTheTarget() {
        XCTAssertEqual(SmoothPointerPath.points(from: .init(x: 1, y: 1), to: .init(x: 2, y: 2), steps: 0).last, .init(x: 2, y: 2))
    }
}
