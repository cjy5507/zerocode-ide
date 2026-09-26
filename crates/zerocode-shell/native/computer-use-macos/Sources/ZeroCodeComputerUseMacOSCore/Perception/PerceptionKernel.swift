import Foundation

/// The helper's perception kernel: one session per validated plan. The runtime opens it when a plan
/// epoch starts and calls it on its evaluation thread with the newest frame only — never in the
/// capture callback, never from two threads. What it answers is data: the runtime issues leases.
public struct PerceptionKernel: ReflexPerceptionKernel {
    public init() {}

    public func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession {
        try PerceptionSession(plan: plan, limits: limits)
    }
}

/// What a frame is read as: a scene lasts while none of these change, and a new one starts every
/// track, cell number and confirmation over.
private struct PerceptionScene: Equatable {
    let run: String
    let stream: UInt64
    let owner: UInt64
    let geometry: UInt64
    let plan: UInt64
    let clock: UInt64
    let width: UInt32
    let height: UInt32

    init(_ frame: ReflexFrameFacts) {
        run = frame.run_id
        stream = frame.stream_epoch
        owner = frame.owner_epoch
        geometry = frame.geometry_epoch
        plan = frame.plan_epoch
        clock = frame.clock_domain
        width = frame.pixel_extent.width
        height = frame.pixel_extent.height
    }
}

public final class PerceptionSession: ReflexPerceptionSession {
    private struct Detector {
        let id: String
        /// The detector's ROI, written against its spec's reference extent.
        let roi: ReflexRoi
        let spec: PerceptionColorSpec
        /// Which blob its target follows when it follows none.
        let pick: ReflexPick
        let palette: PerceptionPalette
        var confirm = PerceptionConfirm()
        var cells: [Int: UInt64] = [:]
        var blobs = PerceptionBlobTracks()
        var marks: [Int32] = []

        mutating func forget() {
            confirm.reset()
            cells = [:]
            blobs.reset()
        }
    }

    private var detectors: [Detector]
    private var scene: PerceptionScene?
    private var lastCapture: UInt64?
    private var nextTrack: UInt64 = 1

    /// Every detector must be a colour detector whose spec passes the helper's own check against
    /// the limits the window sent; anything else refuses the session.
    convenience init(plan: ValidatedReflexPlan, limits: PerceptionLimits) throws {
        try self.init(detectors: plan.plan.detectors.map { detector in
            guard detector.kind == .color, let spec = detector.color else { throw ReflexContractError.perception }
            return (detector.id, detector.roi, spec, detector.pick)
        }, limits: limits)
    }

    /// The detectors in plan order: an id, the ROI written against the spec's reference extent, the
    /// spec and its pick.
    init(detectors: [(id: String, roi: ReflexRoi, spec: PerceptionColorSpec, pick: ReflexPick)], limits: PerceptionLimits) throws {
        self.detectors = try detectors.map { detector in
            guard (try? PerceptionSpecs.validate(detector.spec, roi: detector.roi, limits: limits)) != nil,
                  let palette = PerceptionPalette(detector.spec)
            else { throw ReflexContractError.perception }
            return Detector(id: detector.id, roi: detector.roi, spec: detector.spec, pick: detector.pick, palette: palette)
        }
    }

