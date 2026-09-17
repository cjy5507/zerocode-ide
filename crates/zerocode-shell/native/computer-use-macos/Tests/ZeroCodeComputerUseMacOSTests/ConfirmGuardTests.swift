import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class ConfirmGuardTests: XCTestCase {
    let words = ["payment": ["결제", "pay", "place order"], "transfer": ["이체", "transfer"], "delete": ["삭제", "delete"]]

    func testALabelAnswersToTheFirstKindInOrderThatMatches() {
        XCTAssertEqual(confirmKind(of: "Place Order", words: words), "payment")
        XCTAssertEqual(confirmKind(of: "지금 결제", words: words), "payment")
        XCTAssertEqual(confirmKind(of: "Transfer funds", words: words), "transfer")
        XCTAssertEqual(confirmKind(of: "Delete account", words: words), "delete")
        XCTAssertNil(confirmKind(of: "Cancel", words: words))
        XCTAssertNil(confirmKind(of: nil, words: words))
        XCTAssertNil(confirmKind(of: "Pay now", words: ["delete": ["delete"]]), "a kind the person turned off is not asked about")
    }

    func testOnlyReturnOrEnterFiresTheDefaultButton() {
        XCTAssertTrue(firesDefaultButton("return"))
        XCTAssertTrue(firesDefaultButton("cmd+Enter"))
        XCTAssertFalse(firesDefaultButton("tab"))
        XCTAssertFalse(firesDefaultButton("cmd+r"))
    }
}
