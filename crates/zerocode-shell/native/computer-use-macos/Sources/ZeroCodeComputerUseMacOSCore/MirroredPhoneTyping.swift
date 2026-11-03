import Foundation

/// The language a mirrored phone's keyboard types in.
public enum PhoneLanguage: String, Equatable, Sendable {
    case korean
    case english
}

/// One key of the phone's own keyboard, by its Mac key code.
public struct PhoneKey: Equatable, Sendable {
    public let code: UInt16
    public let shift: Bool

    public init(code: UInt16, shift: Bool = false) {
        self.code = code
        self.shift = shift
    }

    /// The keyboard usage (HID page 7) this key is: what a mirrored phone is
    /// handed for its code (UniversalHIDKit's table), and what a simulator's
    /// keyboard is sent.
    public var usage: UInt32? { Self.usages[code] }

    private static let usages: [UInt16: UInt32] = [
        0: 0x04, 1: 0x16, 2: 0x07, 3: 0x09, 4: 0x0B, 5: 0x0A, 6: 0x1D, 7: 0x1B, 8: 0x06, 9: 0x19, 11: 0x05,
        12: 0x14, 13: 0x1A, 14: 0x08, 15: 0x15, 16: 0x1C, 17: 0x17, 18: 0x1E, 19: 0x1F, 20: 0x20, 21: 0x21,
        22: 0x23, 23: 0x22, 24: 0x2E, 25: 0x26, 26: 0x24, 27: 0x2D, 28: 0x25, 29: 0x27, 30: 0x30, 31: 0x12,
        32: 0x18, 33: 0x2F, 34: 0x0C, 35: 0x13, 37: 0x0F, 38: 0x0D, 39: 0x34, 40: 0x0E, 41: 0x33, 42: 0x31,
        43: 0x36, 44: 0x38, 45: 0x11, 46: 0x10, 47: 0x37, 49: 0x2C, 50: 0x35,
    ]
}

/// A piece of the text typed in one language. `nil`: the same keys write it in
/// either (digits, space and most punctuation).
public struct PhoneRun: Equatable, Sendable {
    public let language: PhoneLanguage?
    public let text: String
    public let keys: [PhoneKey]
}

/// What the phone's keyboard is in, as a probe read it: composing Korean,
/// English, or the jamo of a Korean layout left under an English input mode.
public enum PhoneKeyboardState: Equatable, Sendable {
    case korean
    case english
    case looseJamo
}

/// The chords that move a mirrored phone's keyboard between its languages.
public enum PhoneSwitch: Equatable, Sendable {
    /// Caps Lock, sent as the system-defined event iPhone Mirroring forwards.
    case capsLock
    /// Control held, Space pressed with no flags on it, Control let go.
    case controlSpace
}

/// iPhone Mirroring hands the phone key positions, not characters: a key
/// event's code becomes a keyboard usage and its characters are dropped, a
/// modifier counts only from flagsChanged, and Caps Lock only from a
/// system-defined event (UniversalHIDKit, macOS 26.3). A character sent as
/// Unicode on key code 0 lands as the A key, which a Korean keyboard writes as
/// ㅁ. So text reaches the phone the way a person types it: as the keys of the
/// phone's own keyboard, in the language it is in.
public enum PhoneTyping {
    /// iPhone Mirroring's bundle identifier.
    public static let mirroringBundleId = "com.apple.ScreenContinuity"

    public enum Refusal: Error, Equatable, Sendable {
        /// Characters no key of the phone's keyboard writes: emoji, other scripts.
        case unsupported(String)
        /// A line break would press Return, which can send a message.
        case lineBreak
        /// A lone jamo beside a syllable composes into it; it cannot be typed as written.
        case looseJamo(String)
    }

    /// The text as runs of keys, split where the language must change. Nothing
    /// is planned for text that cannot be typed as written.
    public static func plan(_ text: String) -> Result<[PhoneRun], Refusal> {
        var runs: [PhoneRun] = []
        var unsupported = ""
        var loose = ""
        let characters = Array(text)
        for (index, character) in characters.enumerated() {
            if character == "\n" || character == "\r" || character == "\r\n" {
                return .failure(.lineBreak)
            }
            guard let (language, keys) = keysFor(character) else {
                if !unsupported.contains(character) { unsupported.append(character) }
                continue
            }
            if isCompatibilityJamo(character) {
                let before = index > 0 ? characters[index - 1] : nil
                let after = index + 1 < characters.count ? characters[index + 1] : nil
                if before.map({ composes($0, character) }) == true || after.map({ composes(character, $0) }) == true {
                    loose.append(character)
                    continue
                }
            }
            append(&runs, language: language, text: String(character), keys: keys)
        }
        if !unsupported.isEmpty { return .failure(.unsupported(unsupported)) }
        if !loose.isEmpty { return .failure(.looseJamo(loose)) }
        return .success(runs)
    }

