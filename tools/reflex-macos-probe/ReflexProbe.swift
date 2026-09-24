// The macOS reflex runtime's own clock, measured in an optimized build (t-6765).
//
// Built together with the helper's Core sources (see README.md), so what runs
// is the product's `OperatorHand`, `ReflexSession`, rule book, leases and
// receipts — only the frame source, the kernel, the monitor and the poster are
// the probe's. Nothing reaches the system's input: the poster records when an
// event would have been handed to the window server. So this measures the
// runtime's own latency (a frame's publish to the first event it decides, a
// stop to the release it posts), never an app receiving input.
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

// MARK: - The probe's poster, frames, kernel and monitor

/// Records when each event would have been posted; the pointer stays where the
/// hand put it. Nothing reaches the system.
final class TimingPoster: HandPoster, @unchecked Sendable {
    private let lock = NSLock()
    private var location = SmoothPointerPath.Point(x: 0, y: 0)
    private(set) var releases: [UInt64] = []
    var heard: (@Sendable (Int64) -> Void)?

    func post(_ event: HandEvent, tag: Int64) throws {
        let at = nowNs()
        lock.lock()
        if event.points { location = SmoothPointerPath.Point(x: event.x, y: event.y) }
        if event.letsGo != nil { releases.append(at) }
        let heard = self.heard
        lock.unlock()
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
/// way the eye's callback publishes one. Every capture lends the same blank
/// BGRA buffer of the capture's extent; the probe's kernel does not read it.
final class PacedFrames: ReflexFrameSource, @unchecked Sendable {
    private static let width = 800
    private static let height = 500
    private let lock = NSLock()
    private var capture: ReflexCapture?
    private var wake: (@Sendable () -> Void)?
    private var running = true
    private let blank = UnsafeMutableRawPointer.allocate(byteCount: PacedFrames.width * PacedFrames.height * 4, alignment: 16)
    let periodNs: UInt64

    init(framesPerSecond: UInt64) {
        periodNs = 1_000_000_000 / framesPerSecond
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

    func newest() -> ReflexFrameLoan? {
        lock.lock()
        defer { lock.unlock() }
        let pixels = ReflexPixels(base: UnsafeRawPointer(blank), width: Self.width, height: Self.height, bytesPerRow: Self.width * 4)
        return capture.map { ReflexFrameLoan(capture: $0) { body in
            body(pixels)
            return true
        } }
    }

    func onCapture(_ wake: (@Sendable () -> Void)?) {
        lock.lock()
        self.wake = wake
        lock.unlock()
    }
}

/// Sees the ball for `present` captures, then nothing for `absent` — an edge
/// every cycle, so a rule fires again and again at a known pace — and puts it
/// in the other corner of the ROI each cycle, so every move leaf glides.
struct BlinkingBall: ReflexPerceptionKernel {
    let present: UInt64
    let absent: UInt64

    final class Session: ReflexPerceptionSession {
        let present: UInt64
        let absent: UInt64
        init(present: UInt64, absent: UInt64) {
            self.present = present
            self.absent = absent
        }

        func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, budget: inout ReflexPerceptionBudget) -> [ReflexObservation] {
            guard budget.spend(64), let ref = ReflexFrameRef(frame) else { return [] }
            let cycle = frame.capture_seq / (present + absent)
            let shown = frame.capture_seq % (present + absent) < present
            let corner: Int64 = cycle % 2 == 0 ? 0 : 18
            let box = ReflexRoi(x: corner, y: corner, width: 12, height: 12, space: .pixel)
            return [ReflexObservation(
                detector_id: "ball", frame: ref, unknown: nil, value: shown ? 1 : 0,
                target: shown ? ReflexTarget(track_id: cycle + 1, roi: box, point_x: corner + 6, point_y: corner + 6, velocity_x: 0, velocity_y: 0, uncertainty: 1) : nil,
                scale: ReflexScale(numerator: 1, denominator: 1), cells: nil, samples: 64
            )]
        }
    }

    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession {
        Session(present: present, absent: absent)
    }
}

/// Hears what the poster posts, with the stamp it was posted with, on its own
/// queue as a tap does.
final class EchoingMonitor: ReflexInputMonitor, @unchecked Sendable {
    private let lock = NSLock()
    private let queue = DispatchQueue(label: "probe.monitor")
    private var heard: (@Sendable (InputOrigin) -> Void)?
    private let handTag: Int64

    init(poster: TimingPoster, handTag: Int64) {
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

// MARK: - The measurements

/// A sustained run: frames at the table's rate, the ball blinking, the real
/// session evaluating and acting on the product's hand. Every receipt says
/// when its decision frame was published, when it was admitted, how long its
/// first event waited for a newer capture and when that event went.
func sustainedRun(fixtures: Fixtures, seconds: Double, warmup: Double) throws -> [String: Any] {
    let limits = try fixtures.limits()
    let perception = try fixtures.perception()
    // Two leaves a fire (move, click); the plan's whole budget, halved.
    let plan = try fixtures.plan(maxFires: Int(limits.max_expanded_actions / 2))
    let poster = TimingPoster()
    let hand = OperatorHand(poster: poster, clock: HostUptimeClock(), sleeper: SemaphoreSleeper(), tag: Int64.random(in: 1...Int64.max))
    let frames = PacedFrames(framesPerSecond: limits.frames_per_second)
    // A cycle long enough for a glide, its press and a pause: 30 captures at
    // 60 fps is 500 ms, so the plan's 128 fires last 64 s.
    let kernel = BlinkingBall(present: 18, absent: 12)
    let session = ReflexSession(
        settings: ReflexSession.Settings(runId: "probe", plan: plan, limits: limits, perception: perception, planEpoch: 1),
        hand: hand, source: frames, kernel: kernel,
        monitor: EchoingMonitor(poster: poster, handTag: hand.tag),
        admit: { .admitted },
        fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000
    )
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
    return [
        "seconds": seconds,
        "warmupSeconds": warmup,
        "wallSeconds": wall,
        "cpuPercentOfOneCore": cpu / wall * 100,
        "loadBefore": load0,
        "loadAfter": loadAverages(),
        "framesPerSecondAsked": limits.frames_per_second,
        "framesEvaluated": status.framesEvaluated,
        "framesUnread": status.framesUnread,
        "inadmissible": status.inadmissible,
        "fires": status.fires,
        "leaves": receipts.count,
        "outcomes": outcomes,
        "decisionToFirstEvent": ["move": summary(first("move")), "click": summary(first("click"))],
        "permittingFrameToFirstEvent": ["move": summary(permitted("move")), "click": summary(permitted("click"))],
        "captureWait": summary(settled.filter { $0.outcome == .done }.map(\.captureWaitNs)),
        "decisionToAdmission": summary(settled.compactMap { receipt in receipt.admittedHostNs.flatMap { at in receipt.decidedHostNs.map { at >= $0 ? at - $0 : 0 } } }),
        "leafAPM": Double(settled.filter { $0.outcome == .done }.count) / max(wall - warmup, 1e-9) * 60,
    ]
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
            let poster = TimingPoster()
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
        if arguments.contains("--self-test") {
            precondition(nearestRank([5, 1, 3, 2, 4], 50) == 3)
            precondition(nearestRank([5, 1, 3, 2, 4], 95) == 5)
            precondition(nearestRank([7], 95) == 7)
            precondition(nearestRank([], 50) == nil)
            print(#"{"selfTest":"ok"}"#)
            return
        }
        guard let root = value("--fixtures") else {
            FileHandle.standardError.write(Data("usage: reflex-probe --fixtures <crates/zerocode-core/fixtures> [--seconds 60] [--warmup 2] [--trials 40] [--hold-ms 300]\n".utf8))
            exit(2)
        }
        let fixtures = Fixtures(root: URL(fileURLWithPath: root))
        let seconds = Double(value("--seconds") ?? "60") ?? 60
        let warmup = Double(value("--warmup") ?? "2") ?? 2
        let trials = Int(value("--trials") ?? "40") ?? 40
        let holdMs = UInt64(value("--hold-ms") ?? "300") ?? 300
        let report: [String: Any] = [
            "sustained": try sustainedRun(fixtures: fixtures, seconds: seconds, warmup: warmup),
            "stopToRelease": stopToRelease(trials: trials, holdMs: holdMs),
            "note": "in-process runtime clock only: no event reached the system, so no app receipt, click completion or display latency is claimed",
        ]
        let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data("\n".utf8))
    }
}
