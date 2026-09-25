import Foundation

// MARK: - Captures

/// What a capture knows about one frame before a run stamps its own epochs
/// on it — the one way a platform adapter writes a frame's facts
/// (`ReflexFrameFacts` keeps its memberwise initializer to this module).
public struct ReflexCapture: Equatable, Sendable {
    public let displayId: String
    public let region: ReflexRoi
    public let pixelExtent: ReflexPixelExtent
    public let pointTransform: ReflexPointTransform
    public let orientation: ReflexOrientation
    public let colorSpace: ReflexColorSpace
    public let status: ReflexFrameStatus
    public let dirty: Bool
    public let captureGap: UInt64
    public let deliveredHostNs: UInt64?
    public let captureSeq: UInt64
    public let repaintSeq: UInt64
    public let streamEpoch: UInt64
    public let geometryEpoch: UInt64
    public let clockDomain: UInt64
    /// When the frame was captured on the host clock; nil when the source
    /// did not say — never the delivery time standing in for it.
    public let capturedHostNs: UInt64?

    public init(
        displayId: String,
        region: ReflexRoi,
        pixelExtent: ReflexPixelExtent,
        pointTransform: ReflexPointTransform,
        orientation: ReflexOrientation,
        colorSpace: ReflexColorSpace,
        status: ReflexFrameStatus,
        dirty: Bool,
        captureGap: UInt64,
        deliveredHostNs: UInt64?,
        captureSeq: UInt64,
        repaintSeq: UInt64,
        streamEpoch: UInt64,
        geometryEpoch: UInt64,
        clockDomain: UInt64,
        capturedHostNs: UInt64?
    ) {
        self.displayId = displayId
        self.region = region
        self.pixelExtent = pixelExtent
        self.pointTransform = pointTransform
        self.orientation = orientation
        self.colorSpace = colorSpace
        self.status = status
        self.dirty = dirty
        self.captureGap = captureGap
        self.deliveredHostNs = deliveredHostNs
        self.captureSeq = captureSeq
        self.repaintSeq = repaintSeq
        self.streamEpoch = streamEpoch
        self.geometryEpoch = geometryEpoch
        self.clockDomain = clockDomain
        self.capturedHostNs = capturedHostNs
    }

    /// The frame as one run sees it: the capture's facts under the run's
    /// hold on the hand (`ownerEpoch`) and its plan (`planEpoch`).
    public func facts(runId: String, ownerEpoch: UInt64, planEpoch: UInt64) -> ReflexFrameFacts {
        ReflexFrameFacts(
            run_id: runId,
            display_id: displayId,
            region: region,
            pixel_extent: pixelExtent,
            point_transform: pointTransform,
            orientation: orientation,
            color_space: colorSpace,
            status: status,
            dirty: dirty,
            capture_gap: captureGap,
            delivered_host_ns: deliveredHostNs,
            capture_seq: captureSeq,
            repaint_seq: repaintSeq,
            stream_epoch: streamEpoch,
            owner_epoch: ownerEpoch,
            geometry_epoch: geometryEpoch,
            plan_epoch: planEpoch,
            clock_domain: clockDomain,
            captured_host_ns: capturedHostNs
        )
    }
}

public extension ReflexFrameFacts {
    /// A pixel of this frame on the desktop, in points (`point_transform`).
    func point(ofPixelX x: Double, y: Double) -> SmoothPointerPath.Point {
        let scale = Double(point_transform.points_per_pixel.numerator) / Double(max(point_transform.points_per_pixel.denominator, 1))
        return SmoothPointerPath.Point(x: Double(point_transform.origin_x) + x * scale, y: Double(point_transform.origin_y) + y * scale)
    }

    /// A desktop point in this frame's pixels, or nil when the transform
    /// cannot be inverted.
    func pixel(ofPoint point: SmoothPointerPath.Point) -> (x: Double, y: Double)? {
        guard point_transform.points_per_pixel.numerator > 0 else { return nil }
        let scale = Double(point_transform.points_per_pixel.denominator) / Double(point_transform.points_per_pixel.numerator)
        return ((point.x - Double(point_transform.origin_x)) * scale, (point.y - Double(point_transform.origin_y)) * scale)
    }
}

// MARK: - One eye's captures

/// Where a display sits and how fine it is when an eye reads it: its desktop
/// rectangle in points, its pixels a point and its rotation.
public struct EyeGeometry: Equatable, Sendable {
    public let displayId: String
    public let originX: Double
    public let originY: Double
    public let width: Double
    public let height: Double
    public let scale: Double
    public let orientation: ReflexOrientation

    public init(displayId: String, originX: Double, originY: Double, width: Double, height: Double, scale: Double, orientation: ReflexOrientation) {
        self.displayId = displayId
        self.originX = originX
        self.originY = originY
        self.width = width
        self.height = height
        self.scale = scale
        self.orientation = orientation
    }
}

/// One eye's captures (realtime v1 §5.1) as its platform adapter reports
/// them: every frame the stream delivers is the next capture. A complete or
/// idle frame is ready — its point transform read off the display as it
/// stands at that capture — while the display is still what the eye opened
/// on. A frame that is neither, a display that moved, rescaled or turned, or
/// an eye no longer open makes the newest capture an interrupted one: a
/// reader sees a newer capture it must refuse, and that takes back what it
/// saw before (`ReflexSightings.revoke`). An eye whose display changed stays
/// so; the next eye is another stream. Pure — the adapter reads the stream
/// and the display, the book decides.
public struct ReflexCaptureBook: Sendable {
    public let opened: EyeGeometry
    public let generation: UInt64
    public private(set) var captureSeq: UInt64 = 0
    public private(set) var newest: ReflexCapture?
    /// Why this eye's captures stopped describing its display, once they did.
    public private(set) var lost: String?
    private var gap: UInt64 = 0

    public init(opened: EyeGeometry, generation: UInt64) {
        self.opened = opened
        self.generation = generation
    }

    /// A complete frame, or an idle one of the pixels already held, delivered
    /// with the display standing at `now` (nil: the display is gone).
    @discardableResult
    public mutating func delivered(
        extent: ReflexPixelExtent,
        colorSpace: ReflexColorSpace,
        dirty: Bool,
        repaintSeq: UInt64,
        capturedNs: UInt64?,
        deliveredNs: UInt64,
        now: EyeGeometry?
    ) -> ReflexCapture {
        if lost == nil, now != opened { lost = Self.changed }
        let ready = lost == nil
        let geometry = now ?? opened
        captureSeq += 1
        if !ready { gap += 1 }
        let capture = ReflexCapture(
            displayId: geometry.displayId,
            region: ReflexRoi(x: 0, y: 0, width: Int64(extent.width), height: Int64(extent.height), space: .pixel),
            pixelExtent: extent,
            pointTransform: Self.transform(geometry, pixelWidth: extent.width),
            orientation: geometry.orientation,
            colorSpace: colorSpace,
            status: ready ? .ready : .interrupted,
            dirty: dirty,
            captureGap: gap,
            deliveredHostNs: deliveredNs,
            captureSeq: captureSeq,
            repaintSeq: repaintSeq,
            streamEpoch: generation,
            // One geometry an eye: a display that changed never reads ready again here.
            geometryEpoch: generation,
            clockDomain: ReflexContract.hostUptimeClockDomain,
            capturedHostNs: capturedNs
        )
        if ready { gap = 0 }
        newest = capture
        return capture
    }

    /// A frame that was neither complete nor idle (blank, suspended, the
    /// stream starting or stopping): the newest capture no longer shows the
    /// display. Nil before the first capture — nothing to take back.
    @discardableResult
    public mutating func interrupted(capturedNs: UInt64?, deliveredNs: UInt64?) -> ReflexCapture? {
        guard let last = newest else { return nil }
        captureSeq += 1
        gap += 1
        let capture = ReflexCapture(
            displayId: last.displayId, region: last.region, pixelExtent: last.pixelExtent,
            pointTransform: last.pointTransform, orientation: last.orientation, colorSpace: last.colorSpace,
            status: .interrupted, dirty: false, captureGap: gap, deliveredHostNs: deliveredNs,
            captureSeq: captureSeq, repaintSeq: last.repaintSeq, streamEpoch: last.streamEpoch,
            geometryEpoch: last.geometryEpoch, clockDomain: last.clockDomain, capturedHostNs: capturedNs
        )
        newest = capture
        return capture
    }

