import XCTest
import ApplicationServices
@testable import ZeroCodeComputerUseMacOSCore

final class ParallelActingTests: XCTestCase {
    private func hand() -> (OperatorHand, RecordingPoster) {
        let clock = FakeClock()
        let poster = RecordingPoster()
        return (OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 73), poster)
    }

    func testTwoWindowScopedRequestsActAtOnce() throws {
        let (hand, poster) = hand()
        let first = try hand.acquire(.scoped)
        let second = try hand.acquire(.scoped)
        try hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 1, y: 1, route: .window(pid: 1, id: 1)), by: first)
        try hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 2, y: 2, route: .window(pid: 2, id: 2)), by: second)
        XCTAssertEqual(poster.presses.count, 2)
        XCTAssertEqual(hand.snapshot.scopedHolds, 2)
    }

    func testAWindowScopedRequestActsBesideASharedDesktopRequest() throws {
        let (hand, _) = hand()
        let shared = try hand.acquire(.request)
        let scoped = try hand.acquire(.scoped)
        XCTAssertNil(hand.refusal(for: shared))
        XCTAssertNil(hand.refusal(for: scoped))
    }

    func testOnlyOneSharedDesktopRequestHoldsTheHand() throws {
        let (hand, _) = hand()
        _ = try hand.acquire(.request)
        XCTAssertThrowsError(try hand.acquire(.request)) { error in
            XCTAssertEqual(error as? OperatorHand.Refusal, .busy(.request))
        }
    }

    func testAReflexRunStillHasTheDeskAlone() throws {
        let (hand, _) = hand()
        let scoped = try hand.acquire(.scoped)
        XCTAssertThrowsError(try hand.acquire(.reflex("run"))) { error in
            XCTAssertEqual(error as? OperatorHand.Refusal, .busy(.scoped))
        }
        hand.relinquish(scoped)
        _ = try hand.acquire(.reflex("run"))
        XCTAssertThrowsError(try hand.acquire(.scoped)) { error in
            XCTAssertEqual(error as? OperatorHand.Refusal, .busy(.reflex("run")))
        }
    }

    func testAStopEndsEveryHoldAndLetsGoOfEverything() throws {
        let (hand, poster) = hand()
        let first = try hand.acquire(.scoped)
        let second = try hand.acquire(.scoped)
        let routeOne = HandRoute.window(pid: 1, id: 1)
        let routeTwo = HandRoute.window(pid: 2, id: 2)
        try hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 1, y: 1, route: routeOne), by: first)
        try hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 2, y: 2, route: routeTwo), by: second)
        _ = hand.stop(reason: StopReason.request)
        XCTAssertEqual(poster.releases.count, 2)
        XCTAssertNotNil(hand.refusal(for: first))
        XCTAssertNotNil(hand.refusal(for: second))
    }

    func testEndingOneHoldLetsGoOfItsOwnPressesOnly() throws {
        let (hand, poster) = hand()
        let first = try hand.acquire(.scoped)
        let second = try hand.acquire(.scoped)
        let routeOne = HandRoute.window(pid: 1, id: 1)
        let routeTwo = HandRoute.window(pid: 2, id: 2)
        try hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 1, y: 1, route: routeOne), by: first)
        try hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 2, y: 2, route: routeTwo), by: second)
        hand.relinquish(first)
        XCTAssertEqual(poster.releases.map(\.route), [routeOne])
        XCTAssertEqual(hand.snapshot.held.count, 1)
        XCTAssertNil(hand.refusal(for: second))
    }

    func testAWakeReachesEverySleeper() {
        let sleeper = SemaphoreSleeper()
        let far = DispatchTime.now().uptimeNanoseconds + 30_000_000_000
        let done = expectation(description: "both woke")
        done.expectedFulfillmentCount = 2
        for _ in 0..<2 {
            DispatchQueue.global().async {
                sleeper.sleep(untilNs: far)
                done.fulfill()
            }
        }
        DispatchQueue.global().asyncAfter(deadline: .now() + 0.1) { sleeper.wake() }
        wait(for: [done], timeout: 5)
    }

    func testAWakeWithNobodyWaitingEndsTheNextWait() {
        let sleeper = SemaphoreSleeper()
        sleeper.wake()
        let started = Date()
        sleeper.sleep(untilNs: DispatchTime.now().uptimeNanoseconds + 30_000_000_000)
        XCTAssertLessThan(Date().timeIntervalSince(started), 5)
    }

    func testTheBorrowedDesktopTakesOverOnlyWhatItCanDo() {
        XCTAssertTrue(BackgroundInput.foregroundMayTakeOver(method: "click", refusal: "requires_foreground"))
        XCTAssertTrue(BackgroundInput.foregroundMayTakeOver(method: "pressKey", refusal: "focus_changed"))
        XCTAssertTrue(BackgroundInput.foregroundMayTakeOver(method: "typeText", refusal: "value_not_settable"))
        XCTAssertFalse(BackgroundInput.foregroundMayTakeOver(method: "setValue", refusal: "value_not_settable"))
        XCTAssertFalse(BackgroundInput.foregroundMayTakeOver(method: "click", refusal: "element_not_found"))
        XCTAssertFalse(BackgroundInput.foregroundMayTakeOver(method: "typeText", refusal: "secure_input"))
    }

    func testLaneLocksAreOnePerLane() {
        let lanes = LaneLocks()
        XCTAssertTrue(lanes.lock(for: "a") === lanes.lock(for: "a"))
        XCTAssertFalse(lanes.lock(for: "a") === lanes.lock(for: "b"))
        XCTAssertEqual(lanes.count, 2)
    }
}

