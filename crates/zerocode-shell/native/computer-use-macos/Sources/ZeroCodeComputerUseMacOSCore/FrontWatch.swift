import Foundation

/// A launch told to come up behind: what the person had in front goes back in
/// front if the launched app took it by itself, and nothing moves once the person
/// has done anything since the launch, or quit the app they had. iPhone Mirroring brings itself forward though it was opened
/// with `activates = false`, and a browser does as its first window opens.
public struct FrontWatch {
    public enum Move: Equatable, Sendable {
        case keepWatching
        case giveBack
        /// The person chose: an app of their own, the launched one itself, any
        /// input of theirs, or quitting what they had.
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
    /// `personActed`: any input of theirs since the launch (`personActed(...)`):
    /// a key in Spotlight, a click, a swipe all bring apps forward, and a person
    /// using their hands is not overruled. `front` `nil`: no app holds it.
    public mutating func see(front: pid_t?, originalAlive: Bool, personActed: Bool) -> Move {
        if !originalAlive || personActed { return .stop }
        guard let front, front != original else { return .keepWatching }
        guard front == launched, givenBack < Self.givesBack else { return .stop }
        givenBack += 1
        return .giveBack
    }

    /// Whether the person used any input device since the launch, `sinceLaunch`
    /// seconds ago, by the HID system's table (`reading`, and `sinceAnyInput`
    /// for every input type, gestures too). The helper's own process and window
    /// input never reaches that table; its desktop input does, and an age that
    /// matches its own post is that post (`DesktopBorrow.personAge`).
    public static func personActed(sinceLaunch: Double, reading: DesktopBorrow.Reading, sinceAnyInput: Double) -> Bool {
        if reading.buttonHeld || reading.modifierHeld { return true }
        let own = [reading.sinceOwnKey, reading.sinceOwnPointer, reading.sinceOwnWarp]
        let ages = [
            DesktopBorrow.personAge(system: reading.sinceKey, own: reading.sinceOwnKey),
            DesktopBorrow.personAge(system: reading.sincePointer, own: reading.sinceOwnPointer, reading.sinceOwnWarp),
            DesktopBorrow.personAge(system: sinceAnyInput, own: own[0], own[1], own[2]),
        ]
        return ages.contains { $0 < sinceLaunch }
    }

    /// Watches until `until` says the time is up or the person chose. The moves
    /// run on the caller, each read handed in, so a test reads no real front.
    public mutating func run(
        until: () -> Bool,
        front: () -> pid_t?,
        originalAlive: () -> Bool,
        personActed: () -> Bool,
        giveBack: () -> Void,
        pause: () -> Void
    ) -> Bool {
        while !until() {
            switch see(front: front(), originalAlive: originalAlive(), personActed: personActed()) {
            case .keepWatching: break
            case .giveBack: giveBack()
            case .stop: return true
            }
            pause()
        }
        return false
    }
}
