import Foundation

/// A shortcut behind the person that is one of the app's own menu items' key
/// equivalent (t-125705 D4). A window key never reaches an app's menu
/// shortcuts behind the person — t2's cmd+shift+n was sent and did nothing,
/// and cua-driver found the same — so such a shortcut is that item, found by
/// the shortcut's own keys and the item's place in the menu bar, never by a
/// title, and pressed as a menu bar command is.
public struct MenuShortcut: Equatable, Sendable {
    /// The base key as a menu item spells it: one character, upper case.
    public let character: String
    public let shift: Bool
    public let option: Bool
    public let control: Bool

    /// The `kAXMenuItemModifier*` bits of `AXMenuItemCmdModifiers`.
    static let shiftBit = 1 << 0
    static let optionBit = 1 << 1
    static let controlBit = 1 << 2
    static let noCommandBit = 1 << 3

    /// Menu items read for one shortcut at most, so a huge menu cannot stall it.
    public static let maxItems = 2_000
    /// How deep into submenus a shortcut is looked for.
    public static let maxSubmenuDepth = 2
    /// The whole search shares one AX messaging interval.
    public struct Deadline {
        private let end: TimeInterval
        private let now: () -> TimeInterval

        public init(now: @escaping () -> TimeInterval) {
            self.now = now
            self.end = now() + TimeInterval(AccessibilityTimeouts.messagingSeconds)
        }

        public func remaining() -> TimeInterval { max(0, end - now()) }
        public func expired() -> Bool { remaining() == 0 }
    }

    /// The menu bar's first two menus are the system's and the app menu
    /// (hide, hide others, services, quit): their commands reach past the
    /// app's own window — the person's other apps, their session — so a
    /// shortcut is never looked for there, and keeps the window key it was.
    public static let firstOwnMenu = 2

    /// A shortcut with Command and one printable key — the ones a menu item
    /// answers to; nil for any other (no Command, a named key such as return),
    /// which keeps the window key it was.
    public init?(key: String, command: Bool, shift: Bool, option: Bool, control: Bool) {
        guard command, key.count == 1, let scalar = key.unicodeScalars.first,
              !CharacterSet.whitespacesAndNewlines.contains(scalar),
              !CharacterSet.controlCharacters.contains(scalar)
        else { return nil }
        character = key.uppercased()
        self.shift = shift
        self.option = option
        self.control = control
    }

    /// Whether a menu item's own key equivalent — its `AXMenuItemCmdChar` and
    /// `AXMenuItemCmdModifiers` — is this shortcut, modifiers and all.
    public func matches(character itemCharacter: String?, modifiers: Int?) -> Bool {
        guard let itemCharacter, itemCharacter.uppercased() == character else { return false }
        let bits = modifiers ?? 0
        return bits & Self.noCommandBit == 0
            && (bits & Self.shiftBit != 0) == shift
            && (bits & Self.optionBit != 0) == option
            && (bits & Self.controlBit != 0) == control
    }

    /// A menu item's key equivalent as the app answered it: its character
    /// and its modifier bits, each a value — none at all is a value — or
    /// unread.
    public typealias Equivalent = (character: PressCertainty.Reading<String?>, modifiers: PressCertainty.Reading<Int?>)

    /// Whether an item answers to this shortcut, as far as its key was read
    /// (t-125705 R4). Either unanswered read leaves the search unfinished.
    public func answers(_ key: Equivalent) -> PressCertainty.Reading<Bool> {
        guard case let .read(itemCharacter) = key.character, case let .read(bits) = key.modifiers else { return .unread }
        guard let itemCharacter, itemCharacter.uppercased() == character else { return .read(false) }
        guard let bits else { return .unread }
        return .read(matches(character: itemCharacter, modifiers: bits))
    }

    /// What a search of the app's own menus found, and whether it read them
    /// to the end.
    public struct Found<Item> {
        public let items: [Item]
        /// Every own menu read to its last item: the budget and the time not
        /// spent, no submenu left below `maxSubmenuDepth`, no menu that would
        /// not say what it holds, no item whose key would not be read. Only
        /// then is what was found all the shortcut fires — one item, or none
        /// (t-125705 R4, R10).
        public let finished: Bool
    }

