import CoreGraphics
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// The core's pin matrix (`marks::tests::a_pin_holds_across_a_window_move_and_breaks_on_a_shifted_row`),
/// value for value, and the clip the two walks share (`render::clipped`).
final class MarkPinTests: XCTestCase {
    private let signature = "AXRow\u{1f}row"
    private let rowName = "Message 3"
    private let inbox = "Inbox"
    private let frame = CGRect(x: 200, y: 192, width: 560, height: 44)

    private func holds(_ actualSignature: String?, _ actualName: String?, _ actualContext: String?, _ actualFrame: CGRect?) -> Bool {
        MarkPin.holds(
            expectedSignature: signature,
            expectedName: rowName,
            expectedContext: inbox,
            expectedFrame: frame,
            actualSignature: actualSignature,
            actualName: actualName,
            actualContext: actualContext,
            actualFrame: actualFrame,
            tolerance: 2.0
        )
    }

    func testPinHoldsForTheSameWindowLocalFace() {
        XCTAssertTrue(holds(signature, rowName, inbox, frame))
    }

    func testPinHoldsWithinTheTolerance() {
        XCTAssertTrue(holds(signature, rowName, inbox, CGRect(x: 202, y: 192, width: 560, height: 44)))
        XCTAssertTrue(holds(signature, rowName, inbox, CGRect(x: 200, y: 190, width: 560, height: 44)))
        XCTAssertTrue(holds(signature, rowName, inbox, CGRect(x: 200, y: 192, width: 562, height: 42)))
        XCTAssertFalse(holds(signature, rowName, inbox, CGRect(x: 202.01, y: 192, width: 560, height: 44)), "+2.01")
    }

    func testPinBreaksOnAShiftedRow() {
        XCTAssertFalse(holds(signature, rowName, inbox, CGRect(x: 200, y: 212, width: 560, height: 44)))
    }

    func testPinBreaksOnAChangedSignatureTextOrPlace() {
        XCTAssertFalse(holds("AXRow\u{1f}other", rowName, inbox, frame))
        XCTAssertFalse(holds(signature, "Message 4", inbox, frame), "the row's text changed under the same frame")
        XCTAssertFalse(holds(signature, rowName, "Archive", frame), "another place in the tree under the same frame")
    }

    func testPinBreaksWithoutAFrame() {
        XCTAssertFalse(holds(signature, rowName, inbox, nil))
    }

    func testNoWordsAndNoContextAreTheEmptyWord() {
        XCTAssertTrue(MarkPin.holds(
            expectedSignature: signature, expectedName: "", expectedContext: "", expectedFrame: frame,
            actualSignature: signature, actualName: nil, actualContext: nil, actualFrame: frame, tolerance: 2
        ))
    }

    func testAPinTravelsWholeOrNotAtAll() throws {
        let box: [String: Double] = ["x": 1, "y": 2, "width": 3, "height": 4]
        XCTAssertNil(try MarkPin.parse(signature: nil, name: nil, context: nil, frame: nil, tolerance: nil, present: 0))
        let parsed = try MarkPin.parse(signature: "sig", name: "Save", context: "Toolbar", frame: box, tolerance: 2, present: 5)
        XCTAssertEqual(parsed, MarkPin.Parsed(signature: "sig", name: "Save", context: "Toolbar", frame: CGRect(x: 1, y: 2, width: 3, height: 4), tolerance: 2))
        XCTAssertThrowsError(try MarkPin.parse(signature: "sig", name: "Save", context: nil, frame: box, tolerance: 2, present: 4))
        XCTAssertThrowsError(try MarkPin.parse(signature: "sig", name: "Save", context: "", frame: box, tolerance: -1, present: 5))
        XCTAssertThrowsError(try MarkPin.parse(signature: "sig", name: "Save", context: "", frame: box, tolerance: .infinity, present: 5))
        XCTAssertThrowsError(try MarkPin.parse(signature: "sig", name: "Save", context: "", frame: ["x": 1, "y": 2, "width": -3, "height": 4], tolerance: 2, present: 5))
        XCTAssertThrowsError(try MarkPin.parse(signature: "sig", name: "Save", context: "", frame: ["x": 1, "y": 2, "width": 3], tolerance: 2, present: 5))
        XCTAssertThrowsError(try MarkPin.parse(signature: "sig", name: "Save", context: "", frame: ["x": .nan, "y": 2, "width": 3, "height": 4], tolerance: 2, present: 5))
    }

    func testAClipCutsAFrameAndOneCutOffWholeKeepsNoSize() {
        let row = CGRect(x: 10, y: 100, width: 400, height: 40)
        XCTAssertEqual(MarkPin.clipped(row, by: nil), row)
        XCTAssertEqual(MarkPin.clipped(row, by: CGRect(x: 0, y: 120, width: 600, height: 400)), CGRect(x: 10, y: 120, width: 400, height: 20))
        XCTAssertEqual(MarkPin.clipped(row, by: CGRect(x: 0, y: 500, width: 600, height: 100)), CGRect(x: 10, y: 100, width: 0, height: 0))
    }
}
