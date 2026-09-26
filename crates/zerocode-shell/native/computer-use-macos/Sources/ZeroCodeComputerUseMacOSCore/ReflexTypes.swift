import CryptoKit
import Foundation

/// Wire values have no input authority until `ReflexContract.validate` succeeds.
public struct ReflexPlan: Codable, Sendable {
    public let version: UInt32
    public let plan_hash: String
    public let scope: ReflexScope
    public let detectors: [ReflexDetector]
    public let rules: [ReflexRule]
    public let macros: [ReflexMacro]
    public let pointer: ReflexPointerStyle
}

public struct ReflexScope: Codable, Sendable {
    public let surface: ReflexSurface
    public let target: String
}
public enum ReflexSurface: String, Codable, Sendable { case macos_desktop, ios_device, windows_desktop }
public enum ReflexCoordinateSpace: String, Codable, Sendable { case pixel, point, css, device }
public struct ReflexRoi: Codable, Equatable, Sendable {
    public let x: Int64
    public let y: Int64
    public let width: Int64
    public let height: Int64
    public let space: ReflexCoordinateSpace

    public init(x: Int64, y: Int64, width: Int64, height: Int64, space: ReflexCoordinateSpace) {
        self.x = x
        self.y = y
        self.width = width
        self.height = height
        self.space = space
    }
}
public struct ReflexScale: Codable, Equatable, Sendable {
    public let numerator: UInt64
    public let denominator: UInt64

    public init(numerator: UInt64, denominator: UInt64) {
        self.numerator = numerator
        self.denominator = denominator
    }
}
public enum ReflexDetectorKind: String, Codable, Sendable { case color, template, motion }
/// Which blob a colour detector's target follows when it follows none (`reflex::Pick`); the
/// kernel picks only then (`PerceptionBlobTracks.pick`).
public enum ReflexPick: String, Codable, CaseIterable, Sendable { case first, nearest, largest, newest, oldest }
public struct ReflexDetector: Codable, Sendable {
    public let id: String
    public let kind: ReflexDetectorKind
    public let roi: ReflexRoi
    public let patches: UInt64
    public let scale: ReflexScale
    /// What a colour detector reads its ROI with (`game_state::ColorSpec`); required for `color`.
    public let color: PerceptionColorSpec?
    /// Which blob its target follows when it follows none; the wire leaves the key out when it
    /// is `first`, as the window's serde type does — so a plan without one keeps its bytes.
    public let pick: ReflexPick
}

extension ReflexDetector {
    private enum CodingKeys: String, CodingKey { case id, kind, roi, patches, scale, color, pick }

    public init(from decoder: Decoder) throws {
        let fields = try decoder.container(keyedBy: CodingKeys.self)
        self.init(id: try fields.decode(String.self, forKey: .id),
                  kind: try fields.decode(ReflexDetectorKind.self, forKey: .kind),
                  roi: try fields.decode(ReflexRoi.self, forKey: .roi),
                  patches: try fields.decode(UInt64.self, forKey: .patches),
                  scale: try fields.decode(ReflexScale.self, forKey: .scale),
                  color: try fields.decodeIfPresent(PerceptionColorSpec.self, forKey: .color),
                  pick: try fields.decodeIfPresent(ReflexPick.self, forKey: .pick) ?? .first)
    }

    public func encode(to encoder: Encoder) throws {
        var fields = encoder.container(keyedBy: CodingKeys.self)
        try fields.encode(id, forKey: .id)
        try fields.encode(kind, forKey: .kind)
        try fields.encode(roi, forKey: .roi)
        try fields.encode(patches, forKey: .patches)
        try fields.encode(scale, forKey: .scale)
        try fields.encodeIfPresent(color, forKey: .color)
        if pick != .first { try fields.encode(pick, forKey: .pick) }
    }
}
public enum ReflexPredicateOp: String, Codable, Sendable { case known, eq, not, any, all }
public final class ReflexPredicate: Codable, Sendable {
    public let op: ReflexPredicateOp
    public let value: Int64?
    public let child: ReflexPredicate?
    public let children: [ReflexPredicate]?

    public func evaluate(_ feature: Int64?) -> ReflexTruth {
        switch op {
        case .known: return feature == nil ? .unknown : .yes
        case .eq:
            guard let feature, let value else { return .unknown }
            return feature == value ? .yes : .no
        case .not:
            switch child?.evaluate(feature) ?? .unknown {
            case .yes: return .no
            case .no: return .yes
            case .unknown: return .unknown
            }
        case .any:
            let results = (children ?? []).map { $0.evaluate(feature) }
            if results.contains(.yes) { return .yes }
            return results.contains(.unknown) ? .unknown : .no
        case .all:
            let results = (children ?? []).map { $0.evaluate(feature) }
            if results.contains(.no) { return .no }
            return results.contains(.unknown) ? .unknown : .yes
        }
    }
}
public enum ReflexTruth: Sendable { case yes, no, unknown }
public struct ReflexRule: Codable, Sendable {
    public let id: String
    public let detector: String
    public let predicate: ReflexPredicate
    public let macro_id: String
    public let priority: Int64
    public let cooldown_ms: UInt64
    public let max_fires: UInt64
}
public enum ReflexActionKind: String, Codable, Sendable { case move, click, key, macro }
public struct ReflexAction: Codable, Sendable {
    public let id: String
    public let kind: ReflexActionKind
    public let target: String
}
public struct ReflexMacro: Codable, Sendable {
    public let id: String
    public let `repeat`: UInt64
    public let actions: [ReflexAction]
}
public enum ReflexPointerCurve: String, Codable, Sendable { case linear, cosine }
public struct ReflexPointerStyle: Codable, Sendable {
    public let duration_ms: UInt64
    public let curve: ReflexPointerCurve
    public let instant: Bool
}