    /// The items this shortcut fires among a menu bar's menus: the app's own
    /// menus only (from `firstOwnMenu` on, by their place), their submenus to
    /// `maxSubmenuDepth`, at most `maxItems` items read, until `expired`.
    /// `children` reads what a menu or an item holds — nil when the app would
    /// not say, which leaves the search unfinished; `equivalent` an item's key
    /// equivalent, each part read or not.
    public func search<Item>(
        in menus: [Item],
        children: (Item) -> [Item]?,
        equivalent: (Item) -> Equivalent,
        expired: () -> Bool = { false }
    ) -> Found<Item> {
        var budget = Self.maxItems
        var found: [Item] = []
        var finished = true
        func search(_ owner: Item, depth: Int) {
            guard finished, !expired(), let items = children(owner), !expired() else {
                finished = false
                return
            }
            for item in items {
                guard budget > 0, !expired() else {
                    finished = false
                    return
                }
                budget -= 1
                switch answers(equivalent(item)) {
                case .read(true): found.append(item)
                case .read(false): break
                // A key not read may be this shortcut's.
                case .unread: finished = false
                }
                guard finished, !expired() else {
                    finished = false
                    return
                }
                // What an item holds is its submenu, read whether or not the
                // item has a key of its own: nothing under it goes unread.
                if depth < Self.maxSubmenuDepth {
                    search(item, depth: depth + 1)
                } else if children(item).map({ !$0.isEmpty }) ?? true {
                    finished = false
                }
                guard finished else { return }
            }
        }
        for menu in menus.dropFirst(Self.firstOwnMenu) where finished {
            search(menu, depth: 0)
        }
        return Found(items: found, finished: finished && !expired())
    }

    /// Where a chord behind the person goes (t-125705 D4).
    public enum Road<Item> {
        /// It copies, cuts or pastes. The shared clipboard is the person's:
        /// refused before any menu is read, and no key is sent (R5).
        case clipboard
        /// No own menu item answers to it, by a search that read every own
        /// menu — or the app has no menu bar: it goes as the window key it was.
        case windowKey
        /// The one item it fires, found by a search that read every own menu.
        case item(Item)
        /// More than one item answers to it: refused unsent.
        case several([Item])
        /// The menus were not read to the end — a menu bar, a menu or an
        /// item's key the app would not say, the budget or the time spent —
        /// so an item not read may answer to it: refused unsent, and no
        /// window key goes in its place, whether one item was found or none
        /// (R4, R10).
        case unfinished(Item?)
    }

    /// The road of a chord behind the person: the clipboard's refusal before
    /// anything is read; then, for a menu shortcut of an app behind the person
    /// (`behind`), what a search of every own menu found. `menus` reads the
    /// menu bar's menus — read as nil when the app has no menu bar, unread
    /// when it would not say.
    public static func road<Item>(
        chord: String,
        shortcut: MenuShortcut?,
        behind: () throws -> Bool,
        menus: () -> PressCertainty.Reading<[Item]?>,
        children: (Item) -> [Item]?,
        equivalent: (Item) -> Equivalent,
        expired: () -> Bool = { false }
    ) rethrows -> Road<Item> {
        if KeyboardInputSafety.usesClipboard(chord) { return .clipboard }
        guard let shortcut, try behind() else { return .windowKey }
        let bar: [Item]
        guard !expired() else { return .unfinished(nil) }
        let reading = menus()
        guard !expired() else { return .unfinished(nil) }
        switch reading {
        case .unread: return .unfinished(nil)
        case .read(nil): return .windowKey
        case let .read(read?): bar = read
        }
        let found = shortcut.search(in: bar, children: children, equivalent: equivalent, expired: expired)
        do {
            try PressCertainty.requireCompleteSearch(found.finished)
        } catch {
            return .unfinished(found.items.first)
        }
        guard found.items.count < 2 else { return .several(found.items) }
        return found.items.first.map(Road.item) ?? .windowKey
    }

    public static func severalWords(app: String, key: String, titles: [String]) -> String {
        "\(key) is the shortcut of more than one of \(app)'s menu items (\(titles.map { "'\($0)'" }.joined(separator: ", "))); nothing was sent. "
            + "Read the menus closed with get-app-state --app \(MenuBarCommand.quoted(app)) --root <a menu's number> and press the one you mean"
    }

    public static func unfinishedWords(app: String, key: String, title: String?) -> String {
        let found = title.map { "\(key) is the shortcut of \(app)'s '\($0)' menu item, but its menus could not be read to the end, so another item may answer to it too" }
            ?? "\(app)'s menus could not be read to the end, so a menu item not read may answer to \(key)"
        return found + "; nothing was sent, not even a key. "
            + "Read the menus closed with get-app-state --app \(MenuBarCommand.quoted(app)) --root <a menu's number> and press the one you mean, or use a control in the window"
    }
}
