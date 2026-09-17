import CoreGraphics
import Foundation
import ZeroCodeComputerUseMacOSCore

/// Desktop OCR that keeps its last reading where the eye saw nothing
/// repaint since (docs/design/computer-use-full-operator.md §7.1). One
/// reading of the whole display is kept per display: a later read of the
/// display reads again only what repainted (`OcrReuse`), and a read of a
/// region is answered from it when nothing repainted there. Without an open
/// eye every read is whole, as before.
///
/// What ZeroCode's own windows show (`LeftOut`) is not the reading's
/// business: their repaints are not read again, a whole read reads only
/// what lies outside them when that is a few small pieces, and a read that
/// falls wholly on them reads nothing. The caller drops the lines they show.
enum DesktopReading {
    private struct Kept {
        let generation: Int
        let seq: Int
        let lines: [RecognizedLine]
        /// Where this reading may not have read, or read long ago: the
        /// region it left out.
        let leftOut: [CGRect]
    }

    private static let lock = NSLock()
    nonisolated(unsafe) private static var kept: [CGDirectDisplayID: Kept] = [:]

    /// The lines of `frame` — the display, or a region of it — given where
    /// its display's eye stood before the pixels were taken, and what
    /// ZeroCode's own windows show.
    static func lines(frame: DesktopScreen.Frame, eye: ScreenEye.Standing?, leftOut: LeftOut) throws -> [RecognizedLine] {
        let area = CGRect(x: frame.origin.x, y: frame.origin.y, width: frame.pointsWidth, height: frame.pointsHeight)
        let whole = area == frame.display.bounds
        if leftOut.covers(area) { return [] }
        guard let eye else { return try read(frame, area) }
        lock.lock()
        let last = kept[frame.display.id]
        lock.unlock()
        if let last, last.generation == eye.generation,
           let repainted = ScreenEye.repaints(display: frame.display.id, generation: eye.generation, after: last.seq),
           repainted.whole {
            // ZeroCode repainting its own windows is not read again; what
            // the last reading left out and this one does not, is.
            let here = (repainted.rects.filter { !leftOut.covers($0) } + last.leftOut.flatMap { leftOut.remainder(of: $0) })
                .filter { $0.intersects(area) }
            if here.isEmpty {
                if whole {
                    // All intervening changes were inspected, including
                    // ignored own-window paints. Advance past them so a busy
                    // pane cannot age this cache out of the bounded ring.
                    keep(frame.display.id, Kept(generation: eye.generation, seq: eye.seq, lines: last.lines, leftOut: leftOut.rects))
                }
                return last.lines.filter { area.contains(CGPoint(x: $0.frame.midX, y: $0.frame.midY)) }
            }
            if whole, let pieces = eye.config.ocr.dirty(changed: here, lines: last.lines.map(\.frame), area: area) {
                let merged = OcrReuse.merged(kept: last.lines, pieces: pieces, fresh: try read(frame, pieces))
                keep(frame.display.id, Kept(generation: eye.generation, seq: eye.seq, lines: merged, leftOut: leftOut.rects))
                return merged
            }
        }
        if whole, !leftOut.rects.isEmpty,
           let pieces = eye.config.ocr.dirty(changed: leftOut.remainder(of: area), lines: [], area: area) {
            let lines = try read(frame, pieces)
            keep(frame.display.id, Kept(generation: eye.generation, seq: eye.seq, lines: lines, leftOut: leftOut.rects))
            return lines
        }
        let lines = try read(frame, area)
        if whole {
            keep(frame.display.id, Kept(generation: eye.generation, seq: eye.seq, lines: lines, leftOut: []))
        }
        return lines
    }

    /// Read `pieces` of the frame's pixels, the lines in reading order.
    private static func read(_ frame: DesktopScreen.Frame, _ pieces: [CGRect]) throws -> [RecognizedLine] {
        var lines: [RecognizedLine] = []
        for piece in pieces {
            lines += try read(frame, piece)
        }
        return readingOrder(lines)
    }

    private static func keep(_ id: CGDirectDisplayID, _ reading: Kept) {
        lock.lock()
        kept[id] = reading
        lock.unlock()
    }

    /// Read `piece` (screen points) of the frame's pixels.
    private static func read(_ frame: DesktopScreen.Frame, _ piece: CGRect) throws -> [RecognizedLine] {
        let perPoint = CGFloat(frame.image.width) / max(frame.pointsWidth, 1)
        let pixels = CGRect(
            x: (piece.minX - frame.origin.x) * perPoint,
            y: (piece.minY - frame.origin.y) * perPoint,
            width: piece.width * perPoint,
            height: piece.height * perPoint
        ).integral
        let whole = CGRect(x: 0, y: 0, width: frame.image.width, height: frame.image.height)
        guard pixels != whole else {
            return try recognizeText(in: frame.image, origin: frame.origin, pointsWidth: frame.pointsWidth, pointsHeight: frame.pointsHeight)
        }
        guard let cropped = frame.image.cropping(to: pixels.intersection(whole)) else { return [] }
        return try recognizeText(
            in: cropped,
            origin: CGPoint(x: frame.origin.x + pixels.minX / perPoint, y: frame.origin.y + pixels.minY / perPoint),
            pointsWidth: CGFloat(cropped.width) / perPoint,
            pointsHeight: CGFloat(cropped.height) / perPoint
        )
    }
}
