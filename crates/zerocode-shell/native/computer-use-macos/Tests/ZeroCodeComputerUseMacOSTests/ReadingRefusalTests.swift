import CoreGraphics
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// A press refused because its look showed no control says which of the
/// look's gaps it may have fallen into: the control shows the words only in
/// its text, the look stopped early, or the control stands in another of the
/// app's windows (t-121372). Nothing is pressed either way.
final class ReadingRefusalTests: XCTestCase {
    private let authorize = ReadingRefusal.OtherWindow(windowId: 237041, title: "Authorize Acme")

    func testAWholeLookWithOneWindowKeepsTheRefusalItAlwaysGave() {
        XCTAssertEqual(
            ReadingRefusal.noControl(app: "Fixture", windowTitle: "Report", words: "--role AXButton --label «Continue»", shown: nil, cut: nil, others: nil),
            "no control in Fixture's window «Report» reads --role AXButton --label «Continue»; look with --marks, or find"
        )
        XCTAssertNil(ReadingRefusal.shownWords([]))
        XCTAssertNil(ReadingRefusal.cutWords(truncated: false, cutShort: nil, maxDepthReached: false, read: 34, maxNodes: 1200))
        XCTAssertNil(ReadingRefusal.otherWindowsWords(app: "Fixture", others: []))
    }

    private func shown(_ index: Int, _ role: String, name: String, title: String? = nil, value: String? = nil, description: String? = nil) -> ReadingRefusal.Shown {
        ReadingRefusal.Shown(face: ElementFace(index: index, role: role, label: title, value: value, description: description), name: name)
    }

    func testAnElementThatShowsTheWordsOnlyInItsTextIsNamedByNumber() {
        let query = ElementQuery(text: nil, role: nil, label: "Authorize")
        let found = ReadingRefusal.shownOnly(query, among: [shown(12, "AXLink", name: "[Authorize](https://example.com/a)")])
        XCTAssertEqual(found, [ElementFace(index: 12, role: "AXLink", label: "Authorize", value: nil, description: nil)])
        XCTAssertEqual(
            ReadingRefusal.shownWords(found),
            "these words show only in the text of 12 AXLink «Authorize», which a click by words does not read: press one by its --element-index"
        )
        let rows = (1...4).map { ElementFace(index: $0, role: "AXRow", label: "Authorize \($0)", value: nil, description: nil) }
        XCTAssertEqual(
            ReadingRefusal.shownWords(rows),
            "these words show only in the text of 1 AXRow «Authorize 1»; 2 AXRow «Authorize 2»; 3 AXRow «Authorize 3» and 1 more, which a click by words does not read: press one by its --element-index"
        )
    }

    /// t-121539 고칠 것 2: a link's address is not its text, and an element
    /// whose label already holds a word is not one that shows it only in its text.
    func testALinkAddressAndAMatchedLabelAreNotTextOnly() {
        let settings = ElementQuery(text: nil, role: nil, label: "Settings")
        XCTAssertEqual(ReadingRefusal.shownOnly(settings, among: [shown(3, "AXLink", name: "[Profile](https://example.com/settings/profile)")]), [])
        let both = ElementQuery(text: "now", role: nil, label: "Continue")
        XCTAssertEqual(ReadingRefusal.shownOnly(both, among: [shown(4, "AXButton", name: "Continue now", title: "Continue")]), [])
        let asButton = ElementQuery(text: nil, role: "button", label: "Authorize")
        XCTAssertEqual(ReadingRefusal.shownOnly(asButton, among: [shown(5, "AXLink", name: "[Authorize](https://example.com/a)")]), [])
        XCTAssertEqual(ReadingRefusal.shownText(role: "AXLink", name: "[Save \\[draft\\]](https://example.com/s)"), "Save [draft]")
        XCTAssertEqual(ReadingRefusal.shownText(role: "AXButton", name: "[Save](x)"), "[Save](x)")
        XCTAssertEqual(ReadingRefusal.shownText(role: "AXLink", name: "Save"), "Save")
    }

    /// t-121539 I-5: one item of a refusal keeps 80 characters at most.
    func testALongTitleOrTextIsClippedToEightyCharacters() {
        let long = String(repeating: "a", count: 300)
        let window = ReadingRefusal.otherWindowsWords(app: "Notes", others: [.init(windowId: 7, title: long)]) ?? ""
        XCTAssertTrue(window.contains("«" + String(repeating: "a", count: 79) + "…»"), window)
        XCTAssertFalse(window.contains(String(repeating: "a", count: 80)), window)
        let text = ReadingRefusal.shownWords([ElementFace(index: 1, role: "AXRow", label: long, value: nil, description: nil)]) ?? ""
        XCTAssertTrue(text.contains("«" + String(repeating: "a", count: 79) + "…»"), text)
    }

    func testALookThatStoppedEarlySaysWhereAndWhy() {
        XCTAssertEqual(
            ReadingRefusal.cutWords(truncated: true, cutShort: "time_budget", maxDepthReached: false, read: 812, maxNodes: 1200),
            "this look stopped at its time budget after 812 elements, before the end of the window"
        )
        XCTAssertEqual(
            ReadingRefusal.cutWords(truncated: true, cutShort: nil, maxDepthReached: false, read: 1200, maxNodes: 1200),
            "this look stopped at its 1200-element limit, before the end of the window"
        )
        XCTAssertEqual(
            ReadingRefusal.cutWords(truncated: true, cutShort: nil, maxDepthReached: true, read: 300, maxNodes: 1200),
            "this look read no deeper than its depth limit, so deeper elements were not read"
        )
        XCTAssertEqual(
            ReadingRefusal.cutWords(truncated: true, cutShort: "app_unresponsive", maxDepthReached: false, read: 40, maxNodes: 1200),
            "the app stopped answering after 40 elements, before the end of the window"
        )
        XCTAssertEqual(
            ReadingRefusal.cutWords(truncated: false, cutShort: WebTreeSettling.buildingCode, maxDepthReached: false, read: 4, maxNodes: 1200),
            "this look caught the app still building its web tree"
        )
    }

