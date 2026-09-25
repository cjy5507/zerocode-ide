// The macOS reflex runtime's own clock, measured in an optimized build (t-6765),
// and judged by an oracle of its own.
//
// Built together with the helper's Core sources (see README.md), so what runs
// is the product's `OperatorHand`, `ReflexSession`, rule book, leases and
// receipts — only the frame source, the kernel, the monitor and the poster are
// the probe's. Nothing reaches the system's input: the poster hands each event
// to a synthetic fixture instead, and records when it would have been handed
// to the window server. So this measures the runtime's own latency (a frame's
// publish to the first event it decides, a stop to the release it posts),
// never an app receiving input — and it fails a run that did nothing or did
// the wrong thing, whatever the numbers say.
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
    /// The kernel answers the ball in the other corner from where it is shown.
    case wrongTarget = "wrong-target"
    /// The fixture keeps what an earlier run left and ignores being cleared,
    /// as an app that restores its last state does.
    case restoredState = "restored-state"
}

// MARK: - The fixture the hand acts on

/// The synthetic fixture's scene: where the ball is shown on each capture.
/// It is the ground truth the oracle holds a run to, apart from anything the
/// kernel answers. The ball is shown for `present` captures and hidden for
/// `absent` — an edge each cycle — in the other corner of the ROI each time,
/// so every move glides.
struct BallScene: Sendable {
    let present: UInt64
    let absent: UInt64

    /// The ball's hitbox on capture `seq`, in frame pixels; nil when hidden.
    func ball(on seq: UInt64) -> ReflexRoi? {
        guard seq % (present + absent) < present else { return nil }
        let corner: Int64 = (seq / (present + absent)) % 2 == 0 ? 0 : 18
        return ReflexRoi(x: corner, y: corner, width: 12, height: 12, space: .pixel)
    }

    /// The ball's track: one a cycle.
    func track(on seq: UInt64) -> UInt64 { seq / (present + absent) + 1 }

