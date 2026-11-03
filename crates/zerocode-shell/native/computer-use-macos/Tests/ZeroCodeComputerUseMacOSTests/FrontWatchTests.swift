import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class FrontWatchTests: XCTestCase {
    private let person: pid_t = 100
    private let launched: pid_t = 200
    private let other: pid_t = 300

    /// A desk of pids: the front at each look, and who gets it back.
    private func watch(_ fronts: [pid_t?], pressedAt: Set<Int> = [], quitFrom: Int? = nil) -> (given: [pid_t], stopped: Bool, looks: Int) {
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
            originalAlive: { quitFrom.map { look < $0 } ?? true },
            personActed: { pressedAt.contains { $0 <= look } },
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
        XCTAssertEqual(watching.see(front: nil, originalAlive: true, personActed: false), .keepWatching)
        XCTAssertEqual(watching.see(front: launched, originalAlive: true, personActed: false), .giveBack)
    }

    /// The person quit their app meanwhile: giving it back would open it again.
    func testAnAppThePersonQuitIsNotOpenedAgain() {
        let run = watch([person, launched, launched], quitFrom: 1)
        XCTAssertEqual(run.given, [])
        XCTAssertTrue(run.stopped)
        XCTAssertEqual(run.looks, 2)
    }

    /// Typed into Spotlight a while before the launched app came forward: what
    /// came forward after any input of the person's since the launch is theirs.
    func testAnythingThePersonDidSinceTheLaunchLeavesTheFrontAlone() {
        let run = watch([person, person, person, launched, person], pressedAt: [0])
        XCTAssertEqual(run.given, [])
        XCTAssertTrue(run.stopped)
    }

    // MARK: - The person's input, from the system's table

    func testInputOlderThanTheLaunchIsNotTheirs() {
        let reading = DesktopBorrow.Reading(sinceKey: 5, sincePointer: 9)
        XCTAssertFalse(FrontWatch.personActed(sinceLaunch: 2, reading: reading, sinceAnyInput: 5))
    }

    func testAKeyAPointerOrAGestureSinceTheLaunchIsTheirs() {
        XCTAssertTrue(FrontWatch.personActed(sinceLaunch: 2, reading: .init(sinceKey: 1.5, sincePointer: 9), sinceAnyInput: 1.5), "a key")
        XCTAssertTrue(FrontWatch.personActed(sinceLaunch: 2, reading: .init(sinceKey: 9, sincePointer: 0.1), sinceAnyInput: 0.1), "the pointer")
        XCTAssertTrue(FrontWatch.personActed(sinceLaunch: 2, reading: .init(sinceKey: 9, sincePointer: 9), sinceAnyInput: 1), "a swipe the two leave out")
        XCTAssertTrue(FrontWatch.personActed(sinceLaunch: 2, reading: .init(sinceKey: 9, sincePointer: 9, buttonHeld: true), sinceAnyInput: 9), "a button held down")
    }

    /// The helper's own desktop posts go through the HID tap, which the table
    /// may count: an age that matches one is that post, not the person.
    func testTheHelpersOwnDesktopInputIsNotThePersons() {
        let reading = DesktopBorrow.Reading(sinceKey: 0.3, sincePointer: 9, sinceOwnKey: 0.28)
        XCTAssertFalse(FrontWatch.personActed(sinceLaunch: 2, reading: reading, sinceAnyInput: 0.3))
    }
}
