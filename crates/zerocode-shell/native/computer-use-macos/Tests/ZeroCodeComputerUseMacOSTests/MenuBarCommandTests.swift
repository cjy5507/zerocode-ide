import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// A command from an app's menu bar behind the person (t-125705 D2): read
/// closed, pressed through its own action, never opened over their app.
final class MenuBarCommandTests: XCTestCase {
    func testOpeningAMenuIsRefusedButClosingOneIsAllowed() throws {
        let cases: [(String, [String])] = [
            ("AXMenuBarItem", []), ("AXPopUpButton", []), ("AXMenuButton", []),
            ("AXMenuItem", ["AXMenu"]), ("AXMenuItem", ["AXMenuItem"]),
        ]
        for (role, children) in cases {
            for action in ["AXPress", "AXPick", "AXShowMenu", "AXConfirm"] {
                XCTAssertThrowsError(try PressCertainty.requireClosedCommand(role: role, action: action, children: { children })) {
                    XCTAssertEqual($0 as? PressCertainty.Refusal, .opensMenu)
                }
            }
            try PressCertainty.requireClosedCommand(role: role, action: "AXCancel", children: { children })
        }
        try PressCertainty.requireClosedCommand(role: "AXMenuItem", action: "AXPress", children: { [String]() })
        try PressCertainty.requireClosedCommand(role: "AXButton", action: "AXPress", children: { nil as [String]? })
        XCTAssertThrowsError(try PressCertainty.requireClosedCommand(role: "AXButton", action: "AXShowMenu", children: { [String]() }))
        XCTAssertEqual(MenuBarCommand.menuOpenerRoles, ["AXMenuBarItem", "AXPopUpButton", "AXMenuButton"])
        let popUp = MenuBarCommand.openingWords(app: "Safari", title: "Font size", index: 42, role: "AXPopUpButton")
        XCTAssertTrue(popUp.contains("'Font size' pop-up button"))
        XCTAssertFalse(popUp.contains("--root"))
        XCTAssertTrue(MenuBarCommand.openingWords(app: "TextEdit", title: "Font", index: nil, role: "AXMenuItem").contains("--root <its number>"))
    }

    /// macOS sends a menu command to the app's key and main window: it goes
    /// only when both are the window the agent looked at.
    func testACommandGoesOnlyToTheWindowTheAppSendsItTo() {
        let same: (Int, Int) -> Bool = { $0 == $1 }
        XCTAssertTrue(MenuBarCommand.boundTo(42, focused: 42, main: 42, same: same))
        XCTAssertFalse(MenuBarCommand.boundTo(42, focused: 42, main: 7, same: same))
        XCTAssertFalse(MenuBarCommand.boundTo(42, focused: 7, main: 42, same: same))
        XCTAssertFalse(MenuBarCommand.boundTo(42, focused: nil, main: 42, same: same), "no key window behind the person is no binding")
        XCTAssertFalse(MenuBarCommand.boundTo(42, focused: nil, main: nil, same: same))
        XCTAssertFalse(MenuBarCommand.boundTo(nil, focused: 42, main: 42, same: same), "a look that read no window binds nothing")
    }

    func testTheWordsNameTheMenuTheItemAndTheNextStep() {
        let opening = MenuBarCommand.openingWords(app: "Finder", title: "파일", index: 190)
        XCTAssertTrue(opening.contains("opening Finder's '파일' menu would draw it over the person's app and take their keyboard focus; nothing was done"), opening)
        XCTAssertTrue(opening.contains("get-app-state --app Finder --root 190"), opening)
        XCTAssertTrue(opening.contains("--foreground, and only if they asked for this on their desktop"), opening)

        let disabled = MenuBarCommand.disabledWords(app: "Finder", title: "새로운 폴더")
        XCTAssertTrue(disabled.hasPrefix("macOS keeps Finder's '새로운 폴더' menu item disabled while Finder is behind the person's window; nothing was sent"), disabled)
        XCTAssertTrue(disabled.contains("--foreground only if they asked for this on their desktop"), disabled)

        let unbound = MenuBarCommand.unboundWords(app: "TextEdit", looked: "Notes 2", focused: nil, main: "Notes")
        XCTAssertTrue(unbound.contains("key window (none) and main window ('Notes'), not the window this look read ('Notes 2'); nothing was sent"), unbound)
        XCTAssertTrue(MenuBarCommand.unboundWords(app: "TextEdit", looked: "", focused: nil, main: nil).contains("(an untitled window)"))

        XCTAssertEqual(MenuBarCommand.readingLine(app: "Sublime Text"),
                       "A menu's items, read closed: get-app-state --app \"Sublime Text\" --root <its number>. Nothing opens over the person's app.")
        XCTAssertFalse(MenuBarCommand.openingWords(app: "Finder", title: nil, index: 3).contains("''"), "an untitled menu is not named")
    }

    func testTheHelperReadsClosedMenusAndUsesTheGateAtAXSendTime() throws {
        let package = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let source = try String(contentsOf: package.appendingPathComponent("Sources/ZeroCodeComputerUseMacOS/main.swift"), encoding: .utf8)
        XCTAssertTrue(source.contains("let readsItsMenu = ancestors.isEmpty && MenuBarCommand.menuOwnerRoles.contains(role)"))
        XCTAssertTrue(source.contains("if !readsItsMenu, summary != nil || SnapshotRenderHeuristics.shouldSuppressChildren(node) {"))
        XCTAssertTrue(source.contains("onSurface = record.onSurface"))
        let road = try XCTUnwrap(source.range(of: "private func backgroundAppAction("))
        let send = source[road.upperBound...].prefix(1850)
        let opens = try XCTUnwrap(send.range(of: "try requireClosedCommand("))
        let shows = try XCTUnwrap(send.range(of: "if !closed, let point"))
        let performs = try XCTUnwrap(send.range(of: "try performActionChecked(record.element, action, beforeSend: requireCertainPress)"))
        XCTAssertTrue(send.contains("try requireBackgroundPress("))
        XCTAssertTrue(send.contains("beforeScroll: requireCertainPress"))
        XCTAssertLessThan(opens.lowerBound, shows.lowerBound)
        XCTAssertLessThan(shows.lowerBound, performs.lowerBound)
    }
}
