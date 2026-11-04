import AppKit
import ZeroCodeComputerUseMacOSCore
import XCTest
@testable import ZeroCodeComputerUseMacOS

/// Synthetic events and window rows only; no panel is ordered and no input is posted.
final class ShareControlPanelTests: XCTestCase {
    func testMissingOrBlankControlLabelKeepsTheHumanControlVisible() {
        XCTAssertEqual(ShareControlPanel.controlLabel(""), "Take control")
        XCTAssertEqual(ShareControlPanel.controlLabel(" \n"), "Take control")
        XCTAssertEqual(ShareControlPanel.controlLabel("제어권 가져오기"), "제어권 가져오기")
    }

    func testTheControlRestsAsOneRoundMarkAndOpensLeftwardsUnderThePointer() {
        let rest = ShareControlPanel.controlSize(labelWidth: 88, open: false)
        XCTAssertEqual(rest, CGSize(width: 32, height: 32))
        let open = ShareControlPanel.controlSize(labelWidth: 88.2, open: true)
        XCTAssertEqual(open, CGSize(width: 32 + 89 + 12, height: 32), "the whole name fits")
        let window = CGRect(x: 100, y: 50, width: 800, height: 600)
        let resting = ShareControlPanel.controlBounds(window: window, size: rest)
        let opened = ShareControlPanel.controlBounds(window: window, size: open)
        XCTAssertEqual(resting, CGRect(x: 860, y: 58, width: 32, height: 32))
        XCTAssertEqual(opened.maxX, resting.maxX, "the mark under the pointer does not move")
        XCTAssertEqual(opened.minY, resting.minY)
        XCTAssertTrue(opened.contains(resting))
    }

    func testTheRedrawnControlStillTakesOnlyAPhysicalPress() {
        MainActor.assumeIsolated {
            let button = ShareTakeButton(title: "Take control", target: nil, action: nil)
            XCTAssertFalse(button.accessibilityPerformPress())
            XCTAssertFalse(button.acceptsFirstResponder)
            XCTAssertNotNil(NSImage(systemSymbolName: ShareTakeLook.symbol, accessibilityDescription: nil), "its mark exists on the oldest macOS it runs on")
            XCTAssertEqual(button.accessibilityRole(), .button)
        }
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
        XCTAssertThrowsError(try ShareControlPanel.refuseInput(method: "type", params: [:], bounds: [], taken: state.takenBack)) {
            XCTAssertEqual(($0 as? ProviderError)?.code, "person_took_control")
        }
        // The window acknowledges the press, then the person hands back.
        state.update(target: "Sample", generation: 1, revision: 3, takenBack: true)
        XCTAssertTrue(state.takenBack)
        state.update(target: "Sample", generation: 2, revision: 4)
        XCTAssertFalse(state.takenBack)
        XCTAssertNoThrow(try ShareControlPanel.refuseInput(method: "type", params: [:], bounds: [], taken: state.takenBack))
    }

