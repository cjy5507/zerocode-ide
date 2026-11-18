import CoreGraphics
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class FrontWatchTests: XCTestCase {
    private let person: pid_t = 100
    private let launched: pid_t = 200
    private let other: pid_t = 300

    /// A desk of pids: the front at each look, and who gets it back.
    private func watch(_ fronts: [pid_t?], pressedAt: Set<Int> = [], quitFrom: Int? = nil, chose: ((Int) -> Bool)? = nil) -> (given: [pid_t], stopped: Bool, looks: Int) {
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
            personChose: { chose?(look) ?? pressedAt.contains { $0 <= look } },
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
        XCTAssertEqual(watching.see(front: nil, originalAlive: true, personChose: false), .keepWatching)
        XCTAssertEqual(watching.see(front: launched, originalAlive: true, personChose: false), .giveBack)
    }

    /// The person quit their app meanwhile: giving it back would open it again.
    func testAnAppThePersonQuitIsNotOpenedAgain() {
        let run = watch([person, launched, launched], quitFrom: 1)
        XCTAssertEqual(run.given, [])
        XCTAssertTrue(run.stopped)
        XCTAssertEqual(run.looks, 2)
    }

    /// Pressed a while before the launched app came forward: what came forward
    /// after the person chose is theirs.
    func testAChoiceOfThePersonsSinceTheLaunchLeavesTheFrontAlone() {
        let run = watch([person, person, person, launched, person], pressedAt: [0])
        XCTAssertEqual(run.given, [])
        XCTAssertTrue(run.stopped)
    }

    /// TR-21 (11-17): the person typed on in their own app while the launched
    /// one came forward. Their keys are no choice of it: it goes back, and the
    /// watch goes on.
    func testTypingOnWhileTheLaunchedAppTakesTheFrontGivesItBack() {
        let typing = DesktopBorrow.Reading(sinceKey: 0.05, sincePointer: 0.2, modifierHeld: true, sinceModifier: 0.1)
        let run = watch([person, launched, person, person], chose: { _ in
            FrontWatch.personChose(sinceLaunch: 2, reading: typing, commandHeld: false, sinceClick: 40)
        })
        XCTAssertEqual(run.given, [person], "the launched app does not keep the person's keys")
        XCTAssertFalse(run.stopped)
    }

    // MARK: - The person's choice, from the system's table

    func testAPressOlderThanTheLaunchIsNoChoice() {
        let reading = DesktopBorrow.Reading(sinceKey: 5, sincePointer: 9)
        XCTAssertFalse(FrontWatch.personChose(sinceLaunch: 2, reading: reading, commandHeld: false, sinceClick: 5))
    }

    /// Typing — ⇧ held for a capital too — and moving the pointer choose no app.
    func testTypingAndMovingThePointerSinceTheLaunchChooseNoApp() {
        let typing = DesktopBorrow.Reading(sinceKey: 0.05, sincePointer: 9, modifierHeld: true, sinceModifier: 0.05)
        XCTAssertFalse(FrontWatch.personChose(sinceLaunch: 2, reading: typing, commandHeld: false, sinceClick: 40), "keys")
        let moving = DesktopBorrow.Reading(sinceKey: 9, sincePointer: 0.05)
        XCTAssertFalse(FrontWatch.personChose(sinceLaunch: 2, reading: moving, commandHeld: false, sinceClick: 40), "the pointer moving")
    }

    func testAPressAButtonHeldOrCommandHeldIsTheirChoice() {
        let still = DesktopBorrow.Reading(sinceKey: 9, sincePointer: 9)
        XCTAssertTrue(FrontWatch.personChose(sinceLaunch: 2, reading: still, commandHeld: false, sinceClick: 1.5), "a press since the launch")
        XCTAssertTrue(FrontWatch.personChose(sinceLaunch: 2, reading: .init(sinceKey: 9, sincePointer: 9, buttonHeld: true), commandHeld: false, sinceClick: 40), "a button held down")
        XCTAssertTrue(FrontWatch.personChose(sinceLaunch: 2, reading: still, commandHeld: true, sinceClick: 40), "⌘ held, as ⌘Tab")
    }

    /// ⌘ chooses an app (⌘Tab, ⌘Space); ⇧ is typing, and ⌃ ⌥ go with keys in the
    /// app in front — a remote desktop's copy and paste.
    func testOnlyCommandChoosesAnApp() {
        XCTAssertTrue(FrontWatch.choosesAnApp([.maskCommand]))
        XCTAssertTrue(FrontWatch.choosesAnApp([.maskCommand, .maskShift]))
        XCTAssertFalse(FrontWatch.choosesAnApp([]))
        XCTAssertFalse(FrontWatch.choosesAnApp([.maskShift]), "a capital letter")
        XCTAssertFalse(FrontWatch.choosesAnApp([.maskControl]), "⌃C in a remote desktop")
        XCTAssertFalse(FrontWatch.choosesAnApp([.maskAlternate]), "⌥ for an accent")
    }

    /// The helper's own desktop presses go through the HID tap, which the table
    /// may count: an age that matches one is that press, not the person.
    func testTheHelpersOwnPressIsNotThePersons() {
        let reading = DesktopBorrow.Reading(sinceKey: 9, sincePointer: 0.3, sinceOwnPointer: 0.28)
        XCTAssertFalse(FrontWatch.personChose(sinceLaunch: 2, reading: reading, commandHeld: false, sinceClick: 0.3))
    }
}
