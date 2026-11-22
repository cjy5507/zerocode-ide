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
        let branch = try XCTUnwrap(source.range(of: "let inFront = person.foreground == snapshot.app.pid"))
        let body = source[branch.upperBound...].prefix(3500)
        let refuses = try XCTUnwrap(body.range(of: "guard BackgroundInput.pressesInFront(action: own, textEntry: textEntry) else { throw backgroundTargetInUse() }"))
        let stands = try XCTUnwrap(body.range(of: "try standBackInTheirApp()"))
        let scrolls = try XCTUnwrap(body.range(of: "try inView("))
        // The app's own press goes through the one background road, which
        // finds the control reachable before it presses (pinned below).
        let pressed = try XCTUnwrap(body.range(of: "return try backgroundAppAction(action, on: record, in: snapshot, person: person, at: requestedPoint)"))
        XCTAssertLessThan(refuses.lowerBound, stands.lowerBound)
        XCTAssertLessThan(stands.lowerBound, scrolls.lowerBound)
        XCTAssertLessThan(scrolls.lowerBound, pressed.lowerBound)

        let road = try XCTUnwrap(source.range(of: "private func standBackInTheirApp() throws {"))
        let stand = source[road.upperBound...].prefix(260)
        let asks = try XCTUnwrap(stand.range(of: "try DesktopBorrowHost.refuseIfThePersonIsBusy()"))
        let marks = try XCTUnwrap(stand.range(of: "context.asksThePerson = true"))
        XCTAssertLessThan(asks.lowerBound, marks.lowerBound, "the last look is asked for only once the first let it through")
        XCTAssertFalse(stand.contains("waitFor"), "nothing waits between the look an act was dispatched on and its press")
        XCTAssertEqual(source.components(separatedBy: "try standBackInTheirApp()").count - 1, 3, "the press, an app's own secondary action and the written value")

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
        let road = try XCTUnwrap(source.range(of: "private func requireReachable("))
        let body = source[road.upperBound...].prefix(1500)
        let front = try XCTUnwrap(body.range(of: "if RequestContext.current.inTheirApp {"))
        let asked = try XCTUnwrap(body.range(of: "guard try performActionChecked(record.element, \"AXScrollToVisible\") else { throw covered }"))
        let unasked = try XCTUnwrap(body.range(of: "_ = AXUIElementPerformAction(record.element, \"AXScrollToVisible\" as CFString)"))
        XCTAssertLessThan(front.lowerBound, asked.lowerBound)
        XCTAssertLessThan(asked.lowerBound, unasked.lowerBound, "behind the person the scroll is as it was")
        XCTAssertEqual(body.components(separatedBy: "AXUIElementPerformAction(").count - 1, 1)
    }

    /// An app's own action on its control goes behind the person — closing the
    /// menu it opened, confirming a field, stepping a value, an action the app
    /// named — but not one that raises a window over theirs or opens a menu
    /// over their app (t-125705).
    func testAnAppsOwnActionStaysInItsWindowButARaiseOrAMenuDoesNot() {
        for action in ["AXCancel", "AXConfirm", "AXIncrement", "AXDecrement", "AXPick", "AXShowAlternateUI", "AXPress",
                       "Name:Delete\nTarget:0x0\nSelector:(null)"] {
            XCTAssertTrue(BackgroundInput.secondaryActionStaysInItsWindow(action), action)
        }
        for action in ["AXRaise", "AXShowMenu"] {
            XCTAssertFalse(BackgroundInput.secondaryActionStaysInItsWindow(action), action)
        }
        XCTAssertTrue(BackgroundInput.methods.contains("performSecondaryAction"))
    }

    func testASecondaryActionIsNamedByItsRawOrItsShownName() {
        XCTAssertEqual(BackgroundInput.secondaryAction("cancel", among: ["AXPress", "AXCancel"]), "AXCancel")
        XCTAssertEqual(BackgroundInput.secondaryAction("AXCancel", among: ["AXCancel"]), "AXCancel")
        XCTAssertEqual(BackgroundInput.secondaryAction("Scroll Down", among: ["AXScrollUpByPage", "AXScrollDownByPage"]), "AXScrollDownByPage")
        XCTAssertNil(BackgroundInput.secondaryAction("raise", among: ["AXPress", "AXCancel"]), "only what the control offers")
        XCTAssertNil(BackgroundInput.secondaryAction("cancel", among: []))
    }

    /// The helper's secondary-action road behind the person: what takes their
    /// screen is refused first, in their front app it stands back as a press
    /// does, and only then is anything scrolled or pressed — through the one
    /// road a click's own press takes.
    func testABackgroundSecondaryActionRefusesTheDeskThenStandsBackThenActs() throws {
        let source = try helperSource()
        let road = try XCTUnwrap(source.range(of: "private func backgroundSecondaryAction(_ action: String, on found: ElementRecord, in snapshot: Snapshot) throws -> [String: Any] {"))
        let body = source[road.upperBound...].prefix(1900)
        let desk = try XCTUnwrap(body.range(of: "guard BackgroundInput.secondaryActionStaysInItsWindow(action) else {"))
        let front = try XCTUnwrap(body.range(of: "if person.foreground == snapshot.app.pid {"))
        let refuses = try XCTUnwrap(body.range(of: "guard BackgroundInput.pressesInFront(action: action, textEntry: MarkPin.textEntryRoles.contains(found.role)) else { throw backgroundTargetInUse() }"))
        let stands = try XCTUnwrap(body.range(of: "try standBackInTheirApp()"))
        let acts = try XCTUnwrap(body.range(of: "return try backgroundAppAction(action, on: record, in: snapshot, person: person)"))
        XCTAssertLessThan(desk.lowerBound, front.lowerBound)
        XCTAssertLessThan(front.lowerBound, refuses.lowerBound)
        XCTAssertLessThan(refuses.lowerBound, stands.lowerBound)
        XCTAssertLessThan(stands.lowerBound, acts.lowerBound)
        XCTAssertFalse(body.contains("AXUIElementPerformAction("), "it acts only through the checked road")

        let action = try XCTUnwrap(source.range(of: "private func backgroundAppAction("))
        let act = source[action.upperBound...].prefix(1850)
        let reaches = try XCTUnwrap(act.range(of: "try requireReachable(record, in: snapshot, at: requestedPoint, beforeScroll: requireCertainPress)"))
        let performs = try XCTUnwrap(act.range(of: "guard try performActionChecked(record.element, action, beforeSend: requireCertainPress) else {"))
        XCTAssertLessThan(reaches.lowerBound, performs.lowerBound)
        XCTAssertTrue(act.contains("no fallback was attempted"))

        let dispatch = try XCTUnwrap(source.range(of: "private func performSecondaryAction(params: [String: JSONValue], snapshot: Snapshot) throws -> [String: Any] {"))
        let entry = source[dispatch.upperBound...].prefix(1850)
        let named = try XCTUnwrap(entry.range(of: "guard let action = offered else {"))
        let behind = try XCTUnwrap(entry.range(of: "return try backgroundSecondaryAction(action, on: found, in: snapshot)"))
        XCTAssertLessThan(named.lowerBound, behind.lowerBound)
    }

    /// The person's last step (§1.5) holds a secondary action behind them as
    /// it holds a press: a confirm is read with the default button it fires,
    /// and a control nothing can be read of is not acted on unasked.
    func testABackgroundSecondaryActionIsHeldForThePersonsLastStepAndRefusedUnread() throws {
        let source = try helperSource()
        let guardRoad = try XCTUnwrap(source.range(of: "case \"performSecondaryAction\":\n            let snapshot = try loadSnapshot()"))
        let body = source[guardRoad.upperBound...].prefix(1800)
        XCTAssertTrue(body.contains("try secondaryPressFace(action, item: found.element, requireAnswer: background)"))
        XCTAssertTrue(source.contains("switch nearestDefaultButton(item, strict: requireAnswer)"))
        XCTAssertTrue(body.contains("defaultButton: face.defaultButton, in: snapshot"))
        let unread = try XCTUnwrap(body.range(of: "guard let text = face.label else {"))
        let refused = try XCTUnwrap(body.range(of: "throw ProviderError.coded(\"element_not_found\", ReadingRefusal.backgroundTarget(.notAControl))"))
        XCTAssertLessThan(unread.lowerBound, refused.lowerBound)
        // The guard reads every acting request before it is dispatched, this one too.
        let dispatch = try XCTUnwrap(source.range(of: "let kind = confirmKind(of: target, words: words) {"))
        let held = source[dispatch.upperBound...].prefix(1400)
        XCTAssertTrue(held.contains("throw ProviderError.confirmation(message, token: approval.token)"))
    }

    private func helperSource() throws -> String {
        let here = URL(fileURLWithPath: #filePath)
        let main = here.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/ZeroCodeComputerUseMacOS/main.swift")
        return try String(contentsOf: main, encoding: .utf8)
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
