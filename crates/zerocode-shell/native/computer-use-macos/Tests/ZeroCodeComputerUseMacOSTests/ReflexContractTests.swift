import Foundation
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class ReflexContractTests: XCTestCase {
    private var fixtureRoot: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent() // test module
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // computer-use-macos
            .deletingLastPathComponent() // native
            .deletingLastPathComponent() // zerocode-shell
            .deletingLastPathComponent() // crates
            .appendingPathComponent("zerocode-core/fixtures/reflex-contract")
    }

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

    private func contractLimits() throws -> ReflexLimits {
        try JSONDecoder().decode(ReflexLimits.self, from: fixture("limits"))
    }

    func testSwiftAndRustReadTheSameVersionedPlan() throws {
        let manifest = try XCTUnwrap(JSONSerialization.jsonObject(with: fixture("manifest")) as? [String: Any])
        let names = try XCTUnwrap(manifest["semantic"] as? [String])
        let limits = try contractLimits()
        for name in names {
            let (expected, planData) = try golden(name)
            if expected == "ok" {
                let validated = try ReflexContract.decodeAndValidate(planData, limits: limits)
                XCTAssertEqual(validated.plan.version, ReflexContract.version)
                XCTAssertEqual(try ReflexContract.hash(validated.plan), validated.plan.plan_hash)
                XCTAssertEqual(try ReflexContract.wireBytes(validated.plan), planData)
            } else {
                XCTAssertThrowsError(try ReflexContract.decodeAndValidate(planData, limits: limits), name) { error in
                    XCTAssertEqual((error as? ReflexContractError)?.rawValue, expected, name)
                }
            }
        }
        XCTAssertEqual(names.count, 32)
        let wireNames = try XCTUnwrap(manifest["wire_negative"] as? [String])
        for name in wireNames {
            let data = try Data(contentsOf: fixtureRoot.appendingPathComponent("\(name).txt"))
            XCTAssertThrowsError(try ReflexContract.decodeAndValidate(data, limits: try contractLimits()), name) { error in
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

    func testPointerAndMacroBudgetsComeFromOneTable() throws {
        let fromCore = try contractLimits()
        let envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: fixture("valid_basic")) as? [String: Any])
        let plan = try XCTUnwrap(envelope["plan"])
        let data = try JSONSerialization.data(withJSONObject: plan, options: [.sortedKeys, .withoutEscapingSlashes])
        XCTAssertNoThrow(try ReflexContract.decodeAndValidate(data, limits: fromCore))
    }

    func testUnknownCannotTurnIntoPermission() throws {
        let raw = try XCTUnwrap(JSONSerialization.jsonObject(with: fixture("valid_basic")) as? [String: Any])
        let plan = try XCTUnwrap(raw["plan"])
        let data = try JSONSerialization.data(withJSONObject: plan, options: [.sortedKeys, .withoutEscapingSlashes])
        let validated = try ReflexContract.decodeAndValidate(data, limits: try contractLimits())
        let predicate = try XCTUnwrap(validated.plan.rules.first?.predicate)
        XCTAssertEqual(predicate.evaluate(nil), .unknown)
    }
    func testNonCanonicalOrDuplicateWireCannotBeValidated() throws {
        let envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: fixture("valid_basic")) as? [String: Any])
        let plan = try XCTUnwrap(envelope["plan"])
        let canonical = try JSONSerialization.data(withJSONObject: plan, options: [.sortedKeys, .withoutEscapingSlashes])
        var spaced = Data(" ".utf8)
        spaced.append(canonical)
        XCTAssertThrowsError(try ReflexContract.decodeAndValidate(spaced, limits: try contractLimits()))
        let duplicate = String(decoding: canonical, as: UTF8.self).replacingOccurrences(of: "\"version\":1", with: "\"version\":1,\"version\":1")
        XCTAssertThrowsError(try ReflexContract.decodeAndValidate(Data(duplicate.utf8), limits: try contractLimits()))
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
