import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class ConfirmationTargetTests: XCTestCase {
    func testAFieldLabelCannotHideTheWindowsDefaultButton() {
        XCTAssertEqual(ConfirmationTarget.labels(focused: "Delivery address", defaultButton: "Pay"), "Delivery address\nPay")
    }

    func testMissingAndEmptyLabelsDoNotInventTargets() {
        XCTAssertNil(ConfirmationTarget.labels(focused: nil, defaultButton: ""))
        XCTAssertEqual(ConfirmationTarget.labels(focused: nil, defaultButton: "Delete"), "Delete")
        XCTAssertEqual(ConfirmationTarget.labels(focused: "Submit", defaultButton: nil), "Submit")
    }
}
