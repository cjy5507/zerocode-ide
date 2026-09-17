import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class TextReplaceOutcomeTests: XCTestCase {
    func testAFailedSetIsUnavailableAndMayFallBack() {
        let outcome = TextReplaceOutcome.judge(setSucceeded: false, expected: "echo abc", readback: nil)
        XCTAssertEqual(outcome, .unavailable)
        XCTAssertTrue(outcome.allowsSyntheticFallback)
        XCTAssertNil(outcome.unverifiedReason)
    }

    func testAnExactReadbackIsVerified() {
        let outcome = TextReplaceOutcome.judge(setSucceeded: true, expected: "echo abc", readback: "echo abc")
        XCTAssertEqual(outcome, .verified)
        XCTAssertFalse(outcome.allowsSyntheticFallback)
        XCTAssertNil(outcome.unverifiedReason)
    }

    /// The terminal case: the sink drained the insert to the pty and cleared
    /// itself. The text landed once; a fallback would type it a second time.
    /// Same vocabulary as the shared Rust core the Windows provider uses.
    func testAWriteThatLandedIsNeverAFallbackCase() {
        let drained = TextReplaceOutcome.judge(setSucceeded: true, expected: "echo abc", readback: "")
        XCTAssertEqual(drained, .valueMismatch(readback: ""))
        XCTAssertFalse(drained.allowsSyntheticFallback)
        XCTAssertEqual(drained.unverifiedReason, "value_mismatch")

        let rewritten = TextReplaceOutcome.judge(setSucceeded: true, expected: "echo abc", readback: "echo  abc")
        XCTAssertEqual(rewritten, .valueMismatch(readback: "echo  abc"))
        XCTAssertFalse(rewritten.allowsSyntheticFallback)

        let unreadable = TextReplaceOutcome.judge(setSucceeded: true, expected: "echo abc", readback: nil)
        XCTAssertEqual(unreadable, .readbackUnsupported)
        XCTAssertFalse(unreadable.allowsSyntheticFallback)
        XCTAssertEqual(unreadable.unverifiedReason, "readback_unsupported")
    }

    /// Keys typed into a field are judged by the shared Rust core's case table.
    func testTypedKeysAreJudgedByTheSharedCaseTable() throws {
        let rows = try SharedCaseTable.rows("typed_landing.tsv")
        XCTAssertGreaterThanOrEqual(rows.count, 6)
        let cell = { (word: String) -> String? in
            switch word {
            case "<empty>": return ""
            case "<unread>": return nil
            default: return word
            }
        }
        for row in rows {
            let outcome = TextReplaceOutcome.judgeTyped(before: cell(row[0]) ?? "", expected: cell(row[1]) ?? "", after: cell(row[2]))
            XCTAssertEqual(outcome.unverifiedReason ?? "verified", row[3], "\(row)")
        }
    }
}
