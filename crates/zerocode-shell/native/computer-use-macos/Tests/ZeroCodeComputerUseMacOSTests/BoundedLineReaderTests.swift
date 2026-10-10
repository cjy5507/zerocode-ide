import Foundation
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class BoundedLineReaderTests: XCTestCase {
    private func source(_ text: String, chunk: Int = Int.max, reads: @escaping () -> Void = {}) -> (UnsafeMutableRawBufferPointer) -> Int {
        let bytes = Array(text.utf8)
        var consumed = 0
        return { destination in
            reads()
            let count = min(destination.count, chunk, bytes.count - consumed)
            destination.copyBytes(from: bytes[consumed..<(consumed + count)])
            consumed += count
            return count
        }
    }

    func testCoalescedRequestsUseOneReadAndKeepTheTrailingFrame() throws {
        var reader = BoundedLineReader()
        var reads = 0
        let read = source("first\nsecond\nlast", reads: { reads += 1 })
        XCTAssertEqual(try reader.next(read: read), Data("first".utf8))
        XCTAssertEqual(try reader.next(read: read), Data("second".utf8))
        XCTAssertEqual(reads, 1)
        XCTAssertEqual(try reader.next(read: read), Data("last".utf8))
        XCTAssertNil(try reader.next(read: read))
        XCTAssertEqual(reads, 2)
    }

    func testFragmentedUnicodeIsKeptAsBytesWithoutLossOrExtraStrings() throws {
        var reader = BoundedLineReader()
        let text = "{\"text\":\"한글🙂\"}"
        let read = source(text + "\n", chunk: 1)
        XCTAssertEqual(try reader.next(read: read), Data(text.utf8))
        XCTAssertNil(try reader.next(read: read))
    }

    func testBoundaryExcludesTheDelimiterAndIncludesEveryPayloadByte() throws {
        var reader = BoundedLineReader(limit: 4)
        let read = source("1234\n\n🙂\n12345\nallowed\n", chunk: 3)
        XCTAssertEqual(try reader.next(read: read), Data("1234".utf8))
        XCTAssertEqual(try reader.next(read: read), Data())
        XCTAssertEqual(try reader.next(read: read), Data("🙂".utf8))
        XCTAssertThrowsError(try reader.next(read: read)) { error in
            XCTAssertEqual(error as? BoundedLineReader.Failure, .tooLarge)
        }
        XCTAssertNil(try reader.next(read: read))
    }

    func testReadFailureCannotTurnATrailingSuffixIntoAnotherRequest() throws {
        var reader = BoundedLineReader()
        XCTAssertThrowsError(try reader.next { _ in throw CocoaError(.fileReadUnknown) })
        XCTAssertNil(try reader.next(read: source("{}\n")))
    }

    func testLargeFramesNeedBlockReadsNotASystemCallPerByte() throws {
        let text = String(repeating: "a", count: 1_000_000)
        var reader = BoundedLineReader()
        var reads = 0
        let read = source(text + "\n", reads: { reads += 1 })
        XCTAssertEqual(try reader.next(read: read)?.count, text.utf8.count)
        XCTAssertEqual(reads, 123)
    }

    func testInvalidReadCountsFailClosed() throws {
        for count in [-1, 8193] {
            var reader = BoundedLineReader()
            XCTAssertThrowsError(try reader.next { _ in count }) { error in
                XCTAssertEqual(error as? BoundedLineReader.Failure, .invalidRead)
            }
        }
    }
}
