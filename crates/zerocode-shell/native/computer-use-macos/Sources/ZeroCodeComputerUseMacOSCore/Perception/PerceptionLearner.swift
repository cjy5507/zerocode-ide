import Foundation

/// Which part of the labelled scenes an example comes from (`game_state::learn::Split`). Held-out
/// scenes are scored once a candidate is fixed; they never teach it.
enum PerceptionSplit: String, Codable {
    case train, tune, heldout
}

/// One box the person marked in a demonstration, in frame pixels: what they called it — `nil` for
/// "none of these" — and the input they made on it, if any. Labels come from the person, never from
/// text on the screen.
struct PerceptionExample {
    let scene: String
    let split: PerceptionSplit
    let box: ReflexRoi
    let label: String?
    let input: ReflexLeaseInput?
}

enum PerceptionCandidateStatus: String, Codable {
    case candidate
}

/// A label's colour as the demonstration showed it (`game_state::learn::LearnedClass`).
struct PerceptionLearnedClass: Codable, Equatable {
    let label: String
    let r: UInt8
    let g: UInt8
    let b: UInt8
    let tolerance: UInt8

    var colour: PerceptionColorClass { PerceptionColorClass(r: r, g: g, b: b, tolerance: tolerance) }
}

/// What a demonstration proposes (`game_state::learn::Candidate`): one class per label, the inputs
/// the person made and the scenes it saw. It is a document for the planner — not a spec that runs
/// and not an input permission; the window adopts it only against the person's goal and a held-out
/// score its own scenes never fed.
struct PerceptionCandidate: Codable, Equatable {
    let status: PerceptionCandidateStatus
    let classes: [PerceptionLearnedClass]
    let inputs: [ReflexLeaseInput]
    let scenes: [String]

    /// The label a pixel shows, or nil: a colour no example showed is no label, not the nearest one.
    func label(of pixel: UInt32) -> String? {
        let (b, g, r) = (Int(pixel & 0xFF), Int((pixel >> 8) & 0xFF), Int((pixel >> 16) & 0xFF))
        return classes.first { item in
            let reach = Int(item.tolerance)
            return abs(r - Int(item.r)) <= reach && abs(g - Int(item.g)) <= reach && abs(b - Int(item.b)) <= reach
        }?.label
    }
}

enum PerceptionLearnError: Error, Equatable {
    /// A held-out example was offered to learn from.
    case heldout
    /// No labelled pixel to learn from, or a box outside its scene.
    case empty
    /// A box marked "none of these" shows a learned colour.
    case confusable
    /// The learned classes break the palette rules: too many, too wide, or overlapping.
    case palette(PerceptionSpecError)
}

enum PerceptionLearner {
    /// One class per label from every pixel inside its boxes: each channel's midrange, and the
    /// tolerance that just holds every pixel seen — no margin, so a colour the demonstration did not
    /// show stays unknown. Classes come out in label order.
    static func propose(_ examples: [PerceptionExample], planes: [String: PerceptionPlane],
                        limits: PerceptionLimits) throws -> PerceptionCandidate {
        if examples.contains(where: { $0.split == .heldout }) { throw PerceptionLearnError.heldout }
        var ranges: [String: [ClosedRange<Int>]] = [:]
        var refused: [UInt32] = []
        for example in examples {
            guard let plane = planes[example.scene] else { throw PerceptionLearnError.empty }
            let (x0, y0) = (max(0, Int(example.box.x)), max(0, Int(example.box.y)))
            let x1 = min(plane.width, Int(example.box.x + example.box.width))
            let y1 = min(plane.height, Int(example.box.y + example.box.height))
            guard x0 < x1, y0 < y1 else { throw PerceptionLearnError.empty }
            for y in y0..<y1 {
                for x in x0..<x1 {
                    let pixel = plane.pixel(x, y)
                    guard let label = example.label else {
                        refused.append(pixel)
                        continue
                    }
                    let channels = [Int((pixel >> 16) & 0xFF), Int((pixel >> 8) & 0xFF), Int(pixel & 0xFF)]
                    let seen = ranges[label] ?? channels.map { $0...$0 }
                    ranges[label] = zip(seen, channels).map { min($0.lowerBound, $1)...max($0.upperBound, $1) }
                }
            }
        }
        if ranges.isEmpty { throw PerceptionLearnError.empty }
        let classes = ranges.keys.sorted().map { label -> PerceptionLearnedClass in
            let spans = ranges[label]!
            let middles = spans.map { ($0.lowerBound + $0.upperBound) / 2 }
            let reach = zip(spans, middles).map { max($0.upperBound - $1, $1 - $0.lowerBound) }.max()!
            return PerceptionLearnedClass(label: label, r: UInt8(middles[0]), g: UInt8(middles[1]),
                                          b: UInt8(middles[2]), tolerance: UInt8(reach))
        }
        do {
            try PerceptionSpecs.checkPalette(classes.map(\.colour), ground: nil, limits: limits)
        } catch let error as PerceptionSpecError {
            throw PerceptionLearnError.palette(error)
        }
        let candidate = PerceptionCandidate(
            status: .candidate, classes: classes,
            inputs: Set(examples.compactMap { $0.label == nil ? nil : $0.input }).sorted { $0.rawValue < $1.rawValue },
            scenes: Set(examples.map(\.scene)).sorted())
        if refused.contains(where: { candidate.label(of: $0) != nil }) { throw PerceptionLearnError.confusable }
        return candidate
    }
}
