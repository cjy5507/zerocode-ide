import Foundation
import XCTest
@testable import ZeroCodeComputerUseMacOSCore

final class PerceptionTests: XCTestCase {
    private var fixtureRoot: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent() // test module
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // computer-use-macos
            .deletingLastPathComponent() // native
            .deletingLastPathComponent() // zerocode-shell
            .deletingLastPathComponent() // crates
            .appendingPathComponent("zerocode-core/fixtures/game-state")
    }

    private func fixture(_ name: String) throws -> Data {
        try Data(contentsOf: fixtureRoot.appendingPathComponent(name))
    }

    private func contractLimits() throws -> PerceptionLimits {
        try PerceptionSpecs.decodeLimits(Data(try fixture("limits.json").dropLast()))
    }

    private func specCases() throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: fixture("spec_cases.json")) as? [String: Any])
    }

    /// The shared base spec with a case's top-level fields replaced; a null removes the field.
    private func patched(_ base: [String: Any], _ patch: [String: Any]) -> [String: Any] {
        var spec = base
        for (key, value) in patch {
            spec[key] = value is NSNull ? nil : value
        }
        return spec
    }

    private func decode<T: Decodable>(_ type: T.Type, _ object: Any) throws -> T {
        try JSONDecoder().decode(type, from: JSONSerialization.data(withJSONObject: object))
    }

    func testSharedSpecCasesRunThroughTheRealValidator() throws {
        let fixture = try specCases()
        let base = try XCTUnwrap(fixture["spec"] as? [String: Any])
        let cases = try XCTUnwrap(fixture["cases"] as? [[String: Any]])
        let limits = try contractLimits()
        var mismatches: [String] = []
        for row in cases {
            let name = try XCTUnwrap(row["name"] as? String)
            let roi = try decode(ReflexRoi.self, row["roi"] ?? fixture["roi"] as Any)
            let patch = try XCTUnwrap(row["patch"] as? [String: Any])
            var got: String
            var cost: UInt64?
            if let spec = try? decode(PerceptionColorSpec.self, patched(base, patch)) {
                do {
                    cost = try PerceptionSpecs.validate(spec, roi: roi, limits: limits)
                    got = "ok"
                } catch let error as PerceptionSpecError {
                    got = error.rawValue
                }
            } else {
                got = "wire"
            }
            if got != row["expected"] as? String || cost != (row["cost"] as? NSNumber)?.uint64Value {
                mismatches.append("\(name): got \(got) \(String(describing: cost))")
            }
        }
        XCTAssertEqual(mismatches, [])
        XCTAssertEqual(cases.count, 61)
    }

    func testFrameRoiFollowsOnlyAUniformRescale() throws {
        let fixture = try specCases()
        let base = try XCTUnwrap(fixture["spec"] as? [String: Any])
        let cases = try XCTUnwrap(fixture["frame_roi"] as? [[String: Any]])
        for row in cases {
            let name = try XCTUnwrap(row["name"] as? String)
            let reference = try XCTUnwrap(row["reference"] as? [String: Any])
            let spec = try decode(PerceptionColorSpec.self, patched(base, [
                "reference_width": reference["width"] as Any, "reference_height": reference["height"] as Any,
            ]))
            let roi = try decode(ReflexRoi.self, row["roi"] as Any)
            let frame = try decode(ReflexPixelExtent.self, row["frame"] as Any)
            let placed = PerceptionSpecs.frameRoi(roi, spec: spec, frame: frame)
            guard let expected = row["expected"] as? [String: Any] else {
                XCTAssertNil(placed, name)
                continue
            }
            let placedRoi = try XCTUnwrap(placed?.roi, name)
            let scale = try XCTUnwrap(placed?.scale, name)
            let roiWant = try XCTUnwrap(expected["roi"] as? [String: Any])
            let scaleWant = try XCTUnwrap(expected["scale"] as? [String: Any])
            XCTAssertEqual([placedRoi.x, placedRoi.y, placedRoi.width, placedRoi.height],
                           ["x", "y", "width", "height"].map { (roiWant[$0] as? NSNumber)?.int64Value ?? -1 }, name)
            XCTAssertEqual(placedRoi.space, .pixel, name)
            XCTAssertEqual([scale.numerator, scale.denominator],
                           ["numerator", "denominator"].map { (scaleWant[$0] as? NSNumber)?.uint64Value ?? 0 }, name)
        }
        XCTAssertEqual(cases.count, 9)
    }

    func testPerceptionLimitsComeFromOneTable() throws {
        let wire = try fixture("limits.json")
        XCTAssertEqual(wire.last, UInt8(ascii: "\n"))
        let limits = try PerceptionSpecs.decodeLimits(Data(wire.dropLast()))
        XCTAssertEqual(try JSONSerialization.data(withJSONObject: JSONSerialization.jsonObject(with: JSONEncoder().encode(limits)),
                                                  options: [.sortedKeys, .withoutEscapingSlashes]),
                       Data(wire.dropLast()))
        // A field the helper does not know is refused, not dropped.
        var extra = String(decoding: wire.dropLast(), as: UTF8.self)
        extra.removeLast()
        XCTAssertThrowsError(try PerceptionSpecs.decodeLimits(Data((extra + ",\"zz\":1}").utf8)))
    }
}
