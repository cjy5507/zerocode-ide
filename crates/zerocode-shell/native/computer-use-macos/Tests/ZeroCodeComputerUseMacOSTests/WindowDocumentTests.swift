import AppKit
import XCTest
@testable import ZeroCodeComputerUseMacOS

/// What a window's answers name its document by (t-69973): `find`'s window
/// and a look's window carry the identity of the document the window shows,
/// so two windows under one title tell apart — and never the path or the
/// address the identity was taken from.
final class WindowDocumentTests: XCTestCase {
    /// Two documents of one name in two folders, and two pages of one site.
    private static let notesInA = "file:///fixture/folder-a/notes.txt"
    private static let notesInB = "file:///fixture/folder-b/notes.txt"
    private static let inboxOne = "https://mail.example.com/u/0/"
    private static let inboxTwo = "https://mail.example.com/u/1/"
    /// Their identities, taken outside Swift:
    /// `printf '%s' <value> | shasum -a 256 | cut -c1-16`.
    private static let identities = [
        notesInA: "df603275387c0478",
        notesInB: "cd05fe9df2968931",
        inboxOne: "83a21e9a80bc91e2",
        inboxTwo: "9816670c7fbd3b55",
    ]
    /// What those words are known by inside an answer, a slash escaped or not.
    private static let giveaways = ["folder-a", "folder-b", "mail.example.com"]

    /// A window as a look leaves it — what both answers are drawn from — the
    /// tree aside.
    private static func window(_ id: CGWindowID, _ title: String, document: String? = nil, page: String? = nil) -> Snapshot {
        Snapshot(
            id: "fixture-\(id)",
            app: AppDescriptor(name: "Fixture", bundleId: "dev.zerocode.fixture", pid: getpid(), app: .current),
            windowTitle: title,
            windowBounds: CGRect(x: 40, y: 60, width: 800, height: 600),
            windowId: id,
            windowLayer: 0,
            document: WindowDocument.identity(document: document, webAreaURL: page),
            treeText: "",
            focusedElementId: nil,
            screenshot: nil,
            screenshotStatus: .skipped,
            screenshotScale: CGSize(width: 1, height: 1),
            screenshotEngine: nil,
            elements: [:],
            truncated: false,
            maxDepthReached: false
        )
    }