    /// The newest capture for a run's reader, with the display standing at
    /// `now` and whether this eye is still the one open on it: a changed
    /// display or a closed eye interrupts it once, and it stays so.
    public mutating func read(open: Bool, now: EyeGeometry?) -> ReflexCapture? {
        if lost == nil, !open || now != opened {
            lost = open ? Self.changed : Self.closed
            interrupted(capturedNs: nil, deliveredNs: nil)
        }
        return newest
    }

    private static let changed = "the display changed since the eye opened"
    private static let closed = "the eye closed"

    /// A capture's pixels on the desktop: the display's origin, and its
    /// points over the buffer's pixels as a reduced fraction.
    static func transform(_ geometry: EyeGeometry, pixelWidth: UInt32) -> ReflexPointTransform {
        let points = UInt64(max(0, geometry.width.rounded()))
        let pixels = UInt64(max(1, pixelWidth))
        var (a, b) = (points, pixels)
        while b != 0 { (a, b) = (b, a % b) }
        let common = max(a, 1)
        return ReflexPointTransform(
            origin_x: Int64(geometry.originX.rounded()),
            origin_y: Int64(geometry.originY.rounded()),
            points_per_pixel: ReflexScale(numerator: points / common, denominator: pixels / common)
        )
    }
}

/// One capture with its pixels on loan to the evaluating thread. It holds
/// the platform's buffer, so it stays on the thread that asked for it.
public struct ReflexFrameLoan {
    public let capture: ReflexCapture
    private let read: ((ReflexPixels) -> Void) -> Bool

    public init(capture: ReflexCapture, read: @escaping ((ReflexPixels) -> Void) -> Bool) {
        self.capture = capture
        self.read = read
    }

    /// Read the pixels for the length of `body`; false when they could not be
    /// locked or are not the frame's extent — the frame is then unread.
    public func withPixels(_ body: (ReflexPixels) -> Void) -> Bool { read(body) }
}

/// Where a run's frames come from. The capture callback publishes the newest
/// frame and wakes the run; the run takes the newest on its own thread and
/// never waits in the callback.
public protocol ReflexFrameSource: AnyObject, Sendable {
    /// The newest capture, with its pixels on loan; nil before the first.
    func newest() -> ReflexFrameLoan?
    /// Who to wake on every new capture: cheap, never blocking. Nil clears it.
    func onCapture(_ wake: (@Sendable () -> Void)?)
}

// MARK: - What was seen

/// The newest evaluated frame and the admissible sightings on it, shared by
/// the evaluating thread (which writes) and the hand's thread (which reads
/// before every event). A newer capture the run refuses takes what was seen
/// back: it is no longer evidence for any event, and a leaf decided on it
/// posts nothing more — not even once a later capture is seen again.
public final class ReflexSightings: @unchecked Sendable {
    public struct Seen: Sendable {
        public let frame: ReflexFrameFacts
        public let byDetector: [String: ReflexObservation]
        /// How many times evidence was taken back before this frame was
        /// published: a leaf acts only on frames of the evidence it was
        /// decided on.
        public fileprivate(set) var evidence: UInt64 = 0

        public init(frame: ReflexFrameFacts, byDetector: [String: ReflexObservation]) {
            self.frame = frame
            self.byDetector = byDetector
        }
    }

    private let lock = NSLock()
    private var newestSeen: Seen?
    private var revocations: UInt64 = 0

    public init() {}

    public func publish(_ seen: Seen) {
        lock.lock()
        var stamped = seen
        stamped.evidence = revocations
        newestSeen = stamped
        lock.unlock()
    }

    /// A newer capture was refused, or the frames stopped: nothing seen
    /// before it is evidence any more.
    public func revoke() {
        lock.lock()
        revocations += 1
        newestSeen = nil
        lock.unlock()
    }

    public func latest() -> Seen? {
        lock.lock()
        defer { lock.unlock() }
        return newestSeen
    }
}

// MARK: - Which rule fires

/// Which rule fires on a frame (realtime v1 §5.3): on an edge, never a level.
/// A rule fires when its predicate reads true and it is armed; it is armed at
/// the start and again by reading false on a frame the run evaluated. Unknown
/// neither fires nor re-arms, a frame the run never evaluated — dropped
/// because the evaluator takes only the newest — arms nothing, and the same
/// capture read twice is read once. Among rules ready together the highest
/// priority fires, a tie going to the smaller id; cooldown and the rule's own
/// fire count bound it. While the hand is busy nothing fires and nothing is
/// used up: a rule still armed fires on the next frame that reads true.
///
/// A rule whose `max_fires` are spent does not fire, and an edge it reads
/// meanwhile is used up with nothing fired: whatever comes back, it fires
/// again only on a new false→true. Under a renewing run policy
/// (`ReflexRunPolicy.renew`) a spent rule gets exactly its fire count back —
/// one rule at a time, on a frame read while the hand is free and the run's
/// standing still allows it (`read`'s `renewal`, asked at most once a frame and
/// only when a spent rule is read) — and nothing else: whether it is armed, when
/// it last fired (its cooldown) and the frame read last stand. Without renewal
/// `max_fires` is the rule's total for the run.
public struct ReflexRuleBook {
    private struct Arm {
        var armed = true
        /// Fires since the rule's quota was last given back.
        var fires: UInt64 = 0
        var lastFireNs: UInt64?
        /// When the quota was spent, until it comes back.
        var spentAtNs: UInt64?
    }

    /// What one frame's read came to.
    public struct Read: Sendable {
        /// The rule that fires now, already counted as fired.
        public let fired: ReflexRule?
        /// Each quota given back on this frame: the rule and how long it had
        /// been spent.
        public let renewed: [Renewal]
    }

    public struct Renewal: Equatable, Sendable {
        public let rule: String
        public let spentForNs: UInt64
    }

    private let order: [ReflexRule]
    private let renews: Bool
    private var arms: [String: Arm]
    private var lastFrame: (stream: UInt64, capture: UInt64)?
    /// Every fire of the run, across renewals.
    public private(set) var fires: UInt64 = 0
    /// Every quota given back.
    public private(set) var renewals: UInt64 = 0

    public init(_ plan: ValidatedReflexPlan, policy: ReflexRunPolicy) {
        order = plan.plan.rules.sorted { lhs, rhs in
            lhs.priority != rhs.priority ? lhs.priority > rhs.priority : lhs.id < rhs.id
        }
        renews = policy.renew
        arms = Dictionary(uniqueKeysWithValues: plan.plan.rules.map { ($0.id, Arm()) })
    }

    /// Read one evaluated frame. `values` holds each detector's known value
    /// on it; a detector missing from it is unknown. `renewal` is asked — at
    /// most once, only under a renewing policy, only while the hand is free and
    /// only once a spent rule is read — whether the run may give spent quotas
    /// back now; it re-checks the run's standing and admits nothing.
    public mutating func read(
        streamEpoch: UInt64, captureSeq: UInt64, values: [String: Int64], nowNs: UInt64, handFree: Bool,
        renewal: () -> Bool = { false }
    ) -> Read {
        if let last = lastFrame, (streamEpoch, captureSeq) <= (last.stream, last.capture) { return Read(fired: nil, renewed: []) }
        lastFrame = (streamEpoch, captureSeq)
        var chosen: ReflexRule?
        var renewed: [Renewal] = []
        var mayRenew: Bool?
        for rule in order {
            guard var arm = arms[rule.id] else { continue }
            if arm.fires >= rule.max_fires, renews, handFree {
                if mayRenew == nil { mayRenew = renewal() }
                if mayRenew == true {
                    renewed.append(Renewal(rule: rule.id, spentForNs: arm.spentAtNs.map { nowNs >= $0 ? nowNs - $0 : 0 } ?? 0))
                    arm.fires = 0
                    arm.spentAtNs = nil
                    renewals += 1
                }
            }
            switch rule.predicate.evaluate(values[rule.detector]) {
            case .no:
                arm.armed = true
            case .unknown:
                break
            case .yes:
                if arm.fires >= rule.max_fires {
                    // Spent: the edge is used up, and nothing fires on it later.
                    arm.armed = false
                    break
                }
                let cooled = arm.lastFireNs.map { nowNs >= $0 && nowNs - $0 >= rule.cooldown_ms &* 1_000_000 } ?? true
                if chosen == nil, handFree, arm.armed, cooled {
                    chosen = rule
                    arm.armed = false
                    arm.fires += 1
                    arm.lastFireNs = nowNs
                    if arm.fires >= rule.max_fires { arm.spentAtNs = nowNs }
                    fires += 1
                }
            }
            arms[rule.id] = arm
        }
        return Read(fired: chosen, renewed: renewed)
    }
}

