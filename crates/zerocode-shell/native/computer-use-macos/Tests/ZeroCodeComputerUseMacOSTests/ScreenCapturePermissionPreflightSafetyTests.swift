import XCTest

final class ScreenCapturePermissionPreflightSafetyTests: XCTestCase {
    func testPermissionPreflightCannotRequestOrCaptureTheScreen() throws {
        let source = try computerUseSource()
        let start = try XCTUnwrap(source.range(of: "\nfunc screenCaptureTrusted()"))
        let end = try XCTUnwrap(
            source.range(of: "private func requestScreenCaptureAccess()", range: start.upperBound..<source.endIndex)
        )
        let preflightSource = source[start.lowerBound..<end.lowerBound]

        XCTAssertTrue(preflightSource.contains("CGPreflightScreenCaptureAccess()"))
        XCTAssertTrue(preflightSource.contains("timeoutMs: 2_000"))
        XCTAssertFalse(preflightSource.contains("CGRequestScreenCaptureAccess()"))
        XCTAssertFalse(preflightSource.contains("CGWindowListCreateImage"))
        XCTAssertFalse(preflightSource.contains("SCScreenshotManager"))
    }

    func testExplicitPermissionRequestActuallyCallsBothOSRequestAPIs() throws {
        let source = try computerUseSource()
        XCTAssertTrue(source.contains("AXIsProcessTrustedWithOptions"))
        XCTAssertTrue(source.contains("kAXTrustedCheckOptionPrompt"))
        XCTAssertTrue(source.contains("--permission-request-file"))
    }

    /// A window asked for alone is that window's own picture (I-t123731-rc1):
    /// the frame of a step sent in the background, which a person's window
    /// over the target must never reach. Taking it needs screen recording
    /// and a live window, so the property is read off the source, as the
    /// preflight's is: pictured after the preflight, by the window's number,
    /// refusing the helper's own panels, and never by the screen.
    func testAWindowAloneIsPicturedByItsNumberAfterThePreflightAndNeverByTheScreen() throws {
        let source = try computerUseSource()
        let frame = try XCTUnwrap(source.range(of: "static func frame(params: [String: JSONValue]) throws -> Frame {"))
        let rest = frame.upperBound..<source.endIndex
        let preflight = try XCTUnwrap(source.range(of: "guard screenCaptureTrusted() else", range: rest))
        let alone = try XCTUnwrap(source.range(of: "return try windowOnly(params: params, among: all)", range: rest))
        let display = try XCTUnwrap(source.range(of: "CGDisplayCreateImage(", range: rest))
        XCTAssertLessThan(preflight.lowerBound, alone.lowerBound, "screen recording is checked before a window is pictured")
        XCTAssertLessThan(alone.lowerBound, display.lowerBound, "a window asked for alone is answered before the display is taken")

        let start = try XCTUnwrap(source.range(of: "private static func windowOnly(params:"))
        let end = try XCTUnwrap(source.range(of: "\n    }\n", range: start.upperBound..<source.endIndex))
        let body = source[start.lowerBound..<end.upperBound]
        XCTAssertTrue(body.contains("WindowCapture.captureImage(windowId: windowId, bounds: bounds)"))
        XCTAssertTrue(body.contains("try ShareControlPanel.refuseHelper(pid: owner)"))
        for screen in ["CGDisplayCreateImage", "captureRegion(", "optionOnScreenOnly", "cropping(to:", "kCGNullWindowID"] {
            XCTAssertFalse(body.contains(screen), "a window alone never falls back to the screen: \(screen)")
        }
        // Only that picture says it is one window alone, and the answer says
        // so: the window keeps no picture that does not.
        XCTAssertTrue(body.contains("windowOnly: windowId"))
        XCTAssertEqual(source.components(separatedBy: "windowOnly: windowId").count, 2, "one picture marks itself as a window alone")
        XCTAssertTrue(source.contains("answer[windowOnlyKey] = Int(alone)"))
    }

    /// A window is pictured alone only while the process the act found owning
    /// it still owns its number (I-t123731-rc1, review t-128744 §1–2): the
    /// request names both, and a number the window server gave to another
    /// process since is refused before anything is captured.
    func testAWindowAloneIsPicturedOnlyWhileTheProcessTheActFoundStillOwnsIt() throws {
        let source = try computerUseSource()
        let start = try XCTUnwrap(source.range(of: "private static func windowOnly(params:"))
        let end = try XCTUnwrap(source.range(of: "\n    }\n", range: start.upperBound..<source.endIndex))
        let body = String(source[start.lowerBound..<end.upperBound])
        let named = try XCTUnwrap(body.range(of: "params[windowOwnerKey]"), "the request names the window's owner")
        let checked = try XCTUnwrap(body.range(of: "owner == expected"), "the owner is held to it")
        let captured = try XCTUnwrap(body.range(of: "WindowCapture.captureImage("))
        XCTAssertLessThan(named.lowerBound, checked.lowerBound)
        XCTAssertLessThan(checked.lowerBound, captured.lowerBound, "the owner is checked before the window is captured")
    }

    /// An act's answer names the window it acted in as the request chose it
    /// when it acted — its number and its owner (I-t123731-rc1, review
    /// t-128744 §1): the look after the act chooses again by focus and
    /// order, and may land on another window of the app. A window action
    /// names its window's owner beside its number.
    func testAnActsAnswerNamesTheWindowItActedInAsChosenWhenItActed() throws {
        let source = try computerUseSource()
        // Every act checks itself against the snapshot this chooses, and it
        // keeps the window it chose for the act's answer.
        let current = try XCTUnwrap(source.range(of: "private func currentSnapshot(params: [String: JSONValue]) throws -> Snapshot {"))
        let currentEnd = try XCTUnwrap(source.range(of: "\n    }\n", range: current.upperBound..<source.endIndex))
        XCTAssertTrue(
            source[current.lowerBound..<currentEnd.upperBound].contains("RequestContext.current.actedWindow = "),
            "the window an act is checked against is the one it acted in"
        )
        // Every answer of the act carries it: it is written before the first return.
        let result = try XCTUnwrap(source.range(of: "private func actionResult(params:"))
        let ran = try XCTUnwrap(source.range(of: "var action = try runAction()", range: result.upperBound..<source.endIndex))
        let rest = ran.upperBound..<source.endIndex
        let named = try XCTUnwrap(source.range(of: "action[ActedWindow.key] = ", range: rest))
        let firstReturn = try XCTUnwrap(source.range(of: "return ", range: rest))
        XCTAssertLessThan(named.lowerBound, firstReturn.lowerBound, "every answer of the act names the window it acted in")
        // A window action names the process that owns its window.
        let act = try XCTUnwrap(source.range(of: "static func act(params: [String: JSONValue]) throws -> [String: Any] {"))
        let actEnd = try XCTUnwrap(source.range(of: "\n    }\n", range: act.upperBound..<source.endIndex))
        XCTAssertTrue(source[act.lowerBound..<actEnd.upperBound].contains("\"pid\": Int(pid)"))
    }

    private func computerUseSource() throws -> String {
        let testFile = URL(fileURLWithPath: #filePath)
        let packageRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let mainPath = packageRoot
            .appendingPathComponent("Sources")
            .appendingPathComponent("ZeroCodeComputerUseMacOS")
            .appendingPathComponent("main.swift")
        return try String(contentsOf: mainPath, encoding: .utf8)
    }
}
