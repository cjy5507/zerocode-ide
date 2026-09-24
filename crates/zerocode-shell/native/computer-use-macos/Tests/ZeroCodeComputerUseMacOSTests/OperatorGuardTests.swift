import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class OperatorGuardTests: XCTestCase {
    /// What the window hands the helper today, byte for byte as
    /// `guard_table()` writes it — a source contract in zerocode-shell parses
    /// this same line and holds it to the core's table.
    static let windowTable = #"{"pace":{"burst":null,"mode":"unlimited","perSecond":null},"perSession":5000}"#

    private func windowBudget() throws -> ActionBudget {
        try ActionBudget(windowTable: Self.windowTable)
    }

    private func paced(perSecond: Int, burst: Int, perSession: Int) -> ActionBudget {
        guard let budget = ActionBudget(pace: .paced(perSecond: perSecond, burst: burst), perSession: perSession) else {
            preconditionFailure("a valid pace")
        }
        return budget
    }

    func testTheWindowsTableIsUnlimitedWithAFiniteSession() throws {
        let budget = try windowBudget()
        XCTAssertEqual(budget.pace, .unlimited)
        XCTAssertEqual(budget.perSession, 5_000)
    }

    func testAnUnlimitedHandNeverWaitsAndTheSessionCountStillStops() throws {
        var ledger = OperatorLedger(budget: try windowBudget())
        // The whole session in one instant: no action is ever told to wait.
        for _ in 0..<5_000 {
            XCTAssertEqual(ledger.admit(now: 0), .admitted)
        }
        XCTAssertEqual(ledger.admit(now: 0), .sessionBudget(limit: 5_000), "a count, not a rate, stops it")
        XCTAssertTrue(ledger.isStopped)
        XCTAssertEqual(ledger.actionsInLastMinute(now: 0), 5_000)
    }

    func testTheStopCutsAnUnlimitedHandAtOnce() throws {
        var ledger = OperatorLedger(budget: try windowBudget())
        XCTAssertEqual(ledger.admit(now: 0), .admitted)
        ledger.stop(reason: StopReason.hotkey)
        XCTAssertEqual(ledger.admit(now: 0), .stopped(reason: StopReason.hotkey))
        ledger.resume(resetBudget: false)
        XCTAssertEqual(ledger.admit(now: 0), .admitted)
        XCTAssertEqual(ledger.actions, 2)
    }

    func testWithoutTheWindowsTableNothingActsButTheStopStillStands() {
        var ledger = OperatorLedger(budget: nil)
        XCTAssertEqual(ledger.admit(now: 0), .noBudget, "no numbers of the helper's own")
        XCTAssertEqual(ledger.actions, 0)
        ledger.stop(reason: StopReason.signal)
        XCTAssertEqual(ledger.admit(now: 0), .stopped(reason: StopReason.signal))
        ledger.resume(resetBudget: true)
        XCTAssertEqual(ledger.admit(now: 0), .noBudget)
    }

    func testATableThatCannotBoundTheHandIsRefusedNotGuessed() {
        for table in [
            "",
            "not json",
            #"{"perSession":5000}"#,
            #"{"pace":{"mode":"fast"},"perSession":5000}"#,
            #"{"pace":{"mode":"unlimited","perSecond":10,"burst":null},"perSession":5000}"#,
            #"{"pace":{"mode":"paced","perSecond":10,"burst":null},"perSession":5000}"#,
            #"{"pace":{"mode":"paced","perSecond":0,"burst":20},"perSession":5000}"#,
            #"{"pace":{"mode":"paced","perSecond":-1,"burst":20},"perSession":5000}"#,
            #"{"pace":{"mode":"paced","perSecond":0.5,"burst":20},"perSession":5000}"#,
            #"{"pace":{"mode":"paced","perSecond":10,"burst":0},"perSession":5000}"#,
            #"{"pace":{"mode":"unlimited","perSecond":null,"burst":null},"perSession":0}"#,
            #"{"pace":{"mode":"unlimited","perSecond":null,"burst":null}}"#,
        ] {
            XCTAssertThrowsError(try ActionBudget(windowTable: table), table)
        }
        XCTAssertNil(ActionBudget(pace: .unlimited, perSession: 0))
        XCTAssertNil(ActionBudget(pace: .paced(perSecond: 0, burst: 1), perSession: 1))
        XCTAssertNil(ActionBudget(pace: .paced(perSecond: 1, burst: 0), perSession: 1))
    }

    func testAPacedTableReadsBothNumbers() throws {
        let budget = try ActionBudget(windowTable: #"{"pace":{"mode":"paced","perSecond":10,"burst":20},"perSession":7}"#)
        XCTAssertEqual(budget, paced(perSecond: 10, burst: 20, perSession: 7))
    }

    func testTheRenderedTableIsTheWindowsShape() throws {
        let budget = try windowBudget()
        let data = try JSONSerialization.data(withJSONObject: budget.rendered, options: [.sortedKeys])
        XCTAssertEqual(String(decoding: data, as: UTF8.self), Self.windowTable)
        XCTAssertEqual(try ActionBudget(windowTable: String(decoding: data, as: UTF8.self)), budget)
        let pacedBudget = paced(perSecond: 10, burst: 20, perSession: 5_000)
        let pacedData = try JSONSerialization.data(withJSONObject: pacedBudget.rendered, options: [.sortedKeys])
        XCTAssertEqual(try ActionBudget(windowTable: String(decoding: pacedData, as: UTF8.self)), pacedBudget)
    }

    func testABurstGoesBackToBackAndThenAPacedHandWaitsNotStops() {
        var ledger = OperatorLedger(budget: paced(perSecond: 10, burst: 3, perSession: 100))
        XCTAssertEqual(ledger.admit(now: 0), .admitted)
        XCTAssertEqual(ledger.admit(now: 0), .admitted)
        XCTAssertEqual(ledger.admit(now: 0), .admitted)
        guard case let .wait(seconds) = ledger.admit(now: 0) else {
            return XCTFail("past the burst the hand waits")
        }
        XCTAssertEqual(seconds, 0.1, accuracy: 1e-9, "one action's worth of the pace")
        XCTAssertFalse(ledger.isStopped, "a pace is not a stop")
        XCTAssertEqual(ledger.admit(now: 0.1), .admitted, "the pace came round")
        XCTAssertEqual(ledger.actions, 4)
    }

    func testAPacedHandsLongestWaitIsOneSecond() {
        var ledger = OperatorLedger(budget: paced(perSecond: 1, burst: 1, perSession: 100))
        XCTAssertEqual(ledger.admit(now: 0), .admitted)
        guard case let .wait(seconds) = ledger.admit(now: 0) else {
            return XCTFail("the second action at once waits")
        }
        XCTAssertTrue(seconds.isFinite && seconds > 0 && seconds <= 1, "\(seconds)")
    }

    func testASessionBudgetIsThePersonsToLift() {
        var ledger = OperatorLedger(budget: paced(perSecond: 100, burst: 100, perSession: 2))
        XCTAssertEqual(ledger.admit(now: 0), .admitted)
        XCTAssertEqual(ledger.admit(now: 1), .admitted)
        XCTAssertEqual(ledger.admit(now: 2), .sessionBudget(limit: 2))
        ledger.resume(resetBudget: false)
        XCTAssertEqual(ledger.admit(now: 3), .sessionBudget(limit: 2), "a plain resume does not lift the session cap")
        ledger.resume(resetBudget: true)
        XCTAssertEqual(ledger.admit(now: 4), .admitted)
        XCTAssertEqual(ledger.actions, 1)
    }

    func testAStopByHandOrSignalRefusesUntilResume() throws {
        var ledger = OperatorLedger(budget: try windowBudget())
        ledger.stop(reason: StopReason.hotkey)
        XCTAssertTrue(ledger.isStopped)
        XCTAssertEqual(ledger.admit(now: 0), .stopped(reason: StopReason.hotkey))
        ledger.resume(resetBudget: false)
        XCTAssertEqual(ledger.admit(now: 0), .admitted)
        XCTAssertEqual(ledger.lastActionAt, 0)
    }

    func testTheStopChordIsControlOptionEscapeWithoutCommand() {
        XCTAssertTrue(isStopHotkey(keyCode: 53, control: true, option: true, command: false))
        XCTAssertFalse(isStopHotkey(keyCode: 53, control: true, option: true, command: true), "force quit is the system's")
        XCTAssertFalse(isStopHotkey(keyCode: 53, control: true, option: false, command: false))
        XCTAssertFalse(isStopHotkey(keyCode: 12, control: true, option: true, command: false))
    }

    // MARK: the one hand

    /// A release of something the hand never pressed is dropped: a person's
    /// own button is not the hand's to let go of.
    func testTheHandLetsGoOnlyOfWhatItPressed() throws {
        let clock = FakeClock()
        let poster = RecordingPoster()
        let hand = OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 3)
        let token = try hand.acquire(.request)
        try hand.post(HandEvent(.buttonUp(.left, clickState: 1), x: 1, y: 1), by: token)
        try hand.post(HandEvent(.key(code: 7, down: false, modifier: 0)), by: token)
        XCTAssertEqual(poster.events.count, 0)
        try hand.post(HandEvent(.buttonDown(.right, clickState: 1), x: 1, y: 1), by: token)
        XCTAssertEqual(hand.relinquish(token).released, [.button(.right)], "the end of a hold lets go of what it left")
        XCTAssertEqual(poster.releases.map(\.kind), [.buttonUp(.right, clickState: 1)])
    }

    /// A release the platform could not post is not called let go: the hand
    /// says so and presses nothing again until a person resumes.
    func testAReleaseTheHandCouldNotPostKeepsItsHandsOffUntilResume() throws {
        final class RefusingReleases: HandPoster, @unchecked Sendable {
            struct Refused: Error {}
            func post(_ event: HandEvent, tag: Int64) throws {
                if event.letsGo != nil { throw Refused() }
            }
            func pointerLocation() -> SmoothPointerPath.Point? { nil }
        }
        let clock = FakeClock()
        let hand = OperatorHand(poster: RefusingReleases(), clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 3)
        let token = try hand.acquire(.request)
        try hand.post(HandEvent(.key(code: 9, down: true, modifier: 0)), by: token)
        let release = hand.stop(reason: StopReason.hotkey)
        XCTAssertEqual(release.released, [])
        XCTAssertEqual(release.unconfirmed, [.key(9)])
        hand.relinquish(token)
        XCTAssertThrowsError(try hand.acquire(.request)) { error in
            XCTAssertEqual(error as? OperatorHand.Refusal, .stopped(StopReason.hotkey))
        }
        hand.resume()
        // Taken back without a stop, the same unconfirmed release still keeps
        // the next hold off.
        let run = try hand.acquire(.reflex("r"))
        try hand.post(HandEvent(.key(code: 9, down: true, modifier: 0)), by: run)
        XCTAssertEqual(hand.revoke(run, reason: "external_input").unconfirmed, [.key(9)])
        hand.relinquish(run)
        XCTAssertThrowsError(try hand.acquire(.request)) { error in
            XCTAssertEqual(error as? OperatorHand.Refusal, .releaseUnconfirmed)
        }
        hand.resume()
        XCTAssertNoThrow(try hand.acquire(.request), "a person's resume")
    }
}
