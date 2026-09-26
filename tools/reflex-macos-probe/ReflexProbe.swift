// The macOS reflex runtime's own clock, measured in an optimized build (t-6765,
// t-9205), and judged by an oracle of its own.
//
// Built together with the helper's Core sources (see README.md), so what runs
// is the product's `OperatorHand`, `ReflexSession`, rule book, leases, receipts
// and — with `--kernel perception` — R5's `PerceptionKernel` reading drawn BGRA
// pixels; only the frame source, the monitor, the poster, the receipt collector
// and, by default, a scripted kernel are the probe's. Nothing reaches the
// system's input: the poster hands each event to a synthetic fixture instead,
// and records when it would have been handed to the window server. So this
// measures the runtime's own latency (a frame's publish to the first event it
// decides, a stop to the release it posts), never an app receiving input — and
// it fails a run that did nothing or did the wrong thing, whatever the numbers
// say.
import Darwin
import Foundation

// MARK: - Clock and statistics

private func nowNs() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

/// Nearest-rank percentile of `values` (0 < p <= 100); nil when empty.
func nearestRank(_ values: [UInt64], _ p: Double) -> UInt64? {
    guard !values.isEmpty else { return nil }
    let sorted = values.sorted()
    let rank = Int((p / 100 * Double(sorted.count)).rounded(.up))
    return sorted[min(max(rank, 1), sorted.count) - 1]
}

private func summary(_ values: [UInt64]) -> [String: Any] {
    func ms(_ ns: UInt64?) -> Any { ns.map { Double($0) / 1_000_000 } ?? NSNull() }
    return ["n": values.count, "p50Ms": ms(nearestRank(values, 50)), "p95Ms": ms(nearestRank(values, 95)), "maxMs": ms(values.max())]
}

private func loadAverages() -> [Double] {
    var loads = [Double](repeating: 0, count: 3)
    return getloadavg(&loads, 3) == 3 ? loads : []
}

private func cpuSeconds() -> Double {
    var usage = rusage()
    getrusage(RUSAGE_SELF, &usage)
    return Double(usage.ru_utime.tv_sec) + Double(usage.ru_utime.tv_usec) / 1e6
        + Double(usage.ru_stime.tv_sec) + Double(usage.ru_stime.tv_usec) / 1e6
}

/// SplitMix64: the stimulus schedule from a seed, independent of anything the
/// run does.
struct SeededGenerator: RandomNumberGenerator {
    private var state: UInt64
    init(seed: UInt64) { state = seed }
    mutating func next() -> UInt64 {
        state &+= 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }
}

// MARK: - What can go wrong, on purpose

/// A run the oracle must fail — each made on purpose in the probe's own
/// parts, never in the product's code.
enum Fault: String, CaseIterable {
    /// The frame source lends its pixels without calling the kernel: nothing
    /// is ever seen (the first probe run's own bug).
    case kernelUncalled = "kernel-uncalled"
    /// The poster takes every event and delivers none to the fixture — the
    /// input monitor still hears the hand's echo, so the run starts.
    case inertPoster = "inert-poster"
    /// The scripted kernel answers the ball in the other corner from where it is shown.
    case wrongTarget = "wrong-target"
    /// The fixture keeps what an earlier run left and ignores being cleared,
    /// as an app that restores its last state does.
    case restoredState = "restored-state"
    /// The real kernel reads frames whose pixels never show the ball the
    /// scene says is there: a kernel that is called but sees nothing.
    case kernelBlind = "kernel-blind"

    /// The faults a kernel can meet: the scripted one lies about the target, the
    /// real one reads what the pixels show.
    func applies(to kernel: KernelChoice) -> Bool {
        switch self {
        case .wrongTarget: return kernel == .scene
        case .kernelBlind: return kernel == .perception
        case .kernelUncalled, .inertPoster, .restoredState: return true
        }
    }
}

/// Who reads the frames: the probe's scripted ball, or R5's kernel on drawn pixels.
enum KernelChoice: String {
    case scene
    case perception
}

// MARK: - The fixture the hand acts on

/// The synthetic fixture's scene: where the ball is shown on each capture —
/// the ground truth the oracle holds a run to, apart from anything the kernel
/// answers. Each cycle shows the ball for its `present` captures and hides it
/// for its `absent` — an edge each cycle — at the cycle's own place in the ROI.
struct BallScene: Sendable {
    struct Cycle: Sendable {
        let first: UInt64
        let present: UInt64
        let absent: UInt64
        let box: ReflexRoi
    }

    let cycles: [Cycle]

    /// Cycles of the same length, the ball alternating between two corners.
    static func fixed(present: UInt64, absent: UInt64, captures: UInt64) -> BallScene {
        var cycles: [Cycle] = []
        var first: UInt64 = 0
        while first <= captures {
            let corner: Int64 = cycles.count % 2 == 0 ? 0 : 18
            cycles.append(Cycle(first: first, present: present, absent: absent,
                                box: ReflexRoi(x: corner, y: corner, width: 12, height: 12, space: .pixel)))
            first += present + absent
        }
        return BallScene(cycles: cycles)
    }

    /// Cycles drawn from `seed`: each one's lengths inside the ranges and its
    /// place anywhere the 12-pixel ball fits in valid_basic's 32-pixel ROI.
    static func seeded(_ seed: UInt64, present: ClosedRange<UInt64>, absent: ClosedRange<UInt64>, captures: UInt64) -> BallScene {
        var generator = SeededGenerator(seed: seed)
        var cycles: [Cycle] = []
        var first: UInt64 = 0
        while first <= captures {
            let shown = UInt64.random(in: present, using: &generator)
            let hidden = UInt64.random(in: absent, using: &generator)
            let x = Int64.random(in: 0...20, using: &generator)
            let y = Int64.random(in: 0...20, using: &generator)
            cycles.append(Cycle(first: first, present: shown, absent: hidden, box: ReflexRoi(x: x, y: y, width: 12, height: 12, space: .pixel)))
            first += shown + hidden
        }
        return BallScene(cycles: cycles)
    }

    private func cycle(of seq: UInt64) -> (index: Int, cycle: Cycle)? {
        var low = 0
        var high = cycles.count - 1
        guard high >= 0, seq >= cycles[0].first else { return nil }
        while low < high {
            let middle = (low + high + 1) / 2
            if cycles[middle].first <= seq { low = middle } else { high = middle - 1 }
        }
        return (low, cycles[low])
    }