/// One leaf action of a fired rule: a move or a click at a detector's target.
public struct ReflexLeaf: Equatable, Sendable {
    public let ruleId: String
    public let actionId: String
    public let kind: ReflexActionKind
    public let detector: String

    public init(ruleId: String, actionId: String, kind: ReflexActionKind, detector: String) {
        self.ruleId = ruleId
        self.actionId = actionId
        self.kind = kind
        self.detector = detector
    }
}

public enum ReflexMacros {
    /// The leaf actions one fire of `macroId` runs, in order: a macro's
    /// actions `repeat` times, a nested macro expanded where it stands. The
    /// validated plan is acyclic and inside `max_expanded_actions`.
    public static func leaves(ruleId: String, macroId: String, in plan: ReflexPlan) -> [ReflexLeaf] {
        let byId = Dictionary(plan.macros.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        var out: [ReflexLeaf] = []
        func expand(_ id: String) {
            guard let item = byId[id] else { return }
            for _ in 0..<item.repeat {
                for action in item.actions {
                    switch action.kind {
                    case .macro: expand(action.target)
                    case .move, .click: out.append(ReflexLeaf(ruleId: ruleId, actionId: action.id, kind: action.kind, detector: action.target))
                    case .key: continue
                    }
                }
            }
        }
        expand(macroId)
        return out
    }
}

// MARK: - Leases

extension ReflexActionLease {
    /// A lease for one leaf action, issued from the frame it was decided on:
    /// no longer than the table's `max_lease_ns` nor past the run's deadline,
    /// its target proof no longer than that frame's `max_frame_age_ns`. The
    /// kernel invents no time. A lease issued at or after the deadline ends
    /// where it starts, and so permits nothing.
    static func issue(
        runId: String,
        leaf: ReflexLeaf,
        target: ReflexTarget,
        frame: ReflexFrameFacts,
        nowNs: UInt64,
        deadlineNs: UInt64,
        children: UInt64,
        limits: ReflexLimits
    ) -> ReflexActionLease {
        let validUntil = min(nowNs &+ limits.max_lease_ns, deadlineNs)
        return ReflexActionLease(
            run_id: runId,
            action_id: leaf.actionId,
            target_id: "\(leaf.detector)#\(target.track_id)",
            target_roi: target.roi,
            allowed_inputs: leaf.kind == .click ? [.pointer_move, .left_click] : [.pointer_move],
            owner_epoch: frame.owner_epoch,
            stream_epoch: frame.stream_epoch,
            geometry_epoch: frame.geometry_epoch,
            plan_epoch: frame.plan_epoch,
            clock_domain: frame.clock_domain,
            source_capture_seq: frame.capture_seq,
            issued_host_ns: nowNs,
            valid_until_host_ns: validUntil,
            target_proof_until_host_ns: min(validUntil, (frame.captured_host_ns ?? 0) &+ limits.max_frame_age_ns),
            max_children: children,
            used_children: 0
        )
    }

    /// The same lease, its target proof carried by a newer capture's sighting
    /// of the same target: the hitbox where it is now, proven until that
    /// capture is too old. Source, issue time and end stay.
    func renewed(by frame: ReflexFrameFacts, target: ReflexTarget, limits: ReflexLimits) -> ReflexActionLease {
        ReflexActionLease(
            run_id: run_id, action_id: action_id, target_id: target_id, target_roi: target.roi,
            allowed_inputs: allowed_inputs, owner_epoch: owner_epoch, stream_epoch: stream_epoch,
            geometry_epoch: geometry_epoch, plan_epoch: plan_epoch, clock_domain: clock_domain,
            source_capture_seq: source_capture_seq, issued_host_ns: issued_host_ns,
            valid_until_host_ns: valid_until_host_ns,
            target_proof_until_host_ns: min(valid_until_host_ns, (frame.captured_host_ns ?? 0) &+ limits.max_frame_age_ns),
            max_children: max_children, used_children: used_children
        )
    }

    /// The same lease with one more child event spent.
    func spent() -> ReflexActionLease {
        ReflexActionLease(
            run_id: run_id, action_id: action_id, target_id: target_id, target_roi: target_roi,
            allowed_inputs: allowed_inputs, owner_epoch: owner_epoch, stream_epoch: stream_epoch,
            geometry_epoch: geometry_epoch, plan_epoch: plan_epoch, clock_domain: clock_domain,
            source_capture_seq: source_capture_seq, issued_host_ns: issued_host_ns,
            valid_until_host_ns: valid_until_host_ns, target_proof_until_host_ns: target_proof_until_host_ns,
            max_children: max_children, used_children: used_children + 1
        )
    }
}

// MARK: - Receipts

/// What one leaf action did, for the window's evidence (R3). Times are on the
/// host clock; a nil time is a step the action never reached.
public struct ReflexReceipt: Equatable, Sendable {
    public enum Outcome: String, Equatable, Sendable {
        case done
        /// The operator's count or pace refused it (`OperatorLedger`).
        case admission
        /// Nothing known to aim at on the newest frame.
        case no_target
        /// The target's uncertainty does not fit its hitbox.
        case unaimed
        /// The lease did not hold on the newest frame (age, epoch, proof, end).
        case lease
        /// A newer capture was refused (not ready, its time unknown or earlier
        /// than the last, the display changed) or the frames stopped: what the
        /// lease was decided on is no longer evidence.
        case evidence
        /// A newer capture no longer shows the target (gone, unknown, another
        /// track in its place), or it left the pointer before the press, or
        /// the pointer left the point the boundary was asked about.
        case moved
        /// The point is not the run's to act on: ZeroCode's own window, or a
        /// window of another app than the one the run was started for.
        case scope
        /// The operator stopped, or the run's hold was taken back.
        case stopped
        /// The platform could not make or post an event.
        case failed
    }

    public let ruleId: String
    public let actionId: String
    public let leafIndex: UInt64
    public let outcome: Outcome
    public let targetId: String?
    public let sourceCapture: UInt64?
    public let decidedHostNs: UInt64?
    public let admittedHostNs: UInt64?
    /// How long the first event waited for a capture newer than the one the
    /// lease came from — kept apart from the rest of the first-event delay.
    public let captureWaitNs: UInt64
    public let firstEventHostNs: UInt64?
    /// The capture time of the frame that permitted the first event — the
    /// newer capture the lease waited for.
    public let firstEventFrameHostNs: UInt64?
    public let downHostNs: UInt64?
    public let upHostNs: UInt64?
    public let endedHostNs: UInt64
    public let events: UInt64
}

/// The receipts a run keeps until the window has them on disk (realtime v1
/// §5.8): each numbered in the run's own order, handed to one reader as often
/// as it asks after the last number it holds, and let go only when that
/// reader acknowledges a number it wrote — so an answer lost on the way, or a
/// write that failed, reads the same receipts again, never fewer. Bounded by
/// the window's table and never a wait: the hand offers a receipt and goes on,
/// and a queue the reader left full ends the run rather than the receipts
/// (`ReflexSession`), with nothing it holds waiting on this lock.
public final class ReflexReceipts: @unchecked Sendable {
    public struct Entry: Equatable, Sendable {
        /// The receipt's place in its run, from 1.
        public let seq: UInt64
        public let receipt: ReflexReceipt
    }

    private let lock = NSLock()
    public let capacity: Int
    private var queue: [Entry] = []
    private var issued: UInt64 = 0
    private var acknowledged: UInt64 = 0

    public init(capacity: Int) {
        self.capacity = max(0, capacity)
    }

    /// Whether one more receipt fits beside the ones not yet acknowledged.
    public var hasRoom: Bool {
        lock.lock()
        defer { lock.unlock() }
        return queue.count < capacity
    }

    /// Keep one receipt if there is room, numbered next; never waits.
    @discardableResult
    public func offer(_ receipt: ReflexReceipt) -> UInt64? {
        lock.lock()
        defer { lock.unlock() }
        guard queue.count < capacity else { return nil }
        issued += 1
        queue.append(Entry(seq: issued, receipt: receipt))
        return issued
    }

    /// The kept receipts numbered after `after`, oldest first, up to `limit`:
    /// read, not taken — the same ones come back until they are acknowledged.
    public func read(after: UInt64, limit: Int) -> [Entry] {
        lock.lock()
        defer { lock.unlock() }
        return Array(queue.lazy.filter { $0.seq > after }.prefix(max(0, limit)))
    }