    func testPendingPressSurvivesATargetChangeUntilHandBack() throws {
        var state = ShareControlState()
        state.update(target: "Sample", generation: 0, revision: 1)
        XCTAssertTrue(state.takeControl(displayedRevision: 1))
        // The shell has not heard the press when an already shared app acts.
        state.update(target: "Another", generation: 0, revision: 2)
        XCTAssertEqual(state.target, "Another")
        XCTAssertTrue(state.takenBack)
        XCTAssertThrowsError(try ShareControlPanel.refuseInput(method: "type", params: [:], bounds: [], taken: state.takenBack)) {
            XCTAssertEqual(($0 as? ProviderError)?.code, "person_took_control")
        }
        state.update(target: "Another", generation: 1, revision: 3, takenBack: true)
        XCTAssertTrue(state.takenBack)
        state.update(target: "Another", generation: 2, revision: 4)
        XCTAssertFalse(state.takenBack)
        XCTAssertFalse(state.takeControl(displayedRevision: 1))
        XCTAssertNoThrow(try ShareControlPanel.refuseInput(method: "type", params: [:], bounds: [], taken: state.takenBack))
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
        let lanes = LaneLocks()
        let providerLock = lanes.lock(for: "desktop")
        providerLock.lock()
        defer { providerLock.unlock() }
        // A regression to the provider route times out, rather than hanging the suite.
        let result = Box<[String: Any]>()
        let answered = DispatchSemaphore(value: 0)
        Thread.detachNewThread {
            guard let threaded = try? JSONDecoder().decode(Request.self, from: line) else { answered.signal(); return }
            result.value = handleRequest(provider: Provider(), lanes: lanes, request: threaded,
                                         expectedToken: nil, authorizedPeer: true) as? [String: Any]
            answered.signal()
        }
        XCTAssertEqual(answered.wait(timeout: .now() + 1), .success)
        let answer = try XCTUnwrap(result.value)
        XCTAssertEqual(answer["ok"] as? Bool, true)
        let refused = try XCTUnwrap(handleRequest(provider: Provider(), lanes: lanes, request: request,
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
            XCTAssertThrowsError(try ShareControlPanel.refuseInput(method: method, params: params, bounds: [panel], taken: false), method)
        }
        XCTAssertNoThrow(try ShareControlPanel.refuseInput(method: "mouseClick", params: ["x": .number(610), "y": .number(140)], bounds: [panel], taken: false))
        XCTAssertThrowsError(try ShareControlPanel.refuseInput(method: "type", params: [:], bounds: [], taken: true))
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

/// Two apps in use (t-93286): stand-in windows and rows only; no panel is ordered.
extension ShareControlPanelTests {
    private final class FakePanel {
        let target: String
        var closed = false
        init(target: String) { self.target = target }
    }

    private func notice(_ panels: [(String, Double)]) -> [String: JSONValue] {
        ["revision": .number(7), "generation": .number(1), "target": .string(panels.last?.0 ?? ""),
         "panels": .array(panels.map { .object(["target": .string($0.0), "windowId": .number($0.1), "windowIndex": .null]) })]
    }

    private func row(_ window: UInt32, pid: pid_t, _ frame: CGRect) -> [String: Any] {
        [kCGWindowNumber as String: NSNumber(value: window), kCGWindowOwnerPID as String: NSNumber(value: pid),
         kCGWindowLayer as String: 0, kCGWindowBounds as String: frame.dictionaryRepresentation as NSDictionary]
    }

    private func candidate(_ window: UInt32, _ frame: CGRect) -> WindowCandidate {
        WindowCandidate(windowId: window, layer: 0, bounds: frame, title: nil, alpha: 1, isOnScreen: true, sharingState: nil)
    }

    func testEveryAppInUseAsksForItsOwnControlOnItsOwnWindow() {
        let specs = SharePanelSpec.list(notice([("com.example.browser", 41), ("com.example.chat", 52)]))
        XCTAssertEqual(specs, [
            SharePanelSpec(target: "com.example.browser", windowID: 41, windowIndex: nil),
            SharePanelSpec(target: "com.example.chat", windowID: 52, windowIndex: nil),
        ])
        // A notice naming one target and no list still has its control.
        XCTAssertEqual(SharePanelSpec.list(["target": .string("Sample"), "windowIndex": .number(1)]),
                       [SharePanelSpec(target: "Sample", windowID: nil, windowIndex: 1)])
        XCTAssertEqual(SharePanelSpec.list(["revision": .number(1)]), [])
        XCTAssertEqual(SharePanelSpec.list(["target": .string("Sample"), "panels": .array([])]), [], "nothing in use, no control")
    }

    func testTwoAppsGetTwoControlsAndOneCoveredHidesAlone() throws {
        let browser = CGRect(x: 0, y: 0, width: 800, height: 600)
        let chat = CGRect(x: 900, y: 0, width: 600, height: 600)
        let size = ShareControlPanel.controlSize(labelWidth: 0, open: false)
        let rows = [row(41, pid: 4_101, browser), row(52, pid: 5_201, chat)]
        let first = try XCTUnwrap(ShareControlPanel.placement(
            SharePanelSpec(target: "browser", windowID: 41, windowIndex: nil), pinned: nil, observed: nil,
            windows: [candidate(41, browser)], rows: rows, size: size))
        let second = try XCTUnwrap(ShareControlPanel.placement(
            SharePanelSpec(target: "chat", windowID: 52, windowIndex: nil), pinned: nil, observed: nil,
            windows: [candidate(52, chat)], rows: rows, size: size))
        XCTAssertEqual(first.windowId, 41)
        XCTAssertEqual(second.windowId, 52)
        XCTAssertTrue(browser.contains(first.bounds))
        XCTAssertTrue(chat.contains(second.bounds))
        XCTAssertFalse(first.bounds.intersects(second.bounds))
        // Agent input into either control is refused; between them it goes.
        for bounds in [first.bounds, second.bounds] {
            let inside: [String: JSONValue] = ["x": .number(bounds.midX), "y": .number(bounds.midY)]
            XCTAssertThrowsError(try ShareControlPanel.refuseInput(method: "mouseClick", params: inside, bounds: [first.bounds, second.bounds], taken: false)) {
                XCTAssertEqual(($0 as? ProviderError)?.code, "app_blocked")
            }
        }
        XCTAssertNoThrow(try ShareControlPanel.refuseInput(method: "mouseClick", params: ["x": .number(850), "y": .number(300)],
                                                           bounds: [first.bounds, second.bounds], taken: false))
        // Another window over the chat's corner hides the chat's control only.
        let over = row(77, pid: 7_701, chat.insetBy(dx: -10, dy: -10))
        let covered = [over] + rows
        XCTAssertNil(ShareControlPanel.placement(
            SharePanelSpec(target: "chat", windowID: 52, windowIndex: nil), pinned: nil, observed: nil,
            windows: [candidate(52, chat)], rows: covered, size: size))
        XCTAssertNotNil(ShareControlPanel.placement(
            SharePanelSpec(target: "browser", windowID: 41, windowIndex: nil), pinned: nil, observed: nil,
            windows: [candidate(41, browser)], rows: covered, size: size))
    }

    func testAPressOnEitherControlTakesBackEveryApp() throws {
        var state = ShareControlState()
        state.update(target: "chat", generation: 1, revision: 7)
        XCTAssertTrue(state.takeControl(displayedRevision: 7), "the one state every control shows")
        XCTAssertFalse(state.takeControl(displayedRevision: 7))
        XCTAssertThrowsError(try ShareControlPanel.refuseInput(method: "scroll", params: [:], bounds: [], taken: state.takenBack)) {
            XCTAssertEqual(($0 as? ProviderError)?.code, "person_took_control")
        }
        // Every control presses into the same take-back.
        let source = try String(contentsOfFile: URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().appendingPathComponent("../../Sources/ZeroCodeComputerUseMacOS/ShareTakeButton.swift").standardized.path, encoding: .utf8)
        XCTAssertTrue(source.contains("ShareControlPanel.takeControl(sourcePID:"))
    }

    func testControlsAreReusedAndOneThatLeavesIsClosed() {
        var roster = SharePanelRoster<FakePanel>()
        var made = 0
        let make = { (target: String) -> FakePanel in made += 1; return FakePanel(target: target) }
        roster.reconcile(["browser", "chat"], make: make, close: { $0.closed = true })
        XCTAssertEqual(roster.panels.count, 2)
        let chat = roster.panels["chat"]
        for _ in 0..<5 { roster.reconcile(["browser", "chat"], make: make, close: { $0.closed = true }) }
        XCTAssertEqual(made, 2, "a notice for the same apps makes no window")
        XCTAssertTrue(roster.panels["chat"] === chat)
        var closed: [String] = []
        roster.reconcile(["browser"], make: make, close: { closed.append($0.target); $0.closed = true })
        XCTAssertEqual(closed, ["chat"])
        XCTAssertEqual(chat?.closed, true, "the app that left takes its panel with it")
        XCTAssertEqual(roster.panels.count, 1)
        roster.reconcile([], make: make, close: { $0.closed = true })
        XCTAssertTrue(roster.panels.isEmpty)
        XCTAssertEqual(made, 2)
    }
}
