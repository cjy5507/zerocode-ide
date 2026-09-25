import CoreGraphics
import Darwin
import Foundation
import ZeroCodeComputerUseMacOSCore

// MARK: - Hearing everyone else's input (realtime v1 Q4)

/// Hears input for a reflex run: a listen-only session tap on its own thread
/// and run loop. The hand's own events carry its tag; anything else pauses
/// the run. It asks the system for nothing and prompts for nothing — without
/// the grants it cannot hear, and says so; a tap that exists has still heard
/// nothing until the hand's own echo comes back through it (`ReflexSession`).
final class EventTapMonitor: ReflexInputMonitor, @unchecked Sendable {
    /// Every kind of event a person's hand makes on a Mac.
    private static let heardTypes: [CGEventType] = [
        .mouseMoved, .leftMouseDown, .leftMouseUp, .leftMouseDragged,
        .rightMouseDown, .rightMouseUp, .rightMouseDragged,
        .otherMouseDown, .otherMouseUp, .otherMouseDragged,
        .scrollWheel, .keyDown, .keyUp, .flagsChanged,
    ]

    private let lock = NSLock()
    private let handTag: Int64
    private let handPid = Int64(getpid())
    private var tap: CFMachPort?
    private var runLoop: CFRunLoop?
    private var retained: Unmanaged<EventTapMonitor>?
    private var heard: (@Sendable (InputOrigin) -> Void)?
    private var interrupted: (@Sendable (String) -> Void)?

    init(handTag: Int64) {
        self.handTag = handTag
    }

    func start(heard: @escaping @Sendable (InputOrigin) -> Void, interrupted: @escaping @Sendable (String) -> Void) throws {
        guard CGPreflightListenEventAccess() else {
            throw ReflexRunError.monitorDeaf("Input Monitoring is not granted: a person's keys would go unheard")
        }
        guard CGPreflightPostEventAccess() else {
            throw ReflexRunError.monitorDeaf("Accessibility is not granted: the hand cannot post its echo")
        }
        lock.lock()
        self.heard = heard
        self.interrupted = interrupted
        lock.unlock()
        let ready = DispatchSemaphore(value: 0)
        Thread.detachNewThread { [self] in
            let mask = Self.heardTypes.reduce(CGEventMask(0)) { $0 | (CGEventMask(1) << CGEventMask($1.rawValue)) }
            let keep = Unmanaged.passRetained(self)
            guard let tap = CGEvent.tapCreate(
                tap: .cgSessionEventTap,
                place: .headInsertEventTap,
                options: .listenOnly,
                eventsOfInterest: mask,
                callback: eventTapHeard,
                userInfo: keep.toOpaque()
            ) else {
                keep.release()
                ready.signal()
                return
            }
            let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
            let loop = CFRunLoopGetCurrent()
            CFRunLoopAddSource(loop, source, .commonModes)
            CGEvent.tapEnable(tap: tap, enable: true)
            lock.lock()
            self.tap = tap
            self.runLoop = loop
            self.retained = keep
            lock.unlock()
            ready.signal()
            CFRunLoopRun()
        }
        ready.wait()
        lock.lock()
        let made = tap != nil
        lock.unlock()
        if !made { throw ReflexRunError.monitorDeaf("the event tap could not be made") }
    }

    func stop() {
        lock.lock()
        let tap = self.tap
        let loop = runLoop
        let keep = retained
        self.tap = nil
        runLoop = nil
        retained = nil
        heard = nil
        interrupted = nil
        lock.unlock()
        if let tap {
            CGEvent.tapEnable(tap: tap, enable: false)
            CFMachPortInvalidate(tap)
        }
        if let loop { CFRunLoopStop(loop) }
        keep?.release()
    }

    fileprivate func receive(_ type: CGEventType, _ event: CGEvent) {
        lock.lock()
        let heard = self.heard
        let interrupted = self.interrupted
        let tap = self.tap
        lock.unlock()
        switch type {
        case .tapDisabledByTimeout, .tapDisabledByUserInput:
            if let tap { CGEvent.tapEnable(tap: tap, enable: true) }
            interrupted?(type == .tapDisabledByTimeout ? "the system turned the tap off: timeout" : "the system turned the tap off: user input")
        default:
            heard?(InputOrigin.of(
                userData: event.getIntegerValueField(.eventSourceUserData),
                sourcePid: event.getIntegerValueField(.eventSourceUnixProcessID),
                handTag: handTag,
                handPid: handPid
            ))
        }
    }
}

