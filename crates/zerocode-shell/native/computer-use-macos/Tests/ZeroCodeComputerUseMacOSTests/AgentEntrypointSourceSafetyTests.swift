import XCTest

final class AgentEntrypointSourceSafetyTests: XCTestCase {
    func testDeferredObservationStillPerformsThePinnedActionFirst() throws {
        let source = try agentEntrypointSource()
        let start = try XCTUnwrap(source.range(of: "private func actionResult("))
        let end = try XCTUnwrap(source.range(of: "private func observe(", range: start.upperBound..<source.endIndex))
        let body = String(source[start.lowerBound..<end.lowerBound])
        let acted = try XCTUnwrap(body.range(of: "var action = try runAction()"))
        let deferred = try XCTUnwrap(body.range(of: "params[\"deferObservation\"]?.bool == true"))
        let observed = try XCTUnwrap(body.range(of: "snapshot: observe(params: params)"))
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
            let ups = stroke.ups(route: .desktop)
                    defer {
                        for modifier in ups.dropFirst() { try? OperatorHandHost.post(modifier) }
                    }
            """
        ))
        // Moved with the one hand (t-6765): an app-level click and drag still carry the verb's
        // flags and a process-routed event still goes to its pid — now in CGEventHandPoster.
        XCTAssertTrue(source.contains("made.flags = flags") && source.contains("case let .process(pid):\n            made.postToPid(pid)") && source.contains("flags: flags.rawValue, route: .desktop, source: .session") && source.contains("flags: flags.rawValue, route: .process(pid), source: .session"))
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
