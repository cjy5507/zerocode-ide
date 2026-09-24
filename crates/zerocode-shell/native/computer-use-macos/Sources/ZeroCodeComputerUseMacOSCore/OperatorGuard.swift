import Foundation

/// How fast the operator's hand may go (docs/design/computer-use-full-operator.md
/// §1.4). The window's own words: `ComputerPace` and `pace_table()` in
/// `zerocode-core::computer_use`.
///
/// `unlimited` is the table the window sends today: the person asked for no
/// artificial speed cap (m-3903, 2026-09-13). A runaway is still bounded, by
/// what is not a speed — the stop, read before every action and between every
/// posted event, the person's last step, and the session count (which stops
/// and is the person's to lift).
public enum ActionPace: Equatable, Sendable {
    /// No rate of the operator's own: every action the stop and the session
    /// count admit goes now.
    case unlimited
    /// A person's pace: `burst` actions back to back, then `perSecond` — past
    /// a burst the hand waits for the pace rather than stopping itself.
    case paced(perSecond: Int, burst: Int)
}

/// The words the window writes a pace's `mode` in (`PACE_MODE_*`).
public enum PaceMode {
    public static let unlimited = "unlimited"
    public static let paced = "paced"
}

/// Why the window's guard table could not be read — the helper then acts on
/// nothing rather than on numbers of its own.
public struct ActionBudgetRefusal: Error, Equatable, CustomStringConvertible {
    public let description: String

    public init(_ description: String) {
        self.description = description
    }
}

/// How fast the hand may go and how much it may do in all. Every number is
/// the window's table (`guard_table()`), handed over at launch in
/// `windowTableEnvironmentKey`; the helper keeps no numbers of its own and
/// counts because it is the one hand on the input.
public struct ActionBudget: Equatable, Sendable {
    public let pace: ActionPace
    public let perSession: Int

    /// Nil when a number could not bound the hand: a rate, a burst or a
    /// session count under one. A rate is a whole number of actions a second,
    /// so a pace's longest wait is one second and never a division by zero.
    public init?(pace: ActionPace, perSession: Int) {
        guard perSession >= 1 else { return nil }
        if case let .paced(perSecond, burst) = pace, perSecond < 1 || burst < 1 {
            return nil
        }
        self.pace = pace
        self.perSession = perSession
    }

    /// Where the window puts the table (`COMPUTER_GUARD_TABLE_ENV`).
    public static let windowTableEnvironmentKey = "ZEROCODE_COMPUTER_USE_GUARD_TABLE"
    /// The window `actionsInLastMinute` reports over.
    public static let minuteSeconds: TimeInterval = 60

    private struct WindowTable: Decodable {
        struct Pace: Decodable {
            let mode: String
            let perSecond: Int?
            let burst: Int?
        }

        let pace: Pace
        let perSession: Int
    }

    /// The window's table exactly as `guard_table()` writes it: an unlimited
    /// pace carries no rate, a paced one carries both numbers, and anything
    /// else — a missing table, a mode the helper does not know, a fraction,
    /// a zero — is refused, never read as some pace of the helper's own.
    public init(windowTable json: String) throws {
        guard let data = json.data(using: .utf8),
              let table = try? JSONDecoder().decode(WindowTable.self, from: data)
        else {
            throw ActionBudgetRefusal("the guard table is not the window's: \(json.isEmpty ? "none" : json)")
        }
        let pace: ActionPace
        switch (table.pace.mode, table.pace.perSecond, table.pace.burst) {
        case (PaceMode.unlimited, nil, nil):
            pace = .unlimited
        case (PaceMode.paced, let perSecond?, let burst?):
            pace = .paced(perSecond: perSecond, burst: burst)
        default:
            throw ActionBudgetRefusal("the guard table's pace is not one the helper reads: \(json)")
        }
        guard let budget = ActionBudget(pace: pace, perSession: table.perSession) else {
            throw ActionBudgetRefusal("the guard table's numbers cannot bound the hand: \(json)")
        }
        self = budget
    }

