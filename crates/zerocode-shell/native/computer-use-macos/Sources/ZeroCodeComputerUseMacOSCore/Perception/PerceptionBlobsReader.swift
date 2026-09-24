import Foundation

/// Touching samples of one class, in frame pixels. `point` is the blob's sample nearest its
/// centroid — a place its own samples prove, where the centroid of a bent blob may not be.
struct PerceptionBlob: Equatable {
    let samples: Int
    let minX: Int
    let minY: Int
    let maxX: Int
    let maxY: Int
    let point: PerceptionPoint
}

/// What one scan of a blobs detector saw.
struct PerceptionBlobScan: Equatable {
    /// Blobs of at least the layout's `min_samples`, largest first, then top to bottom, then
    /// left to right.
    let blobs: [PerceptionBlob]
    /// Touching groups of the class too small to be a blob.
    let specks: Int
    /// Samples that are no class and not the ground.
    let foreign: Int
    let samples: Int
}

/// The lattice a blobs detector reads over an ROI in frame pixels: every `step` pixels from the
/// ROI's origin, each sample at the middle of its step, the last clamped inside the ROI.
struct PerceptionBlobLattice: Equatable {
    let xs: [Int]
    let ys: [Int]

    init(roi: ReflexRoi, step: Int) {
        let (x, y, width, height) = (Int(roi.x), Int(roi.y), Int(roi.width), Int(roi.height))
        xs = stride(from: 0, to: width, by: step).map { x + min($0 + step / 2, width - 1) }
        ys = stride(from: 0, to: height, by: step).map { y + min($0 + step / 2, height - 1) }
    }

    var samples: Int { xs.count * ys.count }
}

enum PerceptionBlobsReader {
    /// Classifies every lattice sample and gathers the class's touching samples into blobs. `marks`
    /// is the caller's scratch, grown as needed and reused.
    static func read(klass: Int, minSamples: Int, lattice: PerceptionBlobLattice, plane: PerceptionPlane,
                     palette: PerceptionPalette, marks: inout [Int32]) -> PerceptionBlobScan {
        let columns = lattice.xs.count, rows = lattice.ys.count
        if marks.count < columns * rows { marks = [Int32](repeating: 0, count: columns * rows) }
        var foreign = 0
        let (blobs, specks) = marks.withUnsafeMutableBufferPointer { marks in
            palette.withMasks { masks in
                for row in 0..<rows {
                    let y = lattice.ys[row], line = row * columns
                    for column in 0..<columns {
                        let sample = PerceptionPalette.classify(plane.pixel(lattice.xs[column], y), masks)
                        marks[line + column] = sample == klass ? 1 : 0
                        if sample == PerceptionPalette.foreign { foreign += 1 }
                    }
                }
            }
            return PerceptionComponents.gather(marks, lattice: lattice, minSamples: minSamples)
        }
        return PerceptionBlobScan(blobs: blobs, specks: specks, foreign: foreign, samples: lattice.samples)
    }
}

/// Touching marked samples of a lattice, gathered into groups.
enum PerceptionComponents {
    /// Gathers the samples marked 1 that touch (4-neighbours on the lattice) into groups, marking
    /// each gathered sample 2. Groups of at least `minSamples` are blobs — largest first, then top to
    /// bottom, then left to right; smaller ones are counted as specks.
    static func gather(_ marks: UnsafeMutableBufferPointer<Int32>, lattice: PerceptionBlobLattice,
                       minSamples: Int) -> (blobs: [PerceptionBlob], specks: Int) {
        let columns = lattice.xs.count, count = columns * lattice.ys.count
        var blobs: [PerceptionBlob] = []
        var specks = 0
        var stack: [Int] = []
        var members: [Int] = []
        func visit(_ at: Int) {
            if marks[at] == 1 {
                marks[at] = 2
                stack.append(at)
            }
        }
        for seed in 0..<count where marks[seed] == 1 {
            visit(seed)
            members.removeAll(keepingCapacity: true)
            while let at = stack.popLast() {
                members.append(at)
                let column = at % columns
                if column > 0 { visit(at - 1) }
                if column + 1 < columns { visit(at + 1) }
                if at >= columns { visit(at - columns) }
                if at + columns < count { visit(at + columns) }
            }
            guard members.count >= minSamples else {
                specks += 1
                continue
            }
            var minX = Int.max, minY = Int.max, maxX = Int.min, maxY = Int.min, sumX = 0, sumY = 0
            for at in members {
                let x = lattice.xs[at % columns], y = lattice.ys[at / columns]
                minX = min(minX, x); maxX = max(maxX, x); minY = min(minY, y); maxY = max(maxY, y)
                sumX += x; sumY += y
            }
            // Compared at `n` times scale, so the centroid needs no rounding.
            let n = members.count
            var nearest = members[0], distance = Double.infinity
            for at in members {
                let dx = Double(lattice.xs[at % columns] * n - sumX), dy = Double(lattice.ys[at / columns] * n - sumY)
                let squared = dx * dx + dy * dy
                if squared < distance { (nearest, distance) = (at, squared) }
            }
            blobs.append(PerceptionBlob(samples: n, minX: minX, minY: minY, maxX: maxX, maxY: maxY,
                                        point: PerceptionPoint(x: lattice.xs[nearest % columns],
                                                               y: lattice.ys[nearest / columns])))
        }
        blobs.sort { ($1.samples, $0.minY, $0.minX) < ($0.samples, $1.minY, $1.minX) }
        return (blobs, specks)
    }
}