    /// The reader has every receipt through `through` on disk: they are let go
    /// of. A number past the last one issued acknowledges only what was issued.
    /// Answers how many were let go of now.
    @discardableResult
    public func acknowledge(through: UInt64) -> Int {
        lock.lock()
        defer { lock.unlock() }
        let upTo = min(through, issued)
        let before = queue.count
        queue.removeAll { $0.seq <= upTo }
        acknowledged = max(acknowledged, upTo)
        return before - queue.count
    }

    /// Receipts kept and not yet acknowledged.
    public var pending: Int {
        lock.lock()
        defer { lock.unlock() }
        return queue.count
    }

    /// The last number issued, and the last acknowledged.
    public var numbers: (issued: UInt64, acknowledged: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        return (issued, acknowledged)
    }
}

// MARK: - One leaf on the hand

/// Runs one leaf action on the hand (realtime v1 §5.4): one admission for the
/// leaf; a lease from the frame it was decided on; before every event the
/// lease read against the newest evaluated frame — never the capture the
/// lease came from — and the hold read by the hand itself. Every newer capture
/// must still show the same target, and nothing goes once the evidence the
/// leaf was decided on is taken back. Where it goes is asked of the run's
/// boundary before its first event, and where it presses before the press —
/// read again once the boundary has answered. It lives on the run's hand
/// thread.
struct ReflexLeafRunner {
    let runId: String
    let hand: OperatorHand
    let token: OperatorHand.Token
    let limits: ReflexLimits
    let style: PointerStyle
    let sightings: ReflexSightings
    let admit: @Sendable () -> GuardAdmission
    /// The pause between a press and its release (`SyntheticMouseClickDelivery`).
    let fenceNs: UInt64
    /// What the run may act on (`ReflexInputBoundary`).
    let boundary: any ReflexInputBoundary
    /// The run's one deadline: no lease outlives it.
    let deadlineNs: UInt64

    private struct Draft {
        let leaf: ReflexLeaf
        let index: UInt64
        var targetId: String?
        var sourceCapture: UInt64?
        var decidedHostNs: UInt64?
        var admittedHostNs: UInt64?
        var captureWaitNs: UInt64 = 0
        var firstEventHostNs: UInt64?
        var firstEventFrameHostNs: UInt64?
        var downHostNs: UInt64?
        var upHostNs: UInt64?
        var events: UInt64 = 0

        func end(_ outcome: ReflexReceipt.Outcome, at endedHostNs: UInt64) -> ReflexReceipt {
            ReflexReceipt(
                ruleId: leaf.ruleId, actionId: leaf.actionId, leafIndex: index, outcome: outcome,
                targetId: targetId, sourceCapture: sourceCapture, decidedHostNs: decidedHostNs,
                admittedHostNs: admittedHostNs, captureWaitNs: captureWaitNs,
                firstEventHostNs: firstEventHostNs, firstEventFrameHostNs: firstEventFrameHostNs,
                downHostNs: downHostNs, upHostNs: upHostNs,
                endedHostNs: endedHostNs, events: events
            )
        }
    }

    private enum Halt: Error {
        case outcome(ReflexReceipt.Outcome)
    }

    func run(_ leaf: ReflexLeaf, index: UInt64) -> ReflexReceipt {
        var draft = Draft(leaf: leaf, index: index)
        do {
            try perform(leaf, &draft)
            return draft.end(.done, at: hand.nowNs())
        } catch let Halt.outcome(outcome) {
            return draft.end(outcome, at: hand.nowNs())
        } catch is OperatorHand.Refusal {
            return draft.end(.stopped, at: hand.nowNs())
        } catch {
            return draft.end(.failed, at: hand.nowNs())
        }
    }

    private func perform(_ leaf: ReflexLeaf, _ draft: inout Draft) throws {
        // At the run's deadline no lease could hold: nothing is admitted.
        guard hand.nowNs() < deadlineNs else { throw Halt.outcome(.lease) }
        try admitOnce()
        draft.admittedHostNs = hand.nowNs()
        guard let seen = sightings.latest(), case let (sighting, target)? = seen.sighting(of: leaf.detector)
        else { throw Halt.outcome(.no_target) }
        let evidence = seen.evidence
        let source = seen.frame
        let issuedNs = hand.nowNs()
        guard let aimed = target.aim(atHostNs: issuedNs, capturedHostNs: sighting.frame.captured_host_ns, maxAgeNs: limits.max_frame_age_ns)
        else { throw Halt.outcome(.unaimed) }
        let end = source.point(ofPixelX: Double(aimed.x), y: Double(aimed.y))
        // Where the leaf goes is the run's to act on before anything moves.
        try within(leaf.kind == .click ? .left_click : .pointer_move, at: end)
        let start = hand.pointerNow() ?? end
        let path = PointerSchedule.glide(from: start, to: end, style: style, tickNs: limits.pointer_tick_ns, startNs: issuedNs)
        let presses: UInt64 = leaf.kind == .click ? 2 : 0
        var lease = ReflexActionLease.issue(
            runId: runId, leaf: leaf, target: target, frame: source,
            nowNs: issuedNs, deadlineNs: deadlineNs, children: UInt64(path.count) + presses, limits: limits
        )
        draft.targetId = lease.target_id
        draft.sourceCapture = source.capture_seq
        draft.decidedHostNs = source.captured_host_ns

        var aim = end
        var posted = -1
        while posted + 1 < path.count {
            try hand.sleep(untilNs: path[posted + 1].dueNs, by: token)
            let (frame, now) = try newer(than: lease, evidence: evidence, draft: &draft)
            // Every newer capture must still show the target: one that does
            // not ends the glide, whatever proof the capture before it left.
            guard case let (fresh, moved)? = frame.sighting(of: leaf.detector, track: target.track_id)
            else { throw Halt.outcome(.moved) }
            lease = lease.renewed(by: frame.frame, target: moved, limits: limits)
            if let again = moved.aim(atHostNs: now, capturedHostNs: fresh.frame.captured_host_ns, maxAgeNs: limits.max_frame_age_ns) {
                aim = frame.frame.point(ofPixelX: Double(again.x), y: Double(again.y))
            }
            guard lease.permits(frame.frame, now_host_ns: now, input: .pointer_move, limits: limits) else { throw Halt.outcome(.lease) }
            guard let index = PointerSchedule.due(path, after: posted, nowNs: now) else { continue }
            let share = style.instant || index == path.count - 1 ? 1 : style.share(Double(index + 1) / Double(path.count))
            let point = SmoothPointerPath.Point(x: start.x + (aim.x - start.x) * share, y: start.y + (aim.y - start.y) * share)
            try hand.post(HandEvent(.pointerMove, x: point.x, y: point.y), by: token)
            if draft.firstEventHostNs == nil {
                draft.firstEventHostNs = hand.nowNs()
                draft.firstEventFrameHostNs = frame.frame.captured_host_ns
            }
            draft.events += 1
            lease = lease.spent()
            posted = index
        }
        guard leaf.kind == .click else { return }

        // The press: a capture newer than the one the lease came from still
        // shows the same target, the lease holds, the pointer is inside where
        // the target is now, and the point is the run's to press. The
        // boundary's answer takes its time (the window server is asked), so
        // what became known meanwhile — evidence taken back, a newer capture,
        // the clock past the frame or the lease — is read again before the
        // press, and the press goes only where the boundary was asked.
        var (frame, now) = try newer(than: lease, evidence: evidence, draft: &draft)
        let pointer = try pressPoint(leaf, target, on: frame, at: now, lease: &lease)
        try within(.left_click, at: pointer)
        (frame, now) = try newer(than: lease, evidence: evidence, draft: &draft)
        guard try pressPoint(leaf, target, on: frame, at: now, lease: &lease) == pointer else { throw Halt.outcome(.moved) }
        try hand.post(HandEvent(.buttonDown(.left, clickState: 1), x: pointer.x, y: pointer.y), by: token)
        draft.downHostNs = hand.nowNs()
        if draft.firstEventHostNs == nil {
            draft.firstEventHostNs = draft.downHostNs
            draft.firstEventFrameHostNs = frame.frame.captured_host_ns
        }
        draft.events += 1
        lease = lease.spent()
        // The fence the window server needs between a press and its release;
        // a stop inside it has already let go of the button (`OperatorHand.stop`).
        try hand.sleep(untilNs: hand.nowNs() &+ fenceNs, by: token)
        try hand.post(HandEvent(.buttonUp(.left, clickState: 1), x: pointer.x, y: pointer.y), by: token)
        draft.upHostNs = hand.nowNs()
        draft.events += 1
    }