    /// The table in the window's own shape, for `status` and the handshake:
    /// an unlimited pace answers its rate and burst `null`, never a stand-in.
    public var rendered: [String: Any] {
        let pace: [String: Any]
        switch self.pace {
        case .unlimited:
            pace = ["mode": PaceMode.unlimited, "perSecond": NSNull(), "burst": NSNull()]
        case let .paced(perSecond, burst):
            pace = ["mode": PaceMode.paced, "perSecond": perSecond, "burst": burst]
        }
        return ["pace": pace, "perSession": perSession]
    }
}

/// The answer to "may this action go?"
public enum GuardAdmission: Equatable, Sendable {
    case admitted
    /// Not yet: the pace comes round in this many seconds. Nothing is stopped.
    case wait(seconds: TimeInterval)
    case stopped(reason: String)
    case sessionBudget(limit: Int)
    /// The window's table never reached the helper: nothing acts.
    case noBudget
}

/// Why the operator is stopped, in the words the window and the skill read.
public enum StopReason {
    public static let hotkey = "hotkey"
    public static let signal = "signal"
    public static let request = "request"
    public static let sessionBudget = "budget:session"
}

/// Why the hand took a hold back by itself, in the words a status reads.
public enum HoldRevocation {
    /// The platform refused a release the hold posted: nothing more is
    /// pressed until a person resumes.
    public static let releaseUnconfirmed = "release_unconfirmed"
}

/// The operator's ledger: stopped or not, how many actions, and the pace.
/// Pure — time comes in as a number, so a test can run a minute in an instant.
public struct OperatorLedger: Equatable, Sendable {
    /// The window's table; nil when it never arrived, and then every action is
    /// refused while the stop still stands and answers.
    public let budget: ActionBudget?
    public private(set) var stoppedReason: String?
    public private(set) var actions: Int = 0
    public private(set) var lastActionAt: TimeInterval?
    /// Actions a paced hand may still take back to back; refills at the pace.
    /// An unlimited hand never reads it.
    private var tokens: Double
    private var refilledAt: TimeInterval?
    private var recent: [TimeInterval] = []

    public init(budget: ActionBudget?) {
        self.budget = budget
        self.tokens = Self.fullBurst(budget)
    }

    private static func fullBurst(_ budget: ActionBudget?) -> Double {
        if case let .paced(_, burst)? = budget?.pace { return Double(burst) }
        return 0
    }

    public var isStopped: Bool { stoppedReason != nil }

    public mutating func stop(reason: String) {
        stoppedReason = reason
    }

    /// Lift the stop. The pace starts fresh — a person who resumed means "go
    /// on" — and the session count clears only when asked, because the
    /// session cap is the person's to lift.
    public mutating func resume(resetBudget: Bool) {
        stoppedReason = nil
        tokens = Self.fullBurst(budget)
        refilledAt = nil
        recent.removeAll()
        if resetBudget { actions = 0 }
    }

    /// Count one action at `now` if it may go; otherwise say how long until
    /// the pace allows it, or why the operator is stopped. Only the session
    /// cap stops the ledger; an unlimited pace never answers `.wait`.
    public mutating func admit(now: TimeInterval) -> GuardAdmission {
        if let reason = stoppedReason { return .stopped(reason: reason) }
        guard let budget else { return .noBudget }
        if actions >= budget.perSession {
            stoppedReason = StopReason.sessionBudget
            return .sessionBudget(limit: budget.perSession)
        }
        if case let .paced(perSecond, burst) = budget.pace {
            // `ActionBudget` holds a rate of at least one a second.
            let rate = Double(perSecond)
            if let refilledAt {
                tokens = min(Double(burst), tokens + max(0, now - refilledAt) * rate)
            }
            refilledAt = now
            guard tokens >= 1 else {
                return .wait(seconds: (1 - tokens) / rate)
            }
            tokens -= 1
        }
        recent.removeAll { now - $0 >= ActionBudget.minuteSeconds }
        recent.append(now)
        actions += 1
        lastActionAt = now
        return .admitted
    }

    public func actionsInLastMinute(now: TimeInterval) -> Int {
        recent.filter { now - $0 < ActionBudget.minuteSeconds }.count
    }
}

/// The stop chord on the whole desktop: control + option + escape, and not
/// command — so it never collides with the system's own force-quit chord.
public let stopHotkeyEscapeKeyCode: UInt16 = 53

public func isStopHotkey(keyCode: UInt16, control: Bool, option: Bool, command: Bool) -> Bool {
    keyCode == stopHotkeyEscapeKeyCode && control && option && !command
}

