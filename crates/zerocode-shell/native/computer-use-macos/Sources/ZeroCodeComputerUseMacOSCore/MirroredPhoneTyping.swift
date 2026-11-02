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

    /// The keyboard's state from the screen's words before and after the probe:
    /// the one signature that appeared, or `nil` when none or several did.
    public static func readProbe(before: [String], after: [String]) -> PhoneKeyboardState? {
        let signatures: [(String, PhoneKeyboardState)] = [("ㅎㅏ", .looseJamo), ("하", .korean), ("gk", .english)]
        func count(_ lines: [String], _ text: String) -> Int {
            lines.reduce(0) { $0 + $1.components(separatedBy: text).count - 1 }
        }
        var seen: [PhoneKeyboardState] = []
        for (text, state) in signatures where count(after, text) > count(before, text) {
            // `ㅎㅏ` holds no `하`, and `하` no `ㅎㅏ`: each is counted on its own.
            seen.append(state)
        }
        return seen.count == 1 ? seen[0] : nil
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
        return joining.contains(String([first, second]))
    }

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