private func eventTapHeard(proxy: CGEventTapProxy, type: CGEventType, event: CGEvent, userInfo: UnsafeMutableRawPointer?) -> Unmanaged<CGEvent>? {
    if let userInfo {
        Unmanaged<EventTapMonitor>.fromOpaque(userInfo).takeUnretainedValue().receive(type, event)
    }
    return Unmanaged.passUnretained(event)
}

// MARK: - The helper's reflex runs (realtime v1 §6)

/// A run's frames as the helper hands them to it: a source the run reads,
/// given back when the run ends (the eye gets its rate back).
protocol ReflexRunSource: ReflexFrameSource {
    func end()
}

/// What a reflex run's press may land on (`ReflexInputBoundary`): the verbs'
/// own policy — never ZeroCode's own window (`DesktopSelf.refuseOwn`) — and
/// only a window of the app the run was started for. Whose window is under a
/// point is the window server's answer (`DesktopSelf.ownerPid`); a test hands
/// its own.
struct DesktopRunBoundary: ReflexInputBoundary {
    let scope: ReflexActingScope
    let owner: @Sendable (CGPoint) -> pid_t?
    let isOwn: @Sendable (pid_t) -> Bool

    init(
        scope: ReflexActingScope,
        owner: @escaping @Sendable (CGPoint) -> pid_t? = { DesktopSelf.ownerPid(at: $0) },
        isOwn: @escaping @Sendable (pid_t) -> Bool = { DesktopSelf.isOwn($0) }
    ) {
        self.scope = scope
        self.owner = owner
        self.isOwn = isOwn
    }

    func refusal(_ input: ReflexLeaseInput, at point: SmoothPointerPath.Point) -> String? {
        let at = CGPoint(x: point.x, y: point.y)
        let pid = owner(at)
        do {
            try DesktopSelf.refuseOwn(pid, at: at, isOwn: isOwn)
        } catch let error as ProviderError {
            return error.message
        } catch {
            return "\(error)"
        }
        guard pid == scope.pid else {
            return "(\(Int(at.x)), \(Int(at.y))) is not on a window of \(scope.target), the app this run acts in"
        }
        return nil
    }
}

/// One reflex run at a time, started by the window's control message with the
/// plan, both tables, the run's policy and the capability table, answering
/// status, stop and the receipts outside the provider lock. Every request after
/// the start names the run it means (`run`), and the slot is compared and read
/// — or stopped — under one lock: a request for a run that is not the one
/// standing never reads or stops the one that is, and is answered from that
/// run's own record once it ended, or as `missing`. The operator's stop ends a
/// run as it ends everything.
enum ReflexRuntimeHost {
    /// The platform's parts of a run; a test hands its own.
    struct Parts: Sendable {
        /// The display's frames at the run's rate (`ScreenEye.reader`).
        var reader: @Sendable (_ runId: String, _ eye: EyeConfig, _ display: Int, _ framesPerSecond: Int) throws -> any ReflexRunSource
        /// Hears everyone else's input (`EventTapMonitor`).
        var monitor: @Sendable (_ handTag: Int64) -> any ReflexInputMonitor
        /// What the run's presses may land on (`DesktopRunBoundary`).
        var boundary: @Sendable (ReflexActingScope) -> any ReflexInputBoundary
        /// Rings the run's deadline on a thread of its own (`DispatchAlarm`).
        var alarm: @Sendable () -> any ReflexAlarm

        static let platform = Parts(
            reader: { try ScreenEye.reader($0, config: $1, displayIndex: $2, framesPerSecond: $3) },
            monitor: { EventTapMonitor(handTag: $0) },
            boundary: { DesktopRunBoundary(scope: $0) },
            alarm: { DispatchAlarm() }
        )
    }