public struct ReflexLimits: Codable, Equatable, Sendable {
    public let max_detectors: UInt64
    public let max_rules: UInt64
    public let max_macros: UInt64
    public let max_actions: UInt64
    public let max_expanded_actions: UInt64
    public let max_roi_pixels: UInt64
    public let max_work_pixels: UInt64
    public let max_predicate_depth: UInt64
    public let max_macro_depth: UInt64
    public let max_cooldown_ms: UInt64
    public let max_lease_ns: UInt64
    /// The longest one run may be asked to last (`ReflexRunPolicy.run_ns`).
    public let max_run_ns: UInt64
    public let max_scale_part: UInt64
    public let max_pointer_duration_ms: UInt64
    public let instant_duration_ms: UInt64
    public let max_frame_age_ns: UInt64
    public let frames_per_second: UInt64
    public let pointer_tick_ns: UInt64
}

// MARK: - The window's reflex table

/// The window's reflex table as a run receives it (`ReflexLimits`, the core's
/// `LIMITS` through the start message), read once before a run leans on it.
/// The helper keeps no copy of these numbers.
public enum ReflexTable {
    /// Every bound a run waits or divides by is positive, and the longest
    /// glide plus a press and its release fit one lease's children.
    public static func check(_ limits: ReflexLimits) throws {
        guard limits.max_frame_age_ns > 0, limits.max_lease_ns > 0, limits.max_run_ns > 0,
              limits.frames_per_second > 0, limits.pointer_tick_ns > 0,
              maxGlideWaypoints(limits) < limits.max_expanded_actions,
              limits.max_expanded_actions - maxGlideWaypoints(limits) >= 2
        else { throw ReflexContractError.budget }
    }

    /// The most waypoints one glide can have under the table.
    public static func maxGlideWaypoints(_ limits: ReflexLimits) -> UInt64 {
        let longest = limits.max_pointer_duration_ms.multipliedReportingOverflow(by: 1_000_000)
        guard !longest.overflow, limits.pointer_tick_ns > 0 else { return .max }
        let (whole, part) = longest.partialValue.quotientAndRemainder(dividingBy: limits.pointer_tick_ns)
        return whole + (part > 0 ? 1 : 0)
    }
}

public enum ReflexContractError: String, Error { case wire, version, hash, scope, id, duplicate, reference, cycle, budget, unsupported, perception }

/// A helper must use this return type, never a decoded `ReflexPlan`, for input.
public struct ValidatedReflexPlan: Sendable {
    public let plan: ReflexPlan
    fileprivate init(_ plan: ReflexPlan) { self.plan = plan }
}

public enum ReflexContract {
    /// 2: a colour detector carries its spec inside the plan and its hash.
    public static let version: UInt32 = 2
    /// The clock a macOS frame, lease and observation are stamped in: host uptime
    /// nanoseconds (`mach_absolute_time`), the clock ScreenCaptureKit reports a frame's
    /// display time in (`reflex::HOST_UPTIME_CLOCK`).
    public static let hostUptimeClockDomain: UInt64 = 1
    /// 1: a run's own terms beside its plan (`reflex::RUN_POLICY_VERSION`).
    public static let runPolicyVersion: UInt32 = 1
    /// The longest id a plan, a rule, a macro, an action or a run may carry
    /// (`reflex::MAX_IDENTIFIER_BYTES`), pinned for both by the shared plan cases.
    public static let maxIdentifierBytes = 64

