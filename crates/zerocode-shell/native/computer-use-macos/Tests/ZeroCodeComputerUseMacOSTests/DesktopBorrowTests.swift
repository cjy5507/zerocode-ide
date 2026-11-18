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

    // MARK: A refused act goes again

    private struct Refused: Error, Equatable { let code: String }

    /// One run of `againOncePaused` on a clock the test owns. `acts` answers
    /// each run of the act in turn (true: it goes; false: refused for the
    /// person); past its end the act is refused.
    private func goAgain(
        acts: [Bool], pointerAge: @escaping (Double) -> Double, sent: Bool = false,
        front: Int32? = 73, frontNow: @escaping (Double) -> Int32? = { _ in 73 },
        giveUpAt: Double = DesktopBorrow.longestWaitSeconds, refusal: String = DesktopBorrow.Busy.pointing.rawValue
    ) -> (result: Result<Int, Refused>, runs: Int, clock: Double) {
        var clock = 0.0
        var runs = 0
        let result: Result<Int, Refused>
        do {
            let went = try DesktopBorrow.againOncePaused(
                { () -> Int in
                    runs += 1
                    guard runs <= acts.count, acts[runs - 1] else { throw Refused(code: refusal) }
                    return runs
                },
                refusedForThePerson: { error in
                    guard let refused = error as? Refused else { return false }
                    return DesktopBorrow.Busy(rawValue: refused.code) != nil
                },
                sent: { sent },
                wait: { DesktopBorrow.Wait(giveUpAt: giveUpAt, front: front) },
                frontNow: { frontNow(clock) },
                read: { self.reading(pointer: pointerAge(clock)) },
                now: { clock },
                sleep: { clock += $0 }
            )
            result = .success(went)
        } catch let refused as Refused {
            result = .failure(refused)
        } catch {
            result = .failure(Refused(code: "other"))
        }
        return (result, runs, clock)
    }

    /// The person nudged the mouse as the act was about to go: it waits for
    /// their pause and goes from a fresh look.
    func testAnActRefusedForThePersonGoesAgainOnceTheyPause() {
        // They last moved at 0.1 s on the wait's clock.
        let (result, runs, clock) = goAgain(acts: [false, true], pointerAge: { clock in clock < 0.1 ? 0.01 : clock - 0.1 })
        XCTAssertEqual(result, .success(2))
        XCTAssertEqual(runs, 2)
        XCTAssertGreaterThanOrEqual(clock, DesktopBorrow.pointerQuietSeconds)
        XCTAssertLessThan(clock, 1)
    }

    /// Anything that was sent stays sent: the act is not run a second time.
    func testAnActThatSentSomethingIsNotRunAgain() {
        let (result, runs, clock) = goAgain(acts: [false, true], pointerAge: { _ in self.long }, sent: true)
        XCTAssertEqual(result, .failure(Refused(code: DesktopBorrow.Busy.pointing.rawValue)))
        XCTAssertEqual(runs, 1)
        XCTAssertEqual(clock, 0)
    }

    /// A request that put another app in front is holding the person's desktop,
    /// and one whose person moved to another app while it waited has lost it:
    /// neither waits on, and what cannot be told is not waited on.
    func testNothingOfThePersonsIsHeldForAWait() {
        let held: [(Int32?, (Double) -> Int32?)] = [(73, { _ in 42 }), (nil, { _ in 73 }), (73, { _ in nil }), (0, { _ in 0 })]
        for (front, frontNow) in held {
            let (result, runs, clock) = goAgain(acts: [false, true], pointerAge: { _ in self.long }, front: front, frontNow: frontNow)
            XCTAssertEqual(result, .failure(Refused(code: DesktopBorrow.Busy.pointing.rawValue)))
            XCTAssertEqual(runs, 1)
            XCTAssertEqual(clock, 0)
        }
        // Their desktop held and the person busy: not waited for at all —
        // the look before the wait refuses it, not the one after.
        let busy = goAgain(acts: [false, true], pointerAge: { _ in 0.01 }, frontNow: { _ in 42 })
        XCTAssertEqual(busy.result, .failure(Refused(code: DesktopBorrow.Busy.pointing.rawValue)))
        XCTAssertEqual(busy.runs, 1)
        XCTAssertEqual(busy.clock, 0, "nothing of the person's is held for a wait")
        // In front as the wait begins, gone by the time the person paused.
        let (result, runs, _) = goAgain(
            acts: [false, true], pointerAge: { clock in clock < 0.2 ? 0.01 : clock - 0.2 }, frontNow: { clock in clock < 0.3 ? 73 : 42 }
        )
        XCTAssertEqual(result, .failure(Refused(code: DesktopBorrow.Busy.pointing.rawValue)))
        XCTAssertEqual(runs, 1)
        XCTAssertTrue(DesktopBorrow.waitsAgain(frontWhenAsked: 73, frontNow: 73))
    }

    /// The wait is the request's one wait: a person who never pauses holds it
    /// up until that wait gives up, and no longer.
    func testAPersonWhoNeverPausesIsWaitedForNoLongerThanTheRequestsOneWait() {
        let (result, runs, clock) = goAgain(acts: [false, true], pointerAge: { _ in 0.01 })
        XCTAssertEqual(result, .failure(Refused(code: DesktopBorrow.Busy.pointing.rawValue)))
        XCTAssertEqual(runs, 1)
        XCTAssertGreaterThanOrEqual(clock, DesktopBorrow.longestWaitSeconds)
        XCTAssertLessThan(clock, DesktopBorrow.longestWaitSeconds + 2 * DesktopBorrow.pollSeconds)
        // What the wait it began with already spent is not spent again.
        let spent = goAgain(acts: [false, true], pointerAge: { _ in 0.01 }, giveUpAt: 0)
        XCTAssertEqual(spent.runs, 1)
        XCTAssertEqual(spent.clock, 0)
    }

    /// A refusal that keeps coming while the person is quiet is not a matter
    /// of waiting: the act goes again `redoLimit` times and no more.
    func testAnActRefusedOverAndOverStopsGoingAgain() {
        let (result, runs, clock) = goAgain(acts: [], pointerAge: { _ in self.long })
        XCTAssertEqual(result, .failure(Refused(code: DesktopBorrow.Busy.pointing.rawValue)))
        XCTAssertEqual(runs, DesktopBorrow.redoLimit + 1)
        XCTAssertEqual(clock, 0)
    }

    /// Only the person's input is waited for; any other refusal is the answer.
    func testAnyOtherRefusalIsThrownAsItCame() {
        let (result, runs, _) = goAgain(acts: [false, true], pointerAge: { _ in self.long }, refusal: "element_not_found")
        XCTAssertEqual(result, .failure(Refused(code: "element_not_found")))
        XCTAssertEqual(runs, 1)
    }

    /// A stop does not wait for the person to pause.
    func testAStopEndsTheWaitToGoAgain() {
        struct Stopped: Error {}
        var runs = 0
        XCTAssertThrowsError(try DesktopBorrow.againOncePaused(
            { () -> Int in
                runs += 1
                throw Refused(code: DesktopBorrow.Busy.typing.rawValue)
            },
            refusedForThePerson: { _ in true },
            sent: { false },
            wait: { DesktopBorrow.Wait(giveUpAt: 3, front: 73) },
            frontNow: { 73 },
            read: { self.reading(key: 0.01) },
            now: { 0 },
            sleep: { _ in throw Stopped() }
        )) { error in
            XCTAssertTrue(error is Stopped)
        }
        XCTAssertEqual(runs, 1)
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

    /// Every acting request's act runs where a refusal for the person can send
    /// it round again: in an app's own window, on the shared desktop, and in a
    /// borrow. Nothing else of the request is inside — the first wait, a
    /// window raised and what is given back happen once.
    func testEveryActingRequestGoesAgainThroughTheSameRoad() throws {
        let source = try helperSource()
        let road = "return try DesktopBorrowHost.againOnceThePersonPauses {"
        XCTAssertEqual(source.components(separatedBy: road).count - 1, 3)
        for function in [
            "private func act(method: String, params: [String: JSONValue], scoped: Bool) throws -> Any {",
            "private func borrowingDesktop(bringingUp query: String? = nil, method: String, params: [String: JSONValue]) throws -> Any {",
        ] {
            let body = try self.body(of: function, in: source)
            let waits = try XCTUnwrap(body.range(of: "try DesktopBorrowHost.waitForThePerson()"))
            let holds = try XCTUnwrap(body.range(of: "var pointer = Input.desktopCursorPosition()"))
            let goes = try XCTUnwrap(body.range(of: road, options: .backwards))
            XCTAssertLessThan(waits.lowerBound, goes.lowerBound, function)
            XCTAssertLessThan(holds.lowerBound, goes.lowerBound, function)
            // A go again gives the person back the pointer where they left it
            // while it waited, not where it was before.
            let rereads = try XCTUnwrap(body.range(of: "pointer = Input.desktopCursorPosition()\n                return try dispatch(method: method, params: params)"), function)
            XCTAssertLessThan(goes.lowerBound, rereads.lowerBound, function)
        }
    }

    /// The wait a request begins with records itself, and the app in front
    /// once the person paused: without it the first refusal would start a new
    /// wait of its own, with whatever the request raised already in front.
    func testTheWaitARequestBeginsWithRecordsItself() throws {
        let body = try self.body(of: "static func waitForThePerson() throws {", in: try helperSource())
        let waits = try XCTUnwrap(body.range(of: "try refuse(DesktopBorrow.waitForQuiet("))
        let records = try XCTUnwrap(body.range(of: "RequestContext.current.personWait = DesktopBorrow.Wait(giveUpAt: giveUpAt, front: frontPidNow())"))
        XCTAssertLessThan(waits.lowerBound, records.lowerBound)
    }

    /// One function's body in the helper's source: from its signature to the
    /// brace that closes it at its own indentation.
    private func body(of function: String, in source: String) throws -> Substring {
        let start = try XCTUnwrap(source.range(of: function), function)
        let end = try XCTUnwrap(source.range(of: "\n    }\n", range: start.upperBound..<source.endIndex), function)
        return source[start.upperBound..<end.lowerBound]
    }

    /// The look just before an act's first event still refuses at once: nothing
    /// waits between an act's look at the screen and its press.
    func testTheLookJustBeforeAnActDoesNotWait() throws {
        let source = try helperSource()
        let look = try XCTUnwrap(source.range(of: "func askThePersonIfFirst() throws {"))
        let body = source[look.upperBound...].prefix(220)
        XCTAssertTrue(body.contains("try DesktopBorrowHost.refuseIfThePersonIsBusy()"))
        XCTAssertFalse(body.contains("waitFor"))
    }

    /// Going again has no time of its own for a request that began with a wait:
    /// it spends what that wait left. Only a request that began with none
    /// starts one, the same length.
    func testGoingAgainSpendsTheRequestsOneWait() throws {
        let source = try helperSource()
        let again = try XCTUnwrap(source.range(of: "static func againOnceThePersonPauses<T>(_ act: () throws -> T) throws -> T {"))
        let body = source[again.upperBound...].prefix(900)
        let begun = try XCTUnwrap(body.range(of: "if let begun = context.personWait { return begun }"))
        let starts = try XCTUnwrap(body.range(of: "DesktopBorrow.longestWaitSeconds"))
        XCTAssertLessThan(begun.lowerBound, starts.lowerBound)
        XCTAssertEqual(body.components(separatedBy: "DesktopBorrow.longestWaitSeconds").count - 1, 1)
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
