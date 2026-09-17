import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class KeyboardInputSafetyTests: XCTestCase {
    func testSyntheticInputRequiresFocusedTargetWindow() {
        let cases: [(focused: Bool, restoreWindow: Bool, expectedFailure: KeyboardInputSafety.FocusFailure?)] = [
            (focused: true, restoreWindow: false, expectedFailure: nil),
            (focused: true, restoreWindow: true, expectedFailure: nil),
            (focused: false, restoreWindow: false, expectedFailure: .targetNotFocused),
            (focused: false, restoreWindow: true, expectedFailure: .targetNotFocusedAfterRestore),
        ]

        for testCase in cases {
            XCTAssertEqual(
                KeyboardInputSafety.syntheticInputFocusFailure(
                    targetWindowFocused: testCase.focused,
                    restoreWindowRequested: testCase.restoreWindow
                ),
                testCase.expectedFailure
            )
        }
    }

    /// The shared Rust core's case table, read from the repository: one table,
    /// two implementations, both held to it.
    func testKeysNeverWriteASecretByTheSharedCaseTable() throws {
        let rows = try SharedCaseTable.rows("secret_entry.tsv")
        XCTAssertGreaterThanOrEqual(rows.count, 16)
        for row in rows {
            let flag = { (word: String) -> Bool in word == "true" }
            let verdict = KeyboardInputSafety.secretEntry(
                writesText: flag(row[0]),
                pastes: flag(row[1]),
                secureField: flag(row[2]),
                secureInputOn: flag(row[3]),
                secureInputByReceiver: flag(row[4]),
                focusRead: flag(row[5])
            )
            XCTAssertEqual(verdict.rawValue, row[6], "\(row)")
        }
    }

    /// What a chord writes, by the core's table (its macOS rows).
    func testWhatAChordWritesIsReadOffTheSharedCaseTable() throws {
        let rows = try SharedCaseTable.rows("key_writes.tsv").filter { $0[1] == "macos" }
        XCTAssertGreaterThanOrEqual(rows.count, 20)
        for row in rows {
            XCTAssertEqual(KeyboardInputSafety.chordWrites(row[0]).rawValue, row[2], "\(row)")
        }
    }

    /// How a text is typed, by the core's table.
    func testATextIsTypedByTheSharedPlan() throws {
        let rows = try SharedCaseTable.rows("text_entry.tsv")
        XCTAssertGreaterThanOrEqual(rows.count, 8)
        for row in rows {
            let planned: String
            switch KeyboardInputSafety.textEntryPlan(SharedCaseTable.unescape(row[0]), multiLine: row[1] == "true") {
            case let .success(pieces):
                planned = pieces.map(SharedCaseTable.escape).joined(separator: "|")
            case let .failure(refusal):
                planned = "refused:\(refusal.rawValue)"
            }
            XCTAssertEqual(planned, row[2], "\(row)")
        }
    }

    func testTheKeyboardTableIsTheWindowsOrNothing() {
        XCTAssertEqual(KeyboardGuardConfig(settleMs: 350, pollMs: 50, maxChars: 32768)?.pollMs, 50)
        XCTAssertNil(KeyboardGuardConfig(settleMs: 350, pollMs: nil, maxChars: 1))
        XCTAssertNil(KeyboardGuardConfig(settleMs: 50, pollMs: 350, maxChars: 1), "a poll inside the settle")
        XCTAssertNil(KeyboardGuardConfig(settleMs: 350, pollMs: 50, maxChars: 0))
    }

    func testTypedTextIsCutAfterEveryFocusMovingCharacter() {
        XCTAssertEqual(KeyboardInputSafety.focusMovingPieces("alice\thunter2"), ["alice\t", "hunter2"])
        XCTAssertEqual(KeyboardInputSafety.focusMovingPieces("a\r\nb\n"), ["a\r", "\n", "b\n"])
        XCTAssertEqual(KeyboardInputSafety.focusMovingPieces("plain"), ["plain"])
        XCTAssertEqual(KeyboardInputSafety.focusMovingPieces(""), [])
        XCTAssertTrue(KeyboardInputSafety.movesTheFocus("a\r\n") && !KeyboardInputSafety.movesTheFocus("plain"))
    }
}

/// A case table the shared Rust core keeps under
/// `crates/zerocode-core/src/computer_use_protocol/cases/`: tab-separated
/// rows, `#` lines are comments.
enum SharedCaseTable {
    static func rows(_ name: String) throws -> [[String]] {
        let crates = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()  // ZeroCodeComputerUseMacOSTests
            .deletingLastPathComponent()  // Tests
            .deletingLastPathComponent()  // computer-use-macos
            .deletingLastPathComponent()  // native
            .deletingLastPathComponent()  // zerocode-shell
            .deletingLastPathComponent()  // crates
        let file = crates.appendingPathComponent("zerocode-core/src/computer_use_protocol/cases/\(name)")
        return try String(contentsOf: file, encoding: .utf8)
            .split(separator: "\n", omittingEmptySubsequences: true)
            .filter { !$0.hasPrefix("#") }
            .map { $0.split(separator: "\t", omittingEmptySubsequences: false).map(String.init) }
    }

    /// A text cell: `\t`, `\r`, `\n` and `\\` spelled out, so a row is one line.
    static func unescape(_ cell: String) -> String {
        var out = ""
        var scalars = cell.unicodeScalars.makeIterator()
        while let scalar = scalars.next() {
            guard scalar == "\\" else {
                out.unicodeScalars.append(scalar)
                continue
            }
            switch scalars.next() {
            case "t": out += "\t"
            case "r": out += "\r"
            case "n": out += "\n"
            case let other?: out.unicodeScalars.append(other)
            case nil: out += "\\"
            }
        }
        return out
    }

    static func escape(_ text: String) -> String {
        var out = ""
        for scalar in text.unicodeScalars {
            switch scalar {
            case "\\": out += "\\\\"
            case "\t": out += "\\t"
            case "\r": out += "\\r"
            case "\n": out += "\\n"
            default: out.unicodeScalars.append(scalar)
            }
        }
        return out
    }
}