    /// One window object as the wire writes it, slashes as they are.
    private static func wire(_ object: [String: Any]) throws -> String {
        String(decoding: try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes]), as: UTF8.self)
    }

    func test_a_documents_identity_is_the_first_sixteen_hex_digits_of_its_sha256() {
        for (shown, expected) in Self.identities {
            XCTAssertEqual(WindowDocument.identity(document: shown, webAreaURL: nil), expected, "\(shown) as the window's AXDocument")
            XCTAssertEqual(WindowDocument.identity(document: nil, webAreaURL: shown), expected, "\(shown) as its first web area's AXURL")
        }
    }

    func test_a_window_that_names_its_document_is_known_by_it_before_the_page_it_shows() {
        XCTAssertEqual(
            WindowDocument.identity(document: Self.notesInA, webAreaURL: Self.inboxOne), Self.identities[Self.notesInA],
            "the window's own AXDocument comes first"
        )
        XCTAssertEqual(
            WindowDocument.identity(document: "", webAreaURL: Self.inboxOne), Self.identities[Self.inboxOne],
            "an empty AXDocument is none: the page's address is next"
        )
    }

    /// A window that gives neither has no identity, and its answers carry no
    /// `document` key: both window objects are what they always were.
    func test_a_window_that_gives_neither_answers_no_document_key() {
        XCTAssertNil(WindowDocument.identity(document: nil, webAreaURL: nil))
        XCTAssertNil(WindowDocument.identity(document: "", webAreaURL: ""))
        let bare = Self.window(41, "notes.txt")
        XCTAssertEqual(Set(namedWindow(bare).keys), ["id", "title"], "find's window as it always was")
        XCTAssertEqual(
            Set(placedWindow(bare).keys),
            ["id", "title", "x", "y", "width", "height", "isMinimized", "isOffscreen", "screenIndex", "platform"],
            "a look's window as it always was"
        )
    }

    /// The identity is all the answers say of a document: `find`'s window and
    /// a look's window carry it under `document`, and nothing in either says
    /// the path or the address it was taken from.
    func test_find_and_a_look_name_the_document_by_its_identity_and_never_by_its_words() throws {
        let shown: [(document: String?, page: String?)] = [(Self.notesInA, nil), (nil, Self.inboxOne), (Self.notesInB, Self.inboxTwo)]
        var objects = 0
        var said = 0
        for (document, page) in shown {
            let taken = try XCTUnwrap(document ?? page)
            let expected = try XCTUnwrap(Self.identities[taken])
            let window = Self.window(41, "notes.txt", document: document, page: page)
            for (answer, object) in [("find", namedWindow(window)), ("look", placedWindow(window))] {
                objects += 1
                XCTAssertEqual(object[WindowDocument.key] as? String, expected, "\(answer): the window names its document's identity")
                let written = try Self.wire(object)
                for words in [document, page].compactMap({ $0 }) + Self.giveaways where written.contains(words) {
                    said += 1
                    XCTFail("\(answer): the window object says «\(words)» itself: \(written)")
                }
            }
        }
        print("measured: window objects that say a path or an address: \(said) of \(objects)")
    }

    /// Two windows under one title (t-69973): two documents of one name in two
    /// folders, and two pages of one site. By title alone each pair is two
    /// windows a look cannot tell apart; by title and document, none.
    func test_two_windows_of_one_title_tell_apart_by_their_documents() {
        let scenes = [
            ("documents", [Self.window(41, "notes.txt", document: Self.notesInA), Self.window(42, "notes.txt", document: Self.notesInB)]),
            ("pages", [Self.window(51, "Inbox", page: Self.inboxOne), Self.window(52, "Inbox", page: Self.inboxTwo)]),
        ]
        for (scene, windows) in scenes {
            let named = windows.map(namedWindow)
            /// The windows whose name another window of the scene shares.
            func alike(_ name: ([String: Any]) -> String) -> Int {
                let names = named.map(name)
                return names.filter { mine in names.filter { $0 == mine }.count > 1 }.count
            }
            let byTitle = alike { "\($0["title"] ?? "")" }
            let byDocument = alike { "\($0["title"] ?? "")\u{1f}\($0[WindowDocument.key] ?? "none")" }
            print("measured: \(scene): windows a look cannot tell from another of its title — by title alone \(byTitle), by title and document \(byDocument), of \(windows.count)")
            XCTAssertEqual(byTitle, 2, "\(scene): two windows under one title")
            XCTAssertEqual(byDocument, 0, "\(scene): the document tells each window from the other")
        }
    }

    /// What the identity costs a look inside the helper: `find`'s and a look's
    /// window objects drawn as they were — no document — and with the identity
    /// taken and carried, 10,000 times each. The one attribute read it adds
    /// goes to the observed app; that round trip is not this test's.
    func test_measured_what_the_identity_adds_to_a_look_inside_the_helper() {
        let rounds = 10_000
        var kept = 0
        func timed(_ draw: () -> Int) -> [UInt64] {
            (0..<rounds).map { _ in
                let start = clock_gettime_nsec_np(CLOCK_UPTIME_RAW)
                kept &+= draw()
                return clock_gettime_nsec_np(CLOCK_UPTIME_RAW) - start
            }.sorted()
        }
        let before = timed {
            let window = Self.window(41, "notes.txt")
            return namedWindow(window).count + placedWindow(window).count
        }
        let after = timed {
            let window = Self.window(41, "notes.txt", document: Self.notesInA)
            return namedWindow(window).count + placedWindow(window).count
        }
        func microseconds(_ ns: UInt64) -> String { String(format: "%.2f", Double(ns) / 1_000) }
        let added = Int64(after[rounds / 2]) - Int64(before[rounds / 2])
        print("measured: a look's window objects without a document p50 \(microseconds(before[rounds / 2])) µs max \(microseconds(before[rounds - 1])) µs; with its identity p50 \(microseconds(after[rounds / 2])) µs max \(microseconds(after[rounds - 1])) µs; added p50 \(String(format: "%.2f", Double(added) / 1_000)) µs, over \(rounds) rounds each (\(kept) keys drawn)")
        XCTAssertLessThan(added, 2_000_000, "the identity adds under the 2 ms a look may grow by")
    }

    /// A look reads one attribute more than it did: the matched window's own
    /// `AXDocument`, once. The walk asks nothing new of any node — its reads
    /// are the batch's table (`SnapshotAttributeTableSourceTests`) — and the
    /// page's address is what it already read of its first web area.
    func test_a_look_adds_one_read_the_matched_windows_own_axdocument() throws {
        let source = try Self.helperSource()
        let build = try XCTUnwrap(Self.slice(source, from: "private func buildSnapshot(", to: "private func renderSnapshot("))
        let reads = try NSRegularExpression(pattern: #"stringAttribute\(\s*([A-Za-z]+)\s*,\s*(kAX[A-Za-z]+Attribute)"#)
        let read = reads.matches(in: build, range: NSRange(build.startIndex..<build.endIndex, in: build)).compactMap { match -> String? in
            guard let element = Range(match.range(at: 1), in: build), let attribute = Range(match.range(at: 2), in: build) else { return nil }
            return "\(build[element]).\(build[attribute])"
        }
        XCTAssertEqual(
            read, ["focused.kAXTitleAttribute", "window.kAXTitleAttribute", "window.kAXDocumentAttribute"],
            "a look reads the titles it always read and the matched window's document — the one read added"
        )
        XCTAssertEqual(source.components(separatedBy: "kAXDocumentAttribute").count - 1, 1, "AXDocument is read in one place")
        XCTAssertFalse(source.contains("\"AXDocument\""), "and under no spelled name")
        let walk = try XCTUnwrap(Self.slice(source, from: "private final class TreeRenderer {", to: "private func tabStripCompaction("))
        XCTAssertTrue(
            Self.squeezed(walk).contains("let url = reader.stringAttribute(element, kAXURLAttribute as String)"),
            "the walk reads every node's address with its batch"
        )
        XCTAssertTrue(
            Self.squeezed(walk).contains("if webAreaDepth == 0, !metWebArea { metWebArea = true firstWebAreaURL = url }"),
            "the walk keeps the address it read of the first web area it met"
        )
        XCTAssertTrue(
            Self.squeezed(build).contains(
                "let document = WindowDocument.identity( document: stringAttribute(window, kAXDocumentAttribute as String), webAreaURL: renderer.firstWebAreaURL )"
            ),
            "the document and the page's address go straight into the identity"
        )
        XCTAssertEqual(
            source.components(separatedBy: "firstWebAreaURL").count - 1, 3,
            "the address is declared, kept by the walk and read by the identity — nothing else reads it"
        )
    }

    /// The text between two markers of the helper's source.
    private static func slice(_ source: String, from start: String, to end: String) -> String? {
        guard let from = source.range(of: start), let to = source.range(of: end, range: from.upperBound..<source.endIndex) else { return nil }
        return String(source[from.lowerBound..<to.lowerBound])
    }

    /// Source text with every run of white space one space.
    private static func squeezed(_ text: String) -> String {
        text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
    }

    private static func helperSource() throws -> String {
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
