import Foundation
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// A borrow of the shared pointer and keyboard, every reading written out:
/// nothing here touches an input device.
final class DesktopBorrowTests: XCTestCase {
    private let long = 60.0

    private func reading(key: Double? = nil, pointer: Double? = nil, button: Bool = false, modifier: Double? = nil,
                         ownKey: Double? = nil, ownPointer: Double? = nil, ownWarp: Double? = nil) -> DesktopBorrow.Reading {
        DesktopBorrow.Reading(sinceKey: key ?? long, sincePointer: pointer ?? long, buttonHeld: button,
                              modifierHeld: modifier != nil, sinceModifier: modifier ?? long,
                              sinceOwnKey: ownKey, sinceOwnPointer: ownPointer, sinceOwnWarp: ownWarp)
    }

    // MARK: The person goes first

    func testAPointerThatJustMovedIsThePersons() {
        XCTAssertEqual(DesktopBorrow.busy(reading(pointer: 0.1)), .pointing)
    }

    func testAPointerAtRestIsNotBusy() {
        XCTAssertNil(DesktopBorrow.busy(reading(pointer: 0.6)))
    }

    func testAHeldButtonIsBusyHoweverLongItRested() {
        XCTAssertEqual(DesktopBorrow.busy(reading(pointer: 30, button: true)), .pointing)
    }

    func testAModifierHeldDownIsTyping() {
        XCTAssertEqual(DesktopBorrow.busy(reading(key: 30, modifier: 2)), .typing)
    }

    func testAStickyModifierDoesNotRefuseEverythingForever() {
        XCTAssertNil(DesktopBorrow.busy(reading(modifier: DesktopBorrow.modifierHeldSeconds)))
    }

    func testKeysStillCount() {
        XCTAssertEqual(DesktopBorrow.busy(reading(key: 0.3)), .typing)
        XCTAssertNil(DesktopBorrow.busy(reading(key: 0.9)))
    }

    func testTheRefusalCodesAreTheProtocols() {
        XCTAssertEqual(DesktopBorrow.Busy.pointing.rawValue, "person_pointing")
        XCTAssertEqual(DesktopBorrow.Busy.typing.rawValue, "person_typing")
    }

    // MARK: Our own events are not the person

    func testOurOwnLastEventDoesNotMakeThePersonBusy() {
        // The system's table counted what we posted: the ages are one event.
        XCTAssertNil(DesktopBorrow.busy(reading(pointer: 0.11, ownPointer: 0.10)))
        XCTAssertNil(DesktopBorrow.busy(reading(key: 0.11, ownKey: 0.10)))
        // The system took it a moment after our post returned.
        XCTAssertNil(DesktopBorrow.busy(reading(pointer: 0.095, ownPointer: 0.10)))
    }

    func testAWarpAfterOurLastEventDoesNotHideThatEvent() {
        // The table counted the click, not the warp 0.15 s later, and the other way round.
        XCTAssertNil(DesktopBorrow.busy(reading(pointer: 0.30, ownPointer: 0.30, ownWarp: 0.15)))
        XCTAssertNil(DesktopBorrow.busy(reading(pointer: 0.15, ownPointer: 0.30, ownWarp: 0.15)))
    }

    func testThePersonInBetweenOurStampsIsThePerson() {
        XCTAssertEqual(DesktopBorrow.busy(reading(pointer: 0.22, ownPointer: 0.30, ownWarp: 0.15)), .pointing)
    }

    func testThePersonMovingAfterOurEventIsThePerson() {
        XCTAssertEqual(DesktopBorrow.busy(reading(pointer: 0.1, ownPointer: 0.4)), .pointing)
    }

    func testATableThatDoesNotCountOurEventsStillTellsThePerson() {
        // We posted just now and the table's last event is older, so it is theirs.
        XCTAssertEqual(DesktopBorrow.busy(reading(pointer: 0.3, ownPointer: 0.01)), .pointing)
    }

    // MARK: Waiting

    private func wait(_ readings: @escaping (Double) -> DesktopBorrow.Reading) -> (DesktopBorrow.Busy?, Double) {
        var clock = 0.0
        let busy = DesktopBorrow.waitForQuiet(
            read: { readings(clock) },
            now: { clock },
            giveUpAt: DesktopBorrow.longestWaitSeconds,
            sleep: { clock += $0 }
        )
        return (busy, clock)
    }

    func testAWaitAlreadySpentDoesNotStartAgain() {
        var slept = 0
        let busy = DesktopBorrow.waitForQuiet(
            read: { self.reading(pointer: 0.05) },
            now: { 5 },
            giveUpAt: 5,
            sleep: { _ in slept += 1 }
        )
        XCTAssertEqual(busy, .pointing)
        XCTAssertEqual(slept, 0)
    }

    func testAStopEndsTheWaitAtOnce() {
        struct Stopped: Error {}
        XCTAssertThrowsError(try DesktopBorrow.waitForQuiet(
            read: { self.reading(pointer: 0.05) },
            now: { 0 },
            giveUpAt: 3,
            sleep: { _ in throw Stopped() }
        ))
    }

