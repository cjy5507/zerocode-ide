import AppKit
import XCTest
@testable import ZeroCodeComputerUseMacOS

/// Synthetic events and window rows only; no panel is ordered and no input is posted.
final class ShareControlPanelTests: XCTestCase {
    func testMissingOrBlankControlLabelKeepsTheHumanControlVisible() {
        XCTAssertEqual(ShareControlPanel.controlLabel(""), "Take control")
        XCTAssertEqual(ShareControlPanel.controlLabel(" \n"), "Take control")
        XCTAssertEqual(ShareControlPanel.controlLabel("제어권 가져오기"), "제어권 가져오기")
    }

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
        XCTAssertFalse(state.takeControl(displayedRevision: 0))
        XCTAssertTrue(state.takeControl(displayedRevision: 1))
        XCTAssertFalse(state.takeControl(displayedRevision: 1))
        state.update(target: "Sample", generation: 0, revision: 1)
        XCTAssertTrue(state.takenBack)
        state.update(target: "Sample", generation: 2, revision: 2)
        XCTAssertFalse(state.takenBack)
        state.update(target: nil, generation: 2, revision: 3)
        XCTAssertNil(state.target)
        XCTAssertFalse(state.takeControl(displayedRevision: 1))
    }

    func testPendingPressSurvivesANewerShareRenoticeUntilHandBack() throws {
        var state = ShareControlState()
        state.update(target: "Sample", generation: 0, revision: 1)
        XCTAssertTrue(state.takeControl(displayedRevision: 1))
        state.update(target: "Sample", generation: 0, revision: 2)
        XCTAssertTrue(state.takenBack)
        XCTAssertThrowsError(try ShareControlPanel.refuseInput(method: "type", params: [:], bounds: nil, taken: state.takenBack)) {
            XCTAssertEqual(($0 as? ProviderError)?.code, "person_took_control")
        }
        // The window acknowledges the press, then the person hands back.
        state.update(target: "Sample", generation: 1, revision: 3, takenBack: true)
        XCTAssertTrue(state.takenBack)
        state.update(target: "Sample", generation: 2, revision: 4)
        XCTAssertFalse(state.takenBack)
        XCTAssertNoThrow(try ShareControlPanel.refuseInput(method: "type", params: [:], bounds: nil, taken: state.takenBack))
    }

    func testPendingPressSurvivesATargetChangeUntilHandBack() throws {
        var state = ShareControlState()
        state.update(target: "Sample", generation: 0, revision: 1)
        XCTAssertTrue(state.takeControl(displayedRevision: 1))
        // The shell has not heard the press when an already shared app acts.
        state.update(target: "Another", generation: 0, revision: 2)
        XCTAssertEqual(state.target, "Another")
        XCTAssertTrue(state.takenBack)
        XCTAssertThrowsError(try ShareControlPanel.refuseInput(method: "type", params: [:], bounds: nil, taken: state.takenBack)) {
            XCTAssertEqual(($0 as? ProviderError)?.code, "person_took_control")
        }
        state.update(target: "Another", generation: 1, revision: 3, takenBack: true)
        XCTAssertTrue(state.takenBack)
        state.update(target: "Another", generation: 2, revision: 4)
        XCTAssertFalse(state.takenBack)
        XCTAssertFalse(state.takeControl(displayedRevision: 1))
        XCTAssertNoThrow(try ShareControlPanel.refuseInput(method: "type", params: [:], bounds: nil, taken: state.takenBack))
    }

    func testPendingPressSurvivesTargetChangesAwayAndBack() {
        var state = ShareControlState()
        state.update(target: "Sample", generation: 0, revision: 1)
        XCTAssertTrue(state.takeControl(displayedRevision: 1))
        state.update(target: "Another", generation: 0, revision: 2)
        state.update(target: "Sample", generation: 0, revision: 3)
        XCTAssertTrue(state.takenBack)
    }

    func testAStaleDisplayedTargetCannotStartANewPress() {
        var state = ShareControlState()
        state.update(target: "Sample", generation: 0, revision: 1)
        state.update(target: "Another", generation: 0, revision: 2)
        XCTAssertFalse(state.takeControl(displayedRevision: 1))
        XCTAssertFalse(state.takenBack)
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

extension ShareControlPanelTests {
    func testTakeControlCancelsTheCurrentHoldWithoutPostingInputOrLeavingAGlobalStop() throws {
        XCTAssertThrowsError(try OperatorHandHost.whileHeld {
            ShareControlPanel.cancelInput()
            try OperatorHandHost.sleep(nanoseconds: 0)
        })
        // A fresh request can acquire after the shell hands back. A persistent
        // global stop here would incorrectly require the separate Resume control.
        XCTAssertNoThrow(try OperatorHandHost.whileHeld {})
    }

    func testPanelStateRoadDoesNotWaitForTheProviderLockAndRequiresAuthentication() throws {
        let params: [String: Any] = ["revision": 100, "generation": 0, "takenBack": false]
        let line = try JSONSerialization.data(withJSONObject: ["id": 1, "method": "sharePanel", "params": params])
        let request = try JSONDecoder().decode(Request.self, from: line)
        let providerLock = NSLock()
        providerLock.lock()
        defer { providerLock.unlock() }
        // A regression to the provider route times out, rather than hanging the suite.
        let result = Box<[String: Any]>()
        let answered = DispatchSemaphore(value: 0)
        Thread.detachNewThread {
            guard let threaded = try? JSONDecoder().decode(Request.self, from: line) else { answered.signal(); return }
            result.value = handleRequest(provider: Provider(), lock: providerLock, request: threaded,
                                         expectedToken: nil, authorizedPeer: true) as? [String: Any]
            answered.signal()
        }
        XCTAssertEqual(answered.wait(timeout: .now() + 1), .success)
        let answer = try XCTUnwrap(result.value)
        XCTAssertEqual(answer["ok"] as? Bool, true)
        let refused = try XCTUnwrap(handleRequest(provider: Provider(), lock: providerLock, request: request,
                                                 expectedToken: "fixture-token", authorizedPeer: true) as? [String: Any])
        XCTAssertEqual(refused["ok"] as? Bool, false)
    }
}

extension ShareControlPanelTests {
    func testEveryDesktopPointerPathRefusesSyntheticPanelBounds() {
        let panel = CGRect(x: 400, y: 100, width: 192, height: 32)
        let cases: [(String, [String: JSONValue])] = [
            ("mouseClick", ["x": .number(410), "y": .number(110)]),
            ("mouseMove", ["x": .number(410), "y": .number(110)]),
            ("mouseScroll", ["x": .number(410), "y": .number(110)]),
            ("mouseDrag", ["fromX": .number(410), "fromY": .number(110), "toX": .number(610), "toY": .number(140)]),
            ("mouseDrag", ["fromX": .number(610), "fromY": .number(140), "toX": .number(410), "toY": .number(110)]),
        ]
        for (method, params) in cases {
            XCTAssertThrowsError(try ShareControlPanel.refuseInput(method: method, params: params, bounds: panel, taken: false), method)
        }
        XCTAssertNoThrow(try ShareControlPanel.refuseInput(method: "mouseClick", params: ["x": .number(610), "y": .number(140)], bounds: panel, taken: false))
        XCTAssertThrowsError(try ShareControlPanel.refuseInput(method: "type", params: [:], bounds: nil, taken: true))
    }

    func testObservationDropsHelperRowsAndDirectHelperTargetsAreRefusedBeforeAccessibility() {
        let rows: [[String: Any]] = [
            [kCGWindowOwnerPID as String: getpid(), kCGWindowNumber as String: 10],
            [kCGWindowOwnerPID as String: pid_t(42), kCGWindowNumber as String: 11],
        ]
        let visible = ShareControlPanel.agentWindows(rows)
        XCTAssertEqual(visible.count, 1)
        XCTAssertEqual(visible.first?[kCGWindowNumber as String] as? Int, 11)
        XCTAssertThrowsError(try Provider().handle(method: "getAppState", params: ["app": .string("pid:\(getpid())")])) { error in
            XCTAssertEqual((error as? ProviderError)?.code, "app_blocked")
        }
    }
}

extension ShareControlPanelTests {
    func testPanelChangeWakesTheExistingWatchWithoutChangingTheGlobalStop() throws {
        let before = OperatorGuardHost.status()
        let panel = ShareControlPanel.status()
        let revision = try XCTUnwrap(before["stopRevision"] as? Int)
        let event = try XCTUnwrap(panel["event"] as? Int)
        let stamp = try XCTUnwrap(panel["revision"] as? Int)
        let result = Box<[String: Any]>()
        let answered = DispatchSemaphore(value: 0)
        Thread.detachNewThread {
            result.value = OperatorGuardHost.watch(after: UInt64(revision), waitMs: 5_000, afterShare: event)
            answered.signal()
        }
        XCTAssertEqual(answered.wait(timeout: .now() + 0.05), .timedOut)
        // No target means this fixture can never order a panel on the screen.
        try ShareControlPanel.update(["revision": .number(Double(stamp + 1)), "generation": .number(0)])
        XCTAssertEqual(answered.wait(timeout: .now() + 1), .success)
        XCTAssertEqual(result.value?["stopRevision"] as? Int, revision)
        let heard = try XCTUnwrap(result.value?["shareControl"] as? [String: Any])
        XCTAssertNotEqual(heard["event"] as? Int, event)
    }
}