    /// Where the press goes on `seen` at `now`, `lease` renewed by it: the
    /// leaf's target on that capture, the lease holding a click then, and the
    /// aim and the pointer still inside where the target is. A lease that no
    /// longer holds ends the leaf as `lease` before the target's place is read.
    private func pressPoint(
        _ leaf: ReflexLeaf, _ target: ReflexTarget, on seen: ReflexSightings.Seen, at now: UInt64, lease: inout ReflexActionLease
    ) throws -> SmoothPointerPath.Point {
        guard case let (fresh, moved)? = seen.sighting(of: leaf.detector, track: target.track_id) else { throw Halt.outcome(.moved) }
        lease = lease.renewed(by: seen.frame, target: moved, limits: limits)
        guard lease.permits(seen.frame, now_host_ns: now, input: .left_click, limits: limits) else { throw Halt.outcome(.lease) }
        guard moved.aim(atHostNs: now, capturedHostNs: fresh.frame.captured_host_ns, maxAgeNs: limits.max_frame_age_ns) != nil,
              let pointer = hand.pointerNow(), let pixel = seen.frame.pixel(ofPoint: pointer),
              moved.roi.contains(x: pixel.x, y: pixel.y)
        else { throw Halt.outcome(.moved) }
        return pointer
    }

    /// The boundary's word on `input` at `point`: a refusal ends the leaf
    /// before the event.
    private func within(_ input: ReflexLeaseInput, at point: SmoothPointerPath.Point) throws {
        if boundary.refusal(input, at: point) != nil { throw Halt.outcome(.scope) }
    }

    /// Admit the leaf once: a paced table's wait is slept on the hand, where
    /// a stop ends it; anything else refused ends the leaf before any event.
    private func admitOnce() throws {
        while true {
            switch admit() {
            case .admitted:
                return
            case let .wait(seconds):
                let waitNs = UInt64(max(0, seconds) * 1_000_000_000)
                try hand.sleep(untilNs: hand.nowNs() &+ waitNs, by: token)
            case .stopped, .sessionBudget, .noBudget:
                throw Halt.outcome(.admission)
            }
        }
    }

    /// The newest evaluated frame once it is a capture newer than the lease's
    /// source, and now: the wait for that capture is the leaf's own record. A
    /// lease or proof that ends first ends the leaf, and so does evidence
    /// taken back since the leaf was decided — a later capture seen again
    /// does not bring it back.
    private func newer(than lease: ReflexActionLease, evidence: UInt64, draft: inout Draft) throws -> (ReflexSightings.Seen, UInt64) {
        let waitStarted = hand.nowNs()
        while true {
            let now = hand.nowNs()
            guard let seen = sightings.latest(), seen.evidence == evidence else { throw Halt.outcome(.evidence) }
            if seen.frame.stream_epoch == lease.stream_epoch, seen.frame.capture_seq > lease.source_capture_seq {
                if draft.firstEventHostNs == nil { draft.captureWaitNs &+= now &- waitStarted }
                return (seen, now)
            }
            let deadline = min(lease.valid_until_host_ns, lease.target_proof_until_host_ns)
            guard now < deadline else { throw Halt.outcome(.lease) }
            try hand.nap(untilNs: deadline, by: token)
        }
    }
}

extension ReflexSightings.Seen {
    /// What `detector` sees on this frame when its target is known and there
    /// — of `track` when one is named — else nil.
    func sighting(of detector: String, track: UInt64? = nil) -> (ReflexObservation, ReflexTarget)? {
        guard let sighting = byDetector[detector], sighting.frame.capture_seq == frame.capture_seq,
              let target = sighting.target, sighting.value.map({ $0 != 0 }) == true,
              track.map({ $0 == target.track_id }) ?? true
        else { return nil }
        return (sighting, target)
    }
}

public extension ReflexRoi {
    /// Whether a pixel point lies inside this box (half-open, pixel space).
    func contains(x: Double, y: Double) -> Bool {
        space == .pixel && x >= Double(self.x) && y >= Double(self.y) &&
            x < Double(self.x) + Double(width) && y < Double(self.y) + Double(height)
    }
}

// MARK: - The stream's rate for several readers

/// How fast one display's stream runs when several readers want it (realtime
/// v1 §5.1): the fastest any of them asks. A reader asking for more, or giving
/// it back, changes the rate in place — never a restart — so the repaint
/// numbers, the encoded look and every other reader's cursor carry on. The
/// window's own eye table is the floor; a run is a reader until it ends.
public struct EyeReaders: Equatable, Sendable {
    public enum Change: Equatable, Sendable {
        /// Nothing about the stream changes.
        case keep
        /// Change the running stream's rate in place.
        case rate(Int)
        /// Stop the stream and open a new one (a different table from the window).
        case restart
    }

    public private(set) var config: EyeConfig?
    private var readers: [String: Int] = [:]

    public init() {}

    public var framesPerSecond: Int? {
        guard let config else { return nil }
        return max(config.framesPerSecond, readers.values.max() ?? 0)
    }

    /// Whether a reader other than the window's looks keeps the stream open.
    public var held: Bool { !readers.isEmpty }

    /// The window starts the eye with its table.
    public mutating func start(_ table: EyeConfig) -> Change {
        guard let config else {
            self.config = table
            return .restart
        }
        if config == table { return .keep }
        self.config = table
        return .restart
    }

    /// A reader asks for at least `framesPerSecond`.
    public mutating func add(_ reader: String, framesPerSecond: Int) -> Change {
        let before = self.framesPerSecond
        readers[reader] = framesPerSecond
        return before == self.framesPerSecond ? .keep : self.framesPerSecond.map(Change.rate) ?? .keep
    }

    /// A reader is done.
    public mutating func remove(_ reader: String) -> Change {
        let before = framesPerSecond
        readers[reader] = nil
        return before == framesPerSecond ? .keep : framesPerSecond.map(Change.rate) ?? .keep
    }

    /// The eye closed: its readers go with it.
    public mutating func closed() {
        config = nil
        readers.removeAll()
    }
}

// MARK: - A run

/// Hears other input for a run (a listen-only event tap on the Mac).
public protocol ReflexInputMonitor: AnyObject, Sendable {
    /// Start hearing: `heard` for every event, `interrupted` when the system
    /// turned the monitor off and on again. Throws when it cannot hear at all.
    func start(heard: @escaping @Sendable (InputOrigin) -> Void, interrupted: @escaping @Sendable (String) -> Void) throws
    func stop()
}

/// What one reflex run may act on — the baton its start hands the run: the
/// plan's own scope and the app that scope's target resolved to when the run
/// began. The window names the target in the plan it hashed; the helper
/// resolves it once, never to ZeroCode itself, and every press is held to it
/// (`ReflexInputBoundary`).
public struct ReflexActingScope: Equatable, Sendable {
    public let surface: ReflexSurface
    public let target: String
    public let pid: Int32

    public init(surface: ReflexSurface, target: String, pid: Int32) {
        self.surface = surface
        self.target = target
        self.pid = pid
    }
}

/// The last word before a reflex run's input goes (realtime v1 §5.4): whether
/// `point` is the run's to act on — asked on the hand's thread before a leaf's
/// first event and before its press, and a refusal ends the leaf with nothing
/// posted. The helper's answer is the verbs' own policy (never ZeroCode's own
/// window) held to the run's `ReflexActingScope`; nothing here widens what a
/// verb may do.
public protocol ReflexInputBoundary: Sendable {
    /// Why `input` may not go at `point`, or nil when it may.
    func refusal(_ input: ReflexLeaseInput, at point: SmoothPointerPath.Point) -> String?
}

/// Why a run paused or ended on its own, in the words its status reports. A
/// hold the hand took back by itself says the hand's own reason
/// (`HoldRevocation`); a run the operator's ledger refused to renew says the
/// ledger's (`StopReason`).
public enum ReflexRunReason {
    /// Someone else's input was heard.
    public static let externalInput = "external_input"
    /// The system turned the monitor off: what came between is unknown.
    public static let monitorInterrupted = "monitor_interrupted"
    /// The run could not start.
    public static let startFailed = "start_failed"
    /// The run's one deadline came (`ReflexRunPolicy.run_ns`).
    public static let deadline = "deadline"
    /// The receipts nobody acknowledged filled the queue: the run ends rather
    /// than act with nowhere to keep what it did.
    public static let overflow = "overflow"
    /// A renewal found the window's guard table missing from the ledger.
    public static let noGuardTable = "no_guard_table"
}

// MARK: - The run's deadline

/// Rings once at a host time on a thread of its own — never the evaluator's,
/// the hand's, a frame source's or a receipt reader's — so a run's deadline
/// lets go of what the run holds whatever else has stalled.
public protocol ReflexAlarm: Sendable {
    /// Ring `ring` once at `atNs` on the host clock (a time already past rings
    /// at once); the bell cancels a ring that has not come.
    func set(atNs: UInt64, ring: @escaping @Sendable () -> Void) -> any ReflexAlarmBell
}

public protocol ReflexAlarmBell: Sendable {
    func cancel()
}

/// The platform's alarm: a strict dispatch timer on a user-interactive queue of
/// its own, on the host uptime clock the hand keeps (`HostUptimeClock`).
public struct DispatchAlarm: ReflexAlarm {
    private static let queue = DispatchQueue(label: "dev.zerocode.computer-use.reflex-deadline", qos: .userInteractive)