    /// One run from the moment its start is admitted — starting, its source
    /// and session once made — until it is published running, or stopped. A
    /// stop and the start's publishing ask the same slot under one lock: a
    /// stop during a start ends that start, and the start publishes nothing.
    /// Read and changed only under the host's lock.
    private final class Slot: @unchecked Sendable {
        let runId: String
        var source: (any ReflexRunSource)?
        var session: ReflexSession?

        init(runId: String) {
            self.runId = runId
        }
    }

    /// A run that ended: its session — its receipts kept until the window
    /// acknowledges them — or, for one stopped while it started, only why.
    private struct Ended {
        let session: ReflexSession?
        let reason: String
    }

    private static let lock = NSLock()
    nonisolated(unsafe) private static var slot: Slot?
    /// Every ended run whose receipts the window has not all acknowledged, and
    /// the last one to end whatever it still holds — the one a retried stop or
    /// a late status names.
    nonisolated(unsafe) private static var ended: [String: Ended] = [:]
    nonisolated(unsafe) private static var lastEnded: String?
    nonisolated(unsafe) private static var planEpochs: UInt64 = 0
    /// The kernel that reads a plan's detectors (R5's `Perception/`), installed
    /// once when the helper launches (`HelperLaunch`). A helper without one
    /// refuses every run rather than act on nothing.
    nonisolated(unsafe) static var kernel: (any ReflexPerceptionKernel)?
    nonisolated(unsafe) static var parts = Parts.platform

    /// The word an answer uses for a run this helper does not know.
    static let missing = "missing"

    /// The kernel the helper's runs read with, installed when it launches.
    static func install(kernel: any ReflexPerceptionKernel) {
        lock.lock()
        Self.kernel = kernel
        lock.unlock()
    }

