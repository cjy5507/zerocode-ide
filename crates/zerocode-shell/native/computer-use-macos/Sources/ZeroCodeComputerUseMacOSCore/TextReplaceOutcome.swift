/// What an Accessibility text replacement actually did, judged from the two
/// facts the caller has: whether the `AXValue` set call succeeded, and what the
/// field holds afterwards.
///
/// The judgement exists because a field is allowed to consume what it is
/// given. A terminal's key sink drains every insert to the pty and clears
/// itself; an autocomplete rewrites; a formatter re-spaces. Reading such a
/// field back after a successful set does not return the text — and treating
/// that as "the accessibility road was unavailable" made the caller type the
/// same text again through synthetic keystrokes. 2026-09-10, a ZeroCode
/// terminal pane: `type-text "echo abc"` landed as `echo abcecho abc`, and
/// `paste-text` fell through to a clipboard paste the same way.
///
/// The Windows provider already judged this through the shared Rust core
/// (`computer_use_protocol::text_field::replace_selection`): a write that
/// succeeded is `Applied` whatever the readback says, with `value_mismatch`
/// or `readback_unsupported` as the unverified reason. This is that judgement
/// in Swift, same vocabulary, so both platforms report one way.
public enum TextReplaceOutcome: Equatable {
    /// The value could not be written through Accessibility (not settable, no
    /// value to read, or the set call failed). Synthetic keystrokes are the
    /// honest fallback: nothing has landed yet.
    case unavailable
    /// Written, and the field reads back exactly the composed value.
    case verified
    /// Written — the set call succeeded — but the field now holds something
    /// else (`readback`). The text landed once and the field did something
    /// with it; typing it again would be a second copy.
    case valueMismatch(readback: String)
    /// Written, but the field could not be read back at all. The text landed;
    /// only the confirmation is missing.
    case readbackUnsupported
    /// Typed, and after the settle the field still reads what it read before:
    /// the keys may not have reached it. Look before typing again.
    case unchanged

    /// `setSucceeded` is the `AXUIElementSetAttributeValue` result; `expected`
    /// is the whole composed value the caller wrote; `readback` is the value
    /// read after the set (`nil` when the read itself failed).
    public static func judge(setSucceeded: Bool, expected: String, readback: String?) -> TextReplaceOutcome {
        guard setSucceeded else { return .unavailable }
        guard let readback else { return .readbackUnsupported }
        return readback == expected ? .verified : .valueMismatch(readback: readback)
    }

    /// Keys typed into a field, read back after the settle — the shared Rust
    /// core's `text_field::judge_typed`, run against the same case table
    /// (`computer_use_protocol/cases/typed_landing.tsv`). A field that held
    /// nothing and holds nothing again consumed the keys or never got them (a
    /// terminal's key sink clears itself): that is not called unchanged.
    public static func judgeTyped(before: String, expected: String, after: String?) -> TextReplaceOutcome {
        guard let after else { return .readbackUnsupported }
        if after == expected { return .verified }
        if after == before { return before.isEmpty ? .readbackUnsupported : .unchanged }
        return .valueMismatch(readback: after)
    }

    /// Whether the caller may still fall back to typing the text: only when
    /// nothing landed.
    public var allowsSyntheticFallback: Bool {
        self == .unavailable
    }

    /// The unverified reason the action report carries — the shared core's
    /// spelling (`value_mismatch`, `readback_unsupported`); `nil` when the
    /// outcome is verified or nothing landed.
    public var unverifiedReason: String? {
        switch self {
        case .unavailable, .verified: return nil
        case .valueMismatch: return "value_mismatch"
        case .readbackUnsupported: return "readback_unsupported"
        case .unchanged: return "value_unchanged"
        }
    }
}
