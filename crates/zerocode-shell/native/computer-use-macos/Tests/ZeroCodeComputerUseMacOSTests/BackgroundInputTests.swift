import XCTest
@testable import ZeroCodeComputerUseMacOSCore
@testable import ZeroCodeComputerUseMacOS

final class BackgroundInputTests: XCTestCase {
    func testAdmissionMatchesTheSharedProviderTable() throws {
        for row in try SharedCaseTable.rows("background.tsv") {
            let refusal = BackgroundInput.validate(
                method: row[0], requested: true,
                restoresWindow: row[1] == "true", hasApp: row[2] == "true"
            )
            XCTAssertEqual(refusal?.rawValue ?? "ok", row[3], "\(row)")
        }
    }

    func testNativeDispatchRejectsGlobalInputBeforeTouchingTheDesktop() throws {
        let provider = Provider()
        for method in Provider.actingMethods.subtracting(["click", "setValue", "scroll"]) {
            XCTAssertThrowsError(try provider.handle(method: method, params: [
                "background": .bool(true), "app": .string("Fixture"),
            ])) { error in
                XCTAssertEqual((error as? ProviderError)?.code, "requires_foreground", method)
            }
        }
        XCTAssertThrowsError(try provider.handle(method: "getAppState", params: [
            "background": .bool(true), "restoreWindow": .bool(true), "app": .string("Fixture"),
        ])) { error in
            XCTAssertEqual((error as? ProviderError)?.code, "invalid_argument")
        }
        XCTAssertThrowsError(try provider.handle(method: "click", params: [
            "background": .string("true"), "app": .string("Fixture"),
        ])) { error in
            XCTAssertEqual((error as? ProviderError)?.code, "invalid_argument")
        }
    }

    func testActiveOrUnknownTargetCannotCompeteWithThePerson() {
        XCTAssertTrue(BackgroundInput.admitsTarget(42, foreground: 73))
        for foreground: Int32? in [nil, 0, -1, 42] {
            XCTAssertFalse(BackgroundInput.admitsTarget(42, foreground: foreground))
        }
        XCTAssertFalse(BackgroundInput.admitsTarget(0, foreground: 73))
    }

    func testOneAdvertisedSemanticClickOrNoClick() {
        XCTAssertEqual(BackgroundInput.clickAction(actions: ["AXPress", "AXConfirm"], textEntry: false, plainLeftClick: true), "AXPress")
        XCTAssertNil(BackgroundInput.clickAction(actions: ["AXRaise", "AXFocused"], textEntry: false, plainLeftClick: true))
        XCTAssertNil(BackgroundInput.clickAction(actions: ["AXPress"], textEntry: true, plainLeftClick: true))
        XCTAssertNil(BackgroundInput.clickAction(actions: ["AXPress"], textEntry: false, plainLeftClick: false))
        XCTAssertNil(BackgroundInput.clickAction(actions: [], textEntry: false, plainLeftClick: true))
    }

    func testScrollingChoosesOnlyAnUnambiguousNestedContainer() {
        let outer = BackgroundInput.Scroller(index: 1, frame: CGRect(x: 0, y: 0, width: 300, height: 300))
        let inner = BackgroundInput.Scroller(index: 2, frame: CGRect(x: 10, y: 10, width: 90, height: 90))
        let crossing = BackgroundInput.Scroller(index: 3, frame: CGRect(x: 15, y: 0, width: 90, height: 90))
        let same = BackgroundInput.Scroller(index: 4, frame: inner.frame)
        let point = CGPoint(x: 20, y: 20)
        XCTAssertEqual(BackgroundInput.scrollTarget(at: point, among: [outer, inner]), 2)
        XCTAssertNil(BackgroundInput.scrollTarget(at: point, among: [inner, crossing]))
        XCTAssertNil(BackgroundInput.scrollTarget(at: point, among: [inner, same]))
        XCTAssertNil(BackgroundInput.scrollTarget(at: CGPoint(x: 500, y: 500), among: [outer]))
        XCTAssertNil(BackgroundInput.scrollTarget(at: CGPoint(x: CGFloat.nan, y: 20), among: [outer]))
    }
}
