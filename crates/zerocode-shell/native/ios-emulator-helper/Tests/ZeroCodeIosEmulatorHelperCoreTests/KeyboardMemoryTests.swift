import XCTest

@testable import ZeroCodeIosEmulatorHelperCore

/// A remembered keyboard language is trusted only for a while, and a text typed on it is looked for
/// in the field before it is called done.
final class KeyboardMemoryTests: XCTestCase {
    /// A reading is the language until it ages past `keep`, or the device boots again; forgetting
    /// drops it at once.
    func testAReadingIsTrustedUntilItAgesTheDeviceBootsOrItIsForgotten() {
        let keep = KeyboardMemory<String>.keep
        var memory = KeyboardMemory<String>()
        XCTAssertNil(memory.language(at: 10, boot: 1))
        memory.remember("english", at: 10, boot: 1)
        XCTAssertEqual(memory.language(at: 10, boot: 1), "english")
        XCTAssertEqual(memory.language(at: 10 + keep, boot: 1), "english")
        XCTAssertNil(memory.language(at: 10 + keep + 0.1, boot: 1))
        // Booted again since: its keyboard is whatever its settings say.
        XCTAssertNil(memory.language(at: 11, boot: 2))
        XCTAssertNil(memory.language(at: 11, boot: nil))
        // A clock that went back trusts nothing.
        XCTAssertNil(memory.language(at: 5, boot: 1))
        // A device that does not say its boot is judged by the age alone.
        memory.remember("korean", at: 20, boot: nil)
        XCTAssertEqual(memory.language(at: 21, boot: nil), "korean")
        memory.forget()
        XCTAssertNil(memory.language(at: 21, boot: nil))
    }

    private final class Clock {
        var now = 0.0
        var reads = 0
    }

    private func check(_ screens: [[String]], readTakes: Double = 0.6) -> (TypedCheck, Clock) {
        let clock = Clock()
        let answer = TypedCheck.wait(
            now: { clock.now },
            sleep: { clock.now += $0 },
            read: {
                clock.now += readTakes
                defer { clock.reads += 1 }
                return screens[min(clock.reads, screens.count - 1)]
            },
            shows: { TypedCheck.shows("lantern", in: $0) }
        )
        return (answer, clock)
    }

    /// A native field holds the text at the first read: one read, no wait.
    func testANativeFieldShowsTheTextAtTheFirstRead() {
        let (answer, clock) = check([["lantern"]])
        XCTAssertEqual(answer, .shown)
        XCTAssertEqual(clock.reads, 1)
    }

    /// A web page's field tells the tree late: the text is waited on, by the clock.
    func testAPageFieldIsWaitedOnUntilItShowsTheText() {
        let (answer, clock) = check([["Search shops"], ["Search shops"], ["lantern"]])
        XCTAssertEqual(answer, .shown)
        XCTAssertEqual(clock.reads, 3)
    }

    /// Text the field never shows — another language's letters — is not called done.
    func testLettersOfAnotherLanguageAreNotShown() {
        let (answer, clock) = check([["ㅣ뭇ㄷ구"]])
        XCTAssertEqual(answer, .notShown)
        XCTAssertGreaterThanOrEqual(clock.now, ProbeWait.within)
        XCTAssertLessThan(clock.now, ProbeWait.within + 0.6 + ProbeWait.gap + 0.01)
    }

    /// The text is found case aside and without spaces; another language's letters are not it.
    func testTheTextIsFoundCaseAsideAndWithoutSpaces() {
        XCTAssertTrue(TypedCheck.shows("lantern", in: ["Search shops", "Lantern"]))
        XCTAssertTrue(TypedCheck.shows("우유 2리터", in: ["우유2리터 사기"]))
        XCTAssertFalse(TypedCheck.shows("lantern", in: ["ㅣ뭇ㄷ구"]))
        XCTAssertFalse(TypedCheck.shows(" ", in: ["anything"]))
    }
}