    /// The reflex table as the window sends it (`reflex::limits_wire`): only the canonical
    /// form is read, so a field the helper does not know cannot be dropped silently.
    public static func decodeLimits(_ data: Data) throws -> ReflexLimits {
        guard let limits = try? JSONDecoder().decode(ReflexLimits.self, from: data),
              let object = try? JSONSerialization.jsonObject(with: JSONEncoder().encode(limits)),
              let canonical = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes]),
              canonical == data
        else { throw ReflexContractError.wire }
        return limits
    }

    public static func decodeAndValidate(_ data: Data, limits: ReflexLimits, perception: PerceptionLimits) throws -> ValidatedReflexPlan {
        guard let object = try? JSONSerialization.jsonObject(with: data),
              let canonical = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes]),
              canonical == data,
              let plan = try? JSONDecoder().decode(ReflexPlan.self, from: data)
        else { throw ReflexContractError.wire }
        guard let typedWire = try? wireBytes(plan), typedWire == data else { throw ReflexContractError.wire }
        for rule in plan.rules { try predicateShape(rule.predicate) }
        return try validate(plan, limits: limits, perception: perception)
    }

    /// Whether `text` is an id this contract carries: 1 to `maxIdentifierBytes` bytes of
    /// `[A-Za-z0-9_-]` (`reflex::identifier`).
    public static func identifier(_ text: String) -> Bool {
        guard !text.isEmpty, text.utf8.count <= maxIdentifierBytes else { return false }
        return text.utf8.allSatisfy { (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 || $0 == 95 }
    }

    /// The canonical form of a JSON object: compact, keys sorted at every depth.
    private static func canonical(_ object: Any) -> Data? {
        try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes])
    }

    /// A run's policy as a start carries it (`reflex::decode_run_policy`, pinned for both by
    /// `run_policy_cases.json`): canonical, of the version this contract names — an absent
    /// one or another integer is a version refused before any other field is read, a
    /// version that is not an integer is the wire — with exactly its three fields, and a
    /// length the window's table allows.
    public static func decodeRunPolicy(_ data: Data, limits: ReflexLimits) throws -> ReflexRunPolicy {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              canonical(object) == data
        else { throw ReflexContractError.wire }
        guard let version = object["version"] else { throw ReflexContractError.version }
        guard let number = version as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(),
              !CFNumberIsFloatType(number as CFNumber)
        else { throw ReflexContractError.wire }
        guard number.int64Value == Int64(runPolicyVersion) else { throw ReflexContractError.version }
        guard let policy = try? JSONDecoder().decode(ReflexRunPolicy.self, from: data),
              let again = try? JSONEncoder().encode(policy),
              let reread = try? JSONSerialization.jsonObject(with: again),
              canonical(reread) == data
        else { throw ReflexContractError.wire }
        guard policy.run_ns > 0, policy.run_ns <= limits.max_run_ns else { throw ReflexContractError.budget }
        return policy
    }

    /// The capability table as the window sends it (`reflex::decode_capability`): only the
    /// canonical form with every field known is read.
    public static func decodeCapabilities(_ data: Data) throws -> ReflexCapabilityTable {
        guard let table = try? JSONDecoder().decode(ReflexCapabilityTable.self, from: data),
              let again = try? JSONEncoder().encode(table),
              let object = try? JSONSerialization.jsonObject(with: again),
              canonical(object) == data
        else { throw ReflexContractError.wire }
        return table
    }

    public static func wireBytes(_ plan: ReflexPlan) throws -> Data {
        let encoded = try JSONEncoder().encode(plan)
        let object = try JSONSerialization.jsonObject(with: encoded)
        return try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes])
    }

    public static func hash(_ plan: ReflexPlan) throws -> String {
        let encoded = try JSONEncoder().encode(plan)
        guard var object = try JSONSerialization.jsonObject(with: encoded) as? [String: Any] else { throw ReflexContractError.hash }
        object["plan_hash"] = ""
        let canonical = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes])
        return SHA256.hash(data: canonical).map { String(format: "%02x", $0) }.joined()
    }

    private static func plus(_ lhs: UInt64, _ rhs: UInt64) throws -> UInt64 {
        let result = lhs.addingReportingOverflow(rhs)
        if result.overflow { throw ReflexContractError.budget }
        return result.partialValue
    }
    private static func times(_ lhs: UInt64, _ rhs: UInt64) throws -> UInt64 {
        let result = lhs.multipliedReportingOverflow(by: rhs)
        if result.overflow { throw ReflexContractError.budget }
        return result.partialValue
    }

    private static func predicateShape(_ predicate: ReflexPredicate) throws {
        switch predicate.op {
        case .known:
            if predicate.value != nil || predicate.child != nil || predicate.children != nil { throw ReflexContractError.wire }
        case .eq:
            if predicate.value == nil || predicate.child != nil || predicate.children != nil { throw ReflexContractError.wire }
        case .not:
            guard let child = predicate.child, predicate.value == nil, predicate.children == nil else { throw ReflexContractError.wire }
            try predicateShape(child)
        case .any, .all:
            guard let children = predicate.children, predicate.value == nil, predicate.child == nil else { throw ReflexContractError.wire }
            for child in children { try predicateShape(child) }
        }
    }

    private static func check(_ predicate: ReflexPredicate, depth: UInt64, limits: ReflexLimits) throws {
        if depth > limits.max_predicate_depth { throw ReflexContractError.budget }
        switch predicate.op {
        case .known:
            if predicate.value != nil || predicate.child != nil || predicate.children != nil { throw ReflexContractError.budget }
        case .eq:
            if predicate.value == nil || predicate.child != nil || predicate.children != nil { throw ReflexContractError.budget }
        case .not:
            guard let child = predicate.child, predicate.value == nil, predicate.children == nil else { throw ReflexContractError.budget }
            try check(child, depth: depth + 1, limits: limits)
        case .any, .all:
            guard let children = predicate.children, !children.isEmpty, children.count <= Int(limits.max_actions), predicate.value == nil, predicate.child == nil else { throw ReflexContractError.budget }
            for child in children { try check(child, depth: depth + 1, limits: limits) }
        }
    }

    public static func validate(_ plan: ReflexPlan, limits: ReflexLimits, perception: PerceptionLimits) throws -> ValidatedReflexPlan {
        if plan.version != version { throw ReflexContractError.version }
        if try hash(plan) != plan.plan_hash { throw ReflexContractError.hash }
        if plan.scope.target.isEmpty || plan.scope.target.utf8.count > 128 || !plan.scope.target.utf8.allSatisfy({ (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 || $0 == 46 || $0 == 95 }) { throw ReflexContractError.scope }
        if plan.detectors.isEmpty || plan.rules.isEmpty || plan.macros.isEmpty ||
            plan.detectors.count > Int(limits.max_detectors) || plan.rules.count > Int(limits.max_rules) ||
            plan.macros.count > Int(limits.max_macros) { throw ReflexContractError.budget }
        if plan.pointer.duration_ms == 0 || plan.pointer.duration_ms > limits.max_pointer_duration_ms ||
            (plan.pointer.instant && plan.pointer.duration_ms != limits.instant_duration_ms) { throw ReflexContractError.budget }
        var detectors = Set<String>()
        var work: UInt64 = 0
        for item in plan.detectors {
            if !identifier(item.id) { throw ReflexContractError.id }
            if !detectors.insert(item.id).inserted { throw ReflexContractError.duplicate }
            if item.kind != .color { throw ReflexContractError.unsupported }
            if item.roi.space != .pixel || item.roi.x < 0 || item.roi.y < 0 || item.roi.width <= 0 || item.roi.height <= 0 || item.patches == 0 || item.scale.numerator == 0 || item.scale.denominator == 0 || item.scale.numerator > limits.max_scale_part || item.scale.denominator > limits.max_scale_part { throw ReflexContractError.budget }
            let pixels = try times(UInt64(item.roi.width), UInt64(item.roi.height))
            if pixels > limits.max_roi_pixels { throw ReflexContractError.budget }
            let numerator = try times(times(pixels, item.patches), item.scale.numerator)
            let scaled = try plus(numerator, item.scale.denominator - 1) / item.scale.denominator
            work = try plus(work, scaled)
            if work > limits.max_work_pixels { throw ReflexContractError.budget }
            // A colour detector's structure is its spec's layout: one patch at the scale it
            // was written at, and the spec's own samples bound (`game_state::validate_color`).
            guard let color = item.color else { throw ReflexContractError.perception }
            if item.patches != 1 || item.scale.numerator != 1 || item.scale.denominator != 1 {
                throw ReflexContractError.unsupported
            }
            do {
                _ = try PerceptionSpecs.validate(color, roi: item.roi, limits: perception)
            } catch {
                throw ReflexContractError.perception
            }
            // A pick chooses among blobs: a cells layout has none to choose among.
            if item.pick != .first, case .cells = color.layout { throw ReflexContractError.unsupported }
        }
        var macroIds = Set<String>()
        var actionIds = Set<String>()
        var actions: UInt64 = 0
        for item in plan.macros {
            if !identifier(item.id) { throw ReflexContractError.id }
            if !macroIds.insert(item.id).inserted { throw ReflexContractError.duplicate }
            if item.repeat == 0 || item.actions.isEmpty { throw ReflexContractError.budget }
            actions = try plus(actions, UInt64(item.actions.count))
            if actions > limits.max_actions { throw ReflexContractError.budget }
            for action in item.actions {
                if !identifier(action.id) || !identifier(action.target) { throw ReflexContractError.id }
                if !actionIds.insert(action.id).inserted { throw ReflexContractError.duplicate }
                if (action.kind == .move || action.kind == .click) && !detectors.contains(action.target) { throw ReflexContractError.reference }
                if action.kind == .key { throw ReflexContractError.unsupported }
            }
        }
        let byId = Dictionary(uniqueKeysWithValues: plan.macros.map { ($0.id, $0) })
        func expanded(_ id: String, _ visited: Set<String>, _ depth: UInt64) throws -> UInt64 {
            if depth > limits.max_macro_depth { throw ReflexContractError.budget }
            if visited.contains(id) { throw ReflexContractError.cycle }
            guard let item = byId[id] else { throw ReflexContractError.reference }
            var count: UInt64 = 0
            let nextVisited = visited.union([id])
            for action in item.actions {
                count = try plus(count, action.kind == .macro ? expanded(action.target, nextVisited, depth + 1) : 1)
            }
            let total = try times(count, item.repeat)
            if total > limits.max_expanded_actions { throw ReflexContractError.budget }
            return total
        }
        // Every declaration must be a bounded, referentially valid DAG, even if
        // no rule currently reaches it. Rule max_fires is accounted for below.
        for item in plan.macros {
            _ = try expanded(item.id, [], 0)
        }
        var ruleIds = Set<String>()
        var total: UInt64 = 0
        for rule in plan.rules {
            if !identifier(rule.id) { throw ReflexContractError.id }
            if !ruleIds.insert(rule.id).inserted { throw ReflexContractError.duplicate }
            if !detectors.contains(rule.detector) { throw ReflexContractError.reference }
            try check(rule.predicate, depth: 0, limits: limits)
            if rule.max_fires == 0 || rule.cooldown_ms > limits.max_cooldown_ms { throw ReflexContractError.budget }
            total = try plus(total, times(expanded(rule.macro_id, [], 0), rule.max_fires))
            if total > limits.max_expanded_actions { throw ReflexContractError.budget }
        }
        return ValidatedReflexPlan(plan)
    }
}

