import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// A shortcut behind the person that is one of the app's own menu items'
/// (t-125705 D4): found by its keys and the item's place, never by a title.
final class MenuShortcutTests: XCTestCase {
    /// A menu as the helper reads it: a title for the test's eyes only, the
    /// item's own key equivalent, and what it holds — nil when the app would
    /// not say.
    private struct Item {
        let title: String
        let character: String?
        let modifiers: Int?
        let children: [Item]?

        init(_ title: String, _ character: String? = nil, _ modifiers: Int? = nil, _ children: [Item]? = []) {
            self.title = title
            self.character = character
            self.modifiers = character == nil ? modifiers : (modifiers ?? 0)
            self.children = children
        }
    }

    private let shift = 1, option = 2, control = 4, noCommand = 8

    private func shortcut(_ key: String, shift: Bool = false, option: Bool = false, control: Bool = false) -> MenuShortcut {
        MenuShortcut(key: key, command: true, shift: shift, option: option, control: control)!
    }

    private func search(_ shortcut: MenuShortcut, in bar: [Item]) -> MenuShortcut.Found<Item> {
        shortcut.search(in: bar, children: { $0.children }, equivalent: { (.read($0.character), .read($0.modifiers)) })
    }

    private func titles(_ shortcut: MenuShortcut, in bar: [Item]) -> [String] {
        search(shortcut, in: bar).items.map(\.title)
    }

    /// The road a chord takes behind the person, with the app behind them.
    private func road(_ chord: String, _ shortcut: MenuShortcut?, in bar: [Item]) -> MenuShortcut.Road<Item> {
        MenuShortcut.road(chord: chord, shortcut: shortcut, behind: { true }, menus: { .read(bar) },
                          children: { $0.children }, equivalent: { (.read($0.character), .read($0.modifiers)) })
    }

    /// The one item of a finished search can be considered for a press.
    private func pressed(_ road: MenuShortcut.Road<Item>) -> [String] {
        guard case let .item(item) = road else { return [] }
        return [item.title]
    }

    /// A menu bar as macOS lays it out: the system's menu, the app menu, then
    /// the app's own menus.
    private var bar: [Item] {
        [
            Item("Apple", nil, nil, [Item("Log Out", "Q", shift), Item("Lock Screen", "Q", control), Item("Force Quit", "\u{1b}", option)]),
            Item("TextEdit", nil, nil, [Item("Settings", ","), Item("Hide TextEdit", "H"), Item("Hide Others", "H", option), Item("Quit TextEdit", "Q")]),
            Item("File", nil, nil, [Item("New", "N"), Item("Close", "W"), Item("Open Recent", nil, nil, [Item("Clear Menu", "R", control)])]),
            Item("Window", nil, nil, [Item("Minimize", "M"), Item("Minimize All", "M", option)]),
        ]
    }

    func testOnlyCommandWithOnePrintableKeyIsAMenuShortcut() {
        XCTAssertNotNil(MenuShortcut(key: "n", command: true, shift: false, option: false, control: false))
        XCTAssertNotNil(MenuShortcut(key: ",", command: true, shift: false, option: false, control: false))
        XCTAssertNil(MenuShortcut(key: "n", command: false, shift: false, option: false, control: true), "no Command")
        XCTAssertNil(MenuShortcut(key: "return", command: true, shift: false, option: false, control: false), "a named key")
        XCTAssertNil(MenuShortcut(key: "up", command: true, shift: false, option: false, control: false))
        XCTAssertNil(MenuShortcut(key: " ", command: true, shift: false, option: false, control: false))
        XCTAssertEqual(shortcut("n").character, "N")
    }

    func testAnItemAnswersOnlyToItsOwnKeysAndModifiers() {
        XCTAssertTrue(shortcut("s").matches(character: "S", modifiers: 0))
        XCTAssertTrue(shortcut("s").matches(character: "s", modifiers: nil))
        XCTAssertFalse(shortcut("s").matches(character: "S", modifiers: shift), "Save As is cmd+shift+s")
        XCTAssertTrue(shortcut("s", shift: true).matches(character: "S", modifiers: shift))
        XCTAssertFalse(shortcut("s", shift: true).matches(character: "S", modifiers: 0))
        XCTAssertFalse(shortcut("s").matches(character: "S", modifiers: noCommand), "an item that takes no Command")
        XCTAssertTrue(shortcut("m", option: true).matches(character: "M", modifiers: option))
        XCTAssertFalse(shortcut("m", option: true).matches(character: "M", modifiers: option | control))
        XCTAssertFalse(shortcut("s").matches(character: nil, modifiers: 0))
        XCTAssertFalse(shortcut("s").matches(character: "A", modifiers: 0))
    }