    func testTheAppsOtherWindowsAreNamedByNumberAndTitle() {
        XCTAssertEqual(
            ReadingRefusal.otherWindowsWords(app: "Google Chrome", others: [authorize, .init(windowId: 9, title: nil)]),
            "Google Chrome has other windows on screen: --window-id 237041 «Authorize Acme», --window-id 9; the control may be in one of them — look at it by its --window-id"
        )
        let five = (1...5).map { ReadingRefusal.OtherWindow(windowId: CGWindowID($0), title: " ") }
        XCTAssertEqual(
            ReadingRefusal.otherWindowsWords(app: "Notes", others: five),
            "Notes has other windows on screen: --window-id 1, --window-id 2, --window-id 3 and 2 more; the control may be in one of them — look at it by its --window-id"
        )
    }

    func testTheRefusalCarriesTheCutAndTheOtherWindowsAfterItsOwnWords() {
        let said = ReadingRefusal.noControl(
            app: "Google Chrome",
            windowTitle: "Home | Acme",
            words: "--label «Authorize»",
            shown: nil,
            cut: "this look stopped at its time budget after 812 elements, before the end of the window",
            others: ReadingRefusal.otherWindowsWords(app: "Google Chrome", others: [authorize])
        )
        XCTAssertTrue(said.hasPrefix("no control in Google Chrome's window «Home | Acme» reads --label «Authorize»; look with --marks, or find; "), said)
        XCTAssertTrue(said.contains("time budget after 812 elements"), said)
        XCTAssertTrue(said.hasSuffix("--window-id 237041 «Authorize Acme»; the control may be in one of them — look at it by its --window-id"), said)
    }

    func testABackgroundPointInAnotherWindowIsNamedAndStillRefused() {
        let said = ReadingRefusal.backgroundTarget(.anotherWindow(authorize))
        XCTAssertTrue(said.hasPrefix("the background confirmation target is not a readable control: "), said)
        XCTAssertTrue(said.contains("another of the app's windows (--window-id 237041 «Authorize Acme»)"), said)
        XCTAssertTrue(said.hasSuffix("no input was sent"), said)
    }

    func testABackgroundPointPastTheCutSaysTheLookStopped() {
        let said = ReadingRefusal.backgroundTarget(.notReached("this look stopped at its 1200-element limit, before the end of the window"))
        XCTAssertTrue(said.hasPrefix("the background confirmation target is not a readable control: this look stopped"), said)
        XCTAssertTrue(said.hasSuffix("observe again before retrying; no input was sent"), said)
    }

    func testAContainerOrAnUnlabelledControlIsRefusedInTheWordsItAlwaysWas() {
        XCTAssertEqual(
            ReadingRefusal.backgroundTarget(.notAControl),
            "the background confirmation target is not a readable control; observe again before retrying; no input was sent"
        )
    }
}

/// The helper's refusals are built by those words, so a look's gap reaches the caller.
final class ReadingRefusalSourceTests: XCTestCase {
    private func helperSource() throws -> String {
        let here = URL(fileURLWithPath: #filePath)
        let main = here.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/ZeroCodeComputerUseMacOS/main.swift")
        return try String(contentsOf: main, encoding: .utf8)
    }

    private func body(of function: String, in source: String) throws -> Substring {
        let start = try XCTUnwrap(source.range(of: function), function)
        let end = try XCTUnwrap(source.range(of: "\n    }\n", range: start.upperBound..<source.endIndex), function)
        return source[start.upperBound..<end.lowerBound]
    }

    func testAClickByReadingThatFindsNoneSaysWhatTheLookMissed() throws {
        let chosen = try body(of: "private func chosenIndex(_ query: ElementQuery, in snapshot: Snapshot) throws -> Int {", in: try helperSource())
        XCTAssertTrue(chosen.contains("ReadingRefusal.noControl("), String(chosen))
        XCTAssertTrue(chosen.contains("shown: ReadingRefusal.shownWords(shownFaces(query, in: snapshot))"), String(chosen))
        XCTAssertTrue(chosen.contains("cut: lookCut(snapshot)"), String(chosen))
        XCTAssertTrue(chosen.contains("others: otherWindowsWords(snapshot)"), String(chosen))
    }

    func testTheBackgroundConfirmationSaysWhereToLookAndStillRefuses() throws {
        let source = try helperSource()
        let confirm = try XCTUnwrap(source.range(of: "func confirmTarget(method: String, params: [String: JSONValue], snapshot loadSnapshot: () throws -> Snapshot) throws -> String? {"))
        let label = source[confirm.upperBound...].prefix(1_400)
        XCTAssertTrue(label.contains("let why = record == nil ? unreadablePoint(point, in: snapshot) : .notAControl"), String(label))
        XCTAssertTrue(label.contains("throw ProviderError.coded(\"element_not_found\", ReadingRefusal.backgroundTarget(why))"), String(label))
        XCTAssertTrue(source.contains("return try label(backgroundClickRecord(at: point, in: snapshot), in: snapshot, at: point)"))
    }

    func testTheElementsThatShowTheWordsOnlyInTheirTextAreTheCoresChoice() throws {
        let shown = try body(of: "private func shownFaces(_ query: ElementQuery, in snapshot: Snapshot) -> [ElementFace] {", in: try helperSource())
        XCTAssertTrue(shown.contains("ReadingRefusal.shownOnly(query, among:"), String(shown))
    }
}
