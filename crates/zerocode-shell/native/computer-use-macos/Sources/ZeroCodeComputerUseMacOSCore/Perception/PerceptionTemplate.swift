import Foundation

/// A small picture to find, row-major BGRA. The common plan validator still refuses template
/// detectors, so nothing live reads one: this kernel is tested and measured on its own until a
/// plan version carries it.
struct PerceptionTemplate: Equatable {
    let width: Int
    let height: Int
    let pixels: [UInt32]

    init?(width: Int, height: Int, pixels: [UInt32]) {
        guard width > 0, height > 0, pixels.count == width * height else { return nil }
        self.width = width
        self.height = height
        self.pixels = pixels
    }
}

/// How a template is looked for: at every `step`-th place of the ROI, comparing every `stride`-th
/// template pixel along both axes. A place matches when its mean absolute channel difference is at
/// most `accept`. The best place is the answer only when every place that does not overlap it by
/// half scores at least `margin` worse; otherwise two places look alike and the answer is ambiguous.
struct PerceptionTemplateSearch: Equatable {
    let step: Int
    let stride: Int
    let accept: Int
    let margin: Int
}

enum PerceptionTemplateMatch: Equatable {
    /// The matched place's top-left corner, in frame pixels, and its score.
    case found(x: Int, y: Int, score: Int)
    case absent(best: Int)
    case ambiguous(best: Int, next: Int)
    /// The search costs more samples than were left; nothing was read.
    case budget
}

enum PerceptionTemplates {
    /// Samples one search reads: the places tried times the template pixels compared at each.
    static func cost(_ template: PerceptionTemplate, roi: ReflexRoi, search: PerceptionTemplateSearch) -> Int {
        guard search.step > 0, search.stride > 0, Int(roi.width) >= template.width, Int(roi.height) >= template.height
        else { return 0 }
        let across = (Int(roi.width) - template.width) / search.step + 1
        let down = (Int(roi.height) - template.height) / search.step + 1
        let compared = ((template.width + search.stride - 1) / search.stride) *
            ((template.height + search.stride - 1) / search.stride)
        return across * down * compared
    }

    static func find(_ template: PerceptionTemplate, in plane: PerceptionPlane, roi: ReflexRoi,
                     search: PerceptionTemplateSearch, samples: inout UInt64) -> PerceptionTemplateMatch {
        let cost = cost(template, roi: roi, search: search)
        guard cost > 0, UInt64(cost) <= samples else { return .budget }
        samples -= UInt64(cost)
        let across = (Int(roi.width) - template.width) / search.step + 1
        let down = (Int(roi.height) - template.height) / search.step + 1
        let compared = ((template.width + search.stride - 1) / search.stride) *
            ((template.height + search.stride - 1) / search.stride)
        var scores = [Int](repeating: 0, count: across * down)
        template.pixels.withUnsafeBufferPointer { wanted in
            for placeY in 0..<down {
                for placeX in 0..<across {
                    let originX = Int(roi.x) + placeX * search.step, originY = Int(roi.y) + placeY * search.step
                    var total = 0
                    for y in Swift.stride(from: 0, to: template.height, by: search.stride) {
                        for x in Swift.stride(from: 0, to: template.width, by: search.stride) {
                            let seen = plane.pixel(originX + x, originY + y), want = wanted[y * template.width + x]
                            total += abs(Int(seen & 0xFF) - Int(want & 0xFF)) +
                                abs(Int((seen >> 8) & 0xFF) - Int((want >> 8) & 0xFF)) +
                                abs(Int((seen >> 16) & 0xFF) - Int((want >> 16) & 0xFF))
                        }
                    }
                    scores[placeY * across + placeX] = total / (3 * compared)
                }
            }
        }
        let best = scores.indices.min { scores[$0] < scores[$1] }!
        if scores[best] > search.accept { return .absent(best: scores[best]) }
        let (bestX, bestY) = (best % across, best / across)
        var next: Int?
        for at in scores.indices {
            let offX: Int = abs(at % across - bestX) * search.step * 2
            let offY: Int = abs(at / across - bestY) * search.step * 2
            if offX >= template.width || offY >= template.height { next = min(next ?? scores[at], scores[at]) }
        }
        if let next, next - scores[best] < search.margin {
            return .ambiguous(best: scores[best], next: next)
        }
        return .found(x: Int(roi.x) + bestX * search.step, y: Int(roi.y) + bestY * search.step, score: scores[best])
    }
}
