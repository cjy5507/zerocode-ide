import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// The same cases `the_resize_ladder_starts_at_the_long_edge_and_stops_at_the_floor`
/// pins on the Rust side: the two providers walk one ladder.
final class ScreenshotBudgetTests: XCTestCase {
    func testTheLadderStartsAtTheLongEdgeAndStopsAtTheFloor() {
        let rungs = ScreenshotBudget.ladder(width: 2560, height: 1440)
        XCTAssertEqual(rungs[0], 0.5, accuracy: 1e-9)
        XCTAssertEqual(rungs[1], 0.425, accuracy: 1e-9)
        XCTAssertTrue(rungs.allSatisfy { $0 * 2560 >= ScreenshotBudget.floorLongEdge })
        XCTAssertLessThan(rungs.last! * ScreenshotBudget.step * 2560, ScreenshotBudget.floorLongEdge)
    }

    func testASmallPictureIsTriedAsCaptured() {
        XCTAssertEqual(ScreenshotBudget.ladder(width: 640, height: 480)[0], 1, accuracy: 1e-9)
    }

    func testASixKDisplayWalksTheSameRungsInPixels() {
        let rungs = ScreenshotBudget.ladder(width: 6016, height: 3384)
        XCTAssertEqual(rungs[0] * 6016, ScreenshotBudget.startLongEdge, accuracy: 1e-6)
        XCTAssertEqual(rungs.count, ScreenshotBudget.ladder(width: 2560, height: 1440).count)
    }

    func testTheRetinaLaptopFirstRungIsTheBudgetsLongEdge() {
        XCTAssertEqual(ScreenshotBudget.ladder(width: 3024, height: 1964)[0] * 3024, 1280, accuracy: 1e-6)
    }
}
