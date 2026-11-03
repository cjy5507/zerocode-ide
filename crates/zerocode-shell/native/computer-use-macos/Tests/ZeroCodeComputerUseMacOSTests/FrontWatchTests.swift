import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class FrontWatchTests: XCTestCase {
    private let person: pid_t = 100
    private let launched: pid_t = 200
    private let other: pid_t = 300

    /// A desk of pids: the front at each look, and who gets it back.
    private func watch(_ fronts: [pid_t?], pressedAt: Set<Int> = []) -> (given: [pid_t], stopped: Bool, looks: Int) {
        var desk = fronts
        var look = -1
        var given: [pid_t] = []
        var current: pid_t?
        var watching = FrontWatch(original: person, launched: launched)
        let stopped = watching.run(
            until: { desk.isEmpty },
            front: {
                look += 1
                current = desk.removeFirst() ?? current
                return current
            },
            personJustPressed: { pressedAt.contains(look) },
            giveBack: {
                given.append(self.person)
                current = self.person
            },
            pause: {}
        )
        return (given, stopped, look + 1)
    }

    func testAnAppThatTookTheFrontGivesItBack() {
        let run = watch([person, launched, person, person])
        XCTAssertEqual(run.given, [person])
        XCTAssertFalse(run.stopped)
    }

    func testAnAppThatComesForwardLateGivesItBackToo() {
        let run = watch(Array(repeating: person, count: 50) + [launched, person])
        XCTAssertEqual(run.given, [person])
    }

    func testNothingMovesWhenTheFrontStays() {
        XCTAssertEqual(watch([person, person, nil, person]).given, [])
    }

    func testThePersonGoingElsewhereEndsTheWatch() {
        let run = watch([person, other, launched, person])
        XCTAssertEqual(run.given, [], "the app they chose keeps the front")
        XCTAssertTrue(run.stopped)
        XCTAssertEqual(run.looks, 2)
    }

    func testThePersonPressingTheLaunchedAppKeepsIt() {
        let run = watch([person, launched, person], pressedAt: [1])
        XCTAssertEqual(run.given, [], "it is theirs now")
        XCTAssertTrue(run.stopped)
    }

    func testAnAppThatKeepsTakingTheFrontIsNotFought() {
        let run = watch(Array(repeating: launched, count: 9))
        XCTAssertEqual(run.given.count, FrontWatch.givesBack)
        XCTAssertTrue(run.stopped)
    }

    func testABlankMomentIsNoChoice() {
        var watching = FrontWatch(original: person, launched: launched)
        XCTAssertEqual(watching.see(front: nil, personJustPressed: false), .keepWatching)
        XCTAssertEqual(watching.see(front: launched, personJustPressed: false), .giveBack)
    }
}