public struct ReflexPixelExtent: Codable, Equatable, Sendable {
    public let width: UInt32
    public let height: UInt32

    public init(width: UInt32, height: UInt32) {
        self.width = width
        self.height = height
    }
}
public struct ReflexPointTransform: Codable, Equatable, Sendable {
    public let origin_x: Int64
    public let origin_y: Int64
    public let points_per_pixel: ReflexScale

    public init(origin_x: Int64, origin_y: Int64, points_per_pixel: ReflexScale) {
        self.origin_x = origin_x
        self.origin_y = origin_y
        self.points_per_pixel = points_per_pixel
    }
}
public enum ReflexOrientation: String, Codable, Sendable { case up, right, down, left }
public enum ReflexColorSpace: String, Codable, Sendable { case srgb, display_p3, unknown }
public enum ReflexFrameStatus: String, Codable, Sendable { case ready, stale, interrupted }

public struct ReflexFrameFacts: Codable, Equatable, Sendable {
    public let run_id: String
    public let display_id: String
    public let region: ReflexRoi
    public let pixel_extent: ReflexPixelExtent
    public let point_transform: ReflexPointTransform
    public let orientation: ReflexOrientation
    public let color_space: ReflexColorSpace
    public let status: ReflexFrameStatus
    public let dirty: Bool
    public let capture_gap: UInt64
    public let delivered_host_ns: UInt64?
    public let capture_seq: UInt64
    public let repaint_seq: UInt64
    public let stream_epoch: UInt64
    public let owner_epoch: UInt64
    public let geometry_epoch: UInt64
    public let plan_epoch: UInt64
    public let clock_domain: UInt64
    public let captured_host_ns: UInt64?

