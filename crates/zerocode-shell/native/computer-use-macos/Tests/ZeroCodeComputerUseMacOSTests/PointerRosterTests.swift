import XCTest
@testable import ZeroCodeComputerUseMacOS

/// An agent has one pointer for each app it works in, so an agent working in
/// four apps at once shows four (the person, 2026-11-15: "여러개 마우스가
/// 보여야하는거아냐?"). It wears its colour while any of them is in view.
final class PointerRosterTests: XCTestCase {
    func testOneAgentInFourAppsHasFourPointers() {
        var roster = PointerRoster()
        for pid in [11, 12, 13, 14] as [pid_t] { roster.shown(PointerKey(agent: "claude", pid: pid)) }
        XCTAssertEqual(roster.count, 4)
    }

    func testTheSameAgentInTheSameAppIsOnePointer() {
        var roster = PointerRoster()
        roster.shown(PointerKey(agent: "claude", pid: 11))
        roster.shown(PointerKey(agent: "claude", pid: 11))
        XCTAssertEqual(roster.count, 1)
    }

    func testTwoAgentsInOneAppHaveAPointerEach() {
        var roster = PointerRoster()
        roster.shown(PointerKey(agent: "claude", pid: 11))
        roster.shown(PointerKey(agent: "codex", pid: 11))
        XCTAssertEqual(roster.count, 2)
    }

    func testAnotherAgentsPointerInTheSameAppDoesNotKeepAnAgentInView() {
        var roster = PointerRoster()
        roster.shown(PointerKey(agent: "claude", pid: 11))
        roster.shown(PointerKey(agent: "codex", pid: 11))
        XCTAssertTrue(roster.gone(PointerKey(agent: "claude", pid: 11)))
        XCTAssertEqual(roster.count, 1)
    }

    func testAnAgentLeavesViewWithItsLastPointerOnly() {
        var roster = PointerRoster()
        roster.shown(PointerKey(agent: "claude", pid: 11))
        roster.shown(PointerKey(agent: "claude", pid: 12))
        XCTAssertFalse(roster.gone(PointerKey(agent: "claude", pid: 11)), "one pointer is still in view")
        XCTAssertTrue(roster.gone(PointerKey(agent: "claude", pid: 12)))
        XCTAssertFalse(roster.gone(PointerKey(agent: "claude", pid: 12)), "a pointer that already went")
        XCTAssertEqual(roster.count, 0)
    }

    func testManyPointersAreAllKept() {
        var roster = PointerRoster()
        for pid in 1...64 as ClosedRange<pid_t> { roster.shown(PointerKey(agent: "claude", pid: pid)) }
        XCTAssertEqual(roster.count, 64, "no pointer gives way to another: each goes when its own time is up")
    }
}