    /// Start a run: the helper validates the plan again under the tables the
    /// window sent (never numbers of its own), reads the window's capability
    /// table and the run's policy, resolves what it acts on (`actingScope`: the
    /// plan's own target, never ZeroCode itself), holds the hand for the run,
    /// reads the display at the table's rate and proves its monitor hears. The
    /// run's one deadline counts from the moment this start was first accepted.
    /// The same start asked again — its answer lost on the way — answers the run
    /// it started, whatever became of it, and starts nothing.
    static func start(
        runId: String,
        plan planWire: Data,
        limits limitsWire: Data,
        perception perceptionWire: Data,
        runPolicy policyWire: Data,
        capability capabilityWire: Data,
        eye: EyeConfig,
        display: Int,
        hand: OperatorHand,
        admit: @escaping @Sendable () -> GuardAdmission,
        standing: @escaping @Sendable () -> GuardAdmission,
        actingScope: (ReflexScope) throws -> ReflexActingScope
    ) throws -> [String: Any] {
        let acceptedNs = hand.nowNs()
        if let retried = retriedAnswer(runId) { return retried }
        let limits: ReflexLimits
        let perception: PerceptionLimits
        let capability: ReflexCapabilityTable
        let policy: ReflexRunPolicy
        let plan: ValidatedReflexPlan
        do {
            limits = try ReflexContract.decodeLimits(limitsWire)
            try ReflexTable.check(limits)
        } catch {
            throw ProviderError.coded("invalid_argument", "the reflex table is not the window's canonical table (\(error))")
        }
        do {
            perception = try PerceptionSpecs.decodeLimits(perceptionWire)
        } catch {
            throw ProviderError.coded("invalid_argument", "the perception table is not the window's canonical table (\(error))")
        }
        do {
            capability = try ReflexContract.decodeCapabilities(capabilityWire)
        } catch {
            throw ProviderError.coded("invalid_argument", "the capability table is not the window's canonical table (\(error))")
        }
        do {
            policy = try ReflexContract.decodeRunPolicy(policyWire, limits: limits)
        } catch let error as ReflexContractError {
            throw ProviderError.coded("invalid_argument", "the run policy was refused: \(error.rawValue)")
        }
        do {
            plan = try ReflexContract.decodeAndValidate(planWire, limits: limits, perception: perception)
        } catch let error as ReflexContractError {
            throw ProviderError.coded("invalid_argument", "the reflex plan was refused: \(error.rawValue)")
        }
        guard plan.plan.scope.surface == .macos_desktop else {
            throw ProviderError.coded("unsupported_capability", "this helper runs macOS desktop plans, not \(plan.plan.scope.surface.rawValue)")
        }
        guard ReflexContract.capability(.macos_desktop, table: capability).live_reflex else {
            throw ProviderError.coded("unsupported_capability", "the window's capability table claims no live reflex on this desktop for contract \(ReflexContract.version) and run policy \(ReflexContract.runPolicyVersion)")
        }
        let (deadlineNs, overflow) = acceptedNs.addingReportingOverflow(policy.run_ns)
        guard !overflow else {
            throw ProviderError.coded("invalid_argument", "the run's deadline does not fit the host clock")
        }
        lock.lock()
        let kernel = Self.kernel
        let parts = Self.parts
        let held = slot?.runId
        lock.unlock()
        guard let kernel else {
            throw ProviderError.coded("unsupported_capability", "this helper has no perception kernel for a reflex plan")
        }
        if let held { throw busy(held) }
        let boundary = parts.boundary(try actingScope(plan.plan.scope))
        lock.lock()
        if let answer = retriedAnswerLocked(runId) {
            lock.unlock()
            return answer
        }
        if let held = slot?.runId {
            lock.unlock()
            throw busy(held)
        }
        let mine = Slot(runId: runId)
        slot = mine
        planEpochs += 1
        let epoch = planEpochs
        lock.unlock()
        // The eye's reader once made — given back below whether or not the
        // slot still held the start when it came.
        var reader: (any ReflexRunSource)?
        do {
            let source = try parts.reader(runId, eye, display, Int(clamping: limits.frames_per_second))
            reader = source
            try keep(mine) { $0.source = source }
            let session = ReflexSession(
                settings: ReflexSession.Settings(runId: runId, plan: plan, limits: limits, perception: perception, planEpoch: epoch,
                                                 policy: policy, deadlineNs: deadlineNs),
                hand: hand,
                source: source,
                kernel: kernel,
                monitor: parts.monitor(hand.tag),
                admit: admit,
                standing: standing,
                fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000,
                boundary: boundary,
                alarm: parts.alarm(),
                ended: { [weak mine] reason in if let mine { settle(mine, reason: reason) } }
            )
            // From here a stop reaches the session itself: it lets go on its own thread.
            try keep(mine) { $0.session = session }
            try session.start()
            // Published only while no stop has taken the slot — the same
            // judgement the stop makes; one that comes later stops a run.
            try keep(mine) { _ in }
            return render(session.status)
        } catch {
            lock.lock()
            if slot === mine {
                slot = nil
                remember(runId, Ended(session: mine.session, reason: ReflexRunReason.startFailed))
            }
            let session = mine.session
            lock.unlock()
            session?.stop(reason: ReflexRunReason.startFailed)
            reader?.end()
            throw providerError(error)
        }
    }

    /// Change `mine` while it is still the run's slot; a stop that took it
    /// back ends the start instead.
    private static func keep(_ mine: Slot, _ change: (Slot) -> Void) throws {
        lock.lock()
        defer { lock.unlock() }
        guard slot === mine else { throw ReflexRunError.stoppedWhileStarting }
        change(mine)
    }

    private static func busy(_ run: String) -> ProviderError {
        .coded("hand_busy", "reflex run \(run) holds the hand; stop it first (reflexStop)")
    }

    /// The answer to a start whose run this helper already knows — standing,
    /// starting or ended — marked as the retry it is.
    private static func retriedAnswer(_ runId: String) -> [String: Any]? {
        lock.lock()
        defer { lock.unlock() }
        return retriedAnswerLocked(runId)
    }

    private static func retriedAnswerLocked(_ runId: String) -> [String: Any]? {
        guard var answer = answerLocked(runId) else { return nil }
        answer["retried"] = true
        return answer
    }