// MARK: - The one hand (realtime v1 §5.4, §5.8)

/// Where an event goes: the session's HID tap — the desktop, where a person's
/// own input goes — or one process by its pid.
public enum HandRoute: Hashable, Sendable {
    case desktop
    case process(Int32)
}

/// The source an event is made with, each verb as it was made before the
/// hand: none (the desktop verbs) or the combined session state (the app verbs).
public enum HandSource: Hashable, Sendable {
    case none
    case session
}

/// What the hand holds down until it lets go.
public enum HeldInput: Hashable, Sendable {
    case button(MouseButtonSelection)
    case key(UInt16)
    case text(UInt16)
}

/// One input event as data; the platform makes and posts it (`HandPoster`).
public struct HandEvent: Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        case pointerMove
        case buttonDown(MouseButtonSelection, clickState: Int64)
        case buttonUp(MouseButtonSelection, clickState: Int64)
        case buttonDrag(MouseButtonSelection, clickState: Int64)
        case scroll(wheel1: Int32, wheel2: Int32)
        /// A key by its virtual code; `modifier` is the flag it holds while
        /// down (zero for a plain key).
        case key(code: UInt16, down: Bool, modifier: UInt64)
        /// One UTF-16 unit typed on virtual key 0 (`typeText`).
        case text(unit: UInt16, down: Bool)
    }

    public let kind: Kind
    public let x: Double
    public let y: Double
    public let flags: UInt64
    public let route: HandRoute
    public let source: HandSource

    public init(_ kind: Kind, x: Double = 0, y: Double = 0, flags: UInt64 = 0, route: HandRoute = .desktop, source: HandSource = .none) {
        self.kind = kind
        self.x = x
        self.y = y
        self.flags = flags
        self.route = route
        self.source = source
    }

    /// What this event holds down, if it presses.
    public var holds: HeldInput? {
        switch kind {
        case let .buttonDown(button, _): return .button(button)
        case let .key(code, true, _): return .key(code)
        case let .text(unit, true): return .text(unit)
        default: return nil
        }
    }

    /// What this event lets go of, if it releases. A release always goes.
    public var letsGo: HeldInput? {
        switch kind {
        case let .buttonUp(button, _): return .button(button)
        case let .key(code, false, _): return .key(code)
        case let .text(unit, false): return .text(unit)
        default: return nil
        }
    }

    /// Whether the event puts the pointer somewhere.
    public var points: Bool {
        switch kind {
        case .pointerMove, .buttonDown, .buttonUp, .buttonDrag, .scroll: return true
        case .key, .text: return false
        }
    }

    var modifierFlag: UInt64 {
        if case let .key(_, true, modifier) = kind { return modifier }
        return 0
    }

    /// The release of this press: where the pointer is now, with the
    /// modifiers still held after it.
    func release(at point: SmoothPointerPath.Point, flags: UInt64) -> HandEvent? {
        switch kind {
        case let .buttonDown(button, clickState):
            return HandEvent(.buttonUp(button, clickState: clickState), x: point.x, y: point.y, flags: flags, route: route, source: source)
        case let .key(code, true, modifier):
            return HandEvent(.key(code: code, down: false, modifier: modifier), flags: flags, route: route, source: source)
        case let .text(unit, true):
            return HandEvent(.text(unit: unit, down: false), route: route, source: source)
        default:
            return nil
        }
    }
}

/// Makes and posts one event stamped with the hand's tag, so the input
/// monitor can tell the hand's own events from anyone else's.
public protocol HandPoster: Sendable {
    func post(_ event: HandEvent, tag: Int64) throws
    /// Where the pointer is now, if the platform can say.
    func pointerLocation() -> SmoothPointerPath.Point?
}

/// Nanoseconds on the host's uptime clock — the clock ScreenCaptureKit
/// stamps frames with (`ReflexContract.hostUptimeClockDomain`).
public protocol HandClock: Sendable {
    func nowNs() -> UInt64
}

public struct HostUptimeClock: HandClock {
    public init() {}
    public func nowNs() -> UInt64 { DispatchTime.now().uptimeNanoseconds }
}

