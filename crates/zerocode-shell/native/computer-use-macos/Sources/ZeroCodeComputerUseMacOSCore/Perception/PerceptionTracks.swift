import Foundation

/// A blob followed from capture to capture, in frame pixels.
struct PerceptionTrack: Equatable {
    let id: UInt64
    let point: PerceptionPoint
    /// Frame pixels per second over the last two captures that saw it; 0 on its first.
    let velocityX: Int64
    let velocityY: Int64
    let capture: UInt64
    let hostNs: UInt64
}

/// Follows a blobs detector's blobs. A blob keeps a track's number only when it is the one blob
/// within the gate of that track's last place and that track is the only one whose gate it lies
/// in; anything less certain starts a new track, and a track no capture saw ends. Numbers come
/// from the session's counter and are never reused, so a scene that is replaced — or a target that
/// jumps further than it can move, or two look-alikes that cross — never hands its number to
/// another object.
struct PerceptionBlobTracks {
    private(set) var tracks: [PerceptionTrack] = []
    /// The track the detector's target follows.
    private(set) var primary: UInt64?

    mutating func reset() {
        tracks = []
        primary = nil
    }

    /// The tracks of this capture's blobs, in the blobs' order. `gate` is how far (frame pixels)
    /// a blob may move per capture; a skipped capture widens it by one gate each.
    mutating func follow(_ blobs: [PerceptionBlob], gate: Int, capture: UInt64, hostNs: UInt64,
                         next: inout UInt64) -> [PerceptionTrack] {
        var candidates = [[Int]](repeating: [], count: tracks.count)
        var claimants = [[Int]](repeating: [], count: blobs.count)
        for (at, track) in tracks.enumerated() where capture > track.capture {
            let reach = Double(gate) * Double(capture - track.capture)
            for (index, blob) in blobs.enumerated() {
                let dx = Double(blob.point.x - track.point.x), dy = Double(blob.point.y - track.point.y)
                if dx * dx + dy * dy <= reach * reach {
                    candidates[at].append(index)
                    claimants[index].append(at)
                }
            }
        }
        var followed: [PerceptionTrack] = []
        followed.reserveCapacity(blobs.count)
        for (index, blob) in blobs.enumerated() {
            if claimants[index].count == 1, let at = claimants[index].first, candidates[at].count == 1 {
                let track = tracks[at]
                let elapsed = hostNs > track.hostNs ? Double(hostNs - track.hostNs) : 0
                func speed(_ moved: Int) -> Int64 {
                    guard elapsed > 0 else { return 0 }
                    let perSecond = (Double(moved) * 1_000_000_000 / elapsed).rounded()
                    return Int64(max(min(perSecond, Double(Int32.max)), Double(Int32.min)))
                }
                followed.append(PerceptionTrack(id: track.id, point: blob.point,
                                                velocityX: speed(blob.point.x - track.point.x),
                                                velocityY: speed(blob.point.y - track.point.y),
                                                capture: capture, hostNs: hostNs))
            } else {
                followed.append(PerceptionTrack(id: next, point: blob.point, velocityX: 0, velocityY: 0,
                                                capture: capture, hostNs: hostNs))
                next += 1
            }
        }
        let kept = followed.contains { $0.id == primary }
        primary = kept ? primary : followed.first?.id
        tracks = followed
        return followed
    }
}

/// A value is known only after `need` fresh captures of one scene answered it with the same
/// target — slow to believe. Any unknown or different answer starts over at once — quick to
/// withdraw.
struct PerceptionConfirm {
    private var value: Int64?
    private var track: UInt64?
    private var count: UInt64 = 0

    mutating func reset() {
        value = nil
        track = nil
        count = 0
    }

    /// Counts one fresh capture's known answer; true once `need` captures in a row agreed.
    mutating func agree(value: Int64, track: UInt64?, need: UInt64) -> Bool {
        if count > 0 && self.value == value && self.track == track {
            count = min(count + 1, need)
        } else {
            (self.value, self.track, count) = (value, track, 1)
        }
        return count >= need
    }
}
