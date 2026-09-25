import Foundation

/// Where one cell is read, in frame pixels: its sampled box (the tile less its inset — the only
/// part of the cell its samples prove, so the hitbox), the lattice inside it, and the probes in
/// the gaps around its tile that must be ground.
struct PerceptionCellPlace: Equatable {
    let x: Int
    let y: Int
    let width: Int
    let height: Int
    let xs: [Int]
    let ys: [Int]
    let gaps: [PerceptionPoint]

    var samples: Int { xs.count * ys.count + gaps.count }
}

struct PerceptionPoint: Equatable {
    let x: Int
    let y: Int
}

/// One axis of a cells layout placed in frame pixels (`PerceptionAxis` is its arithmetic).
private struct PerceptionAxisPlace {
    let start: Int
    let sampled: Int
    let lattice: [Int]
    /// The middles of the gaps before and after the tile, when the axis has gaps.
    let gaps: [Int]
}

enum PerceptionCellsReader {
    /// The cells of a layout over an ROI in frame pixels, in reading order; nil when this frame's
    /// scale leaves a cell's sampled box shorter than the lattice or closes a gap the ground must
    /// be seen in. Pitch boxes split each span at `floor(i × span / count)`.
    static func place(_ layout: PerceptionCells, roi: ReflexRoi, withGround: Bool) -> [PerceptionCellPlace]? {
        let inset = Int(layout.inset_permille), lattice = Int(layout.lattice)
        let across = axis(start: Int(roi.x), span: Int(roi.width), count: Int(layout.columns),
                          tile: Int(layout.tile_width_permille), inset: inset, lattice: lattice,
                          withGround: withGround)
        let down = axis(start: Int(roi.y), span: Int(roi.height), count: Int(layout.rows),
                        tile: Int(layout.tile_height_permille), inset: inset, lattice: lattice,
                        withGround: withGround)
        guard let columns = across, let rows = down else { return nil }
        var places: [PerceptionCellPlace] = []
        places.reserveCapacity(rows.count * columns.count)
        for row in rows {
            for column in columns {
                var gaps: [PerceptionPoint] = []
                for x in column.gaps { gaps += row.lattice.map { PerceptionPoint(x: x, y: $0) } }
                for y in row.gaps { gaps += column.lattice.map { PerceptionPoint(x: $0, y: y) } }
                places.append(PerceptionCellPlace(x: column.start, y: row.start, width: column.sampled,
                                                  height: row.sampled, xs: column.lattice, ys: row.lattice, gaps: gaps))
            }
        }
        return places
    }

    private static func axis(start: Int, span: Int, count: Int, tile: Int, inset: Int, lattice: Int,
                             withGround: Bool) -> [PerceptionAxisPlace]? {
        guard count > 0, span >= count else { return nil }
        var places: [PerceptionAxisPlace] = []
        for index in 0..<count {
            let from = start + index * span / count
            let pitch = start + (index + 1) * span / count - from
            let tileWidth = pitch * tile / 1000
            let spare = pitch - tileWidth
            let before = spare / 2, after = spare - spare / 2
            let tileStart = from + before
            let insetWidth = tileWidth * inset / 1000
            let sampled = tileWidth - 2 * insetWidth
            guard sampled >= lattice else { return nil }
            let sampledStart = tileStart + insetWidth
            let positions = (0..<lattice).map { sampledStart + (2 * $0 + 1) * sampled / (2 * lattice) }
            var gaps: [Int] = []
            if withGround && tile < 1000 {
                guard before > 0, after > 0 else { return nil }
                gaps = [from + before / 2, tileStart + tileWidth + after / 2]
            }
            places.append(PerceptionAxisPlace(start: sampledStart, sampled: sampled, lattice: positions, gaps: gaps))
        }
        return places
    }

    /// Reads every cell: its class when the winning class holds at least `minShare` permille of
    /// the cell's samples and, with ground, as much of its gap probes are ground. Short of that the
    /// cell is `ambiguous` when other classes take more of it than foreign colour does, and
    /// `occluded` otherwise (or whenever its gaps are not ground).
    static func read(_ places: [PerceptionCellPlace], plane: PerceptionPlane, palette: PerceptionPalette,
                     minShare: UInt64) -> [ReflexCell] {
        var counts = [Int](repeating: 0, count: palette.classes + 1)
        return palette.withMasks { masks in
            places.map { place in
                for at in counts.indices { counts[at] = 0 }
                var foreign = 0
                for y in place.ys {
                    for x in place.xs {
                        let sample = PerceptionPalette.classify(plane.pixel(x, y), masks)
                        if sample > 0 { counts[sample] += 1 } else { foreign += 1 }
                    }
                }
                var gapGround = 0
                for probe in place.gaps where
                    PerceptionPalette.classify(plane.pixel(probe.x, probe.y), masks) == PerceptionPalette.ground {
                    gapGround += 1
                }
                let total = place.xs.count * place.ys.count
                var winner = 0, winning = 0, second = 0
                for klass in 1..<counts.count {
                    if counts[klass] > winning {
                        second = winning
                        winner = klass
                        winning = counts[klass]
                    } else if counts[klass] > second {
                        second = counts[klass]
                    }
                }
                let share = UInt64(winning * 1000 / total)
                let gapsHold = place.gaps.isEmpty ||
                    UInt64(gapGround) * 1000 >= minShare * UInt64(place.gaps.count)
                if gapsHold && UInt64(winning) * 1000 >= minShare * UInt64(total) {
                    return ReflexCell(class: UInt64(winner), unknown: nil, share_permille: share)
                }
                let reason: ReflexUnknown = gapsHold && second > foreign ? .ambiguous : .occluded
                return ReflexCell(class: 0, unknown: reason, share_permille: share)
            }
        }
    }

    /// The readout over read cells: its value and the index of the cell it names, or why it is
    /// unknown. A `first` is unknown while any cell before its answer is unknown — that cell may
    /// be the first — and a `count` while any cell is.
    static func readout(_ cells: [ReflexCell], _ readout: PerceptionReadout)
        -> (value: Int64?, cell: Int?, unknown: ReflexUnknown?) {
        switch readout {
        case .cell(let index):
            let cell = cells[Int(index) - 1]
            return cell.class > 0 ? (Int64(cell.class), Int(index) - 1, nil) : (nil, nil, cell.unknown)
        case .first(let klass):
            for (at, cell) in cells.enumerated() {
                if cell.class == 0 { return (nil, nil, cell.unknown) }
                if cell.class == klass { return (Int64(at + 1), at, nil) }
            }
            return (0, nil, nil)
        case .count(let klass):
            if let unread = cells.first(where: { $0.class == 0 }) { return (nil, nil, unread.unknown) }
            let first = cells.firstIndex { $0.class == klass }
            return (Int64(cells.filter { $0.class == klass }.count), first, nil)
        }
    }
}
