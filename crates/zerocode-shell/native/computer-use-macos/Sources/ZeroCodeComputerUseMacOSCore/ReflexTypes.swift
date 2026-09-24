import CryptoKit
import Foundation

/// Wire values have no input authority until `ReflexContract.validate` succeeds.
public struct ReflexPlan: Codable {
    public let version: UInt32
    public let plan_hash: String
    public let scope: ReflexScope
    public let detectors: [ReflexDetector]
    public let rules: [ReflexRule]
    public let macros: [ReflexMacro]
    public let pointer: ReflexPointerStyle
}

public struct ReflexScope: Codable {
    public let surface: ReflexSurface
    public let target: String
}
public enum ReflexSurface: String, Codable { case macos_desktop, ios_device, windows_desktop }
public enum ReflexCoordinateSpace: String, Codable { case pixel, point, css, device }
public struct ReflexRoi: Codable {
    public let x: Int64
    public let y: Int64
    public let width: Int64
    public let height: Int64
    public let space: ReflexCoordinateSpace
}
public struct ReflexScale: Codable {
    public let numerator: UInt64
    public let denominator: UInt64
}
public enum ReflexDetectorKind: String, Codable { case color, template, motion }
public struct ReflexDetector: Codable {
    public let id: String
    public let kind: ReflexDetectorKind
    public let roi: ReflexRoi
    public let patches: UInt64
    public let scale: ReflexScale
}
public enum ReflexPredicateOp: String, Codable { case known, eq, not, any, all }
public final class ReflexPredicate: Codable {
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
public enum ReflexTruth { case yes, no, unknown }
public struct ReflexRule: Codable {
    public let id: String
    public let detector: String
    public let predicate: ReflexPredicate
    public let macro_id: String
    public let priority: Int64
    public let cooldown_ms: UInt64
    public let max_fires: UInt64
}
public enum ReflexActionKind: String, Codable { case move, click, key, macro }
public struct ReflexAction: Codable {
    public let id: String
    public let kind: ReflexActionKind
    public let target: String
}
public struct ReflexMacro: Codable {
    public let id: String
    public let `repeat`: UInt64
    public let actions: [ReflexAction]
}
public enum ReflexPointerCurve: String, Codable { case linear, cosine }
public struct ReflexPointerStyle: Codable {
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
    public let max_scale_part: UInt64
    public let max_pointer_duration_ms: UInt64
    public let instant_duration_ms: UInt64
    public let max_frame_age_ns: UInt64
}

public enum ReflexContractError: String, Error { case wire, version, hash, scope, id, duplicate, reference, cycle, budget, unsupported }

/// A helper must use this return type, never a decoded `ReflexPlan`, for input.
public struct ValidatedReflexPlan {
    public let plan: ReflexPlan
    fileprivate init(_ plan: ReflexPlan) { self.plan = plan }
}

public enum ReflexContract {
    public static let version: UInt32 = 1


    public static func decodeAndValidate(_ data: Data, limits: ReflexLimits) throws -> ValidatedReflexPlan {
        guard let object = try? JSONSerialization.jsonObject(with: data),
              let canonical = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys, .withoutEscapingSlashes]),
              canonical == data,
              let plan = try? JSONDecoder().decode(ReflexPlan.self, from: data)
        else { throw ReflexContractError.wire }
        guard let typedWire = try? wireBytes(plan), typedWire == data else { throw ReflexContractError.wire }
        for rule in plan.rules { try predicateShape(rule.predicate) }
        return try validate(plan, limits: limits)
    }

    private static func identifier(_ text: String) -> Bool {
        guard !text.isEmpty, text.utf8.count <= 64 else { return false }
        return text.utf8.allSatisfy { (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 || $0 == 95 }
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

    public static func validate(_ plan: ReflexPlan, limits: ReflexLimits) throws -> ValidatedReflexPlan {
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

public struct ReflexPixelExtent: Codable {
    public let width: UInt32
    public let height: UInt32
}
public struct ReflexPointTransform: Codable {
    public let origin_x: Int64
    public let origin_y: Int64
    public let points_per_pixel: ReflexScale
}
public enum ReflexOrientation: String, Codable { case up, right, down, left }
public enum ReflexColorSpace: String, Codable { case srgb, display_p3, unknown }
public enum ReflexFrameStatus: String, Codable { case ready, stale, interrupted }

public struct ReflexFrameFacts: Codable {
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
}

public enum ReflexLeaseInput: String, Codable { case pointer_move, left_click }

public struct ReflexActionLease: Codable {
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
    /// `source_capture_seq`: the capture that issued the lease never satisfies it. An
    /// unknown capture time is never replaced by the delivery time. Expiry and proof ends
    /// are exclusive; the frame age and lease length limits are inclusive.
    public func permits(_ frame: ReflexFrameFacts, now_host_ns: UInt64, input: ReflexLeaseInput, limits: ReflexLimits) -> Bool {
        !run_id.isEmpty && run_id == frame.run_id && allowed_inputs.contains(input) &&
            frame.status == .ready && frame.pixel_extent.width > 0 && frame.pixel_extent.height > 0 &&
            frame.captured_host_ns.map({ $0 <= now_host_ns && now_host_ns - $0 <= limits.max_frame_age_ns }) == true &&
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

public struct ReflexCapability: Codable, Equatable {
    public let schema_version: UInt32
    public let live_reflex: Bool
}

public extension ReflexContract {
    static func capability(_ surface: ReflexSurface) -> ReflexCapability {
        _ = surface
        return ReflexCapability(schema_version: version, live_reflex: false)
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
