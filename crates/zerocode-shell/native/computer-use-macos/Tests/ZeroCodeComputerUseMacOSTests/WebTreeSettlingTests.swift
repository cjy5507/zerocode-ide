import Foundation
import XCTest
import ZeroCodeComputerUseMacOSCore

/// A fake clock and a fake tree: nothing waits for real and no app is read.
final class WebTreeSettlingTests: XCTestCase {
    private let frame = [
        WebTreeSettling.Part(role: "AXButton", subrole: "AXCloseButton", empty: true),
        WebTreeSettling.Part(role: "AXButton", subrole: "AXMinimizeButton", empty: true),
        WebTreeSettling.Part(role: "AXButton", subrole: "AXZoomButton", empty: true),
        WebTreeSettling.Part(role: "AXButton", subrole: "AXFullScreenButton", empty: true),
    ]

    /// A window whose tree stands at `standsAt` seconds on the fake clock;
    /// each pause moves the clock on by the helper's poll.
    private final class FakeApp {
        var clock = Date(timeIntervalSince1970: 1_000)
        let standsAt: Date?
        var pauses = 0
        init(standsAfter: TimeInterval?) {
            standsAt = standsAfter.map { Date(timeIntervalSince1970: 1_000).addingTimeInterval($0) }
        }
        func read() -> Bool { standsAt.map { clock >= $0 } ?? false }
        func pause() {
            pauses += 1
            clock = clock.addingTimeInterval(TimeInterval(WebAccessibility.pollMicros) / 1_000_000)
        }
    }

    private func settle(_ app: FakeApp, switchedOnAgo: TimeInterval?, stopped: @escaping () -> Bool = { false }) -> WebTreeSettling.Settled<Bool> {
        WebTreeSettling.settle(
            switchedOnAt: switchedOnAgo.map { app.clock.addingTimeInterval(-$0) },
            now: { app.clock },
            read: { app.read() },
            frameOnly: { !$0 },
            pause: { app.pause() },
            stopped: stopped
        )
    }

    func testAWindowShowingOnlyItsFrameIsReadAgainUntilTheTreeStands() {
        let app = FakeApp(standsAfter: 0.7)
        let settled = settle(app, switchedOnAgo: 0)
        XCTAssertTrue(settled.look, "the look that answers holds the content")
        XCTAssertFalse(settled.building)
        XCTAssertGreaterThan(settled.reads, 1)
        // It answers within one poll of the tree standing.
        let poll = TimeInterval(WebAccessibility.pollMicros) / 1_000_000
        XCTAssertGreaterThanOrEqual(settled.waitedSeconds, 0.7)
        XCTAssertLessThan(settled.waitedSeconds, 0.7 + poll + 0.000_001)
    }

    func testATreeThatDoesNotStandInTimeIsAnsweredAsStillBuilding() {
        let app = FakeApp(standsAfter: nil)
        let settled = settle(app, switchedOnAgo: 0)
        XCTAssertFalse(settled.look)
        XCTAssertTrue(settled.building, "a frame is not given as the app's whole content")
        XCTAssertGreaterThanOrEqual(settled.waitedSeconds, WebTreeSettling.waitSeconds)
        let poll = TimeInterval(WebAccessibility.pollMicros) / 1_000_000
        XCTAssertLessThan(settled.waitedSeconds, WebTreeSettling.waitSeconds + poll + 0.000_001, "the wait has its cap")
        XCTAssertEqual(settled.reads, app.pauses + 1)
    }

    func testANativeAppIsReadOnceAndNeverWaitedFor() {
        let app = FakeApp(standsAfter: nil)
        let settled = settle(app, switchedOnAgo: nil)
        XCTAssertEqual(settled.reads, 1)
        XCTAssertEqual(app.pauses, 0)
        XCTAssertFalse(settled.building, "a native app's look is what it is")
    }

    func testAnAppSwitchedOnLongAgoIsNotWaitedFor() {
        let app = FakeApp(standsAfter: nil)
        let settled = settle(app, switchedOnAgo: WebAccessibility.settlesSeconds + 1)
        XCTAssertEqual(settled.reads, 1)
        XCTAssertEqual(app.pauses, 0)
        XCTAssertFalse(settled.building)
    }

    func testAWindowWithContentAtOnceIsReadOnce() {
        let app = FakeApp(standsAfter: 0)
        let settled = settle(app, switchedOnAgo: 0)
        XCTAssertEqual(settled.reads, 1)
        XCTAssertEqual(app.pauses, 0)
        XCTAssertFalse(settled.building)
    }

    func testAStoppedOperatorEndsTheWait() {
        let app = FakeApp(standsAfter: nil)
        let settled = settle(app, switchedOnAgo: 0, stopped: { true })
        XCTAssertEqual(app.pauses, 0)
        XCTAssertTrue(settled.building)
    }

    func testTheFrameIsItsControlsAndEmptyContainers() {
        XCTAssertTrue(WebTreeSettling.isFrameOnly(frame))
        XCTAssertTrue(WebTreeSettling.isFrameOnly([]))
        XCTAssertTrue(WebTreeSettling.isFrameOnly(frame + [WebTreeSettling.Part(role: "AXWebArea", subrole: nil, empty: true)]))
        XCTAssertFalse(WebTreeSettling.isFrameOnly(frame + [WebTreeSettling.Part(role: "AXWebArea", subrole: nil, empty: false)]),
                       "a web area with something inside is content")
        XCTAssertFalse(WebTreeSettling.isFrameOnly(frame + [WebTreeSettling.Part(role: "AXStaticText", subrole: nil, empty: false)]))
        XCTAssertFalse(WebTreeSettling.isFrameOnly(frame + [WebTreeSettling.Part(role: "AXButton", subrole: nil, empty: true)]),
                       "a control of the app's own is content, even unnamed")
        let many = Array(repeating: frame[0], count: WebTreeSettling.mostFrameParts + 1)
        XCTAssertFalse(WebTreeSettling.isFrameOnly(many))
    }

    func testTheLookSaysWhyAndTheHelperMarksIt() throws {
        XCTAssertTrue(WebTreeSettling.buildingLine(app: "Sample").contains("Look again"))
        let source = try String(
            contentsOf: URL(fileURLWithPath: #filePath).deletingLastPathComponent()
                .appendingPathComponent("../../Sources/ZeroCodeComputerUseMacOS/main.swift").standardized,
            encoding: .utf8
        )
        let start = try XCTUnwrap(source.range(of: "private func buildSnapshot("))
        let end = try XCTUnwrap(source.range(of: "private func renderSnapshot(", range: start.upperBound..<source.endIndex))
        let build = String(source[start.lowerBound..<end.lowerBound])
        XCTAssertTrue(build.contains("WebTreeSettling.settle("))
        XCTAssertTrue(build.contains("settled.building ? WebTreeSettling.buildingCode"))
        // Only a look at the whole window waits: a surface or a subtree is not the frame.
        XCTAssertTrue(build.contains("shape.surface == nil && shape.root == nil ? webAccessibilityOnAt[app.pid] : nil"))
    }
}
