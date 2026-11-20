import XCTest

/// CU slowness 11-20 (t-121376): Chrome publishes no `AXWindowNumber`, so two
/// of its windows at one frame and one title could not be told apart, and
/// every act that named either — a window focus, move or resize, and every
/// look or press with its window ID — was refused. Both places that pick a
/// window by its frame hand the pick the accessibility library's own link to
/// the window server, which the pick asks only when frames are shared; a
/// window's own number stays the one it publishes.
final class WindowNumberSourceTests: XCTestCase {
    func testBothPicksByFrameAreHandedTheLibrarysLink() throws {
        let source = try helperSource()
        let looks = try body(of: source, from: "private func matchingWindow(", to: "private func recoverWindow(")
        XCTAssertTrue(
            looks.contains("DesktopWindowPick.pick(target: capture.windowId, targetBounds: capture.bounds, targetTitle: capture.title, windows: rows, linked: { WindowServerNumber.of(windows[$0]) })"),
            "a look or a press with a window ID tells windows at one frame apart by the link")
        let acts = try body(of: source, from: "static func element(windowId: CGWindowID)", to: "static func act(params:")
        XCTAssertTrue(
            acts.contains("DesktopWindowPick.pick(target: windowId, targetBounds: targetBounds, targetTitle: targetTitle, windows: rows, linked: { WindowServerNumber.of(windows[$0]) })"),
            "a window focus, move or resize tells windows at one frame apart by the link")
    }

    func testAWindowsOwnNumberIsStillOnlyTheOneItPublishes() throws {
        let source = try helperSource()
        let own = try body(of: source, from: "private func windowNumber(_ element: AXUIElement) -> CGWindowID? {", to: "private enum WindowServerNumber {")
        XCTAssertTrue(own.contains("numberAttribute(element, \"AXWindowNumber\")"))
        XCTAssertFalse(
            own.contains("WindowServerNumber"),
            "the click recipients and every other reader of a window's number read the published one as before")
        XCTAssertEqual(
            source.components(separatedBy: "WindowServerNumber.of(").count - 1, 4,
            "asked by the two picks, the twin check and the recovery alone")
    }

    /// Once two windows at one frame are told apart, a look or a press can
    /// name the one behind. The keyboard's focus proof reads frames, so the
    /// twin in front would pass it and take the keys: a twin known to be
    /// another window is not the target, and the recovery raises the window
    /// the target is, not the first at its frame.
    func testTheFocusProofsNeverTakeATwinForTheTarget() throws {
        let source = try helperSource()
        let focused = try body(of: source, from: "private func isTargetWindowFocused(_ snapshot: Snapshot) -> Bool {", to: "private func isTwinOfTarget(")
        let twin = try XCTUnwrap(focused.range(of: "isTwinOfTarget(focusedWindow, frame: frame, target: snapshot.windowId, targetBounds: snapshot.windowBounds)"))
        let overlap = try XCTUnwrap(focused.range(of: "let intersection = frame.intersection(snapshot.windowBounds)"))
        XCTAssertLessThan(twin.lowerBound, overlap.lowerBound, "the twin is ruled out before the frames are compared")
        let check = try body(of: source, from: "private func isTwinOfTarget(", to: "private enum AXElementProbe {")
        XCTAssertTrue(check.contains("number: windowNumber(window) ?? WindowServerNumber.of(window)"))
        XCTAssertTrue(check.contains("DesktopWindowPick.isAnotherAtTheSameFrame("))
        let matches = try body(of: source, from: "private func windowMatchesCapture(", to: "private func windowFramesMatch(")
        let twinMatch = try XCTUnwrap(matches.range(of: "isTwinOfTarget(window, frame: frame, target: windowId, targetBounds: windowBounds)"))
        let frameMatch = try XCTUnwrap(matches.range(of: "return windowFramesMatch(frame, windowBounds)"))
        XCTAssertLessThan(twinMatch.lowerBound, frameMatch.lowerBound)
        let recovery = try body(of: source, from: "private func recoverWindow(", to: "private func frontPidNow()")
        XCTAssertTrue(recovery.contains("windows().first { windowNumber($0) == targetId } ?? windows().first { WindowServerNumber.of($0) == targetId }"))
    }

    func testTheLibrarysLookupIsFoundAtRunTimeSoAHelperWithoutItStillStarts() throws {
        let source = try helperSource()
        XCTAssertTrue(source.contains("dlsym(UnsafeMutableRawPointer(bitPattern: -2), \"_AXUIElementGetWindow\")"))
        XCTAssertFalse(
            source.contains("@_silgen_name(\"_AXUIElementGetWindow\")"),
            "a name bound at link time keeps the helper from starting on a system without it")
        XCTAssertTrue(source.contains("guard let symbol = UnsafeRawPointer(bitPattern: address) else { return nil }"))
        XCTAssertTrue(
            source.contains("guard lookup(window, &number) == .success, number != 0 else { return nil }"),
            "a failed or empty answer is no link, so the pick is what it was")
    }

    private func body(of source: String, from start: String, to end: String) throws -> String {
        let begin = try XCTUnwrap(source.range(of: start), start)
        let stop = try XCTUnwrap(source.range(of: end, range: begin.upperBound..<source.endIndex), end)
        return String(source[begin.lowerBound..<stop.lowerBound])
    }

    private func helperSource() throws -> String {
        let packageRoot = URL(fileURLWithPath: #filePath)
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
