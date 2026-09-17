/// The window's keyboard table (the core's `keyboard_guard`): how long a
/// field is given to show typed keys, how often it is read meanwhile, and the
/// longest value worth reading back. The helper keeps no numbers of its own.
public struct KeyboardGuardConfig: Equatable, Sendable {
    public let settleMs: Int
    public let pollMs: Int
    public let maxChars: Int

    /// Nil unless every number is there and makes sense.
    public init?(settleMs: Double?, pollMs: Double?, maxChars: Double?) {
        guard let settleMs, settleMs > 0,
              let pollMs, pollMs > 0, pollMs <= settleMs,
              let maxChars, maxChars >= 1
        else {
            return nil
        }
        self.settleMs = Int(settleMs)
        self.pollMs = Int(pollMs)
        self.maxChars = Int(maxChars)
    }
}

public enum KeyboardInputSafety {
    /// What the helper does with keys it is about to post — the shared Rust
    /// core's `validate::secret_entry`, run against the same case table
    /// (`computer_use_protocol/cases/secret_entry.tsv`).
    public enum SecretEntryVerdict: String, Equatable {
        case clear
        case watched
        case personsEntry = "persons_entry"
    }

    /// Keys never write a secret: text or a paste into the platform's secret
    /// field is the person's to type — and so is text while the keyboard's
    /// secure mode is held by the app the keys go to, or held while the focus
    /// cannot be read; keys that carry no text are never refused; while
    /// another process holds secure mode, keys are posted and watched.
    public static func secretEntry(
        writesText: Bool,
        pastes: Bool,
        secureField: Bool,
        secureInputOn: Bool,
        secureInputByReceiver: Bool,
        focusRead: Bool
    ) -> SecretEntryVerdict {
        let secret = secureField || (secureInputOn && (secureInputByReceiver || !focusRead))
        if (writesText || pastes) && secret {
            return .personsEntry
        }
        return secureInputOn ? .watched : .clear
    }

    /// What one key chord writes into the focused field — the core's
    /// `keys::chord_writes`, run against the same case table
    /// (`cases/key_writes.tsv`, its macOS rows).
    public enum ChordWrites: String, Equatable {
        case nothing
        case character
        case clipboard
    }

    /// The modifier names as the core's `Modifier::parse` reads them, by what
    /// they are on macOS: ⌘ (the primary one), the ones a character is typed
    /// with, and the rest. A source contract holds them to the core's names.
    public static let primaryModifierNames: Set<String> = ["cmd", "command", "meta", "super", "win", "cmdorctrl", "commandorcontrol"]
    public static let textModifierNames: Set<String> = ["shift", "alt", "option"]
    public static let otherModifierNames: Set<String> = ["ctrl", "control"]
    /// The letter the paste chord presses with ⌘.
    public static let pasteKey = "v"

    /// A chord parsed exactly as the helper posts it (split on `+`,
    /// lowercased, the last non-modifier the key): the paste chord writes the
    /// clipboard, a printable key held with nothing but text modifiers a
    /// character, anything else nothing.
    public static func chordWrites(_ spec: String) -> ChordWrites {
        var modifiers: [String] = []
        var key: String?
        for part in spec.split(separator: "+", omittingEmptySubsequences: false).map({ String($0).lowercased() }) {
            if primaryModifierNames.contains(part) || textModifierNames.contains(part) || otherModifierNames.contains(part) {
                modifiers.append(part)
            } else {
                key = part
            }
        }
        guard let key else { return .nothing }
        if key == pasteKey && modifiers.contains(where: { primaryModifierNames.contains($0) }) {
            return .clipboard
        }
        let printable = key == "space" || key.unicodeScalars.count == 1
        return printable && modifiers.allSatisfy({ textModifierNames.contains($0) }) ? .character : .nothing
    }

    /// Why a text is not typed as one — the core's `TextEntryRefusal`.
    public enum TextEntryRefusal: String, Error, Equatable {
        case lineBreak = "line_break"
    }

    /// The characters of the focus-moving ones that press: a Return or a line
    /// break in a field that is not a multi-line text area (the core's
    /// `PRESSING_CHARS`).
    public static let pressingScalars: [Unicode.Scalar] = ["\r", "\n"]

    /// How a text is typed — the core's `text_entry_plan`, against
    /// `cases/text_entry.tsv`: into a multi-line area as one piece; anywhere
    /// else refused when it holds a line break, else cut after each Tab.
    public static func textEntryPlan(_ text: String, multiLine: Bool) -> Result<[String], TextEntryRefusal> {
        if multiLine {
            return .success(text.isEmpty ? [] : [text])
        }
        if text.unicodeScalars.contains(where: { pressingScalars.contains($0) }) {
            return .failure(.lineBreak)
        }
        return .success(focusMovingPieces(text))
    }

    /// The characters that can carry typed keys into another field: text is
    /// typed in pieces cut after each (the core's `FOCUS_MOVING_CHARS`) —
    /// scalars, since a CRLF is one Swift Character but two keys.
    public static let focusMovingScalars: [Unicode.Scalar] = ["\t", "\r", "\n"]

    /// Whether typed text can carry keys out of the field it starts in.
    public static func movesTheFocus(_ text: String) -> Bool {
        text.unicodeScalars.contains { focusMovingScalars.contains($0) }
    }

    /// Text cut after every focus-moving character, each piece keeping it.
    public static func focusMovingPieces(_ text: String) -> [String] {
        var pieces: [String] = []
        var current = String.UnicodeScalarView()
        for scalar in text.unicodeScalars {
            current.append(scalar)
            if focusMovingScalars.contains(scalar) {
                pieces.append(String(current))
                current = String.UnicodeScalarView()
            }
        }
        if !current.isEmpty {
            pieces.append(String(current))
        }
        return pieces
    }

    public enum FocusFailure: Equatable {
        case targetNotFocused
        case targetNotFocusedAfterRestore
    }

    public static func syntheticInputFocusFailure(targetWindowFocused: Bool, restoreWindowRequested: Bool) -> FocusFailure? {
        guard !targetWindowFocused else {
            return nil
        }
        return restoreWindowRequested ? .targetNotFocusedAfterRestore : .targetNotFocused
    }
}
