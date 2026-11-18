import CoreGraphics
import Foundation

/// A launch told to come up behind: what the person had in front goes back in
/// front if the launched app took it by itself, and nothing moves once the person
/// chose (`personChose`), or quit the app they had. iPhone Mirroring brings itself
/// forward though it was opened with `activates = false`, and a browser does as its
/// first window opens.
public struct FrontWatch {
    public enum Move: Equatable, Sendable {
        case keepWatching
        case giveBack
        /// The person chose: an app of their own, the launched one itself by a
        /// press or ⌘, or quitting what they had.
        case stop
    }

    /// How many times the front is given back: an app that keeps taking it is not
    /// fought, and the person is left with a window that stands still.
    public static let givesBack = 3

    public let original: pid_t
    public let launched: pid_t
    public private(set) var givenBack = 0

    public init(original: pid_t, launched: pid_t) {
        self.original = original
        self.launched = launched
    }

    /// What to do at this look at the front. `originalAlive`: the app the person
    /// had is still running; giving back one they quit would open it again.
    /// `personChose`: they chose what is in front since the launch
    /// (`personChose(...)`), and a person choosing with their hands is not
    /// overruled. An app of their own coming forward — by Spotlight, ⌘Tab, the
    /// Dock or a swipe — is a choice too, read from the front itself: neither the
    /// app they had nor the launched one. `front` `nil`: no app holds it.
    public mutating func see(front: pid_t?, originalAlive: Bool, personChose: Bool) -> Move {
        if !originalAlive || personChose { return .stop }
        guard let front, front != original else { return .keepWatching }
        guard front == launched, givenBack < Self.givesBack else { return .stop }
        givenBack += 1
        return .giveBack
    }

    /// Whether the person chose what is in front since the launch, `sinceLaunch`
    /// seconds ago: a button they hold, ⌘ they hold (`choosesAnApp`), or a press of
    /// theirs since the launch — `sinceClick`, the newest button going down by the
    /// HID system's table. Typing and moving the pointer choose no app: a person
    /// typing on in their own app as the launched one came forward did not choose
    /// it, and their keys went into it (11-17, a messenger over the person's
    /// screen for 20 s, the watch stopped by the very typing it was there for).
    /// The helper's own process and window input never reaches that table; its
    /// desktop presses do, and an age that matches its own post is that post
    /// (`DesktopBorrow.personAge`).
    public static func personChose(sinceLaunch: Double, reading: DesktopBorrow.Reading, commandHeld: Bool, sinceClick: Double) -> Bool {
        if reading.buttonHeld || commandHeld { return true }
        return DesktopBorrow.personAge(system: sinceClick, own: reading.sinceOwnPointer) < sinceLaunch
    }

    /// Whether the modifiers held now choose an app: ⌘, as ⌘Tab, ⌘Space and ⌘`
    /// do. ⇧ is typing, and ⌃ ⌥ go with keys in the app in front (a remote
    /// desktop's copy and paste): none of them moves the front by itself.
    public static func choosesAnApp(_ flags: CGEventFlags) -> Bool {
        flags.contains(.maskCommand)
    }

    /// Watches until `until` says the time is up or the person chose. The moves
    /// run on the caller, each read handed in, so a test reads no real front.
    public mutating func run(
        until: () -> Bool,
        front: () -> pid_t?,
        originalAlive: () -> Bool,
        personChose: () -> Bool,
        giveBack: () -> Void,
        pause: () -> Void
    ) -> Bool {
        while !until() {
            switch see(front: front(), originalAlive: originalAlive(), personChose: personChose()) {
            case .keepWatching: break
            case .giveBack: giveBack()
            case .stop: return true
            }
            pause()
        }
        return false
    }
}
