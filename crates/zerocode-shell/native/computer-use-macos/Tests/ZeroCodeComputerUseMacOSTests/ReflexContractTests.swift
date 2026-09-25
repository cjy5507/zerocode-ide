import Foundation
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class ReflexContractTests: XCTestCase {
    private var coreFixtures: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent() // test module
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // computer-use-macos
            .deletingLastPathComponent() // native
            .deletingLastPathComponent() // zerocode-shell
            .deletingLastPathComponent() // crates
            .appendingPathComponent("zerocode-core/fixtures")
    }

    private var fixtureRoot: URL { coreFixtures.appendingPathComponent("reflex-contract") }

    private func fixture(_ name: String) throws -> Data {
        try Data(contentsOf: fixtureRoot.appendingPathComponent("\(name).json"))
    }

    /// Every file is canonical JSON and a final newline, so the plan's wire is
    /// the file's own bytes inside the envelope — the same bytes Rust compares
    /// its output against, whichever map order its build keeps.
    private func golden(_ name: String) throws -> (expected: String, wire: Data) {
        let raw = try fixture(name)
        let envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: raw) as? [String: Any], name)
        let expected = try XCTUnwrap(envelope["expected"] as? String, name)
        let prefix = Data("{\"expected\":\"\(expected)\",\"plan\":".utf8)
        let suffix = Data("}\n".utf8)
        XCTAssertTrue(raw.starts(with: prefix) && raw.suffix(suffix.count) == suffix, "\(name) is not written canonical")
        return (expected, Data(raw.dropFirst(prefix.count).dropLast(suffix.count)))
    }

    /// The reflex table exactly as the window sends it (`reflex::limits_wire`).
    private func contractLimits() throws -> ReflexLimits {
        try ReflexContract.decodeLimits(Data(try fixture("limits").dropLast()))
    }

    /// R5's perception table (`game_state::limits_wire`), which the window sends beside it.
    private func perceptionLimits() throws -> PerceptionLimits {
        let raw = try Data(contentsOf: coreFixtures.appendingPathComponent("game-state/limits.json"))
        return try PerceptionSpecs.decodeLimits(Data(raw.dropLast()))
    }

    func testSwiftAndRustReadTheSameVersionedPlan() throws {
        let manifest = try XCTUnwrap(JSONSerialization.jsonObject(with: fixture("manifest")) as? [String: Any])
        let names = try XCTUnwrap(manifest["semantic"] as? [String])
        let limits = try contractLimits()
        let perception = try perceptionLimits()
        for name in names {
            let (expected, planData) = try golden(name)
            if expected == "ok" {
                let validated = try ReflexContract.decodeAndValidate(planData, limits: limits, perception: perception)
                XCTAssertEqual(validated.plan.version, ReflexContract.version)
                XCTAssertEqual(try ReflexContract.hash(validated.plan), validated.plan.plan_hash)
                XCTAssertEqual(try ReflexContract.wireBytes(validated.plan), planData)
            } else {
                XCTAssertThrowsError(try ReflexContract.decodeAndValidate(planData, limits: limits, perception: perception), name) { error in
                    XCTAssertEqual((error as? ReflexContractError)?.rawValue, expected, name)
                }
            }
        }
        // R1's 32 cases under version 2, the six version 2 adds, and the identifier
        // bound's two sides (t-9205).
        XCTAssertEqual(names.count, 40)
        let wireNames = try XCTUnwrap(manifest["wire_negative"] as? [String])
        for name in wireNames {
            let data = try Data(contentsOf: fixtureRoot.appendingPathComponent("\(name).txt"))
            XCTAssertThrowsError(try ReflexContract.decodeAndValidate(data, limits: limits, perception: perception), name) { error in
                XCTAssertEqual((error as? ReflexContractError)?.rawValue, "wire", name)
            }
        }
        XCTAssertEqual(wireNames.count, 11)
    }

    func testSharedLeaseCasesExercisePermitsAndFrameCursor() throws {
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: self.fixture("lease_cases")) as? [String: Any])
        let baseFrame = try XCTUnwrap(fixture["frame"] as? [String: Any])
        let baseLease = try XCTUnwrap(fixture["lease"] as? [String: Any])
        let limits = try contractLimits()
        let cases = try XCTUnwrap(fixture["cases"] as? [[String: Any]])
        for row in cases {
            let name = try XCTUnwrap(row["name"] as? String)
            let frameData = try JSONSerialization.data(withJSONObject: baseFrame.merging(try XCTUnwrap(row["frame"] as? [String: Any])) { _, new in new })
            let leaseData = try JSONSerialization.data(withJSONObject: baseLease.merging(try XCTUnwrap(row["lease"] as? [String: Any])) { _, new in new })
            let frame = try JSONDecoder().decode(ReflexFrameFacts.self, from: frameData)
            let lease = try JSONDecoder().decode(ReflexActionLease.self, from: leaseData)
            let input = try XCTUnwrap(ReflexLeaseInput(rawValue: try XCTUnwrap(row["input"] as? String)))
            XCTAssertEqual(lease.permits(frame, now_host_ns: try XCTUnwrap(row["now_host_ns"] as? UInt64), input: input, limits: limits),
                try XCTUnwrap(row["expected"] as? Bool), name)
        }
        let cursorCases = try XCTUnwrap(fixture["cursor_cases"] as? [[String: Any]])
        for row in cursorCases {
            let name = try XCTUnwrap(row["name"] as? String)
            let frames = try XCTUnwrap(row["frames"] as? [[String: Any]])
            let expected = try XCTUnwrap(row["expected"] as? [Bool])
            XCTAssertEqual(frames.count, expected.count, name)
            var cursor = ReflexFrameCursor()
            for (patch, verdict) in zip(frames, expected) {
                let data = try JSONSerialization.data(withJSONObject: baseFrame.merging(patch) { _, new in new })
                let frame = try JSONDecoder().decode(ReflexFrameFacts.self, from: data)
                XCTAssertEqual(cursor.observe(frame), verdict, name)
            }
        }
    }

    /// A case replaces top-level fields; a null removes an optional field — except a frame's
    /// capture time, whose null is the value "unknown".
    private func patched(_ base: [String: Any], _ patch: [String: Any]) -> [String: Any] {
        var out = base
        for (key, value) in patch {
            out[key] = value is NSNull && key != "captured_host_ns" ? nil : value
        }
        return out
    }

    private func decode<T: Decodable>(_ type: T.Type, _ object: Any) throws -> T {
        try JSONDecoder().decode(type, from: JSONSerialization.data(withJSONObject: object))
    }

    func testSharedObservationCasesRunThroughAdmissibleAndAim() throws {
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: self.fixture("observation_cases")) as? [String: Any])
        let baseDetector = try XCTUnwrap(fixture["detector"] as? [String: Any])
        let baseFrame = try XCTUnwrap(fixture["frame"] as? [String: Any])
        let baseObservation = try XCTUnwrap(fixture["observation"] as? [String: Any])
        let baseTarget = try XCTUnwrap(fixture["target"] as? [String: Any])
        let samples = try XCTUnwrap(fixture["samples"] as? UInt64)
        var mismatches: [String] = []
        let cases = try XCTUnwrap(fixture["cases"] as? [[String: Any]])
        for row in cases {
            let name = try XCTUnwrap(row["name"] as? String)
            let detector = try decode(ReflexDetector.self, patched(baseDetector, try XCTUnwrap(row["detector"] as? [String: Any])))
            let frame = try decode(ReflexFrameFacts.self, patched(baseFrame, try XCTUnwrap(row["frame"] as? [String: Any])))
            let observation = try decode(ReflexObservation.self, patched(baseObservation, try XCTUnwrap(row["observation"] as? [String: Any])))
            let got = observation.admissible(for: detector, frame: frame, budgetSamples: samples)
            if got != (try XCTUnwrap(row["expected"] as? Bool)) { mismatches.append("admissible \(name): got \(got)") }
        }
        let aims = try XCTUnwrap(fixture["aim_cases"] as? [[String: Any]])
        for row in aims {
            let name = try XCTUnwrap(row["name"] as? String)
            let target = try decode(ReflexTarget.self, patched(baseTarget, try XCTUnwrap(row["target"] as? [String: Any])))
            let got = target.aim(
                atHostNs: try XCTUnwrap(row["at_host_ns"] as? UInt64),
                capturedHostNs: try XCTUnwrap(row["captured_host_ns"] as? UInt64),
                maxAgeNs: try XCTUnwrap(row["max_age_ns"] as? UInt64)
            )
            let expected = (row["expected"] as? [Int64]).map { ($0[0], $0[1]) }
            if got?.x != expected?.0 || got?.y != expected?.1 || (got == nil) != (expected == nil) {
                mismatches.append("aim \(name): got \(String(describing: got))")
            }
        }
        XCTAssertEqual(mismatches, [])
        XCTAssertEqual(cases.count, 34)
        XCTAssertEqual(aims.count, 9)
    }

    func testTheReflexTableIsReadOnlyInItsCanonicalForm() throws {
        let raw = Data(try fixture("limits").dropLast())
        let limits = try ReflexContract.decodeLimits(raw)
        XCTAssertGreaterThan(limits.frames_per_second, 0)
        XCTAssertNoThrow(try ReflexTable.check(limits))
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: raw) as? [String: Any])
        var extra = object
        extra["a_field_the_helper_does_not_know"] = 1
        let widened = try JSONSerialization.data(withJSONObject: extra, options: [.sortedKeys, .withoutEscapingSlashes])
        XCTAssertThrowsError(try ReflexContract.decodeLimits(widened))
        var missing = object
        missing["pointer_tick_ns"] = nil
        let narrowed = try JSONSerialization.data(withJSONObject: missing, options: [.sortedKeys, .withoutEscapingSlashes])
        XCTAssertThrowsError(try ReflexContract.decodeLimits(narrowed))
        let spaced = Data(" ".utf8) + raw
        XCTAssertThrowsError(try ReflexContract.decodeLimits(spaced))
    }

    /// The capability table is one file both sides read (`reflex::capability_wire` sends it,
    /// Rust's `swift_and_rust_read_one_capability_table` reads it): the helper decodes the
    /// file's own bytes, each surface claims its row, and a table written for another
    /// contract or run-policy version claims nothing.
    func test_swift_and_rust_read_one_capability_table() throws {
        let raw = try fixture("capability")
        XCTAssertEqual(raw.last, UInt8(ascii: "\n"))
        let wire = Data(raw.dropLast())
        let table = try ReflexContract.decodeCapabilities(wire)
        XCTAssertEqual(table.contract, ReflexContract.version)
        XCTAssertEqual(table.run_policy, ReflexContract.runPolicyVersion)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: wire) as? [String: Any])
        let surfaces = try XCTUnwrap(object["surfaces"] as? [String: [String: Bool]])
        for surface in [ReflexSurface.macos_desktop, .ios_device, .windows_desktop] {
            let row = try XCTUnwrap(surfaces[surface.rawValue], surface.rawValue)
            let claimed = ReflexContract.capability(surface, table: table)
            XCTAssertEqual(claimed, ReflexCapability(schema_version: ReflexContract.version,
                                                     live_reflex: try XCTUnwrap(row["live_reflex"]),
                                                     instant_pointer: try XCTUnwrap(row["instant_pointer"])), surface.rawValue)
        }
        // A table claiming everything, written for another contract or run policy.
        func generous(contract: UInt32, runPolicy: UInt32) throws -> ReflexCapabilityTable {
            var changed = object
            changed["contract"] = contract
            changed["run_policy"] = runPolicy
            let all = ["instant_pointer": true, "live_reflex": true]
            changed["surfaces"] = ["ios_device": all, "macos_desktop": all, "windows_desktop": all]
            return try ReflexContract.decodeCapabilities(try JSONSerialization.data(withJSONObject: changed, options: [.sortedKeys, .withoutEscapingSlashes]))
        }
        XCTAssertTrue(ReflexContract.capability(.windows_desktop, table: try generous(contract: ReflexContract.version, runPolicy: ReflexContract.runPolicyVersion)).live_reflex)
        for stale in [try generous(contract: ReflexContract.version + 1, runPolicy: ReflexContract.runPolicyVersion),
                      try generous(contract: ReflexContract.version, runPolicy: ReflexContract.runPolicyVersion + 1)] {
            for surface in [ReflexSurface.macos_desktop, .ios_device, .windows_desktop] {
                let claimed = ReflexContract.capability(surface, table: stale)
                XCTAssertFalse(claimed.live_reflex || claimed.instant_pointer, surface.rawValue)
            }
        }
        // Only the canonical bytes with every field known are read.
        let text = String(decoding: wire, as: UTF8.self)
        for bad in [" " + text,
                    text.replacingOccurrences(of: "{\"contract\"", with: "{\"a_claim\":true,\"contract\""),
                    text.replacingOccurrences(of: "\"instant_pointer\":false,", with: ""),
                    text.replacingOccurrences(of: "\"contract\":2,", with: "")] {
            XCTAssertThrowsError(try ReflexContract.decodeCapabilities(Data(bad.utf8)), bad)
        }
    }

    /// A run's policy is decoded as Rust decodes it, from the shared cases.
    func test_shared_run_policy_cases_run_through_the_real_decoder() throws {
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: self.fixture("run_policy_cases")) as? [String: Any])
        let cases = try XCTUnwrap(fixture["cases"] as? [[String: Any]])
        let limits = try contractLimits()
        var mismatches: [String] = []
        for row in cases {
            let name = try XCTUnwrap(row["name"] as? String)
            let wire = Data(try XCTUnwrap(row["wire"] as? String).utf8)
            let got: String
            do {
                let policy = try ReflexContract.decodeRunPolicy(wire, limits: limits)
                XCTAssertEqual(policy.version, ReflexContract.runPolicyVersion, name)
                got = "ok"
            } catch let error as ReflexContractError {
                got = error.rawValue
            }
            if got != (try XCTUnwrap(row["expected"] as? String)) { mismatches.append("\(name): got \(got)") }
        }
        XCTAssertEqual(mismatches, [])
        XCTAssertEqual(cases.count, 18)
    }

    func testAColourDetectorCarriesItsSpecInsideThePlanHash() throws {
        let (_, wire) = try golden("valid_basic")
        let limits = try contractLimits()
        let perception = try perceptionLimits()
        XCTAssertNoThrow(try ReflexContract.decodeAndValidate(wire, limits: limits, perception: perception))
        let text = String(decoding: wire, as: UTF8.self)
        // The spec is hashed: another confirm under the same digest is refused.
        let tampered = text.replacingOccurrences(of: "\"confirm\":1", with: "\"confirm\":2")
        XCTAssertNotEqual(tampered, text)
        XCTAssertThrowsError(try ReflexContract.decodeAndValidate(Data(tampered.utf8), limits: limits, perception: perception)) { error in
            XCTAssertEqual(error as? ReflexContractError, .hash)
        }
    }

    func testPointerAndMacroBudgetsComeFromOneTable() throws {
        let fromCore = try contractLimits()
        let envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: fixture("valid_basic")) as? [String: Any])
        let plan = try XCTUnwrap(envelope["plan"])
        let data = try JSONSerialization.data(withJSONObject: plan, options: [.sortedKeys, .withoutEscapingSlashes])
        XCTAssertNoThrow(try ReflexContract.decodeAndValidate(data, limits: fromCore, perception: try perceptionLimits()))
    }

    func testUnknownCannotTurnIntoPermission() throws {
        let raw = try XCTUnwrap(JSONSerialization.jsonObject(with: fixture("valid_basic")) as? [String: Any])
        let plan = try XCTUnwrap(raw["plan"])
        let data = try JSONSerialization.data(withJSONObject: plan, options: [.sortedKeys, .withoutEscapingSlashes])
        let validated = try ReflexContract.decodeAndValidate(data, limits: try contractLimits(), perception: try perceptionLimits())
        let predicate = try XCTUnwrap(validated.plan.rules.first?.predicate)
        XCTAssertEqual(predicate.evaluate(nil), .unknown)
    }

    func testNonCanonicalOrDuplicateWireCannotBeValidated() throws {
        let envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: fixture("valid_basic")) as? [String: Any])
        let plan = try XCTUnwrap(envelope["plan"])
        let canonical = try JSONSerialization.data(withJSONObject: plan, options: [.sortedKeys, .withoutEscapingSlashes])
        var spaced = Data(" ".utf8)
        spaced.append(canonical)
        XCTAssertThrowsError(try ReflexContract.decodeAndValidate(spaced, limits: try contractLimits(), perception: try perceptionLimits()))
        let duplicate = String(decoding: canonical, as: UTF8.self).replacingOccurrences(of: "\"version\":2", with: "\"version\":2,\"version\":2")
        XCTAssertThrowsError(try ReflexContract.decodeAndValidate(Data(duplicate.utf8), limits: try contractLimits(), perception: try perceptionLimits()))
    }

    func testLeaseAndFrameSequenceUseHostEpochs() throws {
        let limits = try contractLimits()
        let roi = ReflexRoi(x: 0, y: 0, width: 32, height: 32, space: .pixel)
        let frame = ReflexFrameFacts(run_id: "owned", display_id: "fixture", region: roi,
            pixel_extent: ReflexPixelExtent(width: 32, height: 32),
            point_transform: ReflexPointTransform(origin_x: 0, origin_y: 0, points_per_pixel: ReflexScale(numerator: 1, denominator: 1)),
            orientation: .up, color_space: .srgb, status: .ready, dirty: true,
            capture_gap: 0, delivered_host_ns: 91, capture_seq: 10, repaint_seq: 7,
            stream_epoch: 1, owner_epoch: 1, geometry_epoch: 1, plan_epoch: 1,
            clock_domain: 1, captured_host_ns: 90)
        let lease = ReflexActionLease(run_id: "owned", action_id: "click1", target_id: "ball",
            target_roi: roi, allowed_inputs: [.pointer_move], owner_epoch: 1,
            stream_epoch: 1, geometry_epoch: 1, plan_epoch: 1, clock_domain: 1,
            source_capture_seq: 9, issued_host_ns: 80, valid_until_host_ns: 120,
            target_proof_until_host_ns: 110, max_children: 2, used_children: 0)
        XCTAssertTrue(lease.permits(frame, now_host_ns: 100, input: .pointer_move, limits: limits))
        XCTAssertFalse(lease.permits(frame, now_host_ns: 110, input: .pointer_move, limits: limits))
        XCTAssertFalse(lease.permits(frame, now_host_ns: 100, input: .left_click, limits: limits))
        let changed = ReflexFrameFacts(run_id: "owned", display_id: "fixture", region: roi,
            pixel_extent: ReflexPixelExtent(width: 32, height: 32),
            point_transform: ReflexPointTransform(origin_x: 0, origin_y: 0, points_per_pixel: ReflexScale(numerator: 1, denominator: 1)),
            orientation: .up, color_space: .srgb, status: .ready, dirty: true,
            capture_gap: 0, delivered_host_ns: 91, capture_seq: 11, repaint_seq: 7,
            stream_epoch: 1, owner_epoch: 2, geometry_epoch: 1, plan_epoch: 1,
            clock_domain: 1, captured_host_ns: 90)
        XCTAssertFalse(lease.permits(changed, now_host_ns: 100, input: .pointer_move, limits: limits))
        var cursor = ReflexFrameCursor()
        XCTAssertTrue(cursor.observe(frame))
        XCTAssertFalse(cursor.observe(frame))
    }
}