    /// When the frame was in hand on the host clock (`reflex::FrameFacts::observed_host_ns`):
    /// its capture time, or its delivery when that came first. ScreenCaptureKit stamps a frame
    /// with the time the display shows it, which can stand ahead of the moment the frame is
    /// delivered and read (t-10127). Nil when the capture time is unknown: the delivery time
    /// never stands in for it.
    public var observed_host_ns: UInt64? {
        guard let captured = captured_host_ns else { return nil }
        return delivered_host_ns.map { min(captured, $0) } ?? captured
    }
}

public enum ReflexLeaseInput: String, Codable, Sendable { case pointer_move, left_click }

public struct ReflexActionLease: Codable, Equatable, Sendable {
    public let run_id: String
    public let action_id: String
    public let target_id: String
    public let target_roi: ReflexRoi
    public let allowed_inputs: Set<ReflexLeaseInput>
    public let owner_epoch: UInt64
    public let stream_epoch: UInt64
    public let geometry_epoch: UInt64
    public let plan_epoch: UInt64
    public let clock_domain: UInt64
    public let source_capture_seq: UInt64
    public let issued_host_ns: UInt64
    public let valid_until_host_ns: UInt64
    public let target_proof_until_host_ns: UInt64
    public let max_children: UInt64
    public let used_children: UInt64

    /// The same contract as Rust `ActionLease::permits`, pinned for both by the shared
    /// `fixtures/reflex-contract/lease_cases.json`. The frame must be ready and nonempty,
    /// share the lease's nonempty run and its epochs, and be a newer capture than
    /// `source_capture_seq`: the capture that issued the lease never satisfies it. Its age
    /// counts from when it was in hand (`observed_host_ns`), and an unknown capture time is
    /// never replaced by the delivery time. Expiry and proof ends are exclusive; the frame
    /// age and lease length limits are inclusive.
    public func permits(_ frame: ReflexFrameFacts, now_host_ns: UInt64, input: ReflexLeaseInput, limits: ReflexLimits) -> Bool {
        !run_id.isEmpty && run_id == frame.run_id && allowed_inputs.contains(input) &&
            frame.status == .ready && frame.pixel_extent.width > 0 && frame.pixel_extent.height > 0 &&
            frame.observed_host_ns.map({ $0 <= now_host_ns && now_host_ns - $0 <= limits.max_frame_age_ns }) == true &&
            issued_host_ns <= now_host_ns && valid_until_host_ns > issued_host_ns &&
            valid_until_host_ns - issued_host_ns <= limits.max_lease_ns &&
            target_proof_until_host_ns <= valid_until_host_ns &&
            !target_id.isEmpty && target_roi.width > 0 && target_roi.height > 0 &&
            owner_epoch == frame.owner_epoch && stream_epoch == frame.stream_epoch &&
            geometry_epoch == frame.geometry_epoch && plan_epoch == frame.plan_epoch &&
            clock_domain == frame.clock_domain &&
            frame.capture_seq > source_capture_seq && now_host_ns < valid_until_host_ns &&
            now_host_ns < target_proof_until_host_ns && used_children < max_children &&
            max_children <= limits.max_expanded_actions
    }
}