    /// A run ended — its deadline, a full receipt queue, a renewal the ledger
    /// refused, or a stop: the slot is freed if it still holds that run, and the
    /// eye gets its rate back. What it held was let go of first.
    private static func settle(_ run: Slot, reason: String) {
        lock.lock()
        let mine = slot === run
        if mine {
            slot = nil
            remember(run.runId, Ended(session: run.session, reason: reason))
        }
        let source = mine ? run.source : nil
        lock.unlock()
        // Giving the rate back waits on ScreenCaptureKit; the release is done.
        if let source { DispatchQueue.global(qos: .utility).async { source.end() } }
    }

    /// Keep an ended run's record, forgetting any earlier one whose receipts
    /// were all acknowledged. Called under the lock.
    private static func remember(_ runId: String, _ record: Ended) {
        ended = ended.filter { ($0.value.session?.receipts.pending ?? 0) > 0 }
        ended[runId] = record
        lastEnded = runId
    }

    /// Where `run` stands — the run standing, or one that ended — and never
    /// another run's: compared and read under one lock. No receipts: those are
    /// the window's collector's alone (`receipts`).
    static func status(run: String) -> [String: Any] {
        lock.lock()
        defer { lock.unlock() }
        return answerLocked(run) ?? ["runId": run, "state": missing]
    }

    private static func answerLocked(_ run: String) -> [String: Any]? {
        if let current = slot, current.runId == run {
            guard let session = current.session else { return ["runId": run, "state": "starting", "reason": NSNull()] }
            return render(session.status)
        }
        guard let record = ended[run] else { return nil }
        guard let session = record.session else { return ["runId": run, "state": "stopped", "reason": record.reason] }
        return render(session.status)
    }

    /// End `run` — a starting one included — and no other: what the hand held
    /// is let go of on this thread first, then the eye gets its rate back. A run
    /// that already ended answers its own end again; one never known, `missing`.
    static func stop(run: String, reason: String) -> [String: Any] {
        lock.lock()
        guard let current = slot, current.runId == run else {
            let answer = answerLocked(run) ?? ["runId": run, "state": missing]
            lock.unlock()
            return answer
        }
        slot = nil
        remember(run, Ended(session: current.session, reason: reason))
        let session = current.session
        let source = current.source
        lock.unlock()
        session?.stop(reason: reason)
        let status = session.map { render($0.status) } ?? ["runId": run, "state": "stopped", "reason": reason]
        // Giving the rate back waits on ScreenCaptureKit; the release is done.
        if let source { DispatchQueue.global(qos: .utility).async { source.end() } }
        return status
    }

    /// The operator stopped (hotkey, signal, request, the session closing):
    /// whatever run stands ends with it.
    static func operatorStopped(reason: String) {
        lock.lock()
        let current = slot?.runId
        lock.unlock()
        if let current { _ = stop(run: current, reason: reason) }
    }

    /// The window's collector reads `run`'s receipts numbered after `after`: the
    /// same ones again until it acknowledges them, whether the run still stands
    /// or has ended.
    static func receipts(run: String, after: UInt64) -> [String: Any] {
        guard let session = session(of: run) else { return ["runId": run, "state": missing] }
        let entries = session.receipts.read(after: after, limit: session.receipts.capacity)
        var answer = render(session.status)
        answer["receipts"] = entries.map(render)
        return answer
    }

    /// The window's collector has `run`'s receipts through `through` on disk:
    /// they are let go of. An ended run with nothing left to hand over is
    /// forgotten, but for the last one to end.
    static func acknowledge(run: String, through: UInt64) -> [String: Any] {
        guard let session = session(of: run) else { return ["runId": run, "state": missing] }
        session.receipts.acknowledge(through: through)
        lock.lock()
        if ended[run] != nil, session.receipts.pending == 0, lastEnded != run { ended[run] = nil }
        lock.unlock()
        return render(session.status)
    }

    private static func session(of run: String) -> ReflexSession? {
        lock.lock()
        defer { lock.unlock() }
        if let current = slot, current.runId == run { return current.session }
        return ended[run]?.session
    }

    static func handshake() -> [String: Any] {
        lock.lock()
        let kernel = Self.kernel != nil
        lock.unlock()
        return ["planVersion": Int(ReflexContract.version), "runPolicy": Int(ReflexContract.runPolicyVersion), "kernel": kernel]
    }