    /// The ball's hitbox on capture `seq`, in frame pixels; nil when hidden.
    func ball(on seq: UInt64) -> ReflexRoi? {
        guard let (_, cycle) = cycle(of: seq), seq - cycle.first < cycle.present else { return nil }
        return cycle.box
    }

    /// The ball's track: one a cycle.
    func track(on seq: UInt64) -> UInt64 { UInt64(cycle(of: seq)?.index ?? 0) + 1 }

    /// How many times the ball came on among captures `1...seq`.
    func appearances(through seq: UInt64) -> UInt64 {
        UInt64(cycles.filter { $0.first + 1 <= seq && $0.present > 0 }.count)
    }
}

/// The app the fixture stands for: every event the poster delivered to it,
/// the buttons it holds, and each click — down, then up — judged against
/// where the ball was shown on a capture no older than the reflex table's
/// frame age when the button went down. Its record is cleared for each run
/// and read clean before the run starts.
final class SyntheticDesk: @unchecked Sendable {
    struct Record: Sendable {
        var events: UInt64 = 0
        var downs: UInt64 = 0
        var ups: UInt64 = 0
        var hits: UInt64 = 0
        var misses: UInt64 = 0
        /// Hits recorded by another run than the one clearing it.
        var foreignHits: UInt64 = 0
        var held: Set<String> = []
    }

    private let lock = NSLock()
    private let scene: BallScene
    private let frames: PacedFrames
    private let maxFrameAgeNs: UInt64
    /// Keeps what the last run left, whatever `clear` says (`Fault.restoredState`).
    private let restores: Bool
    private var record = Record()
    private var down: (x: Double, y: Double, atNs: UInt64)?

    init(scene: BallScene, frames: PacedFrames, maxFrameAgeNs: UInt64, restores: Bool) {
        self.scene = scene
        self.frames = frames
        self.maxFrameAgeNs = maxFrameAgeNs
        self.restores = restores
        if restores {
            // What an earlier run left: its hits, recorded as that run's.
            record.hits = 3
            record.foreignHits = 3
            record.events = 40
        }
    }

    /// A new run begins: nothing received, nothing held.
    func clear() {
        lock.lock()
        if !restores {
            record = Record()
            down = nil
        }
        lock.unlock()
    }

    var snapshot: Record {
        lock.lock()
        defer { lock.unlock() }
        return record
    }

    func receive(_ event: HandEvent, atNs: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        record.events += 1
        switch event.kind {
        case let .buttonDown(button, _):
            record.downs += 1
            record.held.insert("\(button)")
            down = (event.x, event.y, atNs)
        case let .buttonUp(button, _):
            record.ups += 1
            record.held.remove("\(button)")
            guard let press = down else { return }
            down = nil
            // One point a pixel at the fixture's origin: the point is the pixel.
            let shown = frames.shown(fromNs: press.atNs &- maxFrameAgeNs, toNs: press.atNs)
            let onBall = shown.contains { seq in scene.ball(on: seq)?.contains(x: press.x, y: press.y) == true }
            if onBall { record.hits += 1 } else { record.misses += 1 }
        default:
            break
        }
    }
}

// MARK: - The probe's poster, frames, kernels, monitor and collector

/// Hands each event to the fixture — unless inert — and records when it
/// would have been posted; the pointer stays where the hand put it. Nothing
/// reaches the system.
final class DeskPoster: HandPoster, @unchecked Sendable {
    private let lock = NSLock()
    private let desk: SyntheticDesk?
    private let inert: Bool
    private var location = SmoothPointerPath.Point(x: 0, y: 0)
    private var releases: [UInt64] = []
    var heard: (@Sendable (Int64) -> Void)?

    init(desk: SyntheticDesk?, inert: Bool) {
        self.desk = desk
        self.inert = inert
    }

    func post(_ event: HandEvent, tag: Int64) throws {
        let at = nowNs()
        lock.lock()
        if event.points { location = SmoothPointerPath.Point(x: event.x, y: event.y) }
        if event.letsGo != nil { releases.append(at) }
        let heard = self.heard
        lock.unlock()
        if !inert { desk?.receive(event, atNs: at) }
        heard?(tag)
    }

    func pointerLocation() -> SmoothPointerPath.Point? {
        lock.lock()
        defer { lock.unlock() }
        return location
    }

    func releaseTimes() -> [UInt64] {
        lock.lock()
        defer { lock.unlock() }
        return releases
    }
}

/// One capture's BGRA pixels at the probe's extent: black ground and, when drawn,
/// the ball pure red — what R5's kernel reads. Freed when the last loan of it ends.
final class DrawnBuffer: @unchecked Sendable {
    static let width = 800
    static let height = 500
    let base: UnsafeMutableRawPointer

    init(ball: ReflexRoi?) {
        base = calloc(Self.width * Self.height * 4, 1)!
        guard let ball else { return }
        for y in max(0, Int(ball.y))..<min(Self.height, Int(ball.y + ball.height)) {
            for x in max(0, Int(ball.x))..<min(Self.width, Int(ball.x + ball.width)) {
                let at = (y * Self.width + x) * 4
                base.storeBytes(of: 0xFF, toByteOffset: at + 2, as: UInt8.self)
                base.storeBytes(of: 0xFF, toByteOffset: at + 3, as: UInt8.self)
            }
        }
    }

    deinit { free(base) }

    var pixels: ReflexPixels {
        ReflexPixels(base: UnsafeRawPointer(base), width: Self.width, height: Self.height, bytesPerRow: Self.width * 4)
    }
}

/// Captures at a fixed rate on the host clock, each published and woken the
/// way the eye's callback publishes one, and kept a little while so the
/// fixture can say what was shown when. For the scripted kernel every capture
/// lends one blank buffer; for the real one each capture's own drawing — the
/// ball where the scene shows it, or never with `Fault.kernelBlind`. With
/// `Fault.kernelUncalled` it says it lent the pixels without calling the reader.
final class PacedFrames: ReflexFrameSource, @unchecked Sendable {
    private let lock = NSLock()
    private var capture: ReflexCapture?
    private var drawing: DrawnBuffer?
    private var wake: (@Sendable () -> Void)?
    private var running = true
    private var history: [(seq: UInt64, atNs: UInt64)] = []
    private let lendsPixels: Bool
    private let scene: BallScene
    private let draws: Bool
    private let showsBall: Bool
    private let blank = DrawnBuffer(ball: nil)
    let periodNs: UInt64

    init(framesPerSecond: UInt64, scene: BallScene, lendsPixels: Bool, draws: Bool, showsBall: Bool) {
        periodNs = 1_000_000_000 / framesPerSecond
        self.scene = scene
        self.lendsPixels = lendsPixels
        self.draws = draws
        self.showsBall = showsBall
    }