    public init() {}

    public func set(atNs: UInt64, ring: @escaping @Sendable () -> Void) -> any ReflexAlarmBell {
        let timer = DispatchSource.makeTimerSource(flags: .strict, queue: Self.queue)
        let bell = DispatchBell(timer: timer)
        timer.schedule(deadline: DispatchTime(uptimeNanoseconds: atNs), leeway: .nanoseconds(0))
        timer.setEventHandler { [bell] in
            bell.cancel()
            ring()
        }
        timer.resume()
        return bell
    }
}

private final class DispatchBell: ReflexAlarmBell, @unchecked Sendable {
    private let lock = NSLock()
    private var timer: DispatchSourceTimer?

    init(timer: DispatchSourceTimer) { self.timer = timer }

    func cancel() {
        lock.lock()
        let timer = self.timer
        self.timer = nil
        lock.unlock()
        timer?.cancel()
    }
}

/// A run of one validated plan on the hand (realtime v1 §4): an evaluating
/// thread takes the newest frame, reads the kernel's sightings and picks a
/// rule; a hand thread runs its leaves. The run holds the hand from start to
/// end — no request posts in between — and pauses, letting go of what it
/// held, when anyone else's input is heard, the monitor stops hearing or the
/// hand takes its hold back. It ends at its one deadline, set when the helper
/// first accepted it, on an alarm of its own; when the receipts nobody
/// acknowledged fill their queue; and when a renewal finds the operator's
/// ledger no longer admitting — letting go of what it held first each time.
public final class ReflexSession: @unchecked Sendable {
    public enum State: Equatable, Sendable {
        case starting
        case running
        case paused(String)
        case stopped(String)
    }

    /// What a run is started with: the plan the helper validated, the two tables the
    /// window sent with it and the run's own policy, as they came — the run keeps no
    /// number of its own — and its one deadline on the host clock.
    public struct Settings: Sendable {
        public let runId: String
        public let plan: ValidatedReflexPlan
        public let limits: ReflexLimits
        public let perception: PerceptionLimits
        public let planEpoch: UInt64
        public let policy: ReflexRunPolicy
        public let deadlineNs: UInt64

        public init(runId: String, plan: ValidatedReflexPlan, limits: ReflexLimits, perception: PerceptionLimits, planEpoch: UInt64,
                    policy: ReflexRunPolicy, deadlineNs: UInt64) {
            self.runId = runId
            self.plan = plan
            self.limits = limits
            self.perception = perception
            self.planEpoch = planEpoch
            self.policy = policy
            self.deadlineNs = deadlineNs
        }
    }

    /// One detector's newest admissible word, as a status says it: a value or
    /// why it is unknown, the track it points at, and how old its frame is.
    public struct Sighting: Equatable, Sendable {
        public let detector: String
        public let value: Int64?
        public let unknown: ReflexUnknown?
        public let track: UInt64?
        public let ageNs: UInt64
    }

    /// Where the newest evaluated frame came from — the eye's stream and
    /// geometry, the run's hold on the hand and its plan: a reading of the run
    /// is about this scene, and a newer capture of it is the same scene.
    public struct Scene: Equatable, Sendable {
        public let stream: UInt64
        public let geometry: UInt64
        public let owner: UInt64
        public let plan: UInt64
    }

    public struct Status: Equatable, Sendable {
        public let state: State
        public let runId: String
        public let planHash: String
        public let planEpoch: UInt64
        public let policy: ReflexRunPolicy
        public let deadlineNs: UInt64
        public let framesEvaluated: UInt64
        public let framesUnread: UInt64
        /// Newer captures the run refused (not ready, time unknown or earlier,
        /// out of order) or the frames stopping: each took the evidence back.
        public let framesRefused: UInt64
        public let inadmissible: UInt64
        public let fires: UInt64
        public let renewals: UInt64
        public let leaves: UInt64
        /// Receipts kept and not yet acknowledged, and the last number issued
        /// and acknowledged (`ReflexReceipts`).
        public let receiptsPending: Int
        public let receiptsIssued: UInt64
        public let receiptsAcknowledged: UInt64
        /// How many leaves ended each way, by `ReflexReceipt.Outcome`.
        public let outcomes: [String: UInt64]
        /// The newest evaluated frame's admissible sightings, in plan order.
        public let sightings: [Sighting]
        public let scene: Scene?
        public let lastCapture: UInt64?
        public let lastCaptureAgeNs: UInt64?
        public let monitor: InputMonitorHealth
        public let echoes: UInt64
        public let othersHeard: UInt64
    }

    private let lock = NSLock()
    private let settings: Settings
    private let hand: OperatorHand
    private let source: any ReflexFrameSource
    private let kernel: any ReflexPerceptionKernel
    private let monitor: any ReflexInputMonitor
    private let admit: @Sendable () -> GuardAdmission
    /// The operator's ledger as it stands, read — never counted — when a
    /// spent quota would come back.
    private let standing: @Sendable () -> GuardAdmission
    private let fenceNs: UInt64
    private let boundary: any ReflexInputBoundary
    private let alarm: any ReflexAlarm
    /// Told once the run ends, whatever ended it, with its reason — after
    /// what it held was let go of.
    private let ended: (@Sendable (String) -> Void)?
    public let receipts: ReflexReceipts
    public let sightings = ReflexSightings()

    private var state: State = .starting
    private var token: OperatorHand.Token?
    private var bell: (any ReflexAlarmBell)?
    private var watch = InputWatch()
    private var pendingFire: ReflexRule?
    private var handBusy = false
    private var startFailure: (any Error)?
    private var framesEvaluated: UInt64 = 0
    private var framesUnread: UInt64 = 0
    private var framesRefused: UInt64 = 0
    private var inadmissible: UInt64 = 0
    private var fires: UInt64 = 0
    private var renewals: UInt64 = 0
    private var renewalGaps: [UInt64] = []
    private var leaves: UInt64 = 0
    private var outcomes: [String: UInt64] = [:]
    private let evaluate = DispatchSemaphore(value: 0)
    private let mail = DispatchSemaphore(value: 0)
    private let echo = DispatchSemaphore(value: 0)
    private let started = DispatchSemaphore(value: 0)

    public init(
        settings: Settings,
        hand: OperatorHand,
        source: any ReflexFrameSource,
        kernel: any ReflexPerceptionKernel,
        monitor: any ReflexInputMonitor,
        admit: @escaping @Sendable () -> GuardAdmission,
        standing: @escaping @Sendable () -> GuardAdmission,
        fenceNs: UInt64,
        boundary: any ReflexInputBoundary,
        alarm: any ReflexAlarm,
        ended: (@Sendable (String) -> Void)? = nil
    ) {
        self.settings = settings
        self.hand = hand
        self.source = source
        self.kernel = kernel
        self.monitor = monitor
        self.admit = admit
        self.standing = standing
        self.fenceNs = fenceNs
        self.boundary = boundary
        self.alarm = alarm
        self.ended = ended
        receipts = ReflexReceipts(capacity: Int(clamping: settings.limits.max_expanded_actions))
    }

