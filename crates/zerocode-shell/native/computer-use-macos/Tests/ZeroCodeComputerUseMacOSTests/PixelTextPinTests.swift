import CoreGraphics
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class PixelTextPinTests: XCTestCase {
    private let window = CGRect(x: 100, y: 50, width: 800, height: 600)
    private let frame = CGRect(x: 140, y: 90, width: 90, height: 30)

    private func pin() -> PixelTextPin {
        PixelTextPin(text: "Next view", frame: frame, window: window, tolerance: 2, minimumConfidence: 0.8)
    }

    func testSameWordsAtTheObservedPositionHold() {
        XCTAssertTrue(pin().holds(lines: [RecognizedLine(text: "Next view", confidence: 0.95, frame: frame)], window: window))
    }

    func testChangedWordsAndMovedTextDoNotHold() {
        for line in [
            RecognizedLine(text: "Different action", confidence: 0.95, frame: frame),
            RecognizedLine(text: "Next view", confidence: 0.95, frame: frame.offsetBy(dx: 0, dy: 30)),
            RecognizedLine(text: "Next view", confidence: 0.2, frame: frame)
        ] {
            XCTAssertFalse(pin().holds(lines: [line], window: window))
        }
    }

    func testMovedWindowRefusesEvenWhenTheWordsRemain() {
        XCTAssertFalse(pin().holds(lines: [RecognizedLine(text: "Next view", confidence: 0.95, frame: frame)], window: window.offsetBy(dx: 20, dy: 0)))
    }

    func testAmbiguousCoincidentLinesRefuse() {
        let line = RecognizedLine(text: "Next view", confidence: 0.95, frame: frame)
        XCTAssertFalse(pin().holds(lines: [line, line], window: window))
    }

    func testDuplicateWordsElsewhereDoNotChooseAnotherLine() {
        let lines = [
            RecognizedLine(text: "Next view", confidence: 0.95, frame: frame),
            RecognizedLine(text: "Next view", confidence: 0.95, frame: frame.offsetBy(dx: 0, dy: 100))
        ]
        XCTAssertTrue(pin().holds(lines: lines, window: window))
    }

    func testInvalidOrOutOfWindowRectanglesRefuse() {
        for rectangle in [CGRect(x: 0, y: 0, width: 10, height: 10), CGRect(x: 140, y: 90, width: -5, height: 30)] {
            let invalid = PixelTextPin(text: "Next view", frame: rectangle, window: window, tolerance: 2, minimumConfidence: 0.8)
            XCTAssertFalse(invalid.windowStands(window))
        }
    }
}
