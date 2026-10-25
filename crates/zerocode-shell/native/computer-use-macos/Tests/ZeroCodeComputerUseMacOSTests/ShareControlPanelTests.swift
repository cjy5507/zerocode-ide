import AppKit
import XCTest
@testable import ZeroCodeComputerUseMacOS

/// Synthetic events and window rows only; no panel is ordered and no input is posted.
final class ShareControlPanelTests: XCTestCase {
    func testPanelNeverTakesKeyboardOrMainWindow() {
        MainActor.assumeIsolated {
            let panel = ShareTakePanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            XCTAssertFalse(panel.canBecomeKey)
            XCTAssertFalse(panel.canBecomeMain)
            XCTAssertTrue(panel.styleMask.contains(.nonactivatingPanel))
        }
    }

    func testOnlyPhysicalMouseEventsCanTakeControl() {
        XCTAssertTrue(ShareControlState.humanMouse(sourcePID: 0, sourceUserData: 0))
        XCTAssertFalse(ShareControlState.humanMouse(sourcePID: 42, sourceUserData: 0))
        XCTAssertFalse(ShareControlState.humanMouse(sourcePID: 0, sourceUserData: 123))
    }

    func testTakeControlInvalidatesInflightAndStaleUpdatesCannotRestoreIt() {
        var state = ShareControlState()
        state.update(target: "Sample", generation: 0, revision: 1)
        XCTAssertTrue(state.takeControl())
        XCTAssertFalse(state.takeControl())
        state.update(target: "Sample", generation: 0, revision: 1)
        XCTAssertTrue(state.takenBack)
        state.update(target: "Sample", generation: 2, revision: 2)
        XCTAssertFalse(state.takenBack)
        state.update(target: nil, generation: 2, revision: 3)
        XCTAssertNil(state.target)
        XCTAssertFalse(state.takeControl())
    }

    func testHelperIsExcludedAndPanelCoordinatesAreBlockedEvenWithAllowSelf() throws {
        XCTAssertTrue(ShareControlPanel.isHelperPID(getpid()))
        XCTAssertThrowsError(try ShareControlPanel.refuseHelper(pid: getpid()))
        let source = try String(contentsOfFile: URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().appendingPathComponent("../../Sources/ZeroCodeComputerUseMacOS/main.swift").standardized.path, encoding: .utf8)
        let guardRange = try XCTUnwrap(source.range(of: "try ShareControlPanel.refuseInput(method: method, params: params)"))
        let bypassRange = try XCTUnwrap(source.range(of: "if params[\"allowSelf\"]?.bool != true"))
        XCTAssertLessThan(guardRange.lowerBound, bypassRange.lowerBound)
        XCTAssertTrue(source.contains("!ShareControlPanel.isHelperPID(pid)"))
    }
}