    func testAPointerThatNeverStopsIsRefusedAfterTheWait() {
        let (busy, clock) = wait { _ in self.reading(pointer: 0.05) }
        XCTAssertEqual(busy, .pointing)
        XCTAssertGreaterThanOrEqual(clock, DesktopBorrow.longestWaitSeconds)
        XCTAssertLessThan(clock, DesktopBorrow.longestWaitSeconds + 2 * DesktopBorrow.pollSeconds)
    }

    func testAPointerThatStopsIsWaitedOut() {
        let (busy, clock) = wait { clock in
            // Last moved at 1.0 s on the wait's clock.
            let age = clock < 1 ? 0.01 : clock - 1
            return self.reading(pointer: age)
        }
        XCTAssertNil(busy)
        XCTAssertLessThan(clock, 1.7)
    }

    func testAQuietDeskCostsNoWait() {
        let (busy, clock) = wait { _ in self.reading() }
        XCTAssertNil(busy)
        XCTAssertEqual(clock, 0)
    }

    // MARK: What comes back

    private func point(_ x: Double, _ y: Double) -> SmoothPointerPath.Point {
        SmoothPointerPath.Point(x: x, y: y)
    }

    func testAPointerNothingMovedIsLeftAlone() {
        XCTAssertNil(DesktopBorrow.pointerAfter(saved: point(10, 10), lastPosted: nil, now: point(40, 40)))
    }

    func testAPointerWeMovedGoesBackWhereItWas() {
        XCTAssertEqual(DesktopBorrow.pointerAfter(saved: point(10, 20), lastPosted: point(500, 600), now: point(500, 600)),
                       point(10, 20))
    }

    func testAPointerClickedWhereItRestedIsLeftAlone() {
        XCTAssertNil(DesktopBorrow.pointerAfter(saved: point(10, 20), lastPosted: point(10, 20), now: point(14, 20)))
    }

    func testWhatThePersonMovedMeanwhileStaysTheirs() {
        XCTAssertEqual(DesktopBorrow.pointerAfter(saved: point(10, 20), lastPosted: point(500, 600), now: point(530, 590)),
                       point(40, 10))
    }

    func testThePersonsAppComesBackOverTheOneWeRaised() {
        XCTAssertTrue(DesktopBorrow.givesFrontBack(before: 1, raised: 2, now: 2))
    }

    func testAnAppThePersonSwitchedToIsNotOverwritten() {
        XCTAssertFalse(DesktopBorrow.givesFrontBack(before: 1, raised: 2, now: 3))
    }

    func testNothingRaisedNothingGivenBack() {
        XCTAssertFalse(DesktopBorrow.givesFrontBack(before: 1, raised: nil, now: 3))
        XCTAssertFalse(DesktopBorrow.givesFrontBack(before: 2, raised: 2, now: 2))
        XCTAssertFalse(DesktopBorrow.givesFrontBack(before: nil, raised: 2, now: 2))
    }

    func testAppLessKeysAreRefusedJustAfterABorrowGaveBack() {
        XCTAssertFalse(DesktopBorrow.refusesAppLessKeys(sinceGiveBack: nil))
        XCTAssertTrue(DesktopBorrow.refusesAppLessKeys(sinceGiveBack: 9))
        XCTAssertFalse(DesktopBorrow.refusesAppLessKeys(sinceGiveBack: DesktopBorrow.appLessKeysRefusedSeconds))
    }

    // MARK: Raising a window

    private func raise(upAt: Double, timeout: Double = 0.4) -> (Bool, Double) {
        var clock = 0.0
        let up = DesktopBorrow.waitUntilFront(
            timeout: timeout, interval: 0.015,
            isFront: { clock >= upAt },
            now: { clock },
            sleep: { clock += $0 }
        )
        return (up, clock)
    }

    func testAWindowAlreadyUpCostsNoWait() {
        let (up, clock) = raise(upAt: 0)
        XCTAssertTrue(up)
        XCTAssertEqual(clock, 0)
    }

    func testAWindowComingUpIsWaitedForNoLonger() {
        let (up, clock) = raise(upAt: 0.05)
        XCTAssertTrue(up)
        XCTAssertLessThan(clock, 0.07)
    }

    func testAWindowThatNeverComesUpGivesUp() {
        let (up, clock) = raise(upAt: 9)
        XCTAssertFalse(up)
        XCTAssertLessThan(clock, 0.43)
    }

    // MARK: Text

    func testOneEmojiIsOneStroke() {
        XCTAssertEqual(TextStrokes.strokes("a😀b"), ["a", "😀", "b"])
    }

    func testAJoinedEmojiStaysWhole() {
        let family = "👨‍👩‍👧‍👦"
        XCTAssertEqual(TextStrokes.strokes(family), [family])
    }

    func testDecomposedHangulStaysOneCharacter() {
        let han = "\u{1112}\u{1161}\u{11AB}"
        XCTAssertEqual(TextStrokes.strokes(han), [han])
    }