/// A run's own terms beside its plan (`reflex::RunPolicy`): how long it may last from the
/// moment the helper first accepts it, and whether a rule whose `max_fires` are spent gets
/// exactly those back. The plan and its hash stay what they are.
public struct ReflexRunPolicy: Codable, Equatable, Sendable {
    public let version: UInt32
    public let run_ns: UInt64
    public let renew: Bool

    public init(version: UInt32, run_ns: UInt64, renew: Bool) {
        self.version = version
        self.run_ns = run_ns
        self.renew = renew
    }
}

/// What one surface may claim (`reflex::SurfaceCapability`): a live reflex run, and an
/// instant pointer on the helper's own verbs — two columns, because running plans does
/// not teach the verbs an instant pointer.
public struct ReflexSurfaceCapability: Codable, Equatable, Sendable {
    public let live_reflex: Bool
    public let instant_pointer: Bool
}

public struct ReflexCapabilitySurfaces: Codable, Equatable, Sendable {
    public let ios_device: ReflexSurfaceCapability
    public let macos_desktop: ReflexSurfaceCapability
    public let windows_desktop: ReflexSurfaceCapability
}

/// The one capability table (`fixtures/reflex-contract/capability.json`, `reflex::CapabilityTable`)
/// as the window sends it with a start: each surface's claims, for the plan contract and
/// the run policy it was written against.
public struct ReflexCapabilityTable: Codable, Equatable, Sendable {
    public let contract: UInt32
    public let run_policy: UInt32
    public let surfaces: ReflexCapabilitySurfaces

    public func surface(_ surface: ReflexSurface) -> ReflexSurfaceCapability {
        switch surface {
        case .macos_desktop: return surfaces.macos_desktop
        case .ios_device: return surfaces.ios_device
        case .windows_desktop: return surfaces.windows_desktop
        }
    }
}

public struct ReflexCapability: Codable, Equatable, Sendable {
    public let schema_version: UInt32
    public let live_reflex: Bool
    public let instant_pointer: Bool
}

public extension ReflexContract {
    /// What `table` claims for `surface` (`reflex::capability_in`): nothing unless the table
    /// was written for this contract and this run policy — a helper of another version reads
    /// a table that claims nothing.
    static func capability(_ surface: ReflexSurface, table: ReflexCapabilityTable) -> ReflexCapability {
        let current = table.contract == version && table.run_policy == runPolicyVersion
        let row = table.surface(surface)
        return ReflexCapability(schema_version: version, live_reflex: current && row.live_reflex,
                                instant_pointer: current && row.instant_pointer)
    }
}

/// The same contract as Rust `FrameCursor`. One cursor belongs to one run: a frame from
/// another run is refused, so a new run needs a new cursor. Stream epochs never go back;
/// within an epoch the capture sequence advances and the repaint sequence does not
/// regress, and a later epoch starts a new sequence. Only ready, nonempty frames with a
/// known capture time are observed. The cursor has no clock and grants no input:
/// `ReflexActionLease.permits` checks capture age and expiry at action time.
public struct ReflexFrameCursor {
    private var runId: String?
    private var streamEpoch: UInt64?
    private var captureSeq: UInt64 = 0
    private var repaintSeq: UInt64 = 0

    public init() {}

    public mutating func observe(_ frame: ReflexFrameFacts) -> Bool {
        if frame.run_id.isEmpty || (runId != nil && runId != frame.run_id) ||
            frame.status != .ready || frame.pixel_extent.width == 0 || frame.pixel_extent.height == 0 ||
            frame.captured_host_ns == nil || frame.capture_seq == 0 ||
            streamEpoch.map({ frame.stream_epoch < $0 }) == true { return false }
        if streamEpoch == frame.stream_epoch &&
            (frame.capture_seq <= captureSeq || frame.repaint_seq < repaintSeq) { return false }
        runId = frame.run_id
        streamEpoch = frame.stream_epoch
        captureSeq = frame.capture_seq
        repaintSeq = frame.repaint_seq
        return true
    }
}

// MARK: - What a perception kernel answers (v2, with R5's kernel)

/// Why a detector's value is unknown on a frame (`reflex::Unknown`). Unknown never
/// authorizes input, and `not(unknown)` is unknown too.
public enum ReflexUnknown: String, Codable, Sendable {
    case frame, color_space, extent, orientation, budget, ambiguous, occluded, scene, unconfirmed
}

/// The frame an observation was made on, as the runtime handed it over. The runtime
/// observes only frames its `ReflexFrameCursor` accepted, so the capture time is known.
public struct ReflexFrameRef: Codable, Equatable, Sendable {
    public let run_id: String
    public let stream_epoch: UInt64
    public let capture_seq: UInt64
    public let repaint_seq: UInt64
    public let geometry_epoch: UInt64
    public let plan_epoch: UInt64
    public let owner_epoch: UInt64
    public let clock_domain: UInt64
    public let captured_host_ns: UInt64

