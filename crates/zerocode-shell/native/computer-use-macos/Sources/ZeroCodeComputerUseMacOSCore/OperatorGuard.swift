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
