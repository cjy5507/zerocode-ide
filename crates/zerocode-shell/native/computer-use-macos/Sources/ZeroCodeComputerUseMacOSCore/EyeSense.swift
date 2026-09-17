import CoreGraphics
import Foundation

/// The continuous eye's numbers, as the window's table sends them with
/// `eyeStart` (`zerocode_core::computer_use::eye_table`) — the helper keeps
/// none of its own, so there is one table to change.
public struct EyeConfig: Equatable, Sendable {
    public let framesPerSecond: Int
    public let changesKept: Int
    public let idleStopMs: Int64
    public let firstFrameMs: Int64
    /// How a desktop OCR read keeps its last reading (`OcrReuse`).
    public let ocr: OcrReuse

    /// Nil unless every number is there and makes sense.
    public init?(
        framesPerSecond: Double?,
        changesKept: Double?,
        idleStopMs: Double?,
        firstFrameMs: Double?,
        ocrCellPoints: Double?,
        ocrMarginPoints: Double?,
        ocrMaxShare: Double?,
        ocrMaxPieces: Double?
    ) {
        guard let framesPerSecond, framesPerSecond >= 1,
              let changesKept, changesKept >= 1,
              let idleStopMs, idleStopMs > 0,
              let firstFrameMs, firstFrameMs > 0,
              let ocrCellPoints, ocrCellPoints > 0,
              let ocrMarginPoints, ocrMarginPoints >= 0,
              let ocrMaxShare, (0...1).contains(ocrMaxShare),
              let ocrMaxPieces, ocrMaxPieces >= 1
        else { return nil }
        self.framesPerSecond = Int(framesPerSecond)
        self.changesKept = Int(changesKept)
        self.idleStopMs = Int64(idleStopMs)
        self.firstFrameMs = Int64(firstFrameMs)
        self.ocr = OcrReuse(
            cell: CGFloat(ocrCellPoints),
            margin: CGFloat(ocrMarginPoints),
            maxShare: ocrMaxShare,
            maxPieces: Int(ocrMaxPieces)
        )
    }
}

/// A desktop OCR read keeps its last reading where nothing repainted since
/// (`zerocode_core::computer_use::OCR_REUSE_*`): what did repaint is read
/// again — snapped out to the grid, widened by the margin and by every line
/// of the last reading it touches, so a line is read whole — and past the
/// share or the pieces the whole area is read again.
public struct OcrReuse: Equatable, Sendable {
    public let cell: CGFloat
    public let margin: CGFloat
    public let maxShare: Double
    public let maxPieces: Int

    public init(cell: CGFloat, margin: CGFloat, maxShare: Double, maxPieces: Int) {
        self.cell = cell
        self.margin = margin
        self.maxShare = maxShare
        self.maxPieces = maxPieces
    }

    /// The parts of `area` to read again for the repaints `changed` (screen
    /// points), given the last reading's `lines`: nil when the whole area is
    /// the cheaper read.
    public func dirty(changed: [CGRect], lines: [CGRect], area: CGRect) -> [CGRect]? {
        // A cached line can straddle a crop edge. Growing to its full frame
        // and clipping back on every pass would never reach a fixed point.
        let visibleLines = lines.map { $0.intersection(area) }
            .filter { !$0.isNull && $0.width > 0 && $0.height > 0 }
        var pieces = changed
            .map { $0.insetBy(dx: -margin, dy: -margin).intersection(area) }
            .filter { !$0.isNull && $0.width > 0 && $0.height > 0 }
            .map(snapped)
        // Joined where they touch, and grown over every line they touch,
        // until nothing more joins.
        var grown = true
        while grown {
            grown = false
            for line in visibleLines {
                for at in pieces.indices where pieces[at].intersects(line) && !pieces[at].contains(line) {
                    pieces[at] = pieces[at].union(line)
                    grown = true
                }
            }
            var joined: [CGRect] = []
            for piece in pieces {
                if let at = joined.firstIndex(where: { $0.intersects(piece) }) {
                    joined[at] = joined[at].union(piece)
                    grown = true
                } else {
                    joined.append(piece)
                }
            }
            pieces = joined.map { $0.intersection(area) }
        }
        let share = pieces.reduce(0) { $0 + Double($1.width * $1.height) } / Double(max(area.width * area.height, 1))
        return pieces.count > maxPieces || share > maxShare ? nil : pieces
    }

    /// A rectangle grown out to the grid's lines.
    private func snapped(_ rect: CGRect) -> CGRect {
        let minX = (rect.minX / cell).rounded(.down) * cell
        let minY = (rect.minY / cell).rounded(.down) * cell
        let maxX = (rect.maxX / cell).rounded(.up) * cell
        let maxY = (rect.maxY / cell).rounded(.up) * cell
        return CGRect(x: minX, y: minY, width: maxX - minX, height: maxY - minY)
    }