    /// The run's deadline alarm, then: take the hand, prove the monitor hears,
    /// open the kernel's session and start both threads. Any failure lets go of
    /// the hand and says why; a stop that comes while it starts ends it — the
    /// start publishes nothing and gives the hand back
    /// (`ReflexRunError.stoppedWhileStarting`). A deadline that comes first is
    /// such a stop.
    public func start() throws {
        try ReflexTable.check(settings.limits)
        let style = try PointerStyle(settings.plan.plan.pointer, limits: settings.limits)
        let bell = alarm.set(atNs: settings.deadlineNs) { [weak self] in
            self?.finish(.stopped(ReflexRunReason.deadline))
        }
        lock.lock()
        self.bell = bell
        lock.unlock()
        if ending {
            bell.cancel()
            throw ReflexRunError.stoppedWhileStarting
        }
        let token = try hand.acquire(.reflex(settings.runId))
        lock.lock()
        if case .stopped = state {
            lock.unlock()
            hand.relinquish(token)
            throw ReflexRunError.stoppedWhileStarting
        }
        self.token = token
        lock.unlock()
        do {
            try proveMonitor(token)
            try startThreads(token: token, style: style)
        } catch {
            let stoppedMeanwhile = ending
            finish(.stopped(ReflexRunReason.startFailed))
            throw stoppedMeanwhile ? ReflexRunError.stoppedWhileStarting : error
        }
        lock.lock()
        let running = state == .starting
        if running { state = .running }
        lock.unlock()
        // A stop that came meanwhile has let go already (`finish`).
        if !running, ending { throw ReflexRunError.stoppedWhileStarting }
    }

    /// The monitor must hear the hand's own tagged echo — a move to where
    /// the pointer already is — within one lease length, or it cannot be
    /// trusted to hear anyone else's input.
    private func proveMonitor(_ token: OperatorHand.Token) throws {
        try monitor.start(
            heard: { [weak self] origin in self?.heard(origin) },
            interrupted: { [weak self] reason in self?.interrupted(reason) }
        )
        if ending { throw ReflexRunError.stoppedWhileStarting }
        guard let here = hand.pointerNow() else {
            throw ReflexRunError.monitorDeaf("the pointer's place is unknown, so no echo can be sent")
        }
        try hand.post(HandEvent(.pointerMove, x: here.x, y: here.y), by: token)
        let deadline = DispatchTime.now() + .nanoseconds(Int(clamping: settings.limits.max_lease_ns))
        while true {
            if ending { throw ReflexRunError.stoppedWhileStarting }
            lock.lock()
            let health = watch.health
            lock.unlock()
            switch health {
            case .hearing: return
            case let .unavailable(reason), let .interrupted(reason): throw ReflexRunError.monitorDeaf(reason)
            case .unproven: break
            }
            if echo.wait(timeout: deadline) == .timedOut {
                lock.lock()
                watch.unavailable("no echo within the lease length")
                lock.unlock()
                throw ReflexRunError.monitorDeaf("the monitor did not hear the hand's own echo")
            }
        }
    }

    /// The class a run's two threads run at: user-interactive, the one R5's
    /// ABBA measured a tick's tail under 5 ms at (and the hand's events go out
    /// on the second).
    public static let threadQuality: QualityOfService = .userInteractive

    private func startThreads(token: OperatorHand.Token, style: PointerStyle) throws {
        let settings = self.settings
        let kernel = self.kernel
        let evaluator = Thread { [self] in
            let perception: any ReflexPerceptionSession
            do {
                perception = try kernel.session(for: settings.plan, limits: settings.perception)
            } catch {
                lock.lock()
                startFailure = error
                lock.unlock()
                started.signal()
                return
            }
            started.signal()
            evaluateFrames(perception, token: token)
        }
        evaluator.qualityOfService = Self.threadQuality
        evaluator.name = "reflex evaluator"
        evaluator.start()
        started.wait()
        lock.lock()
        let failure = startFailure
        lock.unlock()
        if let failure { throw failure }
        // Wired only while the run stands: a stop that ends it under the same
        // lock either comes first — nothing is wired — or clears what was.
        lock.lock()
        let stopped: Bool
        if case .stopped = state { stopped = true } else { stopped = false }
        if !stopped {
            source.onCapture { [weak self] in
                self?.evaluate.signal()
                self?.hand.wake()
            }
        }
        lock.unlock()
        if stopped { throw ReflexRunError.stoppedWhileStarting }
        let leaves = Thread { [self] in
            runLeaves(token: token, style: style)
        }
        leaves.qualityOfService = Self.threadQuality
        leaves.name = "reflex hand"
        leaves.start()
        evaluate.signal()
    }

    /// End the run: the hand's hold is taken back and returned, and what it
    /// held is let go of here, on the calling thread, before anything else.
    public func stop(reason: String) {
        finish(.stopped(reason))
    }

    /// The run stops acting but keeps the hand: what it held is let go of now.
    public func pause(reason: String) {
        lock.lock()
        guard state == .running || state == .starting else {
            lock.unlock()
            return
        }
        state = .paused(reason)
        let token = self.token
        lock.unlock()
        if let token { hand.revoke(token, reason: reason) }
    }

    /// The release first, on this thread; the deadline's alarm, the capture and
    /// the monitor are torn down after it, so a teardown that hangs never holds
    /// a release (realtime v1 §5.8). Whoever started the run hears it ended.
    private func finish(_ ending: State) {
        lock.lock()
        if case .stopped = state {
            lock.unlock()
            return
        }
        state = ending
        let token = self.token
        let bell = self.bell
        lock.unlock()
        if let token {
            if case let .stopped(reason) = ending { hand.revoke(token, reason: reason) }
            hand.relinquish(token)
        }
        bell?.cancel()
        evaluate.signal()
        mail.signal()
        echo.signal()
        source.onCapture(nil)
        monitor.stop()
        if case let .stopped(reason) = ending { self.ended?(reason) }
    }

    private func heard(_ origin: InputOrigin) {
        lock.lock()
        let wasUnproven = watch.health == .unproven
        let pausing = watch.heard(origin)
        let nowHearing = wasUnproven && watch.health == .hearing
        lock.unlock()
        if nowHearing { echo.signal() }
        if pausing { pause(reason: ReflexRunReason.externalInput) }
    }

    private func interrupted(_ reason: String) {
        lock.lock()
        watch.interrupted(reason)
        lock.unlock()
        echo.signal()
        pause(reason: ReflexRunReason.monitorInterrupted)
    }

    private var ending: Bool {
        lock.lock()
        defer { lock.unlock() }
        if case .stopped = state { return true }
        return false
    }

    private var acting: Bool {
        lock.lock()
        defer { lock.unlock() }
        return state == .running && watch.permitsActing
    }

    /// Whether the run may act at `now`: running, hearing, and before its deadline.
    private func acting(at now: UInt64) -> Bool {
        now < settings.deadlineNs && acting
    }

    // MARK: the evaluating thread

