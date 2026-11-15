import XCTest
@testable import ZeroCodeComputerUseMacOS

/// The pointers that wait are drawn one a turn of the helper's main thread,
/// where the person's stop chord is heard (t-109788): the longest waiting
/// first, each once before any is drawn again.
final class PointerWaitingTests: XCTestCase {
    private func pointer(_ pid: pid_t, of agent: String = "claude") -> PointerKey {
        PointerKey(agent: agent, pid: pid)
    }

    func testThePointerThatWaitedLongestIsDrawnFirst() {
        var waiting = PointerWaiting<Int>()
        for pid in [11, 12, 13] as [pid_t] { waiting.enter(pointer(pid), at: Int(pid)) }
        XCTAssertEqual(waiting.next()?.key, pointer(11))
        XCTAssertEqual(waiting.next()?.key, pointer(12))
        XCTAssertEqual(waiting.next()?.key, pointer(13))
        XCTAssertNil(waiting.next()?.key)
    }

    func testAPointerThatWaitsAgainKeepsItsTurnAndTakesItsNewestPlace() {
        var waiting = PointerWaiting<Int>()
        waiting.enter(pointer(11), at: 1)
        waiting.enter(pointer(12), at: 2)
        waiting.enter(pointer(11), at: 3)
        let first = waiting.next()
        XCTAssertEqual(first?.key, pointer(11))
        XCTAssertEqual(first?.place, 3)
        XCTAssertEqual(waiting.next()?.place, 2)
        XCTAssertTrue(waiting.isEmpty, "the one that waited twice is drawn once")
    }

    func testOneAgentInTwoAppsAndTwoAgentsInOneAppEachWait() {
        var waiting = PointerWaiting<Int>()
        waiting.enter(pointer(11), at: 1)
        waiting.enter(pointer(12), at: 2)
        waiting.enter(pointer(11, of: "codex"), at: 3)
        XCTAssertEqual(waiting.next()?.place, 1)
        XCTAssertEqual(waiting.next()?.place, 2)
        XCTAssertEqual(waiting.next()?.place, 3)
    }

    func testOnlyTheFirstToWaitBeginsTheDrawing() {
        var waiting = PointerWaiting<Int>()
        XCTAssertTrue(waiting.enter(pointer(11), at: 1))
        XCTAssertFalse(waiting.enter(pointer(12), at: 2))
        XCTAssertFalse(waiting.enter(pointer(11), at: 3), "one that waits again begins nothing")
        _ = waiting.next()
        XCTAssertFalse(waiting.enter(pointer(13), at: 4), "another still waits: the turns go on")
        _ = waiting.next()
        _ = waiting.next()
        XCTAssertTrue(waiting.enter(pointer(11), at: 5), "none waited: the drawing is begun again")
    }

    func testForgottenNoneWaits() {
        var waiting = PointerWaiting<Int>()
        waiting.enter(pointer(11), at: 1)
        waiting.enter(pointer(12), at: 2)
        waiting.forget()
        XCTAssertTrue(waiting.isEmpty)
        XCTAssertNil(waiting.next()?.key)
        XCTAssertTrue(waiting.enter(pointer(11), at: 3), "the drawing is begun again after a stop")
    }
}
