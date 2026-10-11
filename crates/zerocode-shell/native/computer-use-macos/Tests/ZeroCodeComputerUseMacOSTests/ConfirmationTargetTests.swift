import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class ConfirmationTargetTests: XCTestCase {
    func testAnUnreadableBackgroundControlCannotBorrowItsWindowsLabel() {
        XCTAssertNil(ConfirmationTarget.backgroundLabel(role: "AXWindow", actions: ["AXRaise"], label: "Checkout"))
        XCTAssertNil(ConfirmationTarget.backgroundLabel(role: "AXWebArea", actions: [], label: "Store"))
        XCTAssertNil(ConfirmationTarget.backgroundLabel(role: "AXButton", actions: ["AXPress"], label: nil))
        XCTAssertNil(ConfirmationTarget.backgroundLabel(role: "AXButton", actions: ["AXPress"], label: " \n"))
        XCTAssertEqual(ConfirmationTarget.backgroundLabel(role: "AXButton", actions: ["AXPress"], label: "Pay"), "Pay")
        XCTAssertEqual(ConfirmationTarget.backgroundLabel(role: "AXTextField", actions: [], label: "Address"), "Address")
    }

    func testAFieldLabelCannotHideTheWindowsDefaultButton() {
        XCTAssertEqual(ConfirmationTarget.labels(focused: "Delivery address", defaultButton: "Pay"), "Delivery address\nPay")
    }

    func testMissingAndEmptyLabelsDoNotInventTargets() {
        XCTAssertNil(ConfirmationTarget.labels(focused: nil, defaultButton: ""))
        XCTAssertEqual(ConfirmationTarget.labels(focused: nil, defaultButton: "Delete"), "Delete")
        XCTAssertEqual(ConfirmationTarget.labels(focused: "Submit", defaultButton: nil), "Submit")
    }
}