    private static func providerError(_ error: Error) -> Error {
        switch error {
        case let refusal as OperatorHand.Refusal:
            return OperatorHandHost.providerError(refusal, acquiring: true)
        case let ReflexRunError.monitorDeaf(reason):
            return ProviderError.coded("monitor_unavailable", "a reflex run acts only while it can hear a person's input: \(reason)")
        case ReflexRunError.stoppedWhileStarting:
            return ProviderError.coded("stopped", "the reflex run was stopped while it started; it holds nothing")
        case let error as ReflexContractError:
            return ProviderError.coded("unsupported_capability", "the plan cannot run here: \(error.rawValue)")
        default:
            return error
        }
    }

    private static func render(_ status: ReflexSession.Status) -> [String: Any] {
        var state: String
        var reason: Any = NSNull()
        switch status.state {
        case .starting: state = "starting"
        case .running: state = "running"
        case let .paused(why):
            state = "paused"
            reason = why
        case let .stopped(why):
            state = "stopped"
            reason = why
        }
        let monitor: String
        switch status.monitor {
        case .unproven: monitor = "unproven"
        case .hearing: monitor = "hearing"
        case .interrupted: monitor = "interrupted"
        case .unavailable: monitor = "unavailable"
        }
        func nullable<T>(_ value: T?) -> Any { value.map { $0 as Any } ?? NSNull() }
        return [
            "runId": status.runId,
            "state": state,
            "reason": reason,
            "planHash": status.planHash,
            "planEpoch": status.planEpoch,
            "renew": status.policy.renew,
            "deadlineNs": status.deadlineNs,
            "framesEvaluated": status.framesEvaluated,
            "framesUnread": status.framesUnread,
            "framesRefused": status.framesRefused,
            "inadmissible": status.inadmissible,
            "fires": status.fires,
            "renewals": status.renewals,
            "leaves": status.leaves,
            "outcomes": status.outcomes,
            "receiptsPending": status.receiptsPending,
            "receiptsIssued": status.receiptsIssued,
            "receiptsAcknowledged": status.receiptsAcknowledged,
            "sightings": status.sightings.map { sighting in
                [
                    "detector": sighting.detector,
                    "value": nullable(sighting.value),
                    "unknown": nullable(sighting.unknown?.rawValue),
                    "track": nullable(sighting.track),
                    "ageNs": sighting.ageNs,
                ] as [String: Any]
            },
            "scene": status.scene.map { scene in
                ["stream": scene.stream, "geometry": scene.geometry, "owner": scene.owner, "plan": scene.plan] as [String: Any]
            } ?? NSNull(),
            "lastCapture": nullable(status.lastCapture),
            "lastCaptureAgeNs": nullable(status.lastCaptureAgeNs),
            "monitor": monitor,
            "echoes": status.echoes,
            "othersHeard": status.othersHeard,
        ]
    }

    private static func render(_ entry: ReflexReceipts.Entry) -> [String: Any] {
        let receipt = entry.receipt
        func time(_ value: UInt64?) -> Any { value.map { $0 as Any } ?? NSNull() }
        return [
            "seq": entry.seq,
            "ruleId": receipt.ruleId,
            "actionId": receipt.actionId,
            "leafIndex": receipt.leafIndex,
            "outcome": receipt.outcome.rawValue,
            "targetId": receipt.targetId.map { $0 as Any } ?? NSNull(),
            "sourceCapture": time(receipt.sourceCapture),
            "decidedHostNs": time(receipt.decidedHostNs),
            "admittedHostNs": time(receipt.admittedHostNs),
            "captureWaitNs": receipt.captureWaitNs,
            "firstEventHostNs": time(receipt.firstEventHostNs),
            "firstEventFrameHostNs": time(receipt.firstEventFrameHostNs),
            "downHostNs": time(receipt.downHostNs),
            "upHostNs": time(receipt.upHostNs),
            "endedHostNs": receipt.endedHostNs,
            "events": receipt.events,
        ]
    }
}