    /// cmd+Q, cmd+H and cmd+opt+H are the app menu's — quit, hide, hide the
    /// person's other apps — and the system menu's shortcuts log out or lock:
    /// none becomes a menu item's press behind the person.
    func testTheSystemMenuAndTheAppMenuNeverAnswer() {
        XCTAssertEqual(titles(shortcut("q"), in: bar), [])
        XCTAssertEqual(titles(shortcut("h"), in: bar), [])
        XCTAssertEqual(titles(shortcut("h", option: true), in: bar), [])
        XCTAssertEqual(titles(shortcut(","), in: bar), [])
        XCTAssertEqual(titles(shortcut("q", shift: true), in: bar), [])
        XCTAssertEqual(titles(shortcut("q", control: true), in: bar), [])
    }

    func testTheAppsOwnMenusAnswerTheirShortcutsDownToASubmenu() {
        XCTAssertEqual(titles(shortcut("n"), in: bar), ["New"])
        XCTAssertEqual(titles(shortcut("m"), in: bar), ["Minimize"])
        XCTAssertEqual(titles(shortcut("m", option: true), in: bar), ["Minimize All"])
        XCTAssertEqual(titles(shortcut("r", control: true), in: bar), ["Clear Menu"])
        XCTAssertEqual(titles(shortcut("z"), in: bar), [])
    }

    /// What is left out is told by its place in the menu bar, never by a name:
    /// a cmd+Q in the app's own File menu answers, and an app menu titled like
    /// an own menu still does not.
    func testTheMenusLeftOutAreToldByTheirPlaceNotTheirNames() {
        let renamed = [
            Item("File", nil, nil, [Item("Quit", "Q")]),
            Item("Edit", nil, nil, [Item("Quit", "Q"), Item("Hide Others", "H", option)]),
            Item("Apple", nil, nil, [Item("Quit Now", "Q"), Item("Hide Others", "H", option)]),
        ]
        XCTAssertEqual(titles(shortcut("q"), in: renamed), ["Quit Now"])
        XCTAssertEqual(titles(shortcut("h", option: true), in: renamed), ["Hide Others"])
        XCTAssertEqual(MenuShortcut.firstOwnMenu, 2)
    }

    /// Two items with one shortcut are both found, so the press is refused
    /// unsent rather than one being guessed.
    func testTwoItemsWithOneShortcutAreBothFound() {
        var twice = bar
        twice.append(Item("Edit", nil, nil, [Item("Close Tab", "W")]))
        XCTAssertEqual(titles(shortcut("w"), in: twice), ["Close", "Close Tab"])
        guard case let .several(items) = road("cmd+w", shortcut("w"), in: twice) else { return XCTFail("two items are several") }
        XCTAssertEqual(items.map(\.title), ["Close", "Close Tab"])
        let words = MenuShortcut.severalWords(app: "TextEdit", key: "cmd+w", titles: ["Close", "Close Tab"])
        XCTAssertTrue(words.hasPrefix("cmd+w is the shortcut of more than one of TextEdit's menu items ('Close', 'Close Tab'); nothing was sent"), words)
    }

    func testAHugeMenuIsReadOnlySoFar() {
        let filler = (0..<MenuShortcut.maxItems).map { Item("Item \($0)") }
        let huge = [Item("Apple"), Item("App"), Item("Big", nil, nil, filler + [Item("Last", "L")])]
        XCTAssertEqual(titles(shortcut("l"), in: huge), [], "the item past the budget is not read")
        let small = [Item("Apple"), Item("App"), Item("Big", nil, nil, Array(filler.prefix(10)) + [Item("Last", "L")])]
        XCTAssertEqual(titles(shortcut("l"), in: small), ["Last"])
    }

    /// t-125705 R4: one item found is the shortcut's only when every own menu
    /// was read to its end. A budget spent, a submenu deeper than the search
    /// goes, or a menu the app would not read leaves the search unfinished;
    /// an item with a key of its own hides no submenu from it.
    func testASearchIsFinishedOnlyWhenEveryOwnMenuWasReadToItsEnd() {
        XCTAssertTrue(search(shortcut("n"), in: bar).finished)
        XCTAssertTrue(search(shortcut("z"), in: bar).finished, "read to the end and nothing found")

        let filler = (0..<MenuShortcut.maxItems).map { Item("Item \($0)") }
        let past = [Item("Apple"), Item("App"), Item("Big", nil, nil, [Item("First", "F")] + filler)]
        XCTAssertEqual(titles(shortcut("f"), in: past), ["First"])
        XCTAssertFalse(search(shortcut("f"), in: past).finished, "a budget spent before the last item")
        let full = [Item("Apple"), Item("App"), Item("Big", nil, nil, [Item("First", "F")] + filler.dropFirst())]
        XCTAssertTrue(search(shortcut("f"), in: full).finished, "the budget spent on the last item leaves nothing unread")

        let deep = [Item("Apple"), Item("App"), Item("Format", nil, nil, [
            Item("Text", nil, nil, [Item("Direction", nil, nil, [Item("Paragraph", "P"), Item("Selection", nil, nil, [Item("Right to Left", "P")])])]),
        ])]
        XCTAssertEqual(titles(shortcut("p"), in: deep), ["Paragraph"])
        XCTAssertFalse(search(shortcut("p"), in: deep).finished, "a submenu deeper than the search goes")

        let unread = [Item("Apple"), Item("App"), Item("File", nil, nil, [Item("New", "N")]), Item("Edit", nil, nil, nil)]
        XCTAssertEqual(titles(shortcut("n"), in: unread), ["New"])
        XCTAssertFalse(search(shortcut("n"), in: unread).finished, "a menu the app would not read")

        let keyed = [Item("Apple"), Item("App"), Item("View", nil, nil, [Item("Zoom", "Z", nil, [Item("Zoom In", "Z")])])]
        XCTAssertEqual(titles(shortcut("z"), in: keyed), ["Zoom", "Zoom In"], "an item's own key hides no submenu")
    }

