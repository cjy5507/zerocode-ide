import CoreGraphics
import Foundation
import ImageIO
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
        XCTAssertEqual(cases.count, 68)
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

    // MARK: - The kernel

    private static let red: [UInt8] = [230, 40, 40]
    private static let green: [UInt8] = [40, 200, 70]
    private static let blue: [UInt8] = [40, 90, 230]
    private static let yellow: [UInt8] = [240, 200, 40]
    private static let black: [UInt8] = [0, 0, 0]
    private static let grey: [UInt8] = [128, 128, 128]
    private static let purple: [UInt8] = [150, 60, 200]

    private func colour(_ rgb: [UInt8], _ tolerance: UInt8) -> PerceptionColorClass {
        PerceptionColorClass(r: rgb[0], g: rgb[1], b: rgb[2], tolerance: tolerance)
    }

    /// A 2 × 2 board over a 64 × 64 frame: 32-pixel pitch, 24-pixel tiles with 4-pixel gaps on
    /// black, a 12-pixel sampled box, three samples a side, and the first red cell as its value.
    private func boardSpec(readout: PerceptionReadout = .first(class: 1), confirm: UInt64 = 1,
                           anchors: [PerceptionAnchor] = []) -> PerceptionColorSpec {
        PerceptionColorSpec(
            space: .srgb,
            classes: [colour(Self.red, 24), colour(Self.green, 24), colour(Self.blue, 24), colour(Self.yellow, 24)],
            ground: colour(Self.black, 12), reference_width: 64, reference_height: 64,
            layout: .cells(PerceptionCells(rows: 2, columns: 2, tile_width_permille: 750, tile_height_permille: 750,
                                           inset_permille: 250, lattice: 3, min_share_permille: 800, readout: readout)),
            confirm: confirm, anchors: anchors)
    }

    /// Red blobs over a 96 × 64 frame, sampled every 2 pixels, followed while they move at most 8
    /// pixels a capture.
    private func blobSpec(ground: Bool = true) -> PerceptionColorSpec {
        PerceptionColorSpec(
            space: .srgb, classes: [colour(Self.red, 24), colour(Self.green, 24)],
            ground: ground ? colour(Self.black, 12) : nil, reference_width: 96, reference_height: 64,
            layout: .blobs(PerceptionBlobs(class: 1, step: 2, min_samples: 2, gate: 8, max_blobs: 4)),
            confirm: 1, anchors: [])
    }

    private func wholeRoi(_ width: Int, _ height: Int) -> ReflexRoi {
        ReflexRoi(x: 0, y: 0, width: Int64(width), height: Int64(height), space: .pixel)
    }

    private func session(_ specs: [PerceptionColorSpec], limits: PerceptionLimits? = nil) throws -> PerceptionSession {
        try PerceptionSession(detectors: specs.enumerated().map { at, spec in
            ("d\(at)", wholeRoi(Int(spec.reference_width), Int(spec.reference_height)), spec)
        }, limits: limits ?? contractLimits())
    }

    /// The board painted cell by cell in reading order; a nil colour leaves the cell as ground.
    private func board(_ cells: [[UInt8]?], scale: Int = 1) -> Canvas {
        var canvas = Canvas(width: 64 * scale, height: 64 * scale, ground: Self.black)
        for (at, cell) in cells.enumerated() {
            guard let cell else { continue }
            canvas.fill((at % 2 * 32 + 4) * scale, (at / 2 * 32 + 4) * scale, 24 * scale, 24 * scale, cell)
        }
        return canvas
    }

    private func look(_ session: PerceptionSession, _ canvas: Canvas, capture: UInt64, stream: UInt64 = 1,
                      status: ReflexFrameStatus = .ready, colorSpace: ReflexColorSpace = .srgb,
                      orientation: ReflexOrientation = .up, budget: ReflexPerceptionBudget? = nil) -> [ReflexObservation] {
        var budget = budget ?? unlimited()
        let frame = frameFacts(capture: capture, width: canvas.width, height: canvas.height, stream: stream,
                               status: status, colorSpace: colorSpace, orientation: orientation)
        return canvas.withPixels { session.observe(frame: frame, pixels: $0, budget: &budget) }
    }

    func testUnknownOccludedOrAmbiguousFeaturesAreNotActionable() throws {
        let reader = try session([boardSpec()])
        // Clean: the first red cell is the second, and it is the target.
        let clean = look(reader, board([Self.blue, Self.red, Self.green, Self.red]), capture: 1)[0]
        XCTAssertNil(clean.unknown)
        XCTAssertEqual(clean.value, 2)
        XCTAssertEqual(clean.target.map { [$0.roi.x, $0.roi.y, $0.roi.width, $0.roi.height] }, [42, 10, 12, 12])
        XCTAssertEqual(clean.cells?.map { $0.class }, [3, 1, 2, 1])

        // Grey covers the first cell's middle and the gap beside it: that cell is occluded, and
        // the first red cannot be named — the covered cell could be it.
        var covered = board([Self.blue, Self.red, Self.green, Self.red])
        covered.fill(8, 8, 26, 16, Self.grey)
        let hidden = look(reader, covered, capture: 2)[0]
        XCTAssertEqual(hidden.cells?[0].class, 0)
        XCTAssertEqual(hidden.cells?[0].unknown, .occluded)
        XCTAssertNil(hidden.value)
        XCTAssertNil(hidden.target)
        XCTAssertEqual(hidden.unknown, .occluded)

        // Half the first cell red, half blue: ambiguous, not either colour.
        var split = board([Self.blue, Self.red, Self.green, Self.red])
        split.fill(4, 4, 12, 24, Self.red)
        let torn = look(reader, split, capture: 3)[0]
        XCTAssertEqual(torn.cells?[0].unknown, .ambiguous)
        XCTAssertNil(torn.value)
        XCTAssertNil(torn.target)

        // A popup in the palette's own red over the first cell and its gaps is not a red cell.
        var popup = board([Self.blue, Self.green, Self.green, Self.red])
        popup.fill(0, 0, 34, 34, Self.red)
        let camouflaged = look(reader, popup, capture: 4)[0]
        XCTAssertEqual(camouflaged.cells?[0].unknown, .occluded)
        XCTAssertNil(camouflaged.value)

        // A piece no class names is unknown, not the nearest class.
        let foreign = look(reader, board([Self.purple, Self.red, Self.green, Self.red]), capture: 5)[0]
        XCTAssertEqual(foreign.cells?[0].unknown, .occluded)
        XCTAssertNil(foreign.value)

        // A red cell after the unknown one is still not "first": the readout waits for every cell
        // before it. A red cell before it is known.
        let before = look(reader, board([Self.red, Self.purple, Self.green, Self.green]), capture: 6)[0]
        XCTAssertEqual(before.value, 1)

        // Frames the spec does not read: every detector unknown, nothing read.
        for (observation, reason) in [
            (look(reader, board([Self.red, nil, nil, nil]), capture: 7, colorSpace: .display_p3)[0], ReflexUnknown.color_space),
            (look(reader, board([Self.red, nil, nil, nil]), capture: 8, orientation: .right)[0], .orientation),
            (look(reader, board([Self.red, nil, nil, nil]), capture: 9, status: .stale)[0], .frame),
            // A capture no newer than the last one read.
            (look(reader, board([Self.red, nil, nil, nil]), capture: 6)[0], .frame),
        ] {
            XCTAssertEqual(observation.unknown, reason)
            XCTAssertNil(observation.value)
            XCTAssertNil(observation.target)
            XCTAssertEqual(observation.samples, 0)
            XCTAssertNil(observation.cells)
        }

        // An unknown value stays unknown through the plan's predicates, `not` included.
        let negated = try JSONDecoder().decode(ReflexPredicate.self, from: Data(#"{"child":{"op":"known"},"op":"not"}"#.utf8))
        XCTAssertEqual(negated.evaluate(hidden.value), .unknown)

        // Blobs: none seen is a known 0 only against ground with every sample explained.
        let unproven = try session([blobSpec(ground: false)])
        let empty = Canvas(width: 96, height: 64, ground: Self.black)
        XCTAssertEqual(look(unproven, empty, capture: 1)[0].unknown, .occluded)
        let proven = try session([blobSpec()])
        XCTAssertEqual(look(proven, empty, capture: 1)[0].value, 0)
        var smudged = empty
        smudged.fill(40, 20, 10, 10, Self.grey)
        let smudge = look(proven, smudged, capture: 2)[0]
        XCTAssertNil(smudge.value)
        XCTAssertEqual(smudge.unknown, .occluded)
    }

    func testTargetIdentitySurvivesMotionButNotSceneReplacement() throws {
        let follower = try session([blobSpec()])
        func ball(at x: Int, _ y: Int, and other: (Int, Int)? = nil) -> Canvas {
            var canvas = Canvas(width: 96, height: 64, ground: Self.black)
            canvas.fill(x, y, 6, 6, Self.red)
            if let other { canvas.fill(other.0, other.1, 6, 6, Self.red) }
            return canvas
        }
        let first = look(follower, ball(at: 10, 10), capture: 1)[0]
        let track = try XCTUnwrap(first.target?.track_id)
        XCTAssertEqual(first.value, 1)
        // Moved within the gate: the same target, with the motion measured.
        let moved = look(follower, ball(at: 14, 12), capture: 2)[0]
        XCTAssertEqual(moved.target?.track_id, track)
        XCTAssertGreaterThan(moved.target?.velocity_x ?? 0, 0)
        // Jumped further than it can move in one capture: a new target.
        let jumped = look(follower, ball(at: 70, 40), capture: 3)[0]
        let jumpedTrack = try XCTUnwrap(jumped.target?.track_id)
        XCTAssertNotEqual(jumpedTrack, track)
        // The same picture in a replaced scene (a new stream) is a new target, and no number returns.
        let replaced = look(follower, ball(at: 70, 40), capture: 1, stream: 2)[0]
        let replacedTrack = try XCTUnwrap(replaced.target?.track_id)
        XCTAssertNotEqual(replacedTrack, jumpedTrack)
        XCTAssertGreaterThan(replacedTrack, jumpedTrack)
        // Two look-alikes that both could be the followed one: neither keeps its number.
        let twins = look(follower, ball(at: 66, 38, and: (74, 42)), capture: 2, stream: 2)[0]
        XCTAssertEqual(twins.value, 2)
        XCTAssertNotEqual(twins.target?.track_id, replacedTrack)

        // A board cell keeps its number while the board stays; once the board is gone the next
        // one — even the same picture — gets new numbers.
        let reader = try session([boardSpec()])
        let cells = board([Self.blue, Self.red, Self.green, Self.red])
        let cell = try XCTUnwrap(look(reader, cells, capture: 1)[0].target?.track_id)
        XCTAssertEqual(look(reader, cells, capture: 2)[0].target?.track_id, cell)
        let gone = look(reader, Canvas(width: 64, height: 64, ground: Self.grey), capture: 3)[0]
        XCTAssertEqual(gone.unknown, .scene)
        let back = try XCTUnwrap(look(reader, cells, capture: 4)[0].target?.track_id)
        XCTAssertNotEqual(back, cell)
    }

    func testDetectorBudgetExhaustionReturnsUnknown() throws {
        let one = boardSpec()
        let small = PerceptionColorSpec(
            space: .srgb, classes: [colour(Self.red, 24)], ground: colour(Self.black, 12),
            reference_width: 64, reference_height: 64,
            layout: .cells(PerceptionCells(rows: 1, columns: 1, tile_width_permille: 750, tile_height_permille: 750,
                                           inset_permille: 250, lattice: 2, min_share_permille: 800,
                                           readout: .cell(index: 1))),
            confirm: 1, anchors: [])
        let reader = try session([one, one, small])
        let cells = board([Self.red, Self.red, Self.red, Self.red])
        // 2 × 2 cells of 9 samples and 12 gap probes: 84 a board; the small spec reads 12.
        var room = budget(samples: 100, deadlineHostNs: .max, now: { 0 })
        let seen = look(reader, cells, capture: 1, budget: room)
        XCTAssertEqual(seen.map(\.samples), [84, 0, 0])
        XCTAssertEqual(seen[0].value, 1)
        // The second board does not fit, and the small one after it is not read either, though it
        // would: nothing past an exhausted budget is read out of order.
        for observation in seen.dropFirst() {
            XCTAssertEqual(observation.unknown, .budget)
            XCTAssertNil(observation.value)
            XCTAssertNil(observation.target)
            XCTAssertNil(observation.cells)
        }
        // The budget paid exactly what was read.
        let frame = frameFacts(capture: 2, width: 64, height: 64)
        room = budget(samples: 100, deadlineHostNs: .max, now: { 0 })
        _ = cells.withPixels { reader.observe(frame: frame, pixels: $0, budget: &room) }
        XCTAssertEqual(room.samples, 16)
        // A deadline already passed reads nothing.
        let late = look(reader, cells, capture: 3, budget: budget(samples: .max, deadlineHostNs: 10, now: { 11 }))
        XCTAssertEqual(late.map(\.unknown), [.budget, .budget, .budget])
        // A spec whose own cost is over the table's limit never makes a session.
        let limits = try contractLimits()
        let greedy = PerceptionColorSpec(
            space: .srgb, classes: [colour(Self.red, 24)], ground: nil,
            reference_width: UInt32(limits.max_detector_samples), reference_height: 2,
            layout: .blobs(PerceptionBlobs(class: 1, step: 1, min_samples: 1, gate: 1, max_blobs: 1)),
            confirm: 1, anchors: [])
        XCTAssertThrowsError(try session([greedy]))
    }

    /// A tick with time left when a detector began reading and none when it finished: what that
    /// detector read is not an answer, and no detector after it reads.
    func testDeadlinePassedWhileReadingReturnsUnknown() throws {
        let limits = try contractLimits()
        let reader = try session([boardSpec(), boardSpec()])
        let cells = board([Self.red, Self.red, Self.red, Self.red])
        XCTAssertEqual(look(reader, cells, capture: 1).map(\.value), [1, 1])

        var tick = lateTick(limits)
        let frame = frameFacts(capture: 2, width: 64, height: 64)
        let late = cells.withPixels { reader.observe(frame: frame, pixels: $0, budget: &tick) }
        XCTAssertEqual(late.map(\.unknown), [.budget, .budget])
        for observation in late {
            XCTAssertNil(observation.value)
            XCTAssertNil(observation.target)
            XCTAssertNil(observation.cells)
        }
        // What the late reading cost is not hidden: its samples stand on its receipt, and the tick
        // paid them.
        XCTAssertEqual(late.map(\.samples), [84, 0])
        XCTAssertEqual(tick.samples, limits.max_tick_samples - 84)
    }

    /// A frame read past its tick's deadline is no evidence: it counts toward no confirmation, and
    /// no target is followed through it.
    func testFrameReadPastTheDeadlineIsNotEvidence() throws {
        let limits = try contractLimits()
        let careful = try session([boardSpec(confirm: 2)])
        let cells = board([Self.blue, Self.red, Self.green, Self.red])
        XCTAssertEqual(look(careful, cells, capture: 1, budget: lateTick(limits))[0].unknown, .budget)
        // Two fresh captures in time must agree; the late one is not the first of them.
        let after = look(careful, cells, capture: 2)[0]
        XCTAssertEqual(after.unknown, .unconfirmed)
        XCTAssertNil(after.value)
        XCTAssertNil(after.target)
        let confirmed = look(careful, cells, capture: 3)[0]
        XCTAssertEqual(confirmed.value, 2)
        XCTAssertNotNil(confirmed.target)

        // A ball seen, read late one step on, then seen one more step on: the late sighting links
        // nothing, so what follows is a new target with no motion measured across the late frame.
        let follower = try session([blobSpec()])
        func ball(at x: Int) -> Canvas {
            var canvas = Canvas(width: 96, height: 64, ground: Self.black)
            canvas.fill(x, 10, 6, 6, Self.red)
            return canvas
        }
        let seen = try XCTUnwrap(look(follower, ball(at: 10), capture: 1)[0].target?.track_id)
        XCTAssertEqual(look(follower, ball(at: 16), capture: 2, budget: lateTick(limits))[0].unknown, .budget)
        let next = look(follower, ball(at: 22), capture: 3)[0]
        XCTAssertEqual(next.value, 1)
        XCTAssertNotEqual(next.target?.track_id, seen)
        XCTAssertEqual(next.target?.velocity_x, 0)
    }

    func testReadoutsConfirmationsAnchorsAndScale() throws {
        let cells = board([Self.blue, Self.red, Self.green, Self.red])
        let count = try session([boardSpec(readout: .count(class: 1))])
        let counted = look(count, cells, capture: 1)[0]
        XCTAssertEqual(counted.value, 2)
        XCTAssertEqual(counted.target?.roi.x, 42)
        let single = try session([boardSpec(readout: .cell(index: 3))])
        XCTAssertEqual(look(single, cells, capture: 1)[0].value, 2)
        // None red: a known 0 with no target.
        let first = try session([boardSpec()])
        let none = look(first, board([Self.blue, Self.green, Self.green, Self.yellow]), capture: 1)[0]
        XCTAssertEqual(none.value, 0)
        XCTAssertNil(none.target)

        // Two fresh captures must agree before the value is known; a change starts over.
        let careful = try session([boardSpec(confirm: 2)])
        XCTAssertEqual(look(careful, cells, capture: 1)[0].unknown, .unconfirmed)
        XCTAssertEqual(look(careful, cells, capture: 2)[0].value, 2)
        XCTAssertEqual(look(careful, board([Self.red, Self.red, Self.red, Self.red]), capture: 3)[0].unknown, .unconfirmed)

        // An anchor that misses its colour: the frame is not this scene.
        let anchored = try session([boardSpec(anchors: [PerceptionAnchor(x: 1, y: 1, class: 0)])])
        XCTAssertEqual(look(anchored, cells, capture: 1)[0].value, 2)
        var marked = cells
        marked.fill(0, 0, 3, 3, Self.grey)
        let missed = look(anchored, marked, capture: 2)[0]
        XCTAssertEqual(missed.unknown, .scene)
        XCTAssertEqual(missed.samples, 1)

        // The same board at twice the size reads the same, with its target placed at that size.
        let twice = look(try session([boardSpec()]), board([Self.blue, Self.red, Self.green, Self.red], scale: 2), capture: 1)[0]
        XCTAssertEqual(twice.value, 2)
        XCTAssertEqual([twice.scale.numerator, twice.scale.denominator], [2, 1])
        XCTAssertEqual(twice.target.map { [$0.roi.x, $0.roi.y, $0.roi.width, $0.roi.height] }, [84, 20, 24, 24])
        // Stretched on one axis: not this layout.
        var stretched = Canvas(width: 64, height: 96, ground: Self.black)
        stretched.fill(4, 4, 24, 24, Self.red)
        XCTAssertEqual(look(try session([boardSpec()]), stretched, capture: 1)[0].unknown, .extent)
    }

    func testLearnedExamplesDoNotAuthorizeUnseenActions() throws {
        var canvas = Canvas(width: 64, height: 32, ground: Self.black)
        canvas.fill(0, 0, 16, 16, [228, 42, 38])
        canvas.fill(16, 0, 16, 16, [232, 38, 42])
        canvas.fill(32, 0, 16, 16, Self.blue)
        let candidate = try canvas.withPixels { pixels in
            try PerceptionLearner.propose([
                PerceptionExample(scene: "demo", split: .train, box: ReflexRoi(x: 0, y: 0, width: 16, height: 16, space: .pixel),
                                  label: "red", input: .left_click),
                PerceptionExample(scene: "demo", split: .train, box: ReflexRoi(x: 16, y: 0, width: 16, height: 16, space: .pixel),
                                  label: "red", input: .left_click),
                PerceptionExample(scene: "demo", split: .train, box: ReflexRoi(x: 32, y: 0, width: 16, height: 16, space: .pixel),
                                  label: nil, input: nil),
            ], planes: ["demo": PerceptionPlane(pixels)], limits: try contractLimits())
        }
        XCTAssertEqual(candidate.status, .candidate)
        XCTAssertEqual(candidate.classes, [PerceptionLearnedClass(label: "red", r: 230, g: 40, b: 40, tolerance: 2)])
        // Only the input the person made: no move, no key, nothing unseen.
        XCTAssertEqual(candidate.inputs, [.left_click])
        XCTAssertEqual(candidate.scenes, ["demo"])
        // A colour the demonstration never showed is no label — not the nearest one — and the
        // colour marked "none of these" is none.
        XCTAssertEqual(candidate.label(of: 0xFF_E6_28_28), "red")
        XCTAssertNil(candidate.label(of: 0xFF_F0_28_28))
        XCTAssertNil(candidate.label(of: 0xFF_28_C8_46))
        XCTAssertNil(candidate.label(of: 0xFF_28_5A_E6))
        // A box marked "none of these" in a learned colour refuses the candidate.
        XCTAssertThrowsError(try canvas.withPixels { pixels in
            try PerceptionLearner.propose([
                PerceptionExample(scene: "demo", split: .train, box: ReflexRoi(x: 0, y: 0, width: 16, height: 16, space: .pixel),
                                  label: "red", input: .left_click),
                PerceptionExample(scene: "demo", split: .train, box: ReflexRoi(x: 0, y: 0, width: 8, height: 8, space: .pixel),
                                  label: nil, input: nil),
            ], planes: ["demo": PerceptionPlane(pixels)], limits: try contractLimits())
        }) { XCTAssertEqual($0 as? PerceptionLearnError, .confusable) }
        // The document is the window's candidate shape, nothing more.
        let document = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(candidate)) as? [String: Any])
        XCTAssertEqual(Set(document.keys), ["status", "classes", "inputs", "scenes"])
        XCTAssertEqual(document["status"] as? String, "candidate")
    }

    func testHeldOutScenesAreNotTrainingOracleInputs() throws {
        let canvas = Canvas(width: 32, height: 32, ground: Self.red)
        let box = ReflexRoi(x: 0, y: 0, width: 8, height: 8, space: .pixel)
        // A held-out example is refused outright, even beside training ones.
        XCTAssertThrowsError(try canvas.withPixels { pixels in
            try PerceptionLearner.propose([
                PerceptionExample(scene: "train", split: .train, box: box, label: "red", input: .left_click),
                PerceptionExample(scene: "held", split: .heldout, box: box, label: "red", input: .left_click),
            ], planes: ["train": PerceptionPlane(pixels), "held": PerceptionPlane(pixels)], limits: try contractLimits())
        }) { XCTAssertEqual($0 as? PerceptionLearnError, .heldout) }
        // The committed held-out scenes are only ever scored: the kernel reads their pixels and
        // never their truth, which is looked at only after every observation is made.
        let scenes = try heldOutScenes()
        XCTAssertGreaterThanOrEqual(scenes.filter { $0.split == .heldout }.count, 20)
        let groups = Dictionary(grouping: scenes, by: \.group)
        XCTAssertTrue(groups.values.allSatisfy { Set($0.map { $0.split == .heldout }).count == 1 })
    }

    func testTemplateKernelFindsOnlyAnUnambiguousMatch() throws {
        var canvas = Canvas(width: 96, height: 64, ground: Self.black)
        var patch: [UInt32] = []
        for y in 0..<8 {
            for x in 0..<8 { patch.append((x + y) % 3 == 0 ? 0xFF_E6_28_28 : 0xFF_28_5A_E6) }
        }
        for y in 0..<8 {
            for x in 0..<8 where (x + y) % 3 == 0 { canvas.fill(20 + x, 12 + y, 1, 1, Self.red) }
            for x in 0..<8 where (x + y) % 3 != 0 { canvas.fill(20 + x, 12 + y, 1, 1, Self.blue) }
        }
        let template = try XCTUnwrap(PerceptionTemplate(width: 8, height: 8, pixels: patch))
        let search = PerceptionTemplateSearch(step: 1, stride: 1, accept: 8, margin: 16)
        let roi = wholeRoi(96, 64)
        var samples = UInt64.max
        XCTAssertEqual(canvas.withPixels { PerceptionTemplates.find(template, in: PerceptionPlane($0), roi: roi, search: search,
                                                                     samples: &samples) },
                       .found(x: 20, y: 12, score: 0))
        var twice = canvas
        for y in 0..<8 {
            for x in 0..<8 { twice.fill(60 + x, 40 + y, 1, 1, (x + y) % 3 == 0 ? Self.red : Self.blue) }
        }
        samples = .max
        if case .ambiguous = twice.withPixels({ PerceptionTemplates.find(template, in: PerceptionPlane($0), roi: roi,
                                                                          search: search, samples: &samples) }) {} else {
            XCTFail("two copies must be ambiguous")
        }
        samples = .max
        let empty = Canvas(width: 96, height: 64, ground: Self.black)
        if case .absent = empty.withPixels({ PerceptionTemplates.find(template, in: PerceptionPlane($0), roi: roi,
                                                                       search: search, samples: &samples) }) {} else {
            XCTFail("no copy must be absent")
        }
        samples = UInt64(PerceptionTemplates.cost(template, roi: roi, search: search) - 1)
        XCTAssertEqual(canvas.withPixels { PerceptionTemplates.find(template, in: PerceptionPlane($0), roi: roi,
                                                                     search: search, samples: &samples) }, .budget)
    }

    func testMotionKernelSeparatesMotionFromACut() throws {
        var motion = PerceptionMotion(step: 2, threshold: 24, cutPermille: 500)
        let roi = wholeRoi(96, 64)
        func see(_ canvas: Canvas) -> PerceptionMotion.Change {
            canvas.withPixels { motion.observe(PerceptionPlane($0), roi: roi, minSamples: 2) }
        }
        var canvas = Canvas(width: 96, height: 64, ground: Self.black)
        canvas.fill(10, 10, 6, 6, Self.red)
        let baseline = see(canvas), still = see(canvas)
        XCTAssertEqual(baseline, .baseline)
        XCTAssertEqual(still, .still)
        canvas.fill(10, 10, 6, 6, Self.black)
        canvas.fill(30, 30, 6, 6, Self.red)
        guard case .moved(let changes) = see(canvas) else { return XCTFail("a moved ball is motion") }
        // Where it left and where it arrived.
        XCTAssertEqual(changes.count, 2)
        let cut = see(Canvas(width: 96, height: 64, ground: Self.grey))
        XCTAssertEqual(cut, .cut)
    }

    /// Every committed scene, read from its pixels alone by a fresh session with the family's spec;
    /// its truth is looked at only after the observation is made. No scene may be read wrongly — a
    /// known value or cell that is not the truth — every clean scene must be read whole, and the
    /// scenes the spec cannot read must not be read at all.
    func testHeldOutScenesReadWithoutAWrongAction() throws {
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: fixture("scenes.json")) as? [String: Any])
        let spec = try decode(PerceptionColorSpec.self, fixture["spec"] as Any)
        let roi = try decode(ReflexRoi.self, fixture["roi"] as Any)
        let limits = try contractLimits()
        var wrongActions: [String] = [], wrongCells: [String] = [], unread: [String] = [], spoke: [String] = []
        var tally: [String: Int] = [:]
        for row in try XCTUnwrap(fixture["scenes"] as? [[String: Any]]) {
            let id = try XCTUnwrap(row["id"] as? String)
            let frame = try XCTUnwrap(row["frame"] as? [String: String])
            let canvas = try picture(fixtureRoot.appendingPathComponent("scenes/\(id).png"))
            let reader = try PerceptionSession(detectors: [("board", roi, spec)], limits: limits)
            let observation = look(reader, canvas, capture: 1,
                                   colorSpace: try XCTUnwrap(ReflexColorSpace(rawValue: frame["color_space"] ?? "")),
                                   orientation: try XCTUnwrap(ReflexOrientation(rawValue: frame["orientation"] ?? "")))[0]
            // Only now the truth.
            let truth = try XCTUnwrap(row["truth"] as? [String: Any])
            let cells = try XCTUnwrap(truth["cells"] as? [Int]), first = try XCTUnwrap(truth["first"] as? Int)
            let split = try XCTUnwrap(row["split"] as? String), expect = try XCTUnwrap(row["expect"] as? String)
            if let value = observation.value, value != Int64(first) { wrongActions.append(id) }
            for (at, cell) in (observation.cells ?? []).enumerated() where cell.class != 0 && Int(cell.class) != cells[at] {
                wrongCells.append("\(id)#\(at + 1)")
            }
            let whole = observation.value == Int64(first) && (observation.cells ?? []).map { Int($0.class) } == cells
            switch expect {
            case "read" where !whole:
                // Which cells fell short, and why: what the report quotes.
                let short = (observation.cells ?? []).enumerated().filter { $0.element.class == 0 }
                    .map { "\($0.offset + 1):\($0.element.unknown?.rawValue ?? "?")@\($0.element.share_permille)" }
                unread.append("\(id) \(short.joined(separator: " "))")
            case "abstain" where observation.value != nil: spoke.append(id)
            default: break
            }
            tally["\(split)/\(expect)", default: 0] += 1
            tally["\(split)/\(observation.value == nil ? "abstained" : "acted")", default: 0] += 1
        }
        let summary = try JSONSerialization.data(withJSONObject: [
            "tally": tally, "wrong_actions": wrongActions, "wrong_cells": wrongCells, "unread": unread, "spoke": spoke,
        ], options: [.sortedKeys])
        print("game-state held-out:", String(decoding: summary, as: UTF8.self))
        XCTAssertEqual(wrongActions, [])
        XCTAssertEqual(wrongCells, [])
        XCTAssertEqual(unread, [])
        XCTAssertEqual(spoke, [])
        XCTAssertGreaterThanOrEqual(tally["heldout/read", default: 0], 20)
    }

    /// A committed PNG drawn into an owned canvas in sRGB, the space it was written in, so its bytes
    /// come back unchanged.
    private func picture(_ url: URL) throws -> Canvas {
        let source = try XCTUnwrap(CGImageSourceCreateWithURL(url as CFURL, nil), url.lastPathComponent)
        let image = try XCTUnwrap(CGImageSourceCreateImageAtIndex(source, 0, nil), url.lastPathComponent)
        var canvas = Canvas(width: image.width, height: image.height, ground: Self.black)
        canvas.draw(image)
        return canvas
    }

    private struct LabelledScene {
        let id: String
        let group: String
        let split: PerceptionSplit
    }

    private func heldOutScenes() throws -> [LabelledScene] {
        let fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: fixture("scenes.json")) as? [String: Any])
        return try XCTUnwrap(fixture["scenes"] as? [[String: Any]]).map { row in
            LabelledScene(id: try XCTUnwrap(row["id"] as? String), group: try XCTUnwrap(row["group"] as? String),
                          split: try XCTUnwrap(PerceptionSplit(rawValue: try XCTUnwrap(row["split"] as? String))))
        }
    }
}