/// How the hand's holder waits between two events: until a due time or an
/// earlier wake — a stop, a revoke, a new frame. A wake with nobody waiting
/// makes the next wait return early; the waiter reads its news again.
public protocol HandSleeper: Sendable {
    func sleep(untilNs deadlineNs: UInt64)
    func wake()
}

public final class SemaphoreSleeper: HandSleeper {
    private let semaphore = DispatchSemaphore(value: 0)
    public init() {}
    public func sleep(untilNs deadlineNs: UInt64) {
        _ = semaphore.wait(timeout: DispatchTime(uptimeNanoseconds: deadlineNs))
    }
    public func wake() { semaphore.signal() }
}

/// The helper's one hand (realtime v1 §5.4): every synthetic event — a
/// request's verbs and a reflex run's alike — is posted here by whoever holds
/// the hand, and nobody else posts while it is held. It keeps what it pressed
/// and lets go of it the moment the operator stops, on whatever thread the
/// stop came in on: no frame, socket, model or disk stands between a stop and
/// the release. A release always goes, and only of what the hand itself
/// pressed; a press goes only while the hold is good. A lock guards the
/// ledger and each post, never a wait: a holder sleeps outside it.
public final class OperatorHand: @unchecked Sendable {
    public enum Owner: Equatable, Sendable {
        /// One provider request (the verbs a CLI call runs).
        case request
        /// A reflex run, by its run id.
        case reflex(String)
    }

    /// A hold on the hand. A new hold has a new id, so a stale token never
    /// passes for the current one.
    public struct Token: Equatable, Sendable {
        public let id: UInt64
        public let owner: Owner
    }

    public enum Refusal: Error, Equatable, Sendable {
        /// Someone else holds the hand.
        case busy(Owner)
        /// The operator is stopped until a person resumes it.
        case stopped(String)
        /// This hold was taken back; the holder presses nothing more.
        case revoked(String)
        /// The token is not the hand's current hold.
        case notHolder
        /// A release the hand posted could not be confirmed; nothing is
        /// pressed again until a person resumes.
        case releaseUnconfirmed
    }

    /// What a stop, a revoke or the end of a hold let go of, and when.
    public struct Release: Equatable, Sendable {
        public let released: [HeldInput]
        public let unconfirmed: [HeldInput]
        public let startedNs: UInt64
        public let endedNs: UInt64
    }

    public struct Snapshot: Equatable, Sendable {
        public let holder: Owner?
        public let revoked: String?
        public let stopped: String?
        public let held: [HeldInput]
        public let posted: UInt64
        public let unconfirmedReleases: UInt64
    }

    /// One press still down and the hold that pressed it: a release belongs
    /// to that hold, never to whoever holds the hand later.
    private struct Press {
        let event: HandEvent
        let hold: UInt64
    }

    private let lock = NSLock()
    private let poster: any HandPoster
    private let clock: any HandClock
    private let sleeper: any HandSleeper
    /// Stamped on every event the hand posts (`eventSourceUserData`): never zero.
    public let tag: Int64
    private var lastId: UInt64 = 0
    private var holder: Token?
    private var revokedReason: String?
    private var stoppedReason: String?
    /// The presses still down, oldest first — only ever the current hold's:
    /// a hold is let go of whole before the hand changes hands.
    private var held: [Press] = []
    private var pointer: SmoothPointerPath.Point?
    private var posted: UInt64 = 0
    private var unconfirmedReleases: UInt64 = 0

    public init(poster: any HandPoster, clock: any HandClock, sleeper: any HandSleeper, tag: Int64) {
        precondition(tag != 0, "the hand's tag tells its own events apart; zero is everyone's")
        self.poster = poster
        self.clock = clock
        self.sleeper = sleeper
        self.tag = tag
    }

    public func nowNs() -> UInt64 { clock.nowNs() }

    /// Take the hand for `owner`, or say who has it or why nobody may.
    public func acquire(_ owner: Owner) throws -> Token {
        lock.lock()
        defer { lock.unlock() }
        if let stoppedReason { throw Refusal.stopped(stoppedReason) }
        if let holder { throw Refusal.busy(holder.owner) }
        if unconfirmedReleases > 0 { throw Refusal.releaseUnconfirmed }
        lastId += 1
        let token = Token(id: lastId, owner: owner)
        holder = token
        revokedReason = nil
        return token
    }