    /// How many times the ball came on among captures `1...seq`.
    func appearances(through seq: UInt64) -> UInt64 {
        var count: UInt64 = 0
        var shownBefore = false
        for capture in UInt64(1)...max(seq, 1) where seq >= 1 {
            let shown = ball(on: capture) != nil
            if shown, !shownBefore { count += 1 }
            shownBefore = shown
        }
        return count
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

// MARK: - The probe's poster, frames, kernel and monitor

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

/// Captures at a fixed rate on the host clock, each published and woken the
/// way the eye's callback publishes one, and kept a little while so the
/// fixture can say what was shown when. Every capture lends the same blank
/// BGRA buffer of the capture's extent — or, with `Fault.kernelUncalled`,
/// says it lent it without calling the reader at all.
final class PacedFrames: ReflexFrameSource, @unchecked Sendable {
    private static let width = 800
    private static let height = 500
    private let lock = NSLock()
    private var capture: ReflexCapture?
    private var wake: (@Sendable () -> Void)?
    private var running = true
    private var history: [(seq: UInt64, atNs: UInt64)] = []
    private let lendsPixels: Bool
    private let blank = UnsafeMutableRawPointer.allocate(byteCount: PacedFrames.width * PacedFrames.height * 4, alignment: 16)
    let periodNs: UInt64

    init(framesPerSecond: UInt64, lendsPixels: Bool) {
        periodNs = 1_000_000_000 / framesPerSecond
        self.lendsPixels = lendsPixels
        blank.initializeMemory(as: UInt8.self, repeating: 0, count: Self.width * Self.height * 4)
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
                let at = nowNs()
                let next = ReflexCapture(
                    displayId: "probe",
                    region: ReflexRoi(x: 0, y: 0, width: Int64(Self.width), height: Int64(Self.height), space: .pixel),
                    pixelExtent: ReflexPixelExtent(width: UInt32(Self.width), height: UInt32(Self.height)),
                    pointTransform: ReflexPointTransform(origin_x: 0, origin_y: 0, points_per_pixel: ReflexScale(numerator: 1, denominator: 1)),
                    orientation: .up, colorSpace: .srgb, status: .ready, dirty: true, captureGap: 0,
                    deliveredHostNs: at, captureSeq: seq, repaintSeq: seq, streamEpoch: 1, geometryEpoch: 1,
                    clockDomain: ReflexContract.hostUptimeClockDomain, capturedHostNs: at
                )
                lock.lock()
                capture = next
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
        let pixels = ReflexPixels(base: UnsafeRawPointer(blank), width: Self.width, height: Self.height, bytesPerRow: Self.width * 4)
        let lends = lendsPixels
        return capture.map { ReflexFrameLoan(capture: $0) { body in
            if lends { body(pixels) }
            return true
        } }
    }

    func onCapture(_ wake: (@Sendable () -> Void)?) {
        lock.lock()
        self.wake = wake
        lock.unlock()
    }
}

/// Reads the scene on each capture — the probe's stand-in for perception —
/// and counts how often it was asked. With `Fault.wrongTarget` it answers
/// the ball in the other corner from where it is shown.
final class BlinkingBall: ReflexPerceptionKernel, @unchecked Sendable {
    let scene: BallScene
    let lies: Bool
    private let lock = NSLock()
    private var observed: UInt64 = 0

    init(scene: BallScene, lies: Bool) {
        self.scene = scene
        self.lies = lies
    }

    var calls: UInt64 {
        lock.lock()
        defer { lock.unlock() }
        return observed
    }

    fileprivate func count() {
        lock.lock()
        observed += 1
        lock.unlock()
    }

    final class Session: ReflexPerceptionSession {
        let kernel: BlinkingBall
        init(kernel: BlinkingBall) { self.kernel = kernel }

        func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, budget: inout ReflexPerceptionBudget) -> [ReflexObservation] {
            kernel.count()
            guard budget.spend(64), let ref = ReflexFrameRef(frame) else { return [] }
            let shown = kernel.scene.ball(on: frame.capture_seq)
            let box = shown.map { kernel.lies ? ReflexRoi(x: 18 - $0.x, y: 18 - $0.y, width: $0.width, height: $0.height, space: .pixel) : $0 }
            return [ReflexObservation(
                detector_id: "ball", frame: ref, unknown: nil, value: box == nil ? 0 : 1,
                target: box.map { ReflexTarget(track_id: kernel.scene.track(on: frame.capture_seq), roi: $0, point_x: $0.x + 6, point_y: $0.y + 6,
                                              velocity_x: 0, velocity_y: 0, uncertainty: 1) },
                scale: ReflexScale(numerator: 1, denominator: 1), cells: nil, samples: 64
            )]
        }
    }

    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession {
        Session(kernel: self)
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

// MARK: - The plan and tables, from the shared fixtures

struct Fixtures {
    let root: URL

    func limits() throws -> ReflexLimits {
        try ReflexContract.decodeLimits(Data(try Data(contentsOf: root.appendingPathComponent("reflex-contract/limits.json")).dropLast()))
    }

    func perception() throws -> PerceptionLimits {
        try PerceptionSpecs.decodeLimits(Data(try Data(contentsOf: root.appendingPathComponent("game-state/limits.json")).dropLast()))
    }

    /// valid_basic with its rule firing on the ball being there, as often as
    /// the plan's action budget allows.
    func plan(maxFires: Int) throws -> ValidatedReflexPlan {
        let raw = try Data(contentsOf: root.appendingPathComponent("reflex-contract/valid_basic.json"))
        guard let envelope = try JSONSerialization.jsonObject(with: raw) as? [String: Any],
              var object = envelope["plan"] as? [String: Any],
              var rules = object["rules"] as? [[String: Any]]
        else { throw ProbeError.fixture }
        rules[0]["max_fires"] = maxFires
        rules[0]["cooldown_ms"] = 0
        rules[0]["predicate"] = ["op": "eq", "value": 1]
        object["rules"] = rules
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

    init(prepared: SyntheticDesk.Record, ended: SyntheticDesk.Record, kernelCalls: UInt64, handPosted: UInt64, handHeld: Int, doneClicks: UInt64) {
        checks = [
            Check(name: "starts clean", passed: prepared.events == 0 && prepared.hits == 0 && prepared.misses == 0 &&
                    prepared.foreignHits == 0 && prepared.held.isEmpty,
                  detail: "the fixture before the run: \(prepared.events) events, \(prepared.hits) hits (\(prepared.foreignHits) an earlier run's)"),
            Check(name: "kernel read the frames", passed: kernelCalls > 0, detail: "\(kernelCalls) kernel calls"),
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

/// A sustained run: frames at the table's rate, the ball blinking, the real
/// session evaluating and acting on the product's hand, every event handed to
/// the fixture. Every receipt says when its decision frame was published,
/// when it was admitted, how long its first event waited for a newer capture
/// and when that event went; the oracle says whether the run did its job.
func sustainedRun(fixtures: Fixtures, seconds: Double, warmup: Double, fault: Fault?) throws -> (report: [String: Any], verdict: Verdict) {
    let limits = try fixtures.limits()
    let perception = try fixtures.perception()
    // Two leaves a fire (move, click); the plan's whole budget, halved.
    let plan = try fixtures.plan(maxFires: Int(limits.max_expanded_actions / 2))
    // A cycle long enough for a glide, its press and a pause: 30 captures at
    // 60 fps is 500 ms, so the plan's 128 fires last 64 s.
    let scene = BallScene(present: 18, absent: 12)
    let frames = PacedFrames(framesPerSecond: limits.frames_per_second, lendsPixels: fault != .kernelUncalled)
    let desk = SyntheticDesk(scene: scene, frames: frames, maxFrameAgeNs: limits.max_frame_age_ns, restores: fault == .restoredState)
    let poster = DeskPoster(desk: desk, inert: fault == .inertPoster)
    let hand = OperatorHand(poster: poster, clock: HostUptimeClock(), sleeper: SemaphoreSleeper(), tag: Int64.random(in: 1...Int64.max))
    let kernel = BlinkingBall(scene: scene, lies: fault == .wrongTarget)
    let session = ReflexSession(
        settings: ReflexSession.Settings(runId: "probe", plan: plan, limits: limits, perception: perception, planEpoch: 1),
        hand: hand, source: frames, kernel: kernel,
        monitor: EchoingMonitor(poster: poster, handTag: hand.tag),
        admit: { .admitted },
        fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000,
        boundary: WholeFixture()
    )
    desk.clear()
    let prepared = desk.snapshot
    frames.start()
    let load0 = loadAverages()
    let cpu0 = cpuSeconds()
    let wall0 = nowNs()
    try session.start()
    Thread.sleep(forTimeInterval: seconds)
    let status = session.status
    session.stop(reason: "probe_end")
    frames.stop()
    let wall = Double(nowNs() - wall0) / 1e9
    let cpu = cpuSeconds() - cpu0
    let receipts = session.receipts.drain(limit: Int.max)
    let settled = receipts.filter { ($0.decidedHostNs ?? 0) >= wall0 + UInt64(warmup * 1e9) }
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
    let verdict = Verdict(prepared: prepared, ended: ended, kernelCalls: kernel.calls, handPosted: handSnapshot.posted,
                          handHeld: handSnapshot.held.count, doneClicks: doneClicks)
    let report: [String: Any] = [
        "seconds": seconds,
        "warmupSeconds": warmup,
        "wallSeconds": wall,
        "fault": fault?.rawValue ?? NSNull(),
        "cpuPercentOfOneCore": cpu / wall * 100,
        "loadBefore": load0,
        "loadAfter": loadAverages(),
        "framesPerSecondAsked": limits.frames_per_second,
        "framesEvaluated": status.framesEvaluated,
        "framesUnread": status.framesUnread,
        "framesRefused": status.framesRefused,
        "inadmissible": status.inadmissible,
        "fires": status.fires,
        "leaves": receipts.count,
        "outcomes": outcomes,
        "fixture": [
            "ballAppearances": scene.appearances(through: frames.newestSeq),
            "events": ended.events,
            "hits": ended.hits,
            "misses": ended.misses,
            "downs": ended.downs,
            "ups": ended.ups,
        ],
        "decisionToFirstEvent": ["move": summary(first("move")), "click": summary(first("click"))],
        "permittingFrameToFirstEvent": ["move": summary(permitted("move")), "click": summary(permitted("click"))],
        "captureWait": summary(settled.filter { $0.outcome == .done }.map(\.captureWaitNs)),
        "decisionToAdmission": summary(settled.compactMap { receipt in receipt.admittedHostNs.flatMap { at in receipt.decidedHostNs.map { at >= $0 ? at - $0 : 0 } } }),
        "leafAPM": Double(settled.filter { $0.outcome == .done }.count) / max(wall - warmup, 1e-9) * 60,
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
    for (label, arm) in [("A", flagArm), ("B", handArm), ("B", handArm), ("A", flagArm)] {
        let load = loadAverages()
        let values = arm()
        var row = summary(values)
        row["arm"] = label == "A" ? "A: usleep-held key, stop as a flag (before the hand)" : "B: the product's OperatorHand"
        row["loadBefore"] = load
        arms.append(row)
    }
    return ["holdMs": holdMs, "trialsPerArm": trials, "arms": arms]
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
            // With the fixtures, the oracle itself: a clean run passes and every
            // fault made on purpose fails.
            guard let root else {
                print(#"{"selfTest":"ok","oracle":"not run: no --fixtures"}"#)
                return
            }
            let fixtures = Fixtures(root: URL(fileURLWithPath: root))
            var failures: [String] = []
            for fault in [nil] + Fault.allCases.map(Optional.some) {
                let verdict = try sustainedRun(fixtures: fixtures, seconds: 1.5, warmup: 0, fault: fault).verdict
                if verdict.passed != (fault == nil) { failures.append(fault?.rawValue ?? "clean") }
            }
            guard failures.isEmpty else {
                FileHandle.standardError.write(Data("oracle self-test failed for: \(failures.joined(separator: ", "))\n".utf8))
                exit(1)
            }
            print(#"{"selfTest":"ok","oracle":"a clean run passes; kernel-uncalled, inert-poster, wrong-target and restored-state fail"}"#)
            return
        }
        guard let root else {
            FileHandle.standardError.write(Data("usage: reflex-probe --fixtures <crates/zerocode-core/fixtures> [--seconds 60] [--warmup 2] [--trials 40] [--hold-ms 300] [--fault \(Fault.allCases.map(\.rawValue).joined(separator: "|"))]\n".utf8))
            exit(2)
        }
        let fault = value("--fault").map { name in
            guard let fault = Fault(rawValue: name) else {
                FileHandle.standardError.write(Data("unknown fault \(name)\n".utf8))
                exit(2)
            }
            return fault
        }
        let fixtures = Fixtures(root: URL(fileURLWithPath: root))
        let seconds = Double(value("--seconds") ?? "60") ?? 60
        let warmup = Double(value("--warmup") ?? "2") ?? 2
        let trials = Int(value("--trials") ?? "40") ?? 40
        let holdMs = UInt64(value("--hold-ms") ?? "300") ?? 300
        let (sustained, verdict) = try sustainedRun(fixtures: fixtures, seconds: seconds, warmup: warmup, fault: fault)
        let report: [String: Any] = [
            "sustained": sustained,
            "stopToRelease": trials > 0 ? stopToRelease(trials: trials, holdMs: holdMs) : NSNull(),
            "note": "in-process runtime clock only: no event reached the system, so no app receipt, click completion or display latency is claimed; the oracle judges the synthetic fixture",
        ]
        let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data("\n".utf8))
        // A run the oracle fails is a failed probe, whatever its numbers.
        if !verdict.passed { exit(1) }
    }
}