    /// Two keys a probe types and then deletes: `하`, `gk` or `ㅎㅏ` says what the
    /// keyboard is in. Two deletes take either back, composing or not.
    public static let probeKeys: [PhoneKey] = [PhoneKey(code: KeyCode.g), PhoneKey(code: KeyCode.k)]

    /// What the probe's two keys write, by the state each shows.
    private static let probeSignatures: [(String, PhoneKeyboardState)] = [("ㅎㅏ", .looseJamo), ("하", .korean), ("gk", .english)]
    /// What is left of the probe after one delete of its two keys went missing:
    /// its first key, alone on a line (inside a word it says nothing).
    private static let probeFirstKey: Set<String> = ["g", "ㅎ"]

    /// How often `text` stands in the lines, read in lower case and without
    /// spaces (OCR loses and adds them; `g k` is the same two letters).
    private static func count(_ lines: [String], _ text: String) -> Int {
        lines.reduce(0) { $0 + squeezed($1).components(separatedBy: text).count - 1 }
    }

    private static func squeezed(_ line: String) -> String {
        line.lowercased().filter { !$0.isWhitespace }
    }

    /// The keyboard's state from the screen's words before and after the probe:
    /// the one signature that appeared, or `nil` when none or several did.
    /// English is read in any case: a field may capitalize its first letter, and
    /// `GK` (Caps Lock on) is no language change.
    public static func readProbe(before: [String], after: [String]) -> PhoneKeyboardState? {
        var seen: [PhoneKeyboardState] = []
        for (text, state) in probeSignatures where count(after, text) > count(before, text) {
            // `ㅎㅏ` holds no `하`, and `하` no `ㅎㅏ`: each is counted on its own.
            seen.append(state)
        }
        return seen.count == 1 ? seen[0] : nil
    }

    /// What the screen still shows of the probe after its deletes.
    public enum ProbeLeft: Equatable, Sendable {
        case none
        /// Its first key alone on a line: one delete went missing. Deletable only
        /// on the field's line; elsewhere it is a banner or a message.
        case firstKey(onFieldLine: Bool)
        /// Both keys, or what they write. Two deletes both missing is rare, and
        /// the same letters in a message or a late result read the same, so this
        /// is reported, never deleted.
        case both
    }

    /// The line the probe showed on: the first whose signature count grew from
    /// the line in the same place before it. `nil` when none did.
    public static func probeLine(before: [String], after: [String]) -> Int? {
        after.indices.first { index in
            let was = index < before.count ? [before[index]] : []
            return probeSignatures.contains { count([after[index]], $0.0) > count(was, $0.0) }
        }
    }

    /// What the probe left, by what can be seen, so a further delete never rests
    /// on a guess that could take the person's own characters: only a first key
    /// alone on `fieldLine`, the line the probe showed on, is the field's.
    public static func probeLeft(before: [String], after: [String], fieldLine: Int?) -> ProbeLeft {
        if probeSignatures.contains(where: { count(after, $0.0) > count(before, $0.0) }) {
            return .both
        }
        func alone(_ lines: [String]) -> Int {
            lines.filter { probeFirstKey.contains(squeezed($0)) }.count
        }
        guard alone(after) > alone(before) else { return .none }
        let onFieldLine = fieldLine.map { after.indices.contains($0) && probeFirstKey.contains(squeezed(after[$0])) } ?? false
        return .firstKey(onFieldLine: onFieldLine)
    }

    /// The two-key probe, typed and taken back again. The screen is what it knows
    /// by, and a phone is slow and a small field hard to read, so each step reads
    /// more than once, and what it did not read it says it did not read.
    public struct Probe {
        /// A phone's results come after its letters (Spotlight), so a screen
        /// unchanged at the first read says nothing yet: reads, a settle apart.
        public static let readsPerStep = 3

        public enum Failure: Error, Equatable {
            /// The screen never changed: no keys are taken back, since a field that
            /// takes no letters would lose the person's characters to them.
            case nothingShown
            /// The keys could not all be taken back, or the screen could not say
            /// they were: up to two may be left.
            case left(String)
        }

