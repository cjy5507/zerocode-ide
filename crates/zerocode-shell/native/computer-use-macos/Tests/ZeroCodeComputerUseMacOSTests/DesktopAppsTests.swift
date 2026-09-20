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

    func testOtherNamesAreTheBundlesOwnWordsThenTheExecutableStemWithoutRepeats() {
        let calculator: [String: Any] = ["CFBundleDisplayName": "Calculator", "CFBundleName": "Calculator"]
        let executable = URL(fileURLWithPath: "/System/Applications/Calculator.app/Contents/MacOS/Calculator")
        XCTAssertEqual(applicationOtherNames(info: calculator, executableURL: executable), ["Calculator"])
        XCTAssertEqual(
            applicationOtherNames(info: ["CFBundleName": "Code"], executableURL: URL(fileURLWithPath: "/Applications/Visual Studio Code.app/Contents/MacOS/Electron")),
            ["Code", "Electron"]
        )
        XCTAssertEqual(applicationOtherNames(info: ["CFBundleDisplayName": ""], executableURL: nil), [])
        XCTAssertEqual(applicationOtherNames(info: nil, executableURL: nil), [])
        XCTAssertEqual(bundleName(info: calculator), "Calculator")
        XCTAssertEqual(bundleName(info: ["CFBundleName": "Code"]), "Code")
        XCTAssertNil(bundleName(info: ["CFBundleName": ""]))
        XCTAssertNil(bundleName(info: nil))
    }

    func testAnAppAnswersToItsNameIdentifierOrOtherNamesCaseInsensitivelyNeverAFragment() {
        // `launch --app Calculator` answered name=계산기; `observe --app Calculator` must find it.
        XCTAssertTrue(applicationAnswers(to: "calculator", name: "계산기", bundleId: "com.apple.calculator", otherNames: ["Calculator"]))
        XCTAssertTrue(applicationAnswers(to: "COM.APPLE.CALCULATOR", name: "계산기", bundleId: "com.apple.calculator", otherNames: []))
        XCTAssertTrue(applicationAnswers(to: "계산기", name: "계산기", bundleId: nil, otherNames: []))
        XCTAssertTrue(applicationAnswers(to: "electron", name: "Visual Studio Code", bundleId: "com.microsoft.VSCode", otherNames: ["Code", "Electron"]))
        XCTAssertFalse(applicationAnswers(to: "calc", name: "계산기", bundleId: "com.apple.calculator", otherNames: ["Calculator"]))
        XCTAssertFalse(applicationAnswers(to: "calculator", name: "계산기", bundleId: "com.apple.calculator", otherNames: []))
        // The bundle is read only when the name and the identifier miss.
        var read = 0
        XCTAssertTrue(applicationAnswers(to: "계산기", name: "계산기", bundleId: nil, otherNames: { read += 1; return [] }()))
        XCTAssertEqual(read, 0)
        XCTAssertFalse(applicationAnswers(to: "nothing", name: "계산기", bundleId: nil, otherNames: { read += 1; return [] }()))
        XCTAssertEqual(read, 1)
    }

    func testEveryWindowActionHasAName() {
        XCTAssertEqual(DesktopWindowAction.allCases.map(\.rawValue), ["focus", "move", "resize", "minimize", "zoom", "close"])
        XCTAssertNil(DesktopWindowAction(rawValue: "explode"))
        XCTAssertTrue(DesktopWindowAction.move.movesTheWindow && !DesktopWindowAction.focus.movesTheWindow)
    }
}
