import ApplicationServices
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

/// The batch read replaces one `AXUIElementCopyAttributeValue` per attribute,
/// so every way that call could have answered has to survive the trip: a
/// value stays a value, and a failure stays nothing at all.
final class SnapshotAttributeBatchTests: XCTestCase {
    private func axError(_ error: AXError) -> CFTypeRef {
        var raw = error
        return AXValueCreate(.axError, &raw)!
    }

    func testEachValueLandsUnderTheAttributeItWasAskedFor() throws {
        let decoded = try XCTUnwrap(SnapshotAttributeBatch.decode(
            attributes: ["AXRole", "AXTitle"],
            values: ["AXButton" as CFString, "보내기" as CFString]
        ))
        XCTAssertEqual((decoded["AXRole"] ?? nil) as? String, "AXButton")
        XCTAssertEqual((decoded["AXTitle"] ?? nil) as? String, "보내기")
    }

    /// The partial failure the batch reports as a value. A single read
    /// returned nothing here, and every caller upstream reads an optional.
    func testAnAttributeTheElementCannotAnswerIsAbsentRatherThanAValue() throws {
        let decoded = try XCTUnwrap(SnapshotAttributeBatch.decode(
            attributes: ["AXRole", "AXURL", "AXValue"],
            values: [
                "AXGroup" as CFString,
                axError(.attributeUnsupported),
                axError(.noValue),
            ]
        ))
        XCTAssertEqual((decoded["AXRole"] ?? nil) as? String, "AXGroup")
        XCTAssertTrue(decoded.keys.contains("AXURL"))
        XCTAssertNil(decoded["AXURL"] ?? nil)
        XCTAssertTrue(decoded.keys.contains("AXValue"))
        XCTAssertNil(decoded["AXValue"] ?? nil)
    }

    func testANullEntryIsAbsentToo() throws {
        let decoded = try XCTUnwrap(SnapshotAttributeBatch.decode(
            attributes: ["AXPlaceholder"],
            values: [kCFNull]
        ))
        XCTAssertTrue(decoded.keys.contains("AXPlaceholder"))
        XCTAssertNil(decoded["AXPlaceholder"] ?? nil)
    }

    /// A geometry value is an `AXValue` as well; only the error kind means
    /// the read failed, or every frame in the tree would go missing.
    func testAGeometryValueIsNotMistakenForAFailure() throws {
        var point = CGPoint(x: 12, y: 34)
        let position = AXValueCreate(.cgPoint, &point)!
        let decoded = try XCTUnwrap(SnapshotAttributeBatch.decode(
            attributes: ["AXPosition"],
            values: [position]
        ))
        let answered = try XCTUnwrap(decoded["AXPosition"] ?? nil)
        var read = CGPoint.zero
        XCTAssertTrue(AXValueGetValue(answered as! AXValue, .cgPoint, &read))
        XCTAssertEqual(read, CGPoint(x: 12, y: 34))
    }

    func testAnEmptyArrayValueStillCountsAsAnAnswer() throws {
        let decoded = try XCTUnwrap(SnapshotAttributeBatch.decode(
            attributes: ["AXChildren"],
            values: [[] as CFArray]
        ))
        XCTAssertNotNil(decoded["AXChildren"] ?? nil)
        XCTAssertEqual(((decoded["AXChildren"] ?? nil) as? [AnyObject])?.count, 0)
    }

    /// An answer that does not line up with the request tells the reader
    /// nothing it can trust; it falls back to reading each attribute alone.
    func testAnAnswerOfTheWrongLengthIsRefusedWhole() {
        XCTAssertNil(SnapshotAttributeBatch.decode(
            attributes: ["AXRole", "AXTitle"],
            values: ["AXButton" as CFString]
        ))
        XCTAssertNil(SnapshotAttributeBatch.decode(
            attributes: ["AXRole"],
            values: ["AXButton" as CFString, "보내기" as CFString]
        ))
    }

    func testTheTableNamesEachAttributeOnce() {
        let table = SnapshotNodeAttributes.all
        XCTAssertEqual(Set(table).count, table.count, "the table names an attribute twice")
        XCTAssertFalse(table.isEmpty)
        for attribute in table {
            XCTAssertTrue(attribute.hasPrefix("AX"), "\(attribute) is not an Accessibility attribute")
        }
    }
}