        public let read: () throws -> [String]
        public let type: ([PhoneKey]) throws -> Void
        /// One delete, with its pause: two at a key gap can land as one.
        public let deleteOne: () throws -> Void
        public let settle: () throws -> Void

        public init(read: @escaping () throws -> [String], type: @escaping ([PhoneKey]) throws -> Void, deleteOne: @escaping () throws -> Void, settle: @escaping () throws -> Void) {
            self.read = read
            self.type = type
            self.deleteOne = deleteOne
            self.settle = settle
        }

        /// What the keyboard is in, `nil` when the screen changed but did not say.
        /// `shown` is the screen before the keys, when just read. On any return
        /// the screen has been checked for what is left of them.
        public func run(shown: [String]? = nil) throws -> PhoneKeyboardState? {
            let before = try shown ?? read()
            try type(PhoneTyping.probeKeys)
            var after = before
            var state: PhoneKeyboardState?
            do {
                for _ in 0..<Self.readsPerStep where state == nil {
                    try settle()
                    after = try read()
                    state = PhoneTyping.readProbe(before: before, after: after)
                }
            } catch {
                // Unread, the keys may not be in the field at all (one that takes
                // no letters), and two deletes there would take the person's
                // characters: say they may be left instead.
                throw Failure.left("the screen could not be read after the keys were typed, so nothing was deleted (\(error))")
            }
            // Delete only after something showed: a field that takes no letters
            // (digits, a hidden or full one) shows nothing, and two deletes there
            // would take the person's characters.
            if after == before { throw Failure.nothingShown }
            let fieldLine = PhoneTyping.probeLine(before: before, after: after)
            try takeBack()
            try leftIfFailing { try settle() }
            try leftIfFailing { try clear(before: before, fieldLine: fieldLine) }
            return state
        }

        /// A read, a delete or a pause that fails while the keys are taken back
        /// says they may be left, not only its own error.
        private func leftIfFailing(_ step: () throws -> Void) throws {
            do {
                try step()
            } catch let failure as Failure {
                throw failure
            } catch {
                throw Failure.left("taking the keys back stopped: \(error)")
            }
        }

        private func takeBack() throws {
            do {
                for _ in PhoneTyping.probeKeys { try deleteOne() }
            } catch {
                throw Failure.left("the deletes did not go: \(error)")
            }
        }

        /// Reads until the screen no longer shows the keys. A delete that went
        /// missing shows as the first key alone on the field's line, and only
        /// that is deleted once more; anything else is reported, not deleted.
        private func clear(before: [String], fieldLine: Int?) throws {
            var left = PhoneTyping.ProbeLeft.none
            for attempt in 0..<Self.readsPerStep {
                if attempt > 0 { try settle() }
                left = PhoneTyping.probeLeft(before: before, after: try read(), fieldLine: fieldLine)
                if left == .none { return }
            }
            switch left {
            case .none:
                return
            case .both:
                throw Failure.left("after its two deletes the screen still shows the probe's letters, in the field or in something that came in meanwhile (a message, a banner, a result); nothing more was deleted")
            case .firstKey(onFieldLine: false):
                throw Failure.left("after its two deletes the screen shows its first key alone off the field's line; nothing more was deleted")
            case .firstKey(onFieldLine: true):
                try deleteOne()
                try settle()
                if PhoneTyping.probeLeft(before: before, after: try read(), fieldLine: fieldLine) != .none {
                    throw Failure.left("after one more delete the screen still shows the probe's first key")
                }
            }
        }
    }

    /// Whether the screen shows a hidden field: a line of nothing but dots,
    /// which is how a phone draws a password or a PIN.
    public static func showsHiddenText(_ lines: [String]) -> Bool {
        let dots: Set<Character> = ["•", "●", "∙", "·", "*", "⦁", "◦"]
        return lines.contains { line in
            let kept = line.filter { !$0.isWhitespace }
            return kept.count >= 2 && kept.allSatisfy { dots.contains($0) }
        }
    }

    /// Whether the text appears on the screen more often than before it was
    /// typed, read without spaces (OCR loses and adds them).
    public static func newlyShows(_ text: String, before: [String], after: [String]) -> Bool {
        let squeezed = text.filter { !$0.isWhitespace }
        guard !squeezed.isEmpty else { return false }
        func count(_ lines: [String]) -> Int {
            lines.reduce(0) { $0 + $1.filter { !$0.isWhitespace }.components(separatedBy: squeezed).count - 1 }
        }
        return count(after) > count(before)
    }