    func start() {
        Thread.detachNewThread { [self] in
            var seq: UInt64 = 0
            var due = nowNs()
            while true {
                lock.lock()
                let go = running
                lock.unlock()
                if !go { return }
                seq += 1
                let drawn = draws ? DrawnBuffer(ball: showsBall ? scene.ball(on: seq) : nil) : nil
                let at = nowNs()
                let next = ReflexCapture(
                    displayId: "probe",
                    region: ReflexRoi(x: 0, y: 0, width: Int64(DrawnBuffer.width), height: Int64(DrawnBuffer.height), space: .pixel),
                    pixelExtent: ReflexPixelExtent(width: UInt32(DrawnBuffer.width), height: UInt32(DrawnBuffer.height)),
                    pointTransform: ReflexPointTransform(origin_x: 0, origin_y: 0, points_per_pixel: ReflexScale(numerator: 1, denominator: 1)),
                    orientation: .up, colorSpace: .srgb, status: .ready, dirty: true, captureGap: 0,
                    deliveredHostNs: at, captureSeq: seq, repaintSeq: seq, streamEpoch: 1, geometryEpoch: 1,
                    clockDomain: ReflexContract.hostUptimeClockDomain, capturedHostNs: at
                )
                lock.lock()
                capture = next
                drawing = drawn
                history.append((seq, at))
                if history.count > 64 { history.removeFirst(history.count - 64) }
                let wake = self.wake
                lock.unlock()
                wake?()
                due &+= periodNs
                let wait = due > nowNs() ? due - nowNs() : 0
                if wait > 0 { usleep(UInt32(wait / 1_000)) }
            }
        }
    }

    func stop() {
        lock.lock()
        running = false
        lock.unlock()
    }

    /// The captures published between `fromNs` and `toNs`, and the one on
    /// screen when that span began.
    func shown(fromNs: UInt64, toNs: UInt64) -> [UInt64] {
        lock.lock()
        defer { lock.unlock() }
        let before = history.last { $0.atNs <= fromNs }.map { [$0.seq] } ?? []
        return before + history.filter { $0.atNs > fromNs && $0.atNs <= toNs }.map(\.seq)
    }

    /// The newest capture published.
    var newestSeq: UInt64 {
        lock.lock()
        defer { lock.unlock() }
        return capture?.captureSeq ?? 0
    }

    func newest() -> ReflexFrameLoan? {
        lock.lock()
        defer { lock.unlock() }
        let buffer = drawing ?? blank
        let lends = lendsPixels
        return capture.map { ReflexFrameLoan(capture: $0) { body in
            if lends { body(buffer.pixels) }
            return withExtendedLifetime(buffer) { true }
        } }
    }

    func onCapture(_ wake: (@Sendable () -> Void)?) {
        lock.lock()
        self.wake = wake
        lock.unlock()
    }
}

/// What a kernel read, for the oracle: how often it was asked, and whether it
/// named the ball on a capture that showed it and nothing on one that did not.
final class KernelLedger: @unchecked Sendable {
    private let lock = NSLock()
    private(set) var calls: UInt64 = 0
    private(set) var sawBall: UInt64 = 0
    private(set) var sawAbsence: UInt64 = 0

    func note(_ observations: [ReflexObservation], scene: BallScene, seq: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        calls += 1
        let shown = scene.ball(on: seq) != nil
        for observation in observations {
            if shown, let value = observation.value, value > 0, observation.target != nil { sawBall += 1 }
            if !shown, observation.value == 0 { sawAbsence += 1 }
        }
    }

    var counts: (calls: UInt64, sawBall: UInt64, sawAbsence: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        return (calls, sawBall, sawAbsence)
    }
}

/// Reads the scene on each capture — the probe's stand-in for perception.
/// With `Fault.wrongTarget` it answers the ball in the other corner from where
/// it is shown.
final class BlinkingBall: ReflexPerceptionKernel, @unchecked Sendable {
    let scene: BallScene
    let lies: Bool
    let ledger: KernelLedger

    init(scene: BallScene, lies: Bool, ledger: KernelLedger) {
        self.scene = scene
        self.lies = lies
        self.ledger = ledger
    }

    final class Session: ReflexPerceptionSession {
        let kernel: BlinkingBall
        init(kernel: BlinkingBall) { self.kernel = kernel }

        func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, hand: (x: Int64, y: Int64)?,
                     budget: inout ReflexPerceptionBudget) -> [ReflexObservation] {
            guard budget.spend(64), let ref = ReflexFrameRef(frame) else {
                kernel.ledger.note([], scene: kernel.scene, seq: frame.capture_seq)
                return []
            }
            let shown = kernel.scene.ball(on: frame.capture_seq)
            let box = shown.map { kernel.lies ? ReflexRoi(x: 20 - $0.x, y: 20 - $0.y, width: $0.width, height: $0.height, space: .pixel) : $0 }
            let seen = [ReflexObservation(
                detector_id: "ball", frame: ref, unknown: nil, value: box == nil ? 0 : 1,
                target: box.map { ReflexTarget(track_id: kernel.scene.track(on: frame.capture_seq), roi: $0, point_x: $0.x + 6, point_y: $0.y + 6,
                                              velocity_x: 0, velocity_y: 0, uncertainty: 1) },
                scale: ReflexScale(numerator: 1, denominator: 1), cells: nil, samples: 64
            )]
            kernel.ledger.note(seen, scene: kernel.scene, seq: frame.capture_seq)
            return seen
        }
    }

    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession {
        Session(kernel: self)
    }
}

/// R5's kernel as the helper runs it, its every answer noted for the oracle.
final class NotedKernel: ReflexPerceptionKernel, @unchecked Sendable {
    let inner = PerceptionKernel()
    let scene: BallScene
    let ledger: KernelLedger

    init(scene: BallScene, ledger: KernelLedger) {
        self.scene = scene
        self.ledger = ledger
    }

    final class Session: ReflexPerceptionSession {
        let inner: any ReflexPerceptionSession
        let kernel: NotedKernel
        init(inner: any ReflexPerceptionSession, kernel: NotedKernel) {
            self.inner = inner
            self.kernel = kernel
        }

        func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, hand: (x: Int64, y: Int64)?,
                     budget: inout ReflexPerceptionBudget) -> [ReflexObservation] {
            let seen = inner.observe(frame: frame, pixels: pixels, hand: hand, budget: &budget)
            kernel.ledger.note(seen, scene: kernel.scene, seq: frame.capture_seq)
            return seen
        }
    }

    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession {
        Session(inner: try inner.session(for: plan, limits: limits), kernel: self)
    }
}