final class ElementIdentityBookTests: XCTestCase {
    private func book() -> ElementIdentityBook<String> {
        ElementIdentityBook(hash: { UInt(bitPattern: $0.hashValue) }, equal: { $0 == $1 })
    }

    func testAnElementKeepsItsNumberWhenARowArrivesAboveIt() {
        let book = book()
        book.beginWalk()
        let row = book.number(for: "row-5", root: "w")
        book.endCompleteWalk(root: "w")
        book.beginWalk()
        _ = book.number(for: "new-row", root: "w")
        XCTAssertEqual(book.number(for: "row-5", root: "w"), row)
    }

    func testARemovedElementsNumberIsNeverGivenToAnother() {
        let book = book()
        book.beginWalk()
        let gone = book.number(for: "row-3", root: "w")
        _ = book.number(for: "row-4", root: "w")
        book.endCompleteWalk(root: "w")
        book.beginWalk()
        _ = book.number(for: "row-4", root: "w")
        XCTAssertEqual(book.endCompleteWalk(root: "w"), [gone])
        XCTAssertNil(book.existingNumber(for: "row-3"))
        book.beginWalk()
        XCTAssertNotEqual(book.number(for: "row-9", root: "w"), gone)
    }

    func testAWalkOfOneWindowForgetsNothingOfAnother() {
        let book = book()
        book.beginWalk()
        let other = book.number(for: "other", root: "a")
        book.endCompleteWalk(root: "a")
        book.beginWalk()
        _ = book.number(for: "mine", root: "b")
        book.endCompleteWalk(root: "b")
        XCTAssertEqual(book.existingNumber(for: "other"), other)
    }

    func testTheBookNeverGrowsPastItsLimit() {
        let book = book()
        book.beginWalk()
        for at in 0...(ElementIdentityBook<String>.maxEntries + 10) {
            _ = book.number(for: "e\(at)", root: "w")
        }
        book.endCompleteWalk(root: "w")
        XCTAssertLessThanOrEqual(book.remembered, ElementIdentityBook<String>.maxEntries)
    }

    func testARelaunchedAppGoesOnFromItsLastNumber() {
        let numbers = ElementNumbers()
        let old = ElementIdentityBook<String>(numbers: numbers, hash: { UInt(bitPattern: $0.hashValue) }, equal: { $0 == $1 })
        old.beginWalk()
        XCTAssertEqual(["a", "b", "c"].map { old.number(for: $0, root: "w") }, [0, 1, 2])
        let new = ElementIdentityBook<String>(numbers: numbers, hash: { UInt(bitPattern: $0.hashValue) }, equal: { $0 == $1 })
        new.beginWalk()
        XCTAssertEqual(new.number(for: "a", root: "w"), 3, "a number from before the relaunch names nothing new")
    }

