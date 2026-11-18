import XCTest
@testable import ZeroCodeComputerUseMacOSCore
@testable import ZeroCodeComputerUseMacOS

final class BackgroundInputTests: XCTestCase {
    func testCompetingFocusClaimsAreRefusedInsteadOfChoosingTheFirstField() {
        XCTAssertEqual(BackgroundInput.focus(among: []), .none)
        XCTAssertEqual(BackgroundInput.focus(among: [7, 7]), .one(7))
        XCTAssertEqual(BackgroundInput.focus(among: [7, 9]), .ambiguous)
        XCTAssertEqual(BackgroundInput.focus(among: [9, 7]), .ambiguous)
    }

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
        for method in Provider.actingMethods.subtracting(BackgroundInput.methods) {
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

    func testATargetIsBehindInFrontOrUnknown() {
        XCTAssertEqual(BackgroundInput.admission(target: 42, foreground: 73), .behind)
        XCTAssertEqual(BackgroundInput.admission(target: 42, foreground: 42), .inFront)
        for foreground: Int32? in [nil, 0, -1] {
            XCTAssertNil(BackgroundInput.admission(target: 42, foreground: foreground), "an unknown front app is the person's")
        }
        XCTAssertNil(BackgroundInput.admission(target: 0, foreground: 73))
    }

    /// In the app the person has in front only the app's own press of a
    /// control that is no field goes: a field's press would move their
    /// keyboard focus, and a press with no action of the app's own would be a
    /// click in their key window.
    func testOnlyTheAppsOwnPressOfAControlThatIsNoFieldGoesInFront() {
        let own = { (actions: [String], textEntry: Bool, plain: Bool) in
            BackgroundInput.pressesInFront(
                action: BackgroundInput.clickAction(actions: actions, textEntry: textEntry, plainLeftClick: plain),
                textEntry: textEntry
            )
        }
        XCTAssertTrue(own(["AXPress"], false, true))
        XCTAssertTrue(own(["AXOpen"], false, true))
        // A field, even one that offers a press.
        XCTAssertFalse(own(["AXPress"], true, true))
        // No action of the app's own: it would be a window click.
        XCTAssertFalse(own([], false, true))
        XCTAssertFalse(own(["AXRaise", "AXShowMenu"], false, true))
        // A double click or a right click is no plain press.
        XCTAssertFalse(own(["AXPress"], false, false))
    }

    /// The helper's press road: in front it decides before anything is
    /// scrolled or pressed and stands back for the person there. It waits for
    /// nobody between its look and its press.
    func testAPressInFrontStandsBackForThePersonBeforeAnythingMoves() throws {
        let here = URL(fileURLWithPath: #filePath)
        let main = here.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/ZeroCodeComputerUseMacOS/main.swift")
        let source = try String(contentsOf: main, encoding: .utf8)
        let branch = try XCTUnwrap(source.range(of: "let inFront = try backgroundTargetIsInFront(snapshot)"))
        let body = source[branch.upperBound...].prefix(2600)
        let refuses = try XCTUnwrap(body.range(of: "guard BackgroundInput.pressesInFront(action: own, textEntry: textEntry) else { throw backgroundTargetInUse() }"))
        let stands = try XCTUnwrap(body.range(of: "try standBackInTheirApp()"))
        let scrolls = try XCTUnwrap(body.range(of: "try inView("))
        let reaches = try XCTUnwrap(body.range(of: "try requireReachable("))
        XCTAssertLessThan(refuses.lowerBound, stands.lowerBound)
        XCTAssertLessThan(stands.lowerBound, scrolls.lowerBound)
        XCTAssertLessThan(scrolls.lowerBound, reaches.lowerBound)

        let road = try XCTUnwrap(source.range(of: "private func standBackInTheirApp() throws {"))
        let stand = source[road.upperBound...].prefix(260)
        let asks = try XCTUnwrap(stand.range(of: "try DesktopBorrowHost.refuseIfThePersonIsBusy()"))
        let marks = try XCTUnwrap(stand.range(of: "context.asksThePerson = true"))
        XCTAssertLessThan(asks.lowerBound, marks.lowerBound, "the last look is asked for only once the first let it through")
        XCTAssertFalse(stand.contains("waitFor"), "nothing waits between the look an act was dispatched on and its press")
        XCTAssertEqual(source.components(separatedBy: "try standBackInTheirApp()").count - 1, 2, "the press and the written value")

        // Keys, typed text, scrolling and a mirrored phone are refused in front as they were.
        XCTAssertGreaterThanOrEqual(source.components(separatedBy: "try admitBackgroundTarget(snapshot)").count - 1, 3)
    }

    /// A control under its list is scrolled out from under it before a
    /// background press (`requireReachable`). Behind the person that moves
    /// nothing of theirs; in the app they have in front it moves what they
    /// are reading, so there it is an act of its own: asked about, and counted
    /// as sent, so a refusal after it never says nothing was done.
    func testAListScrollInTheirAppIsAnActOfItsOwn() throws {
        let here = URL(fileURLWithPath: #filePath)
        let main = here.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/ZeroCodeComputerUseMacOS/main.swift")
        let source = try String(contentsOf: main, encoding: .utf8)
        let road = try XCTUnwrap(source.range(of: "private func requireReachable(_ record: ElementRecord, in snapshot: Snapshot, at requestedPoint: CGPoint? = nil) throws {"))
        let body = source[road.upperBound...].prefix(1500)
        let front = try XCTUnwrap(body.range(of: "if RequestContext.current.inTheirApp {"))
        let asked = try XCTUnwrap(body.range(of: "guard try performActionChecked(record.element, \"AXScrollToVisible\") else { throw covered }"))
        let unasked = try XCTUnwrap(body.range(of: "_ = AXUIElementPerformAction(record.element, \"AXScrollToVisible\" as CFString)"))
        XCTAssertLessThan(front.lowerBound, asked.lowerBound)
        XCTAssertLessThan(asked.lowerBound, unasked.lowerBound, "behind the person the scroll is as it was")
        XCTAssertEqual(body.components(separatedBy: "AXUIElementPerformAction(").count - 1, 1)
    }

    func testOneAdvertisedSemanticClickOrNoClick() {
        XCTAssertEqual(BackgroundInput.clickAction(actions: ["AXPress", "AXConfirm"], textEntry: false, plainLeftClick: true), "AXPress")
        XCTAssertNil(BackgroundInput.clickAction(actions: ["AXRaise", "AXFocused"], textEntry: false, plainLeftClick: true))
        XCTAssertEqual(BackgroundInput.clickAction(actions: ["AXPress"], textEntry: true, plainLeftClick: true), "AXPress")
        XCTAssertNil(BackgroundInput.clickAction(actions: ["AXConfirm", "AXOpen", "AXFocused"], textEntry: true, plainLeftClick: true))
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
