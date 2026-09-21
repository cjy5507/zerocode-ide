import XCTest

@testable import ZeroCodeIosEmulatorHelperCore

/// The reader refuses to number a truncated tree outright, so what the
/// exporter calls truncation decides whether a whole screen can be used.
final class AccessibilityChildTallyTests: XCTestCase {
    func testAParentThatKeptEveryChildIsNotTruncated() {
        var tally = AccessibilityChildTally(declared: 2)
        tally.record(.seated(["AXLabel": "일반"]))
        tally.record(.seated(["AXLabel": "손쉬운 사용"]))
        XCTAssertFalse(tally.truncated)
        XCTAssertEqual(tally.seats.count, 2)
    }

    /// The bug (t-5445): a subview two parents reference is seated under the
    /// first one, and the second was counted as having lost a child.
    func testAChildAlreadySeatedUnderAnotherParentIsNotALostOne() {
        var tally = AccessibilityChildTally(declared: 2)
        tally.record(.seated(["AXLabel": "일반"]))
        tally.record(.revisited)
        XCTAssertFalse(tally.truncated)
        XCTAssertEqual(tally.seats.count, 1, "a revisited child must not be seated twice")
    }

    func testAChildTheWalkCouldNotSerializeIsTruncation() {
        var tally = AccessibilityChildTally(declared: 2)
        tally.record(.seated(["AXLabel": "일반"]))
        tally.record(.lost)
        XCTAssertTrue(tally.truncated)
    }

    /// A loop that stops on a spent budget hands the rest over at all.
    func testChildrenTheWalkNeverReachedAreTruncation() {
        var tally = AccessibilityChildTally(declared: 3)
        tally.record(.seated(["AXLabel": "일반"]))
        tally.record(.revisited)
        XCTAssertTrue(tally.truncated)
    }

    func testAParentThatDeclaredNoChildrenIsNotTruncated() {
        let tally = AccessibilityChildTally(declared: 0)
        XCTAssertFalse(tally.truncated)
        XCTAssertTrue(tally.seats.isEmpty)
    }
}
