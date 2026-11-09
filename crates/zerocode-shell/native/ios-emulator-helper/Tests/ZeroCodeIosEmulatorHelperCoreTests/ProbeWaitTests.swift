import XCTest

@testable import ZeroCodeIosEmulatorHelperCore

/// The probe keys are waited on by the wall clock, and only their signature lets them be deleted.
final class ProbeWaitTests: XCTestCase {
    /// A clock the test moves: each sleep and each read takes the time it is given.
    private final class Clock {
        var now = 0.0
        var reads = 0
    }

    private static func english(_ before: [String], _ after: [String]) -> String? {
        let count = { (lines: [String]) in lines.filter { $0.lowercased().contains("gk") }.count }
        return count(after) > count(before) ? "english" : nil
    }

    private func wait(_ screens: [[String]], readTakes: Double = 0, before: [String] = ["Search shops"])
        -> (ProbeSighting<String>, Clock)
    {
        let clock = Clock()
        let sighting = ProbeWait.wait(
            before: before,
            now: { clock.now },
            sleep: { clock.now += $0 },
            read: {
                clock.now += readTakes
                defer { clock.reads += 1 }
                return screens[min(clock.reads, screens.count - 1)]
            },
            recognize: Self.english
        )
        return (sighting, clock)
    }

    /// Safari's page field tells the tree late: the keys show on the third read and are recognized.
    func testKeysAPageShowsOnlyOnTheThirdReadAreRecognized() {
        let before = ["Search shops"]
        let (sighting, clock) = wait([before, before, ["Gk"]])
        XCTAssertEqual(sighting, .recognized("english"))
        XCTAssertEqual(clock.reads, 3)
    }

    /// A field that never shows them is unchanged, and the wait ends on the wall clock.
    func testKeysThatNeverShowEndTheWaitOnTheClockWithNothingToDelete() {
        let (sighting, clock) = wait([["Search shops"]])
        XCTAssertEqual(sighting, .unchanged)
        XCTAssertGreaterThanOrEqual(clock.now, ProbeWait.within)
        XCTAssertLessThan(clock.now, ProbeWait.within + ProbeWait.gap + 0.001)
    }

    /// Another value moving (a spinner, the page) is no sight of the keys: nothing is deleted.
    func testAnotherValueThatMovesIsNotTheKeys() {
        let (sighting, _) = wait([["Search shops", "Loading"], ["Search shops", "Loaded"]])
        XCTAssertEqual(sighting, .unrecognized)
    }

    /// A slow tree read counts against the same wall clock: fewer reads, not a longer wait.
    func testASlowReadOfTheTreeStillEndsWithinTheWait() {
        let (sighting, clock) = wait([["Search shops"]], readTakes: 0.5)
        XCTAssertEqual(sighting, .unchanged)
        XCTAssertLessThanOrEqual(clock.reads, 3)
        XCTAssertLessThan(clock.now, ProbeWait.within + ProbeWait.gap + 0.5 + 0.001)
    }

    /// A read that fails is no change: the wait goes on to the next read.
    func testAFailedReadIsSkipped() {
        let clock = Clock()
        let sighting = ProbeWait.wait(
            before: ["Search shops"],
            now: { clock.now },
            sleep: { clock.now += $0 },
            read: {
                defer { clock.reads += 1 }
                return clock.reads == 0 ? nil : ["gk"]
            },
            recognize: Self.english
        )
        XCTAssertEqual(sighting, .recognized("english"))
        XCTAssertEqual(clock.reads, 2)
    }
}
