import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// What a click on a list's row chooses behind the person (t-125705 D1).
final class RowSelectionTests: XCTestCase {
    func testAClickChoosesAsAPersonsClickDoes() {
        XCTAssertEqual(RowSelection.pick(command: false, shift: false, others: false), .only)
        XCTAssertEqual(RowSelection.pick(command: true, shift: false, others: false), .toggle)
        XCTAssertEqual(RowSelection.pick(command: false, shift: true, others: false), .run)
        // Option, control, or both at once choose nothing here: the click goes on as it went.
        XCTAssertNil(RowSelection.pick(command: true, shift: true, others: false))
        XCTAssertNil(RowSelection.pick(command: false, shift: false, others: true))
        XCTAssertNil(RowSelection.pick(command: true, shift: false, others: true))
    }

    func testAPlainClickChoosesTheRowAlone() {
        XCTAssertEqual(RowSelection.after(.only, target: 4, chosen: []), [4])
        XCTAssertEqual(RowSelection.after(.only, target: 4, chosen: [1, 2, 7]), [4])
    }

    func testACommandClickAddsTheRowOrTakesItAway() {
        XCTAssertEqual(RowSelection.after(.toggle, target: 4, chosen: [7, 1]), [1, 4, 7])
        XCTAssertEqual(RowSelection.after(.toggle, target: 7, chosen: [1, 4, 7]), [1, 4])
        XCTAssertEqual(RowSelection.after(.toggle, target: 2, chosen: []), [2])
        XCTAssertEqual(RowSelection.after(.toggle, target: 2, chosen: [2]), [], "the last row taken away leaves nothing chosen")
        XCTAssertEqual(RowSelection.after(.toggle, target: 3, chosen: [5, 5]), [3, 5], "a row read twice is one row")
    }

    func testAShiftClickRunsFromTheOneChosenRow() {
        XCTAssertEqual(RowSelection.after(.run, target: 5, chosen: [2]), [2, 3, 4, 5])
        XCTAssertEqual(RowSelection.after(.run, target: 1, chosen: [3]), [1, 2, 3])
        XCTAssertEqual(RowSelection.after(.run, target: 3, chosen: [3]), [3])
        XCTAssertEqual(RowSelection.after(.run, target: 3, chosen: []), [3], "with nothing chosen it is the row alone")
        // Where a person's run would start among several chosen rows is not known: not guessed.
        XCTAssertNil(RowSelection.after(.run, target: 6, chosen: [1, 4]))
    }

    func testTheItemsAClickWalksUpTo() {
        XCTAssertEqual(RowSelection.itemRoles, ["AXRow", "AXCell", "AXImage"])
        XCTAssertFalse(RowSelection.itemRoles.contains("AXWindow"))
        XCTAssertGreaterThan(RowSelection.itemDepth, 2, "a row's name is a text field inside a cell inside it")
    }

    /// t-125705 R6: after the list's selection is written, the click goes on
    /// by its window road only when the app refused the write outright. An
    /// error or no answer may have chosen the rows all the same — the rule a
    /// press's answer follows — and a click after it would land twice.
    func testAClickGoesOnOnlyAfterAWriteTheAppRefusedOutright() {
        // attributeUnsupported, actionUnsupported, notImplemented, noValue, illegalArgument
        for raw: Int32 in [-25205, -25206, -25208, -25212, -25201] {
            XCTAssertTrue(RowSelection.clickGoesOn(after: AXActionVerdict.of(raw: raw)), "\(raw)")
        }
        var clicks: [Int32] = []
        // failure, cannotComplete, an answer not known, the element gone, the API off, taken
        for raw: Int32 in [-25200, -25204, -1, -25202, -25211, 0] where RowSelection.clickGoesOn(after: AXActionVerdict.of(raw: raw)) {
            clicks.append(raw)
        }
        XCTAssertEqual(clicks, [], "no click after a write that may have landed")
    }

    /// t-125705 R6: rows that cannot be read back are not said to be chosen —
    /// not even when none is wanted (the last row taken away), which an unread
    /// list would otherwise seem to hold.
    func testAChoiceIsVerifiedOnlyByRowsReadBack() {
        let same: (Int, Int) -> Bool = { $0 == $1 }
        XCTAssertEqual(RowSelection.chosenAsWanted([3, 5], read: [5, 3], same: same), true)
        XCTAssertEqual(RowSelection.chosenAsWanted([3, 5], read: [3], same: same), false)
        XCTAssertEqual(RowSelection.chosenAsWanted([3], read: [3, 4], same: same), false)
        XCTAssertEqual(RowSelection.chosenAsWanted([Int](), read: [], same: same), true)
        XCTAssertNil(RowSelection.chosenAsWanted([3], read: nil, same: same))
        XCTAssertNil(RowSelection.chosenAsWanted([Int](), read: nil, same: same), "an unread list is not an empty one")
    }

