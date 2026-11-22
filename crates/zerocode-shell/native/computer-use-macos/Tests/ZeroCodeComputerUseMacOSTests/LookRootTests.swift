import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// Where a look files the numbers it gives (t-125705 R2): a menu read closed
/// keeps its numbers when the whole window is read again, and a click by one
/// of them goes only to the window the app sends its menu commands to.
final class LookRootTests: XCTestCase {
    private func freshBook() -> ElementIdentityBook<String> {
        ElementIdentityBook(hash: { UInt(bitPattern: $0.hashValue) }, equal: ==)
    }

    /// One look as the helper ends it: a complete one forgets what it did not
    /// meet under its root; a look below an element is never complete.
    @discardableResult
    private func look(_ book: ElementIdentityBook<String>, _ elements: [String], root: String, complete: Bool) -> (numbers: [String: Int], gone: [Int]) {
        book.beginWalk()
        var numbers: [String: Int] = [:]
        for element in elements { numbers[element] = book.number(for: element, root: root) }
        return (numbers, complete ? book.endCompleteWalk(root: root) : book.endCutWalk())
    }

    private let window: UInt32 = 7
    private let finder = ["window", "list", "row 1"]
    private let menuBar = ["menu bar", "Apple", "Finder", "File"]

    /// A menu read closed, then a click with no look of the window kept: the
    /// click reads the whole window again, which meets none of the menu.
    private func menuReadThenWindowReadAgain(_ book: ElementIdentityBook<String>) -> (trash: Int, gone: [Int]) {
        look(book, finder, root: LookRoot.window(window), complete: true)
        let bar = look(book, menuBar, root: LookRoot.surface("menubar", window: window), complete: true).numbers
        let file = bar["File"] ?? -1
        let menu = look(book, ["File", "New Folder", "Empty Trash"], root: LookRoot.below(file, onSurface: true, window: window), complete: false).numbers
        let again = look(book, finder, root: LookRoot.window(window), complete: true)
        return (menu["Empty Trash"] ?? -1, again.gone)
    }

    func testAMenuReadClosedKeepsItsNumbersWhenTheWholeWindowIsReadAgain() {
        let book = freshBook()
        let (trash, gone) = menuReadThenWindowReadAgain(book)
        XCTAssertFalse(gone.contains(trash), "the menu item's number outlives the window's look")
        XCTAssertEqual(book.existingNumber(for: "Empty Trash"), trash, "and still names that item")
        // A look at the menu bar again meets none of the menu's items either.
        XCTAssertFalse(look(book, menuBar, root: LookRoot.surface("menubar", window: window), complete: true).gone.contains(trash))
        XCTAssertEqual(book.existingNumber(for: "Empty Trash"), trash)
        // A row the window lost is forgotten by the window's own look, as before.
        let row = book.existingNumber(for: "row 1") ?? -1
        XCTAssertTrue(look(book, ["window", "list"], root: LookRoot.window(window), complete: true).gone.contains(row))
    }

    /// Below one of the window's own elements a look is the window's: what
    /// the window's next complete look no longer meets there is forgotten.
    func testALookBelowTheWindowsOwnElementStaysTheWindows() {
        XCTAssertEqual(LookRoot.below(12, onSurface: false, window: window), LookRoot.window(window))
        XCTAssertNotEqual(LookRoot.below(12, onSurface: true, window: window), LookRoot.window(window))
        XCTAssertNotEqual(LookRoot.below(12, onSurface: true, window: window), LookRoot.surface("menubar", window: window))
        let book = freshBook()
        let numbers = look(book, ["window", "group", "button"], root: LookRoot.window(window), complete: true).numbers
        let below = look(book, ["group", "button", "new button"], root: LookRoot.below(numbers["group"] ?? -1, onSurface: false, window: window), complete: false).numbers
        let again = look(book, ["window", "group", "button"], root: LookRoot.window(window), complete: true)
        XCTAssertTrue(again.gone.contains(below["new button"] ?? -1), "a control the window no longer shows is forgotten")
    }

    /// The item found again by its number is pressed only for the window the
    /// app sends a menu command to — its key and main window both — and not
    /// once for another of its windows.
    func testAMenuItemFoundAgainIsPressedOnlyForTheWindowTheAppSendsItTo() {
        let book = freshBook()
        let (trash, gone) = menuReadThenWindowReadAgain(book)
        var presses: [String] = []
        func click(looked: String, focused: String?, main: String?) {
            guard !gone.contains(trash), book.existingNumber(for: "Empty Trash") == trash,
                  MenuBarCommand.boundTo(looked, focused: focused, main: main, same: ==) else { return }
            presses.append(looked)
        }
        click(looked: "Documents", focused: "Downloads", main: "Downloads")
        click(looked: "Documents", focused: "Documents", main: "Downloads")
        click(looked: "Documents", focused: nil, main: "Documents")
        XCTAssertEqual(presses, [], "another window's command: no press")
        click(looked: "Documents", focused: "Documents", main: "Documents")
        XCTAssertEqual(presses, ["Documents"])
    }

    /// The helper files a look below an element by that element's place.
    func testTheHelperFilesALookByLookRoot() throws {
        let here = URL(fileURLWithPath: #filePath)
        let main = here.deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/ZeroCodeComputerUseMacOS/main.swift")
        let source = try String(contentsOf: main, encoding: .utf8)
        XCTAssertTrue(source.contains("rootKey = LookRoot.surface(surface, window: capture.windowId)"))
        XCTAssertTrue(source.contains("rootKey = LookRoot.below(rootIndex, onSurface: record.onSurface, window: capture.windowId)"))
        XCTAssertTrue(source.contains("rootKey = LookRoot.window(capture.windowId)"))
        XCTAssertFalse(source.contains("rootKey = \"window:"), "no root spelled outside LookRoot")
    }
}