/// Hears what the poster posts, with the stamp it was posted with, on its own
/// queue as a tap does.
final class EchoingMonitor: ReflexInputMonitor, @unchecked Sendable {
    private let lock = NSLock()
    private let queue = DispatchQueue(label: "probe.monitor")
    private var heard: (@Sendable (InputOrigin) -> Void)?
    private let handTag: Int64

    init(poster: DeskPoster, handTag: Int64) {
        self.handTag = handTag
        poster.heard = { [weak self] tag in self?.deliver(tag) }
    }

    func start(heard: @escaping @Sendable (InputOrigin) -> Void, interrupted: @escaping @Sendable (String) -> Void) throws {
        lock.lock()
        self.heard = heard
        lock.unlock()
    }

    func stop() {
        lock.lock()
        heard = nil
        lock.unlock()
    }

    private func deliver(_ tag: Int64) {
        lock.lock()
        let heard = self.heard
        lock.unlock()
        let me = Int64(getpid())
        let handTag = self.handTag
        queue.async { heard?(InputOrigin.of(userData: tag, sourcePid: me, handTag: handTag, handPid: me)) }
    }
}

/// Every point of the synthetic fixture is the run's to act on: the probe
/// has no windows, and no event reaches the system.
struct WholeFixture: ReflexInputBoundary {
    func refusal(_ input: ReflexLeaseInput, at point: SmoothPointerPath.Point) -> String? { nil }
}

/// Reads a run's receipts after the last it holds and acknowledges them, on a
/// thread of its own at `everyNs` — the window's collector's road, minus the disk.
final class ReceiptCollector: @unchecked Sendable {
    private let lock = NSLock()
    private var held: [ReflexReceipts.Entry] = []
    private var running = true
    private let done = DispatchSemaphore(value: 0)

    init(_ receipts: ReflexReceipts, everyNs: UInt64) {
        Thread.detachNewThread { [self] in
            while true {
                let fresh = receipts.read(after: last, limit: receipts.capacity)
                if let seq = fresh.last?.seq {
                    lock.lock()
                    held += fresh
                    lock.unlock()
                    receipts.acknowledge(through: seq)
                }
                lock.lock()
                let go = running
                lock.unlock()
                if !go {
                    done.signal()
                    return
                }
                usleep(UInt32(everyNs / 1_000))
            }
        }
    }

    private var last: UInt64 {
        lock.lock()
        defer { lock.unlock() }
        return held.last?.seq ?? 0
    }

    /// Stop after one more read; every receipt read, in order.
    func finish() -> [ReflexReceipt] {
        lock.lock()
        running = false
        lock.unlock()
        done.wait()
        lock.lock()
        defer { lock.unlock() }
        return held.map(\.receipt)
    }
}

// MARK: - The plan and tables, from the shared fixtures

struct Fixtures {
    let root: URL

    func limits() throws -> ReflexLimits {
        try ReflexContract.decodeLimits(Data(try Data(contentsOf: root.appendingPathComponent("reflex-contract/limits.json")).dropLast()))
    }

    func perception() throws -> PerceptionLimits {
        try PerceptionSpecs.decodeLimits(Data(try Data(contentsOf: root.appendingPathComponent("game-state/limits.json")).dropLast()))
    }

    /// valid_basic, its rule firing on the ball being there `maxFires` times a
    /// quota, changed by `change`.
    func plan(maxFires: Int, _ change: (inout [String: Any]) -> Void = { _ in }) throws -> ValidatedReflexPlan {
        let raw = try Data(contentsOf: root.appendingPathComponent("reflex-contract/valid_basic.json"))
        guard let envelope = try JSONSerialization.jsonObject(with: raw) as? [String: Any],
              var object = envelope["plan"] as? [String: Any],
              var rules = object["rules"] as? [[String: Any]]
        else { throw ProbeError.fixture }
        rules[0]["max_fires"] = maxFires
        rules[0]["cooldown_ms"] = 0
        rules[0]["predicate"] = ["op": "eq", "value": 1]
        object["rules"] = rules
        change(&object)
        object["plan_hash"] = ""
        let unhashed = try JSONDecoder().decode(ReflexPlan.self, from: JSONSerialization.data(withJSONObject: object))
        object["plan_hash"] = try ReflexContract.hash(unhashed)
        let wire = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes])
        return try ReflexContract.decodeAndValidate(wire, limits: limits(), perception: perception())
    }
}

enum ProbeError: Error {
    case fixture
}

// MARK: - The shared pick scenes

