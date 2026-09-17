import CoreGraphics
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class EyeSenseTests: XCTestCase {
    private func config(kept: Double? = 512, frames: Double = 30, share: Double = 0.5) -> EyeConfig? {
        EyeConfig(
            framesPerSecond: frames, changesKept: kept, idleStopMs: 30_000, firstFrameMs: 2_000,
            ocrCellPoints: 24, ocrMarginPoints: 4, ocrMaxShare: share, ocrMaxPieces: 4
        )
    }

    private func config(kept: Double = 512) -> EyeConfig {
        config(kept: kept, frames: 30, share: 0.5)!
    }

    func testTheTableMustBeWholeAndSane() {
        XCTAssertNil(config(kept: nil, frames: 30, share: 0.5), "a missing number is refused, never guessed")
        XCTAssertNil(config(kept: 512, frames: 0, share: 0.5))
        XCTAssertNil(config(kept: 512, frames: 30, share: 1.5), "a share is between 0 and 1")
        XCTAssertEqual(config().framesPerSecond, 30)
        XCTAssertEqual(config().ocr, OcrReuse(cell: 24, margin: 4, maxShare: 0.5, maxPieces: 4))
    }

    func testARectangleLessOthersIsWhatStillShows() {
        let screen = CGRect(x: 0, y: 0, width: 100, height: 100)
        XCTAssertEqual(screen.minus(CGRect(x: 200, y: 0, width: 10, height: 10)), [screen])
        XCTAssertEqual(screen.minus(CGRect(x: 100, y: 0, width: 10, height: 10)), [screen], "touching an edge hides nothing")
        XCTAssertEqual(screen.minus(CGRect(x: -5, y: -5, width: 200, height: 200)), [])
        XCTAssertEqual(screen.minus(CGRect(x: 20, y: 30, width: 40, height: 20)), [
            CGRect(x: 0, y: 0, width: 100, height: 30),
            CGRect(x: 0, y: 50, width: 100, height: 50),
            CGRect(x: 0, y: 30, width: 20, height: 20),
            CGRect(x: 60, y: 30, width: 40, height: 20),
        ], "the same four bands the core cuts")
        XCTAssertEqual(screen.remainder(minus: [CGRect(x: 0, y: 0, width: 100, height: 60), CGRect(x: 0, y: 60, width: 100, height: 40)]), [])
    }

    /// The screen as it stood (2026-09-12): the menu bar, ZeroCode below it,
    /// the Dock's strip below that.
    func testAReadingLeavesOutWhatZeroCodeShows() {
        let zerocode = CGRect(x: 0, y: 33, width: 1512, height: 879)
        let leftOut = LeftOut(rects: [zerocode])
        XCTAssertTrue(leftOut.shows(CGRect(x: 971, y: 270, width: 519, height: 18)), "the agent's own command in its pane")
        XCTAssertFalse(leftOut.shows(CGRect(x: 1300, y: 8, width: 150, height: 16)), "the menu bar's clock")
        XCTAssertTrue(leftOut.covers(CGRect(x: 800, y: 100, width: 300, height: 20)), "a pane's repaint")
        XCTAssertFalse(leftOut.covers(CGRect(x: 800, y: 900, width: 300, height: 40)), "one reaching past ZeroCode")
        XCTAssertFalse(LeftOut(rects: []).covers(CGRect(x: 0, y: 0, width: 1, height: 1)), "nothing left out covers nothing")
        let display = CGRect(x: 0, y: 0, width: 1512, height: 982)
        XCTAssertEqual(leftOut.remainder(of: display), [
            CGRect(x: 0, y: 0, width: 1512, height: 33),
            CGRect(x: 0, y: 912, width: 1512, height: 70),
        ], "what is still read: the menu bar and the Dock's strip")
    }

    func testAWindowAboveTheDocumentsThatSpansADisplayIsAnOverlay() {
        let displays = [CGRect(x: 0, y: 0, width: 1512, height: 982), CGRect(x: 1512, y: 0, width: 1920, height: 1080)]
        XCTAssertTrue(DesktopOverlay.isOverlay(layer: 20, bounds: displays[0], displays: displays), "the Dock's window on macOS 26")
        XCTAssertTrue(DesktopOverlay.isOverlay(layer: 20, bounds: displays[1], displays: displays), "on either display")
        XCTAssertFalse(DesktopOverlay.isOverlay(layer: 0, bounds: displays[0], displays: displays), "a full-screen document window hides what is under it")
        XCTAssertFalse(DesktopOverlay.isOverlay(layer: 24, bounds: CGRect(x: 0, y: 0, width: 1512, height: 33), displays: displays), "the menu bar")
        XCTAssertFalse(DesktopOverlay.isOverlay(layer: 101, bounds: CGRect(x: 300, y: 200, width: 240, height: 400), displays: displays), "a menu")
        XCTAssertFalse(DesktopOverlay.isOverlay(layer: Int(CGWindowLevelForKey(.screenSaverWindow)), bounds: displays[0], displays: displays), "a screen saver is not transparent")
        let bar = CGRect(x: 400, y: 900, width: 600, height: 80)
        XCTAssertEqual(DesktopOverlay.coveringFrames([displays[0], bar], inside: displays[0]), [bar], "the visible Dock surface still covers the window behind it")
    }

    private let reuse = OcrReuse(cell: 24, margin: 4, maxShare: 0.5, maxPieces: 4)
    private let display = CGRect(x: 0, y: 0, width: 1512, height: 982)

    func testAReadingIsReadAgainOnlyWhereItRepaintedAndAlwaysWholeLines() {
        let line = CGRect(x: 100, y: 200, width: 400, height: 16)
        let other = CGRect(x: 100, y: 600, width: 300, height: 16)
        // A caret blinks in the middle of the first line.
        let pieces = reuse.dirty(changed: [CGRect(x: 300, y: 202, width: 2, height: 12)], lines: [line, other], area: display)
        XCTAssertEqual(pieces?.count, 1)
        XCTAssertTrue(pieces![0].contains(line), "the whole line the caret sits in is read again")
        XCTAssertFalse(pieces![0].intersects(other), "the other line is kept")
        XCTAssertEqual(pieces![0].minY.truncatingRemainder(dividingBy: 24), 0, "snapped out to the grid, then grown over the line: \(pieces![0])")
        XCTAssertEqual(pieces![0].minX, line.minX, "the line reaches past the snapped caret")
    }

    func testTouchingRepaintsAreOnePieceAndTooMuchIsAWholeRead() {
        let near = reuse.dirty(changed: [CGRect(x: 10, y: 10, width: 20, height: 10), CGRect(x: 40, y: 10, width: 20, height: 10)], lines: [], area: display)
        XCTAssertEqual(near?.count, 1, "neighbours on the grid join")
        let spread = (0..<5).map { CGRect(x: CGFloat($0) * 300, y: 900, width: 10, height: 10) }
        XCTAssertNil(reuse.dirty(changed: spread, lines: [], area: display), "five pieces cost more than one whole read")
        XCTAssertNil(reuse.dirty(changed: [CGRect(x: 0, y: 0, width: 1512, height: 600)], lines: [], area: display), "past half the display")
        XCTAssertEqual(reuse.dirty(changed: [CGRect(x: 2000, y: 10, width: 20, height: 10)], lines: [], area: display), [], "a repaint off the area reads nothing")
    }

    func testAReadingAcrossTheCropEdgeDoesNotKeepGrowingAfterClipping() {
        let area = CGRect(x: 0, y: 0, width: 100, height: 100)
        let line = CGRect(x: 90, y: 20, width: 20, height: 10)
        let reuse = OcrReuse(cell: 1, margin: 0, maxShare: 1, maxPieces: 4)
        let pieces = reuse.dirty(
            changed: [CGRect(x: 95, y: 22, width: 1, height: 1)],
            lines: [line], area: area
        )
        XCTAssertEqual(pieces, [line.intersection(area)])
    }

    func testTheMergedReadingKeepsUntouchedLinesAndTakesTheFreshOnesInReadingOrder() {
        let kept = [
            RecognizedLine(text: "Total 12", confidence: 1, frame: CGRect(x: 100, y: 200, width: 100, height: 16)),
            RecognizedLine(text: "Header", confidence: 1, frame: CGRect(x: 100, y: 20, width: 100, height: 16)),
        ]
        let fresh = [RecognizedLine(text: "Total 13", confidence: 1, frame: CGRect(x: 100, y: 200, width: 100, height: 16))]
        let piece = CGRect(x: 96, y: 192, width: 120, height: 32)
        let merged = OcrReuse.merged(kept: kept, pieces: [piece], fresh: fresh)
        XCTAssertEqual(merged.map(\.text), ["Header", "Total 13"])
    }

    func testRepaintsAreNumberedInOrderAndReadPastACursorOrFromAMoment() {
        var ring = EyeRing(config: config())
        let square = CGRect(x: 0, y: 0, width: 10, height: 10)
        XCTAssertEqual(ring.note(rects: [square], atMs: 100).seq, 1)
        ring.markAct(atMs: 150)
        XCTAssertEqual(ring.act, EyeMark(seq: 1, atMs: 150), "the act's mark is the number when it began")
        ring.note(rects: [square], atMs: 200)
        ring.note(rects: [square], atMs: 300)
        XCTAssertEqual(ring.latest, 3)
        XCTAssertEqual(ring.changes(after: 1, fromMs: 0).changes.map(\.seq), [2, 3])
        XCTAssertEqual(ring.changes(after: 0, fromMs: 200).changes.map(\.seq), [2, 3])
        XCTAssertTrue(ring.changes(after: 0, fromMs: 0).whole)
    }

    func testARingThatDroppedWhatWasAskedForSaysSo() {
        var ring = EyeRing(config: config(kept: 2))
        for at in [Int64(100), 200, 300, 400] {
            ring.note(rects: [], atMs: at)
        }
        XCTAssertEqual(ring.changes.map(\.seq), [3, 4], "the newest kept")
        XCTAssertFalse(ring.changes(after: 0, fromMs: 0).whole, "1 and 2 were asked for and are gone")
        XCTAssertTrue(ring.changes(after: 2, fromMs: 0).whole)
        XCTAssertTrue(ring.changes(after: 0, fromMs: 250).whole, "nothing that old was asked for")
    }

    func testTheFrameComesAtTheLaddersFirstRungAndItsRepaintsLandInPoints() {
        let size = EyeFrameSize.of(pixelWidth: 3024, pixelHeight: 1964)
        XCTAssertEqual(size.width, 1280)
        XCTAssertEqual(size.height, 831, "1964 x 1280/3024, rounded as the ladder rounds")
        let small = EyeFrameSize.of(pixelWidth: 1024, pixelHeight: 768)
        XCTAssertEqual(small.width, 1024, "a display under the rung is taken as it is")
        XCTAssertEqual(small.height, 768)
        let display = CGRect(x: 1512, y: 0, width: 1512, height: 982)
        let perPoint = 1280.0 / 1512.0
        let placed = EyeFrameSize.onScreen(CGRect(x: 640, y: 100, width: 128, height: 64), unitsPerPoint: perPoint, displayBounds: display)
        XCTAssertEqual(placed.minX, 1512 + 640 / perPoint, accuracy: 1e-9, "a second display sits right of the first")
        XCTAssertEqual(placed.minY, 100 / perPoint, accuracy: 1e-9)
        XCTAssertEqual(placed.width, 128 / perPoint, accuracy: 1e-9)
        XCTAssertEqual(placed.height, 64 / perPoint, accuracy: 1e-9)
    }

    func testTheFirstFrameSaysWhosePixelsTheRepaintsAreIn() {
        let display = CGRect(x: 0, y: 0, width: 1512, height: 982)
        let unit = { (extent: CGFloat?) in
            EyeFrameSize.unitsPerPoint(firstExtent: extent, frameWidth: 1280, displayBounds: display, displayScale: 2)
        }
        XCTAssertEqual(unit(1280), 1280 / 1512, accuracy: 1e-12, "the frame's pixels")
        XCTAssertEqual(unit(3024), 2, "the display's own pixels")
        XCTAssertEqual(unit(1512), 1, "points after all")
        XCTAssertEqual(unit(nil), 1280 / 1512, accuracy: 1e-12, "a first frame that names nothing")
        XCTAssertEqual(unit(0), 1280 / 1512, accuracy: 1e-12)
    }
}