    /// The last reading's lines no piece touches, with the lines read again
    /// in the pieces, in reading order.
    public static func merged(kept: [RecognizedLine], pieces: [CGRect], fresh: [RecognizedLine]) -> [RecognizedLine] {
        readingOrder(kept.filter { line in !pieces.contains { $0.intersects(line.frame) } } + fresh)
    }
}

/// A point in the stream: a repaint's number and when it happened. An act's
/// mark is the stream's number when the act began.
public struct EyeMark: Equatable, Sendable {
    public let seq: Int
    public let atMs: Int64

    public init(seq: Int, atMs: Int64) {
        self.seq = seq
        self.atMs = atMs
    }
}

/// One repaint the stream saw, where it fell in screen points.
public struct EyeChange: Equatable, Sendable {
    public let seq: Int
    public let atMs: Int64
    public let rects: [CGRect]

    public init(seq: Int, atMs: Int64, rects: [CGRect]) {
        self.seq = seq
        self.atMs = atMs
        self.rects = rects
    }
}

/// What the stream saw: a bounded ring of repaints numbered in the order
/// they were painted, and the mark of the last act. What a wait makes of
/// them is the window's (`zerocode_core::computer_use_protocol::eye`).
public struct EyeRing: Equatable, Sendable {
    public let config: EyeConfig
    public private(set) var changes: [EyeChange] = []
    /// The number of the newest repaint — the cursor a reader asks past.
    public private(set) var latest = 0
    public private(set) var act: EyeMark?
    /// The newest repaint dropped off the front, to tell a reader whether
    /// it got everything it asked for.
    private var dropped: EyeMark?

    public init(config: EyeConfig) {
        self.config = config
    }

    /// Note one repaint.
    @discardableResult
    public mutating func note(rects: [CGRect], atMs: Int64) -> EyeChange {
        latest += 1
        let change = EyeChange(seq: latest, atMs: atMs, rects: rects)
        changes.append(change)
        if changes.count > config.changesKept {
            let gone = changes.count - config.changesKept
            if let last = changes.prefix(gone).last {
                dropped = EyeMark(seq: last.seq, atMs: last.atMs)
            }
            changes.removeFirst(gone)
        }
        return change
    }

    /// An act begins: whatever repaints after this is its doing or the
    /// screen's own.
    public mutating func markAct(atMs: Int64) {
        act = EyeMark(seq: latest, atMs: atMs)
    }

    /// The repaints numbered after `after` and painted at or after `fromMs`,
    /// oldest first, and whether none of those was dropped.
    public func changes(after: Int, fromMs: Int64) -> (changes: [EyeChange], whole: Bool) {
        let asked = changes.filter { $0.seq > after && $0.atMs >= fromMs }
        let whole = dropped.map { !($0.seq > after && $0.atMs >= fromMs) } ?? true
        return (asked, whole)
    }
}

/// The size a display's stream is delivered at: the screenshot ladder's
/// first rung, rounded as the ladder's own resize rounds — so the look's
/// PNG is encoded from the frame as it comes, never scaled on the CPU.
public enum EyeFrameSize {
    public static func of(pixelWidth: Int, pixelHeight: Int) -> (width: Int, height: Int) {
        let scale = ScreenshotBudget.ladder(width: pixelWidth, height: pixelHeight)[0]
        return (
            max(1, Int((CGFloat(pixelWidth) * scale).rounded())),
            max(1, Int((CGFloat(pixelHeight) * scale).rounded()))
        )
    }

    /// How many of the repaints' units make a point. ScreenCaptureKit says
    /// its repaints are "in pixels" without saying whose — the frame's, as
    /// delivered, or the display's own — so the first frame, which repaints
    /// everything, decides: the candidate that makes its widest repaint the
    /// display's width in points. A first frame that names nothing is taken
    /// in the frame's pixels.
    public static func unitsPerPoint(firstExtent: CGFloat?, frameWidth: Int, displayBounds: CGRect, displayScale: CGFloat) -> CGFloat {
        let width = max(displayBounds.width, 1)
        let framePixels = CGFloat(max(frameWidth, 1)) / width
        guard let firstExtent, firstExtent > 0 else { return framePixels }
        return [framePixels, max(displayScale, 1), 1].min { lhs, rhs in
            abs(firstExtent / lhs - width) < abs(firstExtent / rhs - width)
        } ?? framePixels
    }

    /// A repaint's rectangle placed on the screen in points: the display's
    /// origin plus its units over the units per point.
    public static func onScreen(_ rect: CGRect, unitsPerPoint: CGFloat, displayBounds: CGRect) -> CGRect {
        let per = max(unitsPerPoint, .leastNonzeroMagnitude)
        return CGRect(
            x: displayBounds.minX + rect.minX / per,
            y: displayBounds.minY + rect.minY / per,
            width: rect.width / per,
            height: rect.height / per
        )
    }
}