/// Every scene of `reflex-contract/pick_cases.json` read by R5's kernel in this optimized
/// build, a fresh session for each word, as the helper's tests read them in a debug one: each
/// frame drawn in the spec's first class on its ground, read near the scene's hand, and the
/// drawn blob the target follows held to the one the scene names. Answers the scenes read and
/// every frame that followed another blob.
func pickSceneMismatches(fixtures: Fixtures) throws -> (scenes: Int, mismatches: [String]) {
    let raw = try Data(contentsOf: fixtures.root.appendingPathComponent("reflex-contract/pick_cases.json"))
    guard let fixture = try JSONSerialization.jsonObject(with: raw) as? [String: Any],
          let scenes = fixture["cases"] as? [[String: Any]], let specObject = fixture["spec"], let frameObject = fixture["frame"]
    else { throw ProbeError.fixture }
    let spec = try JSONDecoder().decode(PerceptionColorSpec.self, from: JSONSerialization.data(withJSONObject: specObject))
    let extent = try JSONDecoder().decode(ReflexPixelExtent.self, from: JSONSerialization.data(withJSONObject: frameObject))
    guard let ground = spec.ground, let ink = spec.classes.first else { throw ProbeError.fixture }
    let (width, height) = (Int(extent.width), Int(extent.height))
    let whole = ReflexRoi(x: 0, y: 0, width: Int64(width), height: Int64(height), space: .pixel)
    let perception = try fixtures.perception()
    var mismatches: [String] = []
    for scene in scenes {
        guard let name = scene["name"] as? String, let frames = scene["frames"] as? [[String: Any]],
              let expected = scene["expected"] as? [String: [Any]] else { throw ProbeError.fixture }
        for pick in ReflexPick.allCases {
            let session = try PerceptionSession(detectors: [("pick", whole, spec, pick)], limits: perception)
            for (at, frame) in frames.enumerated() {
                var bytes = [UInt8](repeating: 0, count: width * height * 4)
                func paint(_ x: Int, _ y: Int, _ w: Int, _ h: Int, _ colour: PerceptionColorClass) {
                    for row in max(0, y)..<min(height, y + h) {
                        for column in max(0, x)..<min(width, x + w) {
                            let pixel = (row * width + column) * 4
                            (bytes[pixel], bytes[pixel + 1], bytes[pixel + 2], bytes[pixel + 3]) = (colour.b, colour.g, colour.r, 0xFF)
                        }
                    }
                }
                paint(0, 0, width, height, ground)
                let blobs = frame["blobs"] as? [[String: Int]] ?? []
                var boxes: [ReflexRoi] = []
                for blob in blobs {
                    let (x, y, w, h) = (blob["x"] ?? 0, blob["y"] ?? 0, blob["width"] ?? 0, blob["height"] ?? 0)
                    paint(x, y, w, h, ink)
                    if let border = blob["border"] { paint(x + border, y + border, w - 2 * border, h - 2 * border, ground) }
                    boxes.append(ReflexRoi(x: Int64(x), y: Int64(y), width: Int64(w), height: Int64(h), space: .pixel))
                }
                let seq = UInt64(at + 1)
                let facts = ReflexCapture(
                    displayId: "pick", region: whole, pixelExtent: extent,
                    pointTransform: ReflexPointTransform(origin_x: 0, origin_y: 0, points_per_pixel: ReflexScale(numerator: 1, denominator: 1)),
                    orientation: .up, colorSpace: .srgb, status: .ready, dirty: true, captureGap: 0, deliveredHostNs: seq,
                    captureSeq: seq, repaintSeq: seq, streamEpoch: 1, geometryEpoch: 1,
                    clockDomain: ReflexContract.hostUptimeClockDomain, capturedHostNs: seq
                ).facts(runId: "pick", ownerEpoch: 1, planEpoch: 1)
                let hand = (frame["hand"] as? [String: Int64]).map { (x: $0["x"] ?? 0, y: $0["y"] ?? 0) }
                var budget = ReflexPerceptionBudget(samples: perception.max_tick_samples, deadlineHostNs: .max, now: { 0 })
                let seen = bytes.withUnsafeBytes { raw in
                    session.observe(frame: facts, pixels: ReflexPixels(base: raw.baseAddress!, width: width, height: height, bytesPerRow: width * 4),
                                    hand: hand, budget: &budget).first
                }
                let followed = seen?.target.flatMap { target in
                    boxes.firstIndex { $0.contains(x: Double(target.point_x), y: Double(target.point_y)) }
                }
                let want = expected[pick.rawValue].flatMap { at < $0.count ? $0[at] as? Int : nil }
                if seen?.value != Int64(blobs.count) || followed != want {
                    mismatches.append("\(name) \(pick.rawValue) frame \(seq): followed \(String(describing: followed)), wanted \(String(describing: want))")
                }
            }
        }
    }
    return (scenes.count, mismatches)
}

// MARK: - The oracle

/// Whether a run did what a run must, judged from the fixture's own record
/// and the probe's own counts — never from the runtime's receipts alone,
/// which it only holds to what the fixture got. Every check is exact: no
/// share, rate or threshold of the probe's own.
struct Verdict {
    struct Check {
        let name: String
        let passed: Bool
        let detail: String
    }

    let checks: [Check]
    var passed: Bool { checks.allSatisfy(\.passed) }

    init(prepared: SyntheticDesk.Record, ended: SyntheticDesk.Record, kernel: (calls: UInt64, sawBall: UInt64, sawAbsence: UInt64),
         handPosted: UInt64, handHeld: Int, doneClicks: UInt64) {
        checks = [
            Check(name: "starts clean", passed: prepared.events == 0 && prepared.hits == 0 && prepared.misses == 0 &&
                    prepared.foreignHits == 0 && prepared.held.isEmpty,
                  detail: "the fixture before the run: \(prepared.events) events, \(prepared.hits) hits (\(prepared.foreignHits) an earlier run's)"),
            Check(name: "kernel read the frames", passed: kernel.calls > 0, detail: "\(kernel.calls) kernel calls"),
            Check(name: "the kernel saw the ball where it was shown", passed: kernel.sawBall > 0,
                  detail: "\(kernel.sawBall) known sightings with a target on captures that showed the ball"),
            Check(name: "the kernel saw nothing where nothing was shown", passed: kernel.sawAbsence > 0,
                  detail: "\(kernel.sawAbsence) sightings of nothing on captures that hid the ball"),
            Check(name: "every posted event reached the fixture", passed: handPosted > 0 && ended.events == handPosted,
                  detail: "the hand posted \(handPosted), the fixture received \(ended.events)"),
            Check(name: "no press off the ball", passed: ended.misses == 0, detail: "\(ended.misses) presses where the ball was not shown"),
            Check(name: "the ball was hit", passed: ended.hits > 0, detail: "\(ended.hits) hits"),
            Check(name: "every done click is a hit", passed: doneClicks <= ended.hits,
                  detail: "\(doneClicks) click receipts done, \(ended.hits) hits at the fixture"),
            Check(name: "buttons balanced", passed: ended.downs == ended.ups && ended.held.isEmpty && handHeld == 0,
                  detail: "\(ended.downs) downs, \(ended.ups) ups, \(ended.held.count) held at the fixture, \(handHeld) at the hand"),
        ]
    }

    var rendered: [String: Any] {
        [
            "verdict": passed ? "pass" : "fail",
            "checks": checks.map { ["check": $0.name, "passed": $0.passed, "detail": $0.detail] as [String: Any] },
        ]
    }
}

// MARK: - The measurements

struct RunOptions {
    var seconds: Double
    var warmup: Double
    var fault: Fault?
    var kernel: KernelChoice
    var renew: Bool
    /// The rule's `max_fires`: under renewal a quota, else the run's total.
    var maxFires: Int?
    /// A seeded stimulus schedule, or nil for the fixed alternating one.
    var seed: UInt64?
    /// The collector's read interval (the window's is a second, an empirical choice).
    var collectEveryNs: UInt64
}

