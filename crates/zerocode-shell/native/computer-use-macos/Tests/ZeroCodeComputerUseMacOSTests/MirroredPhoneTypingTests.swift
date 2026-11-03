import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class MirroredPhoneTypingTests: XCTestCase {
    private let names: [UInt16: String] = [
        0: "a", 1: "s", 2: "d", 3: "f", 4: "h", 5: "g", 6: "z", 7: "x", 8: "c", 9: "v", 11: "b", 12: "q", 13: "w",
        14: "e", 15: "r", 16: "y", 17: "t", 31: "o", 32: "u", 34: "i", 35: "p", 37: "l", 38: "j", 40: "k", 45: "n", 46: "m",
        18: "1", 19: "2", 20: "3", 21: "4", 23: "5", 22: "6", 26: "7", 28: "8", 25: "9", 29: "0",
        27: "-", 24: "=", 33: "[", 30: "]", 42: "\\", 41: ";", 39: "'", 43: ",", 47: ".", 44: "/", 49: " ", 50: "`",
    ]

    /// The keys as a person would read them off the keyboard: a capital is shift.
    private func spelled(_ runs: [PhoneRun]) -> String {
        runs.flatMap(\.keys).map { key in
            let name = names[key.code] ?? "?"
            return key.shift ? "⇧" + name : name
        }.joined()
    }

    private func plan(_ text: String) -> [PhoneRun] {
        guard case let .success(runs) = PhoneTyping.plan(text) else {
            XCTFail("\(text) was refused")
            return []
        }
        return runs
    }

    func testKoreanIsTypedAsItsTwoSetKeys() {
        // The same keys put these three lines on a phone and a simulator (2026-11-02).
        XCTAssertEqual(spelled(plan("안녕하세요 시험 37")), "dkssudgktpdy tlgja 37")
        XCTAssertEqual(spelled(plan("있었다 까치 쌍자음")), "dl⇧tdj⇧tek ⇧rkcl ⇧tkdwkdma")
        XCTAssertEqual(spelled(plan("값 앉아 많다, 1.5?")), "rkqt dkswdk aksgek, 1.5⇧/")
        XCTAssertEqual(plan("안녕하세요 시험 37").map(\.language), [.korean])
    }

    func testEnglishAndSymbolsAreTypedOnTheUSKeys() {
        XCTAssertEqual(spelled(plan("Zulu Kite: 37/52")), "⇧zulu ⇧kite⇧; 37/52")
        XCTAssertEqual(spelled(plan("example.test/path?q=4")), "example.test/path⇧/q=4")
        XCTAssertEqual(plan("Zulu Kite: 37/52").map(\.language), [.english])
    }

    func testTheLanguageChangesOnlyWhereALetterNeedsIt() {
        let runs = plan("오늘 Zulu 37 완료")
        XCTAssertEqual(runs.map(\.language), [.korean, .english, .korean])
        XCTAssertEqual(runs.map(\.text), ["오늘 ", "Zulu 37 ", "완료"])
        XCTAssertEqual(plan("1.5 + 2").map(\.language), [nil], "digits need no language")
    }

    func testTheGraveKeyWritesWonInKoreanAndABacktickInEnglish() {
        XCTAssertEqual(plan("₩1,000").map(\.language), [.korean])
        XCTAssertEqual(plan("`x`").map(\.language), [.english])
        XCTAssertEqual(spelled(plan("~")), "⇧`", "a tilde is the same in both")
        XCTAssertEqual(plan("~").map(\.language), [nil])
    }

    func testWhatNoKeyWritesIsRefusedBeforeAnythingIsPlanned() {
        XCTAssertEqual(PhoneTyping.plan("hi 🙂"), .failure(.unsupported("🙂")))
        XCTAssertEqual(PhoneTyping.plan("こんにちは"), .failure(.unsupported("こんにちは")))
        XCTAssertEqual(PhoneTyping.plan("a\nb"), .failure(.lineBreak))
        // A lone jamo beside a syllable composes into it on the phone.
        XCTAssertEqual(PhoneTyping.plan("가ㄴ"), .failure(.looseJamo("ㄴ")))
        XCTAssertEqual(spelled(plan("ㅋㅋ ㅎ")), "zz g")
        XCTAssertEqual(PhoneTyping.plan("ㄱㅏ"), .failure(.looseJamo("ㄱㅏ")), "a consonant before a vowel is 가")
        XCTAssertEqual(PhoneTyping.plan("ㅗㅏ"), .failure(.looseJamo("ㅗㅏ")), "two vowels that are ㅘ")
        XCTAssertEqual(spelled(plan("ㅠㅠ")), "bb")
        XCTAssertEqual(PhoneTyping.plan("ㄹㄳ"), .failure(.looseJamo("ㄹㄳ")), "ㄹ then ㄳ types ㄺㅅ")
        XCTAssertEqual(spelled(plan("ㄳ")), "rt", "alone it is its two keys")
    }

    func testAProbeReadsWhichSignatureAppeared() {
        let before = ["메모", "있었다"]
        XCTAssertEqual(PhoneTyping.readProbe(before: before, after: before + ["하"]), .korean)
        XCTAssertEqual(PhoneTyping.readProbe(before: before, after: ["메모", "있었다gk"]), .english)
        XCTAssertEqual(PhoneTyping.readProbe(before: before, after: before + ["ㅎㅏ"]), .looseJamo)
        XCTAssertNil(PhoneTyping.readProbe(before: before, after: before), "nothing new is no answer")
        XCTAssertNil(PhoneTyping.readProbe(before: before, after: before + ["하", "gk"]), "two answers are none")
        XCTAssertEqual(PhoneTyping.readProbe(before: ["하하"], after: ["하하하"]), .korean, "counted, not just found")
        XCTAssertEqual(PhoneTyping.readProbe(before: [], after: ["Gk"]), .english, "a capital first letter is still English")
    }

    func testAHiddenFieldIsSeenByItsDots() {
        XCTAssertTrue(PhoneTyping.showsHiddenText(["로그인", "••••••"]))
        XCTAssertTrue(PhoneTyping.showsHiddenText(["● ● ● ●"]))
        XCTAssertFalse(PhoneTyping.showsHiddenText(["로그인", "• 항목 하나"]), "a bullet before words is a list")
        XCTAssertFalse(PhoneTyping.showsHiddenText(["•"]), "one dot is not a field")
    }

    func testTheTextCountsAsTypedOnlyWhenItAppearsAgain() {
        XCTAssertTrue(PhoneTyping.newlyShows("안녕 37", before: ["메모"], after: ["메모", "안녕37"]))
        XCTAssertFalse(PhoneTyping.newlyShows("메모", before: ["메모"], after: ["메모"]), "already there is not typed")
        XCTAssertFalse(PhoneTyping.newlyShows("  ", before: [], after: ["x"]), "spaces alone prove nothing")
    }

    func testEveryPlannedKeyHasTheUsageASimulatorIsSent() {
        var text = ""
        for value in 0x20...0x7E { text.append(Character(Unicode.Scalar(value)!)) }
        text += "가각갂갃간갅갆갇갈갉갊갋갌갍갎갏감갑값갓갔강갖갗갘같갚갛 꽈뙈쐬뚸뛔뀌의 ㅋㅋ ₩"
        for character in text {
            guard case let .success(runs) = PhoneTyping.plan(String(character)) else {
                XCTFail("\(character) was refused")
                continue
            }
            for key in runs.flatMap(\.keys) {
                XCTAssertNotNil(key.usage, "\(character): key code \(key.code)")
            }
        }
        XCTAssertEqual(PhoneKey(code: 0).usage, 0x04, "the A key")
        XCTAssertEqual(PhoneKey(code: 49).usage, 0x2C, "space")
    }

    func testTheChordsAreTheOnesMeasuredToWorkOnAMirroredPhone() {
        XCTAssertEqual(PhoneTyping.route(from: .english, to: .korean), [.capsLock])
        XCTAssertEqual(PhoneTyping.route(from: .korean, to: .english), [.controlSpace])
        XCTAssertEqual(PhoneTyping.route(from: .looseJamo, to: .korean), [.capsLock])
        XCTAssertEqual(PhoneTyping.route(from: .looseJamo, to: .english), [.capsLock, .controlSpace])
        XCTAssertEqual(PhoneTyping.route(from: .korean, to: .korean), [])
        for state in [PhoneKeyboardState.english, .korean, .looseJamo] {
            for language in [PhoneLanguage.english, .korean] {
                var now: PhoneKeyboardState? = state
                for chord in PhoneTyping.route(from: state, to: language) {
                    now = now.flatMap { PhoneTyping.after(chord, from: $0) }
                }
                let reached: PhoneKeyboardState = language == .korean ? .korean : .english
                XCTAssertEqual(now, reached, "\(state) to \(language)")
            }
        }
        // Neither chord that does not reliably go back is ever planned.
        XCTAssertNil(PhoneTyping.after(.capsLock, from: .korean))
        XCTAssertNil(PhoneTyping.after(.controlSpace, from: .english))
    }

    // MARK: - The probe, on a phone made of lines

    /// A field on a phone: what it holds, what the screen reads of it, and the
    /// results that come after it.
    private final class FakePhone {
        var text: String
        let takesLetters: Bool
        /// Reads of the field: nothing where it is too small to read.
        var fieldReadable = true
        /// How many settles the results come behind the field.
        var resultsLag = 0
        /// The deletes that never reach the phone.
        var droppedDeletes = 0
        var unreadable = false
        var failingDelete = false
        var keysTyped: [PhoneKey] = []
        var deletes = 0
        private var settles = 0
        private var changedAt = 0
        let write: String

        init(text: String = "", takesLetters: Bool = true, write: String = "gk") {
            self.text = text
            self.takesLetters = takesLetters
            self.write = write
        }

        var lines: [String] {
            var shown = ["Search"]
            if fieldReadable && !text.isEmpty { shown.append(text) }
            if !text.isEmpty && takesLetters && settles >= changedAt + resultsLag { shown.append("Results: \(text.count)") }
            return shown
        }

        func hand() -> PhoneTyping.Probe {
            PhoneTyping.Probe(
                read: { [unowned self] in
                    if self.unreadable { throw NSError(domain: "read", code: 1) }
                    return self.lines
                },
                type: { [unowned self] keys in
                    self.keysTyped += keys
                    if self.takesLetters {
                        self.text += self.write
                        self.changedAt = self.settles
                    }
                },
                deleteOne: { [unowned self] in
                    if self.failingDelete { throw NSError(domain: "delete", code: 1) }
                    self.deletes += 1
                    if self.droppedDeletes > 0 {
                        self.droppedDeletes -= 1
                        return
                    }
                    guard let last = self.text.last else { return }
                    // Backspace takes a jamo at a time while `하` composes.
                    if last == "하" {
                        self.text.removeLast()
                        self.text.append("ㅎ")
                    } else {
                        self.text.removeLast()
                    }
                    self.changedAt = self.settles
                },
                settle: { [unowned self] in self.settles += 1 }
            )
        }
    }

    func testAProbeReadsEachLanguageAndLeavesTheFieldAsItWas() throws {
        for (write, state) in [("gk", PhoneKeyboardState.english), ("하", .korean), ("ㅎㅏ", .looseJamo)] {
            let phone = FakePhone(text: "abc", write: write)
            XCTAssertEqual(try phone.hand().run(), state, write)
            XCTAssertEqual(phone.text, "abc", write)
            XCTAssertEqual(phone.deletes, 2, write)
        }
    }

    func testAProbeInAFieldTooSmallToReadStillTakesItsKeysBack() throws {
        let phone = FakePhone()
        phone.fieldReadable = false
        XCTAssertNil(try phone.hand().run(), "changed results say nothing of the language")
        XCTAssertEqual(phone.text, "", "no letters stay behind")
        XCTAssertEqual(phone.deletes, 2)
    }

    /// The field drew its letters where OCR cannot read them and its results came
    /// two settles on: one read would have said "nothing showed" and left `gk`.
    func testResultsThatComeLateStillCountAsTheFieldTakingTheKeys() throws {
        let phone = FakePhone()
        phone.fieldReadable = false
        phone.resultsLag = 2
        XCTAssertNil(try phone.hand().run())
        XCTAssertEqual(phone.text, "")
    }

    func testADeleteThatWentMissingIsDeletedAgainWhenTheScreenShowsIt() throws {
        let phone = FakePhone()
        phone.droppedDeletes = 1
        XCTAssertEqual(try phone.hand().run(), .english)
        XCTAssertEqual(phone.text, "", "the `g` the real phone showed once")
        XCTAssertEqual(phone.deletes, 3)
    }

    func testAKeyStillShownAfterTheExtraDeletesIsReported() {
        let phone = FakePhone()
        phone.droppedDeletes = 3
        XCTAssertThrowsError(try phone.hand().run()) { error in
            guard case PhoneTyping.Probe.Failure.left = error else { return XCTFail("\(error)") }
        }
    }

    /// The other side of the same edge: nothing showed, so nothing is deleted,
    /// and the characters the person has there stay.
    func testAFieldThatTakesNoLettersKeepsThePersonsCharacters() {
        let phone = FakePhone(text: "1234", takesLetters: false)
        XCTAssertThrowsError(try phone.hand().run()) { error in
            XCTAssertEqual(error as? PhoneTyping.Probe.Failure, .nothingShown)
        }
        XCTAssertEqual(phone.text, "1234")
        XCTAssertEqual(phone.deletes, 0)
    }

    func testNoExtraDeleteRestsOnAGuess() throws {
        let phone = FakePhone(text: "garden")
        phone.fieldReadable = false
        XCTAssertNil(try phone.hand().run())
        XCTAssertEqual(phone.deletes, 2, "the screen shows no key left: nothing more goes")
        XCTAssertEqual(phone.text, "garden")
    }

    func testAProbeThatCannotReadStillTakesItsKeysBack() {
        let phone = FakePhone()
        let hand = phone.hand()
        let shown = phone.lines
        phone.unreadable = true
        XCTAssertThrowsError(try hand.run(shown: shown))
        XCTAssertEqual(phone.text, "", "the keys went in, whatever the screen says")
        XCTAssertEqual(phone.deletes, 2)
    }

    func testAProbeThatCanNeitherReadNorDeleteSaysSo() {
        let phone = FakePhone()
        let hand = phone.hand()
        let shown = phone.lines
        phone.unreadable = true
        phone.failingDelete = true
        XCTAssertThrowsError(try hand.run(shown: shown)) { error in
            guard case PhoneTyping.Probe.Failure.left = error else { return XCTFail("\(error)") }
        }
    }

    func testAProbeReadWithSpacesInItIsTheSameProbe() {
        XCTAssertEqual(PhoneTyping.readProbe(before: [], after: ["g k"]), .english)
        XCTAssertEqual(PhoneTyping.readProbe(before: [], after: ["ㅎ ㅏ"]), .looseJamo)
    }

    func testOnlyAKeyAloneOnALineCountsAsLeft() {
        XCTAssertEqual(PhoneTyping.probeLeft(before: [], after: ["g"]), 1)
        XCTAssertEqual(PhoneTyping.probeLeft(before: [], after: ["ㅎ"]), 1)
        XCTAssertEqual(PhoneTyping.probeLeft(before: [], after: ["gk"]), 2)
        XCTAssertEqual(PhoneTyping.probeLeft(before: ["g"], after: ["g"]), 0, "it stood there before")
        XCTAssertEqual(PhoneTyping.probeLeft(before: [], after: ["garden"]), 0)
    }
}