    func testAWalkAlwaysCutShortStillKeepsToTheLimitAndForgetsTheOldestSeen() {
        let book = book()
        let limit = ElementIdentityBook<String>.maxEntries
        book.beginWalk()
        for at in 0..<limit { _ = book.number(for: "old \(at)", root: "w") }
        XCTAssertTrue(book.endCutWalk().isEmpty)
        book.beginWalk()
        _ = book.number(for: "old 0", root: "w")
        for at in 0..<10 { _ = book.number(for: "new \(at)", root: "w") }
        let forgotten = book.endCutWalk()
        XCTAssertEqual(forgotten.count, 10, "the table forgets the same numbers")
        XCTAssertEqual(book.remembered, limit)
        XCTAssertNotNil(book.existingNumber(for: "new 9"))
        XCTAssertNotNil(book.existingNumber(for: "old 0"), "seen in the last walk, so it stays")
        XCTAssertNil(book.existingNumber(for: "old 1"))
    }
}

final class ElementChangesTests: XCTestCase {
    func testOnlyWhatChangedIsTold() {
        let changes = ElementChanges.between(
            previous: [1: "1 button A", 2: "2 button B", 3: "3 text C"],
            current: [1: "1 button A", 2: "2 button B pressed", 4: "4 button D"],
            order: [1, 2, 4]
        )
        XCTAssertEqual(changes.added, ["4 button D"])
        XCTAssertEqual(changes.changed, ["2 button B pressed"])
        XCTAssertEqual(changes.removed, [3])
        XCTAssertEqual(changes.omitted, 0)
    }

    func testNothingChangedIsEmpty() {
        XCTAssertTrue(ElementChanges.between(previous: [1: "x"], current: [1: "x"], order: [1]).isEmpty)
    }

    func testTheLimitsCountWhatTheyLeaveOut() {
        let current = Dictionary(uniqueKeysWithValues: (0..<100).map { ($0, "\($0) line") })
        let changes = ElementChanges.between(previous: [:], current: current, order: Array(0..<100), lineLimit: 10)
        XCTAssertEqual(changes.added.count, 10)
        XCTAssertEqual(changes.omitted, 90)
    }

    func testABigAnswerStaysUnderTheByteLimit() {
        let current = Dictionary(uniqueKeysWithValues: (0..<100).map { ($0, String(repeating: "x", count: 400)) })
        let changes = ElementChanges.between(previous: [:], current: current, order: Array(0..<100))
        XCTAssertLessThanOrEqual(changes.added.joined().utf8.count, ElementChanges.byteLimit)
        XCTAssertGreaterThan(changes.omitted, 0)
    }
}

final class ActionDeliveryTests: XCTestCase {
    func testOnlyWhatNeverLeftMayBeRepeated() {
        XCTAssertEqual(ActionDelivery.notDelivered.retry, "safe")
        for delivery in [ActionDelivery.deliveredUnverified, .deliveredVerified, .uncertain] {
            XCTAssertEqual(delivery.retry, "unsafe", delivery.rawValue)
        }
    }

    func testAPressThatTimesOutMayHaveLanded() {
        XCTAssertEqual(AXActionVerdict.of(raw: AXError.cannotComplete.rawValue), .uncertain)
        XCTAssertEqual(AXActionVerdict.of(raw: AXError.failure.rawValue), .uncertain)
    }

    func testAnActionTheElementDoesNotOfferLetsAnotherRoadGo() {
        XCTAssertEqual(AXActionVerdict.of(raw: AXError.actionUnsupported.rawValue), .unsupported)
    }

    func testAGoneElementIsStale() {
        XCTAssertEqual(AXActionVerdict.of(raw: AXError.invalidUIElement.rawValue), .stale)
    }

    func testAFailedActIsUncertainOnlyOnceSomethingWasSent() {
        XCTAssertEqual(ActionDelivery.failed(anythingSent: false), .notDelivered)
        XCTAssertEqual(ActionDelivery.failed(anythingSent: true), .uncertain)
    }

    func testADisabledControlWaits() {
        XCTAssertEqual(
            Actionability.of(enabled: false, frame: CGRect(x: 0, y: 0, width: 1, height: 1), previousFrame: nil, needsFrame: true),
            .wait("disabled")
        )
    }

    func testAnUnknownEnabledStateIsNotDisabled() {
        XCTAssertEqual(
            Actionability.of(enabled: nil, frame: CGRect(x: 0, y: 0, width: 1, height: 1), previousFrame: nil, needsFrame: true),
            .ready
        )
    }

    func testAMovingControlWaits() {
        XCTAssertEqual(
            Actionability.of(enabled: true, frame: CGRect(x: 9, y: 0, width: 1, height: 1), previousFrame: CGRect(x: 0, y: 0, width: 1, height: 1), needsFrame: true),
            .wait("moving")
        )
    }
}