/// A sustained run: frames at the table's rate, the ball blinking, the real
/// session evaluating and acting on the product's hand, every event handed to
/// the fixture, the receipts read and acknowledged as the window's collector
/// reads them. Every receipt says when its decision frame was published, when
/// it was admitted, how long its first event waited for a newer capture and
/// when that event went; the oracle says whether the run did its job.
func sustainedRun(fixtures: Fixtures, options: RunOptions) throws -> (report: [String: Any], verdict: Verdict) {
    let limits = try fixtures.limits()
    let perception = try fixtures.perception()
    // Two leaves a fire (move, click): without renewal the plan's whole budget, halved.
    let maxFires = options.maxFires ?? Int(limits.max_expanded_actions / 2)
    let plan = try fixtures.plan(maxFires: maxFires)
    let captures = UInt64((options.seconds + 2) * Double(limits.frames_per_second))
    // Fixed: a cycle long enough for a glide, its press and a pause — 30 captures at
    // 60 fps is 500 ms. Seeded: 10–11 shown and 3–4 hidden, a mean of 14 captures —
    // about 257 appearances a minute at 60 fps.
    let scene = options.seed.map { BallScene.seeded($0, present: 10...11, absent: 3...4, captures: captures) }
        ?? BallScene.fixed(present: 18, absent: 12, captures: captures)
    let frames = PacedFrames(framesPerSecond: limits.frames_per_second, scene: scene, lendsPixels: options.fault != .kernelUncalled,
                             draws: options.kernel == .perception, showsBall: options.fault != .kernelBlind)
    let desk = SyntheticDesk(scene: scene, frames: frames, maxFrameAgeNs: limits.max_frame_age_ns, restores: options.fault == .restoredState)
    let poster = DeskPoster(desk: desk, inert: options.fault == .inertPoster)
    let hand = OperatorHand(poster: poster, clock: HostUptimeClock(), sleeper: SemaphoreSleeper(), tag: Int64.random(in: 1...Int64.max))
    let ledger = KernelLedger()
    let kernel: any ReflexPerceptionKernel = options.kernel == .perception
        ? NotedKernel(scene: scene, ledger: ledger)
        : BlinkingBall(scene: scene, lies: options.fault == .wrongTarget, ledger: ledger)
    let policy = ReflexRunPolicy(version: ReflexContract.runPolicyVersion,
                                 run_ns: min(limits.max_run_ns, UInt64(options.seconds * 1e9)), renew: options.renew)
    let accepted = nowNs()
    let session = ReflexSession(
        settings: ReflexSession.Settings(runId: "probe", plan: plan, limits: limits, perception: perception, planEpoch: 1,
                                         policy: policy, deadlineNs: accepted + policy.run_ns),
        hand: hand, source: frames, kernel: kernel,
        monitor: EchoingMonitor(poster: poster, handTag: hand.tag),
        admit: { .admitted }, standing: { .admitted },
        fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000,
        boundary: WholeFixture(), alarm: DispatchAlarm()
    )
    desk.clear()
    let prepared = desk.snapshot
    frames.start()
    let load0 = loadAverages()
    let cpu0 = cpuSeconds()
    let wall0 = nowNs()
    try session.start()
    let collector = ReceiptCollector(session.receipts, everyNs: options.collectEveryNs)
    // The run's own deadline ends it; the probe waits a little past it.
    Thread.sleep(forTimeInterval: options.seconds + 0.25)
    let status = session.status
    session.stop(reason: "probe_end")
    frames.stop()
    let wall = Double(nowNs() - wall0) / 1e9
    let cpu = cpuSeconds() - cpu0
    let receipts = collector.finish()
    let settled = receipts.filter { ($0.decidedHostNs ?? 0) >= wall0 + UInt64(options.warmup * 1e9) }
    func first(_ kind: String) -> [UInt64] {
        settled.filter { $0.actionId.hasPrefix(kind) && $0.outcome == .done }
            .compactMap { receipt in receipt.firstEventHostNs.flatMap { at in receipt.decidedHostNs.map { at >= $0 ? at - $0 : 0 } } }
    }
    func permitted(_ kind: String) -> [UInt64] {
        settled.filter { $0.actionId.hasPrefix(kind) && $0.outcome == .done }
            .compactMap { receipt in receipt.firstEventHostNs.flatMap { at in receipt.firstEventFrameHostNs.map { at >= $0 ? at - $0 : 0 } } }
    }
    var outcomes: [String: Int] = [:]
    for receipt in receipts { outcomes[receipt.outcome.rawValue, default: 0] += 1 }
    let ended = desk.snapshot
    let handSnapshot = hand.snapshot
    let doneClicks = UInt64(receipts.filter { $0.actionId.hasPrefix("click") && $0.outcome == .done }.count)
    let verdict = Verdict(prepared: prepared, ended: ended, kernel: ledger.counts, handPosted: handSnapshot.posted,
                          handHeld: handSnapshot.held.count, doneClicks: doneClicks)
    let endedBy: Any
    if case let .stopped(reason) = status.state { endedBy = reason } else { endedBy = NSNull() }
    let minutes = options.seconds / 60
    let report: [String: Any] = [
        "seconds": options.seconds,
        "warmupSeconds": options.warmup,
        "wallSeconds": wall,
        "fault": options.fault?.rawValue ?? NSNull(),
        "kernel": options.kernel.rawValue,
        "renew": options.renew,
        "maxFires": maxFires,
        "seed": options.seed.map { $0 as Any } ?? NSNull(),
        "endedBy": endedBy,
        "cpuPercentOfOneCore": cpu / wall * 100,
        "loadBefore": load0,
        "loadAfter": loadAverages(),
        "framesPerSecondAsked": limits.frames_per_second,
        "framesEvaluated": status.framesEvaluated,
        "framesUnread": status.framesUnread,
        "framesRefused": status.framesRefused,
        "inadmissible": status.inadmissible,
        "fires": status.fires,
        "renewals": status.renewals,
        "renewalGap": summary(session.renewalGapsNs),
        "leaves": receipts.count,
        "outcomes": outcomes,
        "fixture": [
            "ballAppearances": scene.appearances(through: frames.newestSeq),
            "appearancesPerMinute": Double(scene.appearances(through: frames.newestSeq)) / minutes,
            "events": ended.events,
            "hits": ended.hits,
            "hitsPerMinute": Double(ended.hits) / minutes,
            "misses": ended.misses,
            "downs": ended.downs,
            "ups": ended.ups,
        ],
        "kernelSaw": ["calls": ledger.counts.calls, "ball": ledger.counts.sawBall, "absence": ledger.counts.sawAbsence],
        "decisionToFirstEvent": ["move": summary(first("move")), "click": summary(first("click"))],
        "permittingFrameToFirstEvent": ["move": summary(permitted("move")), "click": summary(permitted("click"))],
        "captureWait": summary(settled.filter { $0.outcome == .done }.map(\.captureWaitNs)),
        "decisionToAdmission": summary(settled.compactMap { receipt in receipt.admittedHostNs.flatMap { at in receipt.decidedHostNs.map { at >= $0 ? at - $0 : 0 } } }),
        "leafAPM": Double(settled.filter { $0.outcome == .done }.count) / max(options.seconds - options.warmup, 1e-9) * 60,
        "oracle": verdict.rendered,
    ]
    return (report, verdict)
}