    /// The helper's click road behind the person: a row is chosen through its
    /// list only after an app's own press and a field's focus had their turn,
    /// and before any window click; a write the app refuses outright leaves
    /// nothing said to be sent, and the click goes on as it went; a write
    /// that may have landed sends nothing more (t-125705 R6).
    func testTheClickRoadChoosesARowBeforeAnyWindowClick() throws {
        let here = URL(fileURLWithPath: #filePath)
        let main = here.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/ZeroCodeComputerUseMacOS/main.swift")
        let source = try String(contentsOf: main, encoding: .utf8)
        let branch = try XCTUnwrap(source.range(of: "let person = try personBeforeBackgroundPress()"))
        let body = source[branch.upperBound...].prefix(6100)
        let own = try XCTUnwrap(body.range(of: "return try backgroundAppAction(action, on: record, in: snapshot, person: person, at: requestedPoint)"))
        let field = try XCTUnwrap(body.range(of: "return actionMetadata(path: \"accessibility\", actionName: \"AXFocused\")"))
        let chosen = try XCTUnwrap(body.range(of: "if let record, let chosen = try backgroundSelection(record, modifiers: modifiers, count: count, button: button, in: snapshot, person: person, at: requestedPoint) {"))
        let window = try XCTUnwrap(body.range(of: "try Input.windowClick("))
        XCTAssertLessThan(own.lowerBound, field.lowerBound)
        XCTAssertLessThan(field.lowerBound, chosen.lowerBound)
        XCTAssertLessThan(chosen.lowerBound, window.lowerBound)

        let road = try XCTUnwrap(source.range(of: "private func backgroundSelection("))
        let end = try XCTUnwrap(source.range(of: "private func setValue(params: [String: JSONValue]) throws -> [String: Any] {", range: road.upperBound..<source.endIndex))
        let select = source[road.upperBound..<end.lowerBound]
        // The rows chosen now are read, or nothing is written (an unread list is not an empty one).
        let readFirst = try XCTUnwrap(select.range(of: "let chosen = copyArray(list, kAXSelectedRowsAttribute as String) else {"))
        let reaches = try XCTUnwrap(select.range(of: "try requireReachable(record, in: snapshot, at: requestedPoint, beforeScroll: requireSameRow)"))
        let asks = try XCTUnwrap(select.range(of: "try context.askThePersonIfFirst()"))
        let writes = try XCTUnwrap(select.range(of: "AXUIElementSetAttributeValue($0, kAXSelectedRowsAttribute as CFString, wanted as CFArray)"))
        let verdict = try XCTUnwrap(select.range(of: "let verdict = AXActionVerdict.of(raw: written.rawValue)"))
        let goesOn = try XCTUnwrap(select.range(of: "if RowSelection.clickGoesOn(after: verdict) {"))
        let undone = try XCTUnwrap(select.range(of: "context.sent = sentBefore\n                return nil"))
        let uncertain = try XCTUnwrap(select.range(of: "throw ProviderError.coded(\"delivery_uncertain\""))
        let readBack = try XCTUnwrap(select.range(of: "switch RowSelection.chosenAsWanted(wanted, read: read, same: { CFEqual($0, $1) }) {"))
        XCTAssertLessThan(readFirst.lowerBound, reaches.lowerBound)
        XCTAssertLessThan(reaches.lowerBound, asks.lowerBound)
        let gate = try XCTUnwrap(select.range(of: "try requireSameRow()"))
        XCTAssertLessThan(asks.lowerBound, gate.lowerBound)
        XCTAssertLessThan(gate.lowerBound, writes.lowerBound)
        XCTAssertLessThan(writes.lowerBound, verdict.lowerBound)
        XCTAssertLessThan(verdict.lowerBound, goesOn.lowerBound)
        XCTAssertLessThan(goesOn.lowerBound, undone.lowerBound)
        XCTAssertLessThan(undone.lowerBound, uncertain.lowerBound)
        XCTAssertLessThan(uncertain.lowerBound, readBack.lowerBound)
        XCTAssertEqual(select.components(separatedBy: "return nil").count - 1, 2,
                       "no pick, or a write refused outright")
        XCTAssertTrue(select.contains("the list's rows or selection could not be read; nothing was sent"))
        XCTAssertTrue(select.contains("verification = unverifiedAction(reason: \"readback_unsupported\", expected: expected)"))
        XCTAssertFalse(select.contains("?? []"), "nothing unread is read as empty")
        XCTAssertTrue(select.contains("RowSelection.after(pick, target: target, chosen: now)"))
        XCTAssertTrue(select.contains("rows.count <= RowSelection.maxRows"))
    }
}
