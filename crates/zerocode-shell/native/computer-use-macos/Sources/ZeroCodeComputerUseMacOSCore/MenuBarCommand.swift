import Foundation

/// A command from an app's menu bar, behind the person (t-125705 D2). Its
/// menus are read closed and an item is pressed through its own action, so no
/// menu opens over the person's app: an open menu draws over their app and
/// takes their keyboard focus while it is open (cua-driver's background menu
/// route does the same). macOS sends a menu command to the app's key and main
/// window, so one goes only when that is the window the agent looked at, and
/// an item macOS keeps disabled while the app is behind is refused unsent.
public enum MenuBarCommand {
    /// How far up from a menu item its menu bar may stand: a menu is three
    /// levels (item, menu, menu bar item), each submenu two more.
    public static let barDepth = 16

    /// The controls a look that starts at them reads the menu of, closed.
    public static let menuOwnerRoles: Set<String> = ["AXMenuBarItem", "AXMenuItem"]

    /// The controls a press opens a menu from: a menu bar item, a pop-up
    /// button, a menu button (t-125705 R3). Whether a press opens a menu is
    /// one of the facts every press behind the person reads just before it
    /// goes (`PressCertainty.requireClosedCommand`).
    public static let menuOpenerRoles: Set<String> = ["AXMenuBarItem", "AXPopUpButton", "AXMenuButton"]

    /// Whether the window the agent looked at is where the app sends its menu
    /// commands: its key window and its main window both. `same` tells two
    /// windows apart (the helper compares accessibility elements).
    public static func boundTo<Window>(_ looked: Window?, focused: Window?, main: Window?, same: (Window, Window) -> Bool) -> Bool {
        guard let looked, let focused, let main else { return false }
        return same(focused, looked) && same(main, looked)
    }

    /// The one line a look at the menu bar ends with: how to read a menu.
    public static func readingLine(app: String) -> String {
        "A menu's items, read closed: get-app-state --app \(quoted(app)) --root <its number>. Nothing opens over the person's app."
    }

    /// Words for a menu a press would open. A menu bar item or an item with a
    /// submenu is read closed by its number (`index`; nil for a shortcut's
    /// item, which has none yet); a pop-up or menu button shows its choices
    /// only while its menu is open.
    public static func openingWords(app: String, title: String?, index: Int?, role: String = "AXMenuBarItem") -> String {
        guard role == "AXPopUpButton" || role == "AXMenuButton" else {
            return "opening \(app)'s \(named(title))menu would draw it over the person's app and take their keyboard focus; nothing was done. "
                + "Read its items closed with get-app-state --app \(quoted(app)) --root \(index.map(String.init) ?? "<its number>"), then press the item itself. "
                + "A menu that shows no items there fills them only when it opens: that needs --foreground, and only if they asked for this on their desktop"
        }
        return "pressing \(app)'s \(named(title))\(role == "AXPopUpButton" ? "pop-up" : "menu") button would open its menu over the person's app and take their keyboard focus; nothing was done. "
            + "Its choices show only while that menu is open: hand it to them, or use --foreground only if they asked for this on their desktop"
    }

    public static func disabledWords(app: String, title: String?) -> String {
        "macOS keeps \(app)'s \(named(title))menu item disabled while \(app) is behind the person's window; nothing was sent. "
            + "Use a control in the window instead, or --foreground only if they asked for this on their desktop"
    }

    /// Words for a command the app would send elsewhere; a window is named by
    /// its title, nil being no such window.
    public static func unboundWords(app: String, looked: String?, focused: String?, main: String?) -> String {
        "\(app) sends a menu command to its key window (\(window(focused))) and main window (\(window(main))), "
            + "not the window this look read (\(window(looked))); nothing was sent. "
            + "Look at that window and press the item there, or --foreground only if they asked for this on their desktop"
    }

    private static func named(_ title: String?) -> String {
        guard let title = title?.trimmingCharacters(in: .whitespacesAndNewlines), !title.isEmpty else { return "" }
        return "'\(title)' "
    }

    private static func window(_ title: String?) -> String {
        guard let title else { return "none" }
        let shown = title.trimmingCharacters(in: .whitespacesAndNewlines)
        return shown.isEmpty ? "an untitled window" : "'\(shown)'"
    }

    /// An app's name as a command line takes it.
    static func quoted(_ app: String) -> String {
        app.contains(" ") ? "\"\(app)\"" : app
    }
}