    /// The chords from what the keyboard is in to a language, as measured on a
    /// mirrored phone (2026-11-02): from English, Caps Lock gives Korean; from
    /// Korean, Control-Space gives English; from loose jamo, Caps Lock gives
    /// Korean. Caps Lock from Korean leaves loose jamo, and Control-Space from
    /// English does nothing, so neither is used.
    public static func route(from state: PhoneKeyboardState, to language: PhoneLanguage) -> [PhoneSwitch] {
        switch (state, language) {
        case (.korean, .korean), (.english, .english): return []
        case (.english, .korean), (.looseJamo, .korean): return [.capsLock]
        case (.korean, .english): return [.controlSpace]
        case (.looseJamo, .english): return [.capsLock, .controlSpace]
        }
    }

    /// The state a chord leaves, by the same measurements as `route`.
    public static func after(_ chord: PhoneSwitch, from state: PhoneKeyboardState) -> PhoneKeyboardState? {
        switch (chord, state) {
        case (.capsLock, .english), (.capsLock, .looseJamo): return .korean
        case (.controlSpace, .korean): return .english
        default: return nil
        }
    }

    // MARK: - Keys

    private static func append(_ runs: inout [PhoneRun], language: PhoneLanguage?, text: String, keys: [PhoneKey]) {
        if let last = runs.last, last.language == language || language == nil || last.language == nil {
            // A neutral piece rides with its neighbour, so no chord is spent on a space.
            let merged = last.language ?? language
            runs[runs.count - 1] = PhoneRun(language: merged, text: last.text + text, keys: last.keys + keys)
        } else {
            runs.append(PhoneRun(language: language, text: text, keys: keys))
        }
    }

    static func keysFor(_ character: Character) -> (PhoneLanguage?, [PhoneKey])? {
        if character == "₩" { return (.korean, [PhoneKey(code: KeyCode.grave)]) }
        if character == "`" { return (.english, [PhoneKey(code: KeyCode.grave)]) }
        if let scalar = character.unicodeScalars.first, character.unicodeScalars.count == 1 {
            if let key = ascii[scalar] {
                let letter = ("a"..."z").contains(String(character).lowercased()) && character.isLetter
                return (letter ? .english : nil, [key])
            }
            if let keys = hangul(scalar) { return (.korean, keys) }
        }
        return nil
    }

    private static func isSyllable(_ character: Character) -> Bool {
        guard let scalar = character.unicodeScalars.first, character.unicodeScalars.count == 1 else { return false }
        return (0xAC00...0xD7A3).contains(scalar.value)
    }

    /// Whether typing `second` right after `first` joins them on a 2-Set
    /// keyboard: a jamo beside a syllable, a consonant before a vowel, a pair
    /// that makes a final cluster (ㄱㅅ → ㄳ) or a compound vowel (ㅗㅏ → ㅘ).
    /// `ㅋㅋ` stays two letters.
    private static func composes(_ first: Character, _ second: Character) -> Bool {
        if isSyllable(first) || isSyllable(second) { return isCompatibilityJamo(first) || isCompatibilityJamo(second) }
        guard isCompatibilityJamo(first), isCompatibilityJamo(second) else { return false }
        let vowel: (Character) -> Bool = { $0.unicodeScalars.first!.value >= 0x314F }
        if !vowel(first) && vowel(second) { return true }
        // A compound jamo is two keys, and its first may join the jamo before it
        // (ㄹ then ㄳ types ㄺㅅ), its last the jamo after it.
        if compound.contains(first) || compound.contains(second) { return true }
        return joining.contains(String([first, second]))
    }

    private static let compound: Set<Character> = [
        "ㄳ", "ㄵ", "ㄶ", "ㄺ", "ㄻ", "ㄼ", "ㄽ", "ㄾ", "ㄿ", "ㅀ", "ㅄ", "ㅘ", "ㅙ", "ㅚ", "ㅝ", "ㅞ", "ㅟ", "ㅢ",
    ]

    private static let joining: Set<String> = [
        "ㄱㅅ", "ㄴㅈ", "ㄴㅎ", "ㄹㄱ", "ㄹㅁ", "ㄹㅂ", "ㄹㅅ", "ㄹㅌ", "ㄹㅍ", "ㄹㅎ", "ㅂㅅ",
        "ㅗㅏ", "ㅗㅐ", "ㅗㅣ", "ㅜㅓ", "ㅜㅔ", "ㅜㅣ", "ㅡㅣ",
    ]