    /// The plan's detectors on `frame`, in plan order. A frame whose capture time is unknown has no
    /// reference an observation could name (the runtime's cursor never hands one over): it gets none.
    /// Confirmations and tracks are the session's to settle: a detector that answers `budget`, late
    /// or unread, has already let go of them, so a runtime that drops that answer has nothing of
    /// this session's to take back.
    public func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, hand: (x: Int64, y: Int64)?,
                        budget: inout ReflexPerceptionBudget) -> [ReflexObservation] {
        guard let reference = ReflexFrameRef(frame) else {
            for at in detectors.indices { detectors[at].forget() }
            return []
        }
        let scene = PerceptionScene(frame)
        if scene != self.scene {
            for at in detectors.indices { detectors[at].forget() }
            (self.scene, lastCapture) = (scene, nil)
        }
        if let refusal = refusal(frame, pixels) {
            for at in detectors.indices { detectors[at].forget() }
            return detectors.map { observation($0, frame, reference, unknown: refusal) }
        }
        lastCapture = frame.capture_seq
        let plane = PerceptionPlane(pixels)
        var exhausted = false
        return detectors.indices.map { at in look(&detectors[at], frame, reference, plane, hand, &budget, &exhausted) }
    }

    /// Why no detector can read this frame, if none can: it is not a ready capture with a known
    /// time newer than the last one read, its pixels are not the extent it claims, or it is not an
    /// upright sRGB frame.
    private func refusal(_ frame: ReflexFrameFacts, _ pixels: ReflexPixels) -> ReflexUnknown? {
        if frame.status != .ready || lastCapture.map({ frame.capture_seq <= $0 }) == true {
            return .frame
        }
        if pixels.width != Int(frame.pixel_extent.width) || pixels.height != Int(frame.pixel_extent.height) ||
            pixels.width <= 0 || pixels.height <= 0 || pixels.bytesPerRow < pixels.width * 4 {
            return .extent
        }
        if frame.color_space != .srgb { return .color_space }
        if frame.orientation != .up { return .orientation }
        return nil
    }

    /// One detector's answer on a frame every detector can read: its reading, if the tick's deadline
    /// had not passed when the reading ended. A reading that ends past it is late: it answers
    /// `budget` with no value, target or cells and forgets what the detector held, as an unread one
    /// does, and no detector after it reads. The samples it read stay on its receipt; the tick paid
    /// them.
    private func look(_ detector: inout Detector, _ frame: ReflexFrameFacts, _ reference: ReflexFrameRef,
                      _ plane: PerceptionPlane, _ hand: (x: Int64, y: Int64)?, _ budget: inout ReflexPerceptionBudget,
                      _ exhausted: inout Bool) -> ReflexObservation {
        let seen = read(&detector, frame, reference, plane, hand, &budget, &exhausted)
        // `spend(0)` is the budget's own deadline test with nothing left to pay.
        if exhausted || budget.spend(0) { return seen }
        exhausted = true
        detector.forget()
        return observation(detector, frame, reference, unknown: .budget, samples: seen.samples)
    }

    /// One detector's reading of a frame every detector can read. It works on the detector in place:
    /// a copy would copy its scratch every frame.
    private func read(_ detector: inout Detector, _ frame: ReflexFrameFacts, _ reference: ReflexFrameRef,
                      _ plane: PerceptionPlane, _ hand: (x: Int64, y: Int64)?, _ budget: inout ReflexPerceptionBudget,
                      _ exhausted: inout Bool) -> ReflexObservation {
        func observation(_ detector: Detector, unknown: ReflexUnknown? = nil, value: Int64? = nil,
                         target: ReflexTarget? = nil, cells: [ReflexCell]? = nil, samples: UInt64 = 0) -> ReflexObservation {
            self.observation(detector, frame, reference, unknown: unknown, value: value, target: target, cells: cells,
                             samples: samples)
        }
        guard let (roi, _) = PerceptionSpecs.frameRoi(detector.roi, spec: detector.spec, frame: frame.pixel_extent) else {
            detector.forget()
            return observation(detector, unknown: .extent)
        }
        let (width, across) = (UInt64(frame.pixel_extent.width), UInt64(detector.spec.reference_width))
        var places: [PerceptionCellPlace] = []
        var lattice: PerceptionBlobLattice?
        var step = 1
        switch detector.spec.layout {
        case .cells(let layout):
            guard let placed = PerceptionCellsReader.place(layout, roi: roi, withGround: detector.spec.ground != nil) else {
                detector.forget()
                return observation(detector, unknown: .extent)
            }
            places = placed
        case .blobs(let layout):
            // The step is written in reference pixels; at another scale it keeps the same sample count.
            step = Int(max(1, (layout.step * width + across - 1) / across))
            lattice = PerceptionBlobLattice(roi: roi, step: step)
        }
        let anchors = UInt64(detector.spec.anchors.count)
        let samples = UInt64(lattice?.samples ?? places.reduce(0) { $0 + $1.samples })
        if exhausted || !budget.spend(anchors + samples) {
            exhausted = true
            detector.forget()
            return observation(detector, unknown: .budget)
        }
        guard anchorsHold(detector, frame, plane) else {
            detector.forget()
            return observation(detector, unknown: .scene, samples: anchors)
        }
        let cost = anchors + samples
        switch detector.spec.layout {
        case .cells(let layout):
            let cells = PerceptionCellsReader.read(places, plane: plane, palette: detector.palette,
                                                   minShare: layout.min_share_permille)
            // No cell read at all: the board this plan was written for is not on the screen.
            if cells.allSatisfy({ $0.class == 0 }) {
                detector.forget()
                return observation(detector, unknown: .scene, cells: cells, samples: cost)
            }
            let (value, cell, unknown) = PerceptionCellsReader.readout(cells, layout.readout)
            guard let value else {
                detector.confirm.reset()
                return observation(detector, unknown: unknown ?? .occluded, cells: cells, samples: cost)
            }
            var target: ReflexTarget?
            if let cell {
                let track = detector.cells[cell] ?? nextTrack
                if detector.cells[cell] == nil {
                    detector.cells[cell] = track
                    nextTrack += 1
                }
                let place = places[cell]
                target = ReflexTarget(track_id: track,
                                      roi: ReflexRoi(x: Int64(place.x), y: Int64(place.y), width: Int64(place.width),
                                                     height: Int64(place.height), space: .pixel),
                                      point_x: Int64(place.x + place.width / 2), point_y: Int64(place.y + place.height / 2),
                                      velocity_x: 0, velocity_y: 0, uncertainty: 0)
            }
            guard detector.confirm.agree(value: value, track: target?.track_id, need: detector.spec.confirm) else {
                return observation(detector, unknown: .unconfirmed, cells: cells, samples: cost)
            }
            return observation(detector, value: value, target: target, cells: cells, samples: cost)
        case .blobs(let layout):
            guard let lattice else { return observation(detector, unknown: .extent) }
            let gate = Int(max(1, (layout.gate * width + across - 1) / across))
            let scan = PerceptionBlobsReader.read(klass: Int(layout.class), minSamples: Int(layout.min_samples),
                                                  lattice: lattice, plane: plane, palette: detector.palette,
                                                  marks: &detector.marks)
            if UInt64(scan.blobs.count) > layout.max_blobs {
                detector.forget()
                return observation(detector, unknown: .ambiguous, samples: cost)
            }
            let tracks = detector.blobs.follow(scan.blobs, gate: gate, capture: frame.capture_seq,
                                               hostNs: reference.captured_host_ns, pick: detector.pick, hand: hand,
                                               next: &nextTrack)
            var target: ReflexTarget?
            if let at = tracks.firstIndex(where: { $0.id == detector.blobs.primary }) {
                let (blob, track) = (scan.blobs[at], tracks[at])
                target = ReflexTarget(track_id: track.id, roi: blob.box,
                                      point_x: Int64(blob.point.x), point_y: Int64(blob.point.y),
                                      velocity_x: track.velocityX, velocity_y: track.velocityY,
                                      uncertainty: UInt64(step))
            } else if !(detector.palette.hasGround && scan.foreign == 0 && scan.specks == 0) {
                // None seen, but something could hide one: absence is proven only against ground.
                detector.confirm.reset()
                return observation(detector, unknown: scan.specks > 0 ? .ambiguous : .occluded, samples: cost)
            }
            let value = Int64(scan.blobs.count)
            guard detector.confirm.agree(value: value, track: target?.track_id, need: detector.spec.confirm) else {
                return observation(detector, unknown: .unconfirmed, samples: cost)
            }
            return observation(detector, value: value, target: target, samples: cost)
        }
    }

    /// Whether every anchor shows its colour in this frame, placed as `frameRoi` places an edge.
    private func anchorsHold(_ detector: Detector, _ frame: ReflexFrameFacts, _ plane: PerceptionPlane) -> Bool {
        let (width, height) = (UInt64(frame.pixel_extent.width), UInt64(frame.pixel_extent.height))
        let (across, down) = (UInt64(detector.spec.reference_width), UInt64(detector.spec.reference_height))
        return detector.palette.withMasks { masks in
            detector.spec.anchors.allSatisfy { anchor in
                let x = Int(UInt64(anchor.x) * width / across), y = Int(UInt64(anchor.y) * height / down)
                let want = anchor.class == 0 ? PerceptionPalette.ground : Int(anchor.class)
                return PerceptionPalette.classify(plane.pixel(x, y), masks) == want
            }
        }
    }

    /// What one detector answers: cells only when it read its cells, and the frame's scale over the
    /// spec's reference extent as `frameRoi` reduces it.
    private func observation(_ detector: Detector, _ frame: ReflexFrameFacts, _ reference: ReflexFrameRef,
                             unknown: ReflexUnknown? = nil, value: Int64? = nil, target: ReflexTarget? = nil,
                             cells: [ReflexCell]? = nil, samples: UInt64 = 0) -> ReflexObservation {
        let (width, across) = (UInt64(frame.pixel_extent.width), UInt64(detector.spec.reference_width))
        let common = max(1, PerceptionSpecs.gcd(width, across))
        return ReflexObservation(detector_id: detector.id, frame: reference, unknown: unknown, value: value,
                                 target: target, scale: ReflexScale(numerator: width / common, denominator: across / common),
                                 cells: cells, samples: samples)
    }
}
