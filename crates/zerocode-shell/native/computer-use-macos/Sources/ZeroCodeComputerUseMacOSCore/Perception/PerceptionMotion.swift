import Foundation

/// What changed in an ROI since the last capture it saw, read on a lattice: a sample changed when
/// a channel moved by more than `threshold`. The first capture is only a baseline, and a change over
/// `cutPermille` of the lattice is a scene cut, not motion. No plan runs this kernel yet (the common
/// validator refuses motion detectors); it is tested and measured on its own.
struct PerceptionMotion {
    enum Change: Equatable {
        case baseline
        case still
        case moved([PerceptionBlob])
        case cut
    }

    let step: Int
    let threshold: Int
    let cutPermille: Int
    private var lattice: PerceptionBlobLattice?
    private var previous: [UInt32] = []
    private var marks: [Int32] = []

    init(step: Int, threshold: Int, cutPermille: Int) {
        self.step = step
        self.threshold = threshold
        self.cutPermille = cutPermille
    }

    mutating func observe(_ plane: PerceptionPlane, roi: ReflexRoi, minSamples: Int) -> Change {
        let lattice = PerceptionBlobLattice(roi: roi, step: step)
        var current = [UInt32](repeating: 0, count: lattice.samples)
        for (row, y) in lattice.ys.enumerated() {
            for (column, x) in lattice.xs.enumerated() { current[row * lattice.xs.count + column] = plane.pixel(x, y) }
        }
        defer { (self.lattice, previous) = (lattice, current) }
        guard lattice == self.lattice else { return .baseline }
        if marks.count < current.count { marks = [Int32](repeating: 0, count: current.count) }
        var changed = 0
        for at in current.indices {
            let (now, before) = (current[at], previous[at])
            let blue = abs(Int(now & 0xFF) - Int(before & 0xFF))
            let green = abs(Int((now >> 8) & 0xFF) - Int((before >> 8) & 0xFF))
            let red = abs(Int((now >> 16) & 0xFF) - Int((before >> 16) & 0xFF))
            let moved = max(blue, green, red) > threshold
            marks[at] = moved ? 1 : 0
            if moved { changed += 1 }
        }
        if changed * 1000 > cutPermille * current.count { return .cut }
        let (blobs, _) = marks.withUnsafeMutableBufferPointer {
            PerceptionComponents.gather($0, lattice: lattice, minSamples: minSamples)
        }
        return blobs.isEmpty ? .still : .moved(blobs)
    }
}
