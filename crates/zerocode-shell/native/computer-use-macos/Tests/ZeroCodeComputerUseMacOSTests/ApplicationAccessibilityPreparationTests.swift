import XCTest

final class ApplicationAccessibilityPreparationTests: XCTestCase {
    func testEveryAppRoleIsReadBeforeTheLegacyManualModeFilter() throws {
        let source = try helperSource()
        let start = try XCTUnwrap(source.range(of: "private func prepareApplicationAccessibility("))
        let end = try XCTUnwrap(source.range(of: "private func focusedWindow(", range: start.upperBound..<source.endIndex))
        let preparation = String(source[start.lowerBound..<end.lowerBound])
        let roleRead = try XCTUnwrap(preparation.range(of: "stringAttribute(appElement, kAXRoleAttribute as String)"))
        let legacyFilter = try XCTUnwrap(preparation.range(of: "guard app.needsManualAccessibilityMode"))
        XCTAssertLessThan(roleRead.lowerBound, legacyFilter.lowerBound)
    }

    func testSnapshotPreparesTheApplicationAfterTrustAndBeforeWindowDiscovery() throws {
        let source = try helperSource()
        let start = try XCTUnwrap(source.range(of: "private func buildSnapshot("))
        let end = try XCTUnwrap(source.range(of: "private func renderSnapshot(", range: start.upperBound..<source.endIndex))
        let snapshot = String(source[start.lowerBound..<end.lowerBound])
        let trusted = try XCTUnwrap(snapshot.range(of: "guard accessibilityTrustedSettled()"))
        let preparation = try XCTUnwrap(snapshot.range(of: "prepareApplicationAccessibility(appElement, app: app)"))
        let discovery = try XCTUnwrap(snapshot.range(of: "WindowCapture.candidates(pid: app.pid)"))
        XCTAssertLessThan(trusted.lowerBound, preparation.lowerBound)
        XCTAssertLessThan(preparation.lowerBound, discovery.lowerBound)
    }

    func testApplicationPreparationDoesNotFocusOrTypeOrRequestPermissions() throws {
        let source = try helperSource()
        let start = try XCTUnwrap(source.range(of: "private func prepareApplicationAccessibility("))
        let end = try XCTUnwrap(source.range(of: "private func focusedWindow(", range: start.upperBound..<source.endIndex))
        let preparation = String(source[start.lowerBound..<end.lowerBound])
        for forbidden in ["recoverWindow(", "activate(", "kAXFocusedAttribute", "kAXRaiseAction", "OperatorHandHost", "AXIsProcessTrustedWithOptions"] {
            XCTAssertFalse(preparation.contains(forbidden), forbidden)
        }
    }

    private func helperSource() throws -> String {
        let packageRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        return try String(
            contentsOf: packageRoot.appendingPathComponent("Sources/ZeroCodeComputerUseMacOS/main.swift"),
            encoding: .utf8
        )
    }
}
