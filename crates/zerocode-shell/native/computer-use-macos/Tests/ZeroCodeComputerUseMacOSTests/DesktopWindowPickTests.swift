import CoreGraphics
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// Which accessibility window is the numbered one (t-15085), every window
/// written out: nothing here reads a screen.
final class DesktopWindowPickTests: XCTestCase {
    private typealias Row = (number: UInt32?, frame: CGRect?)
    private let target: UInt32 = 77

    private func pick(_ bounds: CGRect?, _ windows: [Row]) -> DesktopWindowPick.Pick {
        DesktopWindowPick.pick(target: target, targetBounds: bounds, windows: windows)
    }

    func testTheNumberIsFoundWhateverTheFrame() {
        let rows: [Row] = [(1, CGRect(x: 0, y: 0, width: 10, height: 10)), (77, CGRect(x: 500, y: 500, width: 9, height: 9))]
        XCTAssertEqual(pick(CGRect(x: 0, y: 0, width: 10, height: 10), rows), .found(1))
    }

    func testWithNoNumberExactlyOneFrameThatFitsIsUsedOnEveryKindOfDesk() {
        // Origin, a screen left of the main one, above it, and a Retina
        // screen's point frame: all the window list's global top-left points.
        for frame in [
            CGRect(x: 0, y: 0, width: 720, height: 440), CGRect(x: -1920, y: 0, width: 720, height: 440),
            CGRect(x: 100, y: -982, width: 720, height: 440), CGRect(x: 3008, y: 40, width: 3008, height: 1692),
        ] {
            let rows: [Row] = [(nil, CGRect(x: 9_999, y: 9_999, width: 5, height: 5)), (nil, frame)]
            XCTAssertEqual(pick(frame, rows), .found(1), "\(frame)")
        }
    }

    func testTheToleranceIsTwoPointsInclusive() {
        let bounds = CGRect(x: 100, y: 100, width: 600, height: 400)
        XCTAssertEqual(DesktopWindowPick.frameTolerance, 2)
        XCTAssertEqual(pick(bounds, [(nil, CGRect(x: 102, y: 98, width: 602, height: 398))]), .found(0))
        XCTAssertEqual(pick(bounds, [(nil, CGRect(x: 102.5, y: 100, width: 600, height: 400))]), .none)
        XCTAssertEqual(pick(bounds, [(nil, CGRect(x: 100, y: 100, width: 600, height: 402.5))]), .none)
    }

    func testTwoNumberlessWindowsAtTheFrameAreRefusedNotGuessed() {
        let frame = CGRect(x: 50, y: 50, width: 400, height: 300)
        XCTAssertEqual(pick(frame, [(nil, frame), (nil, frame.offsetBy(dx: 1, dy: 0))]), .ambiguous)
    }

    func testNoNumberAndNoFrameThatFitsIsNone() {
        let frame = CGRect(x: 50, y: 50, width: 400, height: 300)
        XCTAssertEqual(pick(frame, [(nil, CGRect(x: 0, y: 0, width: 400, height: 300))]), .none)
        XCTAssertEqual(pick(frame, []), .none)
        XCTAssertEqual(pick(nil, [(nil, frame)]), .none, "the window list's bounds were not read")
    }

    func testAWindowThatCarriesAnotherNumberIsNeverTheFrameMatch() {
        let frame = CGRect(x: 50, y: 50, width: 400, height: 300)
        XCTAssertEqual(pick(frame, [(5, frame)]), .none)
        XCTAssertEqual(pick(frame, [(5, frame), (nil, frame)]), .found(1))
    }

    func testAWindowWhoseFrameCannotBeReadIsSkipped() {
        let frame = CGRect(x: 50, y: 50, width: 400, height: 300)
        XCTAssertEqual(pick(frame, [(nil, nil), (nil, frame)]), .found(1))
    }

    func testTheSameToleranceIsWhatTheRecoveryPathUses() {
        XCTAssertTrue(DesktopWindowPick.framesMatch(CGRect(x: 0, y: 0, width: 10, height: 10), CGRect(x: 2, y: 2, width: 12, height: 8)))
        XCTAssertFalse(DesktopWindowPick.framesMatch(CGRect(x: 0, y: 0, width: 10, height: 10), CGRect(x: 2.1, y: 0, width: 10, height: 10)))
    }
}
