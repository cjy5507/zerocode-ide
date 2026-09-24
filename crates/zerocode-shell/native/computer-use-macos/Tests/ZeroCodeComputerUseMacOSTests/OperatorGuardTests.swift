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

    /// A release goes past a stop, a revoke and the end of its hold — but only
    /// of what that hold pressed. Hold A's thread is held right before its own
    /// release while the run is stopped (A let go of by the revoke, the hand
    /// returned) and hold B presses the same button, then the same key: A's
    /// late release posts nothing and B's press stays down until B lets go.
    func test_a_late_release_from_an_earlier_hold_does_not_let_go_of_the_next_holds_press() throws {
        let inputs: [(String, HandEvent, HandEvent)] = [
            ("button", HandEvent(.buttonDown(.left, clickState: 1), x: 4, y: 4), HandEvent(.buttonUp(.left, clickState: 1), x: 4, y: 4)),
            ("key", HandEvent(.key(code: 12, down: true, modifier: 0)), HandEvent(.key(code: 12, down: false, modifier: 0))),
        ]
        for (name, press, release) in inputs {
            let clock = FakeClock()
            let poster = RecordingPoster()
            let hand = OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 0x1a7e)
            let a = try hand.acquire(.reflex("a"))
            try hand.post(press, by: a)
            let atRelease = DispatchSemaphore(value: 0)
            let go = DispatchSemaphore(value: 0)
            let done = DispatchSemaphore(value: 0)
            Thread.detachNewThread {
                atRelease.signal()
                _ = go.wait(timeout: .now() + 10)
                try? hand.post(release, by: a)
                done.signal()
            }
            XCTAssertEqual(atRelease.wait(timeout: .now() + 5), .success, name)
            // The run stops the way a reflex stop ends it: taken back, then returned.
            XCTAssertEqual(hand.revoke(a, reason: StopReason.request).released, press.holds.map { [$0] }, name)
            hand.relinquish(a)
            let b = try hand.acquire(.request)
            try hand.post(press, by: b)
            let posted = poster.events.count
            go.signal()
            XCTAssertEqual(done.wait(timeout: .now() + 5), .success, name)
            XCTAssertEqual(poster.events.count, posted, "\(name): A's late release posted nothing")
            XCTAssertEqual(hand.snapshot.held, press.holds.map { [$0] }, "\(name): B's press is still down")
            try hand.post(release, by: b)
            XCTAssertEqual(hand.snapshot.held, [], "\(name): B lets go of its own")
            XCTAssertEqual(poster.releases.count, 2, "\(name): A's by the revoke, B's by B")
            hand.relinquish(b)
        }
    }

    /// The platform refuses a hold's ordinary release: the hand does not keep
    /// calling the input held and go on. It counts the release it could not
    /// confirm, takes the hold back and lets go of what else it pressed; the
    /// hold presses nothing more, and nobody takes the hand until a person
    /// resumes.
    func test_a_release_the_platform_refuses_is_settled_before_anything_more_is_pressed() throws {
        let clock = FakeClock()
        let poster = RecordingPoster()
        var refusedOnce = false
        poster.refuse = { event in
            guard !refusedOnce, case .buttonUp = event.kind else { return false }
            refusedOnce = true
            return true
        }
        let hand = OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 0xbad)
        let run = try hand.acquire(.reflex("r"))
        let shift: UInt64 = 0x20000
        try hand.post(HandEvent(.key(code: 56, down: true, modifier: shift), flags: shift), by: run)
        try hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 5, y: 5, flags: shift), by: run)
        XCTAssertThrowsError(try hand.post(HandEvent(.buttonUp(.left, clickState: 1), x: 5, y: 5, flags: shift), by: run))
        let settled = hand.snapshot
        XCTAssertEqual(settled.unconfirmedReleases, 1, "the refused release is counted")
        XCTAssertEqual(settled.held, [], "settled: the button is not called held, and the modifier was let go of")
        XCTAssertNotNil(settled.revoked, "the hold is taken back")
        XCTAssertEqual(poster.releases.map(\.kind), [.key(code: 56, down: false, modifier: shift)])
        XCTAssertThrowsError(try hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 5, y: 5), by: run), "the hold presses nothing more")
        XCTAssertEqual(poster.presses.count, 2, "no press after the refused release")
        hand.relinquish(run)
        XCTAssertThrowsError(try hand.acquire(.request)) { error in
            XCTAssertEqual(error as? OperatorHand.Refusal, .releaseUnconfirmed)
        }
        hand.resume()
        XCTAssertNoThrow(try hand.acquire(.request), "a person's resume")
    }
}
