import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class DesktopAppsTests: XCTestCase {
    func testANameBecomesAppBundlesUnderEachRootThenTheBareName() {
        let roots = [URL(fileURLWithPath: "/Applications"), URL(fileURLWithPath: "/System/Applications")]
        let urls = applicationCandidateURLs(query: "TextEdit", roots: roots).map(\.path)
        XCTAssertEqual(urls, ["/Applications/TextEdit.app", "/Applications/TextEdit", "/System/Applications/TextEdit.app", "/System/Applications/TextEdit"])
        XCTAssertEqual(applicationCandidateURLs(query: "TextEdit.app", roots: [roots[0]]).map(\.path), ["/Applications/TextEdit.app", "/Applications/TextEdit"])
    }

    func testAPathIsItselfAndAnEmptyQueryIsNothing() {
        XCTAssertEqual(applicationCandidateURLs(query: "/opt/Tool.app", roots: [URL(fileURLWithPath: "/Applications")]).map(\.path), ["/opt/Tool.app"])
        XCTAssertEqual(applicationCandidateURLs(query: "   ", roots: [URL(fileURLWithPath: "/Applications")]), [])
    }

    func testABundleIdentifierIsReverseDNSWithoutSpacesOrSlashes() {
        XCTAssertTrue(looksLikeBundleIdentifier("com.apple.TextEdit"))
        XCTAssertFalse(looksLikeBundleIdentifier("TextEdit"))
        XCTAssertFalse(looksLikeBundleIdentifier("Visual Studio Code"))
        XCTAssertFalse(looksLikeBundleIdentifier("/Applications/X.app"))
        XCTAssertFalse(looksLikeBundleIdentifier("a..b"))
    }

    func testEveryWindowActionHasAName() {
        XCTAssertEqual(DesktopWindowAction.allCases.map(\.rawValue), ["focus", "move", "resize", "minimize", "zoom", "close"])
        XCTAssertNil(DesktopWindowAction(rawValue: "explode"))
        XCTAssertTrue(DesktopWindowAction.move.movesTheWindow && !DesktopWindowAction.focus.movesTheWindow)
    }
}