    /// The reference to `frame`, or nil when its capture time is unknown.
    public init?(_ frame: ReflexFrameFacts) {
        guard let captured = frame.captured_host_ns else { return nil }
        run_id = frame.run_id
        stream_epoch = frame.stream_epoch
        capture_seq = frame.capture_seq
        repaint_seq = frame.repaint_seq
        geometry_epoch = frame.geometry_epoch
        plan_epoch = frame.plan_epoch
        owner_epoch = frame.owner_epoch
        clock_domain = frame.clock_domain
        captured_host_ns = captured
    }
}

/// What a sighting points at (`reflex::Target`): a track that keeps its number while the
/// target moves and changes it on another scene, its hitbox in frame pixels (the spatial
/// proof), the point to aim at, its velocity in pixels a second and the kernel's
/// uncertainty about the point in pixels.
public struct ReflexTarget: Codable, Equatable, Sendable {
    public let track_id: UInt64
    public let roi: ReflexRoi
    public let point_x: Int64
    public let point_y: Int64
    public let velocity_x: Int64
    public let velocity_y: Int64
    public let uncertainty: UInt64

    public init(track_id: UInt64, roi: ReflexRoi, point_x: Int64, point_y: Int64, velocity_x: Int64, velocity_y: Int64, uncertainty: UInt64) {
        self.track_id = track_id
        self.roi = roi
        self.point_x = point_x
        self.point_y = point_y
        self.velocity_x = velocity_x
        self.velocity_y = velocity_y
        self.uncertainty = uncertainty
    }

    /// Where to press at `atHostNs` (`reflex::Target::aim`): the point carried forward by the
    /// velocity since `capturedHostNs`, while the square of the uncertainty around it stays
    /// inside the hitbox carried the same way. A capture stamped after `atHostNs` — a display
    /// time that runs ahead of its reading (`ReflexFrameFacts.observed_host_ns`) — carries
    /// nothing. Nil — do not press — when time runs past `maxAgeNs`, a carry overflows, or the
    /// square leaves the hitbox.
    public func aim(atHostNs: UInt64, capturedHostNs: UInt64, maxAgeNs: UInt64) -> (x: Int64, y: Int64)? {
        let since = atHostNs >= capturedHostNs ? atHostNs - capturedHostNs : 0
        guard since <= maxAgeNs, let elapsed = Int64(exactly: since) else { return nil }
        func carry(_ velocity: Int64) -> Int64? {
            let moved = velocity.multipliedReportingOverflow(by: elapsed)
            return moved.overflow ? nil : moved.partialValue / 1_000_000_000
        }
        func add(_ lhs: Int64, _ rhs: Int64) -> Int64? {
            let sum = lhs.addingReportingOverflow(rhs)
            return sum.overflow ? nil : sum.partialValue
        }
        func less(_ lhs: Int64, _ rhs: Int64) -> Int64? {
            let difference = lhs.subtractingReportingOverflow(rhs)
            return difference.overflow ? nil : difference.partialValue
        }
        guard let dx = carry(velocity_x), let dy = carry(velocity_y),
              let x = add(point_x, dx), let y = add(point_y, dy),
              let reach = Int64(exactly: uncertainty),
              let left = add(roi.x, dx), let top = add(roi.y, dy),
              let right = add(left, roi.width), let bottom = add(top, roi.height),
              let west = less(x, reach), let east = add(x, reach),
              let north = less(y, reach), let south = add(y, reach)
        else { return nil }
        return west >= left && east < right && north >= top && south < bottom ? (x, y) : nil
    }
}

/// One cell of a cells layout (`reflex::Cell`): its class numbered from 1 in palette
/// order, or 0 with the reason it is unknown, and the share its winner holds.
public struct ReflexCell: Codable, Equatable, Sendable {
    public let `class`: UInt64
    public let unknown: ReflexUnknown?
    public let share_permille: UInt64

    public init(class klass: UInt64, unknown: ReflexUnknown?, share_permille: UInt64) {
        self.class = klass
        self.unknown = unknown
        self.share_permille = share_permille
    }
}

/// A detector's reading of one frame as the kernel answers it (`reflex::Observation`):
/// data, never permission. The runtime reads it only when `admissible`, and issues any
/// lease itself.
public struct ReflexObservation: Codable, Equatable, Sendable {
    public let detector_id: String
    public let frame: ReflexFrameRef
    public let unknown: ReflexUnknown?
    /// The predicate's input: known exactly when `unknown` is nil; zero means nothing is there.
    public let value: Int64?
    public let target: ReflexTarget?
    /// The frame's scale over the spec's reference extent.
    public let scale: ReflexScale
    public let cells: [ReflexCell]?
    /// Samples this observation read.
    public let samples: UInt64

    public init(detector_id: String, frame: ReflexFrameRef, unknown: ReflexUnknown?, value: Int64?, target: ReflexTarget?, scale: ReflexScale, cells: [ReflexCell]?, samples: UInt64) {
        self.detector_id = detector_id
        self.frame = frame
        self.unknown = unknown
        self.value = value
        self.target = target
        self.scale = scale
        self.cells = cells
        self.samples = samples
    }

