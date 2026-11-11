import XCTest

final class AgentEntrypointSourceSafetyTests: XCTestCase {
    func testAClosedSelectedWindowNeverFallsBackToAnotherWindow() throws {
        let source = try agentEntrypointSource()
        let start = try XCTUnwrap(source.range(of: "private func actionResult("))
        let end = try XCTUnwrap(source.range(of: "private func observe(", range: start.upperBound..<source.endIndex))
        let body = String(source[start.lowerBound..<end.lowerBound])
        XCTAssertEqual(body.components(separatedBy: "lookAfterAct(params:").count - 1, 1)
        XCTAssertFalse(body.contains("removeValue(forKey:"))
        XCTAssertTrue(body.contains("hasRequestedWindowSelector(params)"))
        XCTAssertTrue(body.contains("action[\"targetWindowId\"] = windowID"))
        XCTAssertTrue(body.contains("\"observation\": [\"state\": \"unavailable\", \"reason\": error.code]"))
    }

    func testDeferredObservationStillPerformsThePinnedActionFirst() throws {
        let source = try agentEntrypointSource()
        let start = try XCTUnwrap(source.range(of: "private func actionResult("))
        let end = try XCTUnwrap(source.range(of: "private func observe(", range: start.upperBound..<source.endIndex))
        let body = String(source[start.lowerBound..<end.lowerBound])
        let acted = try XCTUnwrap(body.range(of: "var action = try runAction()"))
        let deferred = try XCTUnwrap(body.range(of: "params[\"deferObservation\"]?.bool == true"))
        let observed = try XCTUnwrap(body.range(of: "lookAfterAct(params: params"))
        XCTAssertLessThan(acted.lowerBound, deferred.lowerBound)
        XCTAssertLessThan(deferred.lowerBound, observed.lowerBound)
        XCTAssertTrue(body.contains("params[\"noScreenshot\"]?.bool == true"))
        XCTAssertTrue(body.contains("let window = params[\"windowId\"]?.number"))
        XCTAssertTrue(body.contains("[\"action\": action, \"observationDeferred\": true]"))
    }

    func testAgentEntrypointDoesNotUnlinkCallerSuppliedPaths() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let packageRoot = testFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let mainPath = packageRoot
            .appendingPathComponent("Sources")
            .appendingPathComponent("ZeroCodeComputerUseMacOS")
            .appendingPathComponent("main.swift")
        let source = try String(contentsOf: mainPath, encoding: .utf8)

        // Why: --agent accepts caller-supplied paths; deleting them in the
        // helper can remove user files if argument validation is bypassed.
        XCTAssertFalse(source.contains("unlink(tokenPath)"))
        XCTAssertFalse(source.contains("unlink(socketPath)"))
    }

    func testSyntheticModifiersHaveGuaranteedReleaseAndModifiedClicksUseFlags() throws {
        let source = try agentEntrypointSource()

        // One chord sequence for the key verbs and a reflex key (t-10384, `KeyChordStroke`): each
        // modifier's release is deferred, so it goes whatever stops the chord.
        XCTAssertTrue(source.contains("let stroke = try KeyMap.parse(key).stroke"))
        XCTAssertTrue(source.contains(
            """
            let ups = stroke.ups(route: route)
                    defer {
                        for modifier in ups.dropFirst() { try? OperatorHandHost.post(modifier) }
                    }
            """
        ))
        // Moved with the one hand (t-6765): an app-level click and drag still carry the verb's
        // flags and a process-routed event still goes to its pid — now in CGEventHandPoster.
        XCTAssertTrue(source.contains("made.flags = flags") && source.contains("case let .process(pid):\n            made.postToPid(pid)") && source.contains("flags: flags.rawValue, route: .desktop, source: .session") && source.contains("flags: flags.rawValue, route: .process(pid), source: .session"))
    }

    /// Two windows of one app at one frame (2026-11-11): a look and a press took
    /// the first window that overlapped — which pictured one window and read
    /// the other. Past the window that carries the number, every pick is the
    /// Core's rule now, and none of them is "the first": a shared frame and
    /// several that cover a target are each refused in their own words.
    func testNoWindowIsTakenForComingFirst() throws {
        let source = try agentEntrypointSource()
        let start = try XCTUnwrap(source.range(of: "private func matchingWindow("))
        let end = try XCTUnwrap(source.range(of: "private func recoverWindow(", range: start.upperBound..<source.endIndex))
        let body = String(source[start.lowerBound..<end.lowerBound])
        let asked = try XCTUnwrap(body.range(of: "DesktopWindowPick.pick(target: capture.windowId"))
        let covered = try XCTUnwrap(body.range(of: "DesktopWindowPick.pickCovering("))
        XCTAssertLessThan(asked.lowerBound, covered.lowerBound)
        XCTAssertTrue(body.contains("targetTitle: capture.title"))
        XCTAssertTrue(body.contains("DesktopWindowPick.ambiguousSaid(capture.windowId)"))
        XCTAssertTrue(body.contains("DesktopWindowPick.coveredSaid(capture.windowId)"))
        XCTAssertTrue(body.contains("focused: CFEqual(window, focused)"))
        XCTAssertTrue(body.contains("return called.count == 1 ? called[0] : focused"))
        XCTAssertEqual(
            body.components(separatedBy: "windows.first").count - 1, 1,
            "only the window that carries the number is taken by `first`")
        XCTAssertTrue(body.contains("windows.first(where: { windowNumber($0) == capture.windowId })"))
        XCTAssertEqual(source.components(separatedBy: "DesktopWindowPick.ambiguousSaid(").count - 1, 2)
        XCTAssertEqual(source.components(separatedBy: "DesktopWindowPick.coveredSaid(").count - 1, 1)
        XCTAssertTrue(source.contains("targetTitle = listed?[kCGWindowName as String] as? String"))
    }

    private func agentEntrypointSource() throws -> String {
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
