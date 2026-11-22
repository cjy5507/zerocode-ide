import Foundation

/// The root a look files the numbers it gives under. A complete look at a
/// root ends by forgetting the numbers of that root it did not meet again
/// (`ElementIdentityBook.endCompleteWalk`), so a number must stand under the
/// root of a look that reaches it.
public enum LookRoot {
    /// A look at the whole window.
    public static func window(_ id: UInt32) -> String {
        "window:\(id)"
    }

    /// A look at what is open over the window: the menu bar, an open menu, a
    /// sheet.
    public static func surface(_ name: String, window id: UInt32) -> String {
        "surface:\(id):\(name)"
    }

    /// A look that starts at an element an earlier look numbered. Below one
    /// of the window's own, the window's: its complete look reaches it again.
    /// Below one that stands on a surface — a menu bar item whose menu is read
    /// closed — that element's own: a later look at the whole window, or at
    /// the menu bar, meets none of that menu's items, and a click by the
    /// number of one must still find it (t-125705 R2).
    public static func below(_ index: Int, onSurface: Bool, window id: UInt32) -> String {
        onSurface ? "below:\(index)" : window(id)
    }
}