    /// Whether the runtime may take this as the kernel's word about `frame` for `detector`
    /// (`reflex::Observation::admissible`, pinned for both by `observation_cases.json`).
    public func admissible(for detector: ReflexDetector, frame: ReflexFrameFacts, budgetSamples: UInt64) -> Bool {
        guard let color = detector.color else { return false }
        if detector_id != detector.id || ReflexFrameRef(frame) != self.frame || samples > budgetSamples ||
            (unknown != nil) == (value != nil) || (value.map { $0 < 0 } ?? false) {
            return false
        }
        guard let placed = PerceptionSpecs.frameRoi(detector.roi, spec: color, frame: frame.pixel_extent) else {
            return value == nil && target == nil && cells == nil
        }
        if scale != placed.scale { return false }
        if let target {
            guard let value, value != 0, target.track_id != 0,
                  Self.within(target.roi, placed.roi),
                  Self.holds(target.roi, x: target.point_x, y: target.point_y)
            else { return false }
        }
        if let cells {
            guard case let .cells(layout) = color.layout else { return false }
            let product = layout.rows.multipliedReportingOverflow(by: layout.columns)
            let classes = UInt64(color.classes.count)
            if product.overflow || product.partialValue != UInt64(cells.count) ||
                cells.contains(where: { $0.share_permille > 1000 || $0.class > classes || ($0.class == 0) != ($0.unknown != nil) }) {
                return false
            }
        }
        return true
    }

    private static func within(_ inner: ReflexRoi, _ outer: ReflexRoi) -> Bool {
        guard inner.space == .pixel, outer.space == .pixel, inner.width > 0, inner.height > 0,
              inner.x >= outer.x, inner.y >= outer.y else { return false }
        let innerRight = inner.x.addingReportingOverflow(inner.width), outerRight = outer.x.addingReportingOverflow(outer.width)
        let innerBottom = inner.y.addingReportingOverflow(inner.height), outerBottom = outer.y.addingReportingOverflow(outer.height)
        return !innerRight.overflow && !outerRight.overflow && !innerBottom.overflow && !outerBottom.overflow &&
            innerRight.partialValue <= outerRight.partialValue && innerBottom.partialValue <= outerBottom.partialValue
    }

    private static func holds(_ roi: ReflexRoi, x: Int64, y: Int64) -> Bool {
        let right = roi.x.addingReportingOverflow(roi.width), bottom = roi.y.addingReportingOverflow(roi.height)
        return !right.overflow && !bottom.overflow && x >= roi.x && y >= roi.y && x < right.partialValue && y < bottom.partialValue
    }
}

/// The pixels of one frame, lent for one `observe` call: BGRA, `bytesPerRow` at least
/// `width * 4`, the frame's own buffer as the runtime locked it — never kept past the call.
public struct ReflexPixels {
    public let base: UnsafeRawPointer
    public let width: Int
    public let height: Int
    public let bytesPerRow: Int

    public init(base: UnsafeRawPointer, width: Int, height: Int, bytesPerRow: Int) {
        self.base = base
        self.width = width
        self.height = height
        self.bytesPerRow = bytesPerRow
    }
}

/// How much one tick may read (`PerceptionLimits.max_tick_samples` and `max_tick_ns`, the
/// window's table): a kernel spends a detector's samples before reading them, and a
/// detector the budget cannot pay for — and every one after it — is `unknown(budget)`.
public struct ReflexPerceptionBudget {
    public private(set) var samples: UInt64
    public let deadlineHostNs: UInt64
    public let now: @Sendable () -> UInt64

    public init(samples: UInt64, deadlineHostNs: UInt64, now: @escaping @Sendable () -> UInt64) {
        self.samples = samples
        self.deadlineHostNs = deadlineHostNs
        self.now = now
    }

    /// Spend `cost` samples if the tick can still pay for them before its deadline; false
    /// leaves the budget as it was.
    public mutating func spend(_ cost: UInt64) -> Bool {
        guard cost <= samples, now() < deadlineHostNs else { return false }
        samples -= cost
        return true
    }
}

/// One run's perception: made on the runtime's evaluating thread when the run starts and
/// called only there, one frame at a time, so it may keep what it tracks across frames.
public protocol ReflexPerceptionSession: AnyObject {
    /// The plan's detectors on `frame`, in plan order. `frame` is one the runtime's cursor
    /// accepted; `pixels` are its buffer, lent for the call. `hand` is where the run's last fire
    /// sent the hand — the point of the target its last leaf acts on, in frame pixels, as the
    /// frame it fired on showed it — nil before its first fire: what a detector picking by
    /// nearness picks near (`ReflexPick.nearest`).
    func observe(frame: ReflexFrameFacts, pixels: ReflexPixels, hand: (x: Int64, y: Int64)?,
                 budget: inout ReflexPerceptionBudget) -> [ReflexObservation]
}

/// A kernel that reads a validated plan's detectors (R5's `Perception/`). It never issues
/// a lease or posts input: what it answers is data.
public protocol ReflexPerceptionKernel: Sendable {
    /// A session for one run; throws when the plan asks for what the kernel cannot read.
    func session(for plan: ValidatedReflexPlan, limits: PerceptionLimits) throws -> any ReflexPerceptionSession
}
