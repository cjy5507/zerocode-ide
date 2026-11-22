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

    /// A row a click chooses (a Finder row offers no press) is read by its own
    /// label before it is chosen behind the person; anything else without a
    /// press of its own stays unreadable, as it was (t-125705 D1).
    func testARowAClickChoosesIsReadByItsOwnLabel() {
        XCTAssertEqual(ConfirmationTarget.backgroundLabel(role: "AXRow", actions: [], label: "report-a.log", selectable: true), "report-a.log")
        XCTAssertNil(ConfirmationTarget.backgroundLabel(role: "AXRow", actions: [], label: "report-a.log"))
        XCTAssertNil(ConfirmationTarget.backgroundLabel(role: "AXRow", actions: [], label: nil, selectable: true))
        XCTAssertNil(ConfirmationTarget.backgroundLabel(role: "AXRow", actions: [], label: " ", selectable: true))
        // Asked only when nothing else made it readable.
        var asked = 0
        func selectable() -> Bool { asked += 1; return true }
        XCTAssertEqual(ConfirmationTarget.backgroundLabel(role: "AXButton", actions: ["AXPress"], label: "OK", selectable: selectable()), "OK")
        XCTAssertEqual(asked, 0)
    }

    func testAFieldLabelCannotHideTheWindowsDefaultButton() {
        XCTAssertEqual(ConfirmationTarget.labels(focused: "Delivery address", defaultButton: "Pay"), "Delivery address\nPay")
    }

    /// A confirm fires what a Return fires there: a field's label cannot hide
    /// the dialog's "Pay", so the person's last step holds it behind them too.
    func testAConfirmIsReadWithTheDefaultButtonItFires() {
        XCTAssertEqual(ConfirmationTarget.secondaryActionLabels(action: "AXConfirm", label: "Card number", defaultButton: "Pay now"), "Card number\nPay now")
        XCTAssertEqual(confirmKind(of: ConfirmationTarget.secondaryActionLabels(action: "AXConfirm", label: "Card number", defaultButton: "Pay now"),
                                   words: ["payment": ["pay"], "delete": ["delete"]]), "payment")
        XCTAssertEqual(ConfirmationTarget.secondaryActionLabels(action: "AXConfirm", label: nil, defaultButton: "Delete"), "Delete")
        // Closing a menu or stepping a value fires no default button.
        XCTAssertEqual(ConfirmationTarget.secondaryActionLabels(action: "AXCancel", label: "File", defaultButton: "Delete"), "File")
        XCTAssertEqual(ConfirmationTarget.secondaryActionLabels(action: "AXIncrement", label: "Copies", defaultButton: "Print"), "Copies")
    }

    /// An action an app named itself is read by its name: a row's "Delete".
    func testAnAppNamedActionIsReadByItsName() {
        let raw = "Name:Delete\nTarget:0x600000c2d0e0\nSelector:(null)"
        XCTAssertEqual(ConfirmationTarget.customActionName(raw), "Delete")
        XCTAssertEqual(ConfirmationTarget.customActionName("Archive"), "Archive")
        XCTAssertEqual(ConfirmationTarget.secondaryActionLabels(action: raw, label: "Invoice 42", defaultButton: "Send"), "Invoice 42\nDelete")
        XCTAssertEqual(confirmKind(of: ConfirmationTarget.secondaryActionLabels(action: raw, label: "Invoice 42", defaultButton: nil),
                                   words: ["delete": ["delete"]]), "delete")
    }

    func testASecondaryActionOnNothingReadableHasNoTarget() {
        XCTAssertNil(ConfirmationTarget.secondaryActionLabels(action: "AXIncrement", label: nil, defaultButton: "OK"))
        XCTAssertNil(ConfirmationTarget.secondaryActionLabels(action: "AXCancel", label: " \n", defaultButton: nil))
        XCTAssertNil(ConfirmationTarget.secondaryActionLabels(action: "AXConfirm", label: nil, defaultButton: " "))
    }

    func testMissingAndEmptyLabelsDoNotInventTargets() {
        XCTAssertNil(ConfirmationTarget.labels(focused: nil, defaultButton: ""))
        XCTAssertEqual(ConfirmationTarget.labels(focused: nil, defaultButton: "Delete"), "Delete")
        XCTAssertEqual(ConfirmationTarget.labels(focused: "Submit", defaultButton: nil), "Submit")
    }
}
