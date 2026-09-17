import CoreVideo
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class EyePixelsTests: XCTestCase {
    private func frame(width: Int = 16, height: Int = 8, word: UInt32) -> CVPixelBuffer {
        var buffer: CVPixelBuffer?
        XCTAssertEqual(CVPixelBufferCreate(kCFAllocatorDefault, width, height, kCVPixelFormatType_32BGRA, nil, &buffer), kCVReturnSuccess)
        let pixels = buffer!
        CVPixelBufferLockBaseAddress(pixels, [])
        let base = CVPixelBufferGetBaseAddress(pixels)!
        for row in 0..<height {
            let words = base.advanced(by: row * CVPixelBufferGetBytesPerRow(pixels)).assumingMemoryBound(to: UInt32.self)
            for column in 0..<width { words[column] = word }
        }
        CVPixelBufferUnlockBaseAddress(pixels, [])
        return pixels
    }

    func testOnlyIdenticalPixelsAreARepeatIncludingDuringTheFirstSecond() {
        let first = frame(word: 0)
        XCTAssertFalse(EyePixels.same(nil, first), "the first picture is a new baseline")
        XCTAssertTrue(EyePixels.same(first, frame(word: 0)), "a repeated full invalidation can reuse its picture")
        XCTAssertFalse(EyePixels.same(first, frame(word: .max)), "a scene transition is never discarded because the stream is young")
        XCTAssertFalse(EyePixels.same(first, frame(width: 8, word: 0)), "different geometry cannot reuse the picture")
    }

    func testTheInitialFrameIsNotBackgroundMotionButItsNextTransitionIsKept() {
        let first = frame(word: 0)
        XCTAssertFalse(EyePixels.isRepaint(nil, first), "a baseline has no previous pixels to compare")
        XCTAssertFalse(EyePixels.isRepaint(first, frame(word: 0)), "duplicate startup invalidation")
        XCTAssertTrue(EyePixels.isRepaint(first, frame(word: .max)), "a real transition right after startup")
    }

    func testAChangeInTheLastPixelIsKept() {
        let before = frame(word: 0)
        let after = frame(word: 0)
        CVPixelBufferLockBaseAddress(after, [])
        let lastRow = CVPixelBufferGetBaseAddress(after)!.advanced(by: (CVPixelBufferGetHeight(after) - 1) * CVPixelBufferGetBytesPerRow(after))
        lastRow.assumingMemoryBound(to: UInt32.self)[CVPixelBufferGetWidth(after) - 1] = 1
        CVPixelBufferUnlockBaseAddress(after, [])
        XCTAssertFalse(EyePixels.same(before, after), "scan every visible pixel, not a sampled hash")
    }
}