    /// Give the hand back; whatever is still held is let go of first.
    @discardableResult
    public func relinquish(_ token: Token) -> Release {
        lock.lock()
        defer { lock.unlock() }
        guard holder == token else { return emptyRelease() }
        let release = releaseHeldLocked()
        holder = nil
        revokedReason = nil
        return release
    }

    /// Post one event for the holder `token`. A release of something this hold
    /// pressed always goes — past a stop, a revoke, the hold's own end; one of
    /// something it did not press is dropped: a person's own button, or the
    /// same button the next hold pressed, is not this hold's to let go of.
    public func post(_ event: HandEvent, by token: Token) throws {
        if let input = event.letsGo {
            try release(input, with: event, by: token)
            return
        }
        lock.lock()
        defer { lock.unlock() }
        if let refusal = refusalLocked(token) { throw refusal }
        try poster.post(event, tag: tag)
        posted += 1
        if event.points { pointer = SmoothPointerPath.Point(x: event.x, y: event.y) }
        if event.holds != nil { held.append(Press(event: event, hold: token.id)) }
    }

    /// A hold's own release of `input`. One the platform refuses is settled at
    /// once, as a stop settles one (`releaseHeldLocked`): counted until a
    /// person resumes, the hold taken back and what else it pressed let go of
    /// — the hand never goes on as if the input were up, nor as if it were
    /// still its own to press around.
    private func release(_ input: HeldInput, with event: HandEvent, by token: Token) throws {
        lock.lock()
        guard let at = held.lastIndex(where: { $0.hold == token.id && $0.event.holds == input }) else {
            lock.unlock()
            return
        }
        held.remove(at: at)
        do {
            try poster.post(event, tag: tag)
        } catch {
            unconfirmedReleases += 1
            if holder == token, revokedReason == nil { revokedReason = HoldRevocation.releaseUnconfirmed }
            _ = releaseHeldLocked()
            lock.unlock()
            sleeper.wake()
            throw error
        }
        posted += 1
        if event.points { pointer = SmoothPointerPath.Point(x: event.x, y: event.y) }
        lock.unlock()
    }

    /// Why `token` may not press now, or nil when it may.
    public func refusal(for token: Token) -> Refusal? {
        lock.lock()
        defer { lock.unlock() }
        return refusalLocked(token)
    }

    private func refusalLocked(_ token: Token) -> Refusal? {
        if let stoppedReason { return .stopped(stoppedReason) }
        guard holder == token else { return .notHolder }
        if let revokedReason { return .revoked(revokedReason) }
        return nil
    }

    /// The operator stops (the hotkey, the signal, a request): nothing is
    /// pressed until `resume`, the current hold is taken back, and every input
    /// the hand pressed is let go of here, on the stopping thread.
    @discardableResult
    public func stop(reason: String) -> Release {
        lock.lock()
        stoppedReason = reason
        if holder != nil, revokedReason == nil { revokedReason = reason }
        let release = releaseHeldLocked()
        lock.unlock()
        sleeper.wake()
        return release
    }

    /// A person resumed: presses may go again, and a release the hand could
    /// not confirm is theirs to have checked.
    public func resume() {
        lock.lock()
        stoppedReason = nil
        unconfirmedReleases = 0
        lock.unlock()
    }

    /// One hold is taken back — a reflex run pauses on someone else's input,
    /// a frame source gone quiet: the holder keeps the hand but presses
    /// nothing more, and what it held is let go of now.
    @discardableResult
    public func revoke(_ token: Token, reason: String) -> Release {
        lock.lock()
        guard holder == token else {
            lock.unlock()
            return emptyRelease()
        }
        if revokedReason == nil { revokedReason = reason }
        let release = releaseHeldLocked()
        lock.unlock()
        sleeper.wake()
        return release
    }

    /// Wait until `deadlineNs` for the holder `token`; a stop or a revoke
    /// ends the wait at once with the refusal, other news only wakes it.
    public func sleep(untilNs deadlineNs: UInt64, by token: Token) throws {
        while true {
            if let refusal = refusal(for: token) { throw refusal }
            if clock.nowNs() >= deadlineNs { return }
            sleeper.sleep(untilNs: deadlineNs)
        }
    }