    /// The newest frame, read once: the same capture taken again is no news;
    /// a newer one the run refuses — not ready, its capture time unknown or
    /// before the last one's, out of order — or the frames stopping take back
    /// what was seen before it (`ReflexSightings.revoke`), and wake the hand
    /// to see it. Unread between captures, the run still looks again once a
    /// frame's age has passed, so a source that went quiet is noticed.
    private func evaluateFrames(_ perception: any ReflexPerceptionSession, token: OperatorHand.Token) {
        var cursor = ReflexFrameCursor()
        var book = ReflexRuleBook(settings.plan, policy: settings.policy)
        let detectors = Dictionary(settings.plan.plan.detectors.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        let clock: @Sendable () -> UInt64 = { [hand] in hand.nowNs() }
        let look = DispatchTimeInterval.nanoseconds(Int(clamping: settings.limits.max_frame_age_ns))
        var lastRead: (stream: UInt64, capture: UInt64)?
        var lastAcceptedNs: UInt64?
        while true {
            _ = evaluate.wait(timeout: .now() + look)
            if ending { return }
            guard let loan = source.newest() else {
                if sightings.latest() != nil { refuseFrame() }
                continue
            }
            let facts = loan.capture.facts(runId: settings.runId, ownerEpoch: token.id, planEpoch: settings.planEpoch)
            if let lastRead, lastRead == (facts.stream_epoch, facts.capture_seq) { continue }
            lastRead = (facts.stream_epoch, facts.capture_seq)
            guard facts.captured_host_ns.map({ at in lastAcceptedNs.map { at >= $0 } ?? true }) == true,
                  cursor.observe(facts)
            else {
                refuseFrame()
                continue
            }
            lastAcceptedNs = facts.captured_host_ns
            var budget = ReflexPerceptionBudget(
                samples: settings.perception.max_tick_samples,
                deadlineHostNs: hand.nowNs() &+ settings.perception.max_tick_ns,
                now: clock
            )
            var seen: [ReflexObservation] = []
            let read = loan.withPixels { pixels in
                seen = perception.observe(frame: facts, pixels: pixels, budget: &budget)
            }
            var byDetector: [String: ReflexObservation] = [:]
            var refused: UInt64 = 0
            // A kernel that read more than the tick could pay for broke its budget: none of
            // what it answered for this frame is read.
            let spent = seen.reduce(UInt64(0)) { $0 &+ $1.samples }
            let honest = spent <= settings.perception.max_tick_samples
            for sighting in seen {
                guard honest,
                      let detector = detectors[sighting.detector_id],
                      byDetector[sighting.detector_id] == nil,
                      sighting.admissible(for: detector, frame: facts, budgetSamples: settings.perception.max_detector_samples)
                else {
                    refused += 1
                    continue
                }
                byDetector[sighting.detector_id] = sighting
            }
            sightings.publish(ReflexSightings.Seen(frame: facts, byDetector: byDetector))
            hand.wake()
            let values = byDetector.compactMapValues(\.value)
            let now = hand.nowNs()
            lock.lock()
            framesEvaluated += 1
            if !read { framesUnread += 1 }
            inadmissible += refused
            let free = !handBusy && pendingFire == nil && state == .running && watch.permitsActing && now < settings.deadlineNs
            lock.unlock()
            let outcome = book.read(streamEpoch: facts.stream_epoch, captureSeq: facts.capture_seq, values: values, nowNs: now,
                                    handFree: free, renewal: { mayRenew(token) })
            lock.lock()
            renewals += UInt64(outcome.renewed.count)
            renewalGaps.append(contentsOf: outcome.renewed.map(\.spentForNs))
            if let rule = outcome.fired {
                pendingFire = rule
                fires += 1
            }
            lock.unlock()
            if outcome.fired != nil { mail.signal() }
        }
    }

    /// Whether spent quotas may come back now (`ReflexRuleBook`): the run acts,
    /// still holds the hand and is before its deadline, and the operator's
    /// ledger — read, never counted — still admits. A renewal changes nothing
    /// else: each new leaf is admitted once like any action, and the session
    /// count, a stop and a release left unconfirmed stand. A ledger that no
    /// longer admits ends the run: a renewal that fails grants nothing more.
    private func mayRenew(_ token: OperatorHand.Token) -> Bool {
        guard acting(at: hand.nowNs()), hand.refusal(for: token) == nil else { return false }
        switch standing() {
        case .admitted, .wait:
            // A stop, a pause or the deadline that came while the ledger was read wins.
            return acting(at: hand.nowNs())
        case let .stopped(reason):
            finish(.stopped(reason))
        case .sessionBudget:
            finish(.stopped(StopReason.sessionBudget))
        case .noBudget:
            finish(.stopped(ReflexRunReason.noGuardTable))
        }
        return false
    }

    /// A newer capture refused, or the frames stopped: nothing seen before is
    /// evidence any more, and the hand hears it now.
    private func refuseFrame() {
        sightings.revoke()
        lock.lock()
        framesRefused += 1
        lock.unlock()
        hand.wake()
    }

    // MARK: the hand thread

    private func runLeaves(token: OperatorHand.Token, style: PointerStyle) {
        let runner = ReflexLeafRunner(
            runId: settings.runId, hand: hand, token: token, limits: settings.limits, style: style,
            sightings: sightings, admit: admit, fenceNs: fenceNs, boundary: boundary, deadlineNs: settings.deadlineNs
        )
        var index: UInt64 = 0
        while true {
            mail.wait()
            if ending { return }
            lock.lock()
            let rule = pendingFire
            pendingFire = nil
            if rule != nil { handBusy = true }
            lock.unlock()
            guard let rule else { continue }
            let ran = Self.runFire(
                ReflexMacros.leaves(ruleId: rule.id, macroId: rule.macro_id, in: settings.plan.plan),
                receipts: receipts,
                acting: { acting(at: hand.nowNs()) },
                overflowed: { finish(.stopped(ReflexRunReason.overflow)) },
                next: &index,
                run: { [self] leaf, at in
                    let receipt = runner.run(leaf, index: at)
                    lock.lock()
                    outcomes[receipt.outcome.rawValue, default: 0] += 1
                    lock.unlock()
                    return receipt
                }
            )
            lock.lock()
            leaves += UInt64(ran)
            handBusy = false
            lock.unlock()
            pauseIfTheHandTookTheHoldBack(token)
        }
    }

    /// The hand took the run's hold back by itself — a release the platform
    /// refused (`HoldRevocation`) — or the operator stopped: the run stops
    /// acting and says the hand's reason, so no rule fires on a hold that
    /// presses nothing.
    private func pauseIfTheHandTookTheHoldBack(_ token: OperatorHand.Token) {
        switch hand.refusal(for: token) {
        case let .revoked(reason)?, let .stopped(reason)?:
            pause(reason: reason)
        case .releaseUnconfirmed?:
            pause(reason: HoldRevocation.releaseUnconfirmed)
        case nil, .notHolder?, .busy?:
            return
        }
    }

    /// One fire's leaves on the hand, in order: a receipt offered after each
    /// — never a wait on whoever reads them — and no leaf started once the run
    /// stops acting or a leaf did not finish. A queue the reader left full
    /// ends the run (`overflowed`) before the next leaf starts: nothing acts
    /// without a place to keep what it did, and freeing a place later does not
    /// bring the run back. Answers how many leaves ran.
    static func runFire(
        _ fire: [ReflexLeaf],
        receipts: ReflexReceipts,
        acting: () -> Bool,
        overflowed: () -> Void,
        next index: inout UInt64,
        run: (ReflexLeaf, UInt64) -> ReflexReceipt
    ) -> Int {
        var ran = 0
        for leaf in fire {
            guard acting() else { break }
            guard receipts.hasRoom else {
                overflowed()
                break
            }
            let receipt = run(leaf, index)
            index += 1
            ran += 1
            receipts.offer(receipt)
            if receipt.outcome != .done { break }
        }
        return ran
    }

    /// How long each spent quota waited to come back, in order.
    public var renewalGapsNs: [UInt64] {
        lock.lock()
        defer { lock.unlock() }
        return renewalGaps
    }

    public var status: Status {
        let latest = sightings.latest()
        let now = hand.nowNs()
        let numbers = receipts.numbers
        let pending = receipts.pending
        let order = settings.plan.plan.detectors.map(\.id)
        let age = latest?.frame.captured_host_ns.map { now >= $0 ? now - $0 : 0 } ?? 0
        let seen: [Sighting] = order.compactMap { id in
            guard let sighting = latest?.byDetector[id] else { return nil }
            return Sighting(detector: id, value: sighting.value, unknown: sighting.unknown, track: sighting.target?.track_id, ageNs: age)
        }
        lock.lock()
        defer { lock.unlock() }
        return Status(
            state: state,
            runId: settings.runId,
            planHash: settings.plan.plan.plan_hash,
            planEpoch: settings.planEpoch,
            policy: settings.policy,
            deadlineNs: settings.deadlineNs,
            framesEvaluated: framesEvaluated,
            framesUnread: framesUnread,
            framesRefused: framesRefused,
            inadmissible: inadmissible,
            fires: fires,
            renewals: renewals,
            leaves: leaves,
            receiptsPending: pending,
            receiptsIssued: numbers.issued,
            receiptsAcknowledged: numbers.acknowledged,
            outcomes: outcomes,
            sightings: seen,
            scene: latest.map { Scene(stream: $0.frame.stream_epoch, geometry: $0.frame.geometry_epoch,
                                      owner: $0.frame.owner_epoch, plan: $0.frame.plan_epoch) },
            lastCapture: latest?.frame.capture_seq,
            lastCaptureAgeNs: latest?.frame.captured_host_ns.map { now >= $0 ? now - $0 : 0 },
            monitor: watch.health,
            echoes: watch.echoes,
            othersHeard: watch.others
        )
    }
}

/// Why a run could not start.
public enum ReflexRunError: Error, Equatable, Sendable {
    /// The input monitor cannot be trusted to hear a person's input.
    case monitorDeaf(String)
    /// It was stopped before it stood: it gave the hand back and runs nothing.
    case stoppedWhileStarting
}