// MARK: - Owned pixels

/// An owned BGRA canvas whose rows run past its width, as capture buffers' rows do.
private struct Canvas {
    let width: Int
    let height: Int
    let bytesPerRow: Int
    private(set) var bytes: [UInt8]

    init(width: Int, height: Int, ground: [UInt8]) {
        self.width = width
        self.height = height
        bytesPerRow = width * 4 + 32
        bytes = [UInt8](repeating: 0, count: bytesPerRow * height)
        fill(0, 0, width, height, ground)
    }

    mutating func fill(_ x: Int, _ y: Int, _ w: Int, _ h: Int, _ rgb: [UInt8]) {
        for row in max(0, y)..<min(height, y + h) {
            for column in max(0, x)..<min(width, x + w) {
                let at = row * bytesPerRow + column * 4
                bytes[at] = rgb[2]
                bytes[at + 1] = rgb[1]
                bytes[at + 2] = rgb[0]
                bytes[at + 3] = 255
            }
        }
    }

    mutating func draw(_ image: CGImage) {
        let (width, height, bytesPerRow) = (self.width, self.height, self.bytesPerRow)
        bytes.withUnsafeMutableBytes { raw in
            let context = CGContext(data: raw.baseAddress, width: width, height: height, bitsPerComponent: 8,
                                    bytesPerRow: bytesPerRow, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                                    bitmapInfo: CGImageAlphaInfo.premultipliedFirst.rawValue |
                                        CGBitmapInfo.byteOrder32Little.rawValue)
            context?.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
        }
    }