    /// Wait once for the holder `token`: until `deadlineNs` or the next wake,
    /// whichever is first — a holder waiting for news (a newer frame) reads
    /// it again after every wake. A stop or a revoke ends it with the refusal.
    public func nap(untilNs deadlineNs: UInt64, by token: Token) throws {
        if let refusal = refusal(for: token) { throw refusal }
        if clock.nowNs() < deadlineNs { sleeper.sleep(untilNs: deadlineNs) }
        if let refusal = refusal(for: token) { throw refusal }
    }

    /// Wake the holder's wait (a new frame, news from another thread).
    public func wake() { sleeper.wake() }

    /// Where the hand last put the pointer, else where the platform says it is.
    public func pointerNow() -> SmoothPointerPath.Point? {
        lock.lock()
        let last = pointer
        lock.unlock()
        return poster.pointerLocation() ?? last
    }

    public var snapshot: Snapshot {
        lock.lock()
        defer { lock.unlock() }
        return Snapshot(
            holder: holder?.owner,
            revoked: revokedReason,
            stopped: stoppedReason,
            held: held.compactMap(\.event.holds),
            posted: posted,
            unconfirmedReleases: unconfirmedReleases
        )
    }

    private func emptyRelease() -> Release {
        let now = clock.nowNs()
        return Release(released: [], unconfirmed: [], startedNs: now, endedNs: now)
    }

    /// Let go of every held input, newest first — a key before the modifiers
    /// it was pressed with — each with the modifiers still held after it.
    private func releaseHeldLocked() -> Release {
        let startedNs = clock.nowNs()
        var released: [HeldInput] = []
        var unconfirmed: [HeldInput] = []
        let at = poster.pointerLocation() ?? pointer
        while let press = held.popLast()?.event {
            guard let input = press.holds else { continue }
            let flags = held.reduce(UInt64(0)) { $0 | $1.event.modifierFlag }
            guard let release = press.release(at: at ?? SmoothPointerPath.Point(x: press.x, y: press.y), flags: flags) else { continue }
            do {
                try poster.post(release, tag: tag)
                posted += 1
                released.append(input)
            } catch {
                unconfirmed.append(input)
            }
        }
        unconfirmedReleases += UInt64(unconfirmed.count)
        return Release(released: released, unconfirmed: unconfirmed, startedNs: startedNs, endedNs: clock.nowNs())
    }
}

// MARK: - Whose input (realtime v1 Q4)

/// Whose an input event is, read from the tag the hand stamps on its own
/// (`eventSourceUserData`) and the process that posted it. It ties an event
/// to the hand; it does not prove a person made the others — another
/// automation, an accessibility client, a remote session are `other` too.
public enum InputOrigin: Equatable, Sendable {
    case hand
    case other

    public static func of(userData: Int64, sourcePid: Int64, handTag: Int64, handPid: Int64) -> InputOrigin {
        handTag != 0 && userData == handTag && sourcePid == handPid ? .hand : .other
    }
}

/// Whether the monitor that hears other input can be trusted to hear it.
/// A tap that exists has not yet heard anything: only the hand's own tagged
/// echo coming back through it proves it hears.
public enum InputMonitorHealth: Equatable, Sendable {
    case unproven
    case hearing
    /// The system turned the tap off (a timeout, user input) — what came
    /// between is unknown.
    case interrupted(String)
    /// It could not be made, or cannot hear (no permission, Secure Input).
    case unavailable(String)
}

/// What the hand's input watch concludes from what the monitor heard.
public struct InputWatch: Equatable, Sendable {
    public private(set) var health: InputMonitorHealth = .unproven
    public private(set) var echoes: UInt64 = 0
    public private(set) var others: UInt64 = 0

    public init() {}

    /// One event heard. The hand's own proves the tap hears and never
    /// pauses anything; anyone else's is a reason to pause.
    public mutating func heard(_ origin: InputOrigin) -> Bool {
        switch origin {
        case .hand:
            echoes += 1
            if health == .unproven { health = .hearing }
            return false
        case .other:
            others += 1
            return true
        }
    }

    public mutating func interrupted(_ reason: String) { health = .interrupted(reason) }
    public mutating func unavailable(_ reason: String) { health = .unavailable(reason) }

    /// The hand acts on its own only while it can hear everyone else.
    public var permitsActing: Bool { health == .hearing }
}
