import Foundation

/// A launch told to come up behind: what the person had in front goes back in
/// front if the launched app took it by itself, and nothing moves once the person
/// has gone elsewhere. iPhone Mirroring brings itself forward though it was opened
/// with `activates = false`, and a browser does as its first window opens.
public struct FrontWatch {
    public enum Move: Equatable, Sendable {
        case keepWatching
        case giveBack
        /// The person chose: an app of their own, or the launched one itself.
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

    /// What to do at this look at the front. `personJustPressed`: a button or a
    /// Command chord of theirs came right before it, which is how a person brings
    /// an app forward. `nil`: no app holds the front this moment.
    public mutating func see(front: pid_t?, personJustPressed: Bool) -> Move {
        guard let front, front != original else { return .keepWatching }
        guard front == launched, !personJustPressed, givenBack < Self.givesBack else { return .stop }
        givenBack += 1
        return .giveBack
    }

    /// Watches until `until` says the time is up or the person chose. The moves
    /// run on the caller, each read handed in, so a test reads no real front.
    public mutating func run(
        until: () -> Bool,
        front: () -> pid_t?,
        personJustPressed: () -> Bool,
        giveBack: () -> Void,
        pause: () -> Void
    ) -> Bool {
        while !until() {
            switch see(front: front(), personJustPressed: personJustPressed()) {
            case .keepWatching: break
            case .giveBack: giveBack()
            case .stop: return true
            }
            pause()
        }
        return false
    }
}
