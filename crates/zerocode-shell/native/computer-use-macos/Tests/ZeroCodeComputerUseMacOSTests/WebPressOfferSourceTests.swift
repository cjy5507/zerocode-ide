import XCTest

/// A web view offers a control's press some seconds after its tree is switched
/// on, and a press before that answers success and does nothing. A press by
/// number waited for the offer before its turn; one by words or by point, and
/// the background confirmation that reads the control's press, did not — a
/// press by words came back refused as "not a readable control" or did
/// nothing (t-121372). These read the helper's source: the wait needs a live
/// accessibility tree.
final class WebPressOfferSourceTests: XCTestCase {
    private func helperSource() throws -> String {
        let here = URL(fileURLWithPath: #filePath)
        let main = here.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/ZeroCodeComputerUseMacOS/main.swift")
        return try String(contentsOf: main, encoding: .utf8)
    }

    private func body(of function: String, in source: String) throws -> Substring {
        let start = try XCTUnwrap(source.range(of: function), function)
        let end = try XCTUnwrap(source.range(of: "\n    }\n", range: start.upperBound..<source.endIndex), function)
        return source[start.upperBound..<end.lowerBound]
    }

    func testTheWaitAnswersThePressItReadAndAControlCarriesIt() throws {
        let source = try helperSource()
        let wait = try body(of: "private func waitForWebPress(_ record: ElementRecord, pid: pid_t) -> [String]? {", in: source)
        XCTAssertTrue(wait.contains("offered = now"), String(wait))
        XCTAssertTrue(wait.contains("if now.contains(\"AXPress\") { break }"), String(wait))
        XCTAssertTrue(wait.contains("return offered"), String(wait))
        let carried = try body(of: "private func offeringItsPress(_ record: ElementRecord, pid: pid_t) -> ElementRecord {", in: source)
        XCTAssertTrue(carried.contains("waitForWebPress(record, pid: pid).map { record.standing(actions: $0, frame: nil) } ?? record"), String(carried))
    }

    func testABackgroundPressByWordsOrPointWaitsForTheOfferBeforeItChoosesItsAction() throws {
        let source = try helperSource()
        let chosen = try XCTUnwrap(source.range(of: "?? requestedPoint.flatMap { backgroundClickRecord(at: $0, in: snapshot) })"))
        let next = source[chosen.upperBound...].prefix(120)
        XCTAssertTrue(next.contains(".map { offeringItsPress($0, pid: snapshot.app.pid) }"), String(next))
    }

    func testAPressByWordsWaitsBeforeTheTurnAndNeverInsideABorrowedDesktop() throws {
        let source = try helperSource()
        let ready = try body(of: "private func waitUntilActionable(method: String, params: [String: JSONValue]) throws {", in: source)
        XCTAssertTrue(ready.contains("waitForWebPressByReading(params)"), String(ready))
        let byReading = try body(of: "private func waitForWebPressByReading(_ params: [String: JSONValue]) {", in: source)
        XCTAssertTrue(byReading.contains("query.choose(among: readingFaces(cached))"), String(byReading))
        XCTAssertTrue(byReading.contains("waitForWebPress(record, pid: cached.app.pid)"), String(byReading))
        // The desktop press itself does not wait, up to its press: it may hold a borrowed desktop.
        let desk = try XCTUnwrap(source.range(of: "// A control named by what it reads is chosen on this dispatch's fresh"))
        let pressed = try XCTUnwrap(source.range(of: "performClickAction(record: record, mouseButton: button)", range: desk.upperBound..<source.endIndex))
        XCTAssertFalse(source[desk.upperBound..<pressed.lowerBound].contains("offeringItsPress"))
    }

    /// t-121539 고칠 것 1: outside a web view's settle window a click by words
    /// matches nothing before its turn, and every wait reads one deadline.
    func testOutsideTheSettleWindowNothingIsMatchedOrWaitedFor() throws {
        let source = try helperSource()
        let byReading = try body(of: "private func waitForWebPressByReading(_ params: [String: JSONValue]) {", in: source)
        let deadline = try XCTUnwrap(byReading.range(of: "webPressDeadline(pid: cached.app.pid) != nil"), String(byReading))
        let matching = try XCTUnwrap(byReading.range(of: "query.choose(among: readingFaces(cached))"), String(byReading))
        XCTAssertLessThan(deadline.lowerBound, matching.lowerBound, "the window is read before the look is matched")
        let wait = try body(of: "private func waitForWebPress(_ record: ElementRecord, pid: pid_t) -> [String]? {", in: source)
        XCTAssertTrue(wait.contains("let giveUp = webPressDeadline(pid: pid)"), String(wait))
        let window = try body(of: "private func webPressDeadline(pid: pid_t) -> Date? {", in: source)
        XCTAssertTrue(window.contains("on?.addingTimeInterval(WebAccessibility.settlesSeconds), Date() < deadline"), String(window))
    }

    func testTheBackgroundConfirmationReadsThePressOfferedByNowAndStillRefusesWithoutOne() throws {
        let source = try helperSource()
        let confirm = try XCTUnwrap(source.range(of: "func confirmTarget(method: String, params: [String: JSONValue], snapshot loadSnapshot: () throws -> Snapshot) throws -> String? {"))
        let label = source[confirm.upperBound...].prefix(1_400)
        XCTAssertTrue(label.contains("guard let record = record.map({ offeringItsPress($0, pid: snapshot.app.pid) }),"), String(label))
        XCTAssertTrue(label.contains("ConfirmationTarget.backgroundLabel(role: record.role, actions: record.actions,"), String(label))
        XCTAssertTrue(label.contains("throw ProviderError.coded(\"element_not_found\", ReadingRefusal.backgroundTarget(why))"), String(label))
    }
}