    private static func isCompatibilityJamo(_ character: Character) -> Bool {
        guard let scalar = character.unicodeScalars.first, character.unicodeScalars.count == 1 else { return false }
        return (0x3131...0x3163).contains(scalar.value)
    }

    /// A syllable as its 2-Set keys, or a compatibility jamo as its own.
    private static func hangul(_ scalar: Unicode.Scalar) -> [PhoneKey]? {
        let value = scalar.value
        if (0xAC00...0xD7A3).contains(value) {
            let index = Int(value - 0xAC00)
            let lead = index / 588, vowel = (index % 588) / 28, tail = index % 28
            return twoSet(leads[lead]) + twoSet(vowels[vowel]) + twoSet(tails[tail])
        }
        if (0x3131...0x3163).contains(value), let keys = compatibility[value] {
            return twoSet(keys)
        }
        return nil
    }

    /// 2-Set letters: a capital is the shifted key (ㄲ is R).
    private static func twoSet(_ letters: String) -> [PhoneKey] {
        letters.map { letter in
            let lower = Character(letter.lowercased())
            return PhoneKey(code: letterCodes[lower]!, shift: letter.isUppercase)
        }
    }

    private static let leads = ["r", "R", "s", "e", "E", "f", "a", "q", "Q", "t", "T", "d", "w", "W", "c", "z", "x", "v", "g"]
    private static let vowels = ["k", "o", "i", "O", "j", "p", "u", "P", "h", "hk", "ho", "hl", "y", "n", "nj", "np", "nl", "b", "m", "ml", "l"]
    private static let tails = ["", "r", "R", "rt", "s", "sw", "sg", "e", "f", "fr", "fa", "fq", "ft", "fx", "fv", "fg", "a", "q", "qt", "t", "T", "d", "w", "c", "z", "x", "v", "g"]
    /// ㄱ (U+3131) to ㅣ (U+3163).
    private static let compatibility: [UInt32: String] = {
        let order = ["r", "R", "rt", "s", "sw", "sg", "e", "E", "f", "fr", "fa", "fq", "ft", "fx", "fv", "fg", "a", "q", "Q", "qt", "t", "T", "d", "w", "W", "c", "z", "x", "v", "g",
                     "k", "o", "i", "O", "j", "p", "u", "P", "h", "hk", "ho", "hl", "y", "n", "nj", "np", "nl", "b", "m", "ml", "l"]
        var table: [UInt32: String] = [:]
        for (offset, keys) in order.enumerated() { table[0x3131 + UInt32(offset)] = keys }
        return table
    }()

    enum KeyCode {
        static let g: UInt16 = 5
        static let k: UInt16 = 40
        static let grave: UInt16 = 50
    }

    private static let letterCodes: [Character: UInt16] = [
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9, "b": 11, "q": 12, "w": 13,
        "e": 14, "r": 15, "y": 16, "t": 17, "o": 31, "u": 32, "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46,
    ]

    /// Printable ASCII on a US keyboard, the layout both of the phone's languages share.
    private static let ascii: [Unicode.Scalar: PhoneKey] = {
        var table: [Unicode.Scalar: PhoneKey] = [:]
        for (letter, code) in letterCodes {
            table[letter.unicodeScalars.first!] = PhoneKey(code: code)
            table[Character(letter.uppercased()).unicodeScalars.first!] = PhoneKey(code: code, shift: true)
        }
        let plain: [(Character, UInt16)] = [
            ("1", 18), ("2", 19), ("3", 20), ("4", 21), ("5", 23), ("6", 22), ("7", 26), ("8", 28), ("9", 25), ("0", 29),
            ("-", 27), ("=", 24), ("[", 33), ("]", 30), ("\\", 42), (";", 41), ("'", 39), (",", 43), (".", 47), ("/", 44), (" ", 49),
        ]
        let shifted: [(Character, UInt16)] = [
            ("!", 18), ("@", 19), ("#", 20), ("$", 21), ("%", 23), ("^", 22), ("&", 26), ("*", 28), ("(", 25), (")", 29),
            ("_", 27), ("+", 24), ("{", 33), ("}", 30), ("|", 42), (":", 41), ("\"", 39), ("<", 43), (">", 47), ("?", 44), ("~", 50),
        ]
        for (character, code) in plain { table[character.unicodeScalars.first!] = PhoneKey(code: code) }
        for (character, code) in shifted { table[character.unicodeScalars.first!] = PhoneKey(code: code, shift: true) }
        return table
    }()
}
