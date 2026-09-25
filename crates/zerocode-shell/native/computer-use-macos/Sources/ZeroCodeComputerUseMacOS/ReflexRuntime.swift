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

/// One reflex run at a time, started by the window's control message with
/// the plan and both tables it validated, answering status and stop outside
/// the provider lock. The operator's stop ends a run as it ends everything.
enum ReflexRuntimeHost {
    /// The platform's parts of a run; a test hands its own.
    struct Parts: Sendable {
        /// The display's frames at the run's rate (`ScreenEye.reader`).
        var reader: @Sendable (_ runId: String, _ eye: EyeConfig, _ display: Int, _ framesPerSecond: Int) throws -> any ReflexRunSource
        /// Hears everyone else's input (`EventTapMonitor`).
        var monitor: @Sendable (_ handTag: Int64) -> any ReflexInputMonitor
        /// What the run's presses may land on (`DesktopRunBoundary`).
        var boundary: @Sendable (ReflexActingScope) -> any ReflexInputBoundary

        static let platform = Parts(
            reader: { try ScreenEye.reader($0, config: $1, displayIndex: $2, framesPerSecond: $3) },
            monitor: { EventTapMonitor(handTag: $0) },
            boundary: { DesktopRunBoundary(scope: $0) }
        )
    }

    /// One run from the moment its start is admitted — starting, its source
    /// and session once made — until it is published running, or stopped. A
    /// stop and the start's publishing ask the same slot under one lock: a
    /// stop during a start ends that start, and the start publishes nothing.
    private final class Slot {
        let runId: String
        var source: (any ReflexRunSource)?
        var session: ReflexSession?

        init(runId: String) {
            self.runId = runId
        }
    }

    private static let lock = NSLock()
    nonisolated(unsafe) private static var slot: Slot?
    nonisolated(unsafe) private static var planEpochs: UInt64 = 0
    /// The kernel that reads a plan's detectors (R5's `Perception/`). A helper
    /// built without one refuses every run rather than acting on nothing.
    nonisolated(unsafe) static var kernel: (any ReflexPerceptionKernel)?
    nonisolated(unsafe) static var parts = Parts.platform
    /// The receipts one status read hands the window at most: the whole queue.
    private static let unlimited = Int.max

    /// Start a run: the helper validates the plan again under the tables the
    /// window sent (never numbers of its own), resolves what it acts on
    /// (`actingScope`: the plan's own target, never ZeroCode itself), holds
    /// the hand for the run, reads the display at the table's rate and proves
    /// its monitor hears.
    static func start(
        runId: String,
        plan planWire: Data,
        limits limitsWire: Data,
        perception perceptionWire: Data,
        eye: EyeConfig,
        display: Int,
        hand: OperatorHand,
        admit: @escaping @Sendable () -> GuardAdmission,
        actingScope: (ReflexScope) throws -> ReflexActingScope
    ) throws -> [String: Any] {
        let limits: ReflexLimits
        let perception: PerceptionLimits
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
            plan = try ReflexContract.decodeAndValidate(planWire, limits: limits, perception: perception)
        } catch let error as ReflexContractError {
            throw ProviderError.coded("invalid_argument", "the reflex plan was refused: \(error.rawValue)")
        }
        guard plan.plan.scope.surface == .macos_desktop else {
            throw ProviderError.coded("unsupported_capability", "this helper runs macOS desktop plans, not \(plan.plan.scope.surface.rawValue)")
        }
        lock.lock()
        let kernel = Self.kernel
        let parts = Self.parts
        let held = slot?.runId
        lock.unlock()
        guard let kernel else {
            throw ProviderError.coded("unsupported_capability", "this helper has no perception kernel for a reflex plan yet")
        }
        if let held { throw busy(held) }
        let boundary = parts.boundary(try actingScope(plan.plan.scope))
        lock.lock()
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
                settings: ReflexSession.Settings(runId: runId, plan: plan, limits: limits, perception: perception, planEpoch: epoch),
                hand: hand,
                source: source,
                kernel: kernel,
                monitor: parts.monitor(hand.tag),
                admit: admit,
                fenceNs: UInt64(SyntheticMouseClickDelivery.interEventPauseMicroseconds) * 1_000,
                boundary: boundary
            )
            // From here a stop reaches the session itself: it lets go on its own thread.
            try keep(mine) { $0.session = session }
            try session.start()
            // Published only while no stop has taken the slot — the same
            // judgement the stop makes; one that comes later stops a run.
            try keep(mine) { _ in }
            return render(session.status, receipts: [])
        } catch {
            lock.lock()
            if slot === mine { slot = nil }
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

    /// Where the run stands, and the receipts it kept since the last read.
    static func status() -> [String: Any] {
        lock.lock()
        let current = slot
        let session = current?.session
        lock.unlock()
        guard let current else { return ["state": "none"] }
        guard let session else { return ["runId": current.runId, "state": "starting", "reason": NSNull()] }
        return render(session.status, receipts: session.receipts.drain(limit: unlimited))
    }

    /// End the run — a starting one included: what the hand held is let go of
    /// on this thread first, then the eye gets its rate back.
    static func stop(reason: String) -> [String: Any] {
        lock.lock()
        let current = slot
        slot = nil
        let session = current?.session
        let source = current?.source
        lock.unlock()
        guard let current else { return ["state": "none"] }
        session?.stop(reason: reason)
        let status = session.map { render($0.status, receipts: $0.receipts.drain(limit: unlimited)) }
            ?? ["runId": current.runId, "state": "stopped", "reason": reason]
        // Giving the rate back waits on ScreenCaptureKit; the release is done.
        if let source { DispatchQueue.global(qos: .utility).async { source.end() } }
        return status
    }

    /// The operator stopped (hotkey, signal, request): a run ends with it.
    static func operatorStopped(reason: String) {
        _ = stop(reason: reason)
    }

    static func handshake() -> [String: Any] {
        lock.lock()
        let kernel = Self.kernel != nil
        lock.unlock()
        return ["planVersion": Int(ReflexContract.version), "kernel": kernel]
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

    private static func render(_ status: ReflexSession.Status, receipts: [ReflexReceipt]) -> [String: Any] {
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
        return [
            "runId": status.runId,
            "state": state,
            "reason": reason,
            "planHash": status.planHash,
            "planEpoch": status.planEpoch,
            "framesEvaluated": status.framesEvaluated,
            "framesUnread": status.framesUnread,
            "framesRefused": status.framesRefused,
            "inadmissible": status.inadmissible,
            "fires": status.fires,
            "leaves": status.leaves,
            "receiptsPending": status.receiptsPending,
            "actionsRefused": status.actionsRefused,
            "lastCapture": status.lastCapture.map { $0 as Any } ?? NSNull(),
            "lastCaptureAgeNs": status.lastCaptureAgeNs.map { $0 as Any } ?? NSNull(),
            "monitor": monitor,
            "echoes": status.echoes,
            "othersHeard": status.othersHeard,
            "receipts": receipts.map(render),
        ]
    }

    private static func render(_ receipt: ReflexReceipt) -> [String: Any] {
        func time(_ value: UInt64?) -> Any { value.map { $0 as Any } ?? NSNull() }
        return [
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