    func testAPlainStringIsOneStrokePerCharacter() {
        XCTAssertEqual(TextStrokes.strokes("Ab:"), ["A", "b", ":"])
        XCTAssertEqual(TextStrokes.strokes(""), [])
    }

    func testCarriageReturnLineFeedStaysTwoPresses() {
        XCTAssertEqual(TextStrokes.strokes("a\r\nb"), ["a", "\r", "\n", "b"])
    }

    func testACharacterPastTheEventLimitSplitsBetweenScalars() {
        let long = "a" + String(repeating: "\u{0301}", count: 25)
        let strokes = TextStrokes.strokes(long)
        XCTAssertEqual(strokes.joined(), long)
        XCTAssertTrue(strokes.allSatisfy { $0.utf16.count <= TextStrokes.unitsPerEvent })
        XCTAssertGreaterThan(strokes.count, 1)
    }

    // MARK: The helper's borrow asks all of this

    private func helperSource() throws -> String {
        let here = URL(fileURLWithPath: #filePath)
        let main = here.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/ZeroCodeComputerUseMacOS/main.swift")
        return try String(contentsOf: main, encoding: .utf8)
    }

    func testTheBorrowAsksThePersonAndRaisesNoFixedWait() throws {
        let source = try helperSource()
        XCTAssertTrue(source.contains("DesktopBorrow.waitForQuiet("))
        XCTAssertTrue(source.contains("DesktopBorrow.pointerAfter("))
        XCTAssertTrue(source.contains("DesktopBorrow.givesFrontBack("))
        XCTAssertTrue(source.contains("DesktopBorrow.waitUntilFront("))
        XCTAssertTrue(source.contains("DesktopBorrow.refusesAppLessKeys("))
        XCTAssertTrue(source.contains("TextStrokes.strokes("))
        XCTAssertFalse(source.contains("Thread.sleep(forTimeInterval: 0.4)"))
    }

    /// The person is asked at the last moment nothing has gone yet, whichever road
    /// goes first and after any window a verb raises on its way there: everything
    /// that says a request sent something says so right after asking.
    func testEverythingThatMarksARequestSentAsksThePersonFirst() throws {
        let lines = try helperSource().components(separatedBy: .newlines)
        var marks = 0
        for (at, line) in lines.enumerated() where line.contains(".sent = true") {
            marks += 1
            XCTAssertTrue(lines[at - 1].contains("askThePersonIfFirst()"), "line \(at + 1) of main.swift: \(line)")
        }
        XCTAssertGreaterThanOrEqual(marks, 5)
    }

    /// A select-all is sent as much as a key is: the person's next key replaces the field.
    func testASelectAllThroughAccessibilityAsksAndCountsAsSent() throws {
        let source = try helperSource()
        let road = try XCTUnwrap(source.range(of: "if isSelectAllHotkey(key), let focused = focusedRecord(snapshot) {"))
        let body = source[road.upperBound...].prefix(300)
        let asks = try XCTUnwrap(body.range(of: "askThePersonIfFirst()"))
        let sent = try XCTUnwrap(body.range(of: "context.sent = true"))
        let select = try XCTUnwrap(body.range(of: "TextInput.selectAll("))
        XCTAssertLessThan(asks.lowerBound, sent.lowerBound)
        XCTAssertLessThan(sent.lowerBound, select.lowerBound)
    }

    /// A clipboard write is seen by whatever watches the clipboard, so it comes after asking.
    func testAPasteAsksThePersonBeforeTheClipboard() throws {
        let source = try helperSource()
        let paste = try XCTUnwrap(source.range(of: "static func pasteText(_ text: String, pid: pid_t) throws {"))
        let body = source[paste.upperBound...].prefix(900)
        let asks = try XCTUnwrap(body.range(of: "askThePersonIfFirst()"))
        let write = try XCTUnwrap(body.range(of: "pasteboard.clearContents()"))
        XCTAssertLessThan(asks.lowerBound, write.lowerBound)
    }

    /// Background input does not go through the input devices, so it says nothing
    /// about them, and stamping it would hide the person behind another agent.
    func testAWindowScopedEventLeavesNoStampOfOurOwn() throws {
        let source = try helperSource()
        let note = try XCTUnwrap(source.range(of: "static func noteOwn(_ event: HandEvent) {"))
        XCTAssertTrue(source[note.upperBound...].prefix(160).contains("if case .window = event.route { return }"))
    }

    /// Acts that take the shared desktop outright, with no borrow, ask too, and
    /// only once the hand is held: a stop or a reflex run answers before the person.
    func testAnActOnTheSharedDeskAsksThePersonWithTheHandHeld() throws {
        let source = try helperSource()
        let act = try XCTUnwrap(source.range(of: "private func act(method: String, params: [String: JSONValue], scoped: Bool) throws -> Any {"))
        let body = source[act.upperBound...].prefix(700)
        let held = try XCTUnwrap(body.range(of: "OperatorHandHost.whileHeld"))
        let asks = try XCTUnwrap(body.range(of: "DesktopBorrowHost.waitForThePerson()"))
        XCTAssertLessThan(held.lowerBound, asks.lowerBound)
    }
}