/// Stop to release, ABBA. Arm B is the product's hand: a key held on it and
/// the stop called from another thread at a random moment of the hold. Arm A
/// reproduces the helper before the hand — `holdKey` slept the whole hold with
/// `usleep` and a stop only set a flag, so the key came up when the hold ended.
/// Each trial reports the stop call to the release's post.
func stopToRelease(trials: Int, holdMs: UInt64) -> [String: Any] {
    func handArm() -> [UInt64] {
        var latencies: [UInt64] = []
        for _ in 0..<trials {
            let poster = DeskPoster(desk: nil, inert: false)
            let hand = OperatorHand(poster: poster, clock: HostUptimeClock(), sleeper: SemaphoreSleeper(), tag: 1)
            let held = DispatchSemaphore(value: 0)
            let done = DispatchSemaphore(value: 0)
            Thread.detachNewThread {
                guard let token = try? hand.acquire(.request) else { return }
                try? hand.post(HandEvent(.key(code: 0, down: true, modifier: 0)), by: token)
                held.signal()
                try? hand.sleep(untilNs: nowNs() + holdMs * 1_000_000, by: token)
                try? hand.post(HandEvent(.key(code: 0, down: false, modifier: 0)), by: token)
                hand.relinquish(token)
                done.signal()
            }
            held.wait()
            usleep(UInt32.random(in: 1_000...UInt32(holdMs * 500)))
            let stoppedAt = nowNs()
            hand.stop(reason: "probe")
            done.wait()
            if let released = poster.releaseTimes().first { latencies.append(released >= stoppedAt ? released - stoppedAt : 0) }
        }
        return latencies
    }
    /// The helper before the hand: the stop is a flag the holder reads only
    /// when its uninterruptible hold ends.
    final class Flag: @unchecked Sendable {
        private let lock = NSLock()
        private var stopped = false
        private var releasedAt: UInt64 = 0
        func stop() { lock.lock(); stopped = true; lock.unlock() }
        func release() { lock.lock(); releasedAt = nowNs(); lock.unlock() }
        var released: UInt64 { lock.lock(); defer { lock.unlock() }; return releasedAt }
    }
    func flagArm() -> [UInt64] {
        var latencies: [UInt64] = []
        for _ in 0..<trials {
            let flag = Flag()
            let held = DispatchSemaphore(value: 0)
            let done = DispatchSemaphore(value: 0)
            Thread.detachNewThread {
                held.signal()
                usleep(UInt32(holdMs * 1_000))
                flag.release()
                done.signal()
            }
            held.wait()
            usleep(UInt32.random(in: 1_000...UInt32(holdMs * 500)))
            let stoppedAt = nowNs()
            flag.stop()
            done.wait()
            latencies.append(flag.released >= stoppedAt ? flag.released - stoppedAt : 0)
        }
        return latencies
    }
    var arms: [[String: Any]] = []
    var product: [UInt64] = []
    for (label, arm) in [("A", flagArm), ("B", handArm), ("B", handArm), ("A", flagArm)] {
        let load = loadAverages()
        let values = arm()
        if label == "B" { product += values }
        var row = summary(values)
        row["arm"] = label == "A" ? "A: usleep-held key, stop as a flag (before the hand)" : "B: the product's OperatorHand"
        row["loadBefore"] = load
        arms.append(row)
    }
    return ["holdMs": holdMs, "trialsPerArm": trials, "arms": arms, "productBothArms": summary(product)]
}

/// One perception tick's cost, ABBA across the evaluator's two candidate thread
/// classes: R5's kernel reading one drawn frame of `plan` per tick on a thread
/// of the class, `ticks` ticks an arm. A tick is one `observe` of the whole plan
/// with the table's samples and no deadline — the kernel's cost, not the
/// runtime's deadline handling.
func tickSeries(plan: ValidatedReflexPlan, perception: PerceptionLimits, ticks: Int) throws -> [String: Any] {
    let session = try PerceptionKernel().session(for: plan, limits: perception)
    let shown = DrawnBuffer(ball: ReflexRoi(x: 8, y: 8, width: 12, height: 12, space: .pixel))
    let hidden = DrawnBuffer(ball: nil)
    var seq: UInt64 = 0
    func arm(_ quality: QualityOfService) -> [UInt64] {
        var costs: [UInt64] = []
        let finished = DispatchSemaphore(value: 0)
        let thread = Thread {
            for index in 0..<ticks {
                seq += 1
                let at = nowNs()
                let capture = ReflexCapture(
                    displayId: "probe", region: ReflexRoi(x: 0, y: 0, width: Int64(DrawnBuffer.width), height: Int64(DrawnBuffer.height), space: .pixel),
                    pixelExtent: ReflexPixelExtent(width: UInt32(DrawnBuffer.width), height: UInt32(DrawnBuffer.height)),
                    pointTransform: ReflexPointTransform(origin_x: 0, origin_y: 0, points_per_pixel: ReflexScale(numerator: 1, denominator: 1)),
                    orientation: .up, colorSpace: .srgb, status: .ready, dirty: true, captureGap: 0, deliveredHostNs: at,
                    captureSeq: seq, repaintSeq: seq, streamEpoch: 1, geometryEpoch: 1,
                    clockDomain: ReflexContract.hostUptimeClockDomain, capturedHostNs: at
                )
                var budget = ReflexPerceptionBudget(samples: perception.max_tick_samples, deadlineHostNs: .max, now: { nowNs() })
                let frame = capture.facts(runId: "ticks", ownerEpoch: 1, planEpoch: 1)
                let buffer = index % 3 == 2 ? hidden : shown
                let started = nowNs()
                _ = session.observe(frame: frame, pixels: buffer.pixels, hand: nil, budget: &budget)
                costs.append(nowNs() - started)
            }
            finished.signal()
        }
        thread.qualityOfService = quality
        thread.start()
        finished.wait()
        return costs
    }
    var rows: [[String: Any]] = []
    var byClass: [String: [UInt64]] = [:]
    for quality in [QualityOfService.default, .userInteractive, .userInteractive, .default] {
        let name = quality == .default ? "default" : "userInteractive"
        let load = loadAverages()
        let costs = arm(quality)
        byClass[name, default: []] += costs
        var row = summary(costs)
        row["qos"] = name
        row["over5ms"] = costs.filter { $0 > 5_000_000 }.count
        row["loadBefore"] = load
        rows.append(row)
    }
    var classes: [String: Any] = [:]
    for (name, costs) in byClass {
        var row = summary(costs)
        row["over5ms"] = costs.filter { $0 > 5_000_000 }.count
        classes[name] = row
    }
    return ["ticksPerArm": ticks, "arms": rows, "byClass": classes]
}

