import Foundation
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class WindowPointerAnchorTests: XCTestCase {
    private let bounds = CGRect(x: 100, y: 150, width: 400, height: 300)

    func testUnverifiedAndNonFinitePointsDoNotBecomeAPointer() {
        XCTAssertNil(WindowPointerAnchor(pid: 0, windowID: 2, bounds: bounds, point: CGPoint(x: 120, y: 180)))
        XCTAssertNil(WindowPointerAnchor(pid: 1, windowID: 0, bounds: bounds, point: CGPoint(x: 120, y: 180)))
        XCTAssertNil(WindowPointerAnchor(pid: 1, windowID: 2, bounds: bounds, point: CGPoint(x: CGFloat.nan, y: 180)))
        XCTAssertNil(WindowPointerAnchor(pid: 1, windowID: 2, bounds: bounds, point: CGPoint(x: 20, y: 180)))
    }

    func testCoveredReusedMovedAndMissingWindowsDoNotShowThePointer() throws {
        let anchor = try XCTUnwrap(WindowPointerAnchor(pid: 1, windowID: 2, bounds: bounds, point: CGPoint(x: 120, y: 180)))
        let target = WindowPointerAnchor.Window(pid: 1, id: 2, bounds: bounds, alpha: 1)
        let cover = WindowPointerAnchor.Window(pid: 3, id: 4, bounds: bounds, alpha: 1)
        XCTAssertTrue(anchor.visible(in: [target, cover], ignoringWindowID: nil))
        XCTAssertFalse(anchor.visible(in: [cover, target], ignoringWindowID: nil))
        XCTAssertFalse(anchor.visible(in: [WindowPointerAnchor.Window(pid: 3, id: 2, bounds: bounds, alpha: 1)], ignoringWindowID: nil))
        XCTAssertFalse(anchor.visible(in: [WindowPointerAnchor.Window(pid: 1, id: 2, bounds: bounds.offsetBy(dx: 20, dy: 0), alpha: 1)], ignoringWindowID: nil))
        XCTAssertFalse(anchor.visible(in: [], ignoringWindowID: nil))
    }

    func testItsOwnOverlayAndTransparentWindowsDoNotHideTheTarget() throws {
        let anchor = try XCTUnwrap(WindowPointerAnchor(pid: 1, windowID: 2, bounds: bounds, point: CGPoint(x: 120, y: 180)))
        let target = WindowPointerAnchor.Window(pid: 1, id: 2, bounds: bounds, alpha: 1)
        XCTAssertTrue(anchor.visible(in: [WindowPointerAnchor.Window(pid: 5, id: 6, bounds: bounds, alpha: 1), target], ignoringWindowID: 6))
        XCTAssertFalse(anchor.visible(in: [WindowPointerAnchor.Window(pid: 5, id: 7, bounds: bounds, alpha: 1), target], ignoringWindowID: 6))
        XCTAssertTrue(anchor.visible(in: [WindowPointerAnchor.Window(pid: 3, id: 4, bounds: bounds, alpha: 0), target], ignoringWindowID: nil))
    }

    func testTheTipUsesScreenPointsOnEitherSideOfTheMainDisplay() throws {
        let anchor = try XCTUnwrap(WindowPointerAnchor(pid: 1, windowID: 2,
            bounds: CGRect(x: -600, y: -300, width: 400, height: 250), point: CGPoint(x: -420, y: -180)))
        let frame = try XCTUnwrap(anchor.panelFrame(mainDisplayHeight: 1000, size: CGSize(width: 28, height: 34)))
        XCTAssertEqual(frame.minX + 1, -420)
        XCTAssertEqual(1000 - frame.maxY + 1, -180)
        XCTAssertNil(anchor.panelFrame(mainDisplayHeight: .nan, size: CGSize(width: 28, height: 34)))
    }
}
