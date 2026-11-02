import Foundation

/// What a borrow of the shared pointer and keyboard asks before it takes them,
/// and what it hands back. The person is using the same pointer: their input
/// goes first, and whatever they did while it was borrowed stands.
public enum DesktopBorrow {
    /// Raising another app under someone typing would hand it their next keys.
    public static let keysQuietSeconds: Double = 0.8
    /// A pointer that moved this recently is in the person's hand.
    public static let pointerQuietSeconds: Double = 0.5
    public static let longestWaitSeconds: Double = 3
    /// A modifier down longer than this is a sticky key, not a chord on its way.
    public static let modifierHeldSeconds: Double = 10
    public static let pollSeconds: Double = 0.05
    /// We stamp an event when its post returns and the system when it takes it.
    /// That far on either side of our stamp the system's last event is ours: it
    /// usually took it first, and under load a little after.
    public static let ownEventOlderSeconds: Double = 0.05
    public static let ownEventNewerSeconds: Double = 0.01
    /// How long after a borrow an agent may still take the app it borrowed for
    /// to be in front: longer than the slowest model turn between two acts.
    public static let appLessKeysRefusedSeconds: Double = 60

    /// Why a borrow stands back. The raw value is the refusal's code.
    public enum Busy: String, Equatable {
        case typing = "person_typing"
        case pointing = "person_pointing"
    }

    /// What the system says about the input devices right now, and when this
    /// helper last posted through them. Ages are seconds; nil is never posted.
    public struct Reading: Equatable {
        public var sinceKey: Double
        public var sincePointer: Double
        public var buttonHeld: Bool
        public var modifierHeld: Bool
        public var sinceModifier: Double
        public var sinceOwnKey: Double?
        public var sinceOwnPointer: Double?
        /// A warp may or may not count as a pointer event, so it has its own stamp.
        public var sinceOwnWarp: Double?

        public init(
            sinceKey: Double,
            sincePointer: Double,
            buttonHeld: Bool = false,
            modifierHeld: Bool = false,
            sinceModifier: Double = .infinity,
            sinceOwnKey: Double? = nil,
            sinceOwnPointer: Double? = nil,
            sinceOwnWarp: Double? = nil
        ) {
            self.sinceKey = sinceKey
            self.sincePointer = sincePointer
            self.buttonHeld = buttonHeld
            self.modifierHeld = modifierHeld
            self.sinceModifier = sinceModifier
            self.sinceOwnKey = sinceOwnKey
            self.sinceOwnPointer = sinceOwnPointer
            self.sinceOwnWarp = sinceOwnWarp
        }
    }

    /// How long ago the person last used a device. The system's table may or
    /// may not count what this helper posted; an age that matches our own post
    /// is that post, and says nothing about the person.
    public static func personAge(system: Double, own: Double?...) -> Double {
        for case let own? in own {
            let older = system - own
            if older <= ownEventOlderSeconds, older >= -ownEventNewerSeconds { return .infinity }
        }
        return system
    }

    /// Whether the person is on a device this very moment.
    public static func busy(_ reading: Reading) -> Busy? {
        if reading.buttonHeld { return .pointing }
        let pointer = personAge(system: reading.sincePointer, own: reading.sinceOwnPointer, reading.sinceOwnWarp)
        if pointer < pointerQuietSeconds { return .pointing }
        // A modifier held down is a chord on its way, for as long as a chord takes.
        if reading.modifierHeld, reading.sinceModifier < modifierHeldSeconds { return .typing }
        if personAge(system: reading.sinceKey, own: reading.sinceOwnKey) < keysQuietSeconds { return .typing }
        return nil
    }

    /// Wait for the person to pause, until `giveUpAt` on the `now` clock. Nil when
    /// they did; otherwise what they are still doing. `sleep` throwing ends the
    /// wait: a stop does not wait for the person to pause.
    public static func waitForQuiet(
        read: () -> Reading,
        now: () -> Double,
        giveUpAt: Double,
        sleep: (Double) throws -> Void
    ) rethrows -> Busy? {
        while let busy = busy(read()) {
            guard now() < giveUpAt else { return busy }
            try sleep(pollSeconds)
        }
        return nil
    }

    /// Where the pointer goes when the act ends, or nil to leave it alone: one
    /// that moved nothing, or left it where it found it, has nothing to put back.
    /// What the person moved since our last event stays theirs, added to where
    /// they had it.
    public static func pointerAfter(
        saved: SmoothPointerPath.Point,
        lastPosted: SmoothPointerPath.Point?,
        now: SmoothPointerPath.Point
    ) -> SmoothPointerPath.Point? {
        guard let lastPosted, lastPosted != saved else { return nil }
        return SmoothPointerPath.Point(
            x: saved.x + (now.x - lastPosted.x),
            y: saved.y + (now.y - lastPosted.y)
        )
    }

    /// Whether the person's app comes back. Only over the app this borrow put
    /// in front: any other front app was the person's own choice.
    public static func givesFrontBack(before: Int32?, raised: Int32?, now: Int32?) -> Bool {
        guard let before, let raised, before != raised else { return false }
        return now == raised
    }

    /// Whether an app-less key goes to whatever is in front when a borrow just
    /// put the person's app back: the agent that sent it means the other one.
    public static func refusesAppLessKeys(sinceGiveBack: Double?) -> Bool {
        guard let sinceGiveBack else { return false }
        return sinceGiveBack < appLessKeysRefusedSeconds
    }

    /// Wait until `isFront` says so or `timeout` passes, whichever is first.
    /// True when it came up. A window already up costs one look.
    public static func waitUntilFront(
        timeout: Double,
        interval: Double,
        isFront: () -> Bool,
        now: () -> Double,
        sleep: (Double) -> Void
    ) -> Bool {
        let giveUp = now() + timeout
        while !isFront() {
            guard now() < giveUp else { return false }
            sleep(interval)
        }
        return true
    }
}

/// How typed text is cut into key events.
public enum TextStrokes {
    /// The most UTF-16 units one key event carries.
    public static let unitsPerEvent = 20

    /// One event per character a person sees: a surrogate pair or a joined
    /// emoji sent in pieces lands as broken characters.
    public static func strokes(_ text: String) -> [String] {
        var strokes: [String] = []
        for character in text {
            // Two presses, as it always was: a person's Return is not this one event.
            if character == "\r\n" {
                strokes += ["\r", "\n"]
                continue
            }
            let whole = String(character)
            if whole.utf16.count <= unitsPerEvent {
                strokes.append(whole)
                continue
            }
            var piece = String.UnicodeScalarView()
            var units = 0
            for scalar in whole.unicodeScalars {
                let width = UTF16.width(scalar)
                if units + width > unitsPerEvent {
                    strokes.append(String(piece))
                    piece = String.UnicodeScalarView()
                    units = 0
                }
                piece.append(scalar)
                units += width
            }
            if !piece.isEmpty { strokes.append(String(piece)) }
        }
        return strokes
    }
}