    /// t-125705 R4: one item from an unfinished search is refused unpressed —
    /// another may answer where the search did not read, even with no match.
    func testOneItemFromAnUnfinishedSearchIsRefusedUnpressed() {
        let trash = Item("File", nil, nil, [Item("Empty Trash", "T", shift)])
        let unread = [Item("Apple"), Item("App"), trash, Item("Edit", nil, nil, nil)]
        let unfinished = road("cmd+shift+t", shortcut("t", shift: true), in: unread)
        guard case let .unfinished(item) = unfinished else { return XCTFail("one item of an unfinished search: \(unfinished)") }
        XCTAssertEqual(item?.title, "Empty Trash")
        XCTAssertEqual(pressed(unfinished), [], "nothing is pressed")
        let words = MenuShortcut.unfinishedWords(app: "Finder", key: "cmd+shift+t", title: "Empty Trash")
        XCTAssertTrue(words.hasPrefix("cmd+shift+t is the shortcut of Finder's 'Empty Trash' menu item, but its menus could not be read to the end"), words)
        XCTAssertTrue(words.contains("nothing was sent"), words)

        let finished = road("cmd+shift+t", shortcut("t", shift: true), in: [Item("Apple"), Item("App"), trash])
        XCTAssertEqual(pressed(finished), ["Empty Trash"], "read to the end, the one item is the shortcut's")

        guard case .unfinished(nil) = road("cmd+k", shortcut("k"), in: unread) else {
            return XCTFail("no match in an unfinished search must still refuse")
        }
    }

    /// t-125705 R5: a copy, cut or paste chord behind the person is refused
    /// before any menu is read — the shared clipboard is the person's — even
    /// where an own menu item answers to it: no item pressed, no key sent.
    func testAClipboardChordIsRefusedBeforeAnyMenuIsRead() {
        let edit = [Item("Apple"), Item("App"), Item("Edit", nil, nil, [
            Item("Copy", "C"), Item("Cut", "X"), Item("Paste", "V"), Item("Paste and Match Style", "V", option | shift), Item("Select All", "A"),
        ])]
        for (chord, shortcut) in [
            ("cmd+c", shortcut("c")), ("cmd+x", shortcut("x")), ("cmd+v", shortcut("v")),
            ("cmd+option+shift+v", shortcut("v", shift: true, option: true)),
        ] {
            var asked = 0
            var read = 0
            let road = MenuShortcut.road(chord: chord, shortcut: shortcut,
                                         behind: { asked += 1; return true }, menus: { read += 1; return .read(edit) },
                                         children: { $0.children }, equivalent: { (.read($0.character), .read($0.modifiers)) })
            guard case .clipboard = road else {
                XCTFail("\(chord) went the \(road) road")
                continue
            }
            XCTAssertEqual(read, 0, "\(chord): no menu read")
            XCTAssertEqual(asked, 0, "\(chord): refused before anything else is asked")
            XCTAssertEqual(pressed(road), [], chord)
        }
        // The chord the clipboard does not take is the menu's as it was.
        XCTAssertEqual(pressed(road("cmd+a", shortcut("a"), in: edit)), ["Select All"])
        XCTAssertEqual(BackgroundInput.clipboardWords, "background keys never use the shared clipboard; pass text directly instead")
    }

    func testShortcutSearchIsSharedByConfirmationAndSending() throws {
        let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let source = try String(contentsOf: package.appendingPathComponent("Sources/ZeroCodeComputerUseMacOS/main.swift"), encoding: .utf8)
        XCTAssertEqual(source.components(separatedBy: "try menuShortcutItem(params, snapshot: snapshot)").count - 1, 1)
        XCTAssertTrue(source.contains("if dispatchShortcut.found { return dispatchShortcut.item }"))
        XCTAssertTrue(source.contains("shortcut: shortcutForDispatch, snapshot: snapshotForDispatch"))
        let road = try XCTUnwrap(source.range(of: "private func menuShortcutItem("))
        let send = try XCTUnwrap(source.range(of: "private func backgroundMenuShortcut("))
        let search = source[road.upperBound..<send.lowerBound]
        XCTAssertTrue(search.contains("case .clipboard:"))
        XCTAssertTrue(search.contains("case let .unfinished(item):"))
        XCTAssertFalse(search.contains("performActionChecked"))
        XCTAssertTrue(source[send.upperBound...].prefix(1000).contains("try requireBackgroundPress("))
    }
}