@main
struct ReflexProbe {
    static func main() throws {
        let arguments = CommandLine.arguments
        func value(_ flag: String) -> String? {
            guard let at = arguments.firstIndex(of: flag), at + 1 < arguments.count else { return nil }
            return arguments[at + 1]
        }
        let root = value("--fixtures")
        if arguments.contains("--self-test") {
            precondition(nearestRank([5, 1, 3, 2, 4], 50) == 3)
            precondition(nearestRank([5, 1, 3, 2, 4], 95) == 5)
            precondition(nearestRank([7], 95) == 7)
            precondition(nearestRank([], 50) == nil)
            let scene = BallScene.seeded(7, present: 10...11, absent: 3...4, captures: 600)
            precondition(scene.ball(on: 0) != nil && scene.track(on: 0) == 1)
            precondition(BallScene.seeded(7, present: 10...11, absent: 3...4, captures: 600).cycles.map(\.box) == scene.cycles.map(\.box),
                         "a seed draws one schedule")
            // With the fixtures, the oracle itself: a clean run of each kernel passes and every
            // fault made on purpose fails.
            guard let root else {
                print(#"{"selfTest":"ok","oracle":"not run: no --fixtures"}"#)
                return
            }
            let fixtures = Fixtures(root: URL(fileURLWithPath: root))
            var failures: [String] = []
            for kernel in [KernelChoice.scene, .perception] {
                for fault in [nil] + Fault.allCases.filter({ $0.applies(to: kernel) }).map(Optional.some) {
                    let options = RunOptions(seconds: 1.5, warmup: 0, fault: fault, kernel: kernel, renew: false, maxFires: nil,
                                             seed: nil, collectEveryNs: 250_000_000)
                    let verdict = try sustainedRun(fixtures: fixtures, options: options).verdict
                    if verdict.passed != (fault == nil) { failures.append("\(kernel.rawValue)/\(fault?.rawValue ?? "clean")") }
                }
            }
            guard failures.isEmpty else {
                FileHandle.standardError.write(Data("oracle self-test failed for: \(failures.joined(separator: ", "))\n".utf8))
                exit(1)
            }
            // The kernel's pick, read here in the optimized build the oracle's runs use.
            let picked = try pickSceneMismatches(fixtures: fixtures)
            guard picked.mismatches.isEmpty else {
                FileHandle.standardError.write(Data("pick scenes read otherwise than written:\n\(picked.mismatches.joined(separator: "\n"))\n".utf8))
                exit(1)
            }
            let words = ReflexPick.allCases.map(\.rawValue).joined(separator: ", ")
            print(#"{"selfTest":"ok","oracle":"a clean run passes on the scripted and the real kernel; kernel-uncalled, inert-poster, restored-state fail on both, wrong-target on the scripted and kernel-blind on the real","pick":"# +
                  "\"\(picked.scenes) scenes of pick_cases.json read as written for each of \(words)\"}")
            return
        }
        guard let root else {
            FileHandle.standardError.write(Data("usage: reflex-probe --fixtures <crates/zerocode-core/fixtures> [--seconds 60] [--warmup 2] [--kernel scene|perception] [--renew] [--max-fires N] [--seed S] [--collect-ms 1000] [--trials 40] [--hold-ms 300] [--ticks 400] [--fault \(Fault.allCases.map(\.rawValue).joined(separator: "|"))]\n".utf8))
            exit(2)
        }
        let fault = value("--fault").map { name in
            guard let fault = Fault(rawValue: name) else {
                FileHandle.standardError.write(Data("unknown fault \(name)\n".utf8))
                exit(2)
            }
            return fault
        }
        guard let kernel = KernelChoice(rawValue: value("--kernel") ?? "scene") else {
            FileHandle.standardError.write(Data("--kernel is scene or perception\n".utf8))
            exit(2)
        }
        let fixtures = Fixtures(root: URL(fileURLWithPath: root))
        let options = RunOptions(
            seconds: Double(value("--seconds") ?? "60") ?? 60,
            warmup: Double(value("--warmup") ?? "2") ?? 2,
            fault: fault,
            kernel: kernel,
            renew: arguments.contains("--renew"),
            maxFires: value("--max-fires").flatMap { Int($0) },
            seed: value("--seed").flatMap { UInt64($0) },
            collectEveryNs: (UInt64(value("--collect-ms") ?? "1000") ?? 1_000) * 1_000_000
        )
        let trials = Int(value("--trials") ?? "40") ?? 40
        let holdMs = UInt64(value("--hold-ms") ?? "300") ?? 300
        let ticks = Int(value("--ticks") ?? "0") ?? 0
        let (sustained, verdict) = try sustainedRun(fixtures: fixtures, options: options)
        var report: [String: Any] = [
            "sustained": sustained,
            "stopToRelease": trials > 0 ? stopToRelease(trials: trials, holdMs: holdMs) : NSNull(),
            "note": "in-process runtime clock only: no event reached the system, so no app receipt, click completion or display latency is claimed; the oracle judges the synthetic fixture",
        ]
        if ticks > 0 {
            let perception = try fixtures.perception()
            // The run's own plan, and one detector reading as many samples as the table lets a
            // detector read (R5's heavy blob tick).
            let heavy = try fixtures.plan(maxFires: 1) { object in
                guard var detectors = object["detectors"] as? [[String: Any]],
                      var color = detectors[0]["color"] as? [String: Any],
                      var layout = color["layout"] as? [String: Any] else { return }
                layout["step"] = 1
                color["layout"] = layout
                detectors[0]["color"] = color
                detectors[0]["roi"] = ["height": 500, "space": "pixel", "width": 512, "x": 0, "y": 0]
                object["detectors"] = detectors
            }
            report["ticks"] = [
                "run": try tickSeries(plan: try fixtures.plan(maxFires: 1), perception: perception, ticks: ticks),
                "heavy256000Samples": try tickSeries(plan: heavy, perception: perception, ticks: ticks),
            ]
        }
        let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data("\n".utf8))
        // A run the oracle fails is a failed probe, whatever its numbers.
        if !verdict.passed { exit(1) }
    }
}