    func withPixels<T>(_ body: (ReflexPixels) throws -> T) rethrows -> T {
        try bytes.withUnsafeBytes { raw in
            try body(ReflexPixels(base: raw.baseAddress!, width: width, height: height, bytesPerRow: bytesPerRow))
        }
    }
}

/// The frame facts an owned canvas stands for: one run, one clock, captured every 60th of a second.
private func frameFacts(capture: UInt64, width: Int, height: Int, stream: UInt64 = 1, status: ReflexFrameStatus = .ready,
                colorSpace: ReflexColorSpace = .srgb, orientation: ReflexOrientation = .up) -> ReflexFrameFacts {
    let captured = capture * 16_666_667
    return ReflexFrameFacts(
        run_id: "owned", display_id: "fixture",
        region: ReflexRoi(x: 0, y: 0, width: Int64(width), height: Int64(height), space: .pixel),
        pixel_extent: ReflexPixelExtent(width: UInt32(width), height: UInt32(height)),
        point_transform: ReflexPointTransform(origin_x: 0, origin_y: 0,
                                              points_per_pixel: ReflexScale(numerator: 1, denominator: 1)),
        orientation: orientation, color_space: colorSpace, status: status, dirty: true, capture_gap: 0,
        delivered_host_ns: captured + 1, capture_seq: capture, repaint_seq: capture, stream_epoch: stream,
        owner_epoch: 1, geometry_epoch: 1, plan_epoch: 1, clock_domain: 1, captured_host_ns: captured)
}

private func budget(samples: UInt64, deadlineHostNs: UInt64, now: @escaping @Sendable () -> UInt64) -> ReflexPerceptionBudget {
    ReflexPerceptionBudget(samples: samples, deadlineHostNs: deadlineHostNs, now: now)
}

private func unlimited() -> ReflexPerceptionBudget {
    budget(samples: .max, deadlineHostNs: .max, now: { 0 })
}

/// A tick of the table's length on a clock that reads its start once — the check a detector passes
/// before it reads — and its deadline ever after: the tick runs out while that detector reads.
private func lateTick(_ limits: PerceptionLimits) -> ReflexPerceptionBudget {
    let clock = LateClock(late: limits.max_tick_ns)
    return budget(samples: limits.max_tick_samples, deadlineHostNs: limits.max_tick_ns, now: { clock.now() })
}

private final class LateClock: @unchecked Sendable {
    private let lock = NSLock()
    private let late: UInt64
    private var read = false

    init(late: UInt64) { self.late = late }

    func now() -> UInt64 {
        lock.lock()
        defer { lock.unlock() }
        let now = read ? late : 0
        read = true
        return now
    }
}
