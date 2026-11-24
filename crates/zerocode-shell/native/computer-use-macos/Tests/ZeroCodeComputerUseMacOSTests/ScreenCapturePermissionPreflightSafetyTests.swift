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
