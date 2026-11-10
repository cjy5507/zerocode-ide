/// The keyboard language the helper last read on its simulator, so the next text in that language
/// types at once instead of reading the language again with two probe keys (about 1.5 s a text on
/// a simulator, t-101963).
///
/// It is only ever what the helper itself read: a probe that recognized its keys, or a text that
/// was then seen in the field. It is dropped when the helper cannot tell what the keyboard is in —
/// a typing that failed part way, a field that hid what was typed — and it is not trusted once it is
/// older than `keep`: the person can change the keyboard on the device itself, which no request
/// here sees. A reading belongs to the boot it was made in (`boot`, the device's own last boot
/// time): a device booted since has its keyboard as its settings say, so nothing is remembered.
///
/// Remembering is not taking on trust. A text typed on a remembered language is looked for in the
/// field afterwards (`TypedCheck`), and a field that does not show it has the language read again
/// at once; a remembered language that turns out wrong is said, and nothing is deleted.
struct KeyboardMemory<State: Equatable> {
    /// How long a reading is trusted, in seconds of the system's uptime.
    static var keep: Double { 600 }

    private var known: (state: State, at: Double, boot: Double?)?

    init() {}

    /// The language last read, when it was read within `keep` of `now` and in the boot `boot`
    /// (`nil` when the device does not say: the age alone decides then).
    func language(at now: Double, boot: Double?) -> State? {
        guard let known, now >= known.at, now - known.at <= Self.keep, known.boot == boot else {
            return nil
        }
        return known.state
    }

    mutating func remember(_ state: State, at now: Double, boot: Double?) {
        known = (state, now, boot)
    }

    mutating func forget() {
        known = nil
    }
}

/// What the field showed after a text was typed on a remembered language.
enum TypedCheck {
    /// The text showed: the language was right.
    case shown
    /// The text did not show within the wait: the language has to be read again before this
    /// typing is called done, since a wrong language writes other letters (`lantern` on a Korean
    /// keyboard is 「ㅣ뭇ㄷ구」).
    case notShown

    /// Whether a line holds the typed text, read without spaces and case aside: a field may
    /// capitalize the first letter a hardware key types ("Lantern"), which is the same language.
    static func shows(_ text: String, in lines: [String]) -> Bool {
        let squeezed = { (line: String) in line.lowercased().filter { !$0.isWhitespace } }
        let typed = squeezed(text)
        guard !typed.isEmpty else { return false }
        return lines.contains { squeezed($0).contains(typed) }
    }

    /// Read the screen's values until `shows` finds the text or the wall clock passes `within` —
    /// a web page's field tells the tree later than a native one (`ProbeWait`). The first read
    /// comes at once: a native field already holds the keys when the last one has been sent.
    static func wait(
        within: Double = ProbeWait.within,
        gap: Double = ProbeWait.gap,
        now: () -> Double,
        sleep: (Double) -> Void,
        read: () -> [String]?,
        shows: ([String]) -> Bool
    ) -> TypedCheck {
        let deadline = now() + within
        while true {
            if let lines = read(), shows(lines) { return .shown }
            if now() >= deadline { return .notShown }
            sleep(gap)
        }
    }
}
