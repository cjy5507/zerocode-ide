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

    func testZeroOrNegativeStepsStillReachTheTarget() {
        XCTAssertEqual(SmoothPointerPath.points(from: .init(x: 1, y: 1), to: .init(x: 2, y: 2), steps: 0).last, .init(x: 2, y: 2))
    }
}
