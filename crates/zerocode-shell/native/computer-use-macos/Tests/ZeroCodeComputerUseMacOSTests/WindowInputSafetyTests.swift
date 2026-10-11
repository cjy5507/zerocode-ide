import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class WindowInputSafetyTests: XCTestCase {
    func testStopReleasesAtTheLastTargetedPointNotThePersonsCursor() throws {
        let clock = FakeClock()
        let poster = RecordingPoster()
        let hand = OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 73)
        let token = try hand.acquire(.request)
        let route = HandRoute.window(pid: 42, id: 100)
        try hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 10, y: 20, route: route), by: token)
        try hand.post(HandEvent(.buttonDrag(.left, clickState: 1), x: 30, y: 40, route: route), by: token)
        poster.movePointer(to: SmoothPointerPath.Point(x: 800, y: 900))
        _ = hand.stop(reason: StopReason.request)
        XCTAssertEqual(poster.releases, [HandEvent(.buttonUp(.left, clickState: 1), x: 30, y: 40, route: route)])
    }

    func testAnotherWindowsMovementDoesNotRelocateAHeldRelease() throws {
        let clock = FakeClock()
        let poster = RecordingPoster()
        let hand = OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 73)
        let token = try hand.acquire(.request)
        let route = HandRoute.window(pid: 42, id: 100)
        try hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: 10, y: 20, route: route), by: token)
        try hand.post(HandEvent(.buttonDrag(.left, clickState: 1), x: 30, y: 40, route: route), by: token)
        try hand.post(HandEvent(.pointerMove, x: 50, y: 60, route: .window(pid: 42, id: 101)), by: token)
        _ = hand.stop(reason: StopReason.request)
        XCTAssertEqual(poster.releases.first?.route, route)
        XCTAssertEqual(poster.releases.first?.x, 30)
        XCTAssertEqual(poster.releases.first?.y, 40)
    }

    func testReleaseCannotCrossRecipientWindows() throws {
        let clock = FakeClock()
        let poster = RecordingPoster()
        let hand = OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 73)
        let token = try hand.acquire(.request)
        try hand.post(HandEvent(.key(code: 0, down: true, modifier: 0), route: .window(pid: 42, id: 100)), by: token)
        try hand.post(HandEvent(.key(code: 0, down: false, modifier: 0), route: .window(pid: 42, id: 101)), by: token)
        XCTAssertTrue(poster.releases.isEmpty)
        XCTAssertEqual(hand.snapshot.held, [.key(0)])
        _ = hand.relinquish(token)
    }

    func testWindowCoordinatesNeverBecomeTheDesktopPointerFallback() throws {
        let clock = FakeClock()
        let poster = RecordingPoster()
        let hand = OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 73)
        let token = try hand.acquire(.request)
        try hand.post(HandEvent(.pointerMove, x: 300, y: 400), by: token)
        try hand.post(HandEvent(.pointerMove, x: 10, y: 20, route: .window(pid: 42, id: 100)), by: token)
        poster.forgetPointer()
        XCTAssertEqual(hand.pointerNow(), SmoothPointerPath.Point(x: 300, y: 400))
        _ = hand.relinquish(token)
    }

    func testModifiersCannotLeakIntoAnotherWindowsRelease() throws {
        let clock = FakeClock()
        let poster = RecordingPoster()
        let hand = OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 73)
        let token = try hand.acquire(.request)
        let shift: UInt64 = 0x20000
        try hand.post(HandEvent(.key(code: 56, down: true, modifier: shift), flags: shift, route: .window(pid: 42, id: 100)), by: token)
        try hand.post(HandEvent(.key(code: 0, down: true, modifier: 0), route: .window(pid: 42, id: 101)), by: token)
        _ = hand.stop(reason: StopReason.request)
        XCTAssertEqual(poster.releases.first?.flags, 0)
    }

    func testUnicodeStopsWithoutLeavingItsVirtualKeyHeld() throws {
        let clock = FakeClock()
        let poster = RecordingPoster()
        let hand = OperatorHand(poster: poster, clock: clock, sleeper: ScriptedSleeper(clock: clock), tag: 73)
        let token = try hand.acquire(.request)
        let route = HandRoute.window(pid: 42, id: 100)
        try hand.post(HandEvent(.unicode("한🙂", down: true), route: route), by: token)
        _ = hand.stop(reason: StopReason.request)
        XCTAssertEqual(poster.releases, [HandEvent(.unicode("한🙂", down: false), route: route)])
        XCTAssertThrowsError(try hand.post(HandEvent(.unicode("next", down: true), route: route), by: token))
    }
}
